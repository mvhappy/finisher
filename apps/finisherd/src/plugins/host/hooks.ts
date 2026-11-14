// Plugin hook handlers (7F plan Task 10, Decisions 19, 20; sp8 spec §3.6; replaces the 8B plan's
// Task 17 finisherd half): the hook engine's plugin layer (`PluginHookSource`).
//
// - **Rows:** every enabled, active code plugin's `contributes.hooks`, read from the installed
//   rows at each resolution (an enable, a disable, a crash loop, a revocation or an update shows at
//   the next event: `digest` is one read of the plugin rows, called at every dispatch). A
//   version's manifest is read once (the store is fixed per version; `program.ts` reads it the
//   same way to start the process; install validated it). The engine orders them by plugin id,
//   then manifest order (`config.ts`).
// - **Who receives an event** (checked at every run, since grants change): the plugin is still
//   enabled and declares `(event, handler)`; a `hooks:observe` grant names the event; a developer
//   plugin (`source: dev`) never gets an `agent` or `mcp` principal's event (sp8b Decision 19);
//   the job's rights reach the event's project, and a server-scoped event (no project) reaches
//   only rights over every project (a person, `local`, `console`, the system): a project's agent
//   never sees the server's. Otherwise the handler does not run (`notReceived`), and no job is
//   submitted. `hooks:modify` grants nothing (Decision 19: no `modify` in v1).
// - **The run:** a `plugin.hook` v1 job (`jobs/kinds/plugin.hook.ts`) owned by the event's
//   principal, `via` the plugin, its rights the principal ∩ the grants (`rights.ts`; an agent's
//   branch reach its session's subtree; `systemInvoker` for system events), on the key's warm
//   process; params `HookRun {handler, request}` (a payload over `PLUGIN_HOOK_PAYLOAD_BYTES` sent
//   as `{payloadOmitted, bytes}`, so the job's start fits the envelope's 1 MiB line). Cancelled at
//   the manifest's `timeoutS` counted from the submit (a `timeout` failure), or when the server
//   stops. Its host calls pin no branch: `project.snapshot`/`project.view` are refused (the
//   handler has the event's payload; least privilege), `log` and a granted `net` work.
// - **The answer** is size-checked as a command hook's stdout (at most 64 KiB of JSON as sent,
//   before control characters are stripped: `tooLarge`), then decoded and checked as its reply
//   (`checkReply`: no `modify`);
//   a job error is a failure under Decision 3 by its code, never its text: `PLUGIN_TIMEOUT` a
//   `timeout`, `PLUGIN_CRASHED` an `exit`, the plugin's own answers (`PLUGIN_PROTOCOL
//   {unknownHook | badReply}`, its own error) `invalid`, anything else (not runnable, refused,
//   cancelled) `unavailable`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { type FinError, HOOK_CLASSES, HOOK_LIMITS, type HookEvent, type JobView } from "@finisher/contracts";
import type { HookContribution, HookRequest, JobRights, Permission } from "@finisher/core-types";
import { Effect } from "effect";
import type { JobEngine } from "../../jobs/api.ts";
import { HOOK_KIND } from "../../jobs/kinds/plugin.hook.ts";
import type { ConfiguredHook, PluginHookRow } from "../../hooks/config.ts";
import type { HookAt } from "../../hooks/engine.ts";
import { checkReply, type HookFailure, type HookFailureKind, type HookRun } from "../../hooks/process.ts";
import type { PluginHookSource } from "../../hooks/wiring.ts";
import type { InstalledFull, PluginTables } from "../tables.ts";
import { MANIFEST_FILE, storeDirOf } from "./program.ts";
import { type Invoker, reaches } from "./rights.ts";

/** A handler's timeout without one in its manifest, and the most it may give (`DEFAULT_HOOK_TIMEOUT_S`, `MAX_HOOK_TIMEOUT_S`). */
export const DEFAULT_HOOK_TIMEOUT_S = 10;
export const MAX_HOOK_TIMEOUT_S = 60;
/**
 * A request's payload larger than this (JSON bytes) is sent as `{payloadOmitted, bytes}`: a job's
 * params are at most 512 KiB (fin-node's `MAX_PARAMS_BYTES`, so a `start` fits its 1 MiB line).
 */
export const PLUGIN_HOOK_PAYLOAD_BYTES = 384 * 1024;
const HANDLER = /^[a-z][a-z0-9-]{0,63}$/;

