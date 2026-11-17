// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// A write to a copy (as on a retried compare-and-swap) leaves the
/// original version as it was.
#[test]
fn a_copy_changes_alone() {
    let mut original = session_with(prefs(50));
    original.apply_record(update(0, UpdateType::CodeChanged, false));
    for attempt in 0..2 {
        let mut copy = original.clone();
        copy.apply_record(update(1 + attempt, UpdateType::CodeChanged, false));
        assert_eq!(copy.records().len(), 2);
        assert_eq!(original.records().len(), 1);
    }
    let mut removing = original.clone();
    let id = original.records()[0].id;
    assert_eq!(removing.remove_records(&[id]), [id]);
    assert!(removing.hot_records().len() == 0);
    assert_eq!(original.hot_records().len(), 1);
    assert!(original.entity_graph().entity_mentions["e0"].contains(&id));
}

/// After a file's records are removed, no view of the session refers to
/// them: records, hot window, change history, current state, flow,
/// mentions, code references and the summary.
#[test]
fn removing_a_files_records_leaves_no_view_referencing_them() {
    let mut s = session_with(prefs(50));
    let decision = update(0, UpdateType::DecisionMade, true);
    let question = update(1, UpdateType::QuestionAnswered, true);
    let other = update(2, UpdateType::DecisionMade, false);
    let gone = [decision.id, question.id];
    let other_id = other.id;
    for u in [decision, question, other] {
        s.apply_record(u);
    }

    assert_eq!(s.remove_updates_for_file("src/lib.rs"), gone);

    let state = s.current_state();
    for id in &gone {
        assert!(s.record(id).is_none());
        assert!(s.hot_records().all(|u| u.id != *id), "hot window");
        assert!(
            s.change_history()
                .iter()
                .all(|c| c.related_update_id != Some(*id)),
            "change history"
        );
        assert!(
            state
                .conversation_flow
                .iter()
                .all(|f| !f.related_updates.contains(id)),
            "flow"
        );
        assert!(
            s.entity_graph()
                .entity_mentions
                .values_unordered()
                .all(|ids| !ids.contains(id)),
            "mentions"
        );
    }
    let decisions: Vec<&str> = state
        .key_decisions
        .iter()
        .map(|d| d.description.as_str())
        .collect();
    assert_eq!(decisions, ["title 2"]);
    assert!(state.answered_questions.is_empty());
    assert!(s.code_references().is_empty());
    let summary = s.context_summary();
    assert!(!summary.contains("description 0") && !summary.contains("description 1"));
    let hot: Vec<Uuid> = s.hot_records().map(|u| u.id).collect();
    assert_eq!(hot, [other_id]);
}

/// Removing records keeps the hot window whole: it is the newest records
/// that remain.
#[test]
fn removing_a_hot_record_keeps_the_hot_window_whole() {
    let mut s = session_with(prefs(2));
    let ids: Vec<Uuid> = (0..4)
        .map(|i| s.apply_record(update(i, UpdateType::CodeChanged, false)).id)
        .collect();
    assert_eq!(s.remove_records(&[ids[3], ids[1]]), [ids[1], ids[3]]);
    let hot: Vec<Uuid> = s.hot_records().map(|u| u.id).collect();
    assert_eq!(hot, [ids[0], ids[2]]);
    assert!(s.remove_records(&[ids[3]]).is_empty());
}

#[test]
fn retiring_replaces_the_record_once() {
    let mut s = session_with(prefs(50));
    let id = s
        .apply_record(update(0, UpdateType::DecisionMade, false))
        .id;
    let at = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
    let retired = s.retire_update(&id, at).unwrap();
    assert_eq!(retired.meta.retired_at, Some(at));
    assert_eq!(s.records()[0].meta.retired_at, Some(at));
    assert!(s.retire_update(&id, at).is_none());
    assert!(s.retire_update(&Uuid::new_v4(), at).is_none());
}

/// An oversized record is cut once, here, and stored cut.
#[test]
fn records_are_normalised() {
    let mut s = session_with(prefs(50));
    let mut big = update(0, UpdateType::DecisionMade, false);
    big.content.description = "é".repeat(MAX_DESCRIPTION_BYTES);
    big.content.title = "t".repeat(MAX_TITLE_BYTES + 1);
    let stored = s.apply_record(big);
    assert!(stored.content.description.len() <= MAX_DESCRIPTION_BYTES + 20);
    assert!(stored.content.description.ends_with("... (truncated)"));
    assert!(stored.content.title.ends_with("..."));
    assert!(Arc::ptr_eq(&stored, &s.records()[0]));
    assert!(
        stored.content.details.is_empty(),
        "only a code change keeps its title"
    );
    assert!(cut_title(&stored.content.title).is_some());
}

/// A code change's title is its `file`: cut, the whole of it stays in its
/// details, and a title that fits is not marked as cut.
#[test]
fn a_cut_code_change_keeps_its_whole_file() {
    let mut s = session_with(prefs(50));
    let mut change = update(0, UpdateType::CodeChanged, false);
    let whole = (0..12)
        .map(|i| format!("crates/post-cortex-memory/src/graph_views/file_{i}.rs"))
        .collect::<Vec<_>>()
        .join(", ");
    change.content.title = whole.clone();
    let stored = s.apply_record(change);
    assert_eq!(
        cut_title(&stored.content.title).map(str::len),
        Some(TRUNCATED_TITLE_BYTES)
    );
    assert_eq!(
        stored.content.details.last().map(String::as_str),
        Some(format!("{FILE_DETAIL_KEY}: {whole}").as_str())
    );
    assert_eq!(cut_title("src/a.rs ..."), None);
}

#[test]
fn metadata_changes_keep_the_creation_time() {
    let mut s = session_with(prefs(50));
    let created = s.created_at();
    let at = Utc.with_ymd_and_hms(2025, 3, 1, 0, 0, 0).unwrap();
    s.update_metadata(Some("new".into()), None, at);
    assert_eq!(s.get_metadata(), (Some("new".into()), None));
    assert_eq!((s.created_at(), s.last_updated()), (created, at));
    s.set_name(None, created);
    assert_eq!(s.name(), None);
    assert_eq!(s.last_updated(), at, "last_updated never goes back");
}
