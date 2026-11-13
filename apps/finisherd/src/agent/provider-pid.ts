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
// never runs two processes on one transcript: one still alive after `SWEEP_WAIT_MS` keeps its
// record and the adapter refuses to start another beside it (`aliveProcesses`). Outside Windows
// a process whose start time cannot be read is never recorded (the adapter kills it). Synchronous:
// recovery runs at start, before the transports listen.
//
// The record also keeps how the session's last process ended (`last`): `clean` (it exited by
// itself, e.g. at stdin's end), `killed` (a signal, or the sweep), `unseen` (it was gone by the
// sweep: it ended while no server watched it). Verified 2.1.321, Task 0: `--resume` restores the
// CLI's cumulative totals after a clean exit and restarts them after a kill (s1d/s1e); and a CLI
// left by a SIGKILLed host in direct mode ran its allowed tool, finished the turn into its
// transcript and exited about 14 s later (s5C), so `unseen` means a tail the runtime never saw.
//
// The record also keeps the CLI's own accounting baseline (`baseline`, Task 10 review): what the
// process's cumulative totals were at its last `result` a server saw (written at each `result`,
// `noteResult`, and at its end), the offset it counted from, and the session's charged usage
// when it started. `--resume` after a clean end restores that process's totals (not the
// session's lifetime spend: a killed process before it restarted them), so the next process
// checks what it restored against it (`claude/usage.ts` `restoredOffset`): only what lies beyond
// (an unseen tail) is charged again; after a kill the CLI restarts its totals and the check finds
// nothing restored. A process that never reached a `result` leaves none: all a resume restores
// is charged.
//
// Every write is durable (`writePrivateFile`: temp file, fsync, rename, directory fsync), and a
// process is recorded before its instance takes any work (the adapter kills it otherwise).
import { lstatSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { type KeyFs, writePrivateFile } from "../auth/keys.ts";
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
  /** The CLI's accounting at the process's last `result` a server saw (`noteResult`). */
  readonly baseline?: CliBaseline;
}

/** How the session's last process ended (module docs). */
export type LastEnd = "clean" | "killed" | "unseen";

/** Tokens and money (micros), as the CLI totals them (`claude/usage.ts` `UsageTotals`). */
export interface Totals {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
  readonly costMicros: number;
}

/** The CLI's own accounting at a process's clean end (module docs). */
export interface CliBaseline {
  /** The CLI's cumulative totals at the process's last `result` (≤ what `--resume` restores). */
  readonly cli: Totals;
  /** What the process counted from (its own restored offset). */
  readonly offset: Totals;
  /** The session's charged usage (`Session.usage`) when the process started. */
  readonly charged: Totals;
}

interface PidFile {
  readonly v: 1;
  readonly processes: readonly ProviderProcess[];
  readonly last?: LastEnd;
  /** Only with an end a server saw (`clean`, `killed`). */
  readonly baseline?: CliBaseline;
}

const TOTAL_KEYS = ["input", "output", "cacheRead", "cacheWrite", "costMicros"] as const;
const isCount = (n: unknown): boolean => typeof n === "number" && Number.isSafeInteger(n) && n >= 0;
const isTotals = (v: unknown): v is Totals => typeof v === "object" && v !== null && TOTAL_KEYS.every((k) => isCount((v as Record<string, unknown>)[k]));
const isBaseline = (v: unknown): v is CliBaseline => typeof v === "object" && v !== null && isTotals((v as CliBaseline).cli) && isTotals((v as CliBaseline).offset) && isTotals((v as CliBaseline).charged);

const fileOf = (sessionDir: string) => join(sessionDir, PROVIDER_PID);

const LAST = new Set<string>(["clean", "killed", "unseen"]);

type Ended = Pick<PidFile, "last" | "baseline">;

function readFile(sessionDir: string): { processes: ProviderProcess[] } & Ended {
  try {
    const f = JSON.parse(readFileSync(fileOf(sessionDir), "utf8")) as Partial<PidFile>;
    const processes = Array.isArray(f.processes) ? f.processes.filter((p) => typeof p?.pid === "number" && Number.isInteger(p.pid) && p.pid > 0) : [];
    if (typeof f.last !== "string" || !LAST.has(f.last)) return { processes };
    return isBaseline(f.baseline) ? { processes, last: f.last, baseline: f.baseline } : { processes, last: f.last };
  } catch {
    return { processes: [] };
  }
}
const read = (sessionDir: string) => readFile(sessionDir).processes;
const endOf = (f: Ended): Ended => (f.last === undefined ? {} : f.baseline === undefined ? { last: f.last } : { last: f.last, baseline: f.baseline });

