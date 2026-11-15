# post-cortex for Cursor

A Cursor plugin that gives Cursor's agent [post-cortex](https://post-cortex.com), a
long-term, searchable project memory. It bundles:

- **MCP server** `post-cortex`: post-cortex cloud by default, or your own daemon.
- **Rule** `post-cortex` (always applied): search the memory before answering, log
  decisions, fixes and changes after.
- **Skill** `post-cortex-memory`: when and how to read and write the memory.
- **Hooks**: the project's session, workspace and cards when a chat starts, what the
  memory knows about a file right after the agent reads it, and (opt-in) the
  conversation turns.

The plugin is installed once and recognises every project by itself (its git remote,
else its directory). Nothing is written into your repositories.

## Install

1. Install `pcx` and sign in once (the MCP server and the hooks run through it):

   ```bash
   brew install julymetodiev/tap/post-cortex     # or: cargo install post-cortex-daemon
   pcx login                                     # paste an API key from https://app.pcx.is (Connections → API keys)
   ```

2. In Cursor open **Customize**, find **post-cortex** under Plugins and choose
   **Install**. Pick **user** scope for memory in every project, or **project** scope
   for one.

3. Start a new chat. The first line of context reads
   `[PCX] Session: ... | Workspace: ...`: the memory of this project.

Self-hosted instead of the cloud: `pcx start`, then `pcx profile use self`. The MCP
server and the hooks follow the active profile, so nothing in Cursor changes.

## How it connects

The MCP entry runs `pcx mcp --author cursor` through `scripts/pcx.sh`, a stdio
server that forwards to the active profile's `/mcp` with the key `pcx login` stored
in `~/.post-cortex/credentials.json` (mode 0600). Why this and not an HTTP entry:

| Option | Works today | Trade-off |
|---|---|---|
| **stdio `pcx mcp`** (used) | Every plan, macOS and Linux | Needs `pcx` installed; it is needed for the hooks anyway. No secret in any Cursor file, one key for every client, cloud or self-hosted by `pcx profile use`. |
| HTTP `url` + `Authorization: Bearer ${API_TOKEN}` plugin variable | Teams and Enterprise only | Plugin `variables` are set by team admins in the dashboard (Plugins > Configure); an individual user has no place to enter the key. |
| HTTP `url` + `${env:PC_AUTH_TOKEN}` | Unreliable | The plugin docs say plugin placeholders are not shell `${env:...}`; a GUI app does not see variables exported in a shell profile. |
| HTTP `url` with OAuth | Not yet | Cursor signs in to OAuth MCP servers by itself (Customize > server > Authenticate). The post-cortex gateway has no OAuth yet; once it does, `mcp.json` becomes a bare `url` and `pcx` is needed only for the hooks. |

`scripts/pcx.sh` looks for `pcx` on `PATH`, then in `~/.cargo/bin`,
`/opt/homebrew/bin`, `/usr/local/bin`, `/home/linuxbrew/.linuxbrew/bin` and
`~/.local/bin` (GUI apps start servers with a minimal `PATH`). `PCX_BIN` points it at
another binary. Without `pcx` the MCP server stops with an install hint in the MCP
log, and every hook exits silently.

## Hooks

| Cursor event | `pcx hook cursor ...` | What it adds |
|---|---|---|
| `sessionStart` | `sessionStart` | The project's session and workspace ids, the user and project cards |
| `postToolUse` (`Read`) | `postToolUse` | What the memory knows about the file just read, before the edit (`preToolUse` cannot add context) |
| `beforeSubmitPrompt` | `beforeSubmitPrompt` | Capture only: the prompt |
| `afterAgentResponse` | `afterAgentResponse` | Capture only: the reply |

All hooks run in global mode (no ids on the command line): `pcx` resolves the project
from the payload's workspace root, first hit wins:

1. `.post-cortex.json` at the git root (`pcx setup --project`; commit it to pin the ids
   for every teammate);
