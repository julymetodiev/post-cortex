#!/bin/bash
# pcx-compact-reinject.sh
# Re-injects Post-Cortex context after context compaction
# This ensures PCX session info and rules survive compaction

cat << 'CONTEXT'
## Post-Cortex Context (re-injected after compaction)

**Session:** bf52f62e-8e26-4e9e-8501-c42753d9a9ee
**Workspace:** ba76f569-a50d-46cd-a5dd-7c5577f12d1f
**Project:** post-cortex

### Mandatory Rules:
1. **Search Before Answering** - Call `semantic_search` (session → workspace → global) BEFORE answering any code/architecture question
2. **Log After Discovery** - Call `update_conversation_context` IMMEDIATELY after making decisions, fixing bugs, or changing code
3. **Self-Check** - After every response: did I search? did I log?

### MCP Tools (call directly — do NOT use subagents):
- `semantic_search`, `update_conversation_context`, `get_structured_summary`,
  `query_conversation_context`, `session`, `manage_workspace`,
  `assemble_context`, `manage_entity`, `admin`

### Recent context:
Call `get_structured_summary` with `include: ["decisions","insights","entities"]` to pull recent session context if needed.
CONTEXT

exit 0
