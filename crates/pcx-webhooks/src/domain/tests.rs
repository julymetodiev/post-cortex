// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use pbjson_types::Timestamp;
use post_cortex_proto::webhooks::AlertmanagerAlert;

fn alertmanager() -> Source {
    Source {
        name: "alertmanager".into(),
        kind: SourceKind::Alertmanager.into(),
        workspace_id: "ws".into(),
        board: "b".into(),
        assignee: "eq-sentinel".into(),
        notify: String::new(),
        workflow: String::new(),
        heartbeats: Vec::new(),
    }
}

fn config() -> WebhooksConfig {
    WebhooksConfig {
        token: "s3cret".into(),
        sources: vec![alertmanager()],
    }
}

fn at(seconds: i64) -> Option<Timestamp> {
    Some(Timestamp { seconds, nanos: 0 })
}

fn hook(status: &str, start: i64, end: i64) -> AlertmanagerWebhook {
    let labels: HashMap<String, String> = [
        ("alertname", "DiskFull"),
        ("severity", "critical"),
        ("instance", "db-1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    AlertmanagerWebhook {
        version: "4".into(),
        group_key: "{}:{alertname=\"DiskFull\"}".into(),
        status: status.into(),
        group_labels: [("alertname", "DiskFull"), ("instance", "db-1")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        common_labels: labels.clone(),
        common_annotations: [("summary".to_string(), "Disk 95% full".to_string())].into(),
        external_url: "https://am".into(),
        alerts: vec![AlertmanagerAlert {
            status: status.into(),
            labels,
            starts_at: at(start),
            ends_at: at(end),
            generator_url: "https://prom/rule".into(),
            ..AlertmanagerAlert::default()
        }],
        ..AlertmanagerWebhook::default()
    }
}

#[test]
fn a_configuration_needs_a_token_and_sources_named_once_with_a_board() {
    assert_eq!(check(&config()), Ok(()));
    let refused = |change: fn(&mut WebhooksConfig)| {
        let mut config = config();
        change(&mut config);
        match check(&config) {
            Err(Refused::Setting { field, .. }) => field,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(refused(|c| c.token = " ".into()), "token");
    assert_eq!(
        refused(|c| c.sources[0].name = "Alert Manager".into()),
        "sources.0.name"
    );
    assert_eq!(
        refused(|c| c.sources.push(alertmanager())),
        "sources.1.name"
    );
    assert_eq!(refused(|c| c.sources[0].kind = 0), "sources.0.kind");
    assert_eq!(
        refused(|c| c.sources[0].board = String::new()),
        "sources.0.board"
    );
    assert_eq!(
        refused(|c| c.sources[0].workspace_id = String::new()),
        "sources.0.workspace_id"
    );
    assert_eq!(
        refused(|c| c.sources[0].workflow = "triage".into()),
        "sources.0.workflow"
    );
    assert_eq!(
        refused(|c| c.sources[0].kind = SourceKind::Json.into()),
        "sources.0.workflow"
    );
    let mut routed = config();
    routed.sources[0].kind = SourceKind::Json.into();
    routed.sources[0].board.clear();
    routed.sources[0].workflow = "triage".into();
    assert_eq!(check(&routed), Ok(()));
    routed.sources[0].workflow = "Triage!".into();
    assert!(check(&routed).is_err());
}

#[test]
fn only_the_modules_token_and_a_known_source_pass() {
    assert!(authorized("s3cret", "s3cret"));
    assert!(!authorized("s3cret", "s3cre"));
    assert!(!authorized("s3cret", "s3creT"));
    assert!(!authorized("s3cret", ""));
    let config = config();
    assert_eq!(source(&config, "alertmanager").unwrap().board, "b");
    assert_eq!(
        source(&config, "github"),
        Err(Refused::UnknownSource("github".into()))
    );
}

#[test]
fn a_group_fires_or_resolves_and_names_its_key() {
    assert_eq!(status(&hook("firing", 0, 0)), Ok(Status::Firing));
    assert_eq!(status(&hook("resolved", 0, 0)), Ok(Status::Resolved));
    assert!(matches!(
        status(&hook("pending", 0, 0)),
        Err(Refused::Body(_))
    ));
    let mut keyless = hook("firing", 0, 0);
    keyless.group_key.clear();
    assert!(matches!(status(&keyless), Err(Refused::Body(_))));
}

#[test]
fn each_incident_of_a_group_has_its_own_key() {
    let prefix = group_prefix("alertmanager", "{}:{alertname=\"DiskFull\"}");
    assert!(prefix.starts_with("alertmanager:") && prefix.ends_with(':'));
    assert_eq!(prefix.len(), "alertmanager:".len() + 33);
    assert_eq!(
        prefix,
        group_prefix("alertmanager", "{}:{alertname=\"DiskFull\"}")
    );
    assert_ne!(
        prefix,
        group_prefix("alertmanager", "{}:{alertname=\"Other\"}")
    );
    assert_ne!(
        prefix,
        group_prefix("staging", "{}:{alertname=\"DiskFull\"}")
    );
    assert_eq!(episode_key(&prefix, 0), format!("{prefix}1"));
    assert_eq!(episode_key(&prefix, 2), format!("{prefix}3"));
}

#[test]
fn a_firing_group_becomes_a_task_with_what_the_alert_says() {
    let task = new_task(&alertmanager(), &hook("firing", 100, 0), "k:1".into());
    assert_eq!(task.title, "DiskFull · db-1");
    assert_eq!(task.board, "b");
    assert_eq!(task.workspace_id, "ws");
    assert_eq!(task.assignee.as_deref(), Some("eq-sentinel"));
    assert_eq!(task.labels, ["alert", "critical", "alertmanager"]);
    assert_eq!(task.priority.as_deref(), Some("P0"));
    assert_eq!(task.key.as_deref(), Some("k:1"));
    assert!(task.text.starts_with("Disk 95% full"));
    assert!(task.text.contains("`instance=db-1`"));
    assert!(task.text.contains("[The rule](https://prom/rule)"));
    let unassigned = Source {
        assignee: String::new(),
        ..alertmanager()
    };
    assert_eq!(
        new_task(&unassigned, &hook("firing", 0, 0), "k".into()).assignee,
        None
    );
}

#[test]
fn a_resolved_group_says_how_long_it_fired() {
    assert_eq!(
        resolved(&hook("resolved", 1_000, 1_000 + 12 * 60)),
        "Resolved after 12 min."
    );
    assert_eq!(lasted(45), "45 s");
    assert_eq!(lasted(3 * 3600 + 5 * 60), "3 h 5 min");
    assert_eq!(lasted(2 * 86_400 + 4 * 3600), "2 d 4 h");
    // Still firing: its end is year 1, before its start.
    assert_eq!(fired_for(&hook("firing", 1_000, -62_135_596_800)), None);
    assert_eq!(
        firing_again(&hook("firing", 0, 0)),
        "Fires again: 1 of 1 alerts."
    );
}

#[test]
fn a_task_closes_on_resolve_only_when_nothing_was_to_be_done_and_nothing_holds_it() {
    let labels = |l: &[&str]| l.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    assert!(closes(&labels(&["alert", NO_ACTION]), false));
    assert!(!closes(&labels(&["alert", NO_ACTION]), true));
    assert!(!closes(&labels(&["alert"]), false));
}

#[test]
fn a_boards_stages_say_where_work_is_open_and_done() {
    let board = Board {
        columns: vec!["To do".into(), "Done".into(), "Canceled".into()],
        stages: vec![
            Stage::Open.into(),
            Stage::Done.into(),
            Stage::Canceled.into(),
        ],
        ..Board::default()
    };
    assert!(open(&board, "To do"));
    assert!(!open(&board, "Done") && !open(&board, "Canceled"));
    assert_eq!(done_column(&board), Some("Done"));
}

#[test]
fn an_incident_says_what_fired_how_long_and_what_was_found() {
    let found = ["- Decided: grow the volume (95% full)".to_string()];
    let incident = incident(&hook("resolved", 0, 600), "DiskFull · db-1", &found);
    assert_eq!(incident.title, "Incident: DiskFull resolved");
    assert_eq!(incident.alert, "DiskFull");
    assert_eq!(incident.targets, ["db-1"]);
    assert!(incident.description.contains("Fired for 10 min."));
    assert!(incident.description.contains("grow the volume"));
}
