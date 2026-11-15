// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! HTTP calls of a module with credentials read from files through its
//! `Sandbox` (ARCHITECTURE section 8, operations, W30, W31): a bearer
//! token's file and a cluster's CA, read at each call (a rotated token is
//! used at once; a file outside the module's grant is `Denied`), never a
//! setting, a prompt or an answer. `observe` reads with them, `ops`
//! changes with them. A cluster's certificate may be verified against a
//! name other than its URL's host, as a kubeconfig's `tls-server-name`.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pcx_module_api::sandbox::{Sandbox, SandboxPath};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};

pub use reqwest::Method;

/// The longest a call waits for its answer.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Most bytes of a credential's file.
const FILE_LIMIT: usize = 64 * 1024;

/// Most bytes of an answer read.
const ANSWER_LIMIT: usize = 16 * 1024 * 1024;

/// Characters of a Kubernetes name.
const NAME_CHARS: usize = 253;

/// Whether `name` is a Kubernetes name (a DNS subdomain: lowercase letters,
/// digits, '-' and '.', not starting with either): what may go into an
/// API's path.
#[must_use]
pub fn kube_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && !name.starts_with(['-', '.'])
}

/// Whether a server's certificate may be verified against `name` with the
/// CA in `ca_file`: an empty name (the URL's host), or a DNS name or an IP
/// address with a CA file.
///
/// # Errors
/// Why it may not.
pub fn tls_server_name(name: &str, ca_file: &str) -> Result<(), &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    if ServerName::try_from(name).is_err() {
        return Err("is no DNS name or IP address");
    }
    if ca_file.trim().is_empty() {
        return Err("needs the cluster's CA file");
    }
    Ok(())
}

fn unavailable(what: impl std::fmt::Display) -> ModuleError {
    ModuleError::new(ModuleErrorKind::Unavailable, what.to_string())
}

/// An API's answer: whether its status was a success, and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Whether the status was a success.
    pub ok: bool,
    /// The HTTP status.
    pub status: u16,
    /// The body.
    pub body: Vec<u8>,
}

/// A body sent: its media type and bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// Its media type, e.g. `application/merge-patch+json`.
    pub media_type: &'static str,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// Where an API's credentials are: files beneath the module's grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credentials<'a> {
    /// The bearer token's file; empty: none.
    pub token_file: &'a str,
    /// The CA certificate's file (PEM); empty: the system's.
    pub ca_file: &'a str,
    /// The name the server's certificate is verified against, with the CA
    /// file's certificates only; empty: the URL's host.
    pub tls_server_name: &'a str,
}

/// WebPKI's verification of a server's certificate against `name` instead
/// of the URL's host.
#[derive(Debug)]
struct NamedVerifier {
    webpki: Arc<WebPkiServerVerifier>,
    name: ServerName<'static>,
}

impl ServerCertVerifier for NamedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _url_host: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.webpki
            .verify_server_cert(end_entity, intermediates, &self.name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.webpki.supported_verify_schemes()
    }
}

/// TLS that trusts only the certificates in `pem` (read from `ca_file`)
/// and verifies the server's against `name`.
fn named_tls(pem: &[u8], ca_file: &str, name: &str) -> Result<rustls::ClientConfig, ModuleError> {
    let name = ServerName::try_from(name.to_string())
        .map_err(|e| unavailable(format!("{name} is no TLS server name: {e}")))?;
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(pem) {
        let cert =
            cert.map_err(|e| unavailable(format!("{ca_file} is no PEM certificate: {e}")))?;
        roots
            .add(cert)
            .map_err(|e| unavailable(format!("{ca_file} holds no CA: {e}")))?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let webpki =
        WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
            .build()
            .map_err(|e| unavailable(format!("{ca_file} holds no CA: {e}")))?;
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(unavailable)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NamedVerifier { webpki, name }))
        .with_no_client_auth())
}

/// The HTTP clients of one module's calls, built once per TLS setting (its
/// CA file and server name) and reused: building one loads the TLS roots
/// and the system's proxies (seconds under load), and a client keeps its
/// connections for the next call. A CA file whose content changed gets a
/// new client.
#[derive(Default)]
pub struct Clients {
    built: parking_lot::Mutex<HashMap<Setting, Built>>,
}

