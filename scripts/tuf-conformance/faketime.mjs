// `faketime "<time>" <command …>` for the tuf-conformance suite where libfaketime is missing:
// runs the command with `FIN_TUF_FAKE_NOW_MS` set to <time>, which the conformance client
// (crates/fin-registry/examples/tuf_conformance_client.rs) reads as its clock. The suite passes
// Python's `str(datetime)`: `2026-11-03 12:00:00` or `… +00:00`; no zone means UTC.
import { spawnSync } from "node:child_process";

const [time, cmd, ...args] = process.argv.slice(2);
if (time === undefined || cmd === undefined) {
  process.stderr.write("usage: faketime <time> <command …>\n");
  process.exit(2);
}
const iso = time.trim().replace(" ", "T");
const ms = Date.parse(/(?:Z|[+-]\d\d:?\d\d)$/.test(iso) ? iso : `${iso}Z`);
if (Number.isNaN(ms)) {
  process.stderr.write(`faketime: cannot read the time ${JSON.stringify(time)}\n`);
  process.exit(2);
}
const r = spawnSync(cmd, args, { stdio: "inherit", env: { ...process.env, FIN_TUF_FAKE_NOW_MS: String(ms) } });
process.exit(r.status ?? 1);
