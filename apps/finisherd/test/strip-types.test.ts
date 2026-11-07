// finisherd runs as `node src/main.ts`: Node strips the types itself, with no build step, and
// refuses syntax that is not erasable (ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX). vitest transforms
// TypeScript on its own, so this test loads every source file in a plain `node` child.
// `control/checkpoint-worker.ts` runs on import (it is a worker's entry point); the control
// store tests load it in a real worker thread, and `scripts/check-strip-types.mjs` covers it.
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { expect, test } from "vitest";

const SRC = fileURLToPath(new URL("../src/", import.meta.url));
const ENTRY_POINTS = new Set(["control/checkpoint-worker.ts"]);

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return sources(path);
    return e.name.endsWith(".ts") && !e.name.endsWith(".d.ts") ? [path] : [];
  });
}

test("finisherd_sources_load_under_node_type_stripping", () => {
  const files = sources(SRC).filter((f) => !ENTRY_POINTS.has(relative(SRC, f).split("\\").join("/")));
  expect(files.length).toBeGreaterThan(20);
  const script = `${files.map((f) => `await import(${JSON.stringify(pathToFileURL(f).href)});`).join("\n")}\nconsole.log("loaded");`;
  const r = spawnSync(process.execPath, ["--input-type=module", "--eval", script], { encoding: "utf8", timeout: 60_000 });
  expect(r.stderr).not.toMatch(/ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX/);
  expect(r.status, r.stderr).toBe(0);
  expect(r.stdout.trim()).toBe("loaded");
});
