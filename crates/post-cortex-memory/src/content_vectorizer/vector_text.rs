// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a record, a message or an entity becomes as vectors: its text, its
//! chunks and their metadata.

use crate::content_vectorizer::types::{
    AUTHOR_KEY, CHUNK_PARENT_KEY, ContentKind, DOCUMENT_KIND_KEY, MESSAGE_ROLE_KEY, MESSAGE_SEQ_KEY,
};
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::core::document::{self, Document};
use post_cortex_core::core::message::{self, Message};
use post_cortex_core::graph::retrieval_graph;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::{MESSAGE_CONTENT_TYPE, VectorMetadata};
use uuid::Uuid;

/// Build a comprehensive natural-language description of an entity for vectorization.
pub fn build_entity_description(
    session: &ActiveSession,
    entity_name: &str,
    entity_data: &post_cortex_core::core::context_update::EntityData,
) -> String {
    let mut description_parts = Vec::new();
    description_parts.push(entity_name.to_string());
    description_parts.push(format!("Type: {:?}", entity_data.entity_type));

    let related: Vec<String> = session
        .entity_graph()
        .get_entity_relationships(entity_name)
        .into_iter()
        .take(5)
        .map(|(target, rel, _)| format!("{} {:?}", target, rel))
        .collect();
    if !related.is_empty() {
        description_parts.push(format!("Related to: {}", related.join(", ")));
    }

    if !entity_data.aliases.is_empty() {
        description_parts.push(format!("Also: {}", entity_data.aliases.join(", ")));
    }

    // No mention count: it changes with every write, so the vector would be
    // re-embedded (and rewritten, racing concurrent writes) on each one.
    description_parts.join(". ")
}

/// Characters shared by consecutive chunks of a long update, so a passage cut
/// at a chunk boundary is whole in one of them.
pub const CHUNK_OVERLAP: usize = 200;

/// The vectors for an update: its text and metadata, one per chunk. Text up
/// to `chunk_size` characters is one vector with the update's id; longer
/// text is split into overlapping chunks of about `chunk_size` characters
/// (the rerankers see about that much), the first keeping the update's id,
/// the others `<id>#<n>` with [`CHUNK_PARENT_KEY`] naming the update. The
/// first also carries the retrieval-graph terms of the whole update and the
/// names of the entities the agent attached to it (see
/// `post_cortex_core::graph::retrieval_graph`).
pub fn update_vectors(
    session_id: Uuid,
    update: &ContextUpdate,
    raw_text: &str,
    chunk_size: usize,
) -> Vec<(String, VectorMetadata)> {
    let content_type = determine_content_type(update).as_str().to_string();
    let entities = update
        .typed_entities
        .iter()
        .map(|e| e.name.as_str())
        .chain(update.creates_entities.iter().map(String::as_str))
        .chain(update.references_entities.iter().map(String::as_str));
    let terms = retrieval_graph::terms(raw_text);
    chunk_text(raw_text, chunk_size, CHUNK_OVERLAP)
        .into_iter()
        .enumerate()
        .map(|(n, chunk)| {
            let id = match n {
                0 => update.id.to_string(),
                n => format!("{}#{}", update.id, n + 1),
            };
            let mut metadata = VectorMetadata::new(
                id,
                chunk.to_string(),
                session_id.to_string(),
                content_type.clone(),
            )
            .with_timestamp(update.timestamp);
            if let Some(author) = &update.author {
                metadata = metadata.with_metadata(AUTHOR_KEY.to_string(), author.clone());
            }
            if n == 0 {
                metadata = metadata
                    .with_metadata(
                        retrieval_graph::TERMS_KEY.to_string(),
                        retrieval_graph::join_keys(terms.iter().map(String::as_str)),
                    )
                    .with_metadata(
                        retrieval_graph::ENTITIES_KEY.to_string(),
                        retrieval_graph::join_keys(entities.clone()),
                    );
            } else {
                metadata =
                    metadata.with_metadata(CHUNK_PARENT_KEY.to_string(), update.id.to_string());
            }
            (chunk.to_string(), metadata)
        })
        .collect()
}

/// The vectors of one message: its text chunked like an update's (see
/// [`update_vectors`]), content ids `message:<seq>` and `message:<seq>#<n>`,
/// with the role, author and position in the metadata.
pub fn message_vectors(message: &Message, chunk_size: usize) -> Vec<(String, VectorMetadata)> {
    let parent = message::content_id(message.seq);
    chunk_text(&message.text, chunk_size, CHUNK_OVERLAP)
        .into_iter()
        .enumerate()
        .map(|(n, chunk)| {
            let id = match n {
                0 => parent.clone(),
                n => format!("{}#{}", parent, n + 1),
            };
            let mut metadata = VectorMetadata::new(
                id,
                chunk.to_string(),
                message.session_id.to_string(),
                MESSAGE_CONTENT_TYPE.to_string(),
            )
            .with_timestamp(message.timestamp)
            .with_metadata(
                MESSAGE_ROLE_KEY.to_string(),
                message.role.as_str().to_string(),
            )
            .with_metadata(MESSAGE_SEQ_KEY.to_string(), message.seq.to_string());
            if let Some(author) = &message.author {
                metadata = metadata.with_metadata(AUTHOR_KEY.to_string(), author.clone());
            }
            if n > 0 {
                metadata = metadata.with_metadata(CHUNK_PARENT_KEY.to_string(), parent.clone());
            }
            (chunk.to_string(), metadata)
        })
        .collect()
}

