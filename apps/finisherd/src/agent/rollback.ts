// Rolling a session back to a run (sub-project 7 §5.3, §4.2; plan 7B Decision 23, Review Focus 5).
//
// - **Who:** people, and a parent session for its own direct children (the stage-4 check in
//   `reach.ts`). Refused while the session runs (`SESSION_BUSY {status}`), while it has a live
//   child (`SESSION_BUSY {reason: liveChildren}`: the child's work would stand on what is undone)
//   and once it has ended (`SESSION_ENDED`). `toRun` is a run's seq (0: before the first); the
//   session's last run, a later one and a run a rollback already took back are `INVALID_REQUEST`.
// - **Its transactions:** the `session_txs` rows of the runs after `toRun` (by `runId`, agent
//   0008; an older row by time), grouped by the branch each landed on (`tx.get`), a branch whose
//   newest row is newest first. Its later undos and redos are reversed too: each branch's plan
//   (`rollback-plan.ts`, before anything is written) redoes the later runs' undo records that
//   took back what was there at the end of `toRun` (plain 4B redos, each checked by a dry run),
//   then **one** 4B undo record (`undo` with `afterSeq`, the seq before the branch's first such
//   row: every unit of the session's author made after it, as `undo --to` takes them after a
//   checkpoint). All under the session's author, with a guard of the session's reach (its
//   subtree less its live children's) and the caller's credential. A branch whose history cannot
//   be taken back exactly with these (an undo a later edit cleared off the redo stack) is
//   `CONFLICT {reason: notReversible, why, tx}` before any write. A branch with nothing left to
//   undo (`NOTHING_TO_UNDO`: the later runs' undos and redos cancel out) is not written; an
//   archived one is left as it was and named (`archived`, in the answer and the notice).
// - **All or nothing:** a failure on any branch (`CONFLICT`: a person's later edit the undo
//   cannot be carried over) reverses what was written, newest first (a redo reverses an undo
//   record whole, an undo a redo), and the call answers that failure with `details.branch` and
//   `redone`; every branch is as it was. When a reversal fails too, the same code answers with
//   `details.partial: true`, `redoFailed` ({branch, code}) and `undone` (that branch's units,
//   still undone: their outcomes are recorded).
// - **Outcomes:** each undone transaction is `undone`, `via: undo`, `by` the caller's
//   (`user`, or `agent` for a parent), with `details.rollback {toRun, by: <kind>:<id>}`. What a
//   redo brings back (`restored`: the later runs' undo records redone) records none.
// - **The conversation:** the later runs' nodes are marked `rolledBack` (their work is undone;
//   a run marked so is no `toRun` again). A provider with the `rollback` capability rolls back
//   to the turn (`ProviderSession.rollback(toRun)`, its instance opened for it when none is);
//   for the built-in loop the marks are the rollback (its next request leaves them out,
//   `historyFromNodes`). Without the capability, or when the provider fails, the provider's
//   history stays and a `notice` says so (spec §4.2: a CLI's rollback undoes the transactions
//   and resumes). Runs and nodes are never removed; the next run takes the next seq. A
//   `notice {rollback: …}` in the last run records every rollback. Once the undos landed the call
//   succeeds: marking the runs or writing the notice failing (once more after a retry) is a
//   `warning` in the answer.
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
import { type BranchPlan, type LaterRec, planBranch } from "./rollback-plan.ts";
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
  /** The later runs' undo records redone (what they took back is back). */
  readonly restored?: readonly string[];
  /** Branches with later-run transactions left as they were: archived. */
  readonly archived?: readonly string[];
  /** What failed after the undos landed (the marks, the notice). */
  readonly warning?: string;
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

/** One branch's share: its later-run records, and how new the newest is. */
interface BranchShare {
  readonly branch: string;
  readonly recs: LaterRec[];
  newest: { at: number; txId: string };
}

/** A record the rollback wrote: an undo (its units and their origins) or a redo of a later-run undo. */
type Step = { readonly branch: string; readonly kind: "undo" | "redo"; readonly txId: string; readonly units: readonly string[]; readonly origins: readonly string[] };

