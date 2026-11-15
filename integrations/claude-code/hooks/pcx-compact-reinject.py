#!/usr/bin/env python3
"""post-cortex SessionStart hook (matcher `compact`): put the session, the
workspace and the memory rules back in front of the session after context
compaction.

Usage (from .claude/settings.json, `pcx setup --project`):
    python3 -I "$CLAUDE_PROJECT_DIR"/.claude/hooks/pcx-compact-reinject.py <SESSION_ID> <WORKSPACE_ID>
Without arguments (the Claude Code plugin) the ids are resolved by
pcx_resolve.py; when that fails the rules are printed without them.
"""

import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the hooks
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    import pcx_resolve
except ImportError:  # an older copy of the hooks without the resolver
    pcx_resolve = None

RULES = """### Mandatory Rules:
1. **Search Before Answering** - Call `semantic_search` BEFORE answering any question about code, architecture, or past decisions
2. **Log After Discovery** - Call `update_conversation_context` IMMEDIATELY after discovering anything new, making decisions, or changing code
3. **Self-Check** - After every response: did I search? did I log?

### MCP Tools (call directly, do NOT use subagents):
- `semantic_search`, `update_conversation_context`, `get_structured_summary`,
  `query_conversation_context`, `session`, `manage_workspace`,
  `assemble_context`, `manage_entity`, `admin`

### Recent context:
Call `get_structured_summary` with `include: ["decisions","insights","entities"]` to pull recent session context if needed."""


def render(ids):
    parts = ["## Post-Cortex Context (re-injected after compaction)"]
    if ids:
        parts.append(
            "**Session:** %s\n**Workspace:** %s" % (ids["session_id"], ids["workspace_id"])
        )
    parts.append(RULES)
    return "\n\n".join(parts)


def main():
    args = sys.argv[1:]
    if len(args) >= 2:
        ids = {"session_id": args[0], "workspace_id": args[1]}
    elif pcx_resolve is None:
        ids = None
    else:
        if pcx_resolve.shadowed_by_project(__file__) or not pcx_resolve.host_settings()["enabled"]:
            return
        context = pcx_resolve.global_context(__file__, pcx_resolve.read_payload(sys.stdin))
        ids = context[2] if context else None
    print(render(ids))


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # fail open: the rules must not disturb the session
        print("pcx-compact-reinject: %s" % exc, file=sys.stderr)
