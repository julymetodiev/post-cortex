// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The boards themselves (ARCHITECTURE section 8): a new board with its
//! session, and the boards as they read, each column with its stage
//! (W26).

use pcx_documents::Item;
use pcx_module_api::ModuleError;

use crate::domain::{self, Board, Kind};
use crate::service::{Service, broken};

impl Service<'_> {
    /// A new board with its session, its columns at `stages` (none: the
    /// last done, the others open), its tasks' branches in `repo`.
    ///
    /// # Errors
    /// The board breaks a rule; memory failed.
    pub async fn create_board(
        &self,
        name: &str,
        columns: Vec<String>,
        stages: Vec<i32>,
        repo: Option<String>,
    ) -> Result<Item<Board>, ModuleError> {
        // The rules first: no session for a board that is refused.
        let checked = domain::board(name, columns, stages, repo, String::new()).map_err(broken)?;
        let session_id = self
            .records
            .create_session(&format!("Board: {}", checked.name))
            .await?;
        let board = Board {
            session_id,
            ..checked
        };
        self.create(Kind::Board, &board, &board.name).await
    }

    /// Board `id` from the version the caller read, renamed to `name` and
    /// its tasks' branches in `repo` (empty: none), each when given; its
    /// columns stay.
    ///
    /// # Errors
    /// No such board; an empty name; it changed since that version
    /// (`Conflict`); memory failed.
    pub async fn update_board(
        &self,
        id: &str,
        version: u64,
        name: Option<String>,
        repo: Option<String>,
    ) -> Result<Item<Board>, ModuleError> {
        let current = self.board(id).await?.data;
        let repo = match repo {
            None => current.repo,
            Some(repo) => Some(repo.trim().to_string()).filter(|r| !r.is_empty()),
        };
        let name = name.unwrap_or(current.name);
        let checked = domain::board(
            &name,
            current.columns,
            current.stages,
            repo,
            current.session_id,
        )
        .map_err(broken)?;
        self.change(Kind::Board, id, version, &checked, &checked.name)
            .await
    }

    /// The boards, or the one with id `board`, each column with its stage.
    ///
    /// # Errors
    /// Memory failed; a board does not read.
    pub async fn boards(&self, board: Option<&str>) -> Result<Vec<Item<Board>>, ModuleError> {
        let boards = match board {
            Some(id) => vec![self.board(id).await?],
            None => self.all(Kind::Board, None::<(&Board, &[&str])>).await?,
        };
        // Every column's stage, a board written before W26 too.
        Ok(boards
            .into_iter()
            .map(|b| Item {
                data: domain::staged(b.data),
                ..b
            })
            .collect())
    }
}
