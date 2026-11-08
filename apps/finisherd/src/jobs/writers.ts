// What an attempt writes besides transitions (Decision 26): item outcomes in one transaction
// per second or per 256 rows, and the newest checkpoint at most once per 10 s, flushed at the
// attempt's end. Both are at-least-once: what is held when `finisherd` dies is lost, so the
// items re-run and the job resumes from the last written checkpoint. `dead()` is true once the
// engine stopped writing (a test's simulated crash): nothing more reaches the store.
import type { ItemOutcome } from "./kinds.ts";
import type { JobStore } from "./store.ts";
import type { Timers } from "./timers.ts";

export const ITEM_FLUSH_MS = 1000;
export const ITEM_FLUSH_ROWS = 256;
export const CHECKPOINT_EVERY_MS = 10_000;

export interface WriterEnv {
  readonly store: JobStore;
  readonly timers: Timers;
  readonly now: () => number;
  dead(): boolean;
}

export class ItemWriter {
  #pending: Array<[string, ItemOutcome]> = [];
  #cancel: (() => void) | undefined;
  readonly #env: WriterEnv;
  readonly #id: string;

  constructor(env: WriterEnv, id: string) {
    this.#env = env;
    this.#id = id;
  }

  add(item: string, outcome: ItemOutcome): void {
    this.#pending.push([item, outcome]);
    if (this.#pending.length >= ITEM_FLUSH_ROWS) this.flush();
    else this.#cancel ??= this.#env.timers.after(ITEM_FLUSH_MS, () => this.flush());
  }

  flush(): void {
    this.#cancel?.();
    this.#cancel = undefined;
    const rows = this.#pending;
    this.#pending = [];
    if (!this.#env.dead()) this.#env.store.writeItems(this.#id, rows);
  }
}

export class CheckpointWriter {
  #last = -Infinity;
  #held: { data: unknown } | undefined;
  #cancel: (() => void) | undefined;
  readonly #env: WriterEnv;
  readonly #id: string;

  constructor(env: WriterEnv, id: string) {
    this.#env = env;
    this.#id = id;
  }

  add(data: unknown): void {
    const wait = this.#last + CHECKPOINT_EVERY_MS - this.#env.now();
    if (wait <= 0 && this.#cancel === undefined) {
      this.#write(data);
      return;
    }
    this.#held = { data };
    this.#cancel ??= this.#env.timers.after(Math.max(0, wait), () => {
      this.#cancel = undefined;
      this.flush();
    });
  }

  #write(data: unknown): void {
    this.#last = this.#env.now();
    if (!this.#env.dead()) this.#env.store.checkpoint(this.#id, data);
  }

  flush(): void {
    this.#cancel?.();
    this.#cancel = undefined;
    const held = this.#held;
    this.#held = undefined;
    if (held !== undefined) this.#write(held.data);
  }
}
