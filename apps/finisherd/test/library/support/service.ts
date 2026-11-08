// A library service over fakes (plan 5D Task 10): a real registry in a real `control.db`, a
// fake engine that records submits and ends jobs on demand, fake catalog threads, scripted root
// probes and a manual clock with timers. For the scheduler's cadence, offline and accept rules
// without workers; `kinds.test.ts`-style rigs cover the real engine.
import { mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { RootState, VolumeIdentity } from "@finisher/core-types";
import { FinError, type JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { type ControlStore, openControlStore } from "../../../src/control/db.ts";
import type { CatalogCounts } from "../../../src/library/catalog/client.ts";
import { rootHealth } from "../../../src/library/health.ts";
import { type LibraryRow, type NewLibrary, Registry, type RootTools } from "../../../src/library/registry.ts";
import { LibraryService, type LibraryServiceDeps, type ServiceTimers } from "../../../src/library/service.ts";
import type { WatchEvent } from "../../../src/library/watch.ts";

export const ACTOR = { kind: "local", id: "test" } as const;

export function identity(key: string, extra: Partial<VolumeIdentity> = {}): VolumeIdentity {
  return { key, kind: "smbfs", devId: "1", mountRoot: false, caseSensitive: true, enumeration: "fake", remote: true, rootFileId: null, sample: [], ...extra };
}

export interface FakeJob {
  view: JobView;
}

/** A manual clock with timers: `advance(ms)` fires what falls due, in order. */
export class ManualTime implements ServiceTimers {
  now = 1_000_000_000_000;
  readonly #timers: Array<{ at: number; f: () => void; on: boolean }> = [];
  after(ms: number, f: () => void): () => void {
    const t = { at: this.now + ms, f, on: true };
    this.#timers.push(t);
    return () => void (t.on = false);
  }
  /** The earliest pending timer's time. */
  next(): number | undefined {
    const live = this.#timers.filter((t) => t.on).map((t) => t.at);
    return live.length === 0 ? undefined : Math.min(...live);
  }
  async advance(ms: number): Promise<void> {
    const end = this.now + ms;
    for (;;) {
      const due = this.#timers.filter((t) => t.on && t.at <= end).sort((a, b) => a.at - b.at)[0];
      if (due === undefined) break;
      due.on = false;
      this.now = Math.max(this.now, due.at);
      due.f();
      await settle();
    }
    this.now = end;
    await settle();
  }
}

export const settle = async (): Promise<void> => {
  for (let i = 0; i < 20; i++) await new Promise((r) => setImmediate(r));
};

export interface Harness {
  readonly svc: LibraryService;
  readonly registry: Registry;
  readonly time: ManualTime;
  readonly control: ControlStore;
  /** Every scan submitted: `{lib, mode, trigger?, priority?}` in order. */
  readonly submits: Array<{ id: string; lib: string; kind: string; mode?: string; priority?: string }>;
  readonly probes: string[];
  readonly offlineFlags: Array<{ lib: string; offline: boolean }>;
  readonly journal: Array<{ kind: string; payload: unknown }>;
  readonly watches: string[];
  readonly unwatches: string[];
  readonly dropGuards: Array<{ lib: string; on: boolean }>;
  /** Jobs end before their submit answers (the submit/end race). */
  endOnSubmit: boolean;
  /** The next probe answers of a root (else `online` with its stored identity). */
  probeAnswers: RootState[];
  measured?: VolumeIdentity;
  emitWatch(e: WatchEvent): void;
  add(root: string, input?: Omit<NewLibrary, "root">, storage?: string): Promise<LibraryRow>;
  /** Ends job `id` (`succeeded` by default) and tells the service. */
  finish(id: string, state?: JobView["state"], extra?: Partial<JobView>): Promise<void>;
  active(): Array<{ id: string; lib: string; mode?: string }>;
  close(): void;
}

const dirs: string[] = [];
export function cleanupHarnesses(): void {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
}

export async function harness(opts: { counts?: CatalogCounts; deps?: Partial<LibraryServiceDeps> } = {}): Promise<Harness> {
  const serverData = realpathSync(mkdtempSync(join(tmpdir(), "fin-libsvc-")));
  dirs.push(serverData);
  const time = new ManualTime();
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false, clock: () => time.now });
  const storageOf = new Map<string, string>();
  const h = {
    submits: [] as Harness["submits"],
    probes: [] as string[],
    offlineFlags: [] as Harness["offlineFlags"],
    journal: [] as Harness["journal"],
    watches: [] as string[],
    unwatches: [] as string[],
    dropGuards: [] as Harness["dropGuards"],
    probeAnswers: [] as RootState[],
    endOnSubmit: false,
    measured: undefined as VolumeIdentity | undefined,
  };
  const tools: RootTools = {
    canonicalRoot: async (path) => ({ root: path, rootKey: path, identity: h.measured ?? identity("aa"), storage: storageOf.get(path) ?? "network" }),
    probeRoot: async (root, stored) => {
      h.probes.push(root);
      return h.probeAnswers.shift() ?? { state: "online", ...stored };
    },
  };
  const registry = new Registry(control, { serverData, tools, clock: () => time.now });
  const jobs = new Map<string, JobView>();
  const done = new Set<(j: JobView) => void>();
  let n = 0;
  const engine: LibraryServiceDeps["engine"] = {
    submit: (_owner, spec) =>
      Effect.sync(() => {
        const p = spec.params as { library: string; mode?: string };
        if (spec.kind === "library.scan" && p.mode !== "dirs") {
          const same = [...jobs.values()].find((j) => j.finishedAt === undefined && j.kind === spec.kind && (j.params as { mode?: string }).mode === p.mode && j.library === p.library);
          if (same !== undefined) return same;
        }
        const id = `job${++n}`;
        const view = {
          id, kind: spec.kind, kindVersion: 1, family: "library", params: spec.params, owner: { kind: "system", id: "library" }, library: p.library,
          priority: spec.priority ?? "background", state: "queued", attempts: 0, maxAttempts: 3, warm: false, createdAt: time.now,
        } as JobView;
        jobs.set(id, view);
        if (h.endOnSubmit) {
          // The job ends before its submit answers: `onDone` comes first, the answer still says `queued`.
          const ended = { ...view, state: "succeeded", finishedAt: time.now } as JobView;
          jobs.set(id, ended);
          for (const l of done) l(ended);
        }
        h.submits.push({ id, lib: p.library, kind: spec.kind, ...(p.mode === undefined ? {} : { mode: p.mode }), ...(spec.priority === undefined ? {} : { priority: spec.priority }) });
        return view;
      }),
    get: (id) => (jobs.has(id) ? Effect.succeed(jobs.get(id)!) : Effect.fail(new FinError({ code: "JOB_NOT_FOUND", message: "no job", details: {} }))),
    cancel: (id) =>
      Effect.sync(() => {
        const v = jobs.get(id)!;
        if (v.finishedAt !== undefined) return v;
        // A cancel ends the job (as the engine's would), and its end is reported.
        const ended = { ...v, state: "cancelled", finishedAt: time.now } as JobView;
        jobs.set(id, ended);
        queueMicrotask(() => {
          for (const l of done) l(ended);
        });
        return ended;
      }),
    wait: (ids, o) =>
      Effect.promise(async () => {
        const end = Date.now() + o.timeoutMs;
        while (Date.now() < end) {
          if (ids.every((id) => jobs.get(id)?.finishedAt !== undefined)) return ids.map((id) => jobs.get(id)!);
          await new Promise((r) => setTimeout(r, 10));
        }
        return undefined;
      }).pipe(Effect.flatMap((v) => (v === undefined ? Effect.fail(new FinError({ code: "WAIT_TIMEOUT", message: "timeout", details: {} })) : Effect.succeed(v)))),
    definePool: () => {},
    onDone: (l) => {
      done.add(l);
      return () => void done.delete(l);
    },
    list: (q) => ({ jobs: [...jobs.values()].filter((j) => j.finishedAt === undefined && (q.library === undefined || j.library === q.library) && (q.kind === undefined || j.kind === q.kind)) }),
  };
  const ok = <A>(v: A) => Effect.succeed(v);
  let watchListener: (e: WatchEvent) => void = () => {};
  const counts = opts.counts ?? { files: { present: 3 }, dirs: { present: 1 }, collisions: 0 };
  const catalogs = {
    indexer: {
      open: () => ok({ path: "x", created: true }),
      watch: (lib: string) => {
        h.watches.push(lib);
        return ok({ ok: true as const });
      },
      unwatch: (lib: string) => {
        h.unwatches.push(lib);
        return ok(undefined);
      },
      setDropGuard: (lib: string, on: boolean) => {
        h.dropGuards.push({ lib, on });
        return ok({ on });
      },
      close: () => ok(undefined),
      setOffline: (lib: string, offline: boolean) => {
        h.offlineFlags.push({ lib, offline });
        return ok({ offline });
      },
      finishScan: () => ok({ scanId: 1 }),
      purgeMissing: () => ok({ deleted: 0, dirs: 0 }),
      acceptUnmounted: () => ok({ dirs: [{ components: ["ZA=="] }] }),
      applyProbed: () => ok({ updated: 0, stale: 0, moved: [] }),
    },
    reader: {
      counts: () => ok(counts),
      fileLines: () => ok({ lines: [], absent: [] }),
      query: () => ok({}),
      files: () => ok({ dirs: [], files: [] }),
    },
    onWatch: (l: (e: WatchEvent) => void) => void (watchListener = l),
  } as unknown as LibraryServiceDeps["catalogs"];
  const health = rootHealth({ library: (id) => registry.get(id), probeRoot: tools.probeRoot, clock: () => time.now });
  const svc = new LibraryService({
    registry, catalogs, engine, health, clock: () => time.now, timers: time,
    journal: { enabled: true, record: (kind, _kv, _ctx, payload) => void h.journal.push({ kind, payload: JSON.parse(payload) }) },
    ...opts.deps,
  });
  await svc.load();
  return {
    ...h,
    get probeAnswers() {
      return h.probeAnswers;
    },
    set probeAnswers(v: RootState[]) {
      h.probeAnswers = v;
    },
    get endOnSubmit() {
      return h.endOnSubmit;
    },
    set endOnSubmit(v: boolean) {
      h.endOnSubmit = v;
    },
    get measured() {
      return h.measured;
    },
    set measured(v: VolumeIdentity | undefined) {
      h.measured = v;
    },
    svc, registry, time, control,
    emitWatch: (e) => watchListener(e),
    async add(root, input = {}, storage = "network") {
      storageOf.set(root, storage);
      const row = await Effect.runPromise(registry.add({ root, ...input }, ACTOR));
      await svc.attach(row, { isNew: true });
      svc.replan();
      await settle();
      return registry.get(row.id)!;
    },
    async finish(id, state = "succeeded", extra = {}) {
      const v = jobs.get(id)!;
      const ended = { ...v, state, finishedAt: time.now, ...extra } as JobView;
      jobs.set(id, ended);
      for (const l of done) l(ended);
      await settle();
    },
    active: () => [...jobs.values()].filter((j) => j.finishedAt === undefined).map((j) => ({ id: j.id, lib: j.library!, mode: (j.params as { mode?: string }).mode })),
    close() {
      svc.close();
      control.close();
    },
  } as Harness;
}
