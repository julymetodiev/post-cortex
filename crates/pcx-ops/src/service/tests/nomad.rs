// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Actions in a Nomad environment: a new allocation, a periodic job
//! launched, jobs reverted.

use post_cortex_proto::ops::NomadPeriodic;

use super::*;

const ALLOCATION: &str = "7c4d5fa2-dc85-52f7-3a01-5c90f660cd2d";

fn version(job: &str, version: u32, stable: bool) -> NomadJob {
    NomadJob {
        id: job.into(),
        r#type: "service".into(),
        version,
        stable,
        submit_time: 1_794_425_336_132_195_478,
        ..NomadJob::default()
    }
}

/// pcx's production as Nomad has it: two released jobs with the version
/// before, the nightly backup, and the cloud's allocation.
fn nomad(trusted: bool) -> World {
    let world = world(trusted);
    {
        let mut versions = world.versions.lock();
        for (job, runs) in [("pcx-app", 32), ("pcx-cloud", 41)] {
            let kept = vec![version(job, runs, true), version(job, runs - 1, true)];
            versions.insert(job.to_string(), kept);
        }
        let backup = NomadJob {
            r#type: "batch".into(),
            stable: false,
            periodic: Some(NomadPeriodic { enabled: true }),
            ..version("surrealdb-backup", 2, false)
        };
        versions.insert("surrealdb-backup".to_string(), vec![backup]);
    }
    world.allocations.lock().push(NomadAlloc {
        id: ALLOCATION.into(),
        job_id: "pcx-cloud".into(),
        job_type: "service".into(),
        client_status: "running".into(),
    });
    world
}

fn on_pcx(wanted: Wanted, dry_run: bool) -> Asked {
    Asked {
        ask: Ask {
            environment: "pcx".into(),
            workspace_id: "pcx".into(),
            task: "t-9".into(),
            why: "api.pcx.is answers 502 since the release of 19:28".into(),
            dry_run,
        },
        wanted,
    }
}

fn revert(jobs: &[&str]) -> Wanted {
    Wanted::RevertJobs {
        jobs: jobs.iter().map(|j| (*j).to_string()).collect(),
    }
}

fn approve(world: &World) {
    let mut decisions = world.decisions.lock();
    for (_, _, record) in decisions.iter_mut() {
        record.pending = false;
        record.decision.as_mut().unwrap().approved_by = Some("julius".into());
    }
}

#[tokio::test]
async fn a_revert_waits_for_a_person_whatever_the_trust_then_runs_job_after_job() {
    let (config, docs, world) = (config(), documents(), nomad(true));
    let svc = service(&config, &docs, &world);
    let would = svc
        .act("pcx-medic", on_pcx(revert(&["pcx-app", "pcx-cloud"]), true))
        .await
        .unwrap()
        .would;
    assert_eq!(
        would,
        "would revert pcx-app from version 32 to 31, pcx-cloud from version 41 to 40; now: \
         pcx-app runs version 32 of 2026-11-11T19:28:56Z; version 31 is of 2026-11-11T19:28:56Z; \
         pcx-cloud runs version 41 of 2026-11-11T19:28:56Z; version 40 is of 2026-11-11T19:28:56Z"
    );
    let asked = svc
        .act(
            "pcx-medic",
            on_pcx(revert(&["pcx-app", "pcx-cloud"]), false),
        )
        .await
        .unwrap()
        .action
        .unwrap();
    assert_eq!(asked.state(), State::Waiting, "the trust approves it not");
    assert_eq!(asked.target, "pcx-app, pcx-cloud");
    let (_, decision, _) = world.decisions.lock()[0].clone();
    assert_eq!(decision.stakes.action.as_deref(), Some("revert_jobs"));
    assert_eq!(decision.stakes.person, Some(true));
    assert!(world.reverted.lock().is_empty());

    let allocation = Wanted::NewAllocation {
        allocation: "7c4d5fa2".into(),
    };
    let held = svc
        .act("pcx-medic", on_pcx(allocation, false))
        .await
        .unwrap_err();
    assert!(
        held.message.contains("waits for approval"),
        "the brake holds each job of the revert: {}",
        held.message
    );

    approve(&world);
    let ran = svc.decided("D-1").await.unwrap().unwrap().action.unwrap();
    assert_eq!(ran.state(), State::Done);
    assert_eq!(
        ran.result,
        "reverted pcx-app from version 32 to 31, pcx-cloud from version 41 to 40"
    );
    assert_eq!(
        *world.reverted.lock(),
        [
            Revert {
                job: "pcx-app".into(),
                from: 32,
                to: 31
            },
            Revert {
                job: "pcx-cloud".into(),
                from: 41,
                to: 40
            },
        ]
    );
}

