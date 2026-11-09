// vitest globalSetup: builds the release napi binary with the `test-hooks` feature into
// packages/fin-node/test/native/; vitest.config.ts points FIN_NODE_BINARY_DIR there.
//
// Tests that run `fin` take it from the checkout's `target/` and skip without one, so a `fin`
// built from an older commit made them fail with answers that are not this tree's. Each
// profile's `fin` that is already built is brought up to date (a no-op when it is; so a
// single-file `vitest run` may compile); none is built anew. Not under `CI`: CI builds `fin`
// itself just before (with the workspace's features, which `-p fin` alone would rebuild), and
// e2e uses that binary after. Bounded at 10 minutes (cargo waits on a busy target lock).
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../../../", import.meta.url));

export default function setup(): void {
  execFileSync("pnpm", ["--filter", "@finisher/fin-node", "build:test"], {
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  const exe = process.platform === "win32" ? "fin.exe" : "fin";
  if (process.env.CI !== undefined && process.env.CI !== "" && process.env.CI !== "false") return;
  for (const [profile, flags] of [["debug", []], ["release", ["--release"]]] as const) {
    // The tests look in the checkout's `target/`, whatever `CARGO_TARGET_DIR` says.
    if (existsSync(join(ROOT, "target", profile, exe))) {
      execFileSync("cargo", ["build", "-p", "fin", ...flags], { cwd: ROOT, stdio: "inherit", timeout: 600_000, env: { ...process.env, CARGO_TARGET_DIR: join(ROOT, "target") } });
    }
  }
}
