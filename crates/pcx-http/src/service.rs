// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The http service (docs/modules/http.md): a call planned under the
//! settings, held to the daily limit and the requests in flight, its host
//! resolved and every address checked, sent, its redirects followed and
//! checked again, its response judged, and the call logged whatever its
//! end; the connections and their test; the presets (`presets`); the log
//! read a page at a time and kept for 30 days (`log`). Between the
//! handlers (and the module's daily pruning) and the ports.

use std::net::SocketAddr;
use std::time::Duration;

use pcx_documents::{Documents, Typed, days};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::http::{
    Call, ConnectionInfo, ConnectionList, HttpRequest, HttpResponse, Outcome, Pair, Preset, Tested,
};
use prost_types::Timestamp;
use tokio::sync::{Mutex, Semaphore};
use url::{Host, Url};

use crate::domain::request::{self, Planned};
use crate::domain::{self, Auth, Broken, Settings, address, answer, redirect};
use crate::ports::{Client, Clock, Received, Resolver};

mod log;
mod presets;

/// Bytes of a response's body at most (docs/modules/http.md, sizes).
pub const MAX_RESPONSE: usize = 2 << 20;

/// The requests of the day, counted in the module: read from the log the
/// first time a day is counted.
#[derive(Debug, Default)]
pub struct Daily {
    day: String,
    count: u32,
}

/// What the module holds between calls: the day's count and the requests
/// in flight.
pub struct Gates {
    /// The day's requests.
    pub daily: Mutex<Daily>,
    /// A permit for each request in flight.
    pub flight: Semaphore,
}

impl Default for Gates {
    fn default() -> Self {
        Self {
            daily: Mutex::new(Daily::default()),
            flight: Semaphore::new(domain::IN_FLIGHT),
        }
    }
}

/// `rule` as the failure of a call.
fn broken(rule: Broken) -> ModuleError {
    let field = rule.field.clone();
    ModuleError::new(rule.kind, rule.to_string()).with_field(field)
}

/// `took` as a proto duration.
///
/// # Errors
/// `Internal`: it is beyond what a proto duration holds.
fn duration(took: Duration) -> Result<prost_types::Duration, ModuleError> {
    prost_types::Duration::try_from(took).map_err(|e| {
        ModuleError::new(
            ModuleErrorKind::Internal,
            format!("a call took {took:?}: {e}"),
        )
    })
}

/// The http module's calls, connections, presets and log in its
/// workspace.
pub struct Service<'a> {
    docs: &'a dyn Documents,
    clock: &'a dyn Clock,
    resolver: &'a dyn Resolver,
    client: &'a dyn Client,
    settings: &'a Settings,
    built_in: &'a [Preset],
    gates: &'a Gates,
}

/// The ports and the state a service works with.
pub struct Parts<'a> {
    /// Its documents.
    pub docs: &'a dyn Documents,
    /// The time.
    pub clock: &'a dyn Clock,
    /// The hosts' addresses.
    pub resolver: &'a dyn Resolver,
    /// The client.
    pub client: &'a dyn Client,
    /// The settings.
    pub settings: &'a Settings,
    /// The built-in presets.
    pub built_in: &'a [Preset],
    /// The day's count and the requests in flight.
    pub gates: &'a Gates,
}

impl<'a> Service<'a> {
    /// The service over its parts.
    #[must_use]
    pub fn new(parts: Parts<'a>) -> Self {
        Self {
            docs: parts.docs,
            clock: parts.clock,
            resolver: parts.resolver,
            client: parts.client,
            settings: parts.settings,
            built_in: parts.built_in,
            gates: parts.gates,
        }
    }