type HistoryOut = { undone?: string[]; origins?: string[] };

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
    const { shares, archived } = await this.#shares(s, toRun);
    const plans = shares.map((sh) => {
      try {
        return { branch: sh.branch, plan: planBranch(sh.branch, sh.recs) };
      } catch (e) {
        const err = e as FinError;
        throw finError(err.code, err.message, { ...((err.details ?? {}) as object), sessionId: s.id, toRun }, err.fix);
      }
    });
    const steps = await this.#write(s, toRun, plans, by, meta);
    const undos = steps.filter((x) => x.kind === "undo");
    const undone = undos.flatMap((x) => x.units);
    this.#d.outcomes.observeRollback(s, undos.flatMap((x) => x.origins), toRun, by);
    const branches = [...new Set(steps.map((x) => x.branch))];
    const restored = steps.filter((x) => x.kind === "redo").flatMap((x) => x.units);
    const txs = { undone, branches, restored, archived };
    const { conversation, warning } = await this.#conversation(s, toRun, txs, ops);
    return { undone, branches, conversation, ...(restored.length === 0 ? {} : { restored }), ...(archived.length === 0 ? {} : { archived }), ...(warning === undefined ? {} : { warning }) };
  }

  /** The later runs' records on each branch, newest branch first; the archived branches left alone. */
  async #shares(s: Session, toRun: number): Promise<{ shares: BranchShare[]; archived: string[] }> {
    const { store, registry } = this.#d;
    const runs = store.runs(s.id);
    const seqOf = new Map(runs.map((r) => [r.id, r.seq] as const));
    const later = (t: SessionTx) => (t.runId !== undefined ? (seqOf.get(t.runId) ?? 0) : runAt(runs, t.at)) > toRun;
    const isArchived = new Set((await run(registry.branches(s.projectId, true))).branches.filter((b) => b.state === "archived").map((b) => b.id));
    const archived = new Set<string>();
    const shares = new Map<string, BranchShare>();
    for (const t of store.txsOfSession(s.id).filter(later)) {
      const got = await run(registry.txGet(s.projectId, t.txId));
      if (got === null || !("tx" in got)) continue;
      const branch = got.tx.branch;
      if (isArchived.has(branch)) {
        archived.add(branch);
        continue;
      }
      const rec: LaterRec = { txId: t.txId, rev: got.rev, link: got.link ?? null };
      const share = shares.get(branch);
      if (share === undefined) shares.set(branch, { branch, recs: [rec], newest: { at: t.at, txId: t.txId } });
      else {
        share.recs.push(rec);
        if (t.at > share.newest.at || (t.at === share.newest.at && t.txId > share.newest.txId)) share.newest = { at: t.at, txId: t.txId };
      }
    }
    const sorted = [...shares.values()].sort((a, b) => b.newest.at - a.newest.at || (a.newest.txId < b.newest.txId ? 1 : -1));
    return { shares: sorted, archived: [...archived] };
  }

  /** Each branch's plan written (redos, then one undo); on a failure what was written is reversed and it is answered. */
  async #write(s: Session, toRun: number, plans: ReadonlyArray<{ branch: string; plan: BranchPlan }>, by: Principal, meta: CallMeta): Promise<Step[]> {
    const { registry, ids } = this.#d;
    const author = sessionAuthor(s);
    const history = async (op: "undo" | "redo", branch: string, extra: Record<string, unknown> = {}) => {
      const txId = ids.ulid();
      const r = (await run(registry.history(s.projectId, op, { projectId: s.projectId, txId, author, branch, ...extra }, meta))) as HistoryOut;
      return { txId, units: r.undone ?? [], origins: r.origins ?? [] };
    };
    const steps: Step[] = [];
    let at = "";
    try {
      for (const { branch, plan } of plans) {
        at = branch;
        for (const u of plan.redo) {
          // The plan's redo must be the one the engine takes: a dry run first, nothing written otherwise.
          const dry = await history("redo", branch, { dryRun: true });
          if (dry.units.length !== 1 || dry.units[0] !== u) {
            throw finError("CONFLICT", `the redo on branch ${branch} would reverse ${dry.units.join(", ") || "nothing"}, not the session's undo ${u}`, { reason: "notReversible", why: "notPlanned", branch, tx: u, got: dry.units });
          }
          steps.push({ branch, kind: "redo", ...(await history("redo", branch)) });
        }
        // After redos nothing of the later runs is left to undo (`rollback-plan.ts`); a dry run checks it,
        // so a branch's steps are all redos or one undo and reverse newest first.
        const after = { afterSeq: plan.afterSeq, ...(plan.redo.length > 0 ? { dryRun: true } : {}) };
        try {
          const r = await history("undo", branch, after);
          if (plan.redo.length > 0) {
            throw finError("CONFLICT", `branch ${branch} has units of the later runs left after its redos`, { reason: "notReversible", why: "notPlanned", branch, got: r.units });
          }
          steps.push({ branch, kind: "undo", ...r });
        } catch (e) {
          if (codeOf(e) !== "NOTHING_TO_UNDO") throw e;
        }
      }
      return steps;
    } catch (e) {
      return await this.#restore(s, toRun, steps, at, e as FinError, by, history);
    }
  }

  /**
   * The steps written reversed, newest first (an undo by a redo, a redo by an undo): the failure
   * `e` on `branch` with `redone` (the branches as they were). A branch whose reversal fails stays
   * as far as it got: `details.partial`, `redoFailed`, and its units still undone (`undone`, their
   * outcomes recorded).
   */
  async #restore(
    s: Session,
    toRun: number,
    steps: readonly Step[],
    branch: string,
    e: FinError,
    by: Principal,
    history: (op: "undo" | "redo", branch: string) => Promise<unknown>,
  ): Promise<never> {
    const redoFailed: Array<{ branch: string; code: string }> = [];
    const left: Step[] = [];
    for (const x of [...steps].reverse()) {
      if (redoFailed.some((f) => f.branch === x.branch)) {
        left.push(x);
        continue;
      }
      try {
        await history(x.kind === "undo" ? "redo" : "undo", x.branch);
      } catch (r) {
        redoFailed.push({ branch: x.branch, code: codeOf(r) ?? "INTERNAL" });
        left.push(x);
      }
    }
    const undos = left.filter((x) => x.kind === "undo");
    if (undos.length > 0) this.#d.outcomes.observeRollback(s, undos.flatMap((x) => x.origins), toRun, by);
    const redone = [...new Set(steps.map((x) => x.branch))].filter((b) => !redoFailed.some((f) => f.branch === b));
    const partial = redoFailed.length === 0 ? {} : { partial: true, redoFailed, undone: undos.flatMap((x) => x.units) };
    const details = { ...((e.details ?? {}) as object), sessionId: s.id, toRun, branch, redone, ...partial };
    const tail = redoFailed.length === 0 ? "; every branch is as it was" : `; ${redoFailed.map((f) => f.branch).join(", ")} could not be restored (partial)`;
    throw finError(e.code ?? "INTERNAL", `the rollback of session ${s.id} to run ${toRun} stopped on branch ${branch}: ${e.message}${tail}`, details, e.fix ?? "resolve the conflict on that branch (or undo the later edit), then roll back again");
  }

  async #conversation(
    s: Session,
    toRun: number,
    txs: { undone: readonly string[]; branches: readonly string[]; restored: readonly string[]; archived: readonly string[] },
    ops: ConversationOps,
  ): Promise<{ conversation: "rolledBack" | "kept"; warning?: string }> {
    const { store } = this.#d;
    const failed: string[] = [];
    // The transactions are undone: what follows never fails the call; a failed write is retried once, then a warning.
    const twice = async (what: string, f: () => unknown) => {
      for (let k = 0; ; k++) {
        try {
          return void (await f());
        } catch (e) {
          if (k === 1) return void failed.push(`${what}: ${e instanceof Error ? e.message : String(e)}`);
        }
      }
    };
    const lastRun = store.runs(s.id).at(-1)!;
    const counts = { undone: txs.undone.length, branches: [...txs.branches], ...(txs.restored.length === 0 ? {} : { restored: txs.restored.length }), ...(txs.archived.length === 0 ? {} : { archived: [...txs.archived] }) };
    const note = (conversation: "rolledBack" | "kept", extra: Record<string, unknown> = {}) =>
      twice("the rollback notice", () => ops.append({ sessionId: s.id, runId: lastRun.id, kind: "notice", payload: { rollback: { toRun, ...counts, conversation, ...extra } } }));
    const done = (conversation: "rolledBack" | "kept") => (failed.length === 0 ? { conversation } : { conversation, warning: `the rollback's transactions are undone, but ${failed.join("; ")}` });
    const kept = async (reason: string, message: string) => {
      await note("kept", { reason, message });
      return done("kept");
    };
    // The later runs are marked whatever the provider does: their work is undone.
    await twice("marking the rolled-back runs", () => store.rollbacks.mark(s.id, toRun));
    try {
      const caps = await ops.caps(s).catch(() => undefined);
      if (caps?.rollback !== true) {
        return await kept("noProviderRollback", `the ${s.harness} provider cannot roll back: its own history keeps the runs after run ${toRun}, whose transactions were undone`);
      }
      if (s.harness === "builtin") {
        const live = ops.live(s.id);
        if (live !== undefined) await call(live.rollback(toRun));
        await note("rolledBack");
        return done("rolledBack");
      }
      let live = ops.live(s.id);
      const opened = live === undefined;
      try {
        live ??= await ops.open(s);
      } catch (e) {
        return await kept(codeOf(e) ?? "PROVIDER_UNAVAILABLE", `the provider could not be opened to roll back: its history keeps the runs after run ${toRun}`);
      }
      const err = await call(live.rollback(toRun));
      if (opened && store.get(s.id)?.status !== "idle") await ops.close(s.id).catch((e: unknown) => void failed.push(`closing the provider: ${e instanceof Error ? e.message : String(e)}`));
      if (err !== null) return await kept(err.code, `the provider did not roll back (${err.message}): its history keeps the runs after run ${toRun}`);
      await note("rolledBack");
      return done("rolledBack");
    } catch (e) {
      failed.push(`the provider's rollback: ${e instanceof Error ? e.message : String(e)}`);
      return done("kept");
    }
  }
}

/** The seq of the run a row without `runId` (before agent 0008) was made in, by time. */
function runAt(runs: ReadonlyArray<Run>, at: number): number {
  let seq = 0;
  for (const r of runs) if (r.startedAt <= at) seq = r.seq;
  return seq;
}
