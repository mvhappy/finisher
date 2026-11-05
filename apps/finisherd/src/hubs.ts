// Live fan-out for subscribers: per-(project, branch) op hubs and per-project event hubs.
// They outlive engine reopens, so a live `ops.subscribe` or `project.events` keeps going
// when an evicted engine is reopened.
import type { CommittedTx } from "@finisher/core-types";
import type { ProjectEvent } from "@finisher/contracts";
import { Effect, PubSub } from "effect";

/**
 * One op-hub item: a committed or imported tx, or a marker that the branch's history from
 * `fromRev` on was set aside (a rewind; `fromRev` 1 for a rebuilt database). A subscriber
 * that already reported a rev at or after `fromRev` (or was started from one) ends with
 * `HISTORY_REWOUND`; one still before it carries on.
 */
export type HubItem =
  | { readonly kind: "tx"; readonly tx: CommittedTx }
  | { readonly kind: "rewound"; readonly fromRev: number; readonly details: Readonly<Record<string, unknown>> };

interface OpHub {
  readonly pubsub: PubSub.PubSub<HubItem>;
  /** Highest rev published since the last rewind: commits and imports overlap, publish once. */
  top: number;
}

const key = (projectId: string, branch: string) => `${projectId}\u0000${branch}`;

export class Hubs {
  readonly #capacity: number;
  readonly #ops = new Map<string, OpHub>();
  readonly #events = new Map<string, PubSub.PubSub<ProjectEvent>>();

  constructor(capacity: number) {
    this.#capacity = capacity;
  }

  #hub(projectId: string, branch: string): OpHub {
    const k = key(projectId, branch);
    let hub = this.#ops.get(k);
    if (!hub) {
      // Sliding: a slow subscriber loses the oldest buffered items, never blocks commits;
      // the subscriber notices the rev gap and reads the missing revs from the log.
      hub = { pubsub: Effect.runSync(PubSub.sliding<HubItem>(this.#capacity)), top: 0 };
      this.#ops.set(k, hub);
    }
    return hub;
  }

  ops(projectId: string, branch: string): PubSub.PubSub<HubItem> {
    return this.#hub(projectId, branch).pubsub;
  }

  /** Publishes `tx` on its branch unless that rev was already published (deduped by rev). */
  publishTx(projectId: string, tx: CommittedTx): void {
    const hub = this.#hub(projectId, tx.tx.branch);
    if (tx.rev <= hub.top) return;
    hub.top = tx.rev;
    Effect.runSync(PubSub.publish(hub.pubsub, { kind: "tx", tx }));
  }

  /** `branch` lost its history from `fromRev` on. */
  rewind(projectId: string, branch: string, fromRev: number, details: Record<string, unknown>): void {
    const hub = this.#hub(projectId, branch);
    hub.top = Math.min(hub.top, fromRev - 1);
    Effect.runSync(PubSub.publish(hub.pubsub, { kind: "rewound", fromRev, details: { branch, rev: fromRev, ...details } }));
  }

  /**
   * Every branch of the project may hold something else now (a rebuilt database): every
   * subscriber that reported a rev ends. `details.rev` is 0, as on fin-store's own
   * `HISTORY_REWOUND` (`REBUILT`): read the branch again from the start.
   */
  rewindAll(projectId: string, details: Record<string, unknown>): void {
    const prefix = key(projectId, "");
    for (const k of this.#ops.keys()) {
      if (k.startsWith(prefix)) this.rewind(projectId, k.slice(prefix.length), 1, { ...details, rev: 0 });
    }
  }

  events(projectId: string): PubSub.PubSub<ProjectEvent> {
    let hub = this.#events.get(projectId);
    if (!hub) {
      hub = Effect.runSync(PubSub.sliding<ProjectEvent>(this.#capacity));
      this.#events.set(projectId, hub);
    }
    return hub;
  }

  publishEvent(projectId: string, event: ProjectEvent): void {
    Effect.runSync(PubSub.publish(this.events(projectId), event));
  }
}
