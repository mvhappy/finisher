// The one way a credential is revoked (review of Task 6, finding 3), in this order:
//   1. it is marked ended in memory (`Credentials.markEnded`: revoked by its owner or itself,
//      or expired), so no handshake proves it again and no request of it runs;
//   2. every transport refuses its next request and any connection that registers with it
//      (a ticket taken before), `CREDENTIAL_REVOKED` (review of Task 7, M3);
//   3. fin-store refuses its writes not yet committed, inside their transactions, and this
//      waits for the ones already past that check (`endCredential`; review of Task 7, M3:
//      a write in flight when the credential is revoked cannot land afterwards);
//   4. every ticket minted for it is dropped;
//   5. every live connection it proved is ended, its open requests answered
//      `CREDENTIAL_REVOKED` and its streams closed with the connection (Task 5 review, L4:
//      streams are authorised once, at subscribe, so they must be ended here);
//   6. the end is written to the store (`persist`), so it holds after a restart. A busy store
//      is retried in the background and never stops steps 2–5 (review of Tasks 8–9, L2).
//
// What "nothing lands after the revocation" covers (review of Tasks 8–9, L1): every store
// write (`Store::write`: commits, branches, history, recovery) and `project.acknowledgeRewind`,
// `project.acknowledgeSalvage` and `diagnostics.project`, which hold the credential's check
// through their change (step 3 waits for them). Other calls that were already past the chain's
// check when the credential was revoked may still finish: the long maintenance calls
// (`project.rebuild`, `compact`, `takeover`), `project.create`/`open`, `asset.probe`,
// `fonts.fetch`, library scans, job submissions, plugin plans and refreshes, and
// `diagnostics.record`. Their next request is refused. Step 3 runs on the main thread: a long
// commit in flight stalls the server until it lands (a known cost).
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
  /** Marks the credential ended (in memory); true when it was not ended yet (Task 9). */
  readonly revoke?: (ref: RevocableRef) => boolean;
  /** Writes ended credentials to the store, last (step 6; retried on failure). */
  readonly persist?: () => void;
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

/** A correct proof of a credential that expired (review of Tasks 8–9, L4): exit 7, as a revoked one. */
export function credentialExpired(): FinError {
  return finError(
    "CREDENTIAL_EXPIRED",
    "this credential expired",
    {},
    "This credential no longer works: it was not used for too long, or reached its time limit. Ask the owner of this Finisher server for a new one.",
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
  // Step 6: a store error here is retried by `persist` itself and never undoes steps 1–5.
  try {
    deps.persist?.();
  } catch (e) {
    process.stderr.write(`finisherd: warning: cannot record a credential's end: ${String(e)}\n`);
  }
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
 * path as a revocation, audited `credential.expire`. Answers how many it ended. A stream of a
 * credential that expired runs until this sweep ends it, up to 60 s past the expiry (its other
 * requests are refused at once; review of Tasks 8–9, L3).
 */
export function sweepExpired(deps: RevokeDeps, expired: () => ReadonlyArray<RevocableRef>): number {
  let n = 0;
  // One credential that cannot be ended never keeps the others alive (review of Tasks 8–9, L2).
  for (const ref of expired()) {
    try {
      if (revokeCredential(deps, ref, { reason: "expired" }).revoked) n++;
    } catch (e) {
      process.stderr.write(`finisherd: warning: cannot end an expired credential: ${String(e)}\n`);
    }
  }
  return n;
}
