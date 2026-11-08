// The catalog threads' default clock (plan 5D Task 7b): the wall clock with jumps held off.
// Backward jumps are absorbed by the indexer (`max(at, lastAt)`); this guards forward ones,
// which would otherwise confirm a `DIR_EMPTY_SUSPECT` at once and age every missing row: a
// reading more than `slackMs` ahead of the last reading plus the monotonic time since is
// held to that expectation, so catalog time keeps the monotonic pace. A jump that persists
// (a corrected clock, a resumed laptop: the monotonic clock stops during sleep) is accepted
// after `persistMs` of monotonic time, and from then on the wall clock is followed again.
export interface GuardedClockOptions {
  readonly wall?: () => number;
  readonly mono?: () => number;
  readonly slackMs?: number;
  readonly persistMs?: number;
}

export function guardedClock(o: GuardedClockOptions = {}): () => number {
  const wall = o.wall ?? Date.now;
  const mono = o.mono ?? (() => performance.now());
  const slack = o.slackMs ?? 5 * 60_000;
  const persist = o.persistMs ?? 10 * 60_000;
  let last: { wall: number; mono: number } | undefined;
  let aheadSince: number | undefined;
  return () => {
    const w = wall();
    const m = mono();
    if (last === undefined) {
      last = { wall: w, mono: m };
      return w;
    }
    const expected = last.wall + (m - last.mono);
    if (w > expected + slack) {
      aheadSince ??= m;
      if (m - aheadSince < persist) {
        last = { wall: expected, mono: m };
        return expected;
      }
    }
    aheadSince = undefined;
    last = { wall: w, mono: m };
    return w;
  };
}
