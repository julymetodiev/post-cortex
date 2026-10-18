# post-cortex-proto

[![Crates.io](https://img.shields.io/crates/v/post-cortex-proto.svg)](https://crates.io/crates/post-cortex-proto)
[![Docs.rs](https://docs.rs/post-cortex-proto/badge.svg)](https://docs.rs/post-cortex-proto)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Protobuf + tonic-generated wire types for [post-cortex](https://docs.rs/post-cortex).

Single source of truth for the post-cortex gRPC schema (`pcx.v1`). External Rust gRPC clients can depend on this crate alone — it pulls `prost`, `prost-types`, `tonic` (with default transport features) and `tonic-prost`, but **not** the post-cortex-daemon server runtime or `rmcp`.

## Install

```toml
[dependencies]
post-cortex-proto = "0.4"
```

## Example

The daemon serves gRPC on port `3738` by default (`grpc_port` / `PC_GRPC_PORT`; HTTP + MCP are on `3737`). When the daemon has an `auth_token` (`PC_AUTH_TOKEN`), every gRPC call must carry `authorization: Bearer <token>` metadata:

```rust,no_run
use post_cortex_proto::pb::HealthRequest;
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use tonic::metadata::MetadataValue;

async fn check_health(token: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:3738")
        .connect()
        .await?;

    let bearer: Option<MetadataValue<_>> = match token {
        Some(t) => Some(format!("Bearer {t}").parse()?),
        None => None,
    };
    let mut client = PostCortexClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        if let Some(value) = &bearer {
            req.metadata_mut().insert("authorization", value.clone());
        }
        Ok(req)
    });

    let response = client.health(HealthRequest {}).await?;
    println!("daemon health: {:?}", response.into_inner());
    Ok(())
}
```

Without a token, `PostCortexClient::connect("http://127.0.0.1:3738")` is enough.

## License

MIT — see [LICENSE](../../LICENSE).
