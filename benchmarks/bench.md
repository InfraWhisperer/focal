# Focal MCP Server — Benchmarks

Measured against **focal's own codebase**: 35 files, 403 symbols, 25 Rust + 10 TypeScript.
All numbers from real MCP tool calls against a live index — no mocks, no simulation.

---

## 1. Token Savings: `get_skeleton` vs Full File Read

Every file in the project, measured. Full `Read` vs `get_skeleton` on the same file.

| File | Full (tokens) | Skeleton (tokens) | Reduction |
|------|-------------:|-----------------:|----------:|
| `core/src/main.rs` | 2,679 | 111 | **95.8%** |
| `core/src/mcp.rs` | 10,722 | 2,532 | **76.4%** |
| `core/src/db.rs` | 16,931 | 3,598 | **78.8%** |
| `core/src/indexer.rs` | 3,801 | 517 | **86.4%** |
| `core/src/context.rs` | 3,572 | 534 | **85.1%** |
| `core/src/graph.rs` | 1,317 | 318 | **75.9%** |
| `core/src/watcher.rs` | 1,121 | 183 | **83.7%** |
| `extension/src/extension.ts` | 485 | 135 | **72.2%** |
| `extension/src/binary-manager.ts` | 1,696 | 453 | **73.3%** |
| `extension/src/mcp-config.ts` | 1,315 | 178 | **86.5%** |
| `extension/src/statusbar.ts` | 491 | 363 | **26.1%** |
| `extension/src/commands.ts` | 506 | 71 | **86.0%** |
| **Total (12 files)** | **44,636** | **8,993** | **79.9%** |

Skeleton views deliver the full API surface — every struct, method signature, trait impl — at **~1/5 the token cost**. Best case (`main.rs`, dominated by function bodies) hits 95.8%. Worst case (`statusbar.ts`, almost entirely short method signatures) still compresses to 74% of original lines.

---

## 2. Smart Context vs Naive Re-Read After Compaction

When Claude Code compacts context, the default behavior is re-reading every file it previously touched. Focal replaces that with targeted retrieval.

| Approach | Tokens consumed | What you get |
|----------|---------------:|--------------------------------------------|
| **Naive re-read** (all 12 source files) | **44,636** | Raw source dumped into context |
| `recover_session` | **365** | Session state: files accessed, symbols viewed, decisions |
| `get_context` ("fix panic in database", 4K) | **1,908** | 56 items — 5 pivot symbols with bodies + 51 adjacent skeletons |
| `get_context` ("indexing pipeline", 4K) | **2,720** | 28 items — 5 pivots + 23 adjacent |
| `get_context` ("MCP request handling", 8K) | **1,179** | 13 items — 5 pivots + 8 adjacent |
| **Focal total** (recover + one context call) | **~2,300** | Focused working set with full context |

**Reduction: 44,636 → 2,300 tokens = 95% savings per compaction event.**

Over a typical session with 3-5 compactions, that's **130K-210K tokens saved** on re-reads alone.

---

## 3. Cost Impact

Token estimates use ~4 chars/token. Pricing at current Claude rates.

| Scenario | Sonnet ($3/MTok) | Opus ($15/MTok) |
|----------|----------------:|---------------:|
| **1 naive re-read** | $0.134 | $0.670 |
| **1 Focal recovery** | $0.007 | $0.035 |
| **Savings per compaction** | $0.127 | $0.635 |
| **3 compactions/session** | $0.38 saved | $1.91 saved |
| **5 compactions/session** | $0.64 saved | $3.18 saved |
| **20 sessions/week (Opus)** | — | **$38-64/week saved** |

For heavy Opus users doing multi-file development, the token savings translate to real dollars. Focal is free and open source.

---

## 4. MCP Server Latency

Measured via stdio round-trips (process init + JSON-RPC + tool execution + response). Three runs per operation, reporting median.

| Operation | Latency (median) | Response tokens |
|-----------|----------------:|---------------:|
| `get_repo_overview` | 14ms | 68 |
| `get_skeleton` (db.rs, 63 symbols) | 13ms | 3,598 |
| `get_context` (4K budget) | 13ms | 1,908 |
| `search_code` (10 results) | 12ms | 579 |
| `get_file_symbols` (mcp.rs, 49 symbols) | 13ms | 1,451 |

All operations complete in **<20ms**. The MCP protocol overhead is negligible compared to LLM inference latency (seconds). Focal adds zero perceptible delay.

---

## 5. Resource Footprint

| Metric | Value |
|--------|------:|
| Binary size (release, darwin-arm64) | 14 MB |
| Database size (403 symbols, 35 files) | 816 KB |
| Database size per symbol | ~2 KB |
| Indexing (full 35-file repo) | <1s |

The SQLite database with FTS5 indexes scales linearly. A 10K-symbol monorepo would use ~20 MB of disk.

---

## 6. Tool-by-Tool Token Efficiency

Real measurements from 12 tool calls against the live index.

| Tool | Tokens returned | Items | Use case |
|------|---------------:|------:|----------|
| `get_repo_overview` | 68 | 1 repo | Orientation — file/symbol counts, languages |
| `recover_session` | 365 | 8 files, 6 symbols | Post-compaction — restore working state |
| `batch_query` (3 symbols) | 545 | 3 | Multi-symbol fetch with bodies |
| `search_code` ("parse") | 579 | 10 | FTS5 search — ranked results |
| `get_context` (8K budget) | 1,179 | 13 | Smart retrieval — 5 pivots + 8 adjacent |
| `get_file_symbols` (mcp.rs) | 1,451 | 49 | File structure — names, kinds, signatures |
| `get_file_symbols` (db.rs) | 1,864 | 63 | File structure — names, kinds, signatures |
| `get_context` (4K, debug intent) | 1,908 | 56 | Intent-aware retrieval — error paths prioritized |
| `search_code` ("SQLite") | 1,924 | 10 | FTS5 search — results with bodies |
| `get_context` (4K, explore) | 2,720 | 28 | Balanced retrieval — 5 pivots + 23 adjacent |
| `get_skeleton` (db.rs) | 3,598 | 63 | Signatures only — 79% cheaper than full read |

Compare: `Read` on `db.rs` costs **16,931 tokens**. `get_skeleton` returns every signature for **3,598 tokens**. Same navigational value, 79% cheaper.

---

## 7. Progressive Disclosure

Focal tracks which symbols you've already seen in a session. On repeat queries:

- **First request**: full body (you need to understand the code)
- **Subsequent requests**: skeleton + note "body previously sent" (you already have it)

This prevents the most wasteful pattern in Claude Code: re-reading the same 500-line function 4 times across a session because the context window forgot it saw it before. Focal remembers.

---

## Summary

| Metric | Without Focal | With Focal | Improvement |
|--------|-------------:|-----------:|------------:|
| Tokens per compaction recovery | 44,636 | ~2,300 | **19x fewer** |
| Cost per recovery (Opus) | $0.67 | $0.04 | **95% cheaper** |
| Tokens to understand a file | 16,931 | 3,598 | **4.7x fewer** |
| Latency overhead | — | <20ms | **imperceptible** |
| Disk overhead | — | 816 KB | **trivial** |
