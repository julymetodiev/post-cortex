// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Moving a task (ARCHITECTURE section 8): to a column of its board, a
//! place in it and a plan, unless a decision holds it; a change of column
//! is the event `board.task_moved`.

use pcx_module_api::ModuleError;
use pcx_team::Act;
use post_cortex_proto::board::MoveTaskRequest;

use crate::domain::{self, Kind};
use crate::ports::BoardEvent;
use crate::service::{Placed, Service, broken, by_id, named, on_board, placed, with};

impl Service<'_> {
    /// Move the task `to` names from the version the caller read to a
    /// column of its board, before a task of it (none: at the end), and to
    /// another plan, unless a decision on it waits for approval. Only the
    /// task moved is written, then its note as the mover's comment (a move
    /// into a canceled column cancels it; nothing is deleted).
    ///
    /// # Errors
    /// No such task; a column its board does not have; a place or plan the
    /// board's rules refuse; an empty note; a decision holds it
    /// (`FailedPrecondition`); it changed since that version (`Conflict`).
    pub async fn move_task(&self, to: MoveTaskRequest) -> Result<Placed, ModuleError> {
        let current = self.task(&to.task).await?;
        let id = current.document.id.clone();
        let id = id.as_str();
        let board = self.board(&current.data.board).await?;
        let column = domain::column(&board.data, Some(&to.column)).map_err(broken)?;
        if domain::closes(&board.data, &column) {
            self.authorize(Act::Close).await?;
        }
        let decisions = self.decisions_of(id).await?;
        domain::may_move(decisions.iter().map(|d| (d.document.id.as_str(), &d.data)))
            .map_err(broken)?;
        // The note is checked before the move: a refused note moves nothing.
        let note = to
            .note
            .as_deref()
            .map(|n| domain::comment(id, n, None, Vec::new()).map(|c| (c, n.trim())))
            .transpose()
            .map_err(broken)?;
        let tasks = self.board_tasks(&board.document.id).await?;
        let board_tasks = by_id(&tasks);
        let mut task = current.data;
        if let Some(parent) = to.parent {
            task.parent = named(parent)
                .map(|p| on_board(&p, &board_tasks))
                .transpose()?;
            if let Some(parent) = &task.parent {
                domain::plan(Some(id), parent, &board_tasks).map_err(broken)?;
            }
        }
        let before = to
            .before
            .as_deref()
            .map(|b| on_board(b, &board_tasks))
            .transpose()?;
        task.rank =
            domain::place(&column, Some(id), before.as_deref(), &board_tasks).map_err(broken)?;
        let from = std::mem::replace(&mut task.column, column);
        let moved = self
            .change(Kind::Task, id, to.version, &task, &current.document.text)
            .await?;
        if from != moved.data.column {
            self.events
                .emitted(BoardEvent::TaskMoved {
                    task: id.to_string(),
                    board: moved.data.board.clone(),
                    from,
                    to: moved.data.column.clone(),
                })
                .await;
        }
        if let Some((comment, text)) = note {
            self.create(Kind::Comment, &comment, text).await?;
        }
        let tasks = with(tasks, &moved);
        let talk = self.talk(Some(id)).await?;
        Ok(placed(&board.data, moved, &tasks, &talk))
    }
}
