// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A Nomad action's target read and checked (ARCHITECTURE section 8,
//! operations, a Nomad environment): the allocation stopped for a new
//! one, the periodic job launched, the jobs a revert takes back; and a
//! revert run, job after job.

use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::ops::{Environment, Kind, Revert};

use crate::domain::{self, Planned};
use crate::service::{Service, refused};

impl Service<'_> {
    /// The plan to stop the allocation whose id starts with `asked`: the
    /// only one that starts so, running, of a service job, which is its
    /// target.
    ///
    /// # Errors
    /// `asked` is no id; no such allocation; Nomad refused or failed.
    pub async fn new_allocation(
        &self,
        env: &Environment,
        asked: &str,
    ) -> Result<Planned, ModuleError> {
        let asked = domain::allocation_named(asked).map_err(refused)?;
        let found = self
            .nomad
            .allocations(env, pcx_nomad::prefix(asked))
            .await?;
        let allocation = domain::replaceable(found, asked).map_err(refused)?;
        domain::acts_on(env, &allocation.job_id).map_err(refused)?;
        Ok(Planned {
            allocation: allocation.id,
            ..Planned::of(
                Kind::NewAllocation,
                allocation.job_id,
                "it runs".to_string(),
            )
        })
    }

    /// The plan to launch the periodic job `job` now.
    ///
    /// # Errors
    /// `job` is no name; it is no periodic job, is stopped, or a launch of
    /// it runs; Nomad refused or failed.
    pub async fn periodic(&self, env: &Environment, job: &str) -> Result<Planned, ModuleError> {
        let job = domain::job_named(job).map_err(refused)?;
        domain::acts_on(env, job).map_err(refused)?;
        let read = self.nomad.job(env, job).await?;
        let launches = self.nomad.launches(env, job).await?;
        domain::launchable(&read, &launches).map_err(refused)?;
        Ok(Planned::of(
            Kind::RunPeriodic,
            job.to_string(),
            "no launch of it runs".to_string(),
        ))
    }

    /// The plan to take `jobs` back, each to its version before.
    ///
    /// # Errors
    /// `jobs` are not 1 to 5 names, each once; a job that may not be
    /// reverted, named; Nomad refused or failed.
    pub async fn reverts(
        &self,
        env: &Environment,
        jobs: &[String],
    ) -> Result<Planned, ModuleError> {
        domain::revertible(jobs).map_err(refused)?;
        for job in jobs {
            domain::acts_on(env, job).map_err(refused)?;
        }
        let mut reverts = Vec::with_capacity(jobs.len());
        let mut now = Vec::with_capacity(jobs.len());
        for job in jobs {
            let read = self.nomad.job(env, job).await?;
            let versions = self.nomad.versions(env, job).await?;
            let deployment = self.nomad.deployment(env, job).await?;
            let revert = domain::revert(&read, &versions, deployment.as_ref()).map_err(refused)?;
            now.push(domain::dated(&revert, &versions));
            reverts.push(revert);
        }
        Ok(Planned {
            reverts,
            ..Planned::of(Kind::RevertJobs, jobs.join(", "), now.join("; "))
        })
    }

    /// Revert each job of `asked`, in order, when `planned`, read now,
    /// still takes the same versions back: what it did. The first job
    /// Nomad does not revert ends it, saying which were reverted before.
    ///
    /// # Errors
    /// A job's versions changed since the revert was asked; Nomad refused
    /// or failed.
    pub async fn revert_all(
        &self,
        env: &Environment,
        asked: &[Revert],
        planned: &Planned,
    ) -> Result<String, ModuleError> {
        if planned.reverts != asked {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!(
                    "the jobs' versions changed since it was asked; now it would revert {}",
                    domain::steps(&planned.reverts)
                ),
            ));
        }
        for (done, revert) in asked.iter().enumerate() {
            if let Err(e) = self.nomad.revert(env, revert).await {
                let before = match domain::steps(&asked[..done]) {
                    steps if steps.is_empty() => "nothing was reverted".to_string(),
                    steps => format!("reverted before it: {steps}"),
                };
                return Err(ModuleError::new(
                    e.kind,
                    format!("{} was not reverted: {}; {before}", revert.job, e.message),
                ));
            }
        }
        Ok(format!("reverted {}", domain::steps(asked)))
    }
}
