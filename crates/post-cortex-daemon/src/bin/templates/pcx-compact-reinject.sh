#!/bin/bash
# pcx-compact-reinject.sh
# Re-injects Post-Cortex context after context compaction

SESSION_ID="<SESSION_ID>"
WORKSPACE_ID="<WORKSPACE_ID>"
PCX_PORT="${PC_PORT:-3737}"

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
CONTEXT

# Try to fetch recent summary from PCX daemon
SUMMARY=$(curl -s --connect-timeout 2 --max-time 5 \
  "http://127.0.0.1:${PCX_PORT}/mcp" \
  -H "Content-Type: application/json" \
  -d "{
    \"jsonrpc\": \"2.0\",
    \"id\": 1,
    \"method\": \"tools/call\",
    \"params\": {
      \"name\": \"get_structured_summary\",
      \"arguments\": {
        \"session_id\": \"${SESSION_ID}\",
        \"include\": [\"decisions\", \"insights\", \"entities\"]
      }
    }
  }" 2>/dev/null)

if [ $? -eq 0 ] && [ -n "$SUMMARY" ]; then
  EXTRACTED=$(echo "$SUMMARY" | jq -r '.result.content[0].text // empty' 2>/dev/null)
  if [ -n "$EXTRACTED" ]; then
    echo ""
    echo "### Recent PCX Context:"
    echo "$EXTRACTED"
  fi
fi

exit 0
