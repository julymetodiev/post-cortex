// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Merging an import into what storage holds: cards and documents by
//! history (cards also by time), messages by free positions, and their
//! embeddings.

use crate::capabilities::transfer::preview::ConflictResolution;
use chrono::{DateTime, Utc};
use post_cortex_core::archive::{ExportedEmbedding, ImportOptions, ImportResult, card_histories};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::core::dismissal::RadarDismissal;
use post_cortex_core::core::document::Document;
use post_cortex_core::core::message::{self, Message};
use post_cortex_ports::storage::Stores;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A history limit that reads every version of a card.
pub const ALL_VERSIONS: usize = i64::MAX as usize;

/// Metadata keys of a message vector that name its position (as the
/// vectorizer writes them).
pub const MESSAGE_SEQ_KEY: &str = "seq";

pub const CHUNK_PARENT_KEY: &str = "parent";

/// Whether `embedding` is the vector of (a chunk of) a captured message.
pub fn is_message_embedding(embedding: &ExportedEmbedding) -> bool {
    embedding.content_type == post_cortex_ports::MESSAGE_CONTENT_TYPE
        && message::seq_of_content_id(&embedding.content_id).is_some()
}

/// A history whose versions an import merges: a card's or a document's.
pub trait Versioned: Clone {
    /// Its version number.
    fn version(&self) -> u64;
    /// Whether `other` holds the same content.
    fn same_content(&self, other: &Self) -> bool;
    /// When it was written.
    fn written_at(&self) -> DateTime<Utc>;
    /// This version renumbered as `version`.
    fn renumbered(&self, version: u64) -> Self;
}

impl Versioned for Card {
    fn version(&self) -> u64 {
        self.version
    }
    fn same_content(&self, other: &Self) -> bool {
        self.text == other.text
    }
    fn written_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
    fn renumbered(&self, version: u64) -> Self {
        Self {
            version,
            ..self.clone()
        }
    }
}

impl Versioned for Document {
    fn version(&self) -> u64 {
        self.version
    }
    fn same_content(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.data == other.data
            && self.title == other.title
            && self.text == other.text
    }
    fn written_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
    fn renumbered(&self, version: u64) -> Self {
        Self {
            version,
            ..self.clone()
        }
    }
}

/// What an import writes for one card or document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryMerge<T> {
    /// Nothing.
    Keep,
    /// These versions, on top of the target's.
    Add(Vec<T>),
    /// These versions, in place of the target's history.
    Replace(Vec<T>),
}

/// What an import writes for the card of one scope.
pub type CardMerge = HistoryMerge<Card>;

/// How the archived history of a card or a document merges into the
/// target's (both oldest first), and the conflict to report when the two
/// diverged.
///
/// None in the target: the archive's history is written as it is. One
/// history continuing the other (same versions with the same content): the
/// target gets the archive's later versions, or stays when it is ahead.
/// The same latest content: nothing to do. Otherwise they diverged, and the
/// version numbers of the two histories say nothing about which is newer:
/// with `overwrite` the archive's history replaces the target's; with
/// `newer` the archive's latest version is added as the target's next one
/// when it was written later, so no version of the target is lost;
/// otherwise the target's latest version stays and the conflict is
/// reported.
pub fn merge_history<T: Versioned>(
    archive: &[T],
    target: &[T],
    overwrite: bool,
    newer: bool,
) -> (HistoryMerge<T>, Option<ConflictResolution>) {
    let (Some(theirs), Some(ours)) = (archive.last(), target.last()) else {
        return if target.is_empty() && !archive.is_empty() {
            (HistoryMerge::Add(archive.to_vec()), None)
        } else {
            (HistoryMerge::Keep, None)
        };
    };
    if theirs.same_content(ours) {
        return (HistoryMerge::Keep, None);
    }
    let same = |a: &T, b: &T| a.version() == b.version() && a.same_content(b);
    let continues =
        |longer: &[T], shorter: &[T]| shorter.iter().all(|c| longer.iter().any(|l| same(l, c)));
    if continues(archive, target) {
        let later = archive
            .iter()
            .filter(|c| c.version() > ours.version())
            .cloned()
            .collect();
        return (HistoryMerge::Add(later), None);
    }
    if continues(target, archive) {
        return (HistoryMerge::Keep, None);
    }
    if overwrite {
        return (
            HistoryMerge::Replace(archive.to_vec()),
            Some(ConflictResolution::Overwrite),
        );
    }
    if newer && theirs.written_at() > ours.written_at() {
        let next = theirs.renumbered(ours.version() + 1);
        (
            HistoryMerge::Add(vec![next]),
            Some(ConflictResolution::Update),
        )
    } else {
        (HistoryMerge::Keep, Some(ConflictResolution::Keep))
    }
}

