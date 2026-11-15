# @post-cortex/pi

Long-term memory for the [Pi coding agent](https://pi.dev) from
[post-cortex](https://post-cortex.com). Install it once: every project you
open is recognised from its git remote and gets its own memory.

## What it does

- **Cards in the system prompt.** On `session_start` the project and its
  cards are fetched; on `before_agent_start` the user card, the project card
  and the session and workspace ids the post-cortex tools use go into the
  `post-cortex` section of Pi's structured system prompt, so Pi only sends a
  delta when they change.
- **File memory after edits.** On the `tool_result` of `edit` and `write`,
  what the memory knows about the file (problems solved there, decisions
  about it, what depends on it) is appended to the result the model reads.
- **Turn capture (opt-in).** On `agent_end`, with capture on, the run's user
  and assistant turns go to the project's message stream.

Every call has a 3 s timeout and any failure is ignored.

## Install

```bash
pi install npm:@post-cortex/pi
```

Add `-l` to install it for the current project only (`.pi/settings.json`).
To try it once without installing: `pi -e npm:@post-cortex/pi`.

Pi passes extensions no options; settings live in `hosts.pi` of
`~/.post-cortex/config.json`:

```json
{ "hosts": { "pi": { "capture": true, "workspace": "team", "enabled": true } } }
```

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

The extension registers no tools. Pi has MCP built in; add the post-cortex
server to `~/.pi/agent/mcp.json`:

Through `pcx` (resolves the profile and token from the post-cortex files):

```json
{
  "mcpServers": {
    "post-cortex": { "command": "pcx", "args": ["mcp", "--author", "pi"] }
  }
}
```

Remote, with the token from the environment:

```json
{
  "mcpServers": {
    "post-cortex": {
      "url": "https://api.pcx.is/mcp",
      "headers": { "Authorization": "Bearer ${PC_AUTH_TOKEN}", "X-PCX-Author": "pi" }
    }
  }
}
```

or from the shell: `pi mcp add post-cortex --url https://api.pcx.is/mcp
--bearer-token-env-var PC_AUTH_TOKEN`. For a self-hosted daemon use
`http://127.0.0.1:3737/mcp`. `pi mcp list` checks the connection; the tools
appear as `mcp__post_cortex__<tool>`.

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
