import * as vscode from "vscode";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { StatusBarManager } from "./statusbar";
import { autoConfigureMcp } from "./mcp-config";

/**
 * Registers all Focal commands.
 *
 * With the lifecycle manager removed, commands that previously interacted
 * with the binary process now show informational messages directing the
 * user to restart Claude Code (which manages the binary via MCP).
 */
export function registerCommands(
  context: vscode.ExtensionContext,
  binaryPath: string,
  workspaceRoots: string[],
  statusBar: StatusBarManager
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("focal.reindex", () => {
      vscode.window.showInformationMessage(
        "Focal: Claude Code will re-index on next connection. " +
        "Restart Claude Code to trigger immediate re-index."
      );
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("focal.clearIndex", () => {
      const dbPath = path.join(os.homedir(), ".focal", "index.db");
      if (fs.existsSync(dbPath)) {
        fs.unlinkSync(dbPath);
        // Clean up WAL/SHM sidecar files left by SQLite WAL mode
        for (const suffix of ["-wal", "-shm"]) {
          const sidecar = dbPath + suffix;
          if (fs.existsSync(sidecar)) {
            fs.unlinkSync(sidecar);
          }
        }
        vscode.window.showInformationMessage("Focal: index cleared. Restart Claude Code to rebuild.");
      } else {
        vscode.window.showInformationMessage("Focal: no index file found.");
      }
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("focal.showMemories", () => {
      vscode.window.showInformationMessage(
        "Focal: use Claude Code's list_memories or search_memory tool."
      );
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("focal.configureMcp", async () => {
      await autoConfigureMcp(binaryPath, workspaceRoots);
    })
  );
}
