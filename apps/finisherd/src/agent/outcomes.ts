// The outcome tracker (plan 7A Task 11, Decision 27; spec §12.5): what became of each agent
// transaction outside a review item. It observes the history RPC results inside `finisherd`
// (the chain's after stage, `outcomeHooks`), branch archiving and session deletion, joins them
// to `session_txs` (which outlive their sessions), and records each outcome once per
// `(txId, via)` in the ledger (`session_tx_outcomes`; the newest in `session_txs.outcome`):
//
// - `history.merge` / `history.cherryPick` copying an agent transaction (the result's
//   `origins`: what it copied, with the origin chains of copies) onto a branch outside that
//   session's subtree → `merged`, `via: merge`; inside the subtree, nothing (for a child, 7B:
//   the subtree of its root session's root, so a parent's merge of its child is no outcome);
// - `history.undo` / `history.revert` naming one (`origins`: an undo's units, a reverted
//   group's records, with their origin chains) → `undone` / `reverted`, `via: undo|revert`, or
//   `via: critic` when a critic session did it;
// - `branch.set {archived: true}` of a session's root, or `agent.session.delete` → each
//   transaction no merge carried out → `abandoned`, `via: archive`;
// - `by`: `agent` for an agent session (the session itself or another), `critic` for a critic
//   session, `user` for a person (`local`, `console`, `device`);
// - a rollback's undos (7B Task 8, `rollback.ts`; not through the chain) → `undone`, `via: undo`,
//   with `details.rollback {toRun, by}` (`by` the principal: a person, or the parent session).
//
// **It defines no verdict kind of its own.** 5E's `review.verdict` is the single source: once
// that kind has a version a `source: agent` entry may carry (`kindVersion`, a lookup each
// time, not a flag), each new outcome is emitted with `itemKind: none`, `via`, `sessionId` and
// `openS` (seconds since the transaction, from the injected clock). Until then outcomes are
// only stored. A review's own merge (`review.merge`, 5E) is not a history RPC, so the tracker
// never sees it and 5E's verdict stands alone. `via: autoMerge` / `by: policy` come with
// sub-project 10 (enabling the policy is `AUTO_MERGE_UNAVAILABLE` until then).
//
// The tracker never fails a call: the write has landed. What it cannot record is reported on
// stderr.
import type { Principal, Session } from "@finisher/contracts";
import type { CommittedTx } from "@finisher/core-types";
import { type Journal, kindVersion } from "@finisher/fin-node";
import { Effect } from "effect";
import type { ChainHooks } from "../chain/hooks.ts";
import type { ChainProjects } from "../chain/resolve.ts";
import { committedOf } from "../history.ts";
import type { BranchLineage } from "./reach.ts";
import type { OpenTx, OutcomeBy, OutcomeVia } from "./store-outcomes.ts";
import type { AgentStore } from "./store.ts";
import type { TxOutcome } from "./store-tables.ts";

export const VERDICT_KIND = "review.verdict";
/** The role whose sessions judge others' work (sub-project 10's critic, spec §6.2). */
export const CRITIC_ROLE = "critic";

/** One `review.verdict` the tracker emits (spec §1.7 item 5): one transaction each. */
export interface OutcomeVerdict {
  readonly verdict: TxOutcome;
  readonly txIds: readonly [string];
  readonly by: OutcomeBy;
  readonly via: OutcomeVia;
  readonly itemKind: "none";
  readonly sessionId: string;
  readonly openS: number;
}

/** Where verdicts go: 5E's kind, when it exists. */
export interface VerdictSink {
  /** `review.verdict`'s payload version when the kind is registered for `agent`, else null. */
  version(): number | null;
  /** True when the entry was handed to the journal. */
  emit(v: OutcomeVerdict, projectId: string | undefined, version: number): boolean;
}

