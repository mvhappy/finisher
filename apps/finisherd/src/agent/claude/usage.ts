// Claude Code's usage and cost as the runtime's `usage` events (plan 7D Task 9, Decision 15;
// Review Focus 4: budgets hold against a process whose model calls Finisher cannot see).
//
// - **Live:** each model call's `stream_event` usage (`message_start`: the input side,
//   `message_delta`: the message's output so far) is a `usage` event at once, with no money
//   (`costMicros: 0`), so the runtime's token budget sees a turn's spend while it runs and stops
//   the session mid-turn (its budget stop: the interrupt, `BUDGET_EXCEEDED`, a `budget` card).
// - **The `result`** (verified 2.1.321, Task 0: s1a, s1b, s1d, s1e, s1f): `usage` is the last
//   turn's only (all zeros on `error_max_budget_usd`), `total_cost_usd` and `modelUsage` are
//   cumulative per process, `model_usage_since_last_result` is the turn's per model (tokens and
//   `costUSD`; its sum is the `total_cost_usd` jump), on every result (the interrupted
//   `aborted_tools` and the budget stop's too). A turn is charged from that key, never from
//   `usage`, and at least what the cumulative totals moved since the last `result` (or since the
//   offset, below), less what the stream already showed of it live.
// - **Across instances:** `--resume` restores the CLI's totals after a clean exit and restarts
//   them after a kill (s1d, s1e): the resumed process's first cumulative totals are the restored
//   part plus the turn. What it restores is **that process's** totals, not the session's
//   lifetime spend (Task 10 review), so the restored part is checked against the CLI's own
//   accounting baseline, kept in `provider.pid` at each end a server saw (`restoredOffset`:
//   never more than the totals that process reported, nor than its offset plus what the ledger
//   charged since it started, `Session.usage`, durable with the ledger): whatever it restores
//   beyond the offset is charged. An end nobody saw has no baseline: the restored part is charged
//   in full (an orphan may have finished a turn into it, s5C). In doubt, over, never under.
// - **The basis:** `subscription` for a plan sign-in (money shown, never charged; spec §13.1),
//   else `reported` (the CLI's own estimate). `modelUsage` gives a per-model split, recorded as a
//   `notice {modelUsage}` per `result`.
//
// The runtime charges a turn's tail (the `result` an interrupt, a budget stop or a replacement
// left) to the run its instance served (`runtime-usage.ts`), so every delta emitted here is
// charged exactly once.
import type { ProviderEvent } from "@finisher/contracts";
import type { CliBaseline } from "../provider-pid.ts";
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

type ByModel = NonNullable<ResultMsg["modelUsage"]>;
const modelTotals = (u: ByModel[string]): UsageTotals => ({ input: u.inputTokens ?? 0, output: u.outputTokens ?? 0, cacheRead: u.cacheReadInputTokens ?? 0, cacheWrite: u.cacheCreationInputTokens ?? 0, costMicros: micros(u.costUSD) });
const sumModels = (m: ByModel): UsageTotals => Object.values(m).reduce<UsageTotals>((t, u) => add(t, modelTotals(u)), ZERO_TOTALS);

/** The turn a `result` closes: `model_usage_since_last_result` (null when absent). */
export function turnTotals(r: ResultMsg): UsageTotals | null {
  return r.model_usage_since_last_result === undefined ? null : sumModels(r.model_usage_since_last_result);
}

/**
 * A `result`'s cumulative totals: tokens from `modelUsage` (absent: `prev` plus the turn, `turn`
 * else `usage`, the last turn's), money from `total_cost_usd`.
 */
export function cumulativeTotals(r: ResultMsg, prev: UsageTotals, turn: UsageTotals | null): UsageTotals {
  const last = turn ?? { input: r.usage.input_tokens, output: r.usage.output_tokens, cacheRead: r.usage.cache_read_input_tokens ?? 0, cacheWrite: r.usage.cache_creation_input_tokens ?? 0, costMicros: 0 };
  const tokens = r.modelUsage === undefined ? add(prev, last) : sumModels(r.modelUsage);
  return { ...tokens, costMicros: micros(r.total_cost_usd) };
}

/**
 * What `--resume` does with the CLI's totals. `reset` (this class's default) charges all a
 * resumed instance reports; `restored` counts from the session's charged usage. Verified 2.1.321,
 * Task 0 (s1d/s1e): restored after the last process's clean exit, restarted after a kill; the
 * adapter picks the mode from how the last process ended (`provider.pid`), and an end nobody saw
 * fails closed (`reset`: over-charged, never under-charged).
 */
export type ResumeTotals = "restored" | "reset";

