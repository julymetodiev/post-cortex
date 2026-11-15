# Dynamic modules and the bindings

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 7. Approved by the
owner on 2026-10-31 (wave 1, board `pcx-cloud`, task `5a56637e`); built in waves 4 and 5.
The probe (`05bc1d1f`) adds its measurements here.

## The rule (owner 2026-10-31)

A module written in a language other than Rust is written only through the bindings to the
Rust core: PyO3 for Python, napi-rs for TypeScript (Node, and Deno and Bun as far as their
Node-API holds). There is no SDK of the protocol in another language: no `pcx.module.v1`
generated for Python or TS, no gRPC library in a module's code, no hand-written server. WASM
is not a tier for these (too slow outside Rust, C and C++). The machine check is
`foreign_modules` in `scripts/check-architecture.sh` (below).

## The core

- **`pcx-module-core`** (library, API layer): what a module process needs, moved out of
  `pcx-module-api` so the bindings and Rust process modules share one implementation: the
  `pcx.module.v1.Module` server on the host's socket, the manifest and its checks, the
  descriptors of the module's own proto, settings and secrets (decoded from `StartRequest`),
  the host handle (`host.call` through the callback socket, documents, the bus), the module's
  log ring, deadlines and cancellation. `pcx-module-api` keeps the in-process `Module` trait
  and re-exports what both use; nothing is copied.