/** Verdicts into the diagnostics journal, as `source: agent`; `lookup` is fin-core's registry. */
export function journalVerdicts(journal: Pick<Journal, "enabled" | "record">, lookup: typeof kindVersion = kindVersion): VerdictSink {
  return {
    version: () => lookup(VERDICT_KIND, "agent"),
    emit: (v, projectId, version) => {
      if (!journal.enabled) return false;
      const ctx = { ...(projectId === undefined ? {} : { project: projectId }), txId: v.txIds[0], actor: { kind: "agent", session: v.sessionId } };
      journal.record(VERDICT_KIND, version, JSON.stringify(ctx), JSON.stringify(v), "agent");
      return true;
    },
  };
}

export interface OutcomeDeps {
  readonly store: AgentStore;
  readonly lineage: BranchLineage;
  readonly verdicts: VerdictSink;
  readonly clock: () => number;
  readonly warn?: (message: string) => void;
}

type Origins = { readonly origins?: ReadonlyArray<string>; readonly dryRun?: boolean };

const warnStderr = (m: string) => void process.stderr.write(`finisherd: warning: ${m}\n`);

export class OutcomeTracker {
  readonly #d: OutcomeDeps;
  readonly #warn: (m: string) => void;

  constructor(deps: OutcomeDeps) {
    this.#d = deps;
    this.#warn = deps.warn ?? warnStderr;
  }

