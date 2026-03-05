import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { generateClaudeMd } from "../mcp-config";

suite("MCP Config", () => {
  let tmpDir: string;

  setup(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "focal-test-"));
  });

  teardown(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  suite("generateClaudeMd", () => {
    test("creates .claude/CLAUDE.md in workspace root", () => {
      generateClaudeMd(tmpDir);
      const claudeMd = path.join(tmpDir, ".claude", "CLAUDE.md");
      assert.ok(fs.existsSync(claudeMd), "CLAUDE.md should be created");

      const content = fs.readFileSync(claudeMd, "utf-8");
      assert.ok(content.includes("Focal Integration"), "should contain header");
      assert.ok(content.includes("get_context"), "should reference get_context tool");
      assert.ok(content.includes("recover_session"), "should reference recover_session");
    });

    test("does not overwrite existing CLAUDE.md", () => {
      const claudeDir = path.join(tmpDir, ".claude");
      fs.mkdirSync(claudeDir, { recursive: true });
      const claudeMd = path.join(claudeDir, "CLAUDE.md");
      fs.writeFileSync(claudeMd, "custom content", "utf-8");

      generateClaudeMd(tmpDir);

      const content = fs.readFileSync(claudeMd, "utf-8");
      assert.strictEqual(content, "custom content", "should not overwrite");
    });

    test("creates .claude directory if missing", () => {
      const claudeDir = path.join(tmpDir, ".claude");
      assert.ok(!fs.existsSync(claudeDir), "dir should not exist yet");

      generateClaudeMd(tmpDir);

      assert.ok(fs.existsSync(claudeDir), "dir should be created");
    });
  });
});
