# ops

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8, operations.

- **`ops`** (W31): changing a system, only through typed actions, each decided on the board.
  Crate `pcx-ops`, built in (its `network` grant, and `filesystem` on its credentials'
  directory); uses `board`, `post-cortex.memory` and the channel; its settings: its
  workspace (where its actions are kept), the environments (each a cluster's API and the
  files of an account whose rights are the actions', apart from observe's; the most
  replicas a scale sets, 3 by default; the TLS server name, as observe's), the brakes and the
  channel destination it tells.
  Kubernetes over HTTP with credentials from files is `pcx-kube`, which observe uses too;
  what is Nomad's own (its names, paths, settings and JSON) is `pcx-nomad`, observe's too.
  - *Actions* (`pcx.ops.v1.Ops`, each an agent's tool): `ops_restart` (a deployment, as
    `kubectl rollout restart`), `ops_delete_pod` (only one a controller makes again, never a
    database's: CloudNativePG's label), `ops_clean_jobs` (a namespace's jobs that failed and
    never succeeded), `ops_scale` (1 to the environment's most) and `ops_resize_volume`
    (grow only, at most twice, never a database's claim: that grows through its cluster, a
    person's; a claim's size is its volume's capacity when that is more than it asked for,
    as a provider's smallest volume may be: Equal 2026-11-11, 2Gi asked and 10Gi had, so
    twice the claim grew nothing); `ops_actions` lists them. Every request names its environment, its task's
    workspace and task and why (the evidence), and may be a dry run, which says what it
    would do and how the target is now and decides nothing. A rollback (a deployment's
    previous template, which is free-form JSON) waits for a decision on how to type it.
  - *A Nomad environment* (owner 2026-11-12, for pcx's own production): an environment
    names a Kubernetes API or a Nomad API, never both (`nomad_api`, `nomad_token_file`,
    `nomad_ca_file`, `nomad_tls_server_name`; `kube_api` is required no more, and
    settings with both or neither are refused, with the field). Its actions:
    `ops_new_allocation` (a running allocation of a `service` job is stopped and Nomad
    places a new one; never a restart in place, which stopped a pot task's logs and froze
    it on 2026-11-02; the allocation is named by its id or the first 8 characters or more
    of it, of which exactly one must start so; refused for an allocation that does not
    run or whose job is not a service), `ops_run_periodic` (a periodic job is launched
    once now; refused when it is stopped or a launch of it runs or waits) and
    `ops_revert_jobs` (one to five `service` jobs, each named once, each back to the
    newest version Nomad keeps before the one it runs: one decision and one approval for
    them all, as a release is its jobs together; refused, with the job, when a job has no
    earlier version, when that version is not stable, when the job is stopped or when
    its deployment still runs). A revert keeps each job's two versions as they were when
    it was asked (`Action.reverts`), which its decision says and a dry run dates; when it
    runs, every job is read again and a job whose versions are others by then fails the
    action before anything is reverted, and Nomad itself refuses a job that runs another
    version (`EnforcePriorVersion`). The jobs are reverted in the order given and the
    first Nomad does not revert ends the action, which says which were reverted. The
    target of the first two is the job; of a revert, each of its jobs, so the brakes
    hold for every one of them. `ops_revert_jobs` is never in a team's trust: its
    decision's stakes say that only a person approves it (`Stakes.person`), and the
    team's policy approves no such decision, whatever its trust names.
    *Its jobs* (owner 2026-11-12): a Nomad environment names the jobs its actions act on
    (`nomad_jobs`: at least one, each a Nomad job's name, each once; settings without
    them, or a Kubernetes environment with them, are refused, with the field). An action
    on any other job (an allocation of it, its launch, a revert that names it) is refused
    as a target that is not what the action is for, naming the job and the environment's
    jobs, before anything is decided: the token's rights are the namespace's, so this
    setting is what keeps a database's job a person's.
    The Kubernetes actions in a Nomad environment, and these in a Kubernetes one, fail
    with `FailedPrecondition`, naming the environment, what it is and its actions.
    *Its account* (owner 2026-11-12): a Nomad ACL token with `read-job`,
    `alloc-lifecycle`, `dispatch-job` (the periodic launch) and `submit-job`. A revert
    needs `submit-job` before Nomad 1.11, which has `revert-job`; FreeBSD has Nomad
    1.9.6 and no newer, so the token may submit any job and the bound is that no agent
    reads it and that the module sends only these three requests. On Nomad 1.11 or later
    the token has `revert-job` and `force-periodic-job` instead.
  - *Each action is a decision* (owner 2026-10-27): its target is read first (it must be
    what the action is for), the brakes are checked, then `ops` records the decision on the
    task as the asker (`decide`, risk `operations`, stakes: the action by name, its
    environment and its target) and keeps the action (`ops/action`: what, why, the
    decision, its state `waiting`, `running`, `done` or `failed`, its result). The team's
    trust in operations (`Trust.operations`: actions and environments by name) approves it
    at once, and it runs (Hermes's L0); else it waits for a person, in the app or in
    Telegram (L1), and runs on the memory write of its approval, once. An action whose
    decision a person declines, or its asker withdraws, is given up (owner 2026-11-11): its
    state is `declined`, a comment on its task says why, and its target is free for another
    action (before, it waited for ever and the brake held its target). *Its look at what
    waits* (owner 2026-11-11): from its start `ops` reads every minute the actions that
    still wait. One whose decision was answered is settled as it stands (the write of a
    decision answered while the module was down is not told again). One no one answered
    for the settings' `remind_minutes` (30) is said again once on the channel; one no one
    answered for `give_up_minutes` (240, more than the reminder's: on Equal the alert's
    repeat interval, so a problem still there asks again) is given up: `ops` withdraws its
    decision in its asker's name, with why. Only the actions the team's trust does not
    approve wait at all, and those never run without a person. What no team may do
    alone (deleting data, a migration) is no action (L2).
  - *Running*: the target is read again (it may have changed since), the change sent (a
    merge patch, the mirror messages encoded by the proto3 JSON mapping; a delete with its
    dependents in the background), and the action written with how it ended; a comment on
    the task says it and the channel is told. A target that changed is a failed action,
    written and said, not an error. *Checked after an approval* (owner 2026-11-11): an
    action that ran on a person's approval ended after its asker's turn, so its comment
    names the asker (`@<asked_by>`), which wakes that member of the team to look at the
    target and say whether it helped (on Equal no one checked or moved the task). One the
    trust approved at once names no one: its asker checks in the turn that asked.
  - *Brakes*, checked before the decision: no other action on the target waits for
    approval; the target was not acted on within the settings' minutes (30); fewer than the
    settings' actions an hour (6) ran in the environment. A refusal says why and that it
    needs a person.
  - *One account per environment*: Hermes kept two (L0 and L1, a broker under another user)
    because the agent could read the L0 kubeconfig; here no agent reads either, so the
    policy and the board are the gate and the account's RBAC the bound.
  - *Board and team*: `Risk::Operations`, `pcx.team.v1.Class::Operations`, `Stakes.action`,
    `Stakes.environment`, `Stakes.target`, `Stakes.person`, `Trust.operations`
    (`OperationsLimit`) are new fields and values only; a refusal says which action or
    environment the trust lacks, or that only a person approves the decision.
