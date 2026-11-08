// Who sees and controls which job (5C-2 plan Decisions 2–3, spec §6.1). A job's owner is the
// principal that submitted it (`{kind, id}`); a principal sees a job when the job is in its reach
// and it owns the job or holds `jobs.control`. Reach: a job's project must be one the principal
// names (or it names `"*"`); a library job needs `library.read` (5D Decision 47: no per-library
// lists in v1). Anything else is answered exactly as an unknown id (`JOB_NOT_FOUND`), so a caller
// learns nothing of jobs it cannot see.
import type { FinError, JobOwner, JobState, JobView, Principal } from "@finisher/contracts";
import { finError } from "../errors.ts";
import type { JobQuery } from "./store.ts";

/** The owner a principal's submits record. */
export const ownerOf = (p: Principal): JobOwner => ({ kind: p.kind, id: p.id });

const owns = (p: Principal, job: Pick<JobView, "owner">): boolean => job.owner.kind === p.kind && job.owner.id === p.id;

/** Whether the job's project and library are in the principal's reach. */
export function inReach(p: Principal, job: Pick<JobView, "project" | "library">): boolean {
  if (job.project !== undefined && p.projects !== "*" && !p.projects.includes(job.project)) return false;
  if (job.library !== undefined && !p.scopes.includes("library.read")) return false;
  return true;
}

/** Whether `p` sees `job`. */
export function canSee(p: Principal, job: Pick<JobView, "owner" | "project" | "library">): boolean {
  return inReach(p, job) && (owns(p, job) || p.scopes.includes("jobs.control"));
}

/** Whose `job` is to `p`, for the chain's `job` resolver: `none` when unknown or not visible. */
export function relationOf(p: Principal, job: JobView | undefined): "own" | "other" | "none" {
  if (job === undefined || !canSee(p, job)) return "none";
  return owns(p, job) ? "own" : "other";
}

/** The answer for an unknown job, and for one the caller cannot see (the engine's too: one text). */
export const jobNotFound = (id: string): FinError =>
  finError("JOB_NOT_FOUND", `no job ${id}`, { job: id }, "use the id of a job you can see (jobs.list lists them)");

export interface ListFilter {
  readonly state?: ReadonlyArray<JobState> | undefined;
  readonly kind?: string | undefined;
  readonly project?: string | undefined;
  readonly mine?: boolean | undefined;
  readonly cursor?: string | undefined;
  readonly limit?: number | undefined;
}

/** The store query of `p`'s `jobs.list`: its own jobs unless it holds `jobs.control`, and only in reach. */
export function listQuery(p: Principal, f: ListFilter): JobQuery {
  const control = p.scopes.includes("jobs.control") && f.mine !== true;
  return {
    ...(control ? {} : { owner: ownerOf(p) }),
    ...(f.state === undefined ? {} : { state: f.state }),
    ...(f.kind === undefined ? {} : { kind: f.kind }),
    ...(f.project === undefined ? {} : { project: f.project }),
    ...(f.cursor === undefined ? {} : { cursor: f.cursor }),
    ...(f.limit === undefined ? {} : { limit: f.limit }),
    reach: { ...(p.projects === "*" ? {} : { projects: p.projects }), libraries: p.scopes.includes("library.read") },
  };
}
