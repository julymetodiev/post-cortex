// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use post_cortex_proto::board::Decision;
use post_cortex_proto::ops::{ClaimResources, Kind, OwnerRef, ThingMeta, ThingSpec, ThingStatus};
use prost_types::Timestamp;

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn prod() -> Environment {
    Environment {
        name: "prod".into(),
        kube_api: "https://k8s:6443".into(),
        kube_token_file: "/creds/prod.token".into(),
        ..Environment::default()
    }
}

fn config() -> OpsConfig {
    OpsConfig {
        workspace_id: "ws".into(),
        environments: vec![prod()],
        ..OpsConfig::default()
    }
}

fn done(target: &str, ran: &str) -> Action {
    Action {
        environment: "prod".into(),
        target: target.into(),
        what: format!("restart deployment {target}"),
        state: State::Done.into(),
        ran: Some(Timestamp {
            seconds: at(ran).timestamp(),
            nanos: 0,
        }),
        ..Action::default()
    }
}

#[test]
fn settings_need_a_workspace_and_each_cluster_its_token() {
    assert_eq!(check(&config()), Ok(()));
    let field = |change: fn(&mut OpsConfig)| {
        let mut c = config();
        change(&mut c);
        match check(&c) {
            Err(Refused::Setting { field, .. }) => field,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(field(|c| c.workspace_id.clear()), "workspace_id");
    assert_eq!(
        field(|c| c.environments.push(prod())),
        "environments.1.name"
    );
    assert_eq!(
        field(|c| c.environments[0].kube_token_file.clear()),
        "environments.0.kube_token_file"
    );
    assert_eq!(
        field(|c| c.environments[0].kube_tls_server_name = "10.254.0.101".into()),
        "environments.0.kube_tls_server_name"
    );
    assert_eq!(
        field(|c| c.environments[0].nomad_jobs.push("pcx-app".into())),
        "environments.0.nomad_jobs"
    );
    let nomad = |jobs: &[&str]| {
        let mut c = config();
        c.environments.push(Environment {
            name: "pcx".into(),
            nomad_api: "https://nomad:4646".into(),
            nomad_token_file: "/creds/pcx.token".into(),
            nomad_jobs: jobs.iter().map(|j| (*j).to_string()).collect(),
            ..Environment::default()
        });
        check(&c).map_err(|e| e.to_string())
    };
    assert_eq!(nomad(&["pcx-app", "pcx-cloud"]), Ok(()));
    assert_eq!(
        nomad(&[]).unwrap_err(),
        "environments.1.nomad_jobs: a Nomad environment names the jobs its actions act on"
    );
    assert_eq!(
        nomad(&["pcx-app", "pcx-app"]).unwrap_err(),
        "environments.1.nomad_jobs: names pcx-app twice"
    );
    assert_eq!(
        nomad(&["a/b"]).unwrap_err(),
        "environments.1.nomad_jobs: a/b is no Nomad job's name"
    );
    let mut named = config();
    named.environments[0].kube_ca_file = "/creds/prod.ca".into();
    named.environments[0].kube_tls_server_name = "10.254.0.101".into();
    assert_eq!(check(&named), Ok(()));
}

#[test]
fn an_action_names_its_task_and_why_and_kubernetes_names() {
    let ask = Ask {
        environment: "prod".into(),
        workspace_id: "ws".into(),
        task: "t-1".into(),
        why: "3 restarts in 10 minutes".into(),
        dry_run: false,
    };
    assert_eq!(asked(&ask), Ok(()));
    let no_why = Ask {
        why: " ".into(),
        ..ask
    };
    assert_eq!(asked(&no_why), Err(Refused::Empty("why")));
    assert_eq!(named("namespace", "equal-rust"), Ok("equal-rust"));
    assert_eq!(named("object", "../x"), Err(Refused::Name("object")));
}

#[test]
fn a_scale_stays_within_its_environments_bounds() {
    assert_eq!(replicas(&prod(), 3), Ok(3));
    assert!(matches!(replicas(&prod(), 4), Err(Refused::Bounds(_))));
    assert!(matches!(replicas(&prod(), 0), Err(Refused::Bounds(_))));
    let bigger = Environment {
        max_replicas: 10,
        ..prod()
    };
    assert_eq!(replicas(&bigger, 10), Ok(10));
}

#[test]
fn brakes_hold_a_target_a_while_and_an_environment_a_few_an_hour() {
    let now = at("2026-10-27T08:00:00Z");
    let config = config();
    assert_eq!(
        brake(&config, &[], "prod", "equal-rust/svc-music", now),
        Ok(())
    );
    let recent = [done("equal-rust/svc-music", "2026-10-27T07:45:00Z")];
    assert!(matches!(
        brake(&config, &recent, "prod", "equal-rust/svc-music", now),
        Err(Refused::Brake(_))
    ));
    assert_eq!(
        brake(&config, &recent, "prod", "equal-rust/svc-feed", now),
        Ok(())
    );
    assert_eq!(
        brake(&config, &recent, "staging", "equal-rust/svc-music", now),
        Ok(())
    );
    let old = [done("equal-rust/svc-music", "2026-10-27T07:20:00Z")];
    assert_eq!(
        brake(&config, &old, "prod", "equal-rust/svc-music", now),
        Ok(())
    );
    let waiting = [Action {
        state: State::Waiting.into(),
        ran: None,
        ..done("equal-rust/svc-music", "2026-10-27T01:00:00Z")
    }];
    let held = brake(&config, &waiting, "prod", "equal-rust/svc-music", now).unwrap_err();
    assert!(held.to_string().contains("waits for approval"));
    let busy: Vec<Action> = (0..6)
        .map(|i| done(&format!("ns/svc-{i}"), "2026-10-27T07:30:00Z"))
        .collect();
    let full = brake(&config, &busy, "prod", "ns/other", now).unwrap_err();
    assert!(full.to_string().contains("6 actions ran in prod this hour"));
}

#[test]
fn only_a_pod_a_controller_makes_again_and_no_databases_is_deleted() {
    let pod = |owners: Vec<OwnerRef>, labels: Vec<(&str, &str)>| KubeThing {
        metadata: Some(ThingMeta {
            name: "p".into(),
            labels: labels
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            owner_references: owners,
        }),
        ..KubeThing::default()
    };
    let controlled = OwnerRef {
        kind: "ReplicaSet".into(),
        controller: true,
    };
    assert_eq!(deletable(&pod(vec![controlled.clone()], vec![])), Ok(()));
    assert!(matches!(
        deletable(&pod(vec![], vec![])),
        Err(Refused::Target(_))
    ));
    assert!(matches!(
        deletable(&pod(vec![controlled], vec![("cnpg.io/cluster", "pg")])),
        Err(Refused::Target(_))
    ));
}

#[test]
fn a_claim_grows_to_more_and_at_most_twice() {
    assert_eq!(gibibytes("10Gi"), Some(10));
    assert_eq!(gibibytes("1Ti"), Some(1024));
    assert_eq!(gibibytes("1500Mi"), Some(2));
    assert_eq!(gibibytes("10G"), None);
    let claim = |size: &str| KubeThing {
        metadata: Some(ThingMeta {
            name: "data".into(),
            ..ThingMeta::default()
        }),
        spec: Some(ThingSpec {
            replicas: 0,
            resources: Some(ClaimResources {
                requests: [("storage".to_string(), size.to_string())].into(),
            }),
        }),
        ..KubeThing::default()
    };
    assert_eq!(growable(&claim("10Gi"), 15), Ok(10));
    assert!(matches!(
        growable(&claim("10Gi"), 10),
        Err(Refused::Bounds(_))
    ));
    assert!(matches!(
        growable(&claim("10Gi"), 21),
        Err(Refused::Bounds(_))
    ));
    // A volume larger than its claim asked for grows from what it has.
    let larger = KubeThing {
        status: Some(ThingStatus {
            capacity: [("storage".to_string(), "10Gi".to_string())].into(),
            ..ThingStatus::default()
        }),
        ..claim("2Gi")
    };
    assert_eq!(growable(&larger, 20), Ok(10));
    assert!(matches!(growable(&larger, 4), Err(Refused::Bounds(_))));
}

#[test]
fn an_action_is_given_up_when_its_decision_is_declined_or_withdrawn() {
    let waiting = Decision::default();
    assert_eq!(given_up(&waiting), None);
    let declined = Decision {
        declined_by: Some("julius".into()),
        decline_note: Some("not now".into()),
        ..Decision::default()
    };
    assert_eq!(
        given_up(&declined).as_deref(),
        Some("declined by julius (not now)")
    );
    let withdrawn = Decision {
        withdrawn: Some("its task was closed".into()),
        ..Decision::default()
    };
    assert_eq!(
        given_up(&withdrawn).as_deref(),
        Some("its decision was withdrawn (its task was closed)")
    );
    assert!(look_again("eq-medic").starts_with("@eq-medic: "));
}

#[test]
fn failed_jobs_are_those_that_never_succeeded() {
    let job = |name: &str, failed: u32, succeeded: u32| KubeThing {
        metadata: Some(ThingMeta {
            name: name.into(),
            ..ThingMeta::default()
        }),
        status: Some(ThingStatus {
            failed,
            succeeded,
            ..ThingStatus::default()
        }),
        ..KubeThing::default()
    };
    let jobs = ThingList {
        items: vec![job("a", 2, 0), job("b", 1, 1), job("c", 0, 1)],
    };
    assert_eq!(failed_jobs(&jobs), ["a"]);
}

#[test]
fn patches_are_the_merge_patches_kubernetes_reads() {
    let restart = serde_json::to_string(&restart(at("2026-10-27T08:00:00Z"))).unwrap();
    assert_eq!(
        restart,
        r#"{"spec":{"template":{"metadata":{"annotations":{"kubectl.kubernetes.io/restartedAt":"2026-10-27T08:00:00Z"}}}}}"#
    );
    assert_eq!(
        serde_json::to_string(&scale(2)).unwrap(),
        r#"{"spec":{"replicas":2}}"#
    );
    assert_eq!(
        serde_json::to_string(&grow(20)).unwrap(),
        r#"{"spec":{"resources":{"requests":{"storage":"20Gi"}}}}"#
    );
    assert_eq!(
        stakes(Kind::DeletePod, "prod", "ns/p").action.as_deref(),
        Some("delete_pod")
    );
    assert_eq!(
        stakes(Kind::Scale, "prod", "ns/d").target.as_deref(),
        Some("ns/d")
    );
}
