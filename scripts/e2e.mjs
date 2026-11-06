#!/usr/bin/env node
// End-to-end check: fin → finisherd → WebSocket subscriber.
//
// Builds `fin` (release) and fin-node, starts finisherd in a temp server-data dir, opens an
// `ops.subscribe` stream from rev 0 over the bearer-token WebSocket, runs the editing flow
// through the `fin` binary, and asserts the subscriber saw every committed rev in order and
// that its last rev equals `snapshot.get`'s. The server and the temp dir are always cleaned up.
//
// Usage: node scripts/e2e.mjs [--skip-build] [basic|storage|history ...]  (default: every scenario)
//
// The "storage" scenario (scripts/e2e/storage.mjs) edits, exports, deletes the database,
// rebuilds from the folder and checks a second finisherd follows the folder read-only.
//
// The "history" scenario (scripts/e2e/history*.mjs) drives branches, merges (conflicting, skipped,
// both directions), undo/redo, checkpoints, try, cherry-pick, revert, dry runs and a real
// `project recover --create` (a second server on a lease-less folder copy diverges, ~12 s of
// lease catch-up), with WebSocket subscribers checking each record lands on its own branch.
//
// After the builds, the run has a deadline (E2E_DEADLINE_MS, default 180 s): when it passes,
// the server is killed and the script exits non-zero. Test hooks (scripts/e2e.test.mjs):
// E2E_SERVER_CMD (a JSON array, `{serverData}` substituted) replaces the finisherd command,
// and E2E_START_TIMEOUT_MS bounds the wait for its `finisherd.json` (default 30 s).
import { pathToFileURL } from "node:url";
import { build, live, removeTmp } from "./e2e/lib.mjs";
import { checkRevs, run as runBasic } from "./e2e/basic.mjs";
import { run as runStorage } from "./e2e/storage.mjs";
import { run as runHistory } from "./e2e/history.mjs";

export { parseFrames } from "./e2e/lib.mjs";
export { checkRevs };

const DEADLINE_MS = Number(process.env.E2E_DEADLINE_MS ?? 180_000);
const SCENARIOS = { basic: runBasic, storage: runStorage, history: runHistory };

async function main(argv) {
  if (!argv.includes("--skip-build")) {
    build("cargo", ["build", "--release", "-p", "fin"]);
    build("pnpm", ["--filter", "@finisher/fin-node", "build"]);
    build("pnpm", ["--filter", "@finisher/fin-node", "build:test"]);
  }
  // The builds run synchronously (a cold release build can take minutes in CI), so the
  // deadline covers the run itself.
  const names = argv.filter((a) => !a.startsWith("--"));
  const unknown = names.filter((n) => !(n in SCENARIOS));
  if (unknown.length > 0) throw new Error(`unknown scenario ${unknown.join(", ")}; known: ${Object.keys(SCENARIOS).join(", ")}`);
  await withDeadline(
    (async () => {
      for (const name of names.length > 0 ? names : Object.keys(SCENARIOS)) {
        process.stdout.write(`e2e: scenario ${name}\n`);
        await SCENARIOS[name]();
      }
    })(),
    DEADLINE_MS,
  );
}

/** Races `work` against `ms`; on expiry the server is killed and the temp dir removed. */
async function withDeadline(work, ms) {
  let timer;
  const expired = new Promise((_, reject) => {
    timer = setTimeout(() => {
      for (const child of live.children) child.kill("SIGKILL");
      if (live.tmp) removeTmp(live.tmp);
      reject(new Error(`watchdog: the run did not finish within ${ms / 1000}s; killed finisherd`));
    }, ms);
  });
  try {
    await Promise.race([work, expired]);
  } finally {
    clearTimeout(timer);
  }
}


if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main(process.argv.slice(2)).catch((e) => {
    process.stderr.write(`e2e: FAIL: ${e instanceof Error ? e.message : String(e)}\n`);
    process.exitCode = 1;
    // After a watchdog expiry the run may still be pending: do not wait on it. Unref'd, so a
    // clean failure exits as soon as the output is flushed.
    setTimeout(() => process.exit(1), 1000).unref();
  });
}
