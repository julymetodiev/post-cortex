// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `ops` module (ARCHITECTURE section 8, operations, W31): a running
//! system changed only through typed actions (restart a deployment, delete
//! a pod a controller makes again, delete failed jobs, scale within bounds,
//! grow a volume claim), each recorded as a decision with risk operations
//! on its task. The team's trust in operations approves it at once (an
//! action and an environment it names), else a person does (the app,
//! Telegram); the action runs then, its target checked again, held by
//! brakes, with ops' own account whose credentials no agent reads. The
//! domain holds the rules; the service applies them over the cluster, the
//! board and the channel (its ports); the handlers only map.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod paths;
mod ports;
mod service;

pub use module::OpsModule;
