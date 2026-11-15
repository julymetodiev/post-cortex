# webhooks

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8, operations.

- **`webhooks`** (W27): events from outside become board tasks, or runs of a workflow. Crate
  `pcx-webhooks`, built in (`[[builtins]] id = "webhooks"`); uses `board`,
  `post-cortex.memory`, `telegram` and `workflow`.
  - *Receiving*: the daemon serves `POST /v1/webhooks/{source}` on its HTTP port and hands
    the request whole to `pcx.webhooks.v1.Webhooks/Receive` (`ReceiveRequest`: the source's
    name, the bearer token it came with, the body as bytes): the route only forwards (rule
    3). The module's `token` setting (a secret) is the bearer token every source sends; a
    token per source waits for secrets inside a list of settings (section 7, settings), when a
    second kind of source comes. A source is a setting of the module (`Source`: its name, its
    kind, the workspace and board its tasks go to, the member they are assigned to, the
    channel destination told at once (`notify`, W29)), or instead of a board the workflow its
    requests run (`workflow`; one of the two, owner 2026-10-31). A request without the token is
    refused (`Unauthenticated`, compared in constant time), an unknown source `NotFound`, a body its
    kind does not decode `InvalidArgument`.
  - *Kinds*: Alertmanager first (`AlertmanagerWebhook`, version 4: `groupKey`, `status`,
    `groupLabels`, `commonLabels`, `commonAnnotations`, `externalURL`, its `alerts` with
    `labels`, `annotations`, `startsAt`, `endsAt`, `fingerprint`, `generatorURL`). Others
    (Forgejo, GitHub, Sentry) are kinds added the same way: a mirror message and the
    domain's rule for what becomes a task. `SOURCE_KIND_JSON` takes any JSON body and is
    routed only to a workflow.
  - *To a workflow* (docs/modules/workflow.md, triggers and approval): a source that names a
    workflow hands it the body as sent (`RunWorkflow`, its `source` and `body`), which reads
    the call as the workflow's `webhook` trigger of that source; no task is added, and the
    answer is `HANDLED_RUN` with the run's id.
  - *A notification with no alerts* (`alerts` empty; owner 2026-10-31, after a test of the path
    added a task and told the channel) does nothing: no task, comment, channel or record
    (`HANDLED_NO_ALERTS`), so a probe or an odd sender costs no agent turn.
  - *A heartbeat* (owner 2026-11-11): a source names the alerts it only acknowledges
    (`Source.heartbeats`, by alert name: Alertmanager's `Watchdog`); a group of nothing but
    them is answered `HANDLED_HEARTBEAT` after the token is checked and adds no task,
    comment, channel or record. Sent every few minutes it proves the path from the sender
    to the module while nothing fires: the sender counts the notifications that fail, so a
    broken path shows in minutes, not with the first real alert (Equal: Hermes read the
    heartbeat's age; here Alertmanager's own failures say it).
  - *One task per incident of an alert group*: Alertmanager keeps a group's key across its
    incidents, so each incident (from the group firing with no open task until it resolves)
    has its own task key, `<source>:<hash of the group key>:<n>` (`AddTaskRequest.key`,
    below); a group that fires again finds its open task. A firing group with none
    adds the task (title from the alert's name and its targets, text from its annotations and
    labels, labels `alert`, its severity and the source), assigned to the source's member,
    and tells the source's destination at once, with no model in the way. A group firing
    again on an open task comments what changed. A resolved group comments its duration;
    a task its member labelled `no-action` (the diagnosis found nothing to do) that is not
    held moves to the board's first `done` column, moved as that member (owner 2026-10-28: the
    source is no member of a team, and only a member closes; the move carries out the member's
    own finding); any other stays for its work. A move the board refuses leaves the task open,
    logged, and the request still succeeds: the resolution is commented and recorded either way. When a group
    resolves, the incident is recorded in memory (`problem_solved` in the board's session:
    the alert, the targets, how long, the task's diagnosis from its decisions and the
    comments of others than the source), so the next alert like it brings it back through
    recall.
  - *Board*: `AddTaskRequest.key` (optional, at most 200 characters): a key the board keeps
    on the task (`Task.key`); adding a task with a key a task of the board already has
    answers that task unchanged (`TaskRecord.existing` true). Keys are unique per board; a
    sender serializes its own adds of one key (the webhooks handle one request at a time).
  - *The route*: `Webhooks/Receive` is declared with visibility `app`, so it is no tool of
    an agent; the daemon's route calls it as a client of the host.
  - *Through the cloud*: a connected daemon's sources may post to the gateway instead,
    `POST /v1/daemons/<daemon id>/webhooks/{source}`, which forwards the body and the token
    over the tunnel to this route (`docs/connect.md` section 3); the module sees no
    difference.