export interface PluginHooksDeps {
  /** Links resolved. */
  readonly serverData: string;
  readonly tables: Pick<PluginTables, "installed" | "installedFull" | "grants">;
  /** `PluginHost.rightsOf`: the invoker ∩ the grants under its class's generation now. */
  readonly rightsOf: (invoker: Invoker, grants: ReadonlyArray<Permission>, sessionRoot?: string) => JobRights;
}

export interface PluginHooks extends PluginHookSource {
  /** The started job engine (`PluginHost.bind`); until then a handler's run is `unavailable`. */
  bind(engine: Pick<JobEngine, "submit" | "wait" | "cancel">): void;
}

const notReceived = (detail: string): HookRun => ({ notRun: "notReceived", detail });
const failure = (kind: HookFailureKind, detail: string, started = false): HookFailure => ({ kind, detail, ...(started ? { started: true as const } : {}) });

/** A manifest's hooks as validated at install; anything else there contributes none. */
function hooksOfManifest(raw: unknown): readonly HookContribution[] {
  const hooks = (raw as { contributes?: { hooks?: unknown } } | null)?.contributes?.hooks;
  if (!Array.isArray(hooks)) return [];
  const ok = hooks.every((h: Partial<HookContribution> | null) => {
    if (h === null || typeof h !== "object" || typeof h.event !== "string" || !Object.hasOwn(HOOK_CLASSES, h.event)) return false;
    if (typeof h.handler !== "string" || !HANDLER.test(h.handler)) return false;
    return h.timeoutS === undefined || (Number.isInteger(h.timeoutS) && h.timeoutS >= 1 && h.timeoutS <= MAX_HOOK_TIMEOUT_S);
  });
  return ok ? (hooks as HookContribution[]) : [];
}

const timeoutOfEntry = (c: HookContribution): number => Math.min(MAX_HOOK_TIMEOUT_S, c.timeoutS ?? DEFAULT_HOOK_TIMEOUT_S);

/** The failure a job error comes to, by its code (Decision 3); never the plugin's text. */
function jobFailure(e: { readonly code: string; readonly details?: unknown }): HookFailure {
  const reason = (e.details as { reason?: unknown } | null | undefined)?.reason;
  const detail = typeof reason === "string" && /^[A-Za-z][A-Za-z0-9]{0,63}$/.test(reason) ? `${e.code} ${reason}` : e.code;
  switch (e.code) {
    case "PLUGIN_TIMEOUT":
    case "JOB_TIMEOUT":
      return failure("timeout", detail);
    case "PLUGIN_CRASHED":
      return failure("exit", detail);
    case "PLUGIN_PROTOCOL":
    case "PLUGIN_COMMAND_FAILED":
      return failure("invalid", detail, true);
    default:
      return failure("unavailable", detail);
  }
}

/** `req` with a payload too large for one envelope line replaced, as a command hook's stdin does. */
function sized(req: HookRequest): HookRequest {
  const bytes = Buffer.byteLength(JSON.stringify(req.payload ?? null));
  return bytes > PLUGIN_HOOK_PAYLOAD_BYTES ? { ...req, payload: { payloadOmitted: true, bytes } } : req;
}

