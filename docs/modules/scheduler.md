# scheduler

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8, operations.

- **`scheduler`** (W28): work at set times. Crate `pcx-scheduler`, built in; uses `board`,
  `post-cortex.memory` and `workflow`.
  - *Its workspaces* (owner 2026-11-10): a job belongs to a workspace, the one the call that
    adds it is asked from (asked from none: the one the `workspace_id` setting names); its runs
    are kept there, a job's name is unique there, and every tool reads and changes only that
    workspace's jobs (docs/connect.md section 4a, a module's data). The scheduler looks for the
    jobs due in every workspace; one whose jobs cannot be read is logged and does not hold the
    others. A job's workflow is the workflow of that name in the job's workspace.
  - *A job* (document `scheduler/job`, `pcx.scheduler.v1.Job`): its name (unique, 1 to 64 of
    a-z, 0-9, '-'), when (a cron expression of five fields with its IANA time zone, or every N
    seconds, at least 60), whether it is paused, and what it does, one of: a *script* (a
    program and its arguments, run through the `Sandbox` port under the scheduler's grants
    (`spawn`, `network` when it needs it) with a timeout, its exit code and the tail of its
    output kept), a *task* (added to a board as `task_add` would, its key
    `scheduler:<job>:<due time>` so a run adds it once), or a *workflow* (`WorkflowAction`:
    the workflow's name and its input, run by `RunWorkflow`, which reads the call as its
    `schedule` trigger; the run's result is the workflow run's id; docs/modules/workflow.md,
    triggers and approval). A member's work at a set time is a
    task assigned to it: the team wakes it as for any task, and its answer is its comment
    (the daily report), so the scheduler has no path of its own to the agent. Tools:
    `job_add`, `job_update` (from the version read), `job_list` (each job with its last run
    and when it is due next), `job_remove`, `job_run` (now, once), `run_list` (a job's runs,
    newest first).
  - *Runs* (document `scheduler/run`: the job, when it was due, when it started and ended,
    its outcome (`ok`, `failed`, `skipped`), its result): a job runs once at a time (a run
    due while one runs is recorded as skipped); after the host was down, a job whose time
    passed runs once, for its latest time, not once per missed time. Time comes from a
    `Clock` port, so the domain (when is it due) is pure and tested without waiting. A
    job's runs only grow (288 a day for one every 5 minutes), so they are read the newest
    first and only as far as a reader needs (docs/host.md, pages in a module): to its last
    scheduled run for when it is due, to the time it owes for its health, to the limit for a
    listing. The
    scheduler looks for due jobs every 15 seconds, the first look one interval after the
    module starts: the host makes its running set callable once every module has started,
    so a look at once would find no module serving the jobs (owner 2026-10-28).
  - *In the app* (owner 2026-11-08): the scheduler's page
    (`Scheduler` under the modules, shown when the module is on), a client of
    `pcx.scheduler.v1.Scheduler` with no logic of its own, like the skills' page:
    - the jobs (`ListJobs`): each one's name, when (the cron expression and its time zone,
      or every N seconds), what it does (the workflow, the board's task, or the program),
      paused or not, when it is due next and its last run (outcome, when, its result: a
      workflow run opens that run's page, a task opens the task);
    - a job's runs (`ListRuns`, the newest first);
    - adding a job and changing one (`AddJob`, `UpdateJob` from the version read; a
      conflict shows the job as it is now), pausing and going on (`UpdateJob` of `paused`),
      running it now (`RunJob`) and removing it (`RemoveJob`); the server checks a job, and
      the page shows its refusal.
    - In the workflow editor, a `schedule` trigger shows the jobs that name its workflow
      (their when, next time and paused) and opens one on the scheduler's page, or adds a
      job for this workflow there (docs/modules/workflow.md, the editor).
  - *Alive*: a dead man's switch is a job like any other (a script that pings an outside
    watcher, e.g. a blackbox target or a healthchecks URL, every few minutes), so a stopped
    scheduler and a machine that is down both show outside; Hermes needed a watchdog file
    for this.
  - *A job watched from outside* (owner 2026-11-11: Equal's cluster probed Hermes's
    `/healthz`, which failed when its probe had not run for 15 minutes): the daemon serves
    `GET /health/jobs/{name}` on its HTTP port, not behind its token (as `/health`, and
    behind the host allowlist), which answers `JobHealth` of the scheduler: 200 while the
    job of that name runs on time, 503 with why when it is paused or late, 404 for no job of
    the name (or no scheduler). A job is *late* when a time of its schedule passed more than
    15 minutes ago and no run of it since that time succeeded or still runs: a run that
    failed or was skipped is no run of that time, so one failed run among good ones is not
    late and three in a row of a job every 5 minutes are. For a workflow's job a run that
    succeeded started the workflow's run; what that run finds or how it fails is the
    workflow's to tell. The scheduler looks for the name in every workspace, in its own
    name: the answer says whether a job of that name runs and nothing else of it. `job_health`
    is the tool.
  - *Telling a channel* (owner 2026-11-11) is a workflow's, not the scheduler's: a job that
    should tell people runs a workflow, whose `tool` block calls the channel's
    `channel_send` (docs/modules/telegram.md) with what it found, when it chooses. A job
    names no destination (`Job.deliver` and `Run.told` are gone, their numbers reserved),
    and the scheduler does not use the channel module.
