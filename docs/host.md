# Module host

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 7, the module host.

```rust
// pcx-module-api (sketch; the crate is the contract)
pub struct Manifest { id, version, api: ApiVersion, kind: ModuleKind, slots, capabilities, events }
pub struct Scope    { tenant, profile, session, workspace, author, role, reach, read_only, person, deadline, cancel }

#[async_trait]
pub trait Module: Send + Sync {
    fn manifest(&self) -> &Manifest;
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError>;
    fn tools(&self) -> Vec<ToolSpec>;                                   // JSON Schema in, JSON out
    async fn call(&self, scope: &Scope, tool: &str, input: Value) -> Result<Value, ModuleError>;
    async fn on_event(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError>;
    fn services(&self) -> Services;                                     // typed gRPC/REST of first-party modules
    async fn stop(&self) -> Result<(), ModuleError>;
}
```

- **A person's role** (`Scope.role`, owner 2026-10-29): the organisation's role (owner, admin,
  member, viewer) of the person who is the call's author, as `x-pcx-role`; none: not known. Set
  by what authenticated the person: the gateway, from the person's key or session, removes a
  client's and sets it only when the call's author is the person (absent or the person's name),
  so an agent using a person's key acts under its own authority; on a daemon, the holder of its
  token (the desktop app's direct connection, its machine's owner). Only a client's call carries
  it: the host removes it from a module's call and from an agent's turn, so what a module or an
  agent does for a person never acts with the person's role.
- **A call's person** (`Scope.person`, owner 2026-11-09: a user card per person of an
  organisation): the cloud user the call is for, as `x-pcx-person` (the user's id); none on a
  local host. The gateway removes a client's and sets it for a person's session or personal
  key, an agent that names itself with the key included, never for a key's agent (C16), which
  has no person. Unlike the role it is no authority: every call made in the scope carries it,
  a module's and an agent's turn's too, so what is done for the person reads the person's user
  card (docs/memory.md, Cards). The memory instance reads it on every way in (MCP, REST,
  `/api/events`, gRPC) and refuses one that is no user id. The person's own card is within
  any reach the call carries (docs/connect.md 4a): a person limited to some workspaces keeps
  their card; the host's card (a call for none) stays every workspace's.
- **A call's workspace** (2026-11-08, prod: a task's commits did not show in the app): a call
  works in its scope's workspace, else the one its request names (`Scope::workspace`); the
  calls a module makes for it carry that workspace in their scope (`Scope::working_in`), even
  when the caller named it only in its request, so a runner's tools, which take only a call
  that names the workspace the runner serves (`docs/connect.md` section 7), and the host's
  reach see it.
