// `media.capabilities` (6A plan Task 13): what this machine's media worker can decode, encode
// and draw on, answered by a warm `media.capabilities` job once per server run and kept.
// A failure is not kept: the next call asks again.
import type { FinError, JobOwner } from "@finisher/contracts";
import type { MediaCapabilities } from "@finisher/core-types";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { MEDIA_CAPABILITIES } from "../jobs/kinds/media.capabilities.ts";
import type { MediaRpcDeps } from "./look.ts";

const WAIT_MS = 60_000;

/** The cache of one server run: the answer, or the ask in flight (concurrent calls share it). */
export function mediaCapabilities(deps: Pick<MediaRpcDeps, "engine">): (owner: JobOwner) => Effect.Effect<MediaCapabilities, FinError> {
  let cached: MediaCapabilities | undefined;
  return (owner) =>
    Effect.suspend(() => {
      if (cached !== undefined) return Effect.succeed(cached);
      return Effect.gen(function* () {
        const job = yield* deps.engine.submit(owner, { kind: MEDIA_CAPABILITIES, params: {} });
        const [done] = yield* deps.engine.wait([job.id], { timeoutMs: WAIT_MS });
        if (done?.state !== "succeeded") {
          const e = done?.error;
          return yield* Effect.fail(e === undefined ? finError("JOB_FAILED", `media.capabilities ended ${done?.state ?? "unknown"}`, { job: job.id }) : finError(e.code, e.message, e.details, e.fix));
        }
        cached = done.result as MediaCapabilities;
        return cached;
      });
    });
}
