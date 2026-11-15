# post-cortex for Gemini CLI

A Gemini CLI extension that gives Gemini CLI [post-cortex](https://post-cortex.com),
a long-term, searchable project memory. It bundles:

- **MCP server** `post-cortex` (HTTP): post-cortex cloud by default, or your own
  daemon.
- **Context file** `GEMINI.md`: the memory rules, loaded in every session.
- **Skill** `post-cortex-memory`: when and how to read and write the memory.
- **Hooks** (when `pcx` is installed): the project's session, workspace and cards at
  session start, what the memory knows about a file right after `read_file`, the
  rules before each turn and (opt-in) the conversation turns.

Installed once for every project; each project is recognised by its git remote, else
its directory. Nothing is written into your repositories.

## Install

```bash
gemini extensions install https://github.com/julymetodiev/post-cortex-gemini
```

(The extension is not published yet; see [Publishing](#publishing) for where that
URL comes from. From a checkout of this repository today:
`gemini extensions install ./integrations/gemini`, or `gemini extensions link
./integrations/gemini` while developing.)

The install asks for one setting:

| Setting | Env var | Notes |
|---|---|---|
| post-cortex API key | `PC_AUTH_TOKEN` | Sensitive: kept in the system keychain. A cloud key from https://app.pcx.is (Connections → API keys), or your daemon's `auth_token`; empty for a local daemon without one. Change it with `gemini extensions config post-cortex`. |

For the hooks, also install `pcx` (they are skipped without it):

```bash
brew install julymetodiev/tap/post-cortex     # or: cargo install post-cortex-daemon
```

The hooks get the key from the extension setting; without it they use the key
`pcx login` stored.

### Self-hosted

The server URL is `${PC_URL:-https://api.pcx.is}/mcp`: export `PC_URL`
(e.g. `http://127.0.0.1:3737`) in the shell Gemini CLI starts from, and run
`pcx profile use self` so the hooks talk to the same daemon. Alternatively
`pcx setup --client gemini` writes a `post-cortex` server into
`~/.gemini/settings.json`, which takes precedence over the extension's server of the
same name.

## How it connects, and why

Gemini CLI is a terminal app, substitutes `${VAR}` and `${VAR:-default}` in the
manifest from the extension's settings and the environment it was started in, and
stores sensitive settings in the keychain. So a plain HTTP entry works without any
local binary: the MCP server, the context file and the skill work right after the
install. The stdio `pcx mcp` route that the Cursor plugin uses would add a
dependency without gaining anything here.

## Hooks

| Gemini event | `pcx hook gemini ...` | What it adds |
|---|---|---|
| `SessionStart` | `session-start` | The project's session and workspace ids, the user and project cards |
| `AfterTool` (`read_file`) | `file-context` | What the memory knows about the file just read (`BeforeTool` cannot add context) |
| `BeforeAgent` | `prompt` | The rules reminder |
| `AfterAgent` | `capture` | Capture only: the prompt and the reply |

The hooks run `scripts/pcx.sh`, which finds `pcx` (on `PATH` or in the usual install
locations) and stays silent when it is missing. `pcx` resolves the project from the
payload's `cwd`: `.post-cortex.json` at the git root, the ids in `AGENTS.md`, the
cache `~/.post-cortex/projects.json`, then `POST /api/projects/resolve` with the
project key (the normalised `origin` remote, else a hash of the path).

Per-client settings in `~/.post-cortex/config.json` under `hosts.gemini-cli`:
`enabled` (default `true`), `capture` (default `false`), `workspace` (the workspace a
new project joins).

Without `pcx` there is no `[PCX] Session:` line; `GEMINI.md` tells the agent to get
the project's session with the `session` tool, action `resolve_project`.

The hooks step aside where they would run twice: in a project set up with
`pcx setup --project` (its `.gemini/settings.json` has the hooks), and when
`pcx setup` (global mode) put pcx hooks into `~/.gemini/settings.json`.

## Limits

- **Windows**: the hooks run `sh` (Gemini CLI runs hook commands in PowerShell there);
  without Git Bash on `PATH` they fail with a warning and the session carries on. The
  MCP server, context file and skill are not affected.

## Development

Generated in part by `integrations/sync.py` (see its docstring):

| File | Source |
|---|---|
| `skills/post-cortex-memory/` | copied from `crates/post-cortex-cli/templates/skills/` |
| `GEMINI.md` | `templates/clients/AGENTS.md` without ids, as `pcx setup` writes it for a global setup |
| `scripts/pcx.sh` | copied from `integrations/cursor/scripts/pcx.sh` |
| `gemini-extension.json` `version` | the workspace `Cargo.toml` version |
| everything else (`gemini-extension.json` metadata, `hooks/hooks.json`, this README) | hand-written |

```bash
python3 integrations/sync.py --check
python3 -m unittest discover -s integrations/cursor/tests -v   # launcher and manifests
npx -y @google/gemini-cli extensions validate integrations/gemini
```

## Publishing

Nothing has been published. What the docs and the CLI source say
([releasing](https://geminicli.com/docs/extensions/releasing/),
[reference](https://geminicli.com/docs/extensions/reference/)):

- `gemini extensions install <source>` takes a GitHub `owner/repo` URL or a local
  path, plus `--ref`, `--auto-update`, `--pre-release`, `--consent`,
  `--skip-settings`. There is no subdirectory option (`tryParseGithubUrl` accepts
  exactly `owner/repo`).
- For a GitHub source it first tries the latest GitHub release: an asset named
  `{platform}.{arch}.{name}.tar.gz` / `{platform}.{name}.tar.gz` (or a single generic
  asset), else the release's source tarball; with no release at all it clones. The
  manifest must be at the root of what it gets.
- The gallery at https://geminicli.com/extensions/browse/ indexes public GitHub
  repositories with the `gemini-cli-extension` topic and `gemini-extension.json` at
  the root of the repository or the release archive, crawled daily; no submission.

So `gemini extensions install https://github.com/julymetodiev/post-cortex` does not
work as the repository is laid out: its releases carry the `pcx-*` binaries (no
match, several assets), so the CLI falls back to the source tarball, whose root has
no manifest. Two ways to fix it:

**A. A mirror repository (recommended).** One step per release, nothing changes here.

1. Create the public repository `julymetodiev/post-cortex-gemini` with the topic
   `gemini-cli-extension`.
2. Copy this directory to its root and push:
   ```bash
   python3 integrations/sync.py
   rsync -a --delete --exclude .git integrations/gemini/ ../post-cortex-gemini/
   git -C ../post-cortex-gemini add -A && git -C ../post-cortex-gemini commit -m "post-cortex 0.6.0"
   git -C ../post-cortex-gemini tag v0.6.0 && git -C ../post-cortex-gemini push --follow-tags
   ```
3. The gallery picks it up within a day. Users install with
   `gemini extensions install https://github.com/julymetodiev/post-cortex-gemini`.

**B. Release archives in this repository.** Attach `darwin.post-cortex.tar.gz`,
`linux.post-cortex.tar.gz` and `win32.post-cortex.zip` (each `integrations/gemini`
with `gemini-extension.json` at the archive root) to every GitHub release; the
platform prefix matches before the `pcx-*` binaries are considered. Then
`gemini extensions install https://github.com/julymetodiev/post-cortex` works and
updates follow the latest release. This needs a step in `release.yml`, and every
release must carry the archives or installs break. Whether the gallery crawler reads
archives of a repository whose root has no manifest is not documented beyond the
sentence above.
