# Installing and connecting post-cortex

Read this only when the post-cortex tools are not available, or the user asks how to set it up.

## Install the `pcx` binary

```bash
cargo install post-cortex-daemon --features "embeddings,surrealdb-storage"   # Rust 1.95+
# or a prebuilt binary:
#   https://github.com/julymetodiev/post-cortex/releases/latest
```

The first semantic operation downloads the default embedding model (`potion-multilingual-128M`, ~50 MB) from Hugging Face once; after that everything runs locally.

## Connect a client

Two transports:

- **HTTP daemon** (recommended; several agents share one memory): `pcx start`, then point the client at `http://127.0.0.1:3737/mcp`.
- **stdio**: the client runs `pcx` itself; it starts a background daemon if none is running.

Claude Code:

```bash
claude mcp add --scope user --transport http post-cortex http://127.0.0.1:3737/mcp
# or
claude mcp add --scope user --transport stdio post-cortex -- pcx
```

Other MCP clients (Cursor, Codex, Gemini CLI, …): add an MCP server with either the HTTP URL above or the command `pcx`.

Then, inside a repository, `pcx setup` creates a session and workspace and writes their IDs plus memory rules into `CLAUDE.md` (and installs Claude Code hooks). For other agents, put the session ID and workspace ID in whatever project-instruction file they read.

## Remote / shared daemons

`~/.post-cortex/daemon.toml` (every key optional):

```toml
host = "0.0.0.0"                     # listen beyond loopback
port = 3737
auth_token = "<long random string>"  # clients send Authorization: Bearer <token>
allowed_hosts = ["my-mac.tailnet.ts.net", "100.64.0.7"]  # Host headers clients use
storage_backend = "rocksdb"          # or "surrealdb" + surrealdb_endpoint / _username / _password
```

Without `allowed_hosts`, a daemon bound to `0.0.0.0` answers remote clients with `403 Host header is not allowed` (DNS-rebinding protection). Without `auth_token`, anyone who can reach the port can read and write the memory — set one for anything beyond loopback.

Env overrides: `PC_HOST`, `PC_PORT`, `PC_AUTH_TOKEN`, `PC_ALLOWED_HOSTS`, `PC_DATA_DIR`, `PC_STORAGE_BACKEND`, `PC_SURREALDB_*`, `PC_REDACT`.

Check it: `pcx status`, or the `admin` tool with `action: "health"`.
