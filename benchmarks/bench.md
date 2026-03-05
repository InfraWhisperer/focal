# Focal MCP Server — Benchmarks

Measured against **focal's own codebase**: 35 files, 403 symbols, 25 Rust + 10 TypeScript.
All numbers from real MCP tool calls, not synthetic estimates.

---

## 1. Token Savings: `get_skeleton` vs Full File Read

The core value proposition — how much do you save by reading structure instead of source?

| File | Full (tokens) | Skeleton (tokens) | Reduction |
|------|-------------:|-----------------:|----------:|
| `core/src/main.rs` | 2,679 | 111 | **95.8%** |
| `core/src/context.rs` | 3,572 | 534 | **85.1%** |
| `core/src/indexer.rs` | 3,800 | 516 | **86.4%** |
| `core/src/mcp.rs` | 10,722 | 2,532 | **76.4%** |
| `core/src/db.rs` | 16,931 | 3,598 | **78.7%** |
| **Total (5 files)** | **37,705** | **7,291** | **80.7%** |

Skeleton views deliver the full API surface — every struct, method signature, trait impl — at **~1/5 the token cost**. Best case (files dominated by function bodies) hits 95%+ reduction.

---

## 2. Smart Context vs Naive Re-Read After Compaction

When Claude Code hits a context window limit and compacts, the default behavior is to re-read every file it previously touched. Focal replaces that with targeted retrieval.

| Approach | Tokens consumed | What you get |
|----------|---------------:|--------------------------------------------|
| **Naive re-read** (all .rs + .ts files) | **55,344** | Raw source of every file in the repo |
| `recover_session` | **250** | Session state: prior decisions, files accessed, symbols viewed |
| `get_context` (4K budget) | **1,441** | 18 relevant symbols — 1 full pivot + 17 skeletons |
| `get_context` (8K budget) | **451** | 6 targeted symbols matching query intent |
| **Focal total** (recover + context) | **~1,700** | Focused working set with full context |

**Reduction: 55,344 → 1,700 tokens = 97% savings per compaction event.**

Over a typical session with 3–5 compactions, that's **150K–270K tokens saved** on re-reads alone.

---

## 3. Cost Impact

Token estimates use ~4 chars/token. Pricing at current Claude rates.

| Scenario | Sonnet ($3/MTok) | Opus ($15/MTok) |
|----------|----------------:|---------------:|
| **1 naive re-read** | $0.166 | $0.830 |
| **1 Focal recovery** | $0.005 | $0.026 |
| **Savings per compaction** | $0.161 | $0.804 |
| **3 compactions/session** | $0.48 saved | $2.41 saved |
| **5 compactions/session** | $0.81 saved | $4.02 saved |
| **20 sessions/week (Opus)** | — | **$48–80/week saved** |

For heavy Opus users doing multi-file development, Focal pays for itself immediately — it's free and open source, but the token savings are real dollars.

---

## 4. MCP Server Latency

Does Focal slow things down? Measured via stdio round-trips including process init.

| Operation | Latency | Response tokens |
|-----------|--------:|---------------:|
| `get_repo_overview` | 20ms | 67 |
| `get_skeleton` (db.rs, 62 symbols) | 27ms | 1,680 |
| `get_context` (8K budget) | 16ms | 451 |
| `search_code` (20 results) | 16ms | 1,756 |
| `recover_session` | <20ms | 250 |
| `get_file_symbols` (48 symbols) | <20ms | 1,335 |

All operations complete in **<30ms**. The MCP protocol overhead is negligible compared to LLM inference latency (seconds). Focal adds zero perceptible delay to the workflow.

---

## 5. Resource Footprint

| Metric | Value |
|--------|------:|
| Binary size (release) | 14 MB |
| Database size (403 symbols, 35 files) | 816 KB |
| Database size per symbol | ~2 KB |
| Indexing (full repo) | <1s |
| Memory (steady state) | ~15 MB RSS |

The SQLite database with FTS5 indexes scales linearly. A 10K-symbol monorepo would use ~20 MB of disk — trivial.

---

## 6. Tool-by-Tool Token Efficiency

How much context does each tool return per call?

| Tool | Typical tokens | Use case |
|------|---------------:|----------|
| `get_repo_overview` | 67 | Orientation — file/symbol counts, languages |
| `recover_session` | 250 | Post-compaction — restore working state |
| `get_context` | 450–1,500 | Smart retrieval — intent-aware symbol selection |
| `get_skeleton` | 500–3,600 | File structure — signatures without bodies |
| `get_file_symbols` | 1,335 | File symbol listing — names and signatures |
| `search_code` (FTS5) | 1,756 | Full-text search — includes bodies |
| `batch_query` | 168–4,000 | Multi-symbol fetch within token budget |
| `get_impact_graph` | <100 | Blast radius — what breaks if you change X |

Compare with `Read` on `db.rs`: **16,931 tokens** for the full file vs `get_skeleton`: **3,598 tokens** for every signature. Same information density for navigation, 79% less cost.

---

## 7. The Real Win: Progressive Disclosure

Focal tracks which symbols you've already seen in a session. On repeat queries:

- **First request**: full body (you need to understand the code)
- **Subsequent requests**: skeleton + note "body previously sent" (you already have it)

This prevents the most wasteful pattern in Claude Code: re-reading the same 500-line function 4 times across a session because the context window forgot it saw it before. Focal remembers.

---

## Summary

| Metric | Without Focal | With Focal | Improvement |
|--------|-------------:|-----------:|------------:|
| Tokens per compaction recovery | 55,344 | ~1,700 | **32x fewer** |
| Cost per recovery (Opus) | $0.83 | $0.03 | **97% cheaper** |
| Tokens to understand a file | 16,931 | 3,598 | **4.7x fewer** |
| Latency overhead | — | <30ms | **imperceptible** |
| Disk overhead | — | 816 KB | **trivial** |
