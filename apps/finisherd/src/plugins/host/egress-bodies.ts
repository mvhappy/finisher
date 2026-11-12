// Response bodies of `net.fetch`, held by the host (sp8b plan Decision 11): each body (at most
// `BODY_BYTES`) is kept under a random handle that belongs to the job whose call fetched it,
// read by `net.read {handle, offset, length}` in pages of at most `READ_BYTES` (base64 in the
// reply, so a page and the envelope fit 5C's 1 MiB line, as `project.snapshot`'s pages), and
// released when that job ends. A job holds at most `JOB_HELD_BYTES` at once.
import { randomBytes } from "node:crypto";
import type { FinError } from "@finisher/contracts";
import { finError } from "../../errors.ts";

/** The largest response body (Decision 11). */
export const BODY_BYTES = 64 * 1024 * 1024;
/** The largest `net.read` page before base64: the same budget as a snapshot page. */
export const READ_BYTES = 768_000;
/** The most body bytes one job holds at once. */
export const JOB_HELD_BYTES = 4 * BODY_BYTES;

export interface HeldBodies {
  /** Holds `body` for `jobId`; its handle, or `EGRESS_TOO_LARGE` past the job's budget. */
  hold(jobId: string, body: Buffer): string | FinError;
  /** A page of a held body of `jobId`. */
  read(jobId: string, handle: string, offset: number, length: number): { readonly bytesB64: string; readonly eof: boolean } | FinError;
  /** `jobId` ended: its bodies go. */
  release(jobId: string): void;
  /** Bytes held now, over all jobs (tests, stats). */
  heldBytes(): number;
}

export function tooLarge(what: "request" | "response" | "held", limit: number): FinError {
  return finError("EGRESS_TOO_LARGE", `the ${what} body is over ${limit} bytes`, { what, limit });
}

export function makeHeldBodies(jobHeld = JOB_HELD_BYTES): HeldBodies {
  const jobs = new Map<string, Map<string, Buffer>>();
  const sizeOf = (m: Map<string, Buffer>): number => {
    let n = 0;
    for (const b of m.values()) n += b.length;
    return n;
  };
  return {
    hold(jobId, body) {
      let m = jobs.get(jobId);
      if (m === undefined) {
        m = new Map();
        jobs.set(jobId, m);
      }
      if (sizeOf(m) + body.length > jobHeld) return tooLarge("held", jobHeld);
      const handle = randomBytes(16).toString("hex");
      m.set(handle, body);
      return handle;
    },
    read(jobId, handle, offset, length) {
      const body = jobs.get(jobId)?.get(handle);
      // Another job's handle is no handle at all.
      if (body === undefined) return finError("INVALID_REQUEST", "net.read names no body this job holds", { reason: "unknownHandle" });
      if (length > READ_BYTES) return finError("INVALID_REQUEST", `a net.read page is at most ${READ_BYTES} bytes`, { reason: "pageTooLarge", limit: READ_BYTES });
      if (offset > body.length) return finError("INVALID_REQUEST", `offset ${offset} is past the body's ${body.length} bytes`, { reason: "offset", length: body.length });
      const end = Math.min(body.length, offset + length);
      return { bytesB64: body.subarray(offset, end).toString("base64"), eof: end === body.length };
    },
    release(jobId) {
      jobs.delete(jobId);
    },
    heldBytes() {
      let n = 0;
      for (const m of jobs.values()) n += sizeOf(m);
      return n;
    },
  };
}
