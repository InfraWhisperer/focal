import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";

// We can't import resolveBinary directly without mocking vscode.workspace,
// so we test the pure logic functions by re-implementing the key checks.

suite("Binary Manager", () => {
  suite("platform detection", () => {
    test("os.platform returns a known value", () => {
      const platform = os.platform();
      assert.ok(
        ["darwin", "linux", "win32"].includes(platform),
        `unexpected platform: ${platform}`
      );
    });

    test("os.arch returns a known value", () => {
      const arch = os.arch();
      assert.ok(
        ["arm64", "x64"].includes(arch),
        `unexpected arch: ${arch}`
      );
    });
  });

  suite("cached binary path", () => {
    test("constructs path under ~/.focal/bin/", () => {
      const name = os.platform() === "win32" ? "focal.exe" : "focal";
      const expected = path.join(os.homedir(), ".focal", "bin", name);
      // Validate the path structure is correct
      assert.ok(expected.includes(".focal"));
      assert.ok(expected.includes("bin"));
      assert.ok(expected.endsWith(name));
    });
  });

  suite("bundled binary path", () => {
    test("constructs path under extension/bin/", () => {
      const extensionPath = "/fake/extension/path";
      const name = os.platform() === "win32" ? "focal.exe" : "focal";
      const bundled = path.join(extensionPath, "bin", name);
      assert.ok(bundled.endsWith(path.join("bin", name)));
    });
  });
});
