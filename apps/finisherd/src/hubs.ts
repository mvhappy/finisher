// Live fan-out for subscribers: per-(project, branch) op channels and per-project event
// channels. They outlive engine reopens, so a live `ops.subscribe` or `project.events` keeps
// going when an evicted engine is reopened (unless the eviction ended it).
//
// A channel is a sliding PubSub (a slow subscriber loses the oldest items, never blocks a
// commit) plus a short log of *endings*: a rewind, a rebuild, an engine eviction. Endings
// must never be lost with the items a slow subscriber drops, so they are not only published:
// each bumps the channel's `seq`, every item carries the `seq` it was published at, and a
// subscriber that receives an item with a higher `seq` than it last saw replays the endings
// in between from the log. One it can no longer replay (the log is pruned) ends it.
import type { CommittedTx } from "@finisher/core-types";
import type { FinError, ProjectEvent } from "@finisher/contracts";
import { Effect, PubSub } from "effect";
import { finError } from "./errors.ts";

/** One ending: subscribers that reported a rev ≥ `fromRev` (0: every one) end with `error`. */
export interface Ending {
  readonly seq: number;
  readonly fromRev: number;
  readonly error: FinError;
}

/** An item as delivered: `value` is absent on the marker an ending publishes. */
export interface Stamped<T> {
  readonly seq: number;
  readonly value?: T;
}

/** Endings kept per channel for subscribers that lost their markers. */
export const ENDINGS_KEPT = 64;

export class Channel<T> {
  readonly pubsub: PubSub.PubSub<Stamped<T>>;
  #seq = 0;
  readonly #endings: Ending[] = [];

  constructor(capacity: number) {
    this.pubsub = Effect.runSync(PubSub.sliding<Stamped<T>>(capacity));
  }

  /** The `seq` a new subscriber starts from: read it before subscribing to `pubsub`. */
  get seq(): number {
    return this.#seq;
  }

  publish(value: T): void {
    Effect.runSync(PubSub.publish(this.pubsub, { seq: this.#seq, value }));
  }

  end(fromRev: number, error: FinError): void {
    this.#seq += 1;
    this.#endings.push({ seq: this.#seq, fromRev, error });
    if (this.#endings.length > ENDINGS_KEPT) this.#endings.shift();
    Effect.runSync(PubSub.publish(this.pubsub, { seq: this.#seq }));
  }

  /** The endings after `seen` up to `upTo`; `undefined` when some were pruned already. */
  endingsIn(seen: number, upTo: number): ReadonlyArray<Ending> | undefined {
    const oldest = this.#seq - this.#endings.length + 1;
    if (seen + 1 < oldest) return undefined;
    return this.#endings.filter((e) => e.seq > seen && e.seq <= upTo);
  }
}

/**
 * Tracks what one subscriber has seen of a channel. `check(item)` fails when an ending it
 * has not applied yet applies to it (`last`: the highest rev it reported, or started from).
 */
export function follower<T>(channel: Channel<T>, branch: string) {
  let seen = channel.seq;
  return (item: Stamped<T>, last: number): FinError | undefined => {
    if (item.seq <= seen) return undefined;
    const missed = channel.endingsIn(seen, item.seq);
    seen = item.seq;
    if (missed === undefined) {
      return finError(
        "HISTORY_REWOUND",
        `this subscriber fell too far behind to tell whether the history of ${branch} it reported still holds`,
        { branch, rev: 0, reason: "TOO_FAR_BEHIND" },
        `read ${branch} again (snapshot.get) and subscribe from that rev`,
      );
    }
    return missed.find((e) => last >= e.fromRev)?.error;
  };
}

const key = (projectId: string, branch: string) => `${projectId}\u0000${branch}`;

export class Hubs {
  readonly #capacity: number;
  readonly #ops = new Map<string, Channel<CommittedTx>>();
  readonly #events = new Map<string, Channel<ProjectEvent>>();

  constructor(capacity: number) {
    this.#capacity = capacity;
  }

  ops(projectId: string, branch: string): Channel<CommittedTx> {
    const k = key(projectId, branch);
    let ch = this.#ops.get(k);
    if (!ch) {
      ch = new Channel<CommittedTx>(this.#capacity);
      this.#ops.set(k, ch);
    }
    return ch;
  }

  /**
   * Publishes `tx` on its branch. Commits and imports may publish the same rev, in either
   * order; subscribers drop revs they already reported (no dedupe here, which would drop a
   * new-history rev published before its rewind is known).
   */
  publishTx(projectId: string, tx: CommittedTx): void {
    this.ops(projectId, tx.tx.branch).publish(tx);
  }

  /** `branch` lost its history from `fromRev` on. */
  rewind(projectId: string, branch: string, fromRev: number, error: FinError): void {
    this.ops(projectId, branch).end(fromRev, error);
  }

  /** Every op channel of the project ends `error` for subscribers that reported rev ≥ `fromRev`. */
  endAll(projectId: string, fromRev: number, error: (branch: string) => FinError): void {
    const prefix = key(projectId, "");
    for (const [k, ch] of this.#ops) {
      if (k.startsWith(prefix)) ch.end(fromRev, error(k.slice(prefix.length)));
    }
  }

  events(projectId: string): Channel<ProjectEvent> {
    let ch = this.#events.get(projectId);
    if (!ch) {
      ch = new Channel<ProjectEvent>(this.#capacity);
      this.#events.set(projectId, ch);
    }
    return ch;
  }

  publishEvent(projectId: string, event: ProjectEvent): void {
    this.events(projectId).publish(event);
  }

  /** Ends every `project.events` subscriber of the project. */
  endEvents(projectId: string, error: FinError): void {
    this.events(projectId).end(0, error);
  }
}
