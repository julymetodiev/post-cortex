# @post-cortex/openclaw

Long-term memory for [OpenClaw](https://openclaw.ai) from
[post-cortex](https://post-cortex.com), shared with every other agent that
uses the same post-cortex workspace.

## What it does

- **Cards in the system prompt.** On `before_prompt_build` the user card, the
  project card and the session and workspace ids the post-cortex tools use are
  appended to the system prompt (`appendSystemContext`, so providers can cache
  it). Fetched again at most every 10 minutes.
- **Related memory for the prompt.** On the same hook the top 3 records of
  the project's workspace for the user's request (`POST /api/search`, mode
  fast) are appended to the turn's context (`appendContext`) as a short
  "Related memory" section. Requests under 12 characters get none.
- **Turn capture (opt-in).** On `agent_end`, with capture on, the run's user
  and assistant turns go to the project's message stream.
- **Tools.** The manifest declares the post-cortex MCP server (`pcx mcp
  --author openclaw`, stdio), so OpenClaw starts it while the plugin is
  enabled and the agent can search and write memory.

The project is the agent's workspace directory (`~/.openclaw/workspace` by
default), resolved like any other directory: from its git remote when it is a
repository, else from its path. Every call has a 3 s timeout and any failure
is ignored.

Recall needs a daemon with `POST /api/search`; an older one answers 404 and
the prompt simply gets no section.

## Install

```bash
openclaw plugins install npm:@post-cortex/openclaw --accept-capabilities
openclaw config set plugins.entries.post-cortex.hooks.allowConversationAccess true
```

`--accept-capabilities` consents to the MCP server the manifest declares
(without it the install stops and asks). OpenClaw blocks `before_prompt_build`
and `agent_end` for non-bundled plugins until
`plugins.entries.post-cortex.hooks.allowConversationAccess` is `true`;
`openclaw plugins inspect post-cortex --runtime --json` lists the two hooks
once they are allowed. Restart the gateway if it is running
(`openclaw gateway restart`).

Settings go under `plugins.entries.post-cortex.config` in
`~/.openclaw/openclaw.json` (all optional; each wins over `hosts.openclaw` in
`~/.post-cortex/config.json`):

```json5
{
  plugins: {
    entries: {
      "post-cortex": {
        enabled: true,
        hooks: { allowConversationAccess: true },
        config: { profile: "cloud", capture: true, workspace: "team" },
      },
    },
  },
}
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
`PC_PROFILE`, `PC_CONFIG_DIR`. The bundled MCP entry runs `pcx`, which reads
the same files.

## MCP server

The default entry needs `pcx` on the gateway's PATH. Your `mcp.servers` in
`openclaw.json` wins over it, so you can point OpenClaw at the remote endpoint
instead, or turn it off:

```json5
{
  mcp: {
    servers: {
      "post-cortex": {
        url: "https://api.pcx.is/mcp",
        transport: "streamable-http",
        headers: { Authorization: "Bearer ${PC_AUTH_TOKEN}", "X-PCX-Author": "openclaw" },
      },
      // or: "post-cortex": { enabled: false },
    },
  },
}
```

For a self-hosted daemon use `http://127.0.0.1:3737/mcp`. `openclaw mcp
show` lists what OpenClaw will start.

## Why not the memory slot

OpenClaw has an exclusive memory slot (`plugins.slots.memory`, default
`memory-core`). Its owner registers a memory capability: the search runtime
behind `memory_search` / `memory_get` (with transcript-hit authorization) and
the pre-compaction flush plan with its persistence tools. Taking the slot
would turn off `memory-core` (MEMORY.md, daily logs) and would need search and
write routes that post-cortex serves over MCP only. This plugin therefore runs
next to `memory-core` as a normal plugin; the post-cortex tools come from the
MCP server.

## Privacy

What is sent and stored: <https://post-cortex.com/privacy>. Turn capture is
off unless you turn it on.

## License

MIT
