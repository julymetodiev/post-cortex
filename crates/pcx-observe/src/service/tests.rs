// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Observe's service over APIs that answer in memory as the real ones do.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use super::*;

mod nomad;

/// A call asked: its API, URL and query.
type Asked = (Api, String, Vec<(String, String)>);

/// Answers by URL; what each call asked.
#[derive(Default)]
struct Answers {
    bodies: HashMap<String, (bool, String)>,
    asked: Mutex<Vec<Asked>>,
}

impl Answers {
    fn with(mut self, url: &str, ok: bool, body: &str) -> Self {
        self.bodies.insert(url.into(), (ok, body.into()));
        self
    }
}

#[async_trait]
impl Apis for Answers {
    async fn get(
        &self,
        _environment: &Environment,
        api: Api,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<(bool, Vec<u8>), ModuleError> {
        self.asked.lock().push((
            api,
            url.into(),
            query
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        ));
        let (ok, body) = self
            .bodies
            .get(url)
            .cloned()
            .unwrap_or((false, "404 page not found".into()));
        Ok((ok, body.into_bytes()))
    }
}

struct At;

impl Clock for At {
    fn now(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-27T07:30:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
}

fn config() -> ObserveConfig {
    ObserveConfig {
        environments: vec![
            Environment {
                name: "prod".into(),
                prometheus: "https://prom/".into(),
                loki: "https://loki".into(),
                alertmanager: "https://am".into(),
                kube_api: "https://k8s:6443".into(),
                kube_token_file: "/creds/prod.token".into(),
                ..Environment::default()
            },
            Environment {
                name: "staging".into(),
                ..Environment::default()
            },
        ],
    }
}

#[tokio::test]
async fn a_promql_query_is_answered_as_series() {
    let apis = Answers::default()
        .with(
            "https://prom/api/v1/query",
            true,
            r#"{"status":"success","data":{"resultType":"vector","result":[
                {"metric":{"__name__":"up","job":"svc-music"},"value":[1793085000,"0"]}]}}"#,
        )
        .with(
            "https://prom/api/v1/query_range",
            true,
            r#"{"status":"success","data":{"resultType":"matrix","result":[
                {"metric":{"job":"svc-music"},"values":[[1793081400,"1"],[1793081460,"0"]]}]}}"#,
        );
    let config = config();
    let service = Service::new(&config, &apis, &At);
    let now = service
        .metrics(MetricsRequest {
            environment: "prod".into(),
            query: "up == 0".into(),
            ..MetricsRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(now.series[0].labels["job"], "svc-music");
    assert_eq!(now.series[0].samples[0].value, 0.0);
    let range = service
        .metrics(MetricsRequest {
            environment: "prod".into(),
            query: "up".into(),
            start: 1_793_082_600,
            ..MetricsRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(range.series[0].samples.len(), 2);
    let asked = apis.asked.lock().clone();
    assert_eq!(
        asked[0].2,
        [
            ("query".into(), "up == 0".into()),
            ("time".into(), "1793086200".into())
        ]
    );
    let step = asked[1].2.iter().find(|(k, _)| k == "step").unwrap();
    assert_eq!(step.1, "60", "an hour in about 60 points");
}

#[tokio::test]
async fn a_query_prometheus_refuses_says_why() {
    let apis = Answers::default().with(
        "https://prom/api/v1/query",
        false,
        r#"{"status":"error","errorType":"bad_data","error":"parse error: unexpected }"}"#,
    );
    let config = config();
    let refused = Service::new(&config, &apis, &At)
        .metrics(MetricsRequest {
            environment: "prod".into(),
            query: "up}".into(),
            ..MetricsRequest::default()
        })
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    assert!(refused.message.contains("parse error"));
}

#[tokio::test]
async fn logs_and_alerts_are_read_as_loki_and_alertmanager_answer() {
    let apis = Answers::default()
        .with(
            "https://loki/loki/api/v1/query_range",
            true,
            r#"{"status":"success","data":{"resultType":"streams","result":[
                {"stream":{"app":"svc-music"},"values":[["1793085000000000000","panic: boom"]]}]}}"#,
        )
        .with(
            "https://am/api/v2/alerts",
            true,
            r#"[{"labels":{"alertname":"DiskFull"},"annotations":{"summary":"95%"},
                "startsAt":"2026-10-27T07:00:00.000Z","endsAt":"2026-10-27T08:00:00.000Z",
                "fingerprint":"f","receivers":[{"name":"pcx"}],
                "status":{"state":"suppressed","silencedBy":["s-1"],"inhibitedBy":[]},
                "updatedAt":"2026-10-27T07:00:00Z","generatorURL":"x"}]"#,
        );
    let config = config();
    let service = Service::new(&config, &apis, &At);
    let logs = service
        .logs(LogsRequest {
            environment: "prod".into(),
            query: r#"{app="svc-music"} |= "panic""#.into(),
            ..LogsRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(logs.streams[0].lines[0].line, "panic: boom");
    let asked = apis.asked.lock()[0].2.clone();
    let start = asked.iter().find(|(k, _)| k == "start").unwrap();
    assert_eq!(
        start.1, "1793082600000000000",
        "an hour back, in nanoseconds"
    );
    let alerts = service
        .alerts(AlertsRequest {
            environment: "prod".into(),
        })
        .await
        .unwrap();
    assert_eq!(alerts.alerts[0].state, "suppressed");
    assert_eq!(alerts.alerts[0].silenced_by, ["s-1"]);
}

#[tokio::test]
async fn a_namespace_and_a_pods_log_are_read_from_the_cluster() {
    let pods = r#"{"kind":"PodList","items":[{"metadata":{"name":"svc-music-7d9","creationTimestamp":"2026-10-27T07:00:00Z"},
        "status":{"phase":"Running","containerStatuses":[{"name":"app","ready":false,"restartCount":7,
          "state":{"waiting":{"reason":"CrashLoopBackOff","message":"back-off"}}}]}}]}"#;
    let deployments = r#"{"items":[{"metadata":{"name":"svc-music"},"spec":{"replicas":2},
        "status":{"readyReplicas":1,"updatedReplicas":2}}]}"#;
    let events = r#"{"items":[{"type":"Warning","reason":"BackOff","message":"Back-off restarting",
        "involvedObject":{"kind":"Pod","name":"svc-music-7d9"},"count":12,"eventTime":null,"lastTimestamp":"2026-10-27T07:25:00Z"},
        {"type":"Normal","reason":"Scheduled","message":"Successfully assigned",
        "involvedObject":{"kind":"Pod","name":"svc-music-7d9"},"eventTime":"2026-10-27T07:26:00.192537Z","firstTimestamp":null,"lastTimestamp":null}]}"#;
    let base = "https://k8s:6443";
    let apis = Answers::default()
        .with(
            &format!("{base}/api/v1/namespaces/equal-rust/pods"),
            true,
            pods,
        )
        .with(
            &format!("{base}/apis/apps/v1/namespaces/equal-rust/deployments"),
            true,
            deployments,
        )
        .with(
            &format!("{base}/apis/batch/v1/namespaces/equal-rust/jobs"),
            true,
            r#"{"items":[]}"#,
        )
        .with(
            &format!("{base}/api/v1/namespaces/equal-rust/events"),
            true,
            events,
        )
        .with(
            &format!("{base}/api/v1/namespaces/equal-rust/pods/svc-music-7d9/log"),
            true,
            "line 1\nthread 'main' panicked\n",
        );
    let config = config();
    let service = Service::new(&config, &apis, &At);
    let cluster = service
        .cluster(ClusterRequest {
            environment: "prod".into(),
            namespace: "equal-rust".into(),
        })
        .await
        .unwrap();
    assert_eq!(cluster.pods[0].reason, "CrashLoopBackOff");
    assert_eq!(cluster.pods[0].restarts, 7);
    assert_eq!(
        (cluster.deployments[0].wanted, cluster.deployments[0].ready),
        (2, 1)
    );
    assert_eq!(cluster.events[0].count, 12);
    assert!(
        apis.asked
            .lock()
            .iter()
            .all(|(api, _, _)| *api == Api::Kube)
    );
    let log = service
        .pod_log(PodLogRequest {
            environment: "prod".into(),
            namespace: "equal-rust".into(),
            pod: "svc-music-7d9".into(),
            previous: true,
            ..PodLogRequest::default()
        })
        .await
        .unwrap();
    assert!(log.text.contains("panicked"));
    let asked = apis.asked.lock().last().unwrap().2.clone();
    assert!(asked.contains(&("tailLines".into(), "100".into())));
    assert!(asked.contains(&("previous".into(), "true".into())));
}

#[tokio::test]
async fn names_unknown_environments_and_missing_sources_are_refused() {
    let (config, apis) = (config(), Answers::default());
    let service = Service::new(&config, &apis, &At);
    let traversal = service
        .cluster(ClusterRequest {
            environment: "prod".into(),
            namespace: "../../secrets".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(traversal.kind, ModuleErrorKind::InvalidArgument);
    let unknown = service
        .alerts(AlertsRequest {
            environment: "dev".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(unknown.kind, ModuleErrorKind::NotFound);
    let none = service
        .alerts(AlertsRequest {
            environment: "staging".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(none.kind, ModuleErrorKind::FailedPrecondition);
    assert!(apis.asked.lock().is_empty(), "nothing refused was asked");
    let forbidden = Answers::default().with(
        "https://k8s:6443/api/v1/namespaces/kube-system/pods",
        false,
        r#"{"kind":"Status","message":"pods is forbidden: User cannot list resource"}"#,
    );
    let refused = Service::new(&config, &forbidden, &At)
        .cluster(ClusterRequest {
            environment: "prod".into(),
            namespace: "kube-system".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
    assert!(refused.message.contains("forbidden"));
}
