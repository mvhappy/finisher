// The server's diagnostics journal (plan D, Task D4; spec §21.1): fin-node's `Journal` for this
// server data, opened after `settings.json`'s `privacy` was read and kept in step with it;
// sessions; uncaught errors as `error.panic`; RPC timings; and the `diagnostics.*` calls with
// their trust rules: the source and the actor come from the connection's principal (5A
// Decision 27, spec §3.4), never from the request.
import { arch, availableParallelism, hostname, platform, release, totalmem } from "node:os";
import { relative } from "node:path";
import { fileURLToPath } from "node:url";
import type { ConsentState, DiagnosticsReport, ExportPage, PurgeReport, Source, WriteGuard } from "@finisher/core-types";
import { type FinError, type Principal, PROTOCOL_VERSION } from "@finisher/contracts";
import { Journal, kindNames, validatePrivacy } from "@finisher/fin-node";
import { Context, Effect, Layer, type Scope } from "effect";
import { consentUiOnly } from "./chain/fixes.ts";
import { privacyArg, readPrivacy, SERVER_VERSION } from "./config.ts";
import { attemptJson, finError, toFinError } from "./errors.ts";
import { mergeSettings } from "./settings-write.ts";

export type { Source } from "@finisher/core-types";

/**
 * The source of a principal's events (spec §3.4): `local` and `agent` are `fin`, `mcp` the MCP
 * adapter, `console` a person at the server, `device` the UI. Nothing else may send any.
 */
export function sourceOf(p: Principal): Source | undefined {
  switch (p.kind) {
    case "local":
    case "agent":
      return "fin";
    case "mcp":
      return "mcp";
    case "console":
      return "console";
    case "device":
      return "ui";
    default:
      return undefined;
  }
}

/** The kinds `diagnostics.record` accepts per source (Decisions 24 and 27). */
export const RECORD_KINDS: Readonly<Partial<Record<Source, ReadonlyArray<string>>>> = {
  fin: ["cli.command", "error.panic"],
  mcp: ["cli.command", "error.panic"],
  console: [],
  ui: [],
};

/**
 * 50 events/s, burst 200, per principal (shared by its connections), and for `local` per
 * connection: every `fin` is `local`, so one looping client must not silence the others
 * (review of Task 7, M1). `local` as a whole has `LOCAL_RECORD_RATE` above that.
 */
export const RECORD_RATE = { perSecond: 50, burst: 200 } as const;
/** Every `local` connection together. */
export const LOCAL_RECORD_RATE = { perSecond: 200, burst: 800 } as const;
/** Principals with a rate-limit bucket at once; past it the least recently created goes. */
export const MAX_BUCKETS = 1024;
/** A `diagnostics.record` payload over this many bytes is refused `EVENT_TOO_LARGE`. */
export const MAX_RECORD_BYTES = 64 * 1024;
/** `MAX_RAW_ID` of fin-core: a longer id from a header is dropped. */
export const MAX_HEADER_ID = 256;

/** The actor kinds `local` may claim from its headers; any other becomes `unknown`. */
const LOCAL_ACTOR_KINDS: ReadonlySet<string> = new Set(["user", "agent", "unknown"]);

/** The actor of a request (Decision 23a), and the chain's write guard for a write. */
export interface CallMeta {
  readonly actor: { readonly kind: string; readonly session?: string; readonly call?: string };
  /** The chain's write guard (5A Task 5), checked by the store inside the write transaction. */
  readonly guard?: WriteGuard;
}

/**
 * Effect RPC hands a handler the request's headers as a record of lower-case names (pairs are
 * accepted too). An empty or over-long value is dropped.
 */
function header(headers: unknown, name: string): string | undefined {
  let v: unknown;
  if (Array.isArray(headers)) {
    const hit = headers.find((h) => Array.isArray(h) && String(h[0]).toLowerCase() === name);
    v = hit?.[1];
  } else if (headers !== null && typeof headers === "object") {
    v = (headers as Record<string, unknown>)[name];
  }
  return typeof v === "string" && v.length > 0 && Buffer.byteLength(v) <= MAX_HEADER_ID ? v : undefined;
}

