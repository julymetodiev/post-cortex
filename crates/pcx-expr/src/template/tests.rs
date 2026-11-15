// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

fn inputs() -> Inputs {
    Inputs::new([
        ("name".to_string(), Type::String),
        ("count".to_string(), Type::Int),
        ("due".to_string(), Type::Timestamp),
        (
            "rows".to_string(),
            Type::parse("list<map<string, string>>").unwrap(),
        ),
    ])
    .unwrap()
}

fn values() -> Values {
    inputs()
        .read(
            json!({
                "name": "Ana <ana@example.com>",
                "count": 3,
                "due": "2026-11-02T09:00:00Z",
                "rows": [{"task": "a & b"}, {"task": "c"}],
            })
            .as_object()
            .unwrap(),
        )
        .unwrap()
}

#[test]
fn a_template_is_filled_and_escaped_for_html() {
    let template = Template::compile(
        "Dear {{ name }}: {{ count }} tasks, the first {{ rows[0].task }}, due {{ due }}{{ count > 2 ? '!' : '.' }}",
        &inputs(),
    )
    .unwrap();
    assert_eq!(
        template.fill(&values(), Escape::None).unwrap(),
        "Dear Ana <ana@example.com>: 3 tasks, the first a & b, due 2026-11-02T09:00:00Z!"
    );
    assert_eq!(
        template.fill(&values(), Escape::Html).unwrap(),
        "Dear Ana &lt;ana@example.com&gt;: 3 tasks, the first a &amp; b, due 2026-11-02T09:00:00Z!"
    );
    let joined = Template::compile("{{ rows.map(r, r.task).join(', ') }}", &inputs()).unwrap();
    assert_eq!(joined.fill(&values(), Escape::None).unwrap(), "a & b, c");
}

#[test]
fn a_template_is_checked_against_its_inputs_when_it_is_compiled() {
    let err = |source: &str| {
        Template::compile(source, &inputs())
            .map(|_| ())
            .unwrap_err()
    };
    let unknown = err("Hi {{ nmae }}");
    assert_eq!(
        (unknown.kind, unknown.at.as_str()),
        (ExprErrorKind::Type, "offset 6")
    );
    assert_eq!(err("{{ count + name }}").kind, ExprErrorKind::Type);
    assert_eq!(err("{{ rows }}").kind, ExprErrorKind::Type);
    assert_eq!(err("{{ count + }}").kind, ExprErrorKind::Syntax);
    let long = format!("{{{{ '{}' }}}}", "x".repeat(4100));
    assert_eq!(err(&long).kind, ExprErrorKind::Limit);
    let wide = format!("{{{{ {} }}}}", vec!["count"; 600].join(" + "));
    assert_eq!(err(&wide).kind, ExprErrorKind::Limit);
}

#[test]
fn values_are_held_to_the_inputs() {
    let given = |v: serde_json::Value| inputs().read(v.as_object().unwrap()).unwrap_err();
    let missing = given(json!({"name": "Ana", "count": 1, "due": "2026-11-02T09:00:00Z"}));
    assert_eq!(
        (missing.kind, missing.at.as_str()),
        (ExprErrorKind::Value, "rows")
    );
    let extra = given(
        json!({"name": "Ana", "count": 1, "due": "2026-11-02T09:00:00Z", "rows": [], "x": 1}),
    );
    assert_eq!(extra.at, "x");
    let wrong =
        given(json!({"name": "Ana", "count": "1", "due": "2026-11-02T09:00:00Z", "rows": []}));
    assert_eq!(wrong.at, "count");
}

#[test]
fn input_names_are_cel_identifiers() {
    for bad in ["Name", "1st", "in", "first-name", ""] {
        let refused = Inputs::new([(bad.to_string(), Type::String)]).unwrap_err();
        assert_eq!(refused.kind, ExprErrorKind::Input, "{bad}");
    }
    let twice = Inputs::new([
        ("a".to_string(), Type::String),
        ("a".to_string(), Type::Int),
    ])
    .unwrap_err();
    assert_eq!(twice.at, "a");
}

#[test]
fn an_evaluation_that_fails_or_spends_its_budget_says_so() {
    let inputs = Inputs::new([
        ("n".to_string(), Type::Int),
        ("items".to_string(), Type::List(Box::new(Type::Int))),
    ])
    .unwrap();
    let values = |items: usize| {
        inputs
            .read(
                json!({"n": 0, "items": vec![1; items]})
                    .as_object()
                    .unwrap(),
            )
            .unwrap()
    };
    let divide = Template::compile("{{ 10 / n }}", &inputs).unwrap();
    assert_eq!(
        divide.fill(&values(1), Escape::None).unwrap_err().kind,
        ExprErrorKind::Eval
    );
    let square = Template::compile(
        "{{ items.map(a, items.map(b, a * b).size()).size() }}",
        &inputs,
    )
    .unwrap();
    assert_eq!(square.fill(&values(10), Escape::None).unwrap(), "10");
    let spent = square.fill(&values(2_000), Escape::None).unwrap_err();
    assert_eq!(spent.kind, ExprErrorKind::Budget);
}

#[test]
fn the_deepest_chains_of_four_kib_neither_overflow_nor_pass() {
    // Called on a test thread (2 MiB, as a worker's): 4 KiB of the chains
    // the bracket rule does not bound.
    let inputs = Inputs::new([("b".to_string(), Type::Bool)]).unwrap();
    for chain in [
        format!("{{{{ {}1 }}}}", "-".repeat(4_000)),
        format!("{{{{ {}b }}}}", "!".repeat(4_000)),
        format!("{{{{ {}1 }}}}", "b ? 1 : ".repeat(500)),
    ] {
        let refused = Template::compile(&chain, &inputs).map(|_| ()).unwrap_err();
        assert_eq!(refused.kind, ExprErrorKind::Limit);
    }
}
