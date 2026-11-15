// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// `docs/TOOLS.md` is the reference rendered from the table. Regenerate it
/// with `PCX_WRITE_TOOLS_REFERENCE=1 cargo test -p post-cortex-memory`.
#[test]
fn the_tools_reference_is_rendered_from_the_table() {
    let rendered = tools_reference(&ToolCatalog::build().unwrap());
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/TOOLS.md");
    if std::env::var_os("PCX_WRITE_TOOLS_REFERENCE").is_some() {
        std::fs::write(path, &rendered).unwrap();
    }
    let stored = std::fs::read_to_string(path).unwrap_or_default();
    assert_eq!(stored, rendered, "docs/TOOLS.md is stale; regenerate it");
}
