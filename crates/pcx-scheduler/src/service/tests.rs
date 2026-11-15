// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The scheduler service over documents held in memory, a clock the test
//! moves and a world that records what it was asked.

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::InMemoryDocuments;
use post_cortex_proto::board::AddTaskRequest;
use post_cortex_proto::scheduler::job::When;
use post_cortex_proto::scheduler::{Script, TaskTemplate, WorkflowAction};

use super::*;

use crate::domain::Ended;

/// The documents' versions are written at 2026-10-25 06:00 UTC; the
/// clock starts two days later.
struct Moving(Mutex<DateTime<Utc>>);

impl Moving {
    fn at(text: &str) -> Self {
        Self(Mutex::new(
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&Utc),
        ))
    }

    fn set(&self, text: &str) {
        *self.0.lock() = DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc);
    }
}

impl Clock for Moving {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock()
    }
}

/// A world whose scripts exit 0 (refused when `fail`) and whose board
/// answers task ids.
#[derive(Default)]
struct Recorded {
    scripts: Mutex<Vec<String>>,
    tasks: Mutex<Vec<AddTaskRequest>>,
    workflows: Mutex<Vec<String>>,
    fail: bool,
}

#[async_trait]
impl World for Recorded {
    async fn script(&self, script: &Script) -> Result<(Ended, String), ModuleError> {
        self.scripts.lock().push(script.program.clone());
        if self.fail {
            return Err(ModuleError::new(ModuleErrorKind::Denied, "no spawn grant"));
        }
        Ok((Ended::Code(0), "probed".into()))
    }

    async fn add_task(&self, task: AddTaskRequest) -> Result<String, ModuleError> {
        self.tasks.lock().push(task);
        Ok(format!("t-{}", self.tasks.lock().len()))
    }

    async fn workflow(&self, action: &WorkflowAction) -> Result<String, ModuleError> {
        self.workflows.lock().push(action.name.clone());
        Ok("r-1".into())
    }
}

fn documents() -> InMemoryDocuments {
    InMemoryDocuments::new(None, post_cortex_proto::SCHEDULER_DESCRIPTOR_SET).unwrap()
}

fn probe() -> Job {
    Job {
        name: "probe".into(),
        when: Some(When::EverySeconds(300)),
        does: Some(Does::Script(Script {
            program: "eq-probe".into(),
            ..Script::default()
        })),
        ..Job::default()
    }
}

fn report() -> Job {
    Job {
        name: "daily-report".into(),
        when: Some(When::Cron(post_cortex_proto::scheduler::Cron {
            expression: "0 9 * * *".into(),
            time_zone: "Europe/Sofia".into(),
        })),
        does: Some(Does::Task(TaskTemplate {
            workspace_id: "ws".into(),
            board: "b".into(),
            title: "Daily report".into(),
            assignee: "eq-sentinel".into(),
            ..TaskTemplate::default()
        })),
        ..Job::default()
    }
}

#[tokio::test]
async fn jobs_are_kept_by_name_with_when_they_are_due_next() {
    let (docs, clock) = (documents(), Moving::at("2026-10-27T06:30:00Z"));
    let service = Service::new(&docs, &clock);
    let added = service.add_job(report()).await.unwrap();
    assert_eq!(
        added.next.as_ref().unwrap().seconds,
        1_793_084_400,
        "09:00 Sofia"
    );
    service.add_job(probe()).await.unwrap();
    let taken = service.add_job(probe()).await.unwrap_err();
    assert_eq!(taken.kind, ModuleErrorKind::Conflict);
    let refused = service
        .add_job(Job {
            when: Some(When::EverySeconds(10)),
            ..probe()
        })
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);

    let names: Vec<String> = service
        .list_jobs()
        .await
        .unwrap()
        .jobs
        .into_iter()
        .map(|j| j.job.unwrap().name)
        .collect();
    assert_eq!(names, ["daily-report", "probe"]);

    let paused = service
        .update_job(UpdateJobRequest {
            id: added.id.clone(),
            version: added.version,
            job: Some(Job {
                paused: true,
                ..report()
            }),
        })
        .await
        .unwrap();
    assert_eq!(paused.next, None, "a paused job is not due");
    let stale = service
        .update_job(UpdateJobRequest {
            id: added.id.clone(),
            version: added.version,
            job: Some(report()),
        })
        .await
        .unwrap_err();
    assert_eq!(stale.kind, ModuleErrorKind::Conflict);
    service.remove_job(&added.id).await.unwrap();
    assert_eq!(service.list_jobs().await.unwrap().jobs.len(), 1);
    assert_eq!(
        service.remove_job(&added.id).await.unwrap_err().kind,
        ModuleErrorKind::NotFound
    );
}

