// The remote listener (5B Decisions 26–29, 34): off by default; when the owner enables it
// (`settings.write server.remote`, or `FIN_LISTEN` in Docker) a **TLS-only** listener serves the
// same RPC protocol (`wss`) and HTTP front as the loopback one, with its own Host and Origin
// lists, its own pre-upgrade pool and a per-address cap, sharing the WebSocket slots.
//
// - Certificates: `selfSigned` is made from the identity key (`selfSignedCert`) for every
//   route host and this machine's name, remade with the same key when the routes change, 30
//   days before it expires and after a rotation, so its pin changes only with the key; `files`
//   reads the owner's pair (checked: parses, key matches, valid now: `TLS_FILES_INVALID`) and
//   re-reads it when a file's mtime changes (every 60 s), keeping the old context and warning
//   when the new pair is bad.
// - It never runs while the environment is `moved` (Decision 31).
// - The `wss` transport is served once at start (`socketServer()`); each started listener
//   forwards its sockets to it, so starting, restarting and stopping need no new RPC layers.
// - Audited `listener.start {spki, mode, source}` / `listener.stop`.
import { readFileSync, statSync } from "node:fs";
import type { Server } from "node:https";
import { X509Certificate } from "node:crypto";
import { selfSignedCert } from "@finisher/fin-node";
import { Context, Effect, Exit, Fiber, Scope, Semaphore } from "effect";
import * as NetAddress from "effect/net/NetAddress";
import type * as Socket from "effect/socket/Socket";
import { SocketServer } from "effect/socket/SocketServer";
import { ServerStartError } from "../config.ts";
import type { Audit } from "../control/audit.ts";
import type { Environment } from "../env/descriptor.ts";
import { type FrontRefusal, makeFront, type Route } from "../http/front.ts";
import type { MediaRequestHandler } from "../media/http.ts";
import { type ListenerPolicy, loopbackHosts, MAX_UNAUTHENTICATED, normalOrigin, PER_ADDRESS } from "../http/policy.ts";
import type { Tickets } from "../auth/tickets.ts";
import type { NonceSource } from "../http/front.ts";
import type { ServerSecrets } from "../secrets/vault.ts";
import { checkTlsFiles } from "./tls-files.ts";
import { makeTicketedWebSocketServer, makeUpgradeCheck, type UpgradeResult, type WebSocketSlots } from "../upgrade.ts";
import type { Routes } from "./routes.ts";
import { isLoopbackHost, parseListen, type RemoteConfig } from "./server-settings.ts";

/** A self-signed certificate's life, and how long before its end it is remade. */
export const CERT_DAYS = 397;
const REMAKE_BEFORE_MS = 30 * 86_400_000;
const FILES_POLL_MS = 60_000;

export interface RemoteDeps {
  readonly slots: WebSocketSlots;
  readonly tickets: Tickets;
  readonly audit: Audit;
  readonly secrets: ServerSecrets;
  readonly routes: Routes;
  readonly environment: Environment;
  readonly nonces: NonceSource;
  readonly httpRoutes: Readonly<Record<string, Route>>;
  readonly maxPayload: number;
  /** `server.origins` (5A's, read as the file changes). */
  readonly origins: () => ReadonlyArray<string>;
  readonly onRefused: (r: FrontRefusal) => void;
  readonly onUpgradeRefused: (r: Extract<UpgradeResult, { ok: false }>) => void;
  /** Tests inject remote addresses (Decision 34). */
  readonly remoteOf?: ((s: never) => string) | undefined;
  /** The media route of this listener (6B-1 Task 17), built with the listener's own Host and Origin lists. */
  readonly media?: ((policy: ListenerPolicy) => MediaRequestHandler) | undefined;
}

interface Running {
  readonly config: RemoteConfig;
  readonly scope: Scope.Closeable;
  readonly inner: SocketServer["Service"];
  forwarded: boolean;
  readonly http: Server;
  readonly host: string;
  readonly port: number;
  spki: string;
  notAfter: number;
  warning?: Record<string, unknown>;
  /** Files mode: reads the pair again when a file changed. */
  reload?: () => void;
}

const startFailed = (code: string, message: string, details: Record<string, unknown> = {}, fix?: string) => new ServerStartError(code, message, details, fix);

