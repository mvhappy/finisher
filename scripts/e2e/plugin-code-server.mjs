// The "plugin-code" scenario's finisherd: `serve` from `apps/finisherd/src/main.ts` with the
// egress broker's test hooks, which `main` never passes (sp8b Task 8; `egress-test-hooks.ts`
// refuses them unless fin-node is its `test-hooks` build, so `FIN_NODE_BINARY_DIR` must name it).
//   node scripts/e2e/plugin-code-server.mjs --server-data <dir>
// `FIN_E2E_EGRESS` = `{host, port, ca, resolveFile}`: `host` (the plugins' approved origin's name)
// resolves to the addresses listed in `resolveFile` (read on every lookup, so the scenario can
// point it at a private address); `127.0.0.1` for `host` passes the private-address check, and
// connections to `https://<host>` go to the scenario's loopback server on `port` (trusting `ca`).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT } from "./lib.mjs";

const at = process.argv.indexOf("--server-data");
const serverData = at < 0 ? undefined : process.argv[at + 1];
const raw = process.env.FIN_E2E_EGRESS;
if (serverData === undefined || raw === undefined) {
  process.stderr.write("usage: FIN_E2E_EGRESS=<json> node scripts/e2e/plugin-code-server.mjs --server-data <dir>\n");
  process.exit(2);
}
const cfg = JSON.parse(raw);
const src = (p) => pathToFileURL(join(ROOT, "apps", "finisherd", "src", p)).href;
const { serve, start } = await import(src("main.ts"));
const { egressTestHooks } = await import(src("plugins/host/egress-test-hooks.ts"));
const origin = `https://${cfg.host}`;
const egress = egressTestHooks({
  lookup: async (h) => (h === cfg.host ? JSON.parse(readFileSync(cfg.resolveFile, "utf8")) : []),
  allow: [{ host: cfg.host, address: "127.0.0.1" }],
  ca: cfg.ca,
  port: (o) => (o === origin ? cfg.port : undefined),
});
start(serve({ serverData, plugins: { egress } }));
