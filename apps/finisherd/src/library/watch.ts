// Watches of local library roots (plan 5D Decision 30), in the `library-indexer` thread: Node's
// recursive `fs.watch` walks the tree synchronously on Linux, so it never runs on the main
// thread. A watch is a **hint**: events (raw name bytes, `encoding: "buffer"`) collect the
// directories they touch, and after 2 s without another event the thread posts them; the
// service lists them with a `mode: dirs` scan (trigger `watch`). An event without a file name,
// or more than 256 directories at once, asks for a poll of the whole library instead. An
// `error` (an inotify `ENOSPC`, a dropped watch) degrades the library to 5-minute polls with
// `WATCH_DEGRADED` until the next restart: the watcher is closed and not restarted.
//
// The `fs.watch` of a root is the one JS call on a library path (Decision 1); it only
// observes. Dot-files and dot-directories are excluded by default (Decision 16), so their
// events are dropped here.
import { type FSWatcher, watch as fsWatch } from "node:fs";

export const WATCH_QUIET_MS = 2_000;
export const WATCH_MAX_DIRS = 256;

/** What the indexer thread posts to the main thread (`{event: WatchEvent}`). */
export type WatchEvent =
  | { readonly type: "watch"; readonly lib: string; readonly dirs: ReadonlyArray<{ readonly components: ReadonlyArray<string> }>; readonly all: boolean }
  | { readonly type: "watchDegraded"; readonly lib: string; readonly reason: string };

export interface WatchOptions {
  readonly quietMs?: number;
  readonly maxDirs?: number;
  /**
   * The deepest directory of `components` (raw bytes) that the library's catalog knows, as
   * base64 components: a listing of an unknown directory would fail its batch, and a new
   * directory is reached through its known parent.
   */
  readonly known: (lib: string, components: ReadonlyArray<Uint8Array>) => ReadonlyArray<string>;
}

interface Watch {
  readonly watcher: FSWatcher;
  readonly dirs: Map<string, ReadonlyArray<string>>;
  all: boolean;
  timer: ReturnType<typeof setTimeout> | undefined;
}

const SLASH = 0x2f;
const BACKSLASH = 0x5c;
const DOT = 0x2e;

/** Splits a watch event's file name into raw components (Windows: `\` and `/`). */
export function splitRaw(name: Uint8Array, windows = process.platform === "win32"): Uint8Array[] {
  const out: Uint8Array[] = [];
  let start = 0;
  for (let i = 0; i <= name.length; i++) {
    if (i === name.length || name[i] === SLASH || (windows && name[i] === BACKSLASH)) {
      if (i > start) out.push(name.subarray(start, i));
      start = i + 1;
    }
  }
  return out;
}

export class Watches {
  readonly #post: (e: WatchEvent) => void;
  readonly #quietMs: number;
  readonly #maxDirs: number;
  readonly #known: WatchOptions["known"];
  readonly #watches = new Map<string, Watch>();

  constructor(post: (e: WatchEvent) => void, opts: WatchOptions) {
    this.#post = post;
    this.#quietMs = opts.quietMs ?? WATCH_QUIET_MS;
    this.#maxDirs = opts.maxDirs ?? WATCH_MAX_DIRS;
    this.#known = opts.known;
  }

  /** Starts watching `root` for `lib` (once); `{ok: false}` when the OS refused (degraded). */
  watch(lib: string, root: string): { ok: true } | { ok: false; reason: string } {
    if (this.#watches.has(lib)) return { ok: true };
    let w: Watch | undefined;
    let watcher: FSWatcher;
    try {
      watcher = fsWatch(root, { recursive: true, encoding: "buffer", persistent: true }, (_type, name) => {
        if (w !== undefined) this.#event(lib, w, name as Buffer | string | null);
      });
    } catch (e) {
      return { ok: false, reason: codeOf(e) };
    }
    w = { watcher, dirs: new Map(), all: false, timer: undefined };
    this.#watches.set(lib, w);
    watcher.on("error", (e) => this.#degrade(lib, codeOf(e)));
    return { ok: true };
  }

  unwatch(lib: string): void {
    const w = this.#watches.get(lib);
    if (w === undefined) return;
    this.#watches.delete(lib);
    if (w.timer !== undefined) clearTimeout(w.timer);
    w.watcher.close();
  }

  /** Test hooks: the watcher fails as an overflow would. */
  fault(lib: string): boolean {
    const w = this.#watches.get(lib);
    if (w === undefined) return false;
    w.watcher.emit("error", Object.assign(new Error("watch overflow (test)"), { code: "WATCH_OVERFLOW" }));
    return true;
  }

  watching(): string[] {
    return [...this.#watches.keys()];
  }

  closeAll(): void {
    for (const lib of [...this.#watches.keys()]) this.unwatch(lib);
  }

  #event(lib: string, w: Watch, name: Buffer | string | null): void {
    if (this.#watches.get(lib) !== w) return;
    if (name === null) w.all = true;
    else if (!w.all) {
      // Node's recursive watcher on Linux (`internal/fs/recursive_watch`) ignores `encoding:
      // "buffer"` and hands names as UTF-8 strings: a non-UTF-8 name arrives lossy, and its
      // directory then maps to its deepest catalogued ancestor like any unknown one.
      const parts = splitRaw(typeof name === "string" ? Buffer.from(name, "utf8") : name);
      if (parts.length === 0 || parts.some((p) => p[0] === DOT)) return;
      const dir = this.#known(lib, parts.slice(0, -1));
      w.dirs.set(dir.join("/"), dir);
      if (w.dirs.size > this.#maxDirs) {
        w.all = true;
        w.dirs.clear();
      }
    }
    if (w.timer !== undefined) clearTimeout(w.timer);
    w.timer = setTimeout(() => this.#flush(lib, w), this.#quietMs);
  }

  #flush(lib: string, w: Watch): void {
    w.timer = undefined;
    if (this.#watches.get(lib) !== w) return;
    const all = w.all;
    const dirs = all ? [] : [...w.dirs.values()].map((components) => ({ components }));
    w.dirs.clear();
    w.all = false;
    if (all || dirs.length > 0) this.#post({ type: "watch", lib, dirs, all });
  }

  #degrade(lib: string, reason: string): void {
    if (!this.#watches.has(lib)) return;
    this.unwatch(lib);
    this.#post({ type: "watchDegraded", lib, reason });
  }
}

function codeOf(e: unknown): string {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === "string" && /^[A-Z][A-Z0-9_]{1,63}$/.test(code) ? code : "WATCH_ERROR";
}
