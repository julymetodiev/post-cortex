// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What an action wants done, as it is asked and as it is kept: its kind
//! with what that kind acts on, what it is planned to do once its target
//! is read, how a person reads it, what its decision puts at stake, and
//! which targets its brakes hold. Pure.

use post_cortex_proto::ops::{Action, Kind, Revert};
use post_cortex_proto::team::Stakes;

use crate::domain::Refused;

/// Characters of an allocation's id a person is shown.
const SHORT_ID: usize = 8;

/// What an action wants done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// A deployment's pods restarted (a rollout).
    Restart {
        /// The namespace.
        namespace: String,
        /// The deployment.
        name: String,
    },
    /// A pod deleted, which its controller makes again.
    DeletePod {
        /// The namespace.
        namespace: String,
        /// The pod.
        name: String,
    },
    /// A namespace's failed jobs deleted.
    CleanJobs {
        /// The namespace.
        namespace: String,
    },
    /// A deployment's replicas set.
    Scale {
        /// The namespace.
        namespace: String,
        /// The deployment.
        name: String,
        /// The replicas it is to have.
        replicas: u32,
    },
    /// A volume claim grown.
    ResizeVolume {
        /// The namespace.
        namespace: String,
        /// The claim.
        name: String,
        /// Its new size in gibibytes.
        gibibytes: u32,
    },
    /// A Nomad allocation stopped, so Nomad places a new one.
    NewAllocation {
        /// The allocation: its id, or the start of it.
        allocation: String,
    },
    /// A periodic Nomad job launched once now.
    RunPeriodic {
        /// The job.
        job: String,
    },
    /// Nomad service jobs taken back, each to the version before.
    RevertJobs {
        /// The jobs, in the order they are reverted.
        jobs: Vec<String>,
    },
}

impl Wanted {
    /// Whether it is an action of a Nomad environment; else of a
    /// Kubernetes one.
    #[must_use]
    pub fn on_nomad(&self) -> bool {
        matches!(
            self,
            Self::NewAllocation { .. } | Self::RunPeriodic { .. } | Self::RevertJobs { .. }
        )
    }
}

/// What the kept `action` wants done: its kind with its target as it was
/// written.
///
/// # Errors
/// It names no kind.
pub fn wanted(action: &Action) -> Result<Wanted, Refused> {
    let (namespace, name) = match action.target.split_once('/') {
        Some((namespace, name)) => (namespace.to_string(), name.to_string()),
        None => (action.target.clone(), String::new()),
    };
    Ok(match action.kind() {
        Kind::Restart => Wanted::Restart { namespace, name },
        Kind::DeletePod => Wanted::DeletePod { namespace, name },
        Kind::CleanJobs => Wanted::CleanJobs {
            namespace: action.target.clone(),
        },
        Kind::Scale => Wanted::Scale {
            namespace,
            name,
            replicas: action.amount,
        },
        Kind::ResizeVolume => Wanted::ResizeVolume {
            namespace,
            name,
            gibibytes: action.amount,
        },
        Kind::NewAllocation => Wanted::NewAllocation {
            allocation: action.allocation.clone(),
        },
        Kind::RunPeriodic => Wanted::RunPeriodic {
            job: action.target.clone(),
        },
        Kind::RevertJobs => Wanted::RevertJobs {
            jobs: action.reverts.iter().map(|r| r.job.clone()).collect(),
        },
        Kind::Unspecified => return Err(Refused::Empty("kind")),
    })
}

/// What an action is to do, its target read and checked for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// Its kind.
    pub kind: Kind,
    /// Its target: namespace/name, a namespace, a Nomad job, or a
    /// revert's jobs `a, b`.
    pub target: String,
    /// Replicas or gibibytes; 0 for none.
    pub amount: u32,
    /// For a new allocation: the allocation stopped, by its whole id.
    pub allocation: String,
    /// For a revert: each job with the version it runs and the one before.
    pub reverts: Vec<Revert>,
    /// How its target is now, for a person.
    pub now: String,
}

impl Planned {
    /// A plan of `kind` on `target`, which is `now` so.
    #[must_use]
    pub fn of(kind: Kind, target: String, now: String) -> Self {
        Self {
            kind,
            target,
            amount: 0,
            allocation: String::new(),
            reverts: Vec::new(),
            now,
        }
    }
}

/// The action's name, as a team's trust in operations names it.
fn action(kind: Kind) -> &'static str {
    match kind {
        Kind::Restart => "restart",
        Kind::DeletePod => "delete_pod",
        Kind::CleanJobs => "clean_jobs",
        Kind::Scale => "scale",
        Kind::ResizeVolume => "resize_volume",
        Kind::NewAllocation => "new_allocation",
        Kind::RunPeriodic => "run_periodic",
        Kind::RevertJobs => "revert_jobs",
        Kind::Unspecified => "unknown",
    }
}

/// What `planned` does, for a person.
#[must_use]
pub fn what(planned: &Planned) -> String {
    let (target, amount) = (&planned.target, planned.amount);
    match planned.kind {
        Kind::Restart => format!("restart deployment {target}"),
        Kind::DeletePod => format!("delete pod {target}"),
        Kind::CleanJobs => format!("delete the failed jobs of {target}"),
        Kind::Scale => format!("scale deployment {target} to {amount} replicas"),
        Kind::ResizeVolume => format!("grow volume claim {target} to {amount}Gi"),
        Kind::NewAllocation => {
            let short: String = planned.allocation.chars().take(SHORT_ID).collect();
            format!("stop allocation {short} of {target}, so Nomad places a new one")
        }
        Kind::RunPeriodic => format!("launch periodic job {target} now"),
        Kind::RevertJobs => format!("revert {}", steps(&planned.reverts)),
        Kind::Unspecified => format!("act on {target}"),
    }
}

/// `reverts` for a person: each job with its versions.
#[must_use]
pub fn steps(reverts: &[Revert]) -> String {
    let steps: Vec<String> = reverts
        .iter()
        .map(|r| format!("{} from version {} to {}", r.job, r.from, r.to))
        .collect();
    steps.join(", ")
}

/// What a decision on the action puts at stake: its action, environment
/// and target, which a team's trust in operations weighs; a revert of jobs
/// is a person's to approve, whatever the trust.
#[must_use]
pub fn stakes(kind: Kind, environment: &str, target: &str) -> Stakes {
    Stakes {
        action: Some(action(kind).to_string()),
        environment: Some(environment.to_string()),
        target: Some(target.to_string()),
        person: (kind == Kind::RevertJobs).then_some(true),
        ..Stakes::default()
    }
}

/// What the brakes hold for an action on `target` with `reverts`: each job
/// of a revert, else its target.
#[must_use]
pub fn targets<'a>(target: &'a str, reverts: &'a [Revert]) -> Vec<&'a str> {
    if reverts.is_empty() {
        vec![target]
    } else {
        reverts.iter().map(|r| r.job.as_str()).collect()
    }
}
