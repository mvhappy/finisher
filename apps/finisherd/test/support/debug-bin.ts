// A workspace binary of the debug profile (`fake-claude`, `fin`): under CARGO_TARGET_DIR when it
// is set (`cargoProfileDirs`), else the checkout's `target/`; `.exe` on Windows. The first that
// exists, else the checkout's path (so a missing build is reported by the caller's own check).
import { existsSync } from "node:fs";
import { basename, join } from "node:path";
import { cargoProfileDirs } from "../../src/cargo-target.ts";

export function debugBin(name: string): string {
  const file = process.platform === "win32" ? `${name}.exe` : name;
  const dirs = cargoProfileDirs().filter((d) => basename(d) === "debug");
  return dirs.map((d) => join(d, file)).find((p) => existsSync(p)) ?? join(dirs.at(-1)!, file);
}
