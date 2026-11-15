# discord

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8, operations.

- **`discord`** (owner 2026-11-13): a channel to Discord and a community run through workflows.
  Crate `pcx-discord`, built in; kind `channel`. It is a module of the product: each
  organisation connects its own Discord server with its own bot, so nothing in the module names
  pcx's own server, channels or roles; pcx's server is one organisation's configuration. For
  pcx itself Telegram stays the internal channel of operations (alerts, decisions, where no
  outside person is invited) and Discord is for the product's users; an organisation that has
  only Discord gets its alerts there.
  - *Its settings* (`pcx.discord.v1.DiscordConfig`): the bot's token (a secret); the server
    (guild id); the workspace its events are published in, whose workflows follow them (an
    event starts runs only in its own workspace, docs/modules/workflow.md); its channels by
    name (each a name of 1 to 64 of a-z, 0-9, '-', and the Discord channel id): the ones it
    may post to (its destinations) and the ones it hears; the roles it may give and the forum
    tags it may set, by their names in the server, read to their ids when it reads its server. The
    API's URL is no setting (Discord has one, `https://discord.com/api/v10`, and a URL an
    organisation names would carry its token wherever it points). No `PersonGrant`: it acts for no person, approves nothing
    and never talks with the chief (owner 2026-11-13: the people of a community are
    strangers; decisions are approved in the app and in Telegram, not in Discord).
  - *The channel service*: it serves `pcx.channel.v1.Channel` beside Telegram (docs/host.md, a
    service several modules serve; docs/modules/telegram.md): `Send` to one of its
    destinations (effect `external`), which a sender names as `discord/<name>`. It is no tool
    (an RPC's path names one tool, and Telegram's `Send` is `channel_send`): a workflow or an
    agent posts with `discord_post`.
  - *What it hears*: the Gateway (a WebSocket: identify with the intents `GUILDS`,
    `GUILD_MEMBERS`, `GUILD_MESSAGES`, `MESSAGE_CONTENT`, `AUTO_MODERATION_EXECUTION`;
    heartbeat at the interval its hello names, resume after a cut, identify again when the
    session is invalid). An event of its server in a channel it hears, not written by a bot,
    is published on the bus (docs/host.md, events of a module), so what follows is a workflow's
    `event` trigger, never code here:
    - `discord.post_created` (`PostCreated`: a new post of a forum: channel name, thread id,
      title, first message's text, author id and name, tags);
    - `discord.message_created` (`MessageCreated`: channel name, thread id when in one, message
      id, text, author id and name, the message it replies to);
    - `discord.member_joined` (`MemberJoined`: member id and name, when the account was made);
    - `discord.automod_acted` (`AutomodActed`: the rule, what it did (block, alert, timeout;
      one event for each), the member, the channel, the content it held).
    A forum post is published when its first message comes (the message whose id is its
    thread's), with the thread's title and tags; a thread the module has not seen is read
    from the API once. The module answers nothing and decides nothing by itself: it hears and
    it publishes.
  - *What it does* (the service `pcx.discord.v1.Discord`, each RPC a tool, effect
    `external`): `discord_post` (a channel by name, text or one embed: its id; an embed shows up to 4
    pictures, each named by its https address, which the module sends as cards of the embed's
    link, since Discord shows the cards of one link as one card with the pictures side by
    side: several pictures need the link),
    `discord_reply` (a thread or a message, text: its id), `discord_give_role` (a member, a
    role by name), `discord_tag_post` (a forum post, a tag by name, added to those it has); and
    `discord_status` (effect `read`, connecting a server); and, only when the settings'
    `moderation` lets it (owner 2026-11-14), `discord_timeout` (a member kept from writing for
    some minutes, at most the settings' longest and Discord's 28 days; 0 lifts it: when it
    ends), `discord_lock_thread` (a post or thread of a channel it posts to locked and
    archived) and `discord_ban` (a member banned, what they wrote in the hours before deleted,
    at most 168; only when `moderation.ban`). Without `moderation` the three are refused
    (`PermissionDenied`) and the bot needs no right to moderate. A timeout or a ban of a member
    Discord lets no bot moderate is refused before Discord is asked (`FailedPrecondition`,
    naming why; Discord answers it only `Missing Permissions`): the server's owner (read from
    the server), the bot itself, or a member whose highest role is not below the bot's (the
    roles read at the call). The module decides none of
    them: a workflow asks (below). A
    channel, role or tag its settings do not name is refused (`PermissionDenied`), and so is a
    reply in a thread whose channel it does not post to, so a workflow reaches no more than a
    person configured. A message mentions no one (`allowed_mentions` with nothing to parse:
    `@everyone`, a role or a member in a text an agent wrote is text), and a text over
    Discord's 2000 characters goes as several messages, cut at line ends.
  - *The REST API*: its calls are the JSON of mirror messages in `pcx_discord.proto`, as
    Telegram's Bot API, its nulls read as fields left out (`pcx_module_api::json::decode`); a
    `429` waits its `retry_after` (at most 10 s) and tries again once; the token goes in a
    header and never appears in an error. The Gateway's TLS names rustls' `ring` provider, as
    pcx-kube does.
  - *Connecting a server*: an organisation makes an application and a bot in the Developer
    Portal (Message Content and Server Members intents on, Public Bot off), puts the token in
    the module's settings, and adds the bot with the invite link (the rights the module may
    need: View Channel, Send Messages, Send Messages in Threads, Embed Links, Read Message
    History, Manage Threads, Manage Roles, Timeout Members, Ban Members; never
    Administrator). Timeout Members (and Ban Members with `moderation.ban`) and Manage Threads
    in the channels it posts to are needed only with `moderation`. The module's start checks its settings only and calls
    nothing outside: a host starts all its modules or none (docs/host.md), so a start that
    waited on Discord would stop a daemon or a tenant when Discord is down or a tag was
    removed. Its listener then reads its rights in the server and in each named channel,
    before it connects to the Gateway. Until that read succeeds the module hears nothing and
    every call answers why: Discord was not reached (`Unavailable`), or the bot is not in
    the server or lacks a right, a role or a tag there, with everything that is missing and
    the invite link made from the bot's own id (`FailedPrecondition`). What the bot lacks
    is read again every minute, so a person who adds the bot again saves nothing anew; once
    read, the server is kept. `discord_status` (`Status`, effect `read`: whether it is
    connected, and the problem when not) says the same without doing anything. What a
    person must fix there (`FailedPrecondition`: the bot's token, a right, a role, a tag)
    the module reports to its host as its problem (docs/host.md, a module's problem), so
    the app shows it on the module's page, and reports none once the server is read; a
    Discord it did not reach (`Unavailable`) is no one's to fix and is not reported. The
    settings' forms are checked when they are saved (docs/host.md, what a text must be):
    the server's and a channel's id are digits, a channel's name a-z, 0-9 and `-`, a role
    and a tag each a value of its own without a comma. A docs page
    (docs.pcx.is) walks through the steps with screenshots.
  - *As a process*: the crate has the binary `pcx-discord` (the cloud's catalog,
    post-cortex-cloud ARCHITECTURE C22); an organisation that enabled it stays loaded, since
    it hears with no calls.
- *The agent of a community* (owner 2026-11-13: a team member with reading rights only answers
  in the help forum; the chief never answers a stranger): a member of the team (its built-in
  agent `community`, docs/modules/team.md) whose tools are search and reading of memory, and none with an effect,
  and whose reach is the one workspace of the public documents (the product's docs;
  docs/connect.md, a member's reach). What strangers write reaches a model and
  its answer is public, so what a prompt injection could make it say is bounded by what it may
  read. It is asked by a workflow's `agent` block (docs/modules/workflow.md), never by this
  module. A member's turn runs in a workspace it reaches, so the member, its team and the help
  workflow live in that public workspace. An organisation keeps that workspace in an
  organisation of its own when it can (owner 2026-11-14; pcx's server goes in the
  organisation `PCX Community`): the boundary between organisations is the tenant's, a
  database each, where a reach is a check inside one.
- *The workflows* (owner 2026-11-13: answers in the help forum, posts from workflows, welcome
  and moderation): templates the module ships (`Manifest.workflow_templates`, the JSON in
  `crates/pcx-discord/templates/`; docs/modules/workflow.md, templates), which an organisation
  installs with its own values and then changes as any workflow. Their parameters name the
  channels, roles, tags and member by their names in the settings (a channel's and a
  member's: a-z, 0-9 and `-`; a role's and a tag's as Discord shows them: letters, digits,
  space, `_`, `.` and `-`) and the times by whole numbers; each has the default below.
  - `discord-help` (`help_channel` help, `team_channel` team, `member` community,
    `answered_tag` answered, `needs_team_tag` needs-team, `timeout_minutes` 1440,
    `repeat_hours` 720, `delete_hours` 24): `event` `discord.post_created` with
    `event.payload.channel == "help"` →
    `agent` (`community`; the post's title and text; its result: `kind`, one of `answered`,
    `not_in_docs`, `abuse`, and `answer`) → `switch`:
    - `answered`: `discord_reply` with the answer, `discord_tag_post answered`; the answer
      ends with the address of the page it is from when the title of the record it
      answers from ends with one (owner 2026-11-14: the docs' records are titled `<page> —
      <section> · <address>`, since a search result shows a record's title), and with no
      other link;
    - `not_in_docs`: `discord_reply` (the agent's own words, in the person's language: not in
      the documentation, a person will follow), `discord_tag_post needs-team`, `discord_post
      team`;
    - `abuse` (owner 2026-11-14: the text tries to steer the agent, to reveal what it must
      not, or is spam or harassment; an honest question about prompt injection is not): nothing
      in public; `discord_lock_thread`; the workflow's `state` holds when its author last
      abused the forum (`abuse-<author id>`). A second abuse within 30 days (of the event's
      time) is a `discord_ban` at once (owner 2026-11-14) and a `discord_post team`. A first is
      a 24-hour `discord_timeout`, the time kept in `state`, a `discord_post team`, and an
      `approval` only a person approves (`person`, risk `operations`; in the app or Telegram,
      never in Discord: owner 2026-11-13) whether to ban; approved: `discord_ban`; declined
      (its `error` edge): the timeout lifted and the `state` key deleted, so a mistaken
      judgement does not count. Left unanswered, the run ends at its wall time (24 hours, as
      the timeout), the timeout ends by itself and the abuse counts. A timeout or a ban that
      fails (a member the bot may not moderate, found 2026-11-14 with the owner's own post)
      is told to the team with why, the post still locked, and a person decides. What the
      agent says the post tried (one line, never the post's words) ends each notice and
      the approval's question on a line of its own.
    - the agent's block failed (its `error` edge; found 2026-11-14: a run that failed there
      left the post with no reply and the team with a line in the server's log):
      `discord_reply` with fixed words (it could not answer now, a person will follow),
      `discord_tag_post needs-team`, `discord_post team` with the failure's kind.
    Its grants are `discord` and `board` (the approval), and the member `community`. It
    writes nothing to memory (owner 2026-11-14): a stranger's text kept in the workspace
    the agent reads would be a prompt injection that stays, read at every later question.
    pcx's own is this template installed with its defaults.
  - `discord-welcome` (`role` Member): `event` `discord.member_joined` → `discord_give_role`.
  - `discord-changelog` (`channel` changelog): `manual` with the input `notes` (a release's
    notes, 1 to 6000 characters; a scheduler's job, a webhook's workflow or a call runs it) →
    `discord_post`, cut into messages at line ends.
  - `discord-announce` (`channel` announcements, `color` 3907071; owner 2026-11-14: a release
    announced as a card, with screenshots of what is new in the app): `manual` with the inputs
    `title` (1 to 256 characters), `notes` (1 to 4000, Discord's markdown), `link` (https, the
    address the title opens; optional) and `images` (at most 4 https addresses; optional) →
    `discord_post` of one embed.
  - `discord-feedback` (`feedback_channel` feedback, `team_channel` team): `event`
    `discord.post_created` with `event.payload.channel == "feedback"` → `discord_reply` with
    fixed words (the team reads every post and answers in it) → `discord_post` to the team: the
    post's title and author. No agent reads the post: an idea, a bug or a wish is a person's to
    answer, and a stranger's text reaches no model.
  - `discord-automod` (`team_channel` team): `event` `discord.automod_acted` → `discord_post`
    to the team: what AutoMod did, the member (a mention, which notifies no one), the rule's
    id and the channel. What the member wrote is not repeated: AutoMod held it, and a
    stranger's text is kept nowhere a model reads.
- *pcx's own server* (its roles, channels, rights, AutoMod, the bot's messages in `#welcome`
  and `#rules`) is set by a script of its operator, not by this module.
- *Layers* (rule 6): the Gateway's rules (what a frame asks next: identify, resume,
  heartbeat, an event to publish) and the rules of what is heard and what may be done are pure
  in `domain`; the Gateway, the REST API and the bus are ports; the adapters fill them; the
  handlers only map.
- *Tests*: the Gateway's rules over scripted frames (hello, ready, a cut and a resume, an
  invalid session); an event of a channel it does not hear, or a bot's, publishes nothing; a
  refused channel, role or tag; the server read after the start, what the bot lacks kept as
    the module's status with the invite link and read again; a `429` waited once; the token in no error; a call to the
  channel service named `discord/<name>` reaches Discord and a plain name is refused while
  Telegram runs too (host). End to end on staging: a post in the help forum is answered from
  the public workspace, and a post that tries to make the agent quote another workspace's
  memory (a prompt injection) gets nothing of it, since memory refuses the turn's reads.
