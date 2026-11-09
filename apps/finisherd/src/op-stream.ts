// `ops.subscribe` over the registry's hubs and engine calls: the backlog from the op log, then
// live commits and imports, every rev exactly once and in order, ended by the hub's endings.
import type { CommittedTx } from "@finisher/core-types";
import type { FinError, Generation } from "@finisher/contracts";
import { Effect, PubSub, Stream } from "effect";
import type { Engine } from "@finisher/fin-node";
import { finError } from "./errors.ts";
import { follower, type Hubs, type Stamped } from "./hubs.ts";

/** A rewound history, as the op stream reports it to a subscriber that saw it. */
export function historyRewound(details: Record<string, unknown>): FinError {
  const branch = String(details.branch);
  const message =
    details.reason === undefined
      ? `the history of ${branch} from rev ${String(details.rev)} on was set aside`
      : details.reason === "GENERATION"
        ? `the history of ${branch} was rewound or rebuilt since the caller read it`
        : `the project database was rebuilt (${String(details.reason)}); revs read before may hold something else`;
  return finError("HISTORY_REWOUND", message, details, `read ${branch} again (snapshot.get) and subscribe from that rev`);
}

/** Ends the streams of an evicted (or closed) engine: their clients reconnect, which reopens it. */
export function engineEvicted(projectId: string, reason: "evicted" | "closed" = "evicted"): FinError {
  return finError(
    "ENGINE_CLOSED",
    reason === "closed" ? "the project's engine was closed" : "the project's engine stopped; its history may have changed since",
    { projectId, reason },
    "read the project again and subscribe again (that reopens it)",
  );
}

export interface OpStreamDeps {
  readonly hubs: Hubs;
  readonly resolveBranch: (projectId: string, branch: string) => Effect.Effect<string, FinError>;
  /** An engine call; re-resolves the engine each time (an eviction between calls reopens it). */
  readonly on: <A>(projectId: string, f: (engine: Engine) => Promise<string>) => Effect.Effect<A, FinError>;
  /** Called once the stream let go of its channel (the engine's idle clock restarts). */
  readonly released: () => void;
}

/** See `ProjectRegistryShape.subscribe`. */
export function opStream(
  { hubs, resolveBranch, on, released }: OpStreamDeps,
  projectId: string,
  requested: string,
  fromRev: number,
  generation: Generation | undefined,
): Stream.Stream<CommittedTx, FinError> {
  return Stream.unwrap(
    Effect.gen(function* () {
      // Hubs are keyed by id: a name, or `<name>#<id>`, resolves before anything else.
      const branch = yield* resolveBranch(projectId, requested);
      const channel = yield* Effect.acquireRelease(
        Effect.sync(() => hubs.acquireOps(projectId, branch)),
        () =>
          Effect.sync(() => {
            hubs.releaseOps(projectId, branch);
            released();
          }),
      );
      // Before subscribing: an ending published from now on is seen, as its marker or
      // through the `seq` of any later item. One whose engine event lands after the
      // backlog below was read at its new generation is skipped (`check` gets that
      // generation): the subscriber already reads the new history.
      const check = follower(channel, branch);
      // Subscribe before reading the backlog: a commit landing in between is then
      // both in the backlog and on the hub, and the rev filter below drops the copy.
      const live = yield* PubSub.subscribe(channel.pubsub);
      // Every engine call re-resolves the engine (`on`): an eviction between calls must
      // not leave this stream talking to a closed engine. The head is read before the
      // backlog; a rewind landing in between is caught by the generation check below.
      const head = yield* on<number>(projectId, async (e) => String(await e.head(branch)));
      const read = yield* on<{ generation: Generation; ops: CommittedTx[] }>(projectId, (e) => e.opsSinceAt(branch, fromRev));
      if (
        generation !== undefined &&
        (generation.incarnation !== read.generation.incarnation || generation.n !== read.generation.n)
      ) {
        return yield* Effect.fail(
          historyRewound({ branch, rev: 0, reason: "GENERATION", generation: read.generation, expected: generation }),
        );
      }
      if (fromRev > head) {
        return yield* Effect.fail(
          finError(
            "BASE_REV_AHEAD",
            `fromRev ${fromRev} is ahead of the head of ${branch} (rev ${head})`,
            { branch, fromRev, head },
            `subscribe from a rev at most ${head} (snapshot.get reports the head)`,
          ),
        );
      }
      let last = fromRev;
      const emit = (item: Stamped<CommittedTx>): Effect.Effect<ReadonlyArray<CommittedTx>, FinError> => {
        const ended = check(item, last, read.generation);
        if (ended !== undefined) return Effect.fail(ended);
        const c = item.value;
        if (c === undefined || c.rev <= last) return Effect.succeed([]);
        if (c.rev === last + 1) {
          last = c.rev;
          return Effect.succeed([c]);
        }
        // A gap: the sliding hub dropped items for this slow subscriber. Fill from the log,
        // read with its generation: a log rewound or rebuilt since the backlog was read
        // holds other history, so the stream ends before any of it (its own ending may
        // not have reached the hub yet).
        return Effect.flatMap(
          on<{ generation: Generation; ops: CommittedTx[] }>(projectId, (e) => e.opsSinceAt(branch, last)),
          (missing) => {
            const now = missing.generation;
            if (now.incarnation !== read.generation.incarnation || now.n !== read.generation.n) {
              return Effect.fail(historyRewound({ branch, rev: 0, reason: "GENERATION", generation: now, expected: read.generation }));
            }
            const out = missing.ops.filter((m) => m.rev > last && m.rev <= c.rev);
            if (out.length > 0) last = out[out.length - 1]!.rev;
            return Effect.succeed(out);
          },
        );
      };
      const backlog = read.ops.map((tx): Stamped<CommittedTx> => ({ seq: -1, value: tx }));
      return Stream.concat(Stream.fromIterable(backlog), Stream.fromSubscription(live)).pipe(
        Stream.mapEffect(emit),
        Stream.flattenIterable,
      );
    }),
  );
}
