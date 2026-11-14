// `isEntry`: the daemon starts when run through a symlinked bin (review #33).
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { isEntry } from "../src/entry.ts";

let tmp: string;
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-entry-"));
});
afterEach(() => rmSync(tmp, { recursive: true, force: true }));

describe("isEntry", () => {
  test("the module itself, a symlink to it and a symlinked folder are the entry; another file is not", () => {
    const main = join(tmp, "main.ts");
    writeFileSync(main, "");
    writeFileSync(join(tmp, "other.ts"), "");
    const url = pathToFileURL(main).href;
    expect(isEntry(url, main)).toBe(true);
    if (process.platform !== "win32") {
      symlinkSync(main, join(tmp, "finisherd"));
      symlinkSync(tmp, join(tmp, "dir-link"));
      expect(isEntry(url, join(tmp, "finisherd"))).toBe(true);
      expect(isEntry(url, join(tmp, "dir-link", "main.ts"))).toBe(true);
    }
    expect(isEntry(url, join(tmp, "other.ts"))).toBe(false);
    expect(isEntry(url, join(tmp, "missing.ts"))).toBe(false);
    expect(isEntry(url, undefined)).toBe(false);
  });
});