- **Control is inverted** (as Temporal's sdk-core does it): the core owns the gRPC server and
  one tokio runtime; it never calls into Python or JS from its threads. The language side
  asks the core for the next call (`next_call()`, awaited on the language's own event loop),
  runs its handler and answers it (`complete(call, bytes)` or `fail(call, error)`). Only
  protobuf bytes and a few plain values (ids, the error's kind and message, the deadline)
  cross the boundary.
- **Messages**: a module's own messages (its package's proto) are generated for its language
  by the language's protobuf compiler (Python `protobuf`, TS `@bufbuild/protobuf`); that is
  message code, not the protocol: the core decodes nothing of the module's package, it hands
  the handler the request's bytes and takes the response's.

## The bindings' API

- **Python** (`pcx-module`, a PyO3 extension built with maturin): free-threaded CPython only
  (owner 2026-10-31), one wheel of the free-threaded stable ABI (`abi3t`, CPython 3.15t and
  later), so a module's handlers run on several cores at once; the host runs dynamic modules
  with its own free-threaded interpreter, never the machine's. The extension declares itself
  safe without the GIL (`gil_used = false`); its state shared between threads is behind
  locks.

  ```python
  from pcx_module import Module, tool, Effect
  from greet.v1 import greet_pb2 as pb

  module = Module(id="greet-py", version="0.1.0", proto="greet/v1/greet.proto",
                  description="Says hello.", uses=["post-cortex.memory"])

  @module.tool("/example.greet.v1.Greeter/Greet", name="greet", effect=Effect.WRITE)
  async def greet(ctx, request: pb.GreetRequest) -> pb.GreetResponse:
      return pb.GreetResponse(text=f"Hello, {request.name}")

  module.run()   # serves on the socket the host gives
  ```

  `Module` also takes `emits` (name, message type, description), `follows`, `config` (the
  message of its settings) and `concurrency` (the calls it runs at once, default 8: one
  worker thread each, with its own event loop). A handler is an `async def`; it fails a call
  by raising `ModuleError(kind, message)`, any other exception fails it as `internal`.
- **TypeScript** (`@post-cortex/module`, napi-rs): the same, `tool(method, { name, effect }, handler)`,
  `method` the generated method (`Greeter.method.greet`, typed) or its path; a handler
  answers a message or what makes one, and throws `ModuleError(kind, message)`. Its calls run
  on the runtime's event loop, `concurrency` at once.
  `workers` (Deno, default off, owner 2026-11-02) runs the handlers on that many Web Workers, for
  a module whose calls are CPU: the main thread keeps the core and hands each call on with its
  bytes, so no worker loads the addon, and a call's `ctx` reaches the host through the main
  thread. The author writes no worker code; a variable of the module is one per worker, not one
  for all calls. Without it a module doing I/O loses nothing. Node refuses it. Written at the first
  module that needs it (owner 2026-11-02).
- `ctx` is the call's scope (author, workspace, caller, the time left, whether its caller gave
  up) and the host handle: `ctx.host.call(path, request)` calls a module it uses (TS: the
  generated method and the request), `ctx.publish(name, message)` emits a declared event,
  `ctx.settings` the decoded settings, `ctx.log`. A failure of a call arrives as
  `ModuleError` with its kind.
- The manifest is built by the core from what the module declares (its RPCs with tool,
  visibility and effect, its events, settings type, uses, capabilities); the same checks as a
  Rust module's refuse it.
- Native packages for Linux (x86_64, aarch64), macOS (aarch64) and FreeBSD (x86_64). PyPI
  takes no FreeBSD wheels: the FreeBSD wheel and the npm `os: freebsd` package are built with the
  image and enter it. `pcx-module` (PyPI) and `@post-cortex/module` (npm, `@pcx` is taken) are public
  for macOS and Linux since 2026-11-02.
- **Where**: `bindings/python` (the extension and `pcx_module`), `bindings/ts` (the addon,
  `index.js`, `index.d.ts`), `examples/greet` (the example in both, its proto),
  `bindings/tests` (the end-to-end test through the real host: the example, and a fixture
  that calls a Rust module through its host, fails with a kind, outlives a caller that gave
  up and runs calls at once). `scripts/bindings-gen.sh` writes the message code of their
  protos; `scripts/test-bindings.sh` builds both bindings and runs the test with Python,
  Deno and Node.

## A dynamic module

A module that is not compiled into the host or shipped in the catalog: its source (Python or
TS, with its proto and a lock file of its dependencies) is installed into a running host by a
person or an agent. Approved by the owner on 2026-11-02, from the decisions of step 0 and its
probes (below).

### The module `dynamic`

- **`dynamic`** (crate `pcx-dynamic`, proto package `pcx.dynamic.v1`, built in on a daemon, a
  catalog process in the cloud): keeps the bundles and their versions, checks them, asks for
  their approval and answers which are approved. It runs nothing itself: launching is the
  host's, through its port (below). Its settings name a `workspace_id` and a `board`, as
  `workflow`'s do; without a board nothing can be approved, so an install refuses
  (`FailedPrecondition`). Its bundles are the organisation's store (owner 2026-11-10;
  docs/connect.md section 4a, a module's data): every caller, from any workspace, reads them;
  one is installed, edited or removed by a caller who reaches that workspace.
- **Its bundle** (document `dynamic/module`, `pcx.dynamic.v1.DynamicModule`, one per module id;
  each change a new version of the document): the id, the version, the language (`python` or
  `ts`), the files (a blob of the workspace, a tar of at most 16 MiB, the one type beside images
  that `Blobs` keeps) and their SHA-256, the grants it asks for,
  and, written only by the module: what the check found (the manifest with
  its descriptors, the log, the tests' result), `approved` and `asked`, as a workflow's. The
  bundle lives in one workspace, and the module it holds runs for the whole organisation, as
  every module does.
- **How a bundle arrives** (owner 2026-11-02): an author with `tools-fs` names a directory (`dir`): the
  module reads its files through that author's `tools-fs`, leaving out hidden files and `node_modules`, so
  what is installed is what it tested and no text passes through its model. `files` in the call remains
  for a caller with no such directory. Both at once are refused.
- **What a bundle holds**: `module.py` or `module.ts`, its protos under `proto/`, its lock file
  and its tests under `tests/`. Refused by the check, each naming the file: more than 16 MiB
  packed, 64 MiB unpacked or 1,000 files; an absolute path, `..`, a link or a device in the tar;
  a file that breaks `foreign_modules` (below); generated message code (it is generated when the
  bundle is prepared, so what runs is what the proto says); a lock file that names the binding
  (the host gives the binding, the one matching it).
- **Tools** (`dynamic_install`, `dynamic_list`, `dynamic_get`, `dynamic_remove`): installing is
  `external` (it runs the bundle's code in the locked run), listing and reading `read`, removing
  `write`. A file comes as its path and text, and the module makes the tar, without times or
  owners, so the same files give the same hash. `dynamic_get` answers the text of the module's
  latest files, approved or not, so an agent in a new session reads what it edits, and changes it
  by installing again. The view's own tools (`dynamic_files`, `dynamic_edit`) are not an agent's.
- **The view** (level 2 of `host.md`, no code in the app): each module's version, files with their
  text, the difference from the approved version, grants, tests' result and log. A decision links
  to it: a person never approves code they cannot see. Approving stays the board's.
- **A person's edit** (owner 2026-11-02): the view changes one file or adds one, with the module's
  version after the change. It is a new version over the module's latest files, its grants and
  language kept, through the same checks and approval as an install; meanwhile the approved
  version goes on. A change that read an older version of the document is refused
  (`Conflict`), and a file is removed by installing without it.

### From a bundle to a running module

1. *Prepared*: the host's launcher unpacks the bundle read-only, installs its packages and
   generates its message code, before any of its code runs. Nothing is compiled here.
   - Generated code keeps no comments, and an RPC's comment is its tool's description: the
     compiler also writes the descriptors with their comments, and the host names them to the
     core (`PCX_MODULE_DESCRIPTORS`).
   - Only from its lock file, each package checked by its hash, without running a package's
     scripts: Python with `requirements.lock` (`uv pip compile --generate-hashes`), installed by
     `pip --require-hashes --only-binary :all:`; TS with `deno.lock`, by `deno cache --frozen` of
     every package the lock names, whoever imports it, which runs no scripts. The network here is the installer's, never the module's.
   - **One version of each native package** (owner 2026-11-02): the list
     `crates/pcx-module-launcher/src/native-packages.txt` names it, for every module and platform.
     A lock file naming another version is refused, saying which to lock. macOS and Linux take that
     version's wheel from the index. PyPI has no FreeBSD wheels, so on FreeBSD the wheels are
     built from the list when the image is built, once for a list and a Python, in a build pot with
     the compilers, and only the wheels and the libraries they link enter `pcx-modules`. A package
     with no wheel for the platform and not on the list is refused, naming it; adding one is a change
     of the list, by us.
   - An environment is kept by the hash of its lock file, the platform and the list, read-only, and
     shared by the bundles with these, on a volume of the node, outside the image.
2. *Checked, in the locked run*: the bundle's code runs before anyone approved it, so it runs
   with nothing: no network, no `uses`, no capabilities, no callback socket, an empty scratch,
   the memory limit below and 60 s in all. The host reads its manifest and runs the same checks
   as for any module, and its tests (`unittest discover`, `deno test --no-check`); a failure refuses the
   version and is kept in its log. A TS bundle's tests have their types checked before them (`deno check tests/`), apart,
   within 1 GB: the compiler took 419 MB where the tests took 196 MB, and the module's limit is for its
   run, not for its compiler. It runs none of the bundle's code. Its `kind` is `tool`: a slot is never dynamic. It also reports
   whether its packages turned the GIL on: a package that does not declare itself safe without it
   (`lxml`, measured) turns it on for the whole process, so its Python threads run one at a time.
   The GIL is never forced off, since the package did not say it is safe; the view shows it.
3. *Approved*: installing a dynamic module, a new version of it, or a widening of its grants is
   a board decision with the risk `code`, which a person approves (or the team's autonomy
   policy, when it allows `code`), asked in the module's name (`dynamic:<id>`). Until then the
   version does not run, and the version approved before it goes on. `approved` and `asked` are
   written only by the module, whatever a request says, by one implementation shared with
   `workflow` (`pcx-approval`, a library: what was approved, whether a write asks, the decision
   heard on the bus or read again at start). A decision no one waits on any more is withdrawn
   by the module in the name it asked in (`WithdrawDecision`, `pcx-approval`'s `withdrawal`;
   owner 2026-11-10: on staging a removed module's decision still waited to be approved): when
   the module is removed; when newer files are asked about instead, or the approved files are
   installed again; and at start, each decision of a module's task that it asked and that still
   waits, besides the one it waits on now. One that waits no more is left as it is, and a board
   that does not answer is logged. Files a person declines (`board.decision_declined`, or read
   again at start; owner 2026-11-11) are kept as declined (`Asked.declined_by`): they do not
   run, the version approved before goes on, no one is asked again, and writing the files
   again asks again.
4. *Loaded*: the composition follows `dynamic.version_approved` and `dynamic.version_removed`
   on the bus, as it feeds memory's writes, reads the approved set from `dynamic` and calls
   `Host::reload` with it, which keeps what already runs. A running tenant is not reloaded,
   so a change waits for nobody (`TenantDirectory::reload` is left to the organisation's
   catalog choices). The set is read again when the host starts.
5. *Run*: it runs while its host does (owner 2026-11-02): a tenant's idle stop (`evict_idle`) stops
   its modules with it, and a process that exits on its own is started again by T4's backoff and
   breaker, as any. The launched manifest must name its id and ask for no more than was approved.

### Where it runs and with what

- **The port** (`ModuleLauncher`, pcx-host's ports, as `Settings`): launching a module's process
  goes only through it; the composition chooses the adapter. Direct, for first-party modules on
  a daemon; under the `Sandbox` port, for a dynamic module on a daemon; the launcher's socket,
  for every module in the cloud (task c199da28): a module of the catalog too, so none runs as
  the cloud's user, with the cloud's environment, or with a network its grants do not give.
  Two run beside the cloud: `dynamic`, whose adapter is the launcher's client (so it has no
  network grant and starts nothing itself), and a module described at the cloud's start (its
  manifest alone: no configuration, no organisation). A cloud without a launcher (`[dynamic]`;
  development and tests) runs its modules directly. The check
  `module_launch` counts any other spawn in the host's process tier; its budget falls to 0.
- *daemon*: the same launcher, served inside the daemon on a socket of its data directory, under
  the OS sandbox (Seatbelt, Landlock and seccomp), so there is one path to run a module (owner
  2026-11-02).
- *cloud*: a pot of its own, `pcx-modules`, beside `pcx-cloud` in its Nomad group so on its
  node. It holds no secret (`/usr/local/etc` is readable from a jail, so nothing is kept there)
  and has its own memory and CPU limit, so a module reaches neither the cloud's credentials nor
  its memory, and `pcx-cloud` needs no mount rights.
  - Its one service is the `pcx` binary's launcher mode (`pcx module-launcher`,
    `pcx.module.v1.Launcher`: `Prepare`, `Launch`, on a socket in a directory shared
    with `pcx-cloud` by nullfs). It confines every process with `pcx sandbox-run` (`host.md`, one
    jail per module and organisation, as the unprivileged `[sandbox] user`).
- **No network, on every host**: `Launch` has no field for it (owner 2026-11-02, kept
  2026-11-08): a dynamic module's jail has no address, and a
  module reaches outside only through the `http` module (`uses`), with its connections, its
  rules and its log (owner 2026-11-02). So `requests` or a database client does not work
  directly, and `module-builder` says so. `Prepare`'s install alone has an address.
  - **The host's sockets**: each module has a directory under the shared one with its socket and
    its callback socket, the only part of it its jail sees. The launcher owns the directory and
    gives the sockets explicit rights for the jail's user; no umask is relied on.
  - **A host that is not root** (owner 2026-11-08: `pcx-cloud` runs as the user `pcxcloud`, uid
    and gid 1501, so a fault in the cloud has no root in its jail, and the node's rules for
    users hold for it too). The launcher names its hosts' group (`--host-group <gid>`) and
    gives it its socket (0660), the shared directory, where a host makes each launch's
    directory (0770), and each launch's directory before the process starts (the jail's user
    owns it, 0770), so the socket a module makes there takes that group; a module serves
    its socket 0660 (`pcx-module-core`, for every language). Without it the launcher's socket
    is its own user's alone, as on a daemon and on staging, where host and launcher are one
    user.
  - **A host that goes**: a module's parent here is the launcher, which outlives a host, so the
    launcher kills a host's modules when that host's connection ends; a module whose launcher is
    gone stops by itself (`host.md`, its parent changed). On FreeBSD a helper waits for the
    process, to remove its jail, so it is never killed: it holds the launcher's end of a pipe on
    its stdin, and kills the process when that closes, however the launcher ended.
  - Staging, on Linux, runs the same launcher mode with Landlock in place of the jail.
  - **A module of the catalog** (task c199da28): its program is one of the launcher's programs
    (`--program`, `--program-network`: the pot's copy of the catalog's binaries, as `--runtime`
    names the runtimes), its SHA-256 the catalog entry's, which `Launch` carries and the launcher
    checks at each launch. It reads only the system's files and runs in its scratch directory
    (`Launch` names nothing to read). Its environment is only what
    `Launch` names (no environment of the cloud's), its limits its catalog entry's (memory,
    open files; 256 MB and 1024 by default, owner 2026-11-08). Which programs have the network
    is the launcher's own configuration (`--program-network`, generated by the release from the
    same catalog file as the cloud's catalog: `cloud_job.py` on prod, `dynamic_swap.sh` on
    staging), never a field of `Launch`: a host cannot ask for it, and a program the catalog
    grants it that the launcher names with `--program` runs without it. Its jail then has the
    pot's address (public addresses only, `host.md` *Network*). A runtime never has it, so a
    dynamic module still has none. Staging runs the same path under Landlock.
- **Grants**: what a bundle asks for must be approved, and its `uses` is within its author's.
  Never: `*`, `dynamic` (a module installs no modules), a slot, `ui_html`, `network`, `spawn`
  (no process but its own) or `filesystem` (files through the modules that serve them), on any
  host (owner 2026-11-02).
- **No settings**: a dynamic module has no `config_type` (owner 2026-11-02): it keeps its
  parameters in its own documents, and a credential in a connection of `http`.
- **Pinned**: a bundle is checked against its hash when it is unpacked, then only the launcher's
  user writes it and its module's jail mounts it read-only (owner 2026-11-02). An environment is
  read-only likewise. The runtimes are the image's on the cloud, and on a daemon hashed once while
  their files stay the same.
- **A lock file** is made by its author: the runner's image has `deno` and `uv` for an agent to
  make one with `bash` (2026-11-02; the `module-builder` skill says how).
- **Limits** (per module, from its grants, under the organisation's plan):
  - *memory*, default 256 MB, which a module asks more of in its grants (numpy and pandas load in
    93 MB, and the probe's work peaked at 393 MB; `module-builder` says so): the launching side reads the process's RSS every second and kills
    it past the limit (its failure names the limit, its breaker counts it), V8 gets
    `--max-old-space-size`, and the pot's total is a hard limit from Nomad (`racct`). `rctl` is
    refused inside a pot, and `RLIMIT_AS` stops V8 from starting, so neither is per module. Until
    the reading sees it, a burst presses only the other modules of its pot;
  - *time per call*, default 10 s: the call's deadline. CPU time per call cannot be read on a
    process that serves calls at once, so a call past its deadline fails, and repeated failures
    open the breaker and restart the process;
  - *open files*: `RLIMIT_NOFILE`, default 256;
  - *processes*, none: FreeBSD `RLIMIT_NPROC` 1 (threads do not count), Linux
    seccomp (its `RLIMIT_NPROC` counts threads), macOS no `process-fork` in the profile;
  - *calls at once*: the module's concurrency, default 8;
  - *modules of an organisation*: a limit of the plan, none else (`free` 3, `pro` 20, `team` none;
    owner 2026-11-02). A new module past it is refused, a new version of one never, and what it
    already keeps stays after a downgrade. The cloud gives it as the process's last argument, so no
    setting can change it; a plan changed applies when the tenant next loads.
- **Runtimes**: CPython 3.15.2t and Deno, with the binding, are the host's: in the `pcx-modules`
  image on the cloud (built from python.org's source with PGO, full LTO and tail calls, as no
  FreeBSD package has it; the package `deno`), and on a daemon the paths `daemon.toml` names
  (owner 2026-11-02). A running host depends on no
  index. Python's version is in `bindings/python/.python-version`, the one place naming it.

## The builder (owner 2026-11-02)

- *The skill* `module-builder` is a built-in skill of `skills`: how to choose the language, what a
  module cannot do, the one version of each native package, the bundle's files, and worked examples,
  the one in Python checked by a test as `pcx-dynamic`'s rules read it.
- *The agent* `module-builder` is a built-in agent of `team`: a harness agent with the skill. It writes
  and installs; a person approves its code on the board.
- *Who reaches a module* (owner 2026-11-02): a module whose `uses` names `dynamic` also reaches every
  module `dynamic` loaded, since a person approved its code: `agent`, `team` and `chief` name it, so
  an agent calls an approved module's tools, within its member's grant. A workflow reaches them
  through its `*`.

## The check: `foreign_modules`

`scripts/check-architecture.sh` counts, in the repository's Python and TS files outside
`bindings/` (the bindings' own sources and their tests), every line with generated
`pcx.module.v1` code or that package's name (`pcx_module_pb`, `pcx_module_grpc`,
`pcx.module.v1`, `pcx/module/v1`); its budget is 0. A gRPC library alone does not count:
the client SDKs in `sdks/` speak the public memory API (`pcx.v1`), not the module protocol.
The release check of a dynamic bundle runs the same rule on the bundle before it is offered
for approval: `pcx-dynamic`'s domain holds the rule's words, and a test of it fails when they
leave this script's.

The other checks of this document: `module_launch` (only the port launches a module's process),
`approval_asks` (one implementation asks a person for code, `pcx-approval`) and
`python_version_copies` (no script names Python's version).

## The probe (05bc1d1f, 2026-10-31)

A minimal core (the inverted control above, over `pcx_module_api::serve`), a PyO3 binding
for CPython 3.14t (free-threaded, 8 worker threads) and a napi-rs binding loaded by Node 26
and Deno 2.9. One module, `probe.v1.Probe`, ran in five forms as a process module of the
real `pcx-host`:
- a Rust module with its generated tonic service (as today);
- Rust on the core (the bindings' handoff, without a language);
- Python and JS on the bindings.

Its RPCs:
- `Echo`: the call alone;
- `Typical`: an HTTP GET of 100 JSON items, their sum, and a call of an in-process module
  through the host;
- `Heavy`: Collatz steps of 1..100 000, pure CPU;
- `Pid`: for the module's memory.

Measured on an 8-core Apple M (6 performance cores), the median of two runs. Code and raw
results are in `pcx-bindings-probe/`, next to this repository.

| | Rust | Rust on core | Python 3.14t | Node | Deno |
|---|---|---|---|---|---|
| ready, ms (of which pin hash) | 64 (41) | 62 (40) | 102 (55) | 505 (439) | 466 (398) |
| `Echo` p50 / p99, µs | 54 / 88 | 69 / 115 | 94 / 134 | 115 / 164 | 108 / 150 |
| `Echo`, calls/s (64 callers) | 30 100 | 31 000 | 29 100 | 16 300 | 16 700 |
| `Typical` p50 / p99, µs | 310 / 415 | 310 / 410 | 402 / 582 | 504 / 970 | 385 / 570 |
| `Typical`, calls/s (32 callers) | 15 900 | 15 700 | 12 700 | 6 500 | 9 700 |
| `Heavy`, ms (one / 8 at once) | 11 / 18 | 11 / 16 | 629 / 1 296 | 18 / 145 | 18 / 144 |
| memory, MB (idle / after load) | 6 / 12 | 5 / 12 | 36 / 44 | 72 / 193 | 86 / 116 |

What it shows:
- **The core's handoff costs ~15 µs a call** (Rust on core against Rust) and no throughput.
  The inverted control holds.
- **The language adds 25–45 µs a call.** A tool that does I/O is within 1.3 times Rust in
  Python and Deno (`Typical`). Node's `fetch` is the slowest under concurrency.
- **Pure CPU in Python is ~57 times Rust.** JS is 1.6 times Rust, but on one thread: 8
  calls at once take 8 times as long. Free-threaded Python runs them on the cores, 3.9
  times faster than one after another (Rust 4.9 times). The skill tells authors so: hot
  loops go to native packages (numpy) or to a Rust module.
- **A start is the pin, not the runtime.** Without the hash, every form is ready in 20–70 ms.
  `pin::at_load` reads and hashes the whole executable at every launch, at about 320 MB/s;
  Node and Deno are 130–146 MB. Decided (owner 2026-10-31), before dynamic modules
  (wave 5):
  - the host's own interpreters are hashed once, when the host starts;
  - a dynamic module's bundle is pinned by its hash, as above, since the interpreter's hash
    does not cover the script;
  - the SHA-256 uses the CPU's instructions.
- **Protobuf on 3.14t is the pure-Python backend** (no upb wheel for free-threaded builds
  yet); part of Python's cost per call. Measure again on 3.15t with the `abi3t` wheel.
- **Deno over Node** for TS (owner 2026-10-31): the same CPU speed, 1.5 times Node's
  `Typical` throughput, and less memory under load. Node stays supported through the same
  addon.

## Decided (owner 2026-10-31)

- `pcx-module-core` is a crate of its own (above), so the bindings link no in-process host
  types.
- Python is free-threaded from the first release (above).
- Deno is the TS runtime the host runs dynamic modules with; the probe's start fixes come
  first ("The probe", above).

## Decided (owner 2026-11-01, bindings 2)

- **The split.** `pcx-module-core` holds what a module process speaks without the
  in-process host: the module's errors, `Scope`, the gRPC channel, the log ring, names,
  JSON, the tool declarations (`Effect`, `Visibility`), configuration types and their
  checks, the tools of a manifest (`rpc_tool`), the module-facing bus, and the
  `pcx.module.v1.Module` server. The server serves a trait of the core, `Process` (its
  manifest; start with the host's API version, the configuration and the callback socket;
  events; resources; prompts; stop; its own services), with any further services on the
  same socket. `pcx-module-api` keeps `Module`, `ModuleHost` and the slots' services
  (models, context engine, turns, sandbox); its `serve` adapts a `Module` to a `Process`
  and adds the services of its slot. It re-exports what moved, so no module changes. The
  bindings depend on the core alone.
- **Python**: built against 3.15.2t with `abi3t-py315`; a later 3.15 needs no new wheel.
- **Packages**: macOS and Linux with bindings 2 (Linux aarch64 cross-built, not run), tested through the real host on both;
  FreeBSD's wheel and npm package are built with the image (wave 5).

## The probes of step 0 (2026-11-02)

CPython 3.15.2t, Deno 2.9.7 and Node, on macOS, Linux and a FreeBSD 15.1 node (Xeon Gold 6148),
through the real host. Scripts and logs were not kept in the repository.

- **The bindings end to end, 6 of 6 on each** of macOS direct and under Seatbelt, Linux direct
  and under Landlock, FreeBSD direct and in a nested jail as an unprivileged user. Refused in
  the sandbox: reading outside the roots, writing to the source (and on FreeBSD `/usr/local`),
  the network (and on FreeBSD the loopback). The wheel builds in about 3 minutes on the node, the
  addon in 2.5.
- **Unix sockets through nullfs** work in all 5 cases: across pots and from a nested jail
  without an address, in both directions.
- **Limits**: `rctl` inside a pot answers `Operation not permitted`, and works from the node, on
  a nested jail too; `RLIMIT_AS` stops Deno up to 16 GB; `--max-old-space-size` stops the heap
  only (2 GB of buffers pass). `limits -u 1` on FreeBSD lets threads run and refuses a fork
  (`EAGAIN`). Linux seccomp and macOS's profile for this are not yet run.
- **A host that goes**: through `jexec` a module's parent becomes 1, so it sees it.
- **Python on FreeBSD** (6 threads run 5.5–5.8 times one; `protobuf` 7.36.2 is pure Python on
  3.15t on all three). Median of 7 runs on the node, in ms, noise 10–18 %:

  | Build | Minutes | MB | fib | json | asyncio | protobuf | one thread | six |
  |---|---|---|---|---|---|---|---|---|
  | plain | 2 | 221 | 170 | 148 | 319 | 359 | 214 | 234 |
  | PGO, thin LTO | 19 | 207 | 148 | 127 | 275 | 320 | 173 | 185 |
  | PGO, full LTO | 18 | 176 | 162 | 119 | 283 | 320 | 158 | 169 |
  | **PGO, full LTO, tail calls** (chosen) | 16 | 181 | 119 | 120 | 263 | 287 | 173 | 179 |

  No JIT: it does nothing without the GIL (gh-133171). No BOLT: `llvm19`'s package lacks it.
- **Native packages on FreeBSD**, built offline in a jail as an unprivileged user, each from its
  source by its hash (about 17 minutes for all, once): numpy with OpenBLAS 85 s, pandas 105 s, scipy
  334 s, pillow 10 s, lxml 102 s, cffi 7 s, maturin 116 s, cryptography 53 s, pydantic-core 158 s.
  The tools cost 1.37 GiB (Rust), 359 MiB (gcc) and 83 MiB (OpenBLAS). macOS and Linux have
  wheels for 3.15t of these (no build there).
- **Deno on Web Workers**, 8 `Heavy` calls at once, the main thread handing them on; the host's
  full path with them is not yet measured:

  | | no workers | 8 workers | Faster | Added per call |
  |---|---|---|---|---|
  | macOS (8 cores) | 141 ms | 25 ms | 5.5 times | 20–30 µs |
  | FreeBSD | 252 ms | 35 ms | 7.2 times | 40–50 µs |

  On FreeBSD 2 workers take 129 ms and 4 take 66 ms; the process grows from 89 MB at the start to
  220 MB at most with 8. The addon also loads in a worker, on both, though this design does not
  need it.
