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
// cold spawn would, and so does one whose `hello` is not a warm worker of its family (Task 11
// review B1). Each worker has its own group (the family's interactive memory rule) and
// directory (`warm-<run>-<n>/`, its cwd; `tmp/` in it, emptied after each job, H2). Under a
// sandbox (`caps().sandboxed`, or a `hello` with `selfSandbox`) warm kinds run cold (owner
// decision, H3): a warm worker is granted the whole jobs root, which only per-job grants from
// sub-project 7C may narrow. Times come from the engine's clock.
import { randomBytes } from "node:crypto";
import { basename, join } from "node:path";
import type { Hello, JobHostExitInfo, JobHostKillReason } from "@finisher/core-types";
import type { JobView } from "@finisher/contracts";
import { resolveMemoryRule } from "@finisher/fin-node";
import { finError, toFinError } from "../errors.ts";
import { helloMismatch, sendWorkerExit } from "./attempt.ts";
import type { WarmLease } from "./attempt-flow.ts";
import type { Engine } from "./engine.ts";
import { removeJobDir } from "./files.ts";
import { mkdirSafe } from "./fsafe.ts";
import { exitVerdict, type ProcEvent } from "./host.ts";
import { type FamilySpec, type KindSpec, type ProgramSpec, programFor } from "./kinds.ts";
import type { RunControl } from "./run-control.ts";
import type { AttemptOutcome } from "./runner.ts";
import type { SchedState, WarmWorkerView } from "./schedule.ts";
import type { JobRow } from "./store.ts";
import { runWarmAttempt } from "./warm-run.ts";
import { emptyTmp, noRun, RECYCLE_MEMORY_SHARE, RETIRE_GRACE_MS, spawnRequest, WARM_DIR_PREFIX, WARM_IDLE_S, WARM_MAX_AGE_S, WARM_MAX_JOBS, WARM_SLOTS, type WarmWorkerInfo, type Worker } from "./warm-worker.ts";

export { RECYCLE_MEMORY_SHARE, RETIRE_GRACE_MS, WARM_DIR_PREFIX, WARM_IDLE_S, WARM_MAX_AGE_S, WARM_MAX_JOBS, WARM_SLOTS, type WarmWorkerInfo } from "./warm-worker.ts";

export class WarmPools {
  readonly #e: Engine;
  readonly #byFamily = new Map<string, Worker[]>();
  /** The workers behind the last `schedState()`'s views, for its retirements. */
  #viewed = new Map<string, Worker[]>();
  #families: { n: number; map: Map<string, FamilySpec> } | undefined;
  #nextId = 1;
  /** This engine run's part of the directory names (H2): a leftover of another run never matches. */
  readonly #runId = randomBytes(4).toString("hex");
  /** Jobs whose last attempt lost its worker: their retry takes a fresh one (M2). */
  readonly #lostOn = new Set<string>();
  /** A worker's `hello` reported a self-applied sandbox: warm kinds run cold from now on (H3). */
  #selfSandboxed = false;
  /** Directory removals in flight (`close` waits for them, H2). */
  readonly #removals = new Set<Promise<void>>();

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

  /**
   * Whether a sandbox is in force (H3, owner decision): then warm kinds run cold, until
   * sub-project 7C grants a warm worker each job's paths instead of the whole jobs root.
   */
  sandboxed(): boolean {
    return this.#selfSandboxed || this.#e.d.host.caps().sandboxed;
  }

