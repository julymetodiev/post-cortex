// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! `pcx hook`: a coding agent's hook (installed by `pcx setup --client`)
//! reads its payload on stdin and prints the reply. Hooks fail open:
//! whatever goes wrong, the agent carries on (the reason goes to stderr).

use std::time::Duration;

use crate::clients::{self, Client};
use crate::local::{Local, bearer_client};
use crate::{profiles, project};

/// How long a hook waits on the backend per request: a hook never holds
/// the agent up for long.
const HOOK_TIMEOUT: Duration = Duration::from_secs(3);

/// Run `client`'s hook for `event`: the project's session and workspace
/// when given, else the project the client runs in.
pub async fn run(
    client: Client,
    event: &str,
    session: Option<String>,
    workspace: Option<String>,
    profile: Option<String>,
    url: Option<String>,
    local: &Local,
) {
    // Hooks fail open: whatever goes wrong, the agent carries on.
    use std::io::Read;
    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("pcx hook {} {event}: stdin not read: {e}", client.arg());
    }
    let input = serde_json::from_str(&stdin).unwrap_or_else(|e| {
        eprintln!("pcx hook {} {event}: input is not JSON: {e}", client.arg());
        serde_json::Value::Null
    });
    // A hook from an older setup passes only --url: the self-hosted
    // daemon, whose token the `self` profile resolves.
    let name = profile.or_else(|| url.as_ref().map(|_| profiles::SELF_HOSTED.into()));
    let store = match profiles::Store::open(local) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("pcx hook {} {event}: {e}", client.arg());
            return;
        }
    };
    let resolved = store.resolve(name.as_deref());
    let (url, token) = match (url, resolved) {
        (Some(url), resolved) => (url, resolved.ok().and_then(|p| p.token)),
        (None, Ok(p)) => (p.url, p.token),
        (None, Err(e)) => {
            eprintln!("pcx hook {} {event}: {e}", client.arg());
            return;
        }
    };
    // A hook waits at most a few seconds on the backend.
    let http = match bearer_client(token.as_deref(), None, Some(HOOK_TIMEOUT)) {
        Ok(http) => http,
        Err(e) => {
            eprintln!("pcx hook {} {event}: {e}", client.arg());
            return;
        }
    };
    let env = match (session, workspace) {
        (Some(session), Some(workspace)) => {
            clients::HookEnv::new(&url, http, &session, &workspace, client.author())
        }
        // A global hook: the project the client runs in, unless it
        // has hooks of its own (project mode wins) or the client is
        // turned off in config.json.
        _ => {
            let host = store
                .host(client.author())
                .unwrap_or(profiles::HostSettings {
                    enabled: true,
                    capture: false,
                    workspace: None,
                });
            let Some(dir) =
                clients::common::payload_dir(&input).or_else(|| std::env::current_dir().ok())
            else {
                eprintln!("pcx hook {} {event}: no working directory", client.arg());
                return;
            };
            let root = project::project_root(&dir);
            if !host.enabled || clients::has_project_hooks(client, &root) {
                return;
            }
            clients::HookEnv::for_project(
                &url,
                http,
                client.author(),
                dir,
                store.dir().to_path_buf(),
                host.workspace,
                host.capture,
            )
        }
    };
    match clients::hook(client, event, input, &env).await {
        Ok(Some(out)) => println!("{out}"),
        Ok(None) => {}
        Err(e) => eprintln!("pcx hook {} {event}: {e}", client.arg()),
    }
}
