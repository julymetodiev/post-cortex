// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a manifest says of the API and the modules it uses.

use crate::ApiVersion;

/// The module API version this crate implements.
pub const API_VERSION: ApiVersion = ApiVersion { major: 1, minor: 0 };

/// Whether a module built against `module` runs on a host speaking
/// `host`: the same major, and a minor the host knows.
#[must_use]
pub fn compatible(host: &ApiVersion, module: &ApiVersion) -> bool {
    host.major == module.major && module.minor <= host.minor
}

/// The entry of `Manifest.uses` that uses every module that runs (the
/// workflow module, docs/modules/workflow.md): what each call reaches is
/// still bounded by its grants and the host's checks.
pub const EVERY_MODULE: &str = "*";

/// Whether `uses`, a manifest's `uses`, names module `module` (or every
/// module).
#[must_use]
pub fn uses(uses: &[String], module: &str) -> bool {
    uses.iter().any(|u| u == module || u == EVERY_MODULE)
}
