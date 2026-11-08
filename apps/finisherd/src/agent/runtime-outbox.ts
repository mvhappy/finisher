// The runtime's outbox delivery (plan 7A Task 6b, Decision 3): every effect a transition queued
// outside `control.db`, oldest first, at least once. Credential effects are Task 7's
// (`SessionCredentials.handle`, idempotent); the journal phases and the synthetic tool results
// go to the `RuntimeJournal`. One that fails stays queued and holds its session's later ones
// until the next drain (after every `apply`, and at boot).
import { toFinError } from "../errors.ts";
import type { SessionCredentials } from "./credentials.ts";
import type { RuntimeJournal } from "./runtime-deps.ts";
import type { AgentStore, OutboxItem } from "./store.ts";

export interface OutboxDeps {
  readonly store: AgentStore;
  readonly credentials: Pick<SessionCredentials, "handle">;
  readonly journal: RuntimeJournal;
  /** True once the runtime is gone (a simulated crash): nothing more is delivered. */
  readonly dead: () => boolean;
  readonly log: (message: string, details: Record<string, unknown>) => void;
}

export class OutboxDrainer {
  readonly #d: OutboxDeps;
  #draining: Promise<void> = Promise.resolve();

  constructor(deps: OutboxDeps) {
    this.#d = deps;
  }

  /** Delivers every queued effect, after any drain already running. */
  drain(): Promise<void> {
    const next = this.#draining.then(() => this.#once());
    this.#draining = next.catch(() => {});
    return next;
  }

  async #once(): Promise<void> {
    const { store } = this.#d;
    const blocked = new Set<string>();
    for (const item of store.outbox()) {
      if (this.#d.dead()) return;
      if (blocked.has(item.sessionId)) continue;
      try {
        if (!this.#d.credentials.handle(item)) await this.#deliver(item);
        store.outboxDone(item.id);
      } catch (e) {
        blocked.add(item.sessionId);
        this.#d.log("agent runtime: an outbox effect failed; it is retried later", { sessionId: item.sessionId, kind: item.effect.kind, error: toFinError(e).code });
      }
    }
  }

  async #deliver(item: OutboxItem): Promise<void> {
    const e = item.effect;
    switch (e.kind) {
      case "journal":
        return void (await this.#d.journal.session(item.sessionId, this.#d.store.get(item.sessionId), e.phase, e.reason));
      case "revokeCredential":
      case "refreshCredential":
        return; // `credentials.handle` took them
      case "toolResult":
        return this.#d.journal.toolResult({
          sessionId: item.sessionId,
          runId: e.runId,
          callId: e.callId,
          ok: false,
          output: { code: "SESSION_INTERRUPTED", synthetic: true },
          synthetic: true,
        });
    }
  }
}
