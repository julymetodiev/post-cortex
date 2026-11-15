// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The http module's log (docs/modules/http.md, the log): a page of it,
//! the newest first, by connection, host or outcome; the calls of a day,
//! which the daily limit counts; the entries past the days it keeps
//! removed.

use pcx_documents::{Item, days};
use pcx_module_api::ModuleError;
use post_cortex_proto::http::{Call, CallRecord, ListLogRequest, LogPage, Outcome};

use crate::domain::{KEEP_DAYS, LOG_PAGE, MAX_LOG_PAGE};
use crate::service::Service;

fn record(item: Item<Call>) -> CallRecord {
    CallRecord {
        id: item.document.id,
        call: Some(item.data),
    }
}

impl Service<'_> {
    /// A page of the log, the newest first: after entry `before` when the
    /// request names one, of its connection, host and outcome when it
    /// names them.
    ///
    /// # Errors
    /// `NotFound` when `before` is no entry of the log; memory failed.
    pub async fn list_log(&self, request: ListLogRequest) -> Result<LogPage, ModuleError> {
        let limit = match request.limit {
            0 => LOG_PAGE,
            n => usize::try_from(n).map_or(MAX_LOG_PAGE, |n| n.min(MAX_LOG_PAGE)),
        };
        let example = Call {
            connection: request.connection.clone(),
            host: request.host.to_ascii_lowercase(),
            outcome: request.outcome,
            ..Call::default()
        };
        let mut fields = Vec::new();
        if !request.connection.is_empty() {
            fields.push("connection");
        }
        if !request.host.is_empty() {
            fields.push("host");
        }
        if request.outcome != i32::from(Outcome::Unspecified) {
            fields.push("outcome");
        }
        let matching = (!fields.is_empty()).then_some((&example, fields.as_slice()));
        let before = Some(request.before.as_str()).filter(|b| !b.is_empty());
        let (entries, more) = self
            .log()
            .after::<Call>(matching, before, limit, |_| true)
            .await?;
        Ok(LogPage {
            entries: entries.into_iter().map(record).collect(),
            more,
        })
    }

    /// The requests sent on `day` (answered or failed; a refused one sent
    /// nothing).
    ///
    /// # Errors
    /// Memory failed.
    pub async fn counted(&self, day: &str) -> Result<u32, ModuleError> {
        let example = Call {
            day: day.to_string(),
            ..Call::default()
        };
        let sent = self
            .log()
            .all::<Call>(Some((&example, &["day"][..])))
            .await?
            .into_iter()
            .filter(|e| e.data.outcome != i32::from(Outcome::Refused))
            .count();
        Ok(u32::try_from(sent).unwrap_or(u32::MAX))
    }

    /// Remove the entries the log no longer keeps: how many.
    ///
    /// # Errors
    /// Memory failed. An entry whose day does not read is left, and said.
    pub async fn prune(&self) -> Result<usize, ModuleError> {
        let today = self.clock.now();
        let mut removed = 0;
        for entry in self.log().all::<Call>(None).await? {
            match days::expired(&entry.data.day, today, KEEP_DAYS) {
                Ok(true) => {
                    self.log().delete(&entry.document.id).await?;
                    removed += 1;
                }
                Ok(false) => {}
                Err(why) => {
                    tracing::warn!(entry = %entry.document.id, %why, "an http log entry is kept: its day does not read");
                }
            }
        }
        Ok(removed)
    }
}
