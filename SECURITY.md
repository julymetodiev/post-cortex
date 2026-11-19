# Security policy

## Supported versions

Active development happens on `main`. Security fixes go into the latest release only:
the `pcx` binaries of the [latest GitHub release](https://github.com/julymetodiev/post-cortex/releases/latest),
the Homebrew formula `julymetodiev/tap/post-cortex` and the hosted service at
[pcx.is](https://pcx.is). The crates on crates.io stop at 0.5.0 and get no fixes; build
from a release tag instead.

| Version | Security fixes |
|---------|----------------|
| The latest release | Yes |
| Every earlier release | No, upgrade |

## Reporting a vulnerability

Please **do not** open public GitHub issues for security vulnerabilities. Report them privately through GitHub: [Security → Report a vulnerability](https://github.com/julymetodiev/post-cortex/security/advisories/new). Expect:

- Acknowledgement within 72 hours.
- A coordinated-disclosure window of up to 90 days for the fix to land in a tagged release before public disclosure.
- A CVE / RustSec advisory on disclosure, with credit to the reporter unless they ask to remain anonymous.

## Known transitive findings

`cargo audit` and `cargo deny` run on every push to `main` and every pull request and fail on any advisory not listed in
[.cargo/audit.toml](.cargo/audit.toml) and [deny.toml](deny.toml), which give each
listed one its reason. Every listed advisory is in a dependency of a dependency, and no
release of the chain that pulls it in fixes it yet:

- `rsa 0.9` (RUSTSEC-2023-0071, Marvin timing attack). No fixed release of `rsa`. It
  comes through `jsonwebtoken` of `surrealdb-core 4.0.1` and is used only for JWT
  signing inside SurrealDB's auth layer, which pcx does not expose.
- `crypto-bigint 0.5` (RUSTSEC-2026-0387, timing). Fixed only in 0.7; `p256` and `p384`
  of `jsonwebtoken 10.4` (of `surrealdb 4.0.1`) require 0.5. The same JWT path as `rsa`.
- `heapless 0.8.0` (RUSTSEC-2026-0574 to RUSTSEC-2026-0582, published 2026-11-18). Fixed
  only in 0.9.4. It comes through `rstar 0.12`, which `geo 0.32` of `surrealdb 4.0.1`
  requires, and no `geo` release moves to `rstar 0.14`, the first on `heapless 0.9`.
  `rstar` uses only `heapless`'s `BinaryHeap`, which none of the advisories names. The
  copies 0.7.17 and 0.6.1 are in `Cargo.lock` as optional dependencies of `geo-types`
  and are not compiled. Accepted until an `rstar 0.12` release on `heapless 0.9`.
- Unmaintained crates, which are notices, not vulnerabilities: `bincode 2.0.1` (3.0.0 is
  only a `compile_error!`; replacing it migrates stored data), `chrono`, `chrono-tz` and
  `humantime` (the time type of the workspace and of `surrealdb`, `cel-core` and
  `object_store`), `number_prefix` (through `tokenizers`), `paste` (through
  `candle-core`) and `atomic-polyfill` (only on targets pcx is not built for).

An entry leaves the lists as soon as a release of its chain fixes it.

## Threat model

post-cortex is designed as a local-first conversation memory system. The expected deployment is:

- The daemon binds to `127.0.0.1` by default — no external network exposure.
- All persisted data lives under the user's home directory.
- Embedding compute and HNSW search are local; no model weights or queries leave the machine.

- `/mcp` and `/api/*` reject requests whose `Host` header is not on the allowlist (`localhost`, `127.0.0.1`, `::1`, a non-wildcard bind host, plus `allowed_hosts` / `PC_ALLOWED_HOSTS`) with `403`, so web pages cannot reach the daemon via DNS rebinding.

Operators who change `PC_HOST` to a non-loopback interface must set `auth_token` (bearer auth on HTTP and gRPC), list the names clients use in `allowed_hosts`, and front the daemon with TLS termination — the daemon does not terminate TLS itself. See [docs/DAEMON_MODE.md](docs/DAEMON_MODE.md#security).

## Build-time integrity

- `cargo-deny` enforces a license allow-list (MIT, Apache-2.0, BSD, ISC, Unicode, CC0, Zlib, MPL-2.0) and bans `openssl-sys` in favour of `rustls`.
- `cargo-audit` and `cargo-deny`'s advisory check run on every push to `main` and every pull request via [CI](.github/workflows/ci.yml), and so does a check that every dependency is at its latest release.

## License

See [LICENSE](LICENSE) (MIT).
