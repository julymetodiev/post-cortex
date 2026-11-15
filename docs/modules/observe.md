# observe

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8, operations.

- **`observe`** (W30): reading a system, never changing it. Crate `pcx-observe`, built in
  (its `network` grant, and `filesystem` on the directory of its credentials' files); its
  settings: the environments (`prod`, `staging`), each with its Prometheus, Loki and
  Alertmanager URLs and the file of their bearer token, and its Kubernetes API with the files
  of a ServiceAccount's token that may only read and of the cluster's CA.
  - *Credentials are files* (owner 2026-10-27): read through the module's `Sandbox` port at
    each call (a rotated token is used at once; a file outside its grant is `Denied`), never
    a setting (no secret inside a list of settings), a prompt or an answer, and outside every
    agent's sandbox. A cluster's credentials are a token and a CA, as inside a pod, not a
    kubeconfig (no YAML parser for it).
  - *TLS server name* (owner 2026-10-29): when a cluster's certificate does not name the
    URL's host (an API reached at its Tailscale address), `kube_tls_server_name` is the name
    it is verified against, as a kubeconfig's `tls-server-name`: a DNS name or an IP address,
    with the CA file, whose certificates are then the only ones trusted (`pcx-kube`, rustls).
  - *Tools* (`pcx.observe.v1.Observe`, each an agent's): `metrics_query` (PromQL at now or
    over a range, a step for about 60 points: series with their labels and samples, at most
    50 of 500), `logs_query` (LogQL over the last hour or a while: streams, the newest lines,
    at most 500), `alerts_list` (Alertmanager's, with their state and silences),
    `cluster_get` (a namespace's pods with ready containers, restarts and why one waits,
    deployments, jobs, and its warnings of the last hour) and `pod_log` (a pod's last
    lines, its previous run after a crash). A namespace, pod or container is a Kubernetes
    name before it goes into a path.
  - *A Nomad environment* (owner 2026-11-12: pcx's own production is Nomad with pot jails
    on FreeBSD, and observe read only Kubernetes and Prometheus). An environment names a
    Kubernetes API or a Nomad API, never both (settings that name both are refused, with
    the field): `nomad_api` (https), `nomad_token_file` (the file of an ACL token that may
    only read: `read-job`, `read-logs`, and `node` `read`), `nomad_ca_file` and
    `nomad_tls_server_name`, as the Kubernetes ones. Its tools: `jobs_get` (the
    environment's jobs: type, status, version, when it was submitted and whether that
    version is stable; its allocations, the newest 10: node, state, the version it runs,
    restarts, since when, and each task with its last 3 events; of a periodic job its
    schedule, as cron with its time zone, and its launches, the newest 5, each with its
    allocations: Nomad's API says no next launch, and it forgets a launch some hours
    after it ended, so a night's backup is read in the morning or not at all),
    `alloc_log` (the last lines of a task's stdout or stderr, 100 or at most 500, of the
    log's last 64 KiB; an allocation by its id or the first 8 characters or more of it,
    which Nomad is asked for and exactly one must start so; a task by name, the
    allocation's only task when none is named, and a refusal names its tasks) and
    `node_get` (each node: its status, eligible or not, draining or not, its Nomad
    version, its memory and what is available of it, and its disks; a node that answers
    no statistics, as one that is down, is listed with why). *A node's disks*: Nomad on
    FreeBSD answers every mounted filesystem, some 45 a node with pot (each ZFS dataset,
    each directory a jail mounts again, devfs); a disk is a device (`/dev/...`, once) or
    a ZFS pool, its datasets together (what they use, of that and what the pool's own
    dataset has available), and a directory mounted again or a filesystem under a
    mebibyte is none. A job, task or allocation is checked as Nomad names it before it
    goes into a path. `cluster_get` and `pod_log` in a Nomad environment, and these
    three in a Kubernetes one, fail with `FailedPrecondition`, naming the environment,
    what it is and the tools that read it; the Prometheus, Loki and Alertmanager tools
    need their URLs in either. Nomad is called over `pcx-kube`'s calls (it takes its ACL
    token as a bearer's, and its TLS is a CA file and a server name); what is Nomad's
    own is `pcx-nomad`, a library `ops` uses too: the names and the starts of ids that
    may go into its paths, the paths, what an environment's settings must be (https, the
    token's file, a CA file for a server name) and its JSON read without its nulls
    (Nomad writes an empty list, map or time as null, which the proto3 mapping takes as
    the field left out). Its answers are decoded into mirror messages whose fields are
    named as Nomad names them (`NomadJobStub`, `NomadJob`, `NomadAllocation`,
    `NomadNode`, `NomadHostStats`).
  - *The APIs' JSON*: each answer is decoded into its mirror message (`PromAnswer`,
    `LokiAnswer`, `AmAlert`, `KubeList`); Prometheus's `[seconds, "value"]` and Loki's
    `["nanoseconds", "line"]`, arrays of mixed types, are `ListValue`s the domain reads into
    typed samples and lines (JSON edges, `scripts/grpc-json-edges.txt`).
  - A member that may only read is given this module's tools and not `ops`'s: read-only is
    a grant, not a promise in a prompt (in Hermes a "read-only" profile could still run
    `curl`). The agent, the team and the chief use `observe`, `ops` (and the agent and the
    chief `scheduler`), so a member's `modules` can name them.
