// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use post_cortex_proto::observe::{
    KubeContainer, KubeContainerState, KubeMeta, KubeReason, KubeRef, KubeStatus, PromData,
    PromSerie,
};

fn json(text: &str) -> Value {
    Value {
        kind: Some(JsonKind::StringValue(text.into())),
    }
}

fn num(n: f64) -> Value {
    Value {
        kind: Some(JsonKind::NumberValue(n)),
    }
}

fn pair(a: Value, b: Value) -> ListValue {
    ListValue { values: vec![a, b] }
}

#[test]
fn environments_are_named_once_and_a_cluster_has_its_token() {
    let env = |name: &str| Environment {
        name: name.into(),
        ..Environment::default()
    };
    let config = |envs: Vec<Environment>| ObserveConfig { environments: envs };
    assert_eq!(check(&config(vec![env("prod"), env("staging")])), Ok(()));
    let named = Environment {
        kube_ca_file: "/creds/prod.ca".into(),
        kube_tls_server_name: "10.254.0.101".into(),
        ..env("prod")
    };
    assert_eq!(check(&config(vec![named])), Ok(()));
    for (envs, field) in [
        (vec![env("Prod")], "environments.0.name"),
        (vec![env("prod"), env("prod")], "environments.1.name"),
        (
            vec![Environment {
                kube_api: "https://k".into(),
                ..env("prod")
            }],
            "environments.0.kube_token_file",
        ),
        (
            vec![Environment {
                kube_tls_server_name: "10.254.0.101".into(),
                ..env("prod")
            }],
            "environments.0.kube_tls_server_name",
        ),
    ] {
        match check(&config(envs)) {
            Err(Refused::Setting { field: f, .. }) => assert_eq!(f, field),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn only_kubernetes_names_go_into_a_path() {
    assert!(kube_name("equal-rust") && kube_name("svc-music-7d9.x"));
    for bad in [
        "",
        "../secrets",
        "a/b",
        "Equal",
        "-x",
        "a?b=1",
        &"a".repeat(254),
    ] {
        assert!(!kube_name(bad), "{bad}");
    }
}

#[test]
fn prometheus_pairs_are_read_as_samples() {
    assert_eq!(
        sample(&pair(num(1_793_084_400.5), json("0.25"))),
        Some(Sample {
            time: 1_793_084_400.5,
            value: 0.25
        })
    );
    assert!(sample(&pair(num(1.0), json("NaN"))).unwrap().value.is_nan());
    assert_eq!(
        sample(&pair(num(1.0), json("+Inf"))).unwrap().value,
        f64::INFINITY
    );
    assert_eq!(sample(&pair(num(1.0), json("lots"))), None);
    assert_eq!(
        sample(&ListValue {
            values: vec![num(1.0)]
        }),
        None
    );
}

#[test]
fn many_series_and_samples_are_cut_and_say_so() {
    let serie = |samples: usize| PromSerie {
        values: (0..samples)
            .map(|i| pair(num(i as f64), json("1")))
            .collect(),
        ..PromSerie::default()
    };
    let answer = |result| PromAnswer {
        status: "success".into(),
        data: Some(PromData {
            result_type: "matrix".into(),
            result,
        }),
        ..PromAnswer::default()
    };
    let small = series(answer(vec![serie(3)]));
    assert!(!small.truncated);
    assert_eq!(small.series[0].samples.len(), 3);
    let long = series(answer(vec![serie(MAX_SAMPLES + 10)]));
    assert!(long.truncated);
    assert_eq!(long.series[0].samples.len(), MAX_SAMPLES);
    assert_eq!(long.series[0].samples[0].time, 10.0, "the latest are kept");
    assert!(series(answer((0..MAX_SERIES + 1).map(|_| serie(1)).collect())).truncated);
}

#[test]
fn loki_pairs_are_read_as_lines() {
    let line = line(&pair(json("1793084400000000000"), json("panic: boom")));
    assert_eq!(
        line,
        Some(LogLine {
            time: 1_793_084_400_000_000_000,
            line: "panic: boom".into()
        })
    );
}

#[test]
fn a_pod_says_why_it_is_not_ready() {
    let container = |ready: bool, restarts: u32, reason: &str| KubeContainer {
        ready,
        restart_count: restarts,
        state: Some(KubeContainerState {
            waiting: (!reason.is_empty()).then(|| KubeReason {
                reason: reason.into(),
            }),
            terminated: None,
        }),
    };
    let object = KubeObject {
        metadata: Some(KubeMeta {
            name: "svc-music-7d9".into(),
            creation_timestamp: "2026-10-27T07:00:00Z".into(),
        }),
        status: Some(KubeStatus {
            phase: "Running".into(),
            container_statuses: vec![
                container(true, 0, ""),
                container(false, 7, "CrashLoopBackOff"),
            ],
            ..KubeStatus::default()
        }),
        ..KubeObject::default()
    };
    let pod = pod(&object);
    assert_eq!((pod.ready, pod.containers, pod.restarts), (1, 2, 7));
    assert_eq!(pod.reason, "CrashLoopBackOff");
    assert_eq!(pod.name, "svc-music-7d9");
}

#[test]
fn only_recent_warnings_are_kept_the_newest_first() {
    let event = |kind: &str, last: &str, reason: &str| KubeObject {
        r#type: kind.into(),
        reason: reason.into(),
        last_timestamp: Some(last.into()),
        involved_object: Some(KubeRef {
            kind: "Pod".into(),
            name: "p".into(),
        }),
        ..KubeObject::default()
    };
    let events = [
        event("Warning", "2026-10-27T07:10:00Z", "BackOff"),
        event("Normal", "2026-10-27T07:20:00Z", "Pulled"),
        event("Warning", "2026-10-27T05:00:00Z", "Old"),
        event("Warning", "2026-10-27T07:30:00Z", "Unhealthy"),
    ];
    let kept = warnings(&events, "2026-10-27T06:40:00Z");
    let reasons: Vec<&str> = kept.iter().map(|e| e.reason.as_str()).collect();
    assert_eq!(reasons, ["Unhealthy", "BackOff"]);
    assert_eq!(kept[0].object, "Pod/p");
}

#[test]
fn an_event_without_a_last_time_happened_at_its_event_time() {
    let event = KubeObject {
        r#type: "Warning".into(),
        reason: "FailedScheduling".into(),
        event_time: Some("2026-10-27T07:30:00.192537Z".into()),
        ..KubeObject::default()
    };
    let kept = warnings(&[event], "2026-10-27T06:40:00Z");
    assert_eq!(kept[0].last, "2026-10-27T07:30:00.192537Z");
}

#[test]
fn a_range_has_a_step_and_limits_have_bounds() {
    assert_eq!(step(0, 3_600, 0), 60);
    assert_eq!(step(0, 30, 0), 1);
    assert_eq!(step(0, 3_600, 15), 15);
    assert_eq!(within(0, LOG_LINES), 100);
    assert_eq!(within(10_000, LOG_LINES), 500);
}
