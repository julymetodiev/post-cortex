#!/usr/bin/env python3
"""post-cortex Stop hook: block when this turn did work it did not record.

Claude Code runs this when the assistant is about to stop and passes the hook
input as JSON on stdin (`transcript_path`, `cwd`, `stop_hook_active`). The
current turn starts after the last user prompt or task notification.

The turn is blocked when:
- a file inside the project it edited, through the host's tools or an MCP
  server's, is named in no `update_conversation_context` call made after its
  last edit (a summary that names one file does not cover the others; an
  edit by symbol with no file is covered by naming the symbol);
- it ran a shell command that does more than read, through the host's tool or
  post-cortex's own, or was opened by a background task ending, and no record
  (or `context-builder` subagent) came after the last of these. A command is
  read only when every program in it is one that only reads (`ls`, `grep`,
  `git status`, ...; READS, GIT_READS) and it writes no file and runs nothing
  inside (`$(...)`); any other may have changed or found something, which this
  cannot tell: it blocks once, and the assistant can say in one line that
  there is nothing to record;
- a record it wrote reads as a correction (CORRECTS: "turned out wrong",
  "outdated", ...) and names no `supersedes`: the earlier record it
  corrects would stay current next to it. It blocks once, asking to end that
  record, or to say in one line that it replaces none.

With `--remind` it is the PostToolUse hook of the edit tools instead: once
REMIND files of the project are edited and not yet recorded, each further
edit of a new file adds a reminder to the model's context naming them, so the
record is written while the work is fresh, not at the end from memory.

Everything else - questions, status updates, subagent reports, edits outside
the project - passes. Any error passes too, so a broken hook never traps the
session.

Usage (from .claude/settings.json):
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-stop-check.py <SESSION_ID>
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-stop-check.py --remind <SESSION_ID>
Without the session id (the Claude Code plugin) the project's session is
resolved by pcx_resolve.py for the message.
"""

import json
import os
import re
import shlex
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the hooks
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    import pcx_resolve
except ImportError:  # an older copy of the hooks without the resolver
    pcx_resolve = None

EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
# What an MCP tool that edits files ends with: Lantern Lattice's, Mimir's and
# post-cortex's own.
MCP_EDITS = re.compile(r"^mcp__.*__(edit_symbols|insert_after|edit_lines|replace_text|write_file|edit|write)$")
# What an MCP tool that runs a command ends with: post-cortex's own.
MCP_SHELL = re.compile(r"^mcp__.*__(bash|tmux_start|tmux_send)$")
TASK_ENDED = "<task-notification>"
# Programs that only read or print (with SED and FIND only without the flags
# that write or run something, below).
READS = {
    "ls", "cat", "head", "tail", "grep", "egrep", "fgrep", "rg", "wc", "cmp", "diff",
    "stat", "file", "which", "type", "pwd", "echo", "printf", "true", "false", "sleep",
    "date", "du", "df", "tree", "sort", "uniq", "cut", "tr", "jq", "basename",
    "dirname", "realpath", "readlink", "test", "[", "cd", "sed", "find",
}
# What git does that only reads.
GIT_READS = {"status", "log", "show", "diff", "grep", "ls-files", "rev-parse", "blame", "describe", "shortlog"}
SED_WRITES = re.compile(r"^(-[a-zA-Z]*i|--in-place)")
FIND_WRITES = {"-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0", "-fprintf", "-fls"}
# What separates the commands of a line.
SEPARATORS = {"|", "||", "&&", ";", "&", "(", ")", ";;", "|&"}
# Shell words that lead a command (`until grep ...; do sleep 5; done`) and
# those that close a block: they run nothing themselves.
KEYWORDS = {"if", "then", "else", "elif", "do", "while", "until", "!", "{", "time"}
ENDS = {"fi", "done", "}", "esac"}
# Words by which a record says an earlier one no longer holds.
CORRECTS = re.compile(
    r"\b(wrong|incorrect|corrected|mistaken|not anymore|"
    r"outdated|obsolete|reversed|reverted|turned out)\b",
    re.IGNORECASE,
)
# Files edited and not recorded past which each new one is reminded of.
REMIND = 8
# Files a message names at most.
NAMED = 12


