#!/usr/bin/env python3
"""post-cortex file hook: tell the agent what the project's memory says about
a file once a session, as early as it can be heard (memory module, owner
2026-11-18).

Claude Code runs this after Read and the read tools of MCP servers
(PostToolUse), and before Edit, Write and MultiEdit and the edit tools of MCP
servers (PreToolUse: Lantern Lattice's, Mimir's, post-cortex's own), with the
tool call on stdin. For each file the call names (`file_path`, else `file` or
`path` and those of its `edits`; a relative path is the project's), up to
FILES, it sends the event to the daemon (POST /api/events) with that file as
`file_path` and the session's id; the memory module answers with the problems
solved in that file, the decisions and requirements about it, what depends on
it (each with why) and the code it was changed with, once per session and
file:
- after a read, as additional context the agent reads with the file;
- before an edit of a file it was not told about, as a block: the hook denies
  the call with that text as the reason, the agent reads it before anything
  is written and sends the edit again, which then goes through.
Nothing known about the files, a file told already, or any failure prints
nothing: the hook never gets in the way of an edit otherwise.

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
EVENTS = ("PreToolUse", "PostToolUse")


def files_of(tool_input, project):
    """The files a tool call names, absolute, in order, without repeats."""
    parts = [tool_input] + [e for e in tool_input.get("edits") or [] if isinstance(e, dict)]
    found = []
    for part in parts:
        path = part.get("file_path") or part.get("file") or part.get("path")
        if isinstance(path, str) and path.strip():
            path = os.path.normpath(os.path.join(project, path.strip()))
            if path not in found:
                found.append(path)
    return found[:FILES]


def output(name, texts, reasons):
    """The hook's answer: a denied edit with the held files' memory as the
    reason (and any memory told without a hold as context), or the memory as
    context; None when there is nothing to say."""
    out = {"hookEventName": name}
    if reasons:
        out["permissionDecision"] = "deny"
        out["permissionDecisionReason"] = "\n\n".join(reasons)
    if texts:
        out["additionalContext"] = "\n\n".join(texts)
    return {"hookSpecificOutput": out} if len(out) > 1 else None


def main():
    if pcx_resolve is None:
        return
    args = sys.argv[1:]
    event = pcx_resolve.read_payload(sys.stdin)
    name = event.get("hook_event_name")
    if name not in EVENTS:
        return
    tool_input = event.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return
    files = files_of(tool_input, pcx_resolve.project_dir(event))
    if not files:
        return
    if args:
        if len(args) < 2:
            return
        url, token, _ = pcx_resolve.resolve_server(args[1], args[2] if len(args) > 2 else None)
        context = None
    else:
        context = pcx_resolve.global_context(__file__, event)
        if context is None:
            return
    # The read's result stays here: the memory needs the file's path only.
    base = {k: v for k, v in event.items() if k != "tool_response"}
    texts, reasons = [], []
    for path in files:
        one = dict(base, hook_event_name=name, tool_input={"file_path": path})
        if context is None:
            answer = pcx_resolve.post_event(url, token, {"workspace_id": args[0]}, one, timeout=3)
        else:
            answer, ids = pcx_resolve.post_project_event(context, event, "workspace_id", one, timeout=3)
            context = context[:2] + (ids,)
        reason = pcx_resolve.block_reason(answer) if name == "PreToolUse" else ""
        if reason:
            reasons.append(reason)
        text = pcx_resolve.added_context(answer)
        if text:
            texts.append(text)
    answer = output(name, texts, reasons)
    if answer:
        print(json.dumps(answer))

if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # fail open: memory must not block an edit
        print("pcx-file-context: %s" % exc, file=sys.stderr)
