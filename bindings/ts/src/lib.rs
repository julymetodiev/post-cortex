// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `@post-cortex/module`'s addon: the Rust core of a pcx module for Deno and Node
//! (docs/modules/dynamic.md, the bindings' API). Every future runs on the
//! core's runtime; JS awaits it on its own event loop. Only bytes and plain
//! values cross; `index.js` is the layer a module's author writes against.

use napi::bindgen_prelude::{Buffer, Error, Result};
use napi_derive::napi;
use pcx_module_core::ModuleError;
use pcx_module_core::handoff::{self, Declaration, Declared, Emitted, Handoff};

/// A failure of the core or of what JS declared: thrown.
fn broken(e: impl std::fmt::Display) -> Error {
    Error::from_reason(e.to_string())
}

/// A failed call, as a value: `index.js` throws it as its `ModuleError`.
#[napi(object)]
pub struct Failure {
    /// Its kind (`not_found`, `denied`, ...).
    pub kind: String,
    /// What went wrong.
    pub message: String,
}

impl From<ModuleError> for Failure {
    fn from(failure: ModuleError) -> Self {
        Self {
            kind: failure.kind.name().to_string(),
            message: failure.message,
        }
    }
}

/// What a call of another module answered: its bytes, or its failure.
#[napi(object)]
pub struct Answered {
    /// The answer's bytes.
    pub body: Option<Buffer>,
    /// Its failure.
    pub failure: Option<Failure>,
}

/// What emitting an event gave: its id, or its failure.
#[napi(object)]
pub struct Published {
    /// The event's id.
    pub id: Option<String>,
    /// Its failure.
    pub failure: Option<Failure>,
}

/// An RPC as JS declares it.
#[napi(object)]
pub struct Rpc {
    /// Its method's path.
    pub method: String,
    /// Its tool's name (empty: not a tool).
    pub tool: String,
    /// `read`, `write` or `external`.
    pub effect: String,
    /// `both`, `model` or `app`.
    pub visibility: String,
}

/// An event as JS declares it.
#[napi(object)]
pub struct Event {
    /// Its name, after the module's id.
    pub name: String,
    /// Its payload's message type.
    pub message: String,
    /// What it means.
    pub description: String,
}

/// What `Module` declares.
#[napi(object)]
pub struct Declaring {
    /// Its id.
    pub id: String,
    /// Its version.
    pub version: String,
    /// What it does.
    pub description: String,
    /// Its gRPC services.
    pub services: Vec<String>,
    /// The `FileDescriptorSet` of its proto.
    pub descriptors: Buffer,
    /// Its RPCs.
    pub rpcs: Vec<Rpc>,
    /// The modules it uses.
    pub uses: Vec<String>,
    /// The events it emits.
    pub emits: Vec<Event>,
    /// The events it follows.
    pub follows: Vec<String>,
    /// Its configuration's message type (empty: none).
    pub config_type: String,
}