/** The identity's self-signed certificate for the current routes, its pin and end. */
function selfSigned(secrets: ServerSecrets, routes: Routes, listenHost: string): { key: string; cert: string; spki: string; notAfter: number } {
  const id = secrets.identity;
  if (id === undefined) throw startFailed("SECRET_STORE_UNAVAILABLE", "the server's identity key is not available now", {}, "Unlock the system keychain, then restart the server.");
  const sans = [...routes.sans(), ...(listenHost === "0.0.0.0" || listenHost === "::" ? [] : [listenHost])];
  const cert = selfSignedCert(id.pem, sans, CERT_DAYS);
  return { key: id.pem, cert, spki: id.spki, notAfter: new X509Certificate(cert).validToDate.getTime() };
}

/** The owner's certificate files, checked. */
export function filesPair(certFile: string, keyFile: string): { key: string; cert: string; spki: string; notAfter: number; mtime: string } {
  let cert: string;
  let key: string;
  let mtime: string;
  try {
    cert = readFileSync(certFile, "utf8");
    key = readFileSync(keyFile, "utf8");
    mtime = `${statSync(certFile).mtimeMs}:${statSync(keyFile).mtimeMs}`;
  } catch {
    throw startFailed("TLS_FILES_INVALID", "the certificate files cannot be read", { reason: "unreadable" }, "Check the certificate and key file paths and their permissions.");
  }
  const report = checkTlsFiles(cert, key);
  if (!report.ok) {
    throw startFailed("TLS_FILES_INVALID", "the certificate files cannot be used", { reason: report.reason }, "Use a certificate and its own key that are valid now.");
  }
  return { key, cert, spki: report.spki!, notAfter: report.notAfter!, mtime };
}

export class RemoteListener {
  readonly #d: RemoteDeps;
  #running: Running | undefined;
  /** One start or stop at a time (two `settings.write` calls must not leave an untracked listener). */
  readonly #lock = Effect.runSync(Semaphore.make(1));
  #startWarning: Record<string, unknown> | undefined;
  /** Forwards an accepted socket to the `wss` RPC server (set once it runs). */
  #runner: ((inner: SocketServer["Service"]) => Fiber.Fiber<unknown, unknown>) | undefined;

  constructor(deps: RemoteDeps) {
    this.#d = deps;
  }

  /** The `wss` transport's socket server, served once (`transport.ts`). */
  socketServer(): SocketServer["Service"] {
    const self = this;
    const address = NetAddress.inetAddressFromIpString("0.0.0.0", 0);
    return SocketServer.of({
      address: address._tag === "Success" ? address.success : (undefined as never),
      run: <R, E, _>(handler: (socket: Socket.Socket) => Effect.Effect<_, E, R>) =>
        Effect.gen(function* () {
          const ctx = yield* Effect.context<R>();
          self.#runner = (inner) => Effect.runForkWith(ctx)(inner.run(handler) as Effect.Effect<never, unknown, R>);
          // A listener started before this transport served is forwarded now.
          self.#forward();
          return yield* Effect.never;
        }),
    });
  }

  /** The listener's Host and Origin lists (Decision 11), read at each request. */
  policy(port: () => number, listenHost: () => string, config: () => RemoteConfig | undefined): ListenerPolicy {
    const d = this.#d;
    const publicHost = () => {
      const u = config()?.publicUrl;
      return u === undefined ? [] : [new URL(u).host.toLowerCase()];
    };
    return {
      name: "remote",
      hosts: () => {
        const p = port();
        const routeHosts = d.routes.current().filter((r) => r.kind !== "loopback" && r.kind !== "public").map((r) => new URL(r.url).host.toLowerCase());
        const listen = listenHost();
        const own = listen === "0.0.0.0" || listen === "::" ? [] : [`${listen.includes(":") ? `[${listen}]` : listen}:${p}`];
        const extra = (config()?.hosts ?? []).map((h) => (/:\d+$/.test(h) ? h : `${h}:${p}`).toLowerCase());
        return [...new Set([...routeHosts, ...own, ...loopbackHosts(p), ...publicHost(), ...extra])];
      },
      origins: () => [...new Set([...this.originsOf(port()), ...d.origins().flatMap((o) => normalOrigin(o) ?? []), ...(config()?.origins ?? []).flatMap((o) => normalOrigin(o) ?? [])])],
      maxUnauthenticated: MAX_UNAUTHENTICATED,
      perAddress: PER_ADDRESS,
    };
  }

