// Connections and their principals (spec §4.3, plan 5A Decision 12).
//
// What Effect 4.0.6's `effect/rpc` gives (read in `RpcServer.ts`, Task 3 Step 1):
// - **Connection id:** a server `Protocol` numbers its connections (`clientId`, a counter per
//   protocol instance, so per transport here) and passes it with every message to the server
//   loop (`run((clientId, message) => …)`). Handlers see it as `options.client.id` (an
//   `Rpc.ServerClient`), beside `options.requestId`, `options.headers` (a lower-cased record)
//   and `options.rpc`. Ids are only unique within one transport: `Connections` is per transport.
// - **Disconnects:** the protocol offers the id to its `disconnects` queue when a socket closes;
//   `RpcServer.make` takes it and interrupts that client's fibers. Nothing else is told, so
//   finisherd's own protocol (`protocol.ts`) drops the connection here before offering the id.
// - **Decoding:** `RpcServer.make` decodes a request's payload with the RPC's schema and, on a
//   failure or an unknown tag, answers with an `Exit` whose cause is a `Die` (a defect); bad
//   JSON from `makeSocketProtocol` becomes a `Defect` frame. finisherd's protocol checks the
//   frame, the tag and the payload first and answers `INVALID_REQUEST` itself (Decision 13), so
//   no handler sees an undecoded payload and no defect frame is sent for a client's mistake.
// - **Closing:** `Protocol.end` is a no-op for sockets and `RpcServer.disconnect` only
//   interrupts fibers; closing the socket ends the protocol's per-socket scope, so the link
//   `protocol.ts` registers here closes it.
import type { FinError } from "@finisher/contracts";
import { Context } from "effect";
import { ANONYMOUS, type Principal } from "./principal.ts";

export type Transport = "socket" | "pipe" | "ws";

export type CredentialRef = { kind: "local" | "console" } | { kind: "agent" | "mcp"; id: string };

/** The hello a connection's next `auth.prove` answers (consumed by that prove). */
export interface Hello {
  readonly clientNonce: Buffer;
  readonly serverNonce: Buffer;
  readonly credential: CredentialRef;
  /** The MAC a correct `auth.prove` carries; absent when the hello gave no proof. */
  readonly expected?: Buffer;
}

export interface Connection {
  readonly clientId: number;
  readonly transport: Transport;
  readonly openedAt: number;
  principal: Principal;
  hello?: Hello | undefined;
  failures: number;
  peer?: { pid?: number; sid?: string };
}

/** What the protocol lets this module do to a live connection. */
export interface Link {
  /** Closes the connection now. */
  close(): void;
  /** Closes it once the reply being written now has been sent. */
  closeAfterReply(): void;
  /** Answers every open request with `error`, then closes. */
  terminate(error: FinError): void;
}

/** The connections of one transport, by Effect RPC client id. */
export class Connections {
  readonly #byId = new Map<number, Connection>();
  readonly #links = new Map<number, Link>();

  /**
   * `initial`: the principal a new connection carries: `anonymous` on the local socket; the
   * WebSocket's bearer token (until Task 6's tickets) makes its connections `local`.
   */
  readonly transport: Transport;
  readonly endpoint: () => string;
  private readonly initial: Principal;
  private readonly now: () => number;

  constructor(transport: Transport, endpoint: () => string, initial: Principal = ANONYMOUS, now: () => number = Date.now) {
    this.transport = transport;
    this.endpoint = endpoint;
    this.initial = initial;
    this.now = now;
  }

  /** The connection with this id, created on first sight. */
  get(clientId: number): Connection {
    let c = this.#byId.get(clientId);
    if (c === undefined) {
      c = { clientId, transport: this.transport, openedAt: this.now(), principal: this.initial, failures: 0 };
      this.#byId.set(clientId, c);
    }
    return c;
  }

  has(clientId: number): boolean {
    return this.#byId.has(clientId);
  }

  /** Registers the protocol's handle on a new connection (`protocol.ts`). */
  attach(clientId: number, link: Link): Connection {
    this.#links.set(clientId, link);
    return this.get(clientId);
  }

  /** Drops the connection (on disconnect). */
  close(clientId: number): void {
    this.#byId.delete(clientId);
    this.#links.delete(clientId);
  }

  /** The protocol's handle on a live connection. */
  link(clientId: number): Link | undefined {
    return this.#links.get(clientId);
  }

  /** Ends every connection `pred` selects, answering its open requests with `error`. */
  closeWhere(pred: (c: Connection) => boolean, error: FinError): number {
    let n = 0;
    for (const c of [...this.#byId.values()]) {
      if (!pred(c)) continue;
      n++;
      this.#links.get(c.clientId)?.terminate(error);
    }
    return n;
  }

  /** Live connections by principal kind. */
  count(): Record<string, number> {
    const out: Record<string, number> = {};
    for (const c of this.#byId.values()) out[c.principal.kind] = (out[c.principal.kind] ?? 0) + 1;
    return out;
  }

  /**
   * The principal a request of this connection runs as: `anonymous` until it proves (Decision 9;
   * Task 3 ran unproved connections as `local`, audited once as `auth.legacy`, until `fin`
   * proved).
   */
  principalOf(clientId: number): Principal {
    return this.get(clientId).principal;
  }
}

/** Every transport's connections, for status counts and revocation. */
export class ConnectionRegistry {
  readonly #all: Connections[] = [];

  add(c: Connections): Connections {
    this.#all.push(c);
    return c;
  }

  closeWhere(pred: (c: Connection) => boolean, error: FinError): number {
    return this.#all.reduce((n, c) => n + c.closeWhere(pred, error), 0);
  }

  count(): Record<string, number> {
    const out: Record<string, number> = {};
    for (const c of this.#all) for (const [k, v] of Object.entries(c.count())) out[k] = (out[k] ?? 0) + v;
    return out;
  }
}

/** The connections of the transport a handler serves (provided per transport, `transport.ts`). */
export class ConnectionsTag extends Context.Service<ConnectionsTag, Connections>()("finisherd/Connections") {}
