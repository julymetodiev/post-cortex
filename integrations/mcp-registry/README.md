# post-cortex in the MCP Registry

The entry for the official MCP Registry
([registry.modelcontextprotocol.io](https://registry.modelcontextprotocol.io)),
namespace `io.github.julymetodiev/post-cortex`:

| File | What it lists | Publishable |
|---|---|---|
| `server.json` | the hosted server: `remotes` streamable-http `https://api.pcx.is/mcp`, header `Authorization: Bearer {api_key}` (required, secret) | once `api.pcx.is` is live |
| `server.self-hosted.json` | the same remote, plus the `cargo` package `post-cortex-daemon` (stdio: plain `pcx` starts the local daemon and proxies MCP to it) | after the prerequisites below |

Both pass `mcp-publisher validate` (1.8.1, schema
`https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json`).
`integrations/sync.py` keeps the `version` fields on the workspace version; the rest
is hand-written. The registry keeps every version: publish `server.json` first and
`server.self-hosted.json` as a later version once its package is ready.

## Package types

The registry accepts `npm`, `pypi`, `nuget`, `cargo` (crates.io only), `oci` (Docker
Hub, GHCR, Quay, Artifact Registry, ACR, MCR) and `mcpb` (GitHub or GitLab release
assets) ([requirements](https://github.com/modelcontextprotocol/registry/blob/main/docs/reference/server-json/official-registry-requirements.md),
[package types](https://github.com/modelcontextprotocol/registry/blob/main/docs/modelcontextprotocol-io/package-types.mdx)).
Homebrew is not a package type. No OCI image of post-cortex is published (the
`Dockerfile` is not pushed by `release.yml`), so `cargo` is the self-hosted package.

Prerequisites for `server.self-hosted.json`:

1. **Ownership marker**: crates.io must render the text
   `mcp-name: io.github.julymetodiev/post-cortex` in the `post-cortex-daemon` README
   as visible text (crates.io strips HTML comments). Add a line such as
   ``- MCP Registry name: `mcp-name: io.github.julymetodiev/post-cortex` `` to
   `crates/post-cortex-daemon/README.md` and publish a crate version that carries it;
   the package `version` must be that version.
2. **Binary name**: the registry's cargo model is "install once, run the binary by
   name", and server.json has no field for a binary name. The crate installs `pcx`,
   not `post-cortex-daemon`, so a client that derives the command from the identifier
   runs the wrong name. Clients that implement cargo today are few (VS Code ignores
   cargo packages, see below); check the client before relying on it.

## Publishing

Nothing has been published. With the GitHub namespace the login is the GitHub account
`julymetodiev` ([quickstart](https://github.com/modelcontextprotocol/registry/blob/main/docs/modelcontextprotocol-io/quickstart.mdx)):

```bash
brew install mcp-publisher        # or a release binary from github.com/modelcontextprotocol/registry/releases
cd integrations/mcp-registry
python3 ../sync.py --check        # versions match Cargo.toml
mcp-publisher validate server.json
mcp-publisher login github        # GitHub sign-in in the browser, as julymetodiev
mcp-publisher publish server.json
curl -s "https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.julymetodiev/post-cortex"   # check the listing
```

From CI instead: `mcp-publisher login github-oidc` in a workflow with
`id-token: write` ([GitHub Actions guide](https://github.com/modelcontextprotocol/registry/blob/main/docs/modelcontextprotocol-io/github-actions.mdx)).

Before publishing: `https://api.pcx.is/mcp` answers, the icon URL
(`raw.githubusercontent.com/.../integrations/cursor/assets/logo.png` on `main`)
resolves, i.e. the plugin is merged.

## Who reads the registry

Checked 2026-10-21.

| Client | Reads the official registry? | How a server gets listed |
|---|---|---|
| VS Code (Copilot Chat, Extensions view `@mcp`) | Indirectly. The gallery reads any registry that implements the registry API at `chat.mcp.gallery.serviceUrl` ([source](https://github.com/microsoft/vscode/blob/main/src/vs/platform/mcp/common/mcpManagement.ts)); the default (product.json) is GitHub's registry. Remotes are supported; package types `npm`, `pypi`, `oci`, `nuget` only, `cargo` is dropped ([source](https://github.com/microsoft/vscode/blob/main/src/vs/platform/mcp/common/mcpGalleryService.ts)). An organisation can point the setting at registry.modelcontextprotocol.io. |
| GitHub MCP Registry (github.com/mcp, `api.mcp.github.com`) | Partly. Same API and server.json format; GitHub says downstream registries pick up updates from the official one, but inclusion is by request to partnerships@github.com ([GitHub blog](https://github.blog/ai-and-ml/generative-ai/how-to-find-install-and-manage-mcp-servers-with-the-github-mcp-registry/)). | Publish to the official registry, then email the request. |
| Windsurf / Devin Desktop | No for the default marketplace (source not documented). Enterprise admins can add "MCP Registry URLs" that "must follow the official MCP registry schema"; it applies to the Devin Local agent, not Cascade ([docs](https://docs.devin.ai/desktop/cascade/mcp)). | Not documented for the default marketplace. |
| Cline | No. The MCP Marketplace is curated from issues in [cline/mcp-marketplace](https://github.com/cline/mcp-marketplace) (repo URL, 400x400 PNG logo, reason; tested by having Cline install from the README or `llms-install.md`). | Separate submission. |
| Goose | No. The extensions directory is the curated `documentation/static/servers.json` in [aaif-goose/goose](https://github.com/aaif-goose/goose/blob/main/documentation/static/servers.json) (command-based entries); no reference to the official registry in the repository. | Pull request to that file. |
| Cursor | No. Its own plugin marketplace (see `integrations/cursor`) and the community site cursor.directory ([docs](https://cursor.com/docs/mcp.md)). | Plugin submission. |
