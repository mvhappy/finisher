// `agent.watch`'s fan-out (spec §3.5; plan 7A Task 6b): live items per session, and a watch
// that takes the stored nodes first and then the live items, every node exactly once. The
// runtime publishes a node only after the store wrote it, in seq order per session.
//
// A watcher that reads too slowly is never dropped from silently: once it holds `bufferSize`
// live items unread (beyond the snapshot it still has to read), its watch ends with
// `WATCH_LAGGING {lastSeq}` and the client watches again from the next node (`fromNode`).
import type { Node as AgentNode, FinError, Session, WatchItem } from "@finisher/contracts";
import { Cause, Effect, Queue, Stream } from "effect";
import { finError } from "../errors.ts";
import { sessionNotFound } from "./store-rows.ts";

type Listener = (item: WatchItem) => void;

/** What a watch reads its snapshot from. */
export interface WatchSource {
  get(sessionId: string): Session | undefined;
  nodes(sessionId: string, fromSeq?: number, limit?: number): ReadonlyArray<AgentNode>;
  lastSeq(sessionId: string): number;
}

const PAGE = 1000;
/** Live items one watcher may hold unread (`agent.watch`'s bound). */
export const WATCH_BUFFER = 1000;

/** The watcher fell behind (exit 5): watch again from the node after `lastSeq`. */
export function watchLagging(sessionId: string, lastSeq: number, bufferSize: number): FinError {
  return finError(
    "WATCH_LAGGING",
    `the watcher fell ${bufferSize} items behind`,
    { sessionId, lastSeq, bufferSize },
    "watch again from the node after `lastSeq` (`fromNode`)",
  );
}

export class WatchHub {
  readonly #listeners = new Map<string, Set<Listener>>();
  /** Per watched session: the highest node seq published. */
  readonly #published = new Map<string, number>();

  /** Whether anyone watches `sessionId`. */
  watched(sessionId: string): boolean {
    return (this.#listeners.get(sessionId)?.size ?? 0) > 0;
  }

  /** The highest seq published for a watched session, else undefined. */
  published(sessionId: string): number | undefined {
    return this.#published.get(sessionId);
  }

  setPublished(sessionId: string, seq: number): void {
    if (this.watched(sessionId)) this.#published.set(sessionId, seq);
  }

  publish(sessionId: string, item: WatchItem): void {
    const set = this.#listeners.get(sessionId);
    if (!set) return;
    for (const l of [...set]) l(item);
  }

  #on(sessionId: string, l: Listener, lastSeq: number): () => void {
    let set = this.#listeners.get(sessionId);
    if (!set) {
      set = new Set();
      this.#listeners.set(sessionId, set);
      this.#published.set(sessionId, lastSeq);
    }
    set.add(l);
    return () => {
      const s = this.#listeners.get(sessionId);
      if (!s) return;
      s.delete(l);
      if (s.size === 0) {
        this.#listeners.delete(sessionId);
        this.#published.delete(sessionId);
      }
    };
  }

  /**
   * The session's nodes from `fromSeq` on, its status, then live items; it ends after the
   * session's `ended` status. Subscribing and reading the snapshot happen in one synchronous
   * step, so nothing published in between is missed or repeated.
   */
  watch(source: WatchSource, sessionId: string, fromSeq = 1, bufferSize = WATCH_BUFFER): Stream.Stream<WatchItem, FinError> {
    const on = (l: Listener, last: number) => this.#on(sessionId, l, last);
    return Stream.callback<WatchItem, FinError>((queue) =>
      Effect.gen(function* () {
        const session = source.get(sessionId);
        if (session === undefined) return yield* Effect.try({ try: () => sessionNotFound(sessionId), catch: (e) => e as FinError });
        let snapshotMax = 0;
        let snapshotSize = 0;
        /** The highest node seq offered to this watcher: where a new watch resumes. */
        let offered = Math.max(0, fromSeq - 1);
        let ended = false;
        const deliver = (item: WatchItem): void => {
          if (ended) return;
          if (item.type === "nodeAdded" && item.node.seq <= snapshotMax) return;
          if (Queue.sizeUnsafe(queue) >= snapshotSize + bufferSize) {
            ended = true;
            Queue.failCauseUnsafe(queue, Cause.fail(watchLagging(sessionId, offered, bufferSize)));
            return;
          }
          if (item.type === "nodeAdded") offered = item.node.seq;
          Queue.offerUnsafe(queue, item);
          if (item.type === "statusChanged" && item.status === "ended") {
            ended = true;
            Queue.endUnsafe(queue);
          }
        };
        const off = on(deliver, source.lastSeq(sessionId));
        yield* Effect.addFinalizer(() => Effect.sync(off));
        for (let seq = Math.max(1, fromSeq); ; ) {
          const page = source.nodes(sessionId, seq, PAGE);
          for (const node of page) Queue.offerUnsafe(queue, { type: "nodeAdded", node });
          snapshotSize += page.length;
          if (page.length > 0) {
            snapshotMax = page[page.length - 1]!.seq;
            offered = snapshotMax;
            seq = snapshotMax + 1;
          }
          if (page.length < PAGE) break;
        }
        snapshotMax = Math.max(snapshotMax, source.lastSeq(sessionId));
        deliver({
          type: "statusChanged",
          status: session.status,
          ...(session.statusReason === undefined ? {} : { reason: session.statusReason }),
        });
      }),
    );
  }
}
