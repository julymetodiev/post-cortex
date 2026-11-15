// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::collections::BTreeMap;

use chrono::DateTime;
use pcx_expr::{Value as Expr, Values};
use post_cortex_proto::workflow::{
    Constraints, Field as Wire, Fired, Graph, Rule, Schema, Step, StepStatus, Workflow, block,
};

use super::*;
use crate::domain::flow::Found;
use crate::domain::{Catalog, Checked, Previewing, Refusal, as_message, check};
use crate::test_support::{block as of, catalog, edge, manual, set};

fn field(name: &str, ty: &str, required: bool) -> Wire {
    Wire {
        name: name.into(),
        r#type: ty.into(),
        required,
        ..Default::default()
    }
}

/// start -> `data` (block `d`) -> `after` (a `set` of `after`), over the
/// input `nums` (`list<int>`) and `people` (`list<map<string, string>>`).
fn flow(data: block::Kind, after: &str, port: &str) -> Workflow {
    Workflow {
        name: "people-report".into(),
        input: Some(Schema {
            fields: vec![
                field("nums", "list<int>", false),
                field("people", "list<map<string, string>>", false),
            ],
        }),
        graph: Some(Graph {
            blocks: vec![manual(), of("d", data), set("after", "x", after)],
            edges: vec![edge("start", "d", ""), edge("d", "after", port)],
            output: String::new(),
        }),
        ..Default::default()
    }
}

fn checked(w: &Workflow) -> Result<(Catalog, Checked), Refusal> {
    let catalog = catalog().with_records(w)?;
    let checked = check(w, &catalog, &BTreeMap::new())?;
    Ok((catalog, checked))
}

fn refused(w: &Workflow) -> Refusal {
    match checked(w) {
        Err(r) => r,
        Ok(_) => panic!("{} checks", w.name),
    }
}

fn data(found: &Found) -> &Data {
    &found.data["d"]
}

fn person(name: &str, age: i64) -> Expr {
    Expr::map([
        ("name".to_string(), Expr::string(name)),
        ("age".to_string(), Expr::string(age.to_string())),
    ])
}

/// The items of `list`.
fn items_of(list: &Expr) -> Vec<Expr> {
    let Expr::List(items) = list else {
        panic!("{list:?} is no list")
    };
    items.iter().map(Result::unwrap).collect()
}

/// Field `name` of `record`, a map.
fn field_of(record: &Expr, name: &str) -> Expr {
    let Expr::Map(entries) = record else {
        panic!("{record:?} is no record")
    };
    entries.get(name).unwrap().unwrap()
}

fn values() -> Values {
    [
        (
            "input.nums".to_string(),
            Expr::list((1..=10).map(Expr::Int)),
        ),
        (
            "input.people".to_string(),
            Expr::list(vec![person("Ann", 30), person("B", 12), person("Cy", 40)]),
        ),
    ]
    .into()
}

fn text_of(m: &Expr, name: &str) -> String {
    match field_of(m, name) {
        Expr::String(s) => s.to_string(),
        other => panic!("{other:?} is no text"),
    }
}

fn filter(source: &str, distinct_by: &str, limit: u32) -> block::Kind {
    block::Kind::Filter(FilterBlock {
        items: "input.nums".into(),
        r#where: source.into(),
        distinct_by: distinct_by.into(),
        limit,
    })
}

#[test]
fn a_filter_keeps_by_its_condition_in_order_deduplicates_and_stops_at_its_limit() {
    let w = flow(
        filter("item % 2 == 0", "item / 4", 0),
        "size(steps.d.items) + steps.d.dropped",
        "",
    );
    let (_, c) = checked(&w).unwrap();
    let gave = data(&c.found).run(&values(), false).unwrap();
    // 2, 4, 6, 8, 10 by keys 0, 1, 1, 2, 2.
    assert_eq!(
        gave.output["items"],
        Expr::list(vec![Expr::Int(2), Expr::Int(4), Expr::Int(8)])
    );
    assert_eq!(gave.output["dropped"], Expr::Int(7));
    assert_eq!((gave.port, gave.items), ("next", 10));

    let w = flow(filter("item > 3", "", 2), "steps.d.items", "");
    let (_, c) = checked(&w).unwrap();
    let gave = data(&c.found).run(&values(), false).unwrap();
    assert_eq!(
        gave.output["items"],
        Expr::list(vec![Expr::Int(4), Expr::Int(5)])
    );
    assert_eq!(gave.output["dropped"], Expr::Int(8));
}

