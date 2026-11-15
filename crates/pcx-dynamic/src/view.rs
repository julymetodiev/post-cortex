// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The dynamic modules' view (docs/modules/dynamic.md): each module's version,
//! grants and what its check found, its files and what changed from the
//! approved version: a person never approves code they cannot see, and
//! approves on the board. A file edited or added is a new version, which waits
//! for that approval as an install does.

use pcx_module_api::view::build::{
    action, asking, bar, dialog, fields, multiline, node, source, stack, text,
};
use pcx_module_api::view::{Body, Component, List, Markdown, View, component, markdown};
use serde_json::json as value;

use crate::specs::path;

/// A module's detail: `{{row.*}}` is its `DynamicRecord`.
fn module() -> Component {
    let get = || value!({ "id": "{{row.module.id}}" });
    let difference = node(component::Kind::Markdown(Markdown {
        content: Some(markdown::Content::Source(source(
            &path("Get"),
            get(),
            "/difference",
        ))),
    }));
    let save = "Save this as a new version? It is checked again, and it runs only once a person approves it; the approved version goes on until then.";
    let add = dialog(
        "Add a file",
        value!({ "path": text("Path, beneath the module"),
                 "version": text("Version after this change"),
                 "text": multiline("Text") }),
        &["path", "version", "text"],
        None,
        action(
            "Add the file",
            &path("Edit"),
            value!({ "id": "{{row.module.id}}", "revision": "{{row.version}}",
                     "version": "{{form.version}}", "path": "{{form.path}}",
                     "text": "{{form.text}}" }),
            Some(save),
        ),
    );
    let edit = asking(
        action(
            "Edit",
            &path("Edit"),
            value!({ "id": "{{row.id}}", "revision": "{{row.revision}}",
                     "version": "{{form.version}}", "path": "{{row.path}}",
                     "text": "{{form.text}}" }),
            Some(save),
        ),
        value!({ "version": text("Version after this change"), "text": multiline("Text") }),
        &["version", "text"],
    );
    let files = node(component::Kind::List(Box::new(List {
        source: Some(source(&path("Files"), get(), "/files")),
        title_field: "shown".to_string(),
        subtitle_field: String::new(),
        row_actions: vec![edit],
        empty: "It has no files.".to_string(),
        detail: None,
        markdown: true,
    })));
    stack(vec![
        fields(source(&path("Get"), get(), "/module"), Vec::new()),
        difference,
        bar(vec![add]),
        files,
    ])
}

/// The module's views.
pub fn views() -> Vec<View> {
    let modules = node(component::Kind::List(Box::new(List {
        source: Some(source(&path("List"), value!({}), "/modules")),
        title_field: "module.id".to_string(),
        subtitle_field: "module.version".to_string(),
        row_actions: Vec::new(),
        empty: "No dynamic module is installed yet.".to_string(),
        detail: Some(Box::new(module())),
        markdown: false,
    })));
    vec![View {
        id: "dynamic".to_string(),
        title: "Dynamic modules".to_string(),
        description: "Modules installed while the host runs: their versions, grants, tests, files and what changed. A person approves them on the board.".to_string(),
        body: Some(Body::Root(Box::new(modules))),
    }]
}

#[cfg(test)]
mod tests {
    use pcx_module_api::Module;
    use pcx_module_launcher::Runtimes;

    /// The view fits the module's RPCs, as the host checks it at load.
    #[test]
    fn the_view_fits_the_rpcs() {
        let module = crate::DynamicModule::new(crate::LauncherConfig {
            socket: "/s".into(),
            sockets: "/d".into(),
            runtimes: Runtimes {
                python: "/py".into(),
                deno: "/deno".into(),
                imports: "/imports.json".into(),
            },
        });
        pcx_module_api::view::check(module.manifest()).unwrap();
    }
}
