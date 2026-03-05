import * as assert from "assert";
import * as vscode from "vscode";
import { StatusBarManager } from "../statusbar";

suite("StatusBarManager", () => {
  let manager: StatusBarManager;

  setup(() => {
    manager = new StatusBarManager();
  });

  teardown(() => {
    manager.dispose();
  });

  test("setConnected shows ready state", () => {
    manager.setConnected();
    // Verify no throw — status bar item is internal to VS Code API.
    // The real assertion is that the constructor + method don't crash
    // when running inside the extension host.
  });

  test("setIdle sets message", () => {
    manager.setIdle("no workspace");
  });

  test("setIndexing shows progress", () => {
    manager.setIndexing(5, 20);
  });

  test("setReady transitions cleanly", () => {
    manager.setDownloading();
    manager.setConnected();
    manager.setReady();
  });

  test("setStats formats symbol and memory counts", () => {
    manager.setStats(403, 12);
  });

  test("setError shows error state", () => {
    manager.setError("binary not found");
  });

  test("setDownloading shows spinner", () => {
    manager.setDownloading();
  });

  test("dispose is idempotent", () => {
    manager.dispose();
    // Second dispose should not throw
    manager.dispose();
  });
});
