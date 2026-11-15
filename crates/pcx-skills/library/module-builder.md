---
name: module-builder
description: Turn a request in words into a dynamic module in Python or TypeScript: write its proto, code and tests, install it, and tell a person what to approve.
tools: dynamic_install, dynamic_list, dynamic_get, dynamic_remove, workflow_tools, bash, read, write, edit
---
# Building modules

A dynamic module is a small program in Python or TypeScript that the host runs as a module,
with tools that agents, workflows and people call. You write it, you install it with
`dynamic_install`, and it runs only after a person approves that exact code on the board.

## Choose the language

- **TypeScript** for logic, API calls and anything CPU-heavy written by hand: it is about 35
  times faster than Python on plain CPU.
- **Python** for data: numpy, pandas and scipy do the heavy work in native code. Plain Python
  loops are slow; a hot loop belongs in numpy.
- A tool that mostly waits on other tools runs the same in both: pick what fits the task.
- **No module at all** when a workflow can do it: a workflow joins existing tools without code.

## What a module cannot do

- **No network.** `requests`, `fetch` or a database client fail. It reaches outside only through
  the `http` module's connections: name `http` in `uses` and call its tools through the host.
- **No other processes.** `subprocess` and `Deno.Command` are refused.
- **No files but its scratch directory** (`TMPDIR`), emptied at each start. What must last goes in
  documents of a kind the manifest declares, through the memory module.
- **No settings and no secrets.** Parameters live in its documents; a credential lives in a
  connection of `http`.
- **Memory** is 256 MB unless `grants.memory_mb` asks for more. Past its limit the process is killed
  and started again. Importing numpy and pandas alone takes about 100 MB: ask for 512 or more.
- **A call has 10 seconds** by default. Long work does not belong in one call.

## Native packages: one version of each

Every module shares one version of each native package, and a lock file naming another version is
refused, saying which to lock: numpy 2.5.4, pandas 3.1.0, scipy 1.18.1, pillow 13.0.0, lxml 6.1.3,
cffi 2.1.1, cryptography 50.0.2, pydantic-core 2.50.2. Another native package has no wheel on
every host, so it may be refused, naming it: then choose a pure package.

`lxml` turns on Python's GIL for the whole process, so its threads run one at a time; the check
shows this to the person approving.

## How you work

Follow these steps in this order. Use each command as it is written.

1. Understand what the tools must take and answer. Ask when it is unclear; never guess.
2. `dynamic_list` shows what exists: a change of one is a new version under its id, not a new id.
   `dynamic_get` answers the text of its latest files: change them, and install them again.
3. Work in `$TMPDIR/<id>`, made with `bash`: `mkdir -p "$TMPDIR/<id>"`, and `echo "$TMPDIR"` tells you
   its path. `read`, `write` and `edit` reach it by that path. Nowhere else: not `/tmp`, not a repository.
4. Write the files there: the proto, the module, its tests, its lock file when it has packages. Use
   `write` for a new file, `edit` to change one and `read` to see one. `bash` is only for commands
   (`mkdir`, `deno`, `uv`, `python3`): never make or change a file with `cat`, `sed`, `perl` or a heredoc.
5. Run its tests as the check will, from that directory, with the caches outside it:
   - TS: `DENO_DIR=$TMPDIR/.deno-<id> deno test --no-prompt --allow-all --no-check --frozen --lock=deno.lock tests/`,
     and the types with `DENO_DIR=$TMPDIR/.deno-<id> deno check --frozen --lock=deno.lock tests/`,
     and keep the logic in `src/` so the tests import it, never `module.ts`. Do not pipe them through
     `tail` or `head`: the pipe answers `exit 0` when a test failed;
   - Python: `python3 -m unittest discover -s tests`.
6. `dynamic_install` with `dir`, that directory's path, its `id`, `version`, `language` and `grants`: it
   reads the files itself. Never put a file's text in the call: what you type again is not what you tested.
7. Tell the person what it does, which modules it uses and that its decision waits on the board.
   Until they approve, it does not run, and any version approved before goes on.
8. A new version asks again, however small the change. `dynamic_remove` stops and removes it.
9. Once approved, the team's agents and the chief call its tools, and they are in `workflow_tools`:
   a workflow calls one with a `tool` block, as any other module's. A workflow whose grants do not reach the module yet needs its own approval.

Write no generated message code (`*_pb2.py`, `*_pb.ts`): it is generated from the proto when the
module is prepared. Never write the `pcx.module.v1` protocol by hand: the bindings speak it.

## When to stop

Going round costs more than asking. Stop and answer in the task's thread, with the exact error and
what you measured, as soon as one of these holds:

- **A limit or a refusal of the host** that no change of your own code answers: memory, time, a grant
  you were told not to ask for, a package refused, a tool missing. Name the limit and the number.
