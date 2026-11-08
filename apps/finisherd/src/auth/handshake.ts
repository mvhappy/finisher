// The handshake's server half (spec §4.2, plan 5A Decisions 8 and 9):
//
//   server.hello {clientNonce, env?, endpoint?, credential?}
//     → {envId, endpoint, serverNonce, proof?}   proof = HMAC(K, hello ‖ envId ‖ endpoint ‖ cn ‖ sn)
//   auth.prove {mac}                              mac   = HMAC(K, prove ‖ envId ‖ endpoint ‖ sn ‖ cn)
//     → {principal}
//
// `K` is the named credential's key. An unknown credential, a foreign `env` or another endpoint
// gets a `serverNonce` and no proof, so ids cannot be probed, and its prove fails. A credential
// that exists but has ended gets a proof (the server shows it is the one that minted it) and a
// correct prove is then refused `CREDENTIAL_REVOKED` or `CREDENTIAL_EXPIRED` (exit 7; review of
// Tasks 8–9, L4), so `fin` can say which instead of suspecting a squatter. Each hello
// is answered by at most one prove; every failed prove is audited (`auth.fail`, aggregated per
// credential kind per minute) and the fifth on one connection closes it. Nonces and MACs are
// never logged, audited or put in an error.
import type { FinError, Principal } from "@finisher/contracts";
import { Context, Effect } from "effect";
import type { Audit } from "../control/audit.ts";
import type { Connection, Connections, CredentialRef, Transport } from "../connections.ts";
import { finError } from "../errors.ts";
import { CONSOLE, isProved, LOCAL } from "../principal.ts";
import { b64u, helloProof, NONCE_BYTES, nonce, proveMac, sameMac } from "./crypto.ts";
import type { Keys } from "./keys.ts";
import { credentialExpired, credentialRevoked, revokeCredential, type RevokeDeps } from "./revoke.ts";

/** Failed proves after which a connection is closed (Decision 9). */
export const MAX_PROVE_FAILURES = 5;

export interface HelloRequest {
  readonly clientNonce?: string | undefined;
  readonly env?: string | undefined;
  readonly endpoint?: string | undefined;
  readonly credential?: CredentialRef | undefined;
}

export interface HelloAnswer {
  readonly envId?: string;
  readonly endpoint?: string;
  readonly serverNonce?: string;
  readonly proof?: string;
}

/**
 * An `agent`/`mcp` credential's key and principal (Task 9), with `ended` once it was revoked or
 * expired; `undefined` for an unknown one.
 */
export type CredentialLookup = (ref: { kind: "agent" | "mcp"; id: string }) => { key: Buffer; principal: Principal; ended?: "revoked" | "expired" } | undefined;

/** Revokes an `agent`/`mcp` credential (Task 9's store); false when it was not live. */
export type CredentialRevoke = (ref: { kind: "agent" | "mcp"; id: string }) => boolean;

export interface HandshakeDeps {
  readonly keys: Keys;
  readonly envId: string;
  readonly audit: Audit;
  readonly credentials?: CredentialLookup;
  readonly revoke?: CredentialRevoke;
}

const UNAUTHENTICATED_FIX =
  "Run the command as the same user as the Finisher server, with that server's data, or give it a credential file through the FIN_CREDENTIAL variable.";

export function unauthenticated(reason: string): FinError {
  return finError("UNAUTHENTICATED", "this connection has not proved a credential", { reason }, UNAUTHENTICATED_FIX);
}

export class Handshake {
  readonly deps: HandshakeDeps;
  constructor(deps: HandshakeDeps) {
    this.deps = deps;
  }

