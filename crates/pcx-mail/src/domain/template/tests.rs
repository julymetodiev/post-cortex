// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::mail::Input;
use serde_json::json;

use super::*;

fn example(value: Json) -> pbjson_types::Value {
    serde_json::from_value(value).unwrap()
}

fn report() -> Template {
    Template {
        name: "weekly-report".into(),
        description: "The report each Monday".into(),
        inputs: vec![
            Input {
                name: "name".into(),
                r#type: "string".into(),
                example: Some(example(json!("Ana"))),
                description: String::new(),
            },
            Input {
                name: "open".into(),
                r#type: "list<string>".into(),
                example: Some(example(json!(["Paint the lighthouse"]))),
                description: String::new(),
            },
        ],
        subject: "{{ size(open) }} open tasks for {{ name }}".into(),
        html: "<p>Hi {{ name }}, first: {{ open[0] }}</p>".into(),
        text: "Hi {{ name }}, first: {{ open[0] }}".into(),
    }
}

#[test]
fn a_template_is_filled_with_its_examples_or_values() {
    let checked = check(&report()).unwrap();
    let filled = checked.fill(None).unwrap();
    assert_eq!(filled.subject, "1 open tasks for Ana");
    assert_eq!(filled.text, "Hi Ana, first: Paint the lighthouse");

    let values = json!({"name": "Bob & <Co>", "open": ["a", "b"]});
    let filled = checked.fill(values.as_object()).unwrap();
    assert_eq!(filled.subject, "2 open tasks for Bob & <Co>");
    assert_eq!(filled.html, "<p>Hi Bob &amp; &lt;Co&gt;, first: a</p>");
    assert_eq!(filled.text, "Hi Bob & <Co>, first: a");
}

#[test]
fn values_that_break_the_inputs_name_the_input() {
    let checked = check(&report()).unwrap();
    let missing = json!({"name": "Ana"});
    assert_eq!(
        checked.fill(missing.as_object()).unwrap_err().field,
        "values.open"
    );
    let empty = json!({"name": "Ana", "open": []});
    // An expression that fails names its part: no first task to show.
    assert_eq!(checked.fill(empty.as_object()).unwrap_err().field, "html");
}

#[test]
fn a_template_that_breaks_a_rule_names_its_field() {
    let field = |change: fn(&mut Template)| {
        let mut template = report();
        change(&mut template);
        check(&template).map(|_| ()).unwrap_err().field
    };
    assert_eq!(field(|t| t.name = "Weekly report".into()), "name");
    assert_eq!(
        field(|t| t.inputs[1].r#type = "list".into()),
        "inputs[1].type"
    );
    assert_eq!(field(|t| t.inputs[0].example = None), "inputs[0].example");
    assert_eq!(
        field(|t| t.inputs[0].example = Some(example(json!(1)))),
        "inputs[0].example"
    );
    assert_eq!(field(|t| t.inputs[1].name = "name".into()), "inputs");
    assert_eq!(field(|t| t.subject = "{{ nme }}".into()), "subject");
    assert_eq!(field(|t| t.text = " ".into()), "text");
    assert_eq!(field(|t| t.html = "{{ open }}".into()), "html");
}
