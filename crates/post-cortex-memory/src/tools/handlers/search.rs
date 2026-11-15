// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `semantic_search`: session, workspace or global scope, one
//! response shape for all three.

use crate::tools::parse::FieldHint;

mod run;
pub use run::handle;

const HINTS: &[FieldHint] = &[
    FieldHint {
        field: "scope",
        expected: "one of: session, workspace, global",
        hint: "Valid scopes: 'session' (requires scope_id), 'workspace' (requires scope_id), 'global' (default)",
    },
    FieldHint {
        field: "scope_id",
        expected: "UUID string (required when scope is 'session' or 'workspace')",
        hint: "When scope is 'session' or 'workspace', provide the session/workspace UUID as scope_id",
    },
    FieldHint {
        field: "query",
        expected: "search query string",
        hint: "Provide a text query to search for in the conversation history",
    },
];