/// A TLS setting: its CA file and server name.
type Setting = (String, String);

/// A client and the CA content it was built with.
type Built = (Vec<u8>, reqwest::Client);

impl Clients {
    /// The client for CA `ca_file` (`pem` its content now; empty: the
    /// built-in roots) and TLS server name `name`, built if this setting
    /// has none yet.
    fn client(
        &self,
        ca_file: &str,
        name: &str,
        pem: &[u8],
    ) -> Result<reqwest::Client, ModuleError> {
        let key = (ca_file.to_string(), name.to_string());
        if let Some((seen, client)) = self.built.lock().get(&key)
            && seen.as_slice() == pem
        {
            return Ok(client.clone());
        }
        let client = build(ca_file, name, pem)?;
        self.built
            .lock()
            .insert(key, (pem.to_vec(), client.clone()));
        Ok(client)
    }
}

/// A client for CA `ca_file` holding `pem` (empty: the built-in roots),
/// verified against `name` when it is set.
fn build(ca_file: &str, name: &str, pem: &[u8]) -> Result<reqwest::Client, ModuleError> {
    let mut client = reqwest::Client::builder().timeout(TIMEOUT);
    if !name.is_empty() {
        client = client.use_preconfigured_tls(named_tls(pem, ca_file, name)?);
    } else if !ca_file.is_empty() {
        let ca = reqwest::Certificate::from_pem(pem)
            .map_err(|e| unavailable(format!("{ca_file} is no PEM certificate: {e}")))?;
        client = client.add_root_certificate(ca);
    }
    client.build().map_err(unavailable)
}

/// Calls of one module, its credentials read through `sandbox`, over its
/// `clients`.
pub struct Caller<'a> {
    sandbox: &'a dyn Sandbox,
    clients: &'a Clients,
}

impl<'a> Caller<'a> {
    /// The calls of the module whose sandbox is `sandbox`, over its
    /// `clients`.
    pub fn new(sandbox: &'a dyn Sandbox, clients: &'a Clients) -> Self {
        Self { sandbox, clients }
    }

    async fn file(&self, path: &str) -> Result<Vec<u8>, ModuleError> {
        self.sandbox.read(&SandboxPath::new(path), FILE_LIMIT).await
    }

    /// `method` `url` with `query` and `body`, its credentials read now:
    /// the answer, whatever its status.
    ///
    /// # Errors
    /// A credential's file that cannot be read (`Denied` outside the
    /// grant), a CA that is no PEM certificate, a TLS server name that is
    /// none or has no CA file, or an API that cannot be reached or answers
    /// too much.
    pub async fn call(
        &self,
        credentials: Credentials<'_>,
        method: Method,
        url: &str,
        query: &[(&str, String)],
        body: Option<Body>,
    ) -> Result<Answer, ModuleError> {
        let name = credentials.tls_server_name.trim();
        let ca_file = credentials.ca_file.trim();
        if !name.is_empty() {
            tls_server_name(name, credentials.ca_file)
                .map_err(|reason| unavailable(format!("TLS server name {name} {reason}")))?;
        }
        let pem = if ca_file.is_empty() {
            Vec::new()
        } else {
            self.file(credentials.ca_file).await?
        };
        let client = self.clients.client(ca_file, name, &pem)?;
        let mut request = client.request(method, url).query(query);
        if !credentials.token_file.trim().is_empty() {
            let token = self.file(credentials.token_file).await?;
            request = request.bearer_auth(String::from_utf8_lossy(&token).trim());
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, body.media_type)
                .body(body.bytes);
        }
        let response = request
            .send()
            .await
            .map_err(|e| unavailable(format!("{url} could not be reached: {e}")))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| unavailable(format!("{url}'s answer did not come: {e}")))?;
        if body.len() > ANSWER_LIMIT {
            return Err(unavailable(format!(
                "{url} answered {} bytes, more than {ANSWER_LIMIT}",
                body.len()
            )));
        }
        Ok(Answer {
            ok: status.is_success(),
            status: status.as_u16(),
            body: body.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests;
