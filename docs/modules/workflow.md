# workflow

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8. Approved by the
owner on 2026-10-31 (wave 1, board `pcx-cloud`, task `511c2415`); the engine is built in wave 3.

- **`workflow`**: graphs of steps that run when something happens, over the tools of every
  module. Crate `pcx-workflow`, built in (the daemon in process, the cloud's catalog as a
  process); proto package `pcx.workflow.v1`; uses `post-cortex.memory` and every module whose
  tools its workflows call, as granted.
  - *One block for every module* (owner 2026-10-31): memory, models, the board, mail and
    operations are modules with typed RPCs; one generic `tool` block reaches any RPC of
    `Manifest.rpcs` whose tool an agent may call, its input a request message of the RPC and
    its output the response message, so every new module (a dynamic one in Python or TS
    included, `dynamic.md`) is a block with no code here. There is no block per subsystem and
    no block past the grants; a running system is reached only through a module that offers
    it (`ops`' typed actions, each a board decision).
  - *A workflow* (document `workflow/workflow`, `pcx.workflow.v1.Workflow`): its name (unique
    in the workspace, 1 to 64 of a-z, 0-9, '-'), its description, its input (a `Schema`), its
    triggers, its blocks and edges, its budgets and the grants it asks for. Each change is the
    document's next version; a run is pinned to the version it started on, so changing a
    workflow never changes a run in flight. Search finds a workflow by its name and
    description, as any document.

## Blocks

- **Triggers** (a workflow has one or more):
  - `schedule`: a `scheduler` job whose action is `workflow` (a new action of
    `pcx.scheduler.v1.Job` beside `script` and `task`: the workflow and its input), so time
    lives in one module;
  - `webhook`: a source of `webhooks` routed to the workflow instead of the board (the
    source's settings name it); the request's verified body is the input;
  - `event`: an event of the bus (host.md, events), by name, with an optional CEL filter over
    the event (`event.payload.column == "Done"`);
  - `manual`: `workflow_run` by a person or an agent;
  - `call`: as a tool (`workflow_run`) of an agent or as a `workflow` block of another
    workflow.
- **Actions**:
  - `tool`: one RPC of a module (`/pcx.board.v1.Boards/AddTask`), its request built from a
    template (below), its response the block's output;
  - `model`: one call of a model with a prompt template and an output `Schema`, no tools: to
    classify, extract or sum up. The model answers through one forced tool whose input schema
    is the output `Schema` (the model's tool call is the one JSON edge, host.md); an answer that
    does not fit fails the block;
  - `agent`: a turn of a team member (or the host's agent) with its own tools, under its own
    grants, its prompt a template and its result a `Schema`, as `model`. A member's turn is
    asked of the team (`ModuleHost::turns_of`, the team's own turns: the member's role, skills,
    model, modules and tools apply as on the board and in the app's chat; no such member, or
    one that is not an agent of the harness, fails the block); without a member it is the
    agent's (`ModuleHost::turns()`), given the tools of the modules the workflow's grants name
    and no other (the module's own reach is every module's): a workflow whose grants name no
    module is refused such a block (`agent.member`: name a member, a module, or use a `model`
    block). The turn is asked to end with one JSON object of the `Schema`, read from the
    answer's first `{` to its last `}`; an answer that is not read (no object, no JSON, one
    that does not fit) is asked for once more in the turn's session, told why, and one not
    read again fails the block with the kind `answer` and the answer's first 300 characters
    (found 2026-11-14 in the Discord help forum: an answer in plain text, then one with a line
    break inside a JSON string, each failed the run at once and the person got no reply).
    Fixed 2026-11-11: a member's turn went to the agent, which reads no member, so it
    ran with the agent's own tools (ops daemon: `eq-sentinel`, a member of `observe`, `board`
    and memory only, reached `workflow_run`), and the agent's own turn had every tool whatever
    the grants named;
  - `workflow`: another workflow (by name, at its latest version when the run starts), its
    input a template, its run's output the block's.
- **Flow**:
  - `if` (a CEL condition; edges `then` and `else`) and `switch` (cases of CEL conditions, the
    first true one, else `default`);
  - `for_each`: a body (a graph of its own) for each item of a CEL list, at most
    `concurrency` at once (default 1, at most 16), its output the list of the bodies' outputs
    in the items' order;
  - `parallel`: branches (graphs of their own) at once, joined by `all` (every branch's
    output), `first` (the first that ends, the others cancelled) or `n` (the first N);
  - `wait`: for a duration, until a time (a CEL timestamp), or until an event of the bus that
    matches a filter, with a timeout;
  - `approval`: stops the run until a person (or the team's autonomy policy, `team.md`)
    approves: a board decision on the run's task with a risk, as `ops` does; the decision's
    answer is the block's output. With `person` (owner 2026-11-14) only a person approves
    it, whatever the team's trust: the decision's `pcx.team.v1.Stakes.person`, as ops' revert
    (a ban of a Discord member, `discord.md`);
  - errors: every block has `retry` (attempts, backoff from 1 s doubling, at most 1 h) and a
    `timeout`; an `error` edge from a block takes the run on when the block fails after its
    retries, its output the failure (`kind`, `message`); a workflow with the trigger `event`
    on `workflow.run_failed` reacts to the failures of others.
- **Data**:
  - the outputs of a run's blocks are its state: `steps.<id>` in CEL, `input` the run's input,
    `trigger` what started it (`trigger.event`, `trigger.webhook`);
  - `set`: named values computed by CEL, its output a map of them;
  - `state`: a key-value store of the workflow, kept between runs (document
    `workflow/state`, one per key, written from the version read, so two runs never overwrite
    each other): `get`, `put` and `delete` blocks, for deduplication and cursors;
  - `filter`, `transform` and `validate`: a list's items kept, reshaped or checked, each by
    a CEL expression for an item (below, "Data blocks");
- **Templates**: a block's request is a `pcx.workflow.v1.Template`: for each field of the
  request message (by proto field name, nested as the message nests), a constant of the
  field's type or `{{ <CEL> }}`, whose type must be the field's; a string field may mix text
  and `{{ }}` parts. The same templates, with one implementation (the pure crate `pcx-expr`,
  owner 2026-10-31), are the mail module's (`mail.md`).

## The messages (owner 2026-10-31)

- *The graph*: blocks and edges. A `Graph` is a list of `Block`s (an id unique in the
  workflow, 1 to 64 of a-z, 0-9, '_'; a name; `retry`; `timeout`; one kind) and a list of
  `Edge`s (from, to, port: `next`, `then`, `else`, `case:<n>`, `default`, `error`). A trigger
  is a block too, so each trigger is a start; a `for_each` body and each `parallel` branch is a
  `Graph` of its own, its blocks' ids unique in the whole workflow.
- *A `Schema`* (a workflow's input, a `model` or `agent` block's output): fields, each a name
  (a lowercase letter or '_', then letters, digits and '_'), a type as CEL writes it (`string`,
  `int`, `uint`, `double`, `bool`, `bytes`, `timestamp`, `duration`, `list<T>`,
  `map<string, T>`), a description, an example and whether it is required; the JSON Schema of a
  model's forced tool is made from it, and its types are read by `pcx-expr`, as the mail
  templates' inputs.
- *A `Template`*: a tree of slots, by proto field name: each slot a constant (`Value`), a text
  with `{{ }}` parts (a string field; one whole `{{ }}` gives a field of any type, a list or a
  message included), a nested `Template` (a message field) or a list of slots (a repeated
  field).
- *What each block gives* (`steps.<id>`): a `tool` its response message; a `model` or an
  `agent` its output schema's fields (`steps.<id>.<field>`), as a `set` its values'; a `call`
  the called workflow's output (`Workflow.output`, a CEL expression over what every end of its
  graph has); an `if` its condition, a `switch` the index of its case (-1: `default`); a
  `for_each` the list of its body's outputs (`Graph.output`), `item` and `index` in the body
  (an inner one's hide an outer's); a `parallel` the list of its branches' outputs (`first`:
  the one); a `wait` the event it waited for, else null; an `approval` the board's
  `Decision`; a `state_get` its value (`dyn`). After an `error` edge `steps.<id>` is a
  `Failure` (`kind`, `message`); a trigger gives `trigger` (a `Fired`). A block reads what every
  edge into it carries with one type.
- *Calls*: a called workflow's signature is read from its latest version, checked with the same
  tools; a workflow that calls itself (through any chain) or calls deeper than the host's limit
  of 4 is refused when it is written.
- *Grants*: the modules a workflow's blocks may call. Its author's are the modules of the tools
  the host gives the author's scope (`tools`); a workflow asks for a subset, every `tool`
  block's module is in it, and the host's route checks each call again.

## The engine's reach (owner 2026-10-31)

- *Models*: a module whose manifest asks for the capability `models` (`CAPABILITY_MODELS`,
  `pcx.module.v1`) is given the host's models (`ModuleHost::model`), as the `agent` and
  `context_engine` slots are; the workflow module asks for it. A `model` block is one
  `Generate` with one forced tool whose input schema is the block's output `Schema`; its tokens
  and cost count in the run's budgets.
- *The run's task*: the workflow module's settings name a board; a run that reaches its first
  `approval` adds a task to it (`Run <id> of <workflow>`, key `run:<id>`, so a retry adds it
  once), and its decisions are recorded there. An `approval` block may name another task
  instead (`task`, a CEL string: a task of the trigger's event, or one a `tool` block added). A
  workflow with an `approval` asks for the `board` in its grants.
- *Events*: a manifest's `follows` may name `<module>.*`, every event a module it uses declares,
  or `*`, every event of every module it uses (host.md, events); the workflow module uses every
  module (`uses: ["*"]`, owner 2026-10-31) and follows `*`, and its `event` triggers and `wait`
  blocks pick theirs by name and filter.
- *Large outputs*: memory keeps a module's binary blobs too (media type
  `application/x-protobuf`, at most 10 MB, kept while a document names one, as images are);
  a step's output above 48 KiB is such a blob, its step naming it by an attachment.
- *Approved*: the board emits `board.decision_approved` when a decision is approved (by a
  person or the team's policy); the engine follows it and the run goes on. A decision the
  policy approves at once is approved in `Decide`'s answer. A run that waited on a decision
  when the module stopped reads it again from the board (`ListDecisions` of its task) when
  the module starts, so an approval made meanwhile is not lost.
- *Declined* (owner 2026-11-11): the board emits `board.decision_declined` when a person
  declines a decision (`board.md`). An `approval` step that waits on it fails with the kind
  `declined` ("declined by <person>"), so its `error` edge runs, or the run fails; read again
  at start as an approval is. Declined grants are kept as declined (`Asked.declined_by`):
  the workflow stays as it was approved (a new one: not approved), no one is asked again
  when the module starts, and writing the workflow again asks again.
- *Withdrawn* (owner 2026-11-10): a run that ends (failed, cancelled, past a budget) while an
  `approval` waits withdraws that decision (`WithdrawDecision`, in the run's name, with why the
  run ended), so no person is asked what no run waits for; a workflow deleted while the
  decision on its grants waits withdraws that one. The steps a run leaves waiting as it ends
  are closed (failed, `cancelled`: the run ended while it waited). At start the module does the
  same for the 50 runs of each workflow that ended last (a run that ended before this, or whose
  board did not answer: its step stays waiting until then). A board that refuses is logged; the
  run's end and the delete stand.

## Triggers and approval (owner 2026-10-31)

- *Who fired it*: a run's trigger is read from the call's `Scope.caller_module`, which the
  host sets and no caller forges: a call from `scheduler` starts the `schedule` trigger, one
  from `webhooks` the `webhook` trigger of the request's `source` (its raw JSON body in the
  request, read here); any other call starts `call` (the request's `call`) or `manual`.
- *Schedule*: `pcx.scheduler.v1.Job` gains the action `workflow` (`WorkflowAction`: the
  workflow's name and its input, a `pcx.workflow.v1.Map`); the scheduler runs it by
  `/pcx.workflow.v1.Workflows/RunWorkflow`, so it uses `workflow`.
- *Webhook*: a `Source` of `webhooks` may name a `workflow` instead of a `board` (one of the two,
  checked); its requests then go to that workflow and add no task. A new kind
  `SOURCE_KIND_JSON` takes any JSON body; an Alertmanager source may be routed too. The body is
  `trigger.webhook.body`, and the run's input is read from its fields by the input `Schema`
  (fields it does not name are left out).
- *Its workspaces* (owner 2026-11-10: workflows are work of a workspace; docs/connect.md
  section 4a, a module's data): a workflow belongs to a workspace, the one the call that makes
  it is asked from (asked from none: the one the `workspace_id` setting names). Its name is
  unique there, its runs, journals and state are kept there, its runs act there, and every tool
  reads and changes only the workflows and runs of the workspace it is asked from; one outside
  the caller's reach is `Denied`. One engine runs every workspace's workflows (owner
  2026-11-10: one, not one for each workspace): what it holds (the runs being driven, the
  `event` triggers, the watchers) is one, and each call, run and event is in its workspace,
  reached through that workspace's ports (`Engine::at`); at start the open runs and the
  triggers of every workspace are read. The
  approvals of a workspace other than the settings' are asked on that workspace's own board
  named `Approvals` (columns waiting, done), which the module makes when a workflow there
  first needs one, so the people of a workspace decide on what runs in it, on its Approvals
  page as any other decision (owner 2026-11-10). An event starts runs only in its own
  workspace (`BusEvent.workspace_id`; an event of none is heard in every workspace; owner
  2026-11-10). The settings'
  `board` is the board of the settings' workspace only.
- *Event*: an `event` trigger starts a run for each event of its name whose `filter` holds; the
  engine keeps the event triggers of the workspace's workflows, read when it starts and after
  each write. The filter reads the event's payload by its fields
  (`event.payload.channel == "help"`) for an event of any module the workflow reaches (its
  grants): it is compiled and the event read when the event comes, with the messages a run of
  the workflow reaches then, so a module started or enabled after the workflow was written is
  read too; a trigger with no filter asks the host nothing. An event's payload is an `Any`
  whose type URL the modules write as `/<name>`; the engine writes it as
  `type.googleapis.com/<name>` when it hears the event, the only form CEL unpacks, so a
  filter, a `wait` and a run's `trigger.event.payload` read the same fields.
- *Deleting* (`workflow_delete`, owner 2026-11-02): refused (`FailedPrecondition`) while a run of
  it is open, since a run reads its version as it goes: cancel it or wait for it to end. The
  workflow then starts no more and its `event` triggers go at once; its ended runs stay to be
  read. A scheduler's job or a webhook that names it then fails with `NotFound`.
- *Approved grants*: `Workflow.approved` (the grants a person approved and the decision) is
  written only by the module: `workflow_update` keeps the previous version's, whatever the
  request says. A new workflow, or a change whose grants are not within `approved`, adds the
  task `Workflow <name>` (key `workflow:<id>`) to the settings' board with a decision of risk
  `code` that names the grants; when it is approved (`board.decision_approved`, or read again
  at start) the module writes `approved`. What is asked and approved is the modules the
  grants name and the team members the `agent` blocks name (`Grants.members`, 2026-11-11:
  read from the graph, bodies and branches included, never from the request, and written by
  the module in `asked` and `approved`), so the question names them ("Let workflow daily run,
  calling telegram, with turns of eq-sentinel?") and a block that names another member asks
  again; a workflow approved before members were asked for is asked again at the module's
  start. So are the tools of effect `external` its `tool` blocks call (`Grants.tools`, owner
  2026-11-14: a person approves each action, not only its module): by tool name, found when
  the workflow is written by checking its graph against its author's tools (bodies and
  branches included, never from the request), written by the module in the workflow's grants,
  `asked` and `approved`, so the question names them ("Let workflow discord-welcome
  run, calling discord, acting with discord_give_role?")
  and a new version that calls another such tool asks again, though its modules are the same;
  a tool that only reads or writes memory is covered by its module. A workflow approved
  before tools were asked for is asked again at the module's start. A run starts only when
  the workflow's grants, members and tools are within `approved`; without a board in the settings a workflow cannot be approved, so
  `workflow_create` refuses (`FailedPrecondition`, field `board`, naming the Workflow
  module's setting "Board of approvals"). It refuses as well, before anything is kept or
  asked (owner 2026-11-14: a template installed in a new organisation failed only at its
  first run, three times, each for another thing missing), a workflow whose `agent` blocks
  ask what is not there: a member that is in no team of the workspace or is no agent of
  the harness (field `agent.member`), no module in the agent slot, no model listed by a
  provider; each refusal says where a person adds it (Teams, the Agent module, Models). It
  is a check of the moment: a member removed later fails the run's step, as before. A
  create is all or nothing: a new
  workflow whose decision the board refuses is not kept (owner 2026-11-01); one kept without
  its question (a change the board refused) is asked again at the module's start. The module
  asks in its own name (`workflow:<name>`, team.md: a module asks, it does not decide).
- *Dry run*: nothing is stored; the run's journal is kept in memory and is the answer. Reads
  are real; a `write` or `external` `tool` block answers its `fixture` (a template of its
  response) or its response's zero values; `wait` blocks pass at once, an `approval` is
  approved, `state_put` and `state_delete` change only the dry run's copy, a `call` is a dry run
  of the called workflow, no event is emitted, `model` and `agent` blocks are real only when the
  request asks (`models`); it lasts at most 5 minutes.

## Templates (owner 2026-11-14)

- *What a module ships*: a manifest's `workflow_templates` (`pcx.module.v1.WorkflowTemplate`,
  host.md): each a name (the workflow's, unique among the module's), what it does, the
  workflow as the proto3 JSON of `pcx.workflow.v1.Workflow` (text: `pcx_module.proto` cannot
  name the message), and its parameters, each a name (1 to 32 of a-z, 0-9 and `_`), what it is,
  a default (empty: it must be given) and a pattern every value matches whole, which a
  parameter must have, so a value cannot break the text it is put in (a quote in a filter). A
  string of the workflow names a parameter as `${<name>}`; the workflow's own `{{ }}` stay as
  they are. Discord ships six (discord.md); a dynamic module may ship its own.
- *Listed*: `workflow_templates` (`ListTemplates`, read) answers the templates of the running
  modules whose tools the caller reaches (one module's when it names one), by module and name,
  each with the workflow of its name when one is kept (`installed`): a template is installed
  once under its name.
- *Installed*: `workflow_install` (`InstallTemplate`, write: a module, a template, a value for
  each parameter to set) puts the values (the given or the default; a value given for no
  parameter, one missing, or one that breaks its pattern is refused naming `values.<name>`;
  a template that does not read, or names a parameter it does not declare, is refused naming
  `template.workflow`), reads the workflow named as the template and writes it as
  `workflow_create` does: checked against the caller's tools, refused when the board of
  approvals or what its `agent` blocks ask is missing, and its grants asked for on the
  board. It is then a workflow like any other, changed in the editor; a release does not
  change an installed one.
- *The app*: the Workflows page lists the templates under the workflows, each with what it
  does, its module, and Install (a dialog with each parameter, its default filled) or Open
  when it is installed; once installed its grants wait for a person as a new workflow's do.

## The builder (owner 2026-10-31)

- *The skill* `workflow-builder` is a built-in skill of `skills` (skills.md): the graph's
  format as `workflow_create` takes it (the proto JSON mapping of `Workflow`), how to find the
  blocks a workflow may use, CEL and templates, triggers, approvals, errors and retries, the
  approval of grants, and worked examples.
- *The catalog*: `workflow_tools` (`ListTools`, read) answers each tool a workflow written by
  the caller may call as a `tool` block: its gRPC path, tool name, module, effect,
  description, and the JSON Schemas of its request and response messages (the proto JSON
  mapping, as the host's tools read them), so an agent writes requests and reads
  `steps.<id>.<field>` without guessing.
- *The agent* `workflow-builder` is a built-in agent of `team` (team.md): a harness agent with
  the skill. Its modules are all its team reaches (owner 2026-10-31): a workflow asks only for
  grants within its author's tools, so an agent limited to `workflow` and `skills` could write
  no workflow that calls another module; its role and skill keep it to building workflows,
  and every workflow's grants still wait for a person's approval. It turns a request in words
  into a workflow: reads the catalog, writes the graph, validates it, runs it dry, tells what
  it will do and which grants it asks for, then creates or updates it (and, for a `schedule`
  trigger, adds the scheduler's job); a person approves the grants on the board.

## The editor (owner 2026-10-31)

- *The canvas* (the app, `90df6515`) is a client of the module's RPCs with no logic of its own:
  its palette is `workflow_tools`, its check `workflow_validate`, its writes `workflow_create`
  and `workflow_update`, its test `workflow_dry_run`, and a run is followed by `WatchRun`.
- *Layout*: `Workflow.layout` keeps where each block is on the canvas: by block id, the
  block's top left corner in canvas units. Only the editor writes it; the engine never reads
  it. The check refuses a position whose key names no block of the workflow (of its bodies
  and branches too). A block without a position is placed by the editor's layered layout, so
  a workflow the builder wrote opens arranged. A change of the layout alone is a version like
  any other and asks no approval (the grants did not change).
- *The workflow's inspector* (no block picked): its name, what it does, its input, its
  output, its budgets (steps; wall time in seconds, empty for the module's default, waits
  included, owner 2026-11-10) and its grants.
- *A schedule trigger* (owner 2026-11-08): the inspector of a `schedule` trigger lists the
  scheduler's jobs that name the workflow (`ListJobs`: when, next time, paused), opens one on
  the scheduler's page and adds one there for this workflow (docs/modules/scheduler.md, in
  the app); without the scheduler on, it says the module is off.
- *Watching a run*: `WatchRun` (a server stream; not a tool: no tool name, visibility `app`,
  effect `read`) answers the run with its steps first, then each change of the run or of one
  of its steps as it is written, and ends after the run ends. A watcher that falls behind is
  sent the whole run again. The engine announces the changes inside the module's process
  (one engine per organisation), not over the bus. A dry run is not watched: its answer holds
  its steps, which the editor replays with their times.

## Expressions: CEL

- The Common Expression Language (owner 2026-10-31): typed, no loops but its macros (`all`,
  `exists`, `map`, `filter`), no IO. The crates `cel-core` and `cel-core-proto` (MIT or
  Apache-2.0; 0.5) parse, type-check and evaluate, and pass the `cel-spec` conformance suite
  (parse, check and eval; run here on 2026-10-31); `cel-core-proto` reads proto messages
  through `prost-reflect` 0.16 and prost 0.14, the workspace's versions (owner 2026-10-31:
  `cel-core`; `cel` has no checker, `cel-cxx` needs cel-cpp's C++ build). The types an
  expression is checked against are the proto messages of the blocks (each module's
  descriptors) and the `Schema`s of the workflow. A value is kept (a step's input and output,
  the store) as a `pcx.workflow.v1.Value` (null, bool, int, uint, double, string, bytes,
  timestamp, duration, list, map, or a message as `Any`, of the workflow's own descriptors: another
  module's is kept by its fields, as a map, so a person or an agent reads a run without that module's
  descriptors, and a run outlives a module removed); at run time it is `pcx-expr`'s
  value, whose text, bytes, lists and maps are shared as CEL holds them (owner 2026-11-01:
  one representation while a run runs), so handing a value to CEL, taking one back and
  copying one copy a pointer, not the items. The two are converted in one place, when a step
  is recorded and written.
- `cel-core` is reached only through `pcx-expr` (parse, check, evaluate), so
  replacing or forking it (one maintainer, last release 2026-02) changes that crate alone; the
  `cel-spec` conformance suite runs in CI against the version pinned.
- Limits at save time (ours: `cel-core` has none): an expression of at most 4 KiB, its
  brackets at most 32 deep, its AST at most 1,000 nodes and 32 deep; a template is compiled on
  a thread whose stack holds the parser's recursion over 4 KiB. At run time each evaluation
  has a budget of 100,000 steps, each node it visits and each iteration of a comprehension one
  (a `for_each` body counts per item): `cel-core` 0.5.1 has none, so the workspace patches it
  (`third_party/cel-core/PATCH.md`, owner 2026-10-31: deterministic, as cel-go's cost limit,
  instead of the 10 ms first written here) until a release has one.
- Patches for speed (owner 2026-11-01, same file): a member read (`item.name`) writes no
  qualified name when its leftmost identifier resolves; a text key of a map is looked up
  without an `Arc<str>` made for it; a node's reference is found in a list sorted by id
  instead of a `HashMap` hashing every node's id.

## Data blocks (owner 2026-11-01, plan `000401af`)

Today a list is kept with `set` and `steps.x.items.filter(i, ...)`, reshaped with `set` and
`map`, and checked with `if` and an `error` edge, one condition at a time. Three blocks of
`pcx.workflow.v1.Block` make them steps of their own, with a palette entry, an inspector and a
preview. Each is pure (no call out, effect `read`), so a dry run runs it for real.

- **Items.** Each of the three takes `items`, a CEL expression that gives `list<T>`, or (`transform`
  and `validate`) `value`, one value instead. In its per-item expressions `item` is the item
  (of type `T`) and `index` its place (`int`); every other variable before the block is there
  too (`input`, `steps.<id>`, an outer `for_each`'s `item` hidden by this one).
- **`filter`** (`FilterBlock`):
  - `where`: a CEL `bool` for an item; the items for which it holds are kept, in their order;
  - `distinct_by` (optional): a CEL key for an item (`string`, `int`, `uint` or `bool`); of
    items with one key only the first is kept;
  - `limit` (optional, 0: none): at most this many are kept; the rest are not evaluated.
  - Its output: `steps.<id>.items` (`list<T>`, the type of the input's items),
    `steps.<id>.dropped` (`int`, the items not kept).
- **`transform`** (`TransformBlock`):
  - `output`: a `Schema` (as a `model`'s) and `fields`, for each field of it a CEL
    expression whose type is the field's (a required field must have one; another without one
    is its zero value);
  - with `value`: its fields' expressions read it as `value`, and its output is the
    schema's fields, `steps.<id>.<field>`, as a `model`'s;
  - with `items`: one record of the schema for each item, `steps.<id>.items`
    (`list<R>`, `R` the schema's record type, below), so `steps.t.items[0].total` checks;
  - `sort_by` (optional): a CEL key over `row` (the item's new record) and `index`, of an
    ordered type (`string`, `int`, `uint`, `double`, `timestamp`, `duration`), and
    `descending`; the sort is stable;
  - `group_by` (optional, with `items`): a CEL `string` key over `row`; the output is then
    `steps.<id>.groups`, a list of the block's group records (each its `key` and its `items`,
    `list<R>`, in the order each key first appears).
- **`validate`** (`ValidateBlock`):
  - `schema` (optional): a `Schema` the value or each item is read by: its required fields,
    types and each field's constraints (below);
  - `rules`: each a CEL `bool` over `item` and `index` (or `value`), its `message` (a text
    with `{{ }}` parts, a template's) and the `path` it names (`end`), for what a schema does
    not say (`item.end > item.start`);
  - every rule and every constraint is checked, not only up to the first broken one: its
    output lists every violation, `steps.<id>.violations`, each its `path` (`items[3].email`),
    its `rule` (a constraint's name, or the rule's index) and its `message`;
  - with `items`: `steps.<id>.valid` (`list<T>`, the items with no violation) and
    `steps.<id>.invalid` (each its `index`, the item being `items[index]` of the input, and
    its violations: an item of any type has no proto field of its own, and JSON is no
    edge here);
  - its ports: `valid` (no violation, or with `items` none at all) and `invalid`. Without an
    edge from `invalid`, a violation fails the block with the kind `invalid` and the
    violations in the message (the first 20, and how many more), so its `error` edge,
    retries (none: a failure of this kind is not retried) and `workflow.run_failed` hold.
- **Constraints of a field.** `pcx.workflow.v1.Field` gains `Constraints`: `minimum` and
  `maximum` (numbers, timestamps, durations), `min_length` and `max_length` (text in
  characters, bytes, lists, maps), `pattern` (text, the `regex` crate's syntax: linear time, no
  backreferences, at most 1 KiB) and `one_of` (constants of the field's type). A `model`'s and an
  `agent`'s output and a workflow's input are read by them too, and the JSON Schema of a
  model's forced tool carries them (`minimum`, `maxLength`, `pattern`, `enum`), so a model is
  told the bounds it is held to.
- **A schema's record type.** The check makes, from a `transform`'s output `Schema`, a proto
  message of its own (`pcx.workflow.record.w_<workflow, '-' as '_'>.<block id>`, its fields
  the schema's, and `<block id>_group` with `key` and `items`), made by `prost-reflect` in the
  catalog of the version checked (a called workflow's records too) and never stored, so a list
  of records is typed field by field (`R` above) instead of `map(string, dyn)`. A record holds
  one level: a list or a map of lists or maps is refused. At run time a record, a group and a
  violation are maps of their fields (CEL reads `row.age` of a map as of a message), so no
  message is built or copied for each item; a field of a `dyn` expression is checked against
  its type then, and a null or missing one is its zero value.
- **Functions** that every expression gains in `pcx-expr` (`cel-core` has the strings and
  math extensions already: `lowerAscii`, `split`, `join`, `replace`, `trim`, `math.round`,
  `math.greatest`):
  - lists: `lists.flatten`, `lists.slice(list, from, to)`, `lists.range(n)`,
    `lists.sum(list)` (of `int`, `uint`, `double` or `duration`), `lists.distinct(list)` (of
    keys);
  - times: `time.parse(text, layout)` and `time.format(timestamp, layout)` (`strftime`
    layouts, UTC, or a zone named as the IANA names it as a third argument).
  - Sorting and grouping are a `transform`'s fields, not functions: CEL has no lambdas outside
    its macros, and a macro of our own would patch `cel-core`'s parser.
- **Speed** (target: a `filter` of 100,000 items with a short `where` within 100 ms on `ml`,
  measured by the domain's `speed_of_100000_items`, run in release, which fails past it):
  - each expression of a workflow version is compiled once, when the run's version is
    checked, and kept with what the check found (`Found`: each block's expressions and texts,
    each graph's output, each data block, the texts of a `tool`'s and a `call`'s templates by
    the path of the field each fills; an `event` trigger's filter when the engine reads the
    triggers);
  - a step's input and output are read once, when the run's journal records the step (the
    engine keeps one journal while it drives a run), not again for each block after it; a
    list kept as a blob (above 48 KiB) is read once with its step;
  - the variables reach CEL as shared values (above), each when an expression first reads
    it; each item binds only `item` and `index` over them (a layered `Activation`, with no
    allocation of its own);
  - a violation's path (`items[3].email`) is written only for what broke;
  - the items are evaluated in waves of 1,024 over the cores (owner 2026-11-01), each on a
    copy of the block's meter, and their results and steps taken in order as one item after
    another would be: the first failure in order, the steps running out, a `filter`'s
    `limit` end the block where a sequential run would, so what it gives does not depend on
    the cores; at most one wave's work is done past that point;
  - a `transform` field whose expression's type has no `dyn` in it is not checked again when
    it runs (the check accepted its type);
  - the patches of `cel-core` (`third_party/cel-core/PATCH.md`): a member read writes no
    qualified name when its leftmost identifier resolves, a map's text key is looked up
    without making one, and a node's reference is found in a sorted list, not hashed;
  - budgets: each item's evaluations have 100,000 steps of their own (as one expression has
    now); a block has at most 1,000,000 items and 50,000,000 steps in all, and fails with the
    kind `budget` past either. A block counts one step of the run's `steps` budget, as now.
- **The editor and the builder.** The palette shows the three under "Data"; each inspector
  edits its expressions (and a `transform`'s schema field by field) and previews them over the
  steps of the run or dry run it is opened from, with the items in and out (a `filter`), the
  records (a `transform`) or the violations (a `validate`), through `PreviewBlock` (read,
  visibility `app`, not a tool): the workflow, the block and its graph (`at`, as the journal
  names it), the run's input and steps; the module walks the steps as a run does, without the
  block's, the `for_each` and `parallel` around it open again, and runs the block over the
  values that gives it, a violation shown rather than failed; nothing stored. A data block
  runs on a blocking thread, within its timeout. The skill `workflow-builder` gains the three
  blocks and an example.

## The check

When a workflow is written (`workflow_create`, `workflow_update`) and before every run, the
domain checks it, and a failure names the block and the field (`workflow_validate` answers it
as `Validation.refused`, block, field and message, not as an error, so the canvas marks the
block; a write refused fails with it):

- every `tool` block names an RPC of a running module the workflow's grants reach, that an
  agent may call (`model` in its visibility); its template's fields are fields of the request
  message and their types match;
- every CEL expression type-checks against what precedes it: `steps.<id>` only of blocks on
  every path to it, `input` against the input `Schema`;
- the graph has one start per trigger, no cycles (a loop is only a `for_each`), every edge's
  ends exist, every block is reachable;
- the grants it asks for are a subset of its author's (below);
- the effect class of each `tool` block is known (host.md, effects), which the dry run, the
  approvals and the retries read.

The agent and the canvas (`90df6515`) build the same graph and run the same check; the canvas
is a client of the module's RPCs, with no logic of its own.

## The engine

- **Durable**: every attempt of every block is a step of the run's journal (document
  `workflow/step`: the run, the block, the attempt, its input, its output or failure, its
  start and end), written before the next block starts; a run (document `workflow/run`: the
  workflow and its version, the trigger, its status `running`, `waiting`, `succeeded`,
  `failed` or `cancelled`, when it waits until, its budgets used) goes on after a restart from
  its journal without running a finished step again. A step's output above 48 KiB is kept as
  a blob of the workspace and named by the step.
- **Idempotency**: a call of a `tool` block whose effect is `external` carries the key
  `<run>:<block>:<attempt>` (metadata `x-pcx-idempotency-key`); a module that acts outside
  passes it on where the outside API takes one, so a retry after a crash acts once. A `read`
  block is retried freely, a `write` block only when the module reports the failure retryable
  (`Unavailable`, `DeadlineExceeded`) or keys its writes (the board's task `key`).
- **Dry run** (`workflow_dry_run`, of a workflow kept or not; see triggers and approval):
  reads are real; a `write` or `external` block is not called, its output an example message
  of its response type (its fields' zero values, or a fixture the block names); `model` and
  `agent` blocks are real only when the dry run asks for it (they cost).
- **Who it is**: a workflow is a participant with rights, a subset of its author's (the
  modules it may call, the workspaces it reaches); running a new workflow or widening its
  grants is a board decision a person approves (risk `code`), as a team member's trust is.
  Its calls run in a scope whose author is `workflow:<name>` and whose reach is its grant.
- **Budgets** (per run, from the workflow, under the organisation's limits): steps (default
  1,000), wall time (default 24 h, waits included), model tokens and cost, `call_depth` (a
  `workflow` block and a `call` trigger count one, the host's limit of 4 holds).
- **History and alerts**: a run's steps with input and output are its history
  (`workflow_runs`, `workflow_run_get`); each block's time and outcome are metrics of the
  module (`admin health`); when the three runs of one workflow that ended last all failed,
  `workflow.failing` is emitted, which a channel or a workflow may follow. A run that
  failed is a warning in the host's log too (owner 2026-11-14: a failed run was seen only
  by who opened its page), with the workflow, the run and the failure's kind, never its
  message: that may carry what a stranger wrote, and agents read the log. Runs and steps
  are read by their keys and by page (owner 2026-11-06; host.md, pages in a module): a
  run's steps by the run, a workflow's runs by the workflow, the open runs by their status,
  never every run.

## Tools

`workflow_create`, `workflow_update` (from the version read), `workflow_validate` (the check,
nothing stored), `workflow_dry_run`, `workflow_run` (the input; answers the run's id),
`workflow_runs` (a workflow's runs, the most recently changed first, one page of at most
`limit`), `workflow_run_get` (a run with its steps), `workflow_cancel`, `workflow_list` (each
workflow with its run that changed last), `workflow_tools` (the catalog: the builder,
above).
`WatchRun` and `PreviewBlock` (data blocks, above) are the app's only (the editor), not
tools.
Effects: `workflow_run` is `external` (a run may do anything its grants allow), the writes are
`write`, the rest `read`. Events emitted:
`workflow.run_succeeded`, `workflow.run_failed`, `workflow.failing`.

## Layers (LAW rule 6)

- **domain** (`pcx-workflow::domain`, pure): the graph and its types, the check (CEL through
  `pcx-expr`: parse, check, evaluate under limits), the scheduling of a run's next blocks from its
  journal (what is ready, what a `parallel` or `for_each` waits for), retry and backoff times.
- **ports** (`pcx-workflow::ports`): `Workflows` (definitions), `Runs` (runs and their
  journal), `State` (the key-value store), `Tools` (call an RPC by path with a scope),
  `Models`, `Turns`, `Events` (follow and publish), `Clock` and `Timers`.
- **adapters**: `Workflows`, `Runs` and `State` over memory's documents (`pcx-documents`), so
  RocksDB and SurrealDB both hold them through memory's own adapters and one writer per
  tenant store holds (T10); `Tools` over the host channel (the route checks the grant and the
  visibility as for any call); `Events` over the host's bus.
- **capability** (`pcx-workflow::service`): the engine and each tool's one method.
- **module**: the manifest and the handlers, which only map.
- *Why documents* (owner 2026-10-31): a workflow module in the cloud is a process that cannot
  open its tenant's store (one writer, T10), and documents already give versions, history,
  conflicts and both engines. One addition to memory: a document kind may be left out of
  search (`DocumentKind.searchable`, default true); the steps, runs and state are not
  embedded.
