# @post-cortex/plugin-core

The shared core of the native [post-cortex](https://post-cortex.com) plugins
(`@post-cortex/opencode`, `@post-cortex/kilo`, `@post-cortex/pi`,
`@post-cortex/openclaw`, `@post-cortex/dsh`). TypeScript, ESM, no runtime
dependencies; runs on Node 20+ and Bun.

You do not install this package yourself: the plugins depend on it.

## What it does

- **Client config.** Reads `~/.post-cortex/config.json` and
  `credentials.json` (the directory is `$PC_CONFIG_DIR` when set) with the
  same rules as `pcx`: profile = plugin option, else `$PC_PROFILE`, else
  `active`, else `cloud`; URL = `$PC_URL`, else the profile's URL, else the
  built-in one (`cloud` is `https://api.pcx.is`, `self` is
  `http://127.0.0.1:3737`); token = `$PC_AUTH_TOKEN`, else the token `pcx
  login` stored for the profile. `hosts.<client>` holds per-client switches:
  `enabled` (default true), `capture` (default false), `workspace`.
- **Project resolution.** The project key is the git `origin` URL
  normalised (`github.com/owner/repo`), else `local:<sha256 of the git root,
  16 hex>`, else the same for the directory. `git` is used when it is on the
  PATH, else `.git/config` is read directly (worktrees included). Ids come from
  `.post-cortex.json` at the repository root, else the `Session ID` /
  `Workspace ID` lines of `AGENTS.md` or `CLAUDE.md`, else the cache
  `~/.post-cortex/projects.json`, else `POST /api/projects/resolve` (the
  workspace and session are created on first use; teammates on the same remote
  share them). The algorithm is asserted against the shared vectors in
  `integrations/testdata/project-keys.json`, like `pcx` and the Python hooks.
- **REST client.** `GET /api/cards`, `GET /api/file`, `POST /api/messages`,
  `POST /api/projects/resolve`, each with a 3 s timeout, `X-PCX-Author` set to
  the client's name and the bearer token when there is one. Every failure
  (offline daemon, HTTP error, timeout) is `null`: memory never blocks the
  agent.
- **Text.** The cards text and the file-memory text are byte-for-byte the
  ones `pcx hook` prints (`HookEnv` in `pcx/clients/common.rs`), plus the
  `[PCX] Session: ... | Workspace: ...` line in global mode, and the
  `apply_patch` path parser.

## API

```ts
import { PostCortex } from "@post-cortex/plugin-core";

const pc = new PostCortex("opencode", { capture: true });
await pc.cardsText(directory);              // string | null
await pc.fileContextText(directory, paths); // string | null
await pc.capture(directory, turns);         // boolean
```

The lower-level pieces (`resolveBackend`, `hostSettings`, `project`,
`normalizeRemote`, `localKey`, `PcxClient`, `cardsText`, `fileContextText`,
`patchPaths`) are exported too. `@post-cortex/plugin-core/testing` has the fake
daemon the plugin tests use.

## Privacy

What is sent and stored: <https://post-cortex.com/privacy>. Turn capture is
off unless you turn it on.

## License

MIT
