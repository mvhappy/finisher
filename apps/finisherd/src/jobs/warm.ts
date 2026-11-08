// Warm workers (spec §6.3, 5C-1 plan Decisions 6, 20, 31, 35, 36; Task 11). Per family, the
// pool's workers, each bound to the warm key of the job it was spawned for: `spawning` until
// its `hello`, then `ready` or `busy`, then `retiring` once it was asked to go. A job whose key
// has a ready worker is handed to it in the submit's own turn (`handOff`, no `schedule()`);
// queued warm jobs, spawns and the retirement of another key's worker come from `schedule()`.
// A worker idle for `idleS` (60 s) gets `shutdown` (`idleExpired`); one that served `maxJobs`
// (500), lived `maxAgeS` (30 min) or reported at its `ready` more than 80 % of its group's
// limit is recycled: its replacement is spawned first and it serves until the replacement's
// `hello`, then it gets `shutdown` (`recycled`). A worker that crashes or is killed during a
// job is lost; the job's retry (once, at once) runs on a worker that has served no job. A
// worker that dies before its `hello` fails the attempt of its key's oldest queued job, as a
// cold spawn would. Each worker has its own group (the family's interactive memory rule) and
// directory (`warm-<n>/`, its cwd and temp directory). Times come from the engine's clock.
import { join } from "node:path";
import type { Hello, JobHostExitInfo, JobHostKillReason, JobSpawnRequest } from "@finisher/core-types";
import type { FinError, JobView } from "@finisher/contracts";
import { resolveMemoryRule } from "@finisher/fin-node";
import { finError, toFinError } from "../errors.ts";
import { childEnv } from "../env.ts";
import { helloMismatch, sendWorkerExit } from "./attempt.ts";
import type { Engine, WarmLease } from "./engine.ts";
import { removeJobDir } from "./files.ts";
import { mkdirSafe } from "./fsafe.ts";
import { exitVerdict, type ProcEvent } from "./host.ts";
import { type FamilySpec, type KindSpec, type ProgramSpec, programFor } from "./kinds.ts";
import type { RunControl } from "./run-control.ts";
import type { AttemptOutcome } from "./runner.ts";
import type { SchedState, WarmWorkerView } from "./schedule.ts";
import type { JobRow } from "./store.ts";
import { runWarmAttempt } from "./warm-run.ts";

export const WARM_SLOTS = 1;
export const WARM_IDLE_S = 60;
export const WARM_MAX_JOBS = 500;
export const WARM_MAX_AGE_S = 30 * 60;
/** A `ready` reporting more than this share of the worker group's limit recycles the worker. */
export const RECYCLE_MEMORY_SHARE = 0.8;
/** How long a worker asked to go may take to exit before its group is killed. */
export const RETIRE_GRACE_MS = 2000;
/** Warm workers' directories under the jobs root: `warm-<n>/`. */
export const WARM_DIR_PREFIX = "warm-";

type State = "spawning" | "ready" | "busy" | "retiring";

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

interface Worker {
  readonly id: number;
  readonly family: FamilySpec;
  readonly key: string;
  readonly program: ProgramSpec;
  readonly group: number;
  readonly memoryBytes: number | undefined;
  readonly dir: string;
  readonly spawnedAt: number;
  state: State;
  served: number;
  proc?: number;
  pid?: number;
  hello?: Hello;
  idleSince?: number;
  memory?: number;
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
  cancelIdle?: (() => void) | undefined;
  cancelKill?: (() => void) | undefined;
  gone: boolean;
}

