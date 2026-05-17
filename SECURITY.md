# Security policy

## Supported versions

Active development happens on `main`. Released crates on crates.io carry semver — patches go to the latest minor of every published 0.x line; older 0.x lines are end-of-life.

| Crate | Latest | Security patches |
|-------|--------|------------------|
| `post-cortex*` (all 8 workspace members) | 0.2.x | Yes |
| `post-cortex` 0.1.x (legacy single-crate) | 0.1.23 | No — superseded by the 8-crate workspace |

## Reporting a vulnerability

Please **do not** open public GitHub issues for security vulnerabilities. Email `security@juliusbiascan.dev` (PGP key on the GitHub profile). Expect:

- Acknowledgement within 72 hours.
- A coordinated-disclosure window of up to 90 days for the fix to land in a tagged release before public disclosure.
- A CVE / RustSec advisory on disclosure, with credit to the reporter unless they ask to remain anonymous.

## Known transitive findings

`cargo audit` reports several findings in the transitive dependency graph, primarily via `surrealdb 3.0` → `tonic 0.10` → `rustls-webpki` / `aws-lc-rs` / `quinn-proto`. These are tracked at [docs/audit-baseline-0.1.23.md](docs/audit-baseline-0.1.23.md) and whitelisted in [deny.toml](deny.toml) with explicit rationales:

- `rsa 0.9.10` — Marvin timing attack (RUSTSEC-2023-0071). No upstream fix; affects only JWT signing inside SurrealDB's auth layer, which post-cortex does not expose externally. Documented accepted risk.
- The rest (aws-lc-sys, rustls-webpki, quinn-proto, time, atomic-polyfill, bincode unmaintained warning) are unblocked once `surrealdb` ships a 3.x update with refreshed transitive pins.

We re-evaluate the whitelist on every quarterly review cycle.

## Threat model

post-cortex is designed as a local-first conversation memory system. The expected deployment is:

- The daemon binds to `127.0.0.1` by default — no external network exposure.
- All persisted data lives under the user's home directory.
- Embedding compute and HNSW search are local; no model weights or queries leave the machine.

Operators who change `PC_HOST` to a non-loopback interface must front the daemon with TLS termination and authentication of their own choosing — the daemon performs no authentication on its REST / gRPC / MCP surfaces.

## Build-time integrity

- `cargo-deny` enforces a license allow-list (MIT, Apache-2.0, BSD, ISC, Unicode, CC0, Zlib, MPL-2.0) and bans `openssl-sys` in favour of `rustls`.
- `cargo-audit` runs on every push to `main` via [CI](.github/workflows/ci.yml).
- `cargo-vet` adoption is on the roadmap for the 0.3.x line.

## License

See [LICENSE](LICENSE) (MIT).
