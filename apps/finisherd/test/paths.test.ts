// `INVALID_PATH` (plan 5A Task 10, Decision 35).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { isAbsoluteNormalised, requireAbsolute } from "../src/paths.ts";
import { NdjsonClient, type RunningServer, startServer } from "./support/ndjson-client.ts";

describe("isAbsoluteNormalised", () => {
  test("POSIX", () => {
    for (const ok of ["/", "/a", "/a/b c/д", "/a/b/", "/a/.hidden", "/a/..b"]) expect(isAbsoluteNormalised(ok, "linux"), ok).toBe(true);
    for (const bad of ["", "x", "./x", "a/../b", "/a/../b", "/a/./b", "/a//b", "/..", "~/x", "C:\\x", "/a\0b"]) {
      expect(isAbsoluteNormalised(bad, "darwin"), bad).toBe(false);
    }
  });

  test("Windows", () => {
    for (const ok of [
      "C:\\",
      "C:\\a\\b",
      "c:/a/b",
      "C:\\a/b\\",
      "\\\\nas\\share",
      "\\\\nas\\share\\a",
      "\\\\?\\C:\\a",
      "\\\\?\\UNC\\nas\\share\\a",
      "C:\\a\\b.c.txt",
      "C:\\a\\console",
      "C:\\a\\com10",
      "C:\\a\\.hidden",
    ]) {
      expect(isAbsoluteNormalised(ok, "win32"), ok).toBe(true);
    }
    for (const bad of [
      "C:foo",
      "C:",
      "\\foo",
      "/foo",
      "foo\\bar",
      "C:\\a\\..\\b",
      "C:\\a\\.\\b",
      "C:\\a\\\\b",
      "\\\\nas",
      // Device namespaces.
      "\\\\.\\PhysicalDrive0",
      "\\\\.\\pipe\\finisher-x",
      "\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1",
      "\\\\?\\Volume{0}\\a",
      "//./pipe/x",
      // Reserved device names, with or without an extension.
      "C:\\a\\CON",
      "C:\\a\\nul.txt",
      "C:\\a\\Com1",
      "C:\\a\\LPT9.log",
      "C:\\aux\\b",
      "C:\\a\\PRN .txt",
      "\\\\nas\\NUL",
      // Streams and other colons after the drive.
      "C:\\a\\b.txt:evil",
      "C:\\a\\b::$DATA",
      "C:\\a:b\\c",
      // Trailing dot or space.
      "C:\\a\\b.",
      "C:\\a\\b ",
      "C:\\a. \\b",
    ]) {
      expect(isAbsoluteNormalised(bad, "win32"), bad).toBe(false);
    }
  });

  test("requireAbsolute names the field", async () => {
    expect(await Effect.runPromise(requireAbsolute("dir", "/a", "linux"))).toBe("/a");
    const e = await Effect.runPromise(Effect.flip(requireAbsolute("dir", "x", "linux")));
    expect(e).toMatchObject({ code: "INVALID_PATH", details: { field: "dir", path: "x" } });
    expect(e.fix).not.toMatch(/`|\bfin \w/);
  });
});

describe("server", () => {
  let tmp: string;
  let server: RunningServer;
  let c: NdjsonClient;

  beforeEach(async () => {
    tmp = mkdtempSync(join(tmpdir(), "paths-"));
    server = await startServer(join(tmp, "sd"));
    c = await NdjsonClient.connect(server.info.socket);
  });

  afterEach(async () => {
    await c.close();
    await server.stop();
    rmSync(tmp, { recursive: true, force: true });
  });

  const invalid = async (tag: string, payload: Record<string, unknown>, field: string) => {
    const exit = await c.call(tag, payload);
    expect(exit.ok, tag).toBe(false);
    if (!exit.ok) expect(exit.error, tag).toMatchObject({ code: "INVALID_PATH", details: { field } });
  };

  test("relative_paths_are_invalid_path", async () => {
    await invalid("project.create", { dir: "x", name: "Demo", rate: "30" }, "dir");
    await invalid("project.create", { dir: `${tmp}/a/../b`, name: "Demo", rate: "30" }, "dir");
    await invalid("project.open", { dir: "./x" }, "dir");
    await invalid("asset.probe", { path: "a/../b" }, "path");
    await invalid("asset.probe", { path: `${tmp}/./clip.mov` }, "path");
    const created = await c.call("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" });
    expect(created.ok).toBe(true);
    const projectId = (created as { value: { projectId: string } }).value.projectId;
    await invalid("project.recover", { projectId, importDir: "rel" }, "importDir");
    // Absolute, normalised paths still reach the handler.
    const opened = await c.call("project.open", { dir: join(tmp, "p") });
    expect(opened.ok).toBe(true);
    const missing = await c.call("asset.probe", { path: join(tmp, "missing.mov") });
    expect(!missing.ok && missing.error.code).not.toBe("INVALID_PATH");
  });
});