    fn log(&self) -> Typed<'a> {
        Typed::new(self.docs, domain::call_kind())
    }

    /// Call `request` for `author`; `reads`: only a GET or a HEAD
    /// (`http_get`); `key`: the call's idempotency key. Answers the
    /// response; logs the call whatever its end.
    ///
    /// # Errors
    /// `InvalidArgument` or `PermissionDenied` for a request the rules
    /// refuse, `ResourceExhausted` past the daily limit (refused, nothing
    /// sent); the client's failure; a status outside `ok` as its kind
    /// (`answer::failure_kind`); `FailedPrecondition` for a body that does
    /// not fit `expect` or too many redirects.
    pub async fn call(
        &self,
        request: HttpRequest,
        reads: bool,
        author: Option<&str>,
        key: Option<&str>,
    ) -> Result<HttpResponse, ModuleError> {
        let at = self.clock.now();
        let mut entry = Call {
            title: asked(&request),
            at: Some(Timestamp {
                seconds: at.timestamp(),
                nanos: 0,
            }),
            day: days::day(at),
            author: author.unwrap_or_default().to_string(),
            connection: request.connection.clone(),
            method: request.method.to_ascii_uppercase(),
            idempotency_key: key.unwrap_or_default().to_string(),
            ..Call::default()
        };
        let planned = match request::plan(&request, self.settings, reads, key) {
            Ok(planned) => planned,
            Err(rule) => return Err(self.refused(entry, rule).await),
        };
        describe(&mut entry, &planned.url);
        entry.sent = planned.body.as_ref().map_or(0, |b| b.bytes.len() as u64);
        if let Err(rule) = self.count(&entry.day).await? {
            return Err(self.refused(entry, rule).await);
        }
        let _permit = self.gates.flight.acquire().await.map_err(|_| {
            ModuleError::new(ModuleErrorKind::Unavailable, "the http module is stopping")
        })?;
        let (planned, received, redirects, took) = self.exchange(planned, &entry).await?;
        let took = duration(took)?;
        entry.status = u32::from(received.status);
        entry.received = received.body.len() as u64;
        entry.duration = Some(took);
        if !answer::answers(received.status, &planned.ok) {
            let kind = answer::failure_kind(received.status);
            let message = answer::failure_message(
                received.status,
                &received.reason,
                &received.body,
                &planned.secrets,
            );
            return Err(self.failed(entry, ModuleError::new(kind, message)).await);
        }
        let content_type = received
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let (body, json) = answer::read(&content_type, received.body);
        if let Some(expect) = &planned.expect
            && let Err(rule) = answer::fits(expect, json.as_ref())
        {
            return Err(self.failed(entry, broken(rule)).await);
        }
        entry.outcome = Outcome::Ok.into();
        if let Err(e) = self.log().create(&entry, "").await {
            tracing::warn!(error = %e, "an http call was answered but not logged");
        }
        let mut url = planned.url;
        url.set_query(None);
        Ok(HttpResponse {
            status: u32::from(received.status),
            headers: answer::shown_headers(received.headers)
                .into_iter()
                .map(|(name, value)| Pair { name, value })
                .collect(),
            body: Some(body),
            content_type,
            url: url.to_string(),
            redirects,
            duration: Some(took),
        })
    }

    /// Send `planned`, following its redirects: the request that was
    /// answered, its answer, the redirects followed and the time they all
    /// took. A failure is logged.
    async fn exchange(
        &self,
        mut planned: Planned,
        entry: &Call,
    ) -> Result<(Planned, Received, u32, Duration), ModuleError> {
        let mut redirects = 0;
        let mut took = Duration::ZERO;
        loop {
            let addresses = match self.addresses(&planned.url).await {
                Ok(Ok(addresses)) => addresses,
                Ok(Err(rule)) => return Err(self.refused(entry.clone(), rule).await),
                Err(failed) => return Err(self.failed(entry.clone(), failed).await),
            };
            let received = match self.client.send(&planned, &addresses, MAX_RESPONSE).await {
                Ok(received) => received,
                Err(failed) => {
                    let message = answer::redact(&failed.message, &planned.secrets);
                    let failed = ModuleError::new(failed.kind, message);
                    return Err(self.failed(entry.clone(), failed).await);
                }
            };
            took += received.took;
            let location = received
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                .map(|(_, value)| value.clone());
            let (true, Some(location)) = (redirect::redirects(received.status), location) else {
                return Ok((planned, received, redirects, took));
            };
            if redirects == redirect::MAX_REDIRECTS {
                let failed = ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    format!("more than {} redirects", redirect::MAX_REDIRECTS),
                );
                return Err(self.failed(entry.clone(), failed).await);
            }
            planned = match redirect::follow(&planned, received.status, &location, self.settings) {
                Ok(next) => next,
                Err(rule) => return Err(self.refused(entry.clone(), rule).await),
            };
            redirects += 1;
        }
    }

    /// The addresses of `url`'s host, each checked; the rule they break.
    async fn addresses(&self, url: &Url) -> Result<Result<Vec<SocketAddr>, Broken>, ModuleError> {
        let Some(port) = url.port_or_known_default() else {
            return Ok(Err(Broken::new("url", "has no port")));
        };
        let (name, found) = match url.host() {
            Some(Host::Domain(name)) => {
                (name.to_string(), self.resolver.resolve(name, port).await?)
            }
            Some(Host::Ipv4(ip)) => (ip.to_string(), vec![SocketAddr::new(ip.into(), port)]),
            Some(Host::Ipv6(ip)) => (ip.to_string(), vec![SocketAddr::new(ip.into(), port)]),
            None => return Ok(Err(Broken::new("url", "has no host"))),
        };
        Ok(address::checked(
            &name,
            found,
            self.settings.private_networks,
        ))
    }

    /// Take a place of the day's requests: the rule the limit breaks when
    /// they are all taken.
    async fn count(&self, day: &str) -> Result<Result<(), Broken>, ModuleError> {
        let mut daily = self.gates.daily.lock().await;
        if daily.day != day {
            daily.count = self.counted(day).await?;
            daily.day = day.to_string();
        }
        if daily.count >= self.settings.daily_limit {
            return Ok(Err(Broken {
                kind: ModuleErrorKind::ResourceExhausted,
                field: "daily_limit".to_string(),
                reason: format!(
                    "allows {} requests a day, which are made",
                    self.settings.daily_limit
                ),
            }));
        }
        daily.count += 1;
        Ok(Ok(()))
    }

    /// Log `entry` as refused by `rule`: the failure.
    async fn refused(&self, mut entry: Call, rule: Broken) -> ModuleError {
        entry.outcome = Outcome::Refused.into();
        entry.failure = rule.kind.name().to_string();
        self.logged(entry, broken(rule)).await
    }

    /// Log `entry` as failed with `failure`: the failure.
    async fn failed(&self, mut entry: Call, failure: ModuleError) -> ModuleError {
        entry.outcome = Outcome::Failed.into();
        entry.failure = failure.kind.name().to_string();
        self.logged(entry, failure).await
    }

    /// Log `entry`, and answer `failure` naming its entry.
    async fn logged(&self, entry: Call, failure: ModuleError) -> ModuleError {
        match self.log().create(&entry, "").await {
            Ok(logged) => ModuleError {
                message: format!("{} (log entry {})", failure.message, logged.document.id),
                ..failure
            },
            Err(e) => {
                tracing::warn!(error = %e, "a failed http call was not logged");
                failure
            }
        }
    }

    /// The connections, without their secrets.
    #[must_use]
    pub fn connections(&self) -> ConnectionList {
        ConnectionList {
            connections: self
                .settings
                .connections
                .iter()
                .map(|c| ConnectionInfo {
                    name: c.name.clone(),
                    description: c.description.clone(),
                    base_url: c.base.to_string(),
                    auth: auth_kind(&c.auth).into(),
                    secret_set: c.secret().is_some(),
                })
                .collect(),
        }
    }

    /// A GET of connection `name`'s test path for `author`: its status,
    /// whatever it is, and the time it took.
    ///
    /// # Errors
    /// As [`Service::call`], but a status never fails.
    pub async fn test(&self, name: &str, author: Option<&str>) -> Result<Tested, ModuleError> {
        let c = self.settings.connection(name).map_err(broken)?;
        let request = HttpRequest {
            connection: c.name.clone(),
            method: "GET".to_string(),
            path: c.test_path.clone(),
            ok: (100..600).collect(),
            ..HttpRequest::default()
        };
        let answered = self.call(request, true, author, None).await?;
        Ok(Tested {
            status: answered.status,
            duration: answered.duration,
        })
    }
}