#[tokio::test]
async fn a_revert_ends_at_the_first_job_nomad_does_not_revert() {
    let (config, docs) = (config(), documents());
    let world = World {
        unrevertible: Some("pcx-cloud"),
        ..nomad(false)
    };
    let svc = service(&config, &docs, &world);
    let jobs = revert(&["pcx-app", "pcx-cloud"]);
    svc.act("pcx-medic", on_pcx(jobs, false)).await.unwrap();
    approve(&world);
    let ran = svc.decided("D-1").await.unwrap().unwrap().action.unwrap();
    assert_eq!(ran.state(), State::Failed);
    assert_eq!(
        ran.result,
        "pcx-cloud was not reverted: Nomad refused /v1/job/x/revert: Permission denied; \
         reverted before it: pcx-app from version 32 to 31"
    );
    assert_eq!(world.reverted.lock().len(), 1);
}

#[tokio::test]
async fn a_revert_whose_versions_changed_before_its_approval_reverts_nothing() {
    let (config, docs, world) = (config(), documents(), nomad(false));
    let svc = service(&config, &docs, &world);
    let jobs = revert(&["pcx-app", "pcx-cloud"]);
    svc.act("pcx-medic", on_pcx(jobs, false)).await.unwrap();
    world
        .versions
        .lock()
        .get_mut("pcx-cloud")
        .unwrap()
        .insert(0, version("pcx-cloud", 42, true));
    approve(&world);
    let ran = svc.decided("D-1").await.unwrap().unwrap().action.unwrap();
    assert_eq!(ran.state(), State::Failed);
    assert!(
        ran.result
            .starts_with("the jobs' versions changed since it was asked"),
        "{}",
        ran.result
    );
    assert!(world.reverted.lock().is_empty(), "not even the first job");
}

#[tokio::test]
async fn a_job_that_may_not_be_reverted_is_refused_by_name() {
    let (config, docs, world) = (config(), documents(), nomad(true));
    let svc = service(&config, &docs, &world);
    let refused = |jobs: Wanted| async {
        let e = svc.act("pcx-medic", on_pcx(jobs, false)).await.unwrap_err();
        (e.kind, e.message)
    };
    world.deploying.lock().push("pcx-cloud".into());
    assert_eq!(
        refused(revert(&["pcx-app", "pcx-cloud"])).await,
        (
            ModuleErrorKind::FailedPrecondition,
            "pcx-cloud's deployment still runs (running)".to_string()
        )
    );
    world.deploying.lock().clear();
    world.versions.lock().get_mut("pcx-app").unwrap()[1].stable = false;
    assert_eq!(
        refused(revert(&["pcx-app"])).await.1,
        "pcx-app's version 31 before 32 is not stable"
    );
    assert_eq!(
        refused(revert(&["surrealdb-backup"])).await.1,
        "surrealdb-backup is a batch job: only a service job is reverted"
    );
    assert_eq!(
        refused(revert(&[])).await.0,
        ModuleErrorKind::InvalidArgument
    );
    assert_eq!(
        refused(revert(&["../../v1/acl/tokens"])).await.0,
        ModuleErrorKind::InvalidArgument
    );
    assert!(world.decisions.lock().is_empty() && world.reverted.lock().is_empty());
}

#[tokio::test]
async fn a_new_allocation_stops_the_running_one_of_a_service_job() {
    let (config, docs, world) = (config(), documents(), nomad(true));
    let svc = service(&config, &docs, &world);
    let short = || Wanted::NewAllocation {
        allocation: "7c4d5fa2-d".into(),
    };
    assert_eq!(
        svc.act("pcx-medic", on_pcx(short(), true))
            .await
            .unwrap()
            .would,
        "would stop allocation 7c4d5fa2 of pcx-cloud, so Nomad places a new one; now: it runs"
    );
    let done = svc
        .act("pcx-medic", on_pcx(short(), false))
        .await
        .unwrap()
        .action
        .unwrap();
    assert_eq!(done.state(), State::Done, "the trust approves it");
    assert_eq!(done.target, "pcx-cloud", "its job, which the brakes hold");
    assert_eq!(done.allocation, ALLOCATION);
    assert_eq!(*world.stopped.lock(), [ALLOCATION]);
    let (_, decision, _) = world.decisions.lock()[0].clone();
    assert_eq!(decision.stakes.action.as_deref(), Some("new_allocation"));
    assert_eq!(decision.stakes.person, None);

    world.allocations.lock()[0].client_status = "pending".into();
    let unknown = Wanted::NewAllocation {
        allocation: "00000000".into(),
    };
    assert_eq!(
        svc.act("pcx-medic", on_pcx(unknown, true))
            .await
            .unwrap_err()
            .message,
        "no allocation's id starts with 00000000"
    );
    let pending = Wanted::NewAllocation {
        allocation: ALLOCATION.into(),
    };
    assert!(
        svc.act("pcx-medic", on_pcx(pending, true))
            .await
            .unwrap_err()
            .message
            .ends_with("of pcx-cloud is pending, not running")
    );
}

