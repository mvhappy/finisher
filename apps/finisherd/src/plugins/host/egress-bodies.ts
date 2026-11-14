// Response bodies of `net.fetch`, held by the host (sp8b plan Decision 11), and the memory
// they take. Each body (at most `BODY_BYTES`) is kept under a random handle that belongs to the
// job whose call fetched it, read by `net.read {handle, offset, length}` in pages of at most
// `READ_BYTES` (base64 in the reply, so a page and the envelope fit 5C's 1 MiB line, as
// `project.snapshot`'s pages), and released when that job ends.
//
// Memory is charged as it arrives, not when a body is complete: bytes still in flight count
// with the held ones against the job's `JOB_HELD_BYTES` and against the server's
// `SERVER_BYTES` over every job, so a burst of large responses cannot grow the host without
// bound. When a job ends its bodies go, its in-flight requests are aborted, and nothing more is
// held for it (it is remembered as ended for `ENDED_MEMORY_MS`, longer than any request).
import { randomBytes } from "node:crypto";
import type { FinError } from "@finisher/contracts";
import { finError } from "../../errors.ts";

/** The largest response body (Decision 11). */
export const BODY_BYTES = 64 * 1024 * 1024;
/** The largest `net.read` page before base64: the same budget as a snapshot page. */
export const READ_BYTES = 768_000;
/** The most body bytes, held and in flight, of one job. */
export const JOB_HELD_BYTES = 4 * BODY_BYTES;
/** The most body bytes, held and in flight, of every job together. */
export const SERVER_BYTES = 512 * 1024 * 1024;
/** How long an ended job is remembered. */
export const ENDED_MEMORY_MS = 10 * 60_000;

export interface BodyLimits {
  readonly jobBytes: number;
  readonly serverBytes: number;
}

interface JobBodies {
  readonly bodies: Map<string, Buffer>;
  held: number;
  inFlight: number;
  readonly aborts: Set<AbortController>;
}

export function tooLarge(what: "request" | "response" | "held", limit: number): FinError {
  return finError("EGRESS_TOO_LARGE", `the ${what} body is over ${limit} bytes`, { what, limit });
}

export function jobEnded(): FinError {
  return finError("INVALID_REQUEST", "the job this call belongs to has ended", { reason: "jobEnded" });
}

export function makeHeldBodies(limits: BodyLimits = { jobBytes: JOB_HELD_BYTES, serverBytes: SERVER_BYTES }, now: () => number = Date.now) {
  const jobs = new Map<string, JobBodies>();
  /** Ended jobs and when they ended, oldest first (pruned after `ENDED_MEMORY_MS`). */
  const ended = new Map<string, number>();
  let total = 0;

  const isEnded = (jobId: string): boolean => {
    const t = now();
    for (const [id, at] of ended) {
      if (t - at < ENDED_MEMORY_MS) break;
      ended.delete(id);
    }
    return ended.has(jobId);
  };

  return {
    /** A request of `jobId` starts: `ctl` is aborted (with `jobEnded`) if the job ends first. */
    track(jobId: string, ctl: AbortController): FinError | undefined {
      let j = jobs.get(jobId);
      if (j === undefined) {
        if (isEnded(jobId)) return jobEnded();
        j = { bodies: new Map(), held: 0, inFlight: 0, aborts: new Set() };
        jobs.set(jobId, j);
      }
      j.aborts.add(ctl);
      return undefined;
    },
    untrack(jobId: string, ctl: AbortController): void {
      jobs.get(jobId)?.aborts.delete(ctl);
    },
    /** `n` more bytes arrived for `jobId`; refused past the job's or the server's budget. */
    charge(jobId: string, n: number): FinError | undefined {
      const j = jobs.get(jobId);
      if (j === undefined) return jobEnded();
      if (j.held + j.inFlight + n > limits.jobBytes) return tooLarge("held", limits.jobBytes);
      if (total + n > limits.serverBytes) {
        return finError("EGRESS_RATE_LIMITED", "the server holds as many response bytes as it may; try again shortly", { reason: "serverBytes", limit: limits.serverBytes, retryable: true });
      }
      j.inFlight += n;
      total += n;
      return undefined;
    },
    /** `n` charged bytes are dropped (a followed redirect's body, a failed request). */
    uncharge(jobId: string, n: number): void {
      // A released job's bytes were returned at release.
      const j = jobs.get(jobId);
      if (j === undefined) return;
      j.inFlight -= n;
      total -= n;
    },
    /** Holds `body` (already charged) for `jobId`: its handle. */
    hold(jobId: string, body: Buffer): string | FinError {
      const j = jobs.get(jobId);
      if (j === undefined) return jobEnded();
      const handle = randomBytes(16).toString("hex");
      j.bodies.set(handle, body);
      j.inFlight -= body.length;
      j.held += body.length;
      return handle;
    },
    /** A page of a held body of `jobId`. */
    read(jobId: string, handle: string, offset: number, length: number): { readonly bytesB64: string; readonly eof: boolean } | FinError {
      const body = jobs.get(jobId)?.bodies.get(handle);
      // Another job's handle is no handle at all.
      if (body === undefined) return finError("INVALID_REQUEST", "net.read names no body this job holds", { reason: "unknownHandle" });
      if (length > READ_BYTES) return finError("INVALID_REQUEST", `a net.read page is at most ${READ_BYTES} bytes`, { reason: "pageTooLarge", limit: READ_BYTES });
      if (offset > body.length) return finError("INVALID_REQUEST", `offset ${offset} is past the body's ${body.length} bytes`, { reason: "offset", length: body.length });
      const end = Math.min(body.length, offset + length);
      return { bytesB64: body.subarray(offset, end).toString("base64"), eof: end === body.length };
    },
    /** `jobId` ended: its bodies go, its requests are aborted, nothing more is held for it. */
    release(jobId: string): void {
      ended.delete(jobId);
      ended.set(jobId, now());
      const j = jobs.get(jobId);
      if (j === undefined) return;
      jobs.delete(jobId);
      total -= j.held + j.inFlight;
      for (const ctl of j.aborts) ctl.abort(jobEnded());
    },
    /** Body bytes held and in flight now, over all jobs (tests, stats). */
    heldBytes(): number {
      return total;
    },
    /** Jobs with held or in-flight bodies (tests). */
    jobs: (): number => jobs.size,
  };
}

export type HeldBodies = ReturnType<typeof makeHeldBodies>;
