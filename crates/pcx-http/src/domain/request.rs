// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A request planned under the settings (docs/modules/http.md, requests):
//! its method, its URL (a path under a connection's base, its places
//! filled one segment each, or an absolute URL to an allowed host), its
//! query and headers (the authentication and the host only the module's),
//! its body, time limit, the statuses that answer, what its body must fit.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use pcx_expr::schema::{self, Field};
use pcx_module_api::json;
use post_cortex_proto::http::HttpRequest;
use post_cortex_proto::http::http_request::Body as Given;
use url::{Origin, Url};

use crate::domain::{Auth, Broken, Connection, DEFAULT_TIMEOUT, MAX_TIMEOUT, Settings, secure};

pub mod headers;
pub mod path;

use headers::header;
use path::expand;

/// Bytes of a request's body at most.
pub const MAX_BODY: usize = 1 << 20;

/// A method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET.
    Get,
    /// HEAD.
    Head,
    /// POST.
    Post,
    /// PUT.
    Put,
    /// PATCH.
    Patch,
    /// DELETE.
    Delete,
}

impl Method {
    /// `text` as a method, in any case.
    ///
    /// # Errors
    /// It is none of the six.
    pub fn parse(text: &str) -> Result<Self, Broken> {
        Ok(match text.to_ascii_uppercase().as_str() {
            "GET" => Self::Get,
            "HEAD" => Self::Head,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "PATCH" => Self::Patch,
            "DELETE" => Self::Delete,
            _ => {
                return Err(Broken::new(
                    "method",
                    format!("{text:?} is none of GET, HEAD, POST, PUT, PATCH, DELETE"),
                ));
            }
        })
    }

    /// Its name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    /// Whether it only reads (GET, HEAD): a `Get`.
    #[must_use]
    pub fn reads(self) -> bool {
        matches!(self, Self::Get | Self::Head)
    }
}

/// A body to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// Its bytes.
    pub bytes: Vec<u8>,
    /// Its media type.
    pub content_type: String,
}

/// What a JSON body must fit.
#[derive(Debug, Clone, PartialEq)]
pub struct Expect {
    /// The fields of each object.
    pub fields: Vec<Field>,
    /// Whether the body is a list of such objects.
    pub list: bool,
}

/// The credentials a request through a connection carries: the
/// connection's origin, which they go to only, and the headers and the
/// query parameter that hold them (none for a connection that does not
/// sign in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The connection's origin.
    pub origin: Origin,
    /// The headers they are in, names in lower case.
    pub headers: Vec<String>,
    /// The query parameter they are in.
    pub query: Option<String>,
}

/// A request, planned: what the client sends.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// Its method.
    pub method: Method,
    /// Its URL, its query included.
    pub url: Url,
    /// Its headers, names in lower case.
    pub headers: Vec<(String, String)>,
    /// Its body.
    pub body: Option<Body>,
    /// Its time limit.
    pub timeout: Duration,
    /// The statuses that answer; empty: 2xx.
    pub ok: Vec<u16>,
    /// What its JSON body must fit.
    pub expect: Option<Expect>,
    /// The connection, by name; none: none.
    pub connection: Option<String>,
    /// Its connection's credentials; none without a connection, or after
    /// a redirect to another origin.
    pub credentials: Option<Credentials>,
    /// The values a message must never show.
    pub secrets: Vec<String>,
    /// The workflow's idempotency key it carries.
    pub idempotency_key: Option<String>,
}

