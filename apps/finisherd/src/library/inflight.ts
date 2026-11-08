// The server-wide in-flight set of the stage pump (plan 5D Decision 40): one fingerprint is
// never in two jobs of one stage, across libraries; one catalogued file is never in two probe
// jobs. Keys are `(fingerprint, stage)` and `(library, fileId)`; each belongs to one job. A
// library that finds a fingerprint already in another library's job waits on that job (its
// own row records the job), and the job's end settles every waiting library's row too.
//
// The set is memory only: at start the pump rebuilds it from the catalogs' `queued`/`running`
// rows (`stages.jobId`) and the engine's active probe jobs.

import type { JobView } from "@finisher/contracts";

const stageKey = (fp: string, stage: string): string => `s\u0000${stage}\u0000${fp}`;
const probeKey = (library: string, fileId: number): string => `p\u0000${library}\u0000${fileId}`;

const keysOf = (c: Pick<JobClaim, "stage" | "library" | "items">): string[] =>
  c.items.map((item) => (c.stage === "probe" ? probeKey(c.library, Number(item)) : stageKey(item, c.stage)));

export interface JobClaim {
  readonly jobId: string;
  /** The library that submitted the job. */
  readonly library: string;
  /** `probe`, or the stage's name. */
  readonly stage: string;
  /** The stage's version the job runs (`""` for probes). */
  readonly version: string;
  /** Items: fingerprints of a stage job, file ids (as strings) of a probe job. */
  readonly items: string[];
  /** Item → the submitting library's file id (stage jobs; for `stage` file events). */
  readonly files: Map<string, number>;
  /** Held before the submit answered (`jobId` is a placeholder until `rename`): nobody waits on it. */
  reserved: boolean;
  /** Other libraries waiting on items of this job: library → fingerprint → that library's file id. */
  readonly waiters: Map<string, Map<string, number>>;
  /** Cancelled by the pump (offline, paused): items return to pending, no attempt is spent. */
  free: boolean;
  /** Its `queued` rows are being written: an end is held in `ended` until they are. */
  recording?: boolean;
  ended?: JobView;
}

export class InFlight {
  readonly #byKey = new Map<string, string>();
  readonly #jobs = new Map<string, JobClaim>();

  /** The job holding `(fp, stage)`, if any. */
  stageJob(fp: string, stage: string): JobClaim | undefined {
    const id = this.#byKey.get(stageKey(fp, stage));
    return id === undefined ? undefined : this.#jobs.get(id);
  }

  probing(library: string, fileId: number): boolean {
    return this.#byKey.has(probeKey(library, fileId));
  }

  /** Records a submitted (or adopted) job; its items' keys point to it. */
  add(claim: Omit<JobClaim, "waiters" | "free" | "reserved" | "items" | "files"> & { readonly items: ReadonlyArray<string>; readonly files?: ReadonlyMap<string, number>; readonly reserved?: boolean }): JobClaim {
    const existing = this.#jobs.get(claim.jobId);
    if (existing !== undefined) return existing;
    const c: JobClaim = { ...claim, items: [...claim.items], files: new Map(claim.files ?? []), waiters: new Map(), free: false, reserved: claim.reserved ?? false };
    this.#jobs.set(c.jobId, c);
    for (const key of keysOf(c)) this.#byKey.set(key, c.jobId);
    return c;
  }

  /** The submit answered: the reserved claim `from` becomes job `to`. */
  rename(from: string, to: string): JobClaim | undefined {
    const c = this.release(from);
    if (c === undefined) return undefined;
    const n = this.add({ ...c, jobId: to, reserved: false });
    for (const [lib, m] of c.waiters) n.waiters.set(lib, m);
    return n;
  }

  /** More items of an adopted job (its other library's rows, after a restart). */
  extend(c: JobClaim, items: ReadonlyArray<string>, files?: ReadonlyMap<string, number>): void {
    for (const item of items) {
      if (!c.items.includes(item)) c.items.push(item);
      const f = files?.get(item);
      if (f !== undefined) c.files.set(item, f);
    }
    for (const key of keysOf(c)) this.#byKey.set(key, c.jobId);
  }

  /** `library` (its file `fileId`) waits on `job` for `fp`. */
  wait(job: JobClaim, library: string, fp: string, fileId: number): void {
    if (library === job.library) return;
    let m = job.waiters.get(library);
    if (m === undefined) job.waiters.set(library, (m = new Map()));
    m.set(fp, fileId);
  }

  get(jobId: string): JobClaim | undefined {
    return this.#jobs.get(jobId);
  }

  /** Jobs submitted by `library` for `stage` (queued or running). */
  count(library: string, stage: string): number {
    let n = 0;
    for (const c of this.#jobs.values()) if (c.library === library && c.stage === stage) n++;
    return n;
  }

  /** Every job submitted by `library`. */
  jobsOf(library: string): JobClaim[] {
    return [...this.#jobs.values()].filter((c) => c.library === library);
  }

  /** Forgets a job and frees its keys; answers it. */
  release(jobId: string): JobClaim | undefined {
    const c = this.#jobs.get(jobId);
    if (c === undefined) return undefined;
    this.#jobs.delete(jobId);
    for (const key of keysOf(c)) if (this.#byKey.get(key) === jobId) this.#byKey.delete(key);
    return c;
  }

  /** Drops `library` as a waiter everywhere (it was removed). */
  forget(library: string): void {
    for (const c of this.#jobs.values()) c.waiters.delete(library);
  }

  size(): number {
    return this.#jobs.size;
  }
}
