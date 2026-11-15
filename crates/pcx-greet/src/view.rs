// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The greet module's views: `greetings`, a tree over its RPCs, and
//! `card`, an HTML view that greets through the MCP Apps bridge.

use pcx_module_api::view::build::{action, dialog, node, source, stack, text};
use pcx_module_api::view::{Body, Column, Format, Stat, Table, View, component};
use serde_json::json as value;

use crate::module::path;

/// The HTML view's resource.
pub const CARD: &str = "ui://greet/card.html";

fn column(field: &str, label: &str) -> Column {
    Column {
        field: field.to_string(),
        label: label.to_string(),
        format: Format::Text.into(),
    }
}

/// How many greetings, a form to greet someone, and the greetings.
pub fn greetings_view() -> View {
    let count = node(component::Kind::Stat(Stat {
        label: "Greetings".to_string(),
        source: Some(source(&path("ListGreetings"), value!({}), "/count")),
        format: Format::Number.into(),
    }));
    let greet = dialog(
        "Greet someone",
        value!({ "name": text("Name") }),
        &["name"],
        None,
        action(
            "Greet",
            &path("Greet"),
            value!({ "name": "{{form.name}}" }),
            None,
        ),
    );
    let greetings = node(component::Kind::Table(Box::new(Table {
        source: Some(source(&path("ListGreetings"), value!({}), "/greetings")),
        columns: vec![column("name", "Name"), column("text", "Greeting")],
        row_actions: vec![
            action(
                "Greet again",
                &path("Greet"),
                value!({ "name": "{{row.name}}" }),
                None,
            ),
            action(
                "Forget",
                &path("Forget"),
                value!({}),
                Some("Forget every greeting?"),
            ),
        ],
        empty: "Nobody was greeted yet.".to_string(),
        detail: None,
    })));
    View {
        id: "greetings".to_string(),
        title: "Greetings".to_string(),
        description: "Who was greeted, and a form to greet someone.".to_string(),
        body: Some(Body::Root(Box::new(stack(vec![count, greet, greetings])))),
    }
}

/// The HTML card.
pub fn card_view() -> View {
    View {
        id: "card".to_string(),
        title: "Greeting card".to_string(),
        description: "Greets through the MCP Apps bridge.".to_string(),
        body: Some(Body::ResourceUri(CARD.to_string())),
    }
}

/// The card: a name field and a button that calls the `greet` tool
/// through the bridge (MCP JSON-RPC over `postMessage`); the tool answers
/// a `Greeting` as JSON.
pub const CARD_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Greeting card</title>
<style>body{font:14px system-ui;margin:16px}input,button{font:inherit;padding:6px 10px}#out{margin-top:12px}</style>
</head><body>
<input id="name" placeholder="Name"> <button id="go">Greet</button>
<div id="out"></div>
<script>
let next = 1; const waiting = new Map();
function ask(method, params) {
  const id = next++;
  window.parent.postMessage({jsonrpc: "2.0", id, method, params}, "*");
  return new Promise((ok, fail) => waiting.set(id, {ok, fail}));
}
window.addEventListener("message", (e) => {
  const m = e.data; if (!m || m.jsonrpc !== "2.0" || !waiting.has(m.id)) return;
  const w = waiting.get(m.id); waiting.delete(m.id);
  m.error ? w.fail(m.error) : w.ok(m.result);
});
ask("ui/initialize", {appInfo: {name: "greet-card", version: "1"}, appCapabilities: {}, protocolVersion: "2025-06-18"})
  .then(() => window.parent.postMessage({jsonrpc: "2.0", method: "ui/notifications/initialized"}, "*"));
document.getElementById("go").onclick = async () => {
  const out = document.getElementById("out");
  try {
    const r = await ask("tools/call", {name: "greet", arguments: {name: document.getElementById("name").value}});
    // The answer's data: its structured content, or the JSON after
    // "Results:" in its text (an MCP host's).
    const data = r.structuredContent
      ?? JSON.parse(r.content.map((c) => c.text).join("").split("Results:").pop());
    out.textContent = data.text;
  } catch (e) { out.textContent = e.message; }
};
</script></body></html>
"#;

#[cfg(test)]
mod tests {
    use pcx_module_api::Module;

    /// The views fit the module's RPCs, as the host checks them at load.
    #[test]
    fn the_views_fit_the_rpcs() {
        let module = crate::module::Greet::new();
        pcx_module_api::view::check(module.manifest()).unwrap();
    }
}
