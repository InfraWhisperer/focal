import * as assert from "assert";
import * as vscode from "vscode";

suite("Extension", () => {
  test("extension is present", () => {
    const ext = vscode.extensions.getExtension("rpotluri.focal-code");
    assert.ok(ext, "focal extension should be registered");
  });

  test("extension activates on startup", async () => {
    const ext = vscode.extensions.getExtension("rpotluri.focal-code");
    if (ext && !ext.isActive) {
      await ext.activate();
    }
    // If we get here without throwing, activation succeeded
    // (or was already active from onStartupFinished).
  });

  test("commands are declared in package.json", async () => {
    const ext = vscode.extensions.getExtension("rpotluri.focal-code");
    assert.ok(ext, "extension should exist");
    const commands: Array<{ command: string }> =
      ext!.packageJSON.contributes?.commands ?? [];
    const commandIds = commands.map((c) => c.command);
    // These commands are declared in package.json contributes.commands.
    // They're only registered at runtime when a workspace with a valid
    // binary is open, so we verify declaration rather than runtime registration.
    assert.ok(commandIds.includes("focal.reindex"), "focal.reindex declared");
    assert.ok(commandIds.includes("focal.clearIndex"), "focal.clearIndex declared");
    assert.ok(commandIds.includes("focal.showMemories"), "focal.showMemories declared");
    assert.ok(commandIds.includes("focal.configureMcp"), "focal.configureMcp declared");
  });
});
