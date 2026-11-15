// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn events_are_read_whole_across_chunks() {
    let mut lines = Lines::new();
    let text =
        "data: {\"a\":\"ж\"}\n\n: keep-alive\ndata: {\"b\":1}\n\ndata: [DONE]\n\ndata: late\n";
    let bytes = text.as_bytes();
    // Split inside the two bytes of `ж`.
    let split = text.find('ж').unwrap() + 1;
    let mut got = lines.feed(&bytes[..split]).unwrap();
    assert!(got.is_empty());
    got.extend(lines.feed(&bytes[split..]).unwrap());
    assert_eq!(got, ["{\"a\":\"ж\"}", "{\"b\":1}"]);
    assert!(lines.done());
    assert!(lines.feed(b"data: more\n").unwrap().is_empty());
}

#[test]
fn a_line_that_is_not_utf8_fails() {
    let mut lines = Lines::new();
    let failed = lines.feed(b"data: \xff\n").unwrap_err();
    assert_eq!(failed.kind, ModelFailureKind::Invalid);
}
