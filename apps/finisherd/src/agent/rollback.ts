// Rolling a session back to a run (sub-project 7 §5.3, §4.2; plan 7B Decision 23, Review Focus 5).
//
// - **Who:** people, and a parent session for its own direct children (the stage-4 check in
//   `reach.ts`). Refused while the session runs (`SESSION_BUSY {status}`), while it has a live
//   child (`SESSION_BUSY {reason: liveChildren}`: the child's work would stand on what is undone)
//   and once it has ended (`SESSION_ENDED`). `toRun` is a run's seq (0: before the first); the
//   session's last run, a later one and a run a rollback already took back are `INVALID_REQUEST`.
// - **Its transactions:** the `session_txs` rows of the runs after `toRun` (by `runId`, agent
//   0008; an older row by time), grouped by the branch each landed on (`tx.get`), a branch whose
//   newest row is newest first. Each branch gets **one** 4B undo record (`undo` with `afterSeq`,
//   the seq before the branch's first such row: every unit of the session's author made after
//   it, newest first, as `undo --to` takes them after a checkpoint) under the session's author,
//   with a guard of the session's reach (its subtree less its live children's) and the caller's
//   credential. A branch with nothing left to undo (`NOTHING_TO_UNDO`: the session undid them
//   itself) is skipped, an archived one too.
// - **All or nothing:** a failure on any branch (`CONFLICT`: a person's later edit the undo
//   cannot be carried over) redoes the branches already undone, newest first (one 4B redo
//   reverses one undo record whole), and the call answers that failure with `details.branch`
//   and `redone`; every branch is as it was (`redoFailed` names any a redo could not restore).
// - **Outcomes:** each undone transaction is `undone`, `via: undo`, `by` the caller's
//   (`user`, or `agent` for a parent), with `details.rollback {toRun, by: <kind>:<id>}`.
// - **The conversation:** the later runs' nodes are marked `rolledBack` (their work is undone;
//   a run marked so is no `toRun` again). A provider with the `rollback` capability rolls back
//   to the turn (`ProviderSession.rollback(toRun)`, its instance opened for it when none is);
//   for the built-in loop the marks are the rollback (its next request leaves them out,
//   `historyFromNodes`). Without the capability, or when the provider fails, the provider's
//   history stays and a `notice` says so (spec §4.2: a CLI's rollback undoes the transactions
//   and resumes). Runs and nodes are never removed; the next run takes the next seq. A
//   `notice {rollback: …}` in the last run records every rollback.
import type { FinError, Principal, Run, Session, TxGetResult } from "@finisher/contracts";
import type { BranchView, WriteGuard } from "@finisher/core-types";
import { Cause, Effect, Exit, Option } from "effect";
import type { CallMeta } from "../diagnostics.ts";
import { finError } from "../errors.ts";
import type { HistoryOp, HistoryPayload } from "../history.ts";
import type { Capabilities, ProviderSession } from "./adapter.ts";
import type { Ids } from "./clock.ts";
import type { OutcomeTracker } from "./outcomes.ts";
import { sessionAuthor } from "./reach.ts";
import { call } from "./runtime-live.ts";
import type { AgentStore, NewNode } from "./store.ts";
import type { SessionTx } from "./store-tables.ts";

/** What a rollback asks of the project registry (`ProjectRegistry` is one). */
export interface RollbackRegistry {
  history(projectId: string, op: HistoryOp, payload: HistoryPayload, meta?: CallMeta): Effect.Effect<unknown, FinError>;
  txGet(projectId: string, txId: string): Effect.Effect<TxGetResult, FinError>;
  branches(projectId: string, all: boolean): Effect.Effect<{ readonly branches: ReadonlyArray<BranchView> }, FinError>;
}

export interface RollbackDeps {
  readonly store: AgentStore;
  readonly registry: RollbackRegistry;
  readonly outcomes: OutcomeTracker;
  readonly ids: Ids;
}

/** What the runtime lends a rollback for the conversation, in the session's queue. */
export interface ConversationOps {
  caps(s: Session): Promise<Capabilities>;
  live(sessionId: string): ProviderSession | undefined;
  open(s: Session): Promise<ProviderSession>;
  close(sessionId: string): Promise<void>;
  append(n: NewNode): Promise<void>;
}

export interface RollbackResult {
  readonly undone: readonly string[];
  readonly branches: readonly string[];
  readonly conversation: "rolledBack" | "kept";
}