- **Kinds and slots**: `memory`, `context_engine` and `agent` (W18) are exclusive slots; `model_provider`,
  `tool`, `channel`, `scheduler`, `sandbox`, `view` allow many. The host's models
  (`ModuleHost::model`) are the `agent`'s and the `context_engine`'s, and any module's whose
  manifest asks for the capability `models` and whose configuration grants it (owner 2026-10-31:
  the workflow module's `model` blocks).
- **Events** (about 12 to start): session start/end, turn start/end, pre/post model call,
  pre/post tool, pre-compact, memory write, schedule fire, channel inbound/outbound. Outcomes can
  add context, block or do nothing; any module may block by its answer (owner 2026-11-18: memory,
  fail-open, holds a file's first edit once).
- **Policy** per module and event: deadline, fail-open or fail-closed (what the module's failure
  or missed deadline does: the turn goes on, or is blocked), concurrency limit.
  In-process calls run under `tokio::time::timeout` with panic capture; out-of-process modules run
  as child processes speaking `pcx.module.v1`, restarted with backoff. A load starts its new
  modules at once (owner 2026-11-01), once every configuration fits its schema: a host starts
  in the time of its slowest module, and when one fails the others it started stop again
  (its error is the first failed in load order). A module process is its
  host's (owner 2026-11-01): it stops when the host calls Stop, or by itself once its host is
  gone (its parent changed); a stop signal sent to every process of a jail or a group
  (SIGTERM, SIGINT, SIGHUP: a Nomad stop, a terminal) leaves the stopping to the host, which
  stops its modules in order.
- **Module protocol**: `pcx.module.v1` in `post-cortex-proto` is the one source. Rust types are
  generated from it (prost) and wrapped by `pcx-module-api`; a module in another language is
  written only through the bindings to the Rust core (PyO3, napi-rs; owner 2026-10-31,
  [`modules/dynamic.md`](modules/dynamic.md)), never with the protocol generated for its
  language. Long-lived modules serve the protocol over gRPC on a Unix
  socket (named pipe on Windows), with a bidirectional stream for events. Every message has a
  JSON form by the proto3 JSON mapping (`pbjson`), used for hook scripts, debugging and logs;
  `int64` fields that a hook reads are typed so their JSON form is a number, and free-form
  payloads (`tool_input`) are `google.protobuf.Struct`.
- **Tools** from every module are exposed through MCP and a generic gRPC `Invoke` by the host;
  first-party modules may add typed services (memory keeps `pcx.v1` for its SDKs).
- **Effects** (owner 2026-10-31, Workflow 1b): every RPC of `Manifest.rpcs`, a tool or not,
  states what calling it does (`Rpc.effect`, `pcx.module.v1.Effect`), and the host's tool
  listing carries it (`ToolSpec.effect`):
  - `read`: changes nothing, in pcx or outside (a search, a listing, `observe`'s queries of
    an outside API);
  - `write`: changes state inside pcx only: memory, documents, settings, the files and the
    local git repositories under the module's roots;
  - `external`: acts outside pcx, or runs what may: sends a message or mail, calls an outside
    API that changes something, runs a process (`bash`, `tmux`), changes a running system
    (`ops`), runs a turn whose tools may do any of that (`chief_ask`, `workflow_run`).

  The class is what the call itself may do, not what modules following its events do later
  (`decide` is `write`; what `ops` does when the decision is approved is `ops`' own
  `external`). The manifest check (`pcx_module_api::rpc_tool`, the one the host and the cloud
  run) refuses an RPC with no effect, naming it. The effect is read by the workflow's dry run,
  approvals and retries (`modules/workflow.md`), the MCP transport (a call without a session, protocol 2026-07-28, sends a notification of
  ours after 10 s, since the answer's head goes only with its first message and a proxy limits
  silence; then keep-alives; `readOnlyHint` for `read`,
  `destructiveHint` for the others, `openWorldHint` for `external`), the app (beside each
  tool), the cloud's catalog and the host's read-only callers (owner 2026-10-31): a call
  whose scope may only read (`Scope.read_only`, `x-pcx-read-only: 1`, set by a gateway for
  a person or key without write permission, whatever the client sent, and carried by every
  call made in that scope) reaches only RPCs whose effect is `read` (the route refuses the
  others, `Denied`, from any origin) and runs no turn (a turn's tools may change things); the
  gateway lets such a caller call a module and leaves the check to the tenant's host, the one
  place that knows each RPC's effect.
  Memory's RPCs take theirs from the operation table (`Effect::Read` and `Search` are `read`,
  `Write` and `Delete` are `write`), and a test ties `Manifest.rpcs` to the table.
  A call that must act outside once whatever retries it (a workflow's `external` block,
  `modules/workflow.md`) carries `x-pcx-idempotency-key` (`metadata::IDEMPOTENCY_KEY`, beside
  the scope's keys, not a part of the scope), which the route passes on unchanged and a module
  passes on where the outside API takes one.
- **Every module** (owner 2026-10-31): a manifest's `uses` may name `*`, every module that runs
  (`pcx_module_api::EVERY_MODULE`, read by `pcx_module_api::uses` wherever the host reads
  `uses`): the workflow module's, whose workflows call any module's tools (a dynamic one's
  included). What each call reaches is still bounded: a workflow's grants (its check) and the
  route's reach and visibility on every call; a process module's configuration names `*` in its
  `uses` for it to load.
- **Workflow templates** (owner 2026-11-14): a module may ship workflows for an organisation
  to install (`Manifest.workflow_templates`, docs/modules/workflow.md, templates). The host
  keeps them as any field of the manifest and gives them with it (`ModuleHost::manifests`; a
  process module: `ListModules` on its callback socket); it checks nothing of them: a
  template is read and checked when it is installed, against the installer's tools.
- **Events of a module** (owner 2026-10-31, Workflow 1c): a module declares the events it
  emits (`Manifest.emits`: each a name, `<module id>.<name>` of a-z, 0-9, '_' and '.', the
  full name of its message in the module's descriptors and a description), and the host keeps
  them per organisation on the bus. They are the module's domain events (a task moved, a
  record added, a mail sent, a run failed), apart from the agent's hook events above, which
  answer within a turn, and from `MEMORY_WRITE`, memory's change feed in process.
  - *Publishing*: `Bus::publish` with the event's name and its message (`Any`); the host
    refuses a name the manifest does not declare and a message of another type, stamps the
    event (an id, UUIDv7; the module; the time; the scope's workspace and author) and keeps it
    before the call answers, so an event is never lost once published. The manifest check
    (`pcx_module_api::bus::check_emits`, the one the host and the cloud run) refuses an event
    not named `<module id>.<name>`, one named twice, and one whose message the module's
    descriptors do not hold.
  - *Following*: a module follows the events of the modules it uses (`uses`, the same grant
    as calling their tools; a manifest that follows an event no module it uses emits is
    refused at load), by name (`Manifest.follows`), or every event of a module it uses by
    `<module>.*` (owner 2026-10-31: the workflow module's event triggers), or every event of
    every module it uses by `*`, read against the
    modules that run (`pcx_module_api::bus::followed`); a module's grant is its organisation's
    whole, so it gets every workspace's. `ModuleHost::bus` (`pcx_module_api::bus::Bus`; a
    process module: `pcx.module.v1.Bus` on its callback socket) answers `follow`, a feed of the
    events after its cursor of each name, in the order they were appended, then each new one
    (a publish wakes it, else it reads again every 5 s), and `ack` (a name and a position:
    every event of the name up to it is handled); the cursor is kept per follower and name, so
    a new feed (a restart) starts where it acknowledged: at least once, and a follower
    deduplicates by the event's id.
  - *Emitting after a write*: an event is published after the write it is about, not in its
    transaction: a write stands when keeping its event fails (logged), and memory's events
    come from its change feed, which drops what a lagging reader missed (logged). A host
    without a store (`Host::start`, tests) refuses a publish (`FailedPrecondition`), which an
    emitter logs and goes on.
  - *Keeping*: events are kept 7 days; a follower whose cursor is older gets one event
    saying so (`fell_behind`) and re-reads its state, as with `MEMORY_WRITE`.
  - *The first events*: `board.task_created`, `board.task_moved` (a column changed),
    `board.decision_made` (`pcx.board.v1.TaskCreated`, `TaskMoved`, `DecisionMade`),
    `post-cortex.memory.record_added` (`pcx.v1.RecordAdded`, once per workspace of the
    record's session); with their modules later, `mail.sent`, `mail.failed`,
    `workflow.run_succeeded`, `workflow.run_failed`, `workflow.failing`.
  - *Layers*: the host checks, stamps and feeds (`pcx-host`, `bus`); it keeps events through
    its port `EventStore` (pcx-host's ports, as `Settings`; `Host::start_with`), which the
    composition fills with memory's `Events` capability in process
    (`post_cortex_daemon::compose::MemoryEvents`; memory's ports `EventLog` and
    `EventCursors` in `post-cortex-ports`, their RocksDB and SurrealDB adapters in
    `post-cortex-storage`), so the tenant's one writer keeps them and the host stays free of
    memory (section 0); the composition drops the old ones daily (`drop_old_events`).
- **Prompts** (W17; owner 2026-10-25): a module may offer MCP prompts, which can change while
  it runs (the skills of section 8). The Rust trait has `prompts(scope)` and
  `prompt(scope, name, arguments)` (none and `NotFound` for a module without prompts);
  `pcx.module.v1.Module` has `ListPrompts` and `GetPrompt`; a module that offers prompts says
  so in its manifest (`Manifest.prompts`), and the host asks no other. The host lists the
  prompts of those running modules in load order and routes a `get` to the module that lists
  the name, under that module's policy (deadline, breaker); when two modules give one name,
  the first in load order is served and the other is logged as a warning; a module whose
  listing fails is left out of it and logged as a warning. The MCP server offers the `prompts`
  capability and maps `prompts/list` and `prompts/get` to the host, in the caller's scope as a
  tool call.
- **Hook scripts**: short-lived scripts get the event as JSON on stdin and answer JSON on stdout.
  The JSON is the proto3 JSON form of the `pcx.module.v1` hook messages, whose field names
  (`json_name`) match Claude Code and Codex hook JSON, so existing hook scripts run unchanged; a
  golden test pins the JSON against recorded Claude Code and Codex payloads.
- **Isolation tiers**: in-process (first-party Rust), out-of-process (third-party default, any
  language), WASM (component model) when a marketplace needs it. A module's process is launched
  only through the port `ModuleLauncher` (pcx-host's ports; the composition chooses the adapter:
  direct for first-party modules, under the OS sandbox or by the launcher of the pot
  `pcx-modules` for a dynamic one, `modules/dynamic.md`).
- **Security**: capability flags enforced by the host (conversation access, memory write,
  network, filesystem paths, spawn); hook scripts pinned by hash and re-approved on change.
- **Module UI** (W11, W12), in three levels, the first that fits wins:
  1. *Settings*: the manifest names the message of the module's configuration
     (`config_type`, W23: section 7, everything gRPC, settings, for its fields, options and
     secrets). The host checks a configuration against it before `start` (a typed
     `InvalidArgument` naming the field); an app renders a form from its descriptor.
     - *What a text must be* (owner 2026-11-14: two forum tags typed on one line with a
       comma were saved, and only the running module found them wrong, in the server's
       log): a string setting's `pattern` (`pcx.module.v1.Setting`) is a regular expression
       its text matches whole (at most 1 KiB; of a repeated string, each value), with an
       `example` that matches it. The one check of a configuration refuses another text
       naming the setting (`setting tags[0] is like answered (it matches [^,]{1,20})`), so
       the cloud refuses it when it is saved and a host before `start`; the app's form says
       it as it is typed and shows the example under the field. A manifest whose setting
       has a pattern on what is no string, one that does not compile, one without an
       example that matches it, or an example without a pattern is refused. A pattern says
       a text's form only (an id of digits, a name without a comma, an `https://` address);
       what only the outside world knows a module reports once it runs (a module's
       problem, below).
     - *A module's problem* (owner 2026-11-14): what a module finds after it started
       that stops it from doing its work (its bot lacks a right in its server, its
       settings name a tag that is not there) it reports to its host, for a person and
       with what would fix it: `ModuleHost::report`, for a process module
       `pcx.module.v1.Host` `Report` on its callback socket (refused to any client). The
       host keeps the last one in the module's health (`ModuleHealth.problem`, empty: none
       it knows of; `ListModules`, `GetModule`, `/health`) and writes it in the module's
       log when it changes (`it cannot work: ...`, `it works again`); a module reports an
       empty one when nothing stops it any more, and a module started again has none. The
       app marks the module `needs attention` and shows the problem above its settings.
       It is not a failed start: the module runs, and its calls answer the same reason.
     - *Settings a module refuses* (owner 2026-11-14: a cloud saved what the settings'
       message takes, the module's own check refused it when its tenant reloaded, and the
       person read "saved"; at the tenant's next start the refusal failed the whole load).
       A reload that a running module's new configuration or start fails leaves it running
       with the one before, and its health's `problem` says so (`its new settings were not
       applied, and it runs with the ones before: ...`) until a reload applies its
       settings. A daemon's host starts all its modules or none, as before, and
       `ConfigureModule` answers the refusal. The host of an organisation
       (`Host::start_with` with its slug: a hosted tenant) goes on without a module, other
       than memory, whose configuration does not fit or that fails to start: its other
       modules and its memory run, the one that ran before stays, and `Host::left_out`
       names those that do not run with why, which the cloud shows the organisation's
       people in the catalog (post-cortex-cloud ARCHITECTURE C8).
     - *Secrets*: a `secret` setting is never returned; a listing says which are set. A
       change that leaves a set secret unset keeps it; one that names it in `clear` removes it.
       Both rules are `pcx-module-api` functions, so every host and the cloud apply the same
       ones.
     - *The host's modules* (`pcx.module.v1.Host`): `ListModules` answers each running module's
       manifest (without its descriptors, the bulk of a manifest: owner 2026-10-26, a listing
       of ten modules was 1.3 MB; `GetModule` answers one running module whole, its manifest
       with its descriptors, to read its configuration and draw its settings, and no module
       when none of that id runs; a module's callback socket answers whole manifests, which
       the memory module reads the document kinds from), its configuration without secrets, the secrets set, its health and (W25, owner
       2026-10-26) its sandbox as the operator granted it (roots, network, processes; the
       capabilities and `uses` it has are its manifest's, which the host refuses past its
       grant); `ModuleLog` (W25) answers a module's log, its newest 200 lines: what the host saw
       of it (a failed or panicked call, its breaker opening, its process exiting and starting
       again) and what its process wrote on stderr (also in the host's own log), kept in
       memory in `pcx-module-api`'s `LogRing` (the host's in its policy state, a process
       module's by the module, `Module::output`); a module's callback socket does not serve it;
       `ConfigureModule` checks a new configuration (the public check, secrets kept as above)
       and hands it to the host's `Settings` port, whose adapter persists it and reloads the
       module. The daemon's adapter writes the module's `config` in `daemon.toml` (comments and
       layout kept; written to a temporary file and renamed) and reloads through the one
       reloader the file watcher also uses, which holds the running set under a lock, so a
       change reloads once and the call answers the reload's result (a module that fails to
       start leaves `daemon.toml` and the running set as they were). It changes only the
       `config` of a configured module, never its command, hash or grants, and adds none; a
       host without the port
       (a hosted tenant, whose organisation configures modules through the cloud) answers
       `FailedPrecondition`.
     - *Grants* (P3, owner 2026-10-28): `GrantModule` changes what a module's sandbox lets it
       use, its roots (`filesystem`: an absolute path to an existing directory, and whether
       it may write, else `InvalidArgument`), `network` and `spawn`, and leaves its other
       grants as they are; a built-in module that does not run yet is turned on with them
       (`ListModules` names the built-ins a host can turn on, `available`). Only on the
       machine: it goes to the same `Settings` port (`Settings::grant`), whose daemon adapter
       writes the entry's `grants` and `filesystem` in `daemon.toml` (adding a `[[builtins]]`
       entry for a built-in it does not name; comments and layout kept) and reloads as above,
       the previous file staying when the module does not start. A host without the port (a
       hosted tenant, and a connected daemon's routes through the cloud) answers
       `FailedPrecondition`, and the gateway refuses it (`Denied`): a person in the
       cloud never widens what runs on a daemon's machine (`docs/connect.md` section 4). A
       grant is the decision of the person at the machine; a manifest still asks nothing past
       it.
  2. *Views*: the manifest declares views as data, not code: a `pcx.view.v1` tree of components
     (list, table, form, stat, chart, markdown, graph) whose data comes from the module's own
     tools and whose actions call them (an action that changes state says so and is confirmed).
     The host serves them (`ListViews`, `GetView` on `pcx.module.v1.Host`; data and actions
     through `Invoke` in the viewer's scope: the workspace the view is shown in, and the
     viewer as the author); the app renders them with its own components, so no module code runs in
     the app and a view looks and behaves like the app on web and desktop.
     - *The tree* (`pcx.view.v1`, versioned on its own): a `View` has an `id` (unique in the
       module), a `title`, a `description` and a root `Component`, one of `stack` (children in a
       column or a row), `list`, `table`, `lanes`, `form`, `stat`, `chart`, `markdown`,
       `graph` and `fields`. `fields` (W22, owner 2026-10-25) shows an object of its source field
       by field (each field's name for a person and its value; a list or an object as nested
       lines; never as JSON), with buttons under it (`actions`). A column's format `yes_no`
       shows a boolean, `fields` an object on one line; a service returns data (numbers,
       booleans, objects), never text made for a person, and the app shows it. `lanes` (W16) groups the rows of its source by a field into lanes, in the order a
       second source gives (an array of strings: a board's columns), each lane a list. A
       `list`, `table` or `lanes` may have a `detail` (W16): a component shown for the row a
       person picks; a `list` with `markdown` shows its titles as Markdown (a thread's comments),
       its links opened by the app (`pcx:record/<id>` a record of the view's workspace,
       `http(s)` in the browser). A `form` with `open` (owner 2026-10-25) is a button that opens it in a
       dialog (its first values from its source), which submitting closes; an action with a
       `form` asks for those fields in a dialog first (in a row action each starting from the
       row's field of the same name; its input takes `{{form.<field>}}` too; not on a form's
       submit); a `stack` row of dialog forms is a bar of their buttons. The board, team and
       skills views add and edit through them. A `chat` (W25, owner 2026-10-26) is the app's
       chat (section 7, the app's chat) with the host's agent or with the module's own turns
       (`agent` empty or the module's id, a module whose manifest declares `turns`), a member
       named by `member` (a name, or `{{row.<field>}}` inside a detail); with `open` it is a
       button that opens the chat in the dock beside the view, else the chat in place. The
       modules' screens (the team's, the chief's, the skills') are their views: the team's
       members each with a `Talk` chat and their work now (`ListWork`), the chief's chat
       beside its rights and actions.
     - *Data*: a `Source` names one of the module's own tools, its input (a
       `google.protobuf.Struct`) and a JSON Pointer (RFC 6901) into the tool's JSON result. A
       `list` or `table` reads an array of objects (fields named per column or line), a `stat` a
       number or a string, a `chart` an array of points (an x field and one or more y fields;
       line or bar), `markdown` a string or fixed text, a `graph` an object with a nodes array
       and an edges array (fields named for the id, the label and each end). A `form` has a
       schema in the settings subset above and an optional `Source` for its first values; a string
       field of `format` `multiline` is text of several lines (a file, instructions).
     - *Actions*: an `Action` has a label, a tool and an input template; a string value that is
       exactly `{{row.<field>}}` (row actions of a list, table or lanes, and any source or action
       input inside a `detail`, where it is the picked row's and, nested, the innermost one's)
       or `{{form.<field>}}` (the submit of a form) is replaced by that value with its type, and
       no other substitution exists. An action that changes state says so (`changes`, with the question to confirm),
       and the component reads its source again after any action. An action with `settings`
       (W22c, owner 2026-10-25) calls no tool: the app opens the module's settings form (level
       1), whose change is `ConfigureModule`, a person's; it has a label only (no tool, input or
       form), is not a form's submit, and needs a module with a `config_type`.
     - *Checks* (`pcx-module-api`, when a manifest is loaded): view ids are unique, every source
       and action names a tool of the module (a `settings` action none, as above), every pointer parses, every `{{form.x}}` names a
       field of its form's schema and `{{row.x}}` appears only in row actions and inside a
       `detail`; any failure
       refuses the manifest, naming the view and the component.
     - *Tools a view uses*: a `ToolSpec` says who may call it (`visibility`: `model`, `app` or
       both, both when unset, as MCP Apps). The host's tool list for agents (MCP `tools/list`,
       `Host.ListTools`) leaves out the tools without `model`; `Invoke` calls any of them.
  3. *Custom HTML* (escape hatch, compatible with MCP Apps `ui://` resources): a module granted
     the `ui_html` capability may serve an HTML resource the app shows in a sandboxed frame
     (scripts only, no same origin, a CSP with no network but the bridge; a webview on desktop).
     The frame talks only through a message bridge that calls the module's own tools in the
     viewer's scope; it never sees the app's session, tokens or memory. Allowed only where a
     real view cannot be expressed in level 2.
     - *The resource*: such a view names a `ui://<module id>/<path>` URI instead of a tree. The
       module serves it (a new `GetResource` RPC on `pcx.module.v1.Module`, a `resource` method
       of the Rust trait) as HTML with the MIME type `text/html;profile=mcp-app`; `GetView`
       answers it. A manifest with such a view and without `CAPABILITY_UI_HTML` is refused.
     - *The bridge* is the MCP Apps one (SEP-1865): MCP JSON-RPC 2.0 over `postMessage`. The app
       answers `ui/initialize` (host info, theme, no host capabilities but `serverTools`),
       `ping`, `tools/call` for the module's own tools whose `visibility` has `app` (`GetView`
       names them; called through `Invoke` in the viewer's scope; any other tool is refused),
       and takes
       `ui/notifications/initialized`, `ui/notifications/size-changed` and
       `notifications/message` (logged). Every other method answers JSON-RPC error -32601, so
       a module cannot post to the conversation, open links or read other resources.
     - *The frame*: the web app serves it from a separate sandbox origin (the MCP Apps proxy
       frame), the desktop app from a webview with no access to the app; the CSP is the MCP
       Apps default with nothing added (`default-src 'none'`, inline script and style, images
       from `data:`, `connect-src 'none'`), whatever domains the resource declares.
- **Modules together** (W14; owner 2026-10-24): a module may build on others: keep its own
  documents in memory, call another module's tools and follow the changes of memory. What it
  uses is declared and granted like its capabilities.
  - *Uses*: `Manifest.uses` names the modules it calls (`post-cortex.memory`, `tools-git`).
    The configuration grants them (`uses = [...]` in `daemon.toml`, the catalog entry in the
    cloud); the host refuses a manifest that uses a module it was not granted, and a call to a
    module outside its grant.
  - *Calls*: once a module's `start` has returned, the host lets it call tools through its
    handle (`HostContext.host`; a process module gets a callback socket in `StartRequest` and
    calls `Host.Invoke` on it). A call runs under the callee's policy with the caller's
    deadline; the host sets its `Scope.caller_module` (a module cannot set it), which the
    callee may check (memory does, below). A call chain is at most 4 deep; deeper is refused.
    A module calls any tool of a module it uses that an agent may call (`model` in its
    visibility, or both); a tool only a person calls is refused to it (below).
  - *Documents of a module*: `Manifest.document_kinds` declares the kinds of documents a module
    keeps in memory (`task`, written `board/task`), each with a JSON Schema of its data in the
    settings subset and the data field that is its title. A document lives in a workspace, has
    an id, a version (1 for the first, one more for every change), its data, a text and an
    author; changing it names the version it read (`expected_version`), so two writers never
    overwrite each other: the second gets a conflict with the latest version and decides again.
    Every version is kept as history; search finds a document by its latest title and text
    only, unless its kind says it is not searchable (`DocumentKind.searchable` false: a
    module's log or a run's journal, never embedded). Memory refuses a document of a module's kind unless that module writes it
    (`caller_module`) and its data fits the kind's schema; anyone with read access to the
    workspace reads it. This is the Documents capability (section 4) over a new
    `DocumentStore` port; records, their stored format and their search are unchanged.
    Export format 1.5.0 adds the documents and their history (1.4 archives still import).
    An import merges a document as a card (owner 2026-10-24, one merge function for both): a
    document the target lacks gets the archive's history; one history continuing the other
    gives the target the later versions; diverged histories keep the target's latest version
    and report the conflict, unless `overwrite` replaces the history (`cards_newer` is for
    cards only). A document is imported when its workspace is, and its latest version is then
    embedded through the derived-work queue.
    The details (owner 2026-10-24):
    - *Ids and deletion*: memory gives a document its id (a UUID) at its first `put`, which
      names no id and `expected_version` 0. `delete` removes the document with every version;
      deleting a workspace deletes its documents, as it deletes its card. A module that needs
      an audit trail keeps the document and marks it in its data instead of deleting it.
    - *Kinds*: the memory module reads the `document_kinds` of the running modules through
      its host handle (`HostContext.host`; a process module through `Host.ListModules` on its
      callback socket) when it checks a write, so a schema always comes from a checked
      manifest of the running set, never from the call (owner 2026-10-24). A document of a kind no running module declares is read but not
      written. A kind's name is lowercase letters, digits, `_` and `-`; its schema has no
      `writeOnly` (anyone who reads the workspace reads the document); its title field is a
      required string of the schema. Any other kind refuses the manifest.
    - *Search*: the latest version's title and text is one `SearchIndex` entry with the
      workspace id as its source, the content id `doc:<id>`, the kind `Document` and the
      document's kind in its metadata (`document_kind`); each new version replaces it. A workspace search covers its sessions and the
      workspace itself, a global search everything, a session search no documents; a hit is
      completed from `DocumentStore`. Inside memory a hit's source is a session or a workspace;
      on the wire a document hit has `content_type` `Document`, an empty `session_id`, its
      `workspace_id` (a new field of `pcx.v1` `SearchResult`) and its kind, version and title in
      `metadata`. A version is embedded through the derived-work queue (keyed by the
      workspace), never on the request path; a delete removes the entry at once (owner
      2026-10-24).
    - *Wire*: the memory tool `manage_document` (actions `put`, `get`, `history`, `list`,
      `delete`), an operation-table row each, reached by modules through `Invoke`; no `pcx.v1`
      RPC or REST route until a client needs one. `list` filters by kind and by equality on
      top-level singular scalar data fields (all must match; another field is refused),
      newest change first, in pages with the views' keyset cursor (the latest version's time
      and the id), each page read by storage alone (docs/memory.md, document keys and
      pages). Data is at most 64 KiB, text at most 32 KiB.
    - *Pages in a module* (owner 2026-11-06): `Documents::page` reads at most a limit of a
      kind, the most recently changed first, and the cursor of the next page; over it
      `Typed::page`, `Typed::scan` (each document to a visitor until it stops, page by page),
      `Typed::after` (a page after the document a reader names, for a list that pages by
      the last id it has) and `Typed::all`. A kind that grows with what people make (a
      board, a task and a member's work on it, a template, a job, an agent) may be read
      whole; a kind a module writes on every run, turn, call or send (a run, a step, a log
      entry, an action, a conversation) is read by a filter that names what it needs, by
      page or by a scan that stops, never whole on a call. A log is read whole only by its
      daily prune, which its days kept bound.
  - *Following changes*: a module subscribes to `EVENT_KIND_MEMORY_WRITE` with a filter
    (`Subscription.memory`: document kinds, record types, workspaces). The host feeds it from
    memory's ChangeLog: each change of what it filtered, in order per workspace, never
    blocking the writer, fail-open. A subscriber that falls behind gets one event saying so and
    re-reads what it follows, so nothing is lost when a host restarts either: the state is in
    memory, the event only says where to look. The event (owner 2026-10-24) is
    `hook_event_name` `MemoryWrite` with the payload `change` (`document` or `record`),
    `workspace_id`, `id`, `kind` (a document's) or `type` (a record's), `version` and
    `deleted`; falling behind is one event with the payload `fell_behind: true`. A record
    matches a workspace of the filter when its session belongs to it. A subscription without
    a filter follows every change; with one, a document whose kind it lists and a record whose
    type it lists (a removed record, whose type is gone, when it lists any type), in the
    workspaces it lists (none: every one). The event names what changed, never what was said,
    so a module without conversation access gets it whole.
  - *Tools only a person calls*: a tool whose visibility is `app` only is refused when an agent
    calls it: over MCP always, from a module always, and through `Invoke` when the caller is
    not a person (in the cloud, an API key; the app's session is a person; on a local daemon
    `Host.Invoke` is the person at the machine). A decision that needs a person's approval is
    such a tool (owner 2026-10-24).

- **Sandbox and tools** (W15; owner 2026-10-24): every file and process access of a module
  goes through the `Sandbox` port, under the grants of the module that asks.
  - *The port*: `Sandbox` is a trait of `pcx-module-api`, filled by the module in the exclusive
    `sandbox` slot (`pcx-sandbox`, built in). A module reaches it through its host handle
    (`ModuleHost::sandbox()`; a process module through a `pcx.module.v1.Sandbox` service on its
    callback socket); the host binds each handle to the policy of its module, which the module
    cannot widen. Without a module in the slot, or on a platform it does not support, every
    call is refused (`FailedPrecondition`): fail closed (T12).
  - *The policy*: a module's roots are the `filesystem` entries of its configuration entry
    (`[[modules]]` or `[[builtins]]` in `daemon.toml`: a path and whether it may write); its
    network and process access are its `network` and `spawn` grants. A manifest that declares
    a filesystem path outside its roots, or asks for a capability not granted, is refused.
    The composition hands the host each module's policy with its configuration
    (`Host::start` / `reload`, by module id); a module never states its own, and one the
    composition names no policy for is granted nothing. A runner's denied names (`[runner]
    deny`, `docs/connect.md` section 7) are in every module's policy (`Policy.deny`): no file
    or process of any module reaches them. A policy marked local only (`Policy.local_only`,
    the tool modules) refuses a client's call held to a reach, a call from the cloud
    (`docs/connect.md` section 4).
    A relative path in a call resolves against the first root, or the root the call names
    (`root`); any other path must lie under a root.
  - *Files* (read, write, list, metadata, remove, rename): run in the host process through
    `cap-std`, one `Dir` per root, so a path resolves beneath its root component by
    component: `..` above it, an absolute path elsewhere and a symlink pointing out are
    refused, with no window between a check and the open. Writes need a root that allows
    them and replace a file atomically.
  - *Processes* (`exec`: a program, its arguments, a working directory under a root, an
    explicit environment, stdin, a deadline and an output limit; its output streamed, its exit
    status): the child runs under the OS sandbox with read access to its roots and the system
    paths a program needs to start, write access to the roots that allow it (Unix sockets
    there included, as tmux's server needs) and to the terminal devices (`/dev/null`, `/dev/tty`,
    the pseudo-terminals), and the network only with the `network` grant. macOS: `sandbox-exec` with a Seatbelt profile built from the
    policy. Linux: the `pcx` binary's helper mode (`pcx sandbox-run --policy … -- program`)
    applies Landlock and seccomp to itself, then runs the program, so no `unsafe` code sets
    them up; in its mount namespace the module's scratch directory is `/tmp` too (owner
    2026-11-01: programs write there by name), and on a host that allows no user namespace
    a module without denied names runs with the system's `/tmp` (macOS: the system's, as
    Seatbelt maps no path). FreeBSD (owner 2026-11-01, e13780aa): the same helper mode, run as root, runs the
    process in the module's jail and waits for it. A module's jail is its policy's
    (`pcx-<hash of the policy>`; the module's scratch directory, a root, makes it its own),
    built for its first process:
    - *its root* `/var/run/pcx-sandbox/<jail>`, of nullfs mounts: the system paths
      read-only (`/bin`, `/sbin`, `/lib`, `/libexec`, `/usr`, `/etc`,
      `/var/run/ld-elf.so.hints`), devfs with ruleset 4 (`devfsrules_jail`), the module's
      scratch directory at `/tmp` as well (owner 2026-11-01: programs write there by name),
      each root at its
      own path (read-only unless it allows writes), and an empty file or directory mounted
      read-only over each denied name under the roots;
    - *the jail* (`jail(8)`, so no `unsafe` code): `ip4`/`ip6` `inherit` with the `network`
      grant (public addresses only: *Network* below), `disable` without it, `enforce_statfs` 2; the program runs in its working
      directory as the unprivileged user `[sandbox] user` of `daemon.toml` names (`jexec -U`,
      so its exit status is its own); without that user every process is refused;
    - *kept* (owner 2026-11-01) while a process runs in it or one a process left lives there,
      such as a tmux server, as such a process outlives its command on Linux and macOS: the
      module's next process enters it with `jexec`. A process that stays after its command
      is one off the command's output (`tmux -d`, `daemon -f`): a command ends when its
      output does. One helper builds or removes a jail at a time (`<jail>.lock`), and each
      helper holds `<jail>.users` shared while its process runs; the last removes both with the
      jail;
    - *after it*, once no helper uses the jail and no process lives in it: the jail removed,
      the mounts undone in reverse (`umount -f`, which unmounts by id: by path the kernel
      refuses a mount over a file) and the root removed, a directory only once it is empty.
      A helper killed before that (a deadline kills the whole process group) leaves the
      jail's mounts, recorded in `/var/run/pcx-sandbox/<jail>.mounts`; the jail's next
      helper undoes them before it builds the jail again.
    The pot (or host) that runs it allows nested jails: `children.max` above 0,
    `allow.mount`, `allow.mount.nullfs`, `allow.mount.devfs` (pot has no attribute for it:
    the cluster's `provision.sh` adds `devfs`), and `enforce_statfs` 0, since a nullfs mount
    over a file inside a jail is recorded without the jail's path and at 1 cannot be undone.
    Limits on memory, time, files and processes are a dynamic module's (`docs/modules/
    dynamic.md`, limits): the helper sets open files and processes with `setrlimit`, the launching
    side watches memory, and the pot's total is its Nomad resources (`racct`). Other platforms:
    refused. A deadline or a cancel kills the child's
    process group.
  - *Network* (task c199da28): on the FreeBSD hosts (production) the kernel bounds what a
    process with the `network` grant reaches: public addresses only. Every process that acts
    for an organisation with the network runs as the sandbox user: a tool module's processes
    (`[sandbox] user` of the runner), a dynamic module's install (`Prepare`) and every module
    of the cloud's catalog (`--user` of the cloud's launcher, `docs/modules/dynamic.md`), one
    uid on every node (1500). The node's pf refuses that uid every connection to the loopback,
    the node's own addresses, and the private, carrier-NAT, link-local and unique-local
    ranges, except its DNS resolver (the `pcx-egress` anchor, before any `pass`) and an
    inside git host's ssh that a runner of the node
    clones from and pushes to, by its address and port (owner 2026-11-08: a runner whose
    git host is on a private network; the operator's `pf.conf`, one address and one port, which
    every process of that uid on the node reaches, a key still needed). `lo0` is filtered for it, as a connection to the
    node's own address goes over it. So a setting that names an inside address (a mail relay,
    a model provider's endpoint), a name that resolves inside, and a library that opens its
    own socket reach nothing inside; the node's services keep their credentials as a second
    wall. A process without the grant has no address (`ip4`/`ip6` `disable`). Linux and macOS
    (a daemon; staging, which is for tests only: owner 2026-11-08) bound the network all or
    nothing, since Landlock and Seatbelt filter no address; there the `http` module's own
    rules hold (`docs/modules/http.md`). No grant names its destinations (`host:port`):
    public addresses only (owner 2026-11-08), since where mail, http and a provider connect is
    the organisation's own setting.
  - *Scratch and environment* (owner 2026-10-24): a module with the `spawn` grant has a
    scratch directory, `<data directory>/scratch/<module id>` (on FreeBSD owned by the jail's
    user), writable in its sandbox and
    emptied when the sandbox module starts, so what another module writes there before this one's first process stays. Its processes get `TMPDIR`,
    `HOME` and `TMUX_TMPDIR` pointing there and only the rest of the environment the call
    names (the tool modules give `PATH` from their configuration, by default
    `/usr/local/bin:/usr/bin:/bin` and `/opt/homebrew/bin` on macOS, `TERM=dumb` and
    `LANG=C.UTF-8`); nothing else of the host's environment, so the user's `~/.gitconfig` and
    keys are out of reach and `tools-git` names a commit's author from its configuration.
    `tools-shell` adds the variables its configuration names (`env`: a toolchain's, such as
    `CARGO_HOME`, `RUSTUP_HOME`, `LIBCLANG_PATH`, `PROTOC`), never `PATH`, `TERM`, `LANG`,
    `HOME`, `TMPDIR` or `TMUX_TMPDIR`; the directories they name are reached only as roots
    grant them.
  - *A commit's author* (owner 2026-11-07): what an agent commits names the agent, never a
    person. The agent of a call is its scope's author when the call carries no person's role
    (`Scope::agent_author`); its address is `<name>@<tenant>.agents.pcx.is`
    (`<name>@agents.pcx.is` on a host without a tenant), the tenant as calls between modules
    say, a domain that receives no mail, the
    name's characters other than letters, digits, `.`, `_` and `-` written `-`. `tools-git`
    commits as the identity its configuration names (`author_name`, `author_email`) and, for
    an agent's call, names the agent the commit's author (`--author`); a person's call stays
    the configured identity's. `tools-shell` gives the processes of an agent's call
    (`bash`, and a `tmux_start` session through `new-session -e`) `GIT_AUTHOR_NAME`,
    `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_NAME` and `GIT_COMMITTER_EMAIL`, the agent in both
    (owner 2026-11-08: git needs a committer, and the sandbox has no identity of its own),
    which git reads before any `user.name`, so a commit through a shell cannot borrow
    another name; its configuration does not set them. A person's call gets none of them: a
    person commits through `tools-git`, as its configured identity. A person's talk with a
    member runs as the member (`modules/team.md`), so its commits are the member's.
  - *The tool modules* (built in, each off until `daemon.toml` names it in `[[builtins]]`
    with its grants; a hosted tenant has none): `tools-fs` (read, write, edit, list_dir, grep,
    glob, over the port's files), `tools-shell` (bash: one command, its deadline and the tail
    of its output; tmux sessions, whose tmux server itself runs under the sandbox; `bash` and
    `tmux` are found through its `path`, as every program it runs is, so a host needs both there),
    `tools-git` (status, diff, log, branch, commit, the commits of a branch and, when its
    configuration names `push`, pushing a task's branch through an ssh-agent, docs/connect.md
    section 7; as the `git`
    program run through `exec`, so git hooks run under the sandbox too; Axon's parsing and
    conventional commits are ported, not `git2`). Crates: `pcx-sandbox`, `pcx-tools-fs`,
    `pcx-tools-shell`, `pcx-tools-git`.

- **Everything gRPC** (W23, owner 2026-10-25; rule 10 of section 2): a module's API is gRPC
  services with typed messages, not JSON tools, and it replaces the JSON tool path of W8 to
  W22 everywhere.
  - *A module's API*: a first-party module's proto package is in `post-cortex-proto`
    (`pcx.chief.v1`, `pcx.team.v1`, `pcx.board.v1`, `pcx.skills.v1`, `pcx.tools.fs.v1`,
    `pcx.tools.git.v1`, `pcx.tools.shell.v1`); a third-party module ships its own. Its
    `Manifest` names its services and carries their `FileDescriptorSet`. Each RPC takes and
    returns its own messages; its handlers are the generated tonic service traits over the
    module's capability service, and they only map messages to typed calls and typed results
    or errors back.
  - *Memory*: `pcx.v1` returns messages for every RPC; the RPCs that answer JSON in a `string`
    today (`TextResponse`: structured summary, key decisions, key insights, entity importance,
    entity network, session statistics, vectorization stats, graph view) get their own
    messages. The documents, sessions and messages modules use (`manage_document`, `session`,
    `manage_workspace`, `manage_messages`) are `pcx.v1` RPCs with messages.
  - *Calls between modules*: through generated clients over the module's host channel
    (`HostContext.channel`, a tonic `Channel`); the host routes a call by its gRPC path to the
    module that serves the service, under the same uses grant, caller, call depth, deadline and
    turn token as W14 and W22a, carried in metadata (`x-pcx-*`), never in the message; the keys only the host sets (caller,
    call depth, turn) are removed from every call a client makes from outside the host. The
    host of an organisation (a hosted tenant, started with its slug) names it as the tenant
    (`x-pcx-tenant`) of every call it routes and every agent client's tool call, whatever the
    call carried; a host of none (a daemon, a runner) passes on the tenant of the call it
    serves, so a runner's calls from its organisation's host keep it (owner 2026-11-08). An
    in-process module's services are served in process; a process module's on its socket;
    the host forwards the call's frames by path without decoding them. Every routed call,
    a client's or a module's, runs under the serving module's policy (T4) as a tool call
    does (owner 2026-10-26): its deadline, a slot of its concurrency, its panic caught, its
    breaker, counted in its health and its log; an answer whose `grpc-status` is not OK is
    a failed call, forwarded to the caller as it came.
  - *A service several modules serve* (owner 2026-11-13: Telegram and Discord both serve
    `pcx.channel.v1.Channel`): a call names the module it goes to (`x-pcx-target`, a module's
    id, set by the caller's client, as `RunTurnRequest.agent` names whose turns), which must
    serve the service, else `NotFound`; without it the call goes to the one module that serves
    the service, and is refused (`FailedPrecondition`, naming them) when several do. The
    target is checked as any callee: the caller's `uses`, depth, reach. A tool's call names
    its module as the target. An RPC's path names one tool (a workflow's `tool` block and the
    catalogs name a tool by its path): of the modules that serve a service at most one names a
    tool on an RPC of it, else the load is refused (`DuplicateToolPath`); Telegram's `Send` is
    the tool `channel_send`, Discord's `Send` is no tool.
  - *Views*: a `Source` and an `Action` name an RPC by its gRPC path, as `Manifest.rpcs` names
    it (`/pcx.chief.v1.Chief/ListTalks`), and a request template in the proto JSON mapping
    (proto field names, `{{row.x}}` and `{{form.x}}` as before); the app builds the request
    with the module's descriptors (a dynamic message; `GetViewResponse.descriptors`), calls it
    through the host or the gateway as any gRPC call, and reads the response by a JSON Pointer
    into its proto JSON mapping. A field a component names (a row's, a column's, a form's
    values, a substitution) is a path of proto field names joined by `.` (`skill.name`), and
    a form's fields nest as the request's messages do (a group per message field). The view
    check (`pcx_module_api::view::check`, the one the host and the cloud run) refuses an RPC
    the manifest does not name, a template key that is no field of the request message (at
    any depth) and a pointer token that is no field of the response. An instance serves the
    running modules' services on every gRPC path its own services do not take, behind its
    check, through the host's channel of a client (`post_cortex_transport::modules`). An HTML
    view's bridge stays on the module's tools (MCP Apps, `GetViewResponse.tools`). `Invoke`
    and `ToolOutput.data` are removed.
  - *MCP (agents)*: the one JSON edge. The manifest names each RPC's tool
    (`Manifest.rpcs`: the method's full path, its tool name, `team_create`, and its visibility,
    `model`, `app` or both, as W12's; its description is the RPC's comment in the descriptors,
    built with their source info), which the host reads for the visibility and grant checks of
    W14 and W22a. The MCP transport lists one tool per RPC an
    agent may call, its input schema from the request message's descriptor, and converts the JSON of a call and of its
    answer with the canonical proto JSON mapping (`prost-reflect`, proto field names), in one
    place: the host's tool bridge (`pcx-host`, `rpc_tools`), which the MCP transport lists and
    calls through, and a model's tool call (the agent's call through its handle) reaches a
    module the same way; the bridge sends the call through the host channel of its origin
    (client, agent or module), so the route checks it as any call by path. An RPC whose
    `Rpc.tool` is empty is no tool: no agent or model sees it; modules call it under their
    uses (memory's documents, sessions, records and messages). The tools of the modules that
    run are built from one pool of all their descriptors (`rpc_tool::pools`), in the host and
    in a module's handle, so an answer that carries another module's message (a run's event,
    whose payload is a `google.protobuf.Any`) maps to JSON; a module with a file of a name
    another has with other content keeps a pool of its own (a pool holds one file of a name),
    and no other module's message is read through its tools.
  - *Settings*: a module's configuration is a message of its package (`Manifest.config_type`,
    its full name in the module's descriptors; empty: it takes none) instead of a JSON Schema
    and JSON. The settings form is built from that message's descriptor: its fields in order,
    each field's comment its description, and the field option `pcx.module.v1.setting`
    (`Setting`): `title`, `required` (refused unset), `secret` (written, never read back) and
    `workspace` (a string naming a workspace of the host by its id; the form offers them) and
    `board` (a string naming a board by its id, of the workspace a `workspace` field of the same
    message names, else of the form's; the form offers its boards; owner 2026-11-01). A
    message field is a group of settings, a repeated field a list, an enum its choices; a map,
    `bytes`, a oneof and a well-known type are no setting, and a secret is a string outside
    any repeated field, or inside an element of a list whose message has one `key` field
    (`Setting.key`, the element's name; the list in no other list; `modules/http.md`, the
    host's change), its path naming the element (`connections[github].secret`):
    the manifest check refuses the others (`pcx_module_api::ConfigType`, the
    one check the host and the cloud run). `StartRequest`, `HostContext`, `ConfigureModule` and
    `ModuleInfo` carry the configuration as `google.protobuf.Any` of that type, `ModuleInfo`'s
    with its secrets cleared and the paths of the secrets set (`secrets_set`, field names
    joined by `.`). A change replaces the configuration but its secrets: a secret it leaves
    unset keeps its value, one it names in `ConfigureModuleRequest.clear` is removed. A
    configuration file (`daemon.toml`, the cloud's store of an organisation's settings) stays a
    file, read into the message by its descriptor (the proto JSON mapping of its section) when
    the host loads it, and written back the same way (owner 2026-10-26: the cloud keeps the
    JSON form with its secrets sealed apart, as before). Two modules never serve one service:
    the host refuses the second (`HostError::DuplicateService`), so a path names one module. A
    hook script has no package and takes no configuration (its command, arguments and
    environment are its settings). A refusal names the setting at fault in
    `ModuleError.field`; `ModuleError.details` (JSON) does not cross the wire (memory's MCP
    tools keep it for MCP clients).
  - *Documents*: a kind's data is a message of its module (`DocumentKind.type`) instead of a
    JSON Schema and JSON; memory keeps it as `google.protobuf.Any` of that type (its type name
    and bytes) and checks it against the type the writing module declared; a data-field filter
    names a field of that message (a filter is an example message of the kind's type and the
    names of its fields that must be equal). Documents written as JSON before W23 are converted
    once by a migration (export format 1.7.0), which the composition runs when the host has
    started the modules that declare their kinds; one whose kind no running module declares
    stays JSON, and gRPC refuses it, until a later start converts it.
  - *The check*: `grpc_json_fields` in `scripts/check-architecture.sh` counts the JSON-shaped
    proto fields (a `Struct`, `Value` or `ListValue`, a `*json*` string) that are not a JSON
    edge named in `scripts/grpc-json-edges.txt` (a model's tool calls and tools, the hook JSON
    of Claude Code and Codex, a view's request templates and form fields), and
    `grpc_json_handlers` any JSON in a module's gRPC handlers; their budgets only fall, to 0.
  - *Not changed*: the slots' ports already typed (`Model`, `ContextEngine`, `pcx.agent.v1`,
    `Sandbox`), events (`Event` messages), views' trees (`pcx.view.v1`).
