# Focal Integration

This project is indexed by Focal, a structural code indexing tool.
You have access to Focal's MCP tools — prefer them over raw file reading
when possible.

## Tool Priority

0. **After context compaction** call `recover_session` — restores your working
   state (decisions, files, symbols) and resets progressive disclosure so
   previously-sent symbol bodies will be re-sent fresh.
1. **Start every new task** with `get_context` — it returns a token-budgeted
   capsule with the most relevant symbols, their dependencies, and linked memories.
2. Use `get_skeleton` before `Read` — 70-90% fewer tokens for understanding file structure.
3. Use `get_impact_graph` before making changes — know the blast radius.
4. Use `search_code` for semantic search (FTS5) alongside Grep for literal search.
5. Use `save_memory` to persist architectural decisions, conventions, and insights.

## Effective Tool Chains

- **After compaction:** `recover_session` → resume with `get_context` on the current task
- **Bug fix:** `get_context("fix X")` → `get_impact_graph("X")` → `query_symbol` for details
- **New feature:** `get_context("add X")` → `get_skeleton` of target files → `get_dependencies`
- **Refactor:** `get_context("refactor X")` → `get_impact_graph` for blast radius → `search_logic_flow`
- **Code review:** `get_skeleton` per changed file → `get_dependencies` for each modified symbol

## Available Tools

| Tool | Purpose |
|------|---------|
| `get_context` | Smart context retrieval with token budgeting and intent detection |
| `query_symbol` | Look up specific symbols by name |
| `search_code` | Full-text search across all indexed code |
| `search_memory` | Full-text search across stored memories |
| `get_skeleton` | Token-efficient file view (signatures only) |
| `get_impact_graph` | Blast radius analysis for a symbol |
| `search_logic_flow` | Trace call paths between two symbols |
| `get_dependencies` | Outgoing dependency edges |
| `get_dependents` | Incoming dependency edges |
| `get_file_symbols` | List all symbols in a file |
| `save_memory` | Store a decision, pattern, or insight |
| `list_memories` | List stored memories |
| `batch_query` | Fetch multiple symbols in one call |
| `get_repo_overview` | High-level repo stats |
| `get_health` | Database diagnostics |
| `get_symbol_history` | Git blame for a symbol |
| `recover_session` | Restore session state after context compaction |
