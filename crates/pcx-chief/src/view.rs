// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief's view (ARCHITECTURE section 8, its view): a chat with it
//! (W25); its rights and budget field by field, changed in its settings
//! form; its conversations, each one's detail its actions, each action's
//! its whole input.

use pcx_module_api::view::build::{bar, fields, node, settings, source, stack};
use pcx_module_api::view::{Body, Chat, Column, Format, List, Table, View, component};
use serde_json::json as value;

use crate::domain::MODULE_ID;
use crate::specs::path;

fn column(field: &str, label: &str, format: Format) -> Column {
    Column {
        field: field.to_string(),
        label: label.to_string(),
        format: format.into(),
    }
}

/// The chief's views.
pub fn views() -> Vec<View> {
    // A person's chat with the chief (W25), beside the view.
    let ask = node(component::Kind::Chat(Chat {
        agent: MODULE_ID.to_string(),
        member: String::new(),
        open: "Ask the chief".to_string(),
    }));
    let rights = fields(
        source(&path("GetRights"), value!({}), ""),
        vec![settings("Change in its settings")],
    );
    // An action's whole input: `{{row.id}}` is the action.
    let input = fields(
        source(
            &path("GetAction"),
            value!({ "id": "{{row.id}}" }),
            "/action/input",
        ),
        Vec::new(),
    );
    let actions = node(component::Kind::Table(Box::new(Table {
        source: Some(source(
            &path("ListActions"),
            value!({ "conversation_id": "{{row.id}}" }),
            "/actions",
        )),
        columns: vec![
            column("action.tool", "Tool", Format::Text),
            column("action.failed", "Failed", Format::YesNo),
            column("action.input", "Input", Format::Fields),
            column("updated_at", "When", Format::Time),
        ],
        row_actions: Vec::new(),
        empty: "No tool calls in this conversation.".to_string(),
        detail: Some(Box::new(input)),
    })));
    let talks = node(component::Kind::List(Box::new(List {
        source: Some(source(&path("ListTalks"), value!({}), "/talks")),
        title_field: "question".to_string(),
        subtitle_field: "talk.asked_by".to_string(),
        row_actions: Vec::new(),
        empty: "No one has asked the chief yet.".to_string(),
        detail: Some(Box::new(actions)),
        markdown: false,
    })));
    vec![View {
        id: "chief".to_string(),
        title: "Chief".to_string(),
        description: "The chief's rights and budget, and what it did in each conversation."
            .to_string(),
        body: Some(Body::Root(Box::new(stack(vec![
            bar(vec![ask]),
            rights,
            talks,
        ])))),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    use pcx_module_api::Module;

    /// The view fits the module's RPCs, as the host checks it at load.
    #[test]
    fn the_view_fits_the_rpcs() {
        let module = crate::ChiefModule::new();
        pcx_module_api::view::check(module.manifest()).unwrap();
        assert_eq!(views().len(), 1);
    }
}
