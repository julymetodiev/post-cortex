// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::collections::HashMap;

use post_cortex_proto::observe::{NomadMemory, NomadPeriodic, NomadTaskEvent};

use super::*;

fn stub(id: &str, parent: &str, kind: &str, status: &str, submitted: i64) -> NomadJobStub {
    NomadJobStub {
        id: id.into(),
        parent_id: parent.into(),
        r#type: kind.into(),
        status: status.into(),
        submit_time: submitted,
    }
}

fn running(id: &str, job: &str, node: &str, made: i64, tasks: &[&str]) -> NomadAllocation {
    NomadAllocation {
        id: id.into(),
        job_id: job.into(),
        job_version: 41,
        node_name: node.into(),
        client_status: "running".into(),
        create_time: made,
        task_states: tasks
            .iter()
            .map(|task| {
                let state = NomadTaskState {
                    state: "running".into(),
                    failed: false,
                    restarts: 1,
                    events: ["Received", "Task Setup", "Started", "Restarting"]
                        .iter()
                        .map(|kind| NomadTaskEvent {
                            r#type: (*kind).into(),
                            display_message: "as Nomad says it".into(),
                            time: 1_794_425_365_233_331_140,
                        })
                        .collect(),
                };
                ((*task).to_string(), state)
            })
            .collect::<HashMap<_, _>>(),
    }
}

#[test]
fn a_job_answers_with_its_version_allocations_and_launches() {
    let stubs = [
        stub(
            "surrealdb-backup/periodic-1794300000",
            "surrealdb-backup",
            "batch",
            "dead",
            1_794_300_000_000_000_000,
        ),
        stub(
            "surrealdb-backup",
            "",
            "batch",
            "running",
            1_793_341_168_808_578_424,
        ),
        stub(
            "pcx-cloud",
            "",
            "service",
            "running",
            1_794_425_336_132_195_478,
        ),
    ];
    let details = [
        NomadJob {
            id: "pcx-cloud".into(),
            version: 41,
            stable: true,
            ..NomadJob::default()
        },
        NomadJob {
            id: "surrealdb-backup".into(),
            version: 2,
            periodic: Some(NomadPeriodic {
                enabled: true,
                spec: String::new(),
                specs: vec!["30 2 * * *".into()],
                time_zone: "Europe/Sofia".into(),
            }),
            ..NomadJob::default()
        },
    ];
    let allocations = [
        running(
            "0a000000-0000-0000-0000-000000000000",
            "pcx-cloud",
            "freebsd",
            1,
            &["cloud"],
        ),
        running(
            "7c4d5fa2-dc85-52f7-3a01-5c90f660cd2d",
            "pcx-cloud",
            "freebsd",
            1_794_425_336_137_950_464,
            &["modules", "cloud"],
        ),
        NomadAllocation {
            client_status: "complete".into(),
            ..running(
                "b0000000-0000-0000-0000-000000000000",
                "surrealdb-backup/periodic-1794300000",
                "freebsd-3",
                1_794_300_000_000_000_000,
                &[],
            )
        },
    ];
    let jobs = jobs(&stubs, &details, &allocations);
    assert_eq!(
        jobs.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(),
        ["pcx-cloud", "surrealdb-backup"],
        "by name, a launch is no job of its own"
    );
    let cloud = &jobs[0];
    assert_eq!((cloud.version, cloud.stable), (41, true));
    assert_eq!(cloud.submitted, "2026-11-11T19:28:56Z");
    assert_eq!(cloud.allocations.len(), 2);
    let newest = &cloud.allocations[0];
    assert_eq!(newest.id, "7c4d5fa2-dc85-52f7-3a01-5c90f660cd2d");
    assert_eq!(newest.since, "2026-11-11T19:28:56Z");
    assert_eq!(newest.restarts, 2, "of its tasks together");
    assert_eq!(
        newest
            .tasks
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["cloud", "modules"]
    );
    assert_eq!(
        newest.tasks[0].events,
        [
            "2026-11-11T19:29:25Z Task Setup: as Nomad says it",
            "2026-11-11T19:29:25Z Started: as Nomad says it",
            "2026-11-11T19:29:25Z Restarting: as Nomad says it",
        ],
        "the last three"
    );
    let backup = &jobs[1];
    assert_eq!(backup.schedule, ["30 2 * * *"]);
    assert_eq!(backup.time_zone, "Europe/Sofia");
    assert_eq!(backup.launches.len(), 1);
    assert_eq!(backup.launches[0].launched, "2026-11-10T08:40:00Z");
    assert_eq!(backup.launches[0].allocations[0].state, "complete");
    assert!(backup.allocations.is_empty());
}

