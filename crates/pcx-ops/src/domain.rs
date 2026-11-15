// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Ops' domain (ARCHITECTURE section 8, operations, W31): what its
//! settings may be, what an action asks and what it may act on, the brakes
//! (a target once in a while, a few actions an hour) and whether the
//! target is still what the action is for; what an action wants and is
//! planned to do in `wanted`, a Nomad action's targets in `nomad`, the
//! patches Kubernetes is sent in `patches`. Pure: no cluster, no board, no
//! clock (time is a parameter).

use std::fmt;

use chrono::{DateTime, Duration, Utc};
use pcx_kube::{kube_name, tls_server_name};
use post_cortex_proto::ops::{Action, Ask, Environment, KubeThing, OpsConfig, State, ThingList};

/// The module's id.
pub const MODULE_ID: &str = "ops";

/// The kind of its documents, `ops/action`.
pub const ACTION: &str = "ops/action";

/// The label of a CloudNativePG database's pods and claims: no action of
/// this module touches them.
const DATABASE: &str = "cnpg.io/cluster";

/// Replicas a scale may set when the environment names no bound.
const MAX_REPLICAS: u32 = 3;

/// Minutes before a target is acted on again when the settings name none.
const PER_TARGET: u32 = 30;

/// Actions an hour in an environment when the settings name none.
const PER_HOUR: u32 = 6;

/// A setting, a request or a target ops refuses.
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
    /// A field that must say something is empty.
    Empty(&'static str),
    /// The environment's cluster is of the other kind.
    Cluster {
        /// The environment.
        environment: String,
        /// What it is: Kubernetes or Nomad.
        is: &'static str,
        /// The actions of such a cluster.
        actions: &'static str,
    },
    /// A name that is no Kubernetes name.
    Name(&'static str),
    /// A name that is no Nomad job's.
    NomadName(&'static str),
    /// What should name an allocation is no id nor the start of one.
    AllocationId,
    /// An amount out of its bounds.
    Bounds(String),
    /// A brake holds the action.
    Brake(String),
    /// The target is not what the action is for.
    Target(String),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setting { field, reason } => write!(f, "{field}: {reason}"),
            Self::UnknownEnvironment(name) => write!(f, "no environment is named {name}"),
            Self::Empty(what) => write!(f, "an action names its {what}"),
            Self::Cluster {
                environment,
                is,
                actions,
            } => write!(
                f,
                "environment {environment} is {is}: its actions are {actions}"
            ),
            Self::Name(what) => write!(
                f,
                "the {what} is no Kubernetes name: lowercase letters, digits, '-' and '.'"
            ),
            Self::NomadName(what) => write!(
                f,
                "the {what} is no Nomad name: letters, digits, '-', '_' and '.'"
            ),
            Self::AllocationId => f.write_str(
                "an allocation is named by its id, or by the first 8 characters or more of it",
            ),
            Self::Bounds(why) | Self::Brake(why) | Self::Target(why) => f.write_str(why),
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

/// Whether `config` may run: its workspace, and environments named once,
/// each a cluster (a Kubernetes API or a Nomad API, never both) with its
/// token's file, a Nomad one with the jobs its actions act on.
///
/// # Errors
/// The first setting that is wrong.
pub fn check(config: &OpsConfig) -> Result<(), Refused> {
    if config.workspace_id.trim().is_empty() {
        return Err(setting("workspace_id".to_string(), "is empty"));
    }
    let (remind, give_up) = patience(config);
    if give_up <= remind {
        return Err(setting(
            "give_up_minutes".to_string(),
            format!("is {give_up}, not more than the reminder's {remind}"),
        ));
    }
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
        match (
            env.kube_api.trim().is_empty(),
            env.nomad_api.trim().is_empty(),
        ) {
            (true, true) => {
                return Err(setting(
                    at("kube_api"),
                    "an environment names its Kubernetes API or its Nomad API",
                ));
            }
            (false, false) => {
                return Err(setting(
                    at("nomad_api"),
                    "an environment names a Kubernetes API or a Nomad API, not both",
                ));
            }
            (false, true) => {
                if env.kube_token_file.trim().is_empty() {
                    return Err(setting(at("kube_token_file"), "is empty"));
                }
                if let Err(reason) = tls_server_name(&env.kube_tls_server_name, &env.kube_ca_file) {
                    return Err(setting(at("kube_tls_server_name"), reason));
                }
                if !env.nomad_jobs.is_empty() {
                    return Err(setting(at("nomad_jobs"), "are a Nomad environment's"));
                }
            }
            (true, false) => {
                if let Err((field, reason)) = pcx_nomad::settings(
                    &env.nomad_api,
                    &env.nomad_token_file,
                    &env.nomad_ca_file,
                    &env.nomad_tls_server_name,
                ) {
                    return Err(setting(at(field), reason));
                }
                if let Err(reason) = nomad::acted_on(&env.nomad_jobs) {
                    return Err(setting(at("nomad_jobs"), reason));
                }
            }
        }
    }
    Ok(())
}

/// The actions of a Kubernetes environment, and those of a Nomad one.
const ACTIONS: (&str, &str) = (
    "ops_restart, ops_delete_pod, ops_clean_jobs, ops_scale and ops_resize_volume",
    "ops_new_allocation, ops_run_periodic and ops_revert_jobs",
);

/// Whether `environment` is the kind of cluster `wanted` acts on.
///
/// # Errors
/// It is the other kind.
pub fn cluster(environment: &Environment, wanted: &Wanted) -> Result<(), Refused> {
    let nomad = environment.kube_api.trim().is_empty();
    if nomad == wanted.on_nomad() {
        return Ok(());
    }
    let (is, actions) = if nomad {
        ("Nomad", ACTIONS.1)
    } else {
        ("Kubernetes", ACTIONS.0)
    };
    Err(Refused::Cluster {
        environment: environment.name.clone(),
        is,
        actions,
    })
}

/// The environment named `name`.
///
/// # Errors
/// No environment of that name.
pub fn environment<'a>(config: &'a OpsConfig, name: &str) -> Result<&'a Environment, Refused> {
    config
        .environments
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| Refused::UnknownEnvironment(name.to_string()))
}

