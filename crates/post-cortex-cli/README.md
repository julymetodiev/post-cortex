# post-cortex-cli

The client side of `pcx`, the [post-cortex](https://github.com/julymetodiev/post-cortex)
command line:

- backend profiles (`self`, `cloud`, your own) and `pcx login`;
- `pcx setup`: coding agents (Claude Code, Codex, Cursor, Gemini CLI, Claude
  Desktop, VS Code, Windsurf, Zed, ...) set up for post-cortex, globally or for
  one project, with their hooks (`pcx hook`);
- the stdio MCP bridge (`pcx`, `pcx mcp --profile ...`);
- `pcx card`, `pcx session`, `pcx workspace`.

It talks to a daemon or the hosted cloud over their public APIs only and never
opens storage. The `pcx` binary ships in `post-cortex-daemon`, which runs the
daemon's own commands and hands every client command to `post_cortex_cli::run`.

License: MIT.
