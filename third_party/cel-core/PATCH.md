# cel-core 0.5.1, patched

The crate `cel-core` 0.5.1 (MIT OR Apache-2.0, https://github.com/ponix-dev/cel-core), as
crates.io published it, with patches: a budget of evaluation steps, which 0.5.1 lacks
(owner 2026-10-31, docs/modules/workflow.md, expressions), and three for speed, which change
no behaviour (owner 2026-11-01, docs/modules/workflow.md, data blocks, speed).

## Budget

- `eval/budget.rs`: `Budget`, the steps an evaluation may take.
- `eval/evaluator.rs`: every node visited spends a step, and every iteration of a
  comprehension one more; a spent budget ends the evaluation with an error of the new kind
  `EvalErrorKind::BudgetExceeded` (`eval/error.rs`), and ends a comprehension's loop at once.
  A child evaluator (a comprehension's, a bind's) spends from the same budget.
- `eval/program.rs`: `Program::eval_with_budget`; the evaluator's setup shared with
  `eval_with_container`.

## Member read

- `eval/evaluator.rs`, `eval_member`: the qualified name (`a.b`) is written and looked up
  only when the leftmost identifier does not resolve. 0.5.1 wrote it with `format!` for every
  member read, `item.name` of a resolving `item` too, and then dropped it. A leading-dot
  chain's leftmost never resolves, so its branch for a resolving leftmost could not be
  reached and is gone. Behaviour is the same; a transform over 100k records spends ~12% less.

## Map key lookup

- `eval/value.rs`: `ValueMap::get_str` and `contains_str` find a text key without making a
  `MapKey` (an `Arc<str>` allocated for each member read of a map in 0.5.1): a borrowed key
  (`KeyRef`, ordered as `MapKey` is) behind `Borrow<dyn Keyed>`. `eval/evaluator.rs` reads a
  map's member and `has()` through them.

## References by node

- `eval/references.rs`: `References`, a checked expression's references sorted by node id
  and found by a binary search. The evaluator looks one up for every node it visits; 0.5.1
  hashed the id with SipHash in a `HashMap` each time. `eval/program.rs` sorts them once when
  the program is made; `CheckResult::reference_map` stays as it was (cel-core-proto reads it).

Each patch goes upstream as a pull request.

The workspace uses it through `[patch.crates-io]`, so `cel-core-proto` gets it too; a workspace
that builds these crates by path (post-cortex-cloud) names the same patch, since a patch holds
only in the workspace that declares it. It is no
member of the workspace: its code is upstream's, outside this repository's rules (the file
sizes and lints of docs/ARCHITECTURE.md), except the patches. When a release has both, this
copy is removed.

Before a release that ships it, the licence texts of the upstream repository (LICENSE-MIT,
LICENSE-APACHE) are copied here: the crates.io package does not carry them.