  /** The origins a device's proof `htu` may name on this listener (its routes, `publicUrl`, its own names). */
  originsOf(port: number): ReadonlyArray<string> {
    const r = this.#running;
    const routeOrigins = this.#d.routes
      .current()
      .filter((x) => x.kind !== "loopback")
      .map((x) => new URL(x.url).origin);
    const own = r === undefined || r.host === "0.0.0.0" || r.host === "::" ? [] : [`https://${r.host.includes(":") ? `[${r.host}]` : r.host}:${port}`];
    return [...new Set([...routeOrigins, ...own, ...loopbackHosts(port).map((h) => `https://${h}`)])];
  }

  /** Starts (or restarts) the listener with `config` (`LISTEN_NEEDS_TLS`, `TLS_FILES_INVALID`, `ENV_MOVED`). */
  start(config: RemoteConfig): Effect.Effect<void, ServerStartError> {
    return this.#lock.withPermits(1)(this.#start(config));
  }

  /** A start at server start that failed: `server.status` warns (the settings asked for it). */
  noteStartFailure(e: ServerStartError): void {
    this.#startWarning = { code: e.code, message: `the remote listener did not start: ${e.message}`, ...(e.fix === undefined ? {} : { fix: e.fix }) };
  }

  /** The config of the listener now, if it runs (`settings.write` restores it on a failure). */
  config(): RemoteConfig | undefined {
    return this.#running?.config;
  }

