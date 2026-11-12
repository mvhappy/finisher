// What the runtime engine (plan 7A Task 6b) needs from the rest of finisherd beside Task 7's
// `AgentCore` (store, session credentials) and `openSession`, as interfaces: the start planner
// and the session environment (Task 12 and 8b fill them from the catalogue, the roles and the
// `fin` map), the journal (Task 10's `AgentJournal`, `journal.ts`), the tool executors (8b's built-in tools; tests' fakes) and
// pricing (the catalogue). The journal and pricing have defaults here.
import type { AgentProvider, AgentSessionStartPayload, Approval, BudgetSpec, CostBasis, FinError, Harness, Mode, Principal, Session, Usage, Weights } from "@finisher/contracts";
import { finError } from "../errors.ts";
import type { ProviderAdapter, ProviderRef } from "./adapter.ts";
import type { AgentCore } from "./core.ts";
import type { BranchRegistry } from "./credentials.ts";
import type { ProviderRequests } from "./provider-requests.ts";
import type { Role } from "./roles.ts";
import type { SessionSandboxes } from "./sandbox.ts";
import type { SessionLimits } from "./settings.ts";
import { type BudgetLimits, DEFAULT_BUDGET, type Reading } from "./budget.ts";
import type { Catalogue, Usage as TokenUsage } from "./catalogue/catalogue.ts";
import type { JournalPhase } from "./session.ts";

/** `agent.session.start`'s payload. */
export type StartRequest = typeof AgentSessionStartPayload.Type;

/** Decision 15.1 resolved: what the session will be. */
export interface SessionPlan {
  readonly harness: Harness;
  readonly provider: AgentProvider;
  readonly model: string;
  readonly weights: Weights;
  readonly role: string;
  readonly mode: Mode;
  /** The effective budget: the role's, lowered or set by the starter. */
  readonly budget: BudgetSpec;
  /** The role's tool allow-set (what a CLI harness leaves enabled, spec §4.5). */
  readonly tools?: readonly string[];
}

export interface SessionPlanner {
  /**
   * Decision 15.1: role (`editor`), mode (`draft`, at most the role's ceiling), harness
   * (`builtin`), preset (`openrouter`), model and weights; the OpenRouter key or the local
   * endpoint checked. Fails `ROLE_UNKNOWN`, `MODE_INVALID`, `MODEL_UNKNOWN`, `PROVIDER_AUTH`,
   * `PROVIDER_UNAVAILABLE` as a `FinError`. With `parent` (a child, 7B Decision 17): the role
   * must be one the parent's role may spawn (`ROLE_UNKNOWN`), the mode at most the parent's
   * (`MODE_INVALID`), the harness and preset the parent's unless given.
   */
  plan(req: StartRequest, by: Principal, parent?: Session): Promise<SessionPlan>;
}

/** What a provider instance is opened with (spec §4.1, §8.5). */
export interface OpenEnvironment {
  /** The role's prompt (plus the `fin` map for the built-in agent): never budget facts. */
  readonly system: string;
  /** `workspace/<sessionId>/` in the project folder. */
  readonly cwd: string;
  /** `childEnv` plus `FIN_SERVER` and `FIN_SESSION` (8b); the runtime sets `FIN_CREDENTIAL`. */
  readonly env: Readonly<Record<string, string>>;
}

export interface SessionEnvironment {
  prepare(s: Session): Promise<OpenEnvironment>;
}

/** One tool call the runtime runs for its adapter (the built-in loop's tools; the fake's). */
export interface ToolCall {
  readonly sessionId: string;
  readonly projectId: string;
  readonly runId: string;
  /** The Finisher call id, a ULID: `FIN_CALL` (spec §12.3). */
  readonly callId: string;
  readonly providerId: string;
  readonly tool: string;
  readonly input: unknown;
  readonly cwd: string;
  readonly env: Readonly<Record<string, string>>;
}

export interface ToolOutcome {
  readonly ok: boolean;
  readonly output: unknown;
}

export interface ToolExecutor {
  /** Runs one call exactly once. A rejection becomes `{ok: false, output: {code, message}}`. */
  run(call: ToolCall, signal: AbortSignal): Promise<ToolOutcome>;
}

