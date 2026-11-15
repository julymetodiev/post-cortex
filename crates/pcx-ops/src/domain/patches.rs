// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The merge patches ops sends Kubernetes: a restart's annotation, a
//! deployment's replicas, a volume claim's size. Pure.

use chrono::{DateTime, Utc};
use post_cortex_proto::ops::{
    ClaimPatch, ClaimResources, ClaimSpec, DeploymentPatch, PatchMeta, PatchSpec, PatchTemplate,
};

/// The annotation a restart sets, as `kubectl rollout restart` does.
const RESTARTED_AT: &str = "kubectl.kubernetes.io/restartedAt";

/// A restart's patch at `now`: the template's annotation, as `kubectl
/// rollout restart` sets it.
#[must_use]
pub fn restart(now: DateTime<Utc>) -> DeploymentPatch {
    DeploymentPatch {
        spec: Some(PatchSpec {
            template: Some(PatchTemplate {
                metadata: Some(PatchMeta {
                    annotations: [(
                        RESTARTED_AT.to_string(),
                        now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    )]
                    .into(),
                }),
            }),
            replicas: None,
        }),
    }
}

/// A scale's patch.
#[must_use]
pub fn scale(replicas: u32) -> DeploymentPatch {
    DeploymentPatch {
        spec: Some(PatchSpec {
            template: None,
            replicas: Some(replicas),
        }),
    }
}

/// A claim's patch to `gib` gibibytes.
#[must_use]
pub fn grow(gib: u32) -> ClaimPatch {
    ClaimPatch {
        spec: Some(ClaimSpec {
            resources: Some(ClaimResources {
                requests: [("storage".to_string(), format!("{gib}Gi"))].into(),
            }),
        }),
    }
}