#[test]
fn a_log_is_asked_of_one_allocation_and_one_task() {
    let two = running(
        "7c4d5fa2-dc85",
        "pcx-cloud",
        "freebsd",
        1,
        &["cloud", "modules"],
    );
    let other = running("7c4d5fa2-ffff", "pcx-app", "freebsd", 1, &["app"]);
    assert_eq!(
        allocation_of(vec![two.clone(), other.clone()], "7c4d5fa2-d")
            .unwrap()
            .id,
        "7c4d5fa2-dc85",
        "Nomad answered a shorter start: the one asked is picked"
    );
    assert_eq!(
        allocation_of(vec![two.clone(), other.clone()], "7c4d5fa2"),
        Err(Refused::Allocations {
            start: "7c4d5fa2".into(),
            count: 2
        })
    );
    assert_eq!(
        allocation_of(Vec::new(), "7c4d5fa2"),
        Err(Refused::NoAllocation("7c4d5fa2".into()))
    );
    assert_eq!(task_of(&other, "").unwrap(), "app");
    assert_eq!(task_of(&two, "modules").unwrap(), "modules");
    assert_eq!(
        task_of(&two, ""),
        Err(Refused::Tasks(vec!["cloud".into(), "modules".into()]))
    );
    assert_eq!(
        task_of(&two, "app"),
        Err(Refused::Tasks(vec!["cloud".into(), "modules".into()]))
    );
    let pending = running("7c4d5fa2-0000", "pcx-app", "freebsd", 1, &[]);
    assert_eq!(task_of(&pending, "app").unwrap(), "app");
    assert_eq!(task_of(&pending, ""), Err(Refused::Empty("task")));
}

#[test]
fn the_end_of_a_log_is_its_last_whole_lines() {
    assert_eq!(tail("a\nb\nc\n", 2, false), "b\nc");
    assert_eq!(tail("a\nb\nc\n", 9, false), "a\nb\nc");
    assert_eq!(
        tail("alf a line\nb\nc", 9, true),
        "b\nc",
        "cut inside a line"
    );
    assert_eq!(tail("", 5, true), "");
}

fn mounted(device: &str, size: u64, used: u64, available: u64) -> NomadDisk {
    NomadDisk {
        device: device.into(),
        size,
        used,
        available,
    }
}

#[test]
fn a_hosts_disks_are_its_devices_and_its_pools() {
    // As Nomad 1.9.6 answers on FreeBSD with pot (2026-11-12): every ZFS
    // dataset, every nullfs mount of a jail and devfs are filesystems.
    let stats = NomadHostStats {
        memory: Some(NomadMemory {
            total: 12_836_077_568,
            available: 10_127_331_328,
        }),
        disk_stats: vec![
            mounted("/dev/da0s1a", 19_742_486_528, 2_656_923_648, 15_506_165_760),
            mounted("devfs", 1024, 0, 1024),
            mounted(
                "potpool/pot/cache",
                46_792_499_200,
                1_017_225_216,
                45_775_273_984,
            ),
            mounted("potpool", 45_775_372_288, 98_304, 45_775_273_984),
            mounted(
                "potpool/pot/fscomp/tikv-data",
                47_575_449_600,
                678_481_920,
                46_896_967_680,
            ),
            mounted(
                "/opt/pot/fscomp/tikv-data",
                47_575_449_600,
                678_481_920,
                46_896_967_680,
            ),
            mounted("/dev/da0s1a", 19_742_486_528, 2_656_923_648, 15_506_165_760),
        ],
    };
    let listed = NomadNode {
        id: "4c7ca7b2".into(),
        name: "freebsd-3".into(),
        status: "ready".into(),
        scheduling_eligibility: "eligible".into(),
        drain: false,
        version: "1.9.6".into(),
    };
    let read = node(&listed, Ok(stats));
    assert!(read.eligible && !read.draining && read.unread.is_empty());
    assert_eq!(read.memory_available, 10_127_331_328);
    assert_eq!(
        read.disks,
        [
            DiskInfo {
                name: "/dev/da0s1a".into(),
                size: 19_742_486_528,
                used: 2_656_923_648,
                used_percent: 13,
            },
            DiskInfo {
                name: "potpool".into(),
                size: 47_471_079_424,
                used: 1_695_805_440,
                used_percent: 3,
            },
        ],
        "a pool is what its datasets use and what its own dataset has available"
    );
    let down = node(
        &NomadNode {
            status: "down".into(),
            scheduling_eligibility: "ineligible".into(),
            ..listed
        },
        Err("Nomad refused: no path to node".into()),
    );
    assert!(!down.eligible && down.disks.is_empty());
    assert_eq!(down.unread, "Nomad refused: no path to node");
}
