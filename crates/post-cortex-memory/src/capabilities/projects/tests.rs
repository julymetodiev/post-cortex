// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::capabilities::Memory;
use crate::memory_system::backends_tests::backends;

async fn resolve(
    memory: &Memory,
    key: &str,
    name: &str,
    workspace: Option<&str>,
) -> Result<ResolveProjectResponse, ProjectError> {
    memory
        .projects
        .resolve(ResolveProjectRequest {
            key: key.to_string(),
            name: name.to_string(),
            workspace: workspace.map(str::to_string),
            client: None,
        })
        .await
}

const KEY: &str = "github.com/julymetodiev/post-cortex";

/// Same key, same workspace and session; both are named after the
/// project and survive a restart.
#[tokio::test]
async fn resolve_is_idempotent_and_survives_a_restart() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let first = resolve(&memory, KEY, "post-cortex", None).await.unwrap();
        assert!(first.created, "{}", backend.name);
        let again = resolve(&memory, KEY, "post-cortex", None).await.unwrap();
        assert_eq!(
            (again.workspace_id, again.session_id, again.created),
            (first.workspace_id, first.session_id, false),
            "{}",
            backend.name
        );
        let ws = memory.workspaces.get(first.workspace_id).unwrap();
        assert_eq!(ws.name, "post-cortex");
        assert_eq!(
            ws.get_session_role(&first.session_id),
            Some(SessionRole::Primary)
        );
        let session = memory.sessions.get(first.session_id).await.unwrap();
        assert_eq!(session.name().as_deref(), Some("post-cortex"));
        drop(memory);

        let restarted = backend.memory().await;
        let after = resolve(&restarted, KEY, "post-cortex", None).await.unwrap();
        assert_eq!(
            (after.workspace_id, after.session_id, after.created),
            (first.workspace_id, first.session_id, false),
            "{}",
            backend.name
        );
        assert_eq!(restarted.workspaces.count(), 1);
    }
}

/// A new key with a workspace name joins that workspace; a name that
/// does not exist yet is created.
#[tokio::test]
async fn new_key_attaches_to_the_named_workspace() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let acme = resolve(&memory, "github.com/acme/api", "api", Some("acme"))
            .await
            .unwrap();
        assert!(acme.created);
        assert_eq!(
            memory.workspaces.get(acme.workspace_id).unwrap().name,
            "acme"
        );
        let web = resolve(&memory, "github.com/acme/web", "web", Some("acme"))
            .await
            .unwrap();
        assert!(web.created, "{}", backend.name);
        assert_eq!(web.workspace_id, acme.workspace_id, "{}", backend.name);
        assert_ne!(web.session_id, acme.session_id);
        let ws = memory.workspaces.get(acme.workspace_id).unwrap();
        assert_eq!(ws.projects().len(), 2);
        assert_eq!(ws.session_count(), 2);
        drop(memory);

        // Both bindings were persisted.
        let restarted = backend.memory().await;
        for (key, expected) in [
            ("github.com/acme/api", &acme),
            ("github.com/acme/web", &web),
        ] {
            let r = resolve(&restarted, key, "x", None).await.unwrap();
            assert_eq!(
                (r.workspace_id, r.session_id, r.created),
                (expected.workspace_id, expected.session_id, false),
                "{} {key}",
                backend.name
            );
        }
    }
}

/// Clients resolving the same new key at once end with one workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_resolves_of_a_new_key_make_one_workspace() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let memory = memory.clone();
                tokio::spawn(async move { resolve(&memory, KEY, "post-cortex", None).await })
            })
            .collect();
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap().unwrap());
        }
        assert_eq!(results.iter().filter(|r| r.created).count(), 1);
        assert!(
            results.iter().all(|r| (r.workspace_id, r.session_id)
                == (results[0].workspace_id, results[0].session_id)),
            "{}",
            backend.name
        );
        assert_eq!(memory.workspaces.count(), 1);
        assert!(memory.projects.rt.project_locks.is_empty());
    }
}

/// A deleted project session is replaced in the same workspace, and the
/// replacement is what a restart finds.
#[tokio::test]
async fn deleted_session_is_replaced() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let first = resolve(&memory, KEY, "post-cortex", None).await.unwrap();
        assert!(memory.sessions.delete(first.session_id).await.unwrap());
        let second = resolve(&memory, KEY, "post-cortex", None).await.unwrap();
        assert_eq!(second.workspace_id, first.workspace_id);
        assert_ne!(second.session_id, first.session_id, "{}", backend.name);
        assert!(!second.created);
        drop(memory);

        let restarted = backend.memory().await;
        let after = resolve(&restarted, KEY, "post-cortex", None).await.unwrap();
        assert_eq!(
            (after.workspace_id, after.session_id),
            (first.workspace_id, second.session_id),
            "{}",
            backend.name
        );
    }
}

/// find_project reports where a key would go and creates nothing.
#[tokio::test]
async fn find_project_is_read_only() {
    for backend in backends().await {
        let memory = backend.memory().await;
        assert_eq!(
            memory.projects.find(KEY, None, None),
            ProjectPlacement::NewWorkspace
        );
        assert_eq!(
            memory.projects.find(KEY, Some("acme"), None),
            ProjectPlacement::NewWorkspace
        );
        assert_eq!(memory.workspaces.count(), 0);

        let acme = resolve(&memory, "github.com/acme/api", "api", Some("acme"))
            .await
            .unwrap();
        assert_eq!(
            memory.projects.find(KEY, Some(" acme "), None),
            ProjectPlacement::Joins {
                workspace_id: acme.workspace_id
            },
            "{}",
            backend.name
        );
        assert_eq!(
            memory.projects.find(KEY, Some(" "), None),
            ProjectPlacement::NewWorkspace
        );
        // A bound key ignores the workspace name, as resolve_project does.
        assert_eq!(
            memory
                .projects
                .find("github.com/acme/api", Some("other"), None),
            ProjectPlacement::Bound {
                workspace_id: acme.workspace_id,
                session_id: Some(acme.session_id),
            }
        );
        assert_eq!(memory.workspaces.count(), 1);
        assert_eq!(
            memory.projects.find(KEY, None, None),
            ProjectPlacement::NewWorkspace
        );
        // acme is bound to a key: no new key named like it joins it.
        assert_eq!(
            memory.projects.find(KEY, None, Some("acme")),
            ProjectPlacement::NewWorkspace
        );
    }
}

/// An empty workspace named `name`, no project key bound to it (what the
/// web app's onboarding makes before any agent connects).
async fn unbound_workspace(memory: &Memory, name: &str) -> Uuid {
    memory
        .workspaces
        .create(CreateWorkspace {
            name: name.to_string(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
}

/// A new key that names no workspace joins the one workspace named like the
/// project that no key is bound to, instead of making a second one; once
/// bound, that workspace is no longer free for another key of the name.
#[tokio::test]
async fn new_key_joins_the_one_unbound_workspace_of_its_name() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let kite = unbound_workspace(&memory, "kite").await;
        let key = "git.example.net/team/kite";
        assert_eq!(
            memory.projects.find(key, None, Some(" kite ")),
            ProjectPlacement::Joins { workspace_id: kite },
            "{}",
            backend.name
        );
        let first = resolve(&memory, key, "kite", None).await.unwrap();
        assert!(first.created);
        assert_eq!(first.workspace_id, kite, "{}", backend.name);
        assert_eq!(memory.workspaces.count(), 1);
        let ws = memory.workspaces.get(kite).unwrap();
        assert_eq!(ws.projects().len(), 1);
        assert_eq!(
            ws.get_session_role(&first.session_id),
            Some(SessionRole::Primary)
        );

        // Another key of the same name finds the workspace taken.
        let other = resolve(&memory, "github.com/fork/kite", "kite", None)
            .await
            .unwrap();
        assert_ne!(other.workspace_id, kite, "{}", backend.name);
        assert_eq!(memory.workspaces.count(), 2);
        drop(memory);

        let restarted = backend.memory().await;
        let after = resolve(&restarted, key, "kite", None).await.unwrap();
        assert_eq!(
            (after.workspace_id, after.session_id, after.created),
            (kite, first.session_id, false),
            "{}",
            backend.name
        );
    }
}

/// Two unbound workspaces of the project's name are no guess: the key gets a
/// workspace of its own, as before.
#[tokio::test]
async fn two_unbound_workspaces_of_the_name_are_no_guess() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let a = unbound_workspace(&memory, "kite").await;
        let b = unbound_workspace(&memory, "kite").await;
        let key = "git.example.net/team/kite";
        assert_eq!(
            memory.projects.find(key, None, Some("kite")),
            ProjectPlacement::NewWorkspace
        );
        let r = resolve(&memory, key, "kite", None).await.unwrap();
        assert!(r.created);
        assert!(![a, b].contains(&r.workspace_id), "{}", backend.name);
        assert_eq!(memory.workspaces.count(), 3);
    }
}

/// A workspace another key is bound to is never taken over by a new key of
/// the same name; an unbound one of the name next to it still is.
#[tokio::test]
async fn a_bound_workspace_is_never_taken_over() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let bound = resolve(&memory, "github.com/a/kite", "kite", None)
            .await
            .unwrap();
        let other = resolve(&memory, "github.com/b/kite", "kite", None)
            .await
            .unwrap();
        assert_ne!(other.workspace_id, bound.workspace_id, "{}", backend.name);
        assert_eq!(
            memory
                .workspaces
                .get(bound.workspace_id)
                .unwrap()
                .projects()
                .len(),
            1
        );

        let free = unbound_workspace(&memory, "kite").await;
        let third = resolve(&memory, "github.com/c/kite", "kite", None)
            .await
            .unwrap();
        assert_eq!(third.workspace_id, free, "{}", backend.name);
        assert_eq!(memory.workspaces.count(), 3);
    }
}

#[test]
fn resolve_project_validation() {
    let req = |key: &str, name: &str, workspace: Option<&str>| ResolveProjectRequest {
        key: key.to_string(),
        name: name.to_string(),
        workspace: workspace.map(str::to_string),
        client: None,
    };
    let ok = req("github.com/a/b", "  b ", Some("  "));
    assert_eq!(validate(&ok).unwrap(), ("github.com/a/b", "b", None));
    let ok = req("local:0123456789abcdef", "b", Some(" acme "));
    assert_eq!(validate(&ok).unwrap().2, Some("acme"));
    for bad in [
        req("", "b", None),
        req(&"k".repeat(513), "b", None),
        req("github.com/a b", "b", None),
        req("github.com/a\tb", "b", None),
        req("a\u{7}", "b", None),
        req("k", "   ", None),
        req("k", &"n".repeat(201), None),
        req("k", "b", Some(&"w".repeat(201))),
    ] {
        assert!(
            matches!(validate(&bad), Err(ProjectError::InvalidArgument(_))),
            "{bad:?}"
        );
    }
    assert!(validate(&req(&"k".repeat(512), &"n".repeat(200), None)).is_ok());
}
