#!/usr/bin/env node
// End-to-end check: fin → finisherd → WebSocket subscriber.
//
// Builds `fin` (release) and fin-node, starts finisherd in a temp server-data dir, opens an
// `ops.subscribe` stream from rev 0 over the ticketed WebSocket, runs the editing flow
// through the `fin` binary, and asserts the subscriber saw every committed rev in order and
// that its last rev equals `snapshot.get`'s. The server and the temp dir are always cleaned up.
//
// Usage: node scripts/e2e.mjs [--skip-build] [basic|storage|history|reel|diagnostics|plugins|plugin-code|auth|look|agent|library|sandbox|remote|review|subagents|proxy ...]  (default: every scenario)
//
// Builds go to `CARGO_TARGET_DIR` when it is set (else `target/`), and so does the run's
// `fin` and `fin-plugin`. Scenario temp dirs fall back to `/tmp` when `TMPDIR` is so deep that the
// server's unix socket path would pass 104 bytes.
//
// The "storage" scenario (scripts/e2e/storage.mjs) edits, exports, deletes the database,
// rebuilds from the folder and checks a second finisherd follows the folder read-only.
//
// The "history" scenario (scripts/e2e/history*.mjs) drives branches, merges (conflicting, skipped,
// both directions), undo/redo, checkpoints, try, cherry-pick, revert, dry runs and a real
// `project recover --create` (a second server on a lease-less folder copy diverges, ~12 s of
// lease catch-up), with WebSocket subscribers checking each record lands on its own branch.
//
// The "reel" scenario (scripts/e2e/reel*.mjs) builds a 30 s reel by `fin` only (linked audio,
// speed, overlays, a graphic, music, captions, keyframes, transitions) on a test font catalog
// served from loopback: captions are unmeasured while fetching is off, then the fonts fetch by
// themselves and eval lays them out with the records unchanged; view, frame and audio plans
// equal `scripts/e2e/reel-golden/` (`E2E_REEL_BLESS=1` rewrites them, refused under CI). Its
// real-font pass needs `FIN_FONT_CACHE` (skipped without it, unless `FIN_REQUIRE_FONTS=1`).
//
// The "diagnostics" scenario (scripts/e2e/diagnostics*.mjs) drives the diagnostics journal by
// `fin` with an agent session: off by env, on by default, every commit outcome, the join of
// `log.commit`, `cli.command` and the op log, the leak grep, a `never` project on two servers,
// training by env only, withdrawal and `purge --all` (plan D7).
//
// The "plugins" scenario (scripts/e2e/plugins*.mjs) drives the plugin lifecycle end to end with
// `fin-plugin`-built registries (a plain folder and a git clone the run resets): the console adds
// them by their root's hash, a person installs, `fin` and agents are refused, packs, fonts and
// code plugins, widening updates, advisories, a `TUF_ROLLBACK`, trust kept on removal, the bundled
// pack's owner switch across tampering, the journal's free-text grep and each step's audit row.
//
// The "plugin-code" scenario (scripts/e2e/plugin-code*.mjs) runs code plugins end to end (plan 8B
// Task 18): both SDK examples and the test probe from a `fin-plugin` registry, installed and
// enabled by a person, commands committed with `via` by the invoker, `fetch-title` through the
// egress broker to a loopback origin (its test hooks: finisherd runs `plugin-code-server.mjs`) and
// `EGRESS_PRIVATE_ADDRESS`, an agent session's reach, authority isolation of memory, temp and
// pluginData between the person, an agent session and its child, the crash loop, disable,
// `PLUGINS_UNCONFINED`'s count and the journal's free-text grep.
//
// The "auth" scenario (scripts/e2e/auth*.mjs) drives principals and credentials over `fin.sock` and
// the ticketed WebSocket: an unproved connection, `fin` as `local`, the human-only refusals and the
// console's challenge codes, ticket and Host/Origin checks, agent credentials (scopes, `branch own`,
// journal actors, revocation ending live streams, expiry, `TICKET_LIMIT`), a squatted socket, a
// restart that rotates the keys, the audit rows and a leak grep over every log and the journal.
//
// The "look" scenario (scripts/e2e/look*.mjs) renders a project of two generated AV1 clips (CFR,
// and VFR rotated 90°) with `fin look` through the real media worker: plan hashes against
// `scripts/e2e/look-golden/`, frame ids read back from the PNGs against the nearest-frame rule,
// every PNG against the CPU reference over a copy of the job directory (tolerance T1, T0 hashes
// per fixture set), `fin-workers media --replay`, `skipped`, and a worker killed mid-job and
// retried (`E2E_LOOK_BLESS=1` rewrites the goldens, refused under CI). It needs a GPU adapter
// (`FIN_GPU=software` on lavapipe) and is skipped without one unless `FIN_REQUIRE_GPU=1`.
//
// The "agent" scenario (scripts/e2e/agent*.mjs) runs a built-in agent session against a scripted model
// server on loopback (the local preset, no key, no network): from the prompt to the merged verdict
// with the session branch's commits and refusals, the journal's call join and replayable trace
// deltas, a `finisherd` killed during a tool call and resumed, a budget stop and its approval, and
// the leak grep (plan 7A Task 13).
//
// The "library" scenario (scripts/e2e/library*.mjs) adds a library from the console, waits for its
// scan with `fin wait`, lists, searches and shows its files, renames, moves, rewrites and deletes
// some and scans again; NFC/NFD twins answer `PATH_AMBIGUOUS`, placeholders and a Nextcloud stub
// stay `cloudOnly` and unopened, a nested mount unmounted through the test overlay goes offline
// with nothing missing, and no scan changes a byte or a timestamp under the root (plan 5D Task 15).
//
// The "sandbox" scenario (scripts/e2e/sandbox.mjs) runs a `fake` session whose turn starts a scripted
// child through the session sandbox (Linux: the launcher, its subreaper and Landlock): `fin` lands
// on the session's branch over the agent listener, `auth/local.key` and writes outside the
// workspace are denied, `/etc/hosts` is `PATH_NOT_ALLOWED`, no `before7C` warning; with a GPU, a
// warm look per project under the worker profile (plan 7C Task 14).
//
// The "remote" scenario (scripts/e2e/remote.mjs, device-lib.mjs) pairs a device written apart from
// the server (WebCrypto P-256, its own DPoP proofs, a pinned TLS client) over the remote listener on
// loopback: the descriptor's pin, one-use offers and safety codes, nonces, tokens and tickets, a
// `wss` subscription, a device's `server.stop`, allowed roots, revocation within 100 ms, a copied
// server data (`ENV_MOVED`, `env reset`) and the leak grep (plan 5B Task 13).
//
// The "review" scenario (scripts/e2e/review.mjs) drives the review workflow with an agent credential
// on its own branches and `fin` as the local reviewer: a protected `main`, a review of four edits
// (reject one, accept two, leave one), the partial merge and the rest after a stale snapshot, the
// drop kept out of a plain merge, a `mergeConflict` item superseded by a later merge, the project
// copied to a second server data (the drop holds, the agent branch is re-derived), the policy
// (`AUTO_MERGE_UNAVAILABLE`, `HUMAN_ONLY`) and what `events/` and the audit hold (plan 5E Task 11).
//
// The "subagents" scenario (scripts/e2e/subagents*.mjs) runs built-in sessions of the local preset
// against a scripted model server: a rule `ask` waits on a card (`APPROVAL_PENDING`, exit 8) and a
// `remember: session` answer lets the next identical call pass; an editor spawns two clip finders,
// waits with `fin agent wait --any`, the carves settle on it, it merges one child and a person rolls
// the merge back; a spawn chain stops at depth 3 (`DEPTH_EXCEEDED`); a saved prompt starts a
// session; a run past 75 % of the window compacts; an `auto` session's review request leaves the
// item open, the `auto` merge trigger answering `unavailable` (plan 7B Task 13).
//
// The "proxy" scenario (scripts/e2e/proxy*.mjs) makes proxies through the real media worker (6B-1
// Task 20): a library's `proxy540` stage over generated AV1 clips (CFR, and VFR rotated 90°), the
// `1080` tier by `fin media proxy`, `fin look` reading the proxies with the frame ids of
// `--originals`, `fin media thumbs --strip`, `media.url` and an HTTP range request, `fin cache
// status` and the console's `gc --to 0` around an open and a closed project, and the journal's
// media entries. H.264: proxied from the OS encoder's clip where the report names one (macOS,
// Windows), `CODEC_UNAVAILABLE` with `laterBatch` guidance on Linux. Needs a GPU adapter, as "look".
//
// After the builds, each scenario has a deadline (E2E_DEADLINE_MS, default 180 s): when it passes,
// the server is killed and the script exits non-zero. Test hooks (scripts/e2e.test.mjs):
// E2E_SERVER_CMD (a JSON array, `{serverData}` substituted) replaces the finisherd command,
// and E2E_START_TIMEOUT_MS bounds the wait for its `finisherd.json` (default 30 s).
import { pathToFileURL } from "node:url";
import { assertNoTestHooks, build, live, removeTmp } from "./e2e/lib.mjs";
import { checkRevs, run as runBasic } from "./e2e/basic.mjs";
import { run as runStorage } from "./e2e/storage.mjs";
import { run as runHistory } from "./e2e/history.mjs";
import { run as runReel } from "./e2e/reel.mjs";
import { run as runDiagnostics } from "./e2e/diagnostics.mjs";
import { run as runPlugins } from "./e2e/plugins.mjs";
import { run as runPluginCode } from "./e2e/plugin-code.mjs";
import { run as runAuth } from "./e2e/auth.mjs";
import { run as runLook } from "./e2e/look.mjs";
import { run as runAgent } from "./e2e/agent.mjs";
import { run as runLibrary } from "./e2e/library.mjs";
import { run as runSandbox } from "./e2e/sandbox.mjs";
import { run as runRemote } from "./e2e/remote.mjs";
import { run as runReview } from "./e2e/review.mjs";
import { run as runSubagents } from "./e2e/subagents.mjs";
import { run as runProxy } from "./e2e/proxy.mjs";

