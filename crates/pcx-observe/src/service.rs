// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Observe's service (ARCHITECTURE section 8, operations, W30): each
//! request checked, its environment's API asked, and its answer decoded
//! (the canonical proto3 JSON mapping of the API's mirror messages) and
//! read as observe's messages; a Nomad environment's in `nomad`. Between
//! the handlers and the ports.

use chrono::Duration;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::observe::{
    AlertList, AlertsRequest, AmAlert, Cluster, ClusterRequest, Environment, KubeList, LogStreams,
    LogsRequest, LokiAnswer, MetricSeries, MetricsRequest, ObserveConfig, PodLogRequest,
    PodLogTail, PromAnswer,
};
use serde::de::DeserializeOwned;

use crate::domain::{self, LOG_LINES, LOG_SINCE, Refused, TAIL_LINES};
use crate::ports::{Api, Apis, Clock};

/// Most bytes of a pod's log kept.
const LOG_BYTES: usize = 64 * 1024;

fn refused(rule: Refused) -> ModuleError {
    let kind = match rule {
        Refused::UnknownEnvironment(_) | Refused::NoAllocation(_) => ModuleErrorKind::NotFound,
        Refused::NoSource { .. } | Refused::Cluster { .. } => ModuleErrorKind::FailedPrecondition,
        Refused::Setting { .. }
        | Refused::Name(_)
        | Refused::NomadName(_)
        | Refused::AllocationId
        | Refused::Allocations { .. }
        | Refused::Tasks(_)
        | Refused::Empty(_) => ModuleErrorKind::InvalidArgument,
    };
    let field = match &rule {
        Refused::Setting { field, .. } => Some(field.clone()),
        _ => None,
    };
    let error = ModuleError::new(kind, rule.to_string());
    match field {
        Some(field) => error.with_field(field),
        None => error,
    }
}

/// `config` checked before the module starts.
///
/// # Errors
/// `InvalidArgument` naming the first wrong setting.
pub fn checked(config: ObserveConfig) -> Result<ObserveConfig, ModuleError> {
    domain::check(&config).map_err(refused)?;
    Ok(config)
}

/// An answer of `api` that is not what it sends.
fn undecoded(api: &str, error: &serde_json::Error) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::Unavailable,
        format!("{api}'s answer is not what it sends: {error}"),
    )
}

/// `body` decoded as `T`, an API's answer.
fn decode<T: DeserializeOwned>(api: &str, body: &[u8]) -> Result<T, ModuleError> {
    serde_json::from_slice(body).map_err(|e| undecoded(api, &e))
}

/// An API's refusal: its status was no success.
fn failed(api: &str, body: &[u8]) -> ModuleError {
    let text = String::from_utf8_lossy(body);
    let head: String = text.chars().take(300).collect();
    ModuleError::new(
        ModuleErrorKind::FailedPrecondition,
        format!("{api} refused: {head}"),
    )
}

/// Observe over its APIs, as `config` sets them.
pub struct Service<'a> {
    config: &'a ObserveConfig,
    apis: &'a dyn Apis,
    clock: &'a dyn Clock,
}

impl<'a> Service<'a> {
    /// The service.
    pub fn new(config: &'a ObserveConfig, apis: &'a dyn Apis, clock: &'a dyn Clock) -> Self {
        Self {
            config,
            apis,
            clock,
        }
    }

