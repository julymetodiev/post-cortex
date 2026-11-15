# Operations

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8, operations: what the modules below share.

The modules that let the harness watch and mend a production system on its own, and replace
the Hermes setup that runs Equal's operations on `ml` (alert receiver, cron, Telegram, the
`eq-sentinel`, `eq-medic` and `eq-eng` profiles, `medic.py` and its L1 broker). They are
general: nothing in them names Equal; what Equal needs is configuration and skills. Every
one follows rule 10: gRPC with its own typed messages, between modules through generated
clients.

- *The outside world's JSON* (owner 2026-10-27, rule 10): an outside API that speaks JSON
  (Alertmanager's webhook, Telegram's Bot API, Prometheus, Loki and Alertmanager's HTTP APIs,
  Kubernetes) is mirrored by proto messages of the module's package, each field as the API
  names it, and its bodies are decoded and encoded only by the canonical proto3 JSON mapping
  of those messages (`pbjson`, unknown fields ignored), in the module's adapter for that API.
  No `serde_json::Value`, no hand-written mapping, no JSON past the adapter: the service,
  the domain and every call between modules see the typed messages. A library with its own
  typed client (`kube`) is used through its types and mapped to the module's messages in
  that adapter.
- *Credentials*: an API's token, a kubeconfig, a bot token are `secret` settings of the
  module that uses them (section 7, settings), never in a document, a prompt, a tool's input
  or answer, or a sandbox root; an agent never reads them. The module that holds a
  credential is the only way to use it: that is the boundary, not a prompt.

- *Roll-out for Equal* (the plan, not code): the modules run beside Hermes first. Alertmanager
  sends to both receivers; `eq-sentinel`'s role becomes a member that reads (`observe`), its
  diagnoses compared with Hermes's for two weeks in a Telegram topic of its own. Then `ops`
  takes `medic.py` and the L1 broker's actions (the same RBAC accounts), the cron jobs move to
  the scheduler, `eq-eng` becomes a member with worktrees, and Hermes is stopped.
- *Not in these waves*: an MCP client that would turn another server's tools into a module's
  (its tools are JSON Schema, not typed messages: rule 10 needs a decision on how first);
  Slack; database actions.
