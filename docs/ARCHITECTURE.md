# pcx architecture

The specification of the system as it is (LAW rule 2: code follows it exactly). This file says
what the parts are and the rules every line obeys; each part's detail is in the file it names.
Work to do is on the `pcx` board, not here; how the system was built is in
[`history/waves.md`](history/waves.md) and git.

Code cites this file by section number ("ARCHITECTURE section 7, modules together"); the
numbers stay, and each section names the file that holds its detail.

## 0. The system

**pcx** is a module host: a small Rust runtime that loads modules behind one typed interface
(`pcx.module.v1`). **Memory** (post-cortex) is its first module and works on its own as a
library, a daemon and a hosted service. The other modules (a model provider, the agent, tools
under a sandbox, skills, the board, the team, the chief, webhooks, the scheduler, channels,
observe and ops) make the same host an agent harness that works on projects and runs systems.
Nothing in the host is specific to memory.

```
  app (closed, free: desktop, web, mobile later)      coding agents (Claude Code, Codex)
        │ direct (desktop only)    │ cloud                      │ MCP
        ▼                          ▼                            ▼
  pcx daemon  ◄── connect ──  cloud (gateway, accounts, plans) ──► hosted tenants
  (MIT; laptop or server:      (commercial)                         (the same pcx, one per
   modules, data, secrets)                                           organisation)
```

- **The daemon** (`pcx`, this repository, MIT) runs where the person or team chooses: a
  laptop or a server. Its modules, data, credentials and actions stay there. Configuration:
  `daemon.toml` ([`DAEMON_MODE.md`](DAEMON_MODE.md)).
- **The app** (Flutter, closed source, free; `post-cortex-cloud/app`) is only a client. The
  desktop app reaches a daemon directly (no account, offline) and the cloud side by side; the
  web app (and mobile, later) reaches the cloud only. Spec:
  `post-cortex-cloud/docs/APP-ARCHITECTURE.md`.
- **The cloud** (`post-cortex-cloud`, commercial) adds what only a service gives: accounts and
  organisations, plans and billing, the gateway, hosted tenants (this repository's composition
  per organisation; modules run as processes of its catalog). A daemon *connected* to an
  organisation is reached through the cloud by its people (planned:
  [`connect.md`](connect.md)), its data and credentials staying on its machine. Spec: `post-cortex-cloud/docs/ARCHITECTURE.md`.
- Everything between the parts is typed gRPC (rule 10); JSON exists only at the outside
  world's edges (MCP, a model's tool calls, outside APIs in an adapter).

## 1. Goals and measurable targets

Correct, fast and boring to extend. Every target is a number CI or a soak test can check.

| # | Target | Check |
|---|---|---|
| T1 | Host ready < 50 ms without modules, < 150 ms with the memory module; the embedding model loads lazily on first use; `pcx --version` < 20 ms | hyperfine in CI |
| T2 | Host overhead per turn (no model or tool time) p50 < 2 ms, p99 < 10 ms with 10 hook subscribers; memory recall hook p50 < 50 ms (fast mode) | turn-pipeline benchmark with no-op modules |
| T3 | Idle RSS: host < 30 MB, host + 8 profiles < 60 MB without memory; memory budgeted separately with one embedding engine per process; flat RSS (±5%) over a 24 h soak | `ps` after 1 h idle, soak job |
| T4 | A stuck, panicking or crashing module never stalls a turn or another session: deadline on every call, declared fail-open or fail-closed policy, panics caught at the boundary, out-of-process modules restarted with backoff | fault-injection tests (panic, loop, 30 s sleep, OOM) |
| T5 | Typed, semver'd module API; manifests declare the API version and capabilities; incompatible modules are refused at load with a clear error; N-1 major supported for at least 6 months | frozen v1 test modules loaded on every release |
| T6 | One discovery and registration path for every module kind, with typed exclusive slots | one code path, one validator |
| T7 | Scope is explicit on every call (tenant, profile, session, author, deadline, cancel); no process-global mutable state reachable from module code; no process-env reads below the composition root | two profiles in one host see only their own data; no mutable `static` in the API crates |
| T8 | One API, three isolation tiers: in-process (first-party Rust), out-of-process (any language, gRPC over UDS / named pipe, JSON for hook scripts), WASM later; third-party modules out of process by default | the same conformance suite in every tier |
| T9 | Hot reload of a module without restarting the host or dropping sessions; prompt and tool changes apply at the next session boundary | reload under load: 0 failed turns |
| T10 | One writer per tenant store; crash-safe | kill -9 during writes x1000: no corruption |
| T11 | Least privilege: modules declare capabilities (read conversation, write memory, network, filesystem paths, spawn); conversation access off for third-party modules by default; hook scripts pinned by hash | manifest audit + deny tests |
| T12 | Tool execution in an OS sandbox by default (Seatbelt, Landlock/seccomp) behind a `Sandbox` port | escape test suite |
| T13 | One static binary < 60 MB without models; no Python or Node needed for the host | release artifact size |
| T14 | Every module call emits a span (module, profile, duration, outcome); per-module p50/p99, timeouts and crashes in `admin health` | health output |

