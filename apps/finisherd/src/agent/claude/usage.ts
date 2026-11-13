// Claude Code's usage and cost as the runtime's `usage` events (plan 7D Task 9, Decision 15;
// Review Focus 4: budgets hold against a process whose model calls Finisher cannot see).
//
// - **Live:** each model call's `stream_event` usage (`message_start`: the input side,
//   `message_delta`: the message's output so far) is a `usage` event at once, with no money
//   (`costMicros: 0`), so the runtime's token budget sees a turn's spend while it runs and stops
//   the session mid-turn (its budget stop: the interrupt, `BUDGET_EXCEEDED`, a `budget` card).
// - **The `result`:** its `usage` and `total_cost_usd` are cumulative per process; the event is
//   what they add beyond what this instance already emitted (live or at earlier `result`s), so
//   the `result` settles the turn's money and any tokens the stream did not show.
// - **Across instances:** whether `--resume` restores the CLI's totals is unverified, so a
//   resumed instance fails closed: everything its totals report is charged, as if they had
//   reset (`resumeTotals: reset`, the default). If they are restored, the session's earlier
//   usage is charged again on each resume: an over-charge, never an under-charge, which no
//   heuristic can rule out (a reset turn can report exactly the earlier totals). Task 0 pins
//   `restored`, which counts from the session's charged usage (`baseline`, the store's running
//   total, crash-consistent with the ledger).
// - **The basis:** `subscription` for a plan sign-in (money shown, never charged; spec §13.1),
//   else `reported` (the CLI's own estimate). `modelUsage` gives a per-model split, recorded as a
//   `notice {modelUsage}` per `result`.
//
// The runtime charges a turn's tail (the `result` an interrupt, a budget stop or a replacement
// left) to the run its instance served (`runtime-usage.ts`), so every delta emitted here is
// charged exactly once.
import type { ProviderEvent } from "@finisher/contracts";
import type { ResultMsg, StreamPayload } from "./protocol.ts";

type UsageEvent = Extract<ProviderEvent, { type: "usage" }>;
type Event = ProviderEvent;

/** Where a `result`'s and the stream's usage become events (`map.ts` calls it). */
export interface UsageSource {
  /** The events of a `result`: its `usage` (when it adds anything), then its per-model split. */
  usage(result: ResultMsg): Event[];
  /** A live `usage` for a top-level stream event, or null. */
  stream(event: StreamPayload): UsageEvent | null;
}

/** Tokens and money (micros), as the CLI totals them. */
export interface UsageTotals {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
  readonly costMicros: number;
}

const KEYS = ["input", "output", "cacheRead", "cacheWrite", "costMicros"] as const;
export const ZERO_TOTALS: UsageTotals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, costMicros: 0 };

const add = (a: UsageTotals, b: UsageTotals): UsageTotals => ({ input: a.input + b.input, output: a.output + b.output, cacheRead: a.cacheRead + b.cacheRead, cacheWrite: a.cacheWrite + b.cacheWrite, costMicros: a.costMicros + b.costMicros });
/** `a − b` per field, never below 0. */
const over = (a: UsageTotals, b: UsageTotals): UsageTotals => ({
  input: Math.max(0, a.input - b.input),
  output: Math.max(0, a.output - b.output),
  cacheRead: Math.max(0, a.cacheRead - b.cacheRead),
  cacheWrite: Math.max(0, a.cacheWrite - b.cacheWrite),
  costMicros: Math.max(0, a.costMicros - b.costMicros),
});
const isZero = (t: UsageTotals): boolean => KEYS.every((k) => t[k] === 0);
const micros = (usd: number | undefined): number => (usd === undefined || !Number.isFinite(usd) || usd <= 0 ? 0 : Math.round(usd * 1_000_000));

/** A `result`'s cumulative totals. */
export function resultTotals(r: ResultMsg): UsageTotals {
  return { input: r.usage.input_tokens, output: r.usage.output_tokens, cacheRead: r.usage.cache_read_input_tokens ?? 0, cacheWrite: r.usage.cache_creation_input_tokens ?? 0, costMicros: micros(r.total_cost_usd) };
}

