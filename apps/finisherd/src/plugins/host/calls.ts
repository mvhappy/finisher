// The host API 1.1 a plugin process reaches through `call {id, method, params}` (sp8b plan
// Task 7; spec §3.6; the `plugin` family's `onCall`). Every call, in order:
// 1. names a method of host API 1.1 (a later batch's is `INVALID_REQUEST {method,
//    availableFrom}`, any other name `INVALID_REQUEST {method, reason: unknownMethod}`) and
//    decodes its params with the `fin-plugin-api` type the generated schema comes from
//    (`PLUGIN_PROTOCOL {method, error}` otherwise; the process is not killed);
// 2. runs under its own job's authority (spec §3.3, Decision 28): the key comes from the job
//    the call belongs to, never from the request; a process whose key is no longer the job's
//    (its class's policy generation moved on) is answered `PLUGIN_AUTHORITY_RETIRED`, which
//    a caller retries under the new key;
// 3. narrows the job's rights by the live invoker (`narrowRights`: revoked → `CREDENTIAL_REVOKED`)
//    and checks the method's permission and reach (`authorizeMethod`, Task 6) against the
//    job's project and branch;
// 4. then acts: `project.snapshot` pages the project's canonical JSON (`PAGE_BYTES` a page
//    before base64) from one snapshot pinned for the job, so every page and view of one job
//    has the same `rev`: a command's is the snapshot `commands.ts` read at submit (`hold`),
//    whose rev is its `CommandRun.rev` and its transaction's `baseRev` (Task 9); any other
//    job's is the head at its first read (fin-node reads a branch at its head only);
//    `project.view` renders `fin view`'s screenplay of the same snapshot, capped at
//    `VIEW_BYTES`; `log` goes to the server log (`log.ts`); `net.fetch` and `net.read` go to
//    the egress broker (`egress.ts`, Task 8) with the job's approved `net` origins.
import { FinError, type JobView } from "@finisher/contracts";
import type { HostMethod, NetFetch, NetRead } from "@finisher/core-types";
import { decodePluginCall, pluginProjectView } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import { type CallCtx, type OnCall, projectOfJob } from "../../jobs/kinds.ts";
import { bareKey } from "./keys.ts";
import type { Egress } from "./egress.ts";
import type { PluginLog } from "./log.ts";
import { authorizeMethod, type ReachDeps } from "./reach.ts";
import { type LiveInvoker, narrowRights } from "./rights.ts";

/** A snapshot page's bytes before base64: its base64 and the envelope fit 5C's 1 MiB line. */
export const PAGE_BYTES = 768_000;
/** The largest `project.view` text, as JSON-encoded bytes. */
export const VIEW_BYTES = 768 * 1024;

/** What the calls read, bound once the server's pieces exist (`PluginHost.serve`). */
export interface CallDeps {
  readonly live: LiveInvoker;
  readonly reach: ReachDeps;
  /** The project's branch now: `{rev, project}` (the registry's `snapshot`). */
  readonly snapshot: (projectId: string, branch: string) => Effect.Effect<{ readonly rev: number; readonly project: unknown }, FinError>;
}

export interface HostCallsDeps {
  /** The bound deps; `undefined` until the server serves calls (`PLUGIN_RUNTIME_UNAVAILABLE`). */
  readonly deps: () => CallDeps | undefined;
  /** The key `job` runs under now (`PluginHost.warmKeyOf`). */
  readonly keyNow: (job: JobView) => string;
  /** The key a cold process of `jobId` was started under. */
  readonly coldKey: (jobId: string) => string | undefined;
  readonly log: PluginLog;
  /** The egress broker (`net.fetch`, `net.read`). */
  readonly egress: Egress;
}

/** A snapshot a job reads: its rev, its history generation and the project's canonical JSON. */
export interface Pinned {
  readonly rev: number;
  readonly generation?: unknown;
  readonly bytes: Buffer;
  readonly json: string;
}

/** `project`'s branch at `rev` as `pinned()` keeps it. */
export function pinOf(s: { readonly rev: number; readonly project: unknown; readonly generation?: unknown }): Pinned {
  const json = JSON.stringify(s.project);
  return { rev: s.rev, ...(s.generation === undefined ? {} : { generation: s.generation }), json, bytes: Buffer.from(json, "utf8") };
}

