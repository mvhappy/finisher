import { describe, expect, it } from "vitest";
import { FinError } from "@finisher/contracts";
import { codexBinary, codexTarget, type PinDeps } from "../src/agent/codex/pin.ts";

const BIN = "codex";
function deps(over: Partial<PinDeps> & { files?: string[] }): PinDeps {
  const files = new Set(over.files ?? []);
  return {
    env: {},
    testHooks: false,
    platform: "linux",
    arch: "x64",
    resourcesDir: "/app/resources",
    cacheRoot: "/repo/.cache/codex",
    version: "0.167.0",
    exists: (p) => files.has(p),
    ...over,
  };
}
const RES = `/app/resources/codex/linux-x64/${BIN}`;
const CACHE = `/repo/.cache/codex/0.167.0/linux-x64/${BIN}`;

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
  it("resources beside finisherd, then the dev cache", () => {
    expect(codexBinary(deps({ files: [RES, CACHE] }))).toBe(RES);
    expect(codexBinary(deps({ files: [CACHE] }))).toBe(CACHE);
  });
  it("Windows names are codex.exe under the win32 target", () => {
    const d = deps({ platform: "win32", arch: "arm64", files: ["/app/resources/codex/win32-arm64/codex.exe"] });
    expect(codexBinary(d)).toBe("/app/resources/codex/win32-arm64/codex.exe");
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
