// Targeted scans per library (plan 5D Decisions 28, 30; the Tasks 9–11 review). At most one
// `dirs` job of a library is **queued** at a time: a request whose directories it already
// lists, at no higher priority, answers that job; one with new directories replaces it with a
// job listing both (the queued one is cancelled, which costs nothing; it is not journaled), at
// the higher of the two priorities. A running job is left alone, since its listing may already
// be past the directory, so a library has at most one queued and one running targeted scan.
// Past 1,000 directories a request becomes a poll of the whole library.
//
// It also remembers, per library, when each directory was last listed by a successful
// targeted scan and when a freshness pass last asked for one (`fresh.ts`): a directory listed
// within 5 minutes is fresh, and a freshness pass asks for a directory at most once a minute.
import type { RawRel } from "@finisher/core-types";
import type { JobView, Priority } from "@finisher/contracts";

export const MAX_MERGED_DIRS = 1000;
/** A freshness pass lists one directory at most this often (Decision 28's answers stay bounded). */
export const FRESH_DIR_RETRY_MS = 60_000;
/** Directories remembered per library (the oldest go first). */
const REMEMBERED = 10_000;

export type DirsTrigger = "add" | "schedule" | "demand" | "watch" | "mount";

export interface DirsRequest {
  readonly dirs: ReadonlyArray<RawRel>;
  readonly trigger: DirsTrigger;
  readonly priority?: Priority;
}

export interface DirsDeps {
  readonly get: (job: string) => Promise<JobView | undefined>;
  readonly cancel: (job: string) => Promise<void>;
  readonly submit: (lib: string, req: { readonly mode: "dirs" | "poll"; readonly dirs?: ReadonlyArray<RawRel>; readonly trigger: DirsTrigger; readonly priority: Priority }) => Promise<JobView | undefined>;
}

interface Queued {
  readonly job: string;
  readonly dirs: Map<string, RawRel>;
  readonly priority: Priority;
}

const RANK: Record<Priority, number> = { background: 0, normal: 1, interactive: 2 };
const higher = (a: Priority, b: Priority): Priority => (RANK[a] >= RANK[b] ? a : b);
export const dirKey = (d: RawRel): string => d.components.join("/");

function remember(m: Map<string, number>, key: string, at: number): void {
  m.delete(key);
  m.set(key, at);
  if (m.size > REMEMBERED) m.delete(m.keys().next().value!);
}

export class DirsQueue {
  readonly #d: DirsDeps;
  readonly #queued = new Map<string, Queued>();
  readonly #merged = new Set<string>();
  readonly #chains = new Map<string, Promise<unknown>>();
  readonly #listed = new Map<string, Map<string, number>>();
  readonly #tried = new Map<string, Map<string, number>>();

  constructor(deps: DirsDeps) {
    this.#d = deps;
  }

  /** Queues a targeted scan of `lib` (one request at a time per library). */
  request(lib: string, req: DirsRequest): Promise<JobView | undefined> {
    const run = (this.#chains.get(lib) ?? Promise.resolve()).then(() => this.#request(lib, req));
    const tail = run.catch(() => undefined);
    this.#chains.set(lib, tail);
    void tail.then(() => {
      if (this.#chains.get(lib) === tail) this.#chains.delete(lib);
    });
    return run;
  }

  async #request(lib: string, req: DirsRequest): Promise<JobView | undefined> {
    const want = req.priority ?? "background";
    let base = this.#queued.get(lib);
    if (base !== undefined) {
      const v = await this.#d.get(base.job);
      if (v === undefined || v.state !== "queued") {
        this.#queued.delete(lib);
        base = undefined;
      } else if (req.dirs.every((d) => base!.dirs.has(dirKey(d))) && RANK[want] <= RANK[base.priority]) {
        return v;
      }
    }
    const dirs = new Map(base?.dirs ?? []);
    for (const d of req.dirs) dirs.set(dirKey(d), d);
    const priority = base === undefined ? want : higher(base.priority, want);
    if (base !== undefined) {
      this.#merged.add(base.job);
      this.#queued.delete(lib);
      await this.#d.cancel(base.job);
    }
    if (dirs.size > MAX_MERGED_DIRS) return this.#d.submit(lib, { mode: "poll", trigger: req.trigger, priority });
    const view = await this.#d.submit(lib, { mode: "dirs", dirs: [...dirs.values()], trigger: req.trigger, priority });
    if (view !== undefined && view.finishedAt === undefined && view.state === "queued") this.#queued.set(lib, { job: view.id, dirs, priority });
    return view;
  }

  /** Whether `job` was cancelled to merge it into a later one (asked once, at its end). */
  merged(job: string): boolean {
    return this.#merged.delete(job);
  }

  /** A targeted scan of `lib` succeeded: its directories are fresh as of `at`. */
  listed(lib: string, dirs: ReadonlyArray<RawRel>, at: number): void {
    let m = this.#listed.get(lib);
    if (m === undefined) this.#listed.set(lib, (m = new Map()));
    for (const d of dirs) remember(m, dirKey(d), at);
  }

  /** When `dir` of `lib` was last listed by a successful targeted scan. */
  listedAt(lib: string, dir: RawRel): number | undefined {
    return this.#listed.get(lib)?.get(dirKey(dir));
  }

  /** Whether a freshness pass may ask for `dir` of `lib` now (then it counts as asked). */
  tryFresh(lib: string, dir: RawRel, now: number): boolean {
    let m = this.#tried.get(lib);
    if (m === undefined) this.#tried.set(lib, (m = new Map()));
    const last = m.get(dirKey(dir));
    if (last !== undefined && now - last < FRESH_DIR_RETRY_MS) return false;
    remember(m, dirKey(dir), now);
    return true;
  }

  forget(lib: string): void {
    this.#queued.delete(lib);
    this.#listed.delete(lib);
    this.#tried.delete(lib);
  }
}
