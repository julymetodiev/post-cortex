// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The `code_reference` argument of a write, in every form the tool
//! documents.

use post_cortex_core::core::context_update::CodeReference;

use crate::tools::Failure;
use crate::tools::check::{self, Problem};

/// Parse an optional `code_reference` parameter.
///
/// Accepts the forms the tool schema documents, plus the canonical struct:
/// - `null` / absent → `None`
/// - `"src/main.rs"`, `"src/main.rs:42"`, `"src/main.rs:10-20"`
/// - `{"file": "src/main.rs", "line": 42, "function": "process_data"}`
///   (`file_path`, `start_line`, `end_line`, `code_snippet`,
///   `change_description`, `commit_hash`, `branch` are honoured too)
///
/// Anything else is rejected instead of being silently dropped.
pub fn code_reference(
    value: Option<&serde_json::Value>,
    parameter_path: &str,
) -> Result<Option<CodeReference>, Failure> {
    use serde_json::Value;

    let invalid = |why: &str, received: &Value| {
        check::invalid(
            format!("Invalid code_reference: {why}: invalid code_reference"),
            parameter_path,
            Problem::NotOneOf,
            Some(received.clone()),
        )
        .expected(
            "\"path[:line[-end]]\" string or {\"file\": ..., \"line\": ..., \"function\": ...} object",
        )
        .hint("Examples: \"src/main.rs:42\" or {\"file\": \"src/main.rs\", \"line\": 42}")
    };

    let reference = |file_path: String, start: u32, end: u32| CodeReference {
        file_path,
        start_line: start,
        end_line: end.max(start),
        code_snippet: String::new(),
        commit_hash: None,
        branch: None,
        change_description: String::new(),
    };

    match value {
        None | Some(Value::Null) => Ok(None),
        Some(v @ Value::String(s)) => {
            let s = s.trim();
            // Only treat a trailing `:N` / `:N-M` as a line spec, so paths
            // containing ':' (e.g. Windows drives) still parse.
            let (path, lines) = match s.rsplit_once(':') {
                Some((p, spec))
                    if !p.is_empty()
                        && !spec.is_empty()
                        && spec.chars().all(|c| c.is_ascii_digit() || c == '-') =>
                {
                    (p, Some(spec))
                }
                _ => (s, None),
            };
            if path.is_empty() {
                return Err(invalid("empty file path", v));
            }
            let (start, end) = match lines {
                None => (0, 0),
                Some(spec) => {
                    let mut parts = spec.splitn(2, '-');
                    let start = parts.next().and_then(|p| p.parse::<u32>().ok());
                    let end = match parts.next() {
                        Some(p) => p.parse::<u32>().ok(),
                        None => start,
                    };
                    match (start, end) {
                        (Some(s), Some(e)) => (s, e),
                        _ => return Err(invalid("malformed line range", v)),
                    }
                }
            };
            Ok(Some(reference(path.to_string(), start, end)))
        }
        Some(v @ Value::Object(map)) => {
            let str_field = |k: &str| map.get(k).and_then(Value::as_str).map(str::to_string);
            let line_field = |k: &str| -> Result<Option<u32>, Failure> {
                match map.get(k) {
                    None | Some(Value::Null) => Ok(None),
                    Some(n) => n
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .map(Some)
                        .ok_or_else(|| {
                            invalid(&format!("'{k}' must be a non-negative integer"), v)
                        }),
                }
            };

            let file_path = str_field("file_path")
                .or_else(|| str_field("file"))
                .filter(|p| !p.trim().is_empty())
                .ok_or_else(|| invalid("missing 'file' (or 'file_path')", v))?;
            let start = line_field("start_line")?
                .or(line_field("line")?)
                .unwrap_or(0);
            let end = line_field("end_line")?.unwrap_or(start);

            let mut code_ref = reference(file_path, start, end);
            code_ref.code_snippet = str_field("code_snippet").unwrap_or_default();
            code_ref.commit_hash = str_field("commit_hash");
            code_ref.branch = str_field("branch");
            code_ref.change_description = str_field("change_description")
                .or_else(|| str_field("description"))
                .or_else(|| str_field("function").map(|f| format!("in {f}")))
                .unwrap_or_default();
            Ok(Some(code_ref))
        }
        Some(v) => Err(invalid("expected a string or an object", v)),
    }
}

#[cfg(test)]
mod tests;
