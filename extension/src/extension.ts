import * as vscode from "vscode";
import { StatusBarManager } from "./statusbar";
import { registerCommands } from "./commands";
import { autoConfigureMcp } from "./mcp-config";
import { resolveBinary } from "./binary-manager";

const RELEASES_URL = "https://github.com/InfraWhisperer/focal/releases/latest";

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const statusBar = new StatusBarManager();
  context.subscriptions.push({ dispose: () => statusBar.dispose() });

  const folders = vscode.workspace.workspaceFolders;
  if (!folders || folders.length === 0) {
    statusBar.setIdle("no workspace");
    return;
  }
  const workspaceRoots = folders.map((f) => f.uri.fsPath);

  // Resolve binary with auto-download fallback
  statusBar.setDownloading();
  const binaryPath = await resolveBinary(context.extensionPath);

  // Validate the resolved binary exists before writing MCP config
  if (!binaryPath) {
    statusBar.setError("binary not found");
    const choice = await vscode.window.showErrorMessage(
      "Focal: could not find or download the focal binary.",
      "Download from GitHub",
      "Set Path"
    );
    if (choice === "Download from GitHub") {
      vscode.env.openExternal(vscode.Uri.parse(RELEASES_URL));
    } else if (choice === "Set Path") {
      vscode.commands.executeCommand(
        "workbench.action.openSettings",
        "focal.coreBinaryPath"
      );
    }
    return;
  }

  // Register commands (reindex triggers via MCP, clearIndex deletes DB)
  registerCommands(context, binaryPath, workspaceRoots, statusBar);

  // Auto-configure MCP for Claude Code
  autoConfigureMcp(binaryPath, workspaceRoots).catch((err) => {
    const msg = err instanceof Error ? err.message : String(err);
    vscode.window.showWarningMessage(
      `Focal: failed to configure MCP — ${msg}`
    );
  });

  statusBar.setConnected();
}

export function deactivate(): void {}