/// Whether `ask` names its task's workspace, its task and why.
///
/// # Errors
/// The first it leaves empty.
pub fn asked(ask: &Ask) -> Result<(), Refused> {
    for (field, value) in [
        ("environment", &ask.environment),
        ("workspace", &ask.workspace_id),
        ("task", &ask.task),
        ("why", &ask.why),
    ] {
        if value.trim().is_empty() {
            return Err(Refused::Empty(field));
        }
    }
    Ok(())
}

/// `name`, a namespace's or an object's, when it is a Kubernetes name.
///
/// # Errors
/// It is not.
pub fn named<'a>(what: &'static str, name: &'a str) -> Result<&'a str, Refused> {
    if kube_name(name) {
        Ok(name)
    } else {
        Err(Refused::Name(what))
    }
}

/// `replicas` within `environment`'s bounds (1 to its most, 3 unless it
/// says).
///
/// # Errors
/// Out of them.
pub fn replicas(environment: &Environment, replicas: u32) -> Result<u32, Refused> {
    let most = match environment.max_replicas {
        0 => MAX_REPLICAS,
        most => most,
    };
    if (1..=most).contains(&replicas) {
        Ok(replicas)
    } else {
        Err(Refused::Bounds(format!(
            "a scale sets 1 to {most} replicas in {}, not {replicas}",
            environment.name
        )))
    }
}

/// Whether the brakes let an action on `target` of `environment` run now:
/// none on it waits for approval, none ran on it within the settings'
/// minutes (30), and fewer than the settings' actions an hour (6) ran in
/// the environment. An action is on each of its targets: a revert on each
/// of its jobs.
///
/// # Errors
/// The brake that holds it, and what to do instead.
pub fn brake(
    config: &OpsConfig,
    actions: &[Action],
    environment: &str,
    target: &str,
    now: DateTime<Utc>,
) -> Result<(), Refused> {
    let minutes = match config.per_target_minutes {
        0 => PER_TARGET,
        m => m,
    };
    let per_hour = match config.per_hour {
        0 => PER_HOUR,
        n => n,
    };
    let here = |a: &&Action| a.environment == environment;
    let on = |a: &&Action| targets(&a.target, &a.reverts).contains(&target);
    if let Some(waiting) = actions
        .iter()
        .filter(here)
        .filter(on)
        .find(|a| a.state() == State::Waiting)
    {
        return Err(Refused::Brake(format!(
            "an action on {target} waits for approval already ({}); approve or drop it first",
            waiting.what
        )));
    }
    let ran = |a: &Action| ran_at(a);
    let since_target = now - Duration::minutes(i64::from(minutes));
    if let Some(recent) = actions
        .iter()
        .filter(here)
        .filter(on)
        .find(|a| ran(a).is_some_and(|t| t > since_target))
    {
        return Err(Refused::Brake(format!(
            "{target} was acted on within {minutes} minutes ({}); if that did not help, it needs a person",
            recent.what
        )));
    }
    let hour = now - Duration::hours(1);
    let this_hour = actions
        .iter()
        .filter(here)
        .filter(|a| ran(a).is_some_and(|t| t > hour))
        .count();
    if this_hour >= per_hour as usize {
        return Err(Refused::Brake(format!(
            "{this_hour} actions ran in {environment} this hour, the most it takes; it needs a person"
        )));
    }
    Ok(())
}

