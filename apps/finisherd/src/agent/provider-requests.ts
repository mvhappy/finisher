// A provider's permission requests in the runtime (sub-project 7B Task 5, Decision 13; spec
// §7.4): the engine hands each `request` event here (`RuntimeEngine.providerRequest`).
//
// - **Policy first.** `permissions.ts` decides: `allow` and `deny` are answered at once through
//   `ProviderSession.respond` and recorded as an `approval` node with `by: policy` (the rule,
//   layer or mode that decided).
// - **Ask.** A request a rule marks `ask` opens a `provider` approval (the card's subject is
//   the tool's name and input, written by the server; the same quotas and one-card-per-identical
//   request as a rule's, `approvals.ts`), its node, and the session waits (`waitingApproval`);
//   the provider's request stays unanswered meanwhile. A person's answer (`Runtime.approve`)
//   settles it: the decision goes back through `respond` and is an `approval` node with `by:
//   user`; an allowed card is consumed (it grants this request only). An expired card is a
//   `deny` by policy, settled at its `expiresAt`: a timer per open card (cleared on its answer;
//   one that fires early by the injected clock waits on) runs the runtime's expiry
//   (`Runtime.expireApprovals`), so a request waits at most the card's 30 minutes; the hourly
//   sweep stays as a backstop. A request that cannot get a card (the
//   run ended, `APPROVAL_LIMIT`) is denied by policy.
// - **Withdrawn.** A provider may take back a request it still waits on (Claude Code's
//   `control_cancel_request`, the `requestCancelled` event): `withdraw` drops it with an
//   `approval` node `{status: expired, reason: withdrawn}` and sends no answer; the card, once no
//   identical request waits on it, is expired (`Expiry.withdraw`). A request already answered is
//   not withdrawn (nothing waits).
// - **Deny messages.** Every deny carries the server's reason (`denyMessage`), which a provider
//   shows its model (Claude Code's `deny.message`).
// - **Unconfined.** `RequestContext.unconfined` (the session sandbox's `agent` profile is not
//   enforced) is handed to `permissions.ts`, whose managed rule then applies (owner Q3a).
// - **Gone.** Requests waiting here are the live instance's: a restart loses them with the
//   instance (the provider is gone too); an answer then reaches no one and is still recorded.
// - **Allowed shell calls** (7D Task 12): every allow, by policy or by a person, first opens the
//   call's attribution window (`attribution.allowed`), then the answer goes out.
// - `userInput` requests have no one to answer them yet: the provider is told to go on without.
import type { Approval, FinError, ProviderEvent, Session } from "@finisher/contracts";
import { Effect, Exit } from "effect";
import type { RuntimeAnswer } from "./adapter.ts";
import type { Clock } from "./clock.ts";
import { argsHash, type AgentApprovals, requestArgs, subjectOf } from "./approvals.ts";
import type { ProviderPermissions } from "./permissions.ts";

export type RequestEvent = Extract<ProviderEvent, { type: "request" }>;

/** What the engine gives one request: its session and run, its node writer and its answer. */
export interface RequestContext {
  readonly session: Session;
  readonly runId: string;
  /** Appends an `approval` node to the run. */
  readonly node: (payload: Record<string, unknown>) => Promise<void>;
  /** Answers the provider's request (the error it refused the answer with, if any). */
  readonly respond: (a: RuntimeAnswer) => Promise<FinError | null | undefined>;
  readonly log: (message: string, details: Record<string, unknown>) => void;
  /** The session's sandbox does not enforce the `agent` profile (`permissions.ts`, owner Q3a). */
  readonly unconfined?: boolean;
  /** The Finisher call the request asks about (`request.providerId`'s), when announced. */
  readonly callId?: string | undefined;
}

export interface ProviderRequestsDeps {
  readonly permissions: ProviderPermissions;
  /** The approvals (made after the runtime: looked up when a request asks). */
  readonly approvals: () => AgentApprovals | undefined;
  readonly key: Buffer;
  /** Marks an allowed provider card used (`AgentStore.consumeApproval`). */
  readonly consume: (approvalId: string) => void;
  /** Runs `f` in `ms` of real time; answers the cancel (default `setTimeout`, unref'd). */
  readonly schedule?: Schedule;
  /** Opens an allowed shell call's window before its answer (`claude/attribution.ts`). */
  readonly attribution?: { allowed(sessionId: string, callId: string | undefined, tool: string, input: unknown): Promise<void> };
}

export type Schedule = (ms: number, f: () => void) => () => void;

/** The runtime's side of a card's expiry (`Runtime` binds it). */
export interface Expiry {
  readonly clock: Clock;
  /** Expires every overdue approval and settles the provider requests waiting on them. */
  readonly expire: () => void;
  /** Expires card `approvalId` now: the requests that waited on it were withdrawn. */
  readonly withdraw?: (approvalId: string) => void;
}