  #keyOf(ref: CredentialRef): { key: Buffer; principal: Principal; ended?: "revoked" | "expired" } | undefined {
    switch (ref.kind) {
      case "local":
        return { key: this.deps.keys.local, principal: LOCAL };
      case "console":
        return { key: this.deps.keys.console, principal: CONSOLE };
      default:
        return this.deps.credentials?.(ref);
    }
  }

  /**
   * The handshake fields of the hello on `conn`, whose canonical endpoint is `endpoint`. A
   * connection that is not live (`undefined`) gets no handshake fields: nothing can be proved.
   */
  hello(conn: Connection | undefined, endpoint: string, req: HelloRequest): Effect.Effect<HelloAnswer, FinError> {
    return Effect.suspend(() => {
      if (req.clientNonce === undefined || conn === undefined) return Effect.succeed({});
      const clientNonce = b64u.decode(req.clientNonce);
      if (clientNonce === null || clientNonce.length !== NONCE_BYTES) {
        return Effect.fail(
          finError("INVALID_REQUEST", `clientNonce is ${NONCE_BYTES} bytes in base64url`, {}, "use fin and finisherd from the same Finisher release"),
        );
      }
      const serverNonce = nonce();
      const credential = req.credential ?? { kind: "local" };
      const { envId } = this.deps;
      const named = this.#keyOf(credential);
      const bound = (req.env === undefined || req.env === envId) && (req.endpoint === undefined || req.endpoint === endpoint);
      const key = bound ? named?.key : undefined;
      conn.hello = {
        clientNonce,
        serverNonce,
        credential,
        ...(key === undefined ? {} : { expected: proveMac(key, envId, endpoint, serverNonce, clientNonce) }),
      };
      return Effect.succeed({
        envId,
        endpoint,
        serverNonce: b64u.encode(serverNonce),
        ...(key === undefined ? {} : { proof: b64u.encode(helloProof(key, envId, endpoint, clientNonce, serverNonce)) }),
      });
    });
  }

  /** `auth.prove` on client `clientId` of `connections`. */
  prove(connections: Connections, clientId: number, req: { readonly mac: string }): Effect.Effect<{ principal: Principal }, FinError> {
    return Effect.suspend(() => {
      const conn = connections.get(clientId);
      if (conn === undefined) return Effect.fail(unauthenticated("unknownConnection"));
      if (isProved(conn.principal)) {
        return Effect.fail(finError("INVALID_REQUEST", "this connection has already proved its credential", {}, "prove once per connection"));
      }
      const hello = conn.hello;
      conn.hello = undefined;
      const mac = b64u.decode(req.mac);
      const expected = hello?.expected;
      if (hello !== undefined && expected !== undefined && mac !== null && sameMac(mac, expected)) {
        const named = this.#keyOf(hello.credential);
        if (named !== undefined && named.ended === undefined) {
          conn.principal = named.principal;
          return Effect.succeed({ principal: named.principal });
        }
        // Ended (or ended since the hello): the caller holds its key, so it may know why.
        if (named?.ended !== undefined) {
          this.#fail(connections, clientId, conn, hello.credential.kind, named.ended);
          return Effect.fail(named.ended === "revoked" ? credentialRevoked() : credentialExpired());
        }
      }
      const reason = hello === undefined ? "noHello" : expected === undefined ? "noProof" : "badMac";
      this.#fail(connections, clientId, conn, hello?.credential.kind ?? "none", reason);
      return Effect.fail(unauthenticated(reason));
    });
  }

  /** A failed prove: counted (the fifth closes the connection) and audited `auth.fail`. */
  #fail(connections: Connections, clientId: number, conn: Connection, credentialKind: string, reason: string): void {
    conn.failures++;
    this.deps.audit.append({
      kind: "auth.fail",
      principal: { kind: "anonymous", id: "" },
      credentialKind,
      outcome: "failed",
      code: reason === "revoked" ? "CREDENTIAL_REVOKED" : reason === "expired" ? "CREDENTIAL_EXPIRED" : "UNAUTHENTICATED",
      details: { reason, transport: conn.transport, ...(conn.peer === undefined ? {} : { peer: conn.peer }) },
    });
    if (conn.failures >= MAX_PROVE_FAILURES) connections.link(clientId)?.closeAfterReply();
  }
}

/**
 * `auth.revokeSelf` (spec §4.8): the calling `agent`/`mcp` credential revokes itself. It takes no
 * id, so it cannot name another credential. It goes the one revoke path (`revoke.ts`): its
 * tickets are dropped and its other connections ended; this one is closed after the reply.
 */
export function revokeSelf(
  deps: RevokeDeps,
  principal: Principal,
  caller: { readonly transport: Transport; readonly clientId: number; readonly closeAfterReply: () => void },
): Effect.Effect<Record<string, never>, FinError> {
  return Effect.suspend(() => {
    const { kind, id } = principal;
    if (kind !== "agent" && kind !== "mcp") {
      return Effect.fail(
        finError(
          "INVALID_REQUEST",
          `a ${kind} principal has no credential of its own to revoke`,
          { kind },
          "Only agent and MCP credentials can revoke themselves; the owner of this Finisher server revokes the others.",
        ),
      );
    }
    revokeCredential(deps, { kind, id }, { rpc: "auth.revokeSelf", reason: "self", except: caller });
    caller.closeAfterReply();
    return Effect.succeed({});
  });
}

export class HandshakeTag extends Context.Service<HandshakeTag, Handshake>()("finisherd/Handshake") {}
