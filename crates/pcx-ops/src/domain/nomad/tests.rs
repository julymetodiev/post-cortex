// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use chrono::Utc;
use post_cortex_proto::ops::{Action, Kind, NomadPeriodic, OpsConfig, State};

use super::*;

use crate::domain::{Wanted, brake, stakes, targets, wanted, what};

fn version(version: u32, stable: bool) -> NomadJob {
    NomadJob {
        id: "pcx-cloud".into(),
        r#type: SERVICE.into(),
        version,
        stable,
        submit_time: 1_794_425_336_132_195_478,
        ..NomadJob::default()
    }
}

fn status(status: &str) -> NomadDeployment {
    NomadDeployment {
        status: status.into(),
    }
}

#[test]
fn a_job_goes_back_to_its_newest_stable_version_before() {
    // Nomad keeps the last versions; a revert made one of its own, so the
    // numbers before need not follow each other.
    let kept = [version(41, true), version(39, true), version(38, true)];
    let done = status("successful");
    assert_eq!(
        revert(&kept[0], &kept, Some(&done)),
        Ok(Revert {
            job: "pcx-cloud".into(),
            from: 41,
            to: 39
        })
    );
    assert_eq!(
        revert(&kept[0], &kept, None).map(|r| r.to),
        Ok(39),
        "a job that never had a deployment"
    );
    let why = |job: &NomadJob, kept: &[NomadJob], deployment: &str| match revert(
        job,
        kept,
        Some(&status(deployment)),
    ) {
        Err(Refused::Target(why)) => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        why(&kept[0], &kept[..1], "successful"),
        "Nomad keeps no version of pcx-cloud before 41"
    );
    assert_eq!(
        why(&kept[0], &[version(41, true), version(40, false)], "failed"),
        "pcx-cloud's version 40 before 41 is not stable"
    );
    assert_eq!(
        why(&kept[0], &kept, "running"),
        "pcx-cloud's deployment still runs (running)"
    );
    let stopped = NomadJob {
        stop: true,
        ..version(41, true)
    };
    assert_eq!(why(&stopped, &kept, "successful"), "pcx-cloud is stopped");
    assert_eq!(
        dated(
            &Revert {
                job: "pcx-cloud".into(),
                from: 41,
                to: 39
            },
            &kept
        ),
        "pcx-cloud runs version 41 of 2026-11-11T19:28:56Z; version 39 is of 2026-11-11T19:28:56Z"
    );
}

#[test]
fn nomads_json_is_read_and_written_as_nomad_names_it() {
    // As Nomad 1.9.6 answers a job (pcx's production, 2026-11-12), cut.
    let job: NomadJob = pcx_module_api::json::decode(
        br#"{"ID":"surrealdb-backup","Type":"batch","Stop":false,"Version":2,"Stable":false,
            "SubmitTime":1793341168808578424,"ParentID":"","Multiregion":null,
            "Periodic":{"Enabled":true,"ProhibitOverlap":true,"Spec":"","SpecType":"cron",
                        "Specs":["30 2 * * *"],"TimeZone":"Europe/Sofia"},
            "TaskGroups":[{"Name":"backup","Tasks":[{"Name":"backup","Type":"x"}]}]}"#,
    )
    .unwrap();
    assert_eq!((job.id.as_str(), job.version), ("surrealdb-backup", 2));
    assert_eq!(job.submit_time, 1_793_341_168_808_578_424);
    assert_eq!(job.periodic, Some(NomadPeriodic { enabled: true }));
    let none: Option<NomadDeployment> = pcx_module_api::json::decode(b"null").unwrap();
    assert_eq!(none, None);
    let sent = serde_json::to_string(&post_cortex_proto::ops::NomadRevert {
        job_id: "pcx-cloud".into(),
        job_version: 40,
        enforce_prior_version: 41,
    })
    .unwrap();
    assert_eq!(
        sent,
        r#"{"JobID":"pcx-cloud","JobVersion":40,"EnforcePriorVersion":41}"#
    );
}

#[test]
fn a_revert_names_one_to_five_jobs_each_once() {
    let jobs = |names: &[&str]| -> Vec<String> { names.iter().map(|n| (*n).to_string()).collect() };
    assert_eq!(revertible(&jobs(&["pcx-app", "pcx-cloud"])), Ok(()));
    assert!(matches!(revertible(&[]), Err(Refused::Bounds(_))));
    assert!(matches!(
        revertible(&jobs(&["a", "b", "c", "d", "e", "f"])),
        Err(Refused::Bounds(_))
    ));
    assert_eq!(
        revertible(&jobs(&["pcx-app", "pcx-app"])),
        Err(Refused::Bounds("a revert names pcx-app twice".into()))
    );
    assert_eq!(
        revertible(&jobs(&["pcx-app", "a/b"])),
        Err(Refused::NomadName("job"))
    );
    assert_eq!(allocation_named(" 7c4d5fa2 "), Ok("7c4d5fa2"));
    assert_eq!(allocation_named("7c4d"), Err(Refused::AllocationId));
}

