// Envelope gate: the Effect schemas that validate RPC envelopes at runtime (packages/contracts)
// must describe exactly the Rust types they carry, as generated into the core-types schema.
// `packages/contracts/src/envelopes.test.ts` does the field-by-field comparison; this runs it
// under vitest without the repository's global setup, so `pnpm gates` fails (ENVELOPE_DRIFT)
// the moment a schema and its Rust type part. Uses the repository's own vitest.
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { report, rootArg } from "./walk.mjs";

const root = rootArg();
const vitest = join(dirname(createRequire(import.meta.url).resolve("vitest/package.json")), "vitest.mjs");
const config = join(dirname(fileURLToPath(import.meta.url)), "envelopes.vitest.config.mjs");

const run = spawnSync(process.execPath, [vitest, "run", "--root", root, "--config", config, "--reporter=dot"], { cwd: root, encoding: "utf8" });
report(run.status === 0 ? [] : [`ENVELOPE_DRIFT: a contracts envelope schema differs from its Rust type\n${(run.stdout + run.stderr).trim()}`]);
