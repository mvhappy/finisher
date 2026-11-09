// 5B's pieces of a starting server, wired in one place so `main.ts` stays a list of call
// sites: the routes and the environment (Task 5), pairing (Task 8), DPoP (Task 7), device
// tokens and the one device revoke path (Task 9), and their HTTP routes on the front.
import type { RouteKind } from "@finisher/contracts";
import { Context, Effect } from "effect";
import type { Credentials } from "../auth/credentials.ts";
import { DeviceTokens } from "../auth/device-tokens.ts";
import type { DeviceStore } from "../auth/devices.ts";
import { DpopVerifier } from "../auth/dpop.ts";
import type { Nonces } from "../auth/nonces.ts";
import { pairRoutes } from "../auth/pair-http.ts";
import { Pairing } from "../auth/pairing.ts";
import { type DeviceRevokeOptions, revokeCredential, type RevokeDeps, type RevokeReport, revokeDevice } from "../auth/revoke.ts";
import type { Tickets } from "../auth/tickets.ts";
import { tokenRoutes } from "../auth/token-http.ts";
import type { ConnectionRegistry, Transport } from "../connections.ts";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import { Environment } from "../env/descriptor.ts";
import type { HttpCtx, Route } from "../http/front.ts";
import { loopbackHosts } from "../http/policy.ts";
import type { ServerSecrets } from "../secrets/vault.ts";
import type { ServerControl } from "../server-admin.ts";
import type { RemoteListener } from "./remote.ts";
import { OS_ROUTES, ROUTES_POLL_MS, Routes } from "./routes.ts";
import { remoteConfigOf, type ServerSettings } from "./server-settings.ts";

/** What device RPCs and the revoke paths share (`auth/device-rpc.ts`). */
export interface DeviceAccess {
  readonly devices: DeviceStore;
  readonly tokens: DeviceTokens;
  /** The one device revoke path, with this server's tickets, connections and tokens. */
  readonly revoke: (id: string, opts: DeviceRevokeOptions) => RevokeReport;
}

export class DeviceAccessTag extends Context.Service<DeviceAccessTag, DeviceAccess>()("finisherd/DeviceAccess") {}

export interface RemoteWiringDeps {
  readonly store: ControlStore;
  readonly audit: Audit;
  readonly devices: DeviceStore;
  readonly secrets: ServerSecrets;
  readonly tickets: Tickets;
  readonly connections: ConnectionRegistry;
  readonly credentials: Credentials;
  readonly revokeDeps: RevokeDeps;
  readonly nonces: Nonces;
  readonly capabilities: ReadonlyArray<string>;
  readonly journal: ConstructorParameters<typeof Pairing>[0]["journal"];
  /** The loopback listener's port (known once it listens). */
  readonly loopbackPort: () => number;
  /** The front's routes, filled in here. */
  readonly httpRoutes: Record<string, Route>;
  /** Writes `finisherd.json` again (a reset's new `envId`). */
  readonly rewriteInfo: () => void;
  /** The remote listener's proof origins (Task 10; made after this wiring, so read late). */
  readonly remoteOrigins: () => ReadonlyArray<string>;
}

/** A timer for the server's lifetime. */
const every = (ms: number, what: string, f: () => void) =>
  Effect.acquireRelease(
    Effect.sync(() => {
      const timer = setInterval(() => {
        try {
          f();
        } catch (e) {
          process.stderr.write(`finisherd: warning: ${what} failed: ${String(e)}\n`);
        }
      }, ms);
      timer.unref();
      return timer;
    }),
    (timer) => Effect.sync(() => clearInterval(timer)),
  );