#[tokio::test]
async fn a_due_job_runs_once_and_its_run_says_how_it_ended() {
    let (docs, clock, world) = (
        documents(),
        Moving::at("2026-10-27T06:30:00Z"),
        Recorded::default(),
    );
    let service = Service::new(&docs, &clock);
    service.add_job(report()).await.unwrap();
    service.add_job(probe()).await.unwrap();
    // The documents were written on the 25th: from then on, only the
    // latest time of each is due, once.
    clock.set("2026-10-27T07:01:00Z");
    let due = service.due().await.unwrap();
    assert_eq!(due.len(), 2);
    for (job, at) in due {
        let run = service.start(&job, Some(at)).await.unwrap();
        assert_eq!(run.data.outcome(), Outcome::Running);
        let ended = service.work(&world, &job, run).await.unwrap().run.unwrap();
        assert_eq!(ended.outcome(), Outcome::Ok);
        assert!(ended.ended.is_some());
    }
    assert_eq!(world.scripts.lock().clone(), ["eq-probe"]);
    let task = world.tasks.lock()[0].clone();
    assert_eq!(
        task.key.as_deref(),
        Some("scheduler:daily-report:1793084400")
    );
    assert_eq!(task.assignee.as_deref(), Some("eq-sentinel"));
    assert!(
        service.due().await.unwrap().is_empty(),
        "each ran for its time"
    );

    clock.set("2026-10-27T07:06:00Z");
    let again = service.due().await.unwrap();
    assert_eq!(again.len(), 1, "the probe, five minutes on");
    let skipped = service.skip(&again[0].0, again[0].1).await.unwrap();
    assert_eq!(skipped.run.unwrap().outcome(), Outcome::Skipped);
    assert!(
        service.due().await.unwrap().is_empty(),
        "a skipped time is not due again"
    );

    let probe_id = again[0].0.document.id.clone();
    let runs = service.list_runs(&probe_id, 0).await.unwrap().runs;
    let outcomes: Vec<Outcome> = runs
        .iter()
        .map(|r| r.run.as_ref().unwrap().outcome())
        .collect();
    assert_eq!(
        outcomes,
        [Outcome::Skipped, Outcome::Ok],
        "the newest first"
    );
}

#[tokio::test]
async fn a_job_runs_a_workflow_and_its_run_names_the_workflows_run() {
    let (docs, clock, world) = (
        documents(),
        Moving::at("2026-10-27T06:30:00Z"),
        Recorded::default(),
    );
    let service = Service::new(&docs, &clock);
    let weekly = Job {
        name: "weekly".into(),
        does: Some(Does::Workflow(WorkflowAction {
            name: "done-report".into(),
            input: None,
        })),
        ..probe()
    };
    let added = service.add_job(weekly).await.unwrap();
    let job = service.job(&added.id).await.unwrap();
    let run = service.start(&job, None).await.unwrap();
    let ended = service.work(&world, &job, run).await.unwrap().run.unwrap();
    assert_eq!(ended.outcome(), Outcome::Ok);
    assert_eq!(ended.result, "r-1");
    assert_eq!(world.workflows.lock().clone(), ["done-report"]);
}