/** Written whole and durably (`writePrivateFile`: 0600, fsynced, renamed, the directory fsynced). */
function write(sessionDir: string, processes: readonly ProviderProcess[], ended: Ended = endOf(readFile(sessionDir)), io?: KeyFs): void {
  writePrivateFile(fileOf(sessionDir), JSON.stringify({ v: 1, processes, ...endOf(ended) } satisfies PidFile), process.platform, io);
}

export interface RecordOptions {
  readonly platform?: NodeJS.Platform;
  /** The record's file calls (tests inject failures). */
  readonly io?: KeyFs;
  /** `processStart` (tests inject a failure). */
  readonly start?: (pid: number) => string | null;
}

/**
 * Records a process the session's instance started (its start time read now). It throws when
 * the record cannot be written, or (outside Windows) when the start time cannot be read: a
 * record without one is never swept. The caller kills the process before it takes any work.
 */
export function recordProcess(sessionDir: string, instance: string, pid: number, o: RecordOptions = {}): void {
  const platform = o.platform ?? process.platform;
  const start = (o.start ?? ((p: number) => processStart(p, platform)))(pid);
  if (start === null && platform !== "win32") throw new Error(`the start time of process ${pid} could not be read`);
  write(sessionDir, [...read(sessionDir).filter((p) => p.pid !== pid), { instance, pid, start }], undefined, o.io);
}

/**
 * The CLI's accounting at a `result` of a running process (Task 10 review, O2): a sweep that
 * finds the process gone or kills it keeps it as the end's baseline, so a resume charges only
 * what the CLI restored beyond the last result a server saw (the unseen tail), not all of it.
 */
export function noteResult(sessionDir: string, pid: number, baseline: CliBaseline): void {
  const all = read(sessionDir);
  if (all.some((p) => p.pid === pid)) write(sessionDir, all.map((p) => (p.pid === pid ? { ...p, baseline } : p)));
}

/** The recorded processes still alive with their recorded start time (a resume must wait for them). */
export function aliveProcesses(sessionDir: string, start: (pid: number) => string | null = (pid) => processStart(pid)): number[] {
  return read(sessionDir)
    .filter((p) => p.start !== null && start(p.pid) === p.start)
    .map((p) => p.pid);
}

/**
 * The process has ended (`clean`: exit 0, no signal): it leaves the record, which keeps how, and
 * the CLI's accounting baseline.
 */
export function forgetProcess(sessionDir: string, pid: number, end: Exclude<LastEnd, "unseen"> = "clean", baseline?: CliBaseline): void {
  const all = read(sessionDir);
  if (all.some((p) => p.pid === pid)) write(sessionDir, all.filter((p) => p.pid !== pid), baseline !== undefined ? { last: end, baseline } : { last: end });
}

/** How the session's last process ended, if the record says. */
export const lastEnd = (sessionDir: string): LastEnd | undefined => readFile(sessionDir).last;

/** The CLI's accounting baseline of the session's last process, when a server saw it end. */
export const lastBaseline = (sessionDir: string): CliBaseline | undefined => readFile(sessionDir).baseline;

/** A record's own baseline when it is valid (a hand-edited file may hold anything). */
const baselineOf = (p: ProviderProcess): CliBaseline | undefined => (isBaseline(p.baseline) ? p.baseline : undefined);

export interface SweepDeps {
  readonly platform?: NodeJS.Platform;
  readonly start?: (pid: number) => string | null;
  readonly kill?: (pid: number, signal: NodeJS.Signals) => void;
  readonly waitMs?: number;
}

/** What one sweep did, by pid. */
export interface Swept {
  readonly killed: readonly number[];
  /**
   * Recorded but not killed: gone already, another process now (start time), no start time to
   * check, or still alive after `SWEEP_WAIT_MS` (its record is kept: a resume waits for it).
   */
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
  const alive: ProviderProcess[] = [];
  let baseline: CliBaseline | undefined;
  const recorded = read(sessionDir);
  for (const p of recorded) {
    if (p.start === null || start(p.pid) !== p.start) {
      left.push(p.pid);
      baseline = baselineOf(p) ?? baseline;
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
    if (start(p.pid) === p.start) {
      // Still there: not counted as killed, its record kept.
      left.push(p.pid);
      alive.push(p);
      continue;
    }
    killed.push(p.pid);
    baseline = baselineOf(p) ?? baseline;
    if (platform !== "win32") removeSocket(`/tmp/cc-socks-${process.getuid?.() ?? 0}/${p.pid}.sock`);
  }
  // A recorded process already gone ended while no server watched it (its tail is unseen).
  // Its baseline is the last `result` a server saw (none recorded: the restored part is charged).
  if (recorded.length > alive.length) {
    const last: LastEnd = killed.length > 0 ? "killed" : "unseen";
    write(sessionDir, alive, baseline === undefined ? { last } : { last, baseline });
  }
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
