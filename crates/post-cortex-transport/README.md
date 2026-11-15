# post-cortex-transport

The transports of [pcx](https://github.com/julymetodiev/post-cortex):

- **the MCP server** (`mcp::McpService`, rmcp): every tool of the host's
  modules (`pcx_host::Host`), each call run under the host's policy within
  `mcp::TOOL_DEADLINE`, cancelled with its MCP request, the caller as its
  default author;
- **the gRPC status of a failed memory operation** (`grpc::status`).

The memory tools, the operation table, the response shapes and the shared
parsers live in `post_cortex_memory::tools`.

```rust,ignore
let host = Arc::new(Host::start(vec![Arc::new(MemoryModule::new(memory)?)], &config).await?);
let service = post_cortex_transport::mcp::McpService::new(host, INSTRUCTIONS);
```

License: MIT.