#[tokio::test]
async fn a_periodic_job_is_launched_when_no_launch_of_it_runs() {
    let (config, docs, world) = (config(), documents(), nomad(true));
    let svc = service(&config, &docs, &world);
    let backup = || Wanted::RunPeriodic {
        job: "surrealdb-backup".into(),
    };
    let done = svc
        .act("pcx-medic", on_pcx(backup(), false))
        .await
        .unwrap()
        .action
        .unwrap();
    assert_eq!(done.state(), State::Done);
    assert_eq!(done.what, "launch periodic job surrealdb-backup now");
    assert_eq!(*world.launched.lock(), ["surrealdb-backup"]);

    let (config, docs, world) = (config.clone(), documents(), nomad(true));
    let svc = service(&config, &docs, &world);
    world.launches.lock().insert(
        "surrealdb-backup".into(),
        NomadChildren {
            pending: 0,
            running: 1,
        },
    );
    assert_eq!(
        svc.act("pcx-medic", on_pcx(backup(), false))
            .await
            .unwrap_err()
            .message,
        "a launch of surrealdb-backup runs already"
    );
    let service_job = Wanted::RunPeriodic {
        job: "pcx-cloud".into(),
    };
    assert_eq!(
        svc.act("pcx-medic", on_pcx(service_job, false))
            .await
            .unwrap_err()
            .message,
        "pcx-cloud is no periodic job"
    );
    assert!(world.launched.lock().is_empty());
}

#[tokio::test]
async fn a_job_the_environment_does_not_name_is_a_persons() {
    let (config, docs, world) = (config(), documents(), nomad(true));
    let svc = service(&config, &docs, &world);
    let database = "0d1e2f3a-dc85-52f7-3a01-5c90f660cd2d";
    world.allocations.lock().push(NomadAlloc {
        id: database.into(),
        job_id: "surrealdb".into(),
        job_type: "service".into(),
        client_status: "running".into(),
    });
    let kept = vec![version("surrealdb", 7, true), version("surrealdb", 6, true)];
    world.versions.lock().insert("surrealdb".to_string(), kept);
    let why = "surrealdb is none of the jobs ops acts on in pcx (pcx-app, pcx-cloud, \
               surrealdb-backup): a person acts on it";
    for wanted in [
        Wanted::NewAllocation {
            allocation: database.into(),
        },
        Wanted::RunPeriodic {
            job: "surrealdb".into(),
        },
        revert(&["pcx-app", "surrealdb"]),
    ] {
        for dry_run in [true, false] {
            let refused = svc
                .act("pcx-medic", on_pcx(wanted.clone(), dry_run))
                .await
                .unwrap_err();
            assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
            assert_eq!(refused.message, why);
        }
    }
    assert!(world.decisions.lock().is_empty());
    assert!(world.stopped.lock().is_empty() && world.reverted.lock().is_empty());
    assert!(world.launched.lock().is_empty());
}

#[tokio::test]
async fn an_action_is_asked_of_a_cluster_of_its_kind() {
    let (config, docs, world) = (config(), documents(), nomad(true));
    let svc = service(&config, &docs, &world);
    let restart_on_nomad = Asked {
        wanted: restart(false).wanted,
        ..on_pcx(revert(&["pcx-app"]), false)
    };
    let refused = svc.act("pcx-medic", restart_on_nomad).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
    assert_eq!(
        refused.message,
        "environment pcx is Nomad: its actions are ops_new_allocation, ops_run_periodic and \
         ops_revert_jobs"
    );
    let revert_on_kube = Asked {
        wanted: revert(&["pcx-app"]),
        ..restart(false)
    };
    let refused = svc.act("eq-medic", revert_on_kube).await.unwrap_err();
    assert!(
        refused
            .message
            .starts_with("environment prod is Kubernetes"),
        "{}",
        refused.message
    );
    assert!(world.decisions.lock().is_empty());
}