export function makePluginHooks(d: PluginHooksDeps): PluginHooks {
  let engine: Pick<JobEngine, "submit" | "wait" | "cancel"> | undefined;
  /** Each installed version's hooks (`<id>@<version>` and its folder), read once. */
  const manifests = new Map<string, readonly HookContribution[]>();

  const enabled = () => d.tables.installed().filter((r) => r.kind === "code" && r.enabled && r.state === "active");

  const hooksOf = (row: InstalledFull): readonly HookContribution[] => {
    const key = `${row.id}@${row.version}\0${row.dir}`;
    const kept = manifests.get(key);
    if (kept !== undefined) return kept;
    let hooks: readonly HookContribution[];
    try {
      hooks = hooksOfManifest(JSON.parse(readFileSync(join(storeDirOf(d.serverData, row), MANIFEST_FILE), "utf8")));
    } catch {
      // Unreadable now: no hooks, read again at the next resolution.
      return [];
    }
    manifests.set(key, hooks);
    return hooks;
  };

  const rowsOf = (): Array<{ readonly row: InstalledFull; readonly hooks: readonly HookContribution[] }> =>
    enabled()
      .map((r) => d.tables.installedFull(r.id))
      .filter((r): r is InstalledFull => r !== undefined && r.kind === "code" && r.enabled === true && r.state === "active")
      .map((row) => ({ row, hooks: hooksOf(row) }));

  /** Whether `at`'s event reaches `row`'s handlers under `rights` (see the module docs). */
  const receives = (row: InstalledFull, at: HookAt, rights: JobRights): string | undefined => {
    if (row.source === "dev" && (at.principal.kind === "agent" || at.principal.kind === "mcp")) return "a developer plugin never gets an agent's events";
    if (at.projectId === undefined) return rights.projects === "all" ? undefined : "a server-scoped event reaches only rights over every project";
    return reaches(rights.projects, at.projectId) ? undefined : "the event's project is out of the invoker's reach";
  };

  const runJob = async (e: NonNullable<typeof engine>, h: ConfiguredHook, row: InstalledFull, timeoutS: number, req: HookRequest, at: HookAt, rights: JobRights, signal?: AbortSignal): Promise<HookRun> => {
    let job: JobView;
    try {
      job = await Effect.runPromise(
        e.submit(
          { kind: at.principal.kind, id: at.principal.id },
          { kind: HOOK_KIND, params: { handler: h.id, request: sized(req) }, ...(at.projectId === undefined ? {} : { project: at.projectId }), via: { plugin: `${row.id}@${row.version}` }, rights, priority: "interactive", maxAttempts: 1 },
        ),
      );
    } catch (x) {
      return failure("unavailable", (x as Partial<FinError>).code ?? "submit failed");
    }
    const cancel = () => void Effect.runPromise(Effect.ignore(e.cancel(job.id)));
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      if (signal?.aborted === true) cancel();
      const ended = await Effect.runPromise(Effect.result(e.wait([job.id], { timeoutMs: timeoutS * 1000 })));
      if (ended._tag === "Failure") {
        cancel();
        return ended.failure.code === "WAIT_TIMEOUT" ? failure("timeout", `no answer within ${timeoutS} s`) : failure("unavailable", ended.failure.code);
      }
      const done = ended.success[0];
      if (done?.state === "succeeded") {
        // The reply as sent, before decoding and stripping: a command hook's stdout cap.
        const bytes = Buffer.byteLength(JSON.stringify(done.result ?? null) ?? "");
        if (bytes > HOOK_LIMITS.stdoutBytes) return failure("tooLarge", `a reply over ${HOOK_LIMITS.stdoutBytes} bytes`, true);
        const r = checkReply(done.result);
        return "kind" in r ? { ...r, started: true } : r;
      }
      if (done?.error !== undefined) return jobFailure(done.error);
      return failure("unavailable", signal?.aborted === true ? "the server is stopping" : `the job ended ${done?.state ?? "unknown"}`);
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  };

  return {
    bind(e) {
      engine = e;
    },
    digest: () =>
      enabled()
        .map((r) => `${r.id}@${r.version}:${r.source}`)
        .join("\n"),
    rows: (): PluginHookRow[] => rowsOf().flatMap(({ row, hooks }) => hooks.map((c) => ({ plugin: row.id, event: c.event as HookEvent, handler: c.handler, timeoutS: timeoutOfEntry(c) }))),
    timeoutOf: (plugin, event, handler) => {
      const at = plugin.lastIndexOf("@");
      const row = at <= 0 ? undefined : d.tables.installedFull(plugin.slice(0, at));
      const c = row === undefined || `${row.id}@${row.version}` !== plugin ? undefined : hooksOf(row).find((x) => x.event === event && x.handler === handler);
      return c === undefined ? MAX_HOOK_TIMEOUT_S : timeoutOfEntry(c);
    },
    run: async (h, req, at, signal) => {
      const id = h.plugin;
      const row = id === undefined ? undefined : d.tables.installedFull(id);
      if (row === undefined || row.kind !== "code" || row.enabled !== true || row.state !== "active") return notReceived("the plugin is not enabled");
      const entry = hooksOf(row).find((c) => c.event === h.event && c.handler === h.id);
      if (entry === undefined) return notReceived("the plugin does not declare this handler");
      const grants = d.tables.grants(row.id).map((g) => JSON.parse(g) as Permission);
      if (!grants.some((g) => g.perm === "hooks:observe" && g.hooks.includes(h.event))) return notReceived("hooks:observe for the event is not granted");
      const rights = d.rightsOf(at.principal, grants, at.principal.kind === "agent" ? at.session?.rootBranch : undefined);
      const refused = receives(row, at, rights);
      if (refused !== undefined) return notReceived(refused);
      if (engine === undefined) return failure("unavailable", "the job engine is not running");
      return runJob(engine, h, row, timeoutOfEntry(entry), req, at, rights, signal);
    },
  };
}