    fn environment(&self, name: &str) -> Result<&'a Environment, ModuleError> {
        domain::environment(self.config, name).map_err(refused)
    }

    /// A PromQL query at now, or over a range.
    ///
    /// # Errors
    /// An unknown environment, or one without Prometheus; an empty query;
    /// a query Prometheus refuses (`InvalidArgument`); Prometheus failed.
    pub async fn metrics(&self, request: MetricsRequest) -> Result<MetricSeries, ModuleError> {
        let env = self.environment(&request.environment)?;
        let base = domain::source(env, &env.prometheus, "Prometheus").map_err(refused)?;
        let query = domain::said("query", &request.query)
            .map_err(refused)?
            .to_string();
        let now = self.clock.now().timestamp();
        let (url, params) = if request.start == 0 {
            let at = vec![("query", query), ("time", now.to_string())];
            (format!("{base}/api/v1/query"), at)
        } else {
            let end = if request.end == 0 { now } else { request.end };
            let step = domain::step(request.start, end, request.step);
            let range = vec![
                ("query", query),
                ("start", request.start.to_string()),
                ("end", end.to_string()),
                ("step", step.to_string()),
            ];
            (format!("{base}/api/v1/query_range"), range)
        };
        let (ok, body) = self.apis.get(env, Api::Monitoring, &url, &params).await?;
        // Prometheus answers a refused query with its error in the same JSON;
        // anything else that is no success (a 401 of a proxy) is a refusal.
        let answer: PromAnswer = match decode("Prometheus", &body) {
            Ok(answer) => answer,
            Err(_) if !ok => return Err(failed("Prometheus", &body)),
            Err(e) => return Err(e),
        };
        if answer.status != "success" {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("Prometheus refused the query: {}", answer.error),
            ));
        }
        Ok(domain::series(answer))
    }

    /// A LogQL query over the last while, the newest lines first.
    ///
    /// # Errors
    /// As [`Service::metrics`], for Loki.
    pub async fn logs(&self, request: LogsRequest) -> Result<LogStreams, ModuleError> {
        let env = self.environment(&request.environment)?;
        let base = domain::source(env, &env.loki, "Loki").map_err(refused)?;
        let query = domain::said("query", &request.query)
            .map_err(refused)?
            .to_string();
        let since = if request.since == 0 {
            LOG_SINCE
        } else {
            request.since
        };
        let end = self.clock.now();
        let start = end - Duration::seconds(i64::from(since));
        let nanos = |t: chrono::DateTime<chrono::Utc>| {
            format!("{}{:09}", t.timestamp(), t.timestamp_subsec_nanos())
        };
        let params = vec![
            ("query", query),
            ("start", nanos(start)),
            ("end", nanos(end)),
            (
                "limit",
                domain::within(request.limit, LOG_LINES).to_string(),
            ),
            ("direction", "backward".to_string()),
        ];
        let url = format!("{base}/loki/api/v1/query_range");
        let (ok, body) = self.apis.get(env, Api::Monitoring, &url, &params).await?;
        if !ok {
            return Err(failed("Loki", &body));
        }
        let answer: LokiAnswer = decode("Loki", &body)?;
        Ok(LogStreams {
            streams: domain::streams(answer),
        })
    }

    /// The alerts Alertmanager has.
    ///
    /// # Errors
    /// As [`Service::metrics`], for Alertmanager.
    pub async fn alerts(&self, request: AlertsRequest) -> Result<AlertList, ModuleError> {
        let env = self.environment(&request.environment)?;
        let base = domain::source(env, &env.alertmanager, "Alertmanager").map_err(refused)?;
        let url = format!("{base}/api/v2/alerts");
        let (ok, body) = self.apis.get(env, Api::Monitoring, &url, &[]).await?;
        if !ok {
            return Err(failed("Alertmanager", &body));
        }
        let alerts: Vec<AmAlert> = decode("Alertmanager", &body)?;
        Ok(AlertList {
            alerts: alerts.into_iter().map(domain::alert).collect(),
        })
    }

    async fn kube_list(&self, env: &Environment, path: &str) -> Result<KubeList, ModuleError> {
        let base = domain::kubernetes(env).map_err(refused)?;
        let url = format!("{base}{path}");
        let (ok, body) = self.apis.get(env, Api::Kube, &url, &[]).await?;
        if !ok {
            return Err(failed("Kubernetes", &body));
        }
        decode("Kubernetes", &body)
    }

    /// A namespace's pods, deployments, jobs and warnings of the last hour.
    ///
    /// # Errors
    /// An unknown environment, or one without a cluster; a namespace that is
    /// no Kubernetes name; the cluster refused or failed.
    pub async fn cluster(&self, request: ClusterRequest) -> Result<Cluster, ModuleError> {
        let env = self.environment(&request.environment)?;
        let ns = request.namespace.as_str();
        if !domain::kube_name(ns) {
            return Err(refused(Refused::Name("namespace")));
        }
        let pods = self
            .kube_list(env, &format!("/api/v1/namespaces/{ns}/pods"))
            .await?;
        let deployments = self
            .kube_list(env, &format!("/apis/apps/v1/namespaces/{ns}/deployments"))
            .await?;
        let jobs = self
            .kube_list(env, &format!("/apis/batch/v1/namespaces/{ns}/jobs"))
            .await?;
        let events = self
            .kube_list(env, &format!("/api/v1/namespaces/{ns}/events"))
            .await?;
        let since = (self.clock.now() - Duration::hours(1))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        Ok(Cluster {
            pods: pods.items.iter().map(domain::pod).collect(),
            deployments: deployments.items.iter().map(domain::deployment).collect(),
            jobs: jobs.items.iter().map(domain::job).collect(),
            events: domain::warnings(&events.items, &since),
        })
    }

    /// The last lines of a pod's log.
    ///
    /// # Errors
    /// As [`Service::cluster`]; a pod or container that is no Kubernetes
    /// name.
    pub async fn pod_log(&self, request: PodLogRequest) -> Result<PodLogTail, ModuleError> {
        let env = self.environment(&request.environment)?;
        let base = domain::kubernetes(env).map_err(refused)?;
        if !domain::kube_name(&request.namespace) {
            return Err(refused(Refused::Name("namespace")));
        }
        if !domain::kube_name(&request.pod) {
            return Err(refused(Refused::Name("pod")));
        }
        let mut params = vec![
            (
                "tailLines",
                domain::within(request.tail, TAIL_LINES).to_string(),
            ),
            ("previous", request.previous.to_string()),
        ];
        if !request.container.is_empty() {
            if !domain::kube_name(&request.container) {
                return Err(refused(Refused::Name("container")));
            }
            params.push(("container", request.container.clone()));
        }
        let url = format!(
            "{base}/api/v1/namespaces/{}/pods/{}/log",
            request.namespace, request.pod
        );
        let (ok, body) = self.apis.get(env, Api::Kube, &url, &params).await?;
        if !ok {
            return Err(failed("Kubernetes", &body));
        }
        let from = body.len().saturating_sub(LOG_BYTES);
        Ok(PodLogTail {
            text: String::from_utf8_lossy(&body[from..]).into_owned(),
        })
    }
}

mod nomad;

#[cfg(test)]
mod tests;