/**
 * `actorFor(principal, headers)` (spec §3.4): the principal bounds the actor, D6's
 * `fin-actor-*` headers only narrow it.
 * - `agent`/`mcp`: always `{kind: agent, session: <credential id>}`; only `call` is read;
 * - `device`: `{kind: user, session: <device id>}`; `console`: `{kind: user}`;
 * - `local`: D6's unverified header actor (`user`, `agent` or `unknown`; a session; a call);
 * - anything else: `unknown`.
 * The ids stay raw: the journal keys every one that is not a ULID.
 */
export function actorFor(p: Principal, headers: unknown): CallMeta {
  const call = header(headers, "fin-actor-call");
  const withCall = call === undefined ? {} : { call };
  switch (p.kind) {
    case "agent":
    case "mcp":
      return { actor: { kind: "agent", session: p.id, ...withCall } };
    case "device":
      return { actor: { kind: "user", session: p.id } };
    case "console":
      return { actor: { kind: "user" } };
    case "local": {
      const kind = header(headers, "fin-actor-kind");
      const session = header(headers, "fin-actor-session");
      return {
        actor: {
          kind: kind !== undefined && LOCAL_ACTOR_KINDS.has(kind) ? kind : "unknown",
          ...(session === undefined ? {} : { session }),
          ...withCall,
        },
      };
    }
    default:
      return { actor: { kind: "unknown" } };
  }
}

/**
 * The rate-limit key of a principal's call: `local:<connection>` (per connection), `console`,
 * `<kind>:<id>` for the others.
 */
