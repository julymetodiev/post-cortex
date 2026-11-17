# Daemon mode

The daemon is the whole harness in one binary, `pcx`: the memory and the
modules, served over MCP (Streamable HTTP at `/mcp`), REST (`/api/*`) and gRPC
to every client on the machine or the network. The design is in
[ARCHITECTURE.md](ARCHITECTURE.md); this page is how to run and configure it.

## Quick start

```bash
pcx init      # optional: writes ~/.post-cortex/daemon.toml to start from
pcx start     # runs the daemon in the foreground
pcx status    # says whether it runs
pcx stop      # stops it (or Ctrl+C in its terminal)
```

Plain `pcx`, with no command, is a stdio MCP server for clients that only speak
stdio: it starts the daemon when it is not running and forwards to it.

## Configuration

**Priority:** environment variables > config file > defaults. `pcx start
--port` and `--host` override all three for one run.

**Config file:** `~/.post-cortex/daemon.toml`
```toml
host = "127.0.0.1"
port = 3737
data_directory = "/Users/username/.post-cortex/data"
```

| Setting | Environment | Default |
|---|---|---|
| `host` | `PC_HOST` | `127.0.0.1` (this computer only) |
| `port` | `PC_PORT` | `3737` |
| `data_directory` | `PC_DATA_DIR` | `~/.post-cortex/data` |
| `auth_token` | `PC_AUTH_TOKEN` | none (see [Bearer token](#bearer-token)) |
| `allowed_hosts` | `PC_ALLOWED_HOSTS` | `localhost`, `127.0.0.1`, `::1`, and `host` when it is a specific address |
| log level | `RUST_LOG` | `info` (`debug`, `trace`) |

`[[hooks]]`, `[[modules]]` and `[[builtins]]` reload without a restart (see
[Hook events and hook scripts](#hook-events-and-hook-scripts)); every other
setting takes effect at the next start.

## Endpoints

**Health check** (open, no token):
```bash
curl http://127.0.0.1:3737/health
```

**MCP** (Streamable HTTP): every tool of the memory and of the running modules.
```bash
claude mcp add --transport http post-cortex http://127.0.0.1:3737/mcp
# or by hand: initialize a session
curl -X POST http://127.0.0.1:3737/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}'
```

The memory's tools are listed in [TOOLS.md](TOOLS.md).

**REST API** (same bearer token and allowed hosts as `/mcp`; `/health` is open):

| Route | What |
|---|---|
| `GET /api/sessions`, `POST /api/sessions`, `DELETE /api/sessions/{id}` | list, create, delete sessions |
| `GET /api/workspaces`, `POST /api/workspaces`, `DELETE /api/workspaces/{id}` | list, create, delete workspaces |
| `POST /api/workspaces/{workspace_id}/sessions/{session_id}` | attach a session (`role`) |
| `POST /api/projects/resolve` | workspace and session of a project key (see [Project resolution](#project-resolution)) |
| `POST /api/messages` | capture turns (see [Conversation capture](#conversation-capture)) |
| `GET /api/cards`, `PUT /api/cards/user`, `PUT /api/cards/workspace/{id}` | user and project cards |
| `GET /api/file` | the file view (see [Graph views and the file hook](#graph-views-and-the-file-hook)) |
| `POST /api/search` | semantic search, as the MCP `semantic_search` tool |

`POST /api/search` runs the same search as `semantic_search`: `scope` is
`session` or `workspace` (both need `id`) or `global` (`id` ignored); `mode`
(`fast`, `local`, `accurate`), `sources` (`records` by default, `messages`,
`all`), `limit` (default 5, max 50) and `author` are optional. Only hits come
back, not the neighbouring records MCP adds around them. Bad input is 400, an
unknown session or workspace 404, a daemon without embeddings 503.

```bash
curl -X POST http://127.0.0.1:3737/api/search \
  -H "Content-Type: application/json" \
  -d '{"query": "why did we pick RocksDB?", "scope": "workspace", "id": "<uuid>", "mode": "fast"}'
# {"results":[{"id":"<entry id>","content_type":"DecisionPoint","title":"...","snippet":"...",
#   "score":0.82,"timestamp":"...","author":"claude-code","session_id":"<uuid>"}]}
```

## Workspaces

A workspace groups the sessions of one project (microservices, a monorepo's
components); `scope: "workspace"` searches across all of them. The MCP tool is
`manage_workspace` (actions `create`, `get`, `list`, `update`, `delete`,
`add_session`, `remove_session`, `archive`, `unarchive`); gRPC and
`/api/workspaces` do the same.

An archived workspace keeps its sessions, records and cards but is left out of
listings (MCP `list`, gRPC `ListWorkspaces`, `GET /api/workspaces`) unless they
are asked to include archived ones (`include_archived`,
`?include_archived=true`).

A session joins a workspace with a role: `primary` (the main session),
`related`, `dependency` or `shared` (one session in several workspaces).

## Running it as a service

```bash
PC_PORT=8080 PC_DATA_DIR=/var/lib/post-cortex pcx start
RUST_LOG=debug pcx start
```

To start it at login or boot, see [install/launchd](../install/launchd/README.md)
(macOS) and [install/systemd](../install/systemd/README.md) (Linux).

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

Every HTTP request except `GET /health` and `GET /health/jobs/<name>` (a
scheduler job watched from outside, below), and every gRPC call, must then send
`Authorization: Bearer <token>` (the webhooks carry their own token). The `pcx` CLI and the stdio proxy read the
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
| `discord_token` | Discord bot tokens |
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
| `fast` | none | 0.803 | ~40 ms |
| `local` | local cross-encoder (`reranker_model`); none in the cloud | 0.879 | ~1 s |
| `accurate` | Voyage (`voyage_api_key`) in a workspace that allows it, else local, if any | 0.937 | ~0.4 s |

A search that asked for a reranker and got none says why in its answer
(`rerank_skipped`): `no_local_reranker` (`local` on a server without one,
as every cloud tenant), `no_reranker` (`accurate` on a server with none),
`remote_not_allowed` (`accurate` where a workspace searched does not allow
Voyage and there is no local reranker to fall back to), `plan_limit` (in the
cloud: the organisation's plan has no reranker calls left, so its gateway ran
the search as `fast`; it marks the request `rerank_withheld`, a field only the
gateway sets).

`fast` and `accurate` from the W8 gate (October 2026, 1 535 questions);
`local` from the last run that measured it (2026-10-20).

The mode is a per-call `mode` argument; `search_mode` sets the default,
otherwise the best mode the configured rerankers allow.

```toml
# ~/.post-cortex/daemon.toml
reranker_model = "cross-encoder/ms-marco-MiniLM-L4-v2"  # local reranker (build feature `rerank`)
voyage_api_key = "pa-..."                               # or VOYAGE_API_KEY (build feature `rerank-voyage`)
voyage_model = "rerank-3-lite"                          # or "rerank-3"
voyage_endpoint = "https://api.voyageai.com/v1/rerank"  # default; a proxy or another region
search_mode = "local"                                   # fast | local | accurate
neighbour_radius = 1                                    # records on each side of a hit; 0 = hits only
```

- The local reranker runs int8 weights on ONNX Runtime. On Linux the daemon
  loads `libonnxruntime.so` at start: install ONNX Runtime 1.31 (Microsoft's
  release build needs glibc 2.28+) and point `ORT_DYLIB_PATH` at the
  library. If it cannot be loaded, the daemon logs an error and searches
  without the local reranker.
- With `voyage_api_key` set, Voyage is available, not on: each workspace
  allows it (`UpdateWorkspaceRequest.remote_rerank`, the app's workspace
  settings), and a new workspace does not. An accurate search sends the
  query and the top-300 candidate texts (each cut to about 256 tokens) to
  Voyage AI only when every workspace of its scope allows it: a workspace
  search its workspace, a session search every workspace the session is in
  (a session in none never), a global search every workspace and every
  session in one. Otherwise it uses the local reranker, if any.
  `SearchResponse.reranked` says whether a reranker ordered the hits; over
  MCP the result's `_meta.reranked` says it too, also when a `select` keeps
  only part of the data (`ToolOutput.meta`; the cloud counts reranker calls
  by it).

## Conversation capture

Records only exist when the agent writes them. Capture keeps the
conversation itself as well: every user prompt and the assistant's final
reply, in a per-session stream kept apart from the records (it never enters
the structured summary, the entity graph or importance scores).

Turn it on with `pcx setup --capture` (or answer yes at the prompt; it is
off by default; for the plugins and global hooks, `hosts.<client>.capture` in
`config.json`). For Claude Code the plugin's `Stop` hook, `pcx-capture.py`,
then posts the last few turns to the backend after every reply. It sends the prompt, the final text, the names of the
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
conversation excerpts. `admin` `vectorize_session` also repairs the session's
message vectors: turns whose embedding failed when they were captured, or whose
stored vectors are not exactly the chunks of their text (a chunk missing,
another text's vectors at that position), are embedded again, and vectors of
positions no stored turn has are removed. With the daemon stopped,
`pcx vectorize-all --messages` does the same for every session.

```toml
# ~/.post-cortex/daemon.toml
message_retention_days = 90   # older turns are deleted daily with their vectors; 0 = keep forever
```

Export and import stream: `pcx export` and the `Export` RPC write the archive
one session at a time (a counting pass first, since the header carries the
counts), and `Import` spools the archive to a temporary file as it arrives,
reads it once to preview and check it (format version, embedding model, the
bytes themselves) and once to write it, so a broken archive is refused before
anything is written. The archive's fields must come in the order pcx writes
them (header, then sessions, workspaces, checkpoints, vectors, cards); a file
re-serialized with its keys sorted is refused.

```toml
max_import_bytes = 1073741824   # largest archive Import takes (default 1 GiB)
```

## Browsers (the app on the web)

A daemon serves web pages only from the origins in `browser_origins`; with
none (the default) it serves no browser routes and answers no CORS
preflight. With origins it adds, on its HTTP port and behind the same bearer
token:

- gRPC-Web for `pcx.v1`, `pcx.views.v1` and `pcx.module.v1`;
- `POST /v1/import?commit=&skip_existing=&overwrite=&cards_newer=&session_id=&workspace_id=`
  with the archive as the raw body (spooled, at most `max_import_bytes`;
  without `commit=true` a preview), answering the import as JSON;
- `POST /v1/export` with an `ExportRequest` as JSON (`scope`, `session_ids`,
  `workspace_id`, `include_checkpoints`, `compression`, `no_messages`),
  answering a single-use link valid five minutes; `GET` of the link (no
  token: the link is the credential) downloads the archive.

```toml
browser_origins = ["https://app.example.com", "localhost"]   # localhost: any local port
```

## Hook events and hook scripts

Client hooks send their event to `POST /api/events`: the body is the
`pcx.module.v1` `EventEnvelope` as JSON, the scope and the client's hook JSON
as Claude Code writes it, and the answer is a Claude Code hook answer the hook
prints as it is.

```bash
curl -s -X POST localhost:3737/api/events -H 'Content-Type: application/json' -d '{
  "scope": {"workspace_id": "<uuid>"},
  "event": {"hook_event_name": "SessionStart", "source": "startup", "cwd": "/repo"}
}'
# {"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"## post-cortex cards ..."}}
```

The memory module answers SessionStart (the cards), PreToolUse (the file's
memory) and Stop (it appends `event.payload.messages`, the turns a hook
captured). Every module that fails or runs late is skipped (fail-open) unless
it is fail-closed, which blocks the turn.

The daemon also runs your own hook scripts as modules, with Claude Code's
contract: the event as JSON on stdin (and `PCX_SESSION_ID`,
`PCX_WORKSPACE_ID`), exit 0 with a JSON answer (or, for SessionStart and
UserPromptSubmit, text to add as context), exit 2 to block with stderr as the
reason. Each script is pinned by the SHA-256 of its file: the daemon refuses
to start with a script that does not match, and refuses to run one that
changed since, until you approve the new hash.

```toml
# ~/.post-cortex/daemon.toml
[[hooks]]
id = "secrets-guard"
command = "/Users/me/hooks/secrets-guard.py"
args = []
sha256 = "<shasum -a 256 secrets-guard.py>"
events = [{ event = "PreToolUse", policy = "fail-closed", deadline_ms = 2000 }]
capabilities = ["read_conversation"]
```

A script or module sees the conversation (the prompt, the tool's input and
result, the answer, the transcript) only with `read_conversation`; without
it, its events say what happened and to which tool, nothing more.

`policy` is `fail-open` (the default) or `fail-closed`; only a fail-closed
script may block. `deadline_ms` 0 takes the host's default (5 s).

A long-lived module in any language runs in its own process: the daemon
starts it with `PCX_MODULE_SOCKET` set, and it serves `pcx.module.v1` (gRPC)
on that Unix socket (in Rust: `pcx_module_api::serve`). Its manifest names
its id, the events it answers and how; its tools join the MCP tool list. It is
pinned by SHA-256 like a hook script, gets its `config` table at start, and
is stopped (killed past 5 s) when the daemon stops.

```toml
[[modules]]
id = "board"
command = "/opt/pcx/board-module"
sha256 = "<shasum -a 256 board-module>"
config = { columns = ["todo", "doing", "done"] }
grants = ["read_conversation", "write_memory"]
uses = ["post-cortex.memory", "tools-git"]
```

`[[hooks]]` and `[[modules]]` reload without a restart: the daemon looks at
`daemon.toml` every 2 s, keeps every module whose entry did not change (a
module process keeps running), starts the new ones and stops the removed
ones. A file that does not load, or a module that does not start, leaves the
running modules as they are (the reason is logged). Other settings take
effect at the next start.

The daemon refuses a module whose manifest asks for a capability `grants`
does not list (`read_conversation`, `write_memory`, `network`, `filesystem`,
`spawn`). The host enforces conversation access itself; `network`,
`filesystem` and `spawn` are declared and granted here; the files and
processes a module reaches through the host's sandbox (below) are confined
by the operating system to what its entry grants.

`uses` names the modules whose tools it may call; the daemon refuses a module
whose manifest uses one it does not list. Once started, the module calls them
through `Host.Invoke` on the callback socket named in its `StartRequest`
(`host_socket`); `Host.ListModules` there answers the running modules'
manifests. The host names the module as the caller (`Scope.caller_module`)
and refuses a chain of calls deeper than 4.

### Built-in tool modules and the sandbox

Files, a shell and git come built in, each off until `[[builtins]]` names it
with what it may do. Every file and process access goes through the daemon's
sandbox, under the grants of the module that asks: files beneath its
`filesystem` roots only (`write = true` to write; `..`, other absolute paths
and symbolic links pointing out are refused), and processes under the
operating system's sandbox (macOS: Seatbelt; Linux: Landlock and seccomp
through `pcx sandbox-run`; other platforms: refused) with read access to the
roots and the system's programs, write access to the writable roots, and the
network only with the `network` grant.

```toml
[[builtins]]
id = "tools-fs"            # read, write, edit, list_dir, grep, glob
grants = ["filesystem"]
filesystem = [{ path = "/Users/me/project", write = true }]

[[builtins]]
id = "tools-shell"         # bash, tmux_start, tmux_read, tmux_send, tmux_list, tmux_kill
grants = ["filesystem", "spawn"]
filesystem = [{ path = "/Users/me/project", write = true }]
config = { path = "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin" }

[[builtins]]
id = "tools-git"           # git_status, git_diff, git_log, git_branches, git_branch,
grants = ["filesystem", "spawn"]   # git_switch, git_commit, git_branch_commits
filesystem = [{ path = "/Users/me/project", write = true }]
config = { author_name = "Agent", author_email = "agent@example.com" }
```

The board (`id = "board"`) keeps boards, tasks, their comments and decisions
as documents in memory; it needs no grants of its own (it reaches memory and
`tools-git` through the host):

```toml
[[builtins]]
id = "board"               # board_create, board_list, task_add, task_update, task_move,
                           # task_list, comment, decide, approve_decision (a person only)
```

`board_create` gives a board its own session in the workspace, where every
decision is recorded (`decision_made`); a decision whose risk is not `none`
holds its task (it does not move) until a person approves it. A board that
names `repo` links a task's branch to its commits through `tools-git`. In the
app the board is a view: boards, a board's tasks in lanes by column, a task's
thread and decisions.

The skills library (`id = "skills"`) keeps skills as documents in memory, in
the one workspace its `config` names, and offers each skill to every MCP
client as a prompt of its name (`prompts/list`, `prompts/get`); like the
board it needs no grants of its own:

```toml
[[builtins]]
id = "skills"              # skill_add, skill_update, skill_list, skill_get
config = { workspace_id = "<the library's workspace id>" }
```

A skill has a name (`a-z`, `0-9` and `-`, unique in the library), a
description, its instructions in Markdown and the tools it needs. Its prompt
takes no arguments and answers the instructions, then those tools. The module
runs nothing: a script is written in the instructions, and the agent runs it
with `tools-shell`, which the skill names among its tools.

The `models` module serves the agent every model of the providers whose API
key its settings set (OpenRouter, Anthropic, Z.AI, OpenAI); it needs the
`network` grant. Each provider is a section of its settings: its `api_key` (a
secret setting) and optionally its `endpoint`. Each lists the models its API
serves when it starts, and any of them may be named:

| section | models |
|---|---|
| `openrouter` | every model OpenRouter routes |
| `anthropic` | Anthropic's Claude models (an API key only) |
| `zai` | Z.AI's GLM models over its coding endpoint |
| `openai` | OpenAI's GPT models |

```toml
[[builtins]]
id = "models"
grants = ["network"]
config = { zai = { api_key = "<the Z.AI API key>" }, openrouter = { api_key = "<the OpenRouter API key>" } }
```

The modules `provider-zai`, `provider-openai`, `provider-openrouter` and
`provider-anthropic` are gone: move each one's `api_key` (and `endpoint`) to its
section of `models`.

The agent (`id = "agent"`) runs turns: the model of its `config` (default
`glm-5.3`, served by the `models` module's Z.AI) with the tools of the
modules it uses that run (memory, `tools-fs`, `tools-shell`, `tools-git`,
`board`, `skills`, `team`, `observe`, `ops`, `scheduler`; a member names those
of them it may use), until the model answers. Every turn is kept whole in the
session's message stream (memory's `manage_messages`), so the next turn goes on
from it; the hooks of the other modules hear each step. The context engine
(`id = "context"`) adds what memory recalls for the prompt and, when a
conversation is too long for the model, sums it up (kept in the stream and as a
record):

```toml
[[builtins]]
id = "agent"
config = { model = "glm-5.3", max_steps = 50 }   # system, max_tokens too

[[builtins]]
id = "context"
```

The team (`id = "team"`) keeps the members of a workspace (`agent_add`: an
agent of the harness, or a name such as `claude-code` or a person) and its
teams (`team_create`). With a team, the board lets only members decide
(authority `decide`) and close tasks (`close`: a move into a done or canceled column). A
member of the harness works on its own: when a task is assigned to it, or a
comment mentions it (`@dev`) or answers it, it takes a turn of the agent with
its role, skills and model and the task's thread, and its answer is posted as
its comment. The trust a person gives the team (`autonomy_set`,
`autonomy_pause`, in the app) lets the board approve decisions with a risk at
once within its limits (`code`: branch prefixes, never protected branches;
`money`: a cap per decision and per day; `data`: reversible changes or
deletions too); `revoke_approval` takes an approval back:

A member's turn on a task may run 30 minutes and is tried twice; work that
failed both labels its task `blocked` with a comment why. On a board that names a
repository, a member works in a worktree of its own for the task
(`<repo>/.worktrees/<task>` on the branch `task/<task>-<title>`), which the task
links; `tools-git` must reach the repository.

```toml
[[builtins]]
id = "team"
config = { work_minutes = 30, tries = 2 }
```

The webhooks (`id = "webhooks"`) turn events from outside into board tasks.
Alertmanager is the first kind of source: it posts to
`POST /v1/webhooks/<source>` on the daemon's HTTP port with
`Authorization: Bearer <token>` (the webhooks' own token, a secret setting, not
the daemon's `auth_token`). Each incident of an alert group becomes one task on
the source's board, assigned to its member: the group firing again comments on
it, resolving comments how long it fired and records the incident in the
board's session (so the next alert like it brings it back), and a task its
member labelled `no-action` closes then. A wrong token answers 403, an unknown
source 404, a body that is not Alertmanager's 400. A source's `heartbeats`
name the alerts it only acknowledges (`heartbeats = ["Watchdog"]`): routed to
the same receiver every few minutes they add no task, and Alertmanager's count
of failed notifications shows a broken path while nothing else fires.

```toml
[[builtins]]
id = "webhooks"
config = { token = "<a long random token>", sources = [
  { name = "alertmanager", kind = "SOURCE_KIND_ALERTMANAGER",
    workspace_id = "<workspace id>", board = "<board id>", assignee = "eq-sentinel" },
] }
```

In Alertmanager:

```yaml
receivers:
  - name: pcx
    webhook_configs:
      - url: https://<daemon host>:3737/v1/webhooks/alertmanager
        send_resolved: true
        http_config:
          authorization: { credentials: <the same token> }
```

The scheduler (`id = "scheduler"`) runs jobs at cron times (five fields, in an
IANA time zone) or every N seconds (at least 60): a script under the sandbox
(it needs the `spawn` grant, and `network` when the script calls out), or a
task added to a board. A member's work at a set time is a task assigned to it
(the daily report: the team wakes the member, and its answer is its comment).
Jobs and their runs are documents in the workspace its `config` names; a job
runs once at a time, and after the daemon was down a job runs once for its
latest missed time. Tools: `job_add`, `job_update`, `job_list` (with each
job's last run and next time), `job_remove`, `job_run` (now, once) and
`run_list`. A dead man's switch is a job: a script that pings an outside
watcher every few minutes. The other way round, an outside probe reads
`GET /health/jobs/<name>` (no token): 200 while the job of that name runs on
time, 503 when it is paused or no run of it succeeded for 15 minutes past a
time of its schedule, 404 for no such job.

```toml
[[builtins]]
id = "scheduler"
grants = ["spawn", "network"]
config = { workspace_id = "<workspace id>" }   # path = "<PATH of its scripts>"
```

Mail (`id = "mail"`, with the `network` grant) sends with your own
credentials, through an SMTP relay (SES through its SMTP interface too), Resend
or Postmark: `mail_send` (recipients, a subject, a text body and an optional
HTML one), `mail_test` (a fixed message to one address, the app's test button)
and `mail_log` (each send with how it ended, kept 90 days, its bodies not kept;
the log is not searched). `allow` limits whom it mails (addresses or
`@domain`); `daily_limit` caps the messages a day (default 200). Passwords,
keys and tokens are secrets. See [modules/mail.md](modules/mail.md).

```toml
[[builtins]]
id = "mail"
grants = ["network"]
config = { workspace_id = "<workspace id>", transport = "TRANSPORT_SMTP",
  from = { address = "reports@example.com", name = "Reports" },
  smtp = { host = "smtp.example.com", user = "reports", password = "<password>" },
  allow = ["@example.com"] }
```

Workflow (`id = "workflow"`) runs graphs of steps over every module's tools
(`workflow_create`, `workflow_update`, `workflow_validate`, `workflow_list`,
`workflow_run`, `workflow_runs`, `workflow_run_get`, `workflow_cancel`); each
attempt of each block is kept in the run's journal, a run goes on after a
restart, and an approval waits for a decision on the board named by `board`. It
uses every module (`uses = ["*"]`) and asks for the host's models (`models`).
See [modules/workflow.md](modules/workflow.md).

```toml
[[builtins]]
id = "workflow"
config = { workspace_id = "<workspace id>", board = "<board id of approvals>" }
```

Telegram (`id = "telegram"`, with the `network` grant) is the first channel: it
sends to its chats by name (`channel_send`, which a workflow's `tool` block
calls; the webhooks' `notify` names one), sends each decision that waits for a person to its
approvals chat with Approve and Decline buttons (Decline ends the decision: who
asked it gives up), and answers a message from one
of its people with the chief's answer. A press approves as the person who
pressed, which the host allows only for the persons its `persons` setting
names; a press by anyone else changes nothing.

```toml
[[builtins]]
id = "telegram"
grants = ["network"]
config = { bot_token = "<from @BotFather>", approvals = "ops",
  chats = [{ name = "ops", chat_id = -5489920751 }],
  people = [{ telegram_id = 408741830, name = "julius" }],
  persons = { persons = ["julius"] } }
```

Discord (`id = "discord"`, with the `network` grant) is a channel to one Discord
server through a bot of your own (Developer Portal: Server Members and Message
Content intents on, Public Bot off; invite it with the rights it needs, never
Administrator). It sends to its channels by name (`discord_post`; the webhooks'
and ops' `notify` name one as `discord/<name>`, as Telegram's is
`telegram/<name>`), replies in forum posts and threads (`discord_reply`), gives
the roles and sets the forum tags its settings name (`discord_give_role`,
`discord_tag_post`), and publishes what people write in the channels it hears as
events (`discord.post_created`, `discord.message_created`,
`discord.member_joined`, `discord.automod_acted`) in the workspace its settings
name, where workflows follow them. It answers nothing by itself. After it starts
it reads its rights in the server; until that succeeds every call says what the
bot lacks there, with the link that adds it, and so does `discord_status`.

```toml
[[builtins]]
id = "discord"
grants = ["network"]
config = { bot_token = "<from the Developer Portal>", guild_id = "<server id>",
  workspace_id = "<workspace id>",
  channels = [{ name = "help", channel_id = "<id>", post = true, hear = true },
    { name = "changelog", channel_id = "<id>", post = true }],
  roles = ["Member"], tags = ["answered", "needs-team"] }
```

Observe (`id = "observe"`) reads a running system and never changes it:
`metrics_query` (PromQL), `logs_query` (LogQL), `alerts_list`, `cluster_get`
(a namespace's pods, deployments, jobs and recent warnings) and `pod_log`; in
a Nomad environment `jobs_get` (jobs, their versions and allocations, a
periodic job's schedule and launches), `alloc_log` and `node_get` (nodes, their
memory and disks). Its credentials are files beneath its `filesystem` grant,
read at each call (keep them outside every agent's roots); a cluster's are a
read-only ServiceAccount's token and the cluster's CA, Nomad's an ACL token
that may only read (`read-job`, `read-logs`, node `read`) and its CA. An
environment names a Kubernetes API or a Nomad API, never both:

```toml
[[builtins]]
id = "observe"
grants = ["network", "filesystem"]
filesystem = [{ path = "/home/dev/.pcx/creds" }]
config = { environments = [{ name = "prod",
  prometheus = "https://prometheus.example.net",
  loki = "https://loki.example.net",
  alertmanager = "https://alertmanager.example.net",
  kube_api = "https://<api server>:6443",
  kube_token_file = "/home/dev/.pcx/creds/prod.token",
  kube_ca_file = "/home/dev/.pcx/creds/prod.ca" },
  { name = "pcx", nomad_api = "https://<nomad server>:4646",
  nomad_token_file = "/home/dev/.pcx/creds/pcx-nomad.token",
  nomad_ca_file = "/home/dev/.pcx/creds/pcx-nomad-ca.pem",
  nomad_tls_server_name = "server.global.nomad" }] }
```

Ops (`id = "ops"`) changes a running system only through typed actions:
`ops_restart`, `ops_delete_pod`, `ops_clean_jobs`, `ops_scale` and
`ops_resize_volume`, in a Nomad environment `ops_new_allocation`,
`ops_run_periodic` and `ops_revert_jobs` (each with `dry_run`), and
`ops_actions`. A revert of jobs waits for a person whatever the trust; its
Nomad token needs `submit-job` before Nomad 1.11. A Nomad environment names
the jobs its actions act on (`nomad_jobs = ["pcx-cloud", "pcx-app"]`, at least
one); any other job is refused. Each is recorded
as a decision with risk `operations` on the task it names: the team's trust in
operations (`autonomy_set`, its `operations` actions and environments) runs it
at once; otherwise it waits for a person (the app, Telegram) and runs when
approved, its target checked again. Brakes hold a target for 30 minutes and an
environment to 6 actions an hour. Its account is its own, apart from observe's:

```toml
[[builtins]]
id = "ops"
grants = ["network", "filesystem"]
filesystem = [{ path = "/home/dev/.pcx/creds" }]
config = { workspace_id = "<workspace id>", notify = "ops", environments = [{
  name = "prod", kube_api = "https://<api server>:6443",
  kube_token_file = "/home/dev/.pcx/creds/prod-ops.token",
  kube_ca_file = "/home/dev/.pcx/creds/prod.ca", max_replicas = 3 }] }
```

`pcx agent "<question>"` runs one turn over `pcx.agent.v1` on the daemon's gRPC
port and prints the answer as it comes (`--session <id>` goes on with a
conversation; without it a new session is created and named).

A module with `spawn` has a scratch directory, `<data_directory>/scratch/<id>`,
emptied when the daemon starts; its processes get `TMPDIR`, `HOME`
and `TMUX_TMPDIR` there, `PATH` from its `config` (default
`/usr/local/bin:/usr/bin:/bin`, plus `/opt/homebrew/bin` on macOS),
`TERM=dumb`, `LANG=C.UTF-8`, and nothing else of the daemon's environment: no
`~/.gitconfig`, no keys. `tools-git` commits only as the author its `config`
names and runs the `git` program, so the repository's hooks run in the
sandbox too. `tools-shell` keeps its tmux sessions in its own tmux server
(`tmux -L pcx`, its socket in the scratch directory), which the daemon ends
when the module stops. On macOS a Unix socket path is at most 104 bytes, so
keep `data_directory` short when tmux is used.

## Project resolution

`POST /api/projects/resolve` finds or creates the workspace and session of a
project, so a client installed globally can recognise the project it runs in.
The caller computes the key: the `origin` remote URL without scheme, user and
`.git`, host lowercased (`git@host:a/b` becomes `host/a/b`), else
`local:<first 16 hex of the sha256 of the absolute git root or working
directory>`. The same key always gets the same workspace and session, so
teammates on the same remote share them; a session deleted meanwhile is
replaced by a new one. A new key gets a session named `name` in the
workspace named `workspace` (created if missing); without `workspace`, in the
one workspace named `name` that no project key is bound to yet (one made
before any agent connected, as the web app's onboarding makes it), else in a
new workspace named `name`. Two or more such workspaces are no guess: a new
one is made; a workspace another key is bound to is never joined this way.
Same as MCP `session` `resolve_project` and gRPC `ResolveProject`. gRPC
`FindProject` (`key`, optional `workspace` and `name`) answers where a key
would go without creating anything: its bound workspace and session, else
the workspace it would join, else nothing.

```bash
curl -X POST http://127.0.0.1:3737/api/projects/resolve \
  -H "Content-Type: application/json" \
  -d '{"key": "github.com/julymetodiev/post-cortex", "name": "post-cortex", "client": "claude-code"}'
# {"workspace_id":"...","session_id":"...","created":true}   (false on later calls)
```

`key`: 1 to 512 printable characters, no whitespace; `name` and `workspace`:
1 to 200 characters. Invalid input is 400.

## Project and user cards

A card is a short markdown note (at most 2 KB) put in front of every session:
the project card (one per workspace: stack, conventions, current goals) and the
user card (preferences that apply everywhere). Every change is a new version;
the history is kept, and a write names the version it was edited from, so two
writers cannot overwrite each other (the second gets the latest back).

- Agents read and write them with `manage_workspace` `get_card` / `update_card`
  (gRPC: `GetCard` / `UpdateCard`).
- The Claude Code plugin's SessionStart hook, `pcx-card.py`, prints both cards at
  the start of every Claude Code session and after compaction.
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

Captured turns can be deleted: one turn (MCP `session` `delete_message` with `seq`, gRPC
`DeleteMessage`) or a session's whole stream (`delete_messages`, gRPC `DeleteMessages`).
Their vectors go with them and the stream's stats follow; positions are never reused.
Records whose `derived_from` names a deleted turn keep the id as provenance.

## Graph views and the file hook

Agents get read-only views of the graph through the MCP `query_conversation_context`
tool and the gRPC `GetGraphView` RPC (JSON): `entity` (records, decisions, relations,
aliases), `impact` (who depends on an entity, with the valid decisions, requirements and
problems about them), `lineage` (problem -> decision -> consequences in time order),
`topics` (entity communities with key decisions), `radar` (unsolved problems, decisions
without rationale, open questions, stale entities, possible aliases) and `file` (past
problems, decisions and dependents of a source file). Add `workspace_id` to view a whole
workspace.

Radar items that are fine as they are can be dismissed per workspace (`manage_workspace`
`dismiss_radar_item` / `restore_radar_item` / `list_radar_dismissals`, gRPC
`DismissRadarItem` / `RestoreRadarItem` / `ListRadarDismissals`), by `kind` and `key`:
`unsolved_problem` and `stale_entity` (entity name), `decision_without_rationale` (record
id), `open_question` (question text), `alias_candidate` (`<name>|<of>`). The radar of the
workspace and of its member sessions then leaves them out and lists the matching
dismissals under `dismissed`; `include_dismissed=true` shows them again.

Records have a validity interval: a record ends when a later one supersedes it
(`supersedes` on write, or the same fact restated about the same entities), when
a later one asserts `conflicts_with` between entities it related, or when it is
forgotten (`manage_entity` `forget`). Ended records stay in the history and search ranks
them lower.

The Claude Code plugin's PreToolUse hook, `pcx-file-context.py`, gives
the agent what the memory says about a file before it edits it: past problems, decisions,
what depends on it (each with the relation that makes it a dependent) and the code it was
changed with. It sends the `PreToolUse` event to `POST /api/events`, and the memory module
answers with that text from the `file` view; every other client (`pcx hook`, the JS
plugins) asks the same way, so all of them get the same words. The view itself is served
over REST, with the same auth and allowed hosts as the rest of `/api/*`:

```bash
curl "http://127.0.0.1:3737/api/file?workspace_id=<uuid>&path=src/lib.rs"
```

## Admin commands and the running daemon

`pcx export`, `pcx import` and `pcx vectorize-all` go through the running
daemon's gRPC API (its `grpc_port` must be set); with no daemon running, and
`pcx migrate` always, they open the storage themselves. `pcx export --pretty`
and `pcx vectorize-all --messages` run offline only: stop the daemon first.
`pcx session` and `pcx workspace` always go through the daemon.

## Export and import over gRPC

`pcx export` / `pcx import` read and write the database files and need the daemon
stopped. A running daemon serves the same archive format over gRPC:

- `Export` (server streaming): `scope` `all` (default), `session` (with `session_ids`) or
  `workspace` (with `workspace_id`), optional `include_checkpoints`, `no_messages` and
  `compression` (`none`, `gzip`, `zstd`). The chunks' `data`, concatenated, is a
  `pcx export` file; the last chunk carries a summary (counts incl. `messages` and card
  versions `cards`, size).
- `Import` (client streaming): the file's bytes in chunks (any compression, detected from
  the first bytes), `settings` on the first chunk. Without `settings.commit` nothing is
  written and the reply is a preview: what the archive holds after the session/workspace
  filters (with `messages`, `messages_new` and `cards`), the sessions and workspaces the
  daemon already has (`conflicts`, resolved by `skip_existing` or `overwrite`, else
  reported as errors) and the cards whose history diverged (`kind` `card`, with
  `target` and `archive`: each side's latest version, time and first line). With
  `commit` the archive is imported as `pcx import` does and the counts come back.
  `settings.cards_newer` is `pcx import --cards-newer` (below).
  Archives up to 1 GiB.

What an archive holds (format 1.10.0, `schemas/export/v1.10.0.json`; 1.8.0 added each
workspace's slug, which an import keeps unless another workspace has it; 1.9.0 whether a
workspace allows the remote reranker, off in older archives; 1.10.0 the user card of each
person, scope `{"person": <user id>}`, which a full export carries), the same from
either backend. The metadata records the embedding model and dimension of the vectors
(`embedding_model`, new in 1.4.1); an import into a store of another model is refused before
anything is written (gRPC `FAILED_PRECONDITION`), and a preview lists it under `warnings`
(an older archive is compared by the length of its vectors):

| Scope | Sessions, records | Vectors | Messages | Cards, dismissals |
|---|---|---|---|---|
| `all` | every session, workspace | of every session | every session's stream | user card, every workspace card and dismissals |
| `workspace` | the workspace and its sessions | of its sessions | its sessions' streams | the workspace card and dismissals |
| `session` | the sessions | of the sessions | their streams | none |

Each record is in the file once (in its session). Cards come with every version
(author, time). `no_messages` (`pcx export --no-messages`) leaves the streams and their
vectors out; streams can be large. Files of format 1.0.0 to 1.4.x import; a newer or
malformed version is refused before anything is written.

How an import merges them:

- **Messages**, per session, by turn: a turn the session stores already is skipped (so a
  second import adds nothing); the others keep their `seq` when it was never used in the
  stream (at or past its next position) and go to the end otherwise; the stream's next
  position and count follow, and the message vectors move with their message. Archives
  of both backends carry message vectors; the daemon loads the restored ones and embeds
  those missing.
- **Cards**: a card the daemon lacks is imported with its history. When one history
  continues the other, the daemon's is brought up to date (or kept when it is ahead).
  When they diverged, the version numbers say nothing (two independent histories each
  count from 1), so only the times of the two latest versions are compared, and only
  when asked:
  - by default (also with `skip_existing`, "keep what is here") the daemon's latest
    version stays; the preview lists the card as a conflict with `resolution` `keep`
    and both latest versions (version, time, first line), so a person can choose;
  - with `cards_newer` (`pcx import --cards-newer`) the archive's latest version is
    added as the daemon's next version when it was written later (`resolution`
    `update`); nothing is deleted, and a later import of the same archive is quiet;
  - `overwrite` replaces the daemon's card history with the archive's.

  The user card is imported unless `workspace_ids` limits the import. `pcx import`
  prints the diverged cards with both latest versions before it writes.
- Messages and cards of a session or workspace the daemon has already merge in when it
  is skipped (`skip_existing`) or overwritten; with neither set it is reported as an
  error and they are left out.

Binaries reading format 1.3.0 at most refuse a 1.4.0 archive before writing anything;
archives of 1.0.0 to 1.3.0 import unchanged. Radar dismissals of an imported (or
skipped) workspace are added; one of the same kind and key is replaced.

Queued session writes are written before an export or import. Sessions imported without
embeddings are vectorized on their first search. Vectors travel with the records and
messages they belong to (message vectors renamed when a merge moves a message to another
position); an export leaves out vectors of positions no exported message has.

## Clients and profiles

Clients pick their backend through a profile. Two always exist: `cloud`, the
hosted service at `https://api.pcx.is` (the default), and `self`, the
daemon in `daemon.toml`. Self-hosting is opt-in.

```bash
pcx login                       # key from https://app.pcx.is (Connections → API keys)
pcx setup --client cursor       # the active profile (cloud)
pcx setup --self-hosted         # the daemon in daemon.toml (= --profile self)
pcx setup --capture-url http://ml.example.ts.net:3737   # a self-hosted daemon elsewhere
pcx profile list | use <name> | add <name> --url <url> | remove <name>
pcx logout
```

Profiles live in `~/.post-cortex/config.json`, keys in
`~/.post-cortex/credentials.json` (mode 0600, written by `pcx login`; no
keychain). `PC_CONFIG_DIR` moves both. The profile is `--profile`, else
`PC_PROFILE`, else `active`; `PC_URL` overrides its URL; the token is
`PC_AUTH_TOKEN`, else the stored key, else (for `self`) `auth_token` from
`daemon.toml`. The schema is documented at the top of
`crates/post-cortex-cli/src/profiles.rs`.

With a remote profile every client gets a stdio MCP entry running
`pcx mcp --profile <P> --author <client>`, which forwards to the profile's
`/mcp` with its key, and hooks run `pcx hook ... --profile <P>`; no key is
written into any client file. A `self` profile keeps the direct HTTP entry.
Plain `pcx` is unchanged: the local daemon proxy, starting the daemon when it
is not running. `pcx mcp-config <client> [--profile P]` prints the entry.

### Global and project setup

`pcx setup` is global by default: a client that reads a user-level config is
set up once for every project, and nothing is written into the project. Its
hooks run without ids (`pcx hook <client> <event> --profile P`) and resolve
the project the client runs in (the payload's `cwd`, else the process cwd):

1. `.post-cortex.json` `{"session_id","workspace_id"}` at the git root;
2. the ids in the project's `AGENTS.md` (`CLAUDE.md` for the Claude Code
   hooks), from a project setup made before;
3. the cache `~/.post-cortex/projects.json` (per project key, valid for the
   same backend URL and the same key: an entry carries `identity`, the first
   16 hex of the sha256 of the bearer token, never the token, so another
   organisation's key on the same URL resolves anew; an entry written before
   identities counts only for a backend reached without a token, and is
   otherwise resolved again and overwritten; ids the backend refuses with
   HTTP 403 or 404 are dropped from the cache, and the next hook resolves
   again);
4. `POST /api/projects/resolve` with the project key (see
   [Project resolution](#project-resolution)), `client` the client's author
   name and `workspace` from `hosts.<author>.workspace` in `config.json`.

A global hook does nothing in a project that has the same client's project
hooks (project mode wins, so a hook never runs twice), or when
`hosts.<author>.enabled` is `false`. The session-start text names the
session and workspace, since no instructions file does.

`pcx setup --project` keeps the per-project setup: it creates or reuses the
session and workspace, writes the client files with the ids into the current
directory, and writes `.post-cortex.json` at the project root, so other
clients and teammates resolve to the same memory. Re-running it replaces
post-cortex's own hooks and Codex `[mcp_servers.post-cortex]` section, so it
also switches a project between `self` and `cloud`; the person's own hooks
stay.

| Client | `pcx setup` (global) | `pcx setup --project` |
|---|---|---|
| Claude Code | prints the two `/plugin` commands; the plugin carries the MCP server, skill and hooks (capture: `hosts.claude-code.capture`) | the same, and only `.post-cortex.json`; what setups before the plugin wrote (the PCX section of CLAUDE.md, post-cortex's hooks in `.claude/settings.json` and `.claude/hooks/`, `~/.claude/agents/post-cortex-agents/`) is taken out (owner 2026-11-06) |
| Codex CLI | `~/.codex/config.toml`, `~/.codex/AGENTS.md`, `~/.codex/hooks.json`, `~/.agents/skills` (`$CODEX_HOME` respected) | `.codex/`, AGENTS.md, `.agents/skills` |
| Cursor | `~/.cursor/mcp.json`, `~/.cursor/hooks.json`, `~/.cursor/skills` (no user-level rules file: the session-start hook carries the rules) | `.cursor/`, `.agents/skills` |
| Gemini CLI | `~/.gemini/settings.json`, `~/.gemini/AGENTS.md`, `~/.agents/skills` | `.gemini/settings.json`, AGENTS.md, `.agents/skills` |
| Claude Desktop | its user-level config (it has no project files) | same |
| OpenCode, VS Code, Windsurf, Zed, Cline, Roo Code, Goose | project mode: they read their MCP entry, hooks or instructions from the project | project files |

```bash
pcx setup                          # Claude Code: the plugin commands
pcx setup --client codex,cursor    # every project, user-level files
pcx setup --project --client claude-code,codex   # this project, fixed ids
```

### Plugins and catalog packages

Clients that install from their own catalog get post-cortex as a package
instead of files written by `pcx setup`: the Claude Code plugin, a Cursor
plugin, a Gemini CLI extension, MCP Registry entries, and native plugins for
OpenCode, Kilo, Pi, OpenClaw and DeepSeek Harness. They take the backend
from their own settings (a URL and a key) or the same profiles
(`config.json`, `credentials.json`), resolve the project the same way (see
above) and call the same REST routes (`/api/cards`, `/api/events`,
`/api/messages`, `/api/projects/resolve`, `/api/search`). See
[`integrations/`](../integrations/README.md).

## Troubleshooting

**Port already in use:** another daemon or program holds it.
```bash
pcx status
lsof -i:3737
```

**403 `Host header is not allowed`:** the name in the client's URL is not on
`allowed_hosts` (see [Allowed hosts](#allowed-hosts-dns-rebinding)).

**401:** `auth_token` is set and the client sends no or another token.

**Config not loading:** check that `~/.post-cortex/daemon.toml` exists and is
valid TOML; the daemon logs the reason at start. Environment variables
override the file.