/// The title of a call refused before its address is known: its method and
/// the host or connection it names, never a path or query (they may carry
/// secrets), and what is missing said as missing; memory refuses a document
/// without a title, so an empty method and address were not logged.
fn asked(request: &HttpRequest) -> String {
    let method = match request.method.trim() {
        "" => "(no method)".to_string(),
        m => m.to_ascii_uppercase(),
    };
    let to = Url::parse(&request.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .or_else(|| (!request.connection.is_empty()).then(|| request.connection.clone()))
        .unwrap_or_else(|| "(no address)".to_string());
    format!("{method} {to}")
}

/// The method, host and path of `url` in `entry`.
fn describe(entry: &mut Call, url: &Url) {
    if let Some(host) = url.host_str() {
        entry.host = host.to_string();
    }
    entry.path = url.path().to_string();
    entry.title = format!("{} {}", entry.method, entry.host);
}

/// The wire's kind of `auth`.
fn auth_kind(auth: &Auth) -> post_cortex_proto::http::Auth {
    use post_cortex_proto::http::Auth as Kind;
    match auth {
        Auth::None => Kind::Unspecified,
        Auth::Bearer(_) => Kind::Bearer,
        Auth::Basic { .. } => Kind::Basic,
        Auth::Header { .. } => Kind::Header,
        Auth::Query { .. } => Kind::Query,
    }
}

#[cfg(test)]
mod tests;
