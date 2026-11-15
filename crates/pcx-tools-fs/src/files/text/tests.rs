// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn a_slice_starts_at_its_offset() {
    assert_eq!(slice("a\nb\nc", Some(2), Some(1)), (2, vec!["b"], 3));
    assert_eq!(slice("a\nb\nc", None, None), (1, vec!["a", "b", "c"], 3));
    assert_eq!(slice("a\nb", Some(9), None), (3, vec![], 2));
}

#[test]
fn a_replacement_needs_one_match() {
    assert_eq!(
        replace_once("x\nold\ny\n", "old", "new").unwrap(),
        ("x\nnew\ny\n".into(), 2)
    );
    // Trailing whitespace is ignored when nothing matches exactly.
    assert_eq!(
        replace_once("a  \nb\n", "a\nb", "c").unwrap(),
        ("c\n".into(), 1)
    );
    assert!(replace_once("old old", "old", "new").is_err());
    assert!(replace_once("abc", "zzz", "new").is_err());
    assert!(replace_once("abc", "abc", "abc").is_err());
}

#[test]
fn grep_lines_merge_their_context_and_stop_at_the_room() {
    let regex = Regex::new("hit").unwrap();
    let text = "a\nhit 1\nb\nhit 2\nc";
    let (lines, hits) = grep_lines(text, &regex, 1, 10);
    assert_eq!(hits, 2);
    let numbers: Vec<(usize, bool)> = lines.iter().map(|l| (l.number, l.matched)).collect();
    assert_eq!(
        numbers,
        [(1, false), (2, true), (3, false), (4, true), (5, false)]
    );
    let (lines, hits) = grep_lines(text, &regex, 0, 1);
    assert_eq!((lines.len(), hits), (1, 2), "one shown, more than room");
}
