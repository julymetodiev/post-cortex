// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn the_head_and_the_tail_keep_whole_lines_within_both_limits() {
    let text = "one\ntwo\nthree\nfour";
    assert_eq!(head(text, 2, 100).text, "one\ntwo");
    assert_eq!(head(text, 10, 9).text, "one\ntwo");
    let all = head(text, 10, 100);
    assert_eq!((all.shown, all.total, all.truncated()), (4, 4, false));
    assert!(head(text, 1, 100).truncated());
    assert_eq!(tail(text, 2, 100).text, "three\nfour");
    assert_eq!(tail(text, 10, 10).text, "three\nfour");
    assert_eq!(tail(text, 10, 100).text, text);
    assert_eq!(clip("abcdef", 3), "abc...");
    assert_eq!(clip("ab cdef", 3), "ab...");
    assert_eq!(clip("abc", 3), "abc");
}
