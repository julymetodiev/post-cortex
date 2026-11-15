// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board an engine's approvals are asked on (docs/modules/workflow.md,
//! its workspaces): the one the settings name, in the settings' workspace;
//! in any other workspace that workspace's own board named `Approvals`,
//! which the module makes when a workflow there first needs it, so the
//! people of a workspace decide on what runs in it.

use std::time::Duration;

use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::board::{BoardList, BoardRecord, CreateBoardRequest, ListBoardsRequest};
use tokio_util::sync::CancellationToken;

use crate::domain::MODULE_ID;
use crate::service::engine::Engine;

/// The name of a workspace's own board of approvals.
pub const APPROVALS: &str = "Approvals";

/// Its columns: a task waits for its decision, then is done.
const COLUMNS: [&str; 2] = ["waiting", "done"];

const LIST_BOARDS: &str = "/pcx.board.v1.Boards/ListBoards";
const CREATE_BOARD: &str = "/pcx.board.v1.Boards/CreateBoard";

/// The deadline of finding or making the board.
const DEADLINE: Duration = Duration::from_secs(60);

impl Engine {
    /// The board approvals are asked on; none: there is none. A workspace's
    /// own board that is missing is made when `make`.
    ///
    /// # Errors
    /// The board refused or failed.
    pub async fn approvals(&self, make: bool) -> Result<Option<String>, ModuleError> {
        if !self.settings.own_board {
            return Ok(Some(self.settings.board.clone()).filter(|board| !board.is_empty()));
        }
        let workspace = self.settings.workspace.clone();
        let mut boards = self.boards.lock().await;
        if let Some(board) = boards.get(&workspace) {
            return Ok(Some(board.clone()));
        }
        let scope = Scope::own(MODULE_ID, &workspace, DEADLINE, CancellationToken::new());
        let listed: BoardList = self
            .board(
                &scope,
                LIST_BOARDS,
                &ListBoardsRequest {
                    workspace_id: workspace.clone(),
                    board: String::new(),
                },
            )
            .await?;
        let found = listed
            .boards
            .into_iter()
            .find(|kept| kept.board.as_ref().is_some_and(|b| b.name == APPROVALS))
            .map(|kept| kept.id);
        let board = match found {
            None if make => {
                let made: BoardRecord = self
                    .board(
                        &scope,
                        CREATE_BOARD,
                        &CreateBoardRequest {
                            workspace_id: workspace.clone(),
                            name: APPROVALS.to_string(),
                            columns: COLUMNS.iter().map(ToString::to_string).collect(),
                            repo: None,
                            stages: Vec::new(),
                        },
                    )
                    .await?;
                Some(made.id)
            }
            found => found,
        };
        if let Some(board) = &board {
            boards.insert(workspace, board.clone());
        }
        Ok(board)
    }
}