/**
 * What `--resume` does with the CLI's totals. `reset` (the default until Task 0 decides) charges
 * all a resumed instance reports; `restored` counts from the session's charged usage.
 * TODO(7D Task 0 Step 1): `--resume` restoring the cumulative totals (CLI reference: "totals
 * restored from earlier runs count"); modelled by fake-claude as `resumeTotals: restored`
 * (default) or `reset`. Until it is known the default fails closed: a restoring CLI's earlier
 * usage is charged again on every resume (over-charged, never under-charged).
 */
export type ResumeTotals = "restored" | "reset";

export interface ClaudeUsageOptions {
  /** The session's charged usage when this instance opened (`Session.usage`). */
  readonly baseline?: UsageTotals;
  /** The instance continues an earlier transcript (`--resume=`). */
  readonly resumed?: boolean;
  /** `subscription`: a plan sign-in whose money is not charged (spec §13.1). */
  readonly basis?: "reported" | "subscription";
  readonly resumeTotals?: ResumeTotals;
}

/** At most this many models in one split, each id at most 128 characters. */
const MAX_MODELS = 16;

/** One instance's usage memory: what it emitted, and the offset its CLI's totals count from. */
export class ClaudeUsage implements UsageSource {
  readonly #baseline: UsageTotals;
  readonly #resumed: boolean;
  readonly #basis: "reported" | "subscription";
  readonly #mode: ResumeTotals;
  /** What the CLI's totals held before this instance. */
  readonly #offset: UsageTotals;
  /** What this instance emitted. */
  #emitted: UsageTotals = ZERO_TOTALS;
  /** The open message's usage as the stream reported it so far (a top-level message). */
  #message: UsageTotals = ZERO_TOTALS;
  /** The last `modelUsage` seen on this instance (the per-model split's baseline), or null. */
  #models: Map<string, UsageTotals> | null = null;

  constructor(o: ClaudeUsageOptions = {}) {
    this.#baseline = o.baseline ?? ZERO_TOTALS;
    this.#resumed = o.resumed === true;
    this.#basis = o.basis ?? "reported";
    this.#mode = o.resumeTotals ?? "reset";
    // A fresh transcript (`--session-id`) counts from zero; so does a resumed one whose totals
    // are not known to be restored (fail closed: Review Focus 4).
    const restored = this.#resumed && this.#mode === "restored" && !isZero(this.#baseline);
    this.#offset = restored ? this.#baseline : ZERO_TOTALS;
    // A restored instance's first split is unknown (its per-model totals hold earlier runs).
    if (!restored) this.#models = new Map();
  }

  /** The basis this instance's events carry. */
  get basis(): "reported" | "subscription" {
    return this.#basis;
  }

  #event(d: UsageTotals): UsageEvent {
    return { type: "usage", input: d.input, output: d.output, cacheRead: d.cacheRead, cacheWrite: d.cacheWrite, reasoning: 0, costMicros: d.costMicros, costBasis: this.#basis };
  }

