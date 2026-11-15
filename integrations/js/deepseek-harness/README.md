# @post-cortex/dsh

Long-term memory for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
(`dsh`) from [post-cortex](https://post-cortex.com). A native Cordis plugin on
the harness's own extension points, distributed as a dsh bundle.

## What it does

| Seam | Behaviour |
|---|---|
| `systemPrompt.context` (rules) | Static `[PCX] Rules: …` banner on every request, present whenever the plugin is enabled, unaffected by fetch failures. |
| `ctx.skills` | Registers the `post-cortex-memory` skill every host ships (`skills/`, copied from the pcx templates by `integrations/sync.py`; its `references/` resolve from there) when the optional skills service is composed; unregisters on unload. |
| `agent/pre-step` | First request of each dsh session: fetch the user card, the project card and the session and workspace ids (3 s bound) for the runtime context `post-cortex:cards`. The context is registered once and its text is read again when each request is admitted, after pre-step, so every request carries the cards once, the first one included. A failed fetch is retried on the next step. |
| `tools/post-execute` | After `edit` and `write`: append what the memory knows about the file (problems solved there, decisions, dependents) to the result content. |
| `session/event` | With capture on: collect the user's own messages (not plugin or harness ones) and the assistant's text. |
| `agent/turn-stopping` | Flush the collected turns to the project's message stream (awaited at the turn boundary; a failed flush is retried at the next one). |
| `ctx.effect` disposer | Final flush on unload. |

The project is resolved from the dsh session's working directory. Every call
has a 3 s timeout and any failure is ignored.

## Install

Plugins install into a named profile; pick the one you run (`web`,
`headless`, `acp`, or your own):

```bash
dsh plugin --profile <name> add @post-cortex/dsh
```

The package declares `dsh.bundle`, so it activates as a configuration layer
(`cordis.patch.yml`, one row: the plugin). Requires dsh `>=0.2.1-rc.2 <0.3.0`
(`engines.dsh`) and Node `^22.19.0 || >=24.0.0`.

Settings live in `hosts.dsh` of `~/.post-cortex/config.json`:

```json
{ "hosts": { "dsh": { "capture": true, "workspace": "team", "enabled": true } } }
```

or, for one dsh profile, in the plugin row's `config` (`profile`, `capture`,
`workspace`, `enabled`) in `$DSH_HOME/profiles/<name>/cordis.patch.yml`.

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

The plugin registers no tools; add the post-cortex MCP server with dsh's MCP
client in the profile's own patch, `$DSH_HOME/profiles/<name>/cordis.patch.yml`.

Through `pcx` (resolves the profile and token from the post-cortex files;
dsh scrubs `*TOKEN*` variables from a stdio server's environment, so the
stored login is what counts here):

```yaml
- insert:
    - id: mcp-post-cortex
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: post-cortex
        transport: stdio
        command: pcx
        args: ['mcp', '--author', 'dsh']
```

Remote, with the token from the environment:

```yaml
- insert:
    - id: mcp-post-cortex
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: post-cortex
        transport: streamable-http
        url: https://api.pcx.is/mcp
        headers:
          Authorization: !!js '`Bearer ${process.env.PC_AUTH_TOKEN}`'
          X-PCX-Author: dsh
```

For a self-hosted daemon use `http://127.0.0.1:3737/mcp`. The tools appear as
`mcp__post-cortex__<tool>`.

## Privacy

What is sent and stored: <https://post-cortex.com/privacy>. Turn capture is
off unless you turn it on.

## License

MIT
