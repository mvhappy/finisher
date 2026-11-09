// The stage pump (plan 5D Decisions 39–41): per library and per stage it keeps up to
// `maxInFlight` (default 2) jobs queued or running, owner `system`, at the stage's priority
// (default `background`). Pending = present, fingerprinted files the stage applies to whose
// dependencies are `done`, without a row at this version or failed with attempts left an hour
// ago (`stage-rows.ts`); the built-in `probe` stage takes present files never probed at this
// `PROBE_VERSION` (`library.probe`, through `submitProbe`: a metadata-only library records
// them `skipped` instead).
//
// - It stops while the library is offline or paused, and cancels the library's jobs when it
//   goes so (`stop`): their rows return to pending without spending an attempt.
// - A server-wide in-flight set (`inflight.ts`) keeps one fingerprint out of two jobs of one
//   stage, across libraries: a library that finds one in another's job waits on it, and the
//   job's end settles both rows. A job's end is held until its rows say `queued`, and the
//   engine is asked once they do, so a job that ended before it was recorded still settles.
// - Items whose file is not what the catalog says cost no attempt; their directories are
//   listed again (`rescan`). A prioritized stage whose dependency failed for good is dropped.
// - `isDone(fp)` marks a row `done` without a job; `appliesTo` false marks it `skipped`.
// - Completion comes from `JobEngine.onDone` (`stage-outcome.ts`); at attach the catalog's
//   `queued`/`running` rows are reconciled with `engine.get(jobId)`.
// - `prioritize(fp, stage)` submits the fingerprint alone at `normal` priority, after its
//   dependencies the same way (Decision 41).
import type { JobView, Priority } from "@finisher/contracts";
import { FinError } from "@finisher/contracts";
import { libraryProbeVersion } from "@finisher/fin-node";
import { Effect } from "effect";
import type { JobEngine } from "../jobs/api.ts";
import type { StageFile, StageSpec, StageState } from "./api.ts";
import type { Catalogs } from "./catalog/client.ts";
import type { StagePendingFile } from "./catalog/core/stage-rows.ts";
import type { FileEvents } from "./events.ts";
import { type JobClaim, InFlight } from "./inflight.ts";
import { PROBE_BATCH, hashParams, probeExtensions, probeParams } from "./kind-params.ts";
import type { LibraryPoolBook } from "./pools.ts";
import type { LibraryRow, Registry } from "./registry.ts";
import { itemFates, rescanItems, rowOf } from "./stage-outcome.ts";
import { DEFAULT_MAX_IN_FLIGHT, MAX_ATTEMPTS, PROBE_STAGE, RETRY_AFTER_MS, type StageRegistry, stageFile } from "./stages.ts";
import { SYSTEM_OWNER, type ServiceTimers, exec, toFin } from "./states.ts";

/** Every library is pumped again this often (failed rows' retry hour, missed kicks). */
export const SWEEP_MS = 5 * 60_000;
/** Submits per stage per pump turn (a turn ends; the next kick goes on). */
const ROUNDS = 8;

export interface PumpDeps {
  readonly registry: Registry;
  readonly catalogs: Pick<Catalogs, "indexer" | "reader">;
  readonly engine: Pick<JobEngine, "submit" | "get" | "cancel" | "list">;
  readonly book: LibraryPoolBook;
  readonly stages: StageRegistry;
  readonly files: FileEvents;
  readonly clock: () => number;
  readonly timers: ServiceTimers;
  readonly log: (level: "debug" | "warning", message: string) => void;
  /** The library's catalog is open (the service attached it). */
  readonly attached: (id: string) => boolean;
  /** A stage job of another family ended `LIBRARY_OFFLINE` (Decision 33): the service puts the library offline. */
  readonly offline?: (lib: string, job: JobView) => void;
  /** Files a stage job found changed, gone or cloud-only: the service lists their directories again. */
  readonly rescan?: (lib: string, fileIds: ReadonlyArray<number>) => void;
}

const wantKey = (fp: string, stage: string): string => `${stage}\u0000${fp}`;

