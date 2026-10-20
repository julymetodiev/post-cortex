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
PC_ALLOWED_HOSTS=my-mac.tailnet.ts.net,100.64.0.7:3737  # extra allowed Host headers
RUST_LOG=info  # debug, trace
```

**Defaults:**
- Host: 127.0.0.1 (localhost only)
- Port: 3737
- Data: ~/.post-cortex/data
- Allowed `Host` headers: `localhost`, `127.0.0.1`, `::1` (+ `host` if not a wildcard)

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
host = "100.64.0.7"   # your tailnet IP
auth_token = "<long random string>"   # or PC_AUTH_TOKEN
```

Every HTTP request except `GET /health`, and every gRPC call, must then send
`Authorization: Bearer <token>`. The `pcx` CLI and the stdio proxy read the
same config and send it automatically. MCP clients over HTTP add the header,
e.g. `claude mcp add --transport http post-cortex http://host:3737/mcp --header "Authorization: Bearer <token>" --header "X-PCX-Author: claude-code"`.
`X-PCX-Author` (or `PC_AUTHOR` for the stdio proxy and the `pcx` CLI) is the
default author of the connection's writes, records and cards; without it the daemon uses the MCP client's name,
unless that only names an SDK (`rmcp`, `mcp`).
Without a token on a non-loopback bind the daemon logs a warning at startup.

### Allowed hosts (DNS rebinding)

`/mcp` and `/api/*` only answer requests whose `Host` header is on an
allowlist; anything else gets `403 Forbidden: Host header is not allowed`.
This stops a web page from reaching the daemon through a DNS name it points
at `127.0.0.1` (RUSTSEC-2026-0189). `/health` accepts any `Host`.

Always allowed: `localhost`, `127.0.0.1`, `::1`, and `host` itself when it is
a specific address (not `0.0.0.0` / `::`). `allowed_hosts` adds to that list:

```toml
# ~/.post-cortex/daemon.toml
host = "0.0.0.0"
allowed_hosts = ["my-mac.tailnet.ts.net", "100.64.0.7:3737"]
# or PC_ALLOWED_HOSTS=my-mac.tailnet.ts.net,100.64.0.7:3737
```

| Entry | Matches |
|---|---|
| `my-mac.tailnet.ts.net` | that name on any port |
| `my-mac.tailnet.ts.net:3737` | that name on port 3737 only (`Host` must carry the port) |
| `100.64.0.7`, `::1`, `[fd7a::1]` | that IP on any port |
| `[fd7a::1]:3737` | that IPv6 address on port 3737 only |
| `*` | everything: disables the check (warned at startup; not recommended) |

Matching is case-insensitive; no schemes, paths or wildcards inside names.
List every name clients put in their URL (`http://<name>:3737/mcp`). Bound to
`0.0.0.0` / `::` with no `allowed_hosts`, the daemon warns at startup that
remote clients will get 403. The allowlist is not authentication: use
`auth_token` too.

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

## Search quality

`semantic_search` fuses vector and BM25 (keyword) rankings, optionally adds
candidates from the session's retrieval graph, reranks the top 300 and
returns each hit with the records right before and after it in its session
(`neighbour_of` names the hit). Measured on LoCoMo (share of questions whose
evidence is in the top 50, with neighbours):

| `mode` | Reranker | all@50 | Latency (server CPU) |
|---|---|---|---|
| `fast` | none | 0.789 | ~40 ms |
| `local` | local cross-encoder (`reranker_model`) | 0.870 | ~1 s |
| `accurate` | Voyage (`voyage_api_key`), local as fallback | 0.927 | ~0.5 s |

The mode is a per-call `mode` argument; `search_mode` sets the default,
otherwise the best mode the configured rerankers allow.

