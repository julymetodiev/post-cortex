# @post-cortex/kilo

Long-term memory for [Kilo Code](https://kilo.ai) from
[post-cortex](https://post-cortex.com). Install it once, globally: every
project you open is recognised from its git remote and gets its own memory.
The plugin runs inside Kilo's local server, so one install covers the Kilo CLI,
the VS Code extension and the JetBrains plugin.

## What it does

- **Cards in every prompt.** The user card (how you like to work) and the
  project card (stack, conventions, current goals) go into the system prompt,
  with the session and workspace ids the post-cortex tools use. Fetched once
  per Kilo session.
- **File memory after edits.** After `edit`, `write` and `apply_patch`, what
  the memory knows about the file (problems solved there, decisions about it,
  what depends on it) is appended to the tool output the model reads.
- **Turn capture (opt-in).** With capture on, the session's user and
  assistant turns go to the project's message stream when the session goes
  idle.

The plugin never blocks the agent: every call has a 3 s timeout and any
failure is ignored.

## Install

```bash
kilo plugin @post-cortex/kilo --global
```

or add it to the `plugin` array of `~/.config/kilo/kilo.jsonc` (or
`kilo.json`; the Kilo Marketplace adds the same entry once the plugin is
listed there):

```json
{
  "plugin": ["@post-cortex/kilo"]
}
```

Settings live in `hosts.kilo` of `~/.post-cortex/config.json`:

```json
{ "hosts": { "kilo": { "capture": true, "workspace": "team" } } }
```

`profile`, `capture`, `workspace` and `enabled` can also be passed as plugin
options, `["@post-cortex/kilo", { "capture": true }]`, on Kilo versions whose
config accepts the `[name, options]` form (Kilo 7.1 rejects it; use
`hosts.kilo` there).

## Connect a backend

The hosted cloud is the default. Create a key at
<https://app.pcx.is> (Connections → API keys) and store it with
[`pcx`](https://github.com/julymetodiev/post-cortex):

```bash
pcx login                 # cloud, stored in ~/.post-cortex/credentials.json (0600)
```

Self-hosted daemon instead:

```bash
pcx profile use self      # http://127.0.0.1:3737, or: pcx profile add self --url http://host:3737
pcx login --profile self --token "$PC_AUTH_TOKEN"   # only when the daemon requires a token
```

Environment variables win over the files: `PC_URL`, `PC_AUTH_TOKEN`,
`PC_PROFILE`, `PC_CONFIG_DIR`.

## Tools (MCP)

The plugin registers no tools; the agent searches and writes memory through
the post-cortex MCP server. Kilo cannot read the credentials file, so pick one
of these entries for the `mcp` key of `kilo.jsonc`:

Through `pcx` (resolves the profile and token itself; recommended):

```json
{
  "mcp": {
    "post-cortex": {
      "type": "local",
      "command": ["pcx", "mcp", "--profile", "cloud", "--author", "kilo"],
      "enabled": true
    }
  }
}
```

Remote, with the token from the environment:

```json
{
  "mcp": {
    "post-cortex": {
      "type": "remote",
      "url": "https://api.pcx.is/mcp",
      "headers": {
        "Authorization": "Bearer {env:PC_AUTH_TOKEN}",
        "X-PCX-Author": "kilo"
      },
      "oauth": false,
      "enabled": true
    }
  }
}
```

`kilo mcp add` adds an entry interactively. For a self-hosted daemon use
`http://127.0.0.1:3737/mcp` (or `--profile self`). Kilo reads the OpenCode
schema, so `pcx mcp-config opencode` prints a usable entry (change
`X-PCX-Author` / `--author` to `kilo`).

## Projects

The project key is the git `origin` remote (`github.com/owner/repo`), else a
hash of the repository root, else of the directory. Its workspace and session
are created on first use and cached in `~/.post-cortex/projects.json`.
`.post-cortex.json` (`{"session_id", "workspace_id"}`) at the repository root
pins fixed ids.

## Kilo Marketplace

`marketplace/plugins/@post-cortex/kilo/PLUGIN.yaml` is the draft entry for
[Kilo-Org/kilo-marketplace](https://github.com/Kilo-Org/kilo-marketplace); it
is submitted there by pull request once the package is on npm.

## Privacy

What is sent and stored: <https://post-cortex.com/privacy>. Turn capture is
off unless you turn it on.

## License

MIT
