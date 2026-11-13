// Built-binary lookups honour CARGO_TARGET_DIR before the checkout's `target/`.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { cargoProfileDirs } from "../src/cargo-target.ts";
import { type AgentSettings, resolveFinPath } from "../src/agent/settings.ts";
import { resolveSandboxExecPath } from "../src/sandbox/exec.ts";

const NONE = {} as AgentSettings;
let repo: string;
beforeEach(() => {
  repo = mkdtempSync(join(tmpdir(), "finisherd-target-"));
});
afterEach(() => rmSync(repo, { recursive: true, force: true }));

const exe = (name: string) => (process.platform === "win32" ? `${name}.exe` : name);
const touch = (dir: string, name: string) => {
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, exe(name)), "");
};

describe("cargoProfileDirs", () => {
  test("unset or empty: the checkout's target only", () => {
    const want = [join(repo, "target", "release"), join(repo, "target", "debug")];
    expect(cargoProfileDirs(repo, {})).toEqual(want);
    expect(cargoProfileDirs(repo, { CARGO_TARGET_DIR: "" })).toEqual(want);
  });

  test("an absolute CARGO_TARGET_DIR comes first, then the checkout's target", () => {
    const abs = join(repo, "elsewhere");
    expect(cargoProfileDirs(repo, { CARGO_TARGET_DIR: abs })).toEqual([
      join(abs, "release"),
      join(abs, "debug"),
      join(repo, "target", "release"),
      join(repo, "target", "debug"),
    ]);
  });

  test("a relative CARGO_TARGET_DIR is taken against the repository; target itself is not repeated", () => {
    expect(cargoProfileDirs(repo, { CARGO_TARGET_DIR: "out" })[0]).toBe(join(repo, "out", "release"));
    expect(cargoProfileDirs(repo, { CARGO_TARGET_DIR: "target" })).toHaveLength(2);
  });
});

describe("binary lookups", () => {
  test("fin: CARGO_TARGET_DIR's build beats a stale in-tree one", () => {
    const custom = join(repo, "custom");
    touch(join(repo, "target", "release"), "fin");
    touch(join(custom, "debug"), "fin");
    expect(resolveFinPath(NONE, { CARGO_TARGET_DIR: custom }, repo)).toBe(join(custom, "debug", exe("fin")));
    expect(resolveFinPath(NONE, {}, repo)).toBe(join(repo, "target", "release", exe("fin")));
  });

  test("fin: falls back to target/ when CARGO_TARGET_DIR has none; explicit finPath wins", () => {
    touch(join(repo, "target", "debug"), "fin");
    expect(resolveFinPath(NONE, { CARGO_TARGET_DIR: join(repo, "none") }, repo)).toBe(join(repo, "target", "debug", exe("fin")));
    expect(resolveFinPath({ ...NONE, finPath: "/x/fin" }, { CARGO_TARGET_DIR: join(repo, "none") }, repo)).toBe("/x/fin");
  });

  test("fin-sandbox-exec: found under CARGO_TARGET_DIR", () => {
    const custom = join(repo, "custom");
    touch(join(custom, "release"), "fin-sandbox-exec");
    const got = resolveSandboxExecPath({ CARGO_TARGET_DIR: custom }, repo);
    expect(got).toBe(join(custom, "release", exe("fin-sandbox-exec")));
  });
});