def is_shell(name):
    return name == "Bash" or bool(MCP_SHELL.match(name))


def program_reads(words):
    """Whether one command (its words, assignments first) only reads."""
    while words and (words[0] in KEYWORDS or ("=" in words[0] and not words[0].startswith("="))):
        words = words[1:]  # FOO=1 cmd, `do cmd`, `if cmd`
    if not words or words[0] in ENDS:
        return True
    if words[0] == "for":
        return True  # `for x in a b`: only binds; its body is the command after `do`
    program, args = os.path.basename(words[0]), words[1:]
    if program == "git":
        while args[:1] in (["-C"], ["-c"]) and len(args) > 1:
            args = args[2:]
        while args and args[0].startswith("--no-"):
            args = args[1:]
        return bool(args) and (args[0] in GIT_READS or args[:2] == ["worktree", "list"])
    if program == "sed":
        return not any(SED_WRITES.match(a) for a in args)
    if program == "find":
        return not FIND_WRITES.intersection(args)
    return program in READS


def substitutions(command):
    """`command` with each command it runs inside (`$(...)`, backticks,
    `<(...)`, `>(...)`) outside single quotes replaced by a word, and those
    inner commands; None when one is not closed."""
    outer, inner, i, quote = [], [], 0, None
    while i < len(command):
        c = command[i]
        if quote == "'":
            quote = None if c == "'" else quote
        elif c == "\\" and i + 1 < len(command):
            outer.append(command[i : i + 2])
            i += 2
            continue
        elif c == "'" and quote is None:
            quote = "'"
        elif c == '"':
            quote = None if quote == '"' else '"'
        elif c == "`":
            end = command.find("`", i + 1)
            if end < 0:
                return None
            inner.append(command[i + 1 : end])
            outer.append("X")
            i = end + 1
            continue
        elif command.startswith(("$(", "<(", ">("), i) and not command.startswith("$((", i):
            depth, j = 1, i + 2
            while j < len(command) and depth:
                depth += {"(": 1, ")": -1}.get(command[j], 0)
                j += 1
            if depth:
                return None
            inner.append(command[i + 2 : j - 1])
            outer.append("X")
            i = j
            continue
        outer.append(c)
        i += 1
    return "".join(outer), inner


