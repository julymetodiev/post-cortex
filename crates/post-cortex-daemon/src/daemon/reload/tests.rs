use serde_json::json;

use super::*;

async fn reloader(dir: &tempfile::TempDir, toml: &str) -> (Reloader, PathBuf) {
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            dir,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let modules = Modules::memory(memory).unwrap();
    let host = Arc::new(crate::compose::host(&modules).await.unwrap());
    let path = dir.path().join("daemon.toml");
    std::fs::write(&path, toml).unwrap();
    (Reloader::new(host, modules, path.clone()), path)
}

/// A module whose executable is not there: it never loads.
const MISSING: &str = r#"# kept
[[modules]]
id = "greet"
command = "/nonexistent/greet"
sha256 = "00"
config = { greeting = "Hello" }
"#;

#[tokio::test]
async fn an_unchanged_file_reloads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (reloader, _) = reloader(&dir, "# no modules\n").await;
    assert!(!reloader.reload().await.unwrap());
}

#[tokio::test]
async fn a_change_whose_module_does_not_load_puts_the_file_back() {
    let dir = tempfile::tempdir().unwrap();
    let (reloader, path) = reloader(&dir, MISSING).await;
    let refused = reloader
        .apply("greet", json!({ "greeting": "Hi" }))
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            SettingsError::Reload(pcx_host::HostError::ExecutableUnreadable { .. })
        ),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), MISSING);
    let ids: Vec<String> = reloader
        .host
        .modules()
        .into_iter()
        .map(|m| m.manifest.id)
        .collect();
    assert_eq!(ids, ["post-cortex.memory"], "the running set stays");
}

#[tokio::test]
async fn a_module_not_in_the_file_is_not_configurable_here() {
    let dir = tempfile::tempdir().unwrap();
    let (reloader, path) = reloader(&dir, MISSING).await;
    let refused = reloader
        .apply("post-cortex.memory", json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(refused, SettingsError::NotConfigurable { .. }),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), MISSING);
}

#[tokio::test]
async fn a_builtin_is_turned_on_with_its_grants_and_a_root_that_is_no_directory_refused() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let (reloader, path) = reloader(&dir, "# no modules\n").await;
    assert!(reloader.available().contains(&"tools-git".to_string()));

    let root = |path: PathBuf| pcx_host::RootGrant { path, write: true };
    let missing = reloader
        .grant(
            "tools-git",
            ModuleGrants {
                roots: vec![root(repo.path().join("nope"))],
                network: false,
                spawn: true,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(missing, SettingsError::Root { .. }), "{missing:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# no modules\n");

    reloader
        .grant(
            "tools-git",
            ModuleGrants {
                roots: vec![root(repo.path().to_path_buf())],
                network: false,
                spawn: true,
            },
        )
        .await
        .unwrap();
    let running = reloader.host.module("tools-git").expect("it runs");
    assert_eq!(running.sandbox.roots.len(), 1);
    assert_eq!(running.sandbox.roots[0].path, repo.path());
    assert!(running.sandbox.spawn && !running.sandbox.network);
    assert!(!reloader.available().contains(&"tools-git".to_string()));
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.starts_with("[[builtins]]\nid = \"tools-git\"\n")
            && written.contains("# no modules\n"),
        "{written}"
    );

    let unknown = reloader
        .grant(
            "greet",
            ModuleGrants {
                roots: Vec::new(),
                network: true,
                spawn: false,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(unknown, SettingsError::Unknown { .. }),
        "{unknown:?}"
    );
}
