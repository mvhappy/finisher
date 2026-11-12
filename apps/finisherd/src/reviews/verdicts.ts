// `review.verdict` entries of the review workflow (5E plan Decision 20, spec §3.4): one entry per
// `review.decide` batch (accepted, rejected), per `requestChanges`, per merge and per close,
// recorded as `source: finisherd`. An entry names at most 64 transactions
// (`MAX_VERDICT_TX_IDS`); a decision on more is several entries, each counting its own.
//
// Review text is content: the only text an entry may carry is `reason`, and only while training
// is in force (the journal strips it otherwise too). Titles, summaries and comments never go in.
// Emitting never fails a call: the decision has landed; what the journal refuses is a warning.
import type { Principal, ReviewItem } from "@finisher/contracts";
import { type Journal, kindVersion } from "@finisher/fin-node";

export const VERDICT_KIND = "review.verdict";
/** fin-core's `MAX_VERDICT_TX_IDS`. */
export const MAX_VERDICT_TX_IDS = 64;

export type ReviewVerdictValue = "accepted" | "rejected" | "merged" | "dismissed" | "changesRequested";
export type ReviewVerdictBy = "user" | "agent" | "critic" | "policy";
export type ReviewVerdictVia = "undo" | "revert" | "archive" | "merge" | "critic" | "autoMerge";

/** The wire form of fin-core's `ReviewVerdictIn` the workflow records. */
export interface ReviewVerdictPayload {
  readonly itemKind: ReviewItem["kind"];
  readonly verdict: ReviewVerdictValue;
  readonly via?: ReviewVerdictVia;
  readonly by: ReviewVerdictBy;
  readonly txIds: ReadonlyArray<string>;
  readonly accepted: number;
  readonly rejected: number;
  readonly sessionId?: string;
  readonly openS: number;
  /** Training only. */
  readonly reason?: string;
}

/** `user` for a person and `local`; `agent` for agent and MCP credentials (Decision 20). */
export function verdictBy(p: Pick<Principal, "kind">): ReviewVerdictBy {
  return p.kind === "agent" || p.kind === "mcp" ? "agent" : "user";
}

export interface VerdictInput {
  readonly item: ReviewItem;
  readonly verdict: ReviewVerdictValue;
  readonly by: ReviewVerdictBy;
  readonly via?: ReviewVerdictVia;
  readonly txIds: ReadonlyArray<string>;
  /** Which count the txIds are: an accepted batch or a merge counts `accepted`, a rejection `rejected`. */
  readonly counts: "accepted" | "rejected" | "none";
  readonly sessionId?: string | undefined;
  readonly reason?: string | undefined;
  /** Milliseconds since the epoch: `openS` is the seconds since the item opened. */
  readonly now: number;
}

/** The entries of one verdict: `txIds` in chunks of at most 64, each counting its own; one entry when there are none. */
export function verdictEntries(v: VerdictInput): ReadonlyArray<ReviewVerdictPayload> {
  const openS = Math.max(0, Math.floor((v.now - v.item.createdAt) / 1000));
  const chunks: Array<ReadonlyArray<string>> = [];
  for (let i = 0; i < v.txIds.length; i += MAX_VERDICT_TX_IDS) chunks.push(v.txIds.slice(i, i + MAX_VERDICT_TX_IDS));
  if (chunks.length === 0) chunks.push([]);
  return chunks.map((txIds) => ({
    itemKind: v.item.kind,
    verdict: v.verdict,
    ...(v.via === undefined ? {} : { via: v.via }),
    by: v.by,
    txIds,
    accepted: v.counts === "accepted" ? txIds.length : 0,
    rejected: v.counts === "rejected" ? txIds.length : 0,
    ...(v.sessionId === undefined ? {} : { sessionId: v.sessionId }),
    openS,
    ...(v.reason === undefined ? {} : { reason: v.reason }),
  }));
}

/** Where the workflow's verdicts go. */
export interface ReviewVerdictSink {
  /** `actor`: the call's (`actorFor`), the entry's context. */
  emit(projectId: string, actor: object | undefined, entries: ReadonlyArray<ReviewVerdictPayload>): void;
}

export const NO_VERDICTS: ReviewVerdictSink = { emit: () => undefined };

type VerdictJournal = Pick<Journal, "enabled" | "training" | "record" | "recordsProject">;

/**
 * Verdicts into the diagnostics journal as `source: finisherd`. `reason` is kept only while
 * training is in force; nothing is built while recording is off or the project's collection is
 * `never`. `lookup` is fin-core's kind registry.
 */
export function journalReviewVerdicts(journal: VerdictJournal, lookup: typeof kindVersion = kindVersion, warn: (m: string) => void = warnStderr): ReviewVerdictSink {
  return {
    emit: (projectId, actor, entries) => {
      try {
        if (!journal.enabled || !journal.recordsProject(projectId)) return;
        const version = lookup(VERDICT_KIND, "finisherd");
        if (version === null) return;
        const ctx = JSON.stringify({ project: projectId, ...(actor === undefined ? {} : { actor }) });
        for (const e of entries) {
          const { reason, ...rest } = e;
          const payload = reason !== undefined && journal.training ? { ...rest, reason } : rest;
          journal.record(VERDICT_KIND, version, ctx, JSON.stringify(payload), "finisherd");
        }
      } catch (e) {
        warn(`the journal refused a review.verdict: ${String(e)}`);
      }
    },
  };
}

const warnStderr = (m: string) => void process.stderr.write(`finisherd: warning: ${m}\n`);
