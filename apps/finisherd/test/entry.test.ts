// `isEntry`: the daemon starts when run through a symlinked bin (review #33).
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { isEntry, withoutVerbatim } from "../src/entry.ts";

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
    if (process.platform === "win32") {
      // A verbatim path (what Rust's `canonicalize` gives), even with `/` in it, is the entry.
      expect(isEntry(url, `\\\\?\\${main}`)).toBe(true);
      expect(isEntry(url, `\\\\?\\${tmp}/main.ts`)).toBe(true);
      expect(isEntry(url, `\\\\?\\${tmp}/other.ts`)).toBe(false);
    }
  });

  test("withoutVerbatim drops the \\\\?\\ prefix on Windows only", () => {
    expect(withoutVerbatim(String.raw`\\?\C:\a\main.ts`, "win32")).toBe(String.raw`C:\a\main.ts`);
    expect(withoutVerbatim(String.raw`\\?\UNC\host\share\main.ts`, "win32")).toBe(String.raw`\\host\share\main.ts`);
    expect(withoutVerbatim(String.raw`C:\a\main.ts`, "win32")).toBe(String.raw`C:\a\main.ts`);
    expect(withoutVerbatim(String.raw`\\?\C:\a`, "linux")).toBe(String.raw`\\?\C:\a`);
  });
});