/// A run of `job` at `at` (`scheduled`: due then; else by hand), written
/// then as memory writes it, through `world`.
async fn ran(
    docs: &InMemoryDocuments,
    clock: &Moving,
    world: &Recorded,
    job: &Item<Job>,
    at: &str,
    scheduled: bool,
) {
    clock.set(at);
    let then = docs.as_author("scheduler").written_at(clock.now());
    let service = Service::new(&then, clock);
    let run = service
        .start(job, scheduled.then(|| clock.now()))
        .await
        .unwrap();
    service.work(world, job, run).await.unwrap();
}

#[tokio::test]
async fn a_jobs_health_reads_its_runs_since_the_time_it_owes() {
    let (docs, clock) = (documents(), Moving::at("2026-10-27T06:30:00Z"));
    let (good, bad) = (
        Recorded::default(),
        Recorded {
            fail: true,
            ..Recorded::default()
        },
    );
    let service = Service::new(&docs, &clock);
    assert_eq!(service.health("probe").await.unwrap(), None, "no such job");
    let added = service.add_job(probe()).await.unwrap();
    let job = service.job(&added.id).await.unwrap();
    let late = |text: &str| {
        Some(domain::Health::Late(
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&Utc),
        ))
    };
    assert_eq!(
        service.health("probe").await.unwrap(),
        late("2026-10-27T06:15:00Z"),
        "it never ran"
    );

    ran(&docs, &clock, &good, &job, "2026-10-27T06:55:00Z", true).await;
    clock.set("2026-10-27T07:00:00Z");
    assert_eq!(
        service.health("probe").await.unwrap(),
        Some(domain::Health::Ok)
    );

    // Half an hour on, its good run is older than the time it owes; a run
    // that failed since is no run of that time.
    ran(&docs, &clock, &bad, &job, "2026-10-27T07:20:00Z", true).await;
    clock.set("2026-10-27T07:30:00Z");
    assert_eq!(
        service.health("probe").await.unwrap(),
        late("2026-10-27T07:15:00Z")
    );

    ran(&docs, &clock, &good, &job, "2026-10-27T07:31:00Z", false).await;
    assert_eq!(
        service.health("probe").await.unwrap(),
        Some(domain::Health::Ok),
        "a run by hand since then is a run"
    );
    // The job's next time is counted from its last scheduled run, found
    // behind the run by hand.
    let record = service.list_jobs().await.unwrap().jobs.remove(0);
    assert_eq!(
        record.last_run.unwrap().run.unwrap().due,
        None,
        "the newest"
    );
    assert_eq!(
        record.next.unwrap().seconds,
        DateTime::parse_from_rfc3339("2026-10-27T07:35:00Z")
            .unwrap()
            .timestamp()
    );
    assert_eq!(service.list_runs(&added.id, 2).await.unwrap().runs.len(), 2);
}

#[tokio::test]
async fn a_script_the_sandbox_refuses_is_a_failed_run() {
    let (docs, clock) = (documents(), Moving::at("2026-10-27T06:30:00Z"));
    let world = Recorded {
        fail: true,
        ..Recorded::default()
    };
    let service = Service::new(&docs, &clock);
    let added = service.add_job(probe()).await.unwrap();
    let job = service.job(&added.id).await.unwrap();
    let run = service.start(&job, None).await.unwrap();
    assert_eq!(run.data.due, None, "by hand");
    let ended = service.work(&world, &job, run).await.unwrap().run.unwrap();
    assert_eq!(ended.outcome(), Outcome::Failed);
    assert!(ended.result.contains("no spawn grant"));
    let record = service.list_jobs().await.unwrap().jobs.remove(0);
    assert_eq!(
        record.last_run.unwrap().run.unwrap().outcome(),
        Outcome::Failed
    );
}