const heldKey = (project: string, branch: string, rev: number): string => `${project}\u0000${branch}\u0000${rev}`;

/** A command job's `rev` (its `CommandRun.rev`), when its params carry one. */
function revOfJob(job: JobView): number | undefined {
  const r = (job.params as { rev?: unknown } | null)?.rev;
  return typeof r === "number" ? r : undefined;
}

export function retired(): FinError {
  return finError(
    "PLUGIN_AUTHORITY_RETIRED",
    "this plugin process serves an authority that was retired (a credential or agent session changed)",
    { retryable: true },
    "Run the command again: it starts under the current authority.",
  );
}

function protocolError(method: string, error: string): FinError {
  return finError("PLUGIN_PROTOCOL", `the ${method} call's params do not decode: ${error}`, { method, error });
}

/** The branch a call of `job` reads: its command's `branch` (Task 9's `CommandRun`). */
function branchOfJob(job: JobView): string | undefined {
  const b = (job.params as { branch?: unknown } | null)?.branch;
  return typeof b === "string" ? b : undefined;
}

/** `text` cut to whole lines so that its JSON encoding stays within `VIEW_BYTES`. */
function capView(lines: ReadonlyArray<string>): string {
  const out: string[] = [];
  let size = 2;
  for (const l of lines) {
    const n = Buffer.byteLength(JSON.stringify(l)) - 1; // the line's escapes plus its newline
    if (size + n > VIEW_BYTES - 64) {
      out.push(`… ${lines.length - out.length} more lines cut at ${VIEW_BYTES} bytes`);
      break;
    }
    out.push(l);
    size += n;
  }
  return out.join("\n");
}

