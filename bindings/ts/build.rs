// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The addon's link setup: Node-API's symbols come from the runtime that
//! loads it.

fn main() {
    napi_build::setup();
}
