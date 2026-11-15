// Copyright (c) 2025 Julius ML
// MIT License

//! HTTP client for the running pcx daemon's REST API + the wire types used
//! by `pcx workspace`, `pcx session`, and `pcx setup`.

use serde::{Deserialize, Serialize};

use crate::local::{Local, bearer_client, env_text};
use crate::stdio_proxy::is_daemon_running;

#[derive(Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub workspace: Option<String>,
}

#[derive(Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub session_count: usize,
}

#[derive(Serialize)]
struct CreateSessionRequest {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Serialize)]
struct CreateWorkspaceRequest {
    name: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct AttachSessionRequest {
    role: String,
}

pub struct DaemonClient {
    base_url: String,
    client: reqwest::Client,
}

impl DaemonClient {
    /// A client for the local daemon, which must be running: the commands
    /// that change memory never open storage themselves.
    ///
    /// # Errors
    /// The daemon is not running, or the client cannot be built.
    pub async fn connect(local: &Local) -> Result<Self, String> {
        if !is_daemon_running(&local.host, local.port).await {
            return Err(format!(
                "the daemon is not running at {}; start it with `pcx start --daemon`",
                local.url()
            ));
        }
        Ok(Self {
            base_url: local.url(),
            client: local.http_client().map_err(|e| e.to_string())?,
        })
    }

    /// A client for a backend profile: its URL and token (`PC_AUTHOR`
    /// names the writes, as for the local daemon).
    pub fn for_profile(profile: &crate::profiles::Profile) -> Result<Self, String> {
        let author = env_text("PC_AUTHOR")?;
        Ok(Self {
            base_url: profile.url.clone(),
            client: bearer_client(profile.token.as_deref(), author.as_deref(), None)
                .map_err(|e| e.to_string())?,
        })
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, String> {
        let resp = self
            .client
            .get(format!("{}/api/sessions", self.base_url))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json().await.map_err(|e| format!("JSON error: {}", e))
    }

    pub async fn create_session(
        &self,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<SessionInfo, String> {
        let resp = self
            .client
            .post(format!("{}/api/sessions", self.base_url))
            .json(&CreateSessionRequest { name, description })
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json().await.map_err(|e| format!("JSON error: {}", e))
    }

    pub async fn delete_session(&self, id: &str) -> Result<(), String> {
        let resp = self
            .client
            .delete(format!("{}/api/sessions/{}", self.base_url, id))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        Ok(())
    }

    pub async fn list_workspaces(&self) -> Result<Vec<WorkspaceInfo>, String> {
        let resp = self
            .client
            .get(format!("{}/api/workspaces", self.base_url))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json().await.map_err(|e| format!("JSON error: {}", e))
    }

    pub async fn create_workspace(
        &self,
        name: String,
        description: String,
    ) -> Result<WorkspaceInfo, String> {
        let resp = self
            .client
            .post(format!("{}/api/workspaces", self.base_url))
            .json(&CreateWorkspaceRequest {
                name,
                description: Some(description),
            })
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json().await.map_err(|e| format!("JSON error: {}", e))
    }

    pub async fn delete_workspace(&self, id: &str) -> Result<(), String> {
        let resp = self
            .client
            .delete(format!("{}/api/workspaces/{}", self.base_url, id))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        Ok(())
    }

    pub async fn attach_session(
        &self,
        workspace_id: &str,
        session_id: &str,
        role: &str,
    ) -> Result<(), String> {
        let resp = self
            .client
            .post(format!(
                "{}/api/workspaces/{}/sessions/{}",
                self.base_url, workspace_id, session_id
            ))
            .json(&AttachSessionRequest {
                role: role.to_string(),
            })
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        Ok(())
    }
}