```toml
# ~/.post-cortex/daemon.toml
reranker_model = "cross-encoder/ms-marco-MiniLM-L4-v2"  # local reranker (build feature `rerank`)
voyage_api_key = "pa-..."                               # or VOYAGE_API_KEY (build feature `rerank-voyage`)
voyage_model = "rerank-3-lite"                          # or "rerank-3"
search_mode = "local"                                   # fast | local | accurate
neighbour_radius = 1                                    # records on each side of a hit; 0 = hits only
```

- The local reranker runs int8 weights on ONNX Runtime. On Linux the daemon
  loads `libonnxruntime.so` at start: install ONNX Runtime 1.31 (Microsoft's
  release build needs glibc 2.28+) and point `ORT_DYLIB_PATH` at the
  library. If it cannot be loaded, the daemon logs an error and searches
  without the local reranker.
- With `voyage_api_key` set, the top-300 candidate texts (each cut to about
  256 tokens) are sent to Voyage AI on every reranked search.

## Conversation capture

Records only exist when the agent writes them. Capture keeps the
conversation itself as well: every user prompt and the assistant's final
reply, in a per-session stream kept apart from the records (it never enters
the structured summary, the entity graph or importance scores).

Turn it on per project with `pcx setup --capture` (or answer yes at the
prompt; it is off by default). That installs a Claude Code `Stop` hook,
`.claude/hooks/pcx-capture.py`, which posts the last few turns to the daemon
after every reply. It sends the prompt, the final text, the names of the
tools used and the paths of the files they touched; tool output is never
sent. The request runs in the background and failures are silent. With
`auth_token` set, export `PC_AUTH_TOKEN` for Claude Code.

Clients post turns to the REST API (idempotent on `turn_id`, so retries are
safe; redaction applies):

```bash
curl -X POST http://127.0.0.1:3737/api/messages \
  -H "Content-Type: application/json" \
  -d '{"session_id": "<uuid>", "messages": [
        {"turn_id": "t-1", "role": "user", "text": "why is the build slow?"},
        {"turn_id": "t-2", "role": "assistant", "text": "...", "author": "claude-code",
         "tools": ["Bash"], "files": ["Cargo.toml"]}]}'
# {"appended":[0,1],"skipped":0,"vectorized":2}
```

Captured turns are searched only when asked for: `semantic_search` with
`sources: "messages"` (or `"all"`; the default is `"records"`). Each turn
found comes back whole, with the turns around it (`neighbour_radius`).
`assemble_context` takes the same `sources` and adds the matching turns as
conversation excerpts. `admin` `vectorize_session` also embeds turns whose
embedding failed when they were captured.

```toml
# ~/.post-cortex/daemon.toml
message_retention_days = 90   # older turns are deleted daily with their vectors; 0 = keep forever
```

## Project and user cards

A card is a short markdown note (at most 2 KB) put in front of every session:
the project card (one per workspace: stack, conventions, current goals) and the
user card (preferences that apply everywhere). Every change is a new version;
the history is kept, and a write names the version it was edited from, so two
writers cannot overwrite each other (the second gets the latest back).

- Agents read and write them with `manage_workspace` `get_card` / `update_card`
  (gRPC: `GetCard` / `UpdateCard`).
- `pcx setup` installs a SessionStart hook, `.claude/hooks/pcx-card.py`, that
  prints both cards at the start of every Claude Code session and after
  compaction (`--daemon-url` points it at a remote daemon).
- `assemble_context` puts them at the top of its result.
- `pcx card show` / `pcx card edit` (`$EDITOR`; `--user`, `--workspace`,
  `--url`).

```bash
curl http://127.0.0.1:3737/api/cards?workspace_id=<uuid>   # {"user": ..., "workspace": ...}
curl -X PUT http://127.0.0.1:3737/api/cards/workspace/<uuid> \
  -H "Content-Type: application/json" \
  -d '{"content": "## Stack\nRust, SurrealDB", "expected_version": 0}'
# 200 with the new version, 409 with the latest one if it changed meanwhile
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
