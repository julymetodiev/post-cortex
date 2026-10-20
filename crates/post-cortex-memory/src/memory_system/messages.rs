// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The raw message stream (ROADMAP Phase 2 item 1): appends from client
//! hooks and retention. Messages bypass `ActiveSession` entirely; they are
//! stored through the storage actor and embedded as `Message` vectors.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use tracing::{info, warn};
use uuid::Uuid;

use post_cortex_core::core::message::{self, Message, NewMessage};

use super::system::ConversationMemorySystem;

/// Messages embedded per call when a session's stream is backfilled.
#[cfg(feature = "embeddings")]
const MESSAGE_EMBED_BATCH: usize = 256;
#[cfg(feature = "embeddings")]
use crate::content_vectorizer::{ContentType, SemanticSearchResult};

impl ConversationMemorySystem {
    /// Append captured turns to `session_id`'s message stream and embed the
    /// ones stored. Turns already captured are skipped. Returns the stored
    /// messages and how many vectors were written; embedding failures are
    /// logged, not returned, since the turns themselves are stored.
    ///
    /// # Errors
    /// The session does not exist, or the storage write failed.
    pub async fn append_messages(
        &self,
        session_id: Uuid,
        messages: Vec<NewMessage>,
    ) -> Result<(Vec<Message>, usize), String> {
        // Fails for an unknown session, so a stream never outlives (or
        // precedes) its session.
        self.get_session(session_id).await?;
        let stored = self
            .storage_actor
            .append_messages(session_id, messages)
            .await?;
        let vectorized = self.vectorize_messages(&stored).await;
        Ok((stored, vectorized))
    }

    /// Delete every message timestamped before `cutoff`, with its vectors.
    /// Returns how many messages were deleted.
    ///
    /// # Errors
    /// The storage delete failed.
    pub async fn expire_messages(&self, cutoff: DateTime<Utc>) -> Result<usize, String> {
        let deleted = self.storage_actor.delete_messages_before(cutoff).await?;
        let mut by_session: HashMap<Uuid, Vec<String>> = HashMap::new();
        for (session_id, seq) in &deleted {
            by_session
                .entry(*session_id)
                .or_default()
                .push(message::content_id(*seq));
        }
        for (session_id, content_ids) in by_session {
            self.drop_content_vectors(session_id, &content_ids).await;
        }
        if !deleted.is_empty() {
            info!("Expired {} messages older than {}", deleted.len(), cutoff);
        }
        Ok(deleted.len())
    }

    /// Complete the message hits of a search: each comes back with its
    /// turn's whole text (a hit may be one chunk of a long turn) and is
    /// followed by up to `neighbour_radius` turns on each side of it in its
    /// stream, read from storage. Results that are not messages pass
    /// through. A turn already returned is not repeated; one that is also a
    /// hit keeps its own scores. Neighbours carry their hit's
    /// `combined_score`, a `similarity_score` of 0 and `neighbour_of`. If a
    /// stream cannot be read, its hits stay as they are.
    #[cfg(feature = "embeddings")]
    pub(crate) async fn complete_message_results(
        &self,
        results: Vec<SemanticSearchResult>,
    ) -> Vec<SemanticSearchResult> {
        let seq_of = |r: &SemanticSearchResult| {
            (r.content_type == ContentType::Message)
                .then(|| message::seq_of_content_id(&r.content_id))
                .flatten()
        };
        if !results.iter().any(|r| seq_of(r).is_some()) {
            return results;
        }
        let radius = self.config.neighbour_radius as u64;
        let hits: HashMap<(Uuid, String), usize> = results
            .iter()
            .enumerate()
            .map(|(i, r)| ((r.session_id, r.content_id.clone()), i))
            .collect();
        let mut seen: HashSet<(Uuid, String)> = HashSet::new();
        let mut out = Vec::with_capacity(results.len());
        for hit in &results {
            let Some(seq) = seq_of(hit) else {
                if seen.insert((hit.session_id, hit.content_id.clone())) {
                    out.push(hit.clone());
                }
                continue;
            };
            let window = seq.saturating_sub(radius)..seq.saturating_add(radius + 1);
            let turns = match self
                .storage_actor
                .load_messages(hit.session_id, window)
                .await
            {
                Ok(turns) => turns,
                Err(e) => {
                    warn!(
                        "Messages around {} of session {} not loaded: {}",
                        hit.content_id, hit.session_id, e
                    );
                    Vec::new()
                }
            };
            // The hit's own turn unreadable (or already expired): keep the hit.
            if !turns.iter().any(|t| t.seq == seq)
                && seen.insert((hit.session_id, hit.content_id.clone()))
            {
                out.push(hit.clone());
            }
            for turn in &turns {
                let id = message::content_id(turn.seq);
                if !seen.insert((hit.session_id, id.clone())) {
                    continue;
                }
                let mut item = match hits.get(&(hit.session_id, id)) {
                    Some(&i) => results[i].clone(),
                    None => message_neighbour(turn, hit),
                };
                item.text_content = message_text(turn);
                out.push(item);
            }
        }
        out
    }