export interface ClaudeUsageOptions {
  /** The CLI's restored totals this instance counts from under `restored` (`restoredOffset`). */
  readonly baseline?: UsageTotals;
  /** The instance continues an earlier transcript (`--resume=`). */
  readonly resumed?: boolean;
  /** `subscription`: a plan sign-in whose money is not charged (spec §13.1). */
  readonly basis?: "reported" | "subscription";
  readonly resumeTotals?: ResumeTotals;
}

/** At most this many models in one split, each id at most 128 characters. */
const MAX_MODELS = 16;

const max = (a: UsageTotals, b: UsageTotals): UsageTotals => ({ input: Math.max(a.input, b.input), output: Math.max(a.output, b.output), cacheRead: Math.max(a.cacheRead, b.cacheRead), cacheWrite: Math.max(a.cacheWrite, b.cacheWrite), costMicros: Math.max(a.costMicros, b.costMicros) });

/**
 * What a process resumed after `b`'s clean end counts from, per field: the CLI's totals at that
 * end, but never more than the offset it counted from plus what the ledger charged since it
 * started (`chargedNow − b.charged`), so a charge the ledger never made is charged again. None
 * recorded: zero (fail closed).
 */
export function restoredOffset(b: CliBaseline | undefined, chargedNow: UsageTotals): UsageTotals {
  if (b === undefined) return ZERO_TOTALS;
  const f = (k: (typeof KEYS)[number]) => Math.max(0, Math.min(b.cli[k], b.offset[k] + Math.max(0, chargedNow[k] - b.charged[k])));
  return { input: f("input"), output: f("output"), cacheRead: f("cacheRead"), cacheWrite: f("cacheWrite"), costMicros: f("costMicros") };
}

/** One instance's usage memory: what it emitted, and the offset its CLI's totals count from. */
export class ClaudeUsage implements UsageSource {
  readonly #baseline: UsageTotals;
  readonly #resumed: boolean;
  readonly #basis: "reported" | "subscription";
  readonly #mode: ResumeTotals;
  /** What the CLI's totals held before this instance. */
  readonly #offset: UsageTotals;
  /** What the stream showed live since the last `result` (the open turn's, charged already). */
  #live: UsageTotals = ZERO_TOTALS;
  /** The CLI's cumulative totals at its last `result` (the offset before any). */
  #cli: UsageTotals;
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
    this.#cli = this.#offset;
    // A restored instance's first split is unknown (its per-model totals hold earlier runs).
    if (!restored) this.#models = new Map();
  }

  /** What this instance counts the CLI's totals from. */
  get offset(): UsageTotals {
    return this.#offset;
  }

  /** The CLI's cumulative totals at its last `result` (the offset before any): `provider.pid`'s baseline at a clean end. */
  cliTotals(): UsageTotals {
    return this.#cli;
  }

  /** The basis this instance's events carry. */
  get basis(): "reported" | "subscription" {
    return this.#basis;
  }

  #event(d: UsageTotals): UsageEvent {
    return { type: "usage", input: d.input, output: d.output, cacheRead: d.cacheRead, cacheWrite: d.cacheWrite, reasoning: 0, costMicros: d.costMicros, costBasis: this.#basis };
  }

  // Verified 2.1.321, Task 0 (s1a): `message_start.message.usage` holds the input side,
  // `message_delta.usage` repeats the input and cache fields and gives `output_tokens` as the
  // message's running total. Any field a later event reports is taken as that message's total
  // so far, so the repeated input side is charged once.
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
    this.#live = add(this.#live, d);
    return this.#event(d);
  }

  /**
   * The turn this `result` closes (module docs): per field the larger of its
   * `model_usage_since_last_result` and what the cumulative totals moved since the last `result`
   * (on a resumed instance's first, since the offset: the restored part beyond it), less what the
   * stream already showed live.
   */
  usage(r: ResultMsg): Event[] {
    const turn = turnTotals(r);
    const now = cumulativeTotals(r, this.#cli, turn);
    const step = max(turn ?? ZERO_TOTALS, over(now, this.#cli));
    this.#cli = now;
    const d = over(step, this.#live);
    this.#live = ZERO_TOTALS;
    const out: Event[] = [];
    if (!isZero(d)) out.push(this.#event(d));
    const split = this.#split(r, step);
    if (split !== null) out.push({ type: "notice", name: "modelUsage", details: split });
    return out;
  }

  /**
   * The per-model split of this `result`: `model_usage_since_last_result` when the CLI gives it,
   * else `modelUsage` against the last one this instance saw. Unknown (null) for the first
   * `result` of an instance that restored earlier totals with several models in them.
   */
  #split(r: ResultMsg, total: UsageTotals): Record<string, UsageTotals> | null {
    const byModel = (m: ByModel): Map<string, UsageTotals> => new Map(Object.entries(m).map(([id, u]) => [id, modelTotals(u)]));
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
