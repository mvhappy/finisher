import { existsSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { FinError } from "@finisher/contracts";
import { codexBinary, codexTarget, defaultPinDeps, type PinDeps } from "../src/agent/codex/pin.ts";

const BIN = "codex";
function deps(over: Partial<PinDeps> & { files?: string[] }): PinDeps {
  const files = new Set(over.files ?? []);
  return {
    env: {},
    testHooks: false,
    devCheckout: false,
    platform: "linux",
    arch: "x64",
    resourcesDir: "/app/resources",
    cacheRoot: "/repo/.cache/codex",
    version: "0.167.0",
    exists: (p) => files.has(p),
    ...over,
  };
}
const RES = join("/app/resources", "codex", "linux-x64", BIN);
const CACHE = join("/repo/.cache/codex", "0.167.0", "linux-x64", BIN);

describe("binary_path_order", () => {
  it("FIN_CODEX_BIN wins only in a test-hooks build", () => {
    const env = { FIN_CODEX_BIN: "/fake/codex" };
    expect(codexBinary(deps({ env, testHooks: true, files: ["/fake/codex", RES, CACHE] }))).toBe("/fake/codex");
    expect(codexBinary(deps({ env, testHooks: false, files: ["/fake/codex", RES, CACHE] }))).toBe(RES);
    expect(() => codexBinary(deps({ env, testHooks: false, files: ["/fake/codex"] }))).toThrow(FinError);
  });
  it("a set FIN_CODEX_BIN that does not exist is not silently skipped in a test-hooks build", () => {
    expect(() => codexBinary(deps({ env: { FIN_CODEX_BIN: "/nope" }, testHooks: true, files: [RES] }))).toThrow(/FIN_CODEX_BIN/);
  });
  it("FIN_CODEX_BIN must be absolute (the spawn's cwd is the workspace, not the server's)", () => {
    expect(() => codexBinary(deps({ env: { FIN_CODEX_BIN: "fake/codex" }, testHooks: true, files: ["fake/codex", RES] }))).toThrow(/absolute/);
  });
  it("resources beside finisherd, then the dev cache (a development checkout or a test-hooks build)", () => {
    expect(codexBinary(deps({ files: [RES, CACHE] }))).toBe(RES);
    expect(codexBinary(deps({ devCheckout: true, files: [CACHE] }))).toBe(CACHE);
    expect(codexBinary(deps({ testHooks: true, files: [CACHE] }))).toBe(CACHE);
  });
  it("Windows names are codex.exe under the win32 target", () => {
    const exe = join("/app/resources", "codex", "win32-arm64", "codex.exe");
    expect(codexBinary(deps({ platform: "win32", arch: "arm64", files: [exe] }))).toBe(exe);
  });
  it("none: PROVIDER_UNAVAILABLE notBundled", () => {
    try {
      codexBinary(deps({}));
      expect.unreachable();
    } catch (e) {
      expect(e).toBeInstanceOf(FinError);
      const f = e as FinError;
      expect(f.code).toBe("PROVIDER_UNAVAILABLE");
      expect(f.details).toMatchObject({ provider: "codex", reason: "notBundled" });
    }
  });
  it("an unsupported platform is notBundled too", () => {
    expect(() => codexBinary(deps({ platform: "freebsd" }))).toThrow(/PROVIDER_UNAVAILABLE|bundled/i);
    expect(codexTarget("darwin", "arm64")).toBe("darwin-arm64");
    expect(codexTarget("freebsd", "x64")).toBeUndefined();
  });
});

describe("binary_path_production_default", () => {
  const EVIL = join("/evil/res", "codex", "linux-x64", "codex");
  it("an installed build reads neither FIN_RESOURCES_DIR nor the dev cache", () => {
    const env = { FIN_RESOURCES_DIR: "/evil/res" };
    expect(codexBinary(deps({ env, files: [EVIL, RES, CACHE] }))).toBe(RES);
    expect(() => codexBinary(deps({ env, files: [EVIL, CACHE] }))).toThrow(/bundled/);
    expect(() => codexBinary(deps({ files: [CACHE] }))).toThrow(/bundled/);
  });
  it("FIN_RESOURCES_DIR stands in for the installation's resources in a checkout or test-hooks build, absolute only", () => {
    const env = { FIN_RESOURCES_DIR: "/evil/res" };
    expect(codexBinary(deps({ env, devCheckout: true, files: [EVIL, RES] }))).toBe(EVIL);
    expect(codexBinary(deps({ env, testHooks: true, files: [EVIL, RES] }))).toBe(EVIL);
    // Never resolved against the working directory.
    expect(() => codexBinary(deps({ env: { FIN_RESOURCES_DIR: "res" }, devCheckout: true, files: [RES, "res/codex/linux-x64/codex"] }))).toThrow(/absolute/);
  });
  it("the defaults: the installation's resources beside this module, whatever the cwd or the environment", () => {
    const saved = process.env.FIN_RESOURCES_DIR;
    process.env.FIN_RESOURCES_DIR = "/evil/res";
    try {
      const d = defaultPinDeps();
      const root = fileURLToPath(new URL("../../../", import.meta.url));
      expect(isAbsolute(d.resourcesDir)).toBe(true);
      expect(d.resourcesDir).toBe(join(root, "resources"));
      expect(d.cacheRoot).toBe(join(root, ".cache", "codex"));
      // This repository is a checkout (its `.git` beside the module tree), not an installation.
      expect(d.devCheckout).toBe(existsSync(join(root, ".git")));
      // The module tree's root is the one holding the pin (pin.ts resolves it a fixed depth up).
      expect(existsSync(join(root, "vendor", "codex", "pin.json"))).toBe(true);
      expect(d.version).toBeTruthy();
    } finally {
      if (saved === undefined) delete process.env.FIN_RESOURCES_DIR;
      else process.env.FIN_RESOURCES_DIR = saved;
    }
  });
});
