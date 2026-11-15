# post-cortex for Node.js

TypeScript client for [post-cortex](https://github.com/julymetodiev/post-cortex),
long-term memory for AI agents. It talks to the daemon's gRPC API (`pcx.v1.PostCortex`,
default `127.0.0.1:3738`) with `@grpc/grpc-js`, so it runs on Node.js 20+ (not in
browsers).

```bash
npm install post-cortex
pcx start                      # the daemon: brew install julymetodiev/tap/post-cortex
```

## Quick start

```ts
import { Client } from "post-cortex";

const pcx = new Client(); // PC_GRPC_TARGET, PC_AUTH_TOKEN and PC_AUTHOR from the environment
const sid = await pcx.createSession("my project");

const entry = await pcx.record(sid, {
  kind: "decision_made",
  title: "Use SurrealDB for storage",
  description: "One shared server, one namespace per tenant",
  entities: [{ name: "SurrealDB", type: "technology" }, { name: "storage" }],
  relations: [{ from: "storage", to: "SurrealDB", type: "depends_on", context: "chosen backend" }],
});

for (const hit of await pcx.search("which database do we use?", { sessionId: sid })) {
  console.log(hit.score, hit.content);
}
pcx.close();
```

Every record needs at least one entity and one relation, with both ends of each relation
among its entities. Writes return once stored; embedding and the graph merge follow on the
daemon in the background, so a record is searchable a moment later.

## Connecting

```ts
new Client({ target: "ml.example:3738", token: "...", author: "my-agent", secure: true, timeoutMs: 30_000 });
```

- `token`: the daemon's bearer token (`auth_token` / `PC_AUTH_TOKEN`).
- `author`: who the records come from; search can filter or prefer by author.
- `secure`: TLS with the system roots; `credentials` for custom channel credentials.
- `client.stub` is the generated client for RPCs it does not wrap; `proto`, `views` and
  `blocks` export the generated messages it returns (`pcx.v1`, `pcx.views.v1`, `pcx.blocks.v1`).

## What it covers

| Area | Methods |
|---|---|
| Sessions | `createSession`, `listSessions`, `getSession`, `searchSessions`, `updateSession`, `deleteSession` |
| Workspaces | `createWorkspace`, `listWorkspaces` (`includeArchived`), `getWorkspace`, `updateWorkspace`, `archiveWorkspace`, `unarchiveWorkspace`, `deleteWorkspace`, `addSession`, `removeSession`, `resolveProject` |
| Records | `record`, `recordMany`, `records` |
| Search | `search` (session, workspace or global; `mode`; `sources`; `author`, `preferAuthor`, `trace`), `assemble` |
| Graph views | `entity`, `impact`, `lineage`, `topics`, `radar` (`includeDismissed`), `fileContext` (each its `pcx.v1` view message) |
| Radar | `dismissRadarItem`, `restoreRadarItem`, `listRadarDismissals` |
| Upkeep | `mergeEntities`, `deleteEntity`, `forget`, `purge` |
| Cards | `getCard`, `updateCard` |
| Messages | `appendMessages`, `deleteMessage`, `deleteMessages` |
| Export / import | `export`, `importArchive` (the `pcx export` file format) |
| Summaries | `structuredSummary`, `keyDecisions`, `sessionStatistics` |
| Admin | `health`, `vectorizeSession`, `createCheckpoint` |

### Projects

`resolveProject` finds or creates the workspace and session of a project key (a normalised
git remote, or `local:<hash>`), so every client on a repository writes to the same place:

```ts
const p = await pcx.resolveProject("github.com/acme/api", "api", { client: "my-agent" });
await pcx.record(p.sessionId, { ... });  // p.created: this call bound the key
await pcx.resolveProject("github.com/acme/web", "web", { workspace: "acme" });  // join by name
```

### Facts that change

Pass `supersedes: [entryId]` (and optionally `validFrom`, `confidence`) to replace older
records. Search keeps them as history with their `validity` set (`validTo`, and `reason`, a
`views.EndReason`), ranked lower; `forget` ends a record now, `purge` erases it with everything derived from it.

### Export and import

`export` resolves to an `Archive`: the bytes of a `pcx export` file (`compression` none, gzip
or zstd) and a summary. `importArchive` sends such a file in chunks; without `commit: true` it
only previews (counts, conflicts with what the daemon has):

```ts
const archive = await pcx.export({ scope: "workspace", workspaceId: ws });
await other.importArchive(archive.data);                                    // preview
await other.importArchive(archive.data, { commit: true, skipExisting: true });
```

### Errors

Failed calls reject with `PostCortexError` (with `code`, the gRPC status), or one of
`NotFound`, `InvalidArgument`, `Unauthenticated`.

## Development

```bash
npm ci && npm test       # starts a throwaway daemon: PCX_BIN, or ../../target/debug/pcx
../gen.sh typescript     # regenerate src/gen from crates/post-cortex-proto/proto/pcx.proto
npm run build            # dist/
```

MIT licence.
