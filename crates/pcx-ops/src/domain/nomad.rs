// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a Nomad action may act on (ARCHITECTURE section 8, operations, a
//! Nomad environment): only the jobs its environment names; the one running allocation of a service job that is
//! stopped for a new one, a periodic job no launch of which runs, and the
//! service jobs a revert takes back, each to its stable version before.
//! Pure.

use chrono::DateTime;
use post_cortex_proto::ops::{
    Environment, NomadAlloc, NomadChildren, NomadDeployment, NomadJob, Revert,
};

use crate::domain::Refused;

/// Most jobs of one revert.
const MOST_JOBS: usize = 5;

/// The type of a job that runs until it is stopped.
const SERVICE: &str = "service";

/// The statuses of a deployment that has ended.
const ENDED: [&str; 3] = ["successful", "failed", "cancelled"];

/// `job`, when it may name a Nomad job in a path.
///
/// # Errors
/// It may not.
pub fn job_named(job: &str) -> Result<&str, Refused> {
    if pcx_nomad::name(job) {
        Ok(job)
    } else {
        Err(Refused::NomadName("job"))
    }
}

/// `id`, when it may be an allocation's id or the start of one.
///
/// # Errors
/// It may not.
pub fn allocation_named(id: &str) -> Result<&str, Refused> {
    let id = id.trim();
    if pcx_nomad::allocation(id) {
        Ok(id)
    } else {
        Err(Refused::AllocationId)
    }
}

fn target(why: String) -> Refused {
    Refused::Target(why)
}

/// Whether `jobs` may be the jobs a Nomad environment's actions act on:
/// at least one, each a Nomad job's name, each once.
///
/// # Errors
/// What is wrong with them.
pub fn acted_on(jobs: &[String]) -> Result<(), String> {
    if jobs.is_empty() {
        return Err("a Nomad environment names the jobs its actions act on".to_string());
    }
    for (i, job) in jobs.iter().enumerate() {
        if !pcx_nomad::name(job) {
            return Err(format!("{job} is no Nomad job's name"));
        }
        if jobs[..i].contains(job) {
            return Err(format!("names {job} twice"));
        }
    }
    Ok(())
}

/// Whether `environment`'s actions act on `job`.
///
/// # Errors
/// Its settings do not name it.
pub fn acts_on(environment: &Environment, job: &str) -> Result<(), Refused> {
    if environment.nomad_jobs.iter().any(|j| j == job) {
        Ok(())
    } else {
        Err(target(format!(
            "{job} is none of the jobs ops acts on in {} ({}): a person acts on it",
            environment.name,
            environment.nomad_jobs.join(", ")
        )))
    }
}

/// The allocation among `found` that is stopped for a new one: the only
/// one whose id starts with `asked`, running, of a service job.
///
/// # Errors
/// None or several start so, it does not run, or its job is no service.
pub fn replaceable(found: Vec<NomadAlloc>, asked: &str) -> Result<NomadAlloc, Refused> {
    let mut its: Vec<NomadAlloc> = found
        .into_iter()
        .filter(|a| a.id.starts_with(asked))
        .collect();
    let allocation = match (its.pop(), its.len()) {
        (None, _) => return Err(target(format!("no allocation's id starts with {asked}"))),
        (Some(only), 0) => only,
        (Some(_), others) => {
            return Err(target(format!(
                "{} allocations' ids start with {asked}: give more of the id",
                others + 1
            )));
        }
    };
    let (job, kind, status) = (
        &allocation.job_id,
        &allocation.job_type,
        &allocation.client_status,
    );
    if kind != SERVICE {
        return Err(target(format!(
            "allocation {asked} is of {job}, a {kind} job: only a service job's is replaced"
        )));
    }
    if status != "running" {
        return Err(target(format!(
            "allocation {asked} of {job} is {status}, not running"
        )));
    }
    Ok(allocation)
}

/// Whether the periodic `job` may be launched now: it is periodic, not
/// stopped, and none of its `launches` runs or waits to.
///
/// # Errors
/// Why not.
pub fn launchable(job: &NomadJob, launches: &NomadChildren) -> Result<(), Refused> {
    let name = &job.id;
    if !job.periodic.as_ref().is_some_and(|p| p.enabled) {
        return Err(target(format!("{name} is no periodic job")));
    }
    if job.stop {
        return Err(target(format!("{name} is stopped")));
    }
    if launches.running > 0 || launches.pending > 0 {
        return Err(target(format!("a launch of {name} runs already")));
    }
    Ok(())
}

/// Whether `jobs` may be one revert: 1 to 5 Nomad jobs, each once.
///
/// # Errors
/// Why not.
pub fn revertible(jobs: &[String]) -> Result<(), Refused> {
    if jobs.is_empty() || jobs.len() > MOST_JOBS {
        return Err(Refused::Bounds(format!(
            "a revert names 1 to {MOST_JOBS} jobs, not {}",
            jobs.len()
        )));
    }
    for (i, job) in jobs.iter().enumerate() {
        job_named(job)?;
        if jobs[..i].contains(job) {
            return Err(Refused::Bounds(format!("a revert names {job} twice")));
        }
    }
    Ok(())
}

/// The revert of `job`, which Nomad keeps `versions` of and whose latest
/// deployment is `deployment`: from the version it runs to the newest one
/// before it.
///
/// # Errors
/// It is no service job, it is stopped, its deployment still runs, it has
/// no version before, or that version is not stable.
pub fn revert(
    job: &NomadJob,
    versions: &[NomadJob],
    deployment: Option<&NomadDeployment>,
) -> Result<Revert, Refused> {
    let (name, runs) = (&job.id, job.version);
    if job.r#type != SERVICE {
        return Err(target(format!(
            "{name} is a {} job: only a service job is reverted",
            job.r#type
        )));
    }
    if job.stop {
        return Err(target(format!("{name} is stopped")));
    }
    if let Some(deployment) = deployment
        && !ENDED.contains(&deployment.status.as_str())
    {
        return Err(target(format!(
            "{name}'s deployment still runs ({})",
            deployment.status
        )));
    }
    let Some(before) = versions
        .iter()
        .filter(|v| v.version < runs)
        .max_by_key(|v| v.version)
    else {
        return Err(target(format!(
            "Nomad keeps no version of {name} before {runs}"
        )));
    };
    if !before.stable {
        return Err(target(format!(
            "{name}'s version {} before {runs} is not stable",
            before.version
        )));
    }
    Ok(Revert {
        job: name.clone(),
        from: runs,
        to: before.version,
    })
}

/// `revert` for a person, with when each of its two versions among
/// `versions` was submitted.
#[must_use]
pub fn dated(revert: &Revert, versions: &[NomadJob]) -> String {
    let of = |version: u32| {
        versions
            .iter()
            .find(|v| v.version == version)
            .map(|v| {
                DateTime::from_timestamp_nanos(v.submit_time)
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string()
            })
            .unwrap_or_default()
    };
    format!(
        "{} runs version {} of {}; version {} is of {}",
        revert.job,
        revert.from,
        of(revert.from),
        revert.to,
        of(revert.to)
    )
}

#[cfg(test)]
mod tests;
