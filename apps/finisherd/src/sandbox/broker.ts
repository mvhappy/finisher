// The ACL broker (plan 7C Decision 18, Task 10): on Windows every ACE a sandboxed process
// needs goes through fin-node's broker (`aclGrant`, `aclRevoke`, `aclSweep`), which logs each
// change to `<server-data>/run/acl-broker.wal` before applying it, refuses protected roots,
// and serialises changes per path. Off Windows the sandboxes grant by path when they start
// (Landlock, Seatbelt), so this is a no-op there.
//
// Owners: `session:<id>` (revoked when the session's sandbox closes), `job:<id>` and
// `call:<id>` (Task 12, sp8), and `static` for what every launch of a kind shares (the
// toolchain roots, a project's folder for its container, a sign-in store): granted once,
// never swept. `main.ts` sweeps before any listener starts with no owner live, since no
// session, job or call outlives the server that started it.
import type { AclGrant } from "@finisher/core-types";
import { type AclBrokerGrant, aclGrant, aclRevoke, aclSweep, sandboxContainerSid } from "@finisher/fin-node";
import { Effect } from "effect";

export const STATIC_OWNER = "static";

/** The owner of a session's own grants. */
export function sessionOwner(sessionId: string): string {
  return `session:${sessionId}`;
}

export interface AclBroker {
  /** False off Windows: every call resolves without doing anything. */
  readonly active: boolean;
  grant(g: AclBrokerGrant): Promise<void>;
  /** Revokes `owner`'s grants (only those on `path` when given); how many. */
  revoke(owner: string, path?: string): Promise<number>;
  /** Revokes the grants of every owner not in `live` (`static` always is); how many. */
  sweep(live: Iterable<string>): Promise<number>;
}

/** What `aclBroker` calls (tests pass fakes). */
export interface BrokerNative {
  readonly aclGrant: typeof aclGrant;
  readonly aclRevoke: typeof aclRevoke;
  readonly aclSweep: typeof aclSweep;
}

const NATIVE: BrokerNative = { aclGrant, aclRevoke, aclSweep };

/** The broker of `serverData` (one per server: fin-node keys it by the canonical directory). */
export function aclBroker(serverData: string, opts: { platform?: NodeJS.Platform; native?: BrokerNative } = {}): AclBroker {
  const native = opts.native ?? NATIVE;
  if ((opts.platform ?? process.platform) !== "win32") {
    return { active: false, grant: async () => {}, revoke: async () => 0, sweep: async () => 0 };
  }
  return {
    active: true,
    grant: (g) => native.aclGrant(serverData, g),
    revoke: (owner, path) => native.aclRevoke(serverData, owner, path),
    sweep: (live) => native.aclSweep(serverData, live),
  };
}

/**
 * The start's sweep (no owner live): how many grants it revoked, or `undefined` when it failed.
 * A failure is a warning, never a start failure (Task 10 review 5): a revoke that cannot be done
 * stays pending in the log for the next sweep, and a broker that cannot open makes every later
 * grant fail, so agents fail closed.
 */
export const sweepAtStart = (broker: AclBroker): Effect.Effect<number | undefined> =>
  Effect.tryPromise({ try: () => broker.sweep([]), catch: (e) => e }).pipe(
    Effect.catch((e) => Effect.as(Effect.logWarning(`finisherd: ACL broker: the start's sweep did not complete (what failed stays pending): ${e instanceof Error ? e.message : String(e)}`), undefined)),
  );

export interface TableOptions {
  /** The owner of the table's `owner`-scoped entries (`session:<id>`, `call:<id>`). */
  readonly owner: string;
  /** A container's SID by its name (default: fin-node's `sandboxContainerSid`). */
  readonly containerSid?: (name: string) => string | null;
  /** Called once per container the table names, with its SID (the pipe's `allow`). */
  readonly onContainer?: (sid: string) => void;
}

/**
 * Grants a launch's table (`sandboxAgentGrants`, `sandboxHookGrants`): every entry inherited
 * by its subtree; `static` entries under the static owner, the others under `opts.owner`. An
 * entry whose path does not exist (a session's `credential.json` before it is written, a
 * sign-in store before the first sign-in) is skipped, as Landlock skips a missing root.
 */
export async function grantTable(broker: AclBroker, table: readonly AclGrant[], opts: TableOptions): Promise<void> {
  if (!broker.active) return;
  const resolve = opts.containerSid ?? sandboxContainerSid;
  const containers = new Map<string, string>();
  for (const g of table) {
    let sid: string;
    if (g.trustee.kind === "sid") sid = g.trustee.value;
    else {
      const known = containers.get(g.trustee.value);
      const resolved = known ?? resolve(g.trustee.value);
      if (resolved === null) throw new Error(`no SID for the container ${g.trustee.value}`);
      if (known === undefined) {
        containers.set(g.trustee.value, resolved);
        opts.onContainer?.(resolved);
      }
      sid = resolved;
    }
    const owner = g.scope === "static" ? STATIC_OWNER : opts.owner;
    try {
      await broker.grant({ path: g.path, sid, mask: g.mask, inherit: true, owner });
    } catch (e) {
      if ((e as { code?: unknown }).code !== "NOT_FOUND") throw e;
    }
  }
}