#[test]
fn a_filter_whose_condition_is_no_bool_or_whose_key_has_no_kind_is_refused() {
    let r = refused(&flow(filter("item + 1", "", 0), "1", ""));
    assert_eq!(
        (r.block.as_deref(), r.field.as_str()),
        (Some("d"), "filter.where")
    );
    let r = refused(&flow(filter("true", "double(item)", 0), "1", ""));
    assert_eq!(r.field, "filter.distinct_by");
    let mut f = FilterBlock {
        items: "input.nums[0]".into(),
        r#where: "true".into(),
        ..Default::default()
    };
    let r = refused(&flow(block::Kind::Filter(f.clone()), "1", ""));
    assert!(r.message.contains("not a list"), "{r}");
    f.items = "input.nums".into();
    let r = refused(&flow(block::Kind::Filter(f), "steps.d.nothing", ""));
    assert_eq!(r.block.as_deref(), Some("after"));
}

fn transform(input: transform_block::Input, fields: &[(&str, &str)]) -> TransformBlock {
    TransformBlock {
        input: Some(input),
        output: Some(Schema {
            fields: vec![field("name", "string", true), field("age", "int", false)],
        }),
        fields: fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        ..Default::default()
    }
}

fn items(source: &str) -> transform_block::Input {
    transform_block::Input::Items(source.into())
}

#[test]
fn a_transform_makes_typed_records_sorted_by_a_key() {
    let mut t = transform(
        items("input.people"),
        &[("name", "item.name"), ("age", "int(item.age)")],
    );
    t.sort_by = "row.age".into();
    t.descending = true;
    // A record's field is typed: `name` is a string, so `+ "!"` checks.
    let w = flow(block::Kind::Transform(t), "steps.d.items[0].name + '!'", "");
    let (catalog, c) = checked(&w).unwrap();
    let record = catalog
        .messages()
        .get_message_by_name("pcx.workflow.record.w_people_report.d")
        .unwrap();
    assert_eq!(record.fields().count(), 2);
    let gave = data(&c.found).run(&values(), false).unwrap();
    let rows = items_of(&gave.output["items"]);
    let names: Vec<String> = rows.iter().map(|r| text_of(r, "name")).collect();
    assert_eq!(names, ["Cy", "Ann", "B"]);
}

#[test]
fn a_transform_groups_its_records_by_a_key_in_the_order_keys_first_appear() {
    let mut t = transform(
        items("input.people"),
        &[("name", "item.name"), ("age", "int(item.age)")],
    );
    t.group_by = "row.age >= 18 ? 'adult' : 'minor'".into();
    let w = flow(
        block::Kind::Transform(t),
        "steps.d.groups[0].items[0].age",
        "",
    );
    let (_, c) = checked(&w).unwrap();
    let gave = data(&c.found).run(&values(), false).unwrap();
    let groups = items_of(&gave.output["groups"]);
    let keys: Vec<String> = groups.iter().map(|g| text_of(g, "key")).collect();
    assert_eq!(keys, ["adult", "minor"]);
    assert_eq!(items_of(&field_of(&groups[0], "items")).len(), 2);
}

#[test]
fn a_transform_of_one_value_gives_its_fields() {
    let t = transform(
        transform_block::Input::Value("input.people[0]".into()),
        &[("name", "value.name")],
    );
    let w = flow(
        block::Kind::Transform(t),
        "steps.d.name + string(steps.d.age)",
        "",
    );
    let (_, c) = checked(&w).unwrap();
    let gave = data(&c.found).run(&values(), false).unwrap();
    assert_eq!(gave.output["name"], Expr::string("Ann"));
    // A field without an expression is its zero value.
    assert_eq!(gave.output["age"], Expr::Int(0));
}