/** Why the runtime denied a request, in words for the provider's model. */
export function denyMessage(why: { readonly reason: string; readonly rule?: string; readonly layer?: string }): string {
  switch (why.reason) {
    case "rule":
      return `Finisher's rule ${why.rule ?? "?"} (${why.layer ?? "?"} layer) denies this call. Do not retry it.`;
    case "mode":
      return "This session is in plan mode: Finisher refuses a write outside the session's workspace.";
    case "unconfined":
      return "The agent sandbox is not enforced on this host: Finisher refuses a write outside the session's workspace.";
    case "expired":
      return "No one answered this request in time: it is denied.";
    case "user":
      return "A person denied this call.";
    default:
      return `Finisher denied this call (${why.reason}).`;
  }
}

/** The longest one timer waits (`setTimeout` holds at most 2^31 - 1 ms). */
const TIMER_MAX_MS = 2 ** 31 - 1;

const realTimer: Schedule = (ms, f) => {
  const t = setTimeout(f, ms);
  t.unref?.();
  return () => clearTimeout(t);
};

interface Pending {
  readonly ctx: RequestContext;
  readonly requestId: string;
  readonly tool: string;
  readonly input: unknown;
}

const run = <A, E>(e: Effect.Effect<A, E>) => Effect.runPromiseExit(e);

/** An allow names the call it decided (an adapter that runs it sets `FIN_CALL`: 7D Task 13). */
const callOf = (ctx: RequestContext) => (ctx.callId === undefined ? {} : { callId: ctx.callId });

/** The 7A answer, kept for a runtime without provider permissions (and for `userInput`). */
async function declined(ctx: RequestContext, e: RequestEvent, reason: string): Promise<void> {
  await ctx.node({ requestId: e.id, kind: e.kind, ...(e.tool === undefined ? {} : { tool: e.tool }), input: e.input ?? null, decision: "deny", by: "policy", reason });
  const answer: RuntimeAnswer = e.kind === "approval" ? { kind: "approval", allow: false, message: denyMessage({ reason }) } : { kind: "userInput", text: "No one can answer now: continue without it." };
  const err = await ctx.respond(answer);
  if (err) ctx.log("agent runtime: the provider refused an answer", { code: err.code });
}

export class ProviderRequests {
  readonly #d: ProviderRequestsDeps;
  /** Requests waiting on a card, by approval id (identical requests share one card). */
  readonly #pending = new Map<string, Pending[]>();
  /** The expiry timer of each card with requests waiting (its cancel). */
  readonly #timers = new Map<string, () => void>();
  #expiry: Expiry | undefined;

  constructor(d: ProviderRequestsDeps) {
    this.#d = d;
  }

  /** The runtime's expiry, for the cards' timers (`Runtime`'s constructor). */
  bind(expiry: Expiry): void {
    this.#expiry = expiry;
  }