/** The guard of a rollback's writes: the session's reach, the caller's credential and denials. */
export function rollbackGuard(reach: ReadonlyArray<string>, caller: WriteGuard | undefined): WriteGuard {
  return { denyBranches: [...(caller?.denyBranches ?? [])], allowBranches: [...reach], ...(caller?.credential === undefined ? {} : { credential: caller.credential }) };
}

async function run<A>(e: Effect.Effect<A, FinError>): Promise<A> {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) return exit.value;
  const err = Cause.findErrorOption(exit.cause);
  throw Option.isSome(err) ? err.value : Cause.squash(exit.cause);
}

const codeOf = (e: unknown): string | undefined => (e as { code?: unknown } | null)?.code as string | undefined;

/** One branch's share: where its undo starts, and how new its newest transaction is. */
interface BranchShare {
  readonly branch: string;
  afterSeq: number;
  newest: { at: number; txId: string };
}

type Undone = { readonly branch: string; readonly units: readonly string[]; readonly origins: readonly string[] };

export class Rollbacks {
  readonly #d: RollbackDeps;

  constructor(deps: RollbackDeps) {
    this.#d = deps;
  }

  /** The refusals, before anything is written. */
  check(s: Session, toRun: number): void {
    const { store } = this.#d;
    if (s.status === "ended") throw finError("SESSION_ENDED", "the session has ended", { status: s.status }, "start a new session from its branch");
    if (s.status !== "idle" && s.status !== "interrupted") {
      throw finError("SESSION_BUSY", "a session rolls back between runs", { status: s.status }, "interrupt it first, or wait for its run to end");
    }
    const children = store.tree.liveChildren(s.id);
    if (children.length > 0) {
      throw finError("SESSION_BUSY", "the session has live children", { reason: "liveChildren", children: [...children] }, "end its children first (`fin agent stop`, `finisherd agent end`)");
    }
    const runs = store.runs(s.id);
    const last = runs.at(-1)?.seq ?? 0;
    const target = runs.find((r) => r.seq === toRun);
    if (toRun >= last || (toRun > 0 && target === undefined)) {
      throw finError("INVALID_REQUEST", `the session has no run after run ${toRun} to roll back`, { field: "toRun", toRun, lastRun: last }, `give a run before its last (${last})`);
    }
    if (target !== undefined && store.rollbacks.runs(s.id).has(target.id)) {
      throw finError("INVALID_REQUEST", `run ${toRun} was rolled back already`, { field: "toRun", toRun, reason: "rolledBack" }, "roll back to a run before it");
    }
  }

  /** `s` back to the end of run `toRun` (`check` first, in the session's queue). */
  async rollback(s: Session, toRun: number, by: Principal, meta: CallMeta, ops: ConversationOps): Promise<RollbackResult> {
    this.check(s, toRun);
    const undone = await this.#undo(s, toRun, meta);
    const units = undone.flatMap((u) => u.units);
    this.#d.outcomes.observeRollback(s, undone.flatMap((u) => u.origins), toRun, by);
    const branches = undone.map((u) => u.branch);
    const conversation = await this.#conversation(s, toRun, { undone: units, branches }, ops);
    return { undone: units, branches, conversation };
  }

