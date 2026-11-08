// The server's own RPCs (spec §4.8, plan 5A Task 8): `server.status` (`server.read`),
// `server.stop` (`auth.admin`: the graceful stop Windows lacks, a service command that needs no
// terminal) and the owner's audit log (`auth.audit.list`, `auth.audit.purge`, `auth.admin`).
import { PROTOCOL_VERSION, type AuditKind, type FinError } from "@finisher/contracts";
import { Context, Effect } from "effect";
import { SERVER_VERSION } from "./config.ts";
import type { ConnectionRegistry, Transport } from "./connections.ts";
import type { Audit } from "./control/audit.ts";
import { finError, toFinError } from "./errors.ts";

/** What `serve` tells the admin RPCs about itself. */
export interface ServerControl {
  readonly envId: string;
  /** The endpoints this server listens on (known once both listen). */
  readonly listeners: () => ReadonlyArray<{ readonly transport: Transport; readonly endpoint: string }>;
  /** Starts the graceful shutdown: `serve` returns, its finalizers run, the process exits 0. */
  readonly stop: () => void;
}

export class ServerControlTag extends Context.Service<ServerControlTag, ServerControl>()("finisherd/ServerControl") {}

export interface AdminDeps {
  readonly control: ServerControl;
  readonly audit: Audit;
  readonly connections: ConnectionRegistry;
  readonly journalEnabled: () => boolean;
}

export interface AuditPurgeRequest {
  readonly all?: boolean | undefined;
  readonly before?: number | undefined;
}

/** `server.status`: no secret, no key, no ticket; counts and endpoints only. */
export function serverStatus(deps: AdminDeps) {
  return Effect.sync(() => ({
    version: SERVER_VERSION,
    protocol: PROTOCOL_VERSION,
    envId: deps.control.envId,
    pid: process.pid,
    listeners: [...deps.control.listeners()],
    connections: deps.connections.count(),
    journal: { enabled: deps.journalEnabled() },
    auditFailures: deps.audit.failures(),
    warnings: [] as Array<Record<string, unknown>>,
  }));
}

/**
 * `server.stop`: answers `{stopping: true}`, and once that reply is written (`afterReply`;
 * at once when the transport cannot say) the server stops: listeners close, open requests are
 * interrupted, engines close, `finisherd.json` is removed.
 */
export function serverStop(deps: AdminDeps, afterReply: ((f: () => void) => void) | undefined) {
  return Effect.sync(() => {
    if (afterReply === undefined) deps.control.stop();
    else afterReply(() => deps.control.stop());
    return { stopping: true as const };
  });
}

export function auditList(deps: AdminDeps, req: { since?: number | undefined; kind?: AuditKind | undefined; cursor?: string | undefined; limit?: number | undefined }) {
  return Effect.sync(() =>
    deps.audit.list({
      ...(req.since === undefined ? {} : { since: req.since }),
      ...(req.kind === undefined ? {} : { kind: req.kind }),
      ...(req.cursor === undefined ? {} : { cursor: req.cursor }),
      ...(req.limit === undefined ? {} : { limit: req.limit }),
    }),
  );
}

/**
 * `auth.audit.purge`: exactly one of `all: true` or `before` (ms). `audit.purge` rows are never
 * purged, and the purge is recorded (`audit.purge {reason, before?, count}`) in the same
 * transaction (`Audit.ownerPurge`), so the log always says it was purged, how far back and by
 * whom.
 */
export function auditPurge(
  deps: AdminDeps,
  req: AuditPurgeRequest,
  by: { readonly kind: string; readonly id: string },
  transport: Transport,
): Effect.Effect<{ removed: number }, FinError> {
  return Effect.suspend(() => {
    const all = req.all === true;
    const before = req.before;
    const validBefore = before !== undefined && Number.isFinite(before) && before >= 0;
    if (all === validBefore || (req.all === false && before === undefined)) {
      return Effect.fail(
        finError(
          "INVALID_REQUEST",
          "auth.audit.purge takes either all or a time before which entries go",
          { tag: "auth.audit.purge" },
          "Name either every entry or a date before which entries are removed.",
        ),
      );
    }
    return Effect.try({
      try: () => ({ removed: deps.audit.ownerPurge(all ? { all: true } : { before: Math.floor(before!) }, by, transport) }),
      catch: toFinError,
    });
  });
}
