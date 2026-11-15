// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's configuration written into `daemon.toml` (ARCHITECTURE
//! section 7, Module UI, settings): only the `config` of a configured
//! `[[modules]]` entry changes, never its command, hash or grants, and the
//! rest of the file keeps its comments and layout. The file is written to a
//! temporary file beside it and renamed, with its permissions (it may hold
//! the auth token).

use std::io::ErrorKind;
use std::path::Path;

use pcx_host::SettingsError;
use serde_json::Value;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table};

/// `daemon.toml` as text.
///
/// # Errors
/// `Store` when there is no file or it cannot be read.
pub fn read(path: &Path) -> Result<String, SettingsError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(SettingsError::Store {
            reason: format!("{} does not exist", path.display()),
        }),
        Err(e) => Err(SettingsError::Store {
            reason: format!("read {}: {e}", path.display()),
        }),
    }
}

/// Replace `path` with `text`, keeping its permissions.
///
/// # Errors
/// `Store` when it cannot be written.
pub fn write(path: &Path, text: &str) -> Result<(), SettingsError> {
    let store = |what: &str, e: std::io::Error| SettingsError::Store {
        reason: format!("{what} {}: {e}", path.display()),
    };
    let permissions = std::fs::metadata(path)
        .map_err(|e| store("read the permissions of", e))?
        .permissions();
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".pcx-new");
    let temporary = Path::new(&temporary);
    let written = std::fs::write(temporary, text)
        .map_err(|e| store("write beside", e))
        .and_then(|()| {
            std::fs::set_permissions(temporary, permissions)
                .map_err(|e| store("set the permissions beside", e))
        })
        .and_then(|()| std::fs::rename(temporary, path).map_err(|e| store("replace", e)));
    if written.is_err() {
        // A failed write leaves nothing beside the file.
        match std::fs::remove_file(temporary) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("remove {}: {e}", temporary.display()),
        }
    }
    written
}

/// `text` (a `daemon.toml`) with module `id`'s `config` set to `config`.
///
/// # Errors
/// `NotConfigurable` when no `[[modules]]` entry has the id;
/// `Unrepresentable` for a value TOML cannot hold (`null`, an integer past
/// `i64`); `Store` when the file does not parse.
pub fn with_config(text: &str, id: &str, config: &Value) -> Result<String, SettingsError> {
    let mut doc: DocumentMut = text.parse().map_err(|e| SettingsError::Store {
        reason: format!("daemon.toml does not parse: {e}"),
    })?;
    let not_configurable = || SettingsError::NotConfigurable {
        module: id.to_string(),
    };
    let entry = match doc.get_mut("modules") {
        Some(Item::ArrayOfTables(entries)) => entries
            .iter_mut()
            .find(|t| t.get("id").and_then(Item::as_str) == Some(id))
            .map(Entry::Table),
        Some(Item::Value(toml_edit::Value::Array(entries))) => entries
            .iter_mut()
            .filter_map(toml_edit::Value::as_inline_table_mut)
            .find(|t| t.get("id").and_then(toml_edit::Value::as_str) == Some(id))
            .map(Entry::Inline),
        _ => None,
    }
    .ok_or_else(not_configurable)?;
    let value = to_toml(config, &mut String::new())?;
    match entry {
        // The new value keeps the old one's comments and spacing, and a
        // `[modules.config]` table stays a table where it was.
        Entry::Table(table) => {
            let item = match (table.get("config"), value) {
                (Some(Item::Table(old)), toml_edit::Value::InlineTable(inline)) => {
                    let mut new = inline.into_table();
                    *new.decor_mut() = old.decor().clone();
                    new.set_position(old.position());
                    Item::Table(new)
                }
                (Some(Item::Value(old)), mut value) => {
                    *value.decor_mut() = old.decor().clone();
                    Item::Value(value)
                }
                (_, value) => Item::Value(value),
            };
            table.insert("config", item);
        }
        Entry::Inline(table) => {
            let mut value = value;
            if let Some(old) = table.get("config") {
                *value.decor_mut() = old.decor().clone();
            }
            table.insert("config", value);
        }
    }
    Ok(doc.to_string())
}

/// A `[[modules]]` entry, as a table or inline.
enum Entry<'a> {
    Table(&'a mut Table),
    Inline(&'a mut InlineTable),
}

/// `value` as TOML; `field` is its path, for an error.
fn to_toml(value: &Value, field: &mut String) -> Result<toml_edit::Value, SettingsError> {
    let unrepresentable = |field: &str, what: &str| SettingsError::Unrepresentable {
        reason: if field.is_empty() {
            format!("the configuration is {what}, which daemon.toml cannot hold")
        } else {
            format!("setting {field} is {what}, which daemon.toml cannot hold")
        },
    };
    Ok(match value {
        Value::Null => return Err(unrepresentable(field, "null")),
        Value::Bool(b) => (*b).into(),
        Value::String(s) => s.as_str().into(),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.into(),
            (None, Some(_)) if n.is_u64() => return Err(unrepresentable(field, "past i64")),
            (None, Some(f)) => f.into(),
            (None, None) => return Err(unrepresentable(field, "not a number TOML has")),
        },
        Value::Array(items) => {
            let mut array = Array::new();
            for (i, item) in items.iter().enumerate() {
                let len = field.len();
                field.push_str(&format!("[{i}]"));
                array.push(to_toml(item, field)?);
                field.truncate(len);
            }
            toml_edit::Value::Array(array)
        }
        Value::Object(fields) => {
            let mut table = InlineTable::new();
            for (name, item) in fields {
                let len = field.len();
                if !field.is_empty() {
                    field.push('.');
                }
                field.push_str(name);
                table.insert(name, to_toml(item, field)?);
                field.truncate(len);
            }
            toml_edit::Value::InlineTable(table)
        }
    })
}

#[cfg(test)]
mod tests;