#[test]
fn a_transform_refuses_a_field_it_has_not_a_required_one_missing_and_another_type() {
    let t = transform(items("input.people"), &[("name", "'x'"), ("nick", "'y'")]);
    assert_eq!(
        refused(&flow(block::Kind::Transform(t), "1", "")).field,
        "transform.fields.nick"
    );
    let t = transform(items("input.people"), &[("age", "1")]);
    assert_eq!(
        refused(&flow(block::Kind::Transform(t), "1", "")).field,
        "transform.fields.name"
    );
    let t = transform(items("input.people"), &[("name", "1")]);
    let r = refused(&flow(block::Kind::Transform(t), "1", ""));
    assert!(r.message.contains("not a string"), "{r}");
    let mut t = transform(
        transform_block::Input::Value("1".into()),
        &[("name", "'a'")],
    );
    t.sort_by = "row.name".into();
    assert_eq!(
        refused(&flow(block::Kind::Transform(t), "1", "")).field,
        "transform.sort_by"
    );
    let mut t = transform(items("input.people"), &[("name", "'a'")]);
    t.output = Some(Schema {
        fields: vec![
            field("name", "string", true),
            field("deep", "list<list<int>>", false),
        ],
    });
    let r = refused(&flow(block::Kind::Transform(t), "1", ""));
    assert!(r.message.contains("one level"), "{r}");
}

fn validate(rules: Vec<Rule>) -> ValidateBlock {
    let mut name = field("name", "string", true);
    name.constraints = Some(Constraints {
        min_length: Some(2),
        ..Default::default()
    });
    let mut age = field("age", "string", false);
    age.constraints = Some(Constraints {
        pattern: "[0-9]+".into(),
        ..Default::default()
    });
    ValidateBlock {
        input: Some(validate_block::Input::Items("input.people".into())),
        schema: Some(Schema {
            fields: vec![name, age],
        }),
        rules,
    }
}

fn adult() -> Rule {
    Rule {
        holds: "int(item.age) >= 18".into(),
        message: "{{ item.name }} is a minor".into(),
        path: "age".into(),
    }
}

#[test]
fn a_validate_finds_every_violation_and_parts_valid_from_invalid_items() {
    let w = flow(
        block::Kind::Validate(validate(vec![adult()])),
        "size(steps.d.valid)",
        "valid",
    );
    let (_, c) = checked(&w).unwrap();
    let gave = data(&c.found).run(&values(), false).unwrap();
    assert_eq!(gave.port, "invalid");
    let found = items_of(&gave.output["violations"]);
    let all: Vec<(String, String)> = found
        .iter()
        .map(|v| (text_of(v, "path"), text_of(v, "rule")))
        .collect();
    assert_eq!(
        all,
        [
            ("items[1].name".to_string(), "min_length".to_string()),
            ("items[1].age".to_string(), "rules[0]".to_string()),
        ]
    );
    assert_eq!(text_of(&found[1], "message"), "B is a minor");
    assert_eq!(items_of(&gave.output["valid"]).len(), 2);
    let invalid = items_of(&gave.output["invalid"]);
    assert_eq!(field_of(&invalid[0], "index"), Expr::Int(1));
}

#[test]
fn a_validate_without_an_invalid_edge_fails_with_its_violations() {
    let w = flow(block::Kind::Validate(validate(vec![adult()])), "1", "valid");
    let (_, c) = checked(&w).unwrap();
    let failed = data(&c.found).run(&values(), true).unwrap_err();
    assert_eq!(failed.kind, "invalid");
    assert!(
        failed.message.contains("items[1].age: B is a minor"),
        "{}",
        failed.message
    );
    // Nothing broken: `valid`, whatever the edges.
    let mut v = validate(Vec::new());
    v.input = Some(validate_block::Input::Value("input.people[0]".into()));
    let w = flow(
        block::Kind::Validate(v),
        "size(steps.d.violations)",
        "valid",
    );
    let (_, c) = checked(&w).unwrap();
    assert_eq!(data(&c.found).run(&values(), true).unwrap().port, "valid");
}

