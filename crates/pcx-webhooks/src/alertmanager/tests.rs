// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// A body as Alertmanager 0.27 sends it (version 4), with a field this
/// mirror does not know.
pub const FIRING: &str = r#"{
  "receiver": "hermes",
  "status": "firing",
  "alerts": [
    {
      "status": "firing",
      "labels": {"alertname": "PodCrashLooping", "namespace": "equal-rust", "pod": "svc-music-7d9", "severity": "warning"},
      "annotations": {"summary": "svc-music restarts", "description": "3 restarts in 10 minutes"},
      "startsAt": "2026-10-27T07:20:11.512Z",
      "endsAt": "0001-01-01T00:00:00Z",
      "generatorURL": "https://prometheus.example/graph?g0.expr=x",
      "fingerprint": "c8a0f0e5b9e1d2a3"
    }
  ],
  "groupLabels": {"alertname": "PodCrashLooping", "namespace": "equal-rust"},
  "commonLabels": {"alertname": "PodCrashLooping", "namespace": "equal-rust", "severity": "warning"},
  "commonAnnotations": {"summary": "svc-music restarts", "description": "3 restarts in 10 minutes"},
  "externalURL": "https://alertmanager.example",
  "version": "4",
  "groupKey": "{}/{severity=~\"warning|critical\"}:{alertname=\"PodCrashLooping\", namespace=\"equal-rust\"}",
  "truncatedAlerts": 0,
  "unknownLater": true
}"#;

#[test]
fn alertmanagers_body_is_read_into_its_mirror() {
    let hook = decode(FIRING.as_bytes()).unwrap();
    assert_eq!(hook.version, "4");
    assert_eq!(hook.status, "firing");
    assert!(hook.group_key.starts_with("{}/"));
    assert_eq!(hook.external_url, "https://alertmanager.example");
    assert_eq!(hook.group_labels["namespace"], "equal-rust");
    let alert = &hook.alerts[0];
    assert_eq!(alert.labels["pod"], "svc-music-7d9");
    assert_eq!(
        alert.generator_url,
        "https://prometheus.example/graph?g0.expr=x"
    );
    assert_eq!(alert.starts_at.as_ref().unwrap().seconds, 1_793_085_611);
    assert!(
        alert.ends_at.as_ref().unwrap().seconds < 0,
        "year 1 while it fires"
    );
}

#[test]
fn a_body_that_is_not_alertmanagers_is_refused() {
    for body in [&b"not json"[..], br#"{"alerts": "none"}"#] {
        assert!(matches!(decode(body), Err(Refused::Body(_))));
    }
}
