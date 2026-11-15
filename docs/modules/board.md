# board

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8.

- **`board`** (W16): boards, tickets, their discussion and decisions.
  - Documents: `board/board` (name, columns), `board/task` (title, text, board, column,
    assignee, labels, priority, links to git: branch, commits, pull request; W24: its plan,
    the tasks it waits on, its rank in its column, its images), `board/comment` (its task, the
    comment it answers, the text; W24: its images), `board/decision` (its task, what was decided
    and why, who decided, the risk: `none`, `code`, `money` or `data`, and who approved it).
  - Moving, assigning or editing a task writes its next version; two agents moving one task at
    once get a conflict, not a fork. A comment or a decision is a new document; a decision
    also writes a `decision_made` record to memory, so it joins the graph and the knowledge an
    agent recalls.
  - Tools: `board_create`, `board_list`, `task_add`, `task_update`, `task_move`, `task_list`,
    `comment`, `decide`, and `approve_decision` (visibility `app` only: a person approves).
    A decision whose risk is not `none` holds its task until it is approved: at once when the
    team's autonomy policy allows it (`approved_by` names the policy and its version), else by a
    person; without a team, by a person.
  - Uses: `post-cortex.memory` (its documents and decision records) and `tools-git` (W15; a
    branch's commits for a task's links).
  - View: the board (its columns as lists, a task's thread and decisions as its detail).
  - Events (owner 2026-10-31; `docs/host.md`, events of a module): `board.task_created` (a
    new task, not one a key finds again), `board.task_moved` (a task whose column changed),
    `board.decision_made` (with whether it waits for an approval),
    `board.decision_approved` (a decision approved by a person, or by the team's policy when it
    is made; owner 2026-10-31, for a workflow's approval, `workflow.md`) and
    `board.decision_declined` (a person declined it, below), each published in the
    call's workspace and by its author after the write, through the `Events` port.
  - Any agent reaches it through its tools (Claude Code, Codex, the harness's own) and a
    person through the app, before the team module exists.
  - *Where it runs* (owner 2026-10-25): one crate, `pcx-board`. The daemon runs it in process
    as a built-in (`[[builtins]] id = "board"`; its manifest uses `post-cortex.memory` and
    `tools-git`, which a built-in, the daemon's own code, needs no grant for); its `pcx-board`
    binary serves it as a module process for the cloud catalog, whose entry grants those uses.
  - *Its workspace and session*: a call's workspace is `Scope.workspace_id`, else its
    `workspace_id` argument. `board_create` also creates the board's session in that workspace
    (through memory's tools) and keeps its id in `board/board`; every decision's
    `decision_made` record goes there, whoever decides (an agent, or a person in the app, who
    has no session of their own).
  - *A short id* (owner 2026-10-29): a call names a task by its id or by its short id, the
    first 8 characters of its id or more, as the app shows it on a card and in a list (a
    click copies it): `task_update`, `task_move`, `task_get`, `comment` and `decide` among
    the workspace's tasks; `parent`, `depends_on` and `before` among the board's. The task
    whose id is the name is that task; else the one task whose id the short id begins. A short
    id that begins more than one is refused (`InvalidArgument`, naming them); what is kept is
    always the full id. The domain decides it (`domain::task_id`); the service reads the
    tasks.
  - *Git*: a board names its repository (`repo`, a path under `tools-git`'s roots); a task's
    branch link gives its commits through `tools-git`'s `git_branch_commits`, kept when it is
    linked and read again for the task's thread (`GetTask`, owner 2026-11-08: a member's
    commits after the link showed nowhere). When they cannot be read then (the runner offline,
    the branch gone), the thread has the kept ones and says why (`Thread.commits_unread`).
  - *Changing a board* (P3, owner 2026-10-28): `board_update` (`UpdateBoard`, visibility `app`:
    only a person changes a board, so an agent never points its own work at another
    repository) renames it or changes its repository (empty: none), from the version the
    caller read (`Conflict` otherwise); its columns stay. A task already in a worktree keeps
    it; the new repository is the next task's. In the app, the board's settings choose the
    repository among the directories `tools-git` may use: on a daemon its roots
    (`ListModules`), in a workspace a runner serves the runner's (`docs/connect.md` section
    7), or a path typed in.
  - *A held task*: while a decision with a risk other than `none` waits for approval, its task
    is not moved or closed (`task_move` answers `FailedPrecondition` naming the decision);
    comments and edits of its text go on, so the decision can be discussed.
  - *Layers*: the board's rules (columns, holds, risks, who may approve) are pure domain code;
    the board service reaches documents and records through a port that its adapter fills with
    memory's tools over the host handle, and commits through a port over `tools-git`; the tool
    handlers only map JSON.
  - *The view*: a list of boards, with a button to create one; a board's detail is its lanes
    (its tasks by column) and a button to add a task; a task's detail is its text, its comments
    and its decisions with their state, buttons to edit, move and decide on it (each a
    dialog), a box to comment and, for a person, an action to approve.
  - *Plans, dependencies and images* (W24; owner 2026-10-26, the design: the canvas's
    `Board` and `BoardTask`):
    - *A plan* is a task that others name as their `parent` (a task id of the same board):
      one kind, so a plan has a column, decisions and comments like any task. A plan has no
      parent of its own (one level), and a task is not its own parent. Its progress is its
      tasks in a `done` column over its tasks not `canceled` (W26), worked out by the service.
    - *Dependencies*: `depends_on`, the tasks of the same board a task waits on. A link that
      names the task itself, another board's task, or makes a cycle is refused
      (`Broken::Cycle` names the chain). Waiting does not hold a task (only a pending decision
      does); it shows on the card and the page, and a task is ready when every task it waits
      on is `done` or `canceled` (W26). What waits on a task is not stored: the service answers it
      from the board's tasks.
    - *Order in a column*: `rank`, a string that sorts the column (a fractional index between
      its neighbours, so a move writes only the task moved: base-62 digits read as a
      fraction, never ending in the lowest digit). `task_move` takes the column, the task it
      goes before (none: the end) and, for a drop in another plan's lane, the new parent
      (empty: no plan). A task written before W24 has no rank and sorts after the ranked ones;
      a place next to it is after the ranked ones. A task is added at the end of its column.
    - *Images*: a task and a comment carry `attachments` (`pcx.v1.Attachment`, in
      `pcx_blob.proto`, which a module's package imports: a blob's SHA-256, its file name,
      media type, size, width and height). The bytes are blobs of memory (section 4, Blobs),
      never in a document, so a new version of a task does not copy its images.
      Only images (PNG, JPEG, WebP, GIF, known by their first bytes, not their name), at most
      10 MB each; an attachment that names a blob the workspace does not have is refused.
    - *Tools*: `task_add` and `task_update` take `parent`, `depends_on` and `attachments`;
      `comment` takes `attachments`; every answer of a task (`TaskRecord`) carries what waits
      on it, whether it is ready and, for a plan, its progress, and how many comments it has
      and the pending decision that holds it (`comments`, `held_by`), worked out from the
      board's documents, not stored; `task_list` answers a board's tasks in its columns'
      order, each column by rank.
    - *Approvals* (the canvas's `Approvals`): `decision_list` (`ListDecisions`, visibility
      `app`) answers the workspace's decisions of every board, waiting for a person or
      approved (by a person or the policy), newest first, each with its task and why the
      policy did not approve it; the app's approvals view and the bell show the waiting ones,
      approve (`approve_decision`), decline (`decline_decision`) and revoke
      (`revoke_approval`). *A person declines* a decision that waits (owner 2026-11-11: an
      ops action declined in Telegram kept waiting and held its target; Decline was only a
      comment): `DeclineDecision`, a person's only as approving is, from the version read and
      with an optional note (`Decision.declined_by`, `decline_note`). A declined decision
      stays in its task's thread, holds its task no more, cannot be approved and is not
      listed; the board emits `board.decision_declined` (`DecisionDeclined`: the decision,
      its task, who declined), and who asked it gives up what it asked for (`ops.md`,
      `workflow.md`, `dynamic.md`): to ask again is a new decision. Its author withdraws a
      decision that waits (`withdraw_decision`, with why, owner 2026-11-10: a workflow's run
      that ended): it stays in its task's thread as withdrawn (`Decision.withdrawn`), holds
      its task no more, cannot be approved and is not listed. Why it waits is worked out
      when the decision is made and kept with it (`Decision.held_because`, owner 2026-10-26):
      the team's policy answers a typed refusal (`pcx_team::Refusal`: paused, supervised, a
      class it does not trust, a protected or untrusted branch, over a money limit, data that
      cannot be undone, ...), or the author is in no team. An approval may carry a note for the
      record (`DecisionRequest.note`, kept as `Decision.approval_note` and recorded in the
      board's session as a `decision_made` record "Approved: <what>").
    - *Dialogs* (the canvas's `BoardDialogs`): a new task (its title, text, plan, assignee,
      what it waits on, priority, labels, images), approving a decision (what it holds and
      what the policy says, a note for the record) and a new board (its columns in order,
      its repository, checked against `tools-git`'s roots).
    - *The view*: the board's plans with their progress (a filter), a filter by assignee,
      and its tasks in lanes by column, one lane or one per plan. A card shows its plan (by
      the plan's color and name), id, title, labels, the decision that holds it, priority,
      what it waits on, how many wait on it, its pull request or branch, its images (the
      first as its cover) and comments, and its assignee; pointing at a card marks what it
      waits on and what waits on it. *Who works now* (owner 2026-10-30): a card whose task a
      member's turn works on now says so (`<member> working`, its dot blinking slowly) with what the turn does now in
      words (its last call's tool and what the call names: a command without its `cd`, a
      path within its worktree, a query; `thinking`; `writing the answer`; never the call's
      JSON), and the board's header names who works now
      and on what; the app reads the members' work (`work_list`, team.md) and the running
      turns' steps every 10 seconds while the board is open, and reads the board again when a turn starts
      or ends (a turn moves, comments and labels its task). Without the team module, nothing
      shows. A card is dragged to a column, a place in it, and in
      the lanes by plan another plan (`task_move`; a held task stays and says why). A
      task's detail: its text, images (drop, paste or choose), dependencies both ways, its
      plan with the plan's tasks, its discussion (comments with images, decisions), its
      details, git and activity. The view's lanes gain drag and drop, a card component and
      grouping in `pcx.view.v1`, for every module's lanes (owner 2026-10-26): `Lanes.card`
      (`Card`: the row's id, labels, priority, alert (what holds it), cover (an array of
      `pcx.v1.Attachment`, the first its cover), link, person, count, and what it waits on
      and what waits on it by id, marked when a card is pointed at), `group_field` and
      `groups` (a source of `{id, title, done, total}`: one lane, or one per group with its
      progress and its color by its order; a group is a filter), `filter_field` (a person
      filter) and `move` (an action on a drop, with `{{drop.lane}}`, `{{drop.before}}` and
      `{{drop.group}}`; a refusal is shown and the card stays). A new component `Images`
      (`source`, `attach`): the app puts a dropped, pasted or chosen image through
      `POST /v1/blobs`, then runs `attach` with `{{images}}` (the images it had and the new
      ones), and shows each through a blob link.
  - *Stages and canceling* (W26; owner 2026-10-27): where a task is in the work and how it
    ended are apart, as in Linear, Jira and GitHub. Each column has a stage
    (`pcx.board.v1.Stage`): `open` (work to do or under way), `done` (finished) or
    `canceled` (given up). `Board.stages` holds them in the columns' order; a board
    written before W26 has none and reads as before: its last column `done`, the others
    `open`. `board_create` takes the stages (each column one, none unspecified); without
    them the last column is `done` and the others `open`. The app's new board starts with
    To do, In progress (`open`), Done (`done`) and Canceled (`canceled`).
    - *What a stage decides*, instead of "the board's last column": moving a task into a
      `done` or a `canceled` column closes it and needs authority `close` (a held task does
      not move there, as anywhere); a task is ready when every task it waits on is `done` or
      `canceled` (a canceled task is waited on no longer); a plan's progress is its `done`
      tasks of its tasks not `canceled`.
    - *Canceling* is a move into a `canceled` column, and moving it back reopens it: the
      task, its comments and decisions are kept. A move may carry a note
      (`MoveTaskRequest.note`, `task_move`'s `note`): why, written as the mover's comment on
      the task. Nothing is deleted.
    - *The view*: a `canceled` column is folded to its name and count (open it to see its
      tasks; a drop on it cancels); the task's page has an action to cancel it (the board's
      first `canceled` column, with the reason as the note) and, for a canceled task, to
      reopen it (the first `open` column). The new board's dialog sets each column's stage.
