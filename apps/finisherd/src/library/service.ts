// The library service (plan 5D Task 10; Decisions 17, 18, 28–33, 50): each library's state
// (`online → scanning → online`, `offline` with a reason ↔ re-probes, `paused`), what starts
// its scans (add, schedule, demand, watch, mount), demand-driven polling, local watches, and
// the `library.scan` journal events. It owns no catalog: every catalog statement runs in the
// indexer or reader thread; it submits jobs on the shared engine, reads job outcomes from
// `JobEngine.onDone`, and keeps the `libraries` rows (state, generations, poll times, volume).
//
// - **Scheduling:** the pure planner (`plan.ts`) runs at each wake-up and whenever something
//   changes (a job ended, a library came into use, a watch degraded); one timer waits for the
//   earliest next action.
// - **Offline** (Decision 18): a scan or probe that found the root unusable (`LIBRARY_OFFLINE`,
//   with the root state the worker saw), a root listing found empty while the catalog holds
//   files (`ROOT_EMPTY_SUSPECT`), or a stuck pool (`RESOURCE_STUCK`) puts the library offline:
//   the catalog's offline flag is set (the diff then marks nothing missing) and the root is
//   re-probed every 60 s (fin-node's per-root thread, ≤ 60 s for a spinning-up disk). An
//   `online` answer brings the library back and starts a full scan (`trigger: mount`); a
//   volume that changed (`ROOT_VOLUME_CHANGED`), an unmounted root or an empty one keep it
//   offline until the owner accepts (`acceptVolume`) or the root returns (`offline.ts`).
//   Offline, nothing is submitted and the watch stops; online again, it restarts.
// - **Targeted scans** go through `DirsQueue` (one queued per library, merged).
// - **Volume identity** (Decisions 15, 17): refreshed from each successful full scan
//   (`LibraryScanResult.identity`) and from an `online` re-probe (a silent key change the
//   probe confirmed); a new key moves the library's pools.
import type { LibraryScanResult, RawRel, RootState, Timing, VolumeIdentity } from "@finisher/core-types";
import { FinError, type JobView, type Priority } from "@finisher/contracts";
import { Effect } from "effect";
import type { JobEngine } from "../jobs/api.ts";
import type { FileEvent, Libraries, LibraryView, LibraryWarning, LastScan, Located, StageSpec, StageState } from "./api.ts";
import type { CatalogCounts, Catalogs } from "./catalog/client.ts";
import { type ApplyCounts, emptyCounts } from "./catalog/core/apply.ts";
import { FileEvents, type JournalLike, type ScanMode, batchEvents, journaled, libraryScanPayload, probedEvents, recordScan } from "./events.ts";
import type { RootHealth } from "./health.ts";
import { scanParams } from "./kind-params.ts";
import type { LibraryKindEvents } from "./kinds.ts";
import { DirsQueue } from "./dirs-queue.ts";
import { goOffline, goOnline, recheck, refreshVolume } from "./offline.ts";
import { DAY_MS, type PlanLibrary, type WatchState, plan } from "./plan.ts";
import { type LocateRef, locate } from "./locate.ts";
import { type LibraryPools, LibraryPoolBook } from "./pools.ts";
import { StagePump, submitHash } from "./pump.ts";
import type { LibraryRow, Registry } from "./registry.ts";
import { SYSTEM_OWNER, type ServiceTimers, WARNING_REASONS, exec, offlineReason, realTimers, toFin } from "./states.ts";
import { StageRegistry } from "./stages.ts";
import { Usage } from "./usage.ts";
import type { WatchEvent } from "./watch.ts";

export { SYSTEM_OWNER, type ServiceTimers, exec, offlineReason, realTimers };

/** A re-probe of an offline root waits this long (spin-up tolerance, Decision 18). */
export const RECHECK_PROBE_MS = 60_000;
/** Missing rows older than this are purged after a full scan (Decision 25). */
export const PURGE_AFTER_MS = 90 * DAY_MS;
const MAX_TIMER_MS = 2 ** 31 - 1;
const ACTIVE = ["queued", "running", "paused"] as const;

export type Trigger = "add" | "schedule" | "demand" | "watch" | "mount";

