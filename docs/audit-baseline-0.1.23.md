# cargo-audit baseline — post-cortex 0.1.23

Captured via `cargo audit` (advisory DB 2026-05-17). All 15 vulnerabilities and 10 unmaintained warnings live in **transitive dependencies**, primarily through `surrealdb 3.0.0` → `surrealdb-tikv-client` → `tonic 0.10` and `surrealdb` → `reqwest 0.13` → `aws-lc-rs`/`rustls-webpki`. There are no direct findings against post-cortex code.

The full report is dumped to `docs/audit-baseline-0.1.23.txt` (gitignored — large, regenerable via `cargo audit > docs/audit-baseline-0.1.23.txt`).

## Vulnerabilities (15)

| Crate | Version | Advisory | Sev | Direct ancestor in our tree |
|---|---|---|---|---|
| aws-lc-sys | 0.37.1 | RUSTSEC-2026-0045 (AES-CCM timing) | medium 5.9 | surrealdb / hf-hub / reqwest |
| aws-lc-sys | 0.37.1 | RUSTSEC-2026-0044 (X.509 name constraints bypass) | — | surrealdb / hf-hub / reqwest |
| aws-lc-sys | 0.37.1 | RUSTSEC-2026-0048 (CRL scope check) | high 7.4 | surrealdb / hf-hub / reqwest |
| aws-lc-sys | 0.37.1 | RUSTSEC-2026-0047 (PKCS7_verify bypass A) | high 7.5 | surrealdb / hf-hub / reqwest |
| aws-lc-sys | 0.37.1 | RUSTSEC-2026-0046 (PKCS7_verify bypass B) | high 7.5 | surrealdb / hf-hub / reqwest |
| quinn-proto | 0.11.13 | RUSTSEC-2026-0037 (DoS on endpoints) | high 8.7 | reqwest → surrealdb |
| rsa | 0.9.10 | RUSTSEC-2023-0071 (Marvin attack) | medium 5.9 | jsonwebtoken → surrealdb-core (**no fix available upstream**) |
| rustls-webpki | 0.101.7 | RUSTSEC-2026-0104 (CRL parse panic) | — | rustls 0.21 → tonic 0.10 → surrealdb-tikv-client |
| rustls-webpki | 0.101.7 | RUSTSEC-2026-0098 (URI name constraints) | — | rustls 0.21 → tonic 0.10 → surrealdb-tikv-client |
| rustls-webpki | 0.101.7 | RUSTSEC-2026-0099 (wildcard name constraints) | — | rustls 0.21 → tonic 0.10 → surrealdb-tikv-client |
| rustls-webpki | 0.103.8 | RUSTSEC-2026-0104 | — | rustls 0.23 → reqwest 0.13 etc. |
| rustls-webpki | 0.103.8 | RUSTSEC-2026-0098 | — | rustls 0.23 → reqwest 0.13 etc. |
| rustls-webpki | 0.103.8 | RUSTSEC-2026-0099 | — | rustls 0.23 → reqwest 0.13 etc. |
| rustls-webpki | 0.103.8 | RUSTSEC-2026-0049 | — | rustls 0.23 → reqwest 0.13 etc. |
| time | 0.3.44 | RUSTSEC-2026-0009 | — | jiff/chrono adjacent |

### Unmaintained / informational (10)

- `atomic-polyfill 1.0.3` (RUSTSEC-2023-0089) — transitive
- `bincode 2.0.1` (RUSTSEC-2025-0141) — **direct dependency**; flagged unmaintained, no successor yet. Keep on the watch list; do not panic-replace.
- `number_prefix 0.4.0` (RUSTSEC-2025-0119) — transitive (indicatif/hf-hub)
- (7 more in `docs/audit-baseline-0.1.23.txt` once dumped)

## Remediation plan

- **Phase 9** (error typing) is the right phase to also bump `surrealdb` if a 3.x release with fresher transitive deps is available, and to bump direct deps to clear what's clearable.
- **Phase 13** (CI rewrite) lands the `cargo audit` job. Until then, the 15 findings are *known baseline*, not regressions. The `.cargo/audit.toml` config (Phase 13) will:
  - Hard-fail on any **new** advisory.
  - Whitelist the baseline IDs above with explicit expiry dates (force re-evaluation in 90 days).
  - Whitelist `bincode 2.0.1` unmaintained finding pending successor.
- **`rsa 0.9.10` (Marvin attack)** has no upstream fix and is dragged in by `jsonwebtoken` → `surrealdb-core`. Document as accepted risk in `SECURITY.md` (Phase 13) since it only affects JWT signing in SurrealDB's auth layer and we do not expose it externally.

## Regeneration

```sh
cargo audit > docs/audit-baseline-0.1.23.txt
```
