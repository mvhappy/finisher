// `library.watch` (plan 5D Task 13): a snapshot of the libraries' views, then what changes. The
// stream holds its libraries in use while it is open (Decision 28: demand polling at 60 s),
// follows their `library.scan` jobs (`scan`, `progress`), reports a view that changed
// (`library`; checked once a second, so a poll that found nothing new says nothing) and
// forwards the service's file events, coalesced: at most five batches a second of at most 200
// events, with the rest counted in `dropped`. A slow reader loses the oldest items of a sliding
// buffer, never the server's memory.
import type { FinError, LibraryViewResult, LibraryWatchEvent } from "@finisher/contracts";
import { Effect, PubSub, Stream } from "effect";
import type { JobEngine } from "../jobs/api.ts";
import type { FileEvent } from "./api.ts";
import { publicJob, withDirWarnings } from "./shape.ts";
import type { LibraryService } from "./service.ts";

export const VIEW_CHECK_MS = 1000;
export const FILES_FLUSH_MS = 200;
export const FILES_BATCH = 200;
const BUFFER = 1024;

/** The stream of `libraries` (ids already resolved and reachable). */
export function watchLibraries(
  svc: LibraryService,
  engine: Pick<JobEngine, "events">,
  libraries: ReadonlyArray<string>,
  timing: { readonly viewCheckMs?: number; readonly flushMs?: number } = {},
): Stream.Stream<LibraryWatchEvent, FinError> {
  const ids = new Set(libraries);
  return Stream.unwrap(
    Effect.gen(function* () {
      const bus = yield* PubSub.sliding<LibraryWatchEvent>(BUFFER);
      const sub = yield* PubSub.subscribe(bus);
      const publish = (e: LibraryWatchEvent): void => void Effect.runSync(PubSub.publish(bus, e));
      const hold = svc.use([...ids]);
      const last = new Map<string, string>();
      const views = (): Promise<LibraryViewResult[]> =>
        Promise.all(
          [...ids].flatMap((id) => {
            const row = svc.registry.get(id);
            return row === undefined ? [] : [withDirWarnings(svc, svc.view(row))];
          }),
        );
      const snapshot = yield* Effect.promise(views);
      for (const v of snapshot) last.set(v.id, JSON.stringify(v));

      let checking = false;
      const check = async (): Promise<void> => {
        if (checking) return;
        checking = true;
        try {
          for (const v of await views()) {
            const now = JSON.stringify(v);
            if (last.get(v.id) === now) continue;
            last.set(v.id, now);
            publish({ type: "library", library: v });
          }
        } finally {
          checking = false;
        }
      };
      const viewTimer = setInterval(() => void check(), timing.viewCheckMs ?? VIEW_CHECK_MS);
      viewTimer.unref?.();

      let queued: FileEvent[] = [];
      let dropped = 0;
      let flushTimer: ReturnType<typeof setTimeout> | undefined;
      const flush = (): void => {
        flushTimer = undefined;
        if (queued.length === 0 && dropped === 0) return;
        publish({ type: "files", events: queued, dropped });
        queued = [];
        dropped = 0;
      };
      const off = svc.onFile((e) => {
        if (!ids.has(e.library)) return;
        if (queued.length < FILES_BATCH) queued.push(e);
        else dropped++;
        flushTimer ??= setTimeout(flush, timing.flushMs ?? FILES_FLUSH_MS);
      });
      yield* Effect.addFinalizer(() =>
        Effect.sync(() => {
          clearInterval(viewTimer);
          if (flushTimer !== undefined) clearTimeout(flushTimer);
          off();
          hold.release();
        }),
      );

      const jobs = engine.events().pipe(
        Stream.map((e): ReadonlyArray<LibraryWatchEvent> => {
          if (e.type === "item") return [];
          const j = e.job;
          if (j.kind !== "library.scan" || j.library === undefined || !ids.has(j.library)) return [];
          if (e.type === "state") return [{ type: "scan", library: j.library, job: publicJob(j) }];
          return j.progress === undefined ? [] : [{ type: "progress", library: j.library, job: j.id, progress: j.progress }];
        }),
        Stream.flattenIterable,
      );
      return Stream.concat(Stream.make({ type: "snapshot", libraries: snapshot } as LibraryWatchEvent), Stream.merge(Stream.fromSubscription(sub), jobs));
    }),
  );
}
