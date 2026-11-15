//! Pairing a daemon with an organisation (`docs/connect.md` section 1): the
//! device flow `pcx connect` runs, and `connect.json`, which keeps its
//! result beside `daemon.toml`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use post_cortex_proto::tunnel::pairing_client::PairingClient;
use post_cortex_proto::tunnel::{
    PairingState, PairingStatus, PollPairingRequest, StartPairingRequest,
};
use serde::{Deserialize, Serialize};

use crate::TunnelError;
use crate::daemon::Connection;
use crate::endpoint::channel;

/// The file of a paired daemon, beside `daemon.toml`.
pub const FILE: &str = "connect.json";

/// A daemon paired with an organisation: what `connect.json` keeps.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paired {
    /// The cloud's URL.
    pub url: String,
    /// The daemon's id in the cloud.
    pub daemon_id: String,
    /// Its organisation's id.
    pub org_id: String,
    /// Its organisation's name.
    pub org_name: String,
    /// Its token (`pcxd_...`).
    pub token: String,
}

impl std::fmt::Debug for Paired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Paired")
            .field("url", &self.url)
            .field("daemon_id", &self.daemon_id)
            .field("org_id", &self.org_id)
            .field("org_name", &self.org_name)
            .finish_non_exhaustive()
    }
}

/// How pairing, or its file, fails.
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    /// `connect.json` could not be read or written.
    #[error("{path}: {source}")]
    File {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// `connect.json` is not a pairing.
    #[error("{path} is no pairing: {source}")]
    Invalid {
        /// The file.
        path: PathBuf,
        /// Why.
        source: serde_json::Error,
    },
    /// A person refused the pairing.
    #[error("the pairing was refused")]
    Refused,
    /// Nobody approved the pairing in time.
    #[error("the pairing expired: run `pcx connect` again")]
    Expired,
    /// The cloud could not be reached, or answered what the protocol does
    /// not allow.
    #[error(transparent)]
    Tunnel(#[from] TunnelError),
}

impl From<tonic::Status> for PairingError {
    fn from(status: tonic::Status) -> Self {
        Self::Tunnel(status.into())
    }
}

impl Paired {
    /// Where and as which daemon its tunnel connects.
    #[must_use]
    pub fn connection(&self) -> Connection {
        Connection {
            url: self.url.clone(),
            token: self.token.clone(),
        }
    }

    /// The pairing kept in `dir`; `None` when the daemon is not paired.
    ///
    /// # Errors
    /// The file cannot be read or is no pairing.
    pub fn load(dir: &Path) -> Result<Option<Self>, PairingError> {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(PairingError::File { path, source }),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|source| PairingError::Invalid { path, source })
    }

    /// Keep the pairing in `dir`, readable by its owner only.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn save(&self, dir: &Path) -> Result<(), PairingError> {
        let path = dir.join(FILE);
        let partial = dir.join(format!("{FILE}.partial"));
        let file_error = |source| PairingError::File {
            path: path.clone(),
            source,
        };
        let text = serde_json::to_vec_pretty(self).map_err(|source| PairingError::Invalid {
            path: path.clone(),
            source,
        })?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&partial).map_err(file_error)?;
        file.write_all(&text).map_err(file_error)?;
        file.sync_all().map_err(file_error)?;
        std::fs::rename(&partial, &path).map_err(file_error)
    }

    /// Forget the pairing kept in `dir`.
    ///
    /// # Errors
    /// The file exists and cannot be removed.
    pub fn remove(dir: &Path) -> Result<(), PairingError> {
        let path = dir.join(FILE);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(PairingError::File { path, source }),
        }
    }
}

/// What a person does to approve a pairing, shown by `pcx connect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    /// The short code they check in the app.
    pub user_code: String,
    /// The page where they approve.
    pub verification_url: String,
    /// How long they have.
    pub expires_in: Duration,
}

/// Pair with an organisation of the cloud at `url`: ask for a pairing,
/// `show` the person what to approve, and wait for their answer.
///
/// # Errors
/// [`PairingError::Refused`] or [`PairingError::Expired`] as the person (or
/// nobody) answered; [`PairingError::Tunnel`] when the cloud cannot be
/// reached or answers outside the protocol.
pub async fn pair(
    url: &str,
    request: StartPairingRequest,
    show: impl FnOnce(&Approval),
) -> Result<Paired, PairingError> {
    let mut client = PairingClient::new(channel(url).await?);
    let started = client.start_pairing(request).await?.into_inner();
    show(&Approval {
        user_code: started.user_code,
        verification_url: started.verification_url,
        expires_in: Duration::from_secs(started.expires_in_secs.into()),
    });
    let interval = Duration::from_secs(started.interval_secs.max(1).into());
    loop {
        tokio::time::sleep(interval).await;
        let state = client
            .poll_pairing(PollPairingRequest {
                device_code: started.device_code.clone(),
            })
            .await?
            .into_inner();
        if let Some(paired) = answered(url, state)? {
            return Ok(paired);
        }
    }
}

/// The pairing once `state` says it is approved; `None` while it waits.
fn answered(url: &str, state: PairingState) -> Result<Option<Paired>, PairingError> {
    match PairingStatus::try_from(state.status) {
        Ok(PairingStatus::Pending) => Ok(None),
        Ok(PairingStatus::Approved) => Ok(Some(Paired {
            url: url.to_owned(),
            daemon_id: state.daemon_id,
            org_id: state.org_id,
            org_name: state.org_name,
            token: state.token,
        })),
        Ok(PairingStatus::Refused) => Err(PairingError::Refused),
        Ok(PairingStatus::Expired) => Err(PairingError::Expired),
        Ok(PairingStatus::Unspecified) | Err(_) => {
            Err(TunnelError::Protocol(format!("a pairing status {}", state.status)).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paired() -> Paired {
        Paired {
            url: "https://api.example".into(),
            daemon_id: "d1".into(),
            org_id: "o1".into(),
            org_name: "Equal".into(),
            token: "pcxd_secret".into(),
        }
    }

    #[test]
    fn a_pairing_is_kept_for_its_owner_only_and_read_back() {
        let dir = std::env::temp_dir().join(format!("pcx-tunnel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(Paired::load(&dir).unwrap(), None);
        paired().save(&dir).unwrap();
        assert_eq!(Paired::load(&dir).unwrap(), Some(paired()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        Paired::remove(&dir).unwrap();
        assert_eq!(Paired::load(&dir).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_token_is_never_printed() {
        assert!(!format!("{:?}", paired()).contains("secret"));
    }

    #[test]
    fn a_status_outside_the_protocol_fails() {
        let state = PairingState {
            status: 9,
            ..PairingState::default()
        };
        assert!(matches!(
            answered("u", state),
            Err(PairingError::Tunnel(TunnelError::Protocol(_)))
        ));
        let approved = PairingState {
            status: PairingStatus::Approved as i32,
            daemon_id: "d1".into(),
            token: "pcxd_x".into(),
            ..PairingState::default()
        };
        assert_eq!(answered("u", approved).unwrap().unwrap().daemon_id, "d1");
    }
}
