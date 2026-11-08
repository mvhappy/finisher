// Long catalog writes in steps (plan 5D Decision 2): an apply runs as a generator that yields
// whenever about a step's worth of rows (≤ the indexer's `txRows`, 2,000) has been written
// since the last yield. The indexer takes an event-loop turn at each yield, so other
// libraries' requests interleave, while the transaction stays open: readers see a subtree
// change (thousands of rows under one excluded or vanished directory) all at once or not at all.

/** Rows written since the last yield. */
export interface Budget {
  readonly step: number;
  rows: number;
  /** The most rows written between two yields so far (the indexer's stats). */
  maxRows: number;
}

/** Work that may yield between steps; `R` is its result. */
export type Work<R = void> = Generator<void, R, void>;

export const budget = (step: number): Budget => ({ step, rows: 0, maxRows: 0 });

/** A budget that never yields, for writes that run in one synchronous op. */
export const unlimited = (): Budget => budget(Number.MAX_SAFE_INTEGER);

/** Makes room for `n` more rows: yields first when they would overrun the step. */
export function* room(b: Budget, n: number): Work {
  if (b.rows > 0 && b.rows + n > b.step) {
    b.maxRows = Math.max(b.maxRows, b.rows);
    b.rows = 0;
    yield;
  }
  b.rows += n;
}

/** Ends a budget's last step (for its `maxRows`). */
export function settle(b: Budget): void {
  b.maxRows = Math.max(b.maxRows, b.rows);
  b.rows = 0;
}

/** Runs `w` to the end without yielding; answers its result. */
export function drain<R>(w: Work<R>): R {
  for (;;) {
    const r = w.next();
    if (r.done === true) return r.value;
  }
}

/** Splits `xs` into arrays of at most `n`. */
export function chunks<T>(xs: ReadonlyArray<T>, n: number): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < xs.length; i += n) out.push(xs.slice(i, i + n));
  return out;
}
