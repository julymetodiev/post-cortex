// Copyright (c) 2025 Julius ML
// Permission is hereby granted, free of charge, to any person obtaining a copy

use super::*;
use pcx_host::Grant;
use std::env;

/// pcx commands take the model and dimension from here: it must be the
/// model the daemon embeds with (the 0.5.0 `pcx export` used 384 against
/// a 256-dim database).
#[test]
fn storage_config_matches_the_daemon_model() {
    let config = DaemonConfig::default();
    assert!(matches!(
        config.storage_config(),
        Ok(crate::compose::StorageConfig::RocksDb { data_directory })
            if data_directory == config.data_directory
    ));
    let model = config.model_config().embedding_model;
    assert_eq!(
        model,
        post_cortex_embeddings::EmbeddingModelType::PotionMultilingual
    );
    assert_eq!(config.vector_dimension(), 256);
    assert_eq!(config.embedding_model().dimension, 256);
}

/// A misspelt backend is refused, never run as RocksDB.
#[test]
fn an_unknown_storage_backend_is_an_error() {
    let config = DaemonConfig {
        storage_backend: "rocksbd".to_string(),
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("rocksbd"), "{err}");
    assert!(config.storage_config().is_err());
    #[cfg(not(feature = "surrealdb-storage"))]
    {
        let surreal = DaemonConfig {
            storage_backend: "surrealdb".to_string(),
            ..Default::default()
        };
        assert!(surreal.storage_config().is_err());
    }
}

#[test]
fn test_default_config() {
    let config = DaemonConfig::default();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 3737);
    assert!(config.data_directory.ends_with(".post-cortex/data"));
}

#[test]
fn test_config_validation() {
    let config = DaemonConfig::default();
    assert!(config.validate().is_ok());

    let invalid = DaemonConfig {
        host: "".to_string(),
        ..Default::default()
    };
    assert!(invalid.validate().is_err());

    let invalid_port = DaemonConfig {
        port: 0,
        ..Default::default()
    };
    assert!(invalid_port.validate().is_err());
}

#[test]
fn test_env_override() {
    // SAFETY: This test runs in isolation and only modifies env vars
    // for the duration of the test. No other tests depend on these env vars.
    #[allow(unsafe_code)]
    unsafe {
        env::set_var("PC_HOST", "0.0.0.0");
        env::set_var("PC_PORT", "8080");
        env::set_var("PC_DATA_DIR", "/tmp/test-data");
    }

    // No file: the overrides over the defaults, whatever this machine's
    // own daemon.toml says.
    let dir = tempfile::tempdir().unwrap();
    let config = DaemonConfig::load_from(&dir.path().join("daemon.toml")).unwrap();

    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 8080);
    assert_eq!(config.data_directory, PathBuf::from("/tmp/test-data"));

    // SAFETY: Cleanup of env vars set above
    #[allow(unsafe_code)]
    unsafe {
        env::remove_var("PC_HOST");
        env::remove_var("PC_PORT");
        env::remove_var("PC_DATA_DIR");
    }
}

#[test]
fn test_host_list_parsing() {
    assert_eq!(
        parse_host_list(" a.example , b.example:3737,,"),
        vec!["a.example", "b.example:3737"]
    );
    assert!(parse_host_list("").is_empty());
}