  /** The later runs' transactions on each branch, newest branch first. */
  async #shares(s: Session, toRun: number): Promise<BranchShare[]> {
    const { store, registry } = this.#d;
    const runs = store.runs(s.id);
    const seqOf = new Map(runs.map((r) => [r.id, r.seq] as const));
    const later = (t: SessionTx) => (t.runId !== undefined ? (seqOf.get(t.runId) ?? 0) : runAt(runs, t.at)) > toRun;
    const archived = new Set((await run(registry.branches(s.projectId, true))).branches.filter((b) => b.state === "archived").map((b) => b.id));
    const shares = new Map<string, BranchShare>();
    for (const t of store.txsOfSession(s.id).filter(later)) {
      const got = await run(registry.txGet(s.projectId, t.txId));
      if (got === null || !("tx" in got)) continue;
      const branch = got.tx.branch;
      if (archived.has(branch)) continue;
      const share = shares.get(branch);
      if (share === undefined) shares.set(branch, { branch, afterSeq: got.rev - 1, newest: { at: t.at, txId: t.txId } });
      else {
        share.afterSeq = Math.min(share.afterSeq, got.rev - 1);
        if (t.at > share.newest.at || (t.at === share.newest.at && t.txId > share.newest.txId)) share.newest = { at: t.at, txId: t.txId };
      }
    }
    return [...shares.values()].sort((a, b) => b.newest.at - a.newest.at || (a.newest.txId < b.newest.txId ? 1 : -1));
  }

  /** One undo record per branch; on a failure the ones made are redone and it is answered. */
  async #undo(s: Session, toRun: number, meta: CallMeta): Promise<Undone[]> {
    const { registry, ids } = this.#d;
    const author = sessionAuthor(s);
    const done: Undone[] = [];
    for (const share of await this.#shares(s, toRun)) {
      const payload = { projectId: s.projectId, txId: ids.ulid(), author, branch: share.branch, afterSeq: share.afterSeq };
      try {
        const r = (await run(registry.history(s.projectId, "undo", payload, meta))) as { undone?: string[]; origins?: string[] };
        done.push({ branch: share.branch, units: r.undone ?? [], origins: r.origins ?? [] });
      } catch (e) {
        if (codeOf(e) === "NOTHING_TO_UNDO") continue;
        const redoFailed: Array<{ branch: string; code: string }> = [];
        for (const u of [...done].reverse()) {
          try {
            await run(registry.history(s.projectId, "redo", { projectId: s.projectId, txId: ids.ulid(), author, branch: u.branch }, meta));
          } catch (r) {
            redoFailed.push({ branch: u.branch, code: codeOf(r) ?? "INTERNAL" });
          }
        }
        const err = e as FinError;
        const details = { ...((err.details ?? {}) as object), sessionId: s.id, toRun, branch: share.branch, redone: done.map((u) => u.branch), ...(redoFailed.length === 0 ? {} : { redoFailed }) };
        throw finError(
          err.code ?? "INTERNAL",
          `the rollback of session ${s.id} to run ${toRun} stopped on branch ${share.branch}: ${err.message}${redoFailed.length === 0 ? "; every branch is as it was" : ""}`,
          details,
          err.fix ?? "resolve the conflict on that branch (or undo the later edit), then roll back again",
        );
      }
    }
    return done;
  }

  async #conversation(s: Session, toRun: number, txs: { undone: readonly string[]; branches: readonly string[] }, ops: ConversationOps): Promise<"rolledBack" | "kept"> {
    const { store } = this.#d;
    const lastRun = store.runs(s.id).at(-1)!;
    const note = (conversation: "rolledBack" | "kept", extra: Record<string, unknown> = {}) =>
      ops.append({ sessionId: s.id, runId: lastRun.id, kind: "notice", payload: { rollback: { toRun, undone: txs.undone.length, branches: [...txs.branches], conversation, ...extra } } });
    const kept = async (reason: string, message: string) => {
      await note("kept", { reason, message });
      return "kept" as const;
    };
    // The later runs are marked whatever the provider does: their work is undone.
    store.rollbacks.mark(s.id, toRun);
    const caps = await ops.caps(s).catch(() => undefined);
    if (caps?.rollback !== true) {
      return kept("noProviderRollback", `the ${s.harness} provider cannot roll back: its own history keeps the runs after run ${toRun}, whose transactions were undone`);
    }
    if (s.harness === "builtin") {
      const live = ops.live(s.id);
      if (live !== undefined) await call(live.rollback(toRun));
      await note("rolledBack");
      return "rolledBack";
    }
    let live = ops.live(s.id);
    const opened = live === undefined;
    try {
      live ??= await ops.open(s);
    } catch (e) {
      return kept(codeOf(e) ?? "PROVIDER_UNAVAILABLE", `the provider could not be opened to roll back: its history keeps the runs after run ${toRun}`);
    }
    const err = await call(live.rollback(toRun));
    if (opened && store.get(s.id)?.status !== "idle") await ops.close(s.id);
    if (err !== null) return kept(err.code, `the provider did not roll back (${err.message}): its history keeps the runs after run ${toRun}`);
    await note("rolledBack");
    return "rolledBack";
  }
}

/** The seq of the run a row without `runId` (before agent 0008) was made in, by time. */
function runAt(runs: ReadonlyArray<Run>, at: number): number {
  let seq = 0;
  for (const r of runs) if (r.startedAt <= at) seq = r.seq;
  return seq;
}
