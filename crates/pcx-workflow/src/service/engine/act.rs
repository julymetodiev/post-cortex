// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One attempt of a block done through the ports, and its step written
//! (docs/modules/workflow.md, the engine): written when it starts and when
//! it ends, so a crash between leaves the attempt to do again. A tool whose
//! effect is `external` carries the key `<run>:<graph>/<block>:<attempt>`;
//! an output above 48 KiB is a blob of the workspace.

mod board;
mod compute;
mod turn;
mod write;

pub use board::{ADD_TASK, DECIDE, GET_TASK, approved, declined};

use std::sync::Arc;
use std::time::Duration;

use pcx_expr::Value as Expr;
use pcx_module_api::{Effect, ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::module::BusEvent;
use post_cortex_proto::workflow::{
    Block, Failure, Fired, Graph, StateEntry, Step, Workflow, block::Kind,
};
use prost_reflect::DynamicMessage;

use pcx_expr::schema::Field;

use crate::domain::{self, Act, Catalog, Work, failure, kinds};
use crate::ports::Kept;
use crate::service::engine::{Engine, Input, Start, written, written_map};

/// A block's attempt's timeout when it names none.
const TIMEOUT: Duration = Duration::from_secs(600);

/// What a run's attempts are done with.
pub struct Actor {
    /// The engine.
    pub engine: Arc<Engine>,
    /// The workflow at the run's version.
    pub workflow: Workflow,
    /// What it reaches.
    pub catalog: Catalog,
    /// The scope it acts in.
    pub scope: Scope,
    /// The run's id.
    pub run: String,
}

/// How a step ends.
pub enum Ended {
    /// With an output, by a port.
    Succeeded(Expr, String),
    /// With a failure.
    Failed(Failure),
}

impl Actor {
    /// Do `act`, `existing` its step when it was written already (a
    /// restart): the step as it stands after.
    ///
    /// # Errors
    /// Memory failed (a block's failure is its step's, not an error).
    pub async fn act(
        &self,
        existing: Option<Kept<Step>>,
        act: Act,
    ) -> Result<Kept<Step>, ModuleError> {
        match act.work {
            Work::Done {
                ref output,
                ref port,
            } => {
                let end = Ended::Succeeded(output.clone(), port.clone());
                self.write(existing, &act, None, Some(end), None).await
            }
            Work::Data {
                ref data,
                ref values,
                fail_invalid,
            } => {
                self.compute(
                    existing,
                    &act,
                    Arc::clone(data),
                    values.clone(),
                    fail_invalid,
                )
                .await
            }
            Work::Fail { ref failure } => {
                self.write(
                    existing,
                    &act,
                    None,
                    Some(Ended::Failed(failure.clone())),
                    None,
                )
                .await
            }
            Work::Open { ref input } => match existing {
                Some(open) => Ok(open),
                None => {
                    self.write(None, &act, Some(written(input)?), None, None)
                        .await
                }
            },
            Work::Close { ref output } => {
                let end = Ended::Succeeded(output.clone(), "next".into());
                self.write(existing, &act, None, Some(end), None).await
            }
            Work::Tool {
                ref path,
                effect,
                ref request,
                ref fixture,
            } => {
                let started = self
                    .begin(existing, &act, written(&Expr::Message(request.clone()))?)
                    .await?;
                if self.engine.dry() && effect != Effect::Read {
                    let end = Ended::Succeeded(Expr::Message(fixture.clone()), "next".into());
                    return self.write(Some(started), &act, None, Some(end), None).await;
                }
                let key = (effect == Effect::External)
                    .then(|| format!("{}:{}/{}:{}", self.run, act.at, act.block, act.attempt));
                let called = self
                    .timed(
                        &act,
                        self.engine
                            .ports
                            .tools
                            .call(&self.scope, path, request, key.as_deref()),
                    )
                    .await;
                let end = match called {
                    Ok(response) => Ended::Succeeded(Expr::Message(response), "next".into()),
                    Err(failed) => Ended::Failed(failed),
                };
                self.write(Some(started), &act, None, Some(end), None).await
            }
            Work::Model {
                ref model,
                ref system,
                ref prompt,
                ref output,
            } => {
                let started = self
                    .begin(existing, &act, written(&Expr::string(prompt.as_str()))?)
                    .await?;
                if self.unreal() {
                    let end = Ended::Succeeded(
                        Expr::map(pcx_expr::schema::example(output)),
                        "next".into(),
                    );
                    return self.write(Some(started), &act, None, Some(end), None).await;
                }
                let schema = pcx_expr::schema::json_schema(output);
                let asked = self
                    .timed(
                        &act,
                        self.engine.ports.models.answer(
                            &self.scope,
                            model,
                            system,
                            prompt,
                            &schema,
                        ),
                    )
                    .await;
                let end = match asked {
                    Ok(answer) => {
                        self.spent(answer.tokens).await?;
                        read(output, &answer.answer)
                    }
                    Err(failed) => Ended::Failed(failed),
                };
                self.write(Some(started), &act, None, Some(end), None).await
            }
            Work::Agent {
                ref member,
                ref prompt,
                ref output,
            } => {
                let started = self
                    .begin(existing, &act, written(&Expr::string(prompt.as_str()))?)
                    .await?;
                if self.unreal() {
                    let end = Ended::Succeeded(
                        Expr::map(pcx_expr::schema::example(output)),
                        "next".into(),
                    );
                    return self.write(Some(started), &act, None, Some(end), None).await;
                }
                let end = match self.turn(&act, member, prompt, output).await {
                    Ok(end) => end,
                    Err(failed) => Ended::Failed(failed),
                };
                self.write(Some(started), &act, None, Some(end), None).await
            }
            Work::Call {
                ref workflow,
                ref input,
            } => {
                let parent = self.engine.latest(&self.run).await?;
                let fired = Fired {
                    author: self.scope.author.clone().unwrap_or_default(),
                    ..Default::default()
                };
                let started = self
                    .engine
                    .start(
                        workflow,
                        &Start::Call,
                        fired,
                        &Input::Map(written_map(input)?),
                        Some(&parent),
                    )
                    .await;
                match started {
                    Ok(child) => self.wait(existing, &act, None, child.id).await,
                    Err(e) => {
                        let end = Ended::Failed(failure(e.kind.name(), &e.message));
                        self.write(existing, &act, None, Some(end), None).await
                    }
                }
            }
            // A dry run does not wait.
            Work::Sleep { .. } | Work::Event { .. } if self.engine.dry() => {
                let end = Ended::Succeeded(Expr::Null, "next".into());
                self.write(existing, &act, None, Some(end), None).await
            }
            Work::Sleep { until } => self.wait(existing, &act, Some(until), String::new()).await,
            Work::Event { until, .. } => self.wait(existing, &act, until, String::new()).await,
            Work::Approval {
                ref question,
                risk,
                ref task,
                person,
            } => {
                let started = self
                    .begin(existing, &act, written(&Expr::string(question.as_str()))?)
                    .await?;
                if self.engine.dry() {
                    // A dry run's approval is approved, by no one.
                    let decision = post_cortex_proto::board::Decision {
                        what: question.clone(),
                        risk,
                        approved_by: Some("dry run".into()),
                        ..Default::default()
                    };
                    let end = Ended::Succeeded(self.decision(&decision)?, "next".into());
                    return self.write(Some(started), &act, None, Some(end), None).await;
                }
                match self.decide(question, risk, person, task.as_deref()).await {
                    Ok(record) => match record.decision.as_ref() {
                        Some(decision) if decision.approved_by.is_some() => {
                            let output = self.decision(decision)?;
                            self.write(
                                Some(started),
                                &act,
                                None,
                                Some(Ended::Succeeded(output, "next".into())),
                                None,
                            )
                            .await
                        }
                        Some(decision) => {
                            // The decision waits; its step keeps it, which it gives
                            // once approved.
                            let input = written(&self.decision(decision)?)?;
                            let waiting = Some((None, record.id.clone()));
                            self.write(Some(started), &act, Some(input), None, waiting)
                                .await
                        }
                        None => {
                            let end = Ended::Failed(failure(
                                "internal",
                                "the board answered no decision",
                            ));
                            self.write(Some(started), &act, None, Some(end), None).await
                        }
                    },
                    Err(failed) => {
                        self.write(Some(started), &act, None, Some(Ended::Failed(failed)), None)
                            .await
                    }
                }
            }
            Work::StateGet { ref key } => {
                let found = self
                    .engine
                    .ports
                    .state
                    .get(&self.workflow.name, key)
                    .await?;
                let value = match found.and_then(|e| e.data.value) {
                    Some(value) => match domain::read_value(&value, self.catalog.messages()) {
                        Ok(value) => Ended::Succeeded(value, "next".into()),
                        Err(why) => Ended::Failed(failure("internal", &why)),
                    },
                    None => Ended::Succeeded(Expr::Null, "next".into()),
                };
                self.write(existing, &act, None, Some(value), None).await
            }
            Work::StatePut { ref key, ref value } => {
                let read = self
                    .engine
                    .ports
                    .state
                    .get(&self.workflow.name, key)
                    .await?;
                let entry = StateEntry {
                    workflow: self.workflow.name.clone(),
                    key: key.clone(),
                    value: Some(written(value)?),
                };
                let put = self
                    .engine
                    .ports
                    .state
                    .put(&entry, read.as_ref().map(|r| (r.id.as_str(), r.version)))
                    .await;
                let end = match put {
                    Ok(()) => Ended::Succeeded(Expr::Null, "next".into()),
                    Err(e) => Ended::Failed(failure(e.kind.name(), &e.message)),
                };
                self.write(existing, &act, None, Some(end), None).await
            }
            Work::StateDelete { ref key } => {
                if let Some(found) = self
                    .engine
                    .ports
                    .state
                    .get(&self.workflow.name, key)
                    .await?
                {
                    self.engine.ports.state.delete(&found.id).await?;
                }
                self.write(
                    existing,
                    &act,
                    None,
                    Some(Ended::Succeeded(Expr::Null, "next".into())),
                    None,
                )
                .await
            }
            Work::Await(_) => Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "a waiting step is settled by the driver, not done",
            )),
        }
    }

    /// `call` within `act`'s block's timeout: its failure as a step's.
    async fn timed<T>(
        &self,
        act: &Act,
        call: impl std::future::Future<Output = Result<T, ModuleError>>,
    ) -> Result<T, Failure> {
        let limit = block(self.workflow.graph.as_ref(), &act.block)
            .and_then(|b| b.timeout)
            .and_then(|t| Duration::try_from(t).ok())
            .unwrap_or(TIMEOUT);
        match tokio::time::timeout(limit, call).await {
            Ok(Ok(done)) => Ok(done),
            Ok(Err(e)) => Err(failure(e.kind.name(), &e.message)),
            Err(_) => Err(failure(
                kinds::TIMEOUT,
                &format!("it took more than {} s", limit.as_secs()),
            )),
        }
    }

    /// Whether a `model` or `agent` block answers its example: a dry run
    /// that did not ask for them.
    fn unreal(&self) -> bool {
        self.engine.settings.dry.is_some_and(|dry| !dry.models)
    }

    /// `tokens` added to what the run spent.
    async fn spent(&self, tokens: u64) -> Result<(), ModuleError> {
        self.engine
            .update_run(&self.run, |run| {
                let used = run.used.get_or_insert_with(Default::default);
                used.tokens += tokens;
            })
            .await
            .map(|_| ())
    }
}

