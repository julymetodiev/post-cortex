# agent

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 7, the agent.

- **The agent** (W18; owner 2026-10-25): the harness's own agent, a working turn end to end:
  a person (or a client) sends a prompt, the agent calls a model and the tools it may use until
  the model answers, and the turn is kept in memory.
  - *Slot*: a new exclusive kind and slot, `agent` (`MODULE_KIND_AGENT`), like `memory` and
    `context_engine`.
  - *Models*: the `ModelProvider` port in `pcx-module-api`, provided as the sandbox is
    (`Module::model_provider()` for a module of kind `model_provider`; the
    `pcx.module.v1.Model` service on the callback socket for a process module): the models it
    serves, and `generate` (a request: model, system prompt, messages of blocks (text,
    thinking, tool use, tool result), tools, the most tokens, tool choice) answering a stream
    (text and thinking deltas, tool uses, usage, the stop reason, or a typed failure:
    `rate_limited` with its retry time, `overloaded`, `context_too_long`, `unauthorized`,
    `quota_exhausted` (a plan used up until it resets: not retried),
    `unavailable`, `invalid`). A provider classifies a failure where it happens (the HTTP
    status, the transport error), never from a message's text; the agent's recovery acts on
    the kind (retry with backoff, compact, stop). The host routes a request
    to the first provider that lists its model; when none lists it yet, once the listings the
    providers began at start are in (within the request's deadline: a provider started a moment
    ago lists its table's models until its API's listing is in); two providers listing one
    model at load are refused.
    Reached through the host handle (`ModuleHost::model()`), by the module in the `agent` and
    `context_engine` slots only. Its turns' clients list them with `pcx.agent.v1.Agent/Models`
    (owner 2026-10-30): the models a turn or a team's member may name, as its host's providers
    serve them (`Turns::models`; turns that do not choose their model, a module's own, answer
    `FailedPrecondition`), so the app offers them where a model is set.
  - *A model* (owner 2026-10-31, `ea0eac90`): one shape for every provider, the generated
    `pcx.module.v1.ModelInfo` (the hand-written copy in `pcx-module-api` goes, section 2 rule
    10): its id, its name for a person, its provider (as its module names it, the `models` module
    `openrouter`, `anthropic`, `zai` or `openai`; else the module's id, set by the host), its
    kind (`chat`, `embedding`, `image`, `speech`, `transcription`, `video`, `rerank`), the
    modalities it takes and gives (`text`, `image`, `audio`, `video`, `file`), what it can do
    (`reasoning`, `tools`, `structured_output`, `prompt_cache`), its context and output tokens,
    its price when known (US dollars per million tokens of input, output and cached input, in
    micro-dollars; unset: not known). A provider lists every model it serves (from its API
    where it has one, read at `start` and at each reload, else from a table in the module);
    the host routes `generate` to any of them, and `Agent/Models` and `Host.ListModels` (the
    app's models screen and its model fields) answer every one, each provider's in one listing.
  - *The models* (owner 2026-11-09, replacing the choice of 2026-10-31): a provider's API key
    is all it needs; every model its API lists may be named, none is turned on first (the
    settings' `models` and `ModelInfo.enabled` are gone, their numbers reserved; the cloud's
    stored settings lose `models` in a migration). A setting naming a model (the `model`
    option of `pcx.module.v1.Setting`: the agent's and the chief's `model`) is offered every
    model of the host's providers, each with its provider, with an id typed in taken as it
    is. A model its API serves that its table does not know is listed as a `chat` model on
    `text` with its window and output unknown (0); the agent counts an unknown window as
    131,072 tokens. A provider begins its listing at `start` and lists in the background (owner
    2026-11-01): a start does not wait on the provider's API, so a tenant loads in the time of
    its other modules; until the listing is in, its models are its table's
    (`pcx_module_api::model::Listed`). A listing that fails (the API down) leaves the
    table's models; a refused key fails the listing, logged and answered by `Models` with
    `settled` (the listing's error, the key as its field), and the provider's calls fail as
    `Unauthorized`. A process provider is listed again once it started (its listing at launch
    had no key), and once more when its listing is in (`ModelsRequest.settled`, in the
    background).
  - *The providers* (owner 2026-11-09: every provider in one module, not a module for each):
    one module, `models` (`pcx-models`, kind `model_provider`, the `network` grant), whose
    settings (`pcx.models.v1.ModelsConfig`) have a section for each provider (`openrouter`,
    `anthropic`, `zai`, `openai`), each its API key (a `secret`) and its API's URL (empty: the
    provider's own). A provider is on when its key is set (with none set the module runs and lists no model: a
    module that does not start would take its host's others down); its models are
    listed with the others', each with its provider's name, and a request goes to the first
    provider on (in the settings' order) that lists its model. A key a provider refuses fails
    the start (or the listing) on its section's field (`zai.api_key`). Each provider is a
    library the module starts with its key (`KeyedProvider`): `pcx-provider-zai`,
    `pcx-provider-openai` and `pcx-provider-openrouter` on `pcx-openai-compat` (its
    `GET …/models` beside the chat completions URL), and `pcx-provider-anthropic` on
    Anthropic's Messages API (thinking, tool use, images and PDF in), an API key only (owner
    2026-10-31: no OAuth, a subscription is not for other applications). OpenRouter's listing
    gives modalities, windows and prices; Z.AI's, OpenAI's and Anthropic's give ids (and
    names), the table the rest. A new provider is a library and a section of the settings. The
    modules `provider-zai`, `provider-openai`, `provider-openrouter` and `provider-anthropic`
    are gone (the cloud's stored settings of them are removed in a migration: their keys are
    sealed to their module, so a person sets the key again in `models`).
  - *Its tools*: the tools of the modules its `uses` grant names, called through the host
    handle as any module calls another (W14); the operator decides what the agent may do.
  - *Events*: the agent dispatches `TURN_START`, `TURN_END`, `PRE_MODEL_CALL`,
    `POST_MODEL_CALL`, `PRE_TOOL` and `POST_TOOL` to the subscribed modules through the host
    handle (`ModuleHost::dispatch`, the `agent` slot only), so hooks (Claude Code and Codex
    JSON) run in its turns as in theirs; a blocking outcome stops the tool call or the turn.
  - *The transcript*: a turn is kept in the session's message stream (Messages), whole: each
    message gains its parent (`parent`, the `turn_id` it follows, for forks) and its blocks
    (text, thinking, tool use, tool result, summary), its `text` stays what search and views
    read. A session is
    resumed from its stream. On-disk change (stated exception): messages written by this
    version carry the two fields (old messages read with none); export 1.6.0. The agent reads
    and writes the stream through memory's `manage_messages` tool (owner 2026-10-25: `append`,
    `page`; visibility both, so any agent may read and write transcripts) over its host
    handle; a message with blocks and no text is kept (only one with neither is skipped).
  - *Its tools and events, through the handle*: `ModuleHost::tools()` lists the tools of the
    modules the caller uses; a turn is given those its request allows, and a call the model
    makes to any other is refused as the tool's error; a turn's calls and events are in its
    session's scope; `ModuleHost::dispatch` sends an event to the subscribed modules,
    for the module in the `agent` slot only.
  - *Recovery* (owner 2026-10-25), by the failure's kind: `rate_limited` and `overloaded`
    retried up to 3 times with backoff from 1 s to 30 s (or the provider's retry time);
    `context_too_long` compacted once (unless the turn compacted already), then retried; `unauthorized`, `quota_exhausted`,
    `invalid` and `unavailable` past its retries end the turn with the failure.
  - *Context*: the module in the `context_engine` slot gives a turn its context (recall
    through memory's `assemble_context`) and compacts a long transcript (a summary through the
    model, written as a record of the session, each tool's result cut to 2,000 characters for
    it); without one, the agent sends the transcript as it is.
  - *The provider's cache* (owner 2026-10-25, after Axon's): a request's start stays the same
    from turn to turn, so a provider that caches a prompt's prefix (Z.AI) serves it: the system
    prompt is the agent's and the turn's instructions only; what the modules add to a turn and
    what memory recalls for it go with its prompt as a context block, kept in the transcript
    with it (the turn's `text`, what search reads, is the prompt alone), and not again while the
    conversation carries the same block; the tools are listed by name. Measured on Z.AI: 99%
    of each turn's input from the second turn on. An explicit cache (Anthropic's
    `cache_control`) comes with its provider.
  - *The window* (owner 2026-10-25, Axon's thresholds): before each model call a request of
    about (four characters a token) more than the model's window less 16,384 tokens is
    compacted first; after a step that called tools this waits at most twice. A compaction
    cuts only before a person's prompt (a tool's call and its result stay together), at the
    oldest one whose turns from it on are at most about 20,000 tokens, else the newest; the
    summary is kept as a turn and the turns from the cut on again after it. A tool's result
    longer than 50 KB of characters (Axon's cap of a tool's output; the tool modules cut their
    own the same way) is cut for the model, which is told how much and how to ask for less. It is the `ContextEngine` port of `pcx-module-api` (`recall`, `summarize`;
    `Module::context_engine()`), reached by the agent through its handle
    (`ModuleHost::context()`, the `agent` slot only; `pcx.module.v1.ContextEngine` on the
    callback socket of a module process). A compaction (owner 2026-10-25) is kept in the
    transcript too: a turn with a `summary` block, after which the newest turn is kept again;
    the conversation a model is sent starts at the newest summary, so a later turn does not
    compact again. A turn compacts once; a conversation still too long ends it.
  - *A turn*: `pcx.agent.v1.Agent/RunTurn` (session, prompt) answers the turn's events as a
    stream (`started` with the turn's id, text, tool calls and results, usage, the end or a
    failure), served by the daemon next to `pcx.v1`; `CancelTurn` (the id) cancels a running
    turn, whose end (`cancelled`, with its steps and usage) still comes on its stream; a turn is
    also cancelled when its stream closes. The module in the `agent`
    slot runs turns through the `Turns` port of `pcx-module-api` (`Module::turns()`, as a
    provider gives its models), and the host gives the one of its agent (`Host::turns()`), so
    `RunTurn` reaches the running agent on every call, across reloads. `pcx agent "<prompt>"` in
    the CLI is its first client.
  - *The app's chat* (W25; owner 2026-10-26, the canvas's `AgentChat`, `Agent`, `Teams` and
    `Chief`): one chat for every agent of the host (the agent, the chief, a team's member),
    a page (its conversations, the transcript, what the agent may use) or a dock beside any
    view (its conversations in a menu).
    - *A conversation* is a session of the agent's workspace with its message stream; the app
      lists an agent's conversations newest first (who asked, the channel, the first
      question, its state: idle, running, failed) and opens one from its stream (`page`).
      Owner 2026-10-26: an agent's conversations are the streams it wrote in as its speaker
      (`RunTurnRequest.speaker`: the agent, `chief`, a team's member by its name; the agent
      writes a turn's prompt as the person who asked and its own turns as the speaker), listed
      by `ListConversations` with `speaker`, which answers who asked and the first question
      (the first prompt) and how each stands (`core::conversation`: a prompt with no end
      after it runs, an end that failed failed, else idle). The agent keeps each turn's end
      in the transcript as a block (`End`: the stop and its detail; on-disk change, stated
      exception: a message block kind; never sent to a model).
    - *Live*: the app runs a person's turn through `RunTurn` for any agent (`agent` names the
      module whose turn it is, as for the chief; a team's member through the team module's
      `Module::turns()`, `agent` `team` and the member named in `RunTurnRequest.member`, its
      role, skills and tools applied as on the board; only a member of the harness, written as
      it; the team's manifest declares turns) and shows its events as they come: each step (thinking, a tool's call with its input,
      its result or failure), the text, the usage. A turn the person did not start (a
      member woken on the board, a channel's) is followed through `WatchActivity`: the
      session's messages as they are written, and a turn's start and end (owner 2026-10-26:
      `WatchActivityRequest.messages`, each appended message an `ACTIVITY_MESSAGE` event; a
      turn starts with its prompt and ends with its end block).
    - *Writing while it works*: a message sent during a turn waits in the app and goes in as
      the next turn when this one ends; it is never put into the running turn. Stop is
      `CancelTurn`: the steps that ran stay in the transcript, with a note that a person
      stopped it.
    - *Delete*: deleting a conversation deletes its session and its message stream (memory's
      session delete); the records its turns wrote stay in memory, so what the agent learned is
      not lost. Only a person deletes, from the app; a conversation with a running turn is
      cancelled first.
    - *Layers*: the chat is the app's (a view component over `pcx.agent.v1.Agent` and
      `pcx.v1`); no module renders it, and the agent keeps no list of its own: conversations
      are sessions.
  - *Crates*: `pcx-agent` (the loop: state, streaming, recovery; Axon's
    `conversation/{agent_loop,state,streaming,lifecycle}.rs` and `recovery.rs`, ported and
    split by responsibility), `pcx-context` (Axon's `compaction.rs` and recall),
    `pcx-openai-compat` (the OpenAI-compatible chat completions wire with SSE streaming, a
    library for every such provider; Axon's `openai_compat.rs`), `pcx-provider-zai` (Z.AI's
    coding endpoint, `enable_thinking`; Axon's `zai.rs`; the default model `glm-5.3`), and
    `pcx-models` (the providers' module). Built in (`[[builtins]]`), or module processes (W20:
    the binaries `pcx-agent`, `pcx-context`, `pcx-models`, as the cloud catalog runs them).
  - *The slots across processes* (W20, owner 2026-10-25): a module process of a slot serves its
    port on its own socket next to `pcx.module.v1.Module` (a model provider `Model`, a context
    engine `ContextEngine`, an agent `pcx.agent.v1.Agent`), and the host reaches it as an
    in-process module's (`ProcessModule::model_provider`, `context_engine`, `turns`; the
    models listed at launch); the host serves the same services on a module's callback socket
    through the module's handle (the models and context for their slots, the agent's turns for
    a module that uses it). `RunTurnRequest` carries the call's scope between a host and a
    module (a client's is not read: the daemon gives it a new one, its author the
    `x-pcx-author` header's, its workspace the `x-pcx-workspace` header's, the workspace the
    turn is asked from, as a module's RPC's; owner 2026-10-26), who asks (`asker`, W25: the
    prompt is written as them; empty, the scope's author; a client's is not read, its asker
    is its author) and what the turn adds to the agent's settings (instructions, model, modules, tools, the
    tools it is not given: `withheld`); a
    `failed` end and a retry carry the `ModelFailure` whole. One implementation of each
    service and client, in `pcx-module-api` (`ModelService`, `ContextService`, `TurnsService`
    over their sources; `RemoteModels`, `RemoteContext`, `RemoteTurns`), so the daemon, the
    host's callback socket and a module process serve and call them alike.
  - *Turns in a module's own name* (W22c, owner 2026-10-25): a module that is not the agent
    may run turns too (`Module::turns()`, the chief's), declared in its manifest
    (`Manifest.turns`), which the host checks (a module that declares them gives them, one
    that gives them declares them; a module process serves `pcx.agent.v1.Agent` on its socket
    when it declares them). `RunTurnRequest.agent` names whose turn a client asks: empty, the
    module in the `agent` slot (`Host::turns()`); a module's id, that module's
    (`Host::turns_of`), `NotFound` when it runs none. Between a host and a module: empty, a
    module asks the agent (`ModuleHost::turns()`) or an agent or module process serves its own;
    a module's id, a module asks the turns that module runs in its own name
    (`ModuleHost::turns_of`, 2026-11-11: a workflow's `agent` block asks a member's turn of the
    team), `Denied` unless its `uses` names that module, `NotFound` when it runs none. Such a
    turn runs as that module runs it, under that module's reach and grant: the asking module
    gives it no token and none of its own reach. A process serving its own turns refuses a
    request that names another (`InvalidArgument`).
  - *A module's turns* (W22a, owner 2026-10-25): a turn a module asks of the agent
    (`ModuleHost::turns()`) runs under that module's reach and grant, which the host enforces,
    so no module gives its turns more than it was given. The host runs it under a token of its
    own (`Scope.turn`, set by the host only, unguessable, known while the turn runs, carried
    across processes in the wire scope); a call of the agent in a scope with a token is
    checked against the module whose turn it is, instead of the agent alone: the callee must be
    a module both the agent and that module use, a tool only a person calls passes only when
    that module's grant names it, and a tool its grant withholds is refused; an unknown token
    is refused. The tools the agent lists (`ModuleHost::tools(scope)`, `ListToolsRequest.scope`
    on the callback socket) are the ones its calls in that scope may make. The grant is
    `TurnGrant` of `pcx-module-api`, read from the module's settings (`person_tools` and
    `withheld`, lists of tool names; none: empty) when the host loads it, a malformed one
    refused as an invalid configuration; it applies to every module that asks turns (the
    chief, the team). A turn without a token (a client's, through `Host::turns()`) is the
    agent's own, as before.
  - *Layers*: the loop's rules (what follows a model answer, a tool result, a stop, a
    failure) are pure; the agent service reaches models, tools, events, context and the
    transcript through ports its adapters fill over the host handle; `RunTurn` only maps.
  - *Not in W18*: steering (a person writing into a running turn; owner 2026-10-25: with the
    TUI, its first client), subagents and orchestration, failover between providers, the
    budget, other providers, the TUI, the cloud (step 6 of the private harness plan and later).
  - *Tests*: a scripted model provider for the loop end to end (a tool turn, a refused tool, a
    failure and its recovery, compaction, resume); a real Z.AI turn only when asked
    (the test reads the key file named by an environment variable).

The memory module fills the `memory` slot: it answers session start (cards), pre-tool (file
memory), turn end (capture), memory-write events, and exposes its tools; the standalone daemon is
the host with only this module and its transports.

## A turn that goes in circles (owner 2026-11-02)

A turn stops as `repeating`, its last failure in its detail, at the third same failure of a tool in a row or the twelfth in all: a model reads each refusal, but only a count ends what it does not see itself doing. Its stream is taken as lost after 600 s without data, and the usual recovery follows.
