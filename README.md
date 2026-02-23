# Focal

Structural code indexing for AI-assisted development. Focal parses your codebase with tree-sitter, builds a dependency graph in SQLite, and serves focused context to Claude Code via MCP — so your AI assistant stops re-reading hundreds of files per session.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## The Problem

Claude Code has no structural awareness of your codebase. Every session, it re-reads files it already analyzed, forgets decisions it made, and burns tokens on redundant context. On a 50K-symbol codebase, I've seen sessions where 60-70% of token usage was Claude re-discovering the same call graph it traced an hour ago.

Focal fixes this by giving Claude a persistent, queryable model of your code's structure — symbols, dependencies, blast radius — plus a memory system that survives across sessions.

## How It Works

```
┌──────────────────────────────────┐
│  VS Code Extension (TypeScript)  │
│  Status bar · Commands · MCP cfg │
└──────────────┬───────────────────┘
               │ spawns
┌──────────────▼───────────────────┐
│  focal-core (Rust binary)        │
│  tree-sitter │ SQLite + FTS5     │
│  File watcher │ Memory store     │
│  MCP server (stdio | HTTP)       │
└──────────────┬───────────────────┘
               │ MCP (stdio default)
┌──────────────▼───────────────────┐
│  Claude Code                     │
│  Calls MCP tools for context     │
│  Stores decisions as memories    │
└──────────────────────────────────┘
```

1. The VS Code extension spawns the Rust binary on workspace open, passing workspace root paths.
2. The binary indexes your code with tree-sitter, stores symbols and edges in SQLite at `~/.focal/index.db`.
3. A file watcher (notify crate, 500ms debounce) detects changes and incrementally re-indexes affected files.
4. Claude Code connects via MCP and calls tools like `get_context`, `query_symbol`, and `get_impact_graph`.
5. Responses include only the relevant symbols and memories — not entire files.

## Quick Start

### Prerequisites

- Rust toolchain (1.75+)
- VS Code with Claude Code
- One of: Go, Rust, TypeScript/JavaScript, or Python codebase

### Install

```bash
# Build the binary
git clone https://github.com/InfraWhisperer/focal.git
cd focal
cargo build --release

# The binary is at target/release/focal
```

### Run standalone (CLI)

The binary works independently — no VS Code extension required. It indexes your workspace on startup, watches for file changes, and serves MCP tools over stdio or HTTP.

```bash
# stdio MCP server (default — AI clients connect via stdin/stdout)
focal /path/to/workspace

# Multiple workspaces in a single index
focal /path/to/repo1 /path/to/repo2

# HTTP MCP server on port 3100 (persistent daemon mode)
focal /path/to/workspace --http

# HTTP on custom port
focal /path/to/workspace --http --port 8080
```

**Verbose logging** (logs go to stderr, MCP traffic on stdout):
```bash
RUST_LOG=focal=debug focal /path/to/workspace
```

#### Connecting Claude Code (stdio)

Add to `~/.claude/settings.json`:
```json
{
  "mcpServers": {
    "focal": {
      "command": "/path/to/focal",
      "args": ["/path/to/workspace"]
    }
  }
}
```

Claude Code spawns the binary automatically on its next session.

#### Connecting VS Code (Copilot, Continue, etc.)

Add to your VS Code `settings.json` (Cmd+Shift+P → "Preferences: Open User Settings (JSON)"):

**stdio mode** (editor spawns the process):
```json
{
  "mcp": {
    "servers": {
      "focal": {
        "type": "stdio",
        "command": "/path/to/focal",
        "args": ["/path/to/workspace"]
      }
    }
  }
}
```

**HTTP mode** (connect to a running server):
```bash
# Start the server first
focal /path/to/workspace --http --port 3100
```
```json
{
  "mcp": {
    "servers": {
      "focal": {
        "url": "http://localhost:3100/mcp"
      }
    }
  }
}
```

HTTP mode is useful when you want the server to outlive editor restarts, serve multiple clients, or when your org restricts MCP server spawning.

### Run via VS Code extension

