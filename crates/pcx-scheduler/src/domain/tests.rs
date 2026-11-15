// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use post_cortex_proto::scheduler::Script;

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn cron(expression: &str, zone: &str) -> Job {
    Job {
        name: "daily-report".into(),
        when: Some(When::Cron(Cron {
            expression: expression.into(),
            time_zone: zone.into(),
        })),
        does: Some(Does::Script(Script {
            program: "true".into(),
            ..Script::default()
        })),
        ..Job::default()
    }
}

fn every(seconds: u32) -> Job {
    Job {
        when: Some(When::EverySeconds(seconds)),
        ..cron("* * * * *", "")
    }
}

#[test]
fn a_job_has_a_name_a_time_and_something_to_do() {
    assert_eq!(check(&cron("0 9 * * *", "Europe/Sofia")), Ok(()));
    assert_eq!(check(&every(300)), Ok(()));
    let named = |name: &str| Job {
        name: name.into(),
        ..every(300)
    };
    assert_eq!(check(&named("Daily Report")), Err(Broken::Name));
    assert_eq!(check(&named("")), Err(Broken::Name));
    assert!(matches!(check(&cron("0 9 * *", "")), Err(Broken::Cron(_))));
    assert!(matches!(
        check(&cron("0 9 * * * *", "")),
        Err(Broken::Cron(_))
    ));
    assert!(matches!(
        check(&cron("61 9 * * *", "")),
        Err(Broken::Cron(_))
    ));
    assert_eq!(
        check(&cron("0 9 * * *", "Mars/Olympus")),
        Err(Broken::TimeZone("Mars/Olympus".into()))
    );
    assert_eq!(check(&every(59)), Err(Broken::TooOften(59)));
    let nothing = Job {
        does: None,
        ..every(300)
    };
    assert_eq!(check(&nothing), Err(Broken::NoDoes));
    let no_program = Job {
        does: Some(Does::Script(Script::default())),
        ..every(300)
    };
    assert_eq!(check(&no_program), Err(Broken::NoProgram));
    let no_board = Job {
        does: Some(Does::Task(TaskTemplate {
            workspace_id: "ws".into(),
            title: "Report".into(),
            ..TaskTemplate::default()
        })),
        ..every(300)
    };
    assert_eq!(check(&no_board), Err(Broken::Task("board")));
    let no_workflow = Job {
        does: Some(Does::Workflow(
            post_cortex_proto::scheduler::WorkflowAction::default(),
        )),
        ..every(300)
    };
    assert_eq!(check(&no_workflow), Err(Broken::Workflow));
}

#[test]
fn a_name_is_free_but_for_the_job_itself() {
    let jobs = [("j1", "probe"), ("j2", "sweep")];
    assert_eq!(free("report", None, jobs.into_iter()), Ok(()));
    assert_eq!(
        free("probe", None, jobs.into_iter()),
        Err(Broken::Taken("probe".into()))
    );
    assert_eq!(free("probe", Some("j1"), jobs.into_iter()), Ok(()));
    assert!(free("probe", Some("j2"), jobs.into_iter()).is_err());
}

#[test]
fn a_cron_job_is_due_at_its_time_in_its_zone() {
    // 09:00 in Sofia is 07:00 UTC in October (EEST, +03:00 until the 25th;
    // then EET, +02:00): 2026-10-27 is in winter time.
    let job = cron("0 9 * * *", "Europe/Sofia");
    let anchor = utc("2026-10-27T06:00:00Z");
    assert_eq!(due(&job, anchor, utc("2026-10-27T06:59:59Z")), Ok(None));
    assert_eq!(
        due(&job, anchor, utc("2026-10-27T07:00:10Z")),
        Ok(Some(utc("2026-10-27T07:00:00Z")))
    );
    assert_eq!(
        next(&job, anchor, utc("2026-10-27T07:00:10Z")),
        Ok(Some(utc("2026-10-28T07:00:00Z")))
    );
    // Run at 07:00: not due again the same day.
    assert_eq!(
        due(
            &job,
            utc("2026-10-27T07:00:00Z"),
            utc("2026-10-27T12:00:00Z")
        ),
        Ok(None)
    );
}

#[test]
fn after_a_long_stop_a_job_runs_once_for_its_latest_time() {
    let job = cron("*/5 * * * *", "");
    let anchor = utc("2026-10-27T07:00:00Z");
    assert_eq!(
        due(&job, anchor, utc("2026-10-27T09:03:00Z")),
        Ok(Some(utc("2026-10-27T09:00:00Z"))),
        "two hours missed: one run, for 09:00"
    );
    let probe = every(300);
    assert_eq!(
        due(&probe, anchor, utc("2026-10-27T07:17:00Z")),
        Ok(Some(utc("2026-10-27T07:15:00Z")))
    );
    assert_eq!(due(&probe, anchor, utc("2026-10-27T07:04:59Z")), Ok(None));
    assert_eq!(
        next(&probe, anchor, utc("2026-10-27T07:17:00Z")),
        Ok(Some(utc("2026-10-27T07:20:00Z")))
    );
}

