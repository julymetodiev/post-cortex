// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn text_and_expressions_in_order() {
    assert_eq!(
        pieces("Hi {{ name }}, {{n}} new").unwrap(),
        vec![
            Piece::Text("Hi "),
            Piece::Expr(" name ", 5),
            Piece::Text(", "),
            Piece::Expr("n", 17),
            Piece::Text(" new"),
        ]
    );
    assert_eq!(
        pieces("no parts }}").unwrap(),
        vec![Piece::Text("no parts }}")]
    );
    assert!(pieces("").unwrap().is_empty());
}

#[test]
fn an_expression_ends_outside_its_strings_and_brackets() {
    let parts = pieces(r#"{{ {"a": {"b": "}}"}}["a"]["b"] }}!"#).unwrap();
    assert_eq!(
        parts,
        vec![
            Piece::Expr(r#" {"a": {"b": "}}"}}["a"]["b"] "#, 2),
            Piece::Text("!")
        ]
    );
    let raw = pieces(r#"{{ r"\" + '''}}''' }}"#).unwrap();
    assert_eq!(raw, vec![Piece::Expr(r#" r"\" + '''}}''' "#, 2)]);
}

#[test]
fn a_part_that_does_not_close_or_nests_too_deep_is_refused() {
    let open = pieces("Hi {{ name").unwrap_err();
    assert_eq!(
        (open.kind, open.at.as_str()),
        (ExprErrorKind::Syntax, "offset 3")
    );
    assert_eq!(
        pieces("{{ 'name }}").unwrap_err().kind,
        ExprErrorKind::Syntax
    );
    assert_eq!(pieces("{{  }}").unwrap_err().kind, ExprErrorKind::Syntax);
    let deep = format!("{{{{ {}1{} }}}}", "(".repeat(33), ")".repeat(33));
    assert_eq!(pieces(&deep).unwrap_err().kind, ExprErrorKind::Limit);
    let fits = format!("{{{{ {}1{} }}}}", "(".repeat(32), ")".repeat(32));
    assert!(pieces(&fits).is_ok());
}