export const wireRemoteAccess = (d: RemoteWiringDeps) =>
  Effect.gen(function* () {
    const routes = new Routes(OS_ROUTES, { loopbackPort: d.loopbackPort });
    yield* every(ROUTES_POLL_MS, "route discovery", () => routes.poll());
    const tokens = new DeviceTokens();
    const revoke = (id: string, opts: DeviceRevokeOptions) =>
      revokeDevice({ tickets: d.tickets, connections: d.connections, audit: d.audit, devices: d.devices, tokens }, id, opts);
    const environment = new Environment({
      store: d.store,
      audit: d.audit,
      devices: d.devices,
      secrets: d.secrets,
      routes,
      capabilities: d.capabilities,
      revokeDevice: (id, reason) => revoke(id, { reason, by: { kind: "system", id: "finisherd" }, audit: false }).revoked,
      revokeCredentials: () => {
        let ended = 0;
        const owner = { ...d.revokeDeps, revoke: (ref: { kind: "agent" | "mcp"; id: string }) => d.credentials.markEnded(ref, "owner") };
        for (const c of d.credentials.list()) if (revokeCredential(owner, { kind: c.kind, id: c.id }, { rpc: "env.reset", reason: "owner" }).revoked) ended++;
        return ended;
      },
      rewriteInfo: d.rewriteInfo,
    });
    const pairing = new Pairing({ store: d.store, devices: d.devices, audit: d.audit, env: environment, journal: d.journal });
    yield* every(10_000, "pairing expiry sweep", () => pairing.sweep());
    const verifier = new DpopVerifier(d.nonces);
    // A proof names the origin of the listener it came in on: loopback's own, or the remote
    // listener's routes, `publicUrl` and its own names (Task 10).
    const allowedOrigins = (ctx: HttpCtx) => (ctx.listener === "loopback" ? loopbackHosts(d.loopbackPort()).map((h) => `http://${h}`) : d.remoteOrigins());
    // The kind of the route a request came over, by its Host (never its address).
    const routeOf = (ctx: HttpCtx): RouteKind => {
      if (ctx.listener === "loopback") return "loopback";
      const r = routes.current().find((x) => x.kind !== "loopback" && new URL(x.url).host.toLowerCase() === ctx.host);
      return r?.kind ?? "other";
    };
    Object.assign(d.httpRoutes, {
      // `GET /.well-known/finisher` (anonymous, spec §5.5): the descriptor's public subset.
      "GET /.well-known/finisher": async () => ({ status: 200, body: environment.wellKnown() }),
      ...pairRoutes({ pairing, verifier, env: environment, audit: d.audit, allowedOrigins, routeOf }),
      ...tokenRoutes({ devices: d.devices, tokens, tickets: d.tickets, verifier, env: environment, audit: d.audit, allowedOrigins, routeOf }),
    });
    const access: DeviceAccess = { devices: d.devices, tokens, revoke };
    return { routes, environment, pairing, verifier, access };
  });

/** `ServerControl` with 5B's parts: the live `envId`, the environment, `wss` and the warnings. */
export function serverControlOf(d: {
  readonly serverData: string;
  readonly store: ControlStore;
  readonly environment: Environment;
  readonly routes: Routes;
  readonly listeners: ReadonlyArray<{ readonly transport: Transport; readonly endpoint: string }>;
  readonly remote: RemoteListener;
  readonly secrets: ServerSecrets;
  readonly settings: ServerSettings;
  readonly stop: () => void;
}): ServerControl {
  return {
    serverData: d.serverData,
    get envId() {
      return d.store.envId;
    },
    environment: () => ({ state: d.environment.state(), spki: d.environment.spki(), routes: [...d.routes.current()] }),
    listeners: () => {
      const r = d.remote.info();
      return r === undefined ? d.listeners : [...d.listeners, { transport: "wss" as const, endpoint: r.endpoint, spki: r.spki, mode: r.mode, source: r.source }];
    },
    stop: d.stop,
    warnings: () => [
      ...d.secrets.warnings(),
      ...d.remote.warnings(),
      ...d.settings.invalid.map(({ key, code }) => ({ code, key, message: `settings.json ${key} is not used: it does not fit its schema` })),
    ],
  };
}

/**
 * Starts the remote listener once its transport serves (`FIN_LISTEN`, the settings or a test),
 * and stops it before the RPC layers close (finalizers run last first). From `FIN_LISTEN`
 * (Docker) a failure stops the start; from the settings it warns. Moved server data waits.
 */
export const startRemote = (
  remote: RemoteListener,
  settings: ServerSettings,
  environment: Environment,
  test?: { readonly forceTls?: boolean; readonly listen?: string },
) =>
  Effect.gen(function* () {
    const config =
      test?.forceTls === true ? { listen: test.listen ?? "127.0.0.1:0", tls: { mode: "selfSigned" as const }, source: "test" as const } : remoteConfigOf(settings);
    yield* Effect.addFinalizer(() => remote.stop());
    if (config === undefined || environment.state() === "moved") return;
    if (config.source === "env") yield* remote.start(config);
    else yield* Effect.catch(remote.start(config), (e) => Effect.sync(() => remote.noteStartFailure(e)));
  });
