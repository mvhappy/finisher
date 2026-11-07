// The control store's checkpoint thread (plan 5A Decision 1). The main connection never
// checkpoints (`wal_autocheckpoint = 0`), so no checkpoint runs on the event loop; this
// worker's own connection does instead:
// - `PASSIVE` every `passiveMs` while there were writes, or at once when the WAL passes
//   `walLimitBytes`;
// - `TRUNCATE` (which shrinks the WAL file) after `idleTruncateMs` without writes.
// Writes are seen through `PRAGMA data_version`, which changes when another connection
// commits. Only Node built-ins are imported: the worker loads this file on its own.
import { statSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { parentPort, workerData } from "node:worker_threads";

interface Data {
  readonly path: string;
  readonly busyMs: number;
  readonly passiveMs: number;
  readonly walLimitBytes: number;
  readonly idleTruncateMs: number;
  readonly pollMs: number;
}

const data = workerData as Data;
const db = new DatabaseSync(data.path);
db.exec(`PRAGMA busy_timeout = ${data.busyMs}; PRAGMA wal_autocheckpoint = 0;`);
const version = db.prepare("SELECT data_version AS v FROM pragma_data_version");

function walSize(): number {
  try {
    return statSync(`${data.path}-wal`).size;
  } catch {
    return 0;
  }
}

let lastVersion = (version.get() as { v: number }).v;
let lastWriteAt = Date.now();
let lastPassiveAt = Date.now();
let writesSincePassive = false;
let writesSinceTruncate = false;

function checkpoint(mode: "PASSIVE" | "TRUNCATE"): void {
  try {
    db.exec(`PRAGMA wal_checkpoint(${mode})`);
  } catch (e) {
    // Busy past the timeout: try again on a later poll.
    process.stderr.write(`finisherd: control.db ${mode} checkpoint skipped: ${String(e)}\n`);
  }
}

function poll(): void {
  const now = Date.now();
  const v = (version.get() as { v: number }).v;
  if (v !== lastVersion) {
    lastVersion = v;
    lastWriteAt = now;
    writesSincePassive = true;
    writesSinceTruncate = true;
  }
  if (writesSincePassive && (walSize() > data.walLimitBytes || now - lastPassiveAt >= data.passiveMs)) {
    checkpoint("PASSIVE");
    lastPassiveAt = now;
    writesSincePassive = false;
  }
  if (writesSinceTruncate && now - lastWriteAt >= data.idleTruncateMs) {
    checkpoint("TRUNCATE");
    writesSinceTruncate = false;
    writesSincePassive = false;
    lastPassiveAt = now;
  }
}

const timer = setInterval(poll, data.pollMs);
parentPort?.on("message", (msg) => {
  if (msg !== "stop") return;
  clearInterval(timer);
  db.close();
  parentPort?.close();
});
