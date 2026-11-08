// The one way a credential is revoked (review of Task 6, finding 3): the store marks it revoked
// (Task 9 provides `revoke`; until then nothing is stored), every ticket minted for it is
// dropped, and every live connection it proved is ended, its open requests answered
// `CREDENTIAL_REVOKED` and its streams closed with the connection. `auth.revokeSelf` uses it
// now; Task 9's owner-side revoke and expiry sweep call the same function.
import type { FinError } from "@finisher/contracts";
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
  readonly connections: Pick<ConnectionRegistry, "closeWhere">;
  readonly audit: Audit;
  /** Marks the credential revoked in the store; true when it was live (Task 9). */
  readonly revoke?: (ref: RevocableRef) => boolean;
}

export interface RevokeOptions {
  /** The RPC that revoked it, for the audit row. */
  readonly rpc: string;
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
  const revoked = deps.revoke?.(ref) ?? false;
  const tickets = deps.tickets.dropWhere((p) => holds(p, ref));
  const except = options.except;
  const connections = deps.connections.closeWhere(
    (c) => holds(c.principal, ref) && !(except !== undefined && c.transport === except.transport && c.clientId === except.clientId),
    credentialRevoked(),
  );
  if (revoked) {
    deps.audit.append({
      kind: options.reason === "expired" ? "credential.expire" : "credential.revoke",
      principal: { kind: ref.kind, id: ref.id },
      credentialKind: ref.kind,
      outcome: "ok",
      details: { rpc: options.rpc, reason: options.reason, count: connections, ...(CREDENTIAL_ID.test(ref.id) ? { credentialId: ref.id } : {}) },
    });
  }
  return { revoked, tickets, connections };
}