  #start(config: RemoteConfig): Effect.Effect<void, ServerStartError> {
    const self = this;
    return Effect.gen(function* () {
      yield* self.#stop();
      self.#startWarning = undefined;
      if (self.#d.environment.state() === "moved") {
        return yield* Effect.fail(startFailed("ENV_MOVED", "the remote listener stays off while this server data waits for its owner", {}, "Adopt or reset this server data first."));
      }
      const listen = parseListen(config.listen);
      if (typeof listen === "string") return yield* Effect.fail(startFailed("SETTINGS_INVALID", `server.remote.listen: ${listen}`, { key: "server.remote" }));
      const pair = yield* Effect.try({
        try: () => (config.tls.mode === "files" ? filesPair(config.tls.certFile, config.tls.keyFile) : selfSigned(self.#d.secrets, self.#d.routes, listen.host)),
        catch: (e) => (e instanceof ServerStartError ? e : startFailed("SERVER_START_FAILED", String(e))),
      });
      const scope = Scope.makeUnsafe();
      let port = 0;
      const policy = self.policy(() => port, () => listen.host, () => self.#running?.config ?? config);
      const check = makeUpgradeCheck({ policy, tickets: self.#d.tickets });
      const built = yield* Scope.provide(
        makeTicketedWebSocketServer({
          check,
          maxPayload: self.#d.maxPayload,
          slots: self.#d.slots,
          policy,
          host: listen.host,
          port: listen.port,
          tls: { key: pair.key, cert: pair.cert },
          onRequest: makeFront(self.#d.httpRoutes, policy, self.#d.nonces, { onRefused: self.#d.onRefused, ...(self.#d.remoteOf === undefined ? {} : { remoteOf: self.#d.remoteOf as never }) }),
          ...(self.#d.media === undefined ? {} : { media: self.#d.media(policy) }),
          onRefused: self.#d.onUpgradeRefused,
          ...(self.#d.remoteOf === undefined ? {} : { remoteOf: self.#d.remoteOf as never }),
        }),
        scope,
      ).pipe(
        Effect.mapError((e) => startFailed("SERVER_START_FAILED", "the remote listener cannot listen", { cause: String((e as { reason?: { cause?: unknown } }).reason?.cause ?? e) })),
        // A listen that failed closes what it opened.
        Effect.onError(() => Scope.close(scope, Exit.void)),
      );
      port = built.port;
      const running: Running = { config, scope, inner: built.server, forwarded: false, http: built.http as unknown as Server, host: listen.host, port, spki: pair.spki, notAfter: pair.notAfter };
      self.#running = running;
      self.#forward();
      self.#d.routes.reconfigure({ remote: { port, pin: config.tls.mode === "files" ? "webpki" : "identity" }, publicUrl: config.publicUrl });
      // Self-signed: remade for new routes, near its end, after a rotation; files: re-read on change.
      if (config.tls.mode === "selfSigned") {
        const remake = () => self.#remake();
        const offRoutes = self.#d.routes.onChange(remake);
        const offEnv = self.#d.environment.onChange(remake);
        const timer = setInterval(() => {
          if (Date.now() > running.notAfter - REMAKE_BEFORE_MS) remake();
        }, 3_600_000);
        timer.unref();
        yield* Scope.addFinalizer(scope, Effect.sync(() => (offRoutes(), offEnv(), clearInterval(timer))));
        // The routes exist only now that the listener runs: the certificate names them.
        remake();
      } else {
        const files = config.tls;
        let mtime = "mtime" in pair ? pair.mtime : "";
        running.reload = () => {
          try {
            const now = `${statSync(files.certFile).mtimeMs}:${statSync(files.keyFile).mtimeMs}`;
            if (now === mtime) return;
            mtime = now;
            const next = filesPair(files.certFile, files.keyFile);
            running.http.setSecureContext({ key: next.key, cert: next.cert });
            running.spki = next.spki;
            running.notAfter = next.notAfter;
            delete running.warning;
          } catch (e) {
            // The old context stays; `server.status` says why.
            running.warning = { code: "TLS_FILES_INVALID", reason: (e as ServerStartError).details?.reason ?? "unreadable", message: "the new certificate files cannot be used; the listener keeps the old ones" };
          }
        };
        const timer = setInterval(() => {
          running.reload?.();
          // An owner's certificate that ends while it runs (an unrenewed `tailscale cert`) warns.
          if (running.warning === undefined && Date.now() > running.notAfter) {
            running.warning = { code: "TLS_FILES_INVALID", reason: "expired", message: "the certificate files expired; renew them" };
          }
        }, FILES_POLL_MS);
        timer.unref();
        yield* Scope.addFinalizer(scope, Effect.sync(() => clearInterval(timer)));
      }
      self.#d.audit.append({ kind: "listener.start", principal: { kind: "system", id: "finisherd" }, outcome: "ok", details: { spki: pair.spki, mode: config.tls.mode, source: config.source } });
    });
  }

  /** Hands the running listener's sockets to the `wss` RPC server, once both exist. */
  #forward(): void {
    const r = this.#running;
    if (r === undefined || r.forwarded || this.#runner === undefined) return;
    r.forwarded = true;
    const fiber = this.#runner(r.inner);
    Effect.runSync(Scope.addFinalizer(r.scope, Fiber.interrupt(fiber)));
  }

  /** A new self-signed certificate from the current identity key for the current routes. */
  #remake(): void {
    const r = this.#running;
    if (r === undefined || r.config.tls.mode !== "selfSigned") return;
    try {
      const pair = selfSigned(this.#d.secrets, this.#d.routes, r.host);
      r.http.setSecureContext({ key: pair.key, cert: pair.cert });
      r.spki = pair.spki;
      r.notAfter = pair.notAfter;
    } catch (e) {
      process.stderr.write(`finisherd: warning: cannot remake the remote listener's certificate: ${String((e as Error).message)}\n`);
    }
  }

  /** Files mode: re-reads the owner's pair now (also every 60 s); self-signed: remakes the certificate. */
  reload(): void {
    const r = this.#running;
    if (r === undefined) return;
    if (r.reload !== undefined) r.reload();
    else this.#remake();
  }

  stop(): Effect.Effect<void> {
    return this.#lock.withPermits(1)(this.#stop());
  }

  #stop(): Effect.Effect<void> {
    const r = this.#running;
    if (r === undefined) return Effect.void;
    this.#running = undefined;
    this.#d.routes.reconfigure({ remote: undefined, publicUrl: undefined });
    return Effect.andThen(Scope.close(r.scope, Exit.void), () =>
      Effect.sync(() => this.#d.audit.append({ kind: "listener.stop", principal: { kind: "system", id: "finisherd" }, outcome: "ok", details: {} })),
    );
  }

  /** The listener for `server.status`: its endpoint, pin, mode and source; none when off. */
  info(): { endpoint: string; spki: string; mode: "selfSigned" | "files"; source: RemoteConfig["source"]; port: number } | undefined {
    const r = this.#running;
    if (r === undefined) return undefined;
    const host = r.host.includes(":") ? `[${r.host}]` : r.host;
    return { endpoint: `wss://${host}:${r.port}`, spki: r.spki, mode: r.config.tls.mode, source: r.config.source, port: r.port };
  }

  warnings(): ReadonlyArray<Record<string, unknown>> {
    return [...(this.#running?.warning === undefined ? [] : [this.#running.warning]), ...(this.#startWarning === undefined ? [] : [this.#startWarning])];
  }

  /** Whether a listen host off loopback would run without TLS (never: Decision 29). */
  static needsTls(listen: string): boolean {
    const l = parseListen(listen);
    return typeof l !== "string" && !isLoopbackHost(l.host);
  }
}

export class RemoteListenerTag extends Context.Service<RemoteListenerTag, RemoteListener>()("finisherd/RemoteListener") {}
