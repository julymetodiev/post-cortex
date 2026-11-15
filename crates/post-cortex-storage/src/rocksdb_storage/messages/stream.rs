// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The row operations of a message stream (see the key families in
//! [`super`]); each runs on a blocking thread, under the session's lock
//! where it writes.

use crate::port_error::Result;
use chrono::{DateTime, Utc};
use post_cortex_ports::error::corrupt;
use rocksdb::{DB, IteratorMode, ReadOptions, WriteBatch};
use std::collections::{BTreeSet, HashSet};
use uuid::Uuid;

use post_cortex_core::core::message::{
    Message, MessageCursor, MessageOrder, MessageStats, NewMessage,
};

use crate::rocksdb_storage::core::{prefix_upper_bound, scan_prefix};
use crate::rocksdb_storage::message_rows;
use crate::rocksdb_storage::messages::{
    MESSAGE_PREFIX, message_authors_key, message_count_key, message_key, message_next_key,
    message_turn_key,
};

/// `message:{session_id}:`, the prefix of every row of one stream.
fn stream_prefix(session_id: Uuid) -> String {
    format!("{}{}:", MESSAGE_PREFIX, session_id)
}

/// Read options bounded to `[lower, upper)`, total-order (see
/// `prefix_read_opts`).
fn bounded_opts(lower: Vec<u8>, upper: Option<Vec<u8>>) -> ReadOptions {
    let mut opts = ReadOptions::default();
    opts.set_total_order_seek(true);
    opts.set_iterate_lower_bound(lower);
    if let Some(upper) = upper {
        opts.set_iterate_upper_bound(upper);
    }
    opts
}

/// Key bounds `[lower, upper)` of the rows of `session_id` that `cursor`
/// names.
fn cursor_bounds(session_id: Uuid, cursor: MessageCursor) -> (Vec<u8>, Option<Vec<u8>>) {
    let prefix = stream_prefix(session_id);
    let stream_end = prefix_upper_bound(prefix.as_bytes());
    match cursor {
        MessageCursor::From(seq) => (message_key(session_id, seq).into_bytes(), stream_end),
        MessageCursor::Before(seq) => (
            prefix.into_bytes(),
            Some(message_key(session_id, seq).into_bytes()),
        ),
    }
}

/// Up to `limit` messages of `session_id` from `cursor`, in `order`.
pub fn page(
    db: &DB,
    session_id: Uuid,
    cursor: MessageCursor,
    limit: usize,
    order: MessageOrder,
) -> Result<Vec<Message>> {
    let (lower, upper) = cursor_bounds(session_id, cursor);
    let mode = match order {
        MessageOrder::Asc => IteratorMode::Start,
        MessageOrder::Desc => IteratorMode::End,
    };
    let mut messages = Vec::new();
    for item in db.iterator_opt(mode, bounded_opts(lower, upper)) {
        if messages.len() == limit {
            break;
        }
        let (key, value) = item?;
        messages.push(decode_message(&key, &value)?);
    }
    Ok(messages)
}

/// Messages stored for `session_id`. The caller holds the session's lock:
/// a stream without a count is counted by a scan.
fn stored_count(db: &DB, session_id: Uuid) -> Result<u64> {
    let count_key = message_count_key(session_id);
    if let Some(bytes) = db.get(count_key.as_bytes())? {
        return decode_seq(&count_key, &bytes);
    }
    let mut count = 0;
    for item in scan_prefix(db, stream_prefix(session_id).as_bytes()) {
        item?;
        count += 1;
    }
    Ok(count)
}

/// The authors of the stored messages of `session_id`, leaving out the
/// positions in `skip` (the ones a delete is about to remove).
fn authors_of_rows(db: &DB, session_id: Uuid, skip: &HashSet<u64>) -> Result<BTreeSet<String>> {
    let prefix = stream_prefix(session_id);
    let mut authors = BTreeSet::new();
    for item in scan_prefix(db, prefix.as_bytes()) {
        let (key, value) = item?;
        if skip.contains(&seq_of_key(&prefix, &key)?) {
            continue;
        }
        if let Some(author) = decode_message(&key, &value)?.author {
            authors.insert(author);
        }
    }
    Ok(authors)
}

/// The authors stored for `session_id`, or `None` for a stream written
/// before they were kept.
fn kept_authors(db: &DB, session_id: Uuid) -> Result<Option<BTreeSet<String>>> {
    let key = message_authors_key(session_id);
    match db.get(key.as_bytes())? {
        None => Ok(None),
        Some(bytes) => {
            Ok(Some(serde_json::from_slice(&bytes).map_err(|e| {
                corrupt("message authors", &key, e.to_string())
            })?))
        }
    }
}

