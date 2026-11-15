// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx connect`: pair the local daemon with an organisation of the cloud,
//! or leave it (`docs/connect.md` section 1). The pairing itself is
//! `pcx_tunnel::pairing`'s; this asks the daemon for its workspaces, shows
//! the person the code and keeps the result.

use pcx_tunnel::pairing::{self, Approval, Paired};
use post_cortex_proto::tunnel::{Offer, StartPairingRequest, Workspace};

use crate::cli::VERSION;
use crate::daemon_client::DaemonClient;
use crate::local::Local;
use crate::profiles::CLOUD_URL;

/// Pair the daemon with an organisation of `cloud` (default: the hosted
/// cloud) under `name` (default: the machine's), offering a runner too
/// with `runner`, or leave it with `off`.
///
/// # Errors
/// The daemon is not running, it is paired already, the cloud cannot be
/// reached, or the person refused or did not answer in time.
pub async fn run(
    cloud: Option<String>,
    name: Option<String>,
    runner: bool,
    off: bool,
    local: &Local,
) -> Result<(), String> {
    let dir = &local.config_dir;
    let kept = Paired::load(dir).map_err(|e| e.to_string())?;
    if off {
        return leave(kept, local).await;
    }
    if let Some(paired) = kept {
        return Err(format!(
            "this daemon is connected to {} already ({}): `pcx connect --off` first",
            paired.org_name, paired.url
        ));
    }
    let workspaces = DaemonClient::connect(local)
        .await?
        .list_workspaces()
        .await?
        .into_iter()
        .map(|w| Workspace {
            id: w.id,
            name: w.name,
        })
        .collect();
    let machine = machine_name()?;
    let request = StartPairingRequest {
        name: name.unwrap_or_else(|| machine.clone()),
        machine,
        version: VERSION.to_owned(),
        workspaces,
        offers: if runner {
            vec![Offer::Space as i32, Offer::Runner as i32]
        } else {
            vec![Offer::Space as i32]
        },
    };
    let url = cloud.unwrap_or_else(|| CLOUD_URL.to_owned());
    let paired = pairing::pair(&url, request, show)
        .await
        .map_err(|e| e.to_string())?;
    paired.save(dir).map_err(|e| e.to_string())?;
    println!(
        "Connected to {}. Restart the daemon (`pcx stop`, then `pcx start --daemon`) to open its tunnel.",
        paired.org_name
    );
    Ok(())
}

/// Tell the cloud the daemon leaves, then forget the pairing.
async fn leave(kept: Option<Paired>, local: &Local) -> Result<(), String> {
    let Some(paired) = kept else {
        println!("This daemon is not connected to an organisation.");
        return Ok(());
    };
    pcx_tunnel::daemon::leave(&paired.connection())
        .await
        .map_err(|e| {
            format!("the cloud was not told ({e}); try again, or revoke the daemon in the app")
        })?;
    Paired::remove(&local.config_dir).map_err(|e| e.to_string())?;
    println!(
        "Left {}. Restart the daemon to close its tunnel.",
        paired.org_name
    );
    Ok(())
}

/// What the person approves, and where.
fn show(approval: &Approval) {
    println!();
    println!("  Open {}", approval.verification_url);
    println!("  and check the code: {}", approval.user_code);
    println!();
    println!(
        "Waiting for an owner or admin to approve (at most {} minutes)...",
        approval.expires_in.as_secs().div_ceil(60)
    );
}

/// This machine's name.
fn machine_name() -> Result<String, String> {
    #[cfg(unix)]
    {
        nix::unistd::gethostname()
            .map_err(|e| format!("the machine's name: {e}"))?
            .into_string()
            .map_err(|name| format!("the machine's name is not text: {name:?}"))
    }
    #[cfg(not(unix))]
    {
        crate::local::env_text("COMPUTERNAME")?
            .ok_or_else(|| "the machine's name: COMPUTERNAME is not set; pass --name".to_owned())
    }
}