const notFound = (fp: string): FinError => new FinError({ code: "FILE_NOT_FOUND", message: "no library holds a file with this fingerprint", details: { fp } });

/**
 * A `library.probe` job of up to 256 catalogued files of `row`. A metadata-only library
 * (Decision 14) opens nothing: the files are recorded `skipped` with `METADATA_ONLY` (and
 * reported as `probed`), and no job is submitted.
 */
export async function submitProbe(
  d: Pick<PumpDeps, "catalogs" | "engine" | "book" | "files">,
  row: LibraryRow,
  fileIds: ReadonlyArray<number>,
  priority?: Priority,
): Promise<{ job?: JobView; skipped: number }> {
  const params = probeParams(row, fileIds, d.book.readSlots(row.id));
  if (params !== undefined) {
    const job = await exec(d.engine.submit(SYSTEM_OWNER, { kind: "library.probe", params, library: row.id, ...(priority === undefined ? {} : { priority }) }));
    return { job, skipped: 0 };
  }
  const { lines } = await exec(d.catalogs.reader.fileLines(row.id, { fileIds: [...fileIds], hydrate: true }));
  const version = libraryProbeVersion();
  const rows = lines.map((l) => ({ fileId: l.id, size: l.expect.size, mtimeNs: l.expect.mtimeNs, probeVersion: version, state: "skipped" as const, code: "METADATA_ONLY" }));
  if (rows.length === 0) return { skipped: 0 };
  const applied = await exec(d.catalogs.indexer.applyProbed(row.id, rows));
  for (const p of applied.probed ?? []) d.files.emit({ type: "probed", library: row.id, fileId: p.fileId, ...(p.fp === null ? {} : { fp: p.fp }) });
  return { skipped: applied.updated };
}

/** A `library.hash` job of up to 64 catalogued files (Decision 38); `LIBRARY_METADATA_ONLY` for a metadata-only library. */
export function submitHash(
  d: { readonly registry: Pick<Registry, "get">; readonly engine: Pick<JobEngine, "submit">; readonly book: LibraryPoolBook },
  library: string,
  fileIds: ReadonlyArray<number>,
): Effect.Effect<JobView, FinError> {
  return Effect.suspend(() => {
    const row = d.registry.get(library);
    if (row === undefined) return Effect.fail(new FinError({ code: "LIBRARY_NOT_FOUND", message: "there is no such library", details: { id: library } }));
    const params = Effect.try({ try: () => hashParams(row, fileIds, d.book.readSlots(library)), catch: (e) => new FinError({ code: "INVALID_REQUEST", message: String((e as Error).message), details: {} }) });
    return Effect.flatMap(params, (p) =>
      p === undefined
        ? Effect.fail(new FinError({ code: "LIBRARY_METADATA_ONLY", message: "the library is catalogued from metadata only", details: { id: library } }))
        : d.engine.submit(SYSTEM_OWNER, { kind: "library.hash", params: p, library }),
    );
  });
}

export class StagePump {
  readonly inflight = new InFlight();
  readonly #d: PumpDeps;
  /** `stage\0fp` the caller prioritized, waiting for a dependency. */
  readonly #wanted = new Map<string, { fp: string; stage: string }>();
  readonly #turns = new Map<string, { again: boolean }>();
  #sweep: (() => void) | undefined;
  #reservations = 0;
  #closed = false;

  constructor(deps: PumpDeps) {
    this.#d = deps;
  }