/// The authors of `session_id`'s stream. The caller holds the session's
/// lock: a stream without them is read from its rows.
fn stored_authors(db: &DB, session_id: Uuid) -> Result<BTreeSet<String>> {
    match kept_authors(db, session_id)? {
        Some(authors) => Ok(authors),
        None => authors_of_rows(db, session_id, &HashSet::new()),
    }
}

/// Queue `authors` as `session_id`'s in `batch`.
fn put_authors(batch: &mut WriteBatch, session_id: Uuid, authors: &BTreeSet<String>) -> Result<()> {
    batch.put(
        message_authors_key(session_id).as_bytes(),
        serde_json::to_vec(authors)?,
    );
    Ok(())
}

/// Stats of `session_id`'s stream. The caller holds the session's lock;
/// a count or authors missing from storage are written back.
pub fn stats(db: &DB, session_id: Uuid) -> Result<MessageStats> {
    let count_key = message_count_key(session_id);
    let count = match db.get(count_key.as_bytes())? {
        Some(bytes) => decode_seq(&count_key, &bytes)?,
        None => {
            let count = stored_count(db, session_id)?;
            if count > 0 {
                db.put(count_key.as_bytes(), count.to_be_bytes())?;
            }
            count
        }
    };
    if count == 0 {
        return Ok(MessageStats::default());
    }
    let authors = match kept_authors(db, session_id)? {
        Some(authors) => authors,
        None => {
            let authors = authors_of_rows(db, session_id, &HashSet::new())?;
            let mut batch = WriteBatch::default();
            put_authors(&mut batch, session_id, &authors)?;
            db.write(batch)?;
            authors
        }
    };
    let ends = |order| page(db, session_id, MessageCursor::From(0), 1, order);
    let first = ends(MessageOrder::Asc)?.pop();
    let last = ends(MessageOrder::Desc)?.pop();
    Ok(MessageStats {
        count,
        first_seq: first.as_ref().map(|m| m.seq),
        last_seq: last.as_ref().map(|m| m.seq),
        first_at: first.map(|m| m.timestamp),
        last_at: last.map(|m| m.timestamp),
        authors: authors.into_iter().collect(),
    })
}

fn encode_message(message: &Message) -> Result<Vec<u8>> {
    message_rows::encode(message)
}

pub fn decode_message(key: &[u8], row: &[u8]) -> Result<Message> {
    message_rows::decode(key, row)
}

/// The big-endian counter stored under `key`; any other length is
/// `Corrupt`.
fn decode_seq(key: &str, bytes: &[u8]) -> Result<u64> {
    let raw: [u8; 8] = bytes
        .try_into()
        .map_err(|_| corrupt("message counter", key, format!("{} bytes", bytes.len())))?;
    Ok(u64::from_be_bytes(raw))
}

/// Append `messages` to `session_id`'s stream. The caller holds the
/// session's lock.
pub fn append(db: &DB, session_id: Uuid, messages: Vec<NewMessage>) -> Result<Vec<Message>> {
    let next_key = message_next_key(session_id);
    let mut next = match db.get(next_key.as_bytes())? {
        Some(bytes) => decode_seq(&next_key, &bytes)?,
        None => 0,
    };
    let mut seen = HashSet::new();
    let mut batch = WriteBatch::default();
    let mut appended = Vec::new();
    for new in messages {
        let turn_key = message_turn_key(session_id, &new.turn_id);
        if !seen.insert(new.turn_id.clone()) || db.get(turn_key.as_bytes())?.is_some() {
            continue;
        }
        let message = new.into_message(session_id, next);
        batch.put(
            message_key(session_id, next).as_bytes(),
            encode_message(&message)?,
        );
        batch.put(turn_key.as_bytes(), next.to_be_bytes());
        appended.push(message);
        next += 1;
    }
    if !appended.is_empty() {
        let count = stored_count(db, session_id)? + appended.len() as u64;
        let mut authors = stored_authors(db, session_id)?;
        authors.extend(appended.iter().filter_map(|m| m.author.clone()));
        batch.put(next_key.as_bytes(), next.to_be_bytes());
        batch.put(
            message_count_key(session_id).as_bytes(),
            count.to_be_bytes(),
        );
        put_authors(&mut batch, session_id, &authors)?;
        db.write(batch)?;
    }
    Ok(appended)
}

