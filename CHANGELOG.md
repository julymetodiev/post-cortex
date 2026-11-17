# Changelog

All notable changes to Post-Cortex will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **A file's memory finds its own history.** The file view and the
  before-edit hook read the files a record names in its `file`, `file_path` or `files` for
  any type, not only a code change's title; a code change's long title is kept whole before
  it is cut. A file's own entities are those named after it, also as an item or member of
  its stem (`Scope::working_in`, `Scope.tenant`, `veil_core::limiter::Limiter`); when
  several files share its name, the records that name it say which are its own, so files
  of one name no longer share one history. The other code its records name is listed as
  changed with it (`changed_with`, a new field of `FileView`), not walked as its
  dependents. Each dependent says why (`- B (depends on A)`).
- **`required_by` is read in its own direction.** `A required_by B` made A a dependent of
  B as well as B a dependent of A; impact analysis, the file view and assembled context
  now list only B.

### Changed

- **One text for the cards and a file's memory in every client.** The JS plugins
  (opencode, kilo, pi, openclaw, the DeepSeek harness) and the opencode template ask the
  memory module for them (`POST /api/events`, a `SessionStart` and a `PreToolUse` event),
  as the Claude Code hooks do, instead of rendering the cards and the file view
  themselves: `PcxClient.cards` answers the module's text, `PcxClient.fileContext`
  replaces `PcxClient.file`, and `cardsText(cards, ...)` and `fileContextText(view, path)`
  are gone from `@post-cortex/plugin-core`. The `[PCX] Session: ... | Workspace: ...`
  line comes before the cards, as the Claude Code plugin prints it.

### Security

- **`candle` 0.11.1** (from 0.9.2) brings `zip` 9.6.1: the five `zip` 7 advisories are gone
  from the ignore lists. `tokenizers` follows to 0.23. `chrono`, `chrono-tz` and `humantime`,
  declared unmaintained with no successor release, are ignored with their reason in
  `deny.toml` and `.cargo/audit.toml`.

## [0.6.1] - 2026-11-16

### Fixed

- **The project cache is kept per key.** `~/.post-cortex/projects.json` entries now carry
  `identity` (the first 16 hex of the sha256 of the bearer token, never the token): another
  organisation's key on the same backend resolves anew instead of writing into the first one's
  session. Ids the backend refuses (403 or 404) are dropped from the cache and resolved again.
  Same in the Claude Code hooks, `pcx` and the JS core.
- **A first agent joins the workspace the app's onboarding made.** A new project key with no
  `workspace` joins the one workspace of the project's name that no key is bound to yet,
  instead of making a second one; with none, or two or more, a new one is made, and a
  workspace another key is bound to is never taken. gRPC `FindProject` takes the `name`.
- **Claude Code's auto memory no longer wins over post-cortex.** The rules and the skill say
  post-cortex replaces it, the Stop hook asks for a write into the auto-memory directory to be
  recorded with `update_conversation_context`, and `pcx setup` says how to turn it off.

## [0.6.0] - 2026-11-15

### Fixed

- **Two writers racing for one document on SurrealDB: the loser gets a conflict** (found
  2026-11-15 by the public CI on Linux). When the loser's transaction failed at its commit,
  SurrealDB answered its statements with "not executed" and put the cause on the commit's own
  row; the adapter read the first error, so the write failed as unavailable where the port
  promises a conflict with the latest version. The adapter now reads the cause; an event
  appended in the same race takes the next position.
- **Two engines started together on a machine without the embedding model: both load it**
  (found 2026-11-15 by the public CI). The second gave up after five seconds on the file the
  first was fetching ("Lock acquisition failed") and answered unavailable. One fetch runs at a
  time in a process; the one that waited finds the model in the cache.
- **The memory-write feed follows writes from when it is made**, not from when its task first
  runs: a write made between the two was never fed to the modules that follow memory.

### Security

- **Dependencies with advisories are updated** (found 2026-11-15 by the first public CI run):
  `rmcp` 3.10.0 (the MCP server and client), `ammonia` 4.2.5, `pulldown-cmark` 0.13.6,
  `libz-sys` 1.1.33, `zstd` 0.13.5. Four advisories stay, each named with its reason in
  `deny.toml` and `.cargo/audit.toml`: three in what SurrealDB 3.3 depends on (fixed in
  SurrealDB 4.0) and one in `zip` through `candle-core`.

### Changed

- **An archived workspace is read-only** (owner 2026-11-15: the docs said so and nothing
  held it). A call that changes memory and names an archived workspace, a session of it or
  a project placed in it is refused (gRPC and MCP `FailedPrecondition`, REST 409) on every
  way in; restoring it and deleting it go on, and so do reads and searches. A daemon reads
  the first message of a call without a reach only when its operation may change memory.

### Added

- **A Discord embed shows pictures, and the template `discord-announce`** (owner 2026-11-14:
  a good-looking announcement of a release, with screenshots of what is new in the app).
  `pcx.discord.v1.Embed.images`: up to 4 https addresses, sent as cards of the embed's link
  (several need the link). `discord-announce`: run with a title, notes, a link and pictures,
  it posts one card in the announcements channel.
- **Discord's template `discord-feedback`** (owner 2026-11-14: the feedback forum of pcx's
  server had no workflow): a new post of the feedback forum is thanked with fixed words and
  the team is told its title and author. No agent reads the post.
- **An agent's answer that is not read is asked for once more, and the help forum answers
  when its agent fails** (found 2026-11-14 on prod: the help forum's agent ended with plain
  text, then with a line break inside a JSON string; each run failed at once and the person
  got no reply). An `agent` block whose answer is not read (no JSON object, no JSON, one that
  does not fit the `Schema`) asks the turn's session once more, told why; not read again, the
  block fails (`answer`, "asked twice"). `discord-help`: the agent's `error` edge replies with
  fixed words that a person will follow, tags the post `needs-team` and tells the team; its
  prompt asks for the page's address as the end of `answer`'s text, not on a line of its own
  (the wording that made the answers unread). `pcx-workflow`: `engine/act/turn.rs`.
