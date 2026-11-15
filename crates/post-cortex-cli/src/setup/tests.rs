// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;

#[test]
fn daemon_url_must_be_a_plain_http_url() {
    assert_eq!(
        validate_daemon_url("http://ml.example.ts.net:3737/").unwrap(),
        "http://ml.example.ts.net:3737"
    );
    assert!(validate_daemon_url("https://[::1]:3737").is_ok());
    assert!(validate_daemon_url("ml:3737").is_err());
    assert!(validate_daemon_url("http://x; rm -rf ~").is_err());
    assert!(validate_daemon_url("http://x$(id)").is_err());
}