/** The records Task 10's `AgentJournal` turns into journal entries. Each call is best effort. */
export interface RuntimeJournal {
  /** The session phases, from the outbox (at least once). `session` is undefined once deleted. */
  session(sessionId: string, session: Session | undefined, phase: JournalPhase, reason: string | null): void | Promise<void>;
  /** One model call's usage, priced (an `agent.turn`'s metadata). */
  turn(r: { session: Session; runId: string; usage: Usage }): void;
  /**
   * The exact model traffic of one call (built-in only): the `agent.turn` content. **A call's
   * `capture` event comes before its `usage` event** (the turn is recorded at `usage` and joins
   * the capture before it; a failed attempt's `usage` has none). `request` is the body as sent
   * (`ModelResult.capture.request`, JSON text); `response` is the chunks as received, or
   * `{chunks, stopReason?, upstream?, latencyMs?, ttftMs?}` with those `ModelResult` fields,
   * which the turn's metadata takes (`agent/journal-content.ts` `captureMeta`).
   */
  capture(r: { session: Session; runId: string; request: unknown; response: unknown }): void;
  /** A call announced; its turn is the run's last recorded one (its model call's `usage` came first). */
  toolCall(r: { session: Session; runId: string; callId: string; providerId: string; tool: string; input: unknown }): void;
  /** A result; `synthetic` for one recovery or an interrupt wrote. */
  toolResult(r: { sessionId: string; runId: string; callId: string; ok: boolean; output: unknown; synthetic: boolean }): void;
  /** An approval opened, answered or expired (`agent.approval` records the decisions). */
  approval(r: { session: Session; approval: Approval }): void;
}

export const NO_JOURNAL: RuntimeJournal = {
  session: () => {},
  turn: () => {},
  capture: () => {},
  toolCall: () => {},
  toolResult: () => {},
  approval: () => {},
};

export interface Pricing {
  /**
   * Whether the model has a price (a local model always does: zero). A session of a non-local
   * model without one is refused at start (`PRICING_UNKNOWN`): its money budget could not hold.
   */
  known(provider: AgentProvider, model: string): boolean;
  /** The money of one model call: what the provider reported, else the catalogue's price. */
  price(s: Session, u: TokenUsage, reportedMicros: number | undefined): { micros: number; basis: CostBasis };
  /** A projected call's cost for the reservation, or null when the price is unknown. */
  projected(s: Session, u: TokenUsage): number | null;
  /** A cache-read token's weight in token budgets. */
  cacheReadWeight(s: Session): number;
}

/**
 * Reported cost when there is one; local models cost nothing; anything else is unknown, so only
 * local sessions can start under it (the default until Task 12 passes `cataloguePricing`).
 */
export const REPORTED_PRICING: Pricing = {
  known: (provider) => provider === "local",
  price: (s, _u, reported) => {
    if (reported !== undefined) return { micros: reported, basis: "reported" };
    if (s.provider === "local") return { micros: 0, basis: "priced" };
    return { micros: 0, basis: "unknown" };
  },
  projected: () => null,
  cacheReadWeight: () => 1,
};

/** `Pricing` from the model catalogue (its price cache; spec §4.7). */
export function cataloguePricing(cat: Pick<Catalogue, "describe" | "price" | "projectedMicros" | "cacheReadWeight">): Pricing {
  return {
    known: (provider, model) => provider === "local" || cat.describe(provider, model).price !== undefined,
    price: (s, u, reported) => cat.price(s.provider, s.model, u, reported),
    projected: (s, u) => cat.projectedMicros(s.provider, s.model, u),
    cacheReadWeight: (s) => cat.cacheReadWeight(s.provider, s.model),
  };
}

export function pricingUnknown(provider: string, model: string): FinError {
  return finError(
    "PRICING_UNKNOWN",
    `no price is known for ${model}, so its money budget cannot be kept`,
    { provider, model },
    "refresh the model catalogue (`finisherd agent providers`), pick a listed model, or set its price in `agents.models`",
  );
}

/** The session's limits: its spec over the server's defaults. */
export function limitsOf(spec: BudgetSpec, defaults: BudgetLimits = DEFAULT_BUDGET): BudgetLimits {
  return {
    tokens: spec.tokens ?? defaults.tokens,
    moneyMicros: spec.moneyMicros ?? defaults.moneyMicros,
    wallMs: spec.wallMs ?? defaults.wallMs,
    modelCallsPerRun: spec.modelCallsPerRun ?? defaults.modelCallsPerRun,
    toolCallsPerRun: spec.toolCallsPerRun ?? defaults.toolCallsPerRun,
    dailyMoneyMicros: defaults.dailyMoneyMicros,
  };
}

const usd = (micros: number) => (micros / 1_000_000).toFixed(2);
const left = (r: Reading) => Math.max(0, r.limit - r.used);

/**
 * The run-head facts (Decision 13; spec §4.4): the project, branch and mode, and what the
 * budgets leave. A user-role message at the head of each run, never the system prompt, so
 * the system prompt stays identical across a role's sessions.
 */
