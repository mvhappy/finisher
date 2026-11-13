// The provider adapter contract (sub-project 7 §4.1; plan 7A Decision 5). Adapters emit only
// `ProviderEvent`s, each bound to the instance (process or HTTP stream) that emitted it; the
// runtime alone writes nodes, budgets and journal entries, and drops events from an instance
// it replaced. No session fork (cut in revision 2).
import { type AgentProvider, type FinError, type Harness, type Mode, ProviderEvent } from "@finisher/contracts";
import { type Effect, Schema, type Scope, type Stream } from "effect";
import { finError } from "../errors.ts";
import type { SessionSandbox } from "./sandbox.ts";

export type { Harness, Mode, ProviderEvent };

export interface Capabilities {
  readonly steer: boolean;
  readonly rollback: boolean;
  readonly resume: boolean;
  readonly reasoning: boolean;
  readonly images: boolean;
  readonly parallelTools: boolean;
  readonly costReported: boolean;
  readonly exactCapture: boolean;
  readonly nativeSubagents: boolean;
}

export interface Detection {
  readonly installed: boolean;
  readonly version: string | null;
  /** The versions this Finisher supports, as text (`>= 2.1.300`). */
  readonly supported: string | null;
  /** Signed in (CLIs, as the CLI reports it) or a key set (the built-in agent). */
  readonly signedIn: boolean;
  readonly capabilities: Capabilities;
}

/** What resume needs: a CLI's session or thread id, or the built-in loop's marker. */
export interface ProviderRef {
  readonly kind: string;
  readonly id: string;
}

export interface SessionSpec {
  readonly sessionId: string;
  readonly projectId: string;
  /** The session's provider (the built-in agent's preset: `openrouter` or `local`). */
  readonly provider?: AgentProvider;
  readonly model: string;
  readonly role: string;
  readonly mode: Mode;
  /** The role's prompt (plus the `fin` map for the built-in agent): never budget facts. */
  readonly system: string;
  /** `workspace/<sessionId>/` in the project folder. */
  readonly cwd: string;
  /** The session environment (§8.5) for processes the adapter starts. */
  readonly env: Readonly<Record<string, string>>;
  readonly providerRef: ProviderRef | null;
  /**
   * The role's tool allow-set the session started with (`sessions.tools`, plan 7D Task 4): a
   * CLI harness enables exactly these of its tools, on its first instance and on every resume.
   */
  readonly tools?: readonly string[];
  /** The runtime's budget check before each model call, for an adapter whose loop runs here (7A-2). */
  readonly gate?: CallGate;
  /**
   * A CLI harness's session sandbox (plan 7C Task 11): the only way its adapter starts a
   * process, with `env` the session environment. Absent for the built-in agent (spec §8.9).
   */
  readonly sandbox?: SessionSandbox;
}

/** Tokens a model call is projected to use (the catalogue's `Usage`). */
export interface ProjectedUsage {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
  readonly reasoning: number;
}

/**
 * Decision 21's check before each model call. `ok` reserves the projected money until the
 * call's `usage` event settles it (one reservation per `usage` event, oldest first); an attempt
 * that ends without a `usage` event calls `release`. `ok: false` ends the run: the runtime
 * stops the session at a safe point (`BUDGET_EXCEEDED`), or, for a per-run cap, only the run.
 */
export interface CallGate {
  beforeCall(projected: ProjectedUsage): { readonly ok: true } | { readonly ok: false; readonly error: FinError };
  release(): void;
}

export interface TurnInput {
  readonly runId: string;
  readonly text: string;
  /** Budget facts and the project, branch and mode: a user-role message at the head of the run. */
  readonly facts?: string;
}

export interface UserInput {
  readonly text: string;
  /** A steer's `userMessage` node and its time (`agent.session.steer` records it first), when the runtime has one. */
  readonly nodeId?: string;
  readonly at?: number;
}

export type RuntimeAnswer =
  /** `message`: the server's reason for a deny, for the provider to show its model (Claude Code's `deny.message`). */
  | { readonly kind: "approval"; readonly allow: boolean; readonly message?: string }
  | { readonly kind: "userInput"; readonly text: string }
  /** The result of a tool the runtime ran for the adapter (the built-in loop's tools, the fake's). */
  | { readonly kind: "toolResult"; readonly ok: boolean; readonly output: unknown };

/** An event and the instance that emitted it. */
export interface BoundEvent {
  readonly instance: string;
  readonly event: ProviderEvent;
}

export interface ProviderSession {
  /** This instance's id; every event it emits carries it. */
  readonly instance: string;
  /** One ordered stream; it ends when the instance is gone (after `exited`, or its scope closes). */
  readonly events: Stream.Stream<BoundEvent>;
  startTurn(i: TurnInput): Effect.Effect<void, FinError>;
  steer(i: UserInput): Effect.Effect<void, FinError>;
  interrupt(): Effect.Effect<void>;
  respond(id: string, a: RuntimeAnswer): Effect.Effect<void, FinError>;
  /** Capability-gated (7B). */
  rollback(toTurn: number): Effect.Effect<void, FinError>;
  ref(): ProviderRef | null;
}

export interface ProviderAdapter {
  readonly harness: Harness;
  detect(): Effect.Effect<Detection, FinError>;
  open(spec: SessionSpec): Effect.Effect<ProviderSession, FinError, Scope.Scope>;
}

const KNOWN = new Set<string>(ProviderEvent.members.map((m) => m.fields.type.literal));
const decodeEvent = Schema.decodeUnknownResult(ProviderEvent);

/**
 * A provider message as a `ProviderEvent`: a known type that does not decode is
 * `PROVIDER_PROTOCOL` (drift in a message the adapter needs); a type the contract does not
 * know becomes `{type: "unknown", name}`, which the runtime records as a notice.
 */
export function toProviderEvent(raw: unknown): ProviderEvent | FinError {
  const type = (raw as { type?: unknown } | null)?.type;
  if (typeof type !== "string" || type.length === 0) {
    return finError("PROVIDER_PROTOCOL", "a provider event without a type", { got: typeof type });
  }
  if (!KNOWN.has(type) || type === "unknown") return { type: "unknown", name: type.slice(0, 64) };
  const r = decodeEvent(raw);
  if (r._tag === "Success") return r.success;
  return finError("PROVIDER_PROTOCOL", `a ${type} event does not match the contract`, { type, issue: String(r.failure) });
}

export const NO_CAPABILITIES: Capabilities = {
  steer: false,
  rollback: false,
  resume: false,
  reasoning: false,
  images: false,
  parallelTools: false,
  costReported: false,
  exactCapture: false,
  nativeSubagents: false,
};
