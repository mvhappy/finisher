// `media.capabilities` (6A plan Task 13): what this machine's media worker can decode, encode
// and draw on, answered by a warm `media.capabilities` job once per server run and kept.
// A failure is not kept: the next call asks again. The first answer sizes the `hwenc` pool
// (6B-1 Task 9, Decision 18): the smallest session limit the probe counted on a hardware
// encoder, else 2 (the pool's default, which a failed or v1 answer leaves as it is).
import type { FinError, JobOwner } from "@finisher/contracts";
// v1 until the worker answers v2 (6B-1 Task 8).
import type { LegacyMediaCapabilities as MediaCapabilities } from "@finisher/core-types";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { MEDIA_CAPABILITIES } from "../jobs/kinds/media.capabilities.ts";
import type { JobEngine } from "../jobs/api.ts";
import type { MediaRpcDeps } from "./look.ts";

const WAIT_MS = 60_000;

/** `hwenc` when the report counts no session limit on a hardware encoder. */
export const HWENC_DEFAULT = 2;

/** The `hwenc` pool size a report asks for: the smallest counted limit of a hardware encoder
 * (the pool is shared by them all), else [`HWENC_DEFAULT`]. Reads v2's `encoders`; a v1
 * answer has none. */
export function hwencSize(answer: unknown): number {
  const encoders: ReadonlyArray<{ hw?: unknown; sessionLimit?: unknown }> =
    typeof answer === "object" && answer !== null && Array.isArray((answer as { encoders?: unknown }).encoders) ? (answer as { encoders: [] }).encoders : [];
  const limits = encoders.flatMap((e) => (e.hw === true && typeof e.sessionLimit === "number" && e.sessionLimit >= 1 ? [e.sessionLimit] : []));
  return limits.length === 0 ? HWENC_DEFAULT : Math.min(...limits);
}

/** The cache's ask, and its answer so far (sync: the `media.proxy` kind's resources read it, 6B-1 Task 14). */
export type CapabilitiesCache = ((owner: JobOwner) => Effect.Effect<MediaCapabilities, FinError>) & { readonly current: () => MediaCapabilities | undefined };

/** The cache of one server run: the answer, or the ask in flight (concurrent calls share it). */
export function mediaCapabilities(deps: {
  readonly engine: MediaRpcDeps["engine"] & Partial<Pick<JobEngine, "definePool">>;
  /** Called once, with the first answer (6B-1 Task 14: the `proxy540` stage registers then). */
  readonly onAnswer?: (answer: MediaCapabilities) => void;
}): CapabilitiesCache {
  let cached: MediaCapabilities | undefined;
  const ask = (owner: JobOwner): Effect.Effect<MediaCapabilities, FinError> =>
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
        const size = hwencSize(cached);
        if (size !== HWENC_DEFAULT) deps.engine.definePool?.("hwenc", size);
        deps.onAnswer?.(cached);
        return cached;
      });
    });
  return Object.assign(ask, { current: () => cached });
}
