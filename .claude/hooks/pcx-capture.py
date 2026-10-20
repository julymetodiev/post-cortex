#!/usr/bin/env python3
"""post-cortex Stop hook: capture the conversation into the session's
message stream (POST /api/messages).

Claude Code runs this when the assistant stops and passes the hook input as
JSON on stdin (`transcript_path`, `session_id`). The last few turns of the
transcript are sent: each user prompt (and any message the person sent while
the assistant was working), and the assistant's final text of the turn (the
text after its last tool call) with the names of the tools it used and the
files they touched. Tool outputs are never sent. Turns already
stored are skipped by the daemon (idempotent on the transcript entry uuid),
so resending recent turns also covers a turn whose Stop never fired.

The request runs in a detached child process, so the hook returns at once,
and any failure is silent: capture must never get in the way of the session.

Usage (from .claude/settings.json):
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-capture.py <SESSION_ID> <DAEMON_URL>
Set PC_AUTH_TOKEN when the daemon requires a bearer token. `--dry-run`
prints the request body instead of sending it.
"""

import json
import os
import sys
import urllib.request

TURNS = 3
AUTHOR = "claude-code"
FILE_ARGS = ("file_path", "notebook_path")
# String prompts Claude Code writes itself rather than the person.
SYNTHETIC_PREFIXES = (
    "<task-notification>",
    "<command-name>",
    "<command-message>",
    "<local-command-",
    "<system-reminder>",
    "[Request interrupted",
)


def read_entries(path):
    entries = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            try:
                entries.append(json.loads(line))
            except ValueError:
                continue
    return [e for e in entries if not e.get("isSidechain")]


def prompt_text(entry):
    """The person's prompt in a user entry, or None for tool results, meta
    messages and prompts Claude Code wrote itself."""
    if entry.get("type") != "user" or entry.get("isMeta"):
        return None
    content = (entry.get("message") or {}).get("content")
    if isinstance(content, str):
        text = content
    elif isinstance(content, list):
        if any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content):
            return None
        text = "\n".join(
            b.get("text", "")
            for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        )
    else:
        return None
    text = text.strip()
    if not text or text.startswith(SYNTHETIC_PREFIXES):
        return None
    return text


def queued_prompt(entry):
    """A message the person typed while the assistant was working: Claude
    Code delivers it inside the running turn as a `queued_command`
    attachment."""
    if entry.get("type") != "attachment":
        return None
    att = entry.get("attachment") or {}
    if att.get("type") != "queued_command" or att.get("commandMode") != "prompt":
        return None
    if (att.get("origin") or {}).get("kind") != "human":
        return None
    text = att.get("prompt")
    if not isinstance(text, str) or not text.strip():
        return None
    return text.strip()


def blocks(entry, kind):
    if entry.get("type") != "assistant":
        return []
    content = (entry.get("message") or {}).get("content")
    if not isinstance(content, list):
        return []
    return [b for b in content if isinstance(b, dict) and b.get("type") == kind]


def turn_messages(prompt, entries, transcript_session):
    """The prompt entry and the entries after it up to the next prompt, as
    captured messages: the prompt, then the assistant's final text."""
    messages = [
        {
            "turn_id": prompt["uuid"],
            "role": "user",
            "source_session": transcript_session,
            "timestamp": prompt.get("timestamp"),
            "text": prompt_text(prompt),
        }
    ]
    tools, files = [], []
    final = []  # text entries after the last tool call
    for entry in entries:
        queued = queued_prompt(entry)
        if queued is not None and entry.get("uuid"):
            messages.append(
                {
                    "turn_id": entry["uuid"],
                    "role": "user",
                    "source_session": transcript_session,
                    "timestamp": entry.get("timestamp"),
                    "text": queued,
                }
            )
        uses = blocks(entry, "tool_use")
        for use in uses:
            name = use.get("name") or ""
            if name and name not in tools:
                tools.append(name)
            args = use.get("input") or {}
            for key in FILE_ARGS:
                path = args.get(key)
                if isinstance(path, str) and path and path not in files:
                    files.append(path)
        if uses:
            final = []
        texts = [b.get("text", "") for b in blocks(entry, "text")]
        if any(t.strip() for t in texts):
            final.append((entry, "\n".join(texts)))
    if final:
        first = final[0][0]
        messages.append(
            {
                "turn_id": first["uuid"],
                "role": "assistant",
                "author": AUTHOR,
                "source_session": transcript_session,
                "timestamp": final[-1][0].get("timestamp"),
                "text": "\n\n".join(t for _, t in final).strip(),
                "tools": tools,
                "files": files,
            }
        )
    return messages


def recent_messages(entries, turns=TURNS):
    starts = [i for i, e in enumerate(entries) if prompt_text(e) is not None]
    messages = []
    for n, start in enumerate(starts[-turns:]):
        later = starts[-turns:][n + 1 :]
        end = later[0] if later else len(entries)
        prompt = entries[start]
        messages.extend(
            turn_messages(prompt, entries[start + 1 : end], prompt.get("sessionId"))
        )
    for m in messages:
        for key in [k for k, v in m.items() if v is None]:
            del m[key]
    return messages


def send(url, body):
    request = urllib.request.Request(
        url.rstrip("/") + "/api/messages",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    token = os.environ.get("PC_AUTH_TOKEN")
    if token:
        request.add_header("Authorization", "Bearer " + token)
    urllib.request.urlopen(request, timeout=30).read()


def detach():
    """Fork; the parent returns True and exits, the child carries on alone.
    Without fork (Windows) the request runs in the hook itself."""
    if not hasattr(os, "fork"):
        return False
    if os.fork() > 0:
        return True
    os.setsid()
    devnull = os.open(os.devnull, os.O_RDWR)
    for fd in (0, 1, 2):
        os.dup2(devnull, fd)
    return False


def main():
    args = [a for a in sys.argv[1:] if a != "--dry-run"]
    dry_run = len(args) < len(sys.argv) - 1
    if len(args) < 2:
        return
    session_id, url = args[0], args[1]
    data = json.load(sys.stdin)
    transcript = data.get("transcript_path")
    if not transcript or not os.path.exists(transcript):
        return
    messages = recent_messages(read_entries(transcript))
    if not messages:
        return
    body = {"session_id": session_id, "messages": messages}
    if dry_run:
        print(json.dumps(body, ensure_ascii=False, indent=2))
        return
    if detach():
        return
    try:
        send(url, body)
    finally:
        if hasattr(os, "fork"):
            os._exit(0)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # fail open: capture must not disturb the session
        print("pcx-capture: %s" % exc, file=sys.stderr)
