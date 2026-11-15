// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A Nomad environment read, Nomad answering as 1.9.6 does (pcx's own
//! production, 2026-11-12).

use post_cortex_proto::observe::{AllocLogRequest, JobsRequest, NodesRequest};

use super::*;

const ALLOCATION: &str = "7c4d5fa2-dc85-52f7-3a01-5c90f660cd2d";

fn nomad() -> ObserveConfig {
    ObserveConfig {
        environments: vec![
            Environment {
                name: "pcx".into(),
                nomad_api: "https://nomad:4646/".into(),
                nomad_token_file: "/creds/pcx.token".into(),
                nomad_ca_file: "/creds/pcx-ca.pem".into(),
                nomad_tls_server_name: "server.global.nomad".into(),
                ..Environment::default()
            },
            Environment {
                name: "prod".into(),
                kube_api: "https://k8s:6443".into(),
                kube_token_file: "/creds/prod.token".into(),
                ..Environment::default()
            },
        ],
    }
}

/// The allocation list as Nomad answers it: an empty list and a time not
/// reached yet are null.
fn allocations() -> String {
    format!(
        r#"[{{"ID":"{ALLOCATION}","JobID":"pcx-cloud","JobType":"service","JobVersion":41,
            "NodeName":"freebsd","ClientStatus":"running","CreateTime":1794425336137950464,
            "PreemptedAllocations":null,
            "TaskStates":{{
              "cloud":{{"State":"running","Failed":false,"Restarts":0,"FinishedAt":null,
                "LastRestart":null,"StartedAt":"2026-11-11T19:29:25.233367478Z",
                "Events":[{{"Type":"Started","DisplayMessage":"Task started by client",
                           "Time":1794425365233331140,"Details":{{}}}}]}},
              "modules":{{"State":"running","Failed":false,"Restarts":0,"Events":null}}}}}}]"#
    )
}

#[tokio::test]
async fn nomads_jobs_are_answered_with_their_allocations() {
    let apis = Answers::default()
        .with(
            "https://nomad:4646/v1/jobs",
            true,
            r#"[{"ID":"pcx-cloud","ParentID":"","Type":"service","Status":"running",
                 "Periodic":false,"SubmitTime":1794425336132195478,"Datacenters":["*"],
                 "Multiregion":null,"JobSummary":{"Children":{"Dead":0}}}]"#,
        )
        .with(
            "https://nomad:4646/v1/job/pcx-cloud",
            true,
            r#"{"ID":"pcx-cloud","Type":"service","Status":"running","Stop":false,"Version":41,
                "Stable":true,"SubmitTime":1794425336132195478,"Periodic":null,
                "TaskGroups":[{"Name":"cloud","Tasks":[{"Name":"cloud","Env":null}]}]}"#,
        )
        .with("https://nomad:4646/v1/allocations", true, &allocations());
    let config = nomad();
    let service = Service::new(&config, &apis, &At);
    let jobs = service
        .jobs(JobsRequest {
            environment: "pcx".into(),
        })
        .await
        .unwrap()
        .jobs;
    assert_eq!(jobs.len(), 1);
    assert_eq!((jobs[0].version, jobs[0].stable), (41, true));
    let allocation = &jobs[0].allocations[0];
    assert_eq!(allocation.node, "freebsd");
    assert_eq!(allocation.tasks.len(), 2);
    assert_eq!(
        allocation.tasks[0].events,
        ["2026-11-11T19:29:25Z Started: Task started by client"]
    );
    assert!(
        apis.asked
            .lock()
            .iter()
            .all(|(api, _, _)| *api == Api::Nomad),
        "with Nomad's credentials"
    );
}

