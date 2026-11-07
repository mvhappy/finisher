// `mergeSettings` (plan D, Decision 8): only the patched `privacy` keys change, everything else
// is kept, a file that does not parse is never overwritten, and the write is temp-then-rename.
import { chmodSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { settingsPath } from "../src/config.ts";
import { mergeSettings } from "../src/settings-write.ts";

const WIN = process.platform === "win32";
let dir: string;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "finisherd-settings-write-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const read = () => JSON.parse(readFileSync(settingsPath(dir), "utf8")) as Record<string, unknown>;

function thrown(f: () => unknown): { code: string; fix?: string; details: Record<string, unknown> } {
  try {
    f();
  } catch (e) {
    return e as { code: string; fix?: string; details: Record<string, unknown> };
  }
  throw new Error("expected a throw");
}

describe("mergeSettings", () => {
  test("settings_writer_merges_and_preserves_unknown_keys", () => {
    writeFileSync(
      settingsPath(dir),
      JSON.stringify({ fonts: { autoFetch: false }, future: { x: [1, 2] }, privacy: { maxMiB: 64, other: true } }),
    );
    expect(mergeSettings(dir, { training: "off" })).toEqual({ maxMiB: 64, other: true, training: "off" });
    expect(read()).toEqual({
      fonts: { autoFetch: false },
      future: { x: [1, 2] },
      privacy: { maxMiB: 64, other: true, training: "off" },
    });
    mergeSettings(dir, { diagnostics: "on", training: null });
    expect(read().privacy).toEqual({ maxMiB: 64, other: true, diagnostics: "on" });
  });

  test("a missing file is created; removing the last key removes privacy", () => {
    expect(mergeSettings(dir, { diagnostics: "off" })).toEqual({ diagnostics: "off" });
    expect(read()).toEqual({ privacy: { diagnostics: "off" } });
    expect(mergeSettings(dir, { diagnostics: null })).toBeUndefined();
    expect(read()).toEqual({});
  });

  for (const [what, text] of [
    ["invalid JSON", '{ "fonts": { "autoFetch": false, } }'],
    ["a non-object file", "[1]"],
    ["a non-object privacy", '{ "privacy": "on" }'],
  ] as const) {
    test(`settings_writer_refuses_unparsable_file: ${what}`, () => {
      writeFileSync(settingsPath(dir), text);
      const e = thrown(() => mergeSettings(dir, { training: "off" }));
      expect(e.code).toBe("SETTINGS_INVALID");
      expect(e.fix).toContain(settingsPath(dir));
      expect(readFileSync(settingsPath(dir), "utf8")).toBe(text);
    });
  }

  test("a merged section the validator refuses is not written, naming the key", () => {
    writeFileSync(settingsPath(dir), JSON.stringify({ privacy: { diagnostics: "on", retentionDays: 0 } }));
    const before = readFileSync(settingsPath(dir), "utf8");
    const e = thrown(() =>
      mergeSettings(dir, { diagnostics: "off" }, (p) => (p?.retentionDays === 0 ? "privacy.retentionDays is not 1–365" : undefined)),
    );
    expect(e.code).toBe("SETTINGS_INVALID");
    expect(e.fix).toContain("privacy.retentionDays");
    expect(e.details.key).toBe("privacy.retentionDays");
    expect(readFileSync(settingsPath(dir), "utf8")).toBe(before);
  });

  test.skipIf(WIN || process.getuid?.() === 0)("a failed write is SETTINGS_WRITE_FAILED and leaves no temp file", () => {
    writeFileSync(settingsPath(dir), "{}");
    chmodSync(dir, 0o500);
    try {
      const e = thrown(() => mergeSettings(dir, { training: "off" }));
      expect(e.code).toBe("SETTINGS_WRITE_FAILED");
      expect(e.fix).toContain(settingsPath(dir));
    } finally {
      chmodSync(dir, 0o700);
    }
    expect(readdirSync(dir)).toEqual(["settings.json"]);
    expect(readFileSync(settingsPath(dir), "utf8")).toBe("{}");
  });

  test("settings_writer_is_temp_then_rename", () => {
    writeFileSync(settingsPath(dir), "{}");
    const before = statSync(settingsPath(dir)).ino;
    mergeSettings(dir, { training: "off" });
    expect(statSync(settingsPath(dir)).ino).not.toBe(before);
    expect(readdirSync(dir)).toEqual(["settings.json"]);
    if (!WIN) expect(statSync(settingsPath(dir)).mode & 0o777).toBe(0o600);
  });
});
