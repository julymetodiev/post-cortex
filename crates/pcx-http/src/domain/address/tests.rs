// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn public_addresses_are_reached() {
    for public in [
        "8.8.8.8",
        "140.82.112.3",
        "2606:4700::6810:84e5",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
    ] {
        assert!(allowed(ip(public), false), "{public}");
    }
}

#[test]
fn private_and_reserved_addresses_are_not_reached_but_with_private_networks() {
    for private in [
        "10.1.2.3",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "100.64.0.1",
        "0.1.2.3",
        "192.0.2.1",
        "198.18.0.1",
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "64:ff9b::a00:1",
        "2002:a00:1::",
        "2001:db8::1",
        "::127.0.0.1",
    ] {
        assert!(!allowed(ip(private), false), "{private}");
        assert!(
            allowed(ip(private), true),
            "{private} with private networks"
        );
    }
}

#[test]
fn unspecified_and_multicast_addresses_are_never_reached() {
    for never in ["0.0.0.0", "255.255.255.255", "224.0.0.1", "::", "ff02::1"] {
        assert!(!allowed(ip(never), true), "{never}");
    }
}

#[test]
fn a_host_is_refused_when_one_of_its_addresses_is() {
    let at = |text: &str| SocketAddr::new(ip(text), 443);
    assert!(checked("x", vec![at("8.8.8.8")], false).is_ok());
    let refused = checked("x", vec![at("8.8.8.8"), at("10.0.0.1")], false).unwrap_err();
    assert_eq!(refused.kind, pcx_module_api::ModuleErrorKind::Denied);
    assert!(checked("x", Vec::new(), false).is_err());
}