/// The vectors of a document's version: its title and text chunked like an
/// update's (see [`update_vectors`]), content ids `doc:<id>` and
/// `doc:<id>#<n>`, with its workspace as the source and its kind and author
/// in the metadata.
pub fn document_vectors(doc: &Document, chunk_size: usize) -> Vec<(String, VectorMetadata)> {
    let parent = document::content_id(doc.id);
    let text = if doc.text.is_empty() {
        doc.title.clone()
    } else {
        format!("{}\n\n{}", doc.title, doc.text)
    };
    chunk_text(&text, chunk_size, CHUNK_OVERLAP)
        .into_iter()
        .enumerate()
        .map(|(n, chunk)| {
            let id = match n {
                0 => parent.clone(),
                n => format!("{}#{}", parent, n + 1),
            };
            let mut metadata = VectorMetadata::new(
                id,
                chunk.to_string(),
                doc.workspace_id.to_string(),
                ContentKind::Document.as_str().to_string(),
            )
            .with_timestamp(doc.updated_at)
            .with_metadata(DOCUMENT_KIND_KEY.to_string(), doc.kind.to_string());
            if let Some(author) = &doc.author {
                metadata = metadata.with_metadata(AUTHOR_KEY.to_string(), author.clone());
            }
            if n > 0 {
                metadata = metadata.with_metadata(CHUNK_PARENT_KEY.to_string(), parent.clone());
            }
            (chunk.to_string(), metadata)
        })
        .collect()
}

/// `text` as windows of at most `size` characters, consecutive ones sharing
/// about `overlap` characters, cut at whitespace where there is any in the
/// second half of a window. Text up to `size` characters is one window.
pub fn chunk_text(text: &str, size: usize, overlap: usize) -> Vec<&str> {
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let chars = starts.len();
    if chars <= size || size == 0 {
        return vec![text];
    }
    let byte = |c: usize| starts.get(c).copied().unwrap_or(text.len());
    let is_space = |c: usize| text[byte(c)..].starts_with(char::is_whitespace);
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let mut end = (start + size).min(chars);
        if end < chars {
            // Back off to the last whitespace in the second half.
            if let Some(cut) = (start + size / 2..end).rev().find(|&c| is_space(c)) {
                end = cut;
            }
        }
        chunks.push(text[byte(start)..byte(end)].trim());
        if end >= chars {
            break;
        }
        let mut next = end.saturating_sub(overlap).max(start + 1);
        // Start the next window at a word.
        while next < end && !is_space(next) {
            next += 1;
        }
        start = if next < end { next + 1 } else { end };
    }
    chunks
}

/// Extract text content from a context update.
pub(crate) fn extract_text_from_update(update: &ContextUpdate) -> String {
    let mut text_parts = Vec::new();

    // ALWAYS include title (question) for all update types.
    // For QA updates: the question is critical for semantic search — users typically
    // search with questions, not answers, so the question must be embedded.
    let title = update.content.title.clone();
    let description = update.content.description.clone();

    tracing::debug!(
        "extract_text_from_update: title='{}', description='{}'",
        &title[..title.floor_char_boundary(50)],
        &description[..description.floor_char_boundary(50)]
    );

    text_parts.push(title);
    text_parts.push(description);
    text_parts.extend(update.content.details.iter().cloned());
    text_parts.extend(update.content.examples.iter().cloned());
    text_parts.extend(update.content.implications.iter().cloned());

    if let Some(code_ref) = &update.related_code {
        text_parts.push(code_ref.code_snippet.clone());
        text_parts.push(code_ref.file_path.clone());
    }

    text_parts.join(" ")
}

/// Determine content type from context update.
pub(crate) const fn determine_content_type(update: &ContextUpdate) -> ContentKind {
    use post_cortex_core::core::context_update::UpdateType;
    match &update.update_type {
        UpdateType::QuestionAnswered => ContentKind::UserMessage,
        UpdateType::ProblemSolved => ContentKind::ProblemSolution,
        UpdateType::CodeChanged => ContentKind::CodeSnippet,
        UpdateType::DecisionMade => ContentKind::DecisionPoint,
        UpdateType::ConceptDefined | UpdateType::RequirementAdded => ContentKind::UpdateContent,
    }
}

#[cfg(test)]
mod tests;

/// Whether `text` is long enough for `vectorizer` to embed.
pub fn worth_embedding(
    vectorizer: &crate::content_vectorizer::ContentVectorizer,
    text: &str,
) -> bool {
    text.trim().len() >= vectorizer.config().min_text_length
}
