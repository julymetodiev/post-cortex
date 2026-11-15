// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The agent service and the model's window: whether a conversation is
//! compacted before the next model call, and the compaction (the context
//! engine sums up what comes before the cut; the summary is kept as a
//! turn, then the turns from the cut on again, as they were).

use pcx_module_api::ModuleError;
use pcx_module_api::model::{Block, ModelTool, Role};

use crate::domain::{self, Turn};
use crate::service::Agent;

impl Agent<'_> {
    /// Whether the conversation of `turns` is compacted before the next
    /// call of `model` with `system` and `tools`: its request leaves too
    /// little of the model's window for the answer. After a step that
    /// called tools (`called`) it waits, at most
    /// [`domain::MAX_COMPACTION_DEFERRALS`] times (`deferrals`), so a
    /// task's tool calls are summed up together.
    pub fn due(
        &self,
        model: &str,
        system: &str,
        turns: &[Turn],
        tools: &[ModelTool],
        called: bool,
        deferrals: &mut u32,
    ) -> bool {
        let window = self
            .models
            .models()
            .iter()
            .find(|m| m.id == model)
            .map_or(0, |m| m.context_tokens);
        let needed = domain::estimate(system, &domain::conversation(turns), tools);
        if !domain::over(needed, window) {
            return false;
        }
        if called && *deferrals < domain::MAX_COMPACTION_DEFERRALS {
            *deferrals += 1;
            return false;
        }
        true
    }

    /// Compact the conversation: the context engine sums up its chain
    /// before the cut ([`domain::to_compact`]), the summary is kept as a
    /// turn by `speaker` and the turns from the cut on again after it (each
    /// by its own author); whether it could (a context engine, and
    /// something before the cut).
    pub async fn compact(
        &self,
        session: &str,
        model: &str,
        turns: &mut Vec<Turn>,
        speaker: Option<String>,
    ) -> Result<bool, ModuleError> {
        let Some(context) = self.context else {
            return Ok(false);
        };
        let Some((before, kept)) = domain::to_compact(turns, domain::KEEP_RECENT_TOKENS) else {
            return Ok(false);
        };
        let kept: Vec<Turn> = kept.into_iter().cloned().collect();
        let summary = context
            .summarize(self.scope, session, model, &before)
            .await?;
        self.keep(
            session,
            turns,
            Role::User,
            vec![Block::Summary { text: summary }],
            speaker,
        )
        .await?;
        for turn in kept {
            self.keep_turn(session, turns, |turn_id, parent| Turn {
                turn_id,
                parent,
                ..turn
            })
            .await?;
        }
        Ok(true)
    }
}