/// Delete the messages of `session_id` at `seqs` that are still stored
/// and older than `cutoff`, with their turn index and the count, in one
/// batch. The caller holds the session's lock. Rechecked here because the
/// session may have been deleted (and its positions reused) since the
/// scan that found them.
pub fn expire(db: &DB, session_id: Uuid, seqs: &[u64], cutoff: DateTime<Utc>) -> Result<Vec<u64>> {
    let mut batch = WriteBatch::default();
    let mut deleted = Vec::new();
    for &seq in seqs {
        let key = message_key(session_id, seq);
        let Some(value) = db.get(key.as_bytes())? else {
            continue;
        };
        let Ok(message) = decode_message(key.as_bytes(), &value) else {
            continue;
        };
        if message.timestamp < cutoff {
            batch.delete(key.as_bytes());
            batch.delete(message_turn_key(session_id, &message.turn_id).as_bytes());
            deleted.push(seq);
        }
    }
    if !deleted.is_empty() {
        let count = stored_count(db, session_id)?.saturating_sub(deleted.len() as u64);
        let left = authors_of_rows(db, session_id, &deleted.iter().copied().collect())?;
        batch.put(
            message_count_key(session_id).as_bytes(),
            count.to_be_bytes(),
        );
        put_authors(&mut batch, session_id, &left)?;
        db.write(batch)?;
    }
    Ok(deleted)
}

/// Delete the messages of `session_id` at `seqs` that are stored, with
/// their turn index and the count, in one batch; `message_next` stays, so
/// positions are not reused. The caller holds the session's lock.
pub fn delete_seqs(db: &DB, session_id: Uuid, seqs: &[u64]) -> Result<Vec<u64>> {
    let mut batch = WriteBatch::default();
    let mut deleted = Vec::new();
    for &seq in seqs {
        let key = message_key(session_id, seq);
        let Some(value) = db.get(key.as_bytes())? else {
            continue;
        };
        batch.delete(key.as_bytes());
        match decode_message(key.as_bytes(), &value) {
            Ok(message) => {
                batch.delete(message_turn_key(session_id, &message.turn_id).as_bytes());
            }
            Err(e) => tracing::warn!(
                "Deleting unreadable message {} of session {}: {:#}",
                seq,
                session_id,
                e
            ),
        }
        deleted.push(seq);
    }
    if !deleted.is_empty() {
        let count = stored_count(db, session_id)?.saturating_sub(deleted.len() as u64);
        let left = authors_of_rows(db, session_id, &deleted.iter().copied().collect())?;
        batch.put(
            message_count_key(session_id).as_bytes(),
            count.to_be_bytes(),
        );
        put_authors(&mut batch, session_id, &left)?;
        db.write(batch)?;
    }
    Ok(deleted)
}

/// Write archived `messages` into `session_id`'s stream (see
/// [`MessageStore::restore_messages`]). The caller holds the session's
/// lock.
pub fn restore(db: &DB, session_id: Uuid, messages: Vec<Message>) -> Result<Vec<(u64, Message)>> {
    let next_key = message_next_key(session_id);
    let next = match db.get(next_key.as_bytes())? {
        Some(bytes) => decode_seq(&next_key, &bytes)?,
        None => 0,
    };
    let mut known = HashSet::new();
    for message in &messages {
        if db
            .get(message_turn_key(session_id, &message.turn_id).as_bytes())?
            .is_some()
        {
            known.insert(message.turn_id.clone());
        }
    }
    let (placed, next) =
        crate::export_import::place_restored_messages(session_id, messages, next, &known);
    if placed.is_empty() {
        return Ok(placed);
    }
    let mut batch = WriteBatch::default();
    for (_, message) in &placed {
        batch.put(
            message_key(session_id, message.seq).as_bytes(),
            encode_message(message)?,
        );
        batch.put(
            message_turn_key(session_id, &message.turn_id).as_bytes(),
            message.seq.to_be_bytes(),
        );
    }
    let count = stored_count(db, session_id)? + placed.len() as u64;
    let mut authors = stored_authors(db, session_id)?;
    authors.extend(placed.iter().filter_map(|(_, m)| m.author.clone()));
    batch.put(next_key.as_bytes(), next.to_be_bytes());
    batch.put(
        message_count_key(session_id).as_bytes(),
        count.to_be_bytes(),
    );
    put_authors(&mut batch, session_id, &authors)?;
    db.write(batch)?;
    Ok(placed)
}

/// The position a row key of the stream with `prefix` names.
fn seq_of_key(prefix: &str, key: &[u8]) -> Result<u64> {
    Ok(std::str::from_utf8(&key[prefix.len()..])
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| corrupt("message key", String::from_utf8_lossy(key), "bad sequence"))?)
}

/// Positions of every stored message of `session_id`.
pub fn stream_seqs(db: &DB, session_id: Uuid) -> Result<Vec<u64>> {
    let prefix = stream_prefix(session_id);
    let mut seqs = Vec::new();
    for item in scan_prefix(db, prefix.as_bytes()) {
        let (key, _) = item?;
        seqs.push(seq_of_key(&prefix, &key)?);
    }
    Ok(seqs)
}
