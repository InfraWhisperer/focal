# Focal

**Stop burning tokens on code Claude already read.**

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/InfraWhisperer/focal)](https://github.com/InfraWhisperer/focal/releases)
[![VS Code Marketplace](https://img.shields.io/visual-studio-marketplace/v/rpotluri.focal)](https://marketplace.visualstudio.com/items?itemName=rpotluri.focal)

Focal is an MCP server that gives Claude Code structural awareness of your codebase — symbols, dependencies, blast radius — so it retrieves exactly the context it needs instead of re-reading entire files every session.

## The Numbers

| Metric | Without Focal | With Focal | Improvement |
|--------|-------------:|-----------:|------------:|
| Tokens per compaction recovery | 55,344 | ~1,700 | **32x fewer** |
| Tokens to understand a file | 16,931 | 3,598 | **4.7x fewer** |
| Cost per recovery (Opus) | $0.83 | $0.03 | **97% cheaper** |
| Tool call latency | — | <30ms | **imperceptible** |
| Disk overhead | — | 816 KB | **trivial** |

Full methodology and raw data: [benchmarks/bench.md](benchmarks/bench.md).

---

## Quick Start

### VS Code (recommended)

1. Install [Focal](https://marketplace.visualstudio.com/items?itemName=rpotluri.focal) from the VS Code Marketplace.
2. Open a project. The extension downloads the binary, configures MCP, and generates a `CLAUDE.md` with tool guidance.
3. Use Claude Code as usual. Focal serves context automatically.

### CLI

```bash
cd your-project
focal --init
```

That writes `.mcp.json` in your project root. Claude Code picks up the MCP server on its next session.

### Build from Source

```bash
git clone https://github.com/InfraWhisperer/focal.git
cd focal && cargo build --release
# Binary at target/release/focal
```

---

## Features

**Progressive disclosure** — The first time Claude requests a symbol, it gets the full body. Subsequent requests return a compact skeleton with an "already sent" marker. This eliminates the re-reading problem at the protocol level.

**Dependency and impact analysis** — Focal knows which symbols call which. Claude can check blast radius before making changes instead of grepping through imports.

**Persistent memory** — Architectural decisions, conventions, and context survive across sessions. Memories link to specific symbols; when those symbols change, linked memories are flagged as potentially stale.

**19 MCP tools** for every coding workflow:

| Category | Tools |
|----------|-------|
| Context and retrieval | `get_context`, `query_symbol`, `search_code`, `get_skeleton`, `batch_query` |
| Graph analysis | `get_dependencies`, `get_dependents`, `get_impact_graph`, `search_logic_flow`, `get_file_symbols` |
| Memory | `save_memory`, `list_memories`, `search_memory`, `update_memory`, `delete_memory` |
| Session and diagnostics | `recover_session`, `get_repo_overview`, `get_health`, `get_symbol_history` |

---

## Supported Languages

Go, Rust, TypeScript/JavaScript, Python — all via tree-sitter.

---

## Client Setup

### Claude Code

Run `focal --init` in your project root — it writes `.mcp.json` automatically. Or add it manually:

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

Claude Code picks up the MCP server on its next session.

### VS Code (Copilot, Continue, etc.)

Add to your VS Code `settings.json`:

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

### HTTP Mode

For persistent daemon use or multi-client access:

```bash
focal /path/to/workspace --http --port 3100
```

---

## Configuration

### VS Code Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `focal.excludePatterns` | `["node_modules", ".git", "vendor", "target", "dist"]` | Glob patterns to exclude from indexing |
| `focal.maxFileSize` | `500000` | Skip files larger than N bytes |
| `focal.coreBinaryPath` | `""` | Path to focal binary (auto-detected if empty) |

### VS Code Commands

- **Focal: Reindex Workspace** — full re-index
- **Focal: Clear Index** — drop and rebuild the database
- **Focal: Show Memories** — list memories with staleness status
- **Focal: Configure MCP** — write/update `.mcp.json` in the workspace root

### CLAUDE.md Integration

Drop a `.claude/CLAUDE.md` in your repo to teach Claude how to use Focal's tools effectively. See [this project's own CLAUDE.md](.claude/CLAUDE.md) for an example.

---

## Contributing

PRs welcome. Open an issue first for non-trivial changes.

```bash
cargo build --release
cargo test
cargo clippy -- -D warnings

# VS Code extension
cd extension && npm run compile
```

Adding a language means implementing the `Grammar` trait in `core/src/grammar/` and registering the tree-sitter grammar crate.

---

## License

[MIT](LICENSE)
