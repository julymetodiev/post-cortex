#!/bin/bash
# pcx-compact-reinject.sh
# Re-injects Post-Cortex context after context compaction

cat << 'CONTEXT'
## Post-Cortex Context (re-injected after compaction)

**Session:** <SESSION_ID>
**Workspace:** <WORKSPACE_ID>

### Mandatory Rules:
1. **Search Before Answering** - Call `semantic_search` BEFORE answering any question about code, architecture, or past decisions
2. **Log After Discovery** - Call `update_conversation_context` IMMEDIATELY after discovering anything new, making decisions, or changing code
3. **Self-Check** - After every response: did I search? did I log?

### MCP Tools (call directly — do NOT use subagents):
- `semantic_search`, `update_conversation_context`, `get_structured_summary`,
  `query_conversation_context`, `session`, `manage_workspace`,
  `assemble_context`, `manage_entity`, `admin`

### Recent context:
Call `get_structured_summary` with `include: ["decisions","insights","entities"]` to pull recent session context if needed.
CONTEXT

exit 0
