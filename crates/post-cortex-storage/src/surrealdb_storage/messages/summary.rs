// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The summary of each message stream, `message_stream` keyed by session
//! id: count, first and last position with their times, the authors. Every
//! write of a stream keeps it in the write's own transaction: an append
//! adds to it, a delete or a restore recounts it from the rows. A stream
//! written before summaries existed gets one from its rows the first time
//! it is read, so the stats of many streams are one read.

use serde::Deserialize;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use post_cortex_core::core::message::MessageStats;
use post_cortex_ports::error::corrupt;

use crate::port_error::Result;

/// Recount the summary of each stream `streams` (a SurrealQL expression
/// of session ids) from its rows: one statement, so it shifts the results
/// of a transaction by one.
pub fn recount(streams: &str) -> String {
    format!(
        "FOR $stream IN {streams} {{ \
             LET $rows = (SELECT seq, timestamp FROM message WHERE session_id = $stream ORDER BY seq); \
             LET $authors = array::sort(array::distinct( \
                 (SELECT VALUE author FROM message WHERE session_id = $stream AND author != NONE))); \
             UPSERT type::record('message_stream', $stream) CONTENT {{ \
                 count: array::len($rows), \
                 first_seq: array::first($rows).seq, first_at: array::first($rows).timestamp, \
                 last_seq: array::last($rows).seq, last_at: array::last($rows).timestamp, \
                 authors: $authors \
             }}; \
         }};"
    )
}

/// Add an append to the summary of `$sid` (the messages appended: `$added`,
/// the first and last of them, their authors); a stream without one is
/// recounted. One statement.
pub fn add_append() -> String {
    let recount = recount("[$sid]");
    format!(
        "IF (SELECT * FROM ONLY type::record('message_stream', $sid)) = NONE {{ {recount} }} ELSE {{ \
             UPDATE type::record('message_stream', $sid) SET \
                 count += $added, \
                 first_seq = first_seq ?? $first_seq, first_at = first_at ?? $first_at, \
                 last_seq = $last_seq, last_at = $last_at, \
                 authors = array::sort(array::union(authors, $authors)); \
         }};"
    )
}

/// A `message_stream` row.
#[derive(Debug, Deserialize, SurrealValue)]
struct SummaryRow {
    count: i64,
    first_seq: Option<i64>,
    first_at: Option<String>,
    last_seq: Option<i64>,
    last_at: Option<String>,
    authors: Vec<String>,
}

impl SummaryRow {
    /// The stats the row of stream `sid` holds; a stream with messages has
    /// both ends, or the row is `Corrupt`.
    fn into_stats(self, sid: &str) -> Result<MessageStats> {
        let count = u64::try_from(self.count)?;
        if count == 0 {
            return Ok(MessageStats::default());
        }
        let end = |seq: Option<i64>, at: Option<String>, which: &str| -> Result<_> {
            let (Some(seq), Some(at)) = (seq, at) else {
                return Err(corrupt(
                    "message_stream",
                    sid,
                    format!("{count} messages without a {which} one"),
                )
                .into());
            };
            Ok((
                u64::try_from(seq)?,
                post_cortex_ports::error::parse_timestamp("message_stream", sid, &at)?,
            ))
        };
        let (first_seq, first_at) = end(self.first_seq, self.first_at, "first")?;
        let (last_seq, last_at) = end(self.last_seq, self.last_at, "last")?;
        Ok(MessageStats {
            count,
            first_seq: Some(first_seq),
            last_seq: Some(last_seq),
            first_at: Some(first_at),
            last_at: Some(last_at),
            authors: self.authors,
        })
    }
}

/// Attempts to make a missing summary before its failure is returned.
const BACKFILL_ATTEMPTS: u64 = 3;

/// The summary row of `sid`, if it has one.
async fn summary_row(db: &Surreal<Any>, sid: &str) -> Result<Option<SummaryRow>> {
    let mut response = db
        .query("SELECT * OMIT id FROM ONLY type::record('message_stream', $sid);")
        .bind(("sid", sid.to_string()))
        .await?
        .check()?;
    Ok(response.take(0)?)
}

/// Make the summary of `sid` from its rows if the stream has been written
/// and has none.
async fn recount_if_missing(db: &Surreal<Any>, sid: &str) -> Result<()> {
    let recount = recount("[$sid]");
    db.query(format!(
        "BEGIN TRANSACTION; \
         IF (SELECT * FROM ONLY type::record('message_stream', $sid)) = NONE \
             AND (SELECT * FROM ONLY type::record('message_next', $sid)) != NONE {{ {recount} }}; \
         COMMIT TRANSACTION;"
    ))
    .bind(("sid", sid.to_string()))
    .await?
    .check()?;
    Ok(())
}

/// The summary of `sid`, made from its rows first when the stream has been
/// written but has none yet. Views read many streams at once, so two of
/// them may make the same summary together and one transaction fails: the
/// row the other made is the answer, else the summary is made again, up to
/// [`BACKFILL_ATTEMPTS`] times.
async fn backfilled(db: &Surreal<Any>, sid: String) -> Result<Option<SummaryRow>> {
    let mut attempt = 1;
    loop {
        match recount_if_missing(db, &sid).await {
            Ok(()) => return summary_row(db, &sid).await,
            Err(failure) => {
                if let Some(row) = summary_row(db, &sid).await? {
                    return Ok(Some(row));
                }
                if attempt == BACKFILL_ATTEMPTS {
                    return Err(failure);
                }
                tokio::time::sleep(std::time::Duration::from_millis(20 * attempt)).await;
                attempt += 1;
            }
        }
    }
}

/// The stats of each of `session_ids`, in that order: one read of their
/// summaries, then a summary made for each stream written without one.
pub async fn stats_of(db: &Surreal<Any>, session_ids: &[Uuid]) -> Result<Vec<MessageStats>> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }
    let sids: Vec<String> = session_ids.iter().map(Uuid::to_string).collect();
    let mut response = db
        .query(
            "RETURN array::map($sids, |$s| \
                 (SELECT * OMIT id FROM ONLY type::record('message_stream', $s)));",
        )
        .bind(("sids", sids.clone()))
        .await?
        .check()?;
    let rows: Vec<Option<SummaryRow>> = response.take(0)?;
    if rows.len() != sids.len() {
        return Err(corrupt(
            "message_stream",
            "summaries",
            format!("{} rows for {} streams", rows.len(), sids.len()),
        )
        .into());
    }
    let mut stats = Vec::with_capacity(sids.len());
    for (sid, row) in sids.into_iter().zip(rows) {
        let row = match row {
            Some(row) => Some(row),
            None => backfilled(db, sid.clone()).await?,
        };
        // No row after the backfill: the stream was never written.
        stats.push(match row {
            Some(row) => row.into_stats(&sid)?,
            None => MessageStats::default(),
        });
    }
    Ok(stats)
}

#[cfg(test)]
mod tests;
