# Focal: Design Document

**Author:** Raghav Potluri  
**Status:** v1, shipping  
**Last updated:** 2026-02-23

---

## Problem

Claude Code re-reads hundreds of files per session. It has no structural awareness of how symbols relate to each other, no memory of past decisions across sessions, and no mechanism to prioritize which code to surface for a given task. The result: burned tokens on redundant context, degraded response quality as conversations grow, and zero persistence between sessions.

Existing mitigations — project-level `CLAUDE.md` files, manual context curation — are fragile and don't scale. What's missing is a structural index that understands the code the way an IDE does, but serves that understanding through a protocol Claude Code already speaks.

## Solution

Focal is a local Rust binary that:

1. **Parses** source code with tree-sitter into a structural symbol graph
2. **Stores** symbols, dependency edges, and persistent memories in SQLite (with FTS5)
3. **Serves** focused, token-budgeted context to Claude Code via MCP

A companion VS Code extension manages the binary lifecycle and auto-configures MCP settings.

The key insight: instead of shipping raw files, ship *symbols* — ranked, budgeted, and enriched with graph context and persistent memory. A function signature costs ~5% of the tokens its full body does. Focal exploits this asymmetry aggressively.

---

## Architecture

```
+-----------------------------------+
|  VS Code Extension (TypeScript)   |
|  - Status bar (index stats)       |
|  - Commands (reindex, clear, MCP) |
|  - Auto-configures MCP settings   |
+----------+------------------------+
           |
           | writes ~/.claude/settings.json
           | generates .claude/CLAUDE.md
           v
+----------+------------------------+
|  focal-core (Rust binary)         |
|  - tree-sitter parser             |
|  - SQLite graph store (WAL, FTS5) |
|  - Memory store (manual + auto)   |
|  - File watcher (notify, 500ms)   |
|  - MCP server (stdio or HTTP)     |
+----------+------------------------+
           | stdio (default) / HTTP (--http)
           v
+----------+------------------------+
|  Claude Code                      |
|  - Calls 19 MCP tools             |
|  - Gets focused context capsules  |
|  - Stores decisions as memories   |
+-----------------------------------+
```

**Claude Code spawns the binary.** The VS Code extension writes the MCP server configuration to `~/.claude/settings.json`; Claude Code reads that config and launches `focal <workspace_roots>` as a child process. stdio transport means no port management, no daemon lifecycle, no firewall concerns. HTTP mode (`--http --port 3100`) is available for power users who want to share a server across tools.

**Data flows one direction for indexing, bidirectional for queries.** The binary indexes on startup, then watches for changes. MCP tool calls arrive over stdin, responses go to stdout. Memories flow from Claude → Focal for storage, and from Focal → Claude during context retrieval.

---

## Data Model

Single SQLite database at `~/.focal/index.db`. WAL journal mode, `busy_timeout = 5000ms`, foreign keys enforced.

### Schema

```sql
CREATE TABLE repositories (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    root_path  TEXT NOT NULL UNIQUE,
    indexed_at TEXT
);

CREATE TABLE files (
    id         INTEGER PRIMARY KEY,
    repo_id    INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,           -- relative to repo root
    language   TEXT NOT NULL,
    hash       TEXT NOT NULL,           -- SHA-256 of file contents
    indexed_at TEXT,
    UNIQUE(repo_id, path)
);

CREATE TABLE symbols (
    id         INTEGER PRIMARY KEY,
    file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,           -- function|method|struct|class|interface|
                                        -- trait|type_alias|const|module|enum
    signature  TEXT NOT NULL DEFAULT '',
    body       TEXT NOT NULL DEFAULT '',
    body_hash  TEXT NOT NULL DEFAULT '', -- SHA-256 of body, used for memory staleness
    start_line INTEGER NOT NULL,
    end_line   INTEGER NOT NULL,
    parent_id  INTEGER REFERENCES symbols(id) ON DELETE SET NULL
);

CREATE TABLE edges (
    id        INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    target_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    kind      TEXT NOT NULL,            -- calls|imports|implements|embeds|type_ref
    UNIQUE(source_id, target_id, kind)
);

CREATE TABLE memories (
    id           INTEGER PRIMARY KEY,
    content      TEXT NOT NULL,
    category     TEXT NOT NULL,           -- decision|pattern|bug_fix|architecture|
                                          -- convention|auto
    source       TEXT NOT NULL DEFAULT 'manual',  -- 'manual' or 'auto:<tool_name>'
    session_id   TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    stale        INTEGER NOT NULL DEFAULT 0,
    needs_review INTEGER NOT NULL DEFAULT 0  -- set when linked symbol body changed
);

CREATE TABLE memory_symbols (           -- junction table
    memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    symbol_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    PRIMARY KEY (memory_id, symbol_id)
);
```

