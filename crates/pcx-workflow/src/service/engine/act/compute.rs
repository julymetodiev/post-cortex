// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A data block's attempt (docs/modules/workflow.md, data blocks): run on
//! a blocking thread, so a list of many items holds no thread of the
//! engine's, within the block's timeout.

use std::sync::Arc;

use pcx_expr::{Value as Expr, Values};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::workflow::Step;

use crate::domain::{Act, Data};
use crate::ports::Kept;
use crate::service::engine::act::{Actor, Ended};
use crate::service::engine::written;

impl Actor {
    /// `data` run over `values` for `act`, its step written when it starts
    /// and when it ends.
    ///
    /// # Errors
    /// Memory failed (the block's failure is its step's).
    pub async fn compute(
        &self,
        existing: Option<Kept<Step>>,
        act: &Act,
        data: Arc<Data>,
        values: Values,
        fail_invalid: bool,
    ) -> Result<Kept<Step>, ModuleError> {
        let started = self.begin(existing, act, written(&Expr::Null)?).await?;
        let running = tokio::task::spawn_blocking(move || data.run(&values, fail_invalid));
        let ran = self
            .timed(act, async move {
                running.await.map_err(|e| {
                    ModuleError::new(
                        ModuleErrorKind::Internal,
                        format!("a data block's thread ended: {e}"),
                    )
                })
            })
            .await;
        let end = match ran {
            Ok(Ok(gave)) => Ended::Succeeded(Expr::map(gave.output), gave.port.to_string()),
            Ok(Err(failed)) | Err(failed) => Ended::Failed(failed),
        };
        self.write(Some(started), act, None, Some(end), None).await
    }
}