/// `answer` read by `output`: its fields, or why it does not fit.
fn read(output: &[Field], answer: &serde_json::Value) -> Ended {
    match pcx_expr::schema::read_answer(output, answer) {
        Ok(fields) => Ended::Succeeded(Expr::map(fields), "next".into()),
        Err(why) => Ended::Failed(failure(kinds::ANSWER, &why)),
    }
}

/// `event` as CEL reads it: a `pcx.module.v1.BusEvent` of `catalog`.
///
/// # Errors
/// The catalog does not hold the message, or it does not read.
pub fn event(event: &BusEvent, catalog: &Catalog) -> Result<DynamicMessage, ModuleError> {
    domain::as_named(event, "pcx.module.v1.BusEvent", catalog.messages())
        .map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))
}

/// Block `id` of `graph`, its bodies' and branches' included.
pub fn block<'a>(graph: Option<&'a Graph>, id: &str) -> Option<&'a Block> {
    let graph = graph?;
    graph.blocks.iter().find_map(|b| {
        if b.id == id {
            return Some(b);
        }
        match &b.kind {
            Some(Kind::ForEach(each)) => block(each.body.as_ref(), id),
            Some(Kind::Parallel(p)) => p.branches.iter().find_map(|g| block(Some(g), id)),
            _ => None,
        }
    })
}
