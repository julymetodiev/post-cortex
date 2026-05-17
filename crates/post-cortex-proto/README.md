# post-cortex-proto

[![Crates.io](https://img.shields.io/crates/v/post-cortex-proto.svg)](https://crates.io/crates/post-cortex-proto)
[![Docs.rs](https://docs.rs/post-cortex-proto/badge.svg)](https://docs.rs/post-cortex-proto)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Protobuf + tonic-generated wire types for [post-cortex](https://docs.rs/post-cortex).

Single source of truth for the post-cortex gRPC schema (`pcx.v1`). External Rust gRPC clients can depend on this crate alone — it pulls `prost`, `prost-types`, and `tonic` (with default transport features), but **not** the post-cortex-daemon server runtime or `rmcp`.

## Install

```toml
[dependencies]
post-cortex-proto = "0.2"
```

## Example

```rust,no_run
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use post_cortex_proto::pb::HealthRequest;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = PostCortexClient::connect("http://127.0.0.1:51900").await?;
    let response = client.health(HealthRequest {}).await?;
    println!("daemon health: {:?}", response.into_inner());
    Ok(())
}
```

## License

MIT — see [LICENSE](../../LICENSE).
