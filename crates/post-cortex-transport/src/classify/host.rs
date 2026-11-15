// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Calls of `pcx.module.v1.Host`, the host as its gRPC clients see it
//! ([`crate::host_api`]): `ListTools`, `ListModules`, `GetModule`,
//! `ListViews`, `GetView`, `ModuleLog` and `ListModels` read; `ConfigureModule` changes a module's
//! configuration, `GrantModule` what its sandbox lets it use.
//! [`host`] answers `None` for a path of another service.

use post_cortex_proto::module as wire;
use prost::Message;

use crate::classify::ClassifyError;

/// The path prefix of the service's RPCs.
const HOST: &str = "/pcx.module.v1.Host/";

/// A call of `pcx.module.v1.Host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCall {
    /// `ListTools`: every module's tools.
    ListTools,
    /// `ListModules`: every running module and its configuration.
    ListModules,
    /// `GetModule`: one running module whole, with its descriptors (W25).
    GetModule,
    /// `ListViews`: every running module's views.
    ListViews,
    /// `GetView`: one view of a module, with its HTML when it has one.
    GetView,
    /// `ModuleLog`: a running module's log (W25).
    ModuleLog,
    /// `ListModels`: every model of the running providers.
    ListModels,
    /// `ConfigureModule`: change module `id`'s configuration.
    ConfigureModule {
        /// The module.
        id: String,
    },
    /// `GrantModule`: change what module `id`'s sandbox lets it use.
    GrantModule {
        /// The module.
        id: String,
    },
}

/// The call at `path` with request `message`.
///
/// # Errors
/// Another RPC of the service, or a request that does not decode.
pub fn host(path: &str, message: &[u8]) -> Option<Result<HostCall, ClassifyError>> {
    let rpc = path.strip_prefix(HOST)?;
    Some(match rpc {
        "ListTools" => Ok(HostCall::ListTools),
        "ListModules" => Ok(HostCall::ListModules),
        "GetModule" => Ok(HostCall::GetModule),
        "ListViews" => Ok(HostCall::ListViews),
        "GetView" => Ok(HostCall::GetView),
        "ModuleLog" => Ok(HostCall::ModuleLog),
        "ListModels" => Ok(HostCall::ListModels),
        "ConfigureModule" => wire::ConfigureModuleRequest::decode(message)
            .map_err(|e| ClassifyError::Malformed(e.to_string()))
            .map(|request| HostCall::ConfigureModule { id: request.id }),
        "GrantModule" => wire::GrantModuleRequest::decode(message)
            .map_err(|e| ClassifyError::Malformed(e.to_string()))
            .map(|request| HostCall::GrantModule { id: request.id }),
        _ => Err(ClassifyError::UnknownRpc(path.to_string())),
    })
}

#[cfg(test)]
mod tests;