  /** Armed once a stage is registered: the built-in probe stage needs no retry hour. */
  #armSweep(): void {
    if (this.#closed || this.#sweep !== undefined) return;
    this.#sweep = this.#d.timers.after(SWEEP_MS, () => {
      this.#sweep = undefined;
      for (const row of this.#d.registry.list()) this.kick(row.id);
      this.#armSweep();
    });
  }

  close(): void {
    this.#closed = true;
    this.#sweep?.();
  }

  /** Pumps `id` soon (coalesced: one turn per library at a time, one more if kicked meanwhile). */
  kick(id: string): void {
    if (this.#closed) return;
    const turn = this.#turns.get(id);
    if (turn !== undefined) {
      turn.again = true;
      return;
    }
    const t = { again: false };
    this.#turns.set(id, t);
    void (async () => {
      try {
        do {
          t.again = false;
          await this.#turn(id);
        } while (t.again && !this.#closed);
      } catch (e) {
        this.#d.log("warning", `stage pump: library ${id}: ${toFin(e).code}`);
      } finally {
        this.#turns.delete(id);
      }
    })();
  }

  /** Prioritized fingerprints waiting for a dependency (tests). */
  wanted(): number {
    return this.#wanted.size;
  }

  /** Resolves once no turn runs (tests). */
  async idle(): Promise<void> {
    while (this.#turns.size > 0) await new Promise((r) => setImmediate(r));
  }

  #usable(id: string): LibraryRow | undefined {
    const row = this.#d.registry.get(id);
    if (row === undefined || row.state === "offline" || row.state === "paused" || !this.#d.attached(id)) return undefined;
    return row;
  }

  async #turn(id: string): Promise<void> {
    if (this.#usable(id) === undefined) return;
    await this.#pumpProbe(id);
    for (const spec of this.#d.stages.forLibrary(this.#usable(id) ?? { stages: [] })) {
      if (this.#usable(id) === undefined) return;
      await this.#pumpStage(id, spec);
    }
  }

  async #pumpProbe(id: string): Promise<void> {
    for (let round = 0; round < ROUNDS; round++) {
      const row = this.#usable(id);
      if (row === undefined || this.inflight.count(id, PROBE_STAGE) >= DEFAULT_MAX_IN_FLIGHT) return;
      const held = this.inflight.jobsOf(id).filter((c) => c.stage === PROBE_STAGE).reduce((n, c) => n + c.items.length, 0);
      const { files } = await exec(
        this.#d.catalogs.indexer.pending(id, { stage: PROBE_STAGE, probeVersion: libraryProbeVersion(), exts: probeExtensions(row.kinds) ?? null, limit: Math.min(1000, PROBE_BATCH + held) }),
      );
      const ids = files.map((f) => f.fileId).filter((f) => !this.inflight.probing(id, f)).slice(0, PROBE_BATCH);
      if (ids.length === 0) return;
      const r = await submitProbe(this.#d, row, ids);
      if (r.job !== undefined && r.job.finishedAt === undefined) {
        this.inflight.add({ jobId: r.job.id, library: id, stage: PROBE_STAGE, version: "", items: ids.map(String) });
        // The job may have ended before it was recorded: its end found no claim.
        await this.#ended(r.job.id);
      } else if (r.job === undefined && r.skipped === 0) return;
    }
  }

  async #pumpStage(id: string, spec: StageSpec, only?: ReadonlyArray<string>, priority?: Priority): Promise<boolean> {
    const deps = this.#d.stages.dependencies(spec);
    if (deps === undefined) return false;
    const max = spec.maxInFlight ?? DEFAULT_MAX_IN_FLIGHT;
    let submitted = false;
    for (let round = 0; round < ROUNDS; round++) {
      const row = this.#usable(id);
      if (row === undefined || (only === undefined && this.inflight.count(id, spec.name) >= max)) return submitted;
      const now = this.#d.clock();
      const { files } = await exec(
        this.#d.catalogs.indexer.pendingStage(id, {
          stage: spec.name, version: spec.version, maxAttempts: MAX_ATTEMPTS, limit: Math.min(1000, spec.batch * 4), dependsOn: deps,
          hydrate: row.hydrate, ...(only === undefined ? { retryBefore: now - RETRY_AFTER_MS } : { fps: only }),
        }),
      );
      if (files.length === 0) return submitted;
      const take: StageFile[] = [];
      const rows: Array<{ fp: string; stage: string; version: string; state: "queued" | "done" | "skipped"; code?: string; jobId?: string; at: number }> = [];
      const done: StageFile[] = [];
      for (const f of files) {
        if (take.length >= spec.batch) break;
        // A prioritized fingerprint goes through `prioritize`, at `normal`, never in a background batch.
        if (only === undefined && this.#wanted.has(wantKey(f.fp, spec.name))) continue;
        const held = this.inflight.stageJob(f.fp, spec.name);
        if (held !== undefined) {
          if (held.library !== id && !held.reserved) {
            this.inflight.wait(held, id, f.fp, f.fileId);
            rows.push({ fp: f.fp, stage: spec.name, version: held.version, state: "queued", jobId: held.jobId, at: now });
          }
          continue;
        }
        const file = stageFile(row, f);
        if (!this.#applies(spec, file)) {
          rows.push({ fp: f.fp, stage: spec.name, version: spec.version, state: "skipped", code: "NOT_APPLICABLE", at: now });
          continue;
        }
        if (spec.isDone !== undefined && (await Effect.runPromise(spec.isDone(f.fp).pipe(Effect.orElseSucceed(() => false))))) {
          rows.push({ fp: f.fp, stage: spec.name, version: spec.version, state: "done", code: "CACHED", at: now });
          done.push(file);
          continue;
        }
        take.push(file);
      }
      let job: string | undefined;
      if (take.length > 0) {
        job = await this.#submit(row, spec, take, priority ?? spec.priority ?? "background");
        if (job === undefined) {
          if (rows.length > 0) await exec(this.#d.catalogs.indexer.stagesSet(id, rows));
          return submitted;
        }
        submitted = true;
        for (const f of take) rows.push({ fp: f.fp, stage: spec.name, version: spec.version, state: "queued", jobId: job, at: now });
      }
      try {
        if (rows.length > 0) await exec(this.#d.catalogs.indexer.stagesSet(id, rows));
      } finally {
        if (job !== undefined) await this.#recorded(job);
      }
      for (const f of done) this.#d.files.emit({ type: "stage", library: id, fileId: f.fileId, fp: f.fp, stage: spec.name });
      if (done.length > 0) this.#dependentsReady(done.map((f) => f.fp), spec.name);
      if (only !== undefined || take.length === 0 && rows.length === 0) return submitted;
    }
    return submitted;
  }

  #applies(spec: StageSpec, file: StageFile): boolean {
    try {
      return spec.appliesTo(file) === true;
    } catch (e) {
      this.#d.log("warning", `stage pump: ${spec.name}.appliesTo threw: ${(e as Error).message}`);
      return false;
    }
  }

  /** Submits one job of `files`; answers its id, or `undefined` when it was not submitted. */
  async #submit(row: LibraryRow, spec: StageSpec, files: ReadonlyArray<StageFile>, priority: Priority): Promise<string | undefined> {
    // Held before the submit, so a prioritize or another library cannot take the same fingerprints meanwhile.
    const reserved = `reserved:${++this.#reservations}`;
    this.inflight.add({ jobId: reserved, library: row.id, stage: spec.name, version: spec.version, items: files.map((f) => f.fp), files: new Map(files.map((f) => [f.fp, f.fileId])), reserved: true });
    try {
      const pools = this.#d.book.pools(row.id);
      if (pools === undefined) throw new FinError({ code: "LIBRARY_NOT_FOUND", message: "the library has no pools", details: { id: row.id } });
      const params = spec.params(files, { library: row.id, pools });
      const view = await exec(this.#d.engine.submit(SYSTEM_OWNER, { kind: spec.jobKind, params, library: row.id, priority }));
      const claim = this.inflight.rename(reserved, view.id);
      // Its end waits until its rows say `queued` (`#recorded`): a quick end must not be overwritten.
      if (claim !== undefined) claim.recording = true;
      if (claim !== undefined && this.#usable(row.id) === undefined) {
        // Paused or offline while the submit ran: cancelled like the library's other jobs.
        claim.free = true;
        await exec(this.#d.engine.cancel(view.id)).catch(() => undefined);
      }
      return view.id;
    } catch (e) {
      this.inflight.release(reserved);
      this.#d.log("warning", `stage pump: a ${spec.name} job of library ${row.id} was not submitted: ${toFin(e).code}`);
      return undefined;
    }
  }

  /** A job's rows are written: an end that came meanwhile, or before its claim had its id, settles now. */
  async #recorded(jobId: string): Promise<void> {
    const claim = this.inflight.get(jobId);
    if (claim === undefined) return;
    claim.recording = false;
    if (claim.ended !== undefined) return this.jobDone(claim.ended);
    await this.#ended(jobId);
  }

  /** Settles `jobId` if the engine says it already ended (its `onDone` came before its claim). */
  async #ended(jobId: string): Promise<void> {
    const view = await exec(this.#d.engine.get(jobId)).catch(() => undefined);
    if (view?.finishedAt !== undefined) this.jobDone(view);
  }

  /** A job ended (wire to `JobEngine.onDone`). */
  jobDone(job: JobView): void {
    const claim = this.inflight.get(job.id);
    if (claim === undefined || this.#closed) return;
    if (claim.recording) {
      claim.ended = job;
      return;
    }
    this.inflight.release(job.id);
    if (claim.stage !== PROBE_STAGE && job.error?.code === "LIBRARY_OFFLINE" && job.family !== "library") this.#d.offline?.(claim.library, job);
    void this.#settle(job, claim).catch((e) => this.#d.log("warning", `stage pump: a job's end was not recorded: ${toFin(e).code}`));
  }

  async #settle(job: JobView, claim: JobClaim): Promise<void> {
    if (claim.stage === PROBE_STAGE) return this.kick(claim.library);
    const fates = itemFates(job, claim.items, claim.free);
    const libs = new Map<string, Map<string, number>>([[claim.library, new Map(claim.files)], ...claim.waiters]);
    const now = this.#d.clock();
    const ready: string[] = [];
    for (const [lib, files] of libs) {
      if (this.#d.registry.get(lib) === undefined || !this.#d.attached(lib)) continue;
      const fps = [...files.keys()].filter((fp) => fates.has(fp));
      if (fps.length === 0) continue;
      const stored = await exec(this.#d.catalogs.reader.stageRows(lib, { fps, stage: claim.stage })).catch(() => ({ rows: [] }));
      const attempts = new Map(stored.rows.map((r) => [r.fp, r.version === claim.version ? r.attempts : 0]));
      const rows = fps.map((fp) => rowOf(fates.get(fp)!, { fp, stage: claim.stage, version: claim.version, at: now }, attempts.get(fp) ?? 0));
      await exec(this.#d.catalogs.indexer.stagesSet(lib, rows));
      const behind = rescanItems(fates).filter((fp) => files.has(fp)).map((fp) => files.get(fp)!);
      if (behind.length > 0) this.#d.rescan?.(lib, behind);
      // A prioritized stage waiting on a dependency that failed for good waits no more.
      for (const r of rows) if (r.state === "failed" && r.attempts >= MAX_ATTEMPTS) this.#dropWanted(r.fp, claim.stage);
      for (const fp of fps) {
        if (fates.get(fp)?.fate !== "done") continue;
        ready.push(fp);
        this.#d.files.emit({ type: "stage", library: lib, fileId: files.get(fp)!, fp, stage: claim.stage });
      }
    }
    this.#dependentsReady(ready, claim.stage);
    for (const lib of libs.keys()) this.kick(lib);
  }

  /** Drops prioritized entries of `fp` for stages that depend on `stage`, directly or not. */
  #dropWanted(fp: string, stage: string): void {
    const after = (s: string, seen = new Set<string>()): boolean => {
      if (seen.has(s)) return false;
      seen.add(s);
      const deps = this.#d.stages.get(s)?.dependsOn ?? [];
      return deps.includes(stage) || deps.some((d) => after(d, seen));
    };
    for (const [key, w] of this.#wanted) if (w.fp === fp && after(w.stage)) this.#wanted.delete(key);
  }

  /** Prioritized stages waiting for `stage` of `fps` go now (their entries stay until `prioritize` submits). */
  #dependentsReady(fps: ReadonlyArray<string>, stage: string): void {
    const set = new Set(fps);
    for (const w of [...this.#wanted.values()]) {
      if (!set.has(w.fp) || !(this.#d.stages.get(w.stage)?.dependsOn ?? []).includes(stage)) continue;
      void Effect.runPromise(Effect.ignore(this.prioritize(w.fp, w.stage)));
    }
  }

  /**
   * Brings a library under the pump (at attach): stale rows of other versions, reconciles its
   * `queued`/`running` rows with the engine, adopts its probe jobs, and pumps it. With
   * `spec`, only that stage (a stage registered after the library attached).
   */
  async attach(id: string, spec?: StageSpec): Promise<void> {
    const row = this.#d.registry.get(id);
    if (row === undefined || !this.#d.attached(id)) return;
    const { indexer, reader } = this.#d.catalogs;
    const specs = spec === undefined ? this.#d.stages.list() : [spec];
    for (const s of specs) await exec(indexer.stagesStale(id, s.name, s.version));
    const open = await exec(reader.stageRows(id, { states: ["queued", "running"], ...(spec === undefined ? {} : { stage: spec.name }) }));
    const known = new Set(specs.map((s) => s.name));
    const byJob = new Map<string, typeof open.rows>();
    for (const r of open.rows) {
      if (!known.has(r.stage)) continue; // reconciled when that stage registers
      const list = byJob.get(r.jobId ?? "") ?? [];
      list.push(r);
      byJob.set(r.jobId ?? "", list);
    }
    for (const [jobId, rows] of byJob) await this.#reconcile(id, jobId, rows);
    if (spec === undefined) {
      for (const job of this.#d.engine.list({ kind: "library.probe", library: id, state: ["queued", "running", "paused"], limit: 500 }).jobs) {
        const items = ((job.params as { items?: unknown } | null)?.items ?? []) as string[];
        this.inflight.add({ jobId: job.id, library: id, stage: PROBE_STAGE, version: "", items });
      }
    }
    this.kick(id);
  }

  async #reconcile(id: string, jobId: string, rows: ReadonlyArray<{ fp: string; stage: string; version: string }>): Promise<void> {
    const fps = rows.map((r) => r.fp);
    const { matches } = await exec(this.#d.catalogs.reader.lookup(id, { fingerprints: fps }));
    const fileOf = new Map<string, number>();
    for (const m of matches) if (!fileOf.has(m.key) || m.file.state !== "missing") fileOf.set(m.key, m.file.id);
    const view = jobId === "" ? undefined : await exec(this.#d.engine.get(jobId)).catch(() => undefined);
    const stage = rows[0]!.stage;
    const version = rows[0]!.version;
    if (view === undefined) {
      // The job is gone: back to pending, no attempt spent.
      const now = this.#d.clock();
      await exec(this.#d.catalogs.indexer.stagesSet(id, fps.map((fp) => ({ fp, stage, version, state: "stale" as const, code: "JOB_LOST", jobId: null, at: now }))));
      return;
    }
    const lib = view.library ?? id;
    let claim = this.inflight.get(jobId);
    if (claim === undefined) claim = this.inflight.add({ jobId, library: lib, stage, version, items: fps, ...(lib === id ? { files: fileOf } : {}) });
    else this.inflight.extend(claim, fps, lib === id ? fileOf : undefined);
    if (lib !== id) for (const fp of fps) this.inflight.wait(claim, id, fp, fileOf.get(fp) ?? 0);
    if (view.finishedAt !== undefined) this.jobDone(view);
  }

  /** The library went offline or was paused: its jobs are cancelled, their rows go back to pending. */
  async stop(id: string): Promise<void> {
    for (const c of this.inflight.jobsOf(id)) {
      c.free = true;
      await exec(this.#d.engine.cancel(c.jobId)).catch(() => undefined);
    }
  }

  /** A removed library: forgets its claims (the caller cancelled its jobs). */
  forget(id: string): void {
    for (const c of this.inflight.jobsOf(id)) this.inflight.release(c.jobId);
    this.inflight.forget(id);
  }

  /** Registers a stage and pumps it in every attached library. */
  register(spec: StageSpec): void {
    this.#d.stages.register(spec);
    this.#armSweep();
    const registered = this.#d.stages.get(spec.name)!;
    for (const row of this.#d.registry.list()) void this.attach(row.id, registered).catch((e) => this.#d.log("warning", `stage pump: ${spec.name} in library ${row.id}: ${toFin(e).code}`));
  }

  /** Decision 41: `stage` of `fp` now, alone, at `normal` priority, after its dependencies. */
  prioritize(fp: string, stage: string): Effect.Effect<void, FinError> {
    return Effect.tryPromise({ try: () => this.#prioritize(fp, stage, new Set()), catch: toFin });
  }

  async #prioritize(fp: string, stage: string, path: Set<string>): Promise<void> {
    if (stage === PROBE_STAGE) return; // a known fingerprint was probed
    const spec = this.#d.stages.get(stage);
    if (spec === undefined) throw new FinError({ code: "INVALID_REQUEST", message: "no such stage", details: { stage } });
    const key = wantKey(fp, stage);
    if (path.has(stage)) return;
    if (this.inflight.stageJob(fp, stage) !== undefined) return void this.#wanted.delete(key);
    path.add(stage);
    const holders = await this.#holders(fp, stage);
    if (holders.length === 0) {
      this.#wanted.delete(key);
      throw notFound(fp);
    }
    const id = holders[0]!;
    const states = await exec(this.#d.catalogs.reader.stageRows(id, { fps: [fp] }));
    const doneAt = (s: string, v: string) => states.rows.some((r) => r.stage === s && r.version === v && r.state === "done");
    if (doneAt(stage, spec.version)) return void this.#wanted.delete(key);
    let waiting = false;
    for (const dep of this.#d.stages.dependencies(spec) ?? []) {
      if (doneAt(dep.stage, dep.version)) continue;
      waiting = true;
      await this.#prioritize(fp, dep.stage, path);
    }
    if (waiting || this.#d.stages.dependencies(spec) === undefined) {
      this.#wanted.set(key, { fp, stage });
      return;
    }
    // Kept until the job is submitted: a pump turn that runs meanwhile (the end of the dependency
    // kicks one) must still see the fingerprint as prioritized, not take it into a background batch.
    try {
      await this.#pumpStage(id, spec, [fp], "normal");
    } finally {
      this.#wanted.delete(key);
    }
  }

  /** Libraries (online, attached, not paused, running `stage`) holding a live file with `fp`. */
  async #holders(fp: string, stage: string): Promise<string[]> {
    const out: string[] = [];
    for (const row of this.#d.registry.list()) {
      if (this.#usable(row.id) === undefined || !this.#d.stages.forLibrary(row).some((s) => s.name === stage)) continue;
      const r = await exec(this.#d.catalogs.reader.lookup(row.id, { fingerprints: [fp] })).catch(() => ({ matches: [] }));
      if (r.matches.some((m) => m.file.state === "present" || (row.hydrate && m.file.state === "cloudOnly"))) out.push(row.id);
    }
    return out;
  }

  /** `Libraries.stageStates`: `fp`'s rows in every attached library holding it, `probe` first. */
  async stageStates(fp: string): Promise<StageState[]> {
    const out: StageState[] = [];
    for (const row of this.#d.registry.list()) {
      if (!this.#d.attached(row.id)) continue;
      const found = await exec(this.#d.catalogs.reader.lookup(row.id, { fingerprints: [fp] })).catch(() => ({ matches: [] }));
      const file = found.matches.find((m) => m.file.state !== "missing")?.file;
      if (file === undefined) continue;
      if (file.probeVersion !== null) out.push({ library: row.id, stage: PROBE_STAGE, version: String(file.probeVersion), state: "done" });
      const { rows } = await exec(this.#d.catalogs.reader.stageRows(row.id, { fps: [fp] })).catch(() => ({ rows: [] }));
      for (const r of rows) out.push({ library: row.id, stage: r.stage, version: r.version, state: r.state, ...(r.code === null ? {} : { code: r.code }) });
    }
    return out;
  }
}

export type { StagePendingFile };
