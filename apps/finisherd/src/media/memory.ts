// Memory rules of the batch media kinds (6B-1 plan Decision 18, spec §14, through 5C's
// `MemoryRule`): `media.proxy`, `media.thumbs` and `media.probeDeep` on the `batch` priority
// take `min(4 GiB, 25 % of RAM)`, and `min(6 GiB, 35 %)` when the source's display size
// passes 1920 × 1080 (more pixels: a 4K decode, its scaled frames and the encoder's
// lookahead). `media.look` keeps 6A's `MEDIA_MEMORY` (`media.look.ts`): half the RAM up to
// 6 GiB for its warm frame cache, a deviation from §14's interactive row recorded in spec §26.
import type { MemoryRule } from "../jobs/kinds.ts";

export const BATCH_MEMORY: MemoryRule = { ramFraction: 0.25, capMiB: 4096 };
export const BATCH_MEMORY_LARGE: MemoryRule = { ramFraction: 0.35, capMiB: 6144 };

/** The rule for a source of `display` size (unknown: the smaller rule). */
export function mediaBatchMemory(display: { readonly width: number; readonly height: number } | undefined): MemoryRule {
  return display !== undefined && display.width * display.height > 1920 * 1080 ? BATCH_MEMORY_LARGE : BATCH_MEMORY;
}
