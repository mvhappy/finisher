// The provider adapter contract (sub-project 7 §4.1; plan 7A Decision 5). Adapters emit only
// `ProviderEvent`s, each bound to the instance (process or HTTP stream) that emitted it; the
// runtime alone writes nodes, budgets and journal entries, and drops events from an instance
// it replaced. No session fork (cut in revision 2).
import { type FinError, type Harness, type Mode, ProviderEvent } from "@finisher/contracts";
import { type Effect, Schema, type Scope, type Stream } from "effect";
import { finError } from "../errors.ts";

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
}

export interface TurnInput {
  readonly runId: string;
  readonly text: string;
  /** Budget facts and the project, branch and mode: a user-role message at the head of the run. */
  readonly facts?: string;
}

export interface UserInput {
  readonly text: string;
}

export type RuntimeAnswer =
  | { readonly kind: "approval"; readonly allow: boolean }
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