export interface LibraryServiceDeps {
  readonly registry: Registry;
  readonly catalogs: Pick<Catalogs, "indexer" | "reader" | "onWatch">;
  readonly engine: Pick<JobEngine, "submit" | "get" | "cancel" | "wait" | "definePool" | "onDone" | "list">;
  readonly health: RootHealth;
  readonly journal?: JournalLike;
  readonly clock?: () => number;
  readonly timers?: ServiceTimers;
  readonly log?: (level: "debug" | "warning", message: string) => void;
  /** Scaled walker deadlines for scans (test builds of `fin-workers` only). */
  readonly timing?: Timing;
  /** A library with an unusable catalog is offline with this reason (`CATALOG_TOO_NEW`, …). */
  readonly onCatalogError?: (lib: string, e: FinError) => void;
}

interface ScanJob {
  readonly mode: ScanMode;
  readonly trigger: Trigger;
  readonly dirs?: ReadonlyArray<RawRel>;
  readonly gen: number;
  readonly startedAt: number;
  readonly applied: ApplyCounts;
}

export interface Live {
  readonly jobs: Map<string, ScanJob>;
  /** Submits in flight, by pool kind. */
  readonly submitting: Set<"full" | "poll">;
  watch: WatchState;
  watchReason?: string;
  lastFullTry?: number;
  lastRecheckAt: number | undefined;
  recheck: boolean;
  noChangePolls: number;
  counts?: CatalogCounts;
  lastScan?: LastScan;
}

export class LibraryService implements Libraries {
  readonly registry: Registry;
  readonly book: LibraryPoolBook;
  readonly usage: Usage;
  /** The stage framework (Task 11). */
  readonly stages: StagePump;
  /** Targeted scans: one queued per library (`dirs-queue.ts`). */
  readonly dirsQueue: DirsQueue;
  readonly #d: LibraryServiceDeps;
  readonly #clock: () => number;
  readonly #timers: ServiceTimers;
  readonly #live = new Map<string, Live>();
  readonly #files: FileEvents;
  /** Libraries whose catalog is open. */
  readonly #attached = new Set<string>();
  readonly #unsubscribe: () => void;
  #timer: (() => void) | undefined;
  #ticking = false;
  #again = false;
  #closed = false;

  constructor(deps: LibraryServiceDeps) {
    this.#d = deps;
    this.registry = deps.registry;
    this.#clock = deps.clock ?? Date.now;
    this.#timers = deps.timers ?? realTimers;
    this.book = new LibraryPoolBook((name, size) => deps.engine.definePool(name, size));
    this.usage = new Usage(this.#clock, () => this.replan());
    this.#files = new FileEvents((m) => this.log("warning", m));
    this.stages = new StagePump({
      registry: deps.registry, catalogs: deps.catalogs, engine: deps.engine, book: this.book, stages: new StageRegistry(), files: this.#files,
      clock: this.#clock, timers: this.#timers, log: (level, m) => this.log(level, m), attached: (id) => this.#attached.has(id),
      offline: (lib, job) => this.kindEvents.offline?.(lib, job),
      rescan: (lib, fileIds) => void this.#rescanFiles(lib, fileIds),
    });
    this.dirsQueue = new DirsQueue({
      get: (job) => exec(deps.engine.get(job)).catch(() => undefined),
      cancel: (job) => exec(deps.engine.cancel(job)).then(() => undefined, () => undefined),
      submit: (lib, req) => this.#submit(lib, req),
    });
    this.#unsubscribe = deps.engine.onDone((job) => {
      this.stages.jobDone(job);
      this.#jobDone(job);
    });
    deps.catalogs.onWatch((e) => this.#watchEvent(e));
  }

  log(level: "debug" | "warning", message: string): void {
    this.#d.log?.(level, message);
  }

  /** The service's dependencies (`admin.ts`, `fresh.ts`). */
  get deps(): LibraryServiceDeps {
    return this.#d;
  }

  /** `Libraries.onFile`'s bus (`admin.ts`, the pump). */
  get fileEvents(): FileEvents {
    return this.#files;
  }

  now(): number {
    return this.#clock();
  }

  get closed(): boolean {
    return this.#closed;
  }