- **The same error twice** after you changed something for it.
- **Your tests pass, and the install is refused for a reason that is not in your files.**

## What you do not do

- Do not read how the host works to get past something: `/opt`, the launcher, the bindings' sources, a
  package cache. What you need of them is written here.
- Do not imitate the check or its preparation with your own runs, beyond the commands above.
- Do not try other flags, cache directories or versions in turn to see what passes.
- Do not build, install or download tools of your own.
- Do not read a package's source when its API is enough: try the one call, in one test.
- Do not write a whole file again for a small change: `edit` it.
- If `read`, `write` or `edit` refuses a path, that is a refusal of the host: stop as above, do not go round it with `bash`.

## The files

- `module.py` or `module.ts`, where it starts; its id is the `id` you install it under;
- `proto/<package>/v1/<name>.proto`: its service and messages, each RPC's comment what an agent reads;
- `requirements.lock` (`uv pip compile --generate-hashes`) or `deno.lock`: exact versions and hashes;
  make it with `bash`, in a scratch directory, never by hand: for Python, a `requirements.in` listing
  the packages, then `UV_CACHE_DIR=$PWD/.uv uv pip compile requirements.in --generate-hashes --python-version 3.15 --python /usr/local/bin/python3.12 -o requirements.lock`;
  for TS, a `deps.ts` with one `import "npm:name@version";` per package, then
  `DENO_DIR=$TMPDIR/.deno-<id> deno cache --lock=deno.lock deps.ts`, with every package the tests import
  in `deps.ts` too. Only `deno.lock` or `requirements.lock` goes in the bundle, and hidden files never do;
- `tests/`: Python `unittest` (`tests/test_<name>.py`), or `deno test` with the packages of its lock
  (`tests/<name>_test.ts`: another name is no test). They run in the same locked
  run, within the module's memory, and failing refuses. The types of the tests and what they import are
  checked apart, before them, within 1 GB, and an error in them refuses too.

`effect` says what a tool does: `READ` changes nothing, `WRITE` changes state inside pcx, `EXTERNAL`
acts outside (through `http`, say).

## Example in Python

The files, in `$TMPDIR/sums`, then `dynamic_install` with
`{"id": "sums", "version": "1", "language": "LANGUAGE_PYTHON", "dir": "<that path>", "grants": {}}`. In
the call's own form, for a module of a few lines where nothing was run first (a tool that cannot reach
a directory), they are:

```json
{
  "id": "sums",
  "version": "1",
  "language": "LANGUAGE_PYTHON",
  "files": [
    {
      "path": "proto/sums/v1/sums.proto",
      "text": "syntax = \"proto3\";\npackage sums.v1;\n\nservice Sums {\n  // The sum of the numbers.\n  rpc Add(Numbers) returns (Total);\n}\n\n// Numbers to add.\nmessage Numbers {\n  // The numbers.\n  repeated double values = 1;\n}\n\n// Their sum.\nmessage Total {\n  // The sum.\n  double total = 1;\n}\n"
    },
    {
      "path": "module.py",
      "text": "from pcx_module import Effect, Module\nfrom sums.v1 import sums_pb2 as pb\n\nmodule = Module(id=\"sums\", version=\"1\", proto=\"sums/v1/sums.proto\", description=\"Adds numbers.\")\n\n\ndef total(values):\n    return sum(values)\n\n\n@module.tool(\"/sums.v1.Sums/Add\", name=\"sums_add\", effect=Effect.READ)\nasync def add(ctx, request: pb.Numbers) -> pb.Total:\n    return pb.Total(total=total(request.values))\n\n\nif __name__ == \"__main__\":\n    module.run()\n"
    },
    {
      "path": "tests/test_total.py",
      "text": "import unittest\n\n\nclass Adds(unittest.TestCase):\n    def test_adds(self):\n        self.assertEqual(sum([1.0, 2.0]), 3.0)\n"
    }
  ],
  "grants": {}
}
```

## Example in TypeScript

The same module's start, in `module.ts`:

```ts
import { Effect, Module } from "@post-cortex/module";
import { file_sums_v1_sums, Sums } from "./gen/sums/v1/sums_pb.ts";

const module = new Module({ id: "sums", version: "1", proto: file_sums_v1_sums, description: "Adds numbers." });

module.tool(Sums.method.add, { name: "sums_add", effect: Effect.READ }, async (_ctx, request) => ({
  total: request.values.reduce((a, b) => a + b, 0),
}));

await module.run();
```

A package from `deno.lock` is imported as `npm:name@version`. A module calls a module it names in
`uses` through its context: `await ctx.host.call(Method, request)` in TS,
`await ctx.host.call("/pkg.v1.Service/Method", request)` in Python.
