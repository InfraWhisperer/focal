import * as vscode from "vscode";

/**
 * Manages the status bar item for Focal.
 * Shows indexing progress, symbol/memory counts, and errors.
 * Clicking opens the command palette filtered to Focal commands.
 */
export class StatusBarManager {
  private item: vscode.StatusBarItem;

  constructor() {
    this.item = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      50
    );
    this.item.command = "workbench.action.quickOpen";
    // Pre-fill the command palette filter — VS Code interprets the argument
    // passed via the command URI, but for status bar items we wire up a
    // wrapper command in commands.ts that opens the palette with ">Focal".
    this.item.command = {
      title: "Focal Commands",
      command: "workbench.action.quickOpen",
      arguments: [">Focal"],
    };
    this.item.show();
  }

  setIdle(msg: string): void {
    this.item.text = `$(database) Focal: ${msg}`;
    this.item.tooltip = `Focal: ${msg}`;
  }

  setIndexing(current: number, total: number): void {
    this.item.text = `$(sync~spin) Indexing ${current}/${total}`;
    this.item.tooltip = `Focal: indexing file ${current} of ${total}`;
  }

  setReady(): void {
    this.item.text = "$(database) Focal";
    this.item.tooltip = "Focal: ready";
  }

  setStats(symbols: number, memories: number): void {
    this.item.text = `$(database) Focal: ${symbols} sym | ${memories} mem`;
    this.item.tooltip = `Focal: ${symbols} symbols, ${memories} memories`;
  }

  setError(msg: string): void {
    this.item.text = `$(error) Focal: ${msg}`;
    this.item.tooltip = `Focal error: ${msg}`;
  }

  dispose(): void {
    this.item.dispose();
  }
}
