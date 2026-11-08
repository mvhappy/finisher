// The built-in agent's loop (sub-project 7 §4.4; plan 7A Decisions 6, 7, 9, 10, 13): an adapter
// like any other. Each turn: the prompt (after the run's facts) → a streamed model call → its
// tool calls (reads in parallel, writes one at a time, in order) → their results appended →
// the next call, until the model ends its turn, a per-run cap, the context stop, a budget stop
// or an interrupt.
//
// - **Tools run in the runtime.** The loop announces a call with `toolStarted` and waits for the
//   runtime's `respond(providerId, toolResult)`, then reports `toolCompleted`. The runtime runs
//   a call as soon as it is announced, so a write is announced only once everything before it
//   has its result: that is how writes stay serial.
// - **Budgets:** `spec.gate.beforeCall` before every model attempt; a refusal ends the run.
//   Every attempt is charged: one `usage` event each (a failed attempt's from its cost), or
//   `gate.release()` for one that reported none.
// - **Capture:** one `capture {request, response}` per model call: the exact body bytes as
//   sent (a string; `bodyHash` is its BLAKE3) and the response chunks with the call's metadata.
// - **Caps (Decision 10):** 60 model calls and 200 tool calls per run; each tool result shown to
//   the model is at most 16 KiB (head 12 KiB + tail 4 KiB + a note); the run stops with
//   `MODEL_CONTEXT_EXCEEDED` when a call's prompt would pass 85 % of the model's window.
import { FinError, type Node, type ProviderEvent } from "@finisher/contracts";
import { type Cause, Effect, Queue, type Scope, Stream } from "effect";
import { finError } from "../../errors.ts";
import { type BoundEvent, type Capabilities, NO_CAPABILITIES, type ProjectedUsage, type ProviderAdapter, type ProviderSession, type RuntimeAnswer, type SessionSpec } from "../adapter.ts";
import type { Catalogue } from "../catalogue/catalogue.ts";
import { type AttemptCost, type KeySource, type ModelClient, type ModelResult, noKey } from "./compat.ts";
import type { FinIndex } from "./fin-tool.ts";
import { historyFromNodes, renderResult } from "./loop-history.ts";
import type { ModelMessage, ModelRequest, Preset, ToolCall } from "./request.ts";
import { FIN_TOOL, isWrite, TOOL_DEFS, TOOL_NAMES } from "./tools.ts";

export interface LoopLimits {
  readonly modelCalls: number;
  readonly toolCalls: number;
  /** Bytes of one tool result shown to the model, and its head and tail when cut. */
  readonly resultBytes: number;
  readonly headBytes: number;
  readonly tailBytes: number;
  /** The share of the window a call's prompt may reach. */
  readonly contextRatio: number;
  /** `max_tokens` of each call (capped by the model's `maxOutput`). */
  readonly maxTokens: number;
}

export const LOOP_LIMITS: LoopLimits = {
  modelCalls: 60,
  toolCalls: 200,
  resultBytes: 16 * 1024,
  headBytes: 12 * 1024,
  tailBytes: 4 * 1024,
  contextRatio: 0.85,
  maxTokens: 8192,
};

export interface BuiltinDeps {
  /** The model client of a preset (`openrouter`; `local` from the owner's base URL). */
  readonly client: (preset: Preset) => ModelClient;
  /** The OpenRouter key for one request (`ModelGateway.withKey`); the local preset uses none. */
  readonly gateway?: { withKey<A>(projectId: string, sessionId: string, f: (key: string) => Promise<A>): Promise<A> };
  readonly catalogue: Catalogue;
  /** The `fin` binary the tools run (`detect` reports whether it is there). */
  readonly finPath: string;
  /** What the tools' write/read split reads (`fin schema commands`); unknown `fin` commands are writes. */
  readonly index?: () => Promise<FinIndex | undefined>;
  /** A session's stored nodes, to rebuild the conversation when it is opened again (resume). */
  readonly nodes?: (sessionId: string) => ReadonlyArray<Node>;
  readonly limits?: Partial<LoopLimits>;
}

export const BUILTIN_CAPABILITIES: Capabilities = {
  ...NO_CAPABILITIES,
  steer: true,
  resume: true,
  reasoning: true,
  parallelTools: true,
  costReported: true,
  exactCapture: true,
};

type EventQueue = Queue.Queue<BoundEvent, Cause.Done>;
type ToolAnswer = { readonly ok: boolean; readonly output: unknown };

/** Interrupted: the turn ends with `turnCompleted {stopReason: "interrupted"}`. */
class Stopped extends Error {}

