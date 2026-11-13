// The "subagents" scenario's finisherd: `serve` from `apps/finisherd/src/main.ts` with one test
// option `main` never passes, the `auto` merge trigger's `observe` (plan 7B Task 12): each
// outcome is appended as a JSON line to the file `FIN_E2E_AUTO_MERGE_LOG` names. Availability and
// the verdict source stay the server's own (`AUTO_MERGE_AVAILABLE`, `review_verdicts`), so what
// the log shows is what a release server decides.
//   FIN_E2E_AUTO_MERGE_LOG=<file> node scripts/e2e/subagents-server.mjs --server-data <dir>
import { appendFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT } from "./lib.mjs";

const at = process.argv.indexOf("--server-data");
const serverData = at < 0 ? undefined : process.argv[at + 1];
const log = process.env.FIN_E2E_AUTO_MERGE_LOG;
if (serverData === undefined || log === undefined) {
  process.stderr.write("usage: FIN_E2E_AUTO_MERGE_LOG=<file> node scripts/e2e/subagents-server.mjs --server-data <dir>\n");
  process.exit(2);
}
const { serve, start } = await import(pathToFileURL(join(ROOT, "apps", "finisherd", "src", "main.ts")).href);
const observe = (ev, outcome) => appendFileSync(log, `${JSON.stringify({ event: ev, outcome })}\n`);
start(serve({ serverData, agents: { autoMerge: { observe } } }));
