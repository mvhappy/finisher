// The server's diagnostics journal (plan D, Task D4; spec §21.1): fin-node's `Journal` for this
// server data, opened after `settings.json`'s `privacy` was read and kept in step with it;
// sessions; uncaught errors as `error.panic`; RPC timings; and the `diagnostics.*` calls with
// their trust rules (the source comes from the transport, never from the request).
import { unwatchFile, watchFile } from "node:fs";
import { arch, availableParallelism, hostname, platform, release, totalmem } from "node:os";
import { relative } from "node:path";
import { fileURLToPath } from "node:url";
import type { ConsentState, DiagnosticsReport, ExportPage, PurgeReport } from "@finisher/core-types";
import { type FinError, PROTOCOL_VERSION } from "@finisher/contracts";
import { Journal } from "@finisher/fin-node";
import { Context, Effect, Layer, type Scope } from "effect";
import { privacyArg, readPrivacy, SERVER_VERSION, settingsPath } from "./config.ts";
import { attemptJson, finError, toFinError } from "./errors.ts";
import { mergeSettings } from "./settings-write.ts";

/** Who is on the other end of a connection: the local socket is `fin`, the WebSocket the UI. */
export type Source = "fin" | "ui";

/** The transport's source, provided to each transport's handlers (`main.ts`). */
export class RpcSource extends Context.Service<RpcSource, Source>()("finisherd/RpcSource") {}

/** The kinds `diagnostics.record` accepts per source (Decision 24). */
export const RECORD_KINDS: Readonly<Record<Source, ReadonlyArray<string>>> = {
  fin: ["cli.command", "error.panic"],
  ui: [],
};

/** 50 events/s per source, burst 200, shared by every connection of the source. */
export const RECORD_RATE = { perSecond: 50, burst: 200 } as const;
/** A `diagnostics.record` payload over this many bytes is refused `EVENT_TOO_LARGE`. */
export const MAX_RECORD_BYTES = 64 * 1024;
/** `MAX_RAW_ID` of fin-core: a longer id from a header is dropped. */
export const MAX_HEADER_ID = 256;

const ACTOR_KINDS = new Set(["user", "agent", "plugin", "system", "unknown"]);

/** The actor of a request (Decision 23a), from its `fin-actor-*` headers, validated. */
export interface CallMeta {
  readonly actor: { readonly kind: string; readonly session?: string; readonly call?: string };
}

/**
 * `callMeta(headers)`: Effect RPC hands a handler the request's headers as a record of lower-case
 * names (pairs are accepted too). An unknown or missing kind is `unknown`; an empty or over-long
 * id is dropped. The ids stay raw: the journal keys every one that is not a ULID.
 */
export function callMeta(headers: unknown): CallMeta {
  const get = (name: string): string | undefined => {
    if (Array.isArray(headers)) {
      const hit = headers.find((h) => Array.isArray(h) && String(h[0]).toLowerCase() === name);
      return typeof hit?.[1] === "string" ? hit[1] : undefined;
    }
    const v = headers !== null && typeof headers === "object" ? (headers as Record<string, unknown>)[name] : undefined;
    return typeof v === "string" ? v : undefined;
  };
  const id = (v: string | undefined) => (v !== undefined && v.length > 0 && Buffer.byteLength(v) <= MAX_HEADER_ID ? v : undefined);
  const kind = get("fin-actor-kind");
  const session = id(get("fin-actor-session"));
  const call = id(get("fin-actor-call"));
  return {
    actor: {
      kind: kind !== undefined && ACTOR_KINDS.has(kind) ? kind : "unknown",
      ...(session === undefined ? {} : { session }),
      ...(call === undefined ? {} : { call }),
    },
  };
}