Not goals for now: dozens of channels or hooks on day one (start with about 12 typed events,
add one only when a module consumes it), and WASM before there is a third-party marketplace.

## 2. Layers

Hexagonal: dependencies point inwards, IO lives at the edge.

```
  transports (MCP, gRPC, REST, hooks, CLI)      adapters in
                 │  map wire <-> typed request, nothing else
  module host    │  registry, scope, policy, events, tracing
                 ▼
  capabilities (application services)           one service per capability
                 │  validation, orchestration, transactions
                 ▼
  domain (pure types and algorithms)            no IO, no async, no globals
                 ▲
  ports (traits) │  implemented by adapters out:
  storage (RocksDB, SurrealDB), vectors, embeddings, rerankers, clock, mail...
```

Rules that hold in every layer:

1. **Domain is pure.** No IO, no `async`, no locks or shared mutable state, no mutable `static`,
   no clock (time is a parameter). Immutable shared data is allowed: an `Arc` of data nothing
   changes, a `LazyLock` of constants. Caches and concurrent registries are capability or adapter
   state, not domain.
2. **One method per operation.** Each user-facing operation is one method on one capability
   service. Validation lives in that method and nowhere else.
3. **Transports contain no logic.** A transport handler parses the wire format into the typed
   request, calls the service method, and maps the typed response and the typed error back. MCP,
   gRPC and REST are at parity by construction: the operation table (section 6) is the single list,
   and a test fails when an operation lacks a transport mapping it is declared to have.
4. **Ports are small and per capability.** A capability depends only on the ports it uses.
5. **Errors are typed.** Each capability has one error enum (`thiserror`); transports map it in
   one place each (gRPC status, MCP tool error, HTTP status). No `unwrap`/`expect` outside tests
   and proven invariants (with a comment). No silent fallback for invalid or unreadable input:
   domain enums have `as_str` and a strict `FromStr` that inverts it; decode failures are typed
   `Corrupt` errors.
6. **Files are cut by responsibility and stay small.** A file that mixes concerns is split along
   the concerns. No source file exceeds 500 lines (aim for 400). Unit tests live in a sibling
   `tests.rs` (or `tests/` directory) and are held to the same limit. Generated code (prost/tonic
   output) is exempt, and so is upstream code the workspace patches (`third_party/`, each with a
   `PATCH.md` naming its source, licence and patch; no member of the workspace, used through
   `[patch.crates-io]`; owner 2026-10-31, cel-core). A CI check (`scripts/check-file-size.sh`) fails the build on any
   non-exempt file over 500 lines.
7. **Module paths are real modules, with an API**: no `#[path]`, no `pub(in ...)`, no `super::` paths outside
   `use super::*;` in a test module, and no `pub(super)`. Each module states its API with `pub`
   items (re-exported by its parent where callers need them) or `pub(crate)` items, and other
   modules use it by its `crate::` path. A helper that two sibling files share belongs to their
   parent or to a named shared module, not to one sibling exposed upward. A new `pub(crate)`
   item needs the owner's approval, listed in `scripts/approved-crate-items.txt` with the
   decision; nothing else is widened. Enforced in CI by `scripts/check-architecture.sh` (with
   `--new`, which fails on any new `pub(crate)` item not listed).
8. **Every `pub` item has a caller** or is part of a documented API. Dead code is deleted, not
   kept "for later".
9. **No hidden copies.** Before adding a function, search for one that does it (the review fails
   on duplicated logic).
