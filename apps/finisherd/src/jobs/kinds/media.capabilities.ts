// The `media.capabilities` kind (6A plan Task 13): what the `media` family's worker can decode,
// encode and draw on, answered by a warm worker. It takes no params and no files; equal
// submissions coalesce while one is queued or running (`dedupe`). The RPC caches the answer
// for the server's run (`apps/finisherd/src/media/caps.ts`).
import type { KindSpec } from "../kinds.ts";

export const MEDIA_CAPABILITIES = "media.capabilities";

/** `media.capabilities` v1. */
export function mediaCapabilitiesKind(): KindSpec<Record<string, never>, unknown> {
  return {
    kind: MEDIA_CAPABILITIES,
    version: 1,
    family: "media",
    scopes: ["server.read"],
    resources: [],
    warm: true,
    defaultPriority: "interactive",
    qos: "normal",
    // Opening an adapter can take a while on a cold machine (a software one compiles shaders).
    timeoutS: 60,
    stallS: 30,
    maxAttempts: 2,
    dedupe: () => MEDIA_CAPABILITIES,
  };
}
