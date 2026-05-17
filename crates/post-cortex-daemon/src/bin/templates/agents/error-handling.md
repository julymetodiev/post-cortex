# Error Handling

Quick reference for Post-Cortex error recovery.

## Common Errors

| Error | Cause | Recovery |
|-------|-------|----------|
| Session not found | Invalid session_id | `session(action="list")` → select valid |
| Workspace not found | Invalid workspace_id | `manage_workspace(action="list")` → select valid |
| Search timeout | Query too broad | Reduce limit, narrow query |
| No results | No matches | Broaden query, try global scope |

## Retry Strategy

For transient errors (timeout, connection):
1. Wait 0.5s
2. Retry up to 3 times
3. Double wait time each retry

## Fallback Patterns

### Search Fallback (session → workspace → global)
```
1. Try semantic_search with scope="session"
2. If no results → try scope="workspace"
3. If no results → try scope="global"
4. If still nothing → try query_conversation_context with keyword
```

### Session Fallback
```
1. Try to use session_id from CLAUDE.md
2. If not found → session(action="list") to find alternatives
3. If no sessions → session(action="create")
```
