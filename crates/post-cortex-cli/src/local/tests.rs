// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// A key that cannot be a header fails the client instead of sending
/// requests without it (audit 2026-10-22).
#[test]
fn an_unsendable_key_is_an_error_not_a_client_without_auth() {
    assert!(matches!(
        bearer_client(Some("bad\nkey"), None, None),
        Err(ClientError::InvalidToken)
    ));
    assert!(matches!(
        bearer_client(None, Some("a\u{7f}"), None),
        Err(ClientError::InvalidAuthor(_))
    ));
    assert!(bearer_client(Some(" "), Some("me"), None).is_ok());
}

#[test]
fn the_local_url_reaches_a_wildcard_bind_on_loopback() {
    let local = |host: &str| Local {
        host: host.to_string(),
        port: 3737,
        grpc_port: 3738,
        auth_token: None,
        data_directory: PathBuf::new(),
        config_dir: PathBuf::new(),
        message_retention_days: 0,
    };
    assert_eq!(local("0.0.0.0").url(), "http://127.0.0.1:3737");
    assert_eq!(local("::1").url(), "http://[::1]:3737");
    assert_eq!(local("ml.ts.net").url(), "http://ml.ts.net:3737");
    assert_eq!(local("0.0.0.0").grpc_url(), "http://127.0.0.1:3738");
}