  /** Every timer stops (shutdown); the requests are the instances', which close too. */
  close(): void {
    for (const cancel of this.#timers.values()) cancel();
    this.#timers.clear();
  }

  /** A timer for card `id` at `expiresAt` (by the bound clock). */
  #arm(id: string, expiresAt: number): void {
    const clock = this.#expiry?.clock;
    if (clock === undefined || !this.#pending.has(id)) return;
    const ms = Math.min(TIMER_MAX_MS, Math.max(0, expiresAt - clock.now()));
    this.#timers.set(id, (this.#d.schedule ?? realTimer)(ms, () => {
      this.#timers.delete(id);
      if (!this.#pending.has(id)) return;
      // Early by the injected clock (or a long wait cut to TIMER_MAX_MS): wait on.
      if (clock.now() < expiresAt) return this.#arm(id, expiresAt);
      this.#expiry?.expire();
    }));
  }

  /** How many requests wait on cards (tests). */
  get waiting(): number {
    return [...this.#pending.values()].reduce((n, l) => n + l.length, 0);
  }

  async handle(ctx: RequestContext, e: RequestEvent): Promise<void> {
    if (e.kind !== "approval") return declined(ctx, e, "userInputUnavailable");
    const tool = e.tool ?? "unknown";
    const input = e.input ?? null;
    const base = { requestId: e.id, kind: "provider", tool, input };
    const answer = async (allow: boolean, node: { readonly reason: string; readonly rule?: string; readonly layer?: string }) => {
      await ctx.node({ ...base, decision: allow ? "allow" : "deny", by: "policy", ...node });
      if (allow) await this.#opened(ctx, tool, input);
      const err = await ctx.respond(allow ? { kind: "approval", allow, ...callOf(ctx) } : { kind: "approval", allow, message: denyMessage(node) });
      if (err) ctx.log("agent runtime: the provider refused an answer", { code: err.code });
    };
    const decided = await run(this.#d.permissions.explain(ctx.session, { tool, input, ...(ctx.unconfined === true ? { unconfined: true } : {}) }));
    if (Exit.isFailure(decided)) return answer(false, { reason: "rulesUnreadable" });
    const x = decided.value;
    const why = { reason: x.reason, ...(x.rule === undefined ? {} : { rule: x.rule }), ...(x.layer === undefined ? {} : { layer: x.layer }) };
    if (x.decision !== "ask") return answer(x.decision === "allow", why);
    const approvals = this.#d.approvals();
    if (approvals === undefined || x.rule === undefined || x.layer === undefined) return answer(false, { reason: "approvalsUnavailable" });
    const subject = subjectOf({ command: x.command, tag: "provider.request", payload: input, rule: x.rule, layer: x.layer });
    const hash = argsHash(this.#d.key, { command: x.command, rpc: "provider.request", args: requestArgs("provider.request", input), branches: [] });
    const asked = await run(approvals.ask(ctx.session, "provider", subject, hash));
    if (Exit.isFailure(asked)) {
      const code = Exit.findErrorOption(asked);
      return answer(false, { reason: code._tag === "Some" ? (code.value as FinError).code : "approvalFailed" });
    }
    if (asked.value === undefined) return answer(false, { reason: "noRun" });
    const list = this.#pending.get(asked.value.id) ?? [];
    list.push({ ctx, requestId: e.id, tool, input });
    this.#pending.set(asked.value.id, list);
    if (!this.#timers.has(asked.value.id)) this.#arm(asked.value.id, asked.value.approval.expiresAt);
  }

  /** An allowed call's attribution window, opened before the answer goes out (never fails the answer). */
  async #opened(ctx: RequestContext, tool: string, input: unknown): Promise<void> {
    try {
      await this.#d.attribution?.allowed(ctx.session.id, ctx.callId, tool, input);
    } catch (e) {
      ctx.log("agent runtime: a shell call's attribution window did not open", { error: String(e) });
    }
  }

  /** A card was answered or expired: its requests get the decision (`by: user`, or a policy `deny` on expiry). */
  async settle(a: Approval): Promise<void> {
    if (a.status === "open") return;
    const list = this.#pending.get(a.id);
    if (list === undefined) return;
    this.#pending.delete(a.id);
    this.#timers.get(a.id)?.();
    this.#timers.delete(a.id);
    const allow = a.status === "answered" && a.decision === "allow";
    if (allow) this.#d.consume(a.id);
    for (const p of list) {
      const node = a.status === "answered" ? { decision: a.decision, by: "user", answeredBy: a.by } : { decision: "deny", by: "policy", reason: "expired" };
      await p.ctx.node({ approvalId: a.id, requestId: p.requestId, kind: "provider", tool: p.tool, ...node });
      if (allow) await this.#opened(p.ctx, p.tool, p.input);
      const err = await p.ctx.respond(allow ? { kind: "approval", allow, ...callOf(p.ctx) } : { kind: "approval", allow, message: denyMessage({ reason: a.status === "answered" ? "user" : "expired" }) });
      if (err) p.ctx.log("agent runtime: the provider refused an answer", { code: err.code });
    }
  }

  /**
   * The provider withdrew request `requestId` of `sessionId` (module docs): it leaves its card
   * unanswered, with a node; the card is expired once no request waits on it.
   */
  async withdraw(sessionId: string, requestId: string): Promise<void> {
    for (const [id, list] of this.#pending) {
      const i = list.findIndex((p) => p.ctx.session.id === sessionId && p.requestId === requestId);
      if (i < 0) continue;
      const [p] = list.splice(i, 1);
      if (list.length === 0) {
        this.#pending.delete(id);
        this.#timers.get(id)?.();
        this.#timers.delete(id);
        this.#expiry?.withdraw?.(id);
      }
      await p!.ctx.node({ approvalId: id, requestId, kind: "provider", tool: p!.tool, status: "expired", reason: "withdrawn" });
      return;
    }
  }

  /** The cards of `sessionId` that are no longer open (the expiry expired them). */
  async settleSession(sessionId: string, approval: (id: string) => Approval | undefined): Promise<void> {
    for (const [id, list] of [...this.#pending]) {
      if (list[0]?.ctx.session.id !== sessionId) continue;
      const a = approval(id);
      if (a !== undefined) await this.settle(a);
    }
  }
}

/** The engine's entry: through `requests` when the runtime has them, else 7A's decline. */
export function handleRequest(requests: ProviderRequests | undefined, ctx: RequestContext, e: RequestEvent): Promise<void> {
  return requests === undefined ? declined(ctx, e, "approvalsUnavailable") : requests.handle(ctx, e);
}
