// The agent listener (spec §8.3, plan 7C Decision 13, Task 6). On Linux the `agent` profile has
// no Unix socket (the seccomp filter that closes `AF_UNIX` is deferred, plan Task 4, but the
// design does not depend on it), so `fin` inside a sandbox reaches finisherd over loopback TCP:
// `127.0.0.1:<ephemeral>`, the local socket's NDJSON framing and handshake, the canonical
// endpoint `tcp:127.0.0.1:<port>` inside both MACs (`FIN_SERVER=tcp:127.0.0.1:<port>`).
//
// - Only `agent` and `mcp` principals are served: any other proof (`local`, `console`) is refused
//   after it is checked and audited `auth.fail {reason: listenerKind}` (`auth/handshake.ts`).
// - A connection that has not proved within `HANDSHAKE_TIMEOUT_MS` is closed; at most
//   `MAX_AGENT_CONNECTIONS` are open at once, of which at most `MAX_UNPROVED` unproved
//   (`Connections`' listener rules), so another local user holding unproved connections cannot
//   take the slots proved agents use. Refusals at a cap are logged, at most once a minute.
//   Residual: such a user can still keep new agents' `fin` from connecting while it holds the
//   unproved slots (each for up to the timeout), since loopback TCP has no peer credentials.
// - It is started on first need (`AgentListener.ensure`, called by the session wiring for a
//   sandboxed session or hook, plan Task 11), never at boot, and stops with the server. The
//   built-in agent's `fin` tool stays on the local socket: it runs outside the OS sandbox
//   (spec §8.9).
import * as NodeSocketServer from "@effect/platform-node/NodeSocketServer";
import { Effect } from "effect";
import { startFailure } from "../config.ts";
import { Connections, type ListenerRules } from "../connections.ts";
import { ANONYMOUS, type Principal } from "../principal.ts";

/** The principals the listener serves. */
export const AGENT_LISTENER_KINDS: ReadonlySet<string> = new Set(["agent", "mcp"]);
/** How long a connection may stay unproved (the WebSocket's header timeout, `upgrade.ts`). */
export const HANDSHAKE_TIMEOUT_MS = 10_000;
/** Open connections at once (every session's `fin` calls share it). */
export const MAX_AGENT_CONNECTIONS = 256;
/** Of those, unproved at once (a handshake takes one round trip). */
export const MAX_UNPROVED = 32;
/** The only address it listens on. */
export const AGENT_LISTENER_HOST = "127.0.0.1";

/** The canonical endpoint both MACs bind, and `FIN_SERVER`'s value. */
export const agentEndpoint = (port: number) => `tcp:${AGENT_LISTENER_HOST}:${port}`;

/** The listener's connections: `agentTcp`, `agent` and `mcp` only, the timeout and the cap. */
export function agentConnections(endpoint: () => string, rules: Partial<ListenerRules> = {}, now: () => number = Date.now): Connections {
  return new Connections("agentTcp", endpoint, ANONYMOUS, now, {
    admits: (p: Principal) => AGENT_LISTENER_KINDS.has(p.kind),
    handshakeTimeoutMs: HANDSHAKE_TIMEOUT_MS,
    maxConnections: MAX_AGENT_CONNECTIONS,
    maxUnproved: MAX_UNPROVED,
    ...rules,
  });
}

/** A loopback TCP server on an ephemeral port, and the port. */
export const makeAgentTcpServer = () =>
  Effect.gen(function* () {
    const server = yield* Effect.mapError(NodeSocketServer.make({ host: AGENT_LISTENER_HOST, port: 0 }), (e) =>
      startFailure((e.reason as { cause?: unknown }).cause ?? e),
    );
    // Bound to `AGENT_LISTENER_HOST` above; an IPv4 address with a port is all it can be.
    const address = server.address;
    if (address._tag !== "InetAddressV4" || address.port === 0) {
      return yield* Effect.fail(startFailure(new Error(`the agent listener is not on ${AGENT_LISTENER_HOST}`)));
    }
    return { server, port: address.port };
  });

/** Starts the listener once, on first need; its endpoint while it runs. */
export interface AgentListener {
  /** The endpoint (`tcp:127.0.0.1:<port>`), starting the listener if it is not running. */
  ensure(): Promise<string>;
  /** The endpoint, or `undefined` before the first `ensure`. */
  current(): string | undefined;
}

/** `start` runs at most once at a time; a failed start is tried again by the next `ensure`. */
export function makeAgentListener(start: () => Promise<string>): AgentListener {
  let started: Promise<string> | undefined;
  let endpoint: string | undefined;
  return {
    ensure() {
      started ??= start().then(
        (e) => {
          endpoint = e;
          return e;
        },
        (err: unknown) => {
          started = undefined;
          throw err;
        },
      );
      return started;
    },
    current: () => endpoint,
  };
}

/** Calls `report(n)` with the number of events since the last report, at most once per `ms`. */
export function rateLimited(ms: number, report: (n: number) => void, now: () => number = Date.now): () => void {
  let last = -Infinity;
  let n = 0;
  return () => {
    n++;
    if (now() - last < ms) return;
    last = now();
    report(n);
    n = 0;
  };
}