#[tokio::test]
async fn a_tasks_log_is_read_by_the_start_of_its_allocations_id() {
    let log = format!("https://nomad:4646/v1/client/fs/logs/{ALLOCATION}");
    let apis = Answers::default()
        .with("https://nomad:4646/v1/allocations", true, &allocations())
        .with(&log, true, "one\ntwo\nthree\n");
    let config = nomad();
    let service = Service::new(&config, &apis, &At);
    let ask = |task: &str| AllocLogRequest {
        environment: "pcx".into(),
        allocation: "7c4d5fa2-d".into(),
        task: task.into(),
        stderr: true,
        tail: 2,
    };
    let tail = service.alloc_log(ask("cloud")).await.unwrap();
    assert_eq!(tail.text, "two\nthree");
    let asked = apis.asked.lock().clone();
    assert_eq!(
        asked[0].2,
        [("prefix".to_string(), "7c4d5fa2".to_string())],
        "Nomad looks an id up by an even number of digits"
    );
    assert_eq!(asked[1].1, log);
    assert!(
        asked[1]
            .2
            .contains(&("task".to_string(), "cloud".to_string()))
    );
    assert!(
        asked[1]
            .2
            .contains(&("type".to_string(), "stderr".to_string()))
    );

    let unnamed = service.alloc_log(ask("")).await.unwrap_err();
    assert_eq!(unnamed.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(
        unnamed.message,
        "the allocation's tasks are cloud, modules: name one of them"
    );
    let traversal = AllocLogRequest {
        allocation: "../../v1/acl/tokens".into(),
        ..ask("cloud")
    };
    assert_eq!(
        service.alloc_log(traversal).await.unwrap_err().kind,
        ModuleErrorKind::InvalidArgument
    );
    assert_eq!(apis.asked.lock().len(), 3, "a bad id asks Nomad nothing");
}

#[tokio::test]
async fn a_node_that_answers_no_statistics_is_still_listed() {
    let apis = Answers::default().with(
        "https://nomad:4646/v1/nodes",
        true,
        r#"[{"ID":"f238b7b4","Name":"freebsd-c1","Status":"down","Drain":false,
             "SchedulingEligibility":"eligible","Version":"1.9.6","Drivers":null}]"#,
    );
    let config = nomad();
    let service = Service::new(&config, &apis, &At);
    let nodes = service
        .nodes(NodesRequest {
            environment: "pcx".into(),
        })
        .await
        .unwrap()
        .nodes;
    assert_eq!(nodes[0].status, "down");
    assert_eq!(nodes[0].unread, "Nomad refused: 404 page not found");
    assert_eq!(
        apis.asked.lock()[1].2,
        [("node_id".to_string(), "f238b7b4".to_string())]
    );
}

#[tokio::test]
async fn a_cluster_is_read_with_the_tools_of_its_kind() {
    let apis = Answers::default();
    let config = nomad();
    let service = Service::new(&config, &apis, &At);
    let pods = service
        .cluster(ClusterRequest {
            environment: "pcx".into(),
            namespace: "default".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(pods.kind, ModuleErrorKind::FailedPrecondition);
    assert_eq!(
        pods.message,
        "environment pcx is Nomad: read it with jobs_get, alloc_log and node_get"
    );
    let jobs = service
        .jobs(JobsRequest {
            environment: "prod".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(jobs.kind, ModuleErrorKind::FailedPrecondition);
    assert_eq!(
        jobs.message,
        "environment prod is Kubernetes: read it with cluster_get and pod_log"
    );
    assert!(apis.asked.lock().is_empty());
}

#[test]
fn an_environment_is_one_kind_of_cluster() {
    let mut both = nomad();
    both.environments[0].kube_api = "https://k8s:6443".into();
    both.environments[0].kube_token_file = "/creds/t".into();
    let refused = checked(both).unwrap_err();
    assert_eq!(refused.field.as_deref(), Some("environments.0.nomad_api"));
    let mut plain = nomad();
    plain.environments[0].nomad_api = "http://nomad:4646".into();
    assert_eq!(
        checked(plain).unwrap_err().message,
        "environments.0.nomad_api: is no https URL"
    );
    assert!(checked(nomad()).is_ok());
}
