# Memory

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): sections 4 to 6, the memory module.

## Capabilities (section 4)

`post_cortex_memory::Memory` is the memory API: one cheap-to-clone service per capability
(`memory.records`, `memory.search`, ...), all built by `Memory::over_ports(MemoryPorts, SystemConfig)` from the ports the daemon's
composition opened (section 5). Each user-facing operation is one method on one
service, with that operation's validation, and each service has one `thiserror` error enum
(`RecordError`, `SearchError`, ...). Transports, `pcx` and the cloud call these methods and
nothing else of the crate.

| Capability | Operations | Ports used |
|---|---|---|
| Records | write, bulk write (atomic: a bad item writes nothing; ids in input order), validate and validate bulk (the write's checks, nothing stored: dry runs), list (by wire type and time, with the total and source references), delete, forget, purge; the commit (record and graph merge in one swap and one persist) and the hand-off of derived work to the bounded, per-session derived-work queue; each commit is published to ChangeLog. Writes never create sessions | SessionStore, MessageStore, FreshnessStore (list), SearchIndex (derived work) |
| Search | one `search(SearchScope)` pipeline for global, session, sessions and workspace scope (scope resolved once, records without vectors embedded first, the retrieval graph in session scope, hits completed from their sessions, message streams and documents); an accurate search uses the remote reranker (Voyage) only when every workspace of its scope allows it (owner 2026-11-06: a workspace search its workspace, a session search every workspace the session is in and never a session in none, a global search every workspace and every session in one), else the local one; related content | SearchIndex, SessionStore, MessageStore, DocumentStore |
| Assembly | assemble context (cards, entity context, impact, records, conversation excerpts through Search); refuses an empty query | Search, Cards, SessionStore |
| Views | graph (nodes and edges with provenance and validity, as of a time), timeline (record and relation events in a window), records (the browser filter: types, authors, entity, text, validity, time), record (with provenance: the turns it was derived from, what it supersedes and what ended it, its entities and edges), overview (counts, recent records, top entities, topics, pending vectorization), entity (view, impact, lineage, rank, topic), impact, lineage, topics, radar, files (filtered by path text) and file (the files a record names: its code reference and, for a code change, the paths its `file` lists, read once by one function), conversations (the message streams of a scope with their stats and authors, every stream's from its summary in one read (owner 2026-10-31: no message is read to list them; the authors are all the stream's); with a speaker, the newest turns of the streams it spoke in; pages of a stream, each turn with the records drawn from it), search as the app shows it (hits grouped with their neighbours, every record and turn in full); structured summary, key decisions/insights, statistics; radar dismissals. The lists that grow without bound (records, files, conversations) are pages, newest first, with a total and one opaque keyset cursor (`<seconds>.<nanoseconds>_<key>`: a record, a path, a session; one type, one paging function), so a page never repeats or skips an item when newer ones arrive. Every view of one call reads one index of its scope, built once (validity, edge provenance, importance, communities), off the async runtime; a scope (a session, a workspace, or every session) is resolved once, with the merged-graph cache | SessionStore, DismissalStore, Messages, Search |
| ChangeLog | subscribe to the changes of a scope: record written, forgotten or purged; message appended or deleted; card version written; document version written or document deleted; workspace created, updated, archived or deleted; membership added or removed. A bounded in-process feed per memory instance; a subscriber that falls behind is told so and re-reads | none (fed by the capabilities that commit) |
| Events | (owner 2026-10-31, `docs/host.md` events of a module) the organisation's bus: append an event the host stamped (its id, name, module, workspace, time, message as `Any`) in one ordered log; read the events after a cursor by name; keep a follower's cursor per name (written from the cursor read); drop events older than 7 days. Reached only by the host through the composition, never by a tool or a client | EventLog, EventCursors |
| Entities | delete, merge | SessionStore |
| Sessions | create, get, list, find, update metadata, delete, checkpoint | SessionStore, CheckpointStore |
| Workspaces | create, get, list, update, archive, delete (its blobs first), membership. An archived workspace is read-only (owner 2026-11-15: the docs said so and nothing held it): a call that changes memory (the operation table's write and delete effects) and names it, a session of it or a project placed in it is refused (`post_cortex_memory::archive`, `Memory::open_to`: FailedPrecondition; REST 409), on every way in (gRPC, REST, the browser's uploads, MCP), with a reach or without one; restoring it and deleting it are the two changes it takes; reads and searches go on. A workspace has a slug, the segment of its addresses (owner 2026-11-06): made when it is created from its name (`post_cortex_core::slug::workspace_slug`: ASCII letters and digits joined by `-`, at most 48; with the first six hex digits of its id when another workspace has that slug), unique, and kept after a rename; an import keeps an archive's slug unless another workspace has it, and gives an older archive's workspaces theirs the same way. A workspace allows or forbids the remote reranker (`remote_rerank`, owner 2026-11-06): forbidden when it is created and in older data and archives, changed only by `update` (gRPC `UpdateWorkspace`, the app's settings; not MCP) | WorkspaceStore, BlobStore |
| Projects | resolve (through Workspaces and Sessions), find | WorkspaceStore |
| Cards | get, update (versioned), history; the user card is the call's person's (`x-pcx-person`, docs/host.md): `person:<id>` for a call for a person, the host's own (`user`) for a call for none (a local daemon, a key's agent); a full export carries the host's and every person's (`CardStore::card_people`), format 1.10.0 (owner 2026-11-09) | CardStore |
| Documents | a module's documents (section 7, modules together): put (versioned: `expected_version`, a conflict answered as cards answer it; the data checked against its kind's schema; only the owning module writes; W24: every `pcx.v1.Attachment` in the data names a blob its workspace has, else the write is refused), get (the latest, or a version), history, list (by kind, the most recently changed first, paged by storage; a filter names top-level singular scalar fields of the kind's message equal to an example's, any other field refused, and is answered from the document's keys, below), delete; the latest version's title and text in the search index, earlier ones only in the history | DocumentStore, SearchIndex, BlobStore |
| Blobs | (W24) put (the bytes and their media type, checked against their first bytes: PNG, JPEG, WebP or GIF; or a module's message, `application/x-protobuf` as its putter declares it in the first chunk (owner 2026-10-31: a workflow step's large output); at most 10 MB; answers the SHA-256, size and media type, and the width and height of an image), get, stat; a blob is kept while a document of its workspace names it (by a `pcx.v1.Attachment` anywhere in its data, in any version; owner 2026-10-26): the sweep deletes the others once they were put more than a day ago (the daemon runs it daily; a workspace with a document whose kind no running module declares is left as it is), and deleted with its workspace; the RPCs of `pcx.v1.Blobs` (`PutBlob`, `GetBlob`, `StatBlob`; a service of its own beside `PostCortex`, for modules and clients) and the browser transfer (`POST /v1/blobs?workspace_id=` with the image as the body, as import's, and `POST /v1/blobs/link` for a single-use download link) | BlobStore, DocumentStore |
| Messages | append (stores the turns, checks the session without loading it, queues their embedding), page, stats (of one stream, or of many in one read: count, first and last position with their times, the authors, from a summary every write of a stream keeps in its own transaction; a stream written before the summary existed gets it from its rows the first time it is read), delete, retention | MessageStore, SessionStore |
| Transfer | export (records the embedding model), preview, import (refuses another model's vectors before writing) | all stores, SearchIndex, ArchiveCodec |
| Freshness | source registration, staleness, invalidation scoped to a session; one state machine (registered, stale, re-registered, orphan) persisted identically by every adapter | FreshnessStore |
| Admin | health (the one `Health` payload of every transport), warm-up, vectorization passes and stats, query cache, shutdown flush | all |

`Radar` as its own service comes with its first consumer; dismissals are part of Views today.
The change feed's first consumers are the app's live views (`WatchActivity`) and the local
mirror.

## Ports and runtime state (section 5)

- **Ports live in `post-cortex-ports`.** Capabilities (`post-cortex-memory`) depend on that crate
  and never on an adapter crate; the daemon (composition root) opens the adapters named in the
  configuration and hands the capabilities `Arc<dyn Port>` values. The compiler enforces the
  direction: `post-cortex-memory` has no dependency on `post-cortex-storage` or
  `post-cortex-embeddings` outside `[dev-dependencies]`.
- **The host's ports live in `pcx-host`** (owner, 2026-10-24). The module host is a hexagon of its
  own beside memory: its ports (`Settings`: where module configurations are kept and reloaded) are
  traits of `pcx-host`, implemented by the daemon (`daemon.toml`) or a hosted tenant, and never
  depend on `post-cortex-ports`; memory's ports never depend on the host.
- **Storage ports** (one trait each): `SessionStore` (the session, its records and its entity
  graph: they are written in one batch, so one port), `WorkspaceStore`, `MessageStore`,
  `CardStore`, `DocumentStore` (W14: a module's documents and their versions), `DismissalStore`,
  `CheckpointStore`, `FreshnessStore`; `ChangeLog` (local mirror)
  is added with its first consumer. Every port returns the typed `PortError`.
- **Document keys and pages** (owner 2026-11-06, task `f47c5546`): a list of documents reads
  only the page it answers, filtered or not, however many documents the kind has.
  - *Keys*: when it writes a version, memory gives the document a key for every top-level
    singular scalar field of its kind's message (string, bytes, bool, a number, an enum),
    default values included: `<field number>:<the first 16 bytes of the SHA-256 of the
    value>` in hex, the value hashed as its UTF-8 (string), its bytes (bytes), one byte
    (bool), eight bytes big endian (an integer, a float's bits) or its number as an `i32`
    (enum). The hash keeps a key short whatever the value. Only the latest version has keys.
  - *Pages*: `DocumentStore::latest_page` answers the latest versions of a kind that have
    every key named (none: all of the kind), the most recently changed first (by the time
    of the latest version descending, then by the id), at most a limit after a cursor,
    with how many match and the next cursor. Memory turns a filter into the keys of its
    example and checks the page against the filter again (a hash that collides).
    `latest_documents` stays for what reads every document (export, the blob sweep).
  - *Unkeyed*: a latest version stored without keys (written before keys, or imported) is
    unkeyed. A page with keys also holds
    the unkeyed documents of its kind, in their place, and memory checks each against the
    filter, so a document not keyed yet is never missed (its page may then hold fewer than
    the limit, and the total counts it). `unkeyed_documents` lists them; at start memory
    keys each (`set_document_keys`) after converting JSON data to
    its message (the one conversion of export format 1.7.0, which the same step replaces),
    so a start after the first reads no document.
  - *Stored*: RocksDB keeps `doc_order:{ws}:{kind}:{inverted ns}:{id}`,
    `doc_key:{ws}:{kind}:{key}:{inverted ns}:{id}`, `doc_keys:{ws}:{id}` (the latest's keys,
    to remove them) and `doc_unkeyed:{ws}:{kind}:{inverted ns}:{id}`; data version 4 adds the order of the
    documents already stored, all unkeyed. SurrealDB keeps small rows beside `document`:
    `document_order` (one per document: workspace, kind, id, version, inverted time),
    `document_key` (one per key of the latest) and `document_unkeyed` (as the order), each read through
    an index on the fields a page names and ordered in the adapter; data version 3 does
    the same. Both write them in the transaction that changes the latest version.
- **`BlobStore`** (W24; owner 2026-10-26): bytes by their SHA-256, per workspace (key
  `<workspace>/<sha256>`): put (an existing blob is not written again), get, stat (size,
  media type, width, height and when it was put, kept as the object's attributes), list (a
  workspace's blobs, for the sweep and the workspace's delete), delete. Its adapter is S3
  (crate `post-cortex-blobs`, any S3 endpoint over `object_store`; a self-hosted RustFS
  for now), apart from the store of records and documents, so
  RocksDB and SurrealDB keep no bytes. The daemon's configuration names the endpoint, bucket
  and region (`[blobs]`); the keys come from the environment (`PCX_BLOBS_ACCESS_KEY`,
  `PCX_BLOBS_SECRET_KEY`), never `daemon.toml`. Stores that share a bucket put their keys
  under a prefix (owner 2026-10-26: the cloud keeps one bucket for the platform, each
  tenant's blobs under `<tenant>/`); a daemon's has none. Without `[blobs]` memory keeps no blobs and a
  document that names one is refused. Its conformance suite runs in memory and, by hand, on S3.
- **`DocumentKinds`** (W14): the document kinds the running modules declare, as core
  `KindDeclaration`s (kind, data schema, title field); memory's `MemoryModule` adapts it over its
  host handle (the manifests). Its typed error is `KindsError` (not ready before the start, host
  unavailable, a declaration that does not read). Documents check ownership and schema against it.
- **A save carries its changes.** `SessionStore` saves a session snapshot with a
  `SessionChanges`: the record rows to write and to delete, whether the whole entity graph
  changed, and the record ids embedded and no longer embedded since the last save. A record
  changes only the entities it names and the edges it states (mentions are derived from the
  records at load), so a save writes the rows of those entities and edges, the records' rows and
  the vectorized-id keys, and nothing else: its cost does not depend on the session's size.
  Operations that change the graph otherwise (entity delete and merge, purge, freshness
  rebuild) mark the whole graph changed and the adapter rewrites the session's graph rows. The
  persister merges the changes of coalesced requests and keeps those of a failed write for the
  retry. Records are stored in session order under an ordinal; there is no index row.
- **`SearchIndex`** is the one retrieval port. An index entry is (session, content id, kind,
  text, metadata, vector); the tenant is the store itself, so an entry has no wider scope; a long record is several chunk entries `<id>#n` of one content
  id. Operations:
  - `upsert(entries)` and `delete(session, content ids)` (a content id deletes its chunks);
  - `entries(session, filter)`: the stored entries of a session, by content ids or kinds, with
    text and metadata (used by graph records, re-embedding checks, the missing-record pass and
    Transfer);
  - `nearest(query vector, scope filter, k)` and `score(query vector, session, content ids)`
    (scores of given candidates, used for graph candidates);
  - `lexical(query text, scope filter, k)`: BM25 over the same entries; the adapter selects the
    query terms (stopwords, document frequency); hits carry no similarity, the capability gets
    it with `score`;
  - `stats()`: counts per session and the load report (entries loaded, rejected for another
    dimension), which health shows and which fails the open when only another model's vectors
    exist. How it indexes and persists is the adapter's choice: the in-memory index of
  `post-cortex-embeddings` over a storage adapter's persisted entries, its vectors and its BM25
  postings, for every backend (owner 2026-11-01: SurrealDB's own full-text index took a round
  trip per matching row on TiKV, past every search's deadline, and is removed); an external
  vector store later. The Search capability keeps only ranking: rank
  fusion (RRF), reranking, graph and recency signals, dedup and order. There is no separate
  `VectorStore` port for the capabilities.
- **`ArchiveCodec`** writes and reads the export archive one part at a time (`ArchivePart`:
  the header, then each session, workspace, checkpoint, vector and card), so no side holds a
  whole archive. The archive model (sessions,
  records, checkpoints, workspaces, cards, dismissals, messages, index entries, embedding model)
  is plain data in core (checkpoints included, so `SessionCheckpoint` is a core type);
  the codec adapter in `post-cortex-storage` owns format versions, the JSON schema and
  compression; the Transfer capability owns what goes into an archive and the import merge rules.
- **Configuration.** The daemon's configuration chooses and configures the adapters (storage
  backend, SurrealDB connection, data directory, model and reranker files, ONNX runtime path,
  API keys, embedding model and dimension); the memory `SystemConfig` holds capability settings
  only. Adapters that load models load them on first use (T1). The `EmbeddingEngine` port has
  `model()` (model id and dimension, known without loading) and `embed(texts)` (one batch); the
  Transfer capability records `model()` in an archive, and the `SearchIndex` adapter checks stored
  dimensions against it at open. The admin commands (`pcx export`, `import`,
  `vectorize-all`) go through the running daemon's gRPC API; without a running daemon (and
  `migrate` always) they run offline: the `pcx` binary composes memory and the module host
  over the stores, without servers, calls Transfer or Admin, and stops the host.
- **One port error.** Every port, storage or model, returns `PortError` with kinds NotFound,
  Conflict, Corrupt, Unavailable.
- **Shared pure helpers** used by several adapters (e.g. the search tokenizer) are domain
  functions in core.
- A backend builds one struct that implements the ports it supports and hands each capability an `Arc<dyn Port>`;
  no enum delegating by hand. `CheckpointStore` is keyed by session and deleted with it;
  `SearchIndex` upserts by (scope, session, content_id). Every port ships a conformance suite run
  against every adapter.
- **Runtime state** (`MemoryRuntime`, crate-private, one per memory instance, shared by the
  services by `Arc`): the session cache (with the per-session derived state it evicts with the
  session: validity, retrieval graphs), the workspace registry, the bounded merged-graph cache,
  the project/workspace locks, the persister, the derived-work counter, the request counters
  and the lazily built vectorizer with its models. It does not hand the ports
  out: each service holds the ports of its row in section 4 (rule 4). It keeps only
  the helpers several services share: the session lookups (`get_session`, `list_sessions`), the
  compare-and-swap loop of session changes, the vector cleanup of removed content, the
  vectorizer start, and the embedding passes that the pipeline, Search, Admin and Transfer all
  run (`vectorize_and_commit`, `vectorize_updates_now`, `vectorize_messages_now`,
  `vectorize_session_updates`, `vectorize_session_messages`, the missing-record pass,
  `flush_all`). No runtime method exists for a single service: an operation's body is that
  service's method.
- **Faults**: breakers sit at the boundaries, not in memory: the host keeps one per module
  (failures in a row refuse its calls for a while; a module process that exits is restarted
  with a backoff) and each storage adapter one for its backend (its failures in a row make
  every port call `Unavailable` at once for a while).
- **Write path**: one writer per tenant store (T10). The `SessionPersister` (ordering and
  coalescing per session) is the only queue of session writes; there is no generic storage
  actor, because the ports are already `async` and `Send + Sync`.
- **Derived work**: the embedding of a written record and of appended turns is queued on the
  bounded pipeline (sessions sharded over a fixed pool of workers, a batch per session at a
  time); a full queue makes the caller wait, then do the work itself, never spawn a task per
  item. `wait_idle` and the shutdown flush wait for it. Which records have vectors is still the
  session's shared `vectorized_update_ids` set, persisted with the session as one key per id
  (the set's additions and removals since the last save travel in `SessionChanges`): the
  missing-record pass after a restart reads it.
- **Runtime state stays out of core**: `ActiveSession` is a record list plus derived indices;
  the caches and registries are owned, per-instance memory state (above);
  adapters hold no per-session write cache (the persister owns "what is stored").
- **Embeddings**: one engine per process, shared through the `EmbeddingEngine` port that the
  composition builds once (`compose::Models`), loaded lazily (T1, T3); the
  model is a strict enum in the configuration and the vector dimension must be its own (checked
  at open). A startup load that fails or finds only another model's vectors fails the vectorizer
  start instead of searching an empty index. Vectors are L2-normalised at write; similarity is a
  SIMD dot product. The engine and rerankers run on a bounded blocking pool with the caller's
  deadline and one semaphore for every backend; nothing on the request path loads the model
  (capture enqueues).

## Operation table (section 6)

The single list of operations, their capability method, and which transports expose them. It is
data in `post_cortex_memory::tools` (one `Operation` entry per row) used by the transports and by a
parity test, and rendered into the tools reference. Each operation has one serde response shape
used by every transport and by `select`; MCP failures always set `isError`. Each entry also states
its effect (read, search with or without a reranker, write of records, messages, cards or
workspaces with a count, delete) and which of its fields name sessions and workspaces, so a
gateway in front of many memory instances classifies a request from the table instead of a
copy of it.

The table also lists the `pcx.views.v1` RPCs (`views` column; their scope is the request's
`scope` message). `post_cortex_transport::classify` reads a request of any transport the way
its transport does, so a gateway holds no parsing of its own:

- an MCP tool call (tool and arguments), a gRPC call (method and message, decoded through the
  descriptor sets of `pcx.v1` and `pcx.views.v1`, `PCX_DESCRIPTOR_SET` and
  `VIEWS_DESCRIPTOR_SET`) or a REST request (method, path, query and JSON body; path parameters
  are named after the scope field they fill) gives a `Classified`: the operation, the session and
  workspace ids it names, the project key it resolves (`project.resolve`, `project.find`),
  whether it asks for the reranker, and what it counts (fixed amounts, or the answer field to
  read with `answer_count`, `grpc_answer_count` or `mcp_answer_count`);
- every transport writes a request's scope the same way: the table's fields, or a `scope` of
  `session`, `workspace` or `global` with its id in `scope_id` (MCP) or `id` (REST);
- a malformed id, an unknown tool, action, RPC or route, or a request that does not decode is a
  `ClassifyError`, never a dropped value or a default; an import with `settings.commit` false
  counts as a read;
- `browser` classifies the browser transfer (`POST /v1/import`, `POST /v1/export`) as the
  `Import` and `Export` RPCs it maps to; behind a gateway an instance's download links point under
  the path the gateway routes to it (`BrowserAccess::Proxied { link_base }`);
- `host` reads a call of the host's generic gRPC service `pcx.module.v1.Host` (`None` for
  another service): `ListTools` is a read, `Invoke` names a tool and its input, classified as the
  MCP call of that tool (`mcp`; an `UnknownTool` is a module's tool); `host_answer` gives the
  data of an `Invoke` answer (`None` when the call failed) and `host_without_rerank` rewrites an
  invoked search; an `Invoke` that names no author is the `x-pcx-author` header's, as over MCP;
- `without_rerank` rewrites a request to run without its reranker (the rerank field set to its
  `fallback`, `fast`), for a gateway whose plan refuses the reranker.

```
operation          capability::method         mcp tool/action                grpc                rest
write              records::write             update_conversation_context    UpdateContext       -
search             search::search             semantic_search                SemanticSearch      POST /api/search
assemble           assembly::assemble         assemble_context               AssembleContext     -
view               views::view                query_conversation_context     GetGraphView        GET /api/file (file)
views.record       views::record              query_conversation_context record  (views GetRecord) -
views.records_of   views::records_of          -                              (views GetRecords)  -
card.update        cards::update              manage_workspace update_card   UpdateCard          PUT /api/cards/*
project.resolve    projects::resolve          session resolve_project        ResolveProject      POST /api/projects/resolve
...                (every operation)
```

A search finds and a read takes (owner 2026-10-25): MCP `semantic_search` shows a long result
as its passage for the query (`detail` snippet, the default: about 400 characters, the window
of the text that holds the most of the query's words, `post_cortex_core::text::passage` over the
same words BM25 indexes; `full_chars` its whole length; a long title likewise, at a quarter of
that), and only the hits it found; with `detail` full, every text whole and the records and
turns around each hit (`neighbour_of`). The ranking is the same either way; a hit's scores show
three decimals. A record a snippet cut is read whole with `query_conversation_context`
`record` (`entry_id`), as gRPC `GetRecord` reads it. REST and gRPC searches show the whole text.

## Crates in detail (section 3)

Published crate names stay (`post-cortex-*` are on crates.io); responsibilities change.

| Crate | Layer | Owns |
|---|---|---|
| `pcx-module-api` (new) | API | Module trait, manifest, `Scope`, typed events and outcomes, tool specs, capability flags, API version. Semver'd; no dependency on any memory crate |
| `pcx-host` (new) | host | registry and discovery (one path), slots, policy (deadlines, fail-open/closed, panic catch), event dispatch, hot reload, out-of-process runner (gRPC over UDS / named pipe; JSON hook-script runner), tracing per module |
| `post-cortex-core` | domain | pure memory domain: records (`ContextUpdate`), entities and the entity graph, validity (`graph::temporal`), analytics, messages, cards, dismissals, freshness (source references and staleness), redaction, summaries, DTOs of the memory capabilities. Stateful types leave core (section 5) |
| `post-cortex-ports` (new) | ports | the port traits: storage ports per capability, `SearchIndex` (vector and lexical retrieval), `ArchiveCodec` (export file format), embedding engine, reranker, clock; the types they speak (index entries and filters, port errors: one `PortError` with kinds NotFound, Conflict, Corrupt, Unavailable) and the shared conformance suites. Depends only on `post-cortex-core`; no IO crate, no adapter, no `anyhow` in a port signature. Third-party adapters (out-of-tree storage, index, embedding or reranker modules) depend on this crate alone |
| `post-cortex-embeddings` | adapter | implements the embedding engine, reranker and `SearchIndex` ports: embedding backends (model loaded on first use), rerankers, kernels, and the in-memory `SearchIndex` (contiguous normalised vector arena with exact scan, ANN only behind a measured recall gate; BM25 lexical index) built over a storage adapter's persisted entries. No PQ until a trained int8 path is designed |
| `post-cortex-storage` | adapters | RocksDB and SurrealDB adapters of the storage ports (SurrealDB may also serve `SearchIndex` natively), their on-disk rows and migrations; the `ArchiveCodec` adapter (export format versions, JSON schema, compression) |
| `post-cortex-memory` | capabilities + module | the memory capability services, their runtime state (session cache, workspace registry, write persister, pipelines), and `MemoryModule` (the module that plugs memory into the host) with its tools (`tools`: argument types and schemas, handlers, the operation table, response shapes, `select`, the failure classification and the shared wire parsers) |
| `post-cortex-transport` (new, from daemon) | adapters in | MCP (rmcp) over the host's tools, the gRPC status of a memory failure; gRPC (tonic, `pcx.v1`, `pcx.views.v1`) and REST (axum) handlers: thin mappings onto capability methods; the browser layer (gRPC-Web, CORS with an origin allowlist) and browser transfer (HTTP upload into import, single-use export download) |
| `post-cortex-daemon` | composition | the `pcx` binary: wires host + modules + transports, config, the daemon process; admin commands; `compose`, the public composition API: opening memory over the configured adapters, the models, the host, and the routes a memory instance serves (the daemon and every hosted tenant serve the same routes from it) |
| `post-cortex-cli` (new, from `src/bin/pcx`) | client | profiles, login, clients/setup, hooks, mirror, card/session/workspace commands; talks to a daemon or the cloud over the public APIs only, never opens storage itself |
| `post-cortex-proto` | API | `pcx.v1` wire schema; `pcx.views.v1` (the typed views of the app); `pcx.blocks.v1` (a message's blocks, which both share with the module protocol); `pcx.module.v1` module protocol (the single source for module types in every language) |
| `post-cortex` | facade | re-exports for library users |

Removed: `post-cortex-mcp` as a logic layer (its operations become capability methods, its
tools become `MemoryModule`'s), the legacy SSE `DaemonServer` (`daemon/server/`,
`sse.rs`), and the `StorageBackend` hand delegation.