    /// The captured turns of `sessions` that best match `query`, each with
    /// the turns around it, as context excerpts in conversation order
    /// (`[date] role: text`), at most `limit` matches. A fast search: no
    /// reranking.
    ///
    /// # Errors
    /// The search failed.
    #[cfg(feature = "embeddings")]
    pub async fn conversation_excerpts(
        &self,
        sessions: &[Uuid],
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::context_assembly::ContextItem>, String> {
        let options = crate::content_vectorizer::SearchOptions {
            limit: Some(limit),
            mode: Some(crate::content_vectorizer::SearchMode::Fast),
            corpus: post_cortex_embeddings::Corpus::Messages,
            ..Default::default()
        };
        let results = match sessions {
            [] => return Ok(Vec::new()),
            [session_id] => {
                self.semantic_search_session(*session_id, query, options)
                    .await?
            }
            sessions => {
                self.semantic_search_multisession(sessions, query, options)
                    .await?
            }
        };
        // Group each match with its neighbours, in stream order.
        let mut turns: Vec<_> = results
            .into_iter()
            .filter(|r| r.content_type == ContentType::Message)
            .collect();
        turns.sort_by_key(|r| {
            (
                r.session_id,
                message::seq_of_content_id(&r.content_id).unwrap_or(u64::MAX),
            )
        });
        Ok(turns
            .into_iter()
            .map(|r| {
                crate::context_assembly::excerpt(
                    format!(
                        "[{}] {}",
                        r.timestamp.format("%Y-%m-%d %H:%M"),
                        r.text_content
                    ),
                    r.combined_score.clamp(0.0, 1.0),
                    format!("{}/{}", r.session_id, r.content_id),
                )
            })
            .collect())
    }

    /// Embed the captured messages of `session_id` that have no vectors
    /// yet. Returns how many vectors were added.
    ///
    /// # Errors
    /// The stream could not be read or the vectorizer not started.
    #[cfg(feature = "embeddings")]
    pub(crate) async fn vectorize_session_messages(
        &self,
        session_id: Uuid,
    ) -> Result<usize, String> {
        let messages = self
            .storage_actor
            .load_messages(session_id, 0..u64::MAX)
            .await?;
        if messages.is_empty() {
            return Ok(0);
        }
        let vectorizer = self.ensure_vectorizer_initialized().await?;
        let mut added = 0;
        for batch in messages.chunks(MESSAGE_EMBED_BATCH) {
            added += vectorizer
                .vectorize_messages(batch)
                .await
                .map_err(|e| format!("Embedding messages of session {session_id} failed: {e:#}"))?;
        }
        if added > 0 {
            info!(
                "Embedded {} message vectors of session {}",
                added, session_id
            );
        }
        Ok(added)
    }

    #[cfg(feature = "embeddings")]
    async fn vectorize_messages(&self, messages: &[Message]) -> usize {
        if messages.is_empty() || !self.config.enable_embeddings {
            return 0;
        }
        let vectorizer = match self.ensure_vectorizer_initialized().await {
            Ok(vectorizer) => vectorizer,
            Err(e) => {
                warn!("Messages stored but not embedded: {e}");
                return 0;
            }
        };
        match vectorizer.vectorize_messages(messages).await {
            Ok(added) => added,
            Err(e) => {
                warn!("Messages stored but not embedded: {e:#}");
                0
            }
        }
    }

    #[cfg(not(feature = "embeddings"))]
    async fn vectorize_messages(&self, _messages: &[Message]) -> usize {
        0
    }
}

/// A turn as search returns it: who said it, then what.
fn message_text(message: &Message) -> String {
    format!("{}: {}", message.role.as_str(), message.text)
}

#[cfg(feature = "embeddings")]
fn message_neighbour(message: &Message, hit: &SemanticSearchResult) -> SemanticSearchResult {
    SemanticSearchResult {
        content_id: message::content_id(message.seq),
        session_id: message.session_id,
        content_type: ContentType::Message,
        text_content: message_text(message),
        similarity_score: 0.0,
        importance_score: ContentType::Message.importance_weight(),
        timestamp: message.timestamp,
        combined_score: hit.combined_score,
        neighbour_of: Some(hit.content_id.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_system::SystemConfig;
    use chrono::Duration;
    use post_cortex_core::core::message::MessageRole;
    use post_cortex_embeddings::{MESSAGE_CONTENT_TYPE, VectorMetadata};

    async fn system() -> (ConversationMemorySystem, tempfile::TempDir) {
        let dir = tempfile::Builder::new()
            .prefix("pcx_messages_")
            .tempdir()
            .unwrap();
        let config = SystemConfig {
            data_directory: dir.path().to_string_lossy().into_owned(),
            enable_embeddings: false,
            ..Default::default()
        };
        (ConversationMemorySystem::new(config).await.unwrap(), dir)
    }

    fn turn(turn_id: &str, age_days: i64) -> NewMessage {
        NewMessage {
            turn_id: turn_id.to_string(),
            role: MessageRole::User,
            author: None,
            source_session: None,
            timestamp: Utc::now() - Duration::days(age_days),
            text: format!("turn {turn_id}"),
            tools: Vec::new(),
            files: Vec::new(),
        }
    }

    #[tokio::test]
    async fn append_needs_a_session_and_skips_known_turns() {
        let (sys, _dir) = system().await;
        assert!(
            sys.append_messages(Uuid::new_v4(), vec![turn("t1", 0)])
                .await
                .is_err()
        );

        let sid = sys.create_session(None, None).await.unwrap();
        let (first, vectorized) = sys
            .append_messages(sid, vec![turn("t1", 0), turn("t2", 0)])
            .await
            .unwrap();
        assert_eq!(first.iter().map(|m| m.seq).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(vectorized, 0, "embeddings are off");
        let (second, _) = sys
            .append_messages(sid, vec![turn("t2", 0), turn("t3", 0)])
            .await
            .unwrap();
        assert_eq!(second.iter().map(|m| m.seq).collect::<Vec<_>>(), [2]);
        assert!(
            sys.get_session(sid)
                .await
                .unwrap()
                .load()
                .incremental_updates
                .is_empty(),
            "messages never enter the session"
        );
    }

    #[cfg(feature = "embeddings")]
    #[tokio::test]
    async fn message_hits_come_with_whole_text_and_adjacent_turns() {
        use crate::content_vectorizer::{ContentType, SemanticSearchResult};

        let (sys, _dir) = system().await;
        let sid = sys.create_session(None, None).await.unwrap();
        let turns: Vec<NewMessage> = (0..5).map(|i| turn(&format!("t{i}"), 0)).collect();
        sys.append_messages(sid, turns).await.unwrap();

        let result =
            |content_id: &str, content_type: ContentType, score: f32| SemanticSearchResult {
                content_id: content_id.to_string(),
                session_id: sid,
                content_type,
                text_content: "chunk".to_string(),
                similarity_score: score,
                importance_score: 0.4,
                timestamp: Utc::now(),
                combined_score: score,
                neighbour_of: None,
            };
        let hits = vec![
            result("message:2", ContentType::Message, 0.9),
            result("some-update", ContentType::DecisionPoint, 0.8),
            // Also a neighbour of message:2; keeps its own score.
            result("message:3", ContentType::Message, 0.5),
        ];
        let out = sys.complete_message_results(hits).await;
        let ids: Vec<&str> = out.iter().map(|r| r.content_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "message:1",
                "message:2",
                "message:3",
                "some-update",
                "message:4"
            ]
        );
        assert_eq!(
            out[1].text_content, "user: turn t2",
            "whole turn, not the chunk"
        );
        assert_eq!(out[0].neighbour_of.as_deref(), Some("message:2"));
        assert_eq!(out[0].similarity_score, 0.0);
        assert_eq!(out[2].similarity_score, 0.5, "a hit keeps its own score");
        assert_eq!(out[2].neighbour_of, None);
        assert_eq!(out[3].text_content, "chunk", "records pass through");
        assert_eq!(out[4].neighbour_of.as_deref(), Some("message:3"));
    }

    #[tokio::test]
    async fn expiry_deletes_old_messages_and_their_vectors() {
        let (sys, _dir) = system().await;
        let sid = sys.create_session(None, None).await.unwrap();
        sys.append_messages(sid, vec![turn("old", 120), turn("new", 1)])
            .await
            .unwrap();
        for seq in [0, 1] {
            let metadata = VectorMetadata::new(
                message::content_id(seq),
                "text".to_string(),
                sid.to_string(),
                MESSAGE_CONTENT_TYPE.to_string(),
            );
            sys.vector_storage
                .add_vector(vec![0.5; sys.config.vector_dimension], metadata)
                .await
                .unwrap();
        }

        let expired = sys
            .expire_messages(Utc::now() - Duration::days(90))
            .await
            .unwrap();
        assert_eq!(expired, 1);
        let left = sys.storage_actor.load_messages(sid, 0..10).await.unwrap();
        assert_eq!(left.iter().map(|m| m.seq).collect::<Vec<_>>(), [1]);
        assert_eq!(
            sys.vector_storage
                .count_session_embeddings(&sid.to_string())
                .await,
            1,
            "the expired message's vector is gone, the other stays"
        );
    }
}