1. Install the Focal extension from the marketplace (or build from `extension/`).
2. Open a workspace. The extension spawns the binary automatically.
3. Run **Focal: Configure MCP** from the command palette — this writes the MCP entry into `.claude/settings.json`.
4. Claude Code picks up the MCP server on its next session.

That's it. Claude Code now has access to all 19 MCP tools.

## MCP Tools

### Context & Retrieval

| Tool | What it does |
|------|-------------|
| `get_context` | Smart context retrieval with intent detection and token budgeting. Given a task description, returns a **context capsule**: pivot symbols with full source, adjacent symbols skeletonized to signatures. Stays within a configurable token budget (default 12K). |
| `query_symbol` | Look up a symbol by name. Returns signature, body, file location, and linked memories. |
| `search_code` | FTS5 full-text search across symbol bodies and signatures. |
| `get_skeleton` | Signatures-only view of a file. 70-90% token reduction vs reading the full file. |
| `batch_query` | Fetch multiple symbols in one call with a shared token budget. |

### Graph Analysis

| Tool | What it does |
|------|-------------|
| `get_dependencies` | Outgoing edges from a symbol (calls, imports, type refs). |
| `get_dependents` | Incoming edges — who depends on this symbol. |
| `get_impact_graph` | Blast radius analysis: all transitive dependents up to depth 5, grouped by file. |
| `search_logic_flow` | Trace execution paths between two symbols through the call graph. |
| `get_file_symbols` | Structural table of contents for a file — names, kinds, signatures, line ranges. |

### Memory System

| Tool | What it does |
|------|-------------|
| `save_memory` | Persist a decision, pattern, or insight linked to specific symbols. |
| `list_memories` | List memories with optional category/symbol/staleness filters. |
| `search_memory` | FTS5 search across stored memories. |
| `update_memory` | Update content, category, or symbol links. |
| `delete_memory` | Remove a memory. |

### Diagnostics

| Tool | What it does |
|------|-------------|
| `get_repo_overview` | Aggregate stats: file count, symbol count, language breakdown, memory count. |
| `get_health` | Database diagnostics. |
| `get_symbol_history` | Git blame for a symbol — recent commits touching its source range. |
| `recover_session` | Restore working state after context compaction — decisions, files, symbols. |

## Key Concepts

### Context Capsules

`get_context` doesn't dump files. It returns a **capsule**: a token-budgeted package of the most relevant symbols for your task. Pivot symbols (highest relevance) get full source. Adjacent symbols get skeletonized to signatures. Linked memories are included. The response reports actual token usage so Claude can plan follow-up queries.

### Intent Detection

When you call `get_context("fix the panic in handleRequest")`, Focal classifies the intent as `debug` and adjusts retrieval — prioritizing error paths and callers over, say, type definitions. Four intents:

- **debug** — error paths, callers, panic sites (keywords: fix, bug, crash, panic, broken)
- **refactor** — blast radius, dependents (keywords: refactor, rename, extract, split)
- **modify** — feature scope, related types (keywords: add, implement, create, build)
- **explore** — default balanced retrieval when no strong signal

### Progressive Disclosure

The first time Claude requests a symbol in a session, it gets the full body. Subsequent requests for the same symbol return the skeleton plus a note that the full body was already sent. This prevents the re-reading problem at the protocol level.

### Memory System

Memories persist across sessions in SQLite. They're linked to symbols, so when a file changes, linked memories get marked stale automatically. Retrieval ranks memories using:

| Signal | Weight | Description |
|--------|--------|-------------|
| BM25 text relevance | 0.35 | FTS5 match against query |
| TF-IDF cosine similarity | 0.25 | Term frequency similarity |
| Recency decay | 0.20 | Exponential decay with 1-week half-life |
| Graph proximity | 0.15 | Shortest path from query symbols to memory symbols |
| Staleness penalty | -0.30 | Applied when the linked source has changed |

### Auto-Capture

Every MCP tool call that touches symbols generates a compact observation memory (~100-200 bytes) automatically. These build a session trail without any explicit action. Auto-observations expire after 90 days; manual memories never expire.

## Supported Languages

