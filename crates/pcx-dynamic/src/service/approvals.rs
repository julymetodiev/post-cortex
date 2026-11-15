// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the board answered: a version approved when its decision is, one a
//! person declined kept as declined, and the decisions read again when the
//! module starts.

use pcx_module_api::ModuleError;
use post_cortex_proto::board::{DecisionApproved, DecisionDeclined};
use post_cortex_proto::dynamic::DynamicModule;
use tracing::{info, warn};

use crate::domain;
use crate::service::Service;

impl Service<'_> {
    /// The board's `approved` heard: the module that asked it is approved.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn decided(&self, approved: &DecisionApproved) -> Result<(), ModuleError> {
        let waiting = self
            .typed()
            .all::<DynamicModule>(None)
            .await?
            .into_iter()
            .find(|item| {
                item.data
                    .asked
                    .as_ref()
                    .is_some_and(|a| a.decision == approved.decision)
            });
        let Some(item) = waiting else {
            return Ok(());
        };
        let changed = self
            .change(&item.document.id, |module| {
                domain::approve(module.clone(), &approved.decision, &approved.approved_by)
            })
            .await?;
        if changed.is_some() {
            info!(module = %item.data.id, by = %approved.approved_by, "a dynamic module was approved");
            self.ports.events.changed(&item.data.id, false).await;
        }
        Ok(())
    }

    /// The board's `declined` heard: the module that asked it keeps that a
    /// person declined its files; what was approved before goes on.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn declined(&self, declined: &DecisionDeclined) -> Result<(), ModuleError> {
        let waiting = self
            .typed()
            .all::<DynamicModule>(None)
            .await?
            .into_iter()
            .find(|item| {
                item.data
                    .asked
                    .as_ref()
                    .is_some_and(|a| a.decision == declined.decision)
            });
        let Some(item) = waiting else {
            return Ok(());
        };
        let changed = self
            .change(&item.document.id, |module| {
                domain::decline(module.clone(), &declined.decision, &declined.declined_by)
            })
            .await?;
        if changed.is_some() {
            info!(module = %item.data.id, by = %declined.declined_by, "a dynamic module's files were declined");
        }
        Ok(())
    }

    /// After the module starts: each decision read again from its task, an
    /// approval or a refusal made meanwhile kept, and the task's older decisions
    /// that still wait withdrawn. A board that does not answer is logged: its
    /// event may still come.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn recheck(&self) -> Result<(), ModuleError> {
        for item in self.typed().all::<DynamicModule>(None).await? {
            let Some(asked) = item.data.asked.clone() else {
                continue;
            };
            let id = &item.data.id;
            match self
                .ports
                .board
                .thread(&domain::asker(id), &asked.task)
                .await
            {
                Ok(thread) => {
                    if let Some(approved) =
                        pcx_approval::approved_in(&thread, &asked.task, &asked.decision)
                    {
                        self.decided(&approved).await?;
                    }
                    if let Some(declined) =
                        pcx_approval::declined_in(&thread, &asked.task, &asked.decision)
                    {
                        self.declined(&declined).await?;
                    }
                    for stale in domain::stale_in(&thread, id, &asked.decision) {
                        let why = format!("Dynamic module {id} changed since this was asked.");
                        self.withdraw(id, &stale, why).await;
                    }
                }
                Err(e) => warn!(module = %item.data.id, "its decision was not read again: {e}"),
            }
        }
        Ok(())
    }
}
