import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

test("identical invalid examples report each fence's own line", () => {
  const dir = mkdtempSync(join(tmpdir(), "pitchfork-examples-"));
  try {
    const file = join(dir, "duplicates.md");
    for (const language of ["toml", "json"]) {
      const fence = `\`\`\`${language}\ninvalid\n\`\`\`\n`;
      // The first copy is nested inside a longer Markdown example fence.
      // It is literal code, so only the later two fences should be validated.
      writeFileSync(
        file,
        `\`\`\`\`markdown\n${fence}\`\`\`\`\n\n${fence}\n${fence}`,
      );
      const result = spawnSync(
        process.execPath,
        [
          fileURLToPath(new URL("./check-content.mjs", import.meta.url)),
          "--examples",
          file,
        ],
        { encoding: "utf8" },
      );
      assert.equal(result.status, 1);
      assert.match(
        result.stdout,
        /Checked 2 TOML\/JSON examples in 1 Markdown files/,
      );
      assert.deepEqual(
        [...result.stderr.matchAll(/example at line (\d+):/g)].map((match) =>
          Number(match[1]),
        ),
        [7, 11],
        result.stderr,
      );
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("nested fences keep their source line numbers", () => {
  const dir = mkdtempSync(join(tmpdir(), "pitchfork-examples-"));
  try {
    const file = join(dir, "nested.md");
    writeFileSync(
      file,
      [
        "> ```json",
        "> invalid",
        "> ```",
        "",
        "- Example",
        "",
        "  ```json",
        "  invalid",
        "  ```",
        "",
      ].join("\n"),
    );
    const result = spawnSync(
      process.execPath,
      [
        fileURLToPath(new URL("./check-content.mjs", import.meta.url)),
        "--examples",
        file,
      ],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 1);
    assert.deepEqual(
      [...result.stderr.matchAll(/example at line (\d+):/g)].map((match) =>
        Number(match[1]),
      ),
      [1, 7],
      result.stderr,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
