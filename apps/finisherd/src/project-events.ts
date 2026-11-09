// `project.events` (`ProjectRegistry.events`): the status now, then every status change and
// rewind/rebuild/background error, in the order the engines emitted them.
import type { FinError, ProjectEvent } from "@finisher/contracts";
import { Effect, PubSub, Stream } from "effect";
import { finError } from "./errors.ts";
import { follower, gapless, type Hubs } from "./hubs.ts";

/**
 * What the event channel carries: an engine event, the engine that emitted it (`source`, the
 * registry's number for it, rising with every engine registered) and its number there (`seq`).
 */
export interface SourcedEvent {
  readonly event: ProjectEvent;
  readonly source: number;
  readonly seq: number;
}

/** A status and the last event its engine (`source`) emitted before it was read. */
export interface StatusAt {
  readonly status: unknown;
  readonly source: number;
  readonly seq: number;
}

export interface ProjectEventsDeps {
  readonly hubs: Hubs;
  /** The events channels' buffer (`WATCH_LAGGING`'s `buffer`). */
  readonly capacity: number;
  readonly statusAt: (projectId: string) => Effect.Effect<StatusAt, FinError>;
  /** The subscriber left. */
  readonly released: () => void;
}

/** A `project.events` subscriber lost events to its full buffer (review #38). */
export const eventsLagging = (projectId: string, buffer: number): FinError =>
  finError(
    "WATCH_LAGGING",
    `this project.events subscriber fell more than ${buffer} events behind and was ended`,
    { projectId, buffer },
    "subscribe to project.events again: its first item is the status now",
  );

/** Whether `e` happened before the status `at` was read: its change is in that status (review #39). */
export const covered = (e: SourcedEvent, at: StatusAt): boolean => e.source < at.source || (e.source === at.source && e.seq <= at.seq);

/**
 * Subscribes first, so nothing emitted while the status is read is missed, then drops what that
 * status already covers: a change is reported once, and never after a status newer than it.
 * Ends `WATCH_LAGGING` when the sliding buffer dropped anything (a rewind, a rebuild or an
 * error must not vanish), and with the channel's endings (an eviction, `project.close`).
 */
export const projectEvents = (deps: ProjectEventsDeps, projectId: string): Stream.Stream<ProjectEvent, FinError> =>
  Stream.unwrap(
    Effect.gen(function* () {
      const channel = yield* Effect.acquireRelease(
        Effect.sync(() => deps.hubs.acquireEvents(projectId)),
        () =>
          Effect.sync(() => {
            deps.hubs.releaseEvents(projectId);
            deps.released();
          }),
      );
      const check = follower(channel, "");
      const lost = gapless(channel);
      const live = yield* PubSub.subscribe(channel.pubsub);
      const at = yield* deps.statusAt(projectId);
      const first: ProjectEvent = { type: "status", status: at.status };
      const items = Stream.fromSubscription(live).pipe(
        Stream.mapEffect((item) => {
          const ended = check(item, 0);
          if (ended !== undefined) return Effect.fail(ended);
          if (lost(item)) return Effect.fail(eventsLagging(projectId, deps.capacity));
          const v = item.value;
          return Effect.succeed(v === undefined || covered(v, at) ? [] : [v.event]);
        }),
        Stream.flattenIterable,
      );
      return Stream.concat(Stream.make(first), items);
    }),
  );