/** A token bucket: `take()` is false once the burst is spent until the rate refills it. */
export class TokenBucket {
  readonly perSecond: number;
  readonly burst: number;
  readonly now: () => number;
  #tokens: number;
  #at: number;
  constructor(perSecond: number, burst: number, now: () => number = () => performance.now()) {
    this.perSecond = perSecond;
    this.burst = burst;
    this.now = now;
    this.#tokens = burst;
    this.#at = now();
  }
  take(): boolean {
    const t = this.now();
    this.#tokens = Math.min(this.burst, this.#tokens + ((t - this.#at) / 1000) * this.perSecond);
    this.#at = t;
    if (this.#tokens < 1) return false;
    this.#tokens -= 1;
    return true;
  }
}

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** A ULID (canonical upper case): a connection's run, for the entries it sends. */
export function ulid(now = Date.now()): string {
  let time = "";
  for (let t = now, i = 0; i < 10; i++, t = Math.floor(t / 32)) time = CROCKFORD[t % 32] + time;
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  let rand = "";
  for (let i = 0; i < 16; i++) rand += CROCKFORD[bytes[i]! % 32];
  return time + rand;
}

/** The platform facts of `session.start` (Decision 5): no host, user, IP or hardware model. */
export function sessionStart(): Record<string, unknown> {
  const os = ({ linux: "linux", darwin: "macos", win32: "windows" } as Record<string, string>)[platform()] ?? "other";
  const kernel = Number.parseInt(release(), 10) || 0;
  // Darwin 20 is macOS 11; Windows and Linux report their own major.
  const osMajor = os === "macos" && kernel >= 20 ? kernel - 9 : kernel;
  const gib = totalmem() / 2 ** 30;
  return {
    app: SERVER_VERSION,
    protocol: PROTOCOL_VERSION,
    os,
    osMajor,
    arch: ({ x64: "x86_64", arm64: "aarch64" } as Record<string, string>)[arch()] ?? "other",
    cores: availableParallelism(),
    memGiB: 2 ** Math.max(0, Math.round(Math.log2(Math.max(gib, 1)))),
    profile: "release",
  };
}

const ROOT = fileURLToPath(new URL("../../../", import.meta.url));

/**
 * The first stack frame of ours, repo-relative (`apps/finisherd/src/x.ts:12`), or undefined when
 * none is (a dependency, Node internals): Decision 26's `location`.
 */
export function locationOf(error: unknown, root = ROOT): string | undefined {
  const stack = (error as { stack?: unknown } | null)?.stack;
  if (typeof stack !== "string") return undefined;
  for (const line of stack.split("\n").slice(1)) {
    const m = /\(?((?:file:\/\/)?[^()\s]+):(\d+):\d+\)?\s*$/.exec(line.trim());
    if (!m) continue;
    let file = m[1]!;
    if (file.startsWith("file://")) {
      try {
        file = fileURLToPath(file);
      } catch {
        continue;
      }
    }
    const rel = relative(root, file).replaceAll("\\", "/");
    if (rel.startsWith("..") || rel.includes("node_modules")) continue;
    if (/^(crates|apps)\/[a-z0-9_./-]{1,160}$/.test(rel)) return `${rel}:${m[2]}`;
  }
  return undefined;
}

/** The `error.panic` wire form of an uncaught error (the journal scrubs and hashes the message). */
export function panicPayload(error: unknown): Record<string, unknown> {
  const location = locationOf(error);
  const message = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
  return {
    where: "finisherd",
    ...(location === undefined ? {} : { location }),
    thirdParty: location === undefined,
    thread: "main",
    message,
  };
}

/**
 * Records uncaught exceptions and unhandled rejections as `error.panic`, synchronously (the
 * process is about to exit): `uncaughtExceptionMonitor` observes without changing Node's
 * behaviour, and an unhandled rejection reaches it as an uncaught exception. Returns the remover.
 */
export function installUncaughtHandlers(journal: Journal): () => void {
  const onUncaught = (error: unknown) => {
    try {
      journal.recordSync("error.panic", 1, null, JSON.stringify(panicPayload(error)), "finisherd", 250);
    } catch {
      // Recording must never stand in the way of the crash being reported as usual.
    }
  };
  process.on("uncaughtExceptionMonitor", onUncaught);
  return () => process.off("uncaughtExceptionMonitor", onUncaught);
}

export interface RecordRequest {
  readonly kind: string;
  readonly kv: number;
  readonly ctx?: { readonly project?: string | undefined; readonly branch?: string | undefined; readonly txId?: string | undefined } | undefined;
  readonly payload: unknown;
}

export interface ExportRequest {
  readonly filter?: unknown;
  readonly cursor?: string | undefined;
  readonly limit?: number | undefined;
  readonly asUpload?: "diagnostics" | "training" | undefined;
}

export interface DiagnosticsShape {
  readonly journal: Journal;
  /** Whether any purpose is on (`finisherd.json` `diagnostics.enabled`). */
  readonly enabled: () => boolean;
  /** Called with the new value each time recording turns on or off. */
  readonly onEnabledChange: (listener: (enabled: boolean) => void) => void;
  /** Times `self` into `rpc.stats` under `tag` (never a stream RPC). */
  readonly timed: <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => Effect.Effect<A, FinError, R>;
  readonly record: (source: Source, client: number, req: RecordRequest, headers: unknown) => Effect.Effect<{ recorded: boolean }, FinError>;
  readonly consent: (source: Source, purpose: "diagnostics" | "training", value: "on" | "off" | "unset") => Effect.Effect<ConsentState, FinError>;
  readonly status: () => Effect.Effect<DiagnosticsReport, FinError>;
  readonly export: (req: ExportRequest) => Effect.Effect<ExportPage, FinError>;
  readonly purge: (selector: unknown) => Effect.Effect<PurgeReport, FinError>;
  /** Events dropped by the per-source rate limit since start. */
  readonly rateLimited: () => number;
}

export class Diagnostics extends Context.Service<Diagnostics, DiagnosticsShape>()("finisherd/Diagnostics") {}

export interface DiagnosticsOptions {
  /** How often `settings.json` is looked at (default 1 s). */
  readonly settingsPollMs?: number;
  /** The rate limit's clock (ms). */
  readonly now?: () => number;
}

const TRAINING_WAYS_IN =
  "a person opts in: in the Finisher app, by setting privacy.training to \"on\" in <server-data>/settings.json, or by starting finisherd with FIN_TRAINING=on";

/** Test-only journal options from the environment (honoured by `test-hooks` builds of fin-node only). */
function testOptions(): { statsEveryMs?: number } | undefined {
  const ms = Number(process.env.FIN_TEST_JOURNAL_STATS_MS);
  return Number.isFinite(ms) && ms > 0 ? { statsEveryMs: ms } : undefined;
}

/**
 * Opens the journal under the `privacy` settings read first (nothing is written under the build
 * default when the user said off), starts the session, watches `settings.json` (last good kept)
 * and records uncaught errors. Closing ends the session and the journal.
 */
export function makeDiagnostics(serverData: string, options: DiagnosticsOptions = {}): Effect.Effect<DiagnosticsShape, never, Scope.Scope> {
  return Effect.gen(function* () {
    const started = performance.now();
    const journal = Journal.open(serverData, privacyArg(readPrivacy(serverData)), hostname(), testOptions());
    const warn = (w: string | undefined) => {
      if (w !== undefined) process.stderr.write(`finisherd: warning: settings.json privacy: ${w}\n`);
    };
    warn(journal.openWarning);
    journal.startSession(JSON.stringify(sessionStart()));

    const listeners: Array<(enabled: boolean) => void> = [];
    let enabled = journal.enabled;
    const changed = () => {
      if (journal.enabled === enabled) return;
      enabled = journal.enabled;
      for (const f of listeners) f(enabled);
    };
    const apply = () => {
      warn(journal.applySettings(privacyArg(readPrivacy(serverData))));
      changed();
    };
    const path = settingsPath(serverData);
    yield* Effect.acquireRelease(
      Effect.sync(() => watchFile(path, { interval: options.settingsPollMs ?? 1000, persistent: false }, apply)),
      () => Effect.sync(() => unwatchFile(path, apply)),
    );
    yield* Effect.acquireRelease(
      Effect.sync(() => installUncaughtHandlers(journal)),
      (remove) =>
        Effect.promise(async () => {
          remove();
          await journal.endSession((performance.now() - started) / 1000).catch(() => false);
          await journal.close().catch(() => undefined);
        }),
    );

    const now = options.now ?? (() => performance.now());
    const buckets: Record<Source, TokenBucket> = {
      fin: new TokenBucket(RECORD_RATE.perSecond, RECORD_RATE.burst, now),
      ui: new TokenBucket(RECORD_RATE.perSecond, RECORD_RATE.burst, now),
    };
    let rateLimited = 0;
    // One run id per connection (`fin` makes one connection per run), the newest 1024 kept.
    const sessions = new Map<number, string>();
    const sessionOf = (client: number) => {
      let s = sessions.get(client);
      if (s === undefined) {
        s = ulid();
        sessions.set(client, s);
        if (sessions.size > 1024) sessions.delete(sessions.keys().next().value!);
      }
      return s;
    };
    const sync = <A>(f: () => A) => Effect.try({ try: f, catch: toFinError });

    return {
      journal,
      enabled: () => journal.enabled,
      onEnabledChange: (f) => void listeners.push(f),
      timed: (tag, self) =>
        Effect.suspend(() => {
          const t0 = performance.now();
          const observe = (code?: string) => {
            if (journal.enabled) journal.observe("rpc", tag, Math.round((performance.now() - t0) * 1000), code);
          };
          return self.pipe(
            Effect.tap(() => Effect.sync(() => observe())),
            Effect.tapError((e) => Effect.sync(() => observe(e.code))),
          );
        }),
      record: (source, client, req, headers) =>
        Effect.gen(function* () {
          if (!RECORD_KINDS[source].includes(req.kind)) {
            return yield* Effect.fail(
              finError("EVENT_INVALID", `${req.kind} is not accepted from ${source}`, { kind: req.kind, source }),
            );
          }
          if (!journal.enabled) return { recorded: false };
          if (!buckets[source].take()) {
            rateLimited += 1;
            return { recorded: false };
          }
          const payload = JSON.stringify(req.payload ?? null);
          if (Buffer.byteLength(payload) > MAX_RECORD_BYTES) {
            return yield* Effect.fail(
              finError("EVENT_TOO_LARGE", `the ${req.kind} payload is over ${MAX_RECORD_BYTES} bytes`, { kind: req.kind }),
            );
          }
          const ctx = { ...(req.ctx ?? {}), actor: callMeta(headers).actor };
          yield* sync(() => journal.record(req.kind, req.kv, JSON.stringify(ctx), payload, source, sessionOf(client)));
          return { recorded: true };
        }),
      consent: (source, purpose, value) =>
        Effect.gen(function* () {
          if (purpose === "training" && value === "on" && source !== "ui") {
            return yield* Effect.fail(
              finError("CONSENT_UI_ONLY", "turning training on is a person's choice, not a command's", { purpose, value, source }, TRAINING_WAYS_IN),
            );
          }
          const privacy = yield* sync(() => mergeSettings(serverData, { [purpose]: value === "unset" ? null : value }));
          warn(journal.applySettings(privacy === undefined ? null : JSON.stringify(privacy)));
          changed();
          return JSON.parse(journal.consent()) as ConsentState;
        }),
      status: () => attemptJson<DiagnosticsReport>(() => journal.status()),
      export: (req) =>
        attemptJson<ExportPage>(() =>
          journal.export(JSON.stringify(req.filter ?? {}), req.cursor, Math.min(Math.max(Math.trunc(req.limit ?? 1000), 1), 1000), req.asUpload),
        ),
      purge: (selector) => attemptJson<PurgeReport>(() => journal.purge(JSON.stringify(selector))),
      rateLimited: () => rateLimited,
    } satisfies DiagnosticsShape;
  });
}

export function layerDiagnostics(serverData: string, options?: DiagnosticsOptions): Layer.Layer<Diagnostics> {
  return Layer.effect(Diagnostics, makeDiagnostics(serverData, options));
}
