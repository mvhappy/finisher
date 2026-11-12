// `plugin.stats` (sp8b plan Task 15; spec §11): per plugin and minute, its host calls by method
// family (calls, errors by code, reach denials, p50/p95, bytes in and out as log₂ buckets),
// its jobs that timed out or crashed, its dropped `log` calls and stderr lines, and its
// brokered requests (Task 8's per-plugin `EgressCounters`, differenced per window). Nothing
// free-text: a family is a `PLUGIN_METHOD_FAMILIES` name, an error a code, a size a bucket;
// the plugin goes to fin-node as its raw id (`recordPlugin`) and is named there.
//
// While the journal is off nothing is counted and no payload is built (`enabled()` is read on
// every count and at every flush); the egress baseline still moves, so a window after turning
// it on counts only its own requests.
import type { Journal } from "@finisher/fin-node";
import type { PluginEgressStats, PluginMethodStats, PluginStats } from "@finisher/core-types";
import type { EgressCounters } from "./egress.ts";

export const STATS_WINDOW_MS = 60_000;
/** Timings kept per family and window for the percentiles (a uniform reservoir past it). */
export const STATS_SAMPLES = 256;
/** Plugins tracked in one window; calls of others are not counted until the next. */
const MAX_PLUGINS = 1024;

const FAMILIES = new Set(["command", "project", "net", "log"]);
const REACH_CODES = new Set(["PROJECT_NOT_FOUND", "BRANCH_NOT_ALLOWED", "LIBRARY_NOT_FOUND"]);
const U32 = 0xffff_ffff;

type StatsJournal = Pick<Journal, "enabled" | "recordPlugin">;
type Payload = Omit<PluginStats, "plugin">;

/** The method family of a host method (`project.snapshot` → `project`), `other` otherwise. */
export function methodFamily(method: string): string {
  const head = method.split(".")[0] ?? "";
  return FAMILIES.has(head) ? head : "other";
}

/** `n` as a log₂ bucket: 0 for 0, else ⌊log₂ n⌋ + 1 (fin-core's `log2_bucket`). */
export function log2Bucket(n: number): number {
  return n <= 0 ? 0 : Math.floor(Math.log2(n)) + 1;
}

class Family {
  calls = 0;
  readonly errors = new Map<string, number>();
  reachDenials = 0;
  bytesIn = 0;
  bytesOut = 0;
  readonly samples: number[] = [];
  seen = 0;

  time(micros: number, random: () => number): void {
    this.seen++;
    if (this.samples.length < STATS_SAMPLES) this.samples.push(micros);
    else {
      const at = Math.floor(random() * this.seen);
      if (at < STATS_SAMPLES) this.samples[at] = micros;
    }
  }

  payload(family: string): PluginMethodStats {
    const sorted = [...this.samples].sort((a, b) => a - b);
    const pct = (p: number): number => (sorted.length === 0 ? 0 : Math.round(sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] ?? 0));
    return {
      family,
      calls: Math.min(this.calls, U32),
      ...(this.errors.size === 0 ? {} : { errors: Object.fromEntries(this.errors) }),
      reachDenials: Math.min(this.reachDenials, U32),
      p50Micros: pct(0.5),
      p95Micros: pct(0.95),
      bytesInLog2: log2Bucket(this.bytesIn),
      bytesOutLog2: log2Bucket(this.bytesOut),
    };
  }
}

class Plugin {
  readonly families = new Map<string, Family>();
  timeouts = 0;
  crashes = 0;
  logDropped = 0;
  stderrDropped = 0;

  family(name: string): Family {
    let f = this.families.get(name);
    if (f === undefined) {
      f = new Family();
      this.families.set(name, f);
    }
    return f;
  }
}

const EGRESS_FIELDS = [
  "requests", "completed", "denied", "notHttps", "privateAddress", "secretMissing", "rateLimited", "tooLarge", "timeouts", "failed", "redirectsFollowed", "redirectsReturned",
] as const satisfies ReadonlyArray<keyof EgressCounters & keyof PluginEgressStats>;

/** `now − before` as a window's egress, or `undefined` when nothing changed. */
function egressDelta(now: Readonly<EgressCounters>, before: Readonly<EgressCounters> | undefined): PluginEgressStats | undefined {
  const d = (k: keyof EgressCounters): number => Math.max(0, now[k] - (before?.[k] ?? 0));
  if (EGRESS_FIELDS.every((k) => d(k) === 0) && d("bytesIn") === 0 && d("bytesOut") === 0) return undefined;
  const out = Object.fromEntries(EGRESS_FIELDS.map((k) => [k, Math.min(d(k), U32)])) as Record<(typeof EGRESS_FIELDS)[number], number>;
  return { ...out, bytesInLog2: log2Bucket(d("bytesIn")), bytesOutLog2: log2Bucket(d("bytesOut")) };
}

/** The plugin id (no version) of `<id>@<version>`. */
export const pluginIdOf = (plugin: string): string => (plugin.lastIndexOf("@") > 0 ? plugin.slice(0, plugin.lastIndexOf("@")) : plugin);

