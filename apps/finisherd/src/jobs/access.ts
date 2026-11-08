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

/**
 * Owner kinds with no per-owner limit (5C-2 review M4): a person's (`console`, a device), the
 * owner's `local` and in-process `system`. Every other owner (agents, MCP, plugins) holds at
 * most `EngineDeps.ownerLimit` unfinished jobs, so none can fill the global queue alone.
 */
const UNLIMITED_OWNERS: ReadonlySet<string> = new Set(["local", "console", "device", "system"]);
export const ownerLimited = (o: JobOwner): boolean => !UNLIMITED_OWNERS.has(o.kind);
export const ownerKey = (o: JobOwner): string => `${o.kind}:${o.id}`;

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

/**
 * `canSee` for one principal, its lists read once into sets: O(1) per job, for a watcher that
 * checks every event the engine publishes (5C-2 review M1).
 */
export function seer(p: Principal): (job: Pick<JobView, "owner" | "project" | "library">) => boolean {
  const projects = p.projects === "*" ? undefined : new Set(p.projects);
  const libraries = p.scopes.includes("library.read");
  const control = p.scopes.includes("jobs.control");
  return (job) =>
    (job.project === undefined || projects === undefined || projects.has(job.project)) &&
    (job.library === undefined || libraries) &&
    (control || owns(p, job));
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
