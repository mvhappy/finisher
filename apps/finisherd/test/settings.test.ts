// `readSettings`: a present `settings.json` that can't be used keeps the last good settings,
// or before any good read switches auto-fetch off (plan 4C Task 22 review).
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { DEFAULT_SETTINGS, readSettings, type Settings, settingsPath, UNREADABLE_SETTINGS } from "../src/config.ts";

let dir: string;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "finisherd-settings-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const write = (text: string) => writeFileSync(settingsPath(dir), text);
const off: Settings = { fonts: { autoFetch: false } };

describe("readSettings", () => {
  test("a missing file is the defaults; a good file is read", () => {
    expect(readSettings(dir)).toEqual({ settings: DEFAULT_SETTINGS });
    write('{ "fonts": { "autoFetch": false } }');
    expect(readSettings(dir)).toEqual({ settings: off });
    write("{}");
    expect(readSettings(dir, off)).toEqual({ settings: DEFAULT_SETTINGS });
  });

  for (const [what, text] of [
    ["a trailing comma", '{ "fonts": { "autoFetch": false, } }'],
    ["a truncated file", '{ "fonts": { "autoF'],
    ["an empty file", ""],
    ["a wrongly typed value", '{ "fonts": { "autoFetch": "no" } }'],
    ["a non-object fonts", '{ "fonts": true }'],
    ["a non-object file", "[]"],
  ] as const) {
    test(`${what} keeps the last good settings, else auto-fetch is off`, () => {
      write(text);
      const first = readSettings(dir);
      expect(first.settings).toEqual(UNREADABLE_SETTINGS);
      expect(first.settings.fonts.autoFetch).toBe(false);
      expect(first.error).toBeTruthy();
      const kept = readSettings(dir, DEFAULT_SETTINGS);
      expect(kept.settings).toBe(DEFAULT_SETTINGS);
      expect(kept.error).toBeTruthy();
      expect(readSettings(dir, off).settings).toBe(off);
    });
  }
});
