// Copyright (c) 2026 Julius ML
// MIT License

use super::*;
use crate::tools::select::parse::parse;
use serde_json::json;

// ---- Parser positive cases ----

#[test]
fn parse_root_only() {
    let p = parse("$").unwrap();
    assert!(p.segments.is_empty());
}

#[test]
fn parse_field() {
    let p = parse("$.formatted_text").unwrap();
    assert_eq!(
        p.segments,
        vec![PathSegment::Field("formatted_text".into())]
    );
}

#[test]
fn parse_nested_fields() {
    let p = parse("$.items[0].text").unwrap();
    assert_eq!(
        p.segments,
        vec![
            PathSegment::Field("items".into()),
            PathSegment::Index(0),
            PathSegment::Field("text".into()),
        ]
    );
}

#[test]
fn parse_positive_index() {
    let p = parse("$.items[3]").unwrap();
    assert_eq!(p.segments[1], PathSegment::Index(3));
}

#[test]
fn parse_negative_index() {
    let p = parse("$.items[-2]").unwrap();
    assert_eq!(p.segments[1], PathSegment::Index(-2));
}

#[test]
fn parse_slice() {
    let p = parse("$.items[0:3]").unwrap();
    assert_eq!(
        p.segments[1],
        PathSegment::Slice {
            start: Some(0),
            end: Some(3)
        }
    );
}

#[test]
fn parse_slice_open_start() {
    let p = parse("$.items[:5]").unwrap();
    assert_eq!(
        p.segments[1],
        PathSegment::Slice {
            start: None,
            end: Some(5)
        }
    );
}

#[test]
fn parse_slice_open_end() {
    let p = parse("$.items[2:]").unwrap();
    assert_eq!(
        p.segments[1],
        PathSegment::Slice {
            start: Some(2),
            end: None
        }
    );
}

#[test]
fn parse_wildcard_bracket() {
    let p = parse("$.items[*]").unwrap();
    assert_eq!(p.segments[1], PathSegment::Wildcard);
}

#[test]
fn parse_wildcard_dot() {
    let p = parse("$.items.*").unwrap();
    assert_eq!(p.segments[1], PathSegment::Wildcard);
}

#[test]
fn parse_union() {
    let p = parse("$.items[0,3,7]").unwrap();
    assert_eq!(p.segments[1], PathSegment::Union(vec![0, 3, 7]));
}

// ---- Parser forbidden constructs ----

#[test]
fn parse_error_recursive_descent() {
    let e = parse("$..name").unwrap_err();
    assert!(e.message.contains("recursive descent"));
}

#[test]
fn parse_error_filter_expression() {
    let e = parse("$.items[?(@.score > 0.5)]").unwrap_err();
    assert!(e.message.contains("filter"));
}

#[test]
fn parse_error_function_length() {
    let e = parse("$.items.length()").unwrap_err();
    assert!(e.message.contains("function"));
}

#[test]
fn parse_error_slice_step() {
    let e = parse("$.items[0:10:2]").unwrap_err();
    assert!(e.message.contains("step"));
}

#[test]
fn parse_error_no_dollar() {
    let e = parse("items[0]").unwrap_err();
    assert!(e.message.contains("'$'"));
}

#[test]
fn parse_error_unexpected_char() {
    let e = parse("$.items|(0)").unwrap_err();
    assert!(e.message.contains("unexpected"));
}

// ---- Executor tests ----

fn sample_response() -> Value {
    json!({
        "items": [
            {"text": "first", "entry_id": "id-1", "score": 0.9},
            {"text": "second", "entry_id": "id-2", "score": 0.7},
            {"text": "third", "entry_id": "id-3", "score": 0.5},
            {"text": "fourth", "entry_id": "id-4", "score": 0.3},
            {"text": "fifth", "entry_id": "id-5", "score": 0.1},
        ],
        "formatted_text": "Here is the assembled context...",
        "total_tokens": 1234,
        "entity_context": [
            {"name": "Rust", "relevance": "direct_mention"},
            {"name": "gRPC", "relevance": "graph_neighbor"},
            {"name": "tonic", "relevance": "graph_neighbor"},
            {"name": "Axon", "relevance": "direct_mention"},
            {"name": "RocksDB", "relevance": "graph_neighbor"},
            {"name": "Post-Cortex", "relevance": "direct_mention"},
        ],
        "impact": [
            {"entity": "Axon", "depends_on": "gRPC", "relation_type": "DependsOn"}
        ]
    })
}

