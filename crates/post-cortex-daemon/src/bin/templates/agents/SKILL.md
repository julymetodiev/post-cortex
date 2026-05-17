---
name: post-cortex-agents
description: Multi-agent orchestration system for Post-Cortex MCP memory infrastructure. Use when working with persistent memory, knowledge graphs, semantic search, session management, or context building.
---

# Post-Cortex Agent System

Specialized agents for Post-Cortex memory operations using 6 consolidated MCP tools.

## Agents

| Agent | subagent_type | Purpose | Model |
|-------|---------------|---------|-------|
| Context Builder | `context-builder` | Log decisions, Q&A, problems, code changes | Haiku |
| Search Specialist | `search-specialist` | Find past knowledge, semantic search | Sonnet |
| Knowledge Analyst | `knowledge-analyst` | Summaries, analysis, entity mapping | Opus |
| Memory Curator | `memory-curator` | Session/workspace management | Haiku |

## MCP Tools (6 Consolidated)

| Tool | Purpose |
|------|---------|
| `session` | Create/list sessions |
| `update_conversation_context` | Add context (single or bulk) |
| `semantic_search` | Universal search with scope |
| `get_structured_summary` | Session summaries |
| `query_conversation_context` | Entity analysis, keyword search |
| `manage_workspace` | Workspace CRUD operations |

## Agent Routing

| User Intent | Route To |
|-------------|----------|
| "find", "search", "what did we discuss" | `search-specialist` |
| "remember", "log", "decided", "fixed" | `context-builder` |
| "summarize", "analyze", "insights" | `knowledge-analyst` |
| "create session", "new workspace" | `memory-curator` |

## Session Protocol

Every operation requires a `session_id`. Get it from:
1. CLAUDE.md (project configuration)
2. `session(action: "list")` to find existing
3. `session(action: "create")` for new sessions

## Multi-Step Workflows

### Knowledge Retrieval (session → workspace → global)
```
1. semantic_search(query, scope="session", scope_id=session_id)
2. IF no results: semantic_search(query, scope="workspace", scope_id=workspace_id)
3. IF no results: semantic_search(query, scope="global")
4. Synthesize and respond
```

### Context Capture
```
1. Identify type: qa | decision_made | problem_solved | code_change | requirement_added | concept_defined
2. Structure content as key-value pairs
3. update_conversation_context(session_id, interaction_type, content)
```

### New Project Setup
```
1. session(action="create", name="project-name")
2. manage_workspace(action="create", name="project-workspace")
3. manage_workspace(action="add_session", workspace_id, session_id, role="primary")
```