impl Declaring {
    fn declaration(self) -> Result<Declaration> {
        let rpcs = self
            .rpcs
            .into_iter()
            .map(|r| {
                Ok(Declared {
                    method: r.method,
                    tool: r.tool,
                    effect: handoff::effect(&r.effect).map_err(broken)?,
                    visibility: handoff::visibility(&r.visibility).map_err(broken)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Declaration {
            id: self.id,
            version: self.version,
            description: self.description,
            services: self.services,
            descriptors: self.descriptors.to_vec(),
            rpcs,
            uses: self.uses,
            emits: self
                .emits
                .into_iter()
                .map(|e| Emitted {
                    name: e.name,
                    message: e.message,
                    description: e.description,
                })
                .collect(),
            follows: self.follows,
            config_type: self.config_type,
        })
    }
}

/// A call handed to JS.
#[napi(object)]
pub struct Call {
    /// Its id, for the answer (below 2^53).
    pub id: i64,
    /// Its method's path.
    pub method: String,
    /// The request's bytes.
    pub body: Buffer,
    /// Who calls; none: unknown.
    pub author: Option<String>,
    /// The workspace it is for; none: the caller's default.
    pub workspace: Option<String>,
    /// The module that calls; none: a person or an agent.
    pub caller: Option<String>,
    /// Milliseconds left before its deadline.
    pub remaining: f64,
}

/// The configuration the host started the module with.
#[napi(object)]
pub struct Config {
    /// Its message type (empty: none).
    pub message: String,
    /// Its bytes.
    pub body: Buffer,
}

/// A call's id from JS.
fn id(id: i64) -> Result<u64> {
    u64::try_from(id).map_err(|_| broken(format!("{id} is no call's id")))
}

/// The core of one module process.
#[napi]
pub struct Core {
    handoff: Handoff,
}

#[napi]
impl Core {
    /// The core of the module `declaring` declares.
    #[napi(constructor)]
    pub fn new(declaring: Declaring) -> Result<Self> {
        let handoff = Handoff::new(declaring.declaration()?).map_err(broken)?;
        Ok(Self { handoff })
    }

    /// Serve on the host's socket until it stops the module.
    #[napi]
    pub async fn serve(&self) -> Result<()> {
        let handoff = self.handoff.clone();
        self.handoff
            .runtime()
            .spawn(async move { handoff.serve().await })
            .await
            .map_err(broken)?
            .map_err(broken)
    }

    /// The next call; null once the module stopped.
    #[napi]
    pub async fn next_call(&self) -> Result<Option<Call>> {
        let handoff = self.handoff.clone();
        let call = self
            .handoff
            .runtime()
            .spawn(async move { handoff.next_call().await })
            .await
            .map_err(broken)?;
        call.map(|c| {
            Ok(Call {
                id: i64::try_from(c.id).map_err(broken)?,
                method: c.method,
                body: c.body.into(),
                remaining: c.scope.remaining().as_secs_f64() * 1000.0,
                author: c.scope.author,
                workspace: c.scope.workspace_id,
                caller: c.scope.caller_module,
            })
        })
        .transpose()
    }

    /// Whether call `id` still waits for its answer (false: its caller
    /// gave up).
    #[napi]
    pub fn waits(&self, id: i64) -> Result<bool> {
        Ok(self.handoff.waits(self::id(id)?))
    }

    /// Answer call `id` with the response's bytes; false when nothing
    /// waits for it any more (its deadline passed).
    #[napi]
    pub fn complete(&self, id: i64, body: Buffer) -> Result<bool> {
        Ok(self.handoff.complete(self::id(id)?, body.to_vec()).is_ok())
    }

    /// Fail call `id` with a failure of `kind`; false when nothing waits
    /// for it any more.
    #[napi]
    pub fn fail(&self, id: i64, kind: String, message: String) -> Result<bool> {
        Ok(self
            .handoff
            .fail(self::id(id)?, handoff::failure(&kind, message))
            .is_ok())
    }

    /// Call `path` of a module this one uses, in call `id`'s scope.
    #[napi]
    pub async fn host_call(&self, id: i64, path: String, body: Buffer) -> Result<Answered> {
        let (id, body) = (self::id(id)?, body.to_vec());
        let handoff = self.handoff.clone();
        let answer = self
            .handoff
            .runtime()
            .spawn(async move { handoff.host_call(id, &path, body).await })
            .await
            .map_err(broken)?;
        Ok(match answer {
            Ok(body) => Answered {
                body: Some(body.into()),
                failure: None,
            },
            Err(failure) => Answered {
                body: None,
                failure: Some(failure.into()),
            },
        })
    }

    /// Emit the event `name`, a `message` of `body`, in call `id`'s scope:
    /// its id.
    #[napi]
    pub async fn publish(
        &self,
        id: i64,
        name: String,
        message: String,
        body: Buffer,
    ) -> Result<Published> {
        let (id, body) = (self::id(id)?, body.to_vec());
        let handoff = self.handoff.clone();
        let published = self
            .handoff
            .runtime()
            .spawn(async move { handoff.publish(id, &name, &message, body).await })
            .await
            .map_err(broken)?;
        Ok(match published {
            Ok(id) => Published {
                id: Some(id),
                failure: None,
            },
            Err(failure) => Published {
                id: None,
                failure: Some(failure.into()),
            },
        })
    }

    /// The configuration the host started the module with.
    #[napi]
    pub fn config(&self) -> Result<Config> {
        let (message, body) = self.handoff.config().map_err(broken)?;
        Ok(Config {
            message,
            body: body.into(),
        })
    }
}
