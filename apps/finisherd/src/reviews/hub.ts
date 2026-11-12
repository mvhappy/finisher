// The review hub (5E plan Decision 23): `review.watch`'s fan-out. Every item the store writes is
// published to its project's watchers (`ReviewStore`'s `onChange`); a watcher gets the project's
// items first, read in the same turn as its subscription, then each change. It never re-plans.
// A watcher `REVIEW_WATCH_BUFFER` events behind ends with `WATCH_LAGGING` (it lists and watches
// again), so one slow watcher never holds back a write or another watcher.
import { type FinError, REVIEW_WATCH_BUFFER, type ReviewItem, type ReviewWatchEvent } from "@finisher/contracts";
import { Cause, Effect, Queue, Stream } from "effect";
import { finError } from "../errors.ts";

type Listener = (item: ReviewItem) => void;

export class ReviewHub {
  readonly #listeners = new Map<string, Set<Listener>>();

  /** Hands `item` to the watchers of its project. */
  publish(item: ReviewItem): void {
    for (const l of [...(this.#listeners.get(item.projectId) ?? [])]) l(item);
  }

  /** Calls `listener` for each item published for `projectId` until the returned function runs. */
  subscribe(projectId: string, listener: Listener): () => void {
    const set = this.#listeners.get(projectId) ?? new Set<Listener>();
    set.add(listener);
    this.#listeners.set(projectId, set);
    return () => {
      set.delete(listener);
      if (set.size === 0 && this.#listeners.get(projectId) === set) this.#listeners.delete(projectId);
    };
  }

  /** Watchers of a project right now (for tests). */
  watchers(projectId: string): number {
    return this.#listeners.get(projectId)?.size ?? 0;
  }

  /** `items`: the project's items, read synchronously (the snapshot sent first). */
  watch(projectId: string, items: () => ReadonlyArray<ReviewItem>, buffer: number = REVIEW_WATCH_BUFFER): Stream.Stream<ReviewWatchEvent, FinError> {
    const hub = this;
    return Stream.callback<ReviewWatchEvent, FinError>(
      (queue) =>
        Effect.gen(function* () {
          const off = yield* Effect.sync(() => {
            Queue.offerUnsafe(queue, { type: "snapshot", items: items() });
            let ended = false;
            const off = hub.subscribe(projectId, (item) => {
              if (ended || Queue.offerUnsafe(queue, { type: "item", item })) return;
              ended = true;
              off();
              Queue.failCauseUnsafe(
                queue,
                Cause.fail(finError("WATCH_LAGGING", `this watcher fell ${buffer} review events behind and was ended`, { buffer }, "list the reviews again, then watch again")),
              );
            });
            return off;
          });
          yield* Effect.addFinalizer(() => Effect.sync(off));
        }),
      { bufferSize: buffer, strategy: "dropping" },
    );
  }
}
