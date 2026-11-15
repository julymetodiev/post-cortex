# post-cortex for Claude Code

A Claude Code plugin that connects Claude Code to [post-cortex](https://post-cortex.com),
a long-term, searchable project memory. It bundles:

- **MCP server** `post-cortex` (HTTP): post-cortex cloud by default, or your own daemon.
- **Skill** `post-cortex-memory`: when and how to read and write the memory.
- **Hooks**: the memory rules on every prompt, the user and project cards on session
  start, the rules again after compaction, what the memory says about a file before
  it is edited, and a Stop check that blocks a turn that changed the project without
  logging it.

## Install

In Claude Code:

```
/plugin marketplace add julymetodiev/post-cortex
/plugin install post-cortex@post-cortex
```

From a shell (`claude plugin install` does not prompt for options; set them from stdin
so the key stays out of the shell history):

```bash
claude plugin marketplace add julymetodiev/post-cortex
claude plugin install post-cortex@post-cortex
printf '{"api_key":"%s"}' "$(cat ~/pcx-key.txt)" | claude plugin configure post-cortex@post-cortex --values-stdin
```

## Configure

The plugin asks for two options when it is enabled. Change them later with
`/plugin configure post-cortex@post-cortex`.

| Option | Default | Notes |
|---|---|---|
| `url` | `https://api.pcx.is` | Base URL of the server, no trailing slash. For a self-hosted daemon: `http://127.0.0.1:3737` (`pcx start`), or the daemon's host. The MCP endpoint is `<url>/mcp`. |
| `api_key` | empty | Sensitive: stored in the macOS Keychain (or `~/.claude/.credentials.json`), never in `settings.json`. Your cloud API key, or the daemon's `auth_token`. Leave empty for a local daemon without `auth_token`. |

Every write is attributed to `claude-code` (`X-PCX-Author` header).

## Projects

The plugin is installed once and recognises each project by itself. The hooks find the
project's post-cortex session and workspace, first hit wins:

1. `.post-cortex.json` (`{"session_id", "workspace_id"}`) at the git root, written by
   `pcx setup --project`: commit it and every teammate and every agent uses the same
   memory;
2. the ids in `CLAUDE.md` (projects set up before the plugin; `pcx setup --project`
   moves them to `.post-cortex.json`);
3. the cache `~/.post-cortex/projects.json`;
4. `POST /api/projects/resolve` with the project key: the git `origin` remote normalised
   (`git@github.com:a/b.git` is `github.com/a/b`), else a hash of the repository path.
   The first session in a project creates a workspace and a session named after it;
   teammates on the same remote get the same ones.

The card hook prints the session and workspace ids with the cards, and the compaction
hook prints them again, so the agent knows where to log.

Backend for the hooks: `PC_URL`, else the plugin's `url` option, else the active profile
of `~/.post-cortex/config.json` (`pcx profile`); key: `PC_AUTH_TOKEN`, else the `api_key`
option, else the key `pcx login` stored for the profile with that URL.

Per-client settings in `~/.post-cortex/config.json`, under `hosts.claude-code`:

| Key | Default | |
|---|---|---|
| `enabled` | `true` | `false` turns the plugin hooks off |
| `capture` | `false` | `true` posts the conversation turns to the session (`pcx setup --capture` sets it) |
| `workspace` | none | the workspace name a new project joins instead of its own |

A project set up before the plugin may still have post-cortex's hooks in
`.claude/settings.json`: each plugin hook steps aside when the same script is in
`$CLAUDE_PROJECT_DIR/.claude/hooks`, so nothing runs twice, and `pcx setup --project`
takes them out (owner 2026-11-06: post-cortex is used only through the plugin).

## Development

This directory is generated in part from the `pcx` crate templates, which stay the
single source of truth:

| File | Source |
|---|---|
| `hooks/pcx-*.py`, `hooks/pcx_resolve.py`, `skills/post-cortex-memory/` | copied from `crates/post-cortex-cli/templates/` |
| `hooks/hooks.json` | generated from `templates/settings.json` (the plugin's hooks, with their `<SESSION_ID>`-style arguments dropped) |
| `.claude-plugin/plugin.json` `version` | the workspace `Cargo.toml` version |
| everything else (`plugin.json` metadata, `.mcp.json`, this README) | hand-written |

Edit the templates, never the copies, then:

```bash
python3 integrations/sync.py           # regenerate
python3 integrations/sync.py --check   # CI: fails when the copies differ
python3 -m unittest discover -s integrations/tests   # the resolver and the hooks
claude plugin validate --strict integrations/claude-code
claude plugin validate .               # the marketplace at the repo root
```

Try it from a checkout without installing: `claude --plugin-dir integrations/claude-code`.
