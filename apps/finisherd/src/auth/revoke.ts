// The one way a credential is revoked (review of Task 6, finding 3), in this order:
//   1. the store marks it ended (`Credentials.end`: revoked by its owner or itself, or expired),
//      so no handshake proves it again, also after a restart;
//   2. every transport refuses its next request and any connection that registers with it
//      (a ticket taken before), `CREDENTIAL_REVOKED` (review of Task 7, M3);
//   3. fin-store refuses its writes not yet committed, inside their transactions, and this
//      waits for the ones already past that check (`endCredential`; review of Task 7, M3:
//      a write in flight when the credential is revoked cannot land afterwards);
//   4. every ticket minted for it is dropped;
//   5. every live connection it proved is ended, its open requests answered
//      `CREDENTIAL_REVOKED` and its streams closed with the connection (Task 5 review, L4:
//      streams are authorised once, at subscribe, so they must be ended here).
// `auth.revokeSelf`, the owner's `auth.credential.revoke` and the expiry sweep all call it.
import type { FinError } from "@finisher/contracts";
import { endCredential } from "@finisher/fin-node";
import type { ConnectionRegistry, Transport } from "../connections.ts";
import type { Audit } from "../control/audit.ts";
import { finError } from "../errors.ts";
import type { Principal } from "../principal.ts";
import type { Tickets } from "./tickets.ts";

/** A credential that can be revoked: `agent` and `mcp` (5A; 5B adds `device`). */
export interface RevocableRef {
  readonly kind: "agent" | "mcp";
  readonly id: string;
}

export interface RevokeDeps {
  readonly tickets: Pick<Tickets, "dropWhere">;
  readonly connections: Pick<ConnectionRegistry, "closeWhere" | "revoke">;
  readonly audit: Audit;
  /** Marks the credential ended in the store; true when it was not ended yet (Task 9). */
  readonly revoke?: (ref: RevocableRef) => boolean;
}

export interface RevokeOptions {
  /** The RPC that revoked it, for the audit row (none for the expiry sweep). */
  readonly rpc?: string;
  readonly reason: "self" | "owner" | "expired";
  /** A connection left open (the caller of `auth.revokeSelf`, closed after its reply). */
  readonly except?: { readonly transport: Transport; readonly clientId: number };
}

export interface RevokeReport {
  readonly revoked: boolean;
  readonly tickets: number;
  readonly connections: number;
}

const CREDENTIAL_ID = /^[0-9A-Za-z]{26}$/;

export function credentialRevoked(): FinError {
  return finError(
    "CREDENTIAL_REVOKED",
    "the credential this connection proved was revoked",
    {},
    "This credential no longer works. Ask the owner of this Finisher server for a new one.",
  );
}

const holds = (p: Principal, ref: RevocableRef) => p.kind === ref.kind && p.id === ref.id;

/** Revokes `ref`: the store, its tickets, its live connections (but `except`), audited once. */
export function revokeCredential(deps: RevokeDeps, ref: RevocableRef, options: RevokeOptions): RevokeReport {
  const stored = deps.revoke?.(ref);
  // Before the sweep: a connection registering or a request arriving from here on is refused
  // even if it took its ticket or started before (review of Task 7, M3).
  deps.connections.revoke(ref);
  endCredential(`${ref.kind}:${ref.id}`);
  const tickets = deps.tickets.dropWhere((p) => holds(p, ref));
  const except = options.except;
  const connections = deps.connections.closeWhere(
    (c) => holds(c.principal, ref) && !(except !== undefined && c.transport === except.transport && c.clientId === except.clientId),
    credentialRevoked(),
  );
  // With the credential store (`serve` always has it, Task 9) only a credential that was not
  // ended yet is audited; without one (unit tests) every revocation is.
  const revoked = stored ?? false;
  if (stored !== false) {
    deps.audit.append({
      kind: options.reason === "expired" ? "credential.expire" : "credential.revoke",
      principal: { kind: ref.kind, id: ref.id },
      credentialKind: ref.kind,
      outcome: "ok",
      details: { ...(options.rpc === undefined ? {} : { rpc: options.rpc }), reason: options.reason, count: connections, ...(CREDENTIAL_ID.test(ref.id) ? { credentialId: ref.id } : {}) },
    });
  }
  return { revoked, tickets, connections };
}

/**
 * Ends every credential that expired since the last sweep (Decision 32: every 60 s): the same
 * path as a revocation, audited `credential.expire`. Answers how many it ended.
 */
export function sweepExpired(deps: RevokeDeps, expired: () => ReadonlyArray<RevocableRef>): number {
  let n = 0;
  for (const ref of expired()) if (revokeCredential(deps, ref, { reason: "expired" }).revoked) n++;
  return n;
}
