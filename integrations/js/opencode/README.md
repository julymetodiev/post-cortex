# @post-cortex/opencode

Long-term memory for [OpenCode](https://opencode.ai) from
[post-cortex](https://post-cortex.com). Install it once, globally: every
project you open is recognised from its git remote and gets its own memory.

## What it does

- **Cards in every prompt.** The user card (how you like to work) and the
  project card (stack, conventions, current goals) go into the system prompt,
  with the session and workspace ids the post-cortex tools use. Fetched once
  per OpenCode session.
- **File memory after edits.** After `edit`, `write` and `apply_patch`, what
  the memory knows about the file (problems solved there, decisions about it,
  what depends on it) is appended to the tool output the model reads.
- **Turn capture (opt-in).** With capture on, the session's user and
  assistant turns go to the project's message stream when the session goes
  idle.

The plugin never blocks the agent: every call has a 3 s timeout and any
failure is ignored. When a project already runs its own copy from `pcx setup
--client opencode` (`.opencode/plugins/post-cortex.ts`), the global plugin
steps aside in that project.

## Install

```bash
opencode plugin @post-cortex/opencode --global
```

or add it to the `plugin` array of `~/.config/opencode/opencode.json`
(options are optional):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": [["@post-cortex/opencode", { "capture": true }]]
}
```

Options: `profile` (backend profile), `capture` (default false), `workspace`
(workspace name a new project goes into), `enabled`. Each wins over
`hosts.opencode` in `~/.post-cortex/config.json`.

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
the post-cortex MCP server. OpenCode cannot read the credentials file, so
pick one of these entries for the `mcp` key of `opencode.json`:

Through `pcx` (resolves the profile and token itself; recommended):

```json
{
  "mcp": {
    "post-cortex": {
      "type": "local",
      "command": ["pcx", "mcp", "--profile", "cloud", "--author", "opencode"],
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
        "X-PCX-Author": "opencode"
      },
      "oauth": false,
      "enabled": true
    }
  }
}
```

The same remote entry from the shell: `opencode mcp add post-cortex --url
https://api.pcx.is/mcp --header "Authorization=Bearer
{env:PC_AUTH_TOKEN}" --header "X-PCX-Author=opencode"`. For a self-hosted
daemon use `http://127.0.0.1:3737/mcp` (or `--profile self`). `pcx mcp-config
opencode` prints the entry for your active profile.

## Projects

The project key is the git `origin` remote (`github.com/owner/repo`), else a
hash of the repository root, else of the directory. Its workspace and session
are created on first use and cached in `~/.post-cortex/projects.json`.
`.post-cortex.json` (`{"session_id", "workspace_id"}`) at the repository root
pins fixed ids.

## Privacy

What is sent and stored: <https://post-cortex.com/privacy>. Turn capture is
off unless you turn it on.

## License

MIT
