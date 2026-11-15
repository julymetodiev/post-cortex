// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The scheduler service (ARCHITECTURE section 8, operations, W28): its
//! jobs and runs as documents, which jobs are due, and a run done through
//! the world's ports. Between the handlers (and the module's loop) and the
//! ports.

use chrono::{DateTime, Utc};
use futures::{StreamExt, TryStreamExt, stream};
use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::scheduler::job::Does;
use post_cortex_proto::scheduler::{
    Job, JobList, JobRecord, Outcome, Run, RunList, RunRecord, UpdateJobRequest,
};
use prost_types::Timestamp;

use crate::domain::{self, Broken, Kind};
use crate::ports::{Clock, World};

/// Runs a listing answers when the request names no limit.
const RUNS: usize = 20;

fn broken(rule: Broken) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, rule.to_string())
}

/// `time` as the wire carries it, to the second.
fn stamp(time: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: time.timestamp(),
        nanos: 0,
    }
}

/// `stamp` as a time; none when unset or out of range.
fn time(stamp: Option<&Timestamp>) -> Option<DateTime<Utc>> {
    stamp.and_then(|t| domain::at(t.seconds, t.nanos))
}

/// A run as the scheduler answers it.
fn record(run: Item<Run>) -> RunRecord {
    RunRecord {
        id: run.document.id,
        run: Some(run.data),
    }
}

/// The scheduler's jobs and runs in its workspace.
pub struct Service<'a> {
    docs: &'a dyn Documents,
    clock: &'a dyn Clock,
}

/// How many jobs' last runs are read at once.
const AT_ONCE: usize = 8;

impl<'a> Service<'a> {
    /// The scheduler over its documents and the clock.
    pub fn new(docs: &'a dyn Documents, clock: &'a dyn Clock) -> Self {
        Self { docs, clock }
    }