#[test]
fn a_job_is_late_when_no_run_succeeded_since_a_time_long_past() {
    let changed = utc("2026-11-11T08:00:00Z");
    let probe = every(300);
    let at = |now: &str, runs: &[(&str, Outcome)]| {
        health(
            &probe,
            changed,
            runs.iter().map(|(when, outcome)| (utc(when), *outcome)),
            utc(now),
        )
        .unwrap()
    };
    // No time of its schedule passed 15 minutes ago yet.
    assert_eq!(at("2026-11-11T08:19:00Z", &[]), Health::Ok);
    // 08:05 passed 15 minutes ago and nothing ran since.
    let owed = Health::Late(utc("2026-11-11T08:05:00Z"));
    assert_eq!(at("2026-11-11T08:20:00Z", &[]), owed);
    assert_eq!(
        owed.to_string(),
        "no run of it succeeded since it was due at 2026-11-11T08:05:00Z"
    );
    // One failed run among good ones is not late; three in a row are.
    let mixed = [
        ("2026-11-11T08:05:00Z", Outcome::Ok),
        ("2026-11-11T08:10:00Z", Outcome::Failed),
        ("2026-11-11T08:15:00Z", Outcome::Ok),
    ];
    assert_eq!(at("2026-11-11T08:31:00Z", &mixed), Health::Ok);
    let failing = [
        ("2026-11-11T08:05:00Z", Outcome::Ok),
        ("2026-11-11T08:10:00Z", Outcome::Failed),
        ("2026-11-11T08:15:00Z", Outcome::Failed),
        ("2026-11-11T08:20:00Z", Outcome::Skipped),
    ];
    assert_eq!(
        at("2026-11-11T08:31:00Z", &failing),
        Health::Late(utc("2026-11-11T08:15:00Z"))
    );
    // A run that still runs is a run of its time.
    let running = [("2026-11-11T08:15:00Z", Outcome::Running)];
    assert_eq!(at("2026-11-11T08:31:00Z", &running), Health::Ok);
    let paused = Job {
        paused: true,
        ..every(300)
    };
    assert_eq!(
        health(&paused, changed, [], utc("2026-11-11T09:00:00Z")),
        Ok(Health::Paused)
    );
}

#[test]
fn a_paused_job_is_never_due() {
    let paused = Job {
        paused: true,
        ..every(60)
    };
    let anchor = utc("2026-10-27T07:00:00Z");
    assert_eq!(due(&paused, anchor, utc("2026-10-27T09:00:00Z")), Ok(None));
    assert_eq!(next(&paused, anchor, utc("2026-10-27T09:00:00Z")), Ok(None));
}

#[test]
fn a_runs_task_is_keyed_by_its_job_and_time() {
    let template = TaskTemplate {
        workspace_id: "ws".into(),
        board: "b".into(),
        title: "Daily report".into(),
        assignee: "eq-sentinel".into(),
        ..TaskTemplate::default()
    };
    let task = task(&template, "daily-report", utc("2026-10-27T07:00:00Z"));
    assert_eq!(
        task.key.as_deref(),
        Some("scheduler:daily-report:1793084400")
    );
    assert_eq!(task.assignee.as_deref(), Some("eq-sentinel"));
    assert_eq!(task.board, "b");
    let unassigned = TaskTemplate {
        assignee: " ".into(),
        ..template
    };
    assert_eq!(task_of(&unassigned).assignee, None);
}

fn task_of(template: &TaskTemplate) -> AddTaskRequest {
    task(template, "j", utc("2026-10-27T07:00:00Z"))
}

#[test]
fn a_scripts_end_is_its_outcome_and_the_tail_of_its_output() {
    assert_eq!(
        script_end(Ended::Code(0), " ok \n"),
        (Outcome::Ok, Some(0), "ok".into())
    );
    assert_eq!(
        script_end(Ended::Code(2), "no"),
        (Outcome::Failed, Some(2), "no".into())
    );
    let (outcome, code, result) = script_end(Ended::TimedOut, "slow");
    assert_eq!((outcome, code), (Outcome::Failed, None));
    assert!(result.starts_with("timed out"));
    let long = "é".repeat(OUTPUT_TAIL);
    let (_, _, kept) = script_end(Ended::Code(0), &long);
    assert!(kept.len() <= OUTPUT_TAIL && kept.chars().all(|c| c == 'é'));
}
