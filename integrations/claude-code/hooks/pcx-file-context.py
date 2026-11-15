#!/usr/bin/env python3
"""post-cortex PreToolUse hook: before a file is edited, tell the agent what
the project's memory says about it.

Claude Code runs this before Edit, Write and MultiEdit, and before the edit
tools of MCP servers (Lantern Lattice's, Mimir's, post-cortex's own), with the
tool call on stdin. For each file the call changes (`file_path`, else `file`
or `path` and those of its `edits`; a relative path is the project's), up to
FILES, it sends the event to the daemon (POST /api/events) with that file as
`file_path`; the memory module answers with the problems solved in that
file, the decisions and requirements about it, and what depends on the
entities naming it, as additional context. Nothing known about the files, or
any failure, prints nothing: the hook must never get in the way of an edit.

Usage (from .claude/settings.json):
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-file-context.py <WORKSPACE_ID> <DAEMON_URL> [<PROFILE>]
The bearer token is PC_AUTH_TOKEN, else the key `pcx login` stored for
<PROFILE> (the hosted cloud's setups pass it). Without arguments (the
Claude Code plugin) the backend and the project are resolved by
pcx_resolve.py.
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

FILES = 3  # files of one call whose memory is told


def files_of(tool_input, project):
    """The files a tool call changes, absolute, in order, without repeats."""
    parts = [tool_input] + [e for e in tool_input.get("edits") or [] if isinstance(e, dict)]
    found = []
    for part in parts:
        path = part.get("file_path") or part.get("file") or part.get("path")
        if isinstance(path, str) and path.strip():
            path = os.path.normpath(os.path.join(project, path.strip()))
            if path not in found:
                found.append(path)
    return found[:FILES]


def main():
    if pcx_resolve is None:
        return
    args = sys.argv[1:]
    event = pcx_resolve.read_payload(sys.stdin)
    tool_input = event.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return
    files = files_of(tool_input, pcx_resolve.project_dir(event))
    if not files:
        return
    if args:
        if len(args) < 2:
            return
        workspace_id = args[0]
        url, token, _ = pcx_resolve.resolve_server(args[1], args[2] if len(args) > 2 else None)
    else:
        context = pcx_resolve.global_context(__file__, event)
        if context is None:
            return
        url, token, ids = context
        workspace_id = ids["workspace_id"]
    texts = []
    for path in files:
        one = dict(event, hook_event_name="PreToolUse", tool_input={"file_path": path})
        answer = pcx_resolve.post_event(url, token, {"workspace_id": workspace_id}, one, timeout=3)
        text = pcx_resolve.added_context(answer)
        if text:
            texts.append(text)
    if texts:
        print(json.dumps({"hookSpecificOutput": {
            "hookEventName": "PreToolUse", "additionalContext": "\n\n".join(texts)}}))

if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # fail open: memory must not block an edit
        print("pcx-file-context: %s" % exc, file=sys.stderr)