const noRun = (error: FinError, cause: "exit" | "mismatch", exit?: JobHostExitInfo): AttemptOutcome => ({
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

export class WarmPools {
  readonly #e: Engine;
  readonly #byFamily = new Map<string, Worker[]>();
  /** The workers behind the last `schedState()`'s views, for its retirements. */
  #viewed = new Map<string, Worker[]>();
  #families: { n: number; map: Map<string, FamilySpec> } | undefined;
  #nextId = 1;

  constructor(e: Engine) {
    this.#e = e;
  }

  /** Families that serve a warm kind (with their slots, the scheduler's warm pools). */
  families(): ReadonlyMap<string, FamilySpec> {
    const kinds = this.#e.kinds.kinds();
    if (this.#families?.n !== kinds.length) {
      const map = new Map<string, FamilySpec>();
      for (const k of kinds) {
        const f = k.warm === true ? this.#e.kinds.family(k.family) : undefined;
        if (f !== undefined) map.set(f.family, f);
      }
      this.#families = { n: kinds.length, map };
    }
    return this.#families.map;
  }

  #list(family: string): Worker[] {
    let list = this.#byFamily.get(family);
    if (list === undefined) {
      list = [];
      this.#byFamily.set(family, list);
    }
    return list;
  }

  /** A job's warm tag: a `warm` kind at `interactive` priority (Decision 35), else `undefined`. */
  tag(row: JobView, kind: KindSpec): { family: string; key: string; fresh?: true } | undefined {
    if (kind.warm !== true || row.priority !== "interactive") return undefined;
    const f = this.families().get(kind.family);
    if (f === undefined) return undefined;
    let key = "";
    try {
      key = f.warmKey?.(row) ?? "";
    } catch (e) {
      this.#e.d.log("warning", `job ${row.id}: the warm key of ${f.family} failed: ${toFinError(e).message}`);
    }
    return { family: f.family, key, ...(row.attempts > 0 ? { fresh: true as const } : {}) };
  }

  /** The cpu slots warm dispatches may take beyond the pool (Decision 35's `warmOverrun`). */
  overrun(): { cpu: number } {
    let cpu = 0;
    for (const f of this.families().values()) cpu += f.warm?.slots ?? WARM_SLOTS;
    return { cpu };
  }

  /** The warm pools as `schedule()` reads them. */
  schedState(): SchedState["warm"] {
    const out = new Map<string, { slots: number; workers: WarmWorkerView[] }>();
    this.#viewed = new Map();
    for (const f of this.families().values()) {
      const live = (this.#byFamily.get(f.family) ?? []).filter((w) => w.state !== "retiring");
      this.#viewed.set(f.family, live);
      const workers = live.map((w): WarmWorkerView => ({
        key: w.key,
        state: w.state === "retiring" ? "busy" : w.state,
        served: w.served,
        ...(w.idleSince === undefined ? {} : { idleSince: w.idleSince }),
      }));
      out.set(f.family, { slots: f.warm?.slots ?? WARM_SLOTS, workers });
    }
    return out;
  }

  #ready(family: string, key: string, fresh: boolean): Worker | undefined {
    return this.#byFamily.get(family)?.find((w) => w.state === "ready" && w.key === key && (!fresh || w.served === 0));
  }

  /**
   * The direct hand-off (Decision 35): a just queued warm job that heads its key's FIFO and has
   * a ready worker runs on it now, in the submit's own turn. False: it goes through `schedule()`.
   */
  handOff(id: string): boolean {
    const e = this.#e;
    if (e.closing || e.pausedAll) return false;
    const q = e.queue.get(id);
    if (q?.warm === undefined) return false;
    const w = this.#ready(q.warm.family, q.warm.key, q.warm.fresh === true);
    if (w === undefined || e.queue.warmHead(q.warm.family, q.warm.key, e.d.now())?.id !== id) return false;
    return e.dispatch(id, this.#lease(w));
  }

  /** `schedule()` gave queued warm job `id` a ready worker of its key. */
  dispatchQueued(id: string, family: string, key: string): void {
    const w = this.#ready(family, key, this.#e.queue.get(id)?.warm?.fresh === true);
    if (w === undefined) this.#e.d.log("error", `job ${id}: scheduled on a warm ${family} worker, but none is ready`);
    else this.#e.dispatch(id, this.#lease(w));
  }

  /** `schedule()`'s `spawnWarm`: retires `retire` (an index of the last state's workers) first. */
  spawnFor(family: string, key: string, retire: number | undefined): void {
    const e = this.#e;
    if (retire !== undefined) {
      const victim = this.#viewed.get(family)?.[retire];
      if (victim !== undefined) this.#retire(victim, "recycled");
    }
    const f = e.kinds.family(family);
    if (f === undefined) return;
    const head = e.queue.warmHead(family, key, e.d.now());
    const row = head === undefined ? undefined : e.d.store.row(head.id);
    let program: ProgramSpec;
    try {
      program = row === undefined ? programFor(f.program as ProgramSpec, "warm") : e.kinds.program(family, row, "warm");
    } catch (err) {
      const error = finError("WORKER_UNAVAILABLE", `the warm ${family} worker has no program: ${toFinError(err).message}`, { reason: "program" });
      if (head !== undefined) e.dispatch(head.id, { run: async () => noRun(error, "exit"), release: () => {} });
      return;
    }
    this.#spawn(f, key, program);
  }

  /** `warm(family)`: one worker now when the pool has none (Decision 36's pre-warming). */
  prewarm(f: FamilySpec): void {
    if ((this.#byFamily.get(f.family) ?? []).some((w) => w.state !== "retiring")) return;
    if (typeof f.program === "function") {
      this.#e.d.log("debug", `warm(${f.family}): its program depends on the job; the first job spawns its worker`);
      return;
    }
    this.#spawn(f, "", programFor(f.program, "warm"));
  }

  #lease(w: Worker): WarmLease {
    w.state = "busy";
    w.cancelIdle?.();
    let used = false;
    return {
      run: (row, kind, family, params, control) => {
        used = true;
        return this.#run(w, row, kind, family, params, control);
      },
      release: () => {
        if (!used && !w.gone && w.state === "busy") this.#idle(w, undefined, false);
      },
    };
  }

  async #run(w: Worker, row: JobRow, kind: KindSpec, family: FamilySpec, params: unknown, control: RunControl): Promise<AttemptOutcome> {
    const mismatch = w.hello === undefined ? undefined : helloMismatch(w.hello, family, kind, true);
    if (mismatch !== undefined || w.proc === undefined || w.gone) {
      this.#idle(w, undefined, false);
      return mismatch === undefined ? noRun(finError("WORKER_UNAVAILABLE", "the warm worker is gone", { reason: "warmExited" }), "exit") : noRun(mismatch, "mismatch");
    }
    const out = await runWarmAttempt(this.#e.env, row, kind, family, params, control, {
      proc: w.proc,
      pid: w.pid,
      memoryBytes: w.memoryBytes,
      reused: w.served > 0,
      bind: (on) => (w.on = on),
      started: () => void w.served++,
      lose: (reason) => this.#kill(w, reason),
    });
    if (out.kept) this.#idle(w, out.memory, true);
    return out;
  }

  /** Back to `ready` (the engine dispatches next once the job's slots are back). */
  #idle(w: Worker, memory: number | undefined, afterJob: boolean): void {
    if (w.gone || w.state === "retiring") return;
    w.state = "ready";
    w.idleSince = this.#e.d.now();
    if (memory !== undefined) w.memory = memory;
    if (w.retireAfterJob === true) {
      this.#retire(w, "recycled");
      return;
    }
    if (afterJob && this.#recycleDue(w)) this.#recycle(w);
    this.#armIdle(w);
  }

  #recycleDue(w: Worker): boolean {
    const f = w.family.warm ?? {};
    if (w.served >= (f.maxJobs ?? WARM_MAX_JOBS)) return true;
    if (this.#e.d.now() - w.spawnedAt >= (f.maxAgeS ?? WARM_MAX_AGE_S) * 1000) return true;
    return w.memoryBytes !== undefined && w.memory !== undefined && w.memory > RECYCLE_MEMORY_SHARE * w.memoryBytes;
  }

  /** The replacement first; the worker serves until its `hello` (Decision 36). */
  #recycle(w: Worker): void {
    if (w.replacement !== undefined || w.retire !== undefined) return;
    const r = this.#spawn(w.family, w.key, w.program);
    r.replaces = w;
    w.replacement = r;
  }

  #armIdle(w: Worker): void {
    w.cancelIdle?.();
    w.cancelIdle = this.#e.d.timers.after((w.family.warm?.idleS ?? WARM_IDLE_S) * 1000, () => {
      w.cancelIdle = undefined;
      if (w.state !== "ready") return;
      this.#retire(w, "idleExpired");
      this.#e.pump();
    });
  }

  /** `shutdown` to the worker; its group is killed if it has not exited after the grace. */
  #retire(w: Worker, reason: "idleExpired" | "recycled" | "done"): void {
    if (w.gone || w.state === "retiring") return;
    w.state = "retiring";
    w.retire = reason;
    w.cancelIdle?.();
    if (w.replacement !== undefined) w.replacement.replaces = undefined;
    w.replacement = undefined;
    if (w.proc !== undefined) this.#e.d.host.send(w.proc, { t: "shutdown" });
    const kill: JobHostKillReason = reason === "idleExpired" ? "idleExpired" : reason === "recycled" ? "recycle" : "shutdown";
    w.cancelKill = this.#e.d.timers.after(RETIRE_GRACE_MS, () => this.#kill(w, kill));
  }

  #kill(w: Worker, reason: JobHostKillReason): void {
    if (w.gone) return;
    w.state = "retiring";
    w.cancelIdle?.();
    void this.#e.d.host.killGroup(w.group, reason).catch(() => {});
  }

  #spawn(f: FamilySpec, key: string, program: ProgramSpec): Worker {
    const e = this.#e;
    const id = this.#nextId++;
    const rule = f.memory?.interactive;
    const memoryBytes = rule === undefined ? undefined : resolveMemoryRule(rule, e.d.totalMem);
    const group = e.d.host.openGroup({ label: `warm ${f.family} ${id}`, qos: "normal", ...(memoryBytes === undefined ? {} : { memoryBytes }) });
    const w: Worker = { id, family: f, key, program, group, memoryBytes, dir: join(e.d.root, `${WARM_DIR_PREFIX}${id}`), spawnedAt: e.d.now(), state: "spawning", served: 0, gone: false };
    this.#list(f.family).push(w);
    void this.#start(w);
    return w;
  }

  async #start(w: Worker): Promise<void> {
    const e = this.#e;
    const tmp = join(w.dir, "tmp");
    try {
      await mkdirSafe(e.d.root, `${WARM_DIR_PREFIX}${w.id}/tmp`);
      const req: JobSpawnRequest = {
        role: "worker",
        profile: w.family.profile,
        family: w.family.family,
        program: "path" in w.program ? { path: w.program.path } : { bundled: w.program.binary },
        args: [...w.program.args],
        env: { ...childEnv(), TMPDIR: tmp, TEMP: tmp, TMP: tmp },
        cwd: w.dir,
        // Its own directory, and the jobs root: the jobs it will serve are not known yet
        // (sub-project 7's sandbox narrows this per job).
        grants: [
          { path: tmp, mode: "readWrite" },
          { path: e.d.root, mode: "readWrite" },
        ],
        qos: "normal",
        label: `warm ${w.family.family}${w.key === "" ? "" : `:${w.key}`}`,
      };
      w.proc = await e.d.host.spawn(w.group, req, (ev) => this.#onEvent(w, ev));
    } catch (err) {
      this.#gone(w, { reason: "spawnFailed", what: toFinError(err).message, stderrTail: "", lifetimeMs: 0 });
    }
  }

  #onEvent(w: Worker, ev: ProcEvent): void {
    if (ev.type === "hello") return this.#onHello(w, ev.hello);
    if (ev.type === "exit") {
      w.on?.(ev);
      return this.#gone(w, ev.info);
    }
    if (w.on !== undefined) return w.on(ev);
    if (ev.type === "stderr") this.#e.d.log("debug", `[warm ${w.family.family}] ${ev.line}`);
    else if (ev.msg.t !== "alive" && ev.msg.t !== "ready") this.#e.d.log("debug", `warm ${w.family.family} worker: ${ev.msg.t} while idle is ignored`);
  }

  #onHello(w: Worker, hello: Hello): void {
    w.pid = hello.pid;
    w.hello = hello;
    if (w.state !== "spawning") return;
    if (hello.role !== "worker" || hello.family !== w.family.family || hello.warm !== true) {
      w.mismatch = finError("WORKER_PROTOCOL_MISMATCH", `the worker is not a warm ${w.family.family} worker`, {
        expected: { role: "worker", family: w.family.family, warm: true },
        got: { role: hello.role, family: hello.family ?? null, warm: hello.warm ?? false },
      });
      this.#kill(w, "jobEnded");
      return;
    }
    w.state = "ready";
    w.idleSince = this.#e.d.now();
    this.#armIdle(w);
    const old = w.replaces;
    w.replaces = undefined;
    if (old !== undefined) {
      old.replacement = undefined;
      if (old.state === "ready") this.#retire(old, "recycled");
      else if (old.state === "busy") old.retireAfterJob = true;
    }
    this.#e.pump();
  }

  #gone(w: Worker, info: JobHostExitInfo): void {
    if (w.gone) return;
    const e = this.#e;
    const before = w.state;
    const replacement = w.replaces !== undefined;
    w.gone = true;
    w.state = "retiring";
    w.cancelIdle?.();
    w.cancelKill?.();
    const list = this.#list(w.family.family);
    list.splice(list.indexOf(w), 1);
    if (w.replaces !== undefined) w.replaces.replacement = undefined;
    if (w.replacement !== undefined) w.replacement.replaces = undefined;
    const base = exitVerdict(info, true).reason;
    const reason = w.retire !== undefined && (base === "done" || info.reason === "killed") ? w.retire : base;
    sendWorkerExit(e.env, w.family, info, reason, w.served, `warm ${w.family.family} worker`);
    void (async () => {
      await e.d.host.killGroup(w.group, "jobEnded").catch(() => {});
      e.d.host.closeGroup(w.group);
      await removeJobDir(w.dir).catch((err) => e.d.log("warning", `warm worker directory ${w.dir}: ${toFinError(err).message}`));
    })();
    if (e.closing || e.crashed) return;
    // A worker that never came up fails its key's oldest job's attempt, as a cold spawn would.
    if (before === "spawning" && !replacement) {
      const head = e.queue.warmHead(w.family.family, w.key, e.d.now());
      const error = w.mismatch ?? exitVerdict(info, false).error;
      if (head !== undefined) e.dispatch(head.id, { run: async () => noRun(error, w.mismatch === undefined ? "exit" : "mismatch", info), release: () => {} });
    }
    e.pump();
  }

  /** Whether `name` under the jobs root is a live warm worker's directory (the hourly sweep keeps it). */
  owns(name: string): boolean {
    for (const list of this.#byFamily.values()) if (list.some((w) => `${WARM_DIR_PREFIX}${w.id}` === name)) return true;
    return false;
  }

  /** `workers().warm`. */
  list(): WarmWorkerInfo[] {
    const now = this.#e.d.now();
    return [...this.#byFamily.values()].flat().map((w) => ({
      family: w.family.family,
      key: w.key,
      ...(w.pid === undefined ? {} : { pid: w.pid }),
      state: w.state,
      served: w.served,
      ageS: (now - w.spawnedAt) / 1000,
      idleS: w.state === "ready" && w.idleSince !== undefined ? (now - w.idleSince) / 1000 : 0,
      ...(w.memory === undefined ? {} : { memory: w.memory }),
    }));
  }

  /** Close (Decision 29): `shutdown` to every worker; resolves once they exited, at most after the grace. */
  async close(): Promise<void> {
    const all = [...this.#byFamily.values()].flat();
    for (const w of all) {
      if (w.state === "spawning") this.#kill(w, "shutdown");
      else this.#retire(w, "done");
    }
    // Real time: a test's TestClock may stand still.
    const end = Date.now() + RETIRE_GRACE_MS;
    while (all.some((w) => !w.gone) && Date.now() < end) await new Promise((r) => setTimeout(r, 10));
  }
}