10. **Everything is gRPC with typed proto messages** (owner 2026-10-25). Every service, the
    memory's, the host's, the agent's and every module's, speaks gRPC with its own proto
    messages: each field typed (numbers as numbers, booleans as booleans, records as messages,
    lists as `repeated`). No service carries JSON: no `google.protobuf.Value`/`Struct` for a
    service's own data, no JSON text in a `string`, no hand-written mapping between JSON and
    proto in either direction, no text made for a person in a response (the client shows the
    data). A module's handlers take and return its generated messages; modules call each other
    through generated gRPC clients. No hand-written copy of a generated type and no mapping
    between the two (`Scope`, `ToolSpec`, `Visibility`, `ModuleError`, the turn and model
    types of `pcx-module-api` today): code uses the generated type and adds behaviour in an
    `impl` or an extension trait; a struct of its own only adds what is not on the wire (a
    scope's `Instant` deadline and cancellation, beside the generated `Scope`). A module's
    domain data (a talk, an action, a task) is its package's generated message. The one place JSON exists is where the outside world is
    JSON: MCP (agents) and a model's tool calls, at their transport, converted with the
    canonical proto JSON mapping from the services' descriptors in one place, never by hand
    (section 7, everything gRPC). Enforced by `scripts/check-architecture.sh` (`grpc_only`: no
    `serde_json::Value`, `json!`, `google.protobuf.Value` or `Struct` in a service's API or a
    module's handlers).


## 3. Crates

Published crate names stay (`post-cortex-*` are on crates.io). Every crate names its layer.

| Crate | Layer | Owns |
|---|---|---|
| `pcx-module-api` | API | the `Module` trait and the host handle `ModuleHost`, the clients and servers of the slots (models, context engine, turns, sandbox), views, a Rust process module's `serve` (a `Module` as the core's `Process`); re-exports `pcx-module-core`; no dependency on a memory crate |
| `pcx-module-core` | API | a module process's core, shared by Rust process modules and the bindings: errors, `Scope`, tool declarations, settings and secrets rules, the tools of a manifest, the bus, the callback socket, the `pcx.module.v1` server of a `Process`, the bindings' inverted control (`handoff`) ([`modules/dynamic.md`](modules/dynamic.md)) |
| `pcx-module-py`, `pcx-module-node` | binding | the core for Python (PyO3, `bindings/python`, package `pcx-module`) and for Deno and Node (napi-rs, `bindings/ts`, package `@post-cortex/module`); only bytes and plain values cross |
| `pcx-bindings-tests` | test | the bindings end to end: the greet example and a fixture in Python and TypeScript, run by the real host (`scripts/test-bindings.sh`) |
| `pcx-host` | host | registry and slots, per-call policy (deadlines, fail-open/closed, panic capture, concurrency), routing and reach (`uses`, grants, persons), events and the organisation's bus, the process runner, per-module health; its ports (`Settings`, `EventStore`) |
| `pcx-sandbox` | adapter | a module's files beneath its roots (cap-std) and processes under the OS sandbox (Seatbelt, Landlock and seccomp) |
| `pcx-module-launcher` | adapter | a module's process under the OS sandbox with its limits, the launcher of the pot `pcx-modules` and its client: the adapters of `pcx-host`'s port `ModuleLauncher` ([`modules/dynamic.md`](modules/dynamic.md)) |
| `pcx-documents` | library | a module's documents in memory: the port, typed reads and writes, the adapter over memory through the host |
| `pcx-kube` | library | HTTP calls with credentials read from files through the sandbox: the Kubernetes API, bearer-token APIs |
| `pcx-nomad` | library | Nomad's API as observe and ops call it over `pcx-kube`: its names, paths, an environment's settings |
| `pcx-openai-compat` | library | the OpenAI-compatible chat completions wire and the provider on it (its listing, its bodies) |
| `pcx-tunnel` | adapter | a connected daemon's tunnel to the cloud (`pcx.tunnel.v1`, [`connect.md`](connect.md)): HTTP exchanges over one gRPC stream, the daemon's side and the gateway's, and the pairing |
| `post-cortex-core` | domain | the pure memory domain: records, entities and their graph, validity, messages, cards, freshness, redaction, summaries |
| `post-cortex-ports` | ports | memory's port traits (storage per capability, the bus's `EventLog` and `EventCursors`, `SearchIndex`, `ArchiveCodec`, embeddings, rerankers, clock), `PortError`, the conformance suites |
| `post-cortex-storage` | adapters | RocksDB and SurrealDB adapters of the storage ports, their rows and migrations; the `ArchiveCodec` adapter; `Connection`, a SurrealDB connection replaced when it stops answering (the SDK never checks its pings: a connection whose packets are lost hung every call, 2026-11-08), which the cloud's control plane and provisioner use too |
| `post-cortex-embeddings` | adapters | the embedding engine, rerankers and the in-memory `SearchIndex` (vector arena, BM25) |
| `post-cortex-blobs` | adapter | `BlobStore` over S3 |
| `post-cortex-memory` | capabilities + module | the memory capability services, their runtime state, and `MemoryModule` with its tools and the operation table |
| `post-cortex-transport` | adapters in | MCP (rmcp) over the host's tools; gRPC (`pcx.v1`, `pcx.views.v1`) and REST handlers; the browser layer (gRPC-Web, CORS) and transfer; the webhooks' HTTP route |
| `post-cortex-daemon` | composition | the `pcx` binary: config, which adapters and modules run with which grants, the routes; `compose`, the public composition every hosted tenant reuses |
| `post-cortex-cli` | client | profiles, login, coding-agent setup and hooks, the stdio MCP bridge, `pcx connect`; talks to a daemon or the cloud over the public APIs only |
| `post-cortex-proto` | API | every wire schema: `pcx.v1`, `pcx.views.v1`, `pcx.blocks.v1` (a message's blocks, shared by memory's API and the module protocol, so a memory client never carries `pcx.module.v1`), `pcx.module.v1`, `pcx.tunnel.v1`, and each module's package |
| `post-cortex` | facade | re-exports for library users |
| `pcx-agent`, `pcx-context` | modules | the agent's slots: turns, the context engine |
| `pcx-models` | module | every model provider in one module, each on with its API key ([`modules/agent.md`](modules/agent.md), the providers) |
| `pcx-provider-zai`, `pcx-provider-openai`, `pcx-provider-openrouter`, `pcx-provider-anthropic` | libraries | the model providers `pcx-models` starts ([`modules/agent.md`](modules/agent.md), the providers) |
| `pcx-tools-fs`, `pcx-tools-shell`, `pcx-tools-git` | modules | files, a shell and git under the sandbox |
| `pcx-board`, `pcx-skills`, `pcx-team`, `pcx-chief` | modules | work: boards, skills, members and teams, the chief |
| `pcx-webhooks`, `pcx-scheduler`, `pcx-telegram`, `pcx-discord`, `pcx-observe`, `pcx-ops` | modules | operations: events in, set times, channels, reading and changing a running system |
| `pcx-approval` | library | what a person approved of a module's code or grants and asking the board once, shared by `workflow` and `dynamic` ([`modules/dynamic.md`](modules/dynamic.md)) |
| `pcx-dynamic` | module | dynamic modules: bundles and their versions, their checks and approval ([`modules/dynamic.md`](modules/dynamic.md)) |
| `pcx-workflow` | module | workflows over every module's tools, CEL through `pcx-expr` ([`modules/workflow.md`](modules/workflow.md)) |
| `pcx-mail` | module | mail with the organisation's credentials, from templates ([`modules/mail.md`](modules/mail.md)) |
| `pcx-http` | module | calls of outside HTTP APIs through connections with the organisation's credentials, presets, the network rules ([`modules/http.md`](modules/http.md)) |
| `pcx-expr` | domain | templates of text and `{{ <CEL> }}` and CEL under limits, pure; the only crate that reaches `cel-core` ([`modules/workflow.md`](modules/workflow.md), expressions) |
| `pcx-greet` | example | the smallest process module, as a third party writes one |