// The servers of every scenario keep their secrets in files, never in the OS keychain (5B Decision 4).
process.env.FIN_SECRETS ??= "file";

export { parseFrames } from "./e2e/lib.mjs";
export { checkRevs };

const DEADLINE_MS = Number(process.env.E2E_DEADLINE_MS ?? 180_000);
const SCENARIOS = { basic: runBasic, storage: runStorage, history: runHistory, reel: runReel, diagnostics: runDiagnostics, plugins: runPlugins, "plugin-code": runPluginCode, auth: runAuth, look: runLook, agent: runAgent, library: runLibrary, sandbox: runSandbox, remote: runRemote, review: runReview, subagents: runSubagents, proxy: runProxy };

async function main(argv) {
  const names = argv.filter((a) => !a.startsWith("--"));
  if (!argv.includes("--skip-build")) {
    build("cargo", ["build", "--release", "-p", "fin"]);
    build("cargo", ["build", "--release", "-p", "fin-plugin-cli"]); // publisher tooling: the plugins scenario builds registries with it
    if (names.length === 0 || names.includes("plugin-code")) build("cargo", ["build", "--release", "-p", "example-rust-tools", "--bin", "rust-tools"]); // the Rust example plugin
    build("pnpm", ["--filter", "@finisher/fin-node", "build"]);
    build("pnpm", ["--filter", "@finisher/fin-node", "build:test"]);
    if (names.length === 0 || names.includes("look") || names.includes("sandbox") || names.includes("proxy")) {
      // The look scenario's fixture writer and CPU oracle (it runs them with `cargo run`, which finds them built); the sandbox and proxy scenarios' AV1 clips.
      build("cargo", ["build", "--release", "-p", "fin-codecs", "--features", "testing", "--example", "make_fixture"]);
      build("cargo", ["build", "--release", "-p", "fin-workers", "--example", "reference_render"]);
    }
  }
  // A fake server (E2E_SERVER_CMD) never reaches `fin`, so the release binary is not needed (or checked).
  if (!process.env.E2E_SERVER_CMD) assertNoTestHooks();
  // The builds run synchronously (a cold release build can take minutes in CI), so the
  // deadline covers the run itself.
  const unknown = names.filter((n) => !(n in SCENARIOS));
  if (unknown.length > 0) throw new Error(`unknown scenario ${unknown.join(", ")}; known: ${Object.keys(SCENARIOS).join(", ")}`);
  // The deadline is per scenario: the sum of the scenarios outgrew one budget (review made it 12).
  for (const name of names.length > 0 ? names : Object.keys(SCENARIOS)) {
    process.stdout.write(`e2e: scenario ${name}\n`);
    await withDeadline(SCENARIOS[name](), DEADLINE_MS);
  }
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
