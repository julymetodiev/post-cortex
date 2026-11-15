# A connected daemon

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 0. Planned (the `pcx`
board, P1); the decisions are the owner's of 2026-10-27, on the draft of the member `dev`.

A daemon on a laptop or a server joins an organisation of the cloud. It opens one **outbound**
connection to the gateway (no open port, no public address, no certificate of its own), and the
organisation's people reach it through the cloud from the web app, the desktop app's cloud side
and mobile. One tunnel serves two uses:

- **A space** (first): the daemon's board, memory, team and agent, which stay on its machine;
  the cloud forwards the app's requests to it. When the daemon is off, its space is offline.
  For full privacy and for operations (the ops daemon on ml).
- **A runner** (second, section 7): the board, memory, team and agent are a hosted tenant's; the
  daemon is where their tools run (`tools-fs`, `tools-shell`, `tools-git`), under its sandbox
  and grants, its credentials never leaving it.

The direct desktop mode stays as it is: a daemon reached by the desktop app without an account
(free). Connecting adds to it and changes nothing a local client sees.

**Where a person's memory lives** (owner, 2026-10-31): three choices, offered to the person.

| Choice | The memory | Reached from | Client |
|---|---|---|---|
| 1. With them | a connected daemon's space, on their machine; the cloud forwards | web, mobile, desktop (and their organisation) | web app, desktop |
| 2. With us | a hosted tenant in the cloud | everywhere, also when their machine is off | web app, desktop |
| 3. Self-hosted | their daemon, not connected; nothing of it reaches the cloud | the desktop app only, directly | desktop only |

The web app (and mobile) offers 1 and 2 only: a browser cannot reach a daemon on the person's
machine (`localhost`). Choice 3 is the desktop app. A runner (section 7) is not a fourth choice:
with 2 a person adds one for the tools; with 1 and 3 the tools are on their machine already.

