# team

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8.

- **`team`** (W19, on the agent of W18): agents with roles, and teams.
  - Documents: `team/agent` (name, role, the skills it carries, its model, the modules and
    tools it may use, its authority: `discuss`, `decide`, `close`, the workspaces it reaches:
    `reach`, none: every one its waker reaches; owner 2026-11-13, docs/connect.md, a member's
    reach), `team/team` (name, the
    workspace it works on, its members). A member is an agent of the harness or an external
    one known by its author name (Claude Code, Codex, a person), which takes part through the
    board's tools; `agent_update` changes which (`harness`), and the team reads it at each
    wake, so a member made a person gets no turn after.
  - It follows the board's documents (memory writes of `board/task` and `board/comment`): an
    assigned or mentioned agent of the harness gets a turn (through the agent module) with its
    role, its skills and the task's thread as context, and answers in the thread.
  - Authority: only a member whose role has `decide` records decisions and only `close` closes a
    task; the board checks the author of the call against the team. An owner or admin of the
    organisation (`Scope.role`, docs/host.md) always may decide and close, a member of a team or
    not (owner 2026-10-29). A module asks a decision in its own name: an author
    `<module>:<name>` on a call the host says comes from that module (`Scope.caller_module`)
    records a decision, which waits for a person or the team's policy as any; it closes
    nothing (owner 2026-11-01: the workflow module's approvals and grants).
  - Autonomy (owner 2026-10-24): `team/autonomy` is the trust a person gives the team, per role
    or agent where they differ: `supervised` (every decision with a risk waits for a person,
    the default), by class (the classes it lists approve themselves), or full within limits.
    Each class has its limits: `code` (the branches it may change; never the protected ones),
    `money` (a cap per decision and per day, counted from the team's approved decisions),
    `data` (reversible changes only, or deletions too). The board approves a decision inside
    the policy at once and holds any other for a person. Only a person changes the policy and
    pauses it (tools with visibility `app` only), so no agent raises its own trust; a pause
    holds every new decision with a risk; every approval by the policy shows in the decision's
    history and a person may revoke it.
  - Uses: `board`, `skills`, `post-cortex.memory`, the agent module, and the tool modules its
    members' turns reach (`tools-fs`, `tools-shell`, `tools-git`, `dynamic`; W22a: a module's turns
    reach only the modules it uses).
  - *A member's turn* (owner 2026-10-25): `Turns::run` takes what a turn adds to the agent's
    settings (the member's role and its skills' instructions, its model, the tools it may use,
    its name as the author; not the board's `comment`, as its answer is its comment, owner
    2026-10-25; told what its authority leaves to others and the id of the task's workspace,
    which its board and team tools work in already; to mention a member only for it to act, to
    answer in the language of the task or the comment it answers, and to cite a record of
    memory as a link by its title, `[title](pcx:record/<entry_id>)`, which the app opens), and the team reaches the agent's turns through its handle
    (`ModuleHost::turns()`, for a module whose `uses` names the agent; a module process
    reaches them through `pcx.agent.v1.Agent` on its callback socket, W20). Each member works on a
    task in a session of its own in the team's workspace (`team/work`: the member, the task,
    the session and the turns it took there, counted as each ends), so two agents' turns never
    share a chain; the task's thread (its text,
    comments and decisions) goes into the turn's prompt.
  - *A person's talk with a member* (W25; owner 2026-11-07): `RunTurn` of `team` naming a
    member runs a turn of the conversation's session with the member's role, skills and tools,
    as on the board, and its instructions name the workspace it runs in, as a task's prompt
    does: a search of memory that names none is refused a turn held to some workspaces
    (docs/connect.md 4a). The person who asks writes the prompt (`asker`); everything the turn does
    is the member's: its calls run with the member as the scope's author and with no person's
    role, so the board checks the member's authority, never the person's, and what it writes
    (a comment, a decision, a commit) names the member.
  - *When a member works*: on a memory write of `board/task` or `board/comment` that assigns
    the task to a member that is an agent of the harness, mentions it (`@name`) or, a person's
    comment, answers its comment (owner 2026-10-25: a member's answer does not wake the one it
    answers, so two members do not keep waking each other); a change the member made itself does
    not wake it; at most one turn runs and one
    waits per task and member. The event is answered at once; the turn runs in the
    background and stops with the module. The turn's last text becomes a comment by the
    member; what it does on the board goes through the board's tools in its name.
  - *Authority, checked by the board*: the board reads its workspace's `team/team` and
    `team/agent` documents through `manage_document` (the board does not use the team module,
    so no module uses the other in a cycle). With a team, `decide` needs authority `decide`,
    moving a task into a `done` or `canceled` column (W26) needs `close`, and an author who is not a
    member is refused; without a team the board works as alone.
  - *Autonomy, applied by the board*: `board/decision` gains optional `amount` and `currency`
    (`money`), `branch` (`code`) and `reversible` (`data`); a day's money is counted from the
    team's decisions approved that day. A decision inside the policy is approved at once with
    `approved_by` `policy:<version>`; any other waits for a person. Person-only tools:
    `autonomy_set` and `autonomy_pause` of the team, `revoke_approval` of the board (the task
    is held again).
  - *What the members are doing* (W25, owner 2026-10-26): `ListWork` (`work_list`) answers
    each member's work, the most recent first, with the task's title and column from the
    board (empty: the task is gone), its session, its turns and when it last took one, and
    whether a turn of it runs now (from the module's running work; a team's members only when
    a team is named). The app's Teams screen shows it as the members' work now.
  - *Stopping a turn* (owner 2026-11-02): `StopWork` (`work_stop`, a person's) cancels the
    member's running turn on the task and the wake that waits after it, spends no try and says
    so on the task; a new comment naming the member wakes it. The members' work has a Stop action.
  - *Tools*: `team_list`, `work_list` and `autonomy_get` for anyone; `team_create`, `agent_add`,
    `agent_update`, `member_add`, `member_remove`, `work_stop` and the two autonomy tools for a person only
    (visibility `app`): who is a member, and with what authority, is a person's to say, so no
    agent raises its own.
  - *The view* (owner 2026-10-25): buttons to create a team and add a member; the
    workspace's teams with their trust, to pause or go on; a team's detail: buttons to add or
    remove a member and change its trust (a dialog from the trust it has), its members (each
    with an edit dialog of its role, authority, skills and model) and its trust. The dialog
    offers the skills of the library (`skill_list`) it does not carry yet and the models of
    the agent (`pcx.agent.v1.Agent/Models`), each in a dropdown that filters as one types
    (the app's `PcCombo`), and still takes a name typed in; a name typed and not yet added is
    added when the dialog saves (owner 2026-10-30).
  - *A new member's name* (owner 2026-10-29): the name is the author the member writes as, so
    the dialog offers the names the space knows that are not members yet, and still takes a
    name typed in (an agent the team runs itself). What the space knows is its extension's to
    say (the app's `AppExtension.authors`): in the cloud, the organisation's people with access
    to the workspace (their author names) and the agents of its API keys that reach it (a
    key's agent, post-cortex-cloud ARCHITECTURE C16), whether they wrote yet or not; a daemon
    without an account knows none. A name picked from them is known by its name (not run by
    the team).
  - *Built-in agents* (owner 2026-10-31): the module ships agents of its own, each a template
    (its name, role, skills, authority and the modules it may use); the first are
    `workflow-builder` (docs/modules/workflow.md, the builder) and `module-builder`
    (docs/modules/dynamic.md, the builder). `community` (docs/modules/discord.md, the agent
    of a community) answers strangers: its modules are memory only, its tools memory's that
    only read, by the names an agent's turn has them (`memory_search`, `memory_context`),
    and its reach the workspace it is added in, so a template
    names its tools and its reach too. A member held to modules without the board is told
    nothing of the board or its threads (its authority, mentions, links to records): what it
    says leaves the app. `team_list` answers the built-ins a workspace has
    no agent of yet (`RosterList.builtin`); `member_add` of a built-in's name in a workspace
    without that agent, and `team_create` with it among its members (found 2026-11-14: the
    team then named a member that was no agent, and no turn was asked of it), adds the agent
    from its template first, so it is a person who brings it in. Once added it is an agent like any other: a person may change it, and the template does
    not touch it again.

- **Work that ends** (W32): a member's work on a task has a deadline and tries, and a task of
  a board with a repository gets a worktree of its own.
  - *Deadline and tries*: the team's settings (`pcx.team.v1.TeamConfig`) give a turn's
    deadline (30 minutes) and the tries (2). A turn that ran past its deadline, whose model
    failed past its recovery for a while (rate limited, overloaded, unreachable), or whose
    call failed for a while is tried again after a pause (a minute, or longer when the
    provider names when to retry; owner 2026-10-29); one a module blocked, a call the rules
    refuse, or a model whose quota is used up, whose credentials were refused, or that
    refused the request as it is (too long, invalid) is not: its task is blocked at once with
    why (another try would end the same). Work that
    failed its tries labels its task `blocked` and comments why, as the team; a person looks
    and asks again in a comment naming the member, and the label comes off when that turn
    starts. The label and the comment are on the board, which
    the channel's people see; the team module (a module process in the cloud) uses no channel.
  - *A worktree per task*: the first turn of a member on a task whose board names a
    repository makes a worktree of it, `<repo>/.worktrees/<task's short id>`, on the branch
    `task/<short id>-<slug of its title>` (`tools-git`'s `git_worktree_add`: made, or the one
    there; a path outside the repository is refused). A new branch starts from the remote's
    copy of the branch the repository has checked out, fetched first when `tools-git` has a
    remote (`push`), so a task never starts on a clone gone stale (owner 2026-11-08: the prod
    runner's clones stood a week behind); a fetch that fails makes no worktree and says why.
    The task links the branch (its commits
    show on it) and the work keeps both (`team/work`: `worktree`, `branch`). Every turn of the
    member on the task, those a person's answer wakes (request changes) too, is told to work
    and commit there only.
  - *How far a turn has come* (owner 2026-11-02): while a turn runs, `ListWork` answers the seconds since it last received or did anything and what its model sent so far, so a person tells a slow model from a stuck one. A model that writes one long tool call sends nothing until it ends.
  - *Waiting for a runner* (docs/connect.md section 7): a call that fails `Offline` (a
    runner's module whose runner is not connected) spends no try: the task is labeled
    `waiting-runner`, the team comments once, and the work waits while the module runs;
    `EVENT_KIND_MODULE_AVAILABLE` takes the label off and runs it again.
  - *Work across a restart* (owner 2026-11-04): a turn that runs or waits for its runner is kept
    as going on its `team/work`; when the module starts, that work goes on in its session and no
    person wakes it, no try spent. A tool call the stop cut has an error for its result, which says
    it may or may not have finished. A person who stops a turn, an answer, a block or a closed task
    ends it.
  - *A turn without an answer* (owner 2026-10-28): a turn that took the most steps it may
    (`step_limit`), or ended with no text, leaves a comment by the member: how it ended, its
    steps, its last few steps (a tool's call, and `✗` when it failed) and the session that
    keeps every step. A turn cut short with its work unfinished, one that took its most steps
    or whose model reached its most tokens (`max_tokens`) without text after the agent went on
    twice (a cut answer is kept, a tool call cut in it is dropped unrun, and the model is told to
    go on in smaller steps, at most twice in a turn; 2026-11-02), spends no try and
    stops: its task is labeled `waiting-person` (by the team) and waits for a person, whose
    answer to that comment wakes the member to go on in the same session; the label comes off
    when the member's next turn on the task starts.
  - *Every step on the task* (owner 2026-10-28): the app's task page shows each member's work
    on the task (`work_list`): the member, its turns, whether one runs now, and its session's
    steps one per line as the Conversations screen draws them, refreshed while a turn runs,
    with a link to the whole conversation.
