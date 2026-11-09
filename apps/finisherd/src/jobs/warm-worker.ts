// A warm worker's record and the parts of its life that need no pool (5C-1 plan Task 11; split
// out of `warm.ts` in Task 12): the constants, the spawn request, emptying its `tmp/` after a
// job (Task 11 review H2) and the outcome of an attempt that never ran.
import { readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import type { Hello, JobHostExitInfo, JobSpawnRequest } from "@finisher/core-types";
import type { FinError } from "@finisher/contracts";
import { toFinError } from "../errors.ts";
import { childEnv, passEnv } from "../env.ts";
import type { ProcEvent } from "./host.ts";
import type { FamilySpec, ProgramSpec } from "./kinds.ts";
import type { AttemptOutcome } from "./runner.ts";

export const WARM_SLOTS = 1;
export const WARM_IDLE_S = 60;
export const WARM_MAX_JOBS = 500;
export const WARM_MAX_AGE_S = 30 * 60;
/** A `ready` reporting more than this share of the worker group's limit recycles the worker. */
export const RECYCLE_MEMORY_SHARE = 0.8;
/** How long a worker asked to go may take to exit before its group is killed. */
export const RETIRE_GRACE_MS = 2000;
/** Warm workers' directories under the jobs root: `warm-<run>-<n>/`. */
export const WARM_DIR_PREFIX = "warm-";

export type State = "spawning" | "ready" | "busy" | "retiring";

/** A warm worker, as `workers().warm` lists it. */
export interface WarmWorkerInfo {
  readonly family: string;
  readonly key: string;
  readonly pid?: number;
  readonly state: State;
  readonly served: number;
  readonly ageS: number;
  readonly idleS: number;
  readonly memory?: number;
}

export interface Worker {
  readonly id: number;
  readonly family: FamilySpec;
  readonly key: string;
  readonly program: ProgramSpec;
  readonly group: number;
  readonly memoryBytes: number | undefined;
  readonly dir: string;
  /** Under a sandbox, its project's read roots (`FamilySpec.warmReads`, plan 7C Decision 19), canonical. */
  readonly reads: ReadonlyArray<string>;
  readonly spawnedAt: number;
  state: State;
  served: number;
  proc?: number;
  pid?: number;
  hello?: Hello;
  idleSince?: number;
  memory?: number;
  /** The running attempt, settled once its job has left the worker's directory. */
  visit?: Promise<void>;
  /** The running attempt's listener. */
  on?: ((e: ProcEvent) => void) | undefined;
  /** Why it was asked to go (its `worker.exit` reason). */
  retire?: "idleExpired" | "recycled" | "done";
  /** Its replacement's `hello` came while it was busy: it goes after this job. */
  retireAfterJob?: boolean;
  replacement?: Worker | undefined;
  /** The worker this one replaces (until its `hello`). */
  replaces?: Worker | undefined;
  mismatch?: FinError;
  /** Its `hello` made it `ready` once (a worker that never did fails its key's head job, B1). */
  cameUp: boolean;
  cancelIdle?: (() => void) | undefined;
  cancelKill?: (() => void) | undefined;
  gone: boolean;
}

export const noRun = (error: FinError, cause: "exit" | "mismatch", exit?: JobHostExitInfo): AttemptOutcome => ({
  ok: false,
  error,
  cause,
  ...(exit === undefined ? {} : { exit }),
  done: [],
  inFlight: [],
  helpers: 0,
  stopSent: false,
  runMicros: 0,
});

/** Where the jobs a warm worker serves visit it (plan 7C Decision 19): `<w.dir>/jobs/`. */
export const VISIT_DIR = "jobs";

/** The spawn request of worker `w`. */
export function spawnRequest(w: Worker): JobSpawnRequest {
  const tmp = join(w.dir, "tmp");
  return {
    role: "worker",
    profile: w.family.profile,
    family: w.family.family,
    program: "path" in w.program ? { path: w.program.path } : { bundled: w.program.binary },
    args: [...w.program.args],
    env: { ...childEnv(), ...passEnv(w.family.env), TMPDIR: tmp, TEMP: tmp, TMP: tmp },
    cwd: w.dir,
    // Its own directory, never the jobs root (plan 7C Decision 19): each job it serves is moved
    // into `jobs/` for its attempt and out again after it. Under a sandbox, its one project's
    // read roots, which the sandbox fixes at start.
    grants: [{ path: tmp, mode: "readWrite" }, { path: join(w.dir, VISIT_DIR), mode: "readWrite" }, ...w.reads.map((path) => ({ path, mode: "read" as const }))],
    qos: "normal",
    label: `warm ${w.family.family}${w.key === "" ? "" : `:${w.key}`}`,
  };
}

/** Empties the worker's `tmp/` (false: it could not be emptied). */
export async function emptyTmp(w: Worker, log: (level: "warning", message: string) => void): Promise<boolean> {
  const tmp = join(w.dir, "tmp");
  try {
    for (const name of await readdir(tmp)) await rm(join(tmp, name), { recursive: true, force: true });
    return true;
  } catch (err) {
    log("warning", `warm ${w.family.family} worker: its temp directory could not be emptied: ${toFinError(err).message}`);
    return false;
  }
}
