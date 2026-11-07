// Combined supply-chain age gate: runs the crate check (Cargo.lock) and the npm check
// (pnpm-lock.yaml) one after the other and fails if either does. Network access required;
// not part of `pnpm gates`. Flags are passed to both (`--days`, `--delay-ms`, `--root`); the
// registry-specific ones stay on the individual scripts.
import { fileURLToPath } from "node:url";
import { main as crateAge } from "./check-crate-age.mjs";
import { main as npmAge } from "./check-npm-age.mjs";

/** Exit code of running `checks` in order: the worst (highest) one, so an infrastructure error
 * (2) outranks a policy violation (1). Every check runs even if an earlier one failed. */
export async function runAll(checks, argv, env) {
  let worst = 0;
  for (const [label, check] of checks) {
    process.exitCode = 0;
    try {
      await check(argv, env);
    } catch (err) {
      console.error(`${label}: ${err.message}`);
      process.exitCode = 2;
    }
    worst = Math.max(worst, Number(process.exitCode ?? 0));
  }
  return worst;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exitCode = await runAll(
    [
      ["crate-age", crateAge],
      ["npm-age", npmAge],
    ],
    process.argv.slice(2),
    process.env,
  );
}