- **The default is 2** (with us): the choice a new person gets unless they pick another.
- **The desktop app's first start asks** where the memory lives, with the three choices.
- **The desktop app bundles the daemon** (owner, 2026-10-31): choices 1 and 3 need no other
  install. The app starts its `pcx` as a login item, so the daemon runs while the app is closed
  (a space stays reachable, the clients' hooks keep writing); it offers to put `pcx` on the
  PATH for the CLI and the plugins; a daemon already running (installed otherwise) is used
  instead of a second one, both on `~/.post-cortex`; the app and its daemon update together
  (one version), signed and notarized with the app.
- **Self-hosting the cloud itself** (pcx-cloud with its database and gateway, not a daemon) is
  not offered: the site asks whoever needs it to contact us, and it is built for them then.

## 1. Connecting: a device flow

1. `pcx connect [--cloud <url>]` (default `https://api.pcx.is`, `docs/ARCHITECTURE.md` section 9) asks the cloud for a
   pairing (`pcx.tunnel.v1` `Pairing/StartPairing`: the daemon's name, the machine's host name, its
   version, its workspaces with their names) and prints a short code and a link. Nothing secret is
   shown or copied.
2. An owner or admin of an organisation opens the link in the app (signed in), sees the daemon's
   name, machine and code, chooses the organisation and **which of the daemon's workspaces join
   it**, and approves (`pcx.account.v1` `ApprovePairing`, the cloud's). The others stay local:
   nobody in the organisation sees them. The approval is audited.
3. `pcx connect` polls (`Pairing/PollPairing`, at the interval the cloud gives, within 10 minutes)
   and receives the daemon's id and its token, once: `pcxd_` and 32 random bytes in base62, kept
   only as its SHA-256 by the cloud (as `pcx_` keys and `pcxs_` sessions are). The daemon keeps it
   in `~/.post-cortex/connect.json` (mode 0600: the cloud's URL, the daemon's id and organisation,
   the token) and connects at its next start and from then on at every start.
4. Ending it: `pcx connect --off` tells the cloud (`Tunnel/Leave`) and removes the file; an owner or
   admin revokes it from the app (`RevokeDaemon`), which closes the tunnel at once. Both are
   audited. The workspaces the space offers change from the app (owner or admin), audited.

The control plane gains a capability **Daemons** (pairings, the daemons of an organisation,
their workspaces, revocation; ports `DaemonStore`, `Clock`; table rows: id, organisation, name,
machine, version, the token's hash, the workspaces offered, created, last seen, revoked).

## 2. The tunnel: `pcx.tunnel.v1`

`crates/post-cortex-proto/proto/pcx_tunnel.proto`, public with the daemon; the daemon is the
client of its two services, the gateway their server: `Pairing` (section 1, no bearer: the
device code is the secret) and `Tunnel/Serve` (one bidirectional stream, the token as the
bearer of the call).

- **Exchanges**: each request of the app forwarded to the daemon is one exchange, numbered by
  the gateway: `Open` (method, path, headers), `Data` (body bytes), `End` (trailers), and
  back `Head` (status, headers), `Data`, `End`; either side sends `Reset` (why) to end one
  early; a canceled request of the app is a `Reset` the daemon turns into a dropped call.
  Streaming calls (Export, Import, a turn's stream) are exchanges like any other.
- **Bytes forwarded unchanged**: the gateway forwards a request's bytes to a tenant today
  without decoding them, and to a daemon the same way; the tunnel's own messages are typed
  protobuf (rule 10). Nothing in the tunnel reads a body.
- **Flow control**: at most 256 exchanges open; each side sends at most a window (256 KiB) of an
  exchange's body bytes not yet granted, and `Credit` grants more as the receiver consumes
  them, so one slow exchange does not stall the others.
- **Liveness**: `Ping` every 20 s each way; three missed and the tunnel is closed. The daemon
  reconnects with backoff (0.5 s doubling to 60 s, with jitter). A daemon has at most one
  tunnel: a new one replaces the old, whose open exchanges are reset.
- **Hello**: the daemon's first frame: its version, what it offers (`space` now, `runner`
  later) and its workspaces now (kept by the control plane, for an owner to choose from). The
  gateway answers `Welcome`: the daemon's id, the workspaces its space offers, the window and
  the most exchanges open at once.

## 3. The gateway: a space as a tenant

The gateway's pipeline stays as it is (`post-cortex-cloud/docs/ARCHITECTURE.md` section 6):

- **Authenticate**: unchanged. A request for a space names it with `x-pcx-space` (the daemon's
  id) beside `x-pcx-org`.
- **Classify**: unchanged.
- **Admit**: the daemon must be the organisation's and not revoked. The person's workspace scope is
  narrowed to the workspaces the space offers (`WorkspaceScope::Only`), so the same access
  check and the same filtering of lists as for a restricted grant apply. Metering counts the
  organisation, as for its tenant (the plan's limits for connected daemons: the board's P5).
- **Route**: a connected daemon is a `TenantInstance` (`pcx-tenant` ports): `send` is an
  exchange over its tunnel, `stop` closes nothing (the data is the daemon's), `modules` are the
  daemon's from its `Hello`. Its `TenantId` is the daemon's id. A daemon without a tunnel
  answers `Unavailable` at once, naming it and when it was last seen ("the daemon ops-ml is
  offline (last seen ...)"); the gateway queues nothing. A gateway that stops closes every
  tunnel first, so each daemon reconnects to the next at once.
- **Record**: unchanged.

A download link of the daemon (an export) is under `/v1/daemons/<daemon id>`, which the gateway
routes to the daemon without a credential, as it does a tenant's under `/v1/orgs/<org>`.

**Webhooks** (Alertmanager, a forge) reach a connected daemon through the cloud, so its machine
needs no open port or egress: a source posts to `POST /v1/daemons/<daemon id>/webhooks/<source>`
on the gateway, which carries no credential of the gateway and is not admitted or counted as a
person's call. The gateway sends the body (at most 1 MiB, else 413), its `Content-Type` and the
source's bearer token, nothing else of the request, over the tunnel to the daemon's own `POST
/v1/webhooks/<source>` (`docs/modules/webhooks.md`), whose module checks the token as it does
locally, and answers what the daemon answered. A daemon whose tunnel is not open answers 503
(502 when the tunnel breaks mid-call), so the source retries; nothing is queued in the cloud.
The daemon's route is the same whether it is a space, a runner or both.

The gateway strips the person's credentials as it does for a tenant and sets `x-pcx-author`,
the person's role (`x-pcx-role`, docs/host.md) and the person (`x-pcx-person`, docs/host.md:
the space answers that person's user card);
the daemon never sees a session, a key or the organisation's other data.

## 4. The daemon

- The tunnel client (`pcx-tunnel`, `daemon::run`) is an adapter of the daemon, started by its
  composition (`post_cortex_daemon::daemon::connect`) when `connect.json` is beside
  `daemon.toml`; `pcx connect` asks the running daemon for its workspaces and keeps the
  pairing, and the daemon's next start opens the tunnel.
- **The routes of a hosted tenant**: each exchange is served, in process (no socket), by
  `compose::routes` as a hosted tenant's are: no bearer of the daemon (nothing outside the
  process reaches this router, and the gateway has authenticated the caller), any `Host`, the
  browser routes behind the gateway with download links under `/v1/daemons/<daemon id>`, and
  no module configured through them. MCP, REST, gRPC and gRPC-Web answer as they do locally.
  The daemon's own ports and their `auth_token` are unchanged.
- **No tool modules to the organisation's people** (owner, 2026-10-28): a space's exchanges
  reach no service of `tools-fs`, `tools-shell` or `tools-git` (`PermissionDenied`), nor their
  MCP tools; the daemon's own agents use them, under its grants. A person runs nothing on the
  daemon's machine through the cloud. The composition marks the tool modules' policy local
  only (`Policy.local_only`); the host refuses a client's call to one (gRPC and MCP alike, in
  `ServiceRoute::target`) when it carries a reach, which a call from the cloud always does (the
  tunnel sets it); a module's call (an agent's turn) is not a client's. A runner's exchange
  (section 7) is served without the reach its tenant's turn carries.
- **Flow control on the daemon**: a request whose answer comes before its body is read (a
  refusal) is drained, so the answer still ends well; on the gateway an answer nobody reads
  any more resets the exchange, which stops the daemon's call.
- A refused token (revoked) stops the daemon trying; any other break is retried with backoff.
- Its work does not depend on the tunnel: the team's turns, the scheduler, webhooks and
  channels run on the daemon whether or not it is connected. A broken tunnel only fails the
  app's requests in flight.

## 4a. Reach: what a call may reach

- **`x-pcx-reach`** (`pcx_module_api::metadata::REACH`, in `Scope.reach` and the wire's
  `Scope.reach`): the workspaces a call may reach, comma-separated (`none`: no workspace);
  absent, every one. Only a trusted forwarder sets it, over whatever the client sent: a gateway
  for a grant limited to some workspaces, and a connected daemon's tunnel for every request to
  its space (the workspaces of the gateway's `Welcome`), so the daemon does not trust the
  gateway to have set it. A module's calls in a scope carry the scope's reach, so whatever a
  person's call makes a module do in memory carries it too.
- **The memory instance holds it**, by one rule (`Memory::within_reach`, `post_cortex_memory::
  reach`), on every way in: a call is read as the operation table sees it (`classify`, beside
  the table in `post_cortex_memory::tools::operations`) and refused when it names a workspace
  outside its reach, a session of no workspace in it (but a new, empty session joining a
  workspace in it), a project placed outside it (or one that would make a new workspace), or
  every workspace (but making a new session, which reaches nothing yet, the instance's health,
  and a person's own user card, read or written by a call for that person, `x-pcx-person`;
  `Classified::reaches_nothing`, `joins_workspace`, `user_card`, which a gateway reads too).
  The ways in:
  - gRPC (`post_cortex_transport::reach::Reached`, in front of `pcx.v1`, its blobs and
    `pcx.views.v1`, served to clients and to the modules), its lists of workspaces and
    sessions answered within the reach;
  - REST (`reach::rest` on `/api/*`), its lists likewise;
  - the browser transfer's uploads (`reach::browser`);
  - a memory tool, from MCP or an agent's turn (`MemoryModule`, the scope's reach): MCP takes
    the reach of its request.
- **A turn acts within the reach of whoever woke it**: a document's write keeps the reach of
  the call that made it (`Writer.reach`), the memory-write feed gives it to the modules that
  follow the write (the event's scope), and the team runs the turn it wakes within it (`Wake.
  reach`); the agent's calls carry the turn's scope.
- **A member's reach** (owner 2026-11-13: the agent that answers strangers in a community must
  not read the organisation's other memory; the limit lives in the host, tested end to end): a
  member of the team may name the workspaces it reaches (`team/agent` `reach`; none: as before).
  Its turn's scope carries the narrower of that and the reach of whoever woke or asked it (the
  workspaces in both; one of them unset, the other), so a member's turn reaches no more than
  either allows, wherever it was asked from (the board, a person's talk, a workflow's `agent`
  block), and it runs in a workspace of that reach (else `FailedPrecondition`). The host keeps
  the reach a turn was asked with beside the turn's token and holds every call the agent makes in the turn
  to it (the call's `x-pcx-reach` is the workspaces both name, set by the host as it sets the
  caller and the depth), so nothing said to the model widens a reach. The memory instance then
  holds the turn's calls to it by the rule above.
- **A turn a client asks acts within the reach of its request**: the agent's `RunTurn` takes
  the `x-pcx-reach` header into the turn's scope (`post_cortex_transport::agent`), as MCP does;
  its conversation is a session of a workspace in the reach (the app starts one as a new
  session added to the workspace it is asked from).
- So the gateway lets a grant limited to some workspaces call a module's gRPC service, a gRPC
  list, an agent's turn (`RunTurn`, `CancelTurn`) and a module's tool over MCP: the memory
  instance holds them to the reach.
- **A module's data and the reach** (owner 2026-11-10: a person limited to some workspaces uses
  every module; prod, an Admin of one workspace was refused Chief, API, Workflows and Scheduler,
  whose data all lay in the one workspace their `workspace_id` setting names). Two kinds:
  - *Work of a workspace* is kept in the workspace a call is asked from (`Scope.workspace_id`;
    asked from none, the one the module's `workspace_id` setting names): the scheduler's jobs and
    runs, the workflows with their runs and state, the chief's conversations and actions. The
    caller's reach holds the call as it holds any call to memory, and the module refuses a
    workspace outside it before it reads in its own name (`Scope::reaches`). What the module
    does by itself (the jobs due, the runs left open, the events heard) it does in each
    workspace (`pcx_documents::every_workspace`).
  - *The organisation's store* stays in the one workspace the `workspace_id` setting names,
    whoever calls and from wherever: the http presets and log, the mail templates and log, the
    skills, the dynamic modules, ops' actions, the chief's daily budget. Every caller reads it and
    uses the module, and the module writes its own records there (a call's log), without the
    caller's reach (`Scope::in_own_store`): the reach holds a person to their workspaces'
    memory, not a module to its store. A caller's change of the store (a preset, a template, a
    skill, a dynamic module) is the caller's write and needs that workspace in their reach
    (`pcx_documents::SharedDocuments`), as before this rule: whoever may write in that
    workspace, not only who manages the whole organisation (owner 2026-11-10); so a person limited to other workspaces sees the log of
    what the organisation's people sent and called, and changes nothing of it.

## 5. The app

- **A daemon is a space of its own** (owner, 2026-10-27), beside its organisation's in the
  switcher: `/<org slug>~<daemon>/...`, where `<daemon>` is the daemon's slug (`ConnectedDaemon.slug`).
  The cloud makes it once, when the pairing is approved, by the rule of workspace slugs
  (`post_cortex_core::slug`: its name's lowercase letters, digits and dashes, at most 48;
  `daemon` when nothing is left), followed by `-` and the first six hex digits of its id when a
  live daemon of the organisation has that slug already; it is kept while the daemon is live, so
  revoking another never moves its address. The app never makes one. The cloud
  extension lists the organisation's live daemons (`pcx.account.v1` `Daemons/ListDaemons`) and
  resolves a space to its organisation and daemon from that list; a request in it carries
  `x-pcx-org` and `x-pcx-space` (the daemon's id). Its pages are the harness's (board, memory,
  team, the chief); the organisation's own pages (team, connections, usage) stay in the
  organisation's space. An offline daemon's space says it is offline and when it was last seen.
  A daemon whose space offers no workspace (a runner only) is not in the switcher; a space's
  address without a workspace (`/<org slug>~<daemon>`) opens its first workspace.
- An owner or admin approves a pairing (the link of `pcx connect`), changes the workspaces a
  space offers and revokes a daemon: Connections lists the organisation's daemons (machine,
  version, online now or when it last connected, what its space offers, the workspace its
  runner serves). Online is whether the gateway holds its tunnel now (`ConnectedDaemon.online`).
- The desktop app's direct connection (`~self`) is unchanged.
- **In the plans** (owner 2026-10-28): the organisation's live daemons count against its plan
  (Free 0, Pro 1, Team 5; post-cortex-cloud `docs/ARCHITECTURE.md` C15); an approval over the
  limit is refused, and the directly connected desktop app needs no plan.

## 6. What the cloud keeps

- **Kept**: the token's hash; each daemon's name, machine, version, organisation, the
  workspaces offered, online and last seen; usage counts; the audit of pairing, approval,
  changes, revocation and connections.
- **Never kept**: records, messages, documents, embeddings, files, module secrets, credentials.
  Requests and answers pass through the gateway's memory only, in transit; nothing logs a body
  (a request's method, size, author and outcome only). End-to-end encryption of the bodies is
  not in the first version.

## 7. A runner

The same tunnel, the other way of using it (owner, 2026-10-27): the board, memory, team and
agent are a hosted tenant's; the daemon is where its tools run, in the daemon's sandbox and
under its grants, as GitHub's self-hosted runners. A daemon may offer both a space and a
runner.

- **What it offers** (`daemon.toml`, `[runner]`): the tool modules it lends (`modules`, of
  `tools-fs`, `tools-shell`, `tools-git`, each also a `[[builtins]]` entry with its grants and
  filesystem roots) and the paths no module of the daemon reaches (`deny`, below). `Hello`
  offers `runner` with the manifests of those modules (`pcx.module.v1.Manifest`), which the
  control plane keeps with the daemon (no secret is in a manifest).
- **One runner per organisation** (owner, 2026-10-28): the host routes a gRPC path to the one
  module that serves it, so a second runner lending the same module would collide; a second
  waits for a call that names its runner.
- **Given to a team's workspace** (owner, 2026-10-28; approved in the app, owner or admin,
  audited, as the space's workspaces are): the approval of a daemon that offers a runner names
  the organisation's workspace it serves (its team's: a team lives in a workspace). Changed or taken back later
  in the same place; never by an agent, which cannot raise its own reach (`docs/modules/
  team.md`). A board's repository (`team.md`, a worktree per task) is a path on the runner.
- **Paired as a runner**: `pcx connect --runner` says the daemon offers one
  (`StartPairingRequest.offers`), so the approver chooses its workspace when approving.
- **Pushing a task's branch** (owner 2026-11-01): `tools-git`'s `git_push` (`Push`, effect
  `external`) sends one local branch to the remote's branch of the same name, never forced,
  and only when the runner's operator configures it in `daemon.toml` (`[[builtins]]
  tools-git`, `config.push`; unset: never): `remote` (default `origin`), `branches` (globs of
  a branch name; default `task/*`, the branches a task's worktree works on, `docs/modules/
  team.md`), `ssh_auth_sock` (the socket of the ssh-agent that holds the key: the person's
  own agent, or one the operator runs with a deploy key), `known_hosts` (else a host's key is
  accepted the first time). The key never enters the sandbox: git's ssh reaches it only
  through the agent's socket, whose directory is one of the module's roots; a push needs the
  `network` grant. Protect the main branches on the git host as well (a deploy key that may
  not push them): the configured globs are pcx's own check, the host's is the second.
  Pushing over HTTPS with a token is not offered: the token would be in the sandboxed
  process, where a repository's hook could read it.
- **Its directories** (P3, owner 2026-10-28): `Hello.roots` names the directories the lent
  modules' grants reach (each a path and whether a module may write under it: the union of
  their roots), which the control plane keeps with the daemon and Connections shows. A board's
  repository in the runner's workspace is chosen among the writable ones in the app (`UpdateBoard`,
  `docs/modules/board.md`; a path under one of them); the grants themselves are given only on
  the machine (`GrantModule`, `docs/host.md`).
- **In the tenant's host**: for each module a runner given to the organisation lends, the
  tenant's host has a module of that id (`tools-git`) whose manifest is the runner's and whose
  services are a channel over the runner's tunnel (`pcx_host::RemoteModule`: its manifest keeps
  what names its services and asks the host for no grant, its capabilities being the runner's;
  the channel is the gateway's, the tenant's host running in the cloud process that holds the
  tunnels). The host
  routes and governs it as any module (uses, `TurnGrant`, deadline, breaker, log; W22a: a
  team's turns reach it when the team's catalog entry uses it), so an agent calls a runner's
  tool as a local one. Its calls carry the turn's scope (author, workspace, reach) as
  metadata; one whose workspace is not the one the runner serves is refused there (`Denied`).
  It is there while the runner is given, whether connected or not: offline, each call fails
  at once with `Unavailable` (`runner offline`); the gateway queues nothing.
- **On the daemon**: the gateway marks a runner's exchange (`x-pcx-use: runner`, set by the
  gateway only); the daemon serves it from a router of the lent modules' services only (by
  gRPC path, `Manifest.rpcs`), as a client's call of its host, under each module's own
  sandbox and grants, which nothing in the cloud widens. Every other path, memory's included,
  is refused (`PermissionDenied`), so a runner is never a door to the daemon's memory or other
  modules.
- **Denied paths** (owner, 2026-10-27): `[runner] deny`, by default `.env`, `.env.*`, `*.pem`,
  `*.key`, `id_rsa*`, `id_ed25519*`, `.ssh`, `.kube`, `kubeconfig*`, `.netrc`, `.aws`,
  `.git-credentials`, `.npmrc`: names (a file's or a directory's, any component of a path) that the
  daemon's `Sandbox` port refuses to every module, so a credential does not reach a model.
  A name is a glob of one component with `*` and `?` only (so the files' check and the OS
  sandbox's rule say the same); `[runner]` is read at start. A name is refused where a path
  names it or resolves through it; a file a link named `.env` points to keeps its own name's
  reach (the guarantee is the names, not the contents).
  - *Files* (`tools-fs`): a path is refused when a component of it, as the call names it,
    matches; and once opened (the file, or the directory it is written, removed or listed
    in), when a component of where the system says it is (`/proc/self/fd` on Linux,
    `F_GETPATH` on macOS) below its root matches, so a link to a denied name, or through a
    denied directory, is seen through with no window between a check and the use. A listing
    leaves denied names out.
  - *Processes* (`tools-shell`, `tools-git`): macOS: the Seatbelt profile denies reading and
    writing the matching paths under the roots. Linux (owner, 2026-10-28; Landlock only
    grants, it cannot carve a file out of a granted root): the helper (`pcx sandbox-run`) enters a new user and mount
    namespace, mounts an empty file (`/dev/null`) or an empty read-only directory over each
    match found under the roots when the process starts (not searching `.git`, `target` or
    `node_modules`), then applies Landlock and seccomp. A file the process itself makes
    later is its own, not a credential of the person. Where the namespace cannot be made
    (unprivileged user namespaces off), a process is refused: fail closed.
- **What passes through the cloud**: each tool call's input and output (files read, a
  command's output, a diff) in transit, to the tenant's model; nothing logs a body (section
  6). Never: the runner's credentials, environment and denied paths, files no tool touched,
  the sandbox itself.
- **A task whose runner is offline waits**: an offline runner's call fails with its own kind
  (`ModuleErrorKind::Offline`, on the wire `Unavailable` marked `x-pcx-offline`), which the team
  reads as no try spent: the task keeps its column, is labeled `waiting-runner` and the team
  comments once; it is not `blocked`. A member's work is kept as going in `team/work` while its turn runs or waits, so a team
  that starts again goes on with it in the same session, no try spent, and told that its last
  call may or may not have finished (owner 2026-11-04); work on a task closed meanwhile is
  dropped. When the runner connects,
  the tenant's runtime emits `EVENT_KIND_MODULE_AVAILABLE` (`pcx_module_api::ModuleAvailable`,
  the lent modules' ids; no conversation in it, so every subscriber gets it whole), and the
  team takes the label off and runs the waiting work again.
- **Counted** as the tenant's module calls (one per tool call), no metric of its own yet.

## 8. Open

- More than one gateway process: which one holds a daemon's tunnel, and how a request reaches
  it (one gateway today).
- A second runner in an organisation: a call that names its runner (the host routing by module
  and path).
- A runner given to some boards of a workspace rather than all (a turn's scope naming its
  board).