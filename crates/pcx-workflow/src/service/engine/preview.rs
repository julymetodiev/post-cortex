// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A data block previewed (docs/modules/workflow.md, data blocks, the
//! editor): the workflow checked for the caller, the run's input read by
//! it, and the block run over the values the run's steps give it, on a
//! blocking thread; nothing is kept.

use pcx_expr::{Value as Expr, Values};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::workflow::{Fired, Preview, PreviewRequest, Workflow};

use crate::domain::{self, Previewing};
use crate::service::engine::Engine;
use crate::service::engine::written_map;

impl Engine {
    /// Block `request.block` of `workflow` previewed for `scope` over
    /// `request`'s input and steps: what it gives, or how it fails.
    ///
    /// # Errors
    /// `InvalidArgument` for a workflow that does not check, an input that
    /// does not read, a block that is no data block or that the steps do not
    /// reach; the host or memory failed.
    pub async fn preview(
        &self,
        scope: &Scope,
        workflow: Workflow,
        request: PreviewRequest,
    ) -> Result<Preview, ModuleError> {
        let (catalog, checked) = self.service().checked(scope, &workflow).await?;
        let invalid = |field: &str, why: String| {
            ModuleError::new(ModuleErrorKind::InvalidArgument, why).with_field(field)
        };
        let read = domain::read_input(
            &checked.signature.input,
            &request.input.unwrap_or_default(),
            catalog.messages(),
        )
        .map_err(|why| invalid("input", why))?;
        let input: Values = read
            .into_iter()
            .map(|(name, value)| (format!("input.{name}"), value))
            .collect();
        let fired = Fired {
            author: scope.author.clone().unwrap_or_default(),
            ..Fired::default()
        };
        let fired = Expr::Message(
            domain::as_message(&fired, catalog.messages())
                .map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))?,
        );
        let now = self.ports.clock.now();
        let (block, at, steps) = (request.block, request.at, request.steps);
        let ran = tokio::task::spawn_blocking(move || {
            Previewing {
                workflow: &workflow,
                checked: &checked,
                catalog: &catalog,
                input: &input,
                fired: &fired,
                now,
            }
            .preview(&block, &at, steps)
        })
        .await
        .map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::Internal,
                format!("a preview's thread ended: {e}"),
            )
        })?
        .map_err(|f| invalid("block", f.message))?;
        Ok(match ran {
            Ok(gave) => Preview {
                output: Some(written_map(&gave.output)?),
                port: gave.port.to_string(),
                failure: None,
                // A block reads at most a million items.
                items: gave.items as u64,
            },
            Err(failed) => Preview {
                output: None,
                port: String::new(),
                failure: Some(failed),
                items: 0,
            },
        })
    }
}