/// `request` planned under `settings`; `reads`: only GET or HEAD without a
/// body (`http_get`); `key`: the call's idempotency key.
///
/// # Errors
/// The first field at fault: `InvalidArgument`, or `PermissionDenied` for
/// a host `allow` does not name.
pub fn plan(
    request: &HttpRequest,
    settings: &Settings,
    reads: bool,
    key: Option<&str>,
) -> Result<Planned, Broken> {
    let method = Method::parse(&request.method)?;
    if reads && (!method.reads() || request.body.is_some()) {
        return Err(Broken::new(
            "method",
            "http_get takes a GET or a HEAD without a body; http_request takes the others",
        ));
    }
    let connection = match request.connection.as_str() {
        "" => None,
        name => Some(settings.connection(name)?),
    };
    let mut url = match connection {
        Some(c) => under(c, request)?,
        None => outside(request, settings)?,
    };
    for (i, pair) in request.query.iter().enumerate() {
        if pair.name.is_empty() {
            return Err(Broken::new(&format!("query[{i}].name"), "is required"));
        }
        url.query_pairs_mut().append_pair(&pair.name, &pair.value);
    }
    let mut headers: Vec<(String, String)> =
        connection.map(|c| c.headers.clone()).unwrap_or_default();
    for (i, pair) in request.headers.iter().enumerate() {
        let name = pair.name.trim().to_ascii_lowercase();
        let field = format!("headers[{i}]");
        header(&name, &pair.value).map_err(|why| Broken::new(&field, why))?;
        if connection.is_some_and(|c| sets(c, &name)) {
            return Err(Broken::new(
                &field,
                format!("{name} is set by the connection"),
            ));
        }
        headers.retain(|(n, _)| *n != name);
        headers.push((name, pair.value.clone()));
    }
    let mut credentials = None;
    let mut secrets = Vec::new();
    if let Some(c) = connection {
        credentials = Some(sign(c, &mut url, &mut headers, &mut secrets));
        if let (Some(name), Some(key)) = (&c.idempotency_header, key) {
            headers.push((name.clone(), key.to_string()));
        }
    }
    Ok(Planned {
        method,
        url,
        headers,
        body: body(request)?,
        timeout: timeout(request, connection)?,
        ok: ok(&request.ok)?,
        expect: expect(request)?,
        connection: connection.map(|c| c.name.clone()),
        credentials,
        secrets,
        idempotency_key: key.map(str::to_string),
    })
}

/// The URL of `request`'s path under connection `c`.
fn under(c: &Connection, request: &HttpRequest) -> Result<Url, Broken> {
    if !request.url.is_empty() {
        return Err(Broken::new(
            "url",
            "is not given with a connection: a path under its base URL is",
        ));
    }
    let path = expand(&request.path, &request.path_params)?;
    let base = c.base.path().trim_end_matches('/');
    let mut url = c.base.clone();
    url.set_path(&format!("{base}{path}"));
    if !url.path().starts_with(base) {
        return Err(Broken::new("path", "leaves the connection's base URL"));
    }
    Ok(url)
}

/// The URL of `request`, given whole, to a host `allow` names.
fn outside(request: &HttpRequest, settings: &Settings) -> Result<Url, Broken> {
    if !request.path.is_empty() || !request.path_params.is_empty() {
        return Err(Broken::new(
            "path",
            "is given with a connection; without one the url is",
        ));
    }
    if request.url.is_empty() {
        return Err(Broken::new("url", "is required without a connection"));
    }
    let url =
        Url::parse(&request.url).map_err(|e| Broken::new("url", format!("is no URL: {e}")))?;
    secure(&url, settings.private_networks).map_err(|why| Broken::new("url", why))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Broken::new("url", "carries no user or password"));
    }
    let Some(host) = url.host_str() else {
        return Err(Broken::new("url", "has no host"));
    };
    if !settings.allows(host) {
        return Err(Broken::denied(
            "url",
            format!("{host} is not among the hosts the settings allow without a connection"),
        ));
    }
    Ok(url)
}

/// Whether connection `c` sets header `name` (its authentication, its
/// idempotency key).
fn sets(c: &Connection, name: &str) -> bool {
    matches!(&c.auth, Auth::Header { name: n, .. } if n == name)
        || c.idempotency_header.as_deref() == Some(name)
}