  /** A job's warm tag: a `warm` kind at `interactive` priority (Decision 35), else `undefined`. */
  tag(row: JobView, kind: KindSpec): { family: string; key: string; fresh?: true } | undefined {
    if (kind.warm !== true || row.priority !== "interactive" || this.sandboxed()) return undefined;
    const f = this.families().get(kind.family);
    if (f === undefined) return undefined;
    let key = "";
    try {
      key = f.warmKey?.(row) ?? "";
    } catch (e) {
      this.#e.d.log("warning", `job ${row.id}: the warm key of ${f.family} failed: ${toFinError(e).message}`);
    }
    return { family: f.family, key, ...(this.#lostOn.has(row.id) ? { fresh: true as const } : {}) };
  }

  /** The job ended: nothing more to remember of it. */
  forget(id: string): void {
    this.#lostOn.delete(id);
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
    if (e.closing || e.crashed) return;
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
    if (this.sandboxed() || (this.#byFamily.get(f.family) ?? []).some((w) => w.state !== "retiring")) return;
    if (f.warmKey !== undefined) {
      // A `""`-key worker would serve no job of a keyed family (L5).
      this.#e.d.log("debug", `warm(${f.family}): its workers are keyed by job; the first job spawns its worker`);
      return;
    }
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
    this.#lostOn.delete(row.id);
    let started = false;
    let out: Awaited<ReturnType<typeof runWarmAttempt>>;
    try {
      out = await runWarmAttempt(this.#e.env, row, kind, family, params, control, {
        proc: w.proc,
        pid: w.pid,
        memoryBytes: w.memoryBytes,
        reused: w.served > 0,
        bind: (on) => (w.on = on),
        started: () => {
          started = true;
          w.served++;
        },
        lose: (reason) => this.#kill(w, reason),
      });
    } catch (err) {
      // H1: the worker never stays `busy`; one that got the `start` may hold the job's state.
      w.on = undefined;
      if (started) {
        this.#lostOn.add(row.id);
        this.#kill(w, "jobEnded");
      } else this.#idle(w, undefined, false);
      throw err;
    }
    if (!out.kept) {
      this.#lostOn.add(row.id);
      return out;
    }
    // Under a sandbox a worker is never handed another job (H3): its grant covers every job.
    if (this.sandboxed()) {
      this.#retire(w, "done");
      return out;
    }
    // Nothing of this job reaches the next one (H2).
    if (!(await emptyTmp(w, this.#e.d.log))) this.#kill(w, "jobEnded");
    else this.#idle(w, out.memory, true);
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
    if (r === undefined) return;
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

  /** A new worker (none while the engine closes, L1). */
  #spawn(f: FamilySpec, key: string, program: ProgramSpec): Worker | undefined {
    const e = this.#e;
    if (e.closing || e.crashed || this.sandboxed()) return undefined;
    const id = this.#nextId++;
    const rule = f.memory?.interactive;
    const memoryBytes = rule === undefined ? undefined : resolveMemoryRule(rule, e.d.totalMem);
    const group = e.d.host.openGroup({ label: `warm ${f.family} ${id}`, qos: "normal", ...(memoryBytes === undefined ? {} : { memoryBytes }) });
    const name = `${WARM_DIR_PREFIX}${this.#runId}-${id}`;
    const w: Worker = { id, family: f, key, program, group, memoryBytes, dir: join(e.d.root, name), spawnedAt: e.d.now(), state: "spawning", served: 0, gone: false, cameUp: false };
    this.#list(f.family).push(w);
    void this.#start(w);
    return w;
  }

  async #start(w: Worker): Promise<void> {
    const e = this.#e;
    try {
      // A leftover of this name is removed first (H2): a worker starts with an empty directory.
      await removeJobDir(w.dir);
      await mkdirSafe(e.d.root, `${basename(w.dir)}/tmp`);
      // The engine began closing meanwhile: no process (L1).
      if (e.closing || e.crashed || w.gone) return this.#drop(w);
      const req = spawnRequest(w, e.d.root);
      w.proc = await e.d.host.spawn(w.group, req, (ev) => this.#onEvent(w, ev));
    } catch (err) {
      this.#gone(w, { reason: "spawnFailed", what: toFinError(err).message, stderrTail: "", lifetimeMs: 0 });
    }
  }

  /** A worker that never got a process goes quietly: no `worker.exit`, its directory removed. */
  #drop(w: Worker): void {
    if (w.gone) return;
    w.gone = true;
    w.state = "retiring";
    const list = this.#list(w.family.family);
    list.splice(list.indexOf(w), 1);
    if (w.replaces !== undefined) w.replaces.replacement = undefined;
    this.#e.d.host.closeGroup(w.group);
    const removal = removeJobDir(w.dir).catch((err) => this.#e.d.log("warning", `warm worker directory ${w.dir}: ${toFinError(err).message}`));
    this.#removals.add(removal);
    void removal.finally(() => this.#removals.delete(removal));
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
    // `none`: a sandbox was asked for and nothing applied (F2, Landlock off): not confined.
    if (hello.selfSandbox !== undefined && hello.selfSandbox !== "" && hello.selfSandbox !== "none") return this.#sandboxFound(w, hello.selfSandbox);
    if (hello.role !== "worker" || hello.family !== w.family.family || hello.warm !== true) {
      w.mismatch = finError("WORKER_PROTOCOL_MISMATCH", `the worker is not a warm ${w.family.family} worker`, {
        expected: { role: "worker", family: w.family.family, warm: true },
        got: { role: hello.role, family: hello.family ?? null, warm: hello.warm ?? false },
      });
      this.#kill(w, "jobEnded");
      return;
    }
    w.state = "ready";
    w.cameUp = true;
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

  /**
   * A warm worker applied a sandbox to itself (H3): warm kinds run cold from now on. Every warm
   * worker goes (a busy one after its job), and the queued warm jobs are queued again as cold.
   */
  #sandboxFound(w: Worker, profile: string): void {
    const e = this.#e;
    this.#selfSandboxed = true;
    e.d.log("warning", `warm ${w.family.family} worker applied the ${profile} sandbox: warm kinds run cold until per-job grants exist (sub-project 7C)`);
    // First the queue, so no exit below finds a warm job to spawn for.
    for (const id of e.queue.warmIds()) {
      const row = e.d.store.row(id);
      const kind = row === undefined ? undefined : e.kinds.kind(row.kind);
      if (row?.state === "queued" && kind !== undefined) e.enqueue(row, kind);
    }
    w.cameUp = true; // its key's job was queued again, not failed
    this.#kill(w, "shutdown");
    for (const other of [...this.#byFamily.values()].flat()) {
      if (other.state === "ready") this.#retire(other, "done");
      else if (other.state === "spawning") {
        other.cameUp = true;
        this.#kill(other, "shutdown");
      }
    }
    e.pump();
  }

  #gone(w: Worker, info: JobHostExitInfo): void {
    if (w.gone) return;
    const e = this.#e;
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
    // An idle worker that would not die holds its slot like a job's process (L2); a busy one's attempt reports it.
    if (info.reason === "abandoned" && w.on === undefined && w.proc !== undefined) e.abandonment.abandoned(w.proc, `warm ${w.family.family} ${w.id}`, []);
    const removal = (async () => {
      await e.d.host.killGroup(w.group, "jobEnded").catch(() => {});
      e.d.host.closeGroup(w.group);
      await removeJobDir(w.dir).catch((err) => e.d.log("warning", `warm worker directory ${w.dir}: ${toFinError(err).message}`));
    })();
    this.#removals.add(removal);
    void removal.finally(() => this.#removals.delete(removal));
    if (e.closing || e.crashed) return;
    // A worker that never came up (it died before its `hello`, or its `hello` was not a warm
    // worker of its family, B1) fails its key's oldest job's attempt, as a cold spawn would.
    if (!w.cameUp && !replacement) {
      const head = e.queue.warmHead(w.family.family, w.key, e.d.now());
      const error = w.mismatch ?? exitVerdict(info, false).error;
      if (head !== undefined) e.dispatch(head.id, { run: async () => noRun(error, w.mismatch === undefined ? "exit" : "mismatch", info), release: () => {} });
    }
    e.pump();
  }

  /** Whether `name` under the jobs root is a live warm worker's directory (the hourly sweep keeps it). */
  owns(name: string): boolean {
    for (const list of this.#byFamily.values()) if (list.some((w) => basename(w.dir) === name)) return true;
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
    // Real time: a test's TestClock may stand still. One that has not gone after the grace is killed.
    const waitGone = async (): Promise<void> => {
      const end = Date.now() + RETIRE_GRACE_MS;
      while (all.some((w) => !w.gone) && Date.now() < end) await new Promise((r) => setTimeout(r, 10));
    };
    await waitGone();
    for (const w of all) this.#kill(w, "shutdown");
    await waitGone();
    // Their directories are gone when `close` returns (H2).
    while (this.#removals.size > 0) await Promise.allSettled([...this.#removals]);
  }
}