#[test]
fn test_allowed_hosts_toml() {
    let parsed: DaemonConfig = toml::from_str(
        "host = \"0.0.0.0\"\nport = 3737\ndata_directory = \"/d\"\n\
         allowed_hosts = [\"my-mac.tailnet.ts.net\", \"100.64.0.7:3737\"]\n",
    )
    .unwrap();
    assert_eq!(
        parsed.allowed_hosts,
        vec!["my-mac.tailnet.ts.net", "100.64.0.7:3737"]
    );
    assert!(parsed.validate().is_ok());

    let invalid = DaemonConfig {
        allowed_hosts: vec!["http://x".into()],
        ..Default::default()
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_partial_toml_keeps_defaults() {
    let parsed: DaemonConfig =
        toml::from_str("host = \"0.0.0.0\"\nallowed_hosts = [\"100.64.0.7\"]\n").unwrap();
    let defaults = DaemonConfig::default();
    assert_eq!(parsed.host, "0.0.0.0");
    assert_eq!(parsed.allowed_hosts, vec!["100.64.0.7"]);
    assert_eq!(parsed.port, defaults.port);
    assert_eq!(parsed.grpc_port, defaults.grpc_port);
    assert_eq!(parsed.data_directory, defaults.data_directory);
    assert_eq!(parsed.storage_backend, defaults.storage_backend);
    assert_eq!(parsed.redact, defaults.redact);
}

#[test]
fn test_toml_serialization() {
    let config = DaemonConfig::default();
    let toml_str = toml::to_string(&config).unwrap();
    let parsed: DaemonConfig = toml::from_str(&toml_str).unwrap();

    assert_eq!(config.host, parsed.host);
    assert_eq!(config.port, parsed.port);
    assert_eq!(config.data_directory, parsed.data_directory);
}

#[test]
fn builtins_are_named_with_their_grants_and_an_unknown_one_is_refused() {
    let parsed: DaemonConfig = toml::from_str(
        r#"
[[builtins]]
id = "tools-git"
grants = ["filesystem", "spawn"]
filesystem = [{ path = "/work/repo", write = true }]
config = { author_name = "Agent", author_email = "agent@pcx.is" }
"#,
    )
    .unwrap();
    let git = &parsed.builtins[0];
    assert_eq!(git.id, crate::compose::Builtin::ToolsGit);
    assert_eq!(git.grants, [Grant::Filesystem, Grant::Spawn]);
    assert!(git.filesystem[0].write);
    assert!(toml::from_str::<DaemonConfig>("[[builtins]]\nid = \"tools-net\"\n").is_err());
}

/// `[blobs]` names an S3 endpoint; its keys come only from the
/// environment, and a table without them is refused.
#[test]
fn blobs_name_an_endpoint_and_take_their_keys_from_the_environment() {
    let mut config: DaemonConfig = toml::from_str(
        "[blobs]\nendpoint = \"https://s3.example:9443\"\nbucket = \"pcx\"\nregion = \"us-east-1\"\n",
    )
    .unwrap();
    let blobs = config.blobs.as_mut().unwrap();
    assert!(blobs.s3().is_err(), "no keys read yet");
    let missing = blobs.read_keys(|name| Ok((name == "PCX_BLOBS_ACCESS_KEY").then(|| "id".into())));
    assert!(matches!(missing, Err(ConfigError::Env(m)) if m.contains("PCX_BLOBS_SECRET_KEY")));
    blobs
        .read_keys(|name| Ok(Some(format!("{name}-value"))))
        .unwrap();
    let s3 = blobs.s3().unwrap();
    assert_eq!(
        (
            s3.endpoint.as_str(),
            s3.bucket.as_str(),
            s3.access_key.as_str()
        ),
        (
            "https://s3.example:9443",
            "pcx",
            "PCX_BLOBS_ACCESS_KEY-value"
        )
    );
    assert!(
        !format!("{blobs:?}").contains("value"),
        "the keys are not shown"
    );
    assert!(
        toml::from_str::<DaemonConfig>(
            "[blobs]\nendpoint = \"x\"\nbucket = \"pcx\"\nregion = \"r\"\naccess_key = \"k\"\n"
        )
        .is_err(),
        "keys are not a daemon.toml key"
    );
}

/// `[runner]`: its denied names by default, its modules only tool modules
/// the daemon runs, its names globs of one component.
#[test]
fn a_runner_lends_tool_modules_it_runs_and_denies_credentials() {
    let config = |text: &str| toml::from_str::<DaemonConfig>(text).unwrap();
    let lent = config(
        "[[builtins]]\nid = \"tools-git\"\ngrants = [\"spawn\"]\n\n[runner]\nmodules = [\"tools-git\"]\n",
    );
    assert_eq!(lent.validate(), Ok(()));
    let runner = lent.runner.unwrap();
    assert!(runner.deny.iter().any(|name| name == ".env"));
    assert!(runner.deny.iter().any(|name| name == ".ssh"));

    let not_run = config("[runner]\nmodules = [\"tools-git\"]\n");
    assert!(
        not_run
            .validate()
            .unwrap_err()
            .contains("no [[builtins]] entry")
    );
    let not_a_tool = config("[[builtins]]\nid = \"board\"\n\n[runner]\nmodules = [\"board\"]\n");
    assert!(
        not_a_tool
            .validate()
            .unwrap_err()
            .contains("no tool module")
    );
    let a_path = config("[runner]\nmodules = []\ndeny = [\"secrets/.env\"]\n");
    assert!(a_path.validate().unwrap_err().contains("names a path"));
}
