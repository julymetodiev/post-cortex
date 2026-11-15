# chief

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8.

- **`chief`** (W22; owner 2026-10-25): the host's
  main agent, which reaches every module and operates the system for the people who own it.
  - *What it is*: one agent per host (a tenant in the cloud), with its name as its author
    (`chief` by default), its instructions, its model, and the tools it may call. It runs
    turns through the agent module like a team member; each conversation is a session of its
    own in the chief's workspace (`chief/talk`: who asked, the channel, the session).
  - *Its tools* (owner: person-only tools only by hand): by default every tool the host lists
    that an agent may call. A person-only tool (`visibility` `app`: team management, autonomy,
    `approve_decision`, `revoke_approval`) is callable only when a person granted it by name;
    a person may also withhold any tool. The grant is enforced by the host, not by the chief
    module, so the chief cannot widen it: the host refuses a person-only tool to an agent's
    call unless the grant of the module whose turn it is names it (section 7, the agent: a
    module's turns, W22a).
  - *Who changes its rights* (owner: in the UI, in a view): the chief's view shows its tools,
    the person-only ones it was granted and the ones withheld; changing them is a person's,
    and audited (where they are kept: below).
  - *Reached from outside* (owner: gRPC and MCP first; Telegram and Slack as modules later):
    gRPC through `pcx.agent.v1.Agent/RunTurn` with the chief as the turn's agent (owner
    2026-10-25: `RunTurnRequest.agent`, the module whose turn it is; empty: the module in the
    `agent` slot, `chief`: the chief's `Module::turns()`); MCP through
    a tool `chief_ask` (the question, an optional conversation id; answers the chief's last
    text and the conversation id), which a channel module (Telegram, Slack) also calls. The
    caller's author is recorded as who asked; the chief acts as its own author. A turn outlives a
    connection that drops: its conversation (`chief/talk`) says whether a turn runs and what the
    last one ended with.
  - *The team* (owner: yes): it wakes and directs members through the board as people do (a
    task assigned to a member, a comment that mentions one), adds and moves tasks, and records
    decisions; on a team it is a member whose authority a person sets, and the board's
    authority and autonomy checks apply to it as to any member.
  - *Audit* (owner: every action, visible in the UI): every tool call of a chief's turn is a
    `chief/action` document (the conversation, who asked, the tool, its input, whether it
    failed, when); the chief's view lists them newest first, by conversation. The chief's
    service writes each action from the turn's events (a tool call and its result), not the
    model (owner 2026-10-25), so no action goes unrecorded; the host writes no module's
    documents.
  - *Layers*: the rules (the action's record, the budget) are pure domain code in
    `pcx-chief`; which tools a grant allows is `TurnGrant` of `pcx-module-api`, checked by the
    host; the service reaches documents through `pcx-documents` and turns through
    `ModuleHost::turns()`; the tool handlers only map JSON.
  - *Its reach* (owner 2026-10-25): `Manifest.uses` names each module it may call; its
    catalog entry (or `[[builtins]]` entry) grants them. No grant for "all modules"; naming `dynamic` also
    reaches the modules a person approved there (`dynamic.md`, who reaches a module).
  - *Its workspace* (owner 2026-10-25): one per host, the module's required setting
    `workspace_id` (as `skills`). Its conversations and actions live in the workspace it is
    asked from (owner 2026-11-10; docs/connect.md section 4a, a module's data), so a person sees
    and goes on with those of the workspaces they reach; asked from none, in its own. Its
    budget is one for the host: the day's use stays in its own workspace.
  - *A budget* (owner 2026-10-25: there is a limit): a cap on its turns' use per day; past it a
    turn is refused with `ResourceExhausted` until the next day. A day's use is a `chief/day`
    document per UTC date in its workspace (the tokens its turns used that day), added to after
    every turn (owner 2026-10-25).
  - *Where its rights are kept* (owner 2026-10-25): in the module's settings (`person_tools`,
    `withheld`: the `TurnGrant` every module that asks turns may carry), which only
    `ConfigureModule` changes (a person's call, audited by the cloud) and the host checks; the
    host enforces them on every call of the chief's turns and reads no module's documents. The chief's view shows its rights and opens the settings form to
    change them.
  - *Its budget and model* (owner 2026-10-25): the budget is tokens per day, set in its
    settings (default 2,000,000); its model is the agent's, unless its settings name one. A
    turn's tokens are its input, output and cache writes (not cache reads). The budget is
    checked before a turn; the turn that crosses it runs to its end.
  - *Its settings* (owner 2026-10-25): `workspace_id` (required), `name` (its author;
    default `chief`), `instructions` (added to the agent's system prompt; empty: none),
    `model`, `budget_tokens`, and the grant's `person_tools` and `withheld`.
  - *Its uses* (owner 2026-10-25): `post-cortex.memory`, the agent, `board`, `team`,
    `skills`, `tools-fs`, `tools-shell`, `tools-git`.
  - *A conversation* (owner 2026-10-25): `chief_ask` runs the turn within its call's
    deadline and answers when it ends; the turn's scope is the chief's (its name as the
    author) in the workspace it is asked from (the asker's scope's; owner 2026-10-26), so its
    calls work there; asked from none, each call of the turn names the workspace it works in
    (a scope's workspace wins over the input's), and one that names none, as a tool without the
    field (`bash`) cannot, runs in the chief's own (`RunTurnRequest.workspace`; 2026-11-02); the question is written as who asked
    (`RunTurnRequest.asker`), the answers as the chief; its tool is for agents and views. The channel is the
    module that asked (`Scope.caller_module`), empty for a client of the host. A conversation
    is continued only by who started it, from the same channel; any other is `Denied`.
  - *Its turns through `RunTurn`* (W22c, owner 2026-10-25): a client's turn of the chief
    (`agent` `chief`) runs as `chief_ask` does, its events streamed as the agent's: the
    request's session is the conversation (it must exist); the chief keeps a `chief/talk` for
    it, made on its first turn (who asked: the header's author; the channel: empty, a client
    of the host), so only who started it goes on with it. It takes only the session and the
    prompt: instructions, a model, modules, tools or `withheld` are `InvalidArgument` (its
    settings are a person's, through `ConfigureModule`).
  - *Its view* (W22c, owner 2026-10-25): a `settings` action (its rights and budget are its
    settings); its grant (`person_tools`, `withheld`) and today's use of its budget
    (`chief_rights`); its conversations newest first (`chief_talks`: who asked, the channel,
    the first question), each one's detail its actions newest first (`chief_actions`: the
    tool, whether it failed, its input as an object, when), each action's detail its whole
    input (`chief_action`), shown with `fields`; its rights as an object (`chief_rights`:
    `budget_tokens`, `used_today`, `person_tools`, `withheld`), shown with `fields` and the
    `settings` action (owner 2026-10-25). The four tools return data only and are for the view
    only (visibility `app`).
  - *An action's record* (owner 2026-10-25): written when the tool's result comes; a call
    with no result (a cancelled turn) is written at the turn's end without `failed`; a record
    that cannot be written fails the `chief_ask` that ran the turn.
