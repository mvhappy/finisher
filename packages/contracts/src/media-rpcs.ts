// The `media.*` RPCs of `FinRpcs`, spread into it (`rpc.ts`) so that file stays small; their
// rules are in `scopes.ts`, their payloads in `media.ts`.
import { Rpc } from "effect/rpc";
import { Schema } from "effect";
import { FinError } from "./errors.ts";
import { MediaCacheGcPayload, MediaCacheGcSuccess, MediaCacheStatusSuccess, MediaCapabilitiesSuccess, MediaLookPayload, MediaLookSuccess, MediaProbeDeepPayload, MediaProbeDeepSuccess } from "./media.ts";

export const MEDIA_RPCS = [
  /** Plan 6A Task 13: looks at a timeline, and what this machine's media worker can do. */
  Rpc.make("media.look", { payload: MediaLookPayload, success: MediaLookSuccess, error: FinError }),
  Rpc.make("media.capabilities", { payload: Schema.Struct({}), success: MediaCapabilitiesSuccess, error: FinError }),
  /** 6B-1 Task 13: the media cache's status, and an eviction down to the cap or lower. */
  Rpc.make("media.cache.status", { payload: Schema.Struct({}), success: MediaCacheStatusSuccess, error: FinError }),
  Rpc.make("media.cache.gc", { payload: MediaCacheGcPayload, success: MediaCacheGcSuccess, error: FinError }),
  /** 6B-1 Task 11: the decode-level probe of one asset (commits nothing). */
  Rpc.make("media.probeDeep", { payload: MediaProbeDeepPayload, success: MediaProbeDeepSuccess, error: FinError }),
] as const;
