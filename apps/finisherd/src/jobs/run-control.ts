// The engine's handle on a running job (spec §6.2–6.3, 5C-1 plan Decisions 23, 29, 37): the
// strongest stop request wins (cancel > close > pause); a pause asked before a close is
// remembered, so the job is paused, not interrupted, when the close stops it (9b review M2).
// Every attempt and every `prepare` of the run binds to it; a request reaches each at once.

/**
 * Why a run is stopped. `close` is a graceful stop of `finisherd`: a resumable job goes back
 * to the queue without spending an attempt (as a pause, 9b review L6); only a crash, found at
 * the next start, spends one.
 */
export type StopReason = "cancel" | "pause" | "close";
const STOP_RANK: Record<StopReason, number> = { pause: 0, close: 1, cancel: 2 };

export class RunControl {
  requested: StopReason | undefined;
  /** A pause was asked (it may have been outranked by a close since). */
  pauseRequested = false;
  #stops = new Set<(reason: StopReason, graceMs: number) => void>();
  #grace = 0;

  request(reason: StopReason, graceMs: number): void {
    if (reason === "pause") this.pauseRequested = true;
    if (this.requested !== undefined && STOP_RANK[this.requested] >= STOP_RANK[reason]) return;
    this.requested = reason;
    this.#grace = graceMs;
    for (const stop of [...this.#stops]) stop(reason, graceMs);
  }

  /** Calls `stop` for each request from now on, and at once for a request made before; the function returned unbinds it. */
  bind(stop: (reason: StopReason, graceMs: number) => void): () => void {
    this.#stops.add(stop);
    if (this.requested !== undefined) stop(this.requested, this.#grace);
    return () => {
      this.#stops.delete(stop);
    };
  }
}
