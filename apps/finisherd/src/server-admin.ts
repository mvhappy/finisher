// The server's own RPCs (spec §4.8, plan 5A Task 8): `server.status` (`server.read`),
// `server.stop` (`auth.admin`: the graceful stop Windows lacks, a service command that needs no
// terminal) and the owner's audit log (`auth.audit.list`, `auth.audit.purge`, `auth.admin`).
import { PROTOCOL_VERSION, type AuditKind, type FinError, type Route } from "@finisher/contracts";
import type { JobHostCaps } from "@finisher/fin-node";
import { Context, Effect } from "effect";
import { SERVER_VERSION } from "./config.ts";
import type { ConnectionRegistry, Transport } from "./connections.ts";
import type { Audit } from "./control/audit.ts";
import { finError, toFinError } from "./errors.ts";
import { agentWarnings } from "./agent/warnings.ts";
import { currentSandboxStatus, profileWarning, sandboxWarnings, type SandboxReporter, type SandboxStatus } from "./sandbox/status.ts";

/** What `serve` tells the admin RPCs about itself. */
export interface ServerControl {
  readonly envId: string;
  /** The server data directory (`settings.write` writes its `settings.json`; `serve` sets it). */
  readonly serverData?: string;
  /** The endpoints this server listens on (known once both listen). */
  readonly listeners: () => ReadonlyArray<{ readonly transport: Transport; readonly endpoint: string }>;
  /** Starts the graceful shutdown: `serve` returns, its finalizers run, the process exits 0. */
  readonly stop: () => void;
  /** The environment's state, pin and routes (5B Task 5). */
  readonly environment?: () => { readonly state: "ok" | "moved"; readonly spki: string; readonly routes: ReadonlyArray<Route> };
  /** Standing warnings of the server itself (5B: where secrets are kept). */
  readonly warnings?: () => ReadonlyArray<Record<string, unknown>>;
}

export class ServerControlTag extends Context.Service<ServerControlTag, ServerControl>()("finisherd/ServerControl") {}

export interface AdminDeps {
  readonly control: ServerControl;
  readonly audit: Audit;
  readonly connections: ConnectionRegistry;
  readonly journalEnabled: () => boolean;
  /** What the job host enforces (`JobEngine.workers().caps`, 5C-2). */
  readonly jobCaps: () => JobHostCaps;
  /** The sandbox status source (default: fin-node's `sandboxStatus`). */
  readonly sandbox?: () => SandboxStatus;
  /** Records `sandbox.unconfined` when a profile's status changed since the last report. */
  readonly sandboxReporter?: SandboxReporter;
  /** Enabled, active code plugins (sp8b Decision 17); without it the plugin profile warns as any other. */
  readonly enabledCodePlugins?: () => number;
}

/**
 * The standing warning while job workers run unconfined (5C-2 Decision 9, spec §6.2; plan 7C
 * Decision 23): the `worker` profile's status, with the job host's spawner (`sandbox`). A host
 * whose spawner does not apply the profile (`caps.profiles.worker`, plan 7C Task 7) warns even
 * where the profile could be enforced.
 */
export function sandboxWarning(caps: JobHostCaps, status: SandboxStatus = currentSandboxStatus()): Record<string, unknown> | undefined {
  const w = profileWarning("worker", status.os, status.worker);
  if (w === undefined && caps.profiles.worker === "enforced") return undefined;
  if (w !== undefined) return { ...w, sandbox: caps.sandbox };
  // The profile could be enforced, but the job host's spawner does not apply it.
  const base = profileWarning("worker", status.os, { kind: "disabled", reason: "profileNotReady" });
  return { ...base, details: { ...base?.details, reason: "spawnerUnconfined" }, sandbox: caps.sandbox };
}

/**
 * `PLUGINS_UNCONFINED {enabled, reason}` (sp8b Decision 17): on every host while at least one
 * enabled code plugin would run unconfined, i.e. the `plugin` profile is not enforced here or the
 * job host's spawner does not apply it (`reason: spawnerUnconfined`). None while no code plugin
 * is enabled.
 */
export function pluginWarnings(caps: JobHostCaps, status: SandboxStatus, enabled: number): Array<Record<string, unknown>> {
  if (enabled === 0) return [];
  // The plugin profile's own warning: `PLUGINS_UNCONFINED`, or `SANDBOX_UNAVAILABLE` where an
  // enforced profile cannot apply (the plugin profile has no sockets warning).
  const own = sandboxWarnings(status, ["plugin"]).find((w) => w.code === "PLUGINS_UNCONFINED" || w.code === "SANDBOX_UNAVAILABLE");
  if (own !== undefined) return [{ ...own, details: { ...own.details, enabled } }];
  if (caps.profiles.plugin === "enforced") return [];
  const base = profileWarning("plugin", status.os, { kind: "disabled", reason: "profileNotReady" });
  return base === undefined ? [] : [{ ...base, details: { ...base.details, reason: "spawnerUnconfined", enabled } }];
}

export interface AuditPurgeRequest {
  readonly all?: boolean | undefined;
  readonly before?: number | undefined;
}

/** `server.status`: no secret, no key, no ticket; counts, endpoints and the job host's caps only. */
export function serverStatus(deps: AdminDeps) {
  return Effect.sync(() => {
    const caps = deps.jobCaps();
    const status = (deps.sandbox ?? currentSandboxStatus)();
    deps.sandboxReporter?.report(status);
    const sandbox = sandboxWarning(caps, status);
    return {
      version: SERVER_VERSION,
      protocol: PROTOCOL_VERSION,
      envId: deps.control.envId,
      ...(deps.control.environment === undefined ? {} : { env: deps.control.environment() }),
      pid: process.pid,
      listeners: [...deps.control.listeners()],
      connections: deps.connections.count(),
      journal: { enabled: deps.journalEnabled() },
      auditFailures: deps.audit.failures(),
      jobs: {
        caps: {
          killTree: caps.killTree,
          watchdog: caps.watchdog,
          cgroupMemory: caps.cgroupMemory,
          leafLimits: caps.leafLimits,
          sandbox: caps.sandbox,
          sandboxed: caps.sandboxed,
          profiles: caps.profiles,
          perJobGrants: caps.perJobGrants,
        },
      },
      // The job host's sandbox (5C-2), then plugins and hooks, then the agent runtime's: each
      // profile not enforced on this host warns (plan 7C Decision 23, owner decision F2).
      warnings: [
        ...(sandbox === undefined ? [] : [sandbox]),
        ...(deps.enabledCodePlugins === undefined ? (sandboxWarnings(status, ["plugin"]) as unknown as Array<Record<string, unknown>>) : pluginWarnings(caps, status, deps.enabledCodePlugins())),
        ...(sandboxWarnings(status, ["hook"]) as unknown as Array<Record<string, unknown>>),
        ...(agentWarnings(status) as Array<Record<string, unknown>>),
        // Where secrets are kept (5B Decision 4): a file store standing in for the keychain.
        ...(deps.control.warnings?.() ?? []),
        // Copied or moved server data waits for its owner (5B Decision 31).
        ...(deps.control.environment?.().state === "moved"
          ? [{ code: "ENV_MOVED", message: "this server data was copied or moved: paired devices wait until its owner adopts or resets it" }]
          : []),
      ],
    };
  });
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