/// [`merge_history`] of a card: `overwrite`, and `cards_newer`.
pub fn merge_card(
    archive: &[Card],
    target: &[Card],
    options: &ImportOptions,
) -> (CardMerge, Option<ConflictResolution>) {
    merge_history(archive, target, options.overwrite, options.cards_newer)
}

/// [`merge_history`] of a document: `overwrite` only (a module's state is
/// not merged by time).
pub fn merge_document(
    archive: &[Document],
    target: &[Document],
    options: &ImportOptions,
) -> (HistoryMerge<Document>, Option<ConflictResolution>) {
    merge_history(archive, target, options.overwrite, false)
}

/// Whether an import with `options` covers the card of `scope`: a user card
/// (the host's or a person's) unless the import is limited to some
/// workspaces, a workspace's card when the workspace is imported.
pub fn card_covered(scope: CardScope, options: &ImportOptions, workspaces: &HashSet<Uuid>) -> bool {
    match scope {
        CardScope::User | CardScope::Person(_) => options.workspace_filter.is_none(),
        CardScope::Workspace(id) => workspaces.contains(&id),
    }
}

/// Where an import stored the archived messages: `(session id, archived
/// seq) -> stored seq`.
pub type Positions = HashMap<(String, u64), u64>;

/// Merge the archived `messages` into the stream of `session_id` (whose
/// session was imported, skipped or overwritten), counting them in
/// `result` and noting where each was stored in `positions`.
pub async fn restore_messages(
    stores: &Stores,
    session_id: Uuid,
    messages: Vec<Message>,
    result: &mut ImportResult,
    positions: &mut Positions,
) {
    if messages.is_empty() {
        return;
    }
    let archived = messages.len();
    match stores.messages.restore_messages(session_id, messages).await {
        Ok(stored) => {
            result.messages_imported += stored.len();
            result.messages_skipped += archived - stored.len();
            if !stored.is_empty() {
                result.message_sessions.push(session_id);
            }
            let sid = session_id.to_string();
            for (from, message) in stored {
                positions.insert((sid.clone(), from), message.seq);
            }
        }
        Err(e) => result.errors.push(format!(
            "Failed to import the messages of session {}: {:#}",
            session_id, e
        )),
    }
}

/// Add the archived `dismissals` of `workspace_id` (imported, skipped or
/// overwritten). A dismissal replaces one of the same kind and key; the
/// target's other dismissals stay.
pub async fn restore_dismissals(
    stores: &Stores,
    workspace_id: Uuid,
    dismissals: Vec<RadarDismissal>,
    result: &mut ImportResult,
) {
    for dismissal in dismissals {
        if let Err(e) = stores
            .dismissals
            .put_dismissal(workspace_id, &dismissal)
            .await
        {
            result.errors.push(format!(
                "Failed to import a radar dismissal of workspace {workspace_id}: {e:#}"
            ));
        }
    }
}

