// What a plugin process writes to the server log (sp8b plan Task 7; spec §11): its `log`
// calls and its stderr, never journaled. Both are scrubbed (paths become `<path>`, control
// characters a space) and named by the plugin's keyed id (a bundled plugin by its name), so
// the server log carries no third-party id in the clear. `log`: at most 4 KiB a message and
// 20 messages a second per process, the rest counted and reported once the window allows
// again; stderr: at most 64 KiB a minute per process, likewise counted.
import { createHmac, randomBytes } from "node:crypto";
import { stripPaths } from "../scrub.ts";

export const LOG_MESSAGE_BYTES = 4096;
export const LOG_PER_SECOND = 20;
export const STDERR_BYTES_PER_MINUTE = 64 * 1024;
/** Processes tracked at once; the oldest window is forgotten past it. */
const MAX_SOURCES = 1024;

export type ServerLogLevel = "debug" | "info" | "warning" | "error";
export type ServerLog = (level: ServerLogLevel, message: string) => void;

/** C0, DEL, C1, line and paragraph separators and bidi controls. */
const CONTROL = new RegExp("[\\u0000-\\u001f\\u007f-\\u009f\\u2028\\u2029\\u200e\\u200f\\u202a-\\u202e\\u2066-\\u2069]", "g");

/** `text` scrubbed and cut to `max` UTF-8 bytes (an ellipsis marks a cut). */
export function scrubLogText(text: string, max = LOG_MESSAGE_BYTES): string {
  // Control characters (C0, DEL, C1, line and paragraph separators, bidi controls) become a space.
  // At most `max` UTF-16 units go through the scrub (a UTF-8 byte is at least one of them).
  const truncated = text.length > max * 2;
  const flat = stripPaths((truncated ? text.slice(0, max * 2) : text).replace(CONTROL, " "));
  const size = Buffer.byteLength(flat);
  if (!truncated && size <= max) return flat;
  if (size <= max - 3) return `${flat}…`;
  // Cut by code point, never inside a surrogate pair, in one pass.
  let bytes = 0;
  let cut = "";
  for (const ch of flat) {
    const n = Buffer.byteLength(ch);
    if (bytes + n > max - 3) break;
    bytes += n;
    cut += ch;
  }
  return `${cut}…`;
}

/** A fixed window: `take(cost)` until `limit` is spent within `windowMs`; refusals are counted. */
class Window {
  #start = 0;
  #used = 0;
  dropped = 0;
  readonly limit: number;
  readonly windowMs: number;
  constructor(limit: number, windowMs: number) {
    this.limit = limit;
    this.windowMs = windowMs;
  }
  /** Whether `cost` fits now; on a new window, how many were dropped in the last ones. */
  take(now: number, cost: number): { ok: boolean; reportDropped: number } {
    let reportDropped = 0;
    if (now - this.#start >= this.windowMs) {
      this.#start = now;
      this.#used = 0;
      reportDropped = this.dropped;
      this.dropped = 0;
    }
    if (this.#used + cost > this.limit) {
      this.dropped++;
      return { ok: false, reportDropped };
    }
    this.#used += cost;
    return { ok: true, reportDropped };
  }
}

export interface PluginLogDeps {
  readonly write: ServerLog;
  readonly clock?: () => number;
  /** Whether `id` is a bundled plugin (named in the clear). */
  readonly bundled?: (id: string) => boolean;
}

/** The plugin id (no version) of `<id>@<version>`. */
const idOf = (plugin: string): string => (plugin.lastIndexOf("@") > 0 ? plugin.slice(0, plugin.lastIndexOf("@")) : plugin);

export function makePluginLog(d: PluginLogDeps) {
  const clock = d.clock ?? Date.now;
  // A key of this run only: the server log is this process's, and the journal's keyed hash
  // (which never reaches TypeScript) is for the journal.
  const key = randomBytes(32);
  const name = (plugin: string): string => {
    const id = idOf(plugin);
    return d.bundled?.(id) === true ? id : `plugin#${createHmac("sha256", key).update(id).digest("hex").slice(0, 16)}`;
  };
  const windows = new Map<string, Window>();
  const windowOf = (source: string, make: () => Window): Window => {
    let w = windows.get(source);
    if (w === undefined) {
      if (windows.size >= MAX_SOURCES) windows.delete(windows.keys().next().value as string);
      w = make();
      windows.set(source, w);
    }
    return w;
  };
  const level = (l: string): ServerLogLevel => (l === "warn" ? "warning" : l === "error" ? "error" : l === "info" ? "info" : "debug");
  return {
    /** The display name of `plugin` in the server log. */
    name,
    /** A `log` call of the process `source` (its warm key, or `job:<id>`); whether it was written. */
    call(source: string, plugin: string, lvl: string, msg: string): boolean {
      const w = windowOf(`log:${source}`, () => new Window(LOG_PER_SECOND, 1000));
      const { ok, reportDropped } = w.take(clock(), 1);
      if (reportDropped > 0) d.write("warning", `[${name(plugin)}] ${reportDropped} log messages dropped (over ${LOG_PER_SECOND} a second)`);
      if (ok) d.write(level(lvl), `[${name(plugin)}] ${scrubLogText(msg)}`);
      return ok;
    },
    /** A stderr line of the process `source`. */
    stderr(source: string, plugin: string, line: string): boolean {
      // The window first, by the line's length as capped, so a flood is never scrubbed.
      const w = windowOf(`stderr:${source}`, () => new Window(STDERR_BYTES_PER_MINUTE, 60_000));
      const { ok, reportDropped } = w.take(clock(), Math.min(Buffer.byteLength(line), LOG_MESSAGE_BYTES));
      const text = ok ? scrubLogText(line) : "";
      if (reportDropped > 0) d.write("warning", `[${name(plugin)} stderr] ${reportDropped} lines dropped (over ${STDERR_BYTES_PER_MINUTE} bytes a minute)`);
      if (ok) d.write("debug", `[${name(plugin)} stderr] ${text}`);
      return ok;
    },
  };
}

export type PluginLog = ReturnType<typeof makePluginLog>;