#[test]
fn exec_field() {
    let p = parse("$.formatted_text").unwrap();
    let v = execute(&p, &sample_response());
    assert_eq!(v, json!("Here is the assembled context..."));
}

#[test]
fn exec_index() {
    let p = parse("$.items[0].text").unwrap();
    let v = execute(&p, &sample_response());
    assert_eq!(v, json!("first"));
}

#[test]
fn exec_negative_index() {
    let p = parse("$.items[-1].text").unwrap();
    let v = execute(&p, &sample_response());
    assert_eq!(v, json!("fifth"));
}

#[test]
fn exec_slice() {
    let p = parse("$.items[0:3].text").unwrap();
    let v = execute(&p, &sample_response());
    assert_eq!(v, json!(["first", "second", "third"]));
}

#[test]
fn exec_wildcard() {
    let p = parse("$.entity_context[*].name").unwrap();
    let v = execute(&p, &sample_response());
    assert_eq!(
        v,
        json!(["Rust", "gRPC", "tonic", "Axon", "RocksDB", "Post-Cortex"])
    );
}

#[test]
fn exec_union() {
    let p = parse("$.items[0,4].entry_id").unwrap();
    let v = execute(&p, &sample_response());
    assert_eq!(v, json!(["id-1", "id-5"]));
}

#[test]
fn exec_empty_match_returns_null() {
    let p = parse("$.nonexistent").unwrap();
    let v = execute(&p, &sample_response());
    assert_eq!(v, Value::Null);
}

#[test]
fn exec_out_of_bounds_returns_null() {
    let p = parse("$.items[99]").unwrap();
    let v = execute(&p, &sample_response());
    assert_eq!(v, Value::Null);
}

// ---- apply_select tests ----

#[test]
fn select_string_form() {
    let resp = sample_response();
    let v = apply_select(&resp, Some("$.formatted_text"), None).unwrap();
    assert_eq!(v, json!("Here is the assembled context..."));
}

#[test]
fn select_map_form() {
    let resp = sample_response();
    let map = std::collections::HashMap::from([
        ("text".into(), "$.formatted_text".into()),
        ("top3".into(), "$.items[0:3].text".into()),
        ("ids".into(), "$.items[*].entry_id".into()),
        ("neighbours".into(), "$.entity_context[0:5].name".into()),
    ]);
    let v = apply_select(&resp, None, Some(&map)).unwrap();
    assert_eq!(v["text"], json!("Here is the assembled context..."));
    assert_eq!(v["top3"], json!(["first", "second", "third"]));
    assert_eq!(v["ids"], json!(["id-1", "id-2", "id-3", "id-4", "id-5"]));
    assert_eq!(
        v["neighbours"],
        json!(["Rust", "gRPC", "tonic", "Axon", "RocksDB"])
    );
}

#[test]
fn select_map_empty_match_gives_null() {
    let resp = sample_response();
    let map = std::collections::HashMap::from([("missing".into(), "$.nonexistent_field".into())]);
    let v = apply_select(&resp, None, Some(&map)).unwrap();
    assert_eq!(v["missing"], Value::Null);
}

#[test]
fn select_invalid_expression_returns_parse_error() {
    let resp = sample_response();
    let e = apply_select(&resp, Some("$..bad"), None).unwrap_err();
    assert!(e.message.contains("recursive descent"));
    assert!(e.byte_offset > 0);
}

#[test]
fn select_none_returns_full_response() {
    let resp = sample_response();
    let v = apply_select(&resp, None, None).unwrap();
    assert_eq!(v, resp);
}