- **A setting says what its text must be, and a module says what stops it** (owner
  2026-11-14, after a new organisation set up Discord's help forum on prod: two forum tags
  typed on one line with a comma were saved, and only the running module found them wrong, in
  the server's log). `pcx.module.v1.Setting` gains `pattern` (a regular expression a string's
  text matches whole; of a repeated string, each value) and `example`; the one check of a
  configuration refuses another text naming the setting (`setting tags[0] is like answered
  (it matches [^,]{1,20})`), so the cloud refuses it when it is saved and a host before the
  module starts. A manifest with a pattern on what is no string, one that does not compile,
  or one without an example that matches is refused. Patterns: Discord's ids, channel names,
  roles and tags; Telegram's chat names and API address; the addresses of http, mail, models,
  observe and ops (Nomad's `https` only); mail's and git's addresses; the names of observe's
  and ops's environments and webhooks' sources; ops's Nomad jobs; the destinations ops and
  webhooks tell. What only the outside world knows a module reports once it runs:
  `ModuleHost::report` (`pcx.module.v1.Host` `Report` on a module's callback socket), kept
  in its health (`ModuleHealth.problem`, also in `/health`) and written in its log when it
  changes. Discord reports what a person must fix in its server (the token, a right, a
  role, a tag) and none once the server is read.
- **Settings a module refuses stop neither its organisation nor the truth about them**
  (found 2026-11-14 while checking the above: a module's own check of its settings, a name
  given twice, runs when it starts, after a cloud saved them). A reload that a running
  module's new settings fail leaves it running with the ones before and its health says so
  until they are applied. The host of an organisation goes on without a module, other than
  memory, that refuses its settings or does not start (`Host::left_out` names it with why);
  before, one such module failed the whole load and the organisation's tenant did not
  start. A daemon's host starts all its modules or none, as before. `compose::host` answers
  a `HostError`; `Modules::without` leaves a module process out. A wrong bot token reads
  "the bot's token was not taken (401): set the bot's token ... in the module's
  settings". `team_create` with a built-in agent's name among its members adds the agent
  first, as `member_add` does: the team named a member that was no agent.
- **A workflow is written only when what its agent blocks ask is there** (owner 2026-11-14:
  the template `discord-help` installed in a new organisation failed only at its first runs,
  for a member no team had, then for no agent). `workflow_create` and `workflow_install`
  refuse (`FailedPrecondition`) a member that is in no team of the workspace or is no agent
  of the harness, no module in the agent slot, and no model listed, each saying where a
  person adds it; the refusal without a board names the Workflow module's setting "Board of
  approvals". A run that failed is a warning in the host's log, with its workflow, its id
  and the failure's kind (never its message, which may carry a stranger's text).
- **Discord's help answers end with the page they are from**: `discord-help`'s agent ends
  its answer with the address at the end of the title of the record it answers from, and
  gives no other link. Its notices to the team and its approval's question end with what
  the post tried on a line of its own (they read `..` when the agent's line ended with a
  full stop), and the agent is told not to quote the post there.

- **Discord moderation** (owner 2026-11-14: a person who abuses the help forum is banned on a
  second abuse within 30 days; a first is a person's decision). `discord_timeout` (0 lifts it),
  `discord_lock_thread` and `discord_ban`, only with the settings' `moderation`
  (`pcx.discord.v1.Moderation`: the longest timeout, 1 to 40320 minutes, and whether it may
  ban); without it they are refused and the bot needs no right to moderate. With it the bot
  needs Timeout Members (Ban Members to ban) and Manage Threads where it posts; the invite
  link names them. The help workflow's agent says what a post is (`answered`, `not_in_docs`,
  `abuse`): an abuse gets nothing in public, its post locked, its author banned when the
  workflow's state holds an abuse of theirs within 30 days, else a 24-hour timeout and a
  decision only a person approves; declined, the timeout is lifted and it does not count. A
  post the documents do not answer is told so in the agent's words, in the person's
  language. docs/modules/discord.md. Tests: moderation refused without its settings, a
  timeout within the longest and lifted with 0, a lock only where the bot posts, a ban only
  when allowed, the rights moderation needs (pcx-discord).
- **An approval only a person approves** (owner 2026-11-14). A workflow's `approval` with
  `person` asks its decision with `pcx.team.v1.Stakes.person`, so no trust of the team
  approves it (as ops' revert). docs/modules/workflow.md. Tests: the decision asked carries
  it (pcx-workflow).
- **Workflow templates** (owner 2026-11-14). A module ships workflows for an organisation to
  install: `Manifest.workflow_templates` (`pcx.module.v1.WorkflowTemplate`: a name, what it
  does, the workflow's JSON, its parameters, each with a default and the pattern every value
  matches, put where its strings name `${<name>}`). `workflow_templates` lists those of the
  modules the caller reaches, each with the workflow installed from it; `workflow_install`
  makes the workflow with the caller's values and writes it as `workflow_create` does, its
  grants asked for. Discord ships `discord-help`, `discord-welcome`, `discord-changelog` and
  `discord-automod` (it tells the team when AutoMod acts, without what the member wrote). The
  app lists them on the Workflows page, Install asking for each value. docs/modules/workflow.md,
  discord.md, host.md. Tests: values given or defaulted, a value that breaks its pattern, one
  for no parameter, a template naming a parameter it lacks, refused; a template installed is
  a new workflow whose grants are asked, installed once (pcx-workflow); Discord's four make
  workflows that check against its tools (pcx-workflow with pcx-discord); the app installs one.
- **Each action of a workflow is approved** (owner 2026-11-14: approving a module approved
  every tool of it, so a new version calling `discord_ban` ran under an approval given for
  `discord_reply`). A workflow's grants name the tools of effect `external` its `tool` blocks
  call (`Grants.tools`), found from its graph when it is written; the question names them and
  a new one asks again. A workflow approved before is asked again once when the module
  starts. The app reads them for its approval and its question, and keeps an approval's
  `person` when the editor saves it. docs/modules/workflow.md. Tests: an external tool asked
  for once, a new one asks again, a reading one does not (pcx-workflow; the app's model).
- **The agent of a community searches its workspace** (found 2026-11-14 in the Discord help
  forum: it answered "not in the documentation" when the answer was there). The built-in
  `community` named memory's tools by their MCP names, so its turn was given none; it now has
  `memory_search` and `memory_context`, the names a turn has them by. A person's talk with a
  member (and a workflow's `agent` block naming one) never told the member its workspace,
  and memory refuses a search that names none to a turn held to some workspaces; its
  instructions now name it. A member held to modules without the board is told nothing of
  the board or its threads (mentions, links to records), since what it says leaves the app.
  docs/modules/team.md. Tests: the community agent is given exactly memory's reading tools
  (daemon, a real host); a talk names its workspace; a member outside the board is told
  only its role (pcx-team).
- **Tools read another module's messages** (found 2026-11-14: `workflow_runs` failed with
  "message 'pcx.discord.v1.PostCreated' not found" for a run a Discord event started). A
  tool's answer was mapped to JSON with its own module's descriptors only, which lack a
  payload `Any` of another module. The tools of the running modules are now built from one
  pool of all their descriptors (`rpc_tool::pools`), in the host and in a module's handle; a
  module with a file of a name another has with other content keeps a pool of its own.
  docs/host.md (tools). Tests: a run with a Discord payload maps to JSON through the
  workflow's tool; a differing file keeps its own pool (pcx-module-core).
- **Discord help writes nothing to memory** (owner 2026-11-14): a stranger's question kept in
  the workspace the agent reads would be a prompt injection read at every later question.
  An organisation keeps its community's public workspace in an organisation of its own;
  pcx's server goes in `PCX Community`. docs/modules/discord.md.

- **Workflow: an event's payload is read** (found 2026-11-14 by the first post in the Discord
  help forum). An `event` trigger's filter that read the payload (`event.payload.channel ==
  "help"`) passed validation, the payload being an `Any`, and failed at each event with
  "field not found": the filter was compiled with the workflow module's own messages only
  (the board's), and the payload's type URL (`/<name>`, as the modules write it) is not one
  CEL unpacks. The filter is now compiled and the event read with the messages a run of the
  workflow reaches (`Engine::fires`), and an event heard has its payload's type URL written
  as `type.googleapis.com/<name>` (`domain::as_heard`), so a filter, a `wait` and
  `trigger.event.payload` read its fields. docs/modules/workflow.md (event). Tests: a filter
  reads the payload of an event of a module the workflow reaches (pcx-workflow).
- **Discord** (owner 2026-11-13: a module of the product, each organisation its own server
  and bot). New crate `pcx-discord`, built in (`id = "discord"`): it serves
  `pcx.channel.v1.Channel` beside Telegram (`notify = "discord/<name>"`) and
  `pcx.discord.v1.Discord` with the tools `discord_post`, `discord_reply`,
  `discord_give_role` and `discord_tag_post`, each only for the channels, roles and forum
  tags its settings name; it listens on Discord's Gateway and publishes
  `discord.post_created`, `discord.message_created`, `discord.member_joined` and
  `discord.automod_acted` in the workspace its settings name, for workflows' `event`
  triggers. A message it sends mentions no one, a long one goes as several, and its
  start calls nothing outside: its listener reads the server after it, and until then every
  call and `discord_status` say why not, with every right its bot lacks in the server and the
  link that adds the bot with the rights it may need. It is a module process too (`pcx-discord`, for the cloud's
  catalog). The built-in agent `community` of the team (memory's reading tools only, its
  reach the workspace it is added in) answers a community's questions when a workflow asks
  it. A Discord bot's token is redacted (`discord_token`). docs/modules/discord.md.
- **A service several modules serve** (owner 2026-11-13). A call names the module it goes
  to (`x-pcx-target`); without it the call goes to the one module that serves the service
  and is refused when several do. A tool's call names its module. An RPC's path names one
  tool: two modules that name a tool on one RPC are refused at load (`DuplicateToolPath`,
  which replaces `DuplicateService`). A channel's destination is `<module>/<name>`
  (`pcx_module_api::channel`), which ops and the webhooks send to. docs/host.md.
- **A member's reach** (owner 2026-11-13: the agent that answers strangers must not read the
  organisation's other memory). A member of a team names the workspaces its turns reach
  (`pcx.team.v1.Agent.reach`, `UpdateAgentRequest.reach`); its turn reaches the workspaces it
  and who woke or asked it both reach, and runs in one of them. The host keeps the reach a
  turn was asked with and holds every call the agent makes in it to that reach.
  docs/connect.md section 4a.

- **Nomad environments in observe and ops** (owner 2026-11-12, for pcx's own production:
  Nomad with pot jails on FreeBSD). An environment names a Kubernetes API or a Nomad API
  (`nomad_api`, `nomad_token_file`, `nomad_ca_file`, `nomad_tls_server_name`), never both.
  Observe reads it with `jobs_get` (jobs with their version, whether it is stable, their
  allocations and tasks, a periodic job's schedule and launches), `alloc_log` (the end of a
  task's log, the allocation named by the start of its id) and `node_get` (nodes with their
  memory and disks: a device, or a ZFS pool's datasets together). Ops acts on it with
  `ops_new_allocation` (a service job's running allocation stopped, a new one placed; never
  a restart in place), `ops_run_periodic` (a periodic job launched once now) and
  `ops_revert_jobs` (one to five service jobs, each back to its stable version before, one
  decision for all, the versions kept as asked and checked again before it runs). A revert
  is approved by a person only: `pcx.team.v1.Stakes.person`, which no trust approves. A
  Nomad environment of ops names the jobs its actions act on (`nomad_jobs`, at least one);
  an action on any other job is refused, so a database's job stays a person's. The
  actions of one kind of cluster asked of the other fail with `FailedPrecondition`. New
  crate `pcx-nomad` (Nomad's names, paths, settings and its JSON without nulls), over
  `pcx-kube`'s calls. `pcx.observe.v1`: `GetJobs`, `AllocLog`, `GetNodes`; `pcx.ops.v1`:
  `NewAllocation`, `RunPeriodic`, `RevertJobs`, `Action.reverts`, `Action.allocation`.
- **A person declines a decision** (owner 2026-11-11). The board's `DeclineDecision`
  (`decline_decision`, a person's only, with an optional note) ends a decision that waits: it
  stays in its task's thread as declined (`Decision.declined_by`, `decline_note`), holds its
  task no more and cannot be approved, and the board emits `board.decision_declined`. Who
  asked it gives up: an `ops` action is written `declined` and its target is free again; a
  workflow's `approval` step fails with the kind `declined`, so its `error` edge runs; declined
  grants of a workflow and declined files of a dynamic module are kept as declined
  (`Asked.declined_by`) and asked about again only when written again. Telegram's Decline calls
  it (it was a comment, and the decision kept waiting); the board's views have Decline beside
  Approve.
- **A job watched from outside**: `GET /health/jobs/{name}` on the daemon, not behind its
  token, answers 200 while the scheduler's job of that name runs on time, 503 when it is
  paused or late (a time of its schedule passed more than 15 minutes ago and no run since then
  succeeded or still runs), 404 for no such job. `JobHealth` of `pcx.scheduler.v1`, tool
  `job_health`.
- **Heartbeats of a webhook source**: `Source.heartbeats` names the alerts a source only
  acknowledges (Alertmanager's `Watchdog`), answered `HANDLED_HEARTBEAT` with no task, so the
  sender's failed notifications show a broken path while nothing fires.

### Fixed

- **`pcx stop` stops the daemon** (found 2026-11-15 by the review of the public docs: it
  printed three ways to stop a daemon and stopped nothing, and `pcx connect` told the person
  to run it). A daemon writes its process id in `daemon.pid` of its data directory once it
  holds its address and removes it when it stops; `pcx stop` sends that process SIGTERM and
  waits, up to 60 seconds, until it has ended, so its sessions are written and its database
  is free. A daemon that left no file (another data directory, an earlier version) is named
  with how to stop it.
- **The template `discord-help` keeps its kinds when the answered tag is renamed** (found
  2026-11-15): the answer's `kind` allowed the value of `answered_tag` where the word
  `answered` belongs, so an organisation that named its tag otherwise had every honest
  answer refused by the schema and sent down the failure path.
- **The released `pcx` reranks with Voyage** (found 2026-11-15: the release build had no
  reranker, so `voyage_api_key` only logged a warning). `release.yml` and `scripts/cargo.sh
  pcx` build with `rerank-voyage`; the local reranker (`reranker_model`) still needs a build
  with `rerank`, which links ONNX Runtime.
- **The greet module carries the module API it is built with** (found 2026-11-14:
  greet's descriptors held `pcx_module.proto` of 2026-11-01, so its file differed
  from every other module's and its tools read no other module's messages). Its build script
  compiles the module API's protos from outside its package, which cargo does not watch, and
  ran again only when greet's own files changed. Now it tells cargo to run again when either
  directory changes, and so does the bindings' test build (the greet example's protos).
  `scripts/check-architecture.sh` counts a build script that compiles protos from outside its
  package without `rerun-if-changed` (`stale_protos`, budget 0).
- **A workflow's `agent` block fails as its turn failed** (found 2026-11-14 in the Discord
  help forum: Z.AI refused a key without a plan, code 1113, and the step said "the agent's
  answer has no JSON object"). The turn's end was read as an answer whatever stopped it, so
  a refused quota was an empty answer. Now a turn the model did not answer is the step's
  failure of its own kind (a quota `resource_exhausted` with the provider's code, a blocked
  turn `denied`, too many steps `resource_exhausted`, a cancelled one `deadline_exceeded`), and
  an answer without its JSON shows its first 300 characters. Tests: each way a turn ends
  (pcx-workflow).
- **Discord moderates only whom Discord lets it** (found 2026-11-14: a timeout of the
  server's owner answered 403 "Missing Permissions" and the help workflow ended telling no
  one). `discord_timeout` and `discord_ban` refuse the owner, the bot itself and a member
  whose highest role is not below the bot's (`FailedPrecondition`, saying why) before Discord
  is asked; `discord-help` tells the team when a timeout or a ban fails, the post still
  locked. docs/modules/discord.md. Tests: the owner, the bot and a higher member are not
  moderated and Discord is not asked (pcx-discord).
- `http` writes a whole number of a JSON body as an integer (ops daemon 2026-11-12: the
  workflow pcx-probe sent `limit` 1 to `/api/search` as `1.0`, which answered 400 "invalid type:
  float"). The body, a `google.protobuf.Value` whose numbers are doubles, was written as it is
  held; it is now written by the proto3 JSON mapping (`pcx_module_api::json::from_wire`), as
  every other JSON the host writes (docs/modules/http.md, requests).
- `ops` looks every minute at the actions that wait. One whose decision was answered while
  the module was down is settled (it heard only the write of a decision, which is not told
  again: an action approved or declined then waited for ever and the brake held its
  target). One no one answers is said again once on the channel after `remind_minutes` (30)
  and given up after `give_up_minutes` (240): its decision is withdrawn in its asker's name
  and its target is free (`OpsConfig.remind_minutes`, `give_up_minutes`, `Action.reminded`).
- The scheduler reads a job's newest runs only as far as it needs: its last scheduled run
  (every look for due jobs, each 15 seconds), the runs since the time it owes (its health) and
  the runs a listing asks for. It read every run of the job each time, and a job every 5
  minutes writes 288 a day.
- `observe` reads a namespace that holds an event without a last time (ops daemon 2026-11-11:
  for about an hour after a pod was made, `cluster` failed with "Kubernetes's answer is not
  what it sends: invalid type: null, expected a string"). The scheduler writes its events
  with `lastTimestamp: null` and the time in `eventTime`; an event's time is now the first of
  the two that is there.
- An `ops` action that ran on a person's approval is checked: its comment on the task names
  its asker, which wakes that member to look at the target and say whether it helped. On the
  ops daemon (2026-11-11) the action ran, and no one looked or moved the task.
- `ops_resize_volume` reads a claim's size from its volume's capacity when that is more than
  the claim asked for. A claim of 2Gi on a 10Gi volume (a provider's smallest) could grow to
  4Gi at most, which grows nothing.
- A decision that ended elsewhere settles its Telegram message however it ended: declined in
  the app, or withdrawn by who asked it. A withdrawn decision's message kept its buttons, which
  answered that the prompt is gone.

- A workflow's `agent` block that names a team member now runs that member's turn. It went to
  the host's agent, which reads no member, so the turn had neither the member's role and skills
  nor its limits: on the ops daemon `eq-sentinel` (modules `observe`, `board`, memory) reached
  `workflow_run` from the workflow `eq-daily` (2026-11-11). The workflow asks the team for the
  turn (`ModuleHost::turns_of`, new: the turns a module runs in its own name, for a module that
  uses it; `RunTurnRequest.agent` names it on a module's callback socket), so the member's
  modules and tools apply as on the board.
- A workflow's `agent` block without a member (the host's agent) is given the tools of the
  modules the workflow's grants name and no other. It had every tool the agent has, whatever a
  person approved; a workflow whose grants name no module is now refused such a block.
- The approval of a workflow names the team members its `agent` blocks have turns of
  (`Grants.members` in `pcx.workflow.v1`, written by the module from the graph): "Let workflow
  daily run, calling telegram, with turns of eq-sentinel?". A block that names another member
  asks again, and a workflow approved before this is asked again at the module's start.

- Telegram asks about a decision in one message. The board writes a decision twice as it asks,
  and the channel sent each version: two messages with buttons, and after a press on one the
  other answered "This prompt is gone" (ops daemon 2026-11-11). A decision written again while
  it waits keeps its message (edited when it says something else, its buttons approving the
  newest version), writes heard together are acted on one at a time, and a message is settled
  once.

### Removed

- **A scheduler job no longer tells a channel** (owner 2026-11-11: telling
  people is a workflow's). `Job.deliver` (`Delivery`) and `Run.told` are gone
  from `pcx.scheduler.v1` (numbers 7 and 9 reserved), with the `World::tell`
  port and the scheduler's use of the channel module. A job that should tell
  people runs a workflow whose `tool` block calls `channel_send`
  (docs/modules/scheduler.md, docs/modules/telegram.md). No job in use
  named a destination.

### Fixed

- **A person limited to some workspaces uses every module** (prod 2026-11-10:
  an Admin of one workspace was refused Chief, API, Workflows and Scheduler,
  whose data all lay in the one workspace their `workspace_id` setting names,
  outside that person's reach). Two kinds of a module's data
  (docs/connect.md section 4a, owner 2026-11-10):
  - *Work of a workspace* is kept in the workspace a call is asked from
    (asked from none: the setting's). The scheduler's jobs and runs: a job
    belongs to a workspace, the scheduler looks for the jobs due in every
    workspace. The workflows with their runs and state: one engine runs every
    workspace's workflows, at work in the workspace of each call, run and
    event (`Engine::at`, the `Spaces` port; the runs being driven are kept by
    workspace and id); an event starts runs only in its own workspace; the
    approvals of a workspace other than the setting's are asked on that
    workspace's own board `Approvals`, made when first needed. The chief's
    conversations and actions; its daily budget stays one for the host. Each
    refuses a workspace outside the caller's reach (`Scope::reaches`).
  - *The organisation's store* stays in the setting's workspace: the http
    presets and log, the mail templates and log, the skills, the dynamic
    modules, ops' actions. Every caller reads it and uses the module, the
    module writes its own records there, and a change is made by a caller who
    reaches that workspace (`pcx_documents::SharedDocuments`,
    `Scope::in_own_store`); another is `Denied` in words.
  `pcx_documents::every_workspace` (the team uses it too). docs/modules:
  scheduler, workflow, chief, http, mail, dynamic. Tests: a limited person
  reads the organisation's skills and does not change them, a scheduler's jobs
  are those of the workspace asked from, runs of two workspaces go at once on
  one engine, an event starts runs only in its workspace, a reach holds a
  call to its workspaces.
- **A dynamic module no longer leaves a decision waiting that no one needs**
  (found 2026-11-10: after `dynamic_remove` of vat, its version 3's decision
  still waited in Approvals and held its task; approving a replaced version's
  decision did nothing). `dynamic` withdraws the decision it asked
  (`WithdrawDecision`, in its name `dynamic:<id>`) when the module is removed,
  when newer files are asked about instead or the approved files are installed
  again (nothing is then asked), and at start for each older decision of a
  module's task that still waits. docs/modules/dynamic.md (approved). Tests:
  removing withdraws, a version asked about no more, the older decisions at
  start.
- **A workflow's run that ends no longer leaves its approval waiting**
  (found 2026-11-10: a run of a workflow failed past its wall time
  while its decision still waited in Approvals for a person). The board gains
  `WithdrawDecision` (`withdraw_decision`): a decision's author withdraws it
  while it waits, with why; it stays in its task's thread as withdrawn
  (`Decision.withdrawn`), holds its task no more, cannot be approved and is
  not listed. A run that ends while an approval waits withdraws that
  decision and closes the steps it leaves waiting; a workflow deleted while
  its grants wait withdraws theirs; at start the module settles the runs
  that ended last of each workflow, so a decision left by a run that ended
  before this is withdrawn too.

### Changed

- **Every model provider in one module, `models`** (owner 2026-11-09). Its
  settings have a section for each provider (OpenRouter, Anthropic, Z.AI,
  OpenAI): set a provider's API key and every model its API lists is offered
  and served, each with its provider's name, none turned on first; a request
  goes to the first provider that lists its model, waiting for the listings
  begun at start when none lists it yet (the first turn after a restart was
  refused). Breaking: the modules `provider-zai`, `provider-openai`,
  `provider-openrouter` and `provider-anthropic` are gone (the `pcx-provider-*`
  crates are libraries of `pcx-models`): move each one's `api_key` to its
  section of `models`; `ModelInfo.enabled` is gone (its number reserved);
  `ModelInfo.provider` is what the module names (else its id). A module setting may say it
  names a model (`Setting.model`: the form offers every model of the host's
  providers, each with its provider) or holds several lines
  (`Setting.multiline`: a text area); the agent's and the chief's model and
  prompt use them.
- **A user card per person** (owner 2026-11-09). A call carries the person it
  is for (`x-pcx-person`, `Scope.person`, `pcx.module.v1.Scope.person`), which
  the cloud's gateway sets for a person's session or key; the memory answers
  the user card with that person's (`CardScope::Person`, stored as
  `person:<id>`) on MCP, REST, `/api/events` and gRPC, and in assembled
  context; a call for no person (a local daemon) keeps the host's card. A
  full export carries every person's card (`CardStore::card_people`, export
  format 1.10.0, `schemas/export/v1.10.0.json`).

- **Voyage reranks only the workspaces that allow it** (owner 2026-11-06). A
  workspace has `remote_rerank`, off when it is created and for every existing
  workspace (RocksDB data version 6, SurrealDB data version 5, export format
  1.9.0); `pcx.v1` `UpdateWorkspaceRequest.remote_rerank` changes it and
  `WorkspaceInfo.remote_rerank` shows it. An accurate search sends the query
  and candidates to Voyage only when every workspace of its scope allows it (a
  session in no workspace never), else it uses the local reranker. A
  `voyage_api_key` in `daemon.toml` makes Voyage available, no longer on: allow
  it per workspace.
- **Every search answer says whether a reranker ordered it**: `reranked` in MCP
  `semantic_search`, REST `POST /api/search` and `pcx.v1 SemanticSearchResponse`
  (as in `views` `SearchResponse`), from `Search::find`. The operation table
  names it (`Rerank::answered`), so a gateway counts a reranker call from the
  answer, not from the mode asked for.
- **Claude Code uses post-cortex only through the plugin** (owner 2026-11-06).
  `pcx setup --project` pins the ids in `.post-cortex.json` and, for Claude Code,
  writes nothing else: it takes out what setups before the plugin wrote (the PCX
  section of `CLAUDE.md`, post-cortex's hooks in `.claude/settings.json` and their
  scripts in `.claude/hooks/`, `~/.claude/agents/post-cortex-agents/` and
  `~/.claude/skills/post-cortex-memory/`), and prints how to install the plugin.
  The templates only it used (`templates/CLAUDE.md`, `templates/agents/`) are gone.
- **The Stop check covers each file**: every project file a turn edited must be
  named in a record made after its last edit; one summary record no longer covers
  the rest. With `--remind`, the same script is the plugin's `PostToolUse` hook of
  the edit tools: past 8 files edited and not recorded, each new one adds a reminder
  naming them to the model's context.
- **Document keys and pages** (task `f47c5546`): memory keys each document's top-level
  singular scalar fields and storage pages the latest versions newest first, by kind
  or by keys, so a filtered or long list reads only its page (RocksDB data version 4,
  SurrealDB data version 3; unkeyed documents are keyed at start). `ListWork`,
  `ListTalks` and `ListActions` take a `limit` (0: 50); a filter on a field that is
  not a singular scalar is refused.

### Added

- **The Stop check sees more than an edit.** It also blocks a turn that ran any
  shell command (post-cortex's own `bash` and `tmux` too), edited through an
  MCP tool (Mimir's or post-cortex's own), or
  was opened by a background task ending, with nothing logged after: a
  measurement changes no file. It still asks once, and one line that there is
  nothing to record ends it.
- **Members write for people.** A member answers in the language of the task
  or comment it answers, cites memory as a link by its title
  (`[title](pcx:record/<id>)`, which the app opens) and mentions a member only
  for it to act; a member's answer no longer wakes the member it answers,
  so two agents do not keep waking each other. A list in a view may show its
  titles as Markdown (`List.markdown`): the board's comments do.
- **A search finds, a read takes.** MCP `semantic_search` shows a long
  result as its passage around the query's words (`detail` snippet, the
  default; `full_chars` says how long it is; a long title likewise) and only
  the hits it found, whole and with the records around each hit with
  `detail` full; the ranking does not change, and scores show three
  decimals. `query_conversation_context` `record` reads one
  record whole by its `entry_id`.
- **The agent keeps the provider's cache.** A turn's system prompt stays
  the same; recall and what the modules add go with the prompt, once; the
  tools are listed by name. On Z.AI, 99% of each turn's input comes from the
  cache from the second turn on. A conversation is compacted before it
  outgrows the model's window (Axon's thresholds), cut only before a prompt
  so a tool's call keeps its result; a summary sees each tool result cut to
  2,000 characters, and the model any tool's result cut to 50 KB, told how
  to ask for less.
- **Add and edit in dialogs.** `pcx.view.v1`: a form with `open` is a button
  that opens it in a dialog, and an action with a `form` asks for its
  fields in a dialog first, a row action's starting from the row. The
  board, team, skills and greet views add with buttons and edit each row
  or task in a dialog instead of a page of forms.
- **The agent across processes (W20).** A model provider, a context engine
  and the agent run as module processes too (`pcx-provider-zai`,
  `pcx-context`, `pcx-agent`), each serving its slot's port on its socket,
  and a module process (the team) runs the agent's turns through its
  callback socket, so the cloud runs a team of agents with its catalog.
  `pcx.agent.v1`: `CancelTurn` cancels a running turn and its end still
  arrives; a turn starts with its id; `RunTurnRequest` carries the scope and
  the turn's settings between a host and a module.
- **Skills from the app.** The skills module has a view: the library's
  skills, a form to add one, and each one's instructions with a form to
  change it. `pcx_module_api::view::build` holds the builders a module's
  view tree is made of (board, team and skills use them).
- **Boards and tasks from the app.** The board's view has a form to create
  a board and, in a board, one to add a task; `task_add` takes `labels`
  null as none (an empty form field).
- **A workspace setting.** A module's `config_schema` may mark a string
  `"format": "workspace"`: a workspace of the host, by its id (checked to be
  a UUID). The app offers the host's workspaces to pick from by name; the
  skills module's library workspace is one.
- **The team (W19).** `pcx-team` (`[[builtins]] id = "team"`): members
  (`team/agent`: role, authority `discuss`, `decide` or `close`, skills,
  model, the modules and tools it may use) and teams (`team/team`) of a
  workspace, with the tools `team_create`, `team_list`, `agent_add`,
  `agent_update`, `member_add`, `member_remove` and `autonomy_get`. A
  member of the harness works on the board by itself: assigned a task,
  mentioned (`@name`) or answered in a comment, it takes a turn of the agent
  in its own session for the task (`team/work`) and answers in the thread.
  With a team, the board lets only members decide or close a task, within
  their authority. A person's trust in the team (`team/autonomy`,
  `autonomy_set` and `autonomy_pause`, a person only) has the board approve
  a decision with a risk at once within its limits (`approved_by`
  `policy:<version>`); a decision gains `amount`, `currency`, `branch` and
  `reversible`; `revoke_approval` (a person only) holds its task again.
  The team's view in the app: forms to add members and teams, the teams with
  their trust (pause, go on), a team's members (change a member's role and
  authority), and the form that sets the team's trust; `team_list` takes a
  `team` and gives each team its autonomy.
- **The agent (W18).** A built-in agent (`pcx-agent`, `[[builtins]] id =
  "agent"`, the `agent` slot): a turn calls the configured model with the
  tools of the modules it uses until the model answers, retrying a rate
  limit, an overload or an unreachable provider with backoff and stopping on
  a refused key, a used-up plan or an invalid request. Its hooks hear every
  step (`UserPromptSubmit`, `PreModelCall`, `PostModelCall`, `PreToolUse`,
  `PostToolUse`, `Stop`); a blocking answer stops the turn or, before a tool,
  that tool. `pcx.agent.v1` `RunTurn` streams a turn (closing the stream
  cancels it); `pcx agent "<question>"` is its first client.
- **The context engine (W18).** `pcx-context` (`[[builtins]] id = "context"`,
  the `context_engine` slot): what memory recalls for a turn's prompt, and a
  summary of a conversation too long for the model, kept in the stream and as
  a record; the next turns go on from it.
- **Whole turns in Messages (W18).** A message has its parent and its blocks
  (text, thinking, tool use, tool result, summary); memory's new
  `manage_messages` tool appends and pages them; a turn with blocks and no
  text is kept. RocksDB writes message rows of version 2 (version 1 rows are
  read), SurrealDB gets two fields; export format 1.6.0
  (`schemas/export/v1.6.0.json`) writes them when a message has them.
- **Models (W18, step 1).** The `ModelProvider` port in `pcx-module-api`
  (requests of text, reasoning, tool use and tool result blocks; answers
  streamed; typed failures: `rate_limited`, `overloaded`,
  `context_too_long`, `unauthorized`, `quota_exhausted`, `unavailable`,
  `invalid`), the `agent` slot (`MODULE_KIND_AGENT`), the host's routing of a
  request to the provider of its model under that provider's policy, and
  `pcx.module.v1.Model` on a module process's callback socket. Models are
  offered to the `agent` and `context_engine` slots only. Two new crates:
  `pcx-openai-compat` (the OpenAI-compatible chat completions wire, its
  failures classified by error code and HTTP status) and `pcx-provider-zai`
  (Z.AI's GLM models, `glm-5.3`, over its coding endpoint; built in as
  `[[builtins]] id = "provider-zai"` with the `network` grant and its API key
  as a secret setting).
- **Skills, as MCP prompts (W17).** A module (`pcx-skills`: built in as
  `[[builtins]] id = "skills"`, and a `pcx-skills` module process for the
  cloud) that keeps a library of skills as `skills/skill` documents in
  memory, in the workspace its configuration names (`workspace_id`), with
  the tools `skill_add`, `skill_update`, `skill_list` and `skill_get`. Every
  skill is a prompt of the host's MCP server. Modules offer prompts through
  `prompts` and `prompt` of the module trait (`Manifest.prompts`,
  `Module.ListPrompts` and `Module.GetPrompt` on `pcx.module.v1`); the host
  lists them in load order and routes a `get` to the module that lists the
  name; the MCP server now offers the `prompts` capability.
- **`pcx-documents` (W17).** A module's documents in memory, shared by the
  board and the skills: the port, typed reads and writes, and its adapter
  over memory's `manage_document` through the host handle.
- **The board (W16).** A module (`pcx-board`: built in as `[[builtins]] id =
  "board"`, and a `pcx-board` module process for the cloud) that keeps
  boards, tasks, comments and decisions as documents in memory
  (`board/board`, `board/task`, `board/comment`, `board/decision`), with the
  tools `board_create`, `board_list`, `task_add`, `task_update`,
  `task_move`, `task_list`, `comment`, `decide` and `approve_decision` (a
  person only). Two agents moving one task at once get a conflict; a
  decision is recorded in the board's session as `decision_made`, and one
  whose risk is not `none` holds its task until a person approves it. A
  task's branch links its commits through `tools-git`. Its view shows the
  boards, a board's tasks in lanes, and a task's thread.
- **Lanes and row details in views (W16).** `pcx.view.v1` gets the `lanes`
  component (rows grouped into lanes by a field, in the order a second
  source gives) and a `detail` on lists, tables and lanes (a component for
  the row a person picks, whose sources and actions take that row's
  `{{row.x}}`). New messages and fields only.

- **Files, shell and git in a sandbox (W15).** Every file and process access
  of a module goes through the `Sandbox` port (`pcx_module_api::sandbox`;
  `ModuleHost::sandbox()`, and the `pcx.module.v1.Sandbox` service on a
  module process's callback socket), filled by the built-in `pcx.sandbox`
  module: files beneath the module's roots through `cap-std`, processes
  under Seatbelt (macOS) or Landlock and seccomp through the new hidden
  `pcx sandbox-run` (Linux), refused elsewhere. The policy is what the
  module's `daemon.toml` entry grants (`grants`, `filesystem`), handed to
  the host by the daemon; a module never states its own. A module with
  `spawn` gets a scratch directory under `<data_directory>/scratch`.
  Three built-in tool modules, each off until `[[builtins]]` names it:
  `tools-fs` (read, write, edit, list_dir, grep, glob), `tools-shell` (bash,
  and tmux sessions whose server runs in the sandbox) and `tools-git`
  (status, diff, log, branches, commits as the configured author, the
  commits of a branch; the `git` program, so hooks run in the sandbox).
  See docs/DAEMON_MODE.md.

- **Modules together (W14).** A module builds on others:
  - *Calls*: `Manifest.uses` names the modules it calls; a module process gets
    them granted with `uses = [...]` in `daemon.toml` (else it is refused).
    Once started, a module calls their tools through its host handle
    (`HostContext.host`; a module process through `Host.Invoke` on the
    callback socket named in `StartRequest.host_socket`), named as the caller
    (`Scope.caller_module`, which the host sets), at most 4 calls deep. A tool
    only a person calls (visibility `app` only) is refused over MCP and to
    modules.
  - *Documents*: `Manifest.document_kinds` declares the kinds of documents a
    module keeps in memory (`board/task`: a JSON Schema of the data in the
    settings subset without `writeOnly`, and a required string title field).
    The new memory tool `manage_document` (`put`, `get`, `history`, `list`,
    `delete`) writes versions with `expected_version` (a stale write gets the
    latest back), keeps every version, lists by kind with a filter on data
    fields in pages, and lets only the declaring module write; anyone reads.
    A `DocumentStore` port with RocksDB and SurrealDB adapters holds them; a
    workspace's documents go with it when it is deleted. Search finds a
    document's latest version under its workspace (`content_type` `Document`,
    the new `pcx.v1` `SearchResult.workspace_id`, kind, version and title in
    `metadata`); a session search finds none.
  - *Following changes*: a `MEMORY_WRITE` subscription with a filter
    (`Subscription.memory`: document kinds, record types, workspaces) gets
    each memory change it follows, and one event when the feed fell behind.
  - *Export format 1.5.0* carries the documents with every version
    (`schemas/export/v1.5.0.json`); an import merges them as it merges cards.
    1.4 archives still import.

- **Requests classified from the operation table (W9c4).**
  `post_cortex_transport::classify` reads a request of any transport the way
  its transport does: an MCP tool call, a gRPC call of `pcx.v1` or
  `pcx.views.v1` (decoded through their descriptor sets; the new
  `VIEWS_DESCRIPTOR_SET` joins `PCX_DESCRIPTOR_SET`) or a REST request gives
  its operation, the session and workspace ids it names, the project key it
  resolves, whether it asks for the reranker, and what it counts.
  `answer_count`, `grpc_answer_count` and `mcp_answer_count` read a count from
  an answer (a tool result's data follows `mcp::RESULTS`), and `without_rerank` rewrites a
  search to run without its reranker. A bad id, an unknown tool, action, RPC
  or route, or a request that does not decode is an error, never a dropped
  value. The operation table lists the `pcx.views.v1` RPCs, and the REST
  routes name their path parameters after the scope field they fill
  (`DELETE /api/sessions/{session_id}`); the paths themselves are unchanged.
  `classify::browser` classifies the browser transfer as the `Import` and
  `Export` RPCs it maps to, and an instance behind a gateway
  (`BrowserAccess::Proxied { link_base }`) answers download links under the
  path the gateway routes to it.
- **The app's views, public (W9).** `pcx.views.v1` (`post-cortex-proto`),
  served by the daemon behind its token: the graph (as of a time, nodes and
  edges with the authors and sessions of the records behind them), the
  timeline, the record browser (filters, an exact cursor, the total), records
  in full with their validity and provenance, the overview, the entity page,
  topics, radar, files, conversations, search with its ranking trace and
  neighbours, and `WatchActivity` (records pushed as they are stored). They
  run on `Memory::detail_views`: a scope (a session, a workspace, or every
  session) is resolved once and indexed once per call, off the async
  runtime. Requests are parsed strictly: an unknown interaction type or enum
  value, a timestamp out of range, a bad id or cursor is `InvalidArgument`.
- **Export and import stream (W9).** An export counts, then writes the
  archive one session at a time into its output (`Transfer::plan_export`,
  `ExportPlan::write`); the `Export` RPC sends chunks as they are written. An
  import spools its archive to a temporary file (`Transfer::spool`, at most
  `max_import_bytes`, default 1 GiB, in `daemon.toml`), previews and checks
  it in a first pass and writes it in a second, so a cut or unreadable
  archive, an unsupported version or another model's vectors is refused
  before anything is written. The `ArchiveCodec` port writes and reads parts
  (`ArchivePart`) instead of whole archives; the bytes are unchanged (the
  1.4.1 golden file is written part by part). An archive's top-level fields
  must come in the order pcx writes them; a file re-serialized with sorted
  keys is refused. A bad archive is `InvalidArgument` on every transport.
- **Browsers (W9).** `post_cortex_transport::browser`: the origin allowlist
  (`browser_origins` in `daemon.toml`, none by default) with its CORS answer,
  and archive transfer over plain HTTP (`POST /v1/import` with the raw
  archive, `POST /v1/export` for a single-use download link); gRPC-Web over
  the gRPC services. `compose::routes` is every route of a memory instance,
  with `BrowserAccess` `Off` (a daemon by default), `Origins` or `Proxied`
  (a tenant behind a gateway); the daemon and every hosted tenant serve it.
  `post_cortex_transport::transfer` is the one mapping of `Export` and
  `Import`, for gRPC and the browser.
- **The operation table states effects (W9).** Each row of
  `tools::operations::OPERATIONS` says what the operation does to memory
  (`Effect`: read; search, with the field and values that make it rerank;
  write or delete, with the counters it moves and by how much: one, per item
  of a request field, or a field of the answer; an import writes only when
  `settings.commit`) and which request fields name its sessions and
  workspaces (`ScopeFields`). `post_cortex_proto::PCX_DESCRIPTOR_SET` is the
  `pcx.v1` descriptor a gateway maps those names to field numbers with; a
  test checks every name against it. `docs/TOOLS.md` lists the effects.
- **The change log (W9).** `Memory::changes.subscribe()` streams every change
  storage holds: records written, forgotten or removed, turns appended or
  deleted, card versions, workspaces created, updated, archived or deleted and
  their membership; bounded, a lagging subscriber is told so.
- **`post-cortex-cli` (W7).** The client side of `pcx`: backend profiles and
  login, coding-agent setup and their hooks, the stdio MCP bridge, and the
  card, session and workspace commands. It talks to a daemon or the cloud
  over their public APIs only; it depends on no memory or storage crate (a
  CI check). The `pcx` binary stays in `post-cortex-daemon`, which runs the
  daemon's own commands and hands the client commands to the library.
- **Admin commands through the running daemon.** While the daemon runs,
  `pcx export`, `pcx import` and `pcx vectorize-all` go through its gRPC API
  (`Export`, `Import`, `VectorizeSession`); otherwise (and `pcx migrate`
  always) they run offline, memory and the module host composed in the
  command without servers.

- **The module host (W6).** `pcx.module.v1` (`post-cortex-proto`) is the
  module protocol: manifest, scope, tools, events and outcomes, with JSON
  field names that match Claude Code and Codex hook JSON (golden tests pin
  them). `pcx-module-api` is the `Module` trait and its types (`Scope`,
  `ToolSpec`, `ToolOutput`, `ModuleError` with kinds and details).
  `pcx-host` loads modules through one path (API version, kind, events,
  unique ids and tool names, the exclusive `memory` and `context_engine`
  slots), runs every call under a deadline, the caller's cancellation, a
  concurrency limit and panic capture, dispatches events with fail-open /
  fail-closed policies and merges their outcomes, and keeps per-module
  health (calls, errors, timeouts, panics, cancellations, p50/p99).
- **`MemoryModule`** (`post-cortex-memory`): memory in the host's `memory`
  slot, offering the nine memory tools. The daemon composes the host with
  it; MCP serves the host's tools, and stopping the host writes the
  sessions.
- **Hook events (`POST /api/events`).** A client hook sends its event (the
  `pcx.module.v1` `EventEnvelope` as JSON: the scope and Claude Code's hook
  JSON) and prints the answer, a Claude Code hook answer. The memory module
  answers SessionStart with the user and project cards, PreToolUse with the
  file's memory and Stop by appending the captured turns; each fails open.
  `pcx-card.py`, `pcx-file-context.py` and `pcx-capture.py` (and the Claude
  Code plugin) send their events there; `/api/cards`, `/api/file` and
  `/api/messages` stay for hooks of earlier versions.
- **Hook scripts as modules** (`[[hooks]]` in `daemon.toml`). The host runs
  your scripts with Claude Code's hook contract (event JSON on stdin, exit 0
  answers, exit 2 blocks), each within its deadline (killed past it), under
  its fail-open or fail-closed policy, pinned by the SHA-256 of its file: a
  script that does not match is refused at start, one that changed since is
  refused at each run.
- **Modules in their own processes** (`[[modules]]` in `daemon.toml`). A
  module in any language serves `pcx.module.v1` over gRPC on the Unix socket
  the host names in `PCX_MODULE_SOCKET` (`pcx_module_api::serve` for Rust);
  the host launches it (pinned by SHA-256), checks its manifest, starts it
  with its `config`, lists its tools to MCP, sends its events on one stream
  and stops it with the daemon.
- **gRPC `pcx.module.v1.Host`** (`ListTools`, `Invoke`) on the daemon's gRPC
  port and in the cloud: every module's tools, called through the host's
  policy as MCP calls them, for clients that speak gRPC.
- **Hot reload of modules.** When `daemon.toml` changes, `[[hooks]]` and
  `[[modules]]` reload without a restart: unchanged modules keep running,
  new ones start, removed ones stop; a configuration that does not load or
  a module that does not start leaves the running set (`Host::reload`, the
  host's one load path).
- **Faults at the boundaries (T4).** The host keeps a breaker per module:
  five failures in a row (past the deadline, a panic, unavailable,
  internal) refuse its calls and events for 30 s; a caller's mistake does
  not count. A module process that exits on its own is restarted with a
  backoff (0.5 s doubling to 60 s) and started with its configuration
  again. Each storage adapter keeps a breaker for its backend: five backend
  failures in a row make every port call `Unavailable` at once for 30 s.
- **Module health in `/health`** (T14): `modules` lists every module's
  calls, errors, timeouts, panics, cancellations, refusals, whether its
  breaker is open, and p50/p99 latency.
- **Capabilities (T11).** A hook script lists its `capabilities`, a module
  process gets `grants`; a module that asks for a capability not granted is
  refused at start. A module without `read_conversation` gets its events
  without the prompt, the tool's input and result, the answer, the
  transcript and the payload.

### Changed

- **Transports in `post-cortex-transport` (ARCHITECTURE section 3).** The
  `pcx.v1` gRPC service (`post_cortex_transport::memory::PcxGrpcService`,
  which takes the build `Health` names) and the REST routes
  (`post_cortex_transport::rest::routes`) moved out of the daemon, which only
  composes and serves them behind its host policy and token. Requests and
  answers are unchanged.
- **The document kinds are a port.** `post_cortex_ports::DocumentKinds`
  (the kinds the running modules declare, as core `KindDeclaration`s, with
  the typed `KindsError`) replaces the host lookup in the `manage_document`
  handler; `MemoryModule` adapts it over its host handle.
  `DeclaredKind::running` is removed.
- **`HealthResponse.total_updates`** is no longer marked `deprecated` in the
  proto (it is still served, the same value as `total_requests`, and still
  documented as deprecated); the bytes on the wire are unchanged.
- **`scripts/check-architecture.sh`** checks the code after a
  `#[cfg(test)] mod x;` declaration (it stopped at the first one), and
  counts `transport_reach`: a transport or tool handler that reaches the
  host, the filesystem, a process or a storage adapter itself.

### Fixed

- **SurrealDB full-text ranking is deterministic.** Entries with equal BM25
  scores came back in the order they were stored, so the fused ranking (and
  the top `k`) of a search could change with the order background
  embedding wrote them (the retrieval pin test failed under load). They now
  rank by text, then session, then content id, as the in-memory index does;
  the `TextSearch` port states the order and its conformance suite checks it.

- A workspace's creation time is the same after a restart: `create` writes
  and caches one time (the store and the cache took the clock separately,
  a second apart across a second boundary).

### Breaking changes

- **No JSON in a gRPC string (W23).** `SemanticSearchRequest` and
  `AssembleContextRequest` have no `select_string` and `select_map`, and
  their responses no `selected_json`: a gRPC client reads the typed
  messages. The MCP tools keep `select`.
- **A module's API is its RPCs (W23).** `pcx.module.v1` has no `Module.ListTools`,
  `Module.Call`, `Host.Invoke`, `CallRequest`, `CallResponse` or `ToolOutput`; a tool is
  an RPC its manifest names (`Manifest.rpcs`), called by its gRPC path, and
  `ListToolsResponse.rpcs` and `GetViewResponse.rpcs` name each one's path. The `Module`
  trait has no `tools`/`call` (memory's MCP tools are `mcp_tools`/`call_mcp`, for MCP
  clients only), `ModuleHost` no `call`, `pcx_host::Host` no `call` (`call_for_agent` is
  MCP's); `UsedTool` carries a `pcx_module_api::rpc_tool::RpcTool`, which a module calls
  through its channel. Agents' models call memory's `memory_search`, `memory_context` and
  `memory_record` (its `pcx.v1` RPCs). Two modules serving one service are refused
  (`HostError::DuplicateService`).
- **Settings are messages (W23).** `Manifest.config_schema` is replaced by `config_type`,
  a message of the module's descriptors whose fields carry the `pcx.module.v1.setting`
  option (`title`, `required`, `secret`, `workspace`); a turn's grant is its
  `pcx.module.v1.TurnGrant` field. `StartRequest`, `ModuleInfo` and
  `ConfigureModuleRequest` carry the configuration as `google.protobuf.Any`, a change
  removes secrets by `clear`, and `HostContext.config` is an `Any` read with
  `HostContext::config::<M>()`. `ModuleError.details` is `field` on the wire.
  `pcx_module_api::ConfigSchema` is `ConfigType` (the JSON Schema subset stays as
  `FormSchema`, for a view's forms). `daemon.toml` keeps each module's section as the
  message's JSON form.
- **Modules together (W14).** `pcx_module_api::HostContext` has the
  module's `host` handle (`ModuleHost`), `Scope` has `caller_module` and
  `call_depth`, `Manifest` has `document_kinds` and `uses`, `Subscription`
  has `memory`, and `pcx_host::ProcessSpec` has `uses`: struct literals name
  them. `SemanticSearchResult.session_id` is `source` (`HitSource::Session` or
  `HitSource::Workspace`); REST search hits name `session_id` only for a
  session's hit, a document's names `workspace_id`. `Changes::new` takes the
  workspaces. Export format 1.5.0 is not read by 0.5.0.
- **Transfer streams.** `Transfer::export` is `plan_export` + `ExportPlan::write`
  (into any `Write`); `Transfer::preview_import` and `Transfer::import` take
  an `ArchiveFile` (`ArchiveFile::at(path)` or a spooled one) and `import`
  returns the preview with the result. `ArchiveCodec` has `writer` and `read`
  in place of `encode` and `decode`; `post_cortex_storage`'s whole-file
  helpers (`encode_export`, `decode_export`, `read_export_file`,
  `write_export_file`, `preview_export_file`, `list_export_sessions`) are
  removed. `ExportData` (a whole archive in memory) is behind
  `post-cortex-core`'s `test-support` feature. The last `Export` chunk
  carries the summary and no data.
- **`Records::subscribe` is replaced by `Changes::subscribe`**
  (`memory.changes`), which streams `Change` (records, messages, cards,
  workspaces); a record change is `Change::Record` (`Change::record()`).
- **Client commands need a daemon or the cloud.** `pcx session` and `pcx
  workspace` no longer open the storage when the daemon is not running: they
  say to start it (`pcx start --daemon`); `pcx setup` against the local
  daemon starts it. With a running daemon, `pcx export --pretty` and `pcx
  vectorize-all --messages` ask to stop it (they run offline only).
- **`pcx hook` (Codex, Cursor, Gemini, ...) sends events** to `POST
  /api/events`, like the Claude Code hooks; it needs a daemon of this
  version.
- **Moved APIs.** `post_cortex_daemon::daemon::{bearer_client, env_text,
  ClientError, is_daemon_running, ensure_daemon_running, proxy_stdio,
  run_stdio_proxy}` are in `post_cortex_cli`; `DaemonConfig::http_client`
  is `DaemonConfig::local().http_client()`; `AUTHOR_HEADER` is
  `post_cortex_proto::AUTHOR_HEADER`; `DaemonConfig::create_example_config`
  is gone (`pcx init` writes the example).

- **The memory instance has no circuit breaker.** `SystemConfig` loses
  `circuit_breaker_failure_threshold` and `circuit_breaker_timeout_seconds`;
  health loses `circuit_breaker_open` and `circuit_breaker_failures`, and its
  `healthy` now says that no session's last write failed. The breakers are at
  the host and the storage adapter (see Added).

- **Hooks of this version need a daemon of this version** (`POST
  /api/events`); against an older daemon they add nothing (they fail
  open).

- **The memory tools live in `post_cortex_memory::tools`.** The operation
  table, response shapes, `select`, `Failure`/`Kind` and the wire parsers
  moved from `post-cortex-transport` (`post_cortex_transport::{operations,
  responses, select, wire, Failure, Kind}` become
  `post_cortex_memory::tools::...`). `post_cortex_transport::mcp` is
  `McpService` over a `pcx_host::Host` (`PostCortexService` and
  `ToolCatalog` are gone); a tool call runs within 300 s
  (`mcp::TOOL_DEADLINE`) and stops when the MCP request is cancelled. The
  JSON after `Results:` lists object keys in sorted order.

- **The configuration loads fail closed.** `DaemonConfig::load()` returns
  `Result<DaemonConfig, ConfigError>`: a `daemon.toml` that cannot be read or
  parsed, an override that is not valid (`PC_PORT`/`PC_GRPC_PORT` not a port,
  `PC_REDACT` neither on nor off, a variable that is not UTF-8) or a result
  that does not validate is an error; the daemon and every `pcx` command
  stop with it instead of running on defaults.
- **No `embeddings` feature.** `post-cortex-memory`, `post-cortex-transport`,
  `post-cortex-daemon` and the facade always build search; whether it runs
  is the configuration's (`enable_embeddings` and an embedding engine).
  Builds that named `--features embeddings` drop it.
- **Typed errors in every library.** `post-cortex-storage` and
  `post-cortex-embeddings` no longer depend on `anyhow`: the archive codec
  returns `ArchiveError` (`Json`, `Io`, `Version`), the embeddings crate its
  `Error` (`ModelLoad`, `FeatureDisabled`, `Tokenization`, `Inference`,
  `NoEmbedding`, `Dimension`, `IndexFull`, `Inactive`, `Busy`, `Timeout`,
  `Join`; `External(anyhow)` is gone), and the adapters' bodies a typed
  `AdapterError` that maps to `PortError`.
- **Redaction fails closed.** `Redactor::new` returns
  `Result<Redactor, RedactError>`: an extra pattern that does not compile is
  an error (`OpenError::Redaction` when memory opens) instead of being
  skipped with a warning, so redaction never runs with fewer rules than
  configured.
- **`DaemonConfig::data_directory` and `StorageConfig::RocksDb` are
  `PathBuf`**: a home directory that is not UTF-8 no longer panics.
- **`pcx export` / `pcx migrate` take `ExportArgs` / `MigrateArgs`**; the
  BERT backend reads its weights instead of mapping them (no `unsafe`).
- **Module structure (ARCHITECTURE rule 7):** no `pub(super)`, `super::` or
  `pub(in ...)` left; the SurrealDB adapter's helpers are the `ids`,
  `schema`, `migrate` and `graph` modules and a `SessionWriter`; gRPC
  handlers are functions over `&Memory`. No `#[allow]` (and no workspace
  lint allows), no `unwrap`/`expect`, no swallowed `Result` outside tests.

- **Reranker conformance suite** (`rerank_conformance_suite!`): one relevance
  in [0, 1] per document in input order, none for no documents. The ONNX
  cross-encoder runs it on the real model (CI caches the model and installs
  ONNX Runtime 1.31); Voyage runs it against a local stand-in of the rerank
  API. `VoyageReranker::new` takes the endpoint first
  (`rerank::VOYAGE_ENDPOINT` for Voyage) and returns `reqwest::Error`; its
  failures are typed `Unavailable`. `ModelConfig::voyage` is a
  `VoyageConfig`; `daemon.toml` gains `voyage_endpoint` (default: Voyage).
- **Unused public items removed:** `Admin::pipeline_backlog` (use `Admin::health().pipeline_backlog`), the
  public `pipeline` and `scoring` modules of `post-cortex-memory`
  (`PipelineConfig` stays re-exported).

- **`CheckpointStore` is keyed by session** (ARCHITECTURE section 3):
  `list_checkpoints(session_id)` lists one session's checkpoints by id;
  `load_checkpoint` and `delete_checkpoint` are gone (nothing but tests
  called them; checkpoints go with their session). Exports read the
  checkpoints of the sessions they export; `pcx migrate` counts them per
  session.
- **Port conformance suites for messages, cards, dismissals and full-text
  search** in `post_cortex_ports::conformance` (`text_search_conformance_suite!`
  for adapters with full-text search); the storage crate's per-backend copies
  are gone.

- **Memory runs on ports; the daemon composes the adapters (W5s part 2).**
  `post-cortex-memory` no longer depends on `post-cortex-storage` or
  `post-cortex-embeddings`. `Memory::open`, `open_with`, `over_stores` and
  `SharedModels` are gone: `Memory::over_ports(MemoryPorts, SystemConfig)`
  (and `over_ports_with` for a pipeline configuration) takes the storage
  ports, the `SearchIndex`, the `EmbeddingEngine`, the rerankers and the
  `ArchiveCodec`; `post_cortex_daemon::compose` (re-exported as
  `post_cortex::compose`) opens them from a `StorageConfig` and a
  `ModelConfig` (`open_memory`, `open_storage`, `search_index`, `ports`,
  `Models`). `SystemConfig` keeps capability settings only:
  `data_directory`, `embeddings_model_type`, `vector_dimension`,
  `reranker_model`, `onnx_runtime`, `voyage_api_key`, `voyage_model`,
  `storage_backend` and the `surrealdb_*` fields moved to the daemon's
  configuration (`DaemonConfig::storage_config` and `model_config` replace
  `storage_system_config`). The memory crate's `bert`, `rerank`,
  `rerank-voyage` and `surrealdb-storage` features moved to the daemon.
- **`SearchIndex` is the one retrieval port** (`post_cortex_ports::index`:
  `upsert`, `delete`, `entries`, `nearest`, `score`, `lexical`, `stats`).
  `Stores` no longer carries `vectors` or `text_search`: `VectorStore` and
  `TextSearch` are what the in-memory index adapter
  (`post_cortex_embeddings::MemoryIndex`) persists through and ranks BM25
  with; `TextSearch::search_text` takes the terms and returns the entries in
  BM25 order (no query vector, no similarity: the index scores the hits).
  `ContentVectorizer::new(config, engine, index)` replaces `new` and
  `with_engine`; its config lost `embedding_config` and `vector_db_config`.
  `Admin::vectorization_stats` is async; `Health.vector_load` is a
  `post_cortex_ports::LoadReport` (same JSON).
- **`EmbeddingEngine` port** (`model()`, `embed(texts)`):
  `LazyEmbeddingEngine` loads its model on the first batch;
  `rerank::LazyCrossEncoder` loads the cross-encoder on its first score.
- **The archive model is plain data in core** (`post_cortex_core::archive`:
  `ExportData`, `ExportOptions`, `ImportOptions`, `EmbeddingModelInfo`, ...;
  `ExportData::new` takes the format version, schema URL and export time),
  `SessionCheckpoint` is `post_cortex_core::session::checkpoint`, and the
  `ArchiveCodec` port encodes it (`post_cortex_storage::JsonArchive`: format
  versions, schema, compression). Export, import, preview and their merge
  rules are the Transfer capability's (`post_cortex_memory::capabilities::
  {export, import, preview_import, read_import_target, ArchivePorts, ...}`);
  `post_cortex_storage` keeps the codec and the archive files.
  `TransferError::Port` carries a port failure. `CompressionType` parses
  with `FromStr` (`UnknownCompression`) instead of an inherent `from_str`.
- **Every port returns the typed `PortError`** (renamed from `StorageError`:
  storage, index, embedding and reranker ports report the same error). `post_cortex_ports::PortError`
  has exactly the kinds `NotFound`, `Conflict`, `Corrupt` and `Unavailable`
  (a backend, I/O or encoding failure, with the full message); the storage
  ports, `VectorStore`, `TextSearch` and `Reranker` return
  `post_cortex_ports::error::Result<T>` instead of `anyhow::Result<T>`, and
  `post-cortex-ports` no longer depends on `anyhow`. `is_not_found(&anyhow::Error)`
  and `is_corrupt` are methods now (`err.is_not_found()`); the `Serialization`,
  `Format`, `Io` and `External` variants are gone. A missing session displays
  as `not found: session <id>` (the `Session not found` prefix is gone), and
  listing sessions no longer prefixes its storage error with
  `Failed to list sessions:`. Each capability maps the port error in one
  function.
- **The vectorized record ids left the domain.** `ActiveSession` no longer
  holds `vectorized_update_ids` (`is_vectorized`, `mark_vectorized`,
  `unmark_vectorized`, `vectorized_ids`, `vectorized_count`,
  `is_vectorization_pending` and `pending_vectorization_count` are removed,
  and `from_records` takes four arguments): the set belongs to the memory
  runtime's session cache entry (`memory_system::VectorizedIds`), and
  `SessionStore` saves it next to the session (`save_session(session,
  vectorized)`, `save_session_with_updates(session, vectorized, ..)`,
  `save_session_with_changes(session, vectorized, ..)`,
  `replace_session(session, vectorized)`) and loads it with it
  (`load_session` returns `StoredSession { session, vectorized }`). The
  stored form is unchanged: the ids stay in the session blob's
  `vectorized_update_ids` field. `ActiveSession` no longer implements serde;
  `StoredSession` is the serialised form (`ExportedSession::session` is a
  `StoredSession`). Migration: `memory.sessions.pending_vectorization_count(id)`
  replaces `session.pending_vectorization_count()`.
- **The BM25 tokenizer is a domain function**: `post_cortex_core::text::tokens`
  (was `post_cortex_ports::text::tokens`).
- **`post-cortex-ports` holds the port traits and the types they speak.**
  The storage ports (`SessionStore`, `CheckpointStore`, `WorkspaceStore`,
  `GraphStore`, `VectorStore`, `TextSearch`, `MessageStore`, `CardStore`,
  `DismissalStore`, `FreshnessStore`, `Stores`, `SessionSummary`), the types
  `SessionCheckpoint`, `StoredWorkspace`, `WorkspaceUpdate`, `CardWrite`,
  `InvalidationScope`, `StaleEntryInfo`, the store errors (`StorageError`,
  `is_not_found`, `is_corrupt`), the vector types (`VectorMetadata`,
  `SearchMatch`, `Corpus`, `MESSAGE_CONTENT_TYPE`), the BM25 tokenizer
  (`text::tokens`) and the `Reranker` trait moved there from
  `post-cortex-storage` and `post-cortex-embeddings`; the shared conformance
  suites are its `conformance` feature (`conformance_suite!`).
  `post_cortex_storage::traits` is gone (`StorageBackendType` is
  `post_cortex_storage::StorageBackendType`), `StorageError` lost its
  `RocksDb` and `SurrealDb` variants, and `post-cortex-storage` no longer
  depends on `post-cortex-embeddings`. Migration: import these items from
  `post_cortex_ports` (or `post_cortex::ports`).
- **`post-cortex-core` no longer re-exports `post-cortex-proto`**
  (`post_cortex_core::proto`); use `post_cortex_proto` or `post_cortex::proto`.
  The `Default` impls of `DecisionItem`, `QuestionItem`, `ConceptItem`,
  `SpecItem` and `FlowItem`, which stamped the current time, are removed:
  the domain takes time as a parameter.

- **`pcx setup` is global and targets the active profile.** Without flags it
  now sets up every project once (see *Global setup* below) and uses the
  active backend profile, which is the hosted `cloud` profile unless changed.
  Migration: run `pcx setup --project` for the earlier per-project files, and
  add `--self-hosted` (or run `pcx profile use self` once) to keep using the
  daemon in `daemon.toml`. Plain `pcx` (the stdio proxy to the local daemon)
  is unchanged.
- **`post-cortex-mcp` has no process-global memory system.** The
  `static MEMORY_SYSTEM` / `SERVICE`, `inject_memory_system`,
  `get_memory_system` and `get_service` are removed. Every read tool takes the
  memory system as its first argument (`list_sessions(&system)`,
  `semantic_search_global(&system, query, ...)`, ...), and
  `update_conversation_context` / `bulk_update_conversation_context` take
  `&dyn PostCortexService` first (one `MemoryServiceImpl` per memory system;
  it owns the write pipeline). Nothing opens `~/.post-cortex/data` implicitly
  any more. In `post-cortex-daemon`, `PostCortexService::new` takes the
  `Arc<MemoryServiceImpl>`, and the MCP tools (and the legacy SSE server's
  tool handlers) act on their own server's memory system, so several memory
  systems can be served from one process without crossing. The
  `embeddings`-off stubs of `semantic_search_global` /
  `semantic_search_session` take the same arguments as the real functions.
  Migration: replace `inject_memory_system(m)` with
  `Arc::new(MemoryServiceImpl::new(m))` and pass `&m` / `&*service` to the
  calls. A single-memory-system daemon behaves as before.
- **New required trait methods** for custom backends and services:
  `Storage` gains `update_workspace`, `save_workspace_projects`,
  `export_archive` and `import_archive`, and requires the new
  `DismissalStorage`; `MessageStorage` gains `message_stats`,
  `load_messages_page`, `delete_message`, `delete_session_messages` and
  `restore_messages`; `CardStorage` gains `restore_card_versions`;
  `PostCortexService` gains `resolve_project`. Migration: implement them (the
  RocksDB and SurrealDB backends and `MemoryServiceImpl` do).
- **Archived workspaces are left out of listings by default**
  (`post_cortex_mcp::list_workspaces`, MCP `manage_workspace` `list`, gRPC
  `ListWorkspaces`, `GET /api/workspaces`). Migration: pass
  `include_archived` (`?include_archived=true` over REST) to list them.
- **Storage layouts.** RocksDB workspace rows are written as `pcx-ws-v3`
  (with `archived_at` and the project keys) and update rows and checkpoints
  carry the record metadata; rows of the earlier layouts still load, but
  data written by this version is not readable by 0.5.0. SurrealDB defines
  `workspace.archived_at`, `workspace.projects` and the `radar_dismissal`
  table on start. Migration: back up before upgrading; nothing to run.
- **Export format 1.3.0.** `pcx export` (and gRPC `Export`) write format
  version 1.3.0, which adds cards and message streams. 0.5.0 and older refuse
  such a file before writing anything ("Incompatible export format
  version: 1.3.0"); files of 1.0.0 to 1.2.0 import unchanged. Migration:
  import with this version, or export with `--no-messages` only if size is
  the concern (the version is 1.3.0 either way).

- **Writes need an existing session.** A write, bulk write, message append
  or file invalidation for an unknown or deleted session now fails with
  not-found (MCP tool error, gRPC `NOT_FOUND`, REST error)
  instead of creating the session. `SessionManager::get_or_create_session`
  is removed; only session create (and project resolution) creates.
  Migration: create the session first.
- **Removed write and pipeline APIs.** `ConversationMemorySystem::
  add_incremental_update` (the inline write mode only tests used),
  `apply_entity_graph_update_now`, `spawn_background_vectorization` and
  `auto_vectorize_if_enabled` are removed; `add_incremental_update_deferred`
  is the one write and now merges the entity graph in the same swap. The
  pipeline has no graph queue (`GraphQueue`, `GraphWorkItem`,
  `submit_graph`, `PipelineConfig::graph_capacity` removed);
  `Pipeline::submit_embedding` is `async` and `PipelineConfig` gains
  `submit_wait`. Migration: write with `add_incremental_update_deferred`
  (or through `MemoryServiceImpl`) and call `vectorize_update_now` where a
  test needs the vector at once.
- **Batched embedding APIs.** `ContentVectorizer::vectorize_update` and
  `vectorize_latest_update` are replaced by `vectorize_updates(session, ids)`
  (one `encode_batch` and one store batch for all of them);
  `ConversationMemorySystem::vectorize_updates_now(session, ids)` is new and
  `vectorize_update_now` calls it with one id. `EmbeddingQueue::start` takes
  the `PipelineConfig`, and `PipelineConfig` gains `embedding_batch` (128)
  and `embedding_workers` (2). Migration: call `vectorize_updates` with the
  ids; build `PipelineConfig` with `..Default::default()`.
- **Storage port changes.** `FreshnessStorage::invalidate_source` takes an
  `InvalidationScope` (`Session(id)` or the explicit `AllSessions`);
  `Storage` gains the required `delete_checkpoint`.
  `ActiveSession::remove_updates_for_file` returns the removed ids.
  `start_grpc_server` takes the daemon's `Arc<MemoryServiceImpl>` and
  `PcxGrpcService::new` is removed (use `from_service`). Migration: pass the
  scope (the session whose file changed) and the shared service.
- **Storage layouts (source references and checkpoints).** RocksDB source
  references are written with their status and session (rows of earlier
  layouts still load, as registered with no known session); checkpoints get
  a per-session index, built once on open. SurrealDB defines
  `source_reference.session_id`. Data written by this version is not
  readable by 0.5.0. Migration: back up before upgrading; nothing to run.
- **Dead code removed (wave W1).** These public items had no caller in
  post-cortex or post-cortex-cloud and are gone:
  - `post-cortex-daemon`: the legacy `DaemonServer` (`daemon::server`, its
    `/message` JSON-RPC and `/stats` routes) and `SSEBroadcaster`
    (`daemon::sse`). The daemon serves `rmcp_server::build_router` /
    `start_rmcp_daemon` only, as before. `DaemonConfig::storage_system_config`
    returns `Result` and fails on an unknown `storage_backend` (see Fixed).
  - `post-cortex-mcp`: `get_memory_system_with_config`, `Interaction`,
    `schemas` / `get_all_tool_schemas`, `enable_embeddings`, the unified
    `semantic_search`, `vectorize_session`, `get_vectorization_stats`,
    `get_tool_catalog`, `create_session_checkpoint_with_system`,
    `load_session_checkpoint(_with_system)`, `load_session(_with_system)`,
    `list_sessions_with_storage`, `mark_important`, `search_sessions`,
    `update_session_metadata`, `query_conversation_context_with_system`,
    `list_workspaces` (use `list_workspaces_filtered`). The facade's
    `post_cortex::tools::mcp` alias is removed; use `post_cortex::mcp`.
  - `post-cortex-core`: `graph::graph_rag`; the `CacheManager` stack
    (`CacheManager`, `CacheProvider`, `CacheManagerSummary`,
    `init_global_cache_manager`, `get_global_cache_manager`, `new_cache!`,
    `EvictionEvent`) and `Cache::resize`, `reset_metrics`,
    `has_performance_issues`, `get_recommendations`, `keys`, `values`,
    `for_each`, `drain_eviction_events`, `clear`, `capacity`; the lock-free
    `session_components::EntityGraph` / `EntityNode`; `HotContext::get_recent`,
    `get_all`, `back`, `to_deque`, `from_deque`, `clear`;
    `SimpleEntityGraph::find_related_entities_by_type`, `search_entities`,
    `update_entity_timestamp`, `find_communities`, `mention_entity` and
    `EdgeData::from_relation_type` (`link_mentions` is private);
    `ActiveSession::rebuild_entity_graph_from_updates` (use
    `rebuild_entity_graph_sync`); `timeout_utils::TimeoutError`,
    `with_timeout`, `with_retry_timeout`, `with_entity_timeout`,
    `DEFAULT_ENTITY_PROCESSING_TIMEOUT` (`with_mcp_timeout` /
    `with_storage_timeout` now return `tokio::time::error::Elapsed`). Unused
    dependencies dropped (tokio-util, crossbeam-channel, crossbeam-queue,
    parking_lot, rayon, num_cpus, rand, log).
  - `post-cortex-embeddings`: `EmbeddingModelType::StaticSimilarityMRL` and
    its hash backend (tests use the new `test-support` feature,
    `test_support::hash_engine`); product quantization
    (`ProductQuantizationCodebook`) and scalar quantization
    (`StoredVector::quantized` / `pq_codes`, `VectorDbConfig::
    enable_quantization`, `quantization_buckets`,
    `enable_product_quantization`, `pq_subvectors`, `pq_bits`); the never-read
    `VectorDbConfig::distance_threshold`, `auto_save_interval`,
    `persistent_storage`, `storage_path` and `EmbeddingConfig::
    memory_pool_size`, `cache_dir`, `enable_caching`,
    `enable_performance_monitoring`; the always-zero
    `VectorDbStats(Snapshot)::search_hit_rate`, `index_efficiency`,
    `quantization_ratio`; `VectorDB::add_vector_compat`, `add_vectors_batch`,
    `remove_vector`, `get_vectorized_update_ids`, `search_by_content_type`,
    `search_with_mode`, `build_index` and `Default`, with `SearchMode` /
    `SearchQualityPreset` (use `add_vectors_bulk`, `remove_vectors`,
    `search`, `search_with_filter`); `LocalEmbeddingEngine::is_bert_based`,
    `current_concurrency_load`, `get_concurrency_stats`. New:
    `LocalEmbeddingEngine::with_backend`.
  - `post-cortex-storage`: the `StorageBackend` enum with its delegation
    impls, `supports_native_graph` / `supports_native_vectors`,
    `StorageConfig`, `SurrealDBConfig`; `GraphStorage::get_entity`,
    `list_entities`, `find_related_entities`, `find_related_by_type`,
    `find_shortest_path`, `get_entity_network` and
    `RealRocksDBStorage::batch_save_graph_data`. `StorageBackendType::
    from_str` (returning `Option`) is replaced by `FromStr` with the typed
    `UnknownStorageBackend` error, plus `as_str` and `ALL`.
  - `post-cortex-memory`: `semantic_query_engine` (`SemanticQueryEngine`,
    `ConversationMemorySystem::semantic_query_engine`,
    `ensure_semantic_engine_initialized`; search through
    `semantic_search_session` / `_multisession` / `_global`); the
    `SystemConfig` fields `semantic_search_threshold` (only that engine read
    it), `max_hot_context_size`, `max_warm_context_size`,
    `context_compression_threshold`, `storage_timeout_seconds`,
    `enable_performance_monitoring` and the deprecated
    `max_vectors_per_session` (also from `EmbeddingConfigHolder`);
    `memory_system::managers` (`IncrementalContextProcessor`,
    `SimpleGraphManager`, the `context_processor` / `graph_manager` fields);
    `SystemMetrics::storage_operations` / `cache_operations` and
    `SystemHealth::contexts_processed` / `processing_errors` /
    `storage_operations`; the summary queue (`SummaryQueue`,
    `SummaryWorkItem`, `Pipeline::submit_summary`, `summary_queue`,
    `embedding_queue`, `PipelineConfig::summary_capacity`) and
    `EmbeddingWorkItem::text`; `enable_embeddings_config`,
    `set_embedding_model`, `find_sessions_by_name_or_description` (use
    `find_session_summaries`), `get_conversation_context`,
    `scoring::CompositeScoreAdjuster`, `context_assembly::boost_by_graph`,
    `ContentVectorizer::has_persisted_embeddings` /
    `count_persisted_embeddings`, `StorageActorHandle::rebuild_entity_graph`.

  Migration: drop the removed config fields from struct literals; the
  replacement named above, where there is one, does what the old item did.

- **Domain purity (wave W2).** Sessions, the entity graph and workspaces
  are plain values: no IO, no async, no clock (time is a parameter), no
  runtime caches in `post-cortex-core`, which no longer depends on tokio,
  arc-swap or futures. (The summary DTOs still read the clock; W4.)
  - `ActiveSession` is a value of metadata, records and entity graph with
    private fields. Read it with `records()`, `record(id)`,
    `records_owned()`, `records_shared()`, `hot_records()`, `metadata()`,
    `last_updated()`, `entity_graph()`, `entity_graph_shared()`, and the
    views derived on read `current_state()` (owned `StructuredContext`),
    `code_references()` (a `BTreeMap`), `change_history()`; vectorization
    state with `is_vectorized`, `mark_vectorized`, `unmark_vectorized`,
    `vectorized_ids`, `vectorized_count`. Build it with
    `ActiveSession::new(id, name, description, created_at)` or
    `ActiveSession::from_records(metadata, last_updated, records, graph,
    vectorized_ids)` (replaces `from_components`; `Default` is gone). Change
    it with the synchronous `apply_record(update) -> Arc<ContextUpdate>`
    (replaces the async `add_incremental_update`,
    `add_incremental_update_fast`, `apply_entity_graph_update` and
    `add_context_update`), `remove_records(&[id])` (replaces
    `remove_update_by_id`), `replace_records` (replaces
    `set_incremental_updates`), `rebuild_entity_graph` (replaces
    `rebuild_entity_graph_sync`), `entity_graph_mut`, `retire_update`,
    `update_metadata(name, description, at)` and `set_name(name, at)`
    (`set_description` is gone). `relink_entity_mentions` is gone (loading
    relinks). Removed types: `HotContext`, `CompressedUpdate`,
    `StructuredSummary`, `MAX_COLD_SUMMARIES` and the session's own
    `CodeReference` (`active_session::CodeReference` now names the one
    `core::context_update::CodeReference`). `SessionMetadata::new` takes
    `created_at` (`with_created_at` is gone). New: `normalised`,
    `MAX_TITLE_BYTES`, `StructuredContext::from_records`,
    `UserPreferences: Default`.
  - `Workspace` is a value: its members and metadata are private fields
    (`session_count`, `get_all_sessions`, `get_session_role`, `metadata`,
    `project(s)`, `archived_at`) changed through `&mut self`
    (`add_session` / `remove_session` return the previous role,
    `set_project`, `set_archived`), and serialised with the workspace.
    `Workspace::new` takes `created_at`. `WorkspaceManager` moved to
    `post_cortex_memory::memory_system::WorkspaceManager` (same methods;
    `add_session_to_workspace` returns the previous role).
  - One codec for enum names: `UpdateType`, `EntityType`, `RelationType`,
    `SessionRole` and the new `core::content::ContentKind` have `ALL`,
    `as_str` (wire name), `stored_name` (serde name), `Display` and a strict
    `FromStr` with `core::wire::UnknownVariant` (it inverts `as_str` and
    accepts the stored name; `RelationType::from_str` used to accept the
    Debug name only, with a `String` error).
    `post_cortex_memory::content_vectorizer::ContentType` is now
    `ContentKind` (moved to core, re-exported there; adds `is_record`).
  - Entity graph: `SimpleEntityGraph::add_relationship(rel, at)` takes the
    time of the record stating the relation; new `entities_in`,
    `name_keys_in`, `name_words`; `context_assembly::find_query_entities`
    is gone (use `entity_graph::entities_in`).
  - Removed: `core::cache` (`Cache`, `SessionCache`, `CacheStats`; the
    memory crate has its own `memory_system::SessionCache`),
    `core::timeout_utils` (now private to `post-cortex-mcp`),
    `From<tokio::task::JoinError> for SystemError`,
    `ConversationMemorySystem::cleanup_expired_sessions` and
    `force_memory_cleanup`, `SystemConfig::session_timeout_minutes`,
    `SessionManager::active_sessions`, `session_cache_hits` and
    `session_cache_misses` (see `sessions.stats()`); `SessionManager::new`
    is crate-private. `CommittedUpdate` is at `memory_system::CommittedUpdate`
    (no longer under `memory_system::system`).
  - Large files were split by concern (`entity_graph`, `graph_views`,
    `context_assembly`, `export_import`, `traits`, the RocksDB and SurrealDB
    session code, the gRPC service); public paths are re-exported unchanged.
    `SurrealDBStorage` gains public inherent methods for every `Storage`
    method (as `RealRocksDBStorage` has).

  Migration: replace field access with the accessors above, `.await` on the
  removed async mutators with `apply_record`, and pass the time where a
  constructor or setter now takes it.
- **Storage correctness and on-disk format (wave W3, part A).**
  - *RocksDB data version 1, migrated at open.* Every row value now starts
    with a marker naming its layout (`pcx-ses-v1` session blobs,
    `pcx-upd-v4` update rows, `pcx-meta-v1` listing records,
    `pcx-ckpt-v4` checkpoints, `pcx-ws-v4` workspace rows without members,
    `pcx-wsm-v1` memberships with the role as its wire name, `pcx-src-v2`
    source references in the backend's own layout instead of the gRPC
    protobuf), and the source-by-file index holds the entry id. Opening a
    database written by 0.5.0 (or this branch before W3) rewrites its rows
    once and records `pcx_meta:data_version = 1`; reads decode only the
    current layouts (no trial decodes, and loading never writes). A row the
    migration cannot read is left and reads as `Corrupt`. A database of a
    newer data version is refused. The `entity:` and `relationship:` rows
    are dropped: the session blob is the only store of a RocksDB session's
    graph. Data written by this version is not readable by 0.5.0.
    Migration: back up the data directory before upgrading; nothing to run.
  - *SurrealDB data version 1.* Edge record ids are
    `{session}_v1_{relation}_{len(from)}:{from}_{to}` (wire names, no
    hasher, so a toolchain change cannot duplicate edges). The first
    start re-keys stored edges, merges duplicates, deletes orphans whose
    entity is gone, and records `pcx_meta:schema.version = 1`; a newer
    version is refused. `open_existing` (read-only) never migrates.
  - *Freshness port on domain types.* `FreshnessStorage` takes and returns
    `post_cortex_core::core::freshness::{SourceReference, SourceScope,
    FreshnessEntry, FreshnessStatus, SymbolId, CascadeInvalidateReport}`;
    the daemon maps `pcx.v1` to them. `post-cortex-storage` and
    `post-cortex-memory` no longer depend on `post-cortex-proto`;
    `FreshnessReportExt` is removed (it had no caller).
  - *Typed corrupt data.* `StorageError::Corrupt { kind, id, reason }`
    (`is_corrupt`, `corrupt`, `parse_timestamp`, `parse_name`,
    `parse_uuid`, `not_found`) replaces the defaults unreadable rows used
    to become (the current time, `Primary`, `Concept`, `RelatedTo`, zero
    counts, `Unknown` freshness, skipped rows). Missing checkpoints are
    `NotFound`. `VectorStorage::has_session_embeddings`,
    `count_session_embeddings` and `total_count` return `Result`.
  - *Port changes.* `Storage` gains `replace_session` and
    `restore_workspace`; `list_session_summaries`,
    `save_session_with_updates`, `save_session_with_changes`,
    `FreshnessStorage::check_freshness_semantic` and `check_freshness_batch`,
    `VectorStorage::search_text` and `remove_session_vector` lose their
    default bodies (each adapter implements them). `add_session_to_workspace`
    fails with not-found when the workspace does not exist (both backends),
    and `batch_save_updates` on RocksDB when the session does not.
    `StoredEntity`, `StoredRelationship` and `StoredWorkspaceSession` are
    removed. Migration: implement the new methods; create a workspace
    before adding members.
- **Storage ports per capability (wave W3, part B).**
  - *Ports.* The `Storage` supertrait (and `GraphStorage: Storage`) is split
    into one trait per capability in `post_cortex_storage::traits`:
    `SessionStore` (sessions and their records), `CheckpointStore`,
    `WorkspaceStore` (members, projects, `WorkspaceUpdate`), `GraphStore`
    (now also `clear_session_entities`), `VectorStore`, `MessageStore`,
    `CardStore`, `DismissalStore`, `FreshnessStore`, and the optional
    `TextSearch`. `CardStorage`, `MessageStorage`, `DismissalStorage` and
    `FreshnessStorage` are renamed to `*Store`. `traits::Stores` holds one
    backend's ports as `Arc<dyn Port>` (`Stores::of(Arc<B>)`,
    `with_text_search`, `with_vectors`). `RealRocksDBStorage` and
    `SurrealDBStorage` implement the ports directly: their inherent methods
    that mirrored the trait (and the Surreal `export_*`/`import_data`/
    `export_archive`/`import_archive`, RocksDB `export_*`/`import_data`/
    `save_embedding`/`batch_save_embeddings`/`load_*_embeddings`/
    `delete_embedding`/`count_embeddings`) are gone; bring the port traits
    into scope. `compact`, `get_key_count` and `get_stats` are removed (no
    caller). Migration: `use post_cortex_storage::traits::{SessionStore, ...}`
    and call the same method names; hand a capability its `Stores` field.
  - *Vector port.* `VectorStore` stores by (session, content id):
    `upsert_vectors(batch)` replaces what is stored under the same content,
    `delete_vectors(session, content_ids)` deletes a batch of one session.
    `add_vector`, `add_vectors_batch`, `remove_vector`, `remove_session_vector`
    and the kNN searches `search`, `search_in_session` and
    `search_by_content_type` are removed: nothing read them (search runs on
    the in-memory index). Full-text search is `TextSearch::search_text`
    (SurrealDB), returning `Vec` instead of `Option<Vec>`. SurrealDB's HNSW
    index (`idx_embedding_hnsw`) and its dimension handling are removed,
    with `PC_REBUILD_VECTOR_INDEX`, `DEFAULT_VECTOR_DIMENSION` and
    `vector_index_dimension`; `SurrealDBStorage::new_with_dimension` now only
    refuses vectors of another length, as `RealRocksDBStorage` does.
    Migration: write with `upsert_vectors`, delete with `delete_vectors`.
  - *No storage actor.* `StorageActor`, `StorageActorHandle`,
    `StorageMessage`, `ConversationMemorySystem::get_storage`,
    `save_workspace_metadata` (the proxy), the `storage_actor` and
    `vector_storage` fields and `OperationType` are removed.
    `ConversationMemorySystem::stores` holds the ports; session writes keep
    going through the per-session persister (ordered, coalesced). Port calls
    return `anyhow::Error` instead of `String`, and no caller-side deadline
    turns a slow write into a reported failure. `ContentVectorizer`
    `with_persistent_storage`/`set_persistent_storage` are replaced by
    `with_store`/`set_stores(vectors, text_search)`, `remove_content` by the
    batched `remove_contents(session, &[ids])`. Migration:
    `system.storage_actor.X(..)` becomes `system.stores.<port>.X(..)`;
    session writes go through `ConversationMemorySystem` methods as before.
  - *One Transfer.* `post_cortex_storage::export_import::{export, import,
    read_import_target}` take `&Stores` and are the one export/import for
    every backend (`pcx export`, `pcx import`, `pcx migrate` and the daemon
    all use them). `pcx migrate` now also copies messages, cards and radar
    dismissals. `check_format_version` is the one version gate (import and
    preview, every backend): `1.<minor>.<patch>` up to 1.4; it replaces
    `SUPPORTED_IMPORT_VERSIONS` (SurrealDB refused 1.2.7, RocksDB accepted
    it). Compression of a file is detected from its bytes in every path.
    `export_import` re-exports its items explicitly (no globs).
  - *Export format 1.4.0* (`schemas/export/v1.4.0.json`). Every scope
    carries the same sections whichever backend wrote it: the stored
    vectors of the exported sessions (records and messages; RocksDB
    exported message vectors only), the radar dismissals of the exported
    workspaces (`ExportedWorkspace::dismissals`, new), checkpoints and
    cards. Each record is written once: in the session, whose derived views
    (`hot_context`, `current_state`, `code_references`, `change_history`)
    are written empty; `ExportedSession::updates` is left out
    (`ExportedSession::records()` reads either layout). Files of 1.0.0 to
    1.3.x import unchanged; binaries that read at most 1.3.0 refuse a 1.4.0
    file before writing anything. Migration: import with this version.
  - *Storage layouts.* RocksDB data version 2: embedding (`pcx-emb-v1`),
    message (`pcx-msg-v1`), card (`pcx-card-v1`) and radar dismissal
    (`pcx-dis-v1`) rows carry layout markers; opening a version-1 database
    marks them once, and reads decode only marked rows. SurrealDB data
    version 2 removes `idx_embedding_hnsw` and defines
    `source_reference.block_start`, `block_end` and `block_hash`. Data
    written by this version is not readable by earlier builds. Migration:
    back up before upgrading; nothing to run.

- **Capability services (wave W4, part A).** `ConversationMemorySystem`,
  `MemoryServiceImpl`, the `PostCortexService` trait (and
  `DynPostCortexService`), `post_cortex_core::SystemError` and
  `post_cortex_memory::Error` are removed. The memory API is
  `post_cortex_memory::Memory`, built by `Memory::open(config)` /
  `open_with(config, SharedModels, PipelineConfig)` / `over_stores(stores,
  config)`, with one service per capability (`memory.records`, `search`,
  `assembly`, `views`, `entities`, `sessions`, `workspaces`, `projects`,
  `cards`, `messages`, `freshness`, `transfer`, `admin`), each with one
  typed error (`RecordError`, `WorkspaceError`, ...). The runtime state
  (session cache, workspace registry, persister, pipeline) is no longer
  public. `post_cortex_core::services` keeps only the request and response
  DTOs (the `Manage*`, `Admin*`, `QueryContext*`, `SemanticSearch*`,
  `AssembleContext*`, `StructuredSummary*` and `HealthReport` types are
  gone). Daemon: `build_router(memory, host_policy, auth_token, ct)`,
  `PostCortexService::new(Arc<Memory>)`, `PcxGrpcService::new(Arc<Memory>)`
  (`from_service` removed), `start_grpc_server(Arc<Memory>, ..)`.
  `post-cortex-mcp` functions take `&Memory`; the write tools take
  `&Records`; `get_structured_summary` and `ContextQuery::AssembleContext`
  are removed. `SummaryGenerator::generate_structured_summary(_filtered)`,
  `QuestionSummary::from_question_item` and
  `EntitySummary::from_entity_analysis` take `now`; graph views render to
  the typed `RenderedView` (same JSON). Migration: replace
  `ConversationMemorySystem::new(c)` + `MemoryServiceImpl::new(m)` with
  `Memory::open(c)` and call `memory.<capability>.<operation>`.

- **Capability services (wave W4, part B).** One search operation:
  `Search::search(SearchScope, query, options)` with `SearchScope::{Global,
  Session, Sessions, Workspace}` replaces `Search::global`, `session` and
  `sessions` (a workspace is resolved by the service; unknown ids are the
  typed `SearchError::SessionNotFound` / `WorkspaceNotFound`), and
  `ContentVectorizer::search(query, limit, VectorScope, options)` replaces
  `semantic_search`, `semantic_search_shared`,
  `semantic_search_multisession` and `..._multisession_shared` (it returns
  the shared `Arc<[SemanticSearchResult]>`). `SystemConfig::
  embeddings_model_type` is an `EmbeddingModelType` (strict `FromStr` by
  variant name, `as_str` inverts it), no longer a string with a silent
  fallback; with embeddings on, `vector_dimension` must be the model's or
  `Memory::open` fails. `Admin::health` returns the typed `Health` (the
  one payload of every transport); `SystemHealth` is removed.
  `VectorDB::add_vectors_bulk` returns `(ids, BulkInsert)` and
  `ContentVectorizer::load_all_embeddings_from_storage` a `BulkInsert`.
  `AppendMessagesResponse` gains `queued`; `vectorized` is always 0 (see
  Fixed). `ExportOptions` and `ImportOptions` gain `embedding_model`,
  `ImportPreview` gains `warnings`, `ExportMetadata` gains
  `embedding_model`; `TransferError::ModelMismatch`,
  `AssemblyError::InvalidArgument`, `EntityError::SessionNotFound`,
  `FreshnessError::SessionNotFound` are new variants. The memory
  `test-support` feature (`Records::write_text` / `write_record`, a write
  path without validation) is removed: tests write through
  `Records::write`. `StructuredContext::open_questions` and
  `StructuredSummaryView::open_questions` are renamed
  `answered_questions` (they only ever held answered questions; serialized
  and stored as `open_questions`, unchanged). Migration: call
  `memory.search.search(SearchScope::Session(id), q, o)` (or `Global`,
  `Sessions(ids)`, `Workspace(id)`); parse model names with `.parse()`;
  build the options with `..Default::default()`.
- **Bulk writes are atomic on every transport.** MCP
  `update_conversation_context` with `updates` used to write the items that
  translated when another item was malformed (an unknown
  `interaction_type`); a bad item now fails the whole batch and nothing is
  written, as a validation failure already did. gRPC `BulkUpdateContext`
  is one bulk write with the same rule (it wrote item by item and skipped
  bad ones): a bad item is `INVALID_ARGUMENT` naming `updates[i]`, and
  `update_ids[i]` is the id of `updates[i]`. Migration: fix or drop the
  bad item and send the batch again.
- **Export format 1.4.1.** The metadata records the embedding model and
  dimension of the vectors (`embedding_model`; schema
  `schemas/export/v1.4.1.json`). 1.4.x readers read it (the field is
  optional); this version reads 1.0 to 1.4.x. An import into a store of
  another model is refused before anything is written (gRPC
  `FAILED_PRECONDITION`, `pcx import` error), and a preview lists it under
  `warnings`; an archive from before 1.4.1 is compared by the length of
  its vectors. Migration: export from a store with the same model, or
  without embeddings.

- **Transports (wave W5, part A): `post-cortex-mcp` is removed;
  `post-cortex-transport` is new.** The public crate list changes:
  `post-cortex-mcp` (and the facade's `post_cortex::mcp`) is gone, its
  read logic moved to the memory capabilities and its formatting to the MCP
  transport. `post-cortex-transport` holds the operation table
  (`operations::OPERATIONS`: every operation, its capability method and its
  MCP tool/action, gRPC RPC and REST route), one serde response shape per
  operation (`responses`), the JSONPath select (`select::Select`, moved from
  `post_cortex_memory::jsonpath`, which is removed), the typed failure
  classification (`Failure`, `Kind`), the shared strict parsers (`wire`) and
  the MCP server (`mcp::PostCortexService`, moved from
  `post_cortex_daemon::daemon::mcp_service`). Removed from the daemon:
  `daemon::mcp_service`, `daemon::coerce`, `daemon::validate`,
  `daemon::format_helpers`, `Error::Mcp`. `PostCortexService::new` takes the
  shared `Arc<ToolCatalog>`; `rmcp_server::build_router` returns
  `Result<Router, SchemaError>` (the tool list is built once, there).
  `post_cortex_memory::capabilities::Views` gains `entity_importance`,
  `entity_network`, `statistics` and `query(ContextQuery)` (the
  `ContextQuery` / `ContextResponse` enums moved from `post_cortex_mcp`). The
  facade re-exports `post_cortex::transport`. Migration: depend on
  `post-cortex-transport`; call `memory.views.*` for what the
  `post_cortex_mcp` functions computed; build the router with `?`.
- **One response shape per operation on gRPC (W5 item 5).** gRPC `select`
  (`SemanticSearch`, `AssembleContext` `selected_json`) runs over the MCP
  tools' shape: search hits are `{content_id, content_type, text_content,
  similarity_score, importance_score, timestamp, combined_score,
  session_id, ...}` under `{query, total_results, results}` (they were
  `{entry_id, content, score, session_id, content_type}` under
  `{results, total_matches}`), the assembly is the tool's payload
  (`query`, `token_budget`, serde `items` / `entity_context` / `impact`).
  `GetEntityImportance`, `GetEntityNetwork` and `GetSessionStatistics`
  return the MCP shapes (`{entities, pagination}` with a default limit of
  100; `{session_id, center_entity, entities, relationships}`; the MCP
  statistics). Migration: read the new keys; pass `limit` for more than 100
  entities.
- **gRPC and REST names are strict (W5 item 5).** A misspelt
  `interaction_type`, `entity_type` (`UpdateContext`, `BulkUpdateContext`)
  or session `role` (`AddSessionToWorkspace`, REST
  `POST /api/workspaces/{id}/sessions/{id}`) is `INVALID_ARGUMENT` / 400; it
  was stored as `ConceptDefined`, `Concept` or `Related`. Names are matched
  without regard to case and `_` against the wire and the stored name
  (`depends_on`, `DependsOn`, `dependson`); an empty entity type or role is
  still `concept` / `related`. gRPC relation types in `AssembleContext`
  (`relationships[].relation_type`, `impact[].relation_type`) are wire names
  (`depends_on`), no longer Debug names (`DependsOn`). Migration: send the
  documented names.

### Added

- **`pcx.v1` additive fields (W4).** `HealthResponse.total_requests`
  (`total_updates`, which counted requests, is deprecated and carries the
  same value); `RegisterSourceRequest.session_id` and
  `RegisterSourceBatchRequest.session_id` (the session as a request field;
  the `x-session-id` metadata stays the fallback for older clients);
  `AppendMessagesResponse.queued` (`vectorized` is deprecated, always 0);
  `ImportResponse.warnings`. `QueryContextResponse.total` is the number of
  records the filter matches and `ContextUpdateEntry.source_ref` is filled
  from the registered source reference.
- **Validate-only writes.** `Records::validate` / `validate_bulk` run every
  check of a write (redaction, validation, the session lookup, the
  references) and store nothing; MCP `dry_run` uses them.
- **`Records::list`** (`ListRecords { types, after, limit }` ->
  `RecordPage { records, total }`): a session's records by wire type and
  time, with the matching total and each record's source reference.
- **`Health`** (MCP `admin health`, gRPC `Health`, REST `/health`) reports
  the request counters, the circuit breaker, the cache hit rate, uptime,
  the pipeline backlog and what the vector index loaded at start
  (`vector_load`).


- **Backend profiles, hosted cloud by default.** `pcx login` / `pcx logout`
  store a profile's API key in `~/.post-cortex/credentials.json` (0600), and
  `pcx profile list | use | add | remove` manage `~/.post-cortex/config.json`
  (`cloud` active by default, `self` from `daemon.toml`; `PC_PROFILE`,
  `PC_URL`, `PC_AUTH_TOKEN` and `PC_CONFIG_DIR` override). `pcx mcp --profile
  P` is a stdio MCP server for any profile. `pcx setup` takes `--profile` and
  `--self-hosted` (`--daemon-url` / `--capture-url` still mean a self-hosted
  daemon) and logs in first when a remote profile has no key. For a remote
  profile every client gets a stdio entry running `pcx mcp --profile` and
  hooks run `pcx hook --profile`, so no key is written into any client file.
- **Setup for more coding agents.** `pcx setup --client` (repeat or
  comma-separate) sets up Claude Code, Codex, Cursor, OpenCode, Gemini CLI,
  VS Code, Windsurf, Zed, Cline, Roo Code, Goose and Claude Desktop: each
  client's MCP entry, the shared instructions block (`AGENTS.md` or the
  client's rules file) and, where the client runs hooks or plugins, those
  too. The hooks call `pcx hook <client> <event>`, which prints the cards on
  session start and what the memory knows about a file before it is edited,
  and captures turns with `--capture`. `pcx mcp-config <client>` (with
  `--profile`) prints only the MCP entry.
- **Global setup, projects recognised automatically.** `pcx setup` sets up
  once for every project the clients with a user-level config: Claude Code
  (it prints the two `/plugin` commands of the post-cortex plugin and writes
  no project file), Codex (`~/.codex/`), Cursor (`~/.cursor/`), Gemini CLI
  (`~/.gemini/`) and Claude Desktop. Their hooks run without ids and resolve
  the project from the directory the client runs in: `.post-cortex.json` at
  the git root, the ids of an earlier project setup, the cache
  `~/.post-cortex/projects.json`, then `POST /api/projects/resolve` with the
  project key (normalised git remote, else `local:<hash>`). A global hook
  steps aside in a project that has its own hooks, and `hosts.<client>` in
  `config.json` turns a client off (`enabled`), on for capture (`capture`)
  or into a named workspace (`workspace`). `pcx setup --project` keeps the
  per-project setup and also writes `.post-cortex.json`, so other clients and
  teammates resolve to the same memory; clients that read their config from
  the project only (OpenCode, VS Code, Windsurf, Zed, Cline, Roo Code, Goose)
  keep project mode. Re-running `pcx setup --project` replaces post-cortex's
  own hooks in `.claude/settings.json` (by script name, the person's hooks
  kept) and an existing Codex `[mcp_servers.post-cortex]` section, so it
  switches a project between `self` and `cloud`; a run without capture takes
  the capture hook out. The key algorithm is shared by `pcx` (`project.rs`),
  the Claude Code hooks (`pcx_resolve.py`) and the native plugins, checked
  against `integrations/testdata/project-keys.json`.
- **Project resolution.** `POST /api/projects/resolve` (MCP `session`
  `resolve_project`, gRPC `ResolveProject`) finds or creates the workspace
  and session of a project key: the same key always gets the same workspace
  and session, a deleted session is replaced, and a new key can join an
  existing workspace by name. The key is stored on the workspace and kept by
  exports.
  gRPC `FindProject` (`ConversationMemorySystem::find_project`) answers
  where a key would resolve without creating anything, so a gateway can
  authorise a resolution first.
- **Client packages** in `integrations/` (prepared, not yet published), all
  kept in step with the `pcx` templates and the workspace version by
  `integrations/sync.py --check` in CI:
  - a Claude Code plugin and marketplace (`.claude-plugin/`): MCP server,
    skill and hooks. The hooks resolve the backend (`PC_URL`, the plugin's
    `url` and `api_key` options, the active profile and its stored key) and
    the project themselves, so the cards, the file memory, the Stop check
    and the compaction re-inject work in any repository; capture runs when
    `hosts.claude-code.capture` is on. A plugin hook does nothing when the
    project runs the same script from `.claude/hooks`;
  - a Cursor plugin (`.cursor-plugin/`), a Gemini CLI extension and MCP
    Registry entries (`server.json`, `server.self-hosted.json`);
  - native plugins for OpenCode, Kilo, Pi, OpenClaw and DeepSeek Harness on
    a shared TypeScript core (`integrations/js`): the cards, the file memory
    and capture; OpenClaw also adds the top 3 records of the project's
    workspace for the prompt (prompts under 12 characters are skipped).
- **REST search.** `POST /api/search` (`query`, `scope` session, workspace or
  global, `id`, optional `mode`, `sources`, `limit` (default 5, max 50) and
  `author`) runs the search of the MCP `semantic_search` tool and gRPC
  `SemanticSearch` and returns hits as `{id, content_type, title, snippet,
  score, timestamp, author, session_id}`, so plugins can recall memory
  without MCP. Bad input is 400, an unknown session or workspace 404.
- **Workspace update and archiving.** `ConversationMemorySystem::update_workspace`
  (name trimmed, 1 to 200 characters and not another workspace's;
  description at most 4000), `set_workspace_archived` and `delete_workspace`
  change memory and storage together (rolled back in memory if storage
  fails; errors are `WorkspaceError` NotFound / InvalidArgument / Conflict /
  Storage). Archived workspaces keep their sessions and data. New gRPC
  `UpdateWorkspace`, `ArchiveWorkspace`, `UnarchiveWorkspace`
  (`WorkspaceInfo.archived` / `archived_at_unix`,
  `ListWorkspacesRequest.include_archived`), MCP `manage_workspace`
  `update` / `archive` / `unarchive`, `list` `include_archived`, and
  `WorkspaceManager::list_workspaces_filtered(include_archived)`. Storage:
  `Storage::update_workspace` with `WorkspaceUpdate`,
  `StoredWorkspace.archived_at`, `WorkspaceMetadata.archived_at`; exports
  carry `archived_at` (optional field, same format version) and imports
  restore it.
- **Export and import over gRPC.** `Export` streams the archive of a scope
  (`all`, `session` with `session_ids`, `workspace`) as the bytes of a
  `pcx export` file (optional gzip / zstd, checkpoints) in 1 MiB chunks, the
  last one with a summary. `Import` reads such bytes in chunks (compression
  detected from the first bytes), settings on the first chunk: without
  `commit` it returns a preview (counts after the filters and the sessions
  and workspaces the daemon already has, with how `skip_existing` /
  `overwrite` resolve them) and writes nothing; with `commit` it imports as
  `pcx import` does and returns the counts. No new format. Core:
  `ExportScope`, `encode_export` / `decode_export`, `preview_import`
  (`ImportPreview`, `ImportConflict`), `Storage::export_archive` /
  `import_archive`, `ConversationMemorySystem::export_archive` /
  `preview_import` / `import_archive` (queued writes flushed first; imported
  workspaces and loaded sessions are refreshed in memory). MCP has no tool
  for them.
- **Cards and message streams in exports.** The `pcx export` format (1.3.0,
  schema `schemas/export/v1.3.0.json`) carries every version of the cards
  (with authors and times) and each exported session's captured messages
  (`seq`, `turn_id`, role, author, source session, time, text, tools,
  files) with their vectors; both fields are optional, so older files read
  as before. A full export holds the user card and every workspace card, a
  workspace export that workspace's card and its sessions' messages, a
  session export the sessions' messages. `pcx export --no-messages` (gRPC
  `ExportRequest.no_messages`, `ExportOptions::include_messages`) leaves the
  streams out. Import merges messages by turn: a turn the session stores
  already is skipped, the others keep their `seq` when it was never used in
  the stream (at or past its next position) and go to the end otherwise,
  with their vectors renamed to match; RocksDB archives now also carry
  message vectors, and a running daemon loads the restored ones and embeds
  any missing. Cards: a target without the card gets the whole history, one
  history continuing the other is fast-forwarded, and a diverged card keeps
  the target's latest version unless `--cards-newer` takes the archive's
  when it was written later (added as the target's next version, so nothing
  is lost) or `--overwrite` replaces the target's history; the preview
  reports diverged cards as conflicts (`kind` `card`, `resolution` `keep`,
  `update` or `overwrite`) with both latest versions.
  Messages and cards of a session or workspace the target has merge in when
  it is skipped (`--skip-existing`) or overwritten, not when it is refused
  as existing. Counts: `ExportMetadata.message_count` / `card_count`,
  `ImportPreview.messages` / `messages_new` / `cards`,
  `ImportResult.messages_imported` / `messages_skipped` / `cards_imported` /
  `cards_skipped`, and the same in gRPC `ExportSummary` and `ImportResponse`
  (new fields only). `preview_import` takes an `ImportTarget`.
- **Graph views for agents.** `query_conversation_context` types `entity`,
  `impact`, `lineage`, `topics`, `radar` and `file` (JSON, also gRPC
  `GetGraphView`): an entity with its records and decisions, who depends on
  it with the risks, the causal chain, topic communities, what needs
  attention, and what the records say about a file (`GET /api/file`). `pcx
  setup` installs a PreToolUse hook that shows the agent past problems,
  decisions and dependents of a file before it edits it.
- **Radar dismissals.** Radar items can be dismissed per workspace by kind
  and key (`post_cortex_core::core::dismissal`: `unsolved_problem` and
  `stale_entity` by entity name, `decision_without_rationale` by record id,
  `open_question` by question text, `alias_candidate` by `<name>|<of>`),
  with time and author. The radar view of the workspace and of its member
  sessions leaves them out (before the lists are cut to `limit`) and lists
  the matching dismissals under `dismissed`; `include_dismissed=true` keeps
  them (MCP `query_conversation_context`, gRPC `GetGraphView`).
  `GraphScope::radar_view_with` / `render_with`,
  `ConversationMemorySystem::dismiss_radar_item` / `restore_radar_item` /
  `radar_dismissals`, gRPC `DismissRadarItem` / `RestoreRadarItem` /
  `ListRadarDismissals`, MCP `manage_workspace` `dismiss_radar_item` /
  `restore_radar_item` / `list_radar_dismissals`. Storage:
  `DismissalStorage` (RocksDB `radar_dismissal:{workspace}:{kind}:{key}`,
  SurrealDB table `radar_dismissal`); dismissals go with their workspace.
- **Record validity.** Records hold until a later record supersedes them
  (new `supersedes` on write, or the same fact restated about the same
  entities), a later `conflicts_with` contradicts a relation they asserted,
  or they are forgotten. Writes take `valid_from` and `confidence` too.
  Search marks records that no longer hold (`valid_to`, `ended_by`,
  `end_reason`) and ranks explicitly ended and low-confidence ones lower.
- **Entity resolution.** Names that differ only in case, spaces, `-` or `_`
  are one entity, with the other spellings as aliases (also for graphs
  stored before, on load). `manage_entity` `merge` (gRPC `MergeEntities`)
  merges synonyms.
- **Forget and purge.** `manage_entity` `forget` ends a record and keeps its
  history; `purge` erases it with its embedding, full-text entry and the
  entities and edges only it introduced (gRPC `ForgetUpdate`,
  `PurgeUpdate`).
- **Records link to their entities.** Every record is linked to the
  entities it mentions, on both backends, rebuilt from the records on load.
- **Importance from the graph.** Entity importance is weighted PageRank over
  the typed relations, personalised by mentions and faded for entities not
  mentioned for months; entity tools and summaries rank by it. Relation
  types weigh differently in the retrieval graph too (from `depends_on` 1.0
  to `related_to` 0.4).
- **Retrieval traces.** `semantic_search` `trace: true` (gRPC `trace`) adds
  each result's rank in the vector, BM25 and graph rankings and its fused
  and reranker scores.
- **Message deletion.** `ConversationMemorySystem::delete_message(session,
  seq)` and `delete_messages(session)` delete one captured turn or a
  session's whole stream with its vectors, ordered with the session's other
  writes; the stream's count and stats follow, positions are never reused
  (`message_next` stays) and a deleted turn can be captured again at a new
  position. Records whose `derived_from` names a deleted turn keep the id as
  provenance. gRPC `DeleteMessage` / `DeleteMessages` (`deleted`,
  `remaining`), MCP `session` `delete_message` (`seq`) / `delete_messages`,
  `MessageStorage::delete_message` / `delete_session_messages` on both
  backends.
- **Message stream stats and paging.** `MessageStorage::message_stats`
  returns a session's `MessageStats {count, first_seq, last_seq, first_at,
  last_at}` without loading the stream, and `load_messages_page(session,
  cursor, limit, order)` reads up to `limit` messages `From(seq)` or
  `Before(seq)`, ascending or descending (`(Before(u64::MAX), Desc)` is the
  newest page). Positions retention removed are skipped, so stats start at
  the oldest message left and a page is full while the stream has messages
  on that side. RocksDB reads bounded key ranges (forward or reverse) and
  keeps a per-stream count (`message_count:{session}`, counted once for
  streams written before it); SurrealDB answers from
  `idx_message_session_seq` (count scan, bounded scans with `LIMIT`). Both
  are on the storage actor and `ConversationMemorySystem`.
- **Record-written notification.** `ConversationMemorySystem::subscribe_records()`
  returns a `broadcast::Receiver<RecordEvent {session_id, entry_id,
  timestamp, kind}>`, `kind` being `Written`, `Forgotten` (`forget`) or
  `Removed` (`purge`, `delete_update`, source invalidation). Events are sent
  by the session persister once the storage write carrying the change has
  finished, in write order per session; records of a failed write are
  announced with the next successful one. The channel holds
  `RECORD_EVENT_CAPACITY` (1024) events per receiver and never blocks
  writers: a receiver that falls behind gets `RecvError::Lagged(n)` and
  should re-read what it watches.
- **Shared models across memory systems.** `SharedModels {embedding_engine,
  reranker}` with `ConversationMemorySystem::new_with_models(config, models)`
  or `.with_shared_models(models)` hands an `Arc<LocalEmbeddingEngine>` (and
  a local reranker) to the vectorizer instead of loading one per system, so
  N memory systems in one process hold one model. An engine whose dimension
  differs from `vector_dimension` is refused at vectorizer init. Without it
  nothing changes. Also `ContentVectorizer::with_engine`,
  `ContentVectorizer::embedding_engine` and `LocalEmbeddingEngine::model_type`.
- **Python and TypeScript SDKs** (`sdks/python`, `sdks/typescript`, packages
  `post-cortex` on PyPI and npm): clients of the gRPC API with sessions,
  workspaces, records, search, context assembly, graph views, cards,
  messages and entity upkeep; generated code checked against `pcx.proto` in
  CI, end-to-end tests against a real daemon.

### Changed

- **Vectors of a deleted session are deleted from storage too**: removing
  a session's vectors (session delete, an embedding that finished after the
  delete) goes through the search index, which deletes the persisted copies
  with the in-memory ones; before, vectors an embedding stored after the
  session was deleted stayed in storage.
- **`pcx export`, `import` and `migrate` run the Transfer capability** over
  the stores they open (embeddings off). An export reads the vectors
  through the search index: vectors of another dimension than the daemon's
  model are left out, and a store that holds only such vectors fails the
  export. Each session's vectors are listed in content-id order. An import
  of another model's archive is refused with Transfer's message
  (`Import refused, nothing was written: ...`).
- **Session views are derived from the records** (W2). Current state,
  conversation flow, code references, the hot window and the change history
  are computed from a session's records on read, the same way on both
  backends: a SurrealDB session no longer gets its own rules (decision
  confidence 0.9/0.7, concepts from entity names, 50-record hot window), and
  a record removed from a session leaves nothing behind in any view. The
  stored session keeps its fields: the derived ones are written from the
  records (`warm_context` and `cold_context` are written empty, change ids
  are the record ids) and ignored on read. Session statistics report 0 for
  the warm and cold tiers. A session's `last_updated` is the time of its
  latest record or metadata change, not the clock at apply time.
- **Entity counts and times are a union** (W2). Merging graphs (workspace
  views, the cloud) sums mention counts and keeps the earliest first and the
  latest last mention and the highest importance; an endpoint a relation
  creates is stamped with the record's time, so replays are deterministic.
  This feeds entity importance and the alias chosen as canonical.
- **Entity names resolve everywhere** (W2): related entities, entity
  context, traces, networks, shortest paths and relation lookups accept
  aliases and any spelling the resolution key joins.
- **Retrieval graph** (W2): agent entities are keyed by the resolution key,
  so a relation joins the records of every spelling of its entities, and
  seed only when the query names them as whole words ("Go" is not in
  "good"); its PageRank runs in f64 through the one kernel shared with
  entity importance. Context assembly splits camelCase and acronym entity
  names before matching them against the query. These change rankings for
  sessions with agent entities.
- **Session cache eviction** (W2): least recently used sessions over
  `cache_capacity` are evicted when a session is installed, except sessions
  still in use, with writes not yet stored or with derived work queued; the
  validity and retrieval-graph caches of evicted sessions go with them.
- A vector with an unknown content type fails the search ("Corrupt content
  type in vector metadata") instead of being treated as update content.
- gRPC `WorkspaceInfo.sessions` are listed by session id.

- The card, file-context and capture hooks moved into
  `templates/settings.json`, the one source of `pcx setup --project` and the
  Claude Code plugin's `hooks.json`.
- `ViewRequest::Radar` has `include_dismissed`; workspace list and get show
  `archived_at`.
- A refused project resolution shows the server's reason: `pcx` hooks print
  `HTTP 403 Forbidden: <message>` instead of the status alone, and the
  Python hooks (`pcx_resolve.py`) and the JS plugin core still fail open but
  write the message to stderr.
- `pcx profile list` shows the URL actually used when `PC_URL` overrides a
  profile (`http://127.0.0.1:8080 (PC_URL; profile: https://api.post-cortex.com)`)
  and marks `(PC_PROFILE)` when the environment picked the active profile.

### Removed

- The compaction re-inject hook `pcx-compact-reinject.sh`; its replacement
  `pcx-compact-reinject.py` also prints the session and workspace.
  Re-running `pcx setup --project` in a project set up before removes the
  old script and installs the new one.

### Fixed

- **Each capability service holds the ports of its row** (ARCHITECTURE
  rule 4): Sessions, Entities, Records, Messages, Freshness, Cards,
  Workspaces and Transfer get their stores at composition; the runtime keeps
  the session cache and shared state and no longer holds the archive codec.
  `check-architecture.sh` counts any service reaching into the runtime's
  ports (`capability_ports`, budget 0).
- **`proto/pcx.proto` is documented** (every message, field and enum value);
  `post-cortex-proto` is back on the workspace lints, `missing_docs` included.
- **The daemon supervises its servers** (`daemon::bootstrap`, split from
  the routes in `rmcp_server`): HTTP and gRPC run as watched tasks; one that
  stops (a failed gRPC bind included) stops the daemon with its error
  instead of leaving it half up. At shutdown both drain their requests
  (gRPC now shuts down gracefully, `start_grpc_server` takes a cancellation
  token) before every session is written; warm-up and message retention
  are owned tasks. An embedding batch that panics is logged and lost, the
  worker keeps running.
- **Input checks live in the capabilities, once**: an empty search query,
  find query, entity name, merge target or related-content topic, and a
  metadata update with neither name nor description, are
  `InvalidArgument` in every transport (MCP did not check the search query).
- **gRPC errors keep their kind**: handlers map capability errors through
  the shared mapping (a missing session is `NOT_FOUND`, not `INTERNAL`);
  `CheckFreshness` returns the error instead of answering `Unknown` for
  every entry, and refuses `checks` of another length than `entry_ids`;
  export failures are no longer classified by their text.
- **REST `/api/search` titles come from the search** (records carry
  `title`); the route no longer reloads sessions. With embeddings off it
  answers 412 (failed precondition) like the other transports.
- **No silent defaults left**: a workspace whose `archived_at` cannot be
  read, an index entry with a bad chunk number, or a store that cannot list
  workspaces at start are errors; `pcx setup` reports a local listing
  failure; compression names are exact in the codec (any case on the
  command line and gRPC).

- **`semantic_search` honours `interaction_type`.** The filter was accepted
  and ignored; it now keeps only records of that type (judged by the record
  itself, ranking four times the limit so the filter still fills it).
  `SearchOptions::update_types` carries it for every transport.
- **Strict names inside, tolerant ones at the edge.** `MessageRole`,
  `RadarItemKind` and `CardScope` parse only their exact names; stored rows
  with any other name are reported corrupt instead of defaulting. Clients
  still send any case and `-`/`_` spelling through the transport, which
  refuses unknown names with the valid ones listed.
- **Queries that need a text say so.** `get_entity_context`,
  `search_updates`, `find_code_references` and the other queries with a
  required text refuse an empty one (`"<field> is required for this
  query"`) instead of running on `""`.
- **`get_recent_changes` without `since` covers the last 30 days** in every
  transport (the capability applies the default, not each caller).
- **One date parser.** `post_cortex_core::time::parse_time` (RFC 3339,
  `YYYY-MM-DD HH:MM:SS`, `YYYY-MM-DD`) replaces the copies in the graph views
  and the transport; MCP date ranges now accept a plain date.
- **Session create/update/delete and dry runs reply with data**
  (`SessionCreated`, `SessionUpdated`, `SessionDeleted`, `DryRun`), not text
  only.
- **An API key is never dropped silently.** `bearer_client(token, author,
  timeout)` returns `Result<_, ClientError>`: a key or author that cannot be
  a header, or a client the system refuses, is an error instead of a client
  without `Authorization`. `DaemonConfig::http_client` returns it too; `pcx`
  hooks take their client from it (the hook's copy is gone) and report the
  error on stderr.
- **Damaged RocksDB rows are `Corrupt`, not `Unavailable` or partial.** An
  update index whose length is not a whole number of ids, a malformed
  message counter or key, and a malformed card head are reported corrupt;
  the index remainder was dropped before.
- **The operation table names the Freshness methods that exist**
  (`check_batch`, `register_symbol_dependencies`, `stale_entries`).
- **`pcx export` reads the database next to a running daemon.** It opens
  RocksDB read-only (no lock, no migration) and SurrealDB without schema
  statements, through the composition root (`compose::open_storage` with
  `Access::ReadOnly`). `pcx import` and `pcx migrate` open for writing and,
  on RocksDB, refuse while the daemon is listening with how to stop it,
  instead of matching the lock error's text. The three commands share one
  offline path (`open_offline`, `offline_memory`) instead of three copies,
  and choose the backend from the strict `storage_config()`: the `== "surrealdb"`
  string comparisons are gone.
- **No Debug names on disk or on the wire.** SurrealDB update rows store the
  update type's codec name and REST `/api/search` sends the content kind's
  wire name (both byte-identical to before).
- **SurrealDB card conflicts are detected by the error kind**
  (`is_already_exists`), not by matching the message.
- **`pcx` lives in `src/bin/pcx/main.rs`**: its modules are found by Cargo, the
  16 `#[path]` attributes are gone. `pcx hook` reports unreadable stdin and
  non-JSON input on stderr (it still fails open).

- **Search with embeddings off fails the same way in every scope and
  transport.** `Search::search` and `Search::related` return the new
  `SearchError::EmbeddingsDisabled` (`FAILED_PRECONDITION`, "Embeddings not
  enabled or initialized") when the configuration turns embeddings off. The
  check was in the MCP tools only and skipped the workspace scope, so gRPC
  `SemanticSearch` and MCP workspace searches ran anyway; they now get the
  typed error too. An MCP search of an empty workspace still answers with its
  note, and REST `POST /api/search` keeps its own 503 check.

- **MCP error details are data (W5 item 9).** A failed tool call's message
  is the human message; its details (`kind`, `parameter`, `expectedType`,
  `receivedValue`, `hint`, `suggestions`) follow as data in the `Details:`
  block, where they used to be JSON encoded into the message. Every tool
  error now names its `kind` (`invalid_argument`, `not_found`, `conflict`,
  `failed_precondition`, `unavailable`, `internal`), the same kind gRPC and
  REST map to their codes. The argument at fault is taken from the typed
  error (serde's path, or the required field the schema lists) and the
  hints from it and the kind of problem, not by searching the error text.
- **The MCP tool list is built once (W5 item 10).** The schemas were rebuilt
  for every MCP session and a failed conversion became an empty schema; the
  list is built when the router is, and a failure stops the start.
  `tools/list` is unchanged (a golden test pins it).
- **gRPC `GetEntityNetwork` honours `max_entities` and `max_relationships`**
  (they were ignored); the network is bounded as over MCP.
- **gRPC and REST map errors in one place each**, by the failure's kind: a
  storage failure of a summary, decision or insight call is `INTERNAL`, no
  longer `NOT_FOUND`; message deletion stopped guessing not-found from the
  error text; an open write breaker is `UNAVAILABLE` / 503.
- MCP `semantic_search` in workspace scope returns `query` and
  `total_results` like the other scopes, and session-scope hits carry their
  `session_id` (one shape). Response JSON follows the struct field order and
  writes scores as `f32` (`0.70000005`, it was `0.7000000476837158`).

- **Workspace create validates and keeps its creation time** (W4 item 7).
  Every create (MCP, gRPC, REST, `pcx`, project resolution) goes through
  `Workspaces::create`: the name is trimmed and must be 1 to 200
  characters, the description at most 4000 (bad input is invalid-argument /
  HTTP 400 instead of a nameless workspace); the creation time a client
  sees is the stored one and survives a restart (it was reset on every
  start).
- **One workspace delete** (W4 item 8): MCP, REST, gRPC and `pcx` delete
  through the locked path, storage first, and the workspace's merged graph
  is dropped (MCP removed memory before storage, REST bypassed the lock).
- **One assemble scope** (W4 item 9): an unknown session in MCP
  `assemble_context` is invalid params like an unknown workspace (it was an
  internal error); `query_conversation_context` `assemble_context` is the
  same assembly as the tool and now includes the cards; the merged
  workspace graph cache is per memory instance, holds at most 64
  workspaces and is dropped on delete and membership change (it was
  process-wide, shared by tenants and never evicted).
- **One structured summary** (W4 item 10): MCP and gRPC use
  `Views::summary`; insights have their own limit; summary ages and
  recency are measured from one clock read per call.
- **Typed writes** (W4 item 12): a write is built into its record once and
  committed without a JSON round trip or `.expect`; the fabricated
  "Incremental Update" record of description-only writes is gone.
- **Search results of different sessions merged** (W4 item 14). Results
  were deduplicated by content id alone, so a global or workspace search
  kept one `entity:<name>` (and one `message:<seq>`) for all sessions that
  have it; they are deduplicated by (session, content id) now. The order is
  total (score, then newest, then session and content id) instead of the
  hash map's order on equal scores (`partial_cmp().unwrap_or(Equal)`), and
  the rank fusion breaks its ties the same way. Ranking-affecting: a
  multi-session search can return entities and turns it dropped before;
  equal scores now have one order.
- **One search pipeline** (W4 item 11): global, session, multi-session and
  workspace search share one scope resolution, preparation and
  completion (the vectorizer had two copies of the ranking); a session
  search returns the record's own text (the session-only "graph
  enrichment" appended `[System Knowledge Map]` / `(Graph expansion: ..)`
  to hit text and could hide the full text); the query-cache key includes
  the retrieval-graph candidates, so a rebuilt graph is a new key; a search
  over an empty session set returns nothing (BM25 read it as every
  session).
- **gRPC QueryContext** (W4 item 15) filtered by a substring of the Debug
  name (`"d"` matched every type, wire names such as `decision_made`
  matched none), reported the page size as `total`, sent Debug names
  (`DecisionMade`) and no source reference. It filters by wire names
  (comma-separated; an unknown name is `INVALID_ARGUMENT`), sends wire
  names, the real total and the source reference.
- **Startup vector load** (W4 item 17): a load that failed (or whose task
  panicked) was logged and reported as 0 vectors, and `add_vectors_bulk`
  dropped vectors of another dimension silently, so a model switch left
  search empty. Both are errors now (the vectorizer start fails and is
  retried); a partial dimension mismatch is counted in `Health`.
- **Message capture** (W4 item 18): `append_messages` embedded the turns
  inline on the hook capture path (the first call loaded the model) and
  loaded the whole session to check it exists. It checks the session
  without loading it, stores the turns and queues their embedding on the
  derived-work pipeline; the response's `vectorized` is 0 and `queued` says
  how many turns wait for their vectors.
- **Embedding model** (W4 item 19): an unknown model name fell back to
  PotionMultilingual with a warning; the configured dimension could
  disagree with the model; `ContentVectorizer::new` did not check the
  engine's dimension. The name is a strict enum, the dimension is checked
  at open, and `new` checks the engine like `with_engine`.
  `ContentVectorizerConfig::default()` sized its index for 384-dim vectors
  while the default model makes 256-dim ones; it takes the default model's
  dimension.
- **Health** (W4 item 20): MCP and gRPC labelled the request count
  `total_updates` and built the payload twice; REST `/health` returned a
  static `{status, service}`. One `Health` payload, `total_requests` named
  as what it counts, used by all three (`total_updates` stays as the same
  count).
- **MCP dry run** (W4 item 21) only checked the interaction type and that
  the session exists; it passed writes a real write refuses (a relation to
  an undeclared entity, an unknown superseded entry). It runs the write's
  own checks through `Records::validate`.
- **Invalid numeric query parameters** (W4 item 22) became defaults
  (`limit=ten` meant 10): `query_conversation_context` parameters are
  parsed strictly and an invalid one is invalid params naming it; an
  unknown `entity_type` of `find_entities_by_type` is invalid too (it was
  `concept`).
- **Assembly refuses an empty query** for every transport (W4 part B): the
  check was in the MCP tool only, so gRPC `AssembleContext` and the
  `query_conversation_context` alias assembled for an empty query. An
  empty or blank query is `AssemblyError::InvalidArgument` (MCP invalid
  params, gRPC `INVALID_ARGUMENT`).
- **Importance levels** (W4 item 10): entity summaries bucketed the
  `(0, 1]` importance score on the old 2.0 / 1.5 / 1.0 scale, so every
  entity was at most `Low`. The levels sit on the `(0, 1]` scale
  (critical from 0.8, high 0.5, medium 0.25, low 0.1), next to
  `analytics::importance`.
- **Typed not-found** for record delete, forget and purge, entity delete
  and merge, and file invalidation of an unknown session (they were
  storage errors).
- **SurrealDB kept entities and edges a session lost.** A save upserted the
  snapshot's graph and never deleted, so entities removed by an
  invalidation (records removed, graph rebuilt) came back on the next
  load. A save now deletes the stored entities and edges the snapshot
  lacks (by key, or by a sweep when this process has not read the stored
  graph), as RocksDB's blob always held exactly the snapshot.
- **SurrealDB read block (line-range) source references as the whole
  file.** Their lines and hash are stored now.
- **Duplicate vectors.** Embedding a record again (a retry after its mark
  was lost, or two jobs racing for it) appended a second in-memory vector
  that took a result slot; an insert now replaces the vector of the same
  session and content. The vectorizer writes storage first, then memory,
  both upserts, so nothing is rolled back by hand.
- **Undo after a session was deleted mid-embedding** deletes the batch's
  vectors in one store call instead of one per record.
- **Waiting writes reported timeouts for writes that landed** (the storage
  actor's 30 s caller-side deadline, and a nested 5 s one on
  `register_source`).

- **Import `--overwrite` replaces a session** (it merged: records, graph
  rows, vectors and messages the target gained since stayed), as it
  replaces a workspace. A workspace overwrite no longer deletes the
  target's card and radar dismissals first.
- **Workspace creation time and member roles survive** saves, imports and
  overwrites (both backends reset `created_at` to now; RocksDB dropped the
  members `save_workspace_metadata` was given, SurrealDB reset existing
  members to `Primary`). `pcx import` writes a workspace in one write, and
  `pcx migrate` copies workspaces with their creation time, roles, projects
  and archive state.
- **SurrealDB session listings report the session's last update**, not the
  time of the last save; a failed graph read fails the load instead of
  loading (and caching) an empty graph.
- **RocksDB workspace writes are atomic under the workspace lock**: a
  delete racing an update or a membership can no longer resurrect the
  workspace or leave an orphan member.
- **RocksDB `delete_entity` removes the entity's edges** and the graph
  operations act on the session as stored (they wrote rows nothing read).
- **The SurrealDB save cache never drops a slot a save holds**, so two
  saves of one session cannot run at once.
- **Freshness**: SurrealDB symbol dependency and invalidation writes are
  checked; an unknown stored status is an error, not `Stale`; RocksDB keeps
  line-range scopes as registered.

- **Bulk ingest keeps up with the writers again.** Since the bounded
  embedding queue (W0), one worker embedded one record at a time: an
  embedding call, a vector store round trip and a session persist per
  record, about 30 records/s against a SurrealDB server, so bulk writes
  waited for queue room (LoCoMo gate: API 161 s instead of 41 s). Workers
  now drain up to `embedding_batch` queued records at a time and embed each
  session's share with one `encode_batch`, one vector store batch, one
  session persist and one pass over the entities they name (each once);
  in-memory index inserts run on a blocking thread. Sessions are sharded
  over `embedding_workers` workers, so a session's records stay in write
  order and the queue stays bounded. Bench `bench_bulk_ingest` (3000
  records in bulk calls of 600, simulated 2 ms server round trip):
  vector store writes 3001 to 26, until vectorized 11.9 s to 0.22 s
  (RocksDB) and 14.0 s to 1.41 s (SurrealDB `mem://`).
- **A write's record is stored as the session holds it.** A description
  over 64 KiB (or a title over 200 bytes) given in a write's metadata was
  cut in the session but stored and returned uncut.
- **Session mutators no longer need a Tokio runtime**: they armed
  timeouts that panicked outside one.
- **Decisions keep their stated confidence** (`min_confidence` filters on
  it, the summary orders by it); a record without one counts as fully
  confident, as in search.
- **The conversation flow is bounded** (500 steps) on every path.
- **Eviction no longer loses work**: the session cache evicted any session
  when it filled up, including one whose writes or embeddings were still
  pending, and the per-session caches were never pruned.

- **A misspelt `storage_backend` no longer runs RocksDB.** `daemon.toml` /
  `PC_STORAGE_BACKEND` values and `pcx migrate` backends are parsed strictly:
  an unknown name (or `surrealdb` in a build without `surrealdb-storage`)
  fails at startup with the supported names, instead of silently using
  RocksDB.
- **Failed MCP tool calls are tool errors.** A rejected write, a timeout,
  bad input, a partial bulk failure or a handler failure now returns
  `isError: true` with the error text (it was reported as success, or as an
  opaque JSON-RPC error clients do not show). A call to an unknown tool stays
  a JSON-RPC error. A bulk `update_conversation_context` returns one result
  per input item in input order (`{index, entry_id}` or `{index, error}`),
  and a single write returns its `entry_id`, `session_id` and `durable`.
- **File invalidation is scoped to the session.** Invalidating a file for
  one session marked every session's source references to it as invalid;
  it now reaches only that session's references (plus references stored
  before references recorded their session). gRPC `Invalidate` without a
  `session_id` is the explicit invalidation of every session. The removal of
  the session's records is retried under contention and any failure is
  returned (it returned success after losing a race). RocksDB now keeps
  invalidated references as stale like SurrealDB (both follow one state
  machine): `CheckFreshness` reports `STALE` instead of `UNKNOWN`,
  `GetStaleEntriesBySource` lists orphaned symbols, and cascade
  invalidation marks instead of deleting. A missing `x-session-id` is
  `INVALID_ARGUMENT` (was `UNAUTHENTICATED`); batch registration runs at
  most 16 writes at a time.
- **Invalidated records leave every view of the session.** Records removed
  by a file invalidation stayed in the hot context, current state,
  conversation flow, change history and entity mentions (summaries kept
  returning them); removing a hot record no longer leaves a gap that hid
  the newest records from recent-context readers.
- **Deleting a session deletes its checkpoints** on RocksDB and SurrealDB
  (they still loaded and were exported).
- **One write pipeline for gRPC, MCP and REST.** The daemon started gRPC on
  a second service with its own write pipeline and redactor, so gRPC writes
  were not ordered with MCP and REST writes.
- **A full derived-work queue makes writers wait.** It spawned one task per
  item (unbounded embeddings, out of order); a writer now waits for room
  and, if the queue stays full, embeds the record itself. A record is
  stored with its graph merge in one write (it took two more session
  writes), and one function embeds and commits (the five copies drifted:
  `vectorize-all` kept vectors of sessions deleted meanwhile).
- **Exports never report success with missing sessions.** Full and
  workspace exports skipped sessions that failed to load; every scope now
  fails and names them. `include_checkpoints` carries the checkpoints of the
  exported sessions in every scope (only full exports had any).
- **The stdio proxy answers every request.** A failed HTTP call left the
  client waiting forever; it now gets a JSON-RPC error. After a daemon
  restart the proxy initialises a new MCP session (every call failed
  before), and server-sent events with an `id:` line are no longer dropped.
- **Smaller MCP fixes.** `assemble_context` with an empty `workspace_id`
  and no session is an invalid request (it panicked); an empty id next to
  the other one is ignored. `get_structured_summary` with `include` returns
  the data of the chosen sections (it returned only a count) with their own
  limits, and supports `questions`; its full output is labelled `Results:`
  like every other tool. gRPC `CreateWorkspace` returns an error when the
  workspace could not be stored (it reported success); every create path
  writes storage first.
- **Debug logging no longer panics on non-Latin text.** The record text
  preview cut titles and descriptions at byte 50 and panicked inside a
  multibyte character when `RUST_LOG=debug`, stopping vectorization.

- **An import no longer replaces a newer card with an older one.** For card
  histories that diverged, the import compared `(version, updated_at)`, so an
  archive's v2 written a day earlier beat the target's current v1 and became
  its head, also with `--skip-existing` ("Keep what is here"). Version
  numbers of two independent histories are not comparable: a diverged card
  now keeps the target's latest version by default (any mode but
  `--overwrite`) and the preview reports it as `keep`. The archive's latest
  version is taken only when it was written later by `updated_at` and the
  import asks for it: `pcx import --cards-newer`, gRPC
  `ImportSettings.cards_newer` (field 6), `ImportOptions::cards_newer`,
  `cards_newer=` / `cardsNewer` in the SDKs. `--overwrite` still replaces
  the history; histories where one continues the other fast-forward as
  before. Card conflicts now carry both latest versions (version, time,
  first line): `ImportConflict.card` (`CardHeads`), gRPC
  `ImportConflict.target` / `archive` (`ImportCardHead`), printed by
  `pcx import` before it writes. No change to the 1.3.0 file format.

- **Message vectors left without their message, and messages never embedded
  again.** An archive imported before 1.3.0 (or an incomplete delete) could
  leave message vectors at positions that have no message in the stream; a
  stream captured afterwards starts at those positions, and
  `vectorize_messages` skipped any turn that already had a vector, so new
  turns kept another text's vectors and a turn with only some of its chunks
  was never completed. Export and import were not losing chunks: such a
  tenant exported every vector (1328, 326 of them for 32 messages) and the
  import kept only those of the exported messages (48). Now:
  - `vectorize_messages` skips a turn only when its stored vectors are
    exactly the chunks of its text (same ids, same texts); otherwise the old
    vectors are removed from memory and storage and the turn is embedded
    again. A turn too short to embed keeps no vectors.
  - Repairing a session's messages (on import, `admin` `vectorize_session`,
    and the new `pcx vectorize-all --messages`) also removes vectors of
    positions no stored message has and chunk sets with gaps.
    `pcx vectorize-all` repairs the messages after the records.
  - `pcx export` leaves out message vectors whose message it does not
    export, so its counts say what an import writes.
  - Deleting messages before the vectorizer started removed only the first
    vector of a chunked message from storage; its chunks go with it now.

- **`pcx export` could drop the HNSW index of a SurrealDB database a daemon
  was serving.** `pcx export`, `pcx import` and `pcx migrate` opened SurrealDB
  through `SurrealDBStorage::new`, which ran the schema initialisation with a
  hard-coded HNSW dimension of 384, while the daemon and its stored vectors
  use the default model's 256. Schema init saw the index at another
  dimension, ran `REMOVE INDEX` and then a `DEFINE INDEX ... DIMENSION 384`
  that failed on the stored 256-dim vectors, leaving the database without
  its vector index (and kNN search) until the daemon was restarted. Running
  any of these commands against a production database was enough. Now:
  - Schema init never drops an HNSW index whose dimension differs while the
    embedding table has rows. It fails with an error naming both dimensions,
    the number of stored vectors and how to migrate (export, import into a
    fresh database and run `pcx vectorize-all`, or set
    `PC_REBUILD_VECTOR_INDEX=1` to delete the stored vectors and rebuild the
    index). Stored vectors of another dimension with no index are refused the
    same way. Only an empty table, or the explicit opt-in, is redefined, and
    the `REMOVE` and `DEFINE` run in one transaction, so a failed `DEFINE`
    keeps the old index.
  - `pcx export` opens SurrealDB read-only (`SurrealDBStorage::open_existing`):
    no schema statement runs, and a database without the post-cortex schema
    (for example a wrong namespace) is reported instead of created.
  - Every pcx command that opens storage (`import`, `migrate`,
    `vectorize-all`, the local admin commands) takes the storage settings,
    embedding model and dimension from `DaemonConfig::storage_system_config`,
    the same resolution the daemon starts from.
  - `SurrealDBStorage::new` defaults to `DEFAULT_VECTOR_DIMENSION` (256, the
    default model) instead of 384.
  - `embedding.metadata` is rewritten (`DEFINE FIELD OVERWRITE`) only when it
    is not FLEXIBLE yet, instead of on every start.
- **Records written just before a stop were stored but never searchable.**
  `flush_all` (and so daemon shutdown) did not wait for the embedding and
  graph work queued on the pipeline, so its vectors were lost with the
  process, and nothing embedded those records again: only a session-scoped
  search re-vectorized. `flush_all` now waits for queued and running
  embeddings and graph merges (at most three quarters of its limit) before
  it writes the session persists they produce, and eviction keeps a session
  in memory while it has such work. After a start, the first global or
  multi-session search (or `warm_up_embeddings`) runs
  `ConversationMemorySystem::vectorize_missing_records` once: records of
  stored sessions that have no vector are embedded, sessions whose records
  all have vectors are not cached. Re-vectorizing is idempotent: an update
  whose vector is already stored is marked, not embedded again.
- **A failed storage write lost its removals and retirements.** The retry
  (the session's next write, `wait_idle` or `flush_all`) wrote only the
  session blob, so a `delete_update`, `purge` or source invalidation (row
  deletes) and a `forget` (`retired_at` row rewrite) of the failed write
  never reached storage: rows deleted or forgotten in memory came back on
  the next load. The persister now keeps the update rows and removals of a
  failed write and carries them in the retry (the newer write's version of
  a row wins), and the record events of the failed write, `Forgotten` and
  `Removed` included (they were dropped), are sent with the write that
  succeeds. A retry whose handle was dropped uses the session's newest live
  handle.
- gRPC `DeleteWorkspace` removed the workspace from memory only, so it came
  back on the next start; it now deletes it from storage too (memberships,
  card, radar dismissals).
- Cards written over MCP (`manage_workspace` `update_card`) or REST
  (`PUT /api/cards/...`) without an author take the `X-PCX-Author` header,
  and over MCP the client's name, like records do; MCP `update_card` also
  takes `author`. `PC_AUTHOR` now applies to every `pcx` request (card edits
  too), not only the stdio proxy.
- `pcx setup --daemon-url` (and `--capture-url`) created the session and
  workspace in the daemon of `daemon.toml` (or its storage) instead of the
  daemon at that URL.
- `--no-default-features` builds of `post-cortex-memory`, `post-cortex-mcp`
  and `post-cortex-daemon` failed to compile (`EmbeddingConfigHolder`
  re-export, MCP `semantic_search`, the daemon's search result formatter).

## [0.5.0] - 2026-10-20 — Retrieval pipeline, conversation capture, cards and authorship

### Added

- **Measured retrieval pipeline.** On LoCoMo (1535
  questions, all@50 with ±1 neighbours, through the daemon): Model2Vec only
  0.704, `fast` 0.793, `local` 0.878, `accurate` 0.936; end to end with
  GLM-5.3 68.2% → 85.0%; BEAM 0.720 → 0.756.
  - BM25 next to vector search: a SurrealDB `FULLTEXT` index on
    `embedding.text`, in-memory postings on RocksDB. Very common query words
    are dropped by document frequency instead of a stopword list. Vector and
    lexical rankings are fused with RRF.
  - Rerankers: a local int8 cross-encoder on ONNX Runtime (feature `rerank`,
    default `cross-encoder/ms-marco-MiniLM-L4-v2`) and Voyage
    `rerank-3-lite` (feature `rerank-voyage`) with the local one as fallback.
  - Search modes `fast` / `local` / `accurate`: `mode` on `semantic_search`
    (MCP and gRPC), `search_mode` as the default.
  - Single-session searches add candidates from a retrieval graph
    (personalised PageRank over terms and entities).
  - Hits come with the records right before and after them in their session
    (`neighbour_radius`, `neighbour_of`). Descriptions are kept up to 64 KiB
    (was 1800 bytes); updates over 1000 characters are embedded as chunks
    and returned whole.
- **Conversation capture.** Raw turns are kept in a
  per-session, append-only message stream next to the records; they never
  enter the structured summary, the entity graph or importance scores.
  - `POST /api/messages` and gRPC `AppendMessages`: idempotent on `turn_id`,
    redacted, embedded as `message:<seq>` vectors.
  - `pcx setup --capture` (or `--capture-url <url>` for a remote daemon)
    installs a Claude Code `Stop` hook that posts prompts, final replies,
    tool names and touched file paths. Tool output is never sent.
  - `sources` (`records` | `messages` | `all`) on `semantic_search` and
    `assemble_context`; message hits return the whole turn with the turns
    around it, and `assemble_context` adds them as conversation excerpts.
  - `message_retention_days` (default 90, 0 = forever) expires old turns
    and their vectors daily.
- **Project and user cards**: a short markdown note (at most 2 KB) per
  workspace and one for the user, versioned with compare-and-set so
  concurrent writers cannot overwrite each other. MCP `manage_workspace`
  `get_card` / `update_card`, REST `/api/cards`, gRPC `GetCard` /
  `UpdateCard`, `pcx card show | edit`. `assemble_context` puts the cards
  first, and `pcx setup` installs a `SessionStart` hook that prints them.
- **Authorship**: records and messages carry `author` (an agent such as
  `claude-code` or `hermes:<profile>`, the user, a teammate). MCP
  `update_conversation_context` takes `author`, else the connection's
  `X-PCX-Author` header (`PC_AUTHOR` for the stdio proxy), else the MCP
  client's name unless that only names an SDK (`rmcp`, `mcp`); gRPC fields
  7 / 3. Search results show it; `author` on
  `semantic_search` keeps one author's results, `prefer_author` ranks them
  a little higher (+0.05) and keeps everyone else's.
- **`derived_from`**: a record can name the captured messages it was drawn
  from (`message:<n>`). Each id must exist in the session; search hits on
  the record show the links. MCP and gRPC (fields 8 / 5).
- **`post-cortex-memory` skill** (`skills/post-cortex-memory/`): teaches an agent
  when and how to use the nine MCP tools — find or create the session, search
  before answering (session → workspace → global), use `assemble_context` for
  "what depends on X", and record decisions and fixes with entities and
  relations. Works without any project `CLAUDE.md`. In a context-free A/B run
  (`claude -p` in an empty directory) it triggered on its own in every task and
  raised the pass rate from 89% to 100%. `pcx setup` installs it to
  `~/.claude/skills/post-cortex-memory/`; other agents can load the folder
  directly.

### Changed

- **`pcx setup` installs a deterministic Stop hook** (`.claude/hooks/pcx-stop-check.py`)
  instead of the prompt-type compliance checker. The old hook asked a model to
  judge every turn and kept flagging status updates and already-logged work,
  which pushed agents into writing empty duplicate entries. The new one reads
  the transcript and blocks only when the current turn edited project files or
  ran `git commit` / `git merge` without a later `update_conversation_context`
  (or `context-builder` subagent). It never blocks twice in a row and fails
  open. Existing projects keep their hooks — `pcx setup` skips a
  `settings.json` that already has `hooks`; replace the `Stop` entry by hand
  (see `crates/post-cortex-daemon/src/bin/templates/settings.json`).
- `pcx setup` no longer writes the outdated `SKILL.md` (six tools, subagent
  routing) and `error-handling.md` into `~/.claude/agents/post-cortex-agents/`;
  the skill above replaces both.
- `pcx setup` adds the PCX hooks a project's `settings.json` lacks instead of
  skipping any file that already has hooks, so rerunning it turns capture and
  cards on for an existing project. `--daemon-url` points the hooks at a
  remote daemon.
- Logging: per-request lines (writes, vectorization, assembly, MCP tool
  calls, rmcp connection lifecycle) moved from `info` to `debug`; the default
  filter is `info,rmcp=warn`. Update titles and descriptions are no longer
  logged at `info`. A full write-pipeline queue is reported once a minute
  with a count. `pcx setup`, `card`, `workspace` and `session` print no
  library info lines.
- `ConversationMemorySystem` search methods take `SearchOptions`.

### Fixed

- Search with a sparse filter (records among many messages, a narrow date
  range, one author) on a vector DB past the exact-search threshold could
  return fewer than `limit` results or none; it now falls back to an exact
  filtered scan.
- Entities a write creates, references or relates are embedded right after
  the graph merge, so search finds them without `vectorize_session`.
- `assemble_context` reserves the measured size of the entity and impact
  sections instead of 50 tokens per entity, and splits the budget between
  records and excerpts after that reserve.
- SurrealDB startup no longer rebuilds the HNSW index every time;
  `embedding.metadata` is `FLEXIBLE` (the schemafull object rejected every
  key).
- gRPC workspace search is one multisession search instead of per-session
  searches truncated after merging.

### Migration

- **RocksDB**: existing update rows and checkpoints keep loading (bincode
  falls back layout by layout; checkpoints are written with a version
  marker). Nothing to run.
- **SurrealDB**: the schema (full-text index, message and card tables) is
  created on start.
- New `daemon.toml` keys: `reranker_model`, `voyage_api_key` (or
  `VOYAGE_API_KEY`), `voyage_model`, `search_mode` (default: the best mode
  the configured rerankers allow), `neighbour_radius` (default 1),
  `message_retention_days` (default 90).
- On Linux the local reranker needs ONNX Runtime 1.31 and `ORT_DYLIB_PATH`
  pointing at `libonnxruntime.so`; without it the daemon logs an error and
  searches without the local reranker.

## [0.4.0] - 2026-10-18 — Auth, redaction, row-backed storage and performance

### Added

- **Bearer-token auth** for the daemon: `auth_token` / `PC_AUTH_TOKEN`.
  Required on every HTTP route except `/health` and on every gRPC call
  (constant-time compare). The `pcx` CLI and stdio proxy send it from the
  same config. A non-loopback bind without a token logs a warning.
- **Redaction on the canonical write path** (`post_cortex_core::redact`):
  credentials, e-mail addresses, phone numbers and public IPv4 addresses
  become `[REDACTED:<kind>]` before storage, for every transport. On by
  default; `redact = false` / `PC_REDACT=0` disables, `redact_patterns`
  adds regexes. `Redactor::redact_json` covers imported documents.
- **Namespace-level SurrealDB users**: any `surrealdb_username` other than
  `root` signs in as a user of `surrealdb_namespace`.
- **Session summaries without loading sessions**: `Storage::list_session_summaries`
  (`SessionSummary { id, name, description, created_at, last_updated, update_count }`),
  served from `session_meta:{id}` records on RocksDB and one query on SurrealDB.
  `ConversationMemorySystem::{list_session_summaries, find_session_summaries}`
  back session listing/search on every transport.
- **Graceful shutdown flush**: `pcx start` handles SIGTERM and writes every
  queued session persist before exiting (`ConversationMemorySystem::flush_all`).
- `RealRocksDBStorage::new_with_dimension`, `Storage::save_session_with_changes`
  (explicit removed update ids), `VectorStorage::remove_session_vector`,
  `post_cortex_storage::is_not_found`.
- VectorDB: `add_vectors_bulk`, `search_in_sources(_with_filter)`,
  `remove_by_content_id`, `remove_source`, and the `dot` / `l2_norm` /
  `cosine_with_norms` / `cosine_similarity` kernels.
- Graph: `relationships_of`, `relation_between`, `edge_count`,
  `analyze_entity_importance_top`, `get_entity_network_bounded`, `insert_entity`.

### Fixed

- **Security: DNS rebinding on the HTTP endpoints (RUSTSEC-2026-0189).** A web
  page could reach a local daemon through a DNS name resolving to `127.0.0.1`.
  rmcp is upgraded from 0.12 to 3.5.1, whose Streamable HTTP transport checks
  the `Host` header, and the same check now guards the REST `/api/*` routes;
  other hosts get `403`. New `allowed_hosts` / `PC_ALLOWED_HOSTS` adds names
  (`host` or `host:port`) to the default `localhost`, `127.0.0.1`, `::1` and
  non-wildcard `host`; `["*"]` turns the check off. `/health` stays open.
  A wildcard bind without `allowed_hosts` logs a warning. MCP tool names,
  schemas and results are unchanged.
- **MCP `manage_workspace`, `session` `list`, and `update_conversation_context`
  returned only the summary line** ("Found 5 workspaces") and dropped the result
  data. They now go through the same `mcp_result_to_call_result` as the other
  tools, so the `Results:` JSON follows the message.
- Workspace `created_at` is an RFC 3339 string instead of a
  `{secs_since_epoch, nanos_since_epoch}` object.
- **A failed session load could wipe the session.** Any load error (bad JSON,
  timeout) was treated as "not found", so the session was re-created empty and
  saved over the stored one. Only a real not-found creates a session now.
- **RocksDB never persisted embeddings**: it hard-coded 384 dimensions and
  rejected every 256-dim Potion vector. The dimension now comes from the config.
- **Entity graph applied twice per update** (legacy spawn plus pipeline), which
  doubled `mention_count` and importance. The pipeline is now the only path;
  a lost CAS retries instead of dropping the graph update.
- **Out-of-order and stale session writes.** Persists ran as independent tasks,
  so an older snapshot could overwrite a newer one. A per-session writer now
  always writes the newest snapshot last, and a reload waits for queued writes.
- **Deleted sessions could come back** when a load or an in-flight persist raced
  `delete_session`; a deleted item's embeddings were removed from every session
  that used the same content id; deleted updates and entities stayed searchable.
- **Stale search results** for global and workspace searches for up to the cache
  TTL after a write.
- **Hot-context duplicates** on CAS retries (the shared hot tier was mutated
  before the swap).
- **Entity vectors duplicated** on every re-vectorization; sessions without
  embeddable updates re-embedded their entities on every search.
- **SurrealQL injection** through entity names interpolated into record ids;
  ids are now bound parameters. Graph queries using `->*->` (which SurrealDB 3
  does not parse) are rewritten. SurrealDB load keeps entity `mention_count`,
  `importance_score` and `first_mentioned`; deleted updates are removed from
  `context_update`; pagination has stable ordering.
- RocksDB `delete_session` left embeddings, entities, relationships and
  workspace memberships behind.
- `update_session_metadata` and `delete_entity` were neither CAS-safe nor persisted.
- `update_context` reported `durable: true` before the write; it now waits for it.
- MCP: `get_open_questions` returned a hard-coded string, entity network and
  importance queries discarded their results, session statistics double-counted
  updates; malformed `code_reference` values were silently dropped (now an
  invalid-params error); type coercion stringified typed fields such as
  `updates` and `limit`.
- `partial_cmp().unwrap()` on importance scores could abort the daemon on NaN.
- Deserializing a session reset `created_at` to the load time.

- **Lost embeddings on back-to-back writes.** Background vectorization
  (pipeline worker and the legacy spawn) embedded "the latest update" at
  the time the job ran, not the update that queued it: two quick writes
  embedded the second twice and never the first, so it was unsearchable.
  Jobs now vectorize their own update id (`ContentVectorizer::vectorize_update`)
  and skip ids already vectorized.
- **Sessions with a NULL name/description could not be loaded** ("Session
  not found"): `SurrealValue` reads JSON NULL into `Option<String>` as an
  error. Session reads now coalesce both fields to NONE
  (`select_session_record`); `session_exists` and session delete go through
  the same path.
- **Backfilled vectors were dated "now".** `vectorize_session` stamped every
  vector with the embedding time, so recency-biased search saw old records as
  new. Vectors now carry the update's timestamp (entities: `last_mentioned`),
  via `VectorMetadata::with_timestamp`.
- **First search after a daemon restart was slow** (~6 s: embedding model plus
  every stored vector loaded lazily), long enough to time out short-deadline
  clients such as the Hermes memory provider. The daemon now warms embeddings
  up in the background at startup (`ConversationMemorySystem::warm_up_embeddings`).
- **`pcx start` ignored `daemon.toml` host/port and `PC_HOST`/`PC_PORT`**:
  the clap defaults always overwrote them. Flags now override only when
  given. The stdio proxy's background start also passed `PCX_*` instead of
  the `PC_*` variables the config reads.
- Stale tests: `delete` is a valid session action; the RocksDB lock test
  ran its daemon on SurrealDB; similarity tests asserted BERT-era absolute
  scores (now: the question-bearing update must rank first and beat an
  answer-only control).
- Clippy 1.99: `double_must_use` (from `async_trait`) allowed workspace-wide;
  `fetch_update` deprecation allowed where the MSRV (1.90) lacks `try_update`.
- **Partial `daemon.toml` files were ignored.** `host`, `port` and
  `data_directory` had no defaults, so a file that set only some keys (for
  example `host` plus `allowed_hosts`) failed to parse and the daemon started
  with every setting at its default, logging only a warning. Every key is now
  optional.
- **Crate metadata pointed at the wrong repository** (`juliusbiascan/post-cortex`,
  an unrelated account). `repository` is now
  `https://github.com/julymetodiev/post-cortex`; `SECURITY.md` sends reports to
  GitHub private vulnerability reporting instead of an unrelated e-mail domain.
- Redaction tests used a Telegram-shaped token that GitHub secret scanning
  reported as leaked, and fixtures with real-looking credentials and host IPs.
  Fake values are now assembled at run time and fixtures use placeholder
  credentials and documentation addresses (RFC 5737 / CGNAT).
- READMEs: wrong default ports (51900/51901 → 3737/3738), stale MSRV, versions
  and feature tables (`embeddings` is not BERT; `bert` is opt-in), removed
  modules, and Rust examples that no longer compiled. Every README example now
  compiles against 0.4.0.

### Changed

- **Breaking (workspace-scope `semantic_search`):** results use the same fields
  as session and global scope — `content_id`, `session_id`, `content_type`,
  `text_content`, `similarity_score`, `importance_score`, `combined_score`,
  `timestamp` — instead of `entry_id`/`content`/`score`/`type`. One JSONPath
  `select` now works for every scope; the `select` examples in the tool
  schema were also wrong (`$.total_matches` does not exist) and are fixed.
- **SurrealDB SDK 3.0 → 3.3.4.** Verified against a SurrealDB 3.3.4 server
  upgraded in place from 3.0 (record counts identical, HNSW `REBUILD INDEX`
  and kNN OK). Not tested against 3.0 servers — upgrade the server first.
- **Breaking (storage format):** session blobs no longer embed
  `incremental_updates`; updates live in their own rows (RocksDB adds a
  `session_updates:{id}` index). Old blobs load and are migrated in place.
  Saves no longer infer deletions from a snapshot — removals are passed
  explicitly via `save_session_with_changes`.
- **Breaking (SurrealDB schema):** embedding records are keyed
  `{session_id}_{content_id}` with a unique `(session_id, content_id)` index;
  the old unique index on `content_id` is dropped. Legacy records stay readable.
- **Breaking (build):** the BERT backend is opt-in (`--features bert`); default
  builds no longer compile candle. tonic/prost 0.14, opentelemetry 0.31.
  Release binaries are stripped.
- **Breaking (API):** `post_cortex_memory::performance` (`PerformanceMonitor`)
  removed; `ActiveSession::incremental_updates` / `change_history` hold `Arc`
  elements; `StorageActorHandle::persist_session_and_update_nowait` replaced by
  `persist_session_nowait` / `persist_session_and_wait`; `delete_session` takes
  the live handle; `QueryCache::{search, cache_results}` replaced by
  `get` / `insert`; MCP `EntityNetwork` / `ImportanceAnalysis` responses are typed.
- Quantization is off by default; `max_vectors` is enforced;
  `max_vectors_per_session` is deprecated (never enforced).
- MCP `list_sessions` reports `entity_count: null` for sessions not in memory;
  listings are ordered by creation time.
- MCP and gRPC text payloads use compact JSON.
- `assemble_context` description now says what it does (entity match, one-hop
  graph, impact analysis, recency) — it does not use embeddings.
- **MSRV 1.90 → 1.95**: `surrealdb-common` 3.3.4 uses `std::hint::cold_path`.
- Dependencies refreshed (`cargo update`, picking up fixes in h2, quinn-proto,
  crossbeam-epoch and rkyv). `cargo deny` allows the BUSL-1.1 SurrealDB 3.3
  sub-crates like `surrealdb` itself. quick-xml (via `surrealdb-core` →
  `object_store`, unused) and zip (via candle, `bert` only, extraction unused)
  advisories are ignored with the reason in `deny.toml` / `.cargo/audit.toml`.
- MCP `tools/list` returns tools sorted by name (the order used to be arbitrary);
  `serverInfo` reports the rmcp version (3.5.1).

### Performance

- **Write path:** persists are coalesced per session (one blob write per burst
  instead of up to five per update); `ActiveSession` serializes by reference
  instead of deep-cloning; update and change-history vectors share `Arc`s, so
  copy-on-write no longer clones every update; cold-tier summaries are compact
  and capped at 100 instead of growing quadratically.
- **Vector search:** no HNSW graph is built below `exact_search_threshold`
  (10 000; startup was O(n²)), and above it it is built once in bulk; bounded
  top-k instead of full sorts; filters run during the scan and metadata is
  cloned only for the results; single-pass cosine using stored norms (~17×
  faster kernel); per-session and per-content-id indexes make session checks
  O(1) and scope searches to that session's vectors.
- **Query cache:** O(1) lookup checked before embedding the query; shared
  `Arc` results; unbounded pattern maps removed.
- **Reads:** `assemble_context` walks O(degree) edges instead of every edge per
  entity and caches merged workspace graphs; listings read `session_meta` only
  and never fill the session cache; RocksDB drops its unused in-memory vector
  index, adds bloom filters, a block cache and zstd, and indexes freshness
  source refs by file; SurrealDB saves only changed entities and edges, and
  listings are one query each.
- Session cache `get` does one counter update; storage actor runs up to 32
  requests concurrently on a bounded queue.

### Migration

**Binding to `0.0.0.0` / `::` (tailnet, LAN):** set `allowed_hosts` (or
`PC_ALLOWED_HOSTS`) to the host names and IPs clients put in their URL, e.g.
`allowed_hosts = ["my-mac.tailnet.ts.net", "100.64.0.7"]`, or remote MCP and
REST calls get `403 Forbidden: Host header is not allowed`. Loopback clients
and clients that reach a daemon bound to one specific IP by that IP need no
change.

Back up the SurrealDB database before the first start: the schema change is
forward-only, and an older daemon will not start against it afterwards.
Verified on a database written by 0.3.1 (sessions, updates and legacy
embeddings load and stay searchable). RocksDB data written by this version is
not readable by older binaries (they see sessions without updates). BERT model
users must build with `--features bert`.

## [0.3.1] - 2026-05-17 — Daemon Potion default fix + CI cleanup

Patch release on top of 0.3.0. No public-API changes; this clears the
last user-facing follow-up from the 0.3.0 cut.

### Fixed

- **`post-cortex-daemon` Potion default** — `rmcp_server.rs` was
  hard-coding `embeddings_model_type = "MultilingualMiniLM"`,
  silently downgrading the workspace's `PotionMultilingual` default
  back to BERT. The runtime vectoriser produced 384-dim embeddings
  while `SystemConfig::default()` sized the HNSW index for 256-dim,
  yielding `Query vector dimension 384 does not match expected 256`
  on every semantic search. The override is gone; the daemon now
  inherits whatever `SystemConfig::default()` declares and logs the
  active model + dim at startup.
- **`cargo test --doc` green** across the workspace. The illustrative
  daemon doctests in `coerce.rs` / `validate.rs` / `format_helpers.rs`
  and the `SurrealDBStorage::new` / `TemporalDecayAdjuster::new`
  snippets reference unqualified internal symbols and the
  `post_cortex` facade; they document API shape rather than being
  standalone runnable. Marked `ignore` so they keep the rich
  docstrings without breaking CI.
- **CI infra (carried from `main` since 0.3.0)** — `protoc` install
  steps on macOS / Linux Release runners; `cargo audit` job gets
  `checks: write` + `contents: read` permissions; `.cargo/audit.toml`
  mirrors the deny.toml advisory ignores; cargo-deny clarifies the
  SurrealDB family as BUSL-1.1; MSRV bumped 1.85 → 1.90 to absorb
  new transitive minimums.

### Migration

Same wire-format and crate APIs as 0.3.0. Drop the in-place upgrade:

```sh
cargo install post-cortex-daemon --version 0.3.1 \
    --features "embeddings,surrealdb-storage" --force
```

After upgrading, semantic search resolves cleanly against the
256-dim Potion HNSW index. If you previously ran 0.3.0 against
SurrealDB and have orphan 384-dim rows, drop them once:

```sql
DELETE embedding;
```

`initialize_schema` already drops + redefines the HNSW index with
the requested dimension, so the rows are no longer indexed; deleting
them stops the warn-spam at startup.

## [0.3.0] - 2026-05-17 — Model2Vec default + non-blocking write pipeline

This release retires the BERT default embedding model in favour of
Model2Vec / Potion, finishes the single-entrypoint write-path
migration, and lands the bounded background pipeline so
`update_context` returns once the entry is durably persisted.

**Breaking summary:** embedding dimension 384 → 256, MCP schema now
requires typed `entities` + `relations`, several internal public
helpers (`update_conversation_context_with_system`,
`interaction_to_context_update`, gRPC `validate_entities_and_relations`)
are gone — see the detailed sections below.

### Single-entrypoint write path

The `update_context` / `bulk_update_context` write path is now routed
through a single canonical implementation across every transport
(gRPC, MCP, future REST). Validation, entity-graph metadata shaping,
and persistence live in exactly one place — `MemoryServiceImpl` in
`post-cortex-memory`. Transports translate their wire payload to a
typed `UpdateContextRequest` and delegate; they no longer carry their
own parsing or graph-shaping logic.

### SurrealDB storage — dimension-aware HNSW + raised limits

The remote `SurrealDBStorage` schema previously hard-coded the HNSW
vector index dimension to **384** (matching the legacy
`MultilingualMiniLM` default). Switching the default embedding to
`PotionMultilingual` (256-dim) hit a server-side `"Incorrect vector
dimension (256). Expected a vector of 384 dimension."` rejection.

#### Added

- **`SurrealDBStorage::new_with_dimension(...)`** — pin the HNSW
  index dim at construction. The system layer plumbs
  `SystemConfig.vector_dimension` through so the index always
  matches the active embedding model.

#### Changed

- **`SurrealDBStorage::new(...)`** keeps its old 5-arg signature for
  backwards compat and defaults to 384.
- **HNSW index migration** — `initialize_schema` now `REMOVE`s the
  existing `idx_embedding_hnsw` index and re-`DEFINE`s it with the
  requested dimension. Existing rows aren't purged but stop being
  catalogued; re-vectorise affected sessions to rebuild the index.
- **Vector-length precheck** moved from a fixed
  `EMBEDDING_DIMENSION = 384` to a `MIN_VECTOR_LEN = 8` floor —
  storage no longer enforces a specific dim, that contract lives in
  the embedding engine + vector_db layer.
- **`post-cortex-memory`** now enables both `bert` and `model2vec`
  on `post-cortex-embeddings`. Selecting `PotionMultilingual` /
  `PotionCode` no longer fails with "the `model2vec` feature is
  disabled".

### Non-blocking writes & bounded background pipeline

The canonical write path now hands derived work (embedding compute,
entity-graph merge, summary refresh) to the bounded
[`Pipeline`](https://docs.rs/post-cortex-memory) queues in
`post-cortex-memory`. `update_context` returns as soon as the entry is
durably persisted; embeddings catch up on a separate worker pool —
search quality lags by milliseconds but the agent loop never blocks
on model inference or HNSW upsert.

#### Added

- **Pipeline workers are real** — previously stubs that only
  decremented the backlog gauge. The embedding worker now calls
  `ConversationMemorySystem::vectorize_latest_update_now`, the graph
  worker calls `apply_entity_graph_update_now`, both run their work
  inside the worker task so the hot path stays in single-digit ms.
- **`vectorize_latest_update_now` / `apply_entity_graph_update_now`**
  on `ConversationMemorySystem` — synchronous (no `tokio::spawn`)
  variants of the existing background helpers, suitable for calling
  from any task. Idempotent: failed CAS-swaps are silently dropped
  and skip-sets prevent double-vectorisation.
- **`ActiveSession::is_vectorization_pending(entry_id)`** and
  **`pending_vectorization_count()`** — per-session HNSW-pending
  visibility. Computed from existing state (hot/incremental contexts
  minus `vectorized_update_ids`), no extra storage.
- **`Pipeline::start` now takes `Arc<ConversationMemorySystem>`** so
  workers can reach the canonical `*_now` helpers without
  reimplementing the load → mutate → CAS → persist flow.
- **`GraphWorkItem::ApplyUpdate { session_id, update }`** — the single
  variant that the canonical write path emits (previously the enum
  carried per-entity / per-relation stubs that no code path used).

#### Changed

- **`MemoryServiceImpl::update_context` / `bulk_update_context`** now
  submit an `EmbeddingWorkItem`, `GraphWorkItem::ApplyUpdate`, and
  `SummaryWorkItem` to the pipeline after each persist. Backpressure
  is logged but never fails the write — the legacy in-system
  `tokio::spawn`s in
  `ConversationMemorySystem::add_incremental_update_internal` stay as
  the safety net while direct callers migrate (0.4.0 will retire
  them).

### Embedding engine — Model2Vec default

The default embedding backend swaps from BERT (`MultilingualMiniLM`,
384-dim, Candle + HuggingFace Hub) to Model2Vec
(`PotionMultilingual`, 256-dim, `minishlab/potion-multilingual-128M`).
Static embeddings are an order of magnitude faster on CPU, ship as a
single ~50 MB safetensors file, and cover Latin / Cyrillic / CJK /
Greek / Arabic out of the box. The BERT path stays available behind
the `bert` feature for users who need transformer-grade similarity.

**Breaking — embedding dimension changed (384 → 256).** Existing HNSW
indices were sized for the BERT default and will refuse to accept the
new vectors. Either rebuild your index (delete `~/.post-cortex/data`
or call the vectorize-session admin op) or pin the previous default:

```rust
let config = EmbeddingConfig {
    model_type: EmbeddingModelType::MultilingualMiniLM,
    ..Default::default()
};
```

#### Added

- **`model2vec` cargo feature** (default-on) on `post-cortex-embeddings`,
  pulling `model2vec-rs = "0.2"`. Disable with `default-features = false`
  to drop the dependency.
- **`Model2VecBackend`** in `crates/post-cortex-embeddings/src/embeddings/backends/model2vec.rs`
  — wraps `StaticModel::from_pretrained`, runs encode through
  `tokio::task::spawn_blocking`, and panics at load time if the
  reported dimension drifts from the `EmbeddingModelType` constant
  (silent drift would size HNSW indices wrong).
- **`EmbeddingModelType::PotionMultilingual`** (256-dim,
  `minishlab/potion-multilingual-128M`, the new `Default`).
- **`EmbeddingModelType::PotionCode`** (512-dim,
  `minishlab/potion-code-16M`) for English code search.
- **`EmbeddingModelType::is_model2vec()`** helper for the engine's
  dispatch branch.

#### Changed

- **`BertBackend` is now feature-gated** behind `bert`. Existing builds
  that explicitly opt out of the default feature set must add
  `features = ["bert"]` to keep transformer-based embeddings.
- **`LocalEmbeddingEngine::new`** dispatches Model2Vec → BERT → static
  hash. Selecting a model whose required feature is disabled returns a
  precise error instead of silently downgrading.
- **Hash-fallback warning** in `LocalEmbeddingEngine` now fires only
  for `StaticSimilarityMRL` (true fallback). Model2Vec routes through
  the same non-BERT direct-dispatch branch but without the spurious
  "semantic search will NOT work correctly" warning.

### Single-entrypoint — write path details

#### Added

- **`MemoryServiceImpl::update_context` / `bulk_update_context`** are
  now wired (previously `not_yet_wired` stubs). They enforce: title +
  description not both empty; entities non-empty; relations non-empty
  with referential integrity (each `from_entity` / `to_entity` must
  appear in the entities list); no self-relations; non-empty relation
  context. Validation failures surface as `SystemError::InvalidArgument`,
  which transports map to `Status::invalid_argument` (gRPC) or an MCP
  error payload.
- **`SystemError::InvalidArgument(String)`** variant in
  `post-cortex-core` — replaces the previous habit of stuffing
  validation failures into `SystemError::Internal`. Existing match
  arms keep compiling because every other variant is unchanged.
- **MCP wire types `EntityItem` and `RelationItem`** (in `post-cortex-mcp`)
  carry the typed entity graph from the LLM client through to the
  canonical impl. Used by both single and bulk update tools.
- **`get_service()` singleton** in `post-cortex-mcp` returns the
  cached `Arc<MemoryServiceImpl>` so MCP write calls share one
  background pipeline with the rest of the process. Built lazily; the
  daemon's `inject_memory_system` swaps in a fresh service alongside
  the new memory system.

#### Changed

- **gRPC `UpdateContext` / `BulkUpdateContext` handlers** in
  `post-cortex-daemon` no longer call `ConversationMemorySystem`
  directly. They translate proto → typed `UpdateContextRequest` via
  `proto_update_to_request` / `proto_bulk_item_to_request` and call
  `MemoryServiceImpl::update_context`. Per-item error semantics in
  bulk mode are preserved (translation failures count as failures, the
  rest still land).
- **MCP `update_conversation_context` signature** now requires
  `entities: Vec<EntityItem>` and `relations: Vec<RelationItem>`
  positional arguments. Both the in-tree daemon handlers
  (`daemon/server/handlers.rs` and `daemon/mcp_service/update_context.rs`)
  have been updated; external Rust consumers that call this function
  directly need to pass the new arguments.
- **`UpdateContextRequest::code_reference`** in
  `post-cortex-core/services/types.rs` is now `Option<CodeReference>`
  (was `Option<String>`). Transports that carry the rich proto
  `CodeReference` no longer lose `code_snippet` / `commit_hash` /
  `branch` / `change_description` at the service boundary.

#### Removed (docs)

- **`docs/api-baseline-0.1.23/`** — placeholder for a `cargo public-api`
  snapshot that Phase 13 was supposed to capture and never did. The
  workspace surface diff is meaningful against 0.2.0 / 0.3.0, not the
  pre-split 0.1.23 layout, so the empty directory was net noise.
- **`docs/perf-baseline-0.1.23.md`** — perf baseline used as the
  regression gate for Phases 1-14 (now complete). Future perf work
  should establish a fresh baseline against the post-split workspace.
- `docs/audit-baseline-0.1.23.md` is **kept** — `SECURITY.md` actively
  links to it and `deny.toml` cites it as the source of the RUSTSEC
  whitelist rationale.

#### Removed

- **`update_conversation_context_with_system`** and
  **`interaction_to_context_update`** in `post-cortex-mcp` — both
  superseded by the canonical service path. Internal tests that
  bypassed the MCP layer for cross-session search experiments now
  call `ConversationMemorySystem::add_incremental_update` directly.
- **`validate_entities_and_relations` / `build_update_metadata` /
  `format_context_description`** in `post-cortex-daemon::grpc_service::helpers`
  — moved into `MemoryServiceImpl` so every transport uses the same
  validation and metadata shape.

#### Breaking — MCP tool schema (`update_conversation_context`,
`bulk_update_conversation_context`)

Both tools' input schemas now **require** `entities` and `relations`
arrays with `minItems: 1`. Schema fragment:

```jsonc
"entities": [
  {"name": "<unique name>", "entity_type": "concept|technology|problem|solution|decision|code_component"}
],
"relations": [
  {"from_entity": "<name>", "to_entity": "<name>",
   "relation_type": "depends_on|implements|caused_by|leads_to|related_to|required_by|conflicts_with|solves",
   "context": "<short why>"}
]
```

The legacy freeform `content["entities"]` / `content["relationships"]`
string fallback is gone. Self-relations and dangling references are
rejected with `InvalidArgument`. Existing agent prompts that ignored
the entity graph now need to surface entities/relations explicitly —
this is the change that finally makes the graph non-empty for
MCP-driven writes.

## [0.2.0] - 2026-05-17 — Workspace split

The single-crate `post-cortex` package is now a Cargo workspace of eight publishable crates. Existing imports through `post_cortex::*` keep working via the facade meta-crate.

### Added

- **Eight publishable crates** on crates.io:
  - `post-cortex-proto` — gRPC wire types (prost + tonic-generated, depend on this if you only need the schema).
  - `post-cortex-core` — lean domain library: types, error, traits (`Storage`, `EmbeddingBackend`, `PostCortexService`), graph, session, workspace, summary. **No rocksdb, no candle, no axum/tonic transport.** Downstream Rust projects can take this for the type system alone.
  - `post-cortex-embeddings` — BERT (Candle + tokenizers + hf-hub) and HNSW vector DB. The `bert` feature gates the ML stack; `default-features = false` leaves only the data types (`VectorMetadata`, `SearchMatch`, …).
  - `post-cortex-storage` — RocksDB (default) and SurrealDB (`surrealdb-storage` feature) backends implementing the `Storage` trait.
  - `post-cortex-memory` — `ConversationMemorySystem` orchestrator + non-blocking write `Pipeline` + `MemoryServiceImpl`.
  - `post-cortex-mcp` — pure-library MCP tool functions; no transport runtime, embed in any MCP host.
  - `post-cortex-daemon` — `pcx` CLI + rmcp + axum + tonic + SSE server.
  - `post-cortex` — facade meta-crate re-exporting the stack.
- **`services::PostCortexService` trait** in `post-cortex-core` — the canonical service surface; every transport (gRPC / MCP / REST) is migrating to delegate here so there's exactly one implementation per operation (`update_context`, `semantic_search`, `query_context`, `assemble_context`, session / workspace / entity / admin lifecycle, `get_structured_summary`). `MemoryServiceImpl` in `post-cortex-memory` is the canonical impl.
- **Non-blocking write pipeline** in `post-cortex-memory::pipeline` — three bounded MPSC queues (`EmbeddingQueue`, `GraphQueue`, `SummaryQueue`) with worker tasks. `Pipeline::backlog()` exposes the gauge for health endpoints. Submission is non-blocking (`PipelineError::Backpressure` when full).
- **Typed error hierarchy** — every crate gets its own `Error` + `Result<T>` aliases with `thiserror`-derived variants, source chains, and a stable `kind()` discriminant for metrics. Transport adapters get explicit `mcp_code()` (MCP JSON-RPC error code) and `grpc_status()` / `http_status()` mappings.
- **OpenTelemetry observability infrastructure** behind an `otel` cargo feature on `post-cortex-memory` and `post-cortex-daemon`. `daemon::observability::init()` installs a `tracing_subscriber` with EnvFilter + fmt layer (compact/pretty/json switchable via `OTEL_LOG_FORMAT`); the OTLP exporter activates when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Every `PostCortexService` method on `MemoryServiceImpl` carries `#[tracing::instrument(name = "post_cortex.<op>")]`. See `crates/post-cortex-daemon/examples/with_otel.rs` for the local-collector demo.
- **`README.md` per crate** with elevator pitch, install snippet, runnable example, feature matrix, and license note.
- **`SECURITY.md`** at the workspace root — supported-versions matrix, vulnerability reporting protocol, threat model (local-first deployment, `127.0.0.1` default bind), build-time integrity guarantees.
- **`RELEASING.md`** at the workspace root — pre-flight checklist, step-by-step cut process, hotfix protocol for stable 0.X lines, yank procedure.
- **`.github/workflows/ci.yml`** — comprehensive CI matrix: `fmt`, `clippy -D warnings`, build/test matrix (default / no-default-features / all-features), integration tests, doctest sweep, `cargo doc` with `RUSTDOCFLAGS=-D rustdoc::broken_intra_doc_links`, `cargo audit`, `cargo deny`, MSRV verification on Rust 1.85, `cargo llvm-cov` to Codecov, `cargo workspaces publish --dry-run` on tag pushes.
- **`deny.toml`** — license allow-list (MIT / Apache-2.0 / BSD-2/3 / ISC / Unicode-3.0 / CC0 / Zlib / MPL-2.0 / OpenSSL), `openssl-sys` banned in favour of `rustls`, RUSTSEC baseline ignored with explicit rationale (Phase 0 audit in `docs/audit-baseline-0.1.23.md`).
- **`rustfmt.toml`** with edition 2024 + StdExternalCrate import grouping + format_code_in_doc_comments.
- **`docs/perf-baseline-0.1.23.md`**, **`docs/audit-baseline-0.1.23.md`**, **`docs/api-baseline-0.1.23/README.md`** — Phase 0 baselines captured before the split.
- **`docs/TODO-documentation.md`** — tracks the ~1073 undocumented public struct fields + enum variants slated for the 0.3.0 docs-complete release.

### Changed

- **`pcx` CLI binary** moved from the legacy root crate to `post-cortex-daemon`. Existing users running `pcx start` / `pcx setup` see no behavioural change; the binary install path (`cargo install post-cortex-daemon`) is new.
- **Workspace `[profile.release]`** now uses `panic = "abort"` + `lto = "thin"` + `codegen-units = 1` to surface bugs hard and produce tighter release binaries.
- **`SystemError::Database`** now carries a `String` (was `rocksdb::Error`) so `post-cortex-core` no longer needs `rocksdb` as a direct dependency. Storage backends call `SystemError::Database(e.to_string())` at the boundary.
- **Workspace lints** centralised in `[workspace.lints.clippy]` and `[workspace.lints.rust]`; member crates opt in via `[lints] workspace = true`. Strict lints (`unsafe_code`, `rustdoc::broken_intra_doc_links`) stay denied; cosmetic categories (`result_large_err`, `type_complexity`, `field_reassign_with_default`, etc.) are allowed with documented rationale.
- **`embeddings`** and **`surrealdb-storage`** feature flags are now per-crate concerns of `post-cortex-memory` and `post-cortex-storage`. The facade crate forwards them transparently — existing `cargo build --features embeddings,surrealdb-storage` invocations keep working.
- **MSRV** declared as Rust 1.85 (edition 2024) in `[workspace.package].rust-version`. Enforced via `cargo-msrv verify` in CI.
- **README.md** at the workspace root gains a "Workspace layout" section with the 8-crate table + ASCII dependency-graph diagram.

### Fixed

- **`daemon::validate::test_validate_session_action_invalid`** — long-standing baseline failure noted in `TODO.md`. The test asserted that `validate_session_action("delete")` returned an error, but `"delete"` was added to the valid actions list during a later refactor. Switched the test input to `"nuke_everything"`.
- **`daemon::coerce::test_coerce_float_to_string`** — `clippy::approx_constant` flagged the 3.14 magic float as an approximation of `f64::consts::PI`. Replaced with 2.71.
- **Lib test pass count: 163 / 0** across the workspace (post-cortex-core 53, post-cortex-daemon 48, post-cortex-memory 26, post-cortex-storage 20, post-cortex-embeddings 16). Was 148 / 1 at the Phase 0 baseline.
- **`cargo clippy --workspace --all-targets --all-features` is clean** (0 warnings, 0 errors). Was 116 warnings + 1 hard error pre-fix.

### Removed

- **`src/main.rs`** — the legacy alternative CLI driver. Per the Phase 0 audit, no external consumers; superseded by `pcx`.
- **Direct workspace-root dependencies** on `candle-core`/`-nn`/`-transformers` (moved to `post-cortex-embeddings`), `surrealdb` (moved to `post-cortex-storage`), `tokenizers`, `hf-hub` (same), and the ~25 transport crates that the daemon now owns.

### Migration notes

- Existing consumers of `post-cortex 0.1.x`: `Cargo.toml` line stays the same (`post-cortex = "0.2"`), and the headline import paths (`post_cortex::ConversationMemorySystem`, `post_cortex::SystemConfig`, `post_cortex::storage::*`, `post_cortex::tools::mcp::*`, `post_cortex::daemon::*`) all keep resolving via the facade crate's re-exports.
- Direct dependence on a single member crate is now preferable for production deployments — smaller compile times, fewer transitive deps. See the workspace `README.md` "Picking the right crate" table.
- `pcx` binary install: `cargo install post-cortex-daemon` (was `cargo install post-cortex` in 0.1.x).

### Deferred to 0.3.0

- **Full migration of `anyhow::Result` in public APIs to typed `Result`** — the typed `Error` enums shipped in 0.2.0 are the migration breadcrumb (every variant has an `External(#[from] anyhow::Error)` catch-all). Per-function sweep is a follow-up.
- **Full `missing_docs` compliance** — 1073 undocumented public struct fields + enum variants are catalogued in `docs/TODO-documentation.md`. `missing_docs` is `warn` in 0.2.0; flips to `deny` in 0.3.0.
- **Full OTLP exporter wiring** — feature flag + env-var detection ship in 0.2.0; the actual `opentelemetry-otlp::new_pipeline()` install + W3C tracecontext propagation across gRPC/MCP is bench-gated work.
- **Per-crate `tests/`** — integration tests still live in `crates/post-cortex/tests/` and exercise the full stack via the facade. Phase 12 follow-up moves them per-crate.
- **`cargo public-api` baseline + diff in CI** — snapshot scaffolding lives in `docs/api-baseline-0.1.23/`; CI integration deferred.

## [0.1.22] - 2026-02-27

### Fixed

- **Daemon workspace state sync via REST API**: In-memory `WorkspaceManager` now updates after `api_create_workspace`, `api_delete_workspace`, and `api_attach_session` REST handlers — previously only storage was updated, causing stale workspace data until daemon restart

### Changed

- **README Quick Start**: Replaced manual JSON config with `claude mcp add` CLI commands for both HTTP and stdio transports; added global scope note

## [0.1.21] - 2026-02-26

### Added

- **Agent auto-installation in `pcx setup`**: Setup wizard now installs post-cortex agent definitions to `~/.claude/agents/post-cortex-agents/` (context-builder, search-specialist, knowledge-analyst, memory-curator). Checks for existing files before writing.

### Changed

- **Removed `docs/examples/`**: All templates are now embedded in the binary via `src/bin/templates/` and installed by `pcx setup`. Eliminates template drift between docs and source.
- **Trimmed README.md**: Condensed from ~340 to ~120 lines. Removed verbose MCP tool examples, storage backend details, and benchmarks. Added `pcx setup` to Quick Start.
- **Rewrote USAGE_GUIDE.md**: Quick Setup now points to `pcx setup` instead of manual MCP calls and file copying. Removed all `docs/examples/` references.

## [0.1.20] - 2026-02-26

### Added

- **`pcx setup` interactive wizard**: New command that automates project initialization for Claude Code integration
  - Creates sessions and workspaces via the running daemon
  - Generates `.claude/` configuration files (CLAUDE.md, settings.json, hooks)
  - Supports non-interactive mode with `--defaults` flag
  - Templates for CLAUDE.md, settings.json, and context re-injection hooks
  - `dialoguer` dependency for interactive CLI prompts

### Changed

- **Project configuration refactored**: Moved agent definitions and hook scripts to `docs/examples/`
  - Example configurations now live in `docs/examples/` (CLAUDE.md, settings.json, hooks, agents)
  - `pcx setup` generates project-specific configs from built-in templates instead of manual copying
  - Simplified CLAUDE.md and USAGE_GUIDE.md to reference template-based setup

### Fixed

- **Daemon workspace state sync**: In-memory workspace state now syncs after API changes (create/delete), preventing stale data in subsequent requests

## [0.1.19] - 2026-02-26

### Added

- **NER engine pre-loading on daemon startup**: DistilBERT NER model is now loaded eagerly via `tokio::spawn` during daemon init, ensuring entity extraction uses the ML model instead of the regex/pattern-matching fallback
- **`pcx rebuild-entities` CLI command**: Rebuilds the entity graph for a session (or all sessions with `--all`) by replaying all context updates through the NER engine, replacing noisy pattern-matched entities with precise DistilBERT-extracted ones
- **`clear_session_entities` on Storage trait**: Deletes all entities and relationships for a session from both RocksDB and SurrealDB before saving the rebuilt graph, preventing stale entities from persisting after rebuild

### Fixed

- **NER engine was never loaded**: `preload_ner_engine()` was defined but never called — all entity extraction silently used the pattern-matching fallback, producing ~4x more noisy entities (e.g., "access", "Arc", "HashMap" stored as entities)
- **Stop hook not enforcing Rule 2**: Rewrote hook reason message from diagnostic to actionable format so Claude auto-fixes compliance violations
- **SurrealDB entity cleanup on rebuild**: `save_session` uses UPSERT which doesn't remove old entities — added explicit DELETE of all entity + relationship tables before saving rebuilt graph

## [0.1.18] - 2026-02-26

### Changed

- **Non-blocking context updates (fire-and-forget persistence)**:
  - `add_incremental_update` no longer blocks on storage writes — data is committed in-memory via ArcSwap CAS, then persisted asynchronously
  - New `PersistSessionAndUpdate` message variant in StorageActor with `persist_session_and_update_nowait()` on the handle
  - Eliminates two sequential actor round-trips + `spawn_blocking` calls from the response path

- **Combined RocksDB write (`save_session_with_updates`)**:
  - New `save_session_with_updates` method on `Storage` trait with default implementation
  - RocksDB backend: single `WriteBatch` + single `spawn_blocking` for both session blob and update records (was 2 separate writes)

- **Background entity graph extraction**:
  - New `add_incremental_update_fast()` on `ActiveSession` — skips `update_entity_graph` (up to 5s timeout)
  - Entity graph update runs in a background `tokio::spawn` task after CAS success
  - New `apply_entity_graph_update()` for the background path
  - Background CAS failure is benign — next update catches up

- **Batched SurrealDB operations**:
  - `save_session` now builds all upserts (session metadata + context updates + entities + relationships) into a single multi-statement SurrealQL query (1 network round-trip instead of N)
  - `batch_save_updates` uses single multi-statement query instead of individual upserts per update
  - Estimated 50-100x reduction in `save_session` latency for remote SurrealDB backends

### Fixed

- SurrealDB `save_session` performance: was doing N+1 individual WebSocket round-trips (1 session + N updates + N entities + N relationships); now 1 round-trip

## [0.1.17] - 2026-02-26

### Added

- **Temporal Decay (Recency Bias) for Semantic Search** (PR #11, thanks to neiromaster):
  - New `recency_bias` parameter on all semantic search scopes (session, workspace, global)
  - Exponential decay formula: `base_score * e^(-lambda * days / 365)`
  - Values: 0.0 (disabled) → 0.1–0.5 (soft bias) → 1.0+ (aggressive bias)
  - Input validation: range [0.0, 10.0], rejects NaN/Infinity/negative values
  - Cache key includes `recency_bias` to prevent collisions between different bias values
  - Date range filtering integrates correctly with recency bias
  - Works across all three search scopes and both storage backends
  - Performance metrics: lock-free atomic counters for decay calculation tracking
  - Criterion microbenchmarks for regression testing (~317 ps per decay calculation)

- **MCP Dry-Run Mode and Error Recovery** (PR #10, thanks to neiromaster):
  - `dry_run` parameter on `update_conversation_context` to validate without persisting
  - Detailed preview of what would be stored
  - `generate_recovery_suggestions()` analyzes error patterns and provides actionable hints
  - Recovery suggestions automatically included in all MCP error responses

- **ScoreAdjuster trait for pluggable scoring strategies**: New `src/core/scoring.rs` module
  - `TemporalDecayAdjuster` encapsulates decay logic
  - `CompositeScoreAdjuster` for composing multiple adjusters (Strategy Pattern)
  - Easy to extend with custom adjusters (popularity, quality, etc.)

- **Batch write methods for graph data and embeddings**: RocksDB WriteBatch for saving multiple entities, relationships, and embeddings in a single `spawn_blocking` call instead of N individual writes

- **Docker Compose for SurrealDB and Surrealist**: Development environment setup

- **Claude Code hooks and agent configuration**:
  - Compact reinject hook to restore PCX context after compaction
  - Stop hook for rule compliance verification
  - UserPromptSubmit reminder hook
  - Post-Cortex agent documentation and usage guide

- **Multisession semantic search support**

- **Workspace scope and new interaction types**: `requirement_added` and `concept_defined` for better knowledge capture

### Changed

- **Removed "LockFree" prefix from all type names**: Consolidated indirection layers by inlining implementations into canonical modules
  - Deleted: `lockfree_embeddings.rs`, `lockfree_query_cache.rs`, `lockfree_vector_db.rs`, `embeddings_compat.rs`, `query_cache_compat.rs`
  - Renamed files: `lockfree_cache.rs` → `cache.rs`, `lockfree_memory_system.rs` → `memory_system.rs`, `lockfree_performance.rs` → `performance.rs`
  - Renamed types: `LockFreeConversationMemorySystem` → `ConversationMemorySystem`, `LockFreeCache` → `Cache`, `LockFreeVectorDB` → `VectorDB`, `LockFreeWorkspaceManager` → `WorkspaceManager`, `LockFreeNEREngine` → `NEREngine`, `LockFreeEntityGraph` → `EntityGraph`, `LockFreeDaemonServer` → `DaemonServer`, `LockFreePerformanceMonitor` → `PerformanceMonitor`

- **SearchOptions struct**: Consolidated search parameters to eliminate method proliferation (`_with_recency` wrappers removed, single method per scope)

- **Bumped surrealdb** from 3.0.0-beta to 3.0.0 stable

### Fixed

- **O(n²) performance bottleneck in workspace search**: Replaced N sequential vector DB searches with single `search_with_filter()` call (10 sessions: 500ms → 50ms, 100 sessions: 5s → 50ms)
- **Vectorizer metrics sharing across clones**: Wrapped AtomicU64 metrics in Arc to aggregate across instances
- **Inconsistent response formats**: All search scopes now return consistent JSON with results array
- **Date range + recency_bias integration**: Date range parsing now occurs before recency_bias check
- **Cache key collision**: `recency_bias` included in cache key to prevent stale results
- **recency_bias CLI parity**: Added parameter extraction in CLI handlers for human users
- **Dry-run session existence validation**: Extracted `check_session_exists_for_dry_run()` helper
- **VALID_INTERACTION_TYPES constant**: Extracted for reuse across validation modules

### Improved

- **Lock-free memory system optimizations**:
  - Per-operation counters (load/save/delete) in StorageActor
  - Reduced CAS retry limit from 100 to 20 with exponential backoff
  - Optimized `search_sessions` to prefer in-memory cache over storage
  - Fast-path exact key lookup in entity graph search
  - Batch insertion for HNSW index rebuild with progress logging
  - Replaced DEBUG-level `info!`/`eprintln!` with proper `debug!` tracing

- **Code quality refactoring**:
  - Extracted `format_search_results()` helper (~80 lines deduplication)
  - Unified search logic to remove conditional branching (~60 lines removed)
  - Test fixture builder pattern in `tests/common/mod.rs`
  - `parse_date_range()` helper eliminates ~75 lines of duplicated parsing

## [0.1.16] - 2026-01-16

### Added

- **HNSW Index for RocksDB**: O(log n) vector search now available in RocksDB backend
  - Hybrid architecture: in-memory HNSW index + RocksDB persistence
  - On startup, embeddings are loaded from disk to rebuild the HNSW index
  - Full feature parity with SurrealDB vector search performance

- **Graph Storage for RocksDB**: Persistent entity and relationship storage
  - `StoredEntity` and `StoredRelationship` structs for RocksDB persistence
  - Full `GraphStorage` trait implementation (upsert_entity, get_entity, create_relationship, etc.)
  - BFS-based `find_shortest_path` and `get_entity_network` implementations
  - Embedding dimension validation (384 dimensions) matches SurrealDB

- **MCP Type Coercion**: Better agent interaction with flexible parameter types
  - Automatic type coercion for string/number/boolean parameters
  - Structured error responses for invalid inputs
  - Integration tests for type coercion scenarios

- **MCP Input Schema**: Added `input_schema` field to tool definitions for better client compatibility

- **MCP Validation Layer**: Custom validation for business logic rules (PR #7) (thanks to neiromaster)
  - `validate_session_id()`, `validate_workspace_id()` - UUID format validation
  - `validate_interaction_type()` - Enum validation (6 valid types)
  - `validate_scope()` - Semantic search scope validation
  - `validate_session_action()`, `validate_workspace_action()` - Action validation
  - `validate_session_role()` - Workspace membership roles (4 roles)
  - `validate_limits()` - Numeric bounds checking
  - Enhanced parameter descriptions with examples and valid values
  - 38 new tests (18 unit + 20 integration)

### Fixed

- **RocksDB Session Serialization**: Changed from bincode to serde_json
  - bincode doesn't support `deserialize_any` required by EdgeData custom deserializer
  - Fixes "Serde(AnyNotSupported)" error when loading large sessions
  - Note: Old RocksDB data (bincode format) must be deleted and re-imported

- **UTF-8 Safe String Truncation**: Prevent panic on multi-byte character boundaries
  - Safe truncation for Bulgarian, Chinese, emoji, and other multi-byte characters

- **Integration Test Race Conditions**: Added `serial_test` for reliable parallel test execution (thanks to neiromaster)
  - Tests no longer flake due to shared resource contention

- **Session ID Format Consistency**: Consistent UUID formatting across all MCP responses (thanks to neiromaster)

- **Full UUIDs in Search Output**: `semantic_search_global` now displays complete UUIDs instead of truncated

### Improved

- **Storage Backend Feature Parity**: Both RocksDB and SurrealDB now support identical features
  - Vector Search: HNSW O(log n) for both
  - Graph Storage: Persistent for both
  - Embedding Validation: 384 dimensions for both

- **Storage Code Organization**: Reorganized imports and suppressed unused mut warnings

## [0.1.15] - 2025-12-22

### Fixed

- **MCP Client Compatibility**: Strip `$schema` from tool input schemas
  - Many MCP clients (Cursor, Windsurf, Continue.dev) don't support JSON Schema draft/2020-12
  - Replaced `#[tool_handler]` macro with manual `list_tools` implementation
  - Removes `$schema` field from all tool schemas and nested `$defs`
  - Error "no schema with key or ref 'https://json-schema.org/draft/2020-12/schema'" is now resolved

## [0.1.14] - 2025-12-21

### Changed

- **RMCP Transport Upgrade**: Migrated from SSE to Streamable HTTP transport
  - Updated `rmcp` to 0.12.0 with `transport-streamable-http-server` feature
  - Replaced `SseServer` with `StreamableHttpService` and `LocalSessionManager`
  - Unified endpoint: `/mcp` replaces `/sse` + `/message`
  - MCP client config: `"type": "http"` with `"url": "http://localhost:3737/mcp"`

- **stdio_proxy Lock-Free Rewrite**: Updated to use Streamable HTTP protocol
  - Replaced `RwLock` with `ArcSwap` for session ID storage
  - Session ID extracted from `Mcp-Session-Id` response header
  - Proper `Accept: application/json, text/event-stream` headers

- **bincode Downgrade**: 3.0.0 → 2.0.1 with `serde` feature
  - bincode 3.0.0 contains intentional `compile_error!` (maintainer protest after doxxing incident)
  - Version 2.0.1 is stable and fully functional

### Fixed

- **Documentation Updates**: Updated all MCP endpoint references
  - README.md: HTTP transport configuration
  - DAEMON_MODE.md: Streamable HTTP examples
  - pcx status: Shows `/mcp` and `/health` endpoints

## [0.1.13] - 2025-12-21

### Added

- **Graph-RAG Enrichment**: Semantic search results now include structural insights from the entity graph
  - `[System Knowledge Map]` shows entity relationships in search results
  - `[Structural Insight]` displays paths between top result entities
  - Automatic entity extraction from search queries
  - Direct graph access for O(1) entity lookups (no async message passing)

- **HNSW Vector Index**: Native approximate nearest neighbor search in SurrealDB
  - `idx_embedding_hnsw` - 384-dimensional COSINE distance index
  - O(log n) KNN queries instead of O(n) full table scan
  - Post-filtering for session/content_type scoped searches

- **Composite Database Indexes**: Optimized ORDER BY queries
  - `idx_update_session_time` - (session_id, timestamp) for context_update
  - `idx_entity_session_importance` - (session_id, importance_score) for entity

- **Explicit Entity/Relationship Parsing**: MCP tool now parses entities from content
  - `entities` field: comma/space separated entity names
  - `relationships` field: "A RELATION B" format (DEPENDS_ON, IMPLEMENTS, CAUSES, etc.)

### Improved

- **Vector Search Performance**: Replaced Rust-side cosine similarity with SurrealDB native KNN
  - `search()` - uses HNSW index directly
  - `search_in_session()` - HNSW with post-filtering (5x overfetch)
  - `search_by_content_type()` - HNSW with post-filtering

- **Case-Insensitive Entity Lookup**: Graph queries now support case-insensitive matching
  - `find_related_entities()` - O(1) exact match with O(n) fallback
  - `find_related_entities_by_type()` - case-insensitive entity names
  - `find_shortest_path()` - case-insensitive from/to nodes

### Fixed

- **Relationship Noise Cleanup**: Removed auto-generated "Co-mentioned in:" relationships
  - Deleted `auto_generate_relationships()` function
  - Deleted `infer_relationship_type()` function
  - Only explicit relationships from MCP content are now stored

- **RUST_LOG Environment Variable**: Now properly respects RUST_LOG settings
  - Uses `EnvFilter::try_from_default_env()` with "info" fallback
  - Fixed duplicate directive issue

- **Verbose Logging**: Reduced noise from `add_incremental_update` instrument

## [0.1.12] - 2025-12-20

### Added

- **Embeddings Export/Import**: Full backup and restore of semantic search embeddings
  - Export format v1.2.0 includes embeddings array with vectors, text, and metadata
  - `ExportedEmbedding` struct: content_id, session_id, vector (384-dim), text, content_type, timestamp
  - Backwards-compatible import (v1.0.0, v1.1.0, v1.2.0 formats supported)
  - JSON Schema `schemas/export/v1.2.0.json` with embeddings definition

### Fixed

- **vectorize-all with SurrealDB**: Now respects `storage_backend` configuration
  - Previously always used RocksDB regardless of daemon.toml settings
  - Now properly connects to SurrealDB when configured

## [0.1.11] - 2025-12-20

### Added

- **SurrealDB Storage Backend**: Native graph database support as alternative to RocksDB
  - Native graph storage for entities and relationships using SurrealDB RELATE
  - Normalized context storage with separate tables for sessions, updates, entities
  - Persistent embeddings storage with vector search capabilities
  - Configurable via `daemon.toml` with `storage_backend = "surrealdb"`
  - Namespace and database configuration (`surrealdb_namespace`, `surrealdb_database`)

- **Export/Import for SurrealDB**: Full data portability between storage backends
  - `pcx export` now works with SurrealDB backend
  - `pcx import` imports directly to configured storage backend
  - Backwards-compatible import from v1.0.0 format (EdgeData deserialization)
  - Auto-generated export filenames: `export-YYYY-MM-DD_HH-MM-SS.json.gz`

- **Export Schema Versioning**: JSON Schema definitions for validation
  - `schemas/export/v1.0.0.json` - Original format (RelationType strings)
  - `schemas/export/v1.1.0.json` - New format (EdgeData objects with context)

- **New MCP Interaction Types**: Extended context update types
  - `requirement_added` - Track new requirements
  - `concept_defined` - Document concept definitions

### Improved

- **CLI Storage Backend Support**: All CLI commands respect `storage_backend` config
  - `pcx session list/create/delete` uses configured backend
  - `pcx workspace list/create/delete/attach` uses configured backend
  - Works both with running daemon (HTTP API) and direct storage access

- **Dual Logging for Daemon**: Logs to both file and stderr in foreground mode

### Fixed

- **Export without output path**: Now auto-generates timestamped filename instead of error

## [0.1.10] - 2025-12-14

### Added

- **Data Export/Import**: Full backup and restore functionality for Post-Cortex data
  - `pcx export` - Export sessions, workspaces, updates, and checkpoints to JSON
  - `pcx import` - Import data with selective filtering and conflict handling
  - **Compression support**: gzip (`.json.gz`) and zstd (`.json.zst`) with auto-detection
  - **Selective export**: Export specific sessions (`--session`) or workspaces (`--workspace`)
  - **Import options**: `--skip-existing`, `--overwrite`, `--list` for preview
  - **Versioned format**: Format version 1.0.0 with forward compatibility support
  - **Overwrite protection**: Prompts before overwriting existing files (`--force` to skip)

### Improved

- **CLI help system**: Added detailed `--help` with examples for all commands and subcommands
- **Storage layer**: Added `load_session_updates` and `list_checkpoints` methods to RocksDB storage

## [0.1.9] - 2025-12-11

### Changed

- **MCP Tools Consolidation**: Reduced from 24 tools to 6 consolidated tools for simpler AI integration
  - `session` - Merges create_session + list_sessions via action parameter
  - `update_conversation_context` - Supports bulk mode via optional `updates` array
  - `semantic_search` - Unified search with `scope` parameter (session/workspace/global)
  - `get_structured_summary` - Extended with `include` parameter for selective sections
  - `query_conversation_context` - Added `entity_importance` and `entity_network` query types
  - `manage_workspace` - Merges 6 workspace tools via action parameter

### Fixed

- **RocksDB prefix iteration for short prefixes**: Fixed `prefix_iterator()` not working for prefixes shorter than 16 bytes (the prefix_extractor size). Session list, workspace list, and delete operations now use `iterator(IteratorMode::From)` instead, correctly handling "session:" (8 bytes), "workspace:" (10 bytes), and "ws_session:" (11 bytes) prefixes

### Improved

- **README documentation**: Cleaner documentation with concise 6-tool API examples
- **SummaryGenerator refactoring**: Converted to use associated functions for better code organization
- **Axum route params**: Updated to use bracket syntax for path parameters

## [0.1.8] - 2025-12-09

### Added

- **Lock-free memory pool statistics**: Added hit/miss counters and pool monitoring for performance diagnostics
- **Session-specific cache invalidation**: New method to invalidate cache entries for a single session instead of clearing entire cache
- **Text similarity computation**: Implemented `compute_text_similarity` in ContentVectorizer for accurate cosine similarity calculations between text embeddings

### Changed

- **Copy-on-Write semantics for ActiveSession**: Wrapped large session fields in `Arc` to enable cheap cloning with `Arc::make_mut()` for efficient updates
- **Logical timestamps in cache**: Replaced `SystemTime` calls with Lamport clock to eliminate syscall overhead (~20-100ns per call) in hot paths
- **Memory pool concurrency**: Implemented proper compare-and-swap loops for atomic operations in memory pool and batch size updates

### Fixed

- **Concurrent entry point race conditions**: Fixed race conditions in search mode with proper ordering traits for `SearchResult`
- **Statistics underflow**: Added saturating subtraction to prevent underflow on double-remove operations
- **NER engine safety**: Changed cache key from hash to full text to avoid collisions, added bounded cache with eviction, proper memory ordering for thread synchronization, and safe UTF-8 slicing
- **Stale vectorized_update_ids after daemon restart**: Added verification to clear stale IDs when vector embeddings are missing in-memory after restart
- **Semantic search similarity filtering**: Applied similarity threshold filtering consistently across all search functions with improved Unicode support in content truncation

### Improved

- **Pool size limits**: Added limits to prevent unbounded memory pool growth
- **Tensor type conversions**: Optimized for BERT compatibility
- **StructuredContext validation**: Enhanced with better documentation and field name handling in MCP tools

## [0.1.7] - 2025-11-25

### Added

- **REST API for CLI commands**: Added HTTP endpoints to daemon for CLI administration when daemon is running
  - `/health` - Health check endpoint
  - `/api/sessions` - GET/POST for session listing and creation
  - `/api/sessions/{id}` - DELETE for session deletion
  - `/api/workspaces` - GET/POST for workspace listing and creation
  - `/api/workspaces/{id}` - DELETE for workspace deletion
  - `/api/workspaces/{workspace_id}/sessions/{session_id}` - POST for attaching sessions

### Fixed

- **CLI RocksDB lock conflict**: CLI commands now detect running daemon and route through HTTP API instead of direct RocksDB access, preventing "Resource temporarily unavailable" lock errors
- **Stale workspace session count**: Workspace listing now filters deleted sessions, showing accurate count of existing sessions only

### Changed

- **CLI architecture**: Added `DaemonClient` HTTP client and `is_daemon_running()` detection for seamless daemon/direct mode switching

## [0.1.6] - 2025-11-25

### Changed

- **Unified binary**: Merged `post-cortex` and `post-cortex-daemon` into single `pcx` binary supporting both stdio and SSE transports
- **Simplified CLI**: `pcx` (stdio mode), `pcx start` (daemon mode), `pcx status`, `pcx stop`
- **Updated README**: Simplified documentation reflecting unified binary architecture

### Added

- **Docker MCP Registry support**: Added Dockerfile and configuration for Docker-based deployment
- **Auto-daemon startup**: Stdio mode automatically starts daemon in background if not running

## [0.1.5] - 2025-11-24

### Added

- **CLI workspace and session management**: New commands `post-cortex-daemon workspace` and `post-cortex-daemon session` for comprehensive workspace and session administration
- **Integration tests for admin tasks**: Comprehensive test suite covering workspace creation, session management, and administrative operations
- **Gemini API compatibility**: Flattened semantic search schema structure for full compatibility with Gemini's JSON Schema validation

### Changed

- **Semantic search API schema**: Replaced nested SearchScope struct with flattened scope_type and scope_id fields for better API compatibility
- **CLI daemon binary**: Enhanced with extensive workspace and session management capabilities

### Technical

- Added JsonSchema derives to CodeReference and ContextUpdateItem for better API documentation
- Improved schema descriptions for MCP tool documentation
- Enhanced daemon parameter parsing with safe error handling

## [0.1.4] - 2025-11-22

### Added

- **Unified semantic search with workspace scoping**: New `semantic_search` MCP tool replaces separate session/global tools with unified interface supporting session, workspace, and global scopes
- **Workspace persistence**: Full RocksDB persistence and hydration for workspaces enabling reliable cross-session workspace management
- **Entity graph relations in vectorizer**: Entities now include up to 5 related concepts in vector embeddings through actual graph traversal
- **Comprehensive integration tests**: Added unified search integration tests covering session, workspace, and global scopes with complex multi-session scenarios

### Fixed

- **Query cache cross-session result leakage**: Critical bug where semantic search returned cached results from wrong sessions due to missing params_hash validation in similarity-based cache lookups
- **Semantic search missing updates after hot context eviction**: Implemented vectorized_update_ids tracking to ensure all updates remain searchable even after eviction from hot/warm tiers to cold storage
- **Unsafe unwraps in daemon server**: Replaced panic-prone .unwrap() calls with safe error handling in parameter parsing logic to prevent server crashes

### Changed

- **Entity graph integration**: Replaced TODO placeholders with actual graph relationship traversal in content vectorizer for both sequential and Rayon parallel processing paths
- **Vectorization strategy**: Modified to iterate incremental_updates instead of hot+warm context only, with DashSet-based tracking for efficient re-vectorization skipping

### Improved

- **Code quality**: Resolved all clippy warnings (27 → 0) including redundant clones, ptr_arg issues, and simplified boolean logic
- **Error handling**: Replaced unsafe unwraps with safe .map().unwrap_or_else() patterns in daemon endpoints
- **Performance**: Incremental vectorization with O(1) DashSet lookups for already-vectorized updates

### Removed

- Unused `extract_text_from_compressed_update` method from codebase

## [0.1.3] - 2025-11-21

### Fixed

- Data persistence: Fixed launchd /tmp data loss on reboot by moving to ~/.post-cortex/data
- Daemon logs moved to ~/.post-cortex/logs with absolute paths
- Unified storage: stdio + daemon now use ~/.post-cortex/data
- Fixed query_conversation_context data field in response

## [0.1.2] - 2025-11-20

### Fixed

- Standardized on MultilingualMiniLM embedding model
- Fixed text extraction to include question titles
- Semantic search similarity improved from 11% to 94%+

## [0.1.1] - 2025-11-19

### Fixed

- Fixed launchd configuration for proper daemon startup
- Resource URL interpolation in Homebrew formula

## [0.1.0] - 2025-11-18

### Added

- Initial release of Post-Cortex
- Lock-free conversation memory system with zero-deadlock guarantees
- Three-tier memory hierarchy (Hot/Warm/Cold)
- Local AI-powered semantic search with HNSW indexing
- NER-powered knowledge graph extraction
- 24 MCP tools for conversation context management
- RocksDB persistence layer
- Daemon mode with HTTP/SSE transport
