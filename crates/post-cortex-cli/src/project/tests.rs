// Copyright (c) 2026 Julius ML
// MIT License

use super::*;

const S: &str = "11111111-2222-3333-4444-555555555555";
const W: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

fn vectors() -> Option<Value> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../integrations/testdata/project-keys.json");
    // Absent when building from the crates.io package.
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

#[test]
fn remote_keys_match_the_shared_vectors() {
    let Some(vectors) = vectors() else { return };
    let remotes = vectors["remotes"].as_array().unwrap();
    assert!(remotes.len() > 10);
    for case in remotes {
        let url = case["url"].as_str().unwrap();
        let key = normalize_remote(url);
        assert_eq!(key.as_deref(), case["key"].as_str(), "{url:?}");
        if let Some(key) = key {
            assert_eq!(key.rsplit('/').next(), case["name"].as_str(), "{url:?}");
        }
    }
}

#[test]
fn local_keys_match_the_shared_vectors() {
    let Some(vectors) = vectors() else { return };
    for case in vectors["local"].as_array().unwrap() {
        assert_eq!(
            local_key(case["root"].as_str().unwrap()),
            case["key"].as_str().unwrap()
        );
    }
}

fn git(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn project_from_remote_git_root_or_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    if !git(&repo, &["init", "-q"]) {
        return; // no git here
    }
    let root = std::fs::canonicalize(&repo).unwrap();
    assert_eq!(
        project(&repo.join("src")),
        Project {
            key: local_key(&root.to_string_lossy()),
            name: "repo".into(),
            root: root.clone(),
        }
    );
    assert!(git(
        &repo,
        &["remote", "add", "origin", "git@GitHub.com:a/b.git"]
    ));
    assert_eq!(
        project(&repo.join("src")),
        Project {
            key: "github.com/a/b".into(),
            name: "b".into(),
            root,
        }
    );
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let real = std::fs::canonicalize(&plain).unwrap();
    assert_eq!(project(&plain).key, local_key(&real.to_string_lossy()));
    assert_eq!(project(&plain).name, "plain");
}

#[test]
fn pinned_ids_from_the_file_then_instructions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert_eq!(pinned_ids(root, &["AGENTS.md"]), None);
    std::fs::write(
        root.join("AGENTS.md"),
        format!("| Session ID | `{S}` |\n|---|---|\n| **Workspace ID** | `{W}` |\n"),
    )
    .unwrap();
    let ids = Ids {
        session_id: S.into(),
        workspace_id: W.into(),
    };
    assert_eq!(pinned_ids(root, &["AGENTS.md"]), Some(ids.clone()));
    assert_eq!(pinned_ids(root, &["CLAUDE.md"]), None);
    std::fs::write(
        root.join("CLAUDE.md"),
        "| Session ID | `<SESSION_ID>` |\n| **Workspace ID** | `<WORKSPACE_ID>` |\n",
    )
    .unwrap();
    assert_eq!(pinned_ids(root, &["CLAUDE.md"]), None);

    let fixed = Ids {
        session_id: "s".into(),
        workspace_id: "w".into(),
    };
    std::fs::write(root.join(PINNED_FILE), r#"{"note":"mine"}"#).unwrap();
    write_pinned(root, &fixed).unwrap();
    assert_eq!(pinned_ids(root, &["AGENTS.md"]), Some(fixed));
    assert_eq!(read_json(&root.join(PINNED_FILE))["note"], "mine");
}

#[test]
fn cache_counts_only_for_the_same_backend() {
    let tmp = tempfile::tempdir().unwrap();
    let ids = Ids {
        session_id: S.into(),
        workspace_id: W.into(),
    };
    assert_eq!(cached_ids(tmp.path(), "k", "http://a"), None);
    cache_ids(tmp.path(), "k", "http://a/", &ids);
    cache_ids(tmp.path(), "other", "http://b", &ids);
    assert_eq!(cached_ids(tmp.path(), "k", "http://a"), Some(ids));
    assert_eq!(cached_ids(tmp.path(), "k", "http://b"), None);
    let v = read_json(&tmp.path().join(CACHE_FILE));
    assert_eq!(
        v["k"],
        json!({"workspace_id": W, "session_id": S, "url": "http://a"})
    );
}

/// A refused resolution reports the server's message, not only the status.
#[tokio::test]
async fn refused_resolution_quotes_the_server() {
    use axum::http::StatusCode;
    const WHY: &str = "this key is limited to workspace(s) acme; creating a workspace for a \
                       new project needs an organisation-wide key";
    let app = axum::Router::new()
        .route(
            "/api/projects/resolve",
            axum::routing::post(|| async { (StatusCode::FORBIDDEN, format!("{WHY}\n")) }),
        )
        .route(
            "/empty/api/projects/resolve",
            axum::routing::post(|| async { StatusCode::FORBIDDEN }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

    let tmp = tempfile::tempdir().unwrap();
    let client = reqwest::Client::new();
    let project = Project {
        key: "github.com/a/b".into(),
        name: "b".into(),
        root: tmp.path().to_path_buf(),
    };
    fn resolver<'a>(client: &'a reqwest::Client, url: &'a str, dir: &'a Path) -> Resolver<'a> {
        Resolver {
            client,
            url,
            author: "test",
            workspace: None,
            config_dir: dir,
            instruction_files: &[],
        }
    }
    let err = resolver(&client, &base, tmp.path())
        .resolve(&project)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!("/api/projects/resolve: HTTP 403 Forbidden: {WHY}")
    );
    let empty = format!("{base}/empty");
    let err = resolver(&client, &empty, tmp.path())
        .resolve(&project)
        .await
        .unwrap_err();
    assert_eq!(err, "/api/projects/resolve: HTTP 403 Forbidden");
}