    fn jobs(&self) -> Typed<'a> {
        Typed::new(self.docs, Kind::Job.document())
    }

    fn runs(&self) -> Typed<'a> {
        Typed::new(self.docs, Kind::Run.document())
    }

    /// A new job.
    ///
    /// # Errors
    /// A job the rules refuse, or whose name is taken; memory failed.
    pub async fn add_job(&self, job: Job) -> Result<JobRecord, ModuleError> {
        domain::check(&job).map_err(broken)?;
        self.free(&job.name, None).await?;
        let added = self.jobs().create(&job, "").await?;
        self.job_record(added).await
    }

    /// Job `change.id` as `change.job`, from the version read.
    ///
    /// # Errors
    /// A job the rules refuse, or whose name another has; no such job; it
    /// changed since that version (`Conflict`); memory failed.
    pub async fn update_job(&self, change: UpdateJobRequest) -> Result<JobRecord, ModuleError> {
        let job = change.job.unwrap_or_default();
        domain::check(&job).map_err(broken)?;
        self.free(&job.name, Some(&change.id)).await?;
        let changed = self
            .jobs()
            .change(&change.id, change.version, &job, "")
            .await?;
        self.job_record(changed).await
    }

    async fn free(&self, name: &str, id: Option<&str>) -> Result<(), ModuleError> {
        let jobs = self.jobs().all::<Job>(None).await?;
        domain::free(
            name,
            id,
            jobs.iter()
                .map(|j| (j.document.id.as_str(), j.data.name.as_str())),
        )
        .map_err(|rule| ModuleError::new(ModuleErrorKind::Conflict, rule.to_string()))
    }

    /// The jobs by name, each with its last run and when it is due next.
    ///
    /// # Errors
    /// Memory failed; a job the rules refuse.
    pub async fn list_jobs(&self) -> Result<JobList, ModuleError> {
        let read = self.jobs().all::<Job>(None).await?;
        // Each job's last run at once: the list waits for the slowest.
        let mut jobs: Vec<JobRecord> = stream::iter(read)
            .map(|job| self.job_record(job))
            .buffered(AT_ONCE)
            .try_collect()
            .await?;
        jobs.sort_by(|a, b| {
            let name = |r: &JobRecord| r.job.as_ref().map(|j| j.name.clone());
            name(a).cmp(&name(b))
        });
        Ok(JobList { jobs })
    }

    /// Job `id`.
    ///
    /// # Errors
    /// No such job; memory failed.
    pub async fn job(&self, id: &str) -> Result<Item<Job>, ModuleError> {
        self.jobs().get(id).await
    }

    /// Remove job `id`; its runs are kept.
    ///
    /// # Errors
    /// No such job; memory failed.
    pub async fn remove_job(&self, id: &str) -> Result<(), ModuleError> {
        self.jobs().delete(id).await
    }

    /// Job `job`'s runs, the newest first, at most `limit` (0: 20).
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list_runs(&self, job: &str, limit: usize) -> Result<RunList, ModuleError> {
        let limit = if limit == 0 { RUNS } else { limit };
        let mut runs = Vec::new();
        self.newest_runs(job, |run| {
            runs.push(record(run));
            runs.len() < limit
        })
        .await?;
        Ok(RunList { runs })
    }

    /// The runs of job `id`, the newest first, each given to `visit` until
    /// it answers false: a job's runs only grow, so no reader takes them
    /// all (docs/host.md, pages in a module).
    async fn newest_runs(
        &self,
        id: &str,
        visit: impl FnMut(Item<Run>) -> bool + Send,
    ) -> Result<(), ModuleError> {
        let of = Run {
            job: id.to_string(),
            ..Run::default()
        };
        self.runs().scan(Some((&of, &["job"][..])), visit).await
    }

    /// Where `job`'s times are counted from: its last scheduled run's time,
    /// or when it was last changed, whichever is later (a change does not
    /// run the times before it).
    async fn anchor(
        &self,
        job: &Item<Job>,
    ) -> Result<(DateTime<Utc>, Option<Item<Run>>), ModuleError> {
        // Read as far as the newest run that had a time (one run by hand
        // has none).
        let (mut last, mut scheduled) = (None, None);
        self.newest_runs(&job.document.id, |run| {
            scheduled = time(run.data.due.as_ref());
            if last.is_none() {
                last = Some(run);
            }
            scheduled.is_none()
        })
        .await?;
        // Memory writes when a document changed; one without it reads as
        // changed now, so none of its past times runs.
        let changed = time(job.document.updated_at.as_ref()).unwrap_or(self.clock.now());
        let anchor = scheduled.map_or(changed, |due| due.max(changed));
        Ok((anchor, last))
    }

    async fn job_record(&self, job: Item<Job>) -> Result<JobRecord, ModuleError> {
        let (anchor, last) = self.anchor(&job).await?;
        let next = domain::next(&job.data, anchor, self.clock.now()).map_err(broken)?;
        Ok(JobRecord {
            id: job.document.id,
            version: job.document.version,
            updated_at: job.document.updated_at,
            job: Some(job.data),
            last_run: last.map(record),
            next: next.map(stamp),
        })
    }

    /// How the job named `name` stands for who watches it from outside;
    /// none when this workspace has no job of the name.
    ///
    /// # Errors
    /// Memory failed; a job the rules refuse.
    pub async fn health(&self, name: &str) -> Result<Option<domain::Health>, ModuleError> {
        let jobs = self.jobs().all::<Job>(None).await?;
        let Some(job) = jobs.into_iter().find(|j| j.data.name == name) else {
            return Ok(None);
        };
        let now = self.clock.now();
        let changed = time(job.document.updated_at.as_ref()).unwrap_or(now);
        // A run is written after its time, so one written before the time
        // the job owes is no run of that time, nor is any older.
        let owed = domain::owed(&job.data, changed, now).map_err(broken)?;
        let mut ran = Vec::new();
        if let Some(owed) = owed {
            self.newest_runs(&job.document.id, |run| {
                let since = time(run.document.updated_at.as_ref()).is_none_or(|at| at >= owed);
                let at = time(run.data.due.as_ref()).or_else(|| time(run.data.started.as_ref()));
                if let (true, Some(at)) = (since, at) {
                    ran.push((at, run.data.outcome()));
                }
                since
            })
            .await?;
        }
        domain::health(&job.data, changed, ran, now)
            .map(Some)
            .map_err(broken)
    }

    /// The jobs due now, each with the one time it is due.
    ///
    /// # Errors
    /// Memory failed; a job the rules refuse.
    pub async fn due(&self) -> Result<Vec<(Item<Job>, DateTime<Utc>)>, ModuleError> {
        let now = self.clock.now();
        let mut due = Vec::new();
        for job in self.jobs().all::<Job>(None).await? {
            let (anchor, _) = self.anchor(&job).await?;
            if let Some(at) = domain::due(&job.data, anchor, now).map_err(broken)? {
                due.push((job, at));
            }
        }
        Ok(due)
    }

    /// A run of `job` due at `due`, not run: its job ran then.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn skip(
        &self,
        job: &Item<Job>,
        due: DateTime<Utc>,
    ) -> Result<RunRecord, ModuleError> {
        let now = stamp(self.clock.now());
        let run = Run {
            job: job.document.id.clone(),
            name: job.data.name.clone(),
            due: Some(stamp(due)),
            started: Some(now),
            ended: Some(now),
            outcome: Outcome::Skipped.into(),
            exit_code: None,
            result: "its job was running".to_string(),
        };
        Ok(record(self.runs().create(&run, "").await?))
    }

    /// A run of `job` started now, due at `due` (none: by hand).
    ///
    /// # Errors
    /// Memory failed.
    pub async fn start(
        &self,
        job: &Item<Job>,
        due: Option<DateTime<Utc>>,
    ) -> Result<Item<Run>, ModuleError> {
        let run = Run {
            job: job.document.id.clone(),
            name: job.data.name.clone(),
            due: due.map(stamp),
            started: Some(stamp(self.clock.now())),
            ended: None,
            outcome: Outcome::Running.into(),
            exit_code: None,
            result: String::new(),
        };
        self.runs().create(&run, "").await
    }

    /// Do what `job` does in `run`, through `world`, and write how it
    /// ended (a failure of the world is a failed run, not an error).
    ///
    /// # Errors
    /// Memory failed writing the run.
    pub async fn work(
        &self,
        world: &dyn World,
        job: &Item<Job>,
        run: Item<Run>,
    ) -> Result<RunRecord, ModuleError> {
        let at = time(run.data.due.as_ref()).unwrap_or(self.clock.now());
        let (outcome, exit_code, result) = match &job.data.does {
            Some(Does::Script(script)) => match world.script(script).await {
                Ok((ended, output)) => domain::script_end(ended, &output),
                Err(e) => (Outcome::Failed, None, e.to_string()),
            },
            Some(Does::Task(template)) => {
                match world
                    .add_task(domain::task(template, &job.data.name, at))
                    .await
                {
                    Ok(task) => (Outcome::Ok, None, task),
                    Err(e) => (Outcome::Failed, None, e.to_string()),
                }
            }
            Some(Does::Workflow(action)) => match world.workflow(action).await {
                Ok(run) => (Outcome::Ok, None, run),
                Err(e) => (Outcome::Failed, None, e.to_string()),
            },
            None => (Outcome::Failed, None, Broken::NoDoes.to_string()),
        };
        let ended = Run {
            ended: Some(stamp(self.clock.now())),
            outcome: outcome.into(),
            exit_code,
            result,
            ..run.data
        };
        let written = self
            .runs()
            .change(&run.document.id, run.document.version, &ended, "")
            .await?;
        Ok(record(written))
    }
}

#[cfg(test)]
mod tests;