#[test]
fn a_validate_has_a_schema_or_a_rule_and_reads_maps_or_messages() {
    let mut v = validate(Vec::new());
    v.schema = None;
    assert_eq!(
        refused(&flow(block::Kind::Validate(v), "1", "valid")).field,
        "validate"
    );
    let mut v = validate(Vec::new());
    v.input = Some(validate_block::Input::Items("input.nums".into()));
    let r = refused(&flow(block::Kind::Validate(v), "1", "valid"));
    assert!(r.message.contains("map or a message"), "{r}");
    let r = refused(&flow(
        block::Kind::Validate(validate(vec![adult()])),
        "1",
        "",
    ));
    assert!(r.message.contains("no port"), "{r}");
}

#[test]
fn a_data_block_is_previewed_over_a_runs_steps() {
    let w = flow(filter("item > 8", "", 0), "1", "");
    let (catalog, c) = checked(&w).unwrap();
    let fired = Expr::Message(as_message(&Fired::default(), catalog.messages()).unwrap());
    let start = Step {
        block: "start".into(),
        attempt: 1,
        status: StepStatus::Succeeded.into(),
        ..Default::default()
    };
    let later = Step {
        block: "d".into(),
        ..start.clone()
    };
    let preview = Previewing {
        workflow: &w,
        checked: &c,
        catalog: &catalog,
        input: &values(),
        fired: &fired,
        now: DateTime::UNIX_EPOCH,
    };
    let gave = preview
        .preview("d", "", vec![start.clone(), later])
        .unwrap()
        .unwrap();
    assert_eq!(
        gave.output["items"],
        Expr::list(vec![Expr::Int(9), Expr::Int(10)])
    );
    let why = preview
        .preview("after", "", vec![start.clone()])
        .unwrap_err();
    assert!(why.message.contains("do not lead"), "{}", why.message);
    let why = preview.preview("d", "", Vec::new()).unwrap_err();
    assert!(why.message.contains("trigger"), "{}", why.message);
}

/// 100,000 items through each data block (docs/modules/workflow.md, data
/// blocks, speed: a `filter` with a short condition within 100 ms on `ml`).
/// Run in release: `cargo test --release -p pcx-workflow --lib speed --
/// --ignored --nocapture`.
#[test]
#[ignore = "a measure, run in release"]
fn speed_of_100000_items() {
    const N: i64 = 100_000;
    let values: Values = [
        ("input.nums".to_string(), Expr::list((0..N).map(Expr::Int))),
        (
            "input.people".to_string(),
            Expr::list((0..N).map(|i| person(&format!("P{i}"), i % 90))),
        ),
    ]
    .into();
    let time = |what: &str, kind: block::Kind| {
        let w = flow(kind, "1", if what == "validate" { "valid" } else { "" });
        let (_, c) = checked(&w).unwrap();
        let data = data(&c.found);
        let mut best = std::time::Duration::MAX;
        for _ in 0..5 {
            let started = std::time::Instant::now();
            let gave = data.run(&values, false).unwrap();
            best = best.min(started.elapsed());
            assert_eq!(gave.items, N as usize);
        }
        println!("{what}: 100000 items, best of 5: {best:?}");
        best
    };
    let filtered = time("filter", filter("item % 3 == 0 && item > 10", "", 0));
    let mut t = transform(
        items("input.people"),
        &[("name", "item.name"), ("age", "int(item.age)")],
    );
    t.sort_by = "row.age".into();
    time("transform", block::Kind::Transform(t));
    time("validate", block::Kind::Validate(validate(vec![adult()])));
    if !cfg!(debug_assertions) {
        assert!(
            filtered.as_millis() < 100,
            "a filter of 100000 items took {filtered:?}"
        );
    }
}
