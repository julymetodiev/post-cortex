# Integrations

post-cortex ships to each coding agent through that agent's own catalog. `pcx` stays
the engine (login, profiles, the stdio MCP proxy, the hook logic); the packages here
are thin and installed once for every project, which each recognises by its git
remote (else its directory).

| Directory | Catalog | Status | Submission steps |
|---|---|---|---|
| [`claude-code/`](claude-code/README.md) | Claude Code plugin marketplace (`/.claude-plugin/marketplace.json`) | ready, `claude plugin validate --strict` passes | [README](claude-code/README.md) |
| [`cursor/`](cursor/README.md) | Cursor Marketplace plugin | ready; needs `/.cursor-plugin/marketplace.json` at the repository root and a local test in Cursor | [README](cursor/README.md#submitting-to-the-cursor-marketplace) |
| [`gemini/`](gemini/README.md) | Gemini CLI extension and gallery | ready, `gemini extensions validate` passes; needs a mirror repository (no subdirectory installs) | [README](gemini/README.md#publishing) |
| [`mcp-registry/`](mcp-registry/README.md) | Official MCP Registry (`server.json`) | ready, `mcp-publisher validate` passes; publish once `api.pcx.is` is live | [README](mcp-registry/README.md#publishing) |
| [`js/`](js) | npm | see below | |

- [`js/`](js): native plugins for OpenCode (`@post-cortex/opencode`), Kilo (`@post-cortex/kilo`), Pi (`@post-cortex/pi`), OpenClaw (`@post-cortex/openclaw`) and DeepSeek Harness (`@post-cortex/dsh`) on a shared core (`@post-cortex/plugin-core`); one npm workspace, `npm ci && npm run build && npm test` in `integrations/js`.

Nothing here has been published or submitted.

## Single source

Every copied or generated file comes from one place, and
`python3 integrations/sync.py --check` (CI job `integrations`) fails when a copy
differs:

| Source | Copied or generated into |
|---|---|
| `crates/post-cortex-cli/templates/skills/post-cortex-memory/` | the skill of `claude-code/`, `cursor/`, `gemini/` |
| `templates/pcx-*.py`, `templates/pcx_resolve.py`, `templates/settings.json` | `claude-code/hooks/` |
| `templates/clients/AGENTS.md` (the instructions `pcx setup` writes) | `cursor/rules/post-cortex.mdc`, `gemini/GEMINI.md` |
| `claude-code/.claude-plugin/icon.png` | `cursor/assets/logo.png` (the MCP Registry entry links to it) |
| `cursor/scripts/pcx.sh` (the pcx launcher for plugin hooks and MCP) | `gemini/scripts/pcx.sh` |
| `Cargo.toml` `[workspace.package] version` | every manifest's `version` |

Edit the sources, never the copies, then run `python3 integrations/sync.py`.

Tests:

```bash
python3 -m unittest discover -s integrations/tests -v          # project resolver, Claude Code hooks
python3 -m unittest discover -s integrations/cursor/tests -v   # pcx launcher, Cursor/Gemini/registry manifests
```