const estimate = (chars: number) => Math.ceil(chars / 4);

export function BuiltinAdapter(deps: BuiltinDeps): ProviderAdapter {
  const limits: LoopLimits = { ...LOOP_LIMITS, ...deps.limits };
  let opened = 0;

  const open = (spec: SessionSpec) =>
    Effect.gen(function* () {
      const preset: Preset = spec.provider === "local" ? "local" : "openrouter";
      const info = deps.catalogue.describe(preset, spec.model);
      if (!info.capabilities.tools) {
        return yield* Effect.fail(finError("MODEL_UNKNOWN", `${spec.model} does not take tools, which the built-in agent needs`, { model: spec.model, reason: "noTools" }, "choose a model with tool support"));
      }
      const client = yield* Effect.try({ try: () => deps.client(preset), catch: (e) => (e instanceof FinError ? e : finError("PROVIDER_UNAVAILABLE", String(e), { provider: preset })) });
      const instance = `builtin-${++opened}`;
      const queue: EventQueue = yield* Queue.unbounded<BoundEvent, Cause.Done>();
      const emit = (event: ProviderEvent) => void Effect.runSync(Queue.offer(queue, { instance, event }));
      const key: KeySource =
        preset === "local" || deps.gateway === undefined ? noKey : <A>(f: (k: string) => Promise<A>) => deps.gateway!.withKey(spec.projectId, spec.sessionId, f);
      const maxTokens = Math.max(1, Math.min(limits.maxTokens, info.maxOutput || limits.maxTokens));
      const viewBudget = Math.max(200, Math.min(4000, Math.floor(info.window * 0.05)));

      const history: ModelMessage[] = deps.nodes ? historyFromNodes(deps.nodes(spec.sessionId)) : [];
      const steers: string[] = [];
      const waits = new Map<string, { resolve: (a: ToolAnswer) => void; reject: (e: unknown) => void }>();
      const early = new Map<string, ToolAnswer>();
      let running: Promise<void> | null = null;
      let ctl: AbortController | null = null;
      /** Tokens of the last call's prompt and reply, and the history length they cover. */
      let known: { tokens: number; covers: number } | null = null;

      const waitFor = (id: string) =>
        new Promise<ToolAnswer>((resolve, reject) => {
          const ready = early.get(id);
          if (ready) {
            early.delete(id);
            return resolve(ready);
          }
          waits.set(id, { resolve, reject });
        }).then((a) => {
          emit({ type: "toolCompleted", providerId: id, ok: a.ok, output: a.output });
          return a;
        });

      /** The prompt tokens of `req`: the last call's count plus the new messages, else all of it. */
      const promptTokens = (req: ModelRequest) => {
        if (known !== null && known.covers <= req.messages.length) {
          return known.tokens + estimate(JSON.stringify(req.messages.slice(known.covers)).length);
        }
        return estimate(JSON.stringify({ s: req.system, t: req.tools, m: req.messages }).length);
      };

      /** Charges one attempt: its usage, else the gate's reservation is released. */
      const settle = (cost: AttemptCost) => {
        if (cost.usage === null) spec.gate?.release();
        else emit({ type: "usage", ...cost.usage, ...(cost.costMicros === undefined ? {} : { costMicros: cost.costMicros }) });
      };

      const admit = (projected: ProjectedUsage): FinError | null => {
        const g = spec.gate?.beforeCall(projected);
        return g === undefined || g.ok ? null : g.error;
      };

      /** One model call with its retries; a refusal by the gate is returned, not thrown. */
      async function modelCall(req: ModelRequest, signal: AbortSignal): Promise<{ result: ModelResult } | { refused: FinError }> {
        const projected: ProjectedUsage = { input: promptTokens(req), output: req.maxTokens, cacheRead: 0, cacheWrite: 0, reasoning: 0 };
        const first = admit(projected);
        if (first) return { refused: first };
        const it = client.stream(req, key, signal)[Symbol.asyncIterator]();
        for (;;) {
          const step = await it.next();
          if (step.done) throw finError("PROVIDER_PROTOCOL", "the model stream ended without a result", {});
          const ev = step.value;
          switch (ev.type) {
            case "textDelta":
              emit({ type: "messageDelta", text: ev.text });
              break;
            case "reasoningDelta":
              emit({ type: "reasoningDelta", text: ev.text });
              break;
            case "retry": {
              settle(ev.cost);
              emit({ type: "notice", name: "retry", details: { attempt: ev.attempt, code: ev.code, delayMs: ev.delayMs } });
              const again = admit(projected);
              if (again) {
                await it.return?.();
                return { refused: again };
              }
              break;
            }
            case "failed":
              settle(ev.cost);
              break;
            case "done": {
              const r = ev.result;
              emit({ type: "usage", ...r.usage, ...(r.costMicros === undefined ? {} : { costMicros: r.costMicros }) });
              return { result: r };
            }
          }
        }
      }

      /** Runs the calls of one reply: reads together, each write alone, results in call order. */
      async function runTools(calls: readonly ToolCall[], used: { tools: number }, signal: AbortSignal): Promise<{ results: Map<string, string>; capped: boolean }> {
        const index = await deps.index?.().catch(() => undefined);
        const results = new Map<string, string>();
        let capped = false;
        type Ready = { id: string; tool: string; input: unknown; write: boolean };
        const ready: Ready[] = [];
        for (const c of calls) {
          if (!TOOL_NAMES.has(c.name)) {
            results.set(c.id, JSON.stringify({ ok: false, error: { code: "USAGE", message: `no tool named ${c.name}`, tools: [...TOOL_NAMES] } }));
            continue;
          }
          let input: unknown;
          try {
            input = c.arguments.trim() === "" ? {} : JSON.parse(c.arguments);
          } catch {
            results.set(c.id, JSON.stringify({ ok: false, error: { code: "USAGE", message: "the tool arguments are not JSON" } }));
            continue;
          }
          if (c.name === FIN_TOOL) input = withViewBudget(input, viewBudget);
          ready.push({ id: c.id, tool: c.name, input, write: isWrite(c.name, input, index) });
        }
        let i = 0;
        while (i < ready.length) {
          if (signal.aborted) throw new Stopped();
          // A run of reads, or one write.
          const batch = ready[i]!.write ? [ready[i]!] : takeWhile(ready, i, (r) => !r.write);
          i += batch.length;
          const pending: Promise<void>[] = [];
          for (const r of batch) {
            if (used.tools >= limits.toolCalls) {
              capped = true;
              results.set(r.id, JSON.stringify({ ok: false, error: { code: "BUDGET_EXCEEDED", message: `the run's ${limits.toolCalls} tool calls are used up`, kind: "toolCalls" } }));
              continue;
            }
            used.tools++;
            emit({ type: "toolStarted", providerId: r.id, tool: r.tool, input: r.input });
            pending.push(waitFor(r.id).then((a) => void results.set(r.id, renderResult(r.id, a, limits))));
          }
          await Promise.all(pending);
        }
        return { results, capped };
      }

      async function runTurn(): Promise<void> {
        const signal = ctl!.signal;
        const used = { tools: 0 };
        emit({ type: "turnStarted" });
        try {
          for (let calls = 0; ; calls++) {
            if (steers.length > 0) history.push({ role: "user", content: [{ type: "text", text: steers.splice(0).join("\n\n") }] });
            if (calls >= limits.modelCalls) return emit({ type: "turnCompleted", stopReason: "modelCallCap" });
            const req: ModelRequest = { system: spec.system, tools: TOOL_DEFS, messages: [...history], model: spec.model, maxTokens };
            const tokens = promptTokens(req);
            if (tokens > limits.contextRatio * info.window) {
              const e = finError("MODEL_CONTEXT_EXCEEDED", "the conversation fills 85 % of the model's context window", { tokens, window: info.window }, "start a new run or session");
              return emit({ type: "turnFailed", error: { code: e.code, message: e.message, details: e.details, fix: e.fix! } });
            }
            const answer = await modelCall(req, signal);
            if ("refused" in answer) {
              const e = answer.refused;
              const kind = (e.details as { kind?: unknown } | null)?.kind;
              if (kind === "modelCalls") return emit({ type: "turnCompleted", stopReason: "modelCallCap" });
              return emit({ type: "turnFailed", error: { code: e.code, message: e.message, details: e.details ?? {}, ...(e.fix === undefined ? {} : { fix: e.fix }) } });
            }
            const r = answer.result;
            emit({
              type: "capture",
              request: r.capture.request,
              response: {
                chunks: r.capture.response,
                bodyHash: r.bodyHash,
                usageEstimated: r.usageEstimated,
                latencyMs: r.latencyMs,
                ...(r.ttftMs === undefined ? {} : { ttftMs: r.ttftMs }),
                ...(r.upstream === undefined ? {} : { upstream: r.upstream }),
                ...(r.generationId === undefined ? {} : { generationId: r.generationId }),
              },
            });
            if (r.reasoningText !== "" || r.reasoning.length > 0) {
              emit({ type: "reasoning", ...(r.reasoningText === "" ? {} : { text: r.reasoningText }), ...(r.reasoning.length === 0 ? {} : { encrypted: r.reasoning }) });
            }
            if (r.text !== "") emit({ type: "message", role: "assistant", blocks: [{ type: "text", text: r.text }] });
            history.push({
              role: "assistant",
              ...(r.text === "" ? {} : { text: r.text }),
              ...(r.toolCalls.length === 0 ? {} : { toolCalls: r.toolCalls }),
              ...(r.reasoning.length === 0 ? {} : { reasoning: r.reasoning }),
            });
            known = { tokens: r.usage.input + r.usage.cacheRead + r.usage.cacheWrite + r.usage.output, covers: history.length };
            if (r.toolCalls.length === 0) return emit({ type: "turnCompleted", stopReason: r.stopReason });
            const { results, capped } = await runTools(r.toolCalls, used, signal);
            for (const c of r.toolCalls) history.push({ role: "tool", toolCallId: c.id, content: results.get(c.id) ?? "" });
            if (capped) return emit({ type: "turnCompleted", stopReason: "toolCallCap" });
          }
        } catch (e) {
          if (e instanceof Stopped || signal.aborted) return emit({ type: "turnCompleted", stopReason: "interrupted" });
          const err = e instanceof FinError ? e : finError("INTERNAL", e instanceof Error ? e.message : String(e), {});
          emit({ type: "turnFailed", error: { code: err.code, message: err.message, details: err.details ?? {}, ...(err.fix === undefined ? {} : { fix: err.fix }) } });
        }
      }

      const stop = async () => {
        const turn = running;
        if (turn === null) return;
        ctl?.abort(new Stopped());
        for (const w of waits.values()) w.reject(new Stopped());
        waits.clear();
        await turn;
      };

      yield* Effect.addFinalizer(() => Effect.promise(stop).pipe(Effect.andThen(Queue.end(queue)), Effect.asVoid));

      const session: ProviderSession = {
        instance,
        events: Stream.fromQueue(queue),
        startTurn: (i) =>
          Effect.suspend(() => {
            if (running !== null) return Effect.fail(finError("SESSION_BUSY", "a turn is running", { instance }));
            const blocks = [...(i.facts === undefined ? [] : [{ type: "text" as const, text: i.facts }]), { type: "text" as const, text: i.text }];
            history.push({ role: "user", content: blocks });
            ctl = new AbortController();
            early.clear();
            running = runTurn().finally(() => {
              running = null;
            });
            return Effect.void;
          }),
        steer: (i) =>
          Effect.sync(() => {
            steers.push(i.text);
          }),
        interrupt: () => Effect.promise(stop),
        respond: (id: string, a: RuntimeAnswer) =>
          Effect.sync(() => {
            if (a.kind !== "toolResult") return;
            const answer = { ok: a.ok, output: a.output };
            const w = waits.get(id);
            if (w) {
              waits.delete(id);
              w.resolve(answer);
            } else {
              early.set(id, answer);
            }
          }),
        rollback: () => Effect.fail(finError("PROVIDER_UNAVAILABLE", "the built-in agent rolls back in 7B", {})),
        ref: () => ({ kind: "builtin", id: spec.sessionId }),
      };
      return session;
    });

  return {
    harness: "builtin",
    detect: () =>
      Effect.tryPromise({
        try: async () => {
          const { stat } = await import("node:fs/promises");
          const installed = await stat(deps.finPath).then((s) => s.isFile(), () => false);
          return { installed, version: "builtin-1", supported: "builtin-1", signedIn: true, capabilities: BUILTIN_CAPABILITIES };
        },
        catch: (e) => finError("INTERNAL", String(e), {}),
      }),
    open: (spec) => open(spec) as Effect.Effect<ProviderSession, FinError, Scope.Scope>,
  };
}

function takeWhile<T>(xs: readonly T[], from: number, p: (x: T) => boolean): T[] {
  const out: T[] = [];
  for (let i = from; i < xs.length && p(xs[i]!); i++) out.push(xs[i]!);
  return out;
}

/** `fin view` gets a `--budget` from the window when it has none (spec §4.4). */
function withViewBudget(input: unknown, budget: number): unknown {
  const argv = (input as { argv?: unknown } | null)?.argv;
  if (!Array.isArray(argv) || argv[0] !== "view" || argv.some((a) => a === "--budget" || (typeof a === "string" && a.startsWith("--budget="))) || argv.includes("--help")) return input;
  return { ...(input as object), argv: [...argv, "--budget", String(budget)] };
}
