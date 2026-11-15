// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn a_tls_server_name_is_a_dns_name_or_an_ip_with_a_ca() {
    assert_eq!(tls_server_name("", ""), Ok(()));
    assert_eq!(tls_server_name("10.254.0.101", "/creds/ca.crt"), Ok(()));
    assert_eq!(tls_server_name("eq-cp-fsn1-otx", "/creds/ca.crt"), Ok(()));
    assert_eq!(
        tls_server_name("10.254.0.101", " "),
        Err("needs the cluster's CA file")
    );
    assert_eq!(
        tls_server_name("not a name", "/creds/ca.crt"),
        Err("is no DNS name or IP address")
    );
}

#[test]
fn a_named_tls_needs_a_ca_in_its_file() {
    for pem in [
        &b""[..],
        b"-----BEGIN CERTIFICATE-----\nnot base64\n-----END CERTIFICATE-----\n",
    ] {
        assert!(named_tls(pem, "/creds/ca.crt", "10.254.0.101").is_err());
    }
}
