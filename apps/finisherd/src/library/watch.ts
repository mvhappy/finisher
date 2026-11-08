// Watches of local library roots (plan 5D Decision 30; the Tasks 9–11 review), in the
// `library-indexer` thread. A watch is a **hint**: the directories it reports collect, and
// after 2 s without another event the thread posts them; the service lists them with a `mode:
// dirs` scan (trigger `watch`). An event without a name, a lost queue, or more than 256
// directories at once asks for a poll of the whole library instead.
//
// - **Linux:** fin-node's `libraryWatchStart`: one inotify watch per directory, on the root's
//   mount only (no nested mount is descended into, no automount triggered), never a
//   dot-directory, walked and read on a thread of its own (not this thread). Before it starts,
//   the catalog's directory count must fit what is left of `fs.inotify.max_user_watches`
//   (less what this thread's other watches reserved and a 10% margin): one that does not fit
//   is never started (`WATCH_LIMIT`, the library is polled).
// - **macOS and Windows:** Node's recursive `fs.watch` (FSEvents, `ReadDirectoryChangesW`),
//   with dot paths ignored.
//
// Any failure, at start or later (`ENOSPC`, `EACCES`, a removed root, an overflowed budget),
// degrades the library to 5-minute polls with `WATCH_DEGRADED`; a watch never runs partially.
// The service starts it again when the library comes back online. Nothing here opens a file
// under a root (Decision 1).
import { type FSWatcher, readFileSync, watch as fsWatch } from "node:fs";
import { libraryWatchStart } from "@finisher/fin-node";

export const WATCH_QUIET_MS = 2_000;
export const WATCH_MAX_DIRS = 256;
/** Watches kept free of `max_user_watches` for everything else the user runs. */
const MARGIN = 0.1;

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
  /** `fs.inotify.max_user_watches` (tests); `undefined`: not Linux or unreadable. */
  readonly maxUserWatches?: () => number | undefined;
  /** Use the native watcher (default: on Linux). */
  readonly native?: boolean;
}

interface Watch {
  readonly close: () => void;
  readonly dirs: Map<string, ReadonlyArray<string>>;
  /** Watches reserved of `max_user_watches` (Linux). */
  readonly reserved: number;
  all: boolean;
  ready: boolean;
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

/**
 * Whether a relative path (`fs.watch`'s `ignore`) has a dot component. With `encoding:
 * "buffer"` Node passes it as bytes where the watch is native (Windows' recursive watch), as a
 * string where Node walks the tree itself (Linux): both are taken (a dot and the separators are
 * ASCII, so the bytes need no decoding).
 */
export const dotPath = (rel: string | Uint8Array): boolean =>
  typeof rel === "string" ? rel.split(/[\\/]/).some((c) => c.startsWith(".")) : splitRaw(rel).some((c) => c[0] === DOT);

function readMaxUserWatches(): number | undefined {
  try {
    const n = Number(readFileSync("/proc/sys/fs/inotify/max_user_watches", "utf8").trim());
    return Number.isSafeInteger(n) && n > 0 ? n : undefined;
  } catch {
    return undefined;
  }
}

/** Watches a library needs for `dirs` catalogued directories: room to grow by a quarter. */
export const watchNeed = (dirs: number): number => Math.max(0, Math.ceil(dirs * 1.25)) + 256;

export class Watches {
  readonly #post: (e: WatchEvent) => void;
  readonly #quietMs: number;
  readonly #maxDirs: number;
  readonly #known: WatchOptions["known"];
  readonly #maxUser: () => number | undefined;
  readonly #native: boolean;
  readonly #watches = new Map<string, Watch>();

  constructor(post: (e: WatchEvent) => void, opts: WatchOptions) {
    this.#post = post;
    this.#quietMs = opts.quietMs ?? WATCH_QUIET_MS;
    this.#maxDirs = opts.maxDirs ?? WATCH_MAX_DIRS;
    this.#known = opts.known;
    this.#native = opts.native ?? process.platform === "linux";
    this.#maxUser = opts.maxUserWatches ?? readMaxUserWatches;
  }