  /** What the library kinds report (wire into `registerLibraryKinds`' `events`). */
  readonly kindEvents: LibraryKindEvents = this.#kindEvents();

  #kindEvents(): LibraryKindEvents {
    return {
      batchApplied: (lib, job, applied) => {
        const sj = this.#live.get(lib)?.jobs.get(job);
        if (sj !== undefined) for (const k of Object.keys(sj.applied) as Array<keyof ApplyCounts>) sj.applied[k] += applied[k];
        const events = batchEvents(lib, applied.files);
        for (const e of events) this.#files.emit(e);
        if (events.some((e) => e.type !== "missing")) this.stages.kick(lib);
      },
      probed: (lib, _job, applied) => {
        for (const e of probedEvents(lib, applied)) this.#files.emit(e);
        if ((applied.probed ?? []).length > 0) this.stages.kick(lib);
      },
      rescan: (lib, dirs) => void this.submitScan(lib, { mode: "dirs", dirs, trigger: "demand" }),
      offline: (lib, job) => {
        const state = (job.error?.details as { state?: RootState } | undefined)?.state;
        void this.goOffline(lib, offlineReason(state), state?.state ?? "unreachable", false);
      },
    };
  }

  /** Opens every catalog, defines pools, adopts running scans, starts watches and the planner. */
  async load(): Promise<void> {
    for (const row of this.registry.list()) await this.attach(row);
    this.replan();
  }

  /**
   * Brings one library under the service (load and add). A catalog created or rebuilt for a
   * library already known (deleted, damaged, foreign) gets a full scan; a new library's first
   * scan is its caller's (`trigger: add`). Answers whether the catalog was new or rebuilt.
   */
  async attach(row: LibraryRow, opts: { readonly isNew?: boolean } = {}): Promise<{ rebuilt: boolean }> {
    const live = this.liveOf(row.id);
    this.book.add(row);
    let rebuilt = false;
    try {
      const info = await exec(this.#d.catalogs.indexer.open(row.id));
      rebuilt = info.rebuilt !== undefined || info.created;
    } catch (e) {
      const err = toFin(e);
      this.#d.onCatalogError?.(row.id, err);
      this.registry.setState(row.id, "offline", err.code === "CATALOG_TOO_NEW" ? "CATALOG_TOO_NEW" : "CATALOG_UNAVAILABLE");
      this.log("warning", `library service: library ${row.id}'s catalog did not open: ${err.code}`);
      return { rebuilt: false };
    }
    for (const job of this.#d.engine.list({ kind: "library.scan", library: row.id, state: [...ACTIVE], limit: 500 }).jobs) {
      const p = job.params as { mode: ScanMode; gen: number };
      live.jobs.set(job.id, { mode: p.mode, trigger: "schedule", gen: p.gen, startedAt: job.createdAt, applied: emptyCounts() });
    }
    if (row.state === "scanning" && !this.#running(row.id, "full")) this.registry.setState(row.id, "online");
    if (row.state === "offline") this.#d.health.note(row.id, "unreachable");
    await this.#refreshCounts(row.id);
    await this.startWatch(row);
    this.#attached.add(row.id);
    void this.stages.attach(row.id).catch((e) => this.log("warning", `library service: library ${row.id}'s stages: ${toFin(e).code}`));
    if (rebuilt && opts.isNew !== true && row.state !== "paused") void this.submitScan(row.id, { mode: "full", trigger: "schedule" });
    return { rebuilt };
  }

  /** The service's memory of `id` (`offline.ts`). */
  liveOf(id: string): Live {
    let l = this.#live.get(id);
    if (l === undefined) {
      l = { jobs: new Map(), submitting: new Set(), watch: "none", lastRecheckAt: undefined, recheck: false, noChangePolls: 0 };
      this.#live.set(id, l);
    }
    return l;
  }

  /** Watches a local root (Decision 30); other roots are polled. */
  async startWatch(row: LibraryRow): Promise<void> {
    const live = this.liveOf(row.id);
    if (row.storage !== "local" || live.watch !== "none") return;
    try {
      const r = await exec(this.#d.catalogs.indexer.watch(row.id, row.root));
      if (r.ok) live.watch = "ok";
      else this.#degrade(row.id, r.reason);
    } catch (e) {
      this.#degrade(row.id, toFin(e).code);
    }
  }

  #degrade(id: string, reason: string): void {
    const live = this.liveOf(id);
    live.watch = "degraded";
    live.watchReason = reason;
    this.log("warning", `library service: library ${id}'s watch degraded (${reason}); polling every 5 minutes`);
    this.replan();
  }

  #watchEvent(e: WatchEvent): void {
    if (this.#closed) return;
    if (e.type === "watchDegraded") return this.#degrade(e.lib, e.reason);
    const row = this.registry.get(e.lib);
    if (row === undefined || row.state === "paused" || row.state === "offline") return;
    if (e.all) void this.submitScan(e.lib, { mode: "poll", trigger: "watch" });
    else void this.submitScan(e.lib, { mode: "dirs", dirs: e.dirs as RawRel[], trigger: "watch" });
  }

  #running(id: string, kind: "full" | "poll"): boolean {
    const live = this.#live.get(id);
    if (live === undefined) return false;
    if (live.submitting.has(kind)) return true;
    // Targeted scans (`dirs`) share the poll pool but are not deduplicated: they never block a poll.
    for (const j of live.jobs.values()) if (j.mode === kind) return true;
    return false;
  }

  /** Plans again soon (coalesced; never re-entrant). */
  replan(): void {
    if (this.#closed) return;
    if (this.#ticking) {
      this.#again = true;
      return;
    }
    this.tick();
  }

  /** Runs the planner now and acts on it; sets the timer for the next action. */
  tick(): void {
    if (this.#closed) return;
    this.#ticking = true;
    try {
      do {
        this.#again = false;
        const now = this.#clock();
        const libs = this.registry.list().map((row) => this.#planLibrary(row));
        let next = Infinity;
        for (const a of plan(libs, now, (id) => this.usage.inUse(id, now))) {
          next = Math.min(next, a.next);
          if (a.action === "poll") void this.submitScan(a.library, { mode: "poll", trigger: "schedule" });
          else if (a.action === "full") void this.submitScan(a.library, { mode: "full", trigger: "schedule" });
          else if (a.action === "recheck") void this.recheck(a.library);
        }
        this.#schedule(next, now);
      } while (this.#again);
    } finally {
      this.#ticking = false;
    }
  }

  #schedule(next: number, now: number): void {
    this.#timer?.();
    this.#timer = undefined;
    if (!Number.isFinite(next)) return;
    this.#timer = this.#timers.after(Math.min(MAX_TIMER_MS, Math.max(0, next - now)), () => this.tick());
  }

  #planLibrary(row: LibraryRow): PlanLibrary {
    const live = this.liveOf(row.id);
    return {
      id: row.id,
      storage: row.storage,
      state: row.state,
      scan: row.scan,
      createdAt: row.createdAt,
      ...(row.lastPollAt === undefined ? {} : { lastPollAt: row.lastPollAt }),
      ...(row.lastFullAt === undefined ? {} : { lastFullAt: row.lastFullAt }),
      ...(live.lastFullTry === undefined ? {} : { lastFullTry: live.lastFullTry }),
      ...(live.lastRecheckAt === undefined ? {} : { lastRecheckAt: live.lastRecheckAt }),
      watch: live.watch,
      running: { full: this.#running(row.id, "full"), poll: this.#running(row.id, "poll"), recheck: live.recheck },
    };
  }

  /**
   * Submits a scan of `id` (owner `system`; a full scan allocates a generation). Full scans and
   * polls are deduplicated per library by the kind (`dedupe`), so a due poll joins a queued
   * one; targeted scans by `DirsQueue` (one queued per library, merged). Nothing is submitted
   * for a paused or offline library (a re-probe brings it back). Answers the job, or
   * `undefined` when nothing was submitted.
   */
  async submitScan(
    id: string,
    req: { readonly mode: ScanMode; readonly trigger: Trigger; readonly dirs?: ReadonlyArray<RawRel>; readonly priority?: Priority },
  ): Promise<JobView | undefined> {
    const row = this.registry.get(id);
    if (row === undefined || this.#closed || row.state === "paused" || row.state === "offline") return undefined;
    if (req.mode !== "dirs") return this.#submit(id, req);
    if ((req.dirs ?? []).length === 0) return undefined;
    return this.dirsQueue.request(id, { dirs: req.dirs ?? [], trigger: req.trigger, ...(req.priority === undefined ? {} : { priority: req.priority }) });
  }

  async #submit(
    id: string,
    req: { readonly mode: ScanMode; readonly trigger: Trigger; readonly dirs?: ReadonlyArray<RawRel>; readonly priority?: Priority },
  ): Promise<JobView | undefined> {
    const row = this.registry.get(id);
    if (row === undefined || this.#closed || row.state === "paused" || row.state === "offline") return undefined;
    const live = this.liveOf(id);
    if (req.mode === "full") {
      // One full scan at a time: the one queued or running answers (it keeps its generation).
      const running = [...live.jobs].find(([, j]) => j.mode === "full")?.[0];
      if (running !== undefined) return exec(this.#d.engine.get(running)).catch(() => undefined);
    }
    const slot = req.mode === "full" ? "full" : "poll";
    const tracked = req.mode !== "dirs";
    if (tracked) live.submitting.add(slot);
    try {
      const gen = req.mode === "full" ? this.registry.nextGen(id) : row.gen;
      if (req.mode === "full") live.lastFullTry = this.#clock();
      const params = scanParams(row, { mode: req.mode, gen, ...(req.dirs === undefined ? {} : { dirs: req.dirs }), ...(this.#d.timing === undefined ? {} : { timing: this.#d.timing }) });
      const view = await exec(
        this.#d.engine.submit(SYSTEM_OWNER, { kind: "library.scan", params, library: id, ...(req.priority === undefined ? {} : { priority: req.priority }) }),
      );
      if (!live.jobs.has(view.id) && view.finishedAt === undefined) {
        const p = view.params as { gen: number };
        live.jobs.set(view.id, { mode: req.mode, trigger: req.trigger, ...(req.dirs === undefined ? {} : { dirs: req.dirs }), gen: p.gen, startedAt: this.#clock(), applied: emptyCounts() });
        if (req.mode === "full" && this.registry.get(id)?.state === "online") this.registry.setState(id, "scanning");
        // Its end may have come before it was recorded (and found nothing to clear).
        const now = await exec(this.#d.engine.get(view.id)).catch(() => undefined);
        const fullLeft = () => [...live.jobs.values()].some((j) => j.mode === "full");
        if (now?.finishedAt !== undefined && live.jobs.delete(view.id) && this.registry.get(id)?.state === "scanning" && !fullLeft()) {
          this.registry.setState(id, "online");
        }
      }
      return view;
    } catch (e) {
      const err = toFin(e);
      this.log("warning", `library service: a ${req.mode} scan of library ${id} was not submitted: ${err.code}`);
      if (err.code === "RESOURCE_STUCK") void this.goOffline(id, "RESOURCE_STUCK", "unreachable", false);
      return undefined;
    } finally {
      if (tracked) live.submitting.delete(slot);
      this.replan();
    }
  }

  #jobDone(job: JobView): void {
    if (this.#closed || job.kind !== "library.scan") return;
    const id = job.library ?? (job.params as { library?: string } | null)?.library;
    if (id === undefined) return;
    void this.#scanDone(id, job).catch((e) => this.log("warning", `library service: a scan's end was not recorded: ${toFin(e).code}`));
  }

  async #scanDone(id: string, job: JobView): Promise<void> {
    const live = this.#live.get(id);
    const p = job.params as { mode: ScanMode; gen: number };
    const sj = live?.jobs.get(job.id) ?? { mode: p.mode, trigger: "schedule" as const, gen: p.gen, startedAt: job.createdAt, applied: emptyCounts() };
    live?.jobs.delete(job.id);
    const row = this.registry.get(id);
    if (row === undefined || live === undefined) return;
    const now = this.#clock();
    const outcome = job.state as "succeeded" | "failed" | "cancelled" | "interrupted";
    const code = job.error?.code;
    const merged = this.dirsQueue.merged(job.id);
    if (outcome === "succeeded") {
      this.#d.health.note(id, "online");
      if (sj.mode === "full") {
        this.registry.touch(id, { lastFullAt: now, lastPollAt: now });
        this.refreshVolume(row, (job.result as LibraryScanResult | undefined)?.identity);
        void exec(this.#d.catalogs.indexer.purgeMissing(id, { before: now - PURGE_AFTER_MS, beforeGen: sj.gen - 1 })).catch(() => undefined);
        // A full scan after an accepted volume went through: no more mass-drop guard.
        await exec(this.#d.catalogs.indexer.setDropGuard(id, false)).catch(() => undefined);
      } else if (sj.mode === "poll") this.registry.touch(id, { lastPollAt: now });
      else this.dirsQueue.listed(id, sj.dirs ?? (job.params as { dirs?: RawRel[] }).dirs ?? [], now);
    } else if (code === "ROOT_EMPTY_SUSPECT") await this.goOffline(id, "ROOT_EMPTY_SUSPECT", "empty", false);
    else if (code === "ROOT_DROP_SUSPECT") await this.goOffline(id, "ROOT_DROP_SUSPECT", "unreachable", false);
    if (this.registry.get(id)?.state === "scanning" && !this.#running(id, "full")) this.registry.setState(id, "online");
    await exec(
      this.#d.catalogs.indexer.finishScan(id, {
        jobId: job.id, mode: sj.mode, trigger: sj.trigger, gen: sj.gen, startedAt: sj.startedAt, finishedAt: Math.max(now, sj.startedAt), outcome,
        ...(code === undefined ? {} : { code }), counts: { ...sj.applied },
      }),
    ).catch((e) => this.log("warning", `library service: finishScan failed: ${toFin(e).code}`));
    await this.#refreshCounts(id);
    if (merged) return this.replan(); // cancelled into a later targeted scan: not a scan of its own
    live.lastScan = { at: now, mode: sj.mode, outcome, ...(code === undefined ? {} : { code }) };
    if (journaled(sj.mode, outcome, sj.applied)) {
      const payload = libraryScanPayload({
        storage: row.storage, mode: sj.mode, trigger: sj.trigger, outcome, ...(code === undefined ? {} : { code }), applied: sj.applied,
        ...(live.counts === undefined ? {} : { counts: live.counts }), micros: Math.max(0, now - sj.startedAt) * 1000,
      });
      recordScan(this.#d.journal, payload, (m) => this.log("warning", m));
    } else live.noChangePolls++;
    this.replan();
  }

  /** Lists again the directories of files a stage job found changed, gone or cloud-only. */
  async #rescanFiles(lib: string, fileIds: ReadonlyArray<number>): Promise<void> {
    const { lines } = await exec(this.#d.catalogs.reader.fileLines(lib, { fileIds: [...fileIds], hydrate: true })).catch(() => ({ lines: [] }));
    const dirs = new Map(lines.map((l) => [l.rel.components.slice(0, -1).join("/"), { components: l.rel.components.slice(0, -1) }]));
    if (dirs.size > 0) await this.submitScan(lib, { mode: "dirs", dirs: [...dirs.values()], trigger: "demand" });
  }

  async #refreshCounts(id: string): Promise<void> {
    try {
      this.liveOf(id).counts = await exec(this.#d.catalogs.reader.counts(id));
    } catch {
      // A catalog that is not attached keeps its last counts.
    }
  }

  /** Stores a volume measured now (a full scan, an online re-probe); a new key moves the pools. */
  refreshVolume(row: LibraryRow, identity: VolumeIdentity | undefined): void {
    refreshVolume(this, row, identity);
  }

  /** Puts `id` offline with `reason` (Decision 18); `probed`: the root state comes from a probe just now. */
  goOffline(id: string, reason: string, state: RootState["state"], probed: boolean): Promise<void> {
    return goOffline(this, id, reason, state, probed);
  }

  /** The root is usable again (`offline.ts`). */
  goOnline(id: string): Promise<void> {
    return goOnline(this, id);
  }

  /** Re-probes an offline library's root (≤ 60 s, Decision 18). */
  recheck(id: string): Promise<void> {
    return recheck(this, id);
  }

  /** The service-side view of a library (Task 13 adds stages). */
  view(row: LibraryRow): LibraryView {
    const live = this.liveOf(row.id);
    const warnings: LibraryWarning[] = [];
    if (row.offlineReason !== undefined && WARNING_REASONS.has(row.offlineReason)) warnings.push({ code: row.offlineReason });
    if (live.watch === "degraded") warnings.push({ code: "WATCH_DEGRADED" });
    if ((live.counts?.collisions ?? 0) > 0) warnings.push({ code: "NAME_COLLISION", count: live.counts!.collisions });
    const { rootKey: _k, volume: _v, storageDetected: _d, storageOverride: _o, gen: _g, createdBy: _c, lastFullAt: _f, ...shown } = row;
    return { ...shown, counts: { ...(live.counts?.files ?? {}) }, warnings, ...(live.lastScan === undefined ? {} : { lastScan: live.lastScan }) };
  }

  /** What `library.status` shows beyond the view (Task 13 adds stages). */
  status(id: string): { readonly jobs: string[]; readonly watch: WatchState; readonly watchReason?: string; readonly noChangePolls: number; readonly inUse: boolean; readonly holders: number } {
    const live = this.liveOf(id);
    return {
      jobs: [...live.jobs.keys()],
      watch: live.watch,
      ...(live.watchReason === undefined ? {} : { watchReason: live.watchReason }),
      noChangePolls: live.noChangePolls,
      inUse: this.usage.inUse(id),
      holders: this.usage.holders(id),
    };
  }

  /** Forgets a removed library (its jobs are cancelled and its catalog closed by the caller). */
  forget(id: string): void {
    this.#attached.delete(id);
    this.stages.forget(id);
    this.dirsQueue.forget(id);
    this.#live.delete(id);
    this.usage.forget(id);
    this.#d.health.forget(id);
    this.book.remove(id);
  }

  // The `Libraries` API (Task 10's members).

  list(): ReadonlyArray<LibraryView> {
    return this.registry.list().map((r) => this.view(r));
  }

  get(id: string): Effect.Effect<LibraryView, FinError> {
    return Effect.suspend(() => {
      const row = this.registry.get(id);
      return row === undefined ? Effect.fail(new FinError({ code: "LIBRARY_NOT_FOUND", message: "there is no such library", details: { id } })) : Effect.succeed(this.view(row));
    });
  }

  pools(id: string): LibraryPools {
    const pools = this.book.pools(id);
    if (pools === undefined) throw new FinError({ code: "LIBRARY_NOT_FOUND", message: "there is no such library", details: { id } });
    return pools;
  }

  online(id: string): Effect.Effect<boolean> {
    return Effect.suspend(() => {
      const row = this.registry.get(id);
      if (row === undefined || row.state === "offline") return Effect.succeed(false);
      return this.#d.health.online(id);
    });
  }

  use(ids: ReadonlyArray<string>): { release(): void } {
    return this.usage.use(ids);
  }

  hash(library: string, fileIds: ReadonlyArray<number>): Effect.Effect<JobView, FinError> {
    return submitHash({ registry: this.registry, engine: this.#d.engine, book: this.book }, library, fileIds);
  }

  onFile(listener: (e: FileEvent) => void): () => void {
    return this.#files.on(listener);
  }

  // Task 11's members: locate and the stage framework.

  locate(ref: LocateRef, reach?: (library: string) => boolean): Effect.Effect<Located | undefined, FinError> {
    return Effect.tryPromise({
      try: () => locate({ registry: this.registry, reader: this.#d.catalogs.reader, attached: (id) => this.#attached.has(id), ...(reach === undefined ? {} : { reach }) }, ref),
      catch: toFin,
    });
  }

  registerStage(spec: StageSpec): void {
    this.stages.register(spec);
  }

  prioritize(fp: string, stage: string): Effect.Effect<void, FinError> {
    return this.stages.prioritize(fp, stage);
  }

  stageStates(fp: string): Effect.Effect<ReadonlyArray<StageState>> {
    return Effect.promise(() => this.stages.stageStates(fp).catch(() => []));
  }

  /** Stops the planner and stops listening; the engine and catalogs close after it. */
  close(): void {
    this.#closed = true;
    this.stages.close();
    this.#timer?.();
    this.#timer = undefined;
    this.#unsubscribe();
  }
}
