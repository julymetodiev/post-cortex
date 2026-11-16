#!/usr/bin/env python3
"""post-cortex SessionStart hook: put the user card and the project card in
front of the session.

Claude Code adds the `additionalContext` of a SessionStart hook's answer to
the session's context. This sends the event to the daemon (POST
/api/events), whose memory module answers with the user card (preferences)
and the card of the project's workspace (stack, conventions, current
goals), or how to create the project card, and prints the answer. Any
failure prints nothing: a missing card must never get in the way of the
session.

Usage (from .claude/settings.json, `pcx setup --project`):
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-card.py <WORKSPACE_ID> <DAEMON_URL> [<PROFILE>]
The bearer token is PC_AUTH_TOKEN, else the key `pcx login` stored for
<PROFILE> (the hosted cloud's setups pass it).

Without arguments (the Claude Code plugin) the backend and the project are
resolved by pcx_resolve.py, and the session and workspace ids are printed
too, since no CLAUDE.md carries them.
"""

import json
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the hooks
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    import pcx_resolve
except ImportError:  # an older copy of the hooks without the resolver
    pcx_resolve = None


def answer_with_ids(answer, ids):
    """The answer with the ids line first (the plugin: no CLAUDE.md carries
    the ids)."""
    context = pcx_resolve.added_context(answer)
    if not context:
        return answer
    return {
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": pcx_resolve.ids_line(ids) + "\n\n" + context,
        }
    }


def main():
    if pcx_resolve is None:
        return
    args = sys.argv[1:]
    payload = pcx_resolve.read_payload(sys.stdin)
    event = dict(payload, hook_event_name="SessionStart")
    if args:
        if len(args) < 2:
            return
        url, token, _ = pcx_resolve.resolve_server(args[1], args[2] if len(args) > 2 else None)
        answer = pcx_resolve.post_event(url, token, {"workspace_id": args[0]}, event)
    else:
        context = pcx_resolve.global_context(__file__, payload)
        if context is None:
            return
        answer, ids = pcx_resolve.post_project_event(context, payload, "workspace_id", event)
        answer = answer_with_ids(answer, ids)
    if pcx_resolve.added_context(answer):
        print(json.dumps(answer))


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # fail open: cards must not disturb the session
        print("pcx-card: %s" % exc, file=sys.stderr)
