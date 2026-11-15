# post-cortex-ports

The ports of [post-cortex](https://github.com/julymetodiev/post-cortex): the
traits the memory capabilities depend on and adapters implement (one storage
trait per capability, the vector store and full-text search, the reranker),
the types they speak (vector metadata, stored workspaces and checkpoints,
store errors with `Corrupt`), and the conformance suites every adapter runs
(feature `conformance`).

Depends only on `post-cortex-core`. An out-of-tree storage, embedding or
reranker adapter depends on this crate alone.
