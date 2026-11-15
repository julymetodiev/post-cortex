// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Observe's domain (ARCHITECTURE section 8, operations, W30): what its
//! settings may be, which names a request may carry into a URL, and how
//! the outside APIs' answers read as its typed messages, cut to what a
//! model can take. Pure: no network, no clock (time is a parameter).

use std::fmt;

use pbjson_types::value::Kind as JsonKind;
use pbjson_types::{ListValue, Value};
pub use pcx_kube::kube_name;
use pcx_kube::tls_server_name;
use post_cortex_proto::observe::{
    ActiveAlert, AmAlert, DeploymentState, Environment, EventState, JobState, KubeObject, LogLine,
    LogStream, LokiAnswer, MetricSeries, ObserveConfig, PodState, PromAnswer, Sample, Serie,
};

/// The module's id.
pub const MODULE_ID: &str = "observe";

/// Most series a query answers.
pub const MAX_SERIES: usize = 50;

/// Most samples of a series.
pub const MAX_SAMPLES: usize = 500;

/// Lines of a log query when the request names none, and the most.
pub const LOG_LINES: (u32, u32) = (100, 500);

/// Lines of a pod's log when the request names none, and the most.
pub const TAIL_LINES: (u32, u32) = (100, 1_000);

/// Seconds a log query looks back when the request names none.
pub const LOG_SINCE: u32 = 3_600;

/// Points a range query gives when the request names no step.
const POINTS: i64 = 60;

/// Most events a namespace answers.
const MAX_EVENTS: usize = 50;

/// A setting or a request observe refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A setting is wrong.
    Setting {
        /// The setting, as a path of field names.
        field: String,
        /// What is wrong.
        reason: String,
    },
    /// No environment of that name.
    UnknownEnvironment(String),
    /// The environment has no such source.
    NoSource {
        /// The environment.
        environment: String,
        /// The source: Prometheus, Loki, Alertmanager, Kubernetes, Nomad.
        source: &'static str,
    },
    /// The environment's cluster is of the other kind.
    Cluster {
        /// The environment.
        environment: String,
        /// What it is: Kubernetes or Nomad.
        is: &'static str,
        /// The tools that read such a cluster.
        tools: &'static str,
    },
    /// A name that is not a Kubernetes name.
    Name(&'static str),
    /// A name that is not a Nomad job's or task's.
    NomadName(&'static str),
    /// What should name an allocation is no id nor the start of one.
    AllocationId,
    /// No allocation's id starts so.
    NoAllocation(String),
    /// Several allocations' ids start so.
    Allocations {
        /// What was asked.
        start: String,
        /// How many start so.
        count: usize,
    },
    /// The allocation has several tasks and the request names none, or one
    /// it does not have.
    Tasks(Vec<String>),
    /// A field that must say something is empty.
    Empty(&'static str),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setting { field, reason } => write!(f, "{field}: {reason}"),
            Self::UnknownEnvironment(name) => write!(f, "no environment is named {name}"),
            Self::NoSource {
                environment,
                source,
            } => write!(f, "environment {environment} has no {source}"),
            Self::Cluster {
                environment,
                is,
                tools,
            } => write!(f, "environment {environment} is {is}: read it with {tools}"),
            Self::Name(what) => write!(
                f,
                "a {what} is a Kubernetes name: lowercase letters, digits, '-' and '.'"
            ),
            Self::NomadName(what) => write!(
                f,
                "a {what} is a Nomad name: letters, digits, '-', '_' and '.'"
            ),
            Self::AllocationId => f.write_str(
                "an allocation is named by its id, or by the first 8 characters or more of it",
            ),
            Self::NoAllocation(start) => write!(f, "no allocation's id starts with {start}"),
            Self::Allocations { start, count } => write!(
                f,
                "{count} allocations' ids start with {start}: give more of the id"
            ),
            Self::Tasks(tasks) => write!(
                f,
                "the allocation's tasks are {}: name one of them",
                tasks.join(", ")
            ),
            Self::Empty(what) => write!(f, "the {what} says nothing"),
        }
    }
}

impl std::error::Error for Refused {}

fn setting(field: String, reason: impl Into<String>) -> Refused {
    Refused::Setting {
        field,
        reason: reason.into(),
    }
}

