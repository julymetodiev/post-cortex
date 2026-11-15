# telegram

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8, operations.

- **`telegram`** (W29): the first channel (`channel` kind). Crate `pcx-telegram`, built in;
  uses `board`, `chief` and `post-cortex.memory`; its settings: the bot token (a secret), its
  chats by name (each a destination), the people it knows by Telegram user id, the persons it
  acts for (`PersonGrant`, below; each of its people must be one), the approvals chat.
  - *The channel service* (`pcx.channel.v1.Channel`, `Send`: a destination by name and the
    text; answers the message's id). The webhooks tell a source's `notify` destination of a
    new incident at once (`ReceiveResponse.told`); a channel that fails does not fail the
    request: it is logged. `Send` is the tool `channel_send` (effect `external`), so a
    workflow tells a destination with a `tool` block and an agent granted the module with
    a tool call; the scheduler tells no one itself (owner 2026-11-11: a job that should
    tell people runs a workflow, docs/modules/scheduler.md). Several modules may serve the service (owner
    2026-11-13: `discord` too; docs/host.md, a service several modules serve): a sender's
    destination is `<module>/<name>` (`telegram/ops`), its module the call's target; a plain
    name goes to the one channel that runs, and is refused when several do. The senders `use`
    the modules they tell (`telegram`, `discord`).
  - *Approvals*: on a memory write of `board/decision` the channel reads it through the
    board (`GetTask`); one that waits for a person (`DecisionRecord.pending`) is sent to the
    approvals chat once, with what, why, the task, who decided and what it risks,
    why the policy did not approve it, and two buttons. A decision has one message: written
    again while it waits (the board writes it twice as it asks: decided, then why it is held),
    its message is edited when it says something else and its buttons approve its newest
    version; the writes are acted on one at a time, so two heard together send one message
    (fixed 2026-11-11: each version was sent, and after a press on one the other kept its
    buttons and answered that its prompt is gone). Approve calls `approve_decision` as
    the person who pressed (the note "approved in Telegram"); Decline calls
    `decline_decision` as that person (the note "declined in Telegram"; 2026-11-11: it was a
    comment and the decision kept waiting). A press by anyone
    the channel does not act for changes nothing and says so. The message says how the
    decision ended and its buttons go, also when it ended elsewhere: approved or declined in
    the app, or withdrawn by who asked it (2026-11-11: a withdrawn decision's message kept
    its buttons, which answered that the prompt is gone). A button carries the prompt's number; the
    decision behind it is kept while the module runs (an older button answers that its
    prompt is gone: the app approves it still).
  - *Acting for a person* (owner 2026-10-27): a module's call of a tool only a person calls
    (visibility `app`) is refused, except when the call's author is one of the persons its
    settings name in a field of type `pcx.module.v1.PersonGrant` (as `TurnGrant` grants a
    module's turns), which the host reads with the configuration and enforces
    (`Reach::Own`). Only a person configures a module, so only a person lets a module act
    for persons.
  - *Talking*: a message in one of its chats from one of its people is a question to the
    chief (`pcx.chief.v1.Chief/Ask` as that person, the conversation kept per chat while the
    module runs), and the answer is sent back. A member's chat (`@name`) waits for a team
    RPC a module can call.
  - *The Bot API*: its calls go as form fields (its form encoding: every value text,
    `reply_markup` the JSON of the buttons), each field the proto3 JSON of the call's mirror
    message; its answers are decoded into their mirror messages. The one shape a proto field
    cannot hold, `inline_keyboard`'s array of rows, is a serde type of the adapter around
    the mirror buttons. Updates are read by long polling (30 s); the token never appears in
    an error.
