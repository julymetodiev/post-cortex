// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;

/// Optional fields stay `"nullable": true` (no `type` arrays with null) and
/// no schema carries `$schema`.
#[test]
fn schemas_keep_their_published_form() {
    let catalog = ToolCatalog::build().unwrap();
    for tool in catalog.tools() {
        let text = Value::Object(tool.input_schema.clone()).to_string();
        assert!(!text.contains("\"$schema\""), "{}: {text}", tool.name);
        assert!(!text.contains("\"null\""), "{}: {text}", tool.name);
        assert_eq!(tool.input_schema["type"], "object");
    }
}

/// The catalog has exactly the tools of the operation table.
#[test]
fn the_catalog_is_the_table() {
    let catalog = ToolCatalog::build().unwrap();
    let mut table = crate::tools::operations::mcp_tools();
    let mut names: Vec<&str> = catalog.tools().iter().map(|t| t.name.as_str()).collect();
    table.sort_unstable();
    names.sort_unstable();
    assert_eq!(names, table);
}

/// A relation's `context` is required and not empty in the schema a model
/// reads, as the memory refuses one without it.
#[test]
fn a_relations_context_is_required_and_not_empty() {
    let catalog = ToolCatalog::build().unwrap();
    let tool = catalog
        .tools()
        .iter()
        .find(|t| t.name == "update_conversation_context")
        .unwrap();
    let relation = &tool.input_schema["$defs"]["RelationItem"];
    assert!(
        relation["required"]
            .as_array()
            .unwrap()
            .contains(&Value::from("context")),
        "{relation}"
    );
    assert_eq!(
        relation["properties"]["context"]["minLength"], 1,
        "{relation}"
    );
}