  /**
   * Starts watching `root` for `lib` (once); `dirs`: the catalog's live directories (the
   * budget). `{ok: false}` when it was refused before it started (degraded); a failure later
   * posts `watchDegraded`.
   */
  watch(lib: string, root: string, dirs = 0): { ok: true } | { ok: false; reason: string } {
    if (this.#watches.has(lib)) return { ok: true };
    return this.#native ? this.#startNative(lib, root, dirs) : this.#startFs(lib, root);
  }

  #startNative(lib: string, root: string, dirs: number): { ok: true } | { ok: false; reason: string } {
    const max = this.#maxUser();
    const need = watchNeed(dirs);
    let left = Number.MAX_SAFE_INTEGER;
    if (max !== undefined) {
      const reserved = [...this.#watches.values()].reduce((n, w) => n + w.reserved, 0);
      left = Math.floor(max * (1 - MARGIN)) - reserved;
      if (need > left) return { ok: false, reason: "WATCH_LIMIT" };
    }
    let w: Watch | undefined;
    let handle: { close(): void };
    try {
      handle = libraryWatchStart(root, Math.min(left, 0xffff_fffe), (json) => {
        if (w !== undefined) this.#message(lib, w, json);
      });
    } catch (e) {
      return { ok: false, reason: codeOf(e) };
    }
    w = { close: () => handle.close(), dirs: new Map(), reserved: max === undefined ? 0 : need, all: false, ready: false, timer: undefined };
    this.#watches.set(lib, w);
    return { ok: true };
  }

  #startFs(lib: string, root: string): { ok: true } | { ok: false; reason: string } {
    let w: Watch | undefined;
    let watcher: FSWatcher;
    try {
      watcher = fsWatch(root, { recursive: true, encoding: "buffer", persistent: true, ignore: dotPath }, (_type, name) => {
        if (w !== undefined) this.#named(lib, w, name as Buffer | string | null);
      });
    } catch (e) {
      return { ok: false, reason: codeOf(e) };
    }
    w = { close: () => watcher.close(), dirs: new Map(), reserved: 0, all: false, ready: true, timer: undefined };
    watcher.on("error", (e) => this.#degrade(lib, codeOf(e)));
    this.#watches.set(lib, w);
    return { ok: true };
  }

  unwatch(lib: string): void {
    const w = this.#watches.get(lib);
    if (w === undefined) return;
    this.#watches.delete(lib);
    if (w.timer !== undefined) clearTimeout(w.timer);
    w.close();
  }

  /** Test hooks: the watcher fails as an overflow would. */
  fault(lib: string): boolean {
    if (!this.#watches.has(lib)) return false;
    this.#degrade(lib, "WATCH_OVERFLOW");
    return true;
  }

  watching(): string[] {
    return [...this.#watches.keys()];
  }

  /** Whether `lib`'s first walk finished (tests). */
  ready(lib: string): boolean {
    return this.#watches.get(lib)?.ready === true;
  }

  closeAll(): void {
    for (const lib of [...this.#watches.keys()]) this.unwatch(lib);
  }

  /** A native watch's message (`libraryWatchStart`). */
  #message(lib: string, w: Watch, json: string): void {
    if (this.#watches.get(lib) !== w) return;
    let m: { ready?: unknown; dirs?: unknown; all?: unknown; failed?: unknown };
    try {
      m = JSON.parse(json) as typeof m;
    } catch {
      return this.#degrade(lib, "WATCH_ERROR");
    }
    if (typeof m.failed === "string") return this.#degrade(lib, /^[A-Z][A-Z0-9_]{1,63}$/.test(m.failed) ? m.failed : "WATCH_ERROR");
    if (typeof m.ready === "number") {
      w.ready = true;
      return;
    }
    if (m.all === true) w.all = true;
    else if (Array.isArray(m.dirs)) {
      for (const d of m.dirs as Array<{ components?: unknown }>) {
        if (w.all || !Array.isArray(d.components)) continue;
        this.#addDir(lib, w, (d.components as string[]).map((c) => new Uint8Array(Buffer.from(c, "base64"))));
      }
    } else return;
    this.#arm(lib, w);
  }

  /** An `fs.watch` event: the directory of the entry it names. */
  #named(lib: string, w: Watch, name: Buffer | string | null): void {
    if (this.#watches.get(lib) !== w) return;
    if (name === null) w.all = true;
    else if (!w.all) {
      const parts = splitRaw(typeof name === "string" ? Buffer.from(name, "utf8") : name);
      if (parts.length === 0 || parts.some((p) => p[0] === DOT)) return;
      this.#addDir(lib, w, parts.slice(0, -1));
    }
    this.#arm(lib, w);
  }

  #addDir(lib: string, w: Watch, parts: ReadonlyArray<Uint8Array>): void {
    if (parts.some((p) => p[0] === DOT)) return;
    const dir = this.#known(lib, parts);
    w.dirs.set(dir.join("/"), dir);
    if (w.dirs.size > this.#maxDirs) {
      w.all = true;
      w.dirs.clear();
    }
  }

  #arm(lib: string, w: Watch): void {
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