| Language | Grammar | Symbol Types |
|----------|---------|-------------|
| Go | `tree-sitter-go` | functions, methods, structs, interfaces, type aliases, constants |
| Rust | `tree-sitter-rust` | functions, methods, structs, enums, traits, type aliases, constants, modules |
| TypeScript/JavaScript | `tree-sitter-typescript` | functions, methods, classes, interfaces, type aliases, constants |
| Python | `tree-sitter-python` | functions, methods, classes, constants |

Adding a language means implementing a `Grammar` trait (language, symbol query, reference query, node classification, import resolution) and registering the tree-sitter grammar crate.

## Configuration

### VS Code Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `focal.excludePatterns` | `["node_modules", ".git", "vendor", "target", "dist"]` | Glob patterns to exclude from indexing |
| `focal.maxFileSize` | `500000` | Skip files larger than N bytes |
| `focal.coreBinaryPath` | `""` | Path to focal binary (auto-detected if empty) |

### VS Code Commands

- **Focal: Reindex Workspace** — triggers full re-index
- **Focal: Clear Index** — drops and rebuilds the database
- **Focal: Show Memories** — quick pick listing memories with staleness status
- **Focal: Configure MCP** — writes/updates MCP config in `.claude/settings.json`

### CLAUDE.md Integration

Drop a `.claude/CLAUDE.md` in your repo to teach Claude how to use Focal's tools effectively. See [this project's own CLAUDE.md](.claude/CLAUDE.md) for an example — it includes tool priority, effective tool chains, and progressive disclosure guidance.

## Design Decisions

**Why SQLite over a graph database?**
SQLite is zero-ops, single-file, embeddable, and handles codebases up to ~100K symbols without issue. The edge table with source/target foreign keys handles 1-3 hop traversals efficiently with indexed joins. A dedicated graph DB adds operational complexity for marginal query benefit at this scale.

**Why tree-sitter over LSP?**
tree-sitter gives deterministic, offline, language-agnostic AST parsing with no server dependency. LSP provides richer semantic info (type resolution, cross-package refs) but requires a running language server per language. tree-sitter is the right starting point; LSP data can supplement it later.

**Why stdio MCP by default?**
No port management, no daemon lifecycle, no firewall concerns. Claude Code natively supports stdio MCP servers. HTTP mode (`--http`) is available for persistent daemon use or multi-client access.

**Why a single database?**
Single DB at `~/.focal/index.db` enables cross-repo queries — "who in repo B calls this function from repo A?" The `repositories` table provides isolation when needed. One file to back up, one file to migrate.

## Project Structure

```
focal/
├── core/                   # Rust binary (focal-core)
│   ├── src/
│   │   ├── main.rs         # CLI + MCP server startup
│   │   ├── mcp.rs          # MCP tool handlers (19 tools)
│   │   ├── db.rs           # SQLite schema + queries
│   │   ├── indexer.rs       # tree-sitter parsing + symbol extraction
│   │   ├── context.rs      # Context engine: intent detection, token budgeting
│   │   ├── graph.rs        # Graph traversal (deps, dependents, impact, flow)
│   │   ├── watcher.rs      # File watcher (notify crate, debounced)
│   │   └── grammar/        # Per-language tree-sitter grammars
│   │       ├── go.rs
│   │       ├── rust_lang.rs
│   │       ├── typescript.rs
│   │       └── python.rs
│   └── tests/              # Integration + unit tests
├── extension/              # VS Code extension (TypeScript)
│   └── src/
│       ├── extension.ts    # Activation, binary lifecycle
│       ├── commands.ts     # Command handlers
│       ├── statusbar.ts    # Index stats display
│       └── mcp-config.ts   # Auto-configure .claude/settings.json
└── docs/plans/             # Design docs
```

## Contributing

PRs welcome. The codebase is small enough to index with Focal itself.

```bash
# Run tests
cargo test

# Run with logging
RUST_LOG=focal=debug cargo run -- /path/to/workspace

# Build the extension
cd extension && npm run compile
```

If you're adding a language, the main work is implementing the `Grammar` trait in `core/src/grammar/` and adding the tree-sitter grammar crate to `core/Cargo.toml`.

## License

MIT