/// Whether `config` may run: environments named once, each a cluster (a
/// Kubernetes API or a Nomad API, never both) with its token's file.
///
/// # Errors
/// The first setting that is wrong.
pub fn check(config: &ObserveConfig) -> Result<(), Refused> {
    let mut names: Vec<&str> = Vec::new();
    for (i, env) in config.environments.iter().enumerate() {
        let at = |field: &str| format!("environments.{i}.{field}");
        if !kube_name(&env.name) || env.name.len() > 64 || env.name.contains('.') {
            return Err(setting(at("name"), "is 1 to 64 of a-z, 0-9 and '-'"));
        }
        if names.contains(&env.name.as_str()) {
            return Err(setting(at("name"), format!("names {} twice", env.name)));
        }
        names.push(&env.name);
        if !env.kube_api.is_empty() && env.kube_token_file.trim().is_empty() {
            return Err(setting(
                at("kube_token_file"),
                "a cluster needs its token's file",
            ));
        }
        if let Err(reason) = tls_server_name(&env.kube_tls_server_name, &env.kube_ca_file) {
            return Err(setting(at("kube_tls_server_name"), reason));
        }
        if env.nomad_api.trim().is_empty() {
            continue;
        }
        if !env.kube_api.trim().is_empty() {
            return Err(setting(
                at("nomad_api"),
                "an environment names a Kubernetes API or a Nomad API, not both",
            ));
        }
        if let Err((field, reason)) = pcx_nomad::settings(
            &env.nomad_api,
            &env.nomad_token_file,
            &env.nomad_ca_file,
            &env.nomad_tls_server_name,
        ) {
            return Err(setting(at(field), reason));
        }
    }
    Ok(())
}

/// The tools that read a Kubernetes cluster, and those that read Nomad.
const TOOLS: (&str, &str) = (
    "cluster_get and pod_log",
    "jobs_get, alloc_log and node_get",
);

/// `environment`'s Kubernetes API.
///
/// # Errors
/// It is a Nomad environment, or has no cluster.
pub fn kubernetes(environment: &Environment) -> Result<&str, Refused> {
    if environment.kube_api.trim().is_empty() && !environment.nomad_api.trim().is_empty() {
        return Err(Refused::Cluster {
            environment: environment.name.clone(),
            is: "Nomad",
            tools: TOOLS.1,
        });
    }
    source(environment, &environment.kube_api, "Kubernetes")
}

/// `environment`'s Nomad API.
///
/// # Errors
/// It is a Kubernetes environment, or has no cluster.
pub fn nomad(environment: &Environment) -> Result<&str, Refused> {
    if environment.nomad_api.trim().is_empty() && !environment.kube_api.trim().is_empty() {
        return Err(Refused::Cluster {
            environment: environment.name.clone(),
            is: "Kubernetes",
            tools: TOOLS.0,
        });
    }
    source(environment, &environment.nomad_api, "Nomad")
}

/// The environment named `name`.
///
/// # Errors
/// No environment of that name.
pub fn environment<'a>(config: &'a ObserveConfig, name: &str) -> Result<&'a Environment, Refused> {
    config
        .environments
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| Refused::UnknownEnvironment(name.to_string()))
}

/// `url` of `environment`'s `source`, when it has one.
///
/// # Errors
/// It has none.
pub fn source<'a>(
    environment: &'a Environment,
    url: &'a str,
    source: &'static str,
) -> Result<&'a str, Refused> {
    match url.trim() {
        "" => Err(Refused::NoSource {
            environment: environment.name.clone(),
            source,
        }),
        url => Ok(url.trim_end_matches('/')),
    }
}

/// `text`, which must say something.
///
/// # Errors
/// It says nothing.
pub fn said<'a>(what: &'static str, text: &'a str) -> Result<&'a str, Refused> {
    match text.trim() {
        "" => Err(Refused::Empty(what)),
        text => Ok(text),
    }
}

/// A range query's step for `start` to `end`: the one asked, else one that
/// gives about 60 points (at least a second).
#[must_use]
pub fn step(start: i64, end: i64, asked: u32) -> i64 {
    if asked > 0 {
        i64::from(asked)
    } else {
        ((end - start) / POINTS).max(1)
    }
}

/// `asked` within `(default, most)`: the default when 0.
#[must_use]
pub fn within(asked: u32, (default, most): (u32, u32)) -> u32 {
    if asked == 0 { default } else { asked.min(most) }
}

/// A JSON number or numeric text as a number; none for anything else.
fn number(value: &Value) -> Option<f64> {
    match value.kind.as_ref()? {
        JsonKind::NumberValue(n) => Some(*n),
        JsonKind::StringValue(text) => match text.as_str() {
            "NaN" => Some(f64::NAN),
            "+Inf" => Some(f64::INFINITY),
            "-Inf" => Some(f64::NEG_INFINITY),
            // Text that is no number is no sample: it is left out.
            text => text.parse().ok(),
        },
        _ => None,
    }
}

/// Prometheus's `[seconds, "value"]` as a sample; none for another shape.
#[must_use]
pub fn sample(pair: &ListValue) -> Option<Sample> {
    let [time, value] = pair.values.as_slice() else {
        return None;
    };
    Some(Sample {
        time: number(time)?,
        value: number(value)?,
    })
}

