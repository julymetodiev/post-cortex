#!/usr/bin/env python3
"""post-cortex Stop hook: block only when this turn changed the project and
never logged it.

Claude Code runs this when the assistant is about to stop and passes the hook
input as JSON on stdin (`transcript_path`, `cwd`, `stop_hook_active`). The
current turn starts after the last user prompt or task notification. The
turn is blocked when it edited files inside the project or ran
`git commit` / `git merge`, and no `update_conversation_context` call (or
`context-builder` subagent) came after the last such change.

Everything else - questions, status updates, subagent reports, edits outside
the project - passes. Any error passes too, so a broken hook never traps the
session.

Usage (from .claude/settings.json):
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-stop-check.py <SESSION_ID>
"""

import json
import os
import re
import sys

EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
GIT_CHANGE = re.compile(r"\bgit\b[^\n;&|]*\b(commit|merge|cherry-pick)\b")


def is_turn_start(entry):
    """A user prompt or task notification opens a new turn; tool results,
    meta messages and hook feedback do not."""
    if entry.get("type") != "user" or entry.get("isMeta"):
        return False
    return isinstance((entry.get("message") or {}).get("content"), str)


def tool_uses(entry):
    if entry.get("type") != "assistant" or entry.get("isSidechain"):
        return []
    content = (entry.get("message") or {}).get("content")
    if not isinstance(content, list):
        return []
    return [b for b in content if isinstance(b, dict) and b.get("type") == "tool_use"]


def inside(path, root):
    try:
        path = os.path.realpath(path)
        root = os.path.realpath(root)
    except (OSError, ValueError):
        return False
    return path == root or path.startswith(root + os.sep)


def is_change(use, project):
    name = use.get("name", "")
    args = use.get("input") or {}
    if name in EDIT_TOOLS:
        path = args.get("file_path") or args.get("notebook_path") or ""
        return bool(path) and inside(path, project) and f"{os.sep}.git{os.sep}" not in path
    if name == "Bash":
        return bool(GIT_CHANGE.search(args.get("command", "")))
    return False


def is_log(use):
    name = use.get("name", "")
    if name.endswith("update_conversation_context"):
        return True
    args = use.get("input") or {}
    return name in ("Agent", "Task") and args.get("subagent_type") == "context-builder"


def needs_log(transcript, project):
    entries = []
    with open(transcript, encoding="utf-8") as f:
        for line in f:
            try:
                entries.append(json.loads(line))
            except ValueError:
                continue
    start = 0
    for i, entry in enumerate(entries):
        if is_turn_start(entry):
            start = i + 1
    pending = None
    for entry in entries[start:]:
        for use in tool_uses(entry):
            if is_change(use, project):
                pending = use
            elif is_log(use):
                pending = None
    return pending


def describe(use):
    args = use.get("input") or {}
    if use.get("name") == "Bash":
        return "ran `%s`" % args.get("command", "").strip().splitlines()[0][:80]
    path = args.get("file_path") or args.get("notebook_path") or "?"
    return "edited %s" % os.path.basename(path)


def main():
    session_id = sys.argv[1] if len(sys.argv) > 1 else "<SESSION_ID>"
    data = json.load(sys.stdin)
    if data.get("stop_hook_active"):
        return  # already asked once this stop; never loop
    transcript = data.get("transcript_path")
    project = os.environ.get("CLAUDE_PROJECT_DIR") or data.get("cwd") or os.getcwd()
    if not transcript or not os.path.exists(transcript):
        return
    change = needs_log(transcript, project)
    if change is None:
        return
    reason = (
        "This turn %s but did not record it in post-cortex. If the change "
        "carries a decision, a fix or anything worth finding later, call "
        "mcp__post-cortex__update_conversation_context with session_id %s "
        "(>=1 entity, >=1 relation). If it is trivial (typo, formatting), "
        "say so in one line and stop."
    ) % (describe(change), session_id)
    print(json.dumps({"decision": "block", "reason": reason}))


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # fail open: a hook bug must not block work
        print("pcx-stop-check: %s" % exc, file=sys.stderr)
