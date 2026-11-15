# post-cortex for Python

Python client for [post-cortex](https://github.com/julymetodiev/post-cortex), long-term
memory for AI agents. It talks to the daemon's gRPC API (`pcx.v1.PostCortex`, default
`127.0.0.1:3738`).

```bash
pip install post-cortex        # Python 3.10+
pcx start                      # the daemon: brew install julymetodiev/tap/post-cortex
```

## Quick start

```python
from post_cortex import Client, Entity, Record, Relation

pcx = Client()  # PC_GRPC_TARGET, PC_AUTH_TOKEN and PC_AUTHOR from the environment
sid = pcx.create_session("my project")

entry = pcx.record(sid, Record(
    "decision_made",
    "Use SurrealDB for storage",
    "One shared server, one namespace per tenant",
    entities=[Entity("SurrealDB", "technology"), Entity("storage", "concept")],
    relations=[Relation("storage", "SurrealDB", "depends_on", "chosen backend")],
))

for hit in pcx.search("which database do we use?", session_id=sid):
    print(hit.score, hit.content)
```

Every record needs at least one entity and one relation, with both ends of each relation
among its entities. Writes return once stored; embedding and the graph merge follow on the
daemon in the background, so a record is searchable a moment later.

## Connecting

```python
Client("ml.example:3738", token="...", author="my-agent", secure=True, timeout=30)
```

- `token`: the daemon's bearer token (`auth_token` / `PC_AUTH_TOKEN`).
- `author`: who the records come from (an agent such as `my-agent`, or a person); search
  can filter or prefer by author.
- `secure`: TLS with the system roots (for a daemon behind a TLS proxy).
- `Client` is a context manager; `client.stub` is the generated stub for RPCs it does not wrap.
- Methods return the generated messages; `post_cortex.proto`, `views` and `blocks` are their
  modules (`pcx.v1`, `pcx.views.v1`, `pcx.blocks.v1`).

## What it covers

| Area | Methods |
|---|---|
| Sessions | `create_session`, `list_sessions`, `get_session`, `search_sessions`, `update_session`, `delete_session` |
| Workspaces | `create_workspace`, `list_workspaces` (`include_archived`), `get_workspace`, `update_workspace`, `archive_workspace`, `unarchive_workspace`, `delete_workspace`, `add_session`, `remove_session`, `resolve_project` |
| Records | `record`, `record_many`, `records` |
| Search | `search` (session, workspace or global; `mode` fast / local / accurate; `sources` records / messages / all; `author`, `prefer_author`, `trace`), `assemble` |
| Graph views | `entity`, `impact`, `lineage`, `topics`, `radar` (`include_dismissed`), `file_context` (each its `pcx.v1` view message) |
| Radar | `dismiss_radar_item`, `restore_radar_item`, `list_radar_dismissals` |
| Upkeep | `merge_entities`, `delete_entity`, `forget`, `purge` |
| Cards | `get_card`, `update_card` |
| Messages | `append_messages`, `delete_message`, `delete_messages` |
| Export / import | `export`, `export_chunks`, `import_archive` (the `pcx export` file format) |
| Summaries | `structured_summary`, `key_decisions`, `session_statistics` |
| Admin | `health`, `vectorize_session`, `create_checkpoint` |

### Projects

`resolve_project` finds or creates the workspace and session of a project key (a
normalised git remote, or `local:<hash>`), so every client on a repository writes to the
same place:

```python
p = pcx.resolve_project("github.com/acme/api", "api", client="my-agent")
pcx.record(p.session_id, ...)                  # p.created: this call bound the key
pcx.resolve_project("github.com/acme/web", "web", workspace="acme")  # join a workspace by name
```

### Facts that change

A record can replace older ones: pass `supersedes=[entry_id, ...]` (and optionally
`valid_from`, `confidence`). Search keeps the older record as history, its `validity`
set (`valid_to`, and `reason`: `views.SUPERSEDED`, `CONTRADICTED`, `RESTATED`,
`DUPLICATE` or `FORGOTTEN`), and ranks records ended this way lower. `forget` ends a record now and keeps
it; `purge` erases it with everything derived from it.

```python
new = pcx.record(sid, Record("decision_made", "Move storage to RocksDB", ...,
                             supersedes=[entry]))
hits = pcx.search("storage", session_id=sid, trace=True)
for h in hits:
    print(h.entry_id, h.HasField("validity"), h.validity.reason, h.signals)
```

### Graph views

```python
pcx.impact("SurrealDB", session_id=sid)        # who depends on it, and the risks
pcx.lineage("SurrealDB", workspace_id=ws)      # problem -> decision -> consequences
pcx.radar(workspace_id=ws)                    # unsolved problems, stale entities, aliases
```

Views return the daemon's JSON as Python objects. A radar item that is fine as it is can be
dismissed for the workspace; it then stays out of the radar of the workspace and its sessions
(listed under `dismissed`) until restored:

```python
pcx.dismiss_radar_item(ws, "stale_entity", "redis")
pcx.radar(workspace_id=ws, include_dismissed=True)
pcx.restore_radar_item(ws, "stale_entity", "redis")
```

### Export and import

`export` returns an `Archive`: the bytes of a `pcx export` file (`compression` none, gzip or
zstd) and a summary. `import_archive` sends such a file; without `commit=True` it only
previews (counts, conflicts with what the daemon has):

```python
archive = pcx.export("workspace", workspace_id=ws, compression="gzip")
other.import_archive(archive.data)                     # preview
other.import_archive(archive.data, commit=True, skip_existing=True)
```

### Errors

Failed calls raise `PostCortexError` (with `code`, the gRPC status), or one of its
subclasses `NotFound`, `InvalidArgument`, `Unauthenticated`.

## Development

```bash
uv run pytest            # starts a throwaway daemon: PCX_BIN, or ../../target/debug/pcx
../gen.sh python         # regenerate src/post_cortex/_pb from crates/post-cortex-proto/proto/pcx.proto
```

MIT licence.
