// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a module's sandbox lets it use, written into `daemon.toml` (docs/
//! host.md, settings, grants): the `filesystem`, `network` and `spawn` of
//! its `[[builtins]]` or `[[modules]]` entry change, its other grants and
//! the rest of the file (comments, layout) stay; a built-in module the file
//! does not name gets a `[[builtins]]` entry, which turns it on.

use pcx_host::{Grant, ModuleGrants, SettingsError};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

/// The grants a person gives on the machine, as the file names them.
const GIVEN: [(Grant, &str); 3] = [
    (Grant::Filesystem, "filesystem"),
    (Grant::Spawn, "spawn"),
    (Grant::Network, "network"),
];

/// `text` (a `daemon.toml`) with module `id` given `grants`; `builtin`:
/// it is a built-in module, added when no entry names it.
///
/// # Errors
/// `Unknown` when no entry names `id` and it is no built-in module; `Store`
/// when the file does not parse or its entries are not tables.
pub fn with_grants(
    text: &str,
    id: &str,
    grants: &ModuleGrants,
    builtin: bool,
) -> Result<String, SettingsError> {
    let mut doc: DocumentMut = text.parse().map_err(|e| SettingsError::Store {
        reason: format!("daemon.toml does not parse: {e}"),
    })?;
    let named = ["builtins", "modules"]
        .into_iter()
        .find(|list| entry(&mut doc, list, id).is_some());
    let entry = match named {
        Some(list) => entry(&mut doc, list, id),
        None if builtin => Some(added(&mut doc, id)?),
        None => None,
    }
    .ok_or_else(|| SettingsError::Unknown {
        module: id.to_string(),
    })?;
    give(entry, grants);
    Ok(doc.to_string())
}

/// The entry of `list` (`[[builtins]]` or `[[modules]]`, tables or an
/// inline array) whose id is `id`.
fn entry<'a>(doc: &'a mut DocumentMut, list: &str, id: &str) -> Option<&'a mut dyn TableLike> {
    match doc.get_mut(list)? {
        Item::ArrayOfTables(entries) => entries
            .iter_mut()
            .find(|t| t.get("id").and_then(Item::as_str) == Some(id))
            .map(|t| t as &mut dyn TableLike),
        Item::Value(Value::Array(entries)) => entries
            .iter_mut()
            .filter_map(Value::as_inline_table_mut)
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id))
            .map(|t| t as &mut dyn TableLike),
        _ => None,
    }
}

/// A new `[[builtins]]` entry of `id`, after the others.
fn added<'a>(doc: &'a mut DocumentMut, id: &str) -> Result<&'a mut dyn TableLike, SettingsError> {
    let not_tables = || SettingsError::Store {
        reason: "daemon.toml's builtins are not a list of tables".to_string(),
    };
    let list = doc
        .entry("builtins")
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
    match list {
        Item::ArrayOfTables(entries) => {
            let mut table = Table::new();
            table.insert("id", toml_edit::value(id));
            entries.push(table);
            entries
                .iter_mut()
                .last()
                .map(|t| t as &mut dyn TableLike)
                .ok_or_else(not_tables)
        }
        Item::Value(Value::Array(entries)) => {
            let mut table = InlineTable::new();
            table.insert("id", id.into());
            entries.push(table);
            entries
                .iter_mut()
                .last()
                .and_then(Value::as_inline_table_mut)
                .map(|t| t as &mut dyn TableLike)
                .ok_or_else(not_tables)
        }
        _ => Err(not_tables()),
    }
}

/// `entry` given `grants`: its other grants kept, in their order.
fn give(entry: &mut dyn TableLike, grants: &ModuleGrants) {
    let given: Vec<&str> = GIVEN.iter().map(|(_, name)| *name).collect();
    let mut names = Array::new();
    if let Some(Item::Value(Value::Array(before))) = entry.get("grants") {
        for name in before.iter().filter_map(Value::as_str) {
            if !given.contains(&name) {
                names.push(name);
            }
        }
    }
    for (grant, name) in GIVEN {
        let on = match grant {
            Grant::Filesystem => !grants.roots.is_empty(),
            Grant::Spawn => grants.spawn,
            Grant::Network => grants.network,
            Grant::ReadConversation | Grant::WriteMemory | Grant::UiHtml | Grant::Models => false,
        };
        if on {
            names.push(name);
        }
    }
    entry.insert("grants", toml_edit::value(names));
    if grants.roots.is_empty() {
        entry.remove("filesystem");
        return;
    }
    let mut roots = Array::new();
    for root in &grants.roots {
        let mut table = InlineTable::new();
        table.insert("path", root.path.to_string_lossy().as_ref().into());
        table.insert("write", root.write.into());
        roots.push(table);
    }
    entry.insert("filesystem", toml_edit::value(roots));
}

#[cfg(test)]
mod tests;
