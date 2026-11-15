// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Kubernetes API's paths of what ops reads and changes; each name a
//! Kubernetes name already (the domain checked it).

/// A deployment.
pub fn deployment(namespace: &str, name: &str) -> String {
    format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}")
}

/// A deployment's scale.
pub fn scale(namespace: &str, name: &str) -> String {
    format!("{}/scale", deployment(namespace, name))
}

/// A pod.
pub fn pod(namespace: &str, name: &str) -> String {
    format!("/api/v1/namespaces/{namespace}/pods/{name}")
}

/// A namespace's jobs.
pub fn jobs(namespace: &str) -> String {
    format!("/apis/batch/v1/namespaces/{namespace}/jobs")
}

/// A job.
pub fn job(namespace: &str, name: &str) -> String {
    format!("{}/{name}", jobs(namespace))
}

/// A volume claim.
pub fn claim(namespace: &str, name: &str) -> String {
    format!("/api/v1/namespaces/{namespace}/persistentvolumeclaims/{name}")
}