export function makeHostCalls(h: HostCallsDeps) {
  /** Each job's pinned snapshot, released when the job ends (`release`). */
  const pins = new Map<string, Promise<Pinned>>();
  /**
   * Snapshots a command run read when it was submitted (Task 9, `hold`), by project, branch
   * and rev, with how many runs hold each: a job of that `CommandRun` reads exactly it.
   */
  const held = new Map<string, { pin: Pinned; count: number }>();

  const pinned = (deps: CallDeps, job: JobView, project: string, branch: string): Effect.Effect<Pinned, FinError> =>
    Effect.suspend(() => {
      let p = pins.get(job.id);
      const rev = revOfJob(job);
      const seeded = rev === undefined ? undefined : held.get(heldKey(project, branch, rev));
      if (p === undefined && seeded !== undefined) {
        p = Promise.resolve(seeded.pin);
        pins.set(job.id, p);
      }
      if (p === undefined) {
        p = Effect.runPromise(Effect.map(deps.snapshot(project, branch), pinOf));
        pins.set(job.id, p);
        // A failed read is not kept: the next call reads again.
        p.catch(() => pins.get(job.id) === p && pins.delete(job.id));
      }
      const got = p;
      return Effect.tryPromise({ try: () => got, catch: (e) => toFinError(e) });
    });

  const act = (deps: CallDeps, ctx: CallCtx, method: HostMethod, params: Record<string, unknown>, branch: string | undefined): Effect.Effect<unknown, FinError> =>
    Effect.gen(function* () {
      const job = ctx.job;
      const plugin = ctx.via?.plugin ?? "";
      switch (method) {
        case "log": {
          h.log.call(ctx.workerKey ?? `job:${job.id}`, plugin, String(params["level"]), String(params["msg"]));
          return {};
        }
        case "project.snapshot":
        case "project.view": {
          const project = projectOfJob(job);
          if (project === undefined || branch === undefined) {
            return yield* Effect.fail(finError("INVALID_REQUEST", `${method} needs a job of a project and branch`, { method, reason: "noProject" }));
          }
          const pin = yield* pinned(deps, job, project, branch);
          if (method === "project.view") {
            const range = typeof params["range"] === "string" ? params["range"] : undefined;
            const budget = typeof params["budget"] === "number" ? params["budget"] : undefined;
            const v = yield* Effect.try({ try: () => pluginProjectView(pin.json, range, budget), catch: (e) => toFinError(e) });
            return { text: capView(v.lines) };
          }
          const pages = Math.max(1, Math.ceil(pin.bytes.length / PAGE_BYTES));
          const page = typeof params["page"] === "number" ? params["page"] : 0;
          if (page >= pages) return yield* Effect.fail(finError("INVALID_REQUEST", `page ${page} is past the snapshot's ${pages} pages`, { page, pages }));
          const bytes = pin.bytes.subarray(page * PAGE_BYTES, (page + 1) * PAGE_BYTES);
          return { bytesB64: bytes.toString("base64"), page, pages, rev: pin.rev };
        }
        case "net.fetch": {
          // The approved origins are the job's `net` grants (narrowing keeps grants as they are).
          const origins = (ctx.rights?.grants ?? []).flatMap((g) => (g.perm === "net" ? g.origins : []));
          const call = { jobId: job.id, plugin, origins };
          return yield* Effect.tryPromise({ try: () => h.egress.fetch(call, params as unknown as NetFetch), catch: (e) => toFinError(e) });
        }
        case "net.read":
          return yield* Effect.try({ try: () => h.egress.read(job.id, params as unknown as NetRead), catch: (e) => toFinError(e) });
      }
    });

  const onCall: OnCall = (ctx, method, raw) =>
    Effect.gen(function* () {
      const decoded = decodePluginCall(method, raw);
      if (!decoded.ok) {
        if ("reserved" in decoded) {
          return yield* Effect.fail(finError("INVALID_REQUEST", `${method} is not part of host API 1.1`, { method, availableFrom: decoded.reserved }));
        }
        if ("unknown" in decoded) return yield* Effect.fail(finError("INVALID_REQUEST", `there is no host method ${method}`, { method, reason: "unknownMethod" }));
        return yield* Effect.fail(protocolError(method, decoded.error));
      }
      const job = ctx.job;
      if (ctx.role !== "worker" || ctx.via === undefined || ctx.rights === undefined) {
        return yield* Effect.fail(finError("PLUGIN_PROTOCOL", "host calls come from a plugin job's worker", { method, reason: "notPluginJob" }));
      }
      // The authority is the job's: the process's key must still be the key the job runs under.
      // A process with no known key (neither a warm worker's tag nor a cold spawn's record) is
      // refused too: the check fails closed (review of Task 7).
      const key = ctx.workerKey !== undefined ? bareKey(ctx.workerKey) : h.coldKey(job.id);
      if (key === undefined || key !== h.keyNow(job)) return yield* Effect.fail(retired());
      const deps = h.deps();
      if (deps === undefined) return yield* Effect.fail(finError("PLUGIN_RUNTIME_UNAVAILABLE", "this server does not serve plugin calls yet", { method }));
      const narrowed = narrowRights(ctx.rights, deps.live(ctx.owner));
      if (narrowed instanceof FinError) return yield* Effect.fail(narrowed);
      const m = method as HostMethod;
      const branch = yield* authorizeMethod(narrowed, m, { project: projectOfJob(job), branch: branchOfJob(job) }, deps.reach);
      return yield* act(deps, ctx, m, decoded.params as Record<string, unknown>, branch ?? branchOfJob(job));
    });

  return {
    onCall,
    /**
     * Holds `pin` (`project`'s `branch` read when a command was submitted, Task 9) until the
     * returned release: a job whose `CommandRun` names that project, branch and rev reads it,
     * so the rev a command commits from is every page's rev.
     */
    hold(project: string, branch: string, pin: Pinned): () => void {
      const k = heldKey(project, branch, pin.rev);
      const h = held.get(k);
      if (h === undefined) held.set(k, { pin, count: 1 });
      else h.count++;
      let done = false;
      return () => {
        if (done) return;
        done = true;
        const now = held.get(k);
        if (now !== undefined && --now.count <= 0) held.delete(k);
      };
    },
    /** How many submitted snapshots are held (tests). */
    heldSnapshots: (): number => held.size,
    /** `jobId` ended: its pinned snapshot and its held response bodies go. */
    release(jobId: string): void {
      pins.delete(jobId);
      h.egress.release(jobId);
    },
    /** How many jobs hold a pinned snapshot (tests). */
    pinnedJobs: (): number => pins.size,
  };
}

export type HostCalls = ReturnType<typeof makeHostCalls>;
