// Which libraries are **in use** (plan 5D Decision 28): a project that references the library
// is open, a `library.watch` stream on it is open (both hold a `use`), or it was searched,
// listed or scanned in the last 10 minutes (`touch`). In use, a demand-polled network library
// is polled every 60 s; otherwise every 6 hours.
//
// `onChange(id)` fires when a library becomes in use, so the service plans it again at once
// (its next poll moves from hours to a minute). Leaving use needs no wake-up: the next
// 60 s tick plans the longer interval.

export const RECENT_MS = 10 * 60_000;

export class Usage {
  readonly #clock: () => number;
  readonly #onChange: (id: string) => void;
  readonly #holds = new Map<string, number>();
  readonly #touched = new Map<string, number>();

  constructor(clock: () => number, onChange: (id: string) => void = () => {}) {
    this.#clock = clock;
    this.#onChange = onChange;
  }

  /** Holds `ids` in use until `release()` (idempotent). */
  use(ids: ReadonlyArray<string>): { release(): void } {
    const held = [...new Set(ids)];
    for (const id of held) {
      const was = this.inUse(id);
      this.#holds.set(id, (this.#holds.get(id) ?? 0) + 1);
      if (!was) this.#onChange(id);
    }
    let released = false;
    return {
      release: () => {
        if (released) return;
        released = true;
        for (const id of held) {
          const n = (this.#holds.get(id) ?? 1) - 1;
          if (n <= 0) this.#holds.delete(id);
          else this.#holds.set(id, n);
        }
      },
    };
  }

  /** The library was searched, listed or scanned now. */
  touch(id: string): void {
    const was = this.inUse(id);
    this.#touched.set(id, this.#clock());
    if (!was) this.#onChange(id);
  }

  inUse(id: string, now: number = this.#clock()): boolean {
    if ((this.#holds.get(id) ?? 0) > 0) return true;
    const at = this.#touched.get(id);
    return at !== undefined && now - at < RECENT_MS;
  }

  /** Holders of `id` (projects and streams), for `library.status`. */
  holders(id: string): number {
    return this.#holds.get(id) ?? 0;
  }

  forget(id: string): void {
    this.#holds.delete(id);
    this.#touched.delete(id);
  }
}
