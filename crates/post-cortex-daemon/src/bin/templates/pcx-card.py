#!/usr/bin/env python3
"""post-cortex SessionStart hook: put the user card and the project card in
front of the session.

Claude Code adds what a SessionStart hook prints to the session's context.
This prints the user card (preferences) and the card of the project's
workspace (stack, conventions, current goals) from GET /api/cards. With no
project card yet it prints how to create one. Any failure prints nothing:
a missing card must never get in the way of the session.

Usage (from .claude/settings.json):
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-card.py <WORKSPACE_ID> <DAEMON_URL>
Set PC_AUTH_TOKEN when the daemon requires a bearer token.
"""

import json
import os
import sys
import urllib.request


def fetch(url, workspace_id):
    request = urllib.request.Request(
        "%s/api/cards?workspace_id=%s" % (url.rstrip("/"), workspace_id)
    )
    token = os.environ.get("PC_AUTH_TOKEN")
    if token:
        request.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.load(response)


def render(cards, workspace_id):
    parts = []
    for key, title in (("user", "User card"), ("workspace", "Project card")):
        card = cards.get(key) or {}
        text = (card.get("text") or "").strip()
        if text:
            parts.append("### %s (v%s)\n%s" % (title, card.get("version"), text))
    if not (cards.get("workspace") or {}).get("text", "").strip():
        parts.append(
            "No project card yet. Once you know the stack, conventions and current "
            "goals, write a short one (max 2 KB): manage_workspace action update_card, "
            "workspace_id %s, expected_version 0." % workspace_id
        )
    return "## post-cortex cards\n\n" + "\n\n".join(parts)


def main():
    if len(sys.argv) < 3:
        return
    workspace_id, url = sys.argv[1], sys.argv[2]
    print(render(fetch(url, workspace_id), workspace_id))


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # fail open: cards must not disturb the session
        print("pcx-card: %s" % exc, file=sys.stderr)
