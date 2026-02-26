#!/bin/bash
# pcx-compact-reinject.sh
# Re-injects Post-Cortex context after context compaction
# This ensures PCX session info and rules survive compaction

SESSION_ID="<YOUR_SESSION_ID>"
WORKSPACE_ID="<YOUR_WORKSPACE_ID>"
PCX_PORT="${PC_PORT:-3737}"

cat << 'CONTEXT'
## Post-Cortex Context (re-injected after compaction)

**Session:** <YOUR_SESSION_ID>
**Workspace:** <YOUR_WORKSPACE_ID>
**Project:** post-cortex

### Mandatory Rules:
1. **Search Before Answering** - Use search-specialist agent BEFORE answering ANY question about code, architecture, or past decisions
2. **Log After Discovery** - Use context-builder agent IMMEDIATELY after discovering anything new, making decisions, or changing code
3. **Self-Check** - After EVERY response: Did I search? Did I log?

### Agents:
- search-specialist: semantic_search, query_conversation_context
- context-builder: update_conversation_context
- knowledge-analyst: get_structured_summary
- memory-curator: session, manage_workspace
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
        \"include\": [\"decisions\", \"problems\", \"insights\"]
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