  // TODO(7D Task 0 Step 1): the `stream_event` usage the CLI forwards with
  // `--include-partial-messages`; modelled by fake-claude (`liveUsage`) on the Messages API
  // stream: `message_start.message.usage` with the input side and `output_tokens` ≤ 1, then
  // `message_delta.usage.output_tokens` as the message's output so far. Any field a later event
  // reports is taken as that message's total so far, so a cumulative `message_delta` that also
  // repeats the input side charges it once.
  stream(e: StreamPayload): UsageEvent | null {
    let seen: Partial<Record<"input_tokens" | "output_tokens" | "cache_read_input_tokens" | "cache_creation_input_tokens", number | null>> | undefined;
    if (e.type === "message_start") {
      this.#message = ZERO_TOTALS;
      seen = e.message.usage;
    } else if (e.type === "message_delta") seen = e.usage;
    if (seen === undefined) return null;
    const now: UsageTotals = {
      input: Math.max(this.#message.input, seen.input_tokens ?? 0),
      output: Math.max(this.#message.output, seen.output_tokens ?? 0),
      cacheRead: Math.max(this.#message.cacheRead, seen.cache_read_input_tokens ?? 0),
      cacheWrite: Math.max(this.#message.cacheWrite, seen.cache_creation_input_tokens ?? 0),
      costMicros: 0,
    };
    const d = over(now, this.#message);
    this.#message = now;
    if (isZero(d)) return null;
    this.#emitted = add(this.#emitted, d);
    return this.#event(d);
  }

  // TODO(7D Task 0 Step 1): `result.usage`/`total_cost_usd` cumulative per process (docs), and
  // whether `--resume` restores them (`ResumeTotals`); modelled by fake-claude as cumulative,
  // with `resumeTotals: restored` (default) or `reset`.
  usage(r: ResultMsg): Event[] {
    const now = resultTotals(r);
    const d = over(now, add(this.#offset, this.#emitted));
    const out: Event[] = [];
    if (!isZero(d)) {
      this.#emitted = add(this.#emitted, d);
      out.push(this.#event(d));
    }
    const split = this.#split(r, d);
    if (split !== null) out.push({ type: "notice", name: "modelUsage", details: split });
    return out;
  }

  /**
   * The per-model split of this `result`: `model_usage_since_last_result` when the CLI gives it,
   * else `modelUsage` against the last one this instance saw. Unknown (null) for the first
   * `result` of an instance that restored earlier totals with several models in them.
   * TODO(7D Task 0 Step 1): the `modelUsage` shape (per model, cumulative like `usage`);
   * modelled by fake-claude so.
   */
  #split(r: ResultMsg, total: UsageTotals): Record<string, UsageTotals> | null {
    const byModel = (m: NonNullable<ResultMsg["modelUsage"]>): Map<string, UsageTotals> =>
      new Map(Object.entries(m).map(([id, u]) => [id, { input: u.inputTokens ?? 0, output: u.outputTokens ?? 0, cacheRead: u.cacheReadInputTokens ?? 0, cacheWrite: u.cacheCreationInputTokens ?? 0, costMicros: micros(u.costUSD) }]));
    const now = r.modelUsage === undefined ? null : byModel(r.modelUsage);
    const last = this.#models;
    if (now !== null) this.#models = now;
    let split: [string, UsageTotals][];
    if (r.model_usage_since_last_result !== undefined) split = [...byModel(r.model_usage_since_last_result)];
    else if (now === null) return null;
    else if (last !== null) split = [...now].map(([id, t]) => [id, over(t, last.get(id) ?? ZERO_TOTALS)]);
    else if (now.size === 1) split = [[[...now.keys()][0]!, total]];
    else return null;
    const kept = split.filter(([, t]) => !isZero(t)).slice(0, MAX_MODELS);
    return kept.length === 0 ? null : Object.fromEntries(kept.map(([id, t]) => [id.slice(0, 128), t]));
  }
}

/** The session's charged usage as a baseline (`Session.usage`). */
export function baselineOf(u: { input: number; output: number; cacheRead: number; cacheWrite: number; moneyMicros: number } | undefined): UsageTotals {
  return u === undefined ? ZERO_TOTALS : { input: u.input, output: u.output, cacheRead: u.cacheRead, cacheWrite: u.cacheWrite, costMicros: u.moneyMicros };
}

/** The per-turn projection `gate.beforeCall` reserves (Decision 15): the estimate, at most what is left. */
export const TURN_ESTIMATE_TOKENS = 8_192;

export function turnProjection(remainingTokens: number | null, estimate = TURN_ESTIMATE_TOKENS): { input: number; output: number; cacheRead: number; cacheWrite: number; reasoning: number } {
  const output = remainingTokens === null ? estimate : Math.max(0, Math.min(estimate, Math.floor(remainingTokens)));
  return { input: 0, output, cacheRead: 0, cacheWrite: 0, reasoning: 0 };
}

type LedgerKind = "tokens" | "money";
/** What is left of each budget in `ledger` (limit − used − reserved), for `ClaudeDeps.remaining`. */
export function remainingIn(ledger: () => { read(sessionId: string, kind: LedgerKind): { limit: number | null; used: number; reserved: number } }) {
  return (sessionId: string): { tokens: number | null; moneyMicros: number | null } => {
    const l = ledger();
    const left = (k: LedgerKind) => {
      const r = l.read(sessionId, k);
      return r.limit === null ? null : Math.max(0, r.limit - r.used - r.reserved);
    };
    return { tokens: left("tokens"), moneyMicros: left("money") };
  };
}
