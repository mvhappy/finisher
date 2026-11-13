// The provider processes a session runs, recorded for the orphan sweep (plan 7D Decisions 12, 17;
// Task 10). Each CLI instance's process is written to `agents/<id>/provider.pid` (0600, in the
// session directory, outside the sandbox's write roots) when it starts and dropped when it ends.
//
// Under the Linux launcher a `finisherd` that dies takes the CLI with it (parent-death signal,
// 7C); in `direct` mode (every OS until the `agent` profile is ready) and on macOS a SIGKILLed
// `finisherd` leaves the CLI running with stdin at its end, finishing its turn and its `fin`
// commits unwatched. So crash recovery (`AgentStore.recoverAfterCrash`) sweeps each session it
// interrupts before anything resumes it: a recorded process still alive **with the recorded start
// time** has its group killed (a recycled pid, another start time, is left alone), and a resume
// never runs two processes on one transcript. Synchronous: recovery runs at start, before the
// transports listen.
//
// The record also keeps how the session's last process ended (`last`): `clean` (it exited by
// itself, e.g. at stdin's end), `killed` (a signal, or the sweep), `unseen` (it was gone by the
// sweep: it ended while no server watched it). Verified 2.1.321, Task 0: `--resume` restores the
// CLI's cumulative totals after a clean exit and restarts them after a kill (s1d/s1e); and a CLI
// left by a SIGKILLed host in direct mode ran its allowed tool, finished the turn into its
// transcript and exited about 14 s later (s5C), so `unseen` means a tail the runtime never saw.
import { lstatSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { processStart } from "../sandbox/process-start.ts";

export const PROVIDER_PID = "provider.pid";
/** How long the sweep waits for a killed process to be gone. */
export const SWEEP_WAIT_MS = 2_000;

export interface ProviderProcess {
  /** The instance (ULID) the process serves. */
  readonly instance: string;
  /** The process the sandbox started (the launcher under `launcher` mode); its group has the same id. */
  readonly pid: number;
  /** `processStart(pid)` when it started; null where the OS gives none (Windows). */
  readonly start: string | null;
}

/** How the session's last process ended (module docs). */
export type LastEnd = "clean" | "killed" | "unseen";

interface PidFile {
  readonly v: 1;
  readonly processes: readonly ProviderProcess[];
  readonly last?: LastEnd;
}

const fileOf = (sessionDir: string) => join(sessionDir, PROVIDER_PID);

const LAST = new Set<string>(["clean", "killed", "unseen"]);

function readFile(sessionDir: string): { processes: ProviderProcess[]; last?: LastEnd } {
  try {
    const f = JSON.parse(readFileSync(fileOf(sessionDir), "utf8")) as Partial<PidFile>;
    const processes = Array.isArray(f.processes) ? f.processes.filter((p) => typeof p?.pid === "number" && Number.isInteger(p.pid) && p.pid > 0) : [];
    return typeof f.last === "string" && LAST.has(f.last) ? { processes, last: f.last } : { processes };
  } catch {
    return { processes: [] };
  }
}
const read = (sessionDir: string) => readFile(sessionDir).processes;

/** Written whole: a temp file (0600) renamed over the record. */
function write(sessionDir: string, processes: readonly ProviderProcess[], last = readFile(sessionDir).last): void {
  const file = fileOf(sessionDir);
  const tmp = `${file}.${process.pid}.${Date.now()}.tmp`;
  writeFileSync(tmp, JSON.stringify({ v: 1, processes, ...(last === undefined ? {} : { last }) } satisfies PidFile), { mode: 0o600 });
  renameSync(tmp, file);
}

/** Records a process the session's instance started (its start time read now). */
export function recordProcess(sessionDir: string, instance: string, pid: number, platform: NodeJS.Platform = process.platform): void {
  write(sessionDir, [...read(sessionDir).filter((p) => p.pid !== pid), { instance, pid, start: processStart(pid, platform) }]);
}

/** The process has ended (`clean`: by itself, no signal): it leaves the record, which keeps how. */
export function forgetProcess(sessionDir: string, pid: number, end: Exclude<LastEnd, "unseen"> = "clean"): void {
  const all = read(sessionDir);
  if (all.some((p) => p.pid === pid)) write(sessionDir, all.filter((p) => p.pid !== pid), end);
}

/** How the session's last process ended, if the record says. */
export const lastEnd = (sessionDir: string): LastEnd | undefined => readFile(sessionDir).last;

export interface SweepDeps {
  readonly platform?: NodeJS.Platform;
  readonly start?: (pid: number) => string | null;
  readonly kill?: (pid: number, signal: NodeJS.Signals) => void;
  readonly waitMs?: number;
}

/** What one sweep did, by pid. */
export interface Swept {
  readonly killed: readonly number[];
  /** Recorded but not killed: gone already, another process now (start time), or no start time to check. */
  readonly left: readonly number[];
}

const sleep = (ms: number) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

/**
 * Decision 17's sweep of one session directory: each recorded process alive with its recorded
 * start time has its group killed, and is waited for (≤ `SWEEP_WAIT_MS`); then the CLI's inbox
 * socket it left (`/tmp/cc-socks-<uid>/<pid>.sock`, Task 0) is removed, and the record.
 */
export function sweepProviderProcesses(sessionDir: string, deps: SweepDeps = {}): Swept {
  const platform = deps.platform ?? process.platform;
  const start = deps.start ?? ((pid: number) => processStart(pid, platform));
  const kill = deps.kill ?? ((pid: number, signal: NodeJS.Signals) => process.kill(pid, signal));
  const killed: number[] = [];
  const left: number[] = [];
  const recorded = read(sessionDir);
  for (const p of recorded) {
    if (p.start === null || start(p.pid) !== p.start) {
      left.push(p.pid);
      continue;
    }
    // The group (the process is its leader: the sandbox starts each one detached), then the
    // process itself in case it left it.
    for (const target of [-p.pid, p.pid]) {
      try {
        kill(target, "SIGKILL");
      } catch {
        // Gone already.
      }
    }
    const end = Date.now() + (deps.waitMs ?? SWEEP_WAIT_MS);
    while (start(p.pid) === p.start && Date.now() < end) sleep(10);
    killed.push(p.pid);
    if (platform !== "win32") removeSocket(`/tmp/cc-socks-${process.getuid?.() ?? 0}/${p.pid}.sock`);
  }
  // A recorded process already gone ended while no server watched it (its tail is unseen).
  if (recorded.length > 0) write(sessionDir, [], killed.length > 0 ? "killed" : "unseen");
  return { killed, left };
}

/** The sweep of the session `sessionId` under `<server-data>/agents` (`AgentStore.recoverAfterCrash`). */
export const sweepSession = (agentsRoot: string, sessionId: string): Swept => sweepProviderProcesses(join(agentsRoot, sessionId));

function removeSocket(path: string): void {
  try {
    if (lstatSync(path).isSocket()) rmSync(path, { force: true });
  } catch {
    // None there.
  }
}
