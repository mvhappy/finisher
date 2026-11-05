// Live fan-out for subscribers: per-(project, branch) op channels and per-project event
// channels. They outlive engine reopens, so a live `ops.subscribe` or `project.events` keeps
// going when an evicted engine is reopened (unless the eviction ended it). A channel is
// removed when its last subscriber leaves (see `Hubs`).
//
// A channel is a sliding PubSub (a slow subscriber loses the oldest items, never blocks a
// commit) plus a short log of *endings*: a rewind, a rebuild, an engine eviction. Endings
// must never be lost with the items a slow subscriber drops, so they are not only published:
// each bumps the channel's `seq`, every item carries the `seq` it was published at, and a
// subscriber that receives an item with a higher `seq` than it last saw replays the endings
// in between from the log. One it can no longer replay (the log is pruned) ends it.
import type { CommittedTx, Generation } from "@finisher/core-types";
import type { FinError, ProjectEvent } from "@finisher/contracts";
import { Effect, PubSub } from "effect";
import { finError } from "./errors.ts";

/**
 * One ending: subscribers that reported a rev ≥ `fromRev` (0: every one) end with `error`.
 * `produces` is the generation the ending leads to (a rewind's new generation; a rebuild's new
 * incarnation, `n` absent): a subscriber whose backlog was already read at that generation or
 * later reads the new history and is not ended. Events reach the hub on the engine's next
 * tick, so a subscriber can attach after the change but before its ending (an open that
 * rebuilt the database queues `rebuilt` before anyone could subscribe).
 */
export interface Ending {
  readonly seq: number;
  readonly fromRev: number;
  readonly error: FinError;
  readonly produces?: { readonly incarnation: string; readonly n?: number };
}

/** Whether a subscriber that read at `read` already sees what `ending` produced. */
function alreadyRead(ending: Ending, read: Generation | undefined): boolean {
  const p = ending.produces;
  if (p === undefined || read === undefined || p.incarnation !== read.incarnation) return false;
  return p.n === undefined || read.n >= p.n;
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

  end(fromRev: number, error: FinError, produces?: Ending["produces"]): void {
    this.#seq += 1;
    this.#endings.push(produces === undefined ? { seq: this.#seq, fromRev, error } : { seq: this.#seq, fromRev, error, produces });
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
 * has not applied yet applies to it (`last`: the highest rev it reported, or started from;
 * `read`: the generation its backlog was read at, if any).
 */
export function follower<T>(channel: Channel<T>, branch: string) {
  let seen = channel.seq;
  return (item: Stamped<T>, last: number, read?: Generation): FinError | undefined => {
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
    return missed.find((e) => last >= e.fromRev && !alreadyRead(e, read))?.error;
  };
}

const key = (projectId: string, branch: string) => `${projectId}\u0000${branch}`;

/** A channel and the subscribers holding it. */
interface Held<T> {
  readonly channel: Channel<T>;
  holders: number;
}

/**
 * Channels exist while a subscriber holds them. The last release removes the channel: nothing
 * else refers to it (engine events and commits look channels up by name when they publish, and
 * publish nothing when there is none).
 *
 * Dropping a channel (with its log of endings) cannot make a later subscriber miss an ending.
 * A subscriber always acquires its channel first and only then reads the backlog and the
 * branch's generation from the engine (`opsSinceAt`), so what it reads is the history as it
 * stands after every ending published before it acquired; an ending published later reaches
 * the channel it holds. A fresh channel starts at `seq` 0 with no log, and `follower` starts
 * from that `seq`, so nothing older is replayed to it.
 */
export class Hubs {
  readonly #capacity: number;
  readonly #ops = new Map<string, Held<CommittedTx>>();
  readonly #events = new Map<string, Held<ProjectEvent>>();

  constructor(capacity: number) {
    this.#capacity = capacity;
  }

  /** @internal Tests: how many channels exist (ops, events). */
  get size(): { ops: number; events: number } {
    return { ops: this.#ops.size, events: this.#events.size };
  }

  #acquire<T>(map: Map<string, Held<T>>, k: string): Channel<T> {
    let held = map.get(k);
    if (!held) {
      held = { channel: new Channel<T>(this.#capacity), holders: 0 };
      map.set(k, held);
    }
    held.holders += 1;
    return held.channel;
  }

  #release<T>(map: Map<string, Held<T>>, k: string): void {
    const held = map.get(k);
    if (!held) return;
    held.holders -= 1;
    if (held.holders <= 0) {
      map.delete(k);
      Effect.runSync(PubSub.shutdown(held.channel.pubsub));
    }
  }

  /** The op channel of the branch, held until `releaseOps`. */
  acquireOps(projectId: string, branch: string): Channel<CommittedTx> {
    return this.#acquire(this.#ops, key(projectId, branch));
  }

  releaseOps(projectId: string, branch: string): void {
    this.#release(this.#ops, key(projectId, branch));
  }

  /**
   * Publishes `tx` on its branch. Commits and imports may publish the same rev, in either
   * order; subscribers drop revs they already reported (no dedupe here, which would drop a
   * new-history rev published before its rewind is known).
   */
  publishTx(projectId: string, tx: CommittedTx): void {
    this.#ops.get(key(projectId, tx.tx.branch))?.channel.publish(tx);
  }

  /** `branch` lost its history from `fromRev` on. */
  rewind(projectId: string, branch: string, fromRev: number, error: FinError, produces?: Ending["produces"]): void {
    this.#ops.get(key(projectId, branch))?.channel.end(fromRev, error, produces);
  }

  /** Every op channel of the project ends `error` for subscribers that reported rev ≥ `fromRev`. */
  endAll(projectId: string, fromRev: number, error: (branch: string) => FinError, produces?: Ending["produces"]): void {
    const prefix = key(projectId, "");
    for (const [k, held] of this.#ops) {
      if (k.startsWith(prefix)) held.channel.end(fromRev, error(k.slice(prefix.length)), produces);
    }
  }

  /** The event channel of the project, held until `releaseEvents`. */
  acquireEvents(projectId: string): Channel<ProjectEvent> {
    return this.#acquire(this.#events, projectId);
  }

  releaseEvents(projectId: string): void {
    this.#release(this.#events, projectId);
  }

  publishEvent(projectId: string, event: ProjectEvent): void {
    this.#events.get(projectId)?.channel.publish(event);
  }

  /** Ends every `project.events` subscriber of the project. */
  endEvents(projectId: string, error: FinError): void {
    this.#events.get(projectId)?.channel.end(0, error);
  }
}