A module crate is a hexagon of its own (rule 6 of CLAUDE.md's LAW): `domain` (pure), `ports`,
`adapters`, `service`, `handlers` (only map), `module` (the manifest and wiring), and a `main.rs`
when it runs as a process.

## 4-6. Memory

Memory is one cheap-to-clone service per capability (records, search, assembly, views,
change log, entities, sessions, workspaces, projects, cards, documents, blobs, messages,
transfer, freshness, admin), built from the ports the composition opens. Each operation is one
method with its validation and one typed error enum per capability; transports call these
methods and nothing else. Its ports live in `post-cortex-ports`; one writer per tenant store;
derived work (embeddings) on a bounded pipeline; the operation table is the single list of
operations and their transports, which a gateway classifies requests by.

Detail: [`memory.md`](memory.md) (section 4, capabilities; section 5, ports and runtime state;
section 6, the operation table).

## 7. Module host

A module has a manifest (id, version, API version, kind, slots, capabilities, the modules it
uses, its document kinds, its settings message, its views) and serves `pcx.module.v1`: tools,
prompts, events, its own gRPC services. The host loads modules in process or as processes over a
Unix socket, gives each call a scope (tenant, profile, session, workspace, author, deadline,
cancel), enforces the policy per module (deadline, fail-open or closed, breaker, restart with
backoff), the grants (network, filesystem roots, spawn, memory write, UI) and the reach between
modules (`uses`; the origin of a call: a client, an agent, a module; tools only a person calls),
and serves every module's tools over MCP and its services over gRPC. Settings are a typed
message the host checks before start, with secrets never returned. A module keeps its state as
versioned documents of its own kinds in memory. Every file and process access goes through the
sandbox. Every RPC states its effect (`read`, `write`, `external`), and a module declares the
events it emits and follows on the organisation's bus (owner 2026-10-31). A module in another
language than Rust is written only through the bindings to the Rust core. The agent (turns, the
context engine, model providers) is a set of slots.

Detail: [`host.md`](host.md) (the trait, kinds and slots, events, policy, the protocol, tools
and prompts, hook scripts, isolation, security, module UI and settings, modules together,
sandbox and tools, everything gRPC, effects, events of a module); [`modules/agent.md`](modules/agent.md)
(the agent); [`modules/dynamic.md`](modules/dynamic.md) (the bindings and dynamic modules).

## 8. Modules

Each is useful alone and keeps its state as documents of its own kinds in memory (versioned,
safe when two agents change one at once, with history, found by search). The cloud runs them
like any other module.

| Module | What it does | Detail |
|---|---|---|
| `board` | boards with columns and stages, tasks, their threads and decisions; a decision with a risk waits for a person | [`modules/board.md`](modules/board.md) |
| `skills` | a library of skills, each an MCP prompt | [`modules/skills.md`](modules/skills.md) |
| `team` | members (agents of the harness, people, outside agents) with roles and authority, teams, the trust a person gives them; a member works on its tasks with a deadline, tries and a worktree | [`modules/team.md`](modules/team.md) |
| `chief` | the host's main agent: reaches every module for its owners, under a grant, an audit and a daily budget | [`modules/chief.md`](modules/chief.md) |
| `agent`, `context`, providers | turns with a model and tools; recall and summaries; models | [`modules/agent.md`](modules/agent.md) |
| `webhooks` | events from outside (Alertmanager first) become board tasks, one per incident | [`modules/webhooks.md`](modules/webhooks.md) |
| `scheduler` | jobs at cron times or intervals: a script under the sandbox or a board task | [`modules/scheduler.md`](modules/scheduler.md) |
| `telegram` | the first channel: messages, approvals with buttons, the chief in a chat | [`modules/telegram.md`](modules/telegram.md) |
| `discord` | a channel and a community: messages, what its server says as events for workflows, posts, replies, roles and tags | [`modules/discord.md`](modules/discord.md) |
| `workflow` | graphs of blocks over every module's tools, started by a time, a webhook, an event, a person or an agent; durable, with dry runs, approvals and budgets | [`modules/workflow.md`](modules/workflow.md) |
| `mail` | sends mail with the organisation's credentials, from templates with `{{ }}`; a log of what it sent | [`modules/mail.md`](modules/mail.md) |
| `http` | calls outside HTTP APIs through connections that keep the organisation's credentials; presets as the editor's API blocks; no private address; a log without bodies | [`modules/http.md`](modules/http.md) |
| `dynamic` | modules in Python or TS installed into a running host, each version checked in a locked run and approved by a person | [`modules/dynamic.md`](modules/dynamic.md) |
| `observe` | reads a running system (metrics, logs, alerts, a cluster), never changes it | [`modules/observe.md`](modules/observe.md) |
| `ops` | changes a running system only through typed actions, each a board decision | [`modules/ops.md`](modules/ops.md) |

What the operations modules share (the outside world's JSON, credentials, the roll-out beside
Hermes): [`modules/operations.md`](modules/operations.md).

## 9. Names

The harness is **pcx** (home `pcx.is`): the module host with its modules, run by the `pcx`
binary. **post-cortex** is the memory module, usable on its own (libraries, daemon, plugins,
documentation at post-cortex.com); its commands stay commands of the `pcx` CLI.

The hosted product is pcx's (owner, 2026-10-31): the site `pcx.is`, the web app `app.pcx.is`,
the gateway `api.pcx.is`, the clients' default. `api.post-cortex.com` stays an alias of the same
gateway (the clients of 0.5.0 default to it), and post-cortex.com sends whatever is hosted to
`pcx.is`. The app and the gateway share one domain (the session cookie is same-site).
