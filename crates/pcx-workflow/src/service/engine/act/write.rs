// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An attempt's step written (docs/modules/workflow.md, the engine): when
//! it starts, when it waits and when it ends, created or changed (a step
//! written before a crash is the same document); an output above 48 KiB
//! kept as a blob of the workspace.

use pcx_expr::Value as Expr;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::pb::Attachment;
use post_cortex_proto::workflow::{Failure, Step, StepStatus, Value};
use prost::Message;

use crate::domain::{self, Act, Work, failure};
use crate::ports::Kept;
use crate::service::engine::Written;
use crate::service::engine::act::{Actor, Ended};
use crate::service::engine::written;

/// An output's bytes above which it is kept as a blob.
const INLINE: usize = 48 * 1024;

/// The media type of a message blob.
const PROTOBUF: &str = "application/x-protobuf";

impl Actor {
    /// The waiting step of `act` closed with `end` (its time came, its
    /// event or decision was heard, its called run ended).
    ///
    /// # Errors
    /// Memory failed.
    pub async fn close(
        &self,
        existing: Option<Kept<Step>>,
        act: &Act,
        end: Result<Expr, Failure>,
    ) -> Result<Kept<Step>, ModuleError> {
        let end = match (end, &act.work) {
            // An approval's output is its decision, approved.
            (Ok(Expr::String(approved_by)), Work::Await(domain::Awaited::Decision { .. })) => {
                let decided = existing
                    .as_ref()
                    .and_then(|s| s.data.input.as_ref())
                    .ok_or_else(|| {
                        ModuleError::new(
                            ModuleErrorKind::Internal,
                            "an approval's step has no decision",
                        )
                    })?;
                match domain::read_value(decided, self.catalog.messages()) {
                    Ok(Expr::Message(mut decision)) => {
                        decision
                            .try_set_field_by_name(
                                "approved_by",
                                prost_reflect::Value::String(approved_by.to_string()),
                            )
                            .map_err(|e| {
                                ModuleError::new(ModuleErrorKind::Internal, e.to_string())
                            })?;
                        Ended::Succeeded(Expr::Message(decision), "next".into())
                    }
                    _ => Ended::Failed(failure("internal", "an approval's decision does not read")),
                }
            }
            (Ok(output), _) => Ended::Succeeded(output, "next".into()),
            (Err(failed), _) => Ended::Failed(failed),
        };
        self.write(existing, act, None, Some(end), None).await
    }

    /// `act`'s step started: written as running with `input`, unless it
    /// was (a restart).
    pub async fn begin(
        &self,
        existing: Option<Kept<Step>>,
        act: &Act,
        input: Value,
    ) -> Result<Kept<Step>, ModuleError> {
        match existing {
            Some(started) => Ok(started),
            None => self.write(None, act, Some(input), None, None).await,
        }
    }

    /// `act`'s step waiting, until `until` or for `waits_for`.
    pub async fn wait(
        &self,
        existing: Option<Kept<Step>>,
        act: &Act,
        until: Option<chrono::DateTime<chrono::Utc>>,
        waits_for: String,
    ) -> Result<Kept<Step>, ModuleError> {
        self.write(existing, act, None, None, Some((until, waits_for)))
            .await
    }

    /// `act`'s step written: started (`input`), ended (`end`), or waiting
    /// (`waiting`); created when there is none, else changed.
    pub async fn write(
        &self,
        existing: Option<Kept<Step>>,
        act: &Act,
        input: Option<Value>,
        end: Option<Ended>,
        waiting: Option<(Option<chrono::DateTime<chrono::Utc>>, String)>,
    ) -> Result<Kept<Step>, ModuleError> {
        let now = domain::timestamp(self.engine.ports.clock.now());
        let mut step = existing
            .as_ref()
            .map(|s| s.data.clone())
            .unwrap_or_else(|| Step {
                run: self.run.clone(),
                at: act.at.clone(),
                block: act.block.clone(),
                attempt: act.attempt,
                started_at: Some(now),
                ..Default::default()
            });
        if input.is_some() {
            step.input = input;
        }
        step.status = StepStatus::Running.into();
        if let Some((until, waits_for)) = waiting {
            step.status = StepStatus::Waiting.into();
            step.waits_until = until.map(domain::timestamp);
            step.waits_for = waits_for;
        }
        match end {
            None => {}
            Some(Ended::Succeeded(output, port)) => {
                let (output, blob) = self.output(&output).await?;
                step.status = StepStatus::Succeeded.into();
                step.output = output;
                step.output_blob = blob;
                step.port = port;
                step.ended_at = Some(now);
            }
            Some(Ended::Failed(failed)) => {
                step.status = StepStatus::Failed.into();
                step.failure = Some(failed);
                step.ended_at = Some(now);
            }
        }
        let runs = &self.engine.ports.runs;
        let written = match existing {
            Some(kept) => runs.change_step(&kept.id, kept.version, &step).await,
            None => runs.put_step(&step).await,
        }?;
        self.engine
            .wrote(Written::Step(Box::new(written.data.clone())));
        Ok(written)
    }

    /// `output` as the step keeps it: inline, or a blob above 48 KiB.
    pub async fn output(
        &self,
        output: &Expr,
    ) -> Result<(Option<Value>, Option<Attachment>), ModuleError> {
        let value = written(output)?;
        let size = value.encoded_len();
        if size <= INLINE {
            return Ok((Some(value), None));
        }
        let sha256 = self.engine.ports.blobs.put(value.encode_to_vec()).await?;
        Ok((
            None,
            Some(Attachment {
                sha256,
                name: "output".into(),
                media_type: PROTOBUF.into(),
                size: size as u64,
                ..Default::default()
            }),
        ))
    }
}