export function principalKey(p: Principal, connection: string): string {
  return p.kind === "local" ? `local:${connection}` : p.kind === "console" ? p.kind : `${p.kind}:${p.id}`;
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
  /** Whether a token is there now, taking none. */
  peek(): boolean {
    this.#refill();
    return this.#tokens >= 1;
  }
  take(): boolean {
    this.#refill();
    if (this.#tokens < 1) return false;
    this.#tokens -= 1;
    return true;
  }
  #refill(): void {
    const t = this.now();
    this.#tokens = Math.min(this.burst, this.#tokens + ((t - this.#at) / 1000) * this.perSecond);
    this.#at = t;
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

/** What `record` needs to know of the request's connection. */
export interface RecordFrom {
  readonly principal: Principal;
  /** The connection, unique across transports (`<transport>:<client id>`): one run id each. */
  readonly connection: string;
}

export interface DiagnosticsShape {
  readonly journal: Journal;
  /** Whether any purpose is on (`finisherd.json` `diagnostics.enabled`). */
  readonly enabled: () => boolean;
  /** Called with the new value each time recording turns on or off. */
  readonly onEnabledChange: (listener: (enabled: boolean) => void) => void;
  /** Times `self` into `rpc.stats` under `tag` (never a stream RPC). */
  readonly timed: <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => Effect.Effect<A, FinError, R>;
  readonly record: (from: RecordFrom, req: RecordRequest, headers: unknown) => Effect.Effect<{ recorded: boolean }, FinError>;
  /** The chain already refused what `principal` may not change; training on is re-checked here. */
  readonly consent: (principal: Principal, purpose: "diagnostics" | "training", value: "on" | "off" | "unset") => Effect.Effect<ConsentState, FinError>;
  readonly status: () => Effect.Effect<DiagnosticsReport, FinError>;
  /** `redact`: content fields removed (a caller that is not a person, Decision 22). */
  readonly export: (req: ExportRequest, redact: boolean) => Effect.Effect<ExportPage, FinError>;
  readonly purge: (selector: unknown) => Effect.Effect<PurgeReport, FinError>;
  /** Events dropped by the per-principal rate limit since start. */
  readonly rateLimited: () => number;
}

export class Diagnostics extends Context.Service<Diagnostics, DiagnosticsShape>()("finisherd/Diagnostics") {}

export interface DiagnosticsOptions {
  /** How often `settings.json` is looked at (default 1 s). */
  readonly settingsPollMs?: number;
  /** The rate limit's clock (ms). */
  readonly now?: () => number;
}

/** How long startup waits for the writer to confirm `session.start` (then it goes on). */
const START_ACK_MS = 100;
/** The `privacy` argument for a settings file that is unusable (here: vanished for one poll). */
const UNUSABLE = privacyArg({ error: "settings.json vanished" });
const NONE: unique symbol = Symbol("none");
/** Every kind of fin-core's registry: an unknown one is `EVENT_KIND_UNKNOWN`. */
const KNOWN_KINDS: ReadonlySet<string> = new Set(kindNames());

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
    const first = readPrivacy(serverData);
    const journal = Journal.open(serverData, privacyArg(first), hostname(), testOptions());
    const warn = (w: string | undefined) => {
      if (w !== undefined) process.stderr.write(`finisherd: warning: settings.json privacy: ${w}\n`);
    };
    warn(journal.openWarning);
    // Acknowledged by the writer (bounded) before the server reports ready, so a kill right
    // after ready cannot lose `session.start` and `openSession`; a slow disk only delays it.
    if (!journal.startSession(JSON.stringify(sessionStart()), START_ACK_MS)) {
      process.stderr.write("finisherd: warning: the diagnostics journal did not confirm the session start in time\n");
    }

    const listeners: Array<(enabled: boolean) => void> = [];
    let enabled = journal.enabled;
    const changed = () => {
      if (journal.enabled === enabled) return;
      enabled = journal.enabled;
      for (const f of listeners) f(enabled);
    };
    // The settings watch (polled). Two rules keep an opt-out from flipping on by accident:
    // - a file that vanishes is unusable for one poll (an editor saving by rename leaves a gap,
    //   and a missing file would read as the defaults: diagnostics on);
    // - while recording is off, a change applies only once two consecutive polls agree.
    let applied = privacyArg(first);
    let hadFile = first.missing !== true;
    let candidate: string | null | typeof NONE = NONE;
    const poll = () => {
      const read = readPrivacy(serverData);
      const arg = read.missing === true && hadFile ? UNUSABLE : privacyArg(read);
      hadFile = read.missing !== true;
      if (arg === applied) {
        candidate = NONE;
        return;
      }
      if (!journal.enabled && arg !== candidate) {
        candidate = arg;
        return;
      }
      candidate = NONE;
      applied = arg;
      warn(journal.applySettings(arg));
      changed();
    };
    yield* Effect.acquireRelease(
      Effect.sync(() => {
        const timer = setInterval(poll, options.settingsPollMs ?? 1000);
        timer.unref();
        return timer;
      }),
      (timer) => Effect.sync(() => clearInterval(timer)),
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
    // One bucket per key (`principalKey`), the newest MAX_BUCKETS kept; `local` as a whole has
    // its own, kept apart from the per-key map so that it is never evicted.
    const buckets = new Map<string, TokenBucket>();
    const localBucket = new TokenBucket(LOCAL_RECORD_RATE.perSecond, LOCAL_RECORD_RATE.burst, now);
    const bucketOf = (key: string) => {
      let b = buckets.get(key);
      if (b === undefined) {
        b = new TokenBucket(RECORD_RATE.perSecond, RECORD_RATE.burst, now);
        buckets.set(key, b);
        if (buckets.size > MAX_BUCKETS) buckets.delete(buckets.keys().next().value!);
      }
      return b;
    };
    /** One event from the call's buckets (`local`: the whole first, so a refusal costs the connection nothing). */
    const admit = (p: Principal, connection: string) =>
      p.kind === "local" ? localBucket.peek() && bucketOf(principalKey(p, connection)).take() && localBucket.take() : bucketOf(principalKey(p, connection)).take();
    let rateLimited = 0;
    // One run id per connection (`fin` makes one connection per run), the newest 1024 kept.
    const sessions = new Map<string, string>();
    const sessionOf = (connection: string) => {
      let s = sessions.get(connection);
      if (s === undefined) {
        s = ulid();
        sessions.set(connection, s);
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
      record: ({ principal, connection }, req, headers) =>
        Effect.gen(function* () {
          const source = sourceOf(principal);
          if (!KNOWN_KINDS.has(req.kind)) {
            return yield* Effect.fail(finError("EVENT_KIND_UNKNOWN", `${req.kind} is not a known event kind`, { kind: req.kind }));
          }
          if (source === undefined || !(RECORD_KINDS[source] ?? []).includes(req.kind)) {
            return yield* Effect.fail(
              finError("EVENT_INVALID", `${req.kind} is not accepted from ${source ?? principal.kind}`, { kind: req.kind, source: source ?? principal.kind }),
            );
          }
          if (!journal.enabled) return { recorded: false };
          if (!admit(principal, connection)) {
            rateLimited += 1;
            return { recorded: false };
          }
          const payload = JSON.stringify(req.payload ?? null);
          if (Buffer.byteLength(payload) > MAX_RECORD_BYTES) {
            return yield* Effect.fail(
              finError("EVENT_TOO_LARGE", `the ${req.kind} payload is over ${MAX_RECORD_BYTES} bytes`, { kind: req.kind }),
            );
          }
          const ctx = { ...(req.ctx ?? {}), actor: actorFor(principal, headers).actor };
          yield* sync(() => journal.record(req.kind, req.kv, JSON.stringify(ctx), payload, source, sessionOf(connection)));
          return { recorded: true };
        }),
      consent: (principal, purpose, value) =>
        Effect.gen(function* () {
          // The chain refuses it first (`deviceOnly`); kept here so no other path can.
          if (purpose === "training" && value === "on" && principal.kind !== "device") {
            return yield* Effect.fail(consentUiOnly("diagnostics.consent", principal));
          }
          // The merged section is checked with the journal's own rules before it is written:
          // a change stored beside a key the journal refuses would answer success and not apply.
          const privacy = yield* sync(() =>
            mergeSettings(serverData, { [purpose]: value === "unset" ? null : value }, (p) => validatePrivacy(JSON.stringify(p ?? null))),
          );
          const arg = privacy === undefined ? null : JSON.stringify(privacy);
          applied = arg;
          hadFile = true;
          candidate = NONE;
          const refused = journal.applySettings(arg);
          changed();
          if (refused !== undefined) {
            return yield* Effect.fail(
              finError("SETTINGS_INVALID", `the privacy settings were written but not applied: ${refused}`, { reason: refused }, "fix privacy in <server-data>/settings.json by hand, then retry"),
            );
          }
          return JSON.parse(journal.consent()) as ConsentState;
        }),
      status: () => attemptJson<DiagnosticsReport>(() => journal.status()),
      export: (req, redact) =>
        attemptJson<ExportPage>(() =>
          journal.export(
            JSON.stringify(req.filter ?? {}),
            req.cursor,
            Math.min(Math.max(Math.trunc(req.limit ?? 1000), 1), 1000),
            req.asUpload ?? null,
            redact,
          ),
        ),
      purge: (selector) => attemptJson<PurgeReport>(() => journal.purge(JSON.stringify(selector))),
      rateLimited: () => rateLimited,
    } satisfies DiagnosticsShape;
  });
}

export function layerDiagnostics(serverData: string, options?: DiagnosticsOptions): Layer.Layer<Diagnostics> {
  return Layer.effect(Diagnostics, makeDiagnostics(serverData, options));
}
