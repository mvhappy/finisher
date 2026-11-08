// The poll planner (plan 5D Decisions 18, 28–30): pure. Given every library's schedule, state
// and what is running, it answers one action per library (`poll | full | recheck | none`) and
// when to look again. The service runs it at every wake-up and whenever something changes.
//
// - **Network roots, `poll: demand`** (Decision 28): a poll every 60 s while the library is in
//   use, every 6 hours otherwise. Its daily full scan runs at the **first poll tick after it is
//   due**, so an unused NAS wakes at most at the 6-hour ticks.
// - **Sync roots** poll at their interval (default 10 minutes) and scan fully daily, the same
//   way (Decision 29).
// - **Local roots** are watched (Decision 30): no polls while the watch is healthy, 5-minute
//   polls once it degraded; a full scan weekly, when due. A fixed `poll` interval always wins
//   (a degraded watch caps it at 5 minutes).
// - **Offline** libraries are only re-probed, every 60 s (Decision 18); **paused** ones do
//   nothing.
// - At most one job per pool kind per library: no full scan while one runs, no poll while one
//   runs (a poll may run beside a full scan: separate pools, Decision 32).
// - A full scan that did not complete is tried again an hour after its last try, not at every
//   tick.
import type { LibraryScan, LibraryState, LibraryStorage } from "./registry.ts";

export const IN_USE_POLL_MS = 60_000;
export const IDLE_POLL_MS = 6 * 3_600_000;
export const DEGRADED_POLL_MS = 5 * 60_000;
export const RECHECK_MS = 60_000;
export const FULL_RETRY_MS = 3_600_000;
export const DAY_MS = 86_400_000;
export const WEEK_MS = 7 * DAY_MS;

export type WatchState = "none" | "ok" | "degraded";

export interface PlanLibrary {
  readonly id: string;
  readonly storage: LibraryStorage;
  readonly state: LibraryState;
  readonly scan: LibraryScan;
  readonly createdAt: number;
  readonly lastPollAt?: number;
  readonly lastFullAt?: number;
  /** When the service last submitted a full scan (in memory). */
  readonly lastFullTry?: number;
  /** When the root was last probed while offline. */
  readonly lastRecheckAt?: number;
  readonly watch: WatchState;
  readonly running: { readonly full: boolean; readonly poll: boolean; readonly recheck: boolean };
}

export type ActionKind = "poll" | "full" | "recheck" | "none";

export interface Action {
  readonly library: string;
  readonly action: ActionKind;
  /** When to plan this library again (ms; `Infinity`: only when something changes). Always after `now`. */
  readonly next: number;
}

/** The poll interval of `lib` now, or `undefined` when it is not polled. */
export function pollInterval(lib: Pick<PlanLibrary, "storage" | "scan" | "watch">, inUse: boolean): number | undefined {
  const fixed = typeof lib.scan.poll === "number" ? lib.scan.poll * 1000 : undefined;
  if (lib.storage === "local") {
    if (lib.watch === "degraded") return Math.min(fixed ?? DEGRADED_POLL_MS, DEGRADED_POLL_MS);
    if (fixed !== undefined) return fixed;
    // Watched: changes arrive as hints. Without a watcher (it could not start), demand cadence.
    return lib.watch === "ok" ? undefined : inUse ? IN_USE_POLL_MS : IDLE_POLL_MS;
  }
  return fixed ?? (inUse ? IN_USE_POLL_MS : IDLE_POLL_MS);
}

/** The full-scan interval, or `undefined` for `manual`. */
export function fullInterval(full: LibraryScan["full"]): number | undefined {
  return full === "daily" ? DAY_MS : full === "weekly" ? WEEK_MS : undefined;
}

function planOne(lib: PlanLibrary, now: number, inUse: boolean): Action {
  const at = (action: ActionKind, next: number): Action => ({ library: lib.id, action, next: Math.max(next, now + 1) });
  if (lib.state === "paused") return at("none", Infinity);
  if (lib.state === "offline") {
    if (lib.running.recheck) return at("none", now + RECHECK_MS);
    const due = (lib.lastRecheckAt ?? -Infinity) + RECHECK_MS;
    return now >= due ? at("recheck", now + RECHECK_MS) : at("none", due);
  }
  const poll = pollInterval(lib, inUse);
  const full = fullInterval(lib.scan.full);
  // A full scan just started covers the next poll: the first one beside it runs an interval later.
  const pollBase = Math.max(lib.lastPollAt ?? lib.lastFullAt ?? lib.createdAt, lib.lastFullTry ?? -Infinity);
  const pollDue = poll !== undefined && now >= pollBase + poll;
  const retryAt = lib.lastFullTry === undefined ? -Infinity : lib.lastFullTry + FULL_RETRY_MS;
  const fullAt = full === undefined ? Infinity : Math.max((lib.lastFullAt ?? lib.createdAt) + full, retryAt);
  const fullDue = now >= fullAt;
  // Remote roots take their full scan at a poll tick; local ones (and unpolled ones) when due.
  const atTicks = lib.storage !== "local" && poll !== undefined;
  const nextPoll = (polled: boolean): number => (poll === undefined ? Infinity : polled ? now + poll : pollBase + poll);
  const nextFull = (started: boolean): number => {
    if (atTicks || full === undefined) return Infinity;
    return started ? now + Math.min(full, FULL_RETRY_MS) : fullAt;
  };
  if (fullDue && !lib.running.full && (!atTicks || pollDue)) {
    return at("full", Math.min(nextPoll(true), nextFull(true)));
  }
  if (pollDue && !lib.running.poll) return at("poll", Math.min(nextPoll(true), nextFull(false)));
  // A due job blocked by a running one is planned again when that job ends.
  const pollNext = pollDue ? Infinity : nextPoll(false);
  const fullNext = fullDue ? Infinity : nextFull(false);
  return at("none", Math.min(pollNext, fullNext));
}

/** One action per library, in input order (Decisions 18, 28–30). */
export function plan(libs: ReadonlyArray<PlanLibrary>, now: number, inUse: (id: string) => boolean): Action[] {
  return libs.map((lib) => planOne(lib, now, inUse(lib.id)));
}
