// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A host's dynamic modules kept as `dynamic` says (docs/modules/dynamic.md,
//! loaded): read when the host starts and when a version is approved or a
//! module removed, then the running set changed in place. A module that does
//! not launch is logged, and what ran before goes on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pcx_dynamic::{LauncherConfig, VERSION_APPROVED, VERSION_REMOVED, host_grants, memory_of};
use pcx_host::{Host, Origin, ProcessModule, ProcessSpec};
use pcx_module_api::{Module, Scope};
use pcx_module_launcher::{Remote, Requested};
use post_cortex_proto::dynamic::dynamic_client::DynamicClient;
use post_cortex_proto::dynamic::{ApprovedModule, Language, ListApprovedRequest};
use tokio_util::sync::CancellationToken;

/// How long reading the approved set may take: it may prepare a bundle.
const READING: Duration = Duration::from_mins(10);

/// Between reads while `dynamic` does not answer yet.
const AGAIN: Duration = Duration::from_secs(30);

/// A module's limit of open files.
const OPEN_FILES: u32 = 256;

/// Keep `host`'s dynamic modules, launched through `launcher` for `owner`,
/// until `stop`.
pub async fn keep_dynamic(
    host: Arc<Host>,
    launcher: LauncherConfig,
    owner: String,
    stop: CancellationToken,
) {
    let mut published = host.published();
    let mut running: HashMap<String, String> = HashMap::new();
    loop {
        let settled = match approved(&host, &stop).await {
            Ok(modules) => {
                change(&host, &launcher, &owner, modules, &mut running).await;
                true
            }
            Err(status) if status.code() == tonic::Code::NotFound => {
                // This host runs no `dynamic`: nothing to keep until it does.
                tracing::debug!("no dynamic module runs here");
                true
            }
            Err(status) => {
                tracing::warn!(error = %status.message(), "the approved modules were not read");
                false
            }
        };
        let again = async {
            if settled {
                std::future::pending::<()>().await;
            } else {
                tokio::time::sleep(AGAIN).await;
            }
        };
        tokio::select! {
            () = stop.cancelled() => return,
            () = again => {}
            () = changed(&mut published) => {}
        }
    }
}

/// Until an event of a dynamic module's set is published, or some were missed.
async fn changed(published: &mut Option<tokio::sync::broadcast::Receiver<String>>) {
    let Some(names) = published else {
        return std::future::pending().await;
    };
    loop {
        match names.recv().await {
            Ok(name) if name == VERSION_APPROVED || name == VERSION_REMOVED => return,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return std::future::pending().await;
            }
        }
    }
}

/// What `dynamic` says runs.
async fn approved(
    host: &Host,
    stop: &CancellationToken,
) -> Result<Vec<ApprovedModule>, tonic::Status> {
    let mut request = tonic::Request::new(ListApprovedRequest {});
    Scope::own("dynamic", "", READING, stop.child_token())
        .to_metadata(request.metadata_mut())
        .map_err(tonic::Status::from)?;
    request.set_timeout(READING);
    let answer = DynamicClient::new(host.channel(Origin::Client))
        .list_approved(request)
        .await?;
    Ok(answer.into_inner().modules)
}

/// The running set made `modules`.
async fn change(
    host: &Host,
    launcher: &LauncherConfig,
    owner: &str,
    modules: Vec<ApprovedModule>,
    running: &mut HashMap<String, String>,
) {
    let mut added: Vec<Arc<dyn Module>> = Vec::new();
    let mut hashes = Vec::new();
    for module in &modules {
        if running.get(&module.id) == Some(&module.sha256) {
            continue;
        }
        match launched(launcher, owner, module).await {
            Ok(process) => {
                added.push(Arc::new(process));
                hashes.push((module.id.clone(), module.sha256.clone()));
            }
            Err(e) => tracing::error!(module = %module.id, "a dynamic module did not launch: {e}"),
        }
    }
    let launched_ids: Vec<&str> = hashes.iter().map(|(id, _)| id.as_str()).collect();
    let removed: Vec<String> = running
        .keys()
        .filter(|id| launched_ids.contains(&id.as_str()) || !modules.iter().any(|m| m.id == **id))
        .cloned()
        .collect();
    if added.is_empty() && removed.is_empty() {
        return;
    }
    match host.change(added, &removed).await {
        Ok(()) => {
            for id in &removed {
                running.remove(id);
            }
            running.extend(hashes);
            tracing::info!(running = running.len(), "the dynamic modules changed");
        }
        Err(e) => tracing::error!("the dynamic modules did not change: {e}"),
    }
}

/// `module`'s process, launched through `launcher`.
async fn launched(
    launcher: &LauncherConfig,
    owner: &str,
    module: &ApprovedModule,
) -> Result<ProcessModule, String> {
    let grants = module.grants.clone().unwrap_or_default();
    let memory_mb = memory_of(grants.memory_mb);
    let language = Language::try_from(module.language)
        .map_err(|e| format!("its language {}: {e}", module.language))?;
    let environment: Option<PathBuf> =
        (!module.environment.is_empty()).then(|| PathBuf::from(&module.environment));
    let run = launcher.runtimes.module(
        language,
        Path::new(&module.bundle),
        environment.as_deref(),
        memory_mb,
    )?;
    let requested = Requested {
        owner: owner.to_string(),
        read: run.read,
        cwd: run.cwd,
        env: run.env,
        memory_mb,
        open_files: OPEN_FILES,
        sha256: String::new(),
    };
    let spec = ProcessSpec {
        id: module.id.clone(),
        command: run.program,
        args: run.args,
        // The launcher runs what its image holds.
        sha256: String::new(),
        config: None,
        grants: host_grants(&grants),
        uses: grants.uses,
        filesystem: Vec::new(),
    };
    ProcessModule::launch_by(
        &spec,
        &launcher.sockets,
        Arc::new(Remote::new(launcher.socket.clone(), requested)),
    )
    .await
    .map_err(|e| e.to_string())
}