  /**
   * One history RPC's answer (`rpc` its tag; `projects` the chain's, for the branch index). A
   * dry run, a failed call and any other tag change nothing.
   */
  observeHistory(rpc: string, payload: unknown, result: unknown, principal: Principal, projects: ChainProjects): Effect.Effect<void> {
    const p = (payload ?? {}) as { projectId?: unknown; dryRun?: unknown };
    const r = (result ?? {}) as Origins;
    if (typeof p.projectId !== "string" || p.dryRun === true || r.dryRun === true) return Effect.void;
    const projectId = p.projectId;
    const by = this.#byOf(principal);
    const origins = r.origins ?? [];
    const work: Effect.Effect<void, unknown> = (() => {
      switch (rpc) {
        case "history.merge":
        case "history.cherryPick": {
          const target = committedOf(result)[0]?.tx.branch;
          return target === undefined ? Effect.void : this.#merged(projectId, origins, target, by, projects);
        }
        case "history.undo":
          return Effect.sync(() => this.#took(projectId, origins, "undone", by === "critic" ? "critic" : "undo", by));
        case "history.revert":
          return Effect.sync(() => this.#took(projectId, origins, "reverted", by === "critic" ? "critic" : "revert", by));
        case "branch.set": {
          const link = (result as CommittedTx | null)?.link;
          return link?.kind === "branchSet" && link.archived === true
            ? Effect.sync(() => this.observeBranch({ projectId, branchId: link.branchId, archived: true, principal }))
            : Effect.void;
        }
        default:
          return Effect.void;
      }
    })();
    return Effect.catchCause(work, (cause) => Effect.sync(() => this.#warn(`cannot record the outcome of ${rpc}: ${String(cause)}`)));
  }

  /**
   * The undos of a rollback of `s` to run `toRun` by `principal` (7B Task 8): `origins` their
   * units' records, as an undo's result names them. Never fails: the undos have landed.
   */
  observeRollback(s: Session, origins: ReadonlyArray<string>, toRun: number, principal: Principal): void {
    try {
      const by = this.#byOf(principal);
      const details = { rollback: { toRun, by: `${principal.kind}:${principal.id}` } };
      this.#took(s.projectId, origins, "undone", by === "critic" ? "critic" : "undo", by, details);
    } catch (e) {
      this.#warn(`cannot record session ${s.id}'s rollback outcomes: ${String(e)}`);
    }
  }

  /** A branch's status changed: archiving a session's root abandons its unmerged transactions. */
  observeBranch(e: { readonly projectId: string; readonly branchId: string; readonly archived: boolean; readonly principal: Principal }): void {
    if (!e.archived) return;
    const by = this.#byOf(e.principal);
    for (const t of this.#d.store.outcomes.unmergedOfRoot(e.projectId, e.branchId)) this.#record(t, e.projectId, "abandoned", "archive", by);
  }

  /**
   * `agent.session.delete` (owner): its unmerged transactions are abandoned. Called before the
   * session's product state goes; its `session_txs` rows stay either way.
   */
  observeSessionDeleted(s: Session): void {
    try {
      for (const t of this.#d.store.outcomes.unmergedOfSession(s.id)) this.#record(t, s.projectId, "abandoned", "archive", "user");
    } catch (e) {
      this.#warn(`cannot record session ${s.id}'s abandoned transactions: ${String(e)}`);
    }
  }

  /** Copies onto `target`: each agent transaction among them lands outside its subtree or not. */
  #merged(projectId: string, origins: ReadonlyArray<string>, target: string, by: OutcomeBy, projects: ChainProjects): Effect.Effect<void, unknown> {
    const rows = [...this.#d.store.sessionOfTx(origins).values()];
    if (rows.length === 0) return Effect.void;
    return Effect.gen({ self: this }, function* () {
      const inside = new Map<string, boolean>();
      for (const row of rows) {
        const s = this.#d.store.get(row.sessionId);
        const top = s?.rootSession === undefined ? undefined : this.#d.store.get(s.rootSession);
        const root = top?.rootBranch ?? s?.rootBranch ?? row.rootBranch;
        if (root === undefined) continue;
        let within = inside.get(root);
        if (within === undefined) {
          within = (yield* this.#d.lineage.subtree(projects, projectId, root, [target])).has(target);
          inside.set(root, within);
        }
        if (!within) this.#record(row, projectId, "merged", "merge", by);
      }
    });
  }

  /** An undo or revert took these back. */
  #took(projectId: string, origins: ReadonlyArray<string>, verdict: TxOutcome, via: OutcomeVia, by: OutcomeBy, details?: Record<string, unknown>): void {
    for (const row of this.#d.store.sessionOfTx(origins).values()) this.#record(row, projectId, verdict, via, by, details);
  }

  #record(t: OpenTx, projectId: string | undefined, verdict: TxOutcome, via: OutcomeVia, by: OutcomeBy, details?: Record<string, unknown>): void {
    const at = this.#d.clock();
    const ledger = this.#d.store.outcomes;
    if (!ledger.record({ txId: t.txId, via, verdict, by, sessionId: t.sessionId, at, ...(details === undefined ? {} : { details }) })) return;
    const version = this.#d.verdicts.version();
    if (version === null) return;
    const v: OutcomeVerdict = { verdict, txIds: [t.txId], by, via, itemKind: "none", sessionId: t.sessionId, openS: Math.max(0, Math.floor((at - t.at) / 1000)) };
    try {
      if (this.#d.verdicts.emit(v, projectId, version)) ledger.markEmitted(t.txId, via);
    } catch (e) {
      this.#warn(`the journal refused the review.verdict of ${t.txId}: ${String(e)}`);
    }
  }

  #byOf(p: Principal): OutcomeBy {
    if (p.kind === "agent") return this.#d.store.get(p.id)?.role === CRITIC_ROLE ? "critic" : "agent";
    return p.kind === "mcp" ? "agent" : "user";
  }
}

const OBSERVED: ReadonlySet<string> = new Set(["history.merge", "history.cherryPick", "history.undo", "history.revert", "branch.set"]);

/**
 * The chain's after stage for the tracker, run after `first` (an agent session's own writes
 * are in `session_txs` before their outcomes are looked for).
 */
export function withOutcomes(hooks: ChainHooks, tracker: OutcomeTracker, projects: ChainProjects): ChainHooks {
  const first = hooks.after;
  const after: ChainHooks["after"] = (tag, principal, payload, result, headers) =>
    Effect.andThen(first === undefined ? Effect.void : first(tag, principal, payload, result, headers), () =>
      OBSERVED.has(tag) ? tracker.observeHistory(tag, payload, result, principal, projects) : Effect.void,
    );
  return { ...hooks, after };
}