export function runFacts(s: Session, readings: ReadonlyArray<Reading>): string {
  const r = new Map(readings.map((x) => [x.kind, x]));
  const branch = s.rootBranch ?? s.base.branch;
  const lines = [
    `Project ${s.projectId}, branch ${branch}${s.rootBranch === undefined ? " (read only)" : ""}, mode ${s.mode}.`,
  ];
  const parts: string[] = [];
  const tokens = r.get("tokens");
  if (tokens) parts.push(`${left(tokens)} of ${tokens.limit} tokens`);
  const money = r.get("money");
  if (money && s.provider !== "local") parts.push(`${usd(left(money))} of ${usd(money.limit)} USD`);
  const wall = r.get("wallMs");
  if (wall) parts.push(`${Math.floor(left(wall) / 60_000)} of ${Math.floor(wall.limit / 60_000)} minutes`);
  const calls = r.get("modelCalls");
  if (calls) parts.push(`${left(calls)} model calls`);
  const tools = r.get("toolCalls");
  if (tools) parts.push(`${left(tools)} tool calls this run`);
  if (parts.length > 0) lines.push(`Budget left: ${parts.join(", ")}.`);
  return lines.join("\n");
}

export const errorShape = (e: FinError) => ({
  code: e.code,
  message: e.message,
  details: e.details ?? {},
  ...(e.fix === undefined ? {} : { fix: e.fix }),
});

export function budgetError(r: Reading): FinError {
  return finError("BUDGET_EXCEEDED", `the session's ${r.kind} budget is used up`, { kind: r.kind, used: r.used, limit: r.limit }, "raise it with `finisherd agent approve <approval> allow --add <amount>`");
}

export function parseRef(ref: string | undefined): ProviderRef | null {
  if (ref === undefined) return null;
  try {
    const r = JSON.parse(ref) as Partial<ProviderRef> | null;
    return r !== null && typeof r.kind === "string" && typeof r.id === "string" ? { kind: r.kind, id: r.id } : null;
  } catch {
    return null;
  }
}

/** `SessionSpec.tools` from the stored allow-set (agent 0010): absent when none was recorded. */
export const toolsOf = (tools: readonly string[] | null): { readonly tools?: readonly string[] } => (tools === null ? {} : { tools });

export function harnessUnknown(harness: string): never {
  throw finError("HARNESS_UNKNOWN", `no ${harness} harness on this server`, { harness }, "list them with `finisherd agent providers`");
}

/** What the runtime engine (`runtime-engine.ts`) is built from. */
export interface RuntimeDeps {
  /** Task 7's per-server pieces (`AgentCoreTag`): the one store and the session credentials. */
  readonly core: AgentCore;
  /** The project registry: branch heads, and the root branch created as the starter (`openSession`). */
  readonly registry: BranchRegistry;
  readonly adapters: ReadonlyArray<ProviderAdapter>;
  readonly planner: SessionPlanner;
  readonly environment: SessionEnvironment;
  /** CLI harnesses' session sandboxes (plan 7C Task 11); none: CLI processes are not started. */
  readonly sandboxes?: SessionSandboxes;
  /** The harnesses whose tool calls the runtime runs (8b's built-in tools; a test's fake). */
  readonly tools?: Partial<Record<Harness, ToolExecutor>>;
  readonly journal?: RuntimeJournal;
  readonly pricing?: Pricing;
  /** `agents.defaultBudget` and `agents.dailyMoneyMicros` (Decision 20). */
  readonly defaults?: BudgetLimits;
  /** `agents.retentionDays` (Decision 4). */
  readonly retentionDays?: number;
  /** How long a `budget` approval stays open (default 7 days). */
  readonly approvalTtlMs?: number;
  /** How long a budget stop waits for the tool calls in flight before it aborts them (30 s). */
  readonly stopGraceMs?: number;
  /** How many items one `agent.watch` may hold unread before it ends `WATCH_LAGGING` (1000). */
  readonly watchBufferSize?: number;
  /** Start the hourly retention sweep and the budget check at `boot` (default true). */
  readonly timers?: boolean;
  /** The roles (a parent's `spawn.maxChildren`) and the session limits (7B Task 6, `settings.ts`). */
  readonly roles?: ReadonlyMap<string, Role>;
  readonly limits?: Partial<SessionLimits>;
  /** Provider permission requests (7B Task 5); none: every request is declined by policy. */
  readonly requests?: ProviderRequests;
  /** A test's pause after a child's open, before its spawn re-reads the parent (7B Task 6). */
  readonly afterChildOpened?: (childId: string) => Promise<void>;
  /** Where the engine reports what it could not do (an outbox effect, a late provider error). */
  readonly log?: (message: string, details: Record<string, unknown>) => void;
}