### Indexes

| Index | Columns | Purpose |
|-------|---------|---------|
| `idx_files_repo_id` | `files(repo_id)` | Scope file lookups to a repository |
| `idx_symbols_file_name` | `symbols(file_id, name)` | Fast symbol lookup within a file |
| `idx_symbols_kind_name` | `symbols(kind, name)` | Filter by symbol kind |
| `idx_symbols_name` | `symbols(name)` | Cross-repo name resolution |
| `idx_edges_source` | `edges(source_id)` | Forward traversal (dependencies) |
| `idx_edges_target` | `edges(target_id)` | Reverse traversal (dependents) |
| `idx_edges_unique` | `edges(source_id, target_id, kind)` | Deduplicate edges |
| `idx_memory_symbols_sym` | `memory_symbols(symbol_id)` | Find memories linked to a symbol |

### FTS5 Virtual Tables

```sql
-- Content-synced: reads from the content table, no data duplication
CREATE VIRTUAL TABLE symbols_fts
    USING fts5(name, signature, body, content=symbols, content_rowid=id);

CREATE VIRTUAL TABLE memories_fts
    USING fts5(content, category, content=memories, content_rowid=id);
```

FTS5 indexes are maintained incrementally — rows inserted into `symbols_fts` on every `insert_symbol` call, deleted before `delete_symbols_by_file`. This avoids full `REBUILD` operations which would block MCP handlers during re-indexing.

### Design Decision: SQLite Over a Graph Database

I considered Neo4j, DGraph, and embedded alternatives like Oxigraph. SQLite won because:

- **Zero-ops deployment.** Single file, no server, embeddable. Users `cargo install` and they're running.
- **Fast enough.** The edge table with indexed source/target FKs handles 1-3 hop BFS traversals in sub-millisecond for graphs under 100K symbols. I'm not doing PageRank or community detection — I'm doing breadth-first fan-out, which is `O(branching_factor^depth)` random lookups. SQLite handles this fine up to depth 5.
- **FTS5 comes free.** No separate search infrastructure needed.
- **Transaction semantics.** IMMEDIATE transactions with automatic rollback on error. Re-indexing a file is atomic — if parsing fails halfway, no partial state persists.
- **WAL mode** allows concurrent reads (MCP handlers) during writes (file watcher re-indexing).

The tradeoff: multi-hop traversals beyond depth ~5 get expensive, and there's no built-in graph query language. Both are acceptable constraints for this use case.

---

## Indexing Pipeline

### Initial Index

On startup, the binary walks each workspace root with `walkdir`:

```
walk directory
  → filter excluded paths (node_modules, .git, vendor, target, dist, __pycache__)
  → filter by file extension (grammar support check)
  → filter by size (max 500KB)
  → compute SHA-256 hash
  → skip if hash matches existing record
  → parse with tree-sitter
  → extract symbols (recursive, preserving parent-child nesting)
  → insert into DB within IMMEDIATE transaction
  → mark linked memories as stale
```

After all files are processed, a second pass resolves cross-file edges: for each file, re-parse to extract references, then resolve each reference name against a pre-built `HashMap<String, i64>` of all symbol names in the repo. This turns `O(refs × query_cost)` into `O(refs)` with a single upfront query.

The symbol map handles name ambiguity by preferring functions/methods over types (ordered by `CASE kind`), and generates unqualified aliases for qualified names (`Config::new` → `new` as fallback).