/// Merge the archived `cards` into `stores`: the user card unless the
/// import is limited to some workspaces, the cards of the `workspaces`
/// imported, skipped or overwritten.
pub async fn restore_cards(
    stores: &Stores,
    cards: Vec<Card>,
    options: &ImportOptions,
    workspaces: &HashSet<Uuid>,
    result: &mut ImportResult,
) {
    for (scope, archived) in card_histories(cards) {
        if !card_covered(scope, options, workspaces) {
            continue;
        }
        let merged = match stores.cards.card_history(scope, ALL_VERSIONS).await {
            Ok(mut target) => {
                target.reverse();
                merge_card(&archived, &target, options).0
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to read the card {}: {:#}", scope.key(), e));
                continue;
            }
        };
        let write = match merged {
            CardMerge::Keep => {
                result.cards_skipped += 1;
                continue;
            }
            CardMerge::Add(versions) => stores.cards.restore_card_versions(scope, versions, false),
            CardMerge::Replace(versions) => {
                stores.cards.restore_card_versions(scope, versions, true)
            }
        };
        match write.await {
            Ok(()) => result.cards_imported += 1,
            Err(e) => result.errors.push(format!(
                "Failed to import the card {}: {:#}",
                scope.key(),
                e
            )),
        }
    }
}

/// The archived `embedding` as an import writes it, given where its
/// messages were stored ([`Positions`]): the vector of a message not
/// written (its turn was stored already) is left out, one of a message
/// stored at another position is renamed to it. Other vectors pass.
pub fn restored_embedding(
    mut embedding: ExportedEmbedding,
    positions: &Positions,
) -> Option<ExportedEmbedding> {
    if !is_message_embedding(&embedding) {
        return Some(embedding);
    }
    let from = message::seq_of_content_id(&embedding.content_id)?;
    let to = *positions.get(&(embedding.session_id.clone(), from))?;
    if to != from {
        let chunk = embedding
            .content_id
            .find('#')
            .map_or(String::new(), |at| embedding.content_id[at..].to_string());
        embedding.content_id = format!("{}{}", message::content_id(to), chunk);
        if embedding.metadata.contains_key(MESSAGE_SEQ_KEY) {
            embedding
                .metadata
                .insert(MESSAGE_SEQ_KEY.to_string(), to.to_string());
        }
        if embedding.metadata.contains_key(CHUNK_PARENT_KEY) {
            embedding
                .metadata
                .insert(CHUNK_PARENT_KEY.to_string(), message::content_id(to));
        }
    }
    Some(embedding)
}

/// Merge one archived document's history (oldest first) into `stores`,
/// counting it in `result`; whether it was written. Its latest version is
/// unkeyed until memory keys it at its next start (docs/memory.md,
/// document keys and pages).
pub async fn restore_document(
    stores: &Stores,
    archived: Vec<Document>,
    options: &ImportOptions,
    result: &mut ImportResult,
) -> bool {
    let Some(first) = archived.first() else {
        return false;
    };
    let (workspace_id, id) = (first.workspace_id, first.id);
    let merged = match stores
        .documents
        .document_history(workspace_id, id, ALL_VERSIONS)
        .await
    {
        Ok(mut target) => {
            target.reverse();
            merge_document(&archived, &target, options).0
        }
        Err(e) => {
            result
                .errors
                .push(format!("Failed to read the document {id}: {e:#}"));
            return false;
        }
    };
    let write = match merged {
        HistoryMerge::Keep => {
            result.documents_skipped += 1;
            return false;
        }
        HistoryMerge::Add(versions) => {
            stores
                .documents
                .restore_document_versions(workspace_id, id, versions, false, None)
        }
        HistoryMerge::Replace(versions) => {
            stores
                .documents
                .restore_document_versions(workspace_id, id, versions, true, None)
        }
    };
    match write.await {
        Ok(()) => {
            result.documents_imported += 1;
            result.documents.push((workspace_id, id));
            true
        }
        Err(e) => {
            result
                .errors
                .push(format!("Failed to import the document {id}: {e:#}"));
            false
        }
    }
}

/// An archive's document versions grouped by document: an export writes
/// each document's versions one after the other, so neither pass holds
/// more than one document's.
#[derive(Default)]
pub struct DocumentRuns {
    run: Vec<Document>,
}

impl DocumentRuns {
    /// Take the next version; the previous document's versions, oldest
    /// first, when this one starts another document.
    pub fn push(&mut self, version: Document) -> Option<Vec<Document>> {
        let done = match self.run.first() {
            Some(first) if (first.workspace_id, first.id) != (version.workspace_id, version.id) => {
                Some(self.take())
            }
            _ => None,
        };
        self.run.push(version);
        done
    }

    /// The last document's versions, oldest first, if any.
    pub fn finish(&mut self) -> Option<Vec<Document>> {
        (!self.run.is_empty()).then(|| self.take())
    }

    fn take(&mut self) -> Vec<Document> {
        let mut run = std::mem::take(&mut self.run);
        run.sort_by_key(|d| d.version);
        run
    }
}