2. the ids in the project's `AGENTS.md`;
3. the cache `~/.post-cortex/projects.json`;
4. `POST /api/projects/resolve` with the project key: the `origin` remote normalised
   (`github.com/owner/repo`), else a hash of the path. Teammates on the same remote
   share the memory.

Per-client settings in `~/.post-cortex/config.json` under `hosts.cursor`:

| Key | Default | |
|---|---|---|
| `enabled` | `true` | `false` turns the hooks off |
| `capture` | `false` | `true` posts the prompts and replies to the session |
| `workspace` | none | the workspace a new project joins instead of its own |

The hooks step aside so nothing runs twice: in a project with its own pcx hooks
(`pcx setup --project` wrote `.cursor/hooks.json`), and when `pcx setup` (global
mode) already put pcx hooks in `~/.cursor/hooks.json`. `pcx setup --client cursor`
also writes a `post-cortex` server to `~/.cursor/mcp.json`; with the plugin installed
you do not need it, remove one of the two.

## Limits

- **Windows**: the MCP server and the hooks run `sh`. Without Git Bash or WSL on
  `PATH` the server does not start and the hooks fail open (Cursor carries on).
- **Cloud agents** load project, team and enterprise hooks only (the hooks docs list
  no plugin or user-level hooks there), and they have no `pcx` login.

## Development

Generated in part by `integrations/sync.py` (see its docstring):

| File | Source |
|---|---|
| `skills/post-cortex-memory/` | copied from `crates/post-cortex-cli/templates/skills/` |
| `rules/post-cortex.mdc` | `templates/clients/AGENTS.md` without ids, as `pcx setup` writes it for a global setup |
| `assets/logo.png` | `integrations/claude-code/.claude-plugin/icon.png` |
| `.cursor-plugin/plugin.json` `version` | the workspace `Cargo.toml` version |
| everything else (`plugin.json` metadata, `mcp.json`, `hooks/hooks.json`, `scripts/pcx.sh`, this README) | hand-written; `scripts/pcx.sh` is also copied into the Gemini CLI extension |

```bash
python3 integrations/sync.py           # regenerate
python3 integrations/sync.py --check   # CI: fails when a copy differs
python3 -m unittest discover -s integrations/cursor/tests -v
```

Try it without the marketplace: link this directory to
`~/.cursor/plugins/local/post-cortex` (`ln -s "$PWD/integrations/cursor"
~/.cursor/plugins/local/post-cortex`), then **Developer: Reload Window**. The
**Hooks** tab in Customize and the Hooks output channel show what ran.

## Submitting to the Cursor Marketplace

Nothing has been submitted. Cursor lists plugins from public Git repositories and
reviews each submission and each update by hand
([docs](https://cursor.com/docs/reference/plugins.md#submitting-a-plugin)).

1. Add the multi-plugin manifest at the repository root, since this plugin lives in a
   subdirectory. `/.cursor-plugin/marketplace.json`:

   ```json
   {
     "name": "post-cortex",
     "owner": { "name": "Julius ML" },
     "metadata": {
       "description": "post-cortex: long-term, searchable project memory for coding agents"
     },
     "plugins": [
       {
         "name": "post-cortex",
         "source": "./integrations/cursor",
         "description": "post-cortex MCP server, rule, skill and memory hooks"
       }
     ]
   }
   ```

2. Make sure `https://post-cortex.com` and `https://api.pcx.is/mcp` are live
   (reviewers install the plugin), then push to the public repository.
3. Submit `https://github.com/julymetodiev/post-cortex` at
   [cursor.com/marketplace/publish](https://cursor.com/marketplace/publish).

Checklist from the docs, all met: `.cursor-plugin/plugin.json` with a unique
kebab-case `name`; a description; valid frontmatter in the rule and the skill; the
logo committed and referenced by a relative path; this README; no `${VAR}` that would
need a `variables` schema (`${CURSOR_PLUGIN_ROOT}` is Cursor's own); relative manifest
paths without `..`. Still to do by hand: a local test in Cursor (see Development).