def reads_only(command):
    """Whether a shell command line only reads: every command in it does,
    and it writes no file (`>` only to /dev/null or another descriptor) and
    runs nothing inside it but what only reads (`$(...)`, backticks, `<(...)`;
    inside single quotes they are text). Anything it cannot parse does more."""
    split = substitutions(command)
    if split is None:
        return False
    command, inner = split
    if not all(reads_only(c) for c in inner):
        return False
    # A new line starts a new command; inside quotes it only splits a word.
    lexer = shlex.shlex(command.replace("\n", " ; "), posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    try:
        tokens = list(lexer)
    except ValueError:
        return False
    words, i = [], 0
    while i < len(tokens):
        token = tokens[i]
        if token in SEPARATORS:
            if not program_reads(words):
                return False
            words = []
        elif set(token) <= set("<>&") and ">" in token:
            target = tokens[i + 1] if i + 1 < len(tokens) else ""
            if not (target == "/dev/null" or target.isdigit() or target == "-"):
                return False
            i += 1  # the target is not a word of the command
            if words and words[-1].isdigit():
                words.pop()  # the descriptor of `2>`
        elif token in ("<", "<<", "<<<"):
            i += 1  # reading a file or a here document
        else:
            words.append(token)
        i += 1
    return program_reads(words)


def runs_something(use):
    """Whether a shell tool call may have changed or found something: a
    command that does more than read, or any command a tmux session takes."""
    name, args = use.get("name", ""), use.get("input") or {}
    if not is_shell(name):
        return False
    if name == "Bash" or name.endswith("__bash"):
        return not reads_only(args.get("command") or "")
    return True


def is_turn_start(entry):
    """A user prompt or task notification opens a new turn; tool results,
    meta messages and hook feedback do not."""
    if entry.get("type") != "user" or entry.get("isMeta"):
        return False
    return isinstance((entry.get("message") or {}).get("content"), str)


def opens_with_task_end(entry):
    """Whether a background task ending opened the turn: a task notification
    with its status. A monitor's event (no status) only reports progress."""
    content = (entry.get("message") or {}).get("content", "")
    return content.startswith(TASK_ENDED) and "<status>" in content


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


def in_project(path, project):
    """A relative path is in the project; a path in .git is not an edit."""
    if f"{os.sep}.git{os.sep}" in path or path.startswith(".git" + os.sep):
        return False
    return not os.path.isabs(path) or inside(path, project)


def edited(name, args, project):
    """What an edit tool call changes in the project, each as the name a
    record must mention: a file's base name, or the symbol of an edit by
    symbol that names no file."""
    if name in EDIT_TOOLS:
        path = args.get("file_path") or args.get("notebook_path") or ""
        return [os.path.basename(path)] if path and os.path.isabs(path) and in_project(path, project) else []
    if not MCP_EDITS.match(name):
        return []
    parts = [args] + [e for e in args.get("edits") or [] if isinstance(e, dict)]
    names = []
    for part in parts:
        path = part.get("file_path") or part.get("file") or part.get("path") or ""
        if path:
            if in_project(path, project):
                names.append(os.path.basename(path))
        elif part.get("symbol"):
            names.append(str(part["symbol"]))
    return names


def is_log(use):
    name = use.get("name", "")
    if name.endswith("update_conversation_context"):
        return True
    args = use.get("input") or {}
    return name in ("Agent", "Task") and args.get("subagent_type") == "context-builder"


def records_of(use):
    """The records a log call writes: each of its `updates`, or the call."""
    args = use.get("input") or {}
    updates = args.get("updates")
    if isinstance(updates, list):
        return [u for u in updates if isinstance(u, dict)]
    return [args]


def unended_corrections(uses):
    """The words of each record of the turn that reads as a correction and
    names no `supersedes`, in the order written."""
    found = []
    for use in uses:
        if not use.get("name", "").endswith("update_conversation_context"):
            continue
        for record in records_of(use):
            content = record.get("content")
            if record.get("supersedes") or not isinstance(content, dict):
                continue
            match = CORRECTS.search(" ".join(str(v) for v in content.values()))
            if match:
                found.append(match.group(0))
    return found


def entries_of(transcript):
    entries = []
    with open(transcript, encoding="utf-8") as f:
        for line in f:
            try:
                entries.append(json.loads(line))
            except ValueError:
                continue
    return entries


def turn_uses(entries):
    """The tool calls of the current turn, and whether a task ending opened
    it."""
    start, task = 0, False
    for i, entry in enumerate(entries):
        if is_turn_start(entry):
            start, task = i + 1, opens_with_task_end(entry)
    return [use for entry in entries[start:] for use in tool_uses(entry)], task


def unrecorded(uses, task, project):
    """What the turn left unrecorded: the names of edited files no later
    record mentions, in the order edited, and the last command (or task
    ending) no record followed."""
    pending_files = {}  # name -> order of its last edit
    pending_other = {"name": "task"} if task else None
    for order, use in enumerate(uses):
        name, args = use.get("name", ""), use.get("input") or {}
        if is_log(use):
            text = json.dumps(args, ensure_ascii=False)
            pending_files = {f: o for f, o in pending_files.items() if f not in text}
            pending_other = None
            continue
        for file in edited(name, args, project):
            pending_files[file] = order
        if runs_something(use):
            pending_other = use
    files = sorted(pending_files, key=pending_files.get)
    return files, pending_other


def described(use):
    args = use.get("input") or {}
    if use.get("name") == "task":
        return "was opened by a background task ending"
    lines = (args.get("command") or args.get("text") or "").strip().splitlines()
    return "ran `%s`" % (lines[0][:80] if lines else "")


def named(files):
    shown = ", ".join(files[:NAMED])
    return shown + (" and %d more" % (len(files) - NAMED) if len(files) > NAMED else "")


def session_of(data, args):
    """The session id to log to: the argument, else the resolved project's,
    else None. The plugin copy steps aside for the project's own hook."""
    if args:
        return args[0]
    if pcx_resolve is None:
        return None
    context = pcx_resolve.global_context(__file__, data)
    return context[2]["session_id"] if context else None


def stands_aside(args):
    """The plugin's copy leaves the work to a project's own hook, or to
    nothing when post-cortex is off."""
    return not args and pcx_resolve is not None and (
        pcx_resolve.shadowed_by_project(__file__) or not pcx_resolve.host_settings()["enabled"]
    )


def stop(data, args, project):
    if data.get("stop_hook_active"):
        return None  # already asked once this stop; never loop
    uses, task = turn_uses(entries_of(data["transcript_path"]))
    files, other = unrecorded(uses, task, project)
    corrections = unended_corrections(uses)
    if not files and other is None and not corrections:
        return None
    reasons = []
    if files or other is not None:
        session_id = session_of(data, args)
        session = "session_id %s" % session_id if session_id else "the project's session_id"
        did = []
        if files:
            did.append("edited %s, which no record after its edit names" % named(files))
        if other is not None:
            did.append(described(other) + " with nothing recorded after it")
        reasons.append((
            "This turn %s. Record what changed and why with post-cortex's "
            "update_conversation_context, %s (>=1 entity, >=1 relation), "
            "naming each file (bulk `updates` for several). If there is nothing to find "
            "later (a typo, a look at a status), say so in one line and stop."
        ) % ("; it ".join(did), session))
    if corrections:
        reasons.append((
            "A record of this turn reads as a correction (\"%s\") but names no "
            "`supersedes`, so the record it corrects stays current next to it. Find "
            "that record (semantic_search; its content_id is the entry_id) and end it: "
            "manage_entity `forget` with its entry_id, and pass `supersedes` in the "
            "next correction. If it corrects no earlier record, say so in one line."
        ) % corrections[0])
    return {"decision": "block", "reason": " ".join(reasons)}


def remind(data, args, project):
    uses, task = turn_uses(entries_of(data["transcript_path"]))
    # The call this runs after may be in the transcript already: left out,
    # so what it adds is told apart from what was pending before it.
    this = data.get("tool_use_id")
    earlier, _ = unrecorded([u for u in uses if not this or u.get("id") != this], task, project)
    current = edited(data.get("tool_name", ""), data.get("tool_input") or {}, project)
    new = [f for f in dict.fromkeys(current) if f not in earlier]
    files = earlier + new
    # Only an edit that adds a new file past the mark reminds, once each.
    if len(files) < REMIND or not new:
        return None
    session_id = session_of(data, args)
    session = "session_id %s" % session_id if session_id else "the project's session_id"
    text = (
        "[PCX] %d files edited and not recorded yet: %s. Record them now with "
        "update_conversation_context, %s, naming each file and why it changed, "
        "before going on."
    ) % (len(files), named(files), session)
    return {"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": text}}


def main():
    argv = sys.argv[1:]
    reminding = bool(argv) and argv[0] == "--remind"
    args = argv[1:] if reminding else argv
    data = json.load(sys.stdin)
    if stands_aside(args):
        return
    transcript = data.get("transcript_path")
    if not transcript or not os.path.exists(transcript):
        return
    project = os.environ.get("CLAUDE_PROJECT_DIR") or data.get("cwd") or os.getcwd()
    answer = (remind if reminding else stop)(data, args, project)
    if answer is not None:
        print(json.dumps(answer))


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # noqa: BLE001 - a broken hook must not block
        print("pcx-stop-check: %s" % exc, file=sys.stderr)