/// When `action` ran (ops writes whole seconds); none when it did not.
/// How far back [`brake`] looks at what ran: the settings' minutes per
/// target or an hour, whichever is longer. An action that ran is written
/// when it ran, so one not written since is older than its run.
#[must_use]
pub fn brake_window(config: &OpsConfig) -> Duration {
    let minutes = match config.per_target_minutes {
        0 => PER_TARGET,
        m => m,
    };
    Duration::minutes(i64::from(minutes)).max(Duration::hours(1))
}

fn ran_at(action: &Action) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(action.ran.as_ref()?.seconds, 0)
}

/// Whether `pod` may be deleted: a controller makes it again, and it is no
/// database's.
///
/// # Errors
/// It has no controller, or it is a database's.
pub fn deletable(pod: &KubeThing) -> Result<(), Refused> {
    let meta = pod.metadata.clone().unwrap_or_default();
    if meta.labels.contains_key(DATABASE) {
        return Err(Refused::Target(format!(
            "pod {} is a database's: a person acts on it",
            meta.name
        )));
    }
    if !meta.owner_references.iter().any(|o| o.controller) {
        return Err(Refused::Target(format!(
            "pod {} has no controller: deleted, it would not come back",
            meta.name
        )));
    }
    Ok(())
}

/// `n` as a whole number; none when it is not one.
fn whole(n: &str) -> Option<u64> {
    n.parse().ok()
}

/// A quantity of storage in whole gibibytes (`10Gi`, `1Ti`, `10240Mi`
/// rounded up); none for another unit or no whole number.
#[must_use]
pub fn gibibytes(quantity: &str) -> Option<u64> {
    let quantity = quantity.trim();
    if let Some(n) = quantity.strip_suffix("Ti") {
        whole(n)?.checked_mul(1024)
    } else if let Some(n) = quantity.strip_suffix("Gi") {
        whole(n)
    } else if let Some(n) = quantity.strip_suffix("Mi") {
        Some(whole(n)?.div_ceil(1024))
    } else {
        None
    }
}

/// Whether `claim` may grow to `to` gibibytes: no database's, its size
/// known, larger and at most twice it. Its size is its volume's (its
/// capacity) when that is more than it asked for: a provider's smallest
/// volume may be larger than the claim (Equal 2026-11-11: 2Gi asked, 10Gi
/// had), and a claim grown to less than its volume grows nothing.
///
/// # Errors
/// Why not.
pub fn growable(claim: &KubeThing, to: u32) -> Result<u64, Refused> {
    let meta = claim.metadata.clone().unwrap_or_default();
    if meta.labels.contains_key(DATABASE) {
        return Err(Refused::Target(format!(
            "claim {} is a database's: it grows through its cluster, a person's",
            meta.name
        )));
    }
    let asked = claim
        .spec
        .as_ref()
        .and_then(|s| s.resources.as_ref())
        .and_then(|r| r.requests.get("storage"))
        .and_then(|q| gibibytes(q));
    let has = claim
        .status
        .as_ref()
        .and_then(|s| s.capacity.get("storage"))
        .and_then(|q| gibibytes(q));
    let size = asked
        .max(has)
        .ok_or_else(|| Refused::Target(format!("claim {} says no size", meta.name)))?;
    let to = u64::from(to);
    if to <= size || to > size * 2 {
        return Err(Refused::Bounds(format!(
            "claim {} has {size}Gi: it grows to more, at most {}Gi, not {to}Gi",
            meta.name,
            size * 2
        )));
    }
    Ok(size)
}

/// The jobs of `jobs` that failed and never succeeded, by name.
#[must_use]
pub fn failed_jobs(jobs: &ThingList) -> Vec<String> {
    jobs.items
        .iter()
        .filter(|j| {
            j.status
                .as_ref()
                .is_some_and(|s| s.failed > 0 && s.succeeded == 0)
        })
        .filter_map(|j| j.metadata.as_ref().map(|m| m.name.clone()))
        .collect()
}

mod nomad;
mod patches;
mod waiting;
mod wanted;
pub use nomad::{
    acts_on, allocation_named, dated, job_named, launchable, replaceable, revert, revertible,
};
pub use patches::{grow, restart, scale};
pub use waiting::{Waited, given_up, look_again, patience, waited};
pub use wanted::{Planned, Wanted, stakes, steps, targets, wanted, what};

#[cfg(test)]
mod tests;
