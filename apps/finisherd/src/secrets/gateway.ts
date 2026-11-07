// `ModelGateway` (plan 7A Decision 23; sub-project 7 §10): the only path from a stored key to a
// model request. It picks the project's OpenRouter key, else the server's default, opens it
// for one request, and zero-fills the buffer it was opened into when the request settles. A
// session that is not running gets no key. The first grant per session and every refusal are
// audited.
//
// What zero-filling does and does not do (review L1): `f` receives the key as a JS string,
// because the HTTP client needs one; strings are immutable and live until the garbage
// collector reclaims them, so that copy cannot be wiped. Only the store's plaintext buffer is
// zero-filled. The bound is that the string is made per request and never kept or logged.
import type { Clock } from "../agent/clock.ts";
import { FinError } from "@finisher/contracts";
import { finError } from "../errors.ts";
import type { SecretStore } from "./store.ts";

/** One audit record (5A's `Audit` in 7A-2; the `grant` kind). Never holds a value. */
export interface GrantAudit {
  readonly kind: "grant";
  readonly outcome: "granted" | "denied";
  readonly provider: "openrouter";
  readonly sessionId: string;
  readonly projectId: string;
  /** The key used: its id and whether it is the project's or the server's default. */
  readonly secretId?: string;
  readonly scope?: "project" | "server";
  readonly reason?: string;
  readonly at: number;
}

export interface AuditSink {
  record(e: GrantAudit): void;
}

export interface RunningSessions {
  isRunning(sessionId: string): boolean;
}

const REDACTED = "[redacted]";

export class ModelGateway {
  private readonly granted = new Set<string>();

  constructor(
    private readonly store: SecretStore,
    private readonly sessions: RunningSessions,
    private readonly audit: AuditSink,
    private readonly clock: Clock,
  ) {}

  /** Whether a key would be found for `projectId` (for `agent.providers` and start checks). */
  hasKey(projectId: string): boolean {
    return this.pick(projectId) !== undefined;
  }

  /**
   * Calls `f` with the OpenRouter key for one request. `PROVIDER_AUTH` when the session is not
   * running or no key is set. An error from `f` that quotes the key is rethrown with the key
   * replaced.
   */
  async withKey<A>(projectId: string, sessionId: string, f: (key: string) => Promise<A>): Promise<A> {
    const base = { kind: "grant" as const, provider: "openrouter" as const, sessionId, projectId };
    if (!this.sessions.isRunning(sessionId)) {
      this.audit.record({ ...base, outcome: "denied", reason: "sessionNotRunning", at: this.clock.now() });
      throw finError("PROVIDER_AUTH", "the session is not running, so it gets no model key", { sessionId }, "resume the session");
    }
    const entry = this.pick(projectId);
    if (entry === undefined) {
      this.audit.record({ ...base, outcome: "denied", reason: "noKey", at: this.clock.now() });
      throw finError(
        "PROVIDER_AUTH",
        "no OpenRouter key is set for this project or the server",
        { provider: "openrouter" },
        "a person sets one with `finisherd provider key set openrouter` (the key on stdin)",
      );
    }
    const buf = this.store.reveal(entry.id);
    try {
      if (!this.granted.has(sessionId)) {
        this.granted.add(sessionId);
        this.audit.record({ ...base, outcome: "granted", secretId: entry.id, scope: entry.scope, at: this.clock.now() });
      }
      const key = buf.toString("latin1");
      try {
        return await f(key);
      } catch (e) {
        throw scrubbed(e, key);
      }
    } finally {
      buf.fill(0);
    }
  }

  /** Forgets a session's first grant (it ended); the next session with the id is audited again. */
  forget(sessionId: string): void {
    this.granted.delete(sessionId);
  }

  private pick(projectId: string): { id: string; scope: "project" | "server" } | undefined {
    const keys = this.store.list().filter((m) => m.provider === "openrouter");
    const own = keys.find((m) => m.projectId === projectId);
    if (own) return { id: own.id, scope: "project" };
    const server = keys.find((m) => m.projectId === undefined);
    return server ? { id: server.id, scope: "server" } : undefined;
  }
}

/**
 * `e`, or when it quotes `key` anywhere (message, stack, fields, the `cause` chain), a new
 * error in its place:
 * a `FinError` keeps its code and fix with the key replaced in the message and the details
 * dropped; anything else becomes `PROVIDER_PROTOCOL` with a cleaned message.
 */
function scrubbed(e: unknown, key: string): unknown {
  const quotes = (v: unknown): boolean => {
    if (typeof v === "string") return v.includes(key);
    try {
      return JSON.stringify(v)?.includes(key) ?? false;
    } catch {
      return true;
    }
  };
  /** Whether `x` or anything on its `cause` chain quotes the key (`cause` is not enumerable). */
  const leaks = (x: unknown, depth: number): boolean => {
    if (depth > 8) return true;
    if (!(x instanceof Error)) return quotes(x);
    if (quotes(x.message) || quotes(x.stack) || quotes({ ...x })) return true;
    return x.cause !== undefined && leaks(x.cause, depth + 1);
  };
  if (!leaks(e, 0)) return e;
  const err = e instanceof Error ? e : undefined;
  // The replacement has no cause: the chain that quoted the key is dropped whole.
  const message = (err?.message ?? String(e)).split(key).join(REDACTED);
  if (e instanceof FinError) {
    return new FinError({ code: e.code, message, details: {}, ...(e.fix === undefined ? {} : { fix: e.fix }) });
  }
  return finError("PROVIDER_PROTOCOL", message, {});
}