/// Put connection `c`'s credentials into `url` or `headers`, its secrets
/// into `secrets`: where they went.
fn sign(
    c: &Connection,
    url: &mut Url,
    headers: &mut Vec<(String, String)>,
    secrets: &mut Vec<String>,
) -> Credentials {
    let mut held = Credentials {
        origin: c.base.origin(),
        headers: Vec::new(),
        query: None,
    };
    match &c.auth {
        Auth::None => {}
        Auth::Bearer(token) => {
            headers.push(("authorization".to_string(), format!("Bearer {token}")));
            held.headers.push("authorization".to_string());
            secrets.push(token.clone());
        }
        Auth::Basic { user, password } => {
            let encoded = STANDARD.encode(format!("{user}:{password}"));
            headers.push(("authorization".to_string(), format!("Basic {encoded}")));
            held.headers.push("authorization".to_string());
            secrets.extend([password.clone(), encoded]);
        }
        Auth::Header { name, value } => {
            headers.push((name.clone(), value.clone()));
            held.headers.push(name.clone());
            secrets.push(value.clone());
        }
        Auth::Query { name, value } => {
            url.query_pairs_mut().append_pair(name, value);
            held.query = Some(name.clone());
            secrets.push(value.clone());
        }
    }
    held
}

/// The body of `request`, at most [`MAX_BODY`].
fn body(request: &HttpRequest) -> Result<Option<Body>, Broken> {
    let media = |default: &str| match request.content_type.trim() {
        "" => Ok(default.to_string()),
        given if given.contains(['\r', '\n']) => Err(Broken::new("content_type", "is one line")),
        given => Ok(given.to_string()),
    };
    let body = match &request.body {
        None => return Ok(None),
        // By the proto3 JSON mapping, so a whole number goes as an integer:
        // written as the double it is held as (`1.0`), an API that reads an
        // integer refuses it.
        Some(Given::Json(value)) => Body {
            bytes: serde_json::to_vec(&json::from_wire(value.clone()))
                .map_err(|e| Broken::new("json", format!("does not encode: {e}")))?,
            content_type: media("application/json")?,
        },
        Some(Given::Text(text)) => Body {
            bytes: text.clone().into_bytes(),
            content_type: media("text/plain; charset=utf-8")?,
        },
        Some(Given::Raw(bytes)) => Body {
            bytes: bytes.clone(),
            content_type: media("application/octet-stream")?,
        },
    };
    if body.bytes.len() > MAX_BODY {
        return Err(Broken::new("body", format!("is at most {MAX_BODY} bytes")));
    }
    Ok(Some(body))
}

/// The time limit of `request`: its own, else its connection's, else 30 s.
fn timeout(request: &HttpRequest, c: Option<&Connection>) -> Result<Duration, Broken> {
    let Some(given) = &request.timeout else {
        return Ok(c.map_or(DEFAULT_TIMEOUT, |c| c.timeout));
    };
    let refused = || Broken::new("timeout", "is above 0 and at most 5 min");
    let (Ok(seconds), Ok(nanos)) = (u64::try_from(given.seconds), u32::try_from(given.nanos))
    else {
        return Err(refused());
    };
    if seconds > MAX_TIMEOUT.as_secs() || nanos >= 1_000_000_000 {
        return Err(refused());
    }
    let limit = Duration::new(seconds, nanos);
    if limit.is_zero() || limit > MAX_TIMEOUT {
        return Err(refused());
    }
    Ok(limit)
}

/// The statuses that answer.
fn ok(given: &[u32]) -> Result<Vec<u16>, Broken> {
    given
        .iter()
        .enumerate()
        .map(|(i, s)| {
            u16::try_from(*s)
                .ok()
                .filter(|s| (100..600).contains(s))
                .ok_or_else(|| Broken::new(&format!("ok[{i}]"), "is a status, 100 to 599"))
        })
        .collect()
}

/// What `request`'s JSON body must fit.
fn expect(request: &HttpRequest) -> Result<Option<Expect>, Broken> {
    let Some(given) = &request.expect else {
        if request.expect_list {
            return Err(Broken::new("expect_list", "is given with expect"));
        }
        return Ok(None);
    };
    let fields = schema::fields(
        given
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.r#type.as_str(), f.required)),
    )
    .map_err(|(i, part, why)| Broken::new(&format!("expect.fields[{i}].{part}"), why))?;
    Ok(Some(Expect {
        fields,
        list: request.expect_list,
    }))
}

#[cfg(test)]
mod tests;
