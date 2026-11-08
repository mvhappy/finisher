// The launcher is found beside the bundled binaries and refuses a request on argv (plan 7C Task 5).
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { expect, test } from "vitest";
import { resolveSandboxExecPath } from "../src/sandbox/exec.ts";

test("resolve_sandbox_exec_path_finds_a_launcher_that_runs", () => {
  const p = resolveSandboxExecPath();
  expect(existsSync(p)).toBe(true);
  const r = spawnSync(p, ["--policy", "{}", "--", "true"], { encoding: "utf8", env: {} });
  expect(r.status).toBe(64);
  expect(JSON.parse(r.stderr.trim().split("\n").at(-1)!)).toMatchObject({ code: "USAGE" });
});
