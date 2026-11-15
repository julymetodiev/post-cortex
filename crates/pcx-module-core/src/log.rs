// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's log (W25): the newest lines of what the host saw of it and
//! what its process wrote, kept in a ring of [`LOG_LINES`].

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use post_cortex_proto::module as wire;

/// The lines a ring keeps; the oldest goes when one more comes.
pub const LOG_LINES: usize = 200;

/// Who wrote a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    /// The host, of what it saw of the module.
    Host,
    /// The module's process, on its stderr.
    Process,
}

/// A line of a module's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// When it was written.
    pub at: SystemTime,
    /// Who wrote it.
    pub source: LogSource,
    /// The line.
    pub text: String,
}

impl LogLine {
    /// The wire form.
    #[must_use]
    pub fn to_wire(&self) -> wire::LogLine {
        let source = match self.source {
            LogSource::Host => wire::LogSource::Host,
            LogSource::Process => wire::LogSource::Process,
        };
        wire::LogLine {
            at: self.at.duration_since(UNIX_EPOCH).ok().and_then(|d| {
                Some(pbjson_types::Timestamp {
                    seconds: i64::try_from(d.as_secs()).ok()?,
                    nanos: i32::try_from(d.subsec_nanos()).ok()?,
                })
            }),
            source: source.into(),
            text: self.text.clone(),
        }
    }
}

/// The newest lines of a log.
#[derive(Debug, Default)]
pub struct LogRing {
    lines: Mutex<VecDeque<LogLine>>,
}

impl LogRing {
    /// Write `text` as `source`, now.
    pub fn push(&self, source: LogSource, text: impl Into<String>) {
        let mut lines = self.lines.lock();
        if lines.len() == LOG_LINES {
            lines.pop_front();
        }
        lines.push_back(LogLine {
            at: SystemTime::now(),
            source,
            text: text.into(),
        });
    }

    /// Its lines, the oldest first.
    #[must_use]
    pub fn lines(&self) -> Vec<LogLine> {
        self.lines.lock().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_keeps_the_newest_lines() {
        let ring = LogRing::default();
        for i in 0..=LOG_LINES {
            ring.push(LogSource::Process, format!("line {i}"));
        }
        let lines = ring.lines();
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(lines[0].text, "line 1", "the oldest went");
        assert_eq!(lines[LOG_LINES - 1].text, format!("line {LOG_LINES}"));
        assert!(lines[0].to_wire().at.is_some());
    }
}
