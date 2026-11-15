#!/usr/bin/env python3
"""Refuses a task restart in place on the FreeBSD cluster (owner, 2026-11-02): under the
pot driver the old writers of the task's log fifo can all be gone before the new one
opens it. Nomad's reader then reads EOF and never reads again, the pipe fills, and the
task stops at its next log line. A new allocation has a new reader. It reads the whole
command, quotes and heredocs too, since this runs through ssh and scripts."""

import json
import re
import sys

RESTART = re.compile(r"nomad\s+(?:alloc\s+restart|alloc-restart)\b")

command = json.load(sys.stdin).get("tool_input", {}).get("command", "")
if RESTART.search(command):
    print(
        json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": (
                        "nomad alloc restart can stop a pot task's logs and then the task. "
                        "Make a new "
                        "allocation: nomad job run, or nomad alloc stop."
                    ),
                }
            }
        )
    )
