# Daemon Mode

HTTP server for multi-client access to shared RocksDB instance.

## Overview

Daemon mode enables multiple Claude Code instances to share a single Post-Cortex database.

**Architecture:**
- Single HTTP daemon with lock-free concurrent access
- Multiple clients connect via HTTP/JSON-RPC
- Shared RocksDB storage with workspace isolation
- Streamable HTTP transport for MCP communication

## Quick Start

**1. Initialize config (optional):**
```bash
post-cortex-daemon init
```

**2. Start daemon:**
```bash
post-cortex-daemon start
```

**3. Check status:**
```bash
post-cortex-daemon status
```

## Configuration

**Priority:** Environment variables > Config file > Defaults

**Config file:** `~/.post-cortex/daemon.toml`
```toml
host = "127.0.0.1"
port = 3737
data_directory = "/Users/username/.post-cortex/data"
```

**Environment variables:**
```bash
PC_HOST=127.0.0.1
PC_PORT=3737
PC_DATA_DIR=~/.post-cortex/data
RUST_LOG=info  # debug, trace
```

**Defaults:**
- Host: 127.0.0.1 (localhost only)
- Port: 3737
- Data: ~/.post-cortex/data

## Commands

```bash
post-cortex-daemon init     # Create example config file
post-cortex-daemon start    # Start daemon (foreground)
post-cortex-daemon status   # Check if running
post-cortex-daemon stop     # Show stop instructions
post-cortex-daemon help     # Show usage
```

## Endpoints

**Health check:**
```bash
curl http://127.0.0.1:3737/health
```

**Server stats:**
```bash
curl http://127.0.0.1:3737/stats
```

**MCP JSON-RPC:**
```bash
curl -X POST http://127.0.0.1:3737/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "create_session",
    "params": {"name": "test", "description": "test session"},
    "id": 1
  }'
```

**MCP Streamable HTTP:**
```bash
# Initialize session
curl -X POST http://127.0.0.1:3737/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}'
```

## Workspaces

Organize related sessions (microservices, monorepo components).

**Tools:**
- `create_workspace` - Create workspace
- `get_workspace` - Get workspace details
- `list_workspaces` - List all workspaces
- `delete_workspace` - Delete workspace
- `add_session_to_workspace` - Add session to workspace
- `remove_session_from_workspace` - Remove session from workspace

**Session roles:**
- `Primary` - Main session
- `Related` - Related functionality
- `Dependency` - External dependency
- `Shared` - Shared utilities

**Example:**
```json
{
  "jsonrpc": "2.0",
  "method": "create_workspace",
  "params": {
    "name": "Microservices Project",
    "description": "Auth, API, Worker services"
  },
  "id": 1
}
```

## Production Usage

**Start with custom config:**
```bash
PC_PORT=8080 PC_DATA_DIR=/var/lib/post-cortex post-cortex-daemon start
```

**Debug logging:**
```bash
RUST_LOG=debug post-cortex-daemon start
```

**Stop daemon:**
```bash
# Ctrl+C in daemon terminal, or:
kill $(lsof -t -i:3737)
pkill -f post-cortex-daemon
```

## Lock-Free Architecture

- DashMap for concurrent session cache
- ArcSwap for atomic data structure updates
- Actor pattern for RocksDB operations
- Atomic counters for statistics
- Zero deadlock guarantee

## Performance

- Session operations: 1000+ ops/sec
- Concurrent clients: scales with CPU cores
- Hot/warm/cold memory hierarchy
- HNSW semantic search: 50-200ms
- Keyword search: <10ms

## Security

- Localhost only by default (127.0.0.1)
- File system permissions protect data directory
- No CORS layer: the daemon is not meant to be called from browsers

### Bearer token

Set a token before binding to anything but loopback (tailnet, LAN):

```toml
# ~/.post-cortex/daemon.toml
host = "100.125.13.76"
auth_token = "<long random string>"   # or PC_AUTH_TOKEN
```

Every HTTP request except `GET /health`, and every gRPC call, must then send
`Authorization: Bearer <token>`. The `pcx` CLI and the stdio proxy read the
same config and send it automatically. MCP clients over HTTP add the header,
e.g. `claude mcp add --transport http post-cortex http://host:3737/mcp --header "Authorization: Bearer <token>"`.
Without a token on a non-loopback bind the daemon logs a warning at startup.

### Redaction

On by default. Every write goes through the canonical `update_context`, which
replaces secrets and personal data with `[REDACTED:<kind>]` in the title,
description, details, examples, implications, entity names, relation
endpoints/context and code snippets:

| Kind | What |
|---|---|
| `private_key` | PEM private key blocks |
| `jwt` | JSON Web Tokens |
| `api_key` | `sk-…`, `sk-ant-…`, `ghp_…`, `github_pat_…`, `glpat-…`, `AKIA…`, `xox?-…`, `AIza…` |
| `telegram_token` | Telegram bot tokens |
| `bearer` | `Bearer <token>` (the word is kept) |
| `url_password` | password in `scheme://user:pass@host` (user and host kept) |
| `secret` | value of `password`/`token`/`secret`/`api_key`/… `=` or `:` |
| `email`, `phone` | e-mail addresses; `+<country>…` and Bulgarian `08x/09x` numbers |
| `ip` | public IPv4 only — private, loopback, link-local and tailnet (100.64/10) are kept |

Kept on purpose: UUIDs, host/pod/node names, private addresses — they are
pseudonymous and needed to correlate events. Replacements are deterministic,
so an entity and the relations that name it stay consistent.

```toml
redact = true                        # PC_REDACT=0 disables
redact_patterns = ['ORD-\d{6}']      # extra regexes → [REDACTED:custom]
```

Counts per kind are logged (`update_context: redacted sensitive values`),
never the values.

### SurrealDB credentials

`surrealdb_username = "root"` signs in at root level; any other user is
treated as a namespace user (`DEFINE USER … ON NAMESPACE`) of
`surrealdb_namespace`, so the daemon does not need root:

```sql
USE NS post_cortex;
DEFINE USER pcx ON NAMESPACE PASSWORD '…' ROLES OWNER;
```

## Troubleshooting

**Port already in use:**
```bash
lsof -i:3737
kill <PID>
```

**Permission denied:**
```bash
chmod 755 ~/.post-cortex
chmod 644 ~/.post-cortex/daemon.toml
```

**Config not loading:**
- Check file exists: `ls ~/.post-cortex/daemon.toml`
- Verify TOML syntax
- Use env vars to override

**High memory:**
- Reduce hot/warm cache sizes in code
- Monitor with `/stats` endpoint
