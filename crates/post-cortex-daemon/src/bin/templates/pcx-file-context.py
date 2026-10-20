#!/usr/bin/env python3
"""post-cortex PreToolUse hook: before a file is edited, tell the agent what
the project's memory says about it.

Claude Code runs this before Edit, Write and MultiEdit with the tool call on
stdin. It asks the daemon (GET /api/file) for the problems solved in that
file, the decisions and requirements about it, and what depends on the
entities naming it, and returns them as additional context. Nothing known
about the file, or any failure, prints nothing: the hook must never get in
the way of an edit.

Usage (from .claude/settings.json):
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-file-context.py <WORKSPACE_ID> <DAEMON_URL>
Set PC_AUTH_TOKEN when the daemon requires a bearer token.
"""

import json
import os
import sys
import urllib.parse
import urllib.request

PER_LIST = 3


def fetch(url, workspace_id, path):
    query = urllib.parse.urlencode(
        {"workspace_id": workspace_id, "path": path, "limit": PER_LIST}
    )
    request = urllib.request.Request("%s/api/file?%s" % (url.rstrip("/"), query))
    token = os.environ.get("PC_AUTH_TOKEN")
    if token:
        request.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(request, timeout=3) as response:
        return json.load(response)


def line(record):
    when = (record.get("timestamp") or "")[:10]
    text = record.get("title") or ""
    description = (record.get("description") or "").strip()
    if description:
        text += ": " + description[:200]
    validity = record.get("validity") or {}
    if validity.get("valid_to"):
        text += " (no longer holds: %s)" % (validity.get("reason") or "ended")
    return "- %s %s [entry %s]" % (when, text, record.get("id"))


def render(view, path):
    parts = []
    if view.get("problems"):
        parts.append(
            "Problems solved here before:\n"
            + "\n".join(line(r) for r in view["problems"][:PER_LIST])
        )
    if view.get("decisions"):
        parts.append(
            "Decisions and requirements about it:\n"
            + "\n".join(line(r) for r in view["decisions"][:PER_LIST])
        )
    dependents = view.get("dependents") or []
    if dependents:
        names = ", ".join(d["entity"] for d in dependents[:10])
        parts.append("Depends on it: %s" % names)
    if not parts:
        return None
    return "post-cortex memory for %s:\n\n%s" % (path, "\n\n".join(parts))


def main():
    if len(sys.argv) < 3:
        return
    workspace_id, url = sys.argv[1], sys.argv[2]
    event = json.load(sys.stdin)
    path = (event.get("tool_input") or {}).get("file_path")
    if not path:
        return
    context = render(fetch(url, workspace_id, path), path)
    if context:
        print(
            json.dumps(
                {
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "additionalContext": context,
                    }
                }
            )
        )


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # fail open: memory must not block an edit
        print("pcx-file-context: %s" % exc, file=sys.stderr)