#[test]
fn a_periodic_job_is_launched_only_when_none_of_its_launches_runs() {
    let backup = NomadJob {
        id: "surrealdb-backup".into(),
        r#type: "batch".into(),
        periodic: Some(NomadPeriodic { enabled: true }),
        ..NomadJob::default()
    };
    let none = NomadChildren::default();
    assert_eq!(launchable(&backup, &none), Ok(()));
    let waits = NomadChildren {
        pending: 1,
        running: 0,
    };
    assert!(launchable(&backup, &waits).is_err());
    let off = NomadJob {
        periodic: Some(NomadPeriodic { enabled: false }),
        ..backup.clone()
    };
    assert!(launchable(&off, &none).is_err());
    let stopped = NomadJob {
        stop: true,
        ..backup
    };
    assert!(launchable(&stopped, &none).is_err());
}

#[test]
fn only_the_one_running_allocation_of_a_service_job_is_replaced() {
    let allocation = |id: &str, kind: &str, state: &str| NomadAlloc {
        id: id.into(),
        job_id: "pcx-cloud".into(),
        job_type: kind.into(),
        client_status: state.into(),
    };
    let found = vec![
        allocation("7c4d5fa2-dc85", SERVICE, "running"),
        allocation("7c4d5fa2-ffff", SERVICE, "running"),
    ];
    assert_eq!(
        replaceable(found.clone(), "7c4d5fa2-d").map(|a| a.id),
        Ok("7c4d5fa2-dc85".to_string())
    );
    assert!(replaceable(found, "7c4d5fa2").is_err(), "two start so");
    assert!(replaceable(vec![allocation("7c4d5fa2", "batch", "running")], "7c4d5fa2").is_err());
    assert!(replaceable(vec![allocation("7c4d5fa2", SERVICE, "failed")], "7c4d5fa2").is_err());
}

#[test]
fn a_kept_action_wants_what_it_was_asked_and_brakes_each_of_its_targets() {
    let reverts = vec![
        Revert {
            job: "pcx-app".into(),
            from: 32,
            to: 31,
        },
        Revert {
            job: "pcx-cloud".into(),
            from: 41,
            to: 40,
        },
    ];
    let waiting = Action {
        kind: Kind::RevertJobs.into(),
        what: "revert both".into(),
        environment: "pcx".into(),
        target: "pcx-app, pcx-cloud".into(),
        state: State::Waiting.into(),
        reverts: reverts.clone(),
        ..Action::default()
    };
    assert_eq!(
        wanted(&waiting),
        Ok(Wanted::RevertJobs {
            jobs: vec!["pcx-app".into(), "pcx-cloud".into()]
        })
    );
    assert_eq!(targets(&waiting.target, &reverts), ["pcx-app", "pcx-cloud"]);
    assert_eq!(
        targets("equal-rust/svc-music", &[]),
        ["equal-rust/svc-music"]
    );
    let config = OpsConfig::default();
    let kept = [waiting];
    let held = brake(&config, &kept, "pcx", "pcx-cloud", Utc::now());
    assert!(matches!(held, Err(Refused::Brake(_))), "{held:?}");
    assert_eq!(
        brake(&config, &kept, "pcx", "surrealdb", Utc::now()),
        Ok(())
    );
    assert_eq!(
        brake(&config, &kept, "prod", "pcx-cloud", Utc::now()),
        Ok(())
    );

    let scale = Action {
        kind: Kind::Scale.into(),
        target: "equal-rust/svc-music".into(),
        amount: 3,
        ..Action::default()
    };
    assert_eq!(
        wanted(&scale),
        Ok(Wanted::Scale {
            namespace: "equal-rust".into(),
            name: "svc-music".into(),
            replicas: 3
        })
    );
    assert_eq!(wanted(&Action::default()), Err(Refused::Empty("kind")));
    assert_eq!(stakes(Kind::RevertJobs, "pcx", "a, b").person, Some(true));
    assert_eq!(stakes(Kind::NewAllocation, "pcx", "a").person, None);
    let planned = crate::domain::Planned {
        reverts,
        ..crate::domain::Planned::of(Kind::RevertJobs, "pcx-app, pcx-cloud".into(), String::new())
    };
    assert_eq!(
        what(&planned),
        "revert pcx-app from version 32 to 31, pcx-cloud from version 41 to 40"
    );
}