### Incremental Re-indexing

The `notify` crate provides platform-native file watching (FSEvents on macOS, inotify on Linux). Raw events are coalesced in a background thread with a 500ms debounce window, then delivered as deduplicated path batches.

For each changed path:

1. If deleted → remove file record, symbols, and edges from DB
2. If modified → hash check → re-parse → replace symbols + edges within a transaction
3. Mark all memories linked to affected symbols as `stale = true`
4. Re-link memories to new symbol IDs by matching on `(symbol_name, body_hash)`:
   - Name matches, hash matches → `stale = 0, needs_review = 0` (fresh)
   - Name matches, hash differs → `stale = 0, needs_review = 1` (body changed)
   - Name not found → memory stays stale

Each file is processed under its own DB lock acquisition, keeping lock hold time short and avoiding blocking MCP handlers for the entire batch.

### Tree-sitter Grammar System

```rust
pub trait Grammar: Send + Sync {
    fn language(&self) -> tree_sitter::Language;
    fn file_extensions(&self) -> &[&str];
    fn extract_symbols(&self, source: &[u8], tree: &tree_sitter::Tree) -> Vec<ExtractedSymbol>;
    fn extract_references(&self, source: &[u8], tree: &tree_sitter::Tree) -> Vec<ExtractedReference>;
}
```

`GrammarRegistry` maps file extensions to grammar implementations. Adding a new language requires implementing this trait — no framework changes needed.

**v1 grammars:** Go, Rust, TypeScript/TSX/JavaScript, Python

**Why tree-sitter over LSP:**

- Deterministic, offline parsing with no server dependency
- Produces a full AST from a byte buffer — no project configuration required
- Same code path for all languages via the `Grammar` trait
- LSP provides richer semantic data (type inference, cross-crate resolution) but requires a running language server per language. v2 will bridge LSP data to supplement tree-sitter's syntactic-only analysis.

---

## Context Engine

The core differentiator. Instead of serving raw files, Focal builds **context capsules** — token-budgeted packages of symbols, graph context, and memories tuned to the task at hand.

### Intent Detection

Every `get_context` query is classified by intent via keyword matching:

| Intent | Keywords | Graph Expansion Strategy |
|--------|----------|--------------------------|
| **Debug** | fix, bug, crash, fail, panic, broken, debug | Dependents + dependencies (callers and callees) |
| **Refactor** | refactor, rename, extract, split, reorganize | Dependents only (blast radius) |
| **Modify** | add, implement, create, build, feature | Dependencies only (what I'll use) |
| **Explore** | *(no match)* | Dependencies only (balanced) |

Keyword counting with priority-ordered tiebreaking (Debug > Refactor > Modify). Counts are computed per category; highest wins. No ML model — the keyword approach is cheap, deterministic, and correct enough for driving graph expansion direction.

### Capsule Algorithm

```
get_capsule(query, max_tokens, repo_id, already_sent):

  1. Detect intent from query text
  2. Strip intent keywords from query (so "fix Database" becomes FTS for "Database")
  3. FTS5 search for pivot symbols (top 5)
     - Fallback: if < 3 FTS results, supplement with LIKE-based name matching
  4. For each pivot (within budget):
     - If not in already_sent → include full body
     - If in already_sent → include placeholder "(full body sent earlier in session)"
     - Track token cost: (name + kind + sig + body + file_path + 20) / 4
  5. Expand from pivots via graph edges (direction per intent):
     - Add adjacent symbols as skeletons (signature only, no body)
     - Stop when budget exhausted
  6. Attach memories linked to pivot symbols (capped at 10% of token budget)
  7. Return ContextCapsule { intent, items, memories, total_tokens, budget }
```

Token estimation: `len_chars / 4`. No tokenizer dependency — this is budgeting, not billing. Off by ~15% in practice, which is fine for preventing context overflow.

### Progressive Disclosure

The `sent_symbols` set (per MCP session, in-memory `HashSet<i64>`) tracks which symbol bodies have already been delivered. On subsequent requests for the same symbol, the capsule returns signature + `"(full body sent earlier in session)"` instead of the full body. This saves ~95% of tokens on repeated lookups.

The `recover_session` tool clears this set — after Claude Code's context window compacts, it no longer has those bodies in context, so progressive disclosure would hide content Claude genuinely needs to re-read.

---

## Memory System

Two categories of memory, one storage mechanism.

### Manual Memories

Created explicitly via `save_memory`. Linked to symbols by name resolution. Categories: `decision`, `pattern`, `bug_fix`, `architecture`, `convention`. **Never expire.**

### Auto-Observations

Generated by every MCP tool call that touches symbols. Source field is `auto:<tool_name>`. Compact (~100-200 bytes each), e.g.:

```
"Explored 'Database' (3 results)"
"Traversed dependencies of 'get_capsule' (depth=1, 8 nodes)"
"Impact analysis of 'Memory' (depth=2, 14 affected)"
```

Session ID generated once per MCP session (Unix timestamp-based). **Auto-observations older than 90 days are deleted on startup.** This prevents unbounded growth while preserving the session trail for `recover_session`.

### Staleness Propagation (Body-Hash Model)

v1 used binary staleness: any file change marked all linked memories as `stale = true`. This produced excessive false positives — a whitespace-only edit would invalidate an architectural decision.

v2 uses **body-hash comparison** for a three-state model:

| State | `stale` | `needs_review` | Meaning |
|-------|---------|----------------|---------|
| **Fresh** | 0 | 0 | Symbol body unchanged; memory is current |
| **Needs review** | 0 | 1 | Symbol name survived but body hash changed; memory may or may not still apply |
| **Stale** | 1 | 0 | Symbol disappeared entirely; memory has no valid link target |

Each symbol's body is SHA-256 hashed at index time and stored in `symbols.body_hash`. During re-indexing:

1. Before deleting symbols from a file, the indexer collects `(memory_id, symbol_name, body_hash)` triples
2. After inserting new symbols, it re-links by name and compares hashes
3. If the name matches but the hash differs → `needs_review = 1, stale = 0`
4. If the name matches and the hash matches → `needs_review = 0, stale = 0`
5. If the name has no match → memory stays in `stale = 1` state from the bulk mark

This avoids false positives from variable renames, comment changes, and formatting edits that don't alter the symbol's structural identity. The `needs_review` flag signals to Claude: "this decision was made about code that has since changed — verify before reusing."

### Memory–Symbol Relinking

Symbol IDs change on re-indexing (old symbols deleted, new ones inserted). The indexer captures `(memory_id, symbol_name, body_hash)` triples before deletion, then re-links by name after insertion, comparing body hashes to set the appropriate staleness state. This preserves memory associations across index rebuilds while detecting semantic changes.

### Schema Migration

Existing databases are migrated automatically. `apply_additive_migrations()` checks for missing columns via a speculative `SELECT` and adds them with `ALTER TABLE`:

```sql
ALTER TABLE symbols ADD COLUMN body_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE memories ADD COLUMN needs_review INTEGER NOT NULL DEFAULT 0;
```

The `DEFAULT ''` / `DEFAULT 0` ensures existing rows are valid without backfill. Body hashes for pre-existing symbols will be computed on the next re-index.

---

## Graph Traversal

### Impact Graph (Blast Radius)

BFS over reverse edges (dependents) from a root symbol. Returns all transitively affected symbols up to configurable depth (default 2, max 5), grouped by distance.

```
impact_graph(symbol_name, max_depth, repo_id):
  root = resolve_symbol(symbol_name)
  visited = {root.id}
  queue = [(root.id, 0)]
  results = []

  while queue not empty:
    (current_id, depth) = queue.pop_front()
    if depth >= max_depth: continue

    for (edge, sym) in db.get_dependents(current_id):
      if sym.id not in visited:
        visited.add(sym.id)
        results.push(ImpactNode { name, kind, file_path, distance: depth+1, edge_kind })
        queue.push((sym.id, depth+1))

  return results
```

### Logic Flow (Path Finding)

BFS through forward dependency edges to find paths from symbol A to symbol B. Returns up to N distinct paths (default 3), each capped at length 10 to prevent runaway traversal. Queue size capped at 10,000 entries.

The implementation uses a path-copying approach (each queue entry is a `Vec<i64>` of the path so far) rather than a predecessor map, because I need multiple distinct paths, not a single shortest path. Memory is bounded by the queue cap.

---

## MCP Tool Surface

19 tools organized into five groups. All tools accept JSON parameters via MCP and return JSON responses.

### Symbol Queries

| Tool | Purpose | Key Parameters |
|------|---------|----------------|
| `query_symbol` | Lookup by name/kind with linked memories | `name`, `kind?`, `repo?` |
| `get_file_symbols` | Structural TOC (signatures only) | `file_path`, `repo?` |
| `get_skeleton` | Token-efficient file view (70-90% reduction) | `file_path`, `repo?`, `detail?` |
| `batch_query` | Multi-symbol fetch with token budget + dependency hints | `symbol_names[]`, `max_tokens?`, `include_body?` |

### Graph Traversal

| Tool | Purpose | Key Parameters |
|------|---------|----------------|
| `get_dependencies` | Outgoing edges (depth 1-3) | `symbol_name`, `depth?` |
| `get_dependents` | Incoming edges (depth 1-3) | `symbol_name`, `depth?` |
| `get_impact_graph` | Blast radius analysis (depth 1-5) | `symbol_name`, `depth?`, `repo?` |
| `search_logic_flow` | Path tracing between two symbols | `from_symbol`, `to_symbol`, `max_paths?`, `repo?` |

### Search

| Tool | Purpose | Key Parameters |
|------|---------|----------------|
| `search_code` | FTS5 across symbol bodies | `query`, `kind?`, `repo?`, `max_results?` |
| `search_memory` | FTS5 across memories | `query`, `max_results?` |
| `get_context` | Context capsule with intent detection + budgeting | `query`, `max_tokens?`, `repo?` |

### Memory Management

| Tool | Purpose | Key Parameters |
|------|---------|----------------|
| `save_memory` | Persist decisions/patterns | `content`, `category`, `symbol_names?[]` |
| `list_memories` | Filtered listing | `category?`, `include_stale?`, `symbol_name?` |
| `update_memory` | Modify content/links | `memory_id`, `content?`, `category?`, `symbol_names?[]` |
| `delete_memory` | Remove | `memory_id` |

### Meta

| Tool | Purpose | Key Parameters |
|------|---------|----------------|
| `get_repo_overview` | Stats (files, symbols, memories, languages) | `repo?` |
| `get_health` | DB diagnostics (size, counts, FTS integrity) | *(none)* |
| `get_symbol_history` | Git blame for a symbol's file | `symbol_name`, `max_entries?`, `repo?` |
| `recover_session` | Post-compaction state restoration | `session_id?` |

### Auto-Observation Recording

Every tool call that touches symbols (query_symbol, search_code, get_context, get_dependencies, get_dependents, get_impact_graph) records a compact observation as an auto-memory. This creates a session trail that `recover_session` can reconstruct after context compaction.

### Dependency Hints (`batch_query`)

When `batch_query` returns a set of symbols, there are often critical unseen dependencies — interfaces a struct implements, traits a type references, or imports a function uses — that aren't in the result set but are necessary for understanding the returned code.

Phase 3 of `batch_query` queries the edges table for outgoing `type_ref`, `imports`, and `calls` edges from each returned symbol, filters out symbols already in the result set, and returns human-readable hints:

```
"References trait `Handler` (not in context)"
"Imports `serde_json` (not in context)"
"Calls `validate_input` (not in context)"
```

These hints let Claude make informed decisions about whether to fetch additional context without burning tokens on speculative `query_symbol` calls.

### Intent-Gated Recency Bias

FTS5 search in `get_context` supports an optional recency boost that promotes recently-changed files in the ranking. The boost is **gated on debug intent only** — bugs correlate strongly with recent edits, but this assumption doesn't hold for refactor, modify, or explore intents.

Implementation: `search_code_with_recency(query, kind, repo_id, limit, recency_boost)` adjusts the FTS5 rank score by a factor derived from the file's `indexed_at` timestamp relative to a 2-day window. The `recency_boost` parameter (0.0–1.0) controls the blend:

```sql
ORDER BY rank * (1.0 - ?recency_boost * MAX(0.0,
  (julianday(f.indexed_at) - julianday('now', '-2 days')) / 2.0))
```

For debug intent, `recency_boost = 0.5`. For all other intents, `recency_boost = 0.0` (no effect).

---

## VS Code Extension

TypeScript extension managing the Focal integration lifecycle.

### Responsibilities

1. **MCP Configuration** — Writes `focal` server entry to `~/.claude/settings.json` with the resolved binary path and workspace roots. Generates `.claude/CLAUDE.md` in each workspace root with tool usage guidance.

2. **Binary Resolution** — Platform-aware: resolves `focal.coreBinaryPath` setting → bundled `bin/focal` (or `bin/focal.exe` on Windows) → PATH fallback.

3. **Status Bar** — Shows indexing progress, symbol/memory counts, or error state. Clicking opens the command palette filtered to Focal commands.

4. **Commands:**
   - `focal.reindex` — Informational (Claude Code manages the binary lifecycle)
   - `focal.clearIndex` — Deletes `~/.focal/index.db` and WAL/SHM sidecar files
   - `focal.showMemories` — Directs to MCP tools
   - `focal.configureMcp` — Re-runs MCP auto-configuration

### MCP Settings Auto-Configuration

```typescript
// ~/.claude/settings.json
{
  "mcpServers": {
    "focal": {
      "command": "/path/to/focal",
      "args": ["/workspace/root1", "/workspace/root2"]
    }
  }
}
```

Idempotent — skips if the existing entry matches the current binary path and workspace roots.

---

## Concurrency Model

The binary uses `tokio` for async runtime, but the core data path is synchronous Rust behind `Arc<Mutex<Database>>`:

- MCP handlers acquire the mutex, execute SQL, release.
- File watcher runs in a dedicated OS thread. Acquires the mutex per-file (not per-batch) to minimize lock contention with concurrent MCP handlers.
- No connection pooling — single connection with WAL mode handles the read/write concurrency profile (many short reads, occasional writes).

This is intentionally minimal. The expected load is <100 MCP calls/minute from one Claude Code instance, and file change batches of <50 files. `Mutex` overhead is negligible at this scale. If multi-tenant or high-throughput use cases emerge, I'd move to `r2d2` connection pooling or separate reader/writer connections.

---

## Session Recovery

When Claude Code's context window fills and compacts, it loses all previously-sent symbol bodies and the conversational context about what was explored. `recover_session` reconstructs this:

```rust
pub struct SessionRecoveryData {
    pub session_id: String,
    pub manual_memories: Vec<Memory>,      // explicit decisions (highest signal)
    pub auto_observations: Vec<Memory>,    // tool usage trail
    pub recent_files: Vec<String>,         // files accessed
    pub symbol_names_accessed: Vec<String>, // symbols viewed
}
```

The recovery summary is structured for quick orientation:
1. **Stored decisions/notes** — manual memories with category tags
2. **Session activity** — auto-observations grouped by tool, last observation per tool
3. **Files accessed** — capped at 20 entries
4. **Symbols previously viewed** — capped at 30, with note that bodies will be re-sent fresh

After recovery, the `sent_symbols` set is cleared so progressive disclosure resets — Claude needs those bodies again since its context no longer has them.

---

## Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `rmcp` | 0.16 | MCP server (stdio + streamable HTTP) |
| `tree-sitter` | 0.26 | Incremental parsing framework |
| `tree-sitter-{go,rust,typescript,python}` | latest | Language grammars |
| `rusqlite` | 0.38 (bundled-full) | SQLite + FTS5 (statically linked) |
| `notify` | 8.2 | Platform-native filesystem watching |
| `tokio` | 1.x | Async runtime |
| `axum` | 0.8 | HTTP server for `--http` mode |
| `clap` | 4.x | CLI argument parsing |
| `sha2` | 0.10 | File content hashing |
| `serde` / `serde_json` | 1.x | JSON serialization for MCP responses |
| `walkdir` | 2.x | Recursive directory traversal |
| `dirs` | 6.x | Platform-specific home directory resolution |

`rusqlite` is built with `bundled-full` — SQLite is compiled from source and statically linked. No system SQLite dependency, guaranteed FTS5 availability, and consistent behavior across platforms.

---

## Failure Modes

| Failure | Impact | Mitigation |
|---------|--------|------------|
| Corrupt SQLite DB | All queries fail | `focal.clearIndex` command deletes and rebuilds from scratch |
| tree-sitter parse fails | File skipped, logged as error | Other files still indexed; errors surfaced in `IndexStats` |
| File watcher drops events | Stale index until next change or restart | Debounce thread + full re-index on startup |
| Mutex poisoned (panic in holder) | All subsequent lock attempts fail | Process restart; panic should not happen in steady state |
| Binary crash | MCP tools unavailable | Claude Code re-launches on next tool call |
| FTS5 desync | Search returns stale results | `get_health` checks FTS integrity; `clearIndex` rebuilds |
| Memory–symbol links broken | Memories lose symbol association | Re-linking by name on re-index; worst case: memory exists but is orphaned |

---

## Future Work

**Embedding-based retrieval.** FTS5 is syntactic — it doesn't understand that `"authentication"` relates to `"login_handler"`. Plan: integrate `fastembed` for local embedding generation, store vectors in a companion table, and blend cosine similarity with BM25 scores. No external API dependency.

**LSP Bridge.** Capture hover/definition/reference data from the active VS Code language server. This gives cross-crate type resolution, inferred types, and richer edge data than tree-sitter alone. The Grammar trait stays for baseline parsing; LSP supplements when available. This is the highest-leverage v2 feature — it fills the semantic gap that tree-sitter's syntax-only analysis leaves open.

**Cross-session analytics.** Track token usage before/after Focal per session. Instrument `get_context` calls with budget utilization metrics. Goal: quantify actual token savings to justify adoption.

---

## Running

```bash
# Build
cargo build --release

# Run with stdio MCP (default — Claude Code uses this)
./target/release/focal /path/to/workspace

# Run with HTTP MCP
./target/release/focal /path/to/workspace --http --port 3100

# Multiple workspaces
./target/release/focal /path/to/repo1 /path/to/repo2
```

Database location: `~/.focal/index.db`

Logs to stderr via `tracing` with `RUST_LOG=focal=info` default. Set `RUST_LOG=focal=debug` for verbose output.

---

## Release Pipeline

GitHub Actions builds platform-specific binaries and VS Code extensions on tag push (`v*`).

### Build Matrix

| Target | Runner | Artifact |
|--------|--------|----------|
| `x86_64-unknown-linux-gnu` | ubuntu-latest | `focal-linux-amd64` |
| `aarch64-unknown-linux-gnu` | ubuntu-latest | `focal-linux-arm64` |
| `x86_64-apple-darwin` | macos-latest | `focal-darwin-amd64` |
| `aarch64-apple-darwin` | macos-latest | `focal-darwin-arm64` |
| `x86_64-pc-windows-msvc` | windows-latest | `focal-windows-amd64.exe` |

Linux ARM64 cross-compiles with `gcc-aarch64-linux-gnu`. macOS builds natively on `macos-latest` (which supports both x64 and arm64 targets).

### Platform VSIX Bundling

Each binary is bundled into a platform-specific `.vsix` package via `vsce package --target <platform>`. This eliminates the need for users to separately install the Rust binary — the extension carries the correct native binary for the target platform.

| VS Code Target | Binary Source |
|----------------|---------------|
| `linux-x64` | `focal-linux-amd64` |
| `linux-arm64` | `focal-linux-arm64` |
| `darwin-x64` | `focal-darwin-amd64` |
| `darwin-arm64` | `focal-darwin-arm64` |
| `win32-x64` | `focal-windows-amd64.exe` |

The extension's `resolveBinaryPath()` checks for the bundled binary at `extension/bin/focal` (or `focal.exe` on Windows) before falling back to PATH lookup. SHA-256 checksums are published alongside binaries in each release.
