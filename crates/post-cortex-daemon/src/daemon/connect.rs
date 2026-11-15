// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A daemon connected to an organisation of the cloud (`docs/connect.md`
//! sections 4 and 7): when `pcx connect` left `connect.json` beside
//! `daemon.toml`, the daemon serves the routes of a hosted tenant over its
//! tunnel, to the organisation's people the gateway lets through, and, as
//! a runner, the services of the tool modules it lends, to a hosted
//! tenant's agents.

use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, Request, Response};
use pcx_host::{Host, Origin};
use pcx_module_api::metadata;
use pcx_tunnel::daemon::RequestBody;
use pcx_tunnel::pairing::Paired;
use post_cortex_memory::Memory;
use post_cortex_ports::DocumentKinds;
use post_cortex_proto::module::SandboxRoot;
use post_cortex_proto::tunnel::{Hello, Offer, Welcome, Workspace};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tower::util::{BoxCloneSyncService, MapRequest};
use tower::{Service, ServiceExt};
use tracing::{error, info, warn};

use crate::compose::{BrowserAccess, Builtin, RouteConfig};
use crate::daemon::host_allowlist::HostPolicy;

/// Serve the tunnel of the pairing kept in `dir`, if the daemon is paired,
/// as a task of `background` until `ct` ends, lending the tool modules of
/// `lent` as a runner. The routes are a hosted tenant's: no bearer (nothing
/// outside the process reaches them, and the gateway has authenticated the
/// caller), browser routes behind the gateway, no module configured
/// through them.
///
/// # Errors
/// `connect.json` cannot be read or is no pairing.
pub fn serve(
    dir: &Path,
    host: Arc<Host>,
    memory: Arc<Memory>,
    kinds: Arc<dyn DocumentKinds>,
    lent: &[Builtin],
    background: &mut JoinSet<()>,
    ct: &CancellationToken,
) -> Result<(), String> {
    let Some(paired) = Paired::load(dir).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let runner = Runner::new(&host, lent);
    let routes = crate::compose::routes(
        Arc::clone(&host),
        Arc::clone(&memory),
        kinds,
        None,
        RouteConfig {
            host_policy: HostPolicy::Any,
            auth_token: None,
            browser: BrowserAccess::Proxied {
                link_base: format!("/v1/daemons/{}", paired.daemon_id),
            },
        },
        ct,
    );
    info!(
        organisation = %paired.org_name,
        daemon = %paired.daemon_id,
        cloud = %paired.url,
        "  Connected to an organisation: serving it over the tunnel"
    );
    let lends = runner.clone();
    background.spawn(pcx_tunnel::daemon::run(
        paired.connection(),
        move |welcome: &Welcome| {
            dispatch(reached(routes.clone(), &welcome.workspaces), runner.clone())
        },
        move || hello(Arc::clone(&memory), lends.clone()),
        ct.child_token(),
    ));
    Ok(())
}

/// What the daemon lends as a runner: the tool modules of `[runner]` that
/// run, called as an agent's calls of the host (each module's sandbox and
/// grants hold).
#[derive(Clone)]
struct Runner {
    host: Arc<Host>,
    modules: Arc<[Builtin]>,
    call: BoxCloneSyncService<Request<axum::body::Body>, Response<axum::body::Body>, Infallible>,
}

impl Runner {
    fn new(host: &Arc<Host>, lent: &[Builtin]) -> Self {
        Self {
            host: Arc::clone(host),
            modules: lent.into(),
            call: post_cortex_transport::modules::service(
                host.channel(Origin::Agent),
                crate::daemon::auth::from_client,
            ),
        }
    }

    /// The manifests of the lent modules that run now.
    fn manifests(&self) -> Vec<pcx_module_api::Manifest> {
        self.modules
            .iter()
            .filter_map(|module| {
                let running = self.host.module(module.id()).map(|info| info.manifest);
                if running.is_none() {
                    warn!(
                        module = module.id(),
                        "a module [runner] lends does not run: not lent"
                    );
                }
                running
            })
            .collect()
    }

