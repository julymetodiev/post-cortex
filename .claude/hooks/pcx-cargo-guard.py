#!/usr/bin/env python3
"""Refuses cargo commands that compile, except through scripts/cargo.sh, whose one
package and feature set keeps surrealdb from compiling again (owner, 2026-11-02).
Reads what the command runs, not text it only writes or sends elsewhere: heredoc
bodies and quoted strings are left out."""

import json
import re
import sys

COMPILES = re.compile(
    r"(?:^|[;&|(\s])cargo\s+(?:\+\S+\s+)?(?:build|b|check|c|clippy|test|t|nextest|run|r|rustc|doc)\b"
)
HEREDOC = re.compile(r"<<-?\s*(['\"]?)(\w+)\1.*?^\s*\2\s*$", re.S | re.M)
QUOTED = re.compile(r"'[^']*'|\"(?:[^\"\\]|\\.)*\"")


def runs_cargo(command):
    return bool(COMPILES.search(QUOTED.sub(" ", HEREDOC.sub(" ", command))))


command = json.load(sys.stdin).get("tool_input", {}).get("command", "")
if runs_cargo(command):
    print(
        json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": (
                        "Direct cargo is refused here: another -p or feature set compiles surrealdb "
                        "again. Use scripts/cargo.sh clippy|test|ignored|doc|cloud."
                    ),
                }
            }
        )
    )
