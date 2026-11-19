// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Telling an agent a file's memory (docs/memory.md, a file's memory; owner
//! 2026-11-18): once per agent session and file, as early as it can be
//! heard. When the agent reads the file, its memory is added to what the read
//! returned; when it edits a file it was not told about, the edit is held
//! once with the memory as the reason, so the agent reads it before anything
//! is written and sends the edit again. A file the memory knows nothing about
//! is never held and never marked told.
//!
//! Which files a session was told is kept in this instance only: a restart
//! tells each file once more. Sessions idle for twelve hours are forgotten,
//! and at most 4096 are kept.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use uuid::Uuid;

use crate::capabilities::scope::Target;
use crate::capabilities::views::{Views, ViewsError};
use crate::graph_views::{FileView, RenderedView, ViewRequest};

/// A session idle this long is forgotten.
const IDLE: Duration = Duration::from_secs(12 * 60 * 60);
/// Sessions kept at most; past it, the longest idle goes first.
const SESSIONS: usize = 4096;

/// What the agent is doing with the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// It read the file: the read already happened.
    Read,
    /// It is about to change the file.
    Edit,
}

/// What to do with a file's memory now.
#[derive(Debug, Clone)]
pub enum Telling {
    /// Nothing: the memory knows nothing about the file, or the session was
    /// told already.
    Nothing,
    /// Add the view to what the agent reads (a read, or a client that names
    /// no session, which is told every time as before).
    Add(Box<FileView>),
    /// Hold the edit once with the view as the reason.
    Hold(Box<FileView>),
}

/// One agent session's files that were told.
struct Told {
    seen: Instant,
    files: HashSet<String>,
}

/// The FileTelling capability service.
#[derive(Clone)]
pub struct FileTelling {
    views: Views,
    told: Arc<Mutex<HashMap<(Uuid, String), Told>>>,
}

impl FileTelling {
    /// The service over `views`, which renders a file's memory; no session
    /// was told anything yet.
    pub fn new(views: Views) -> Self {
        Self {
            views,
            told: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// What to tell agent session `session` (the client's own id; empty: a
    /// client that names none) of workspace `workspace` about the file
    /// `request` names, at `moment`.
    ///
    /// # Errors
    /// The file view failed.
    pub async fn tell(
        &self,
        workspace: Uuid,
        session: &str,
        request: ViewRequest,
        moment: Moment,
    ) -> Result<Telling, ViewsError> {
        let ViewRequest::File { path, .. } = &request else {
            return Err(ViewsError::InvalidArgument(
                "a file's memory is told for a file view".to_string(),
            ));
        };
        let path = path.clone();
        let key = (workspace, session.to_string());
        if !session.is_empty() && self.was_told(&key, &path) {
            return Ok(Telling::Nothing);
        }
        let target = Target::of(None, Some(workspace))?;
        let view = match self.views.view(target, request).await? {
            RenderedView::File(view) => view,
            _ => {
                return Err(ViewsError::InvalidArgument(
                    "the file view rendered another view".to_string(),
                ));
            }
        };
        if !knows_anything(&view) {
            return Ok(Telling::Nothing);
        }
        if session.is_empty() {
            return Ok(Telling::Add(Box::new(view)));
        }
        // Two calls at once for one file: the first to mark it tells.
        if !self.mark(key, path) {
            return Ok(Telling::Nothing);
        }
        Ok(match moment {
            Moment::Read => Telling::Add(Box::new(view)),
            Moment::Edit => Telling::Hold(Box::new(view)),
        })
    }

    fn was_told(&self, key: &(Uuid, String), path: &str) -> bool {
        let mut told = self.told.lock();
        match told.get_mut(key) {
            Some(session) => {
                session.seen = Instant::now();
                session.files.contains(path)
            }
            None => false,
        }
    }

    /// Mark `path` told to `key`; false when it already was.
    fn mark(&self, key: (Uuid, String), path: String) -> bool {
        let now = Instant::now();
        let mut told = self.told.lock();
        if !told.contains_key(&key) {
            told.retain(|_, session| now.duration_since(session.seen) < IDLE);
            if told.len() >= SESSIONS
                && let Some(oldest) = told
                    .iter()
                    .min_by_key(|(_, session)| session.seen)
                    .map(|(key, _)| key.clone())
            {
                told.remove(&oldest);
            }
        }
        let session = told.entry(key).or_insert_with(|| Told {
            seen: now,
            files: HashSet::new(),
        });
        session.seen = now;
        session.files.insert(path)
    }
}

/// Whether a file's view says anything an agent is told: problems solved
/// there, decisions about it, what depends on it, the code changed with it.
fn knows_anything(view: &FileView) -> bool {
    !(view.problems.is_empty()
        && view.decisions.is_empty()
        && view.dependents.is_empty()
        && view.changed_with.is_empty())
}
