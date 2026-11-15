# mail

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8. Approved by the
owner on 2026-10-31 (wave 3, board `pcx-cloud`, plan `f364d976`, task `e51c6dea`), with the
plan's decisions of the same day.

- **`mail`**: sends mail with the organisation's own credentials, from templates kept per
  workspace. Crate `pcx-mail`, built in (the daemon in process, the cloud's catalog as a
  process); proto package `pcx.mail.v1`; uses `post-cortex.memory` (its templates and its log
  are documents).
  - Agents, workflows (a `tool` block, `workflow.md`) and other modules reach it only through
    its tools; a scheduler job sends a report through a workflow or a task (below).
  - The cloud sends its own mail (verification, sign-in links, invitations, notices) with the
    same library, its system templates and our credentials (below): there is one path for
    mail.

## Settings

`pcx.mail.v1.MailConfig`, edited in the panel as any module's (`config_schema`; one path for
the daemon and the cloud):

- `workspace_id`: the workspace its templates and log are kept in, the organisation's store
  (owner 2026-11-10; docs/connect.md section 4a, a module's data): every caller, from any
  workspace, reads them and sends, each send logged there by the module; a template is saved
  by a caller who reaches that workspace;
- `from`: the sender's address and its name; `reply_to`: optional;
- `transport`, one of:
  - `smtp`: host, port, security (`starttls`, `tls` or `none`; `none` only to a loopback
    host), user, password;
  - `resend`: an API key;
  - `postmark`: a server token and the message stream (default `outbound`);
  - SES through its SMTP interface (`email-smtp.<region>.amazonaws.com`, SES SMTP
    credentials) as an `smtp` transport, so there is no AWS signing in the module; SES's HTTP
    API comes only if a need for it does;
- `allow`: the recipients it may send to, each an address or `@domain`; empty: any (so a
  person narrows whom an agent with the tool can mail);
- `daily_limit`: messages a day (default 200, at most 10,000; in the cloud the plan's limit
  caps it).

Passwords, keys and tokens are `secret` settings (write-only, sealed for the organisation and
the module); the panel shows only that they are set (`secrets_set`).

## Templates

A template (document `mail/template`, `pcx.mail.v1.Template`, titled by its name):

- its name (unique in the workspace, 1 to 64 of a-z, 0-9, '-'), its description;
- its inputs: each a name (a CEL identifier: a lowercase letter or `_`, then letters, digits
  and `_`), a type as CEL writes it (`string`, `int`, `double`, `bool`, `timestamp`,
  `list<T>`, `map<string, T>`), a description and an example, which is required (what the
  panel's preview and a test fill it with);
- its subject, its HTML body and its text body, each a template of text and `{{ <CEL> }}`
  parts (`workflow.md`, templates and expressions); the simple case is `{{ name }}`.

Filling (one function of the domain, the same `{{ }}` and CEL as the workflows'):

- on save, every expression is parsed and type-checked against the inputs, and the limits of
  `workflow.md` hold (4 KiB an expression, its AST at most 1,000 nodes and 32 deep); a
  failure names the part (`subject`, `html`, `text`) and the expression's place;
- on send, the values (a JSON object, `google.protobuf.Struct`; a whole number for `int`, an
  RFC 3339 string for `timestamp`) are checked against the inputs: a missing input or one of
  another type
  is a typed failure (`InvalidArgument`, naming the input), never an empty place; an input not
  declared is refused too;
- each value put into the HTML body is HTML-escaped; into the subject and the text body it is
  put as it is, a subject's line breaks refused (header injection);
- an evaluation has the budget of steps of `workflow.md` (100,000).

The template engine (the `{{ }}` parser, the CEL check and evaluation under limits, the value
conversion) is its own pure crate, `pcx-expr` (owner 2026-10-31), used by `pcx-mail` and
`pcx-workflow`: mail is built first, and no module crate depends on another module's crate for
its domain. `cel-core` is reached only through it.

## Tools

| tool | what | effect |
|------|------|--------|
| `mail_send` | recipients (to, cc, bcc; at least one `to`, at most 50 in all), and either a template with its values or a subject (one line, at most 998 characters) with a text body (and an optional HTML body, with no `{{ }}`; each body at most 1 MiB); answers the log entry | `external` |
| `mail_test` | a fixed message to one address with the module's current settings; what the panel's "send a test mail" button calls (visibility `app`) | `external` |
| `mail_templates_list` | the templates, by name | `read` |
| `mail_templates_get` | one template | `read` |
| `mail_templates_save` | adds a template or changes it from the version read, after the check | `write` |
| `mail_templates_preview` | a template (saved, by name, or one being edited) filled with its examples or given values, nothing sent | `read` |
| `mail_log` | the log, newest first, a page at a time (50, at most 200, after an entry), by template or recipient | `read` |

- Sending from an agent is an `external` call, so the team's autonomy (`team.md`) decides
  whether it waits for a person, as for any action with an outside effect.
- *Idempotency*: a call carrying `x-pcx-idempotency-key` (a workflow's retry,
  `workflow.md`) whose key the log already has answers that entry and sends nothing; the key
  goes on to Resend as its `Idempotency-Key`. It comes with the workflow engine, which carries
  the key in the call's scope; until then no caller sends one.
- Sends pass one at a time, so two never both take the daily limit's last place.
- A recipient outside `allow`, or a send past `daily_limit`, is refused before anything is
  sent (`PermissionDenied`, `ResourceExhausted`), and the refusal is logged.

## The log

Each send (document `mail/sent`, `pcx.mail.v1.Sent`, not searchable: the
`DocumentKind.searchable` that `workflow.md` adds to memory, built here first): when, who sent
it (the call's author), the template or none, the recipients, the subject, the outcome (`sent`,
`refused`, `failed`), the transport's message id or its error, the idempotency key. The bodies
and the values are not kept (they may carry what a recipient alone should read).

The log keeps 90 days; older entries are removed by the module once a day. The
daily limit counts the log's `sent` entries since midnight UTC.

## The cloud's own mail

- The control plane's `Mailer` port has one adapter over `pcx-mail`'s library (its transports,
  a template checked and filled, a message checked): each email is a kind and its values
  (`Email::values`), filled from its system template, a file of the cloud repository
  (`<kind>.txt`: `Subject: ...`, a blank line, the text body), with its inputs declared beside
  it and checked at start the way a saved template is. Today's `[mail]` keeps its form: `smtp`
  is the mail module's SMTP transport (465: TLS, else STARTTLS) and `dev-files` a transport
  that writes each message to a file; the capture for tests stays a double of the `Mailer`
  port, keeping each email's kind and values.
- It is no tenant's module and needs no tenant: the control plane sends before an
  organisation exists (sign up). An organisation's own mail goes through its own module, with
  its own credentials, never ours.
- Our mail is sent through Migadu's SMTP as `no-reply@pcx.is` (owner 2026-10-31): the domain's
  MX, SPF, DKIM and DMARC are Migadu's.

## Scheduler and workflows

A job of the scheduler that mails is a `workflow` job (`workflow.md`, triggers) whose workflow
has a `tool` block on `/pcx.mail.v1.Mail/Send`; until the workflows run, a `task` job assigns
the work to a member with the tool. The scheduler gets no mail action of its own.

## Layers (LAW rule 6)

- **domain** (`pcx-mail::domain`, pure; `pcx-expr`): a template's check, filling and
  escaping, an address and `allow`, the limit from the log.
- **ports** (`pcx-mail::ports`): `Transport` (send a built message; answers its id),
  `Templates` and `Log`, `Clock`.
- **adapters**: `Transport` over SMTP (`lettre`), Resend and Postmark (HTTP, their JSON as
  mirror messages, as telegram's Bot API), the cloud's files and capture; `Templates` and
  `Log` over memory's documents (`pcx-documents`).
- **capability** (`pcx-mail::service`): each tool's one method.
- **module**: the manifest and the handlers, which only map.

## Not in the first version

Attachments (blobs of the workspace, for invoices; owner 2026-10-31: later), receiving mail (a `mail`
trigger of the workflows), bounces and complaints from the providers' webhooks (into the log).