/// Prometheus's answer as series, cut to [`MAX_SERIES`] of
/// [`MAX_SAMPLES`] (the latest kept).
#[must_use]
pub fn series(answer: PromAnswer) -> MetricSeries {
    let result = answer.data.map(|d| d.result).unwrap_or_default();
    let mut truncated = result.len() > MAX_SERIES;
    let series = result
        .into_iter()
        .take(MAX_SERIES)
        .map(|s| {
            let mut samples: Vec<Sample> = s
                .value
                .iter()
                .chain(s.values.iter())
                .filter_map(sample)
                .collect();
            if samples.len() > MAX_SAMPLES {
                truncated = true;
                samples.drain(..samples.len() - MAX_SAMPLES);
            }
            Serie {
                labels: s.metric,
                samples,
            }
        })
        .collect();
    MetricSeries { series, truncated }
}

/// Loki's `["nanoseconds", "line"]` as a line; none for another shape.
#[must_use]
pub fn line(pair: &ListValue) -> Option<LogLine> {
    let [time, text] = pair.values.as_slice() else {
        return None;
    };
    let time = match time.kind.as_ref()? {
        JsonKind::StringValue(ns) => match ns.parse() {
            Ok(ns) => ns,
            Err(_) => return None,
        },
        _ => return None,
    };
    let JsonKind::StringValue(line) = text.kind.as_ref()? else {
        return None;
    };
    Some(LogLine {
        time,
        line: line.clone(),
    })
}

/// Loki's answer as streams.
#[must_use]
pub fn streams(answer: LokiAnswer) -> Vec<LogStream> {
    answer
        .data
        .map(|d| d.result)
        .unwrap_or_default()
        .into_iter()
        .map(|s| LogStream {
            labels: s.stream,
            lines: s.values.iter().filter_map(line).collect(),
        })
        .collect()
}

/// Alertmanager's alert.
#[must_use]
pub fn alert(alert: AmAlert) -> ActiveAlert {
    let status = alert.status.unwrap_or_default();
    ActiveAlert {
        labels: alert.labels,
        annotations: alert.annotations,
        starts_at: alert.starts_at,
        state: status.state,
        silenced_by: status.silenced_by,
    }
}

fn name(object: &KubeObject) -> String {
    object
        .metadata
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_default()
}

/// A pod: its phase, ready containers, restarts and why one waits or ended.
#[must_use]
pub fn pod(object: &KubeObject) -> PodState {
    let status = object.status.clone().unwrap_or_default();
    let containers = &status.container_statuses;
    let reason = containers
        .iter()
        .filter_map(|c| c.state.as_ref())
        .flat_map(|s| [s.waiting.as_ref(), s.terminated.as_ref()])
        .flatten()
        .map(|r| r.reason.clone())
        .find(|r| !r.is_empty())
        .unwrap_or_default();
    PodState {
        name: name(object),
        phase: status.phase.clone(),
        ready: count(containers.iter().filter(|c| c.ready).count()),
        containers: count(containers.len()),
        restarts: containers.iter().map(|c| c.restart_count).sum(),
        reason,
        created: object
            .metadata
            .as_ref()
            .map(|m| m.creation_timestamp.clone())
            .unwrap_or_default(),
    }
}

/// A deployment: the replicas it wants, those ready and up to date.
#[must_use]
pub fn deployment(object: &KubeObject) -> DeploymentState {
    let status = object.status.clone().unwrap_or_default();
    DeploymentState {
        name: name(object),
        wanted: object.spec.as_ref().map_or(0, |s| s.replicas),
        ready: status.ready_replicas,
        updated: status.updated_replicas,
    }
}

/// A job: its pods that run, succeeded and failed.
#[must_use]
pub fn job(object: &KubeObject) -> JobState {
    let status = object.status.clone().unwrap_or_default();
    JobState {
        name: name(object),
        active: status.active,
        succeeded: status.succeeded,
        failed: status.failed,
    }
}

/// When `event` happened last: its last time, or the time its writer names
/// instead (Kubernetes writes one or the other, the other null).
fn happened(event: &KubeObject) -> &str {
    event
        .last_timestamp
        .as_deref()
        .or(event.event_time.as_deref())
        .unwrap_or_default()
}

/// The warnings among `events` that happened at or after `since` (RFC
/// 3339, as Kubernetes writes times, compares as text), the newest first,
/// at most 50.
#[must_use]
pub fn warnings(events: &[KubeObject], since: &str) -> Vec<EventState> {
    let mut kept: Vec<EventState> = events
        .iter()
        .filter(|e| e.r#type == "Warning" && happened(e) >= since)
        .map(|e| EventState {
            reason: e.reason.clone(),
            message: e.message.clone(),
            object: e
                .involved_object
                .as_ref()
                .map(|o| format!("{}/{}", o.kind, o.name))
                .unwrap_or_default(),
            count: e.count,
            last: happened(e).to_string(),
        })
        .collect();
    kept.sort_by(|a, b| b.last.cmp(&a.last));
    kept.truncate(MAX_EVENTS);
    kept
}

/// A count as the wire carries it (a namespace never holds more).
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

mod nomad;
pub use nomad::{ALLOC_LINES, allocation_of, jobs, node, parents, tail, task_of};

#[cfg(test)]
mod tests;
