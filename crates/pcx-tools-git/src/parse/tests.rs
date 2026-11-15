// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn status_reads_paths_renames_and_states() {
    let out = " M src/lib.rs\0R  new.rs\0old.rs\0?? notes.md\0UU both.rs\0";
    let changes = status(out).unwrap();
    assert_eq!(changes.len(), 4);
    assert_eq!(
        (changes[0].path.as_str(), changes[0].kind()),
        ("src/lib.rs", ChangeKind::Modified)
    );
    assert_eq!(changes[1].from.as_deref(), Some("old.rs"));
    assert_eq!(changes[1].kind(), ChangeKind::Renamed);
    assert_eq!(changes[2].kind(), ChangeKind::Untracked);
    assert_eq!(changes[3].kind(), ChangeKind::Conflict);
}

#[test]
fn log_and_branch_records_are_read_by_field() {
    let log_out = "abc\u{1f}Ana\u{1f}2026-10-25T10:00:00+03:00\u{1f}feat: one\u{1e}\ndef\u{1f}Bo\u{1f}2026-10-24T09:00:00+03:00\u{1f}fix: two\u{1e}\n";
    let commits = log(log_out).unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(
        (commits[1].sha.as_str(), commits[1].subject.as_str()),
        ("def", "fix: two")
    );

    let branch_out = "main\u{1f}*\u{1f}origin/main\u{1e}\ntopic\u{1f} \u{1f}\u{1e}\n";
    let listed = branches(branch_out).unwrap();
    assert!(listed[0].current);
    assert_eq!(listed[0].upstream.as_deref(), Some("origin/main"));
    assert_eq!(
        (listed[1].current, listed[1].upstream.as_deref()),
        (false, None)
    );
}

#[test]
fn output_in_another_format_is_an_error() {
    assert!(status("M\0").is_err());
    assert!(status("R  new.rs\0").is_err());
    assert!(log("abc\u{1f}Ana\u{1e}").is_err());
    assert!(branches("main\u{1e}").is_err());
}

#[test]
fn conventional_commits_are_parsed() {
    let c = conventional("feat(board)!: tasks link to branches\n\nbody").unwrap();
    assert_eq!(c.kind, "feat");
    assert_eq!(c.scope.as_deref(), Some("board"));
    assert!(c.breaking);
    assert_eq!(c.description, "tasks link to branches");
    assert_eq!(conventional("fix: x").unwrap().scope, None);
    assert!(conventional("update stuff").is_none());
    assert!(conventional("wip: x").is_none());
    assert!(conventional("feat: ").is_none());
    assert!(conventional("feat(x: y").is_none());
}
