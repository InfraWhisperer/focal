import * as vscode from "vscode";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { StatusBarManager } from "./statusbar";
import { registerCommands } from "./commands";
import { autoConfigureMcp } from "./mcp-config";

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const statusBar = new StatusBarManager();
  context.subscriptions.push({ dispose: () => statusBar.dispose() });

  const folders = vscode.workspace.workspaceFolders;
  if (!folders || folders.length === 0) {
    statusBar.setIdle("no workspace");
    return;
  }
  const workspaceRoots = folders.map((f) => f.uri.fsPath);
  const binaryPath = resolveBinaryPath(context.extensionPath);

  // Register commands (reindex triggers via MCP, clearIndex deletes DB)
  registerCommands(context, binaryPath, workspaceRoots, statusBar);

  // Auto-configure MCP for Claude Code
  autoConfigureMcp(binaryPath, workspaceRoots).catch((err) => {
    const msg = err instanceof Error ? err.message : String(err);
    vscode.window.showWarningMessage(
      `Focal: failed to configure MCP — ${msg}`
    );
  });

  statusBar.setReady();
}

export function deactivate(): void {}

/**
 * Resolve the focal binary path using platform-aware detection:
 * 1. User-configured path (focal.coreBinaryPath)
 * 2. Platform-specific bundled binary (extension/bin/focal or focal.exe)
 * 3. Fall back to PATH lookup
 */
function resolveBinaryPath(extensionPath: string): string {
  const configured = vscode.workspace
    .getConfiguration("focal")
    .get<string>("coreBinaryPath", "");
  if (configured && fs.existsSync(configured)) {
    return configured;
  }

  const binaryName = os.platform() === "win32" ? "focal.exe" : "focal";
  const bundled = path.join(extensionPath, "bin", binaryName);
  if (fs.existsSync(bundled)) {
    return bundled;
  }

  return "focal";
}
