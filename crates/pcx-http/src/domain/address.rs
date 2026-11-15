// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The addresses a request may reach (docs/modules/http.md, the network
//! rules): every address a host resolves to is checked before one is
//! used; private, loopback, link-local (the clouds' metadata among them),
//! carrier NAT, documentation, benchmark and reserved ranges only with
//! private networks; unspecified and multicast addresses never. An IPv6
//! address that carries an IPv4 one (mapped, NAT64, 6to4) is judged as
//! that one.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::Host;

use crate::domain::Broken;

/// Whether `ip` may be reached; `private`: with private networks.
#[must_use]
pub fn allowed(ip: IpAddr, private: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_allowed(v4, private),
        IpAddr::V6(v6) => v6_allowed(v6, private),
    }
}

fn v4_allowed(ip: Ipv4Addr, private: bool) -> bool {
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
        return false;
    }
    private || public_v4(ip)
}

/// Whether `ip` is an address of the public internet.
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..128).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (18..20).contains(&b))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn v6_allowed(ip: Ipv6Addr, private: bool) -> bool {
    if ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    if let Some(v4) = carried(ip) {
        return v4_allowed(v4, private);
    }
    private || public_v6(ip)
}

/// The IPv4 address `ip` carries: mapped (`::ffff:a.b.c.d`), NAT64
/// (`64:ff9b::a.b.c.d`) or 6to4 (`2002:aabb:ccdd::`).
fn carried(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let o = ip.octets();
    let v4 = |at: usize| Ipv4Addr::new(o[at], o[at + 1], o[at + 2], o[at + 3]);
    match ip.segments() {
        [0, 0, 0, 0, 0, 0xffff, _, _] | [0x64, 0xff9b, 0, 0, 0, 0, _, _] => Some(v4(12)),
        [0x2002, ..] => Some(v4(2)),
        _ => None,
    }
}

/// Whether `ip` is an address of the public internet (it carries no IPv4
/// one).
fn public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    !(ip.is_loopback()
        // The old IPv4-compatible form, ::a.b.c.d.
        || s[..6] == [0; 6]
        // Local NAT64, 64:ff9b:1::/48.
        || (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1)
        // Discard, 100::/64.
        || s[..4] == [0x100, 0, 0, 0]
        // Teredo (2001::/32) and documentation (2001:db8::/32).
        || (s[0] == 0x2001 && (s[1] == 0 || s[1] == 0xdb8))
        // Unique local, fc00::/7.
        || (s[0] & 0xfe00) == 0xfc00
        // Link-local fe80::/10, site-local fec0::/10.
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xffc0) == 0xfec0)
}

/// Whether `host` is a loopback host: `localhost`, or a loopback address.
#[must_use]
pub fn loopback_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// `addresses`, what `host` resolved to, when every one may be reached.
///
/// # Errors
/// `PermissionDenied` when one may not, or there is none.
pub fn checked(
    host: &str,
    addresses: Vec<SocketAddr>,
    private: bool,
) -> Result<Vec<SocketAddr>, Broken> {
    if addresses.is_empty() {
        return Err(Broken::denied(
            "url",
            format!("{host} resolves to no address"),
        ));
    }
    if let Some(refused) = addresses.iter().find(|a| !allowed(a.ip(), private)) {
        return Err(Broken::denied(
            "url",
            format!(
                "{host} resolves to {}, which is not reached from here (private, loopback, link-local or reserved)",
                refused.ip()
            ),
        ));
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests;
