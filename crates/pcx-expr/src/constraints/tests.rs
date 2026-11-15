// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::collections::BTreeMap;

use super::*;
use crate::schema::{bounds, fields, json_schema};

fn bound(v: Value) -> Option<Bound> {
    Some(Bound::new(v).unwrap())
}

/// `name` (required text of 2 to 20 letters), `age` (an int, 0 to 150),
/// `role` (one of two).
fn person() -> Vec<Field> {
    let mut f = fields([
        ("name", "string", true),
        ("age", "int", false),
        ("role", "string", false),
    ])
    .unwrap();
    f[0].constraints = Constraints {
        min_length: Some(2),
        max_length: Some(20),
        pattern: Some(Pattern::new("[A-Za-z ]+").unwrap()),
        ..Constraints::default()
    };
    f[1].constraints = Constraints {
        minimum: bound(Value::Int(0)),
        maximum: bound(Value::Int(150)),
        ..Constraints::default()
    };
    f[2].constraints = Constraints {
        one_of: vec![
            Bound::new(Value::String("admin".into())).unwrap(),
            Bound::new(Value::String("guest".into())).unwrap(),
        ],
        ..Constraints::default()
    };
    f
}

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    )
}

#[test]
fn every_constraint_a_value_breaks_is_found_not_only_the_first() {
    let mut out = Vec::new();
    let value = map(&[
        ("name", Value::String("J4".into())),
        ("age", Value::Int(200)),
        ("role", Value::String("root".into())),
    ]);
    violations(&person(), &value, "items[3]", &mut out);
    let rules: Vec<(&str, &str)> = out
        .iter()
        .map(|v| (v.path.as_str(), v.rule.as_str()))
        .collect();
    assert_eq!(
        rules,
        [
            ("items[3].name", "pattern"),
            ("items[3].age", "maximum"),
            ("items[3].role", "one_of"),
        ]
    );
    assert!(out[1].message.contains("150"), "{out:?}");

    let mut out = Vec::new();
    violations(
        &person(),
        &map(&[("age", Value::String("x".into()))]),
        "",
        &mut out,
    );
    let rules: Vec<&str> = out.iter().map(|v| v.rule.as_str()).collect();
    assert_eq!(rules, ["required", "type"]);

    let mut out = Vec::new();
    violations(&person(), &Value::Int(1), "", &mut out);
    assert_eq!(out[0].rule, "type");
}

#[test]
fn a_value_that_keeps_them_breaks_nothing() {
    let mut out = Vec::new();
    let value = map(&[
        ("name", Value::String("Ann Lee".into())),
        ("age", Value::Int(40)),
        ("role", Value::String("guest".into())),
    ]);
    violations(&person(), &value, "", &mut out);
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn constraints_fit_their_fields_type() {
    let int = Type::Int;
    let c = |c: Constraints| c.fit(&int).map_err(|(part, _)| part);
    assert_eq!(
        c(Constraints {
            pattern: Some(Pattern::new("x").unwrap()),
            ..Constraints::default()
        }),
        Err("pattern")
    );
    assert_eq!(
        c(Constraints {
            min_length: Some(1),
            ..Constraints::default()
        }),
        Err("min_length")
    );
    assert_eq!(
        c(Constraints {
            minimum: bound(Value::String("a".into())),
            ..Constraints::default()
        }),
        Err("minimum")
    );
    assert_eq!(
        c(Constraints {
            minimum: bound(Value::Int(5)),
            maximum: bound(Value::Int(1)),
            ..Constraints::default()
        }),
        Err("minimum")
    );
    assert!(Bound::new(Value::Double(f64::NAN)).is_err());
    assert!(
        Pattern::new(&"a".repeat(2_000))
            .unwrap_err()
            .contains("1024")
    );
    assert!(Pattern::new("(").is_err());
}

#[test]
fn a_pattern_matches_the_whole_text() {
    let p = Pattern::new("[0-9]+").unwrap();
    assert!(p.matches("123"));
    assert!(!p.matches("12a"));
    assert!(!p.matches("a12"));
}

#[test]
fn a_model_is_told_the_bounds_and_an_answer_is_held_to_them() {
    let schema = json_schema(&person());
    assert_eq!(schema["properties"]["name"]["maxLength"], 20);
    assert_eq!(schema["properties"]["name"]["pattern"], "^(?:[A-Za-z ]+)$");
    assert_eq!(schema["properties"]["age"]["maximum"], 150);
    assert_eq!(schema["properties"]["role"]["enum"][1], "guest");

    let read: BTreeMap<String, Value> = [
        ("name".to_string(), Value::String("A".into())),
        ("age".to_string(), Value::Int(-1)),
    ]
    .into();
    let why = bounds(&person(), read).unwrap_err();
    assert!(why.contains("name") && why.contains("age"), "{why}");
}
