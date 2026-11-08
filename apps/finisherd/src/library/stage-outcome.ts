// What a stage job's end does to its rows (plan 5D Decision 40), as a pure function of the
// job's view: items → `done`; item errors → `failed` with their code (`POISON_ITEM`
// permanently for this version); a whole failed job → `failed` for its items. A failure that
// says the library was unusable (`LIBRARY_OFFLINE`, `RESOURCE_STUCK`), and a job the pump
// cancelled because the library went offline or was paused, cost no attempt: the rows go back
// to pending (`stale` at the same version, attempts kept). So does an item whose file is not
// what the catalog says (`FILE_CHANGED`, `FILE_NOT_FOUND`, `FILE_CLOUD_ONLY`): the file is not
// to blame, and its directory is listed again (`rescanItems`), as for a hash.
import type { JobView } from "@finisher/contracts";
import { MAX_ATTEMPTS } from "./stages.ts";

/** Codes that blame the library, not the file: no attempt is spent. */
export const FREE_CODES: ReadonlySet<string> = new Set(["LIBRARY_OFFLINE", "RESOURCE_STUCK"]);
/** Item codes that say the catalog is behind the file: no attempt, and its directory is rescanned. */
export const RESCAN_CODES: ReadonlySet<string> = new Set(["FILE_CHANGED", "FILE_NOT_FOUND", "FILE_CLOUD_ONLY"]);
export const POISON = "POISON_ITEM";

export type ItemFate =
  | { readonly fate: "done" }
  | { readonly fate: "failed"; readonly code: string; readonly permanent: boolean }
  | { readonly fate: "pending"; readonly code: string };

const isObj = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);

/**
 * The fate of each of `items` after `job` ended. `free`: the pump cancelled the job itself
 * (offline, paused). Items a succeeded job neither answered nor failed are failed
 * (`STAGE_ITEM_UNANSWERED`): the kind broke the batch contract.
 */
export function itemFates(job: Pick<JobView, "state" | "result" | "error" | "itemErrors">, items: ReadonlyArray<string>, free: boolean): Map<string, ItemFate> {
  const out = new Map<string, ItemFate>();
  const errors = new Map<string, string>();
  const listed = job.itemErrors ?? (isObj(job.result) && Array.isArray(job.result.itemErrors) ? (job.result.itemErrors as JobView["itemErrors"]) : []);
  for (const e of listed ?? []) errors.set(e.item, e.error.code);
  const values = isObj(job.result) && isObj(job.result.items) ? job.result.items : {};
  const jobCode = job.error?.code ?? (job.state === "cancelled" ? "JOB_CANCELLED" : "JOB_FAILED");
  const wholeFree = free || FREE_CODES.has(jobCode);
  for (const item of items) {
    const code = errors.get(item);
    if (job.state === "succeeded" && code === undefined && Object.hasOwn(values, item)) {
      out.set(item, { fate: "done" });
    } else if (code !== undefined) {
      if (FREE_CODES.has(code) || RESCAN_CODES.has(code)) out.set(item, { fate: "pending", code });
      else out.set(item, { fate: "failed", code, permanent: code === POISON });
    } else if (job.state === "succeeded") {
      out.set(item, { fate: "failed", code: "STAGE_ITEM_UNANSWERED", permanent: false });
    } else if (wholeFree) {
      out.set(item, { fate: "pending", code: jobCode });
    } else {
      out.set(item, { fate: "failed", code: jobCode, permanent: false });
    }
  }
  return out;
}

/** The stage row a fate writes (`attempts`: the stored count before this job). */
export function rowOf(
  fate: ItemFate,
  base: { readonly fp: string; readonly stage: string; readonly version: string; readonly at: number },
  attempts: number,
): { fp: string; stage: string; version: string; state: "done" | "failed" | "stale"; code: string | null; jobId: null; attempts: number; at: number } {
  switch (fate.fate) {
    case "done":
      return { ...base, state: "done", code: null, jobId: null, attempts };
    case "pending":
      return { ...base, state: "stale", code: fate.code, jobId: null, attempts };
    case "failed":
      return { ...base, state: "failed", code: fate.code, jobId: null, attempts: fate.permanent ? Math.max(MAX_ATTEMPTS, attempts + 1) : attempts + 1 };
  }
}

/** The items of `fates` whose files must be listed again (`RESCAN_CODES`). */
export function rescanItems(fates: ReadonlyMap<string, ItemFate>): string[] {
  return [...fates].filter(([, f]) => f.fate === "pending" && RESCAN_CODES.has(f.code)).map(([item]) => item);
}