    /// The directories the lent modules that run now may use (the union
    /// of their roots, a root once, writable when any may write there),
    /// among which a board's repository is chosen in the app.
    fn roots(&self) -> Vec<SandboxRoot> {
        let mut roots: Vec<SandboxRoot> = Vec::new();
        let policies = self
            .modules
            .iter()
            .filter_map(|module| self.host.module(module.id()).map(|info| info.sandbox));
        for root in policies.flat_map(|policy| policy.roots) {
            let path = root.path.display().to_string();
            match roots.iter_mut().find(|r| r.path == path) {
                Some(known) => known.write |= root.write,
                None => roots.push(SandboxRoot {
                    path,
                    write: root.write,
                }),
            }
        }
        roots
    }

    /// Whether `path` (`/<service>/<method>`) is a service of a lent module.
    fn serves(&self, path: &str) -> bool {
        let service = path
            .trim_start_matches('/')
            .rsplit_once('/')
            .map_or("", |(service, _)| service);
        self.manifests()
            .iter()
            .any(|manifest| manifest.services.iter().any(|s| s == service))
    }
}

/// Each exchange to what it is for: a runner's call (the gateway's
/// `x-pcx-use: runner`) to the lent modules' services only, refused on any
/// other path, the daemon's memory included; any other exchange to the
/// space's routes (`space`), where the tool modules refuse it (their
/// policy is local only, and the space holds every call to a reach).
fn dispatch<S>(
    space: S,
    runner: Runner,
) -> impl Service<
    Request<RequestBody>,
    Response = Response<axum::body::Body>,
    Error = Infallible,
    Future = impl Send,
> + Clone
+ Send
+ use<S>
where
    S: Service<Request<RequestBody>, Response = Response<axum::body::Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
{
    tower::service_fn(move |request: Request<RequestBody>| {
        let (space, runner) = (space.clone(), runner.clone());
        async move {
            let for_runner = request
                .headers()
                .get(pcx_tunnel::USE_HEADER)
                .is_some_and(|v| v.as_bytes() == pcx_tunnel::USE_RUNNER.as_bytes());
            if !for_runner {
                return space.oneshot(request).await;
            }
            if !runner.serves(request.uri().path()) {
                let refused = tonic::Status::permission_denied(format!(
                    "{} is no service of a module this runner lends",
                    request.uri().path()
                ));
                return Ok(refused.into_http::<axum::body::Body>());
            }
            // The reach a tenant's turn carries holds its memory, not the
            // modules lent here, which serve the runner's calls by design.
            let mut request = request.map(axum::body::Body::new);
            request.headers_mut().remove(metadata::REACH);
            runner.call.oneshot(request).await
        }
    })
}

/// `routes` with every request's reach the workspaces the space offers
/// (`offered`, from the gateway's `Welcome`): set here, whatever the request
/// carried, so memory refuses what the organisation's people may not reach
/// whoever forwarded the call.
fn reached(
    routes: Router,
    offered: &[String],
) -> MapRequest<Router, impl Fn(Request<RequestBody>) -> Request<RequestBody> + Clone + use<>> {
    let joined = if offered.is_empty() {
        metadata::NO_WORKSPACE.to_string()
    } else {
        offered.join(",")
    };
    let reach = HeaderValue::from_str(&joined).unwrap_or_else(|e| {
        error!(error = %e, "the offered workspaces are no header value: the space reaches none");
        HeaderValue::from_static(metadata::NO_WORKSPACE)
    });
    MapRequest::new(routes, move |mut request: Request<RequestBody>| {
        request.headers_mut().insert(metadata::REACH, reach.clone());
        request
    })
}

/// What the daemon offers at each connection: its space and its
/// workspaces now, of which the organisation's owners choose; a runner of
/// the modules it lends, when it lends any that run.
async fn hello(memory: Arc<Memory>, runner: Runner) -> Hello {
    let (lent, roots) = (runner.manifests(), runner.roots());
    let mut offers = vec![Offer::Space as i32];
    if !lent.is_empty() {
        offers.push(Offer::Runner as i32);
    }
    Hello {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        offers,
        lent,
        roots,
        workspaces: memory
            .workspaces
            .list(false)
            .iter()
            .map(|w| Workspace {
                id: w.id.to_string(),
                name: w.name.clone(),
            })
            .collect(),
    }
}