export interface PluginStatsDeps {
  readonly journal?: StatsJournal;
  /** The broker's cumulative counters by plugin id (`Egress.stats`). */
  readonly egress?: () => ReadonlyMap<string, Readonly<EgressCounters>>;
  /** The server log, for a refused entry (never thrown). */
  readonly log?: (message: string) => void;
  readonly random?: () => number;
  readonly now?: () => number;
}

export function makePluginStats(d: PluginStatsDeps) {
  const random = d.random ?? Math.random;
  const now = d.now ?? Date.now;
  /** When this window began: `windowS` is its real length (the last one, flushed at close, is short). */
  let windowStart = now();
  let window = new Map<string, Plugin>();
  let egressBefore = new Map<string, Readonly<EgressCounters>>();
  let timer: ReturnType<typeof setInterval> | undefined;
  const enabled = (): boolean => d.journal?.enabled === true;

  /** `plugin`'s (an id, a version is dropped) counters of this window, while recording is on. */
  const of = (plugin: string): Plugin | undefined => {
    if (!enabled()) return undefined;
    const id = pluginIdOf(plugin);
    let p = window.get(id);
    if (p === undefined) {
      if (id === "" || window.size >= MAX_PLUGINS) return undefined;
      p = new Plugin();
      window.set(id, p);
    }
    return p;
  };

  /** This window's payloads (one per active plugin) and a fresh window. */
  const take = (): Array<[string, Payload]> => {
    const ended = window;
    window = new Map();
    const at = now();
    const windowS = Math.max(1, Math.round((at - windowStart) / 1000));
    windowStart = at;
    const egressNow = d.egress?.() ?? new Map<string, Readonly<EgressCounters>>();
    const before = egressBefore;
    egressBefore = new Map(egressNow);
    if (!enabled()) return [];
    const ids = new Set([...ended.keys(), ...egressNow.keys()]);
    const out: Array<[string, Payload]> = [];
    for (const id of ids) {
      const p = ended.get(id);
      const now = egressNow.get(id);
      const egress = now === undefined ? undefined : egressDelta(now, before.get(id));
      if (p === undefined && egress === undefined) continue;
      out.push([
        id,
        {
          windowS,
          families: [...(p?.families ?? new Map<string, Family>())].map(([name, f]) => f.payload(name)),
          timeouts: Math.min(p?.timeouts ?? 0, U32),
          crashes: Math.min(p?.crashes ?? 0, U32),
          logDropped: Math.min(p?.logDropped ?? 0, U32),
          stderrDropped: Math.min(p?.stderrDropped ?? 0, U32),
          ...(egress === undefined ? {} : { egress }),
        },
      ]);
    }
    return out;
  };

  /** Records this window's `plugin.stats` and starts the next. */
  const flush = (): number => {
    const entries = take();
    for (const [id, payload] of entries) {
      try {
        d.journal?.recordPlugin("plugin.stats", 1, id, JSON.stringify(payload));
      } catch (e) {
        d.log?.(`plugin stats: the journal refused a plugin.stats: ${(e as Error).message}`);
      }
    }
    return entries.length;
  };

  return {
    /** Whether calls are counted now (the journal records). */
    counting: enabled,
    /** One host call of `plugin` (`<id>@<version>`): `code` when it failed; params and reply bytes. */
    call(plugin: string, method: string, micros: number, code: string | undefined, bytesIn: number, bytesOut: number): void {
      const f = of(plugin)?.family(methodFamily(method));
      if (f === undefined) return;
      f.calls++;
      f.time(Math.max(0, micros), random);
      f.bytesIn += bytesIn;
      f.bytesOut += bytesOut;
      if (code !== undefined) {
        f.errors.set(code, (f.errors.get(code) ?? 0) + 1);
        if (REACH_CODES.has(code)) f.reachDenials++;
      }
    },
    /** A job of `plugin` ended with `code` (`PLUGIN_TIMEOUT` and `PLUGIN_CRASHED` count). */
    jobEnded(plugin: string, code: string | undefined): void {
      if (code !== "PLUGIN_TIMEOUT" && code !== "PLUGIN_CRASHED") return;
      const p = of(plugin);
      if (p === undefined) return;
      if (code === "PLUGIN_TIMEOUT") p.timeouts++;
      else p.crashes++;
    },
    /** A `log` call (`log`) or a stderr line (`stderr`) of `plugin` was dropped by its limit. */
    dropped(plugin: string, what: "log" | "stderr"): void {
      const p = of(plugin);
      if (p === undefined) return;
      if (what === "log") p.logDropped++;
      else p.stderrDropped++;
    },
    flush,
    /** The payloads `flush` would record now, starting the next window (tests). */
    take,
    /** Flushes every `STATS_WINDOW_MS` (unref'd) until `stop`. */
    start(): void {
      if (timer !== undefined) return;
      timer = setInterval(flush, STATS_WINDOW_MS);
      timer.unref?.();
    },
    stop(): void {
      if (timer !== undefined) clearInterval(timer);
      timer = undefined;
    },
  };
}

export type PluginStatsRecorder = ReturnType<typeof makePluginStats>;
