// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `observe` module (ARCHITECTURE section 8, operations, W30): reading
//! a running system, never changing it. Per environment, its metrics
//! (Prometheus), logs (Loki), alerts (Alertmanager) and cluster
//! (Kubernetes, with a ServiceAccount that may only read), each an agent's
//! tool. Credentials are files read through the module's sandbox, never
//! settings, prompts or answers. The domain reads the APIs' answers into
//! typed messages; the service asks the APIs (its port); the handlers only
//! map.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod ports;
mod service;

pub use module::ObserveModule;

#[cfg(test)]
mod tests;
