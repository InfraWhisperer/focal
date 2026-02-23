import * as vscode from "vscode";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";

interface McpServerEntry {
  command: string;
  args?: string[];
  [key: string]: unknown;
}

interface ClaudeSettings {
  mcpServers?: Record<string, McpServerEntry>;
  [key: string]: unknown;
}

/**
 * Auto-configures ~/.claude/settings.json so Claude Code picks up
 * the focal MCP server. Adds the entry only if it's missing
 * or if the workspace roots changed.
 */
export async function autoConfigureMcp(
  binaryPath: string,
  workspaceRoots: string[]
): Promise<void> {
  const claudeDir = path.join(os.homedir(), ".claude");
  const settingsPath = path.join(claudeDir, "settings.json");

  let settings: ClaudeSettings = {};

  // Read existing settings if present
  if (fs.existsSync(settingsPath)) {
    try {
      const raw = fs.readFileSync(settingsPath, "utf-8");
      settings = JSON.parse(raw) as ClaudeSettings;
    } catch {
      // Corrupted file — we'll overwrite the mcpServers section
      // but preserve whatever top-level keys we can.
      vscode.window.showWarningMessage(
        "Focal: ~/.claude/settings.json was malformed; rewriting MCP config."
      );
    }
  }

  if (!settings.mcpServers) {
    settings.mcpServers = {};
  }

  const existing = settings.mcpServers["focal"];
  const desiredArgs = [...workspaceRoots];

  // Skip if already configured with the same binary and args
  if (
    existing &&
    existing.command === binaryPath &&
    JSON.stringify(existing.args) === JSON.stringify(desiredArgs)
  ) {
    return;
  }

  settings.mcpServers["focal"] = {
    command: binaryPath,
    args: desiredArgs,
  };

  // Ensure ~/.claude/ exists
  if (!fs.existsSync(claudeDir)) {
    fs.mkdirSync(claudeDir, { recursive: true });
  }

  fs.writeFileSync(settingsPath, JSON.stringify(settings, null, 2) + "\n", "utf-8");

  vscode.window.showInformationMessage(
    "Focal: configured MCP server in ~/.claude/settings.json"
  );

  // Generate project-level CLAUDE.md for each workspace root
  for (const root of workspaceRoots) {
    generateClaudeMd(root);
  }
}

/**
 * Generates a project-level .claude/CLAUDE.md with Focal tool usage
 * guidance. Skips if the file already exists to avoid overwriting user edits.
 */
export function generateClaudeMd(workspaceRoot: string): void {
  const claudeDir = path.join(workspaceRoot, ".claude");
  const claudeMdPath = path.join(claudeDir, "CLAUDE.md");

  // Don't overwrite existing CLAUDE.md
  if (fs.existsSync(claudeMdPath)) {
    return;
  }

  if (!fs.existsSync(claudeDir)) {
    fs.mkdirSync(claudeDir, { recursive: true });
  }

  const content = `# Focal Integration

This project is indexed by Focal, a structural code indexing tool.
You have access to Focal's MCP tools — prefer them over raw file reading
when possible.

## Tool Priority

0. **After context compaction** call \`recover_session\` — restores your working
   state (decisions, files, symbols) and resets progressive disclosure so
   previously-sent symbol bodies will be re-sent fresh.
1. **Start every new task** with \`get_context\` — it returns a token-budgeted
   capsule with the most relevant symbols, their dependencies, and linked memories.
2. Use \`get_skeleton\` before \`Read\` — 70-90% fewer tokens for understanding file structure.
3. Use \`get_impact_graph\` before making changes — know the blast radius.
4. Use \`search_code\` for semantic search (FTS5) alongside Grep for literal search.
5. Use \`save_memory\` to persist architectural decisions, conventions, and insights.

## Effective Tool Chains

- **After compaction:** \`recover_session\` → resume with \`get_context\` on the current task
- **Bug fix:** \`get_context("fix X")\` → \`get_impact_graph("X")\` → \`query_symbol\` for details
- **New feature:** \`get_context("add X")\` → \`get_skeleton\` of target files → \`get_dependencies\`
- **Refactor:** \`get_context("refactor X")\` → \`get_impact_graph\` for blast radius → \`search_logic_flow\`
- **Code review:** \`get_skeleton\` per changed file → \`get_dependencies\` for each modified symbol

## Available Tools

| Tool | Purpose |
|------|---------|
| \`get_context\` | Smart context retrieval with token budgeting and intent detection |
| \`query_symbol\` | Look up specific symbols by name |
| \`search_code\` | Full-text search across all indexed code |
| \`search_memory\` | Full-text search across stored memories |
| \`get_skeleton\` | Token-efficient file view (signatures only) |
| \`get_impact_graph\` | Blast radius analysis for a symbol |
| \`search_logic_flow\` | Trace call paths between two symbols |
| \`get_dependencies\` | Outgoing dependency edges |
| \`get_dependents\` | Incoming dependency edges |
| \`get_file_symbols\` | List all symbols in a file |
| \`save_memory\` | Store a decision, pattern, or insight |
| \`list_memories\` | List stored memories |
| \`batch_query\` | Fetch multiple symbols in one call |
| \`get_repo_overview\` | High-level repo stats |
| \`get_health\` | Database diagnostics |
| \`get_symbol_history\` | Git blame for a symbol |
| \`recover_session\` | Restore session state after context compaction |
`;

  fs.writeFileSync(claudeMdPath, content, "utf-8");
}
