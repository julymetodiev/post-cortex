# http

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8. Approved by the
owner on 2026-11-01 (board `pcx-cloud`, plan `7722e1ac`, task `68852a44`), with the owner's
answers of the same day to the plan's four questions (below) and the proposals of the spec
(two tools by effect, 2 MiB bodies, a log of 30 days, `--public-only` in the cloud, presets as
the editor's data, `expect` as a `pcx.workflow.v1.Schema`, no grants per connection yet).

- **`http`**: calls outside HTTP APIs with the organisation's own credentials, kept as
  connections. Crate `pcx-http`, built in (the daemon in process, the cloud's catalog as a
  process); proto package `pcx.http.v1`; uses `post-cortex.memory` (its presets and its log
  are documents); asks for the `network` grant.
  - Workflows reach it with the one `tool` block (`workflow.md`, blocks): `/pcx.http.v1.Http/Get`
    or `/pcx.http.v1.Http/Request`. There is no new kind of block in the engine; an API block
    of the editor's palette is a preset (below), a `tool` block with its template filled.
  - Agents reach it through the same tools; `webhooks` stays the way in (requests from
    outside), this module the way out.

## The owner's answers (2026-11-01)

1. A response body is `dyn` (`google.protobuf.Value`) when no schema is given; a request that
   names one (`expect`, below) has its body checked against it, and the call fails when it does
   not fit. Typing the body for CEL is the data blocks' transformer's (`000401af`), not this
   module's.
2. The first presets: GitHub, Slack, Stripe (reads) and generic REST.
3. Connections are a list of the settings from the start, each with its own secrets: the host
   learns secrets inside a list (below, the host's change).
4. OAuth2 comes later (its own task); the first version has API keys, bearer tokens and basic
   authentication.

## Settings

`pcx.http.v1.HttpConfig`, edited in the panel as any module's (`config_type`):

- `workspace_id`: the workspace its presets and its log are kept in (as mail's), the
  organisation's store (owner 2026-11-10; docs/connect.md section 4a, a module's data): every
  caller, from any workspace, reads them and makes requests, each logged there by the module;
  a preset is saved or deleted by a caller who reaches that workspace;
- `connections`: a list of `Connection`, each an outside service:
  - `name` (the list's key, below: 1 to 64 of a-z, 0-9, '-'), `description`;
  - `base_url`: `https://` with a host and an optional path prefix, no query, no user info;
    `http://` only to a loopback host and only with `private_networks`;
  - `auth` (an enum: a oneof is no setting), with `auth_name`, `user` and `secret` (the
    connection's one secret):
    - none;
    - `AUTH_BEARER`: `Authorization: Bearer <secret>`;
    - `AUTH_BASIC`: `user` and `secret` as the password;
    - `AUTH_HEADER`: the secret in the header `auth_name`, e.g. `X-Api-Key`;
    - `AUTH_QUERY`: the secret in the query parameter `auth_name`, for APIs that take a key
      in the URL only;
  - `headers`: headers sent with each request (a list of `name` and `value`, no secrets);
  - `idempotency_header`: the header that carries the engine's idempotency key (Stripe:
    `Idempotency-Key`); empty: none is sent;
  - `timeout_seconds`: its default (0: 30 s; at most 300);
  - `test_path`: what the panel's "test request" calls (a `GET`; default `/`).
- `allow`: the hosts a request without a connection may reach, each a host or `*.domain`;
  empty: none (so an agent with the tool reaches only the connections a person set up);
- `private_networks`: whether private, loopback and link-local addresses may be reached
  (default false; for a daemon calling services of its own network). The cloud's catalog starts
  the module with `--public-only`, which refuses a configuration that sets it (`InvalidArgument`
  at start), so no tenant reaches the cloud's own network;
- `daily_limit`: requests a day (default 5,000, at most 100,000; in the cloud the plan's limit
  caps it).

Tokens, passwords and key values are `secret` settings (write-only, sealed for the
organisation and the module); the panel shows only that they are set (`secrets_set`).

## The host's change: secrets inside a list

Today a secret is a string outside any repeated field (`host.md`, everything gRPC, settings;
`pcx_module_api::ConfigType`), because a change replaces the list and nothing says which old
element a new one is. The change:

- `pcx.module.v1.Setting` gains `key` (a bool): a string field of the message a repeated field
  holds, whose value names the element (1 to 64 of a-z, 0-9, '-'; required; unique in the
  list, else `InvalidArgument` naming the field and the value).
- A secret may be inside an element of a list whose message has exactly one `key` field, the
  list itself in no other list; the manifest check refuses the others as today.
- A secret's path names the element by its key: `connections[github].secret` in
  `secrets_set` and in `ConfigureModuleRequest.clear`.
- A change keeps the secret of an element whose key it keeps (unless `clear` names it); an
  element the change leaves out takes its secrets with it; a renamed element is a new one, its
  secrets set again.
- The rules stay the `pcx-module-api` functions every host and the cloud apply
  (`ConfigType::redact`, `keep_secrets`, and `secrets` and `with_secrets`, with which a store
  that keeps the secrets apart, the cloud's, splits and joins them by path without reading
  paths itself); the panel's
  form shows a list's secrets per element. A token per source of `webhooks` (`webhooks.md`) can
  use the same rule later.

## Requests

`pcx.http.v1.HttpRequest` (an RPC and a message cannot share the name `Request`):

- `connection`: a connection's name, or empty;
- `method`: `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE`;
- `path` (with a connection): a path under its `base_url`, beginning with `/`, with
  `{name}` places filled from `path_params`; `url` (without one): an absolute `https://` URL
  whose host `allow` names. A request with a connection that gives an absolute URL is refused,
  so a connection's credentials go only to its own host;
- `path_params`: a list of `name` and `value` (a list, not a map, so a workflow's template fills
  each place with its own constant or `{{ }}`, owner 2026-11-01), each value percent-encoded as
  one path segment (a value never adds a `/`, a `..` or a query); a place without its value, a
  value without its place, or a place given twice is refused;
- `query`: a list of `name` and `value`, encoded by the module;
- `headers`: a list of `name` and `value`; `Authorization`, `Proxy-Authorization`, `Cookie`,
  `Host`, the hop-by-hop headers and the connection's own auth header are refused (only the
  connection sets them);
- `body`, one of: `json` (`google.protobuf.Value`), `text` (with `content_type`, default
  `text/plain; charset=utf-8`), `raw` (bytes, with `content_type`), or none; at most 1 MiB.
  A `json` body is written by the proto3 JSON mapping (`pcx_module_api::json`): a whole number
  a double holds exactly goes as an integer (`1`, not `1.0`: an API that reads an integer
  refuses the second; pcx-probe 2026-11-12, `limit` of `/api/search`);
- `timeout`: at most 5 min; empty: the connection's (or 30 s);
- `ok`: the statuses that answer instead of failing (e.g. `404` for "is it there"); empty:
  2xx;
- `expect`: a `pcx.workflow.v1.Schema` (the schema a workflow's input and a `model` block's
  output use) the JSON body must fit, and `expect_list` when the body is a list of such
  objects; checked with `pcx-expr` (each field's type read from its JSON value, the same
  reading as a workflow's input); fields the schema does not name are kept.

`pcx.http.v1.HttpResponse`: `status`, `headers` (names in lower case; `Set-Cookie` left out), the
body (`json` when the response's media type is JSON, `text` when it is `text/*` or another
textual type, else `raw`), `content_type`, `url` (the final URL, without its query),
`redirects` and `duration`. A JSON number is a `double` (`google.protobuf.Value`): a whole
number above 2^53 loses its precision, which an API's ids as strings avoid.

## Failures

- A status outside `ok` is a typed failure, so a workflow block's `retry` and `error` edge work:
  `401`, `403` → `PermissionDenied`; `404` → `NotFound`; `408`, `429`, 5xx → `Unavailable`
  (retryable, `workflow.md`, idempotency); others (other 4xx, a 1xx, a 3xx without a
  location) → `FailedPrecondition`. The message is `HTTP <status> <reason>` and the first 512 characters of the body
  as text, with every secret of the connection replaced by `***`.
- A body that does not fit `expect` → `FailedPrecondition`, naming the field.
- A refused request (its URL, host, address, header, size, the daily limit) is refused before
  anything is sent: `InvalidArgument`, `PermissionDenied` (a host or address not allowed) or
  `ResourceExhausted` (the limit), and logged.
- A timeout → `DeadlineExceeded`; a connection that fails → `Unavailable`.

## The network rules

- *Addresses*: a host is resolved by the module, and every address it answers is checked
  before one is used: loopback, private (10/8, 172.16/12, 192.168/16, fc00::/7), carrier NAT
  (100.64/10), link-local (169.254/16 with the clouds' metadata at 169.254.169.254, fe80::/10),
  unspecified, broadcast, multicast, documentation and benchmark ranges are refused unless
  `private_networks`; an IPv4-mapped or NAT64 address is checked as the IPv4 address it holds.
  The connection is made to a checked address only (the resolver answers the client only
  those), so a name that resolves again to another address (rebinding) reaches nothing new.
- *Redirects*: followed by the module, at most 5, each one's URL and addresses checked again
  as a request without a connection would be, except that a redirect to the connection's own
  host keeps its credentials; to another host it goes without them, and only when `allow`
  names that host. A `303`, or a `301`/`302` of a `POST`, becomes a `GET` without a body.
- *Sizes*: a response body of at most 2 MiB (the body is read up to that and the call failed
  past it, `ResourceExhausted`), so a response stays within the host channel's 4 MiB message;
  a step's output above 48 KiB is a blob of the workspace (`workflow.md`, large outputs).
- *Idempotency*: a call carrying `x-pcx-idempotency-key` (a workflow's `external` block)
  sends it in the connection's `idempotency_header`, so a retry after a crash acts once where
  the API takes the key.
- *In flight*: at most 16 requests of the organisation at once; the others wait.
- *Who calls*: a request says `User-Agent: pcx-http/<version>` unless its connection's headers
  or its own give another (some APIs, GitHub's among them, refuse a request without one).

## Presets

A preset (document `http/preset`, `pcx.http.v1.Preset`, titled by its name) is a ready-made
call the editor's palette shows as an API block and the builder reads:

- its name (unique in the workspace, 1 to 64 of a-z, 0-9, '-'), its description, its service
  (a label the palette groups by: `github`, `slack`, `stripe`, `rest`);
- the connection it expects by default (a name, e.g. `github`) and, for the panel, that
  connection's form (`base_url`, the auth kind, `headers`), so "add a GitHub connection" fills
  them;
- its method (`GET` or `HEAD`: a `Get`, else a `Request`) and path with `{name}` places;
- its parameters: each a name, where it goes (`path`, `query`, `header`, `body`: a field of
  the JSON body), a type as CEL writes it, a description, an example and whether it is
  required;
- its response: a `Schema` and whether the body is a list of it, which becomes the request's
  `expect`.

Placing a preset (the editor, or the builder) writes an ordinary `tool` block: an `HttpRequest`
template with the preset's connection, method and path, and a slot per parameter in
`path_params`, `query`, `headers` or `body` for the person or the agent to fill with a constant
or `{{ }}`. Nothing at run time reads the preset: a run is the same `HttpRequest` the block holds,
so changing a preset never changes a workflow.

The built-in presets ship with the module (read-only; their names are reserved), the first
(owner 2026-11-01):

- `rest`: `rest-get` (GET of a path, any JSON) and `rest-post-json` (POST of a JSON body);
- `github` (`https://api.github.com`, bearer, `Accept: application/vnd.github+json`,
  `X-GitHub-Api-Version`): `github-repo-get`, `github-issues-list`, `github-issue-get`,
  `github-issue-create`, `github-issue-comment`, `github-pulls-list`;
- `slack` (`https://slack.com/api`, bearer): `slack-message-post` (`chat.postMessage`),
  `slack-channels-list` (`conversations.list`); Slack answers 200 with `ok: false` on a
  failure, so its presets' `expect` requires `ok` and the block's `if` reads it;
- `stripe` (`https://api.stripe.com`, bearer with the secret key, `Idempotency-Key`), reads
  only: `stripe-customers-list`, `stripe-customer-get`, `stripe-charges-list`,
  `stripe-invoices-list`.

An organisation saves its own presets (`http_presets_save`) and removes them
(`http_presets_delete`); a built-in one is neither changed nor removed (it is duplicated under
another name). Importing presets from an OpenAPI document comes later.

## The editor (owner 2026-11-01)

Adding an endpoint is filling a form, never a request message's fields. The app is a client of
the module's RPCs, with no rule of its own:

- *An HTTP block* of the workflow editor (a `tool` block on `Get` or `Request`) is edited as a
  call: the connection (from `http_connections`), the method (which picks `Get` or `Request`,
  so the effect), the path, each of whose `{places}` is a row, the query's and the headers'
  rows, the JSON body (for a method that sends one); each value a constant or a `{{ }}`. A body
  whose values are all constants is a constant; one with an expression is one `{{ }}` of a CEL
  map, edited as such.
- *Try it* calls the block's request as it is, each `{{ }}` replaced by a sample the person
  types beside it; a `Get` is sent at once, a `Request` only after the person confirms (it acts
  outside). Its answer shows; *expect from the answer* proposes `expect`'s fields from a JSON
  object (or a list's first one), each with the type its value has, the person keeping those
  the workflow reads.
- *Save as API block* keeps the block as a preset (`http_presets_save`): its service (by default
  its connection's name), its connection and its base URL, method, path, its rows as
  parameters (their constants or samples as examples), `expect` as its response. The palette
  offers it at once.
- *The API screen* adds, changes, duplicates and removes the organisation's presets with the
  same form, and duplicates a built-in one.
- *A connection's Edit* on the API screen (for who manages the workspace) opens the module's
  settings in their dialog, the form Modules shows: the connections are settings of the module,
  changed there and nowhere else (owner 2026-11-08).

## Tools

| tool | what | effect |
|------|------|--------|
| `http_get` | an `HttpRequest` whose method is `GET` or `HEAD` and that has no body (else `InvalidArgument`); answers the `HttpResponse` | `read` |
| `http_request` | an `HttpRequest` of any method; answers the `HttpResponse` | `external` |
| `http_connections` | the connections: name, description, base URL, auth kind, which secrets are set; never a secret | `read` |
| `http_test` | a connection's `test_path` with its current settings; answers the status and the time; what the panel's "test request" button calls (visibility `app`) | `read` |
| `http_presets_list` | the presets, built-in and the organisation's, by service and name | `read` |
| `http_presets_get` | one preset | `read` |
| `http_presets_save` | adds a preset or changes one from the version read, after the check (its path's places are its `path` parameters, its types parse, a built-in's name is refused) | `write` |
| `http_presets_delete` | removes one of the organisation's presets, from the version read (a built-in one is refused) | `write` |
| `http_log` | the log, newest first, a page at a time (50, at most 200, after an entry), by connection, host or outcome | `read` |

- Two tools because an RPC has one effect (`host.md`, effects): a `GET` is a read, which a dry
  run calls for real and a read-only scope may make; every other method is `external`, so the
  team's autonomy (`team.md`) decides whether an agent's call waits for a person, and a
  workflow's dry run answers its fixture instead.
- An API that changes something on a `GET` is the API's fault, not a read here: a person who
  connects one should know it; the panel says so beside the connection.

## The log

Each call (document `http/call`, `pcx.http.v1.Call`, not searchable): when, who called it (the
call's author: a person, an agent, `workflow:<name>`), the connection or none, the method, the
host, the path without its query, the status, the outcome (`ok`, `failed`, `refused`), the
failure's kind, the duration, the sizes of the request and response bodies, the idempotency
key. No header, query, body or secret is kept.

The log keeps 30 days (a module that may make 100,000 calls a day; mail keeps 90); older
entries are removed by the module once a day. The daily limit is counted in the module from
the log's entries since midnight UTC, read when it starts.

## Layers (LAW rule 6)

- **domain** (`pcx-http::domain`, pure): a request's check (method, URL or path under a
  connection, path places and their encoding, query, the refused headers, sizes, timeout), the
  connection's auth applied, the address rules, the redirect rule, a status to its failure,
  `expect` through `pcx-expr`, the redaction of secrets, a preset's check, the daily limit
  from the log.
- **ports** (`pcx-http::ports`): `Client` (send one checked request to the checked
  addresses, answer its status, headers and body up to the limit), `Resolver` (a host's
  addresses), `Clock`; the presets and the log are documents, through `pcx-documents`'
  `Documents` port, as mail's.
- **adapters**: `Client` over `reqwest` (a client per request whose host resolves only to the
  addresses the service checked, no proxy, no redirects of its own); `Resolver` over the
  system's (`tokio::net::lookup_host`); the built-in presets as a file of the crate
  (`presets.json`, a `BuiltIn`), read and checked at start.
- **capability** (`pcx-http::service`): each tool's one method; following redirects is the
  service's loop over the domain's rule.
- **module**: the manifest and the handlers, which only map; a `[[bin]]` for the cloud's
  process, as every built-in module's.

## Not in the first version

OAuth2 (client credentials and authorization code, with refresh; its own task), grants per
connection (a workflow's grants naming the connections it may use, beside the modules),
importing presets from OpenAPI, streaming responses and bodies above 2 MiB (blobs), mutual TLS,
and a proxy of the organisation.
