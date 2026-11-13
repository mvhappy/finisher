// The hook engine (7F plan Task 4; Decisions 1, 3, 4, 5, 8, 18, 21; spec §9.1): which hooks run
// for an event, in what order, and what their replies come to.
//
// - **Order** (Decision 4): managed, user, project, local, plugin; file order within a layer
//   (`HookConfig.hooks`). `match` filters by the names the source gives (`HookAt.names`: the
//   call's command names, a job kind, `status.<s>`/`budget.<kind>`, an approval kind; a hook
//   with a `match` and no names does not run); `principals` filters `PreCommand`/`PostCommand`
//   (`agent` by default: only an agent's calls; `all`: everyone's).
// - **The chain** runs sequentially. By the event's class (Decision 1): `blockingBefore` stops
//   at the first block and answers it (`HookOutcome.blocked`: the caller refuses the action,
//   `HOOK_BLOCKED`, audited `hook.block`); `blockingAfter` (`PostCommand`) stops at the first
//   block, audited `{after: true}`, its reason a notice and handed to `SessionNotices.deliver`
//   (the command stands); `awaited` and `observe` ignore a block with an `ignoredBlock` notice,
//   counted, and go on. `context` texts join with a blank line, at most 64 KiB in all; a later
//   one that does not fit is dropped with a `contextDropped` notice.
// - **Failures** (Decision 3): a timeout or any other failure follows the hook's `onTimeout`
//   (default `allow`, `block` for `PreMerge`): `block` answers `reason: "timeout" | "failed"`.
//   A timeout writes a `HOOK_TIMEOUT` notice, any other failure a `failed` notice with the
//   failure's kind, detail and the start of its stderr (at most 512 bytes, `process.ts`,
//   control characters stripped: a person reads it in the session). A hook that no longer
//   applies when its process would start (`HookNotRun`, the `applies` re-check) is skipped,
//   not a failure. An unreadable layer's synthetic hook (Decision 8) blocks with `reason:
//   "unreadable"`. An event in a project this server does not know fails each command hook
//   (`unavailable`).
// - **Off the caller's path** (`observe`: `PostCommand` and the observe-only events): one FIFO
//   per server, one event at a time, at most `HOOK_LIMITS.queue` waiting (the next is dropped
//   and counted). An event with no hook when it is observed is not queued (it takes no slot);
//   a queued event's hooks are resolved again when it is taken off the queue, so a
//   configuration change while it waits applies to it. Its outcome (context, a `PostCommand` block) goes to
//   `SessionNotices.deliver` (Task 6 steers or holds it).
// - **Freshness and cost** (Decision 5): every dispatch `stat`s each configuration file that
//   applies (managed, user, the local layer's, and the project's only while its trust record
//   holds a `hooksHash` other than `none`) and compares their `(ino, size, mtime, ctime)`, the
//   trusted hash, the project's folder and the plugins' digest with the per-project
//   `EventIndex` it keeps: unchanged, an event costs that and one `Map.get`, never a read, a
//   parse or a process. Anything changed re-resolves through `HookConfig` (which reads only
//   what changed). Not kept: a file changed within `RACY_MS` (re-resolved at every dispatch until
//   it is older), and a trusted project file whose hooks do not apply as trusted (a bound script
//   changed: re-checked at each dispatch until it is restored or re-trusted). A project script
//   edited while the index is kept is caught by the re-check before its process starts.
// - **The `fin` credential** (Task 5, Decision 11): a `fin: true` command hook's run takes its
//   credential from `credentials` (minted on first use, one per hook and project); every
//   resolution tells `credentials.retain` what applies now, so a hook that changed or is gone has
//   its credential ended at the next dispatch that sees it. **Recursion guard:** an event whose
//   principal is a hook credential held now runs no hook (`fire` answers `{ran: 0}`, `observe`
//   queues nothing).
// - **Sandbox warning:** a hook that runs unconfined (`direct`) is logged once per server start
//   (`server.status` lists the `hook` profile's warning all along).
// - **Closing** (`close`, at server stop): the queue is dropped, running processes are killed
//   (their group, as at a deadline) and waited for, so their temps are removed; afterwards
//   nothing starts and each hook a dispatch finds is an `unavailable` failure under its
//   `onTimeout` (`PreMerge` blocks, the rest pass). `buildHooks` sweeps temps a crash left.
// - **Stats** (Decision 21): per event `runs` (hooks that started: a process, or a plugin
//   handler that answered; not a slot timeout, a missing program or a refused start), `blocks` (every block reply,
//   ignored ones included, and failures resolved to a block), `timeouts`, `failures`; `dropped`;
//   `unreadable` layers.
import { statSync } from "node:fs";
import { type FinError, HOOK_CLASSES, HOOK_LIMITS, type HookEvent, type HookLayer, type Principal, type Session } from "@finisher/contracts";
import type { HookRequest, HookSession } from "@finisher/core-types";
import { Effect } from "effect";
import { RACY_MS } from "../agent/file-cache.ts";
import { globMatch } from "../agent/glob.ts";
import type { AuditInput } from "../control/audit.ts";
import { finError } from "../errors.ts";
import type { ConfiguredHook, HookConfig } from "./config.ts";
import type { HookCredentialPort } from "./credential.ts";
import { type HookFailure, type HookFailureKind, type HookProcessDeps, type HookRun, HookSlots, isCommandHook, isNotRun, runCommandHook } from "./process.ts";

/** Where an event happens and whose it is. */
export interface HookAt {
  readonly projectId: string;
  readonly session?: Session;
  /** The caller (a block is audited as theirs; `principals` filters on its kind). */
  readonly principal: Principal;
  /** What a hook's `match` is tested against (Decision 6); none: hooks with a `match` do not run. */
  readonly names?: readonly string[];
}

export interface HookBlock {
  readonly hook: string;
  readonly reason: string;
  readonly layer: HookLayer;
}

export interface HookOutcome {
  readonly blocked?: HookBlock;
  readonly context?: string;
  /** The hooks whose context is in `context`, in order. */
  readonly contextHooks?: readonly string[];
  /** Hooks that started (`HookStats.runs`). */
  readonly ran: number;
}

/** What the engine writes into the session of an event (Task 6 turns them into `notice` nodes). */
export type HookNotice =
  | { readonly code: "HOOK_TIMEOUT"; readonly hook: string; readonly event: HookEvent; readonly layer: HookLayer; readonly timeoutS: number }
  | { readonly hook: HookNoticeOf<{ readonly failed: HookFailureKind; readonly detail: string; readonly stderr?: string }> }
  | { readonly hook: HookNoticeOf<{ readonly ignoredBlock: string }> }
  | { readonly hook: HookNoticeOf<{ readonly blocked: string }> }
  | { readonly hook: HookNoticeOf<{ readonly contextDropped: number }> };
type HookNoticeOf<T> = { readonly event: HookEvent; readonly id: string; readonly layer: HookLayer } & T;

/** The session side (Task 6 fills it; a no-op until then). */
export interface SessionNotices {
  notice(at: HookAt, notice: HookNotice): void;
  /** An outcome of an event run off the caller's path that carries context or a `PostCommand` block. */
  deliver(at: HookAt, event: HookEvent, outcome: HookOutcome): void;
}

/** The plugin layer (Task 10 fills it): one handler's run for a request. */
export interface PluginHookPort {
  run(h: ConfiguredHook, req: HookRequest, at: HookAt): Promise<HookRun>;
  /** Changes whenever the enabled plugins' hook rows do. */
  digest(): string;
}

export interface HookCounters {
  runs: number;
  blocks: number;
  timeouts: number;
  failures: number;
}

export interface HookStats {
  readonly events: Readonly<Record<HookEvent, Readonly<HookCounters>>>;
  readonly dropped: number;
  readonly unreadable: readonly HookLayer[];
}

export interface HookEngine {
  /** Blocking and awaited events: the caller waits for the outcome. */
  fire(event: HookEvent, at: HookAt, payload: unknown): Effect.Effect<HookOutcome>;
  /** `PostCommand` and observe-only events: queued; hooks resolved when it runs. */
  observe(event: HookEvent, at: HookAt, payload: unknown): void;
  stats(): HookStats;
}

/** A file's identity as the index keys it. */
export interface FileKey {
  readonly ino: number;
  readonly size: number;
  readonly mtimeMs: number;
  readonly ctimeMs: number;
}

export interface HookEngineDeps {
  readonly config: HookConfig;
  readonly serverData: string;
  readonly audit: { append(e: AuditInput): void };
  readonly notices?: SessionNotices;
  readonly plugins?: PluginHookPort;
  /** `fin` hooks' credentials and the recursion guard (absent: a `fin` hook fails `unavailable`). */
  readonly credentials?: HookCredentialPort;
  /** The process's other deps (the sandbox status, launcher, ACL broker; tests). */
  readonly process?: Partial<Omit<HookProcessDeps, "serverData" | "slots" | "recheck" | "onStart" | "credential">>;
  /** The server's process slots (default: `HOOK_LIMITS.processes`). */
  readonly slots?: HookSlots;
  /** A configuration file's key, `undefined` when there is none (tests count calls). */
  readonly stat?: (path: string) => FileKey | undefined;
  readonly log?: (message: string) => void;
}

/** The engine as `buildHooks` holds it: also waits for the queue (tests) and closes. */
export interface HookEngineImpl extends HookEngine {
  /** Resolves once the observe queue is empty and nothing runs. */
  settled(): Promise<void>;
  /** Drops what waits, stops taking events, and waits (at most 2 s) for the one running. */
  close(): Promise<void>;
}

const LAYERS: readonly HookLayer[] = ["managed", "user", "project", "local", "plugin"];
const EVENTS = Object.keys(HOOK_CLASSES) as HookEvent[];
/** Bytes of a failure's stderr a notice carries (`process.ts` keeps no more). */
const STDERR_BYTES = 512;
const CONTROL = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g;
/** Kept indexes at most (past it the memo starts again). */
const MEMO_MAX = 1024;

/** `HOOK_BLOCKED {hook, reason}` (exit 7) for a block that refuses the action. */
export function hookBlockedError(event: HookEvent, b: HookBlock): FinError {
  return finError("HOOK_BLOCKED", `the ${event} hook ${b.hook} blocked this: ${b.reason}`, { hook: b.hook, reason: b.reason, event, layer: b.layer }, "see the hook's reason; a person changes or removes the hook");
}

/** Whether a failure answers `block` (Decision 3): the hook's `onTimeout`, else `PreMerge`'s default. */
const failsClosed = (h: ConfiguredHook, event: HookEvent) => (h.spec?.onTimeout ?? (event === "PreMerge" ? "block" : "allow")) === "block";

const sessionOf = (s: Session): HookSession => ({ id: s.id, role: s.role, mode: s.mode, harness: s.harness, ...(s.rootBranch === undefined ? {} : { rootBranch: s.rootBranch }) });

const defaultStat = (p: string): FileKey | undefined => statSync(p, { throwIfNoEntry: false });

interface Memo {
  readonly key: string;
  readonly index: ReadonlyMap<HookEvent, readonly ConfiguredHook[]>;
}

interface Queued {
  readonly event: HookEvent;
  readonly at: HookAt;
  readonly payload: unknown;
}

class Engine implements HookEngineImpl {
  readonly #d: HookEngineDeps;
  readonly #slots: HookSlots;
  readonly #stat: (path: string) => FileKey | undefined;
  readonly #memo = new Map<string, Memo>();
  readonly #unreadable = new Map<string, readonly HookLayer[]>();
  readonly #counters = Object.fromEntries(EVENTS.map((e) => [e, { runs: 0, blocks: 0, timeouts: 0, failures: 0 }])) as Record<HookEvent, HookCounters>;
  readonly #queue: Queued[] = [];
  /** Aborted by `close`: running processes are killed, none starts after it. */
  readonly #stopping = new AbortController();
  /** Hook runs under way (`close` waits for them, so their temps are removed). */
  readonly #running = new Set<Promise<unknown>>();
  #draining: Promise<void> | undefined;
  #dropped = 0;
  #warned = false;
  #closed = false;

  constructor(d: HookEngineDeps) {
    this.#d = d;
    this.#slots = d.slots ?? new HookSlots();
    this.#stat = d.stat ?? defaultStat;
  }

  fire(event: HookEvent, at: HookAt, payload: unknown): Effect.Effect<HookOutcome> {
    return Effect.suspend(() => {
      if (this.#d.credentials?.isHook(at.principal) === true) return Effect.succeed({ ran: 0 });
      const hooks = this.#applying(event, at);
      return hooks.length === 0 ? Effect.succeed({ ran: 0 }) : Effect.promise(() => this.#chain(event, at, payload, hooks));
    });
  }

  observe(event: HookEvent, at: HookAt, payload: unknown): void {
    if (this.#closed || this.#d.credentials?.isHook(at.principal) === true) return;
    // No hook for it now: it takes no queue slot (a hook added meanwhile does not see it).
    if (this.#applying(event, at).length === 0) return;
    if (this.#draining !== undefined) {
      if (this.#queue.length >= HOOK_LIMITS.queue) this.#dropped += 1;
      else this.#queue.push({ event, at, payload });
      return;
    }
    this.#draining = this.#drain({ event, at, payload });
  }

  async #drain(first: Queued): Promise<void> {
    // Never settles within `observe`: `#draining` is set before this clears it.
    await null;
    for (let item: Queued | undefined = first; item !== undefined && !this.#closed; item = this.#queue.shift()) {
      try {
        // Resolved now, not when it was queued (Decision 4).
        const hooks = this.#applying(item.event, item.at);
        if (hooks.length === 0) continue;
        const out = await this.#chain(item.event, item.at, item.payload, hooks);
        if (out.context !== undefined || out.blocked !== undefined) this.#d.notices?.deliver(item.at, item.event, out);
      } catch (e) {
        this.#d.log?.(`hooks: ${item.event} failed: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
    this.#draining = undefined;
  }

  async settled(): Promise<void> {
    while (this.#draining !== undefined) await this.#draining;
  }

  async close(): Promise<void> {
    this.#closed = true;
    this.#queue.length = 0;
    // Kills the running processes (their group, as at a deadline); their temps are removed as they end.
    this.#stopping.abort();
    await Promise.allSettled([...this.#running]);
    const draining = this.#draining;
    if (draining !== undefined) await Promise.race([draining, new Promise((r) => setTimeout(r, 2_000).unref())]);
  }

  stats(): HookStats {
    // The server layers as they are now; each project's local layer as its last dispatch saw it.
    this.#index(undefined);
    const seen = new Set<HookLayer>(this.#unreadable.get("") ?? []);
    for (const [slot, layers] of this.#unreadable) if (slot !== "" && layers.includes("local")) seen.add("local");
    return {
      events: Object.fromEntries(EVENTS.map((e) => [e, { ...this.#counters[e] }])) as Record<HookEvent, HookCounters>,
      dropped: this.#dropped,
      unreadable: LAYERS.filter((l) => seen.has(l)),
    };
  }

  /** The hooks that run for `event` at `at`, from the kept index (Decision 5). */
  #applying(event: HookEvent, at: HookAt): readonly ConfiguredHook[] {
    const all = this.#index(at.projectId).get(event);
    if (all === undefined) return [];
    const commands = event === "PreCommand" || event === "PostCommand";
    return all.filter((h) => {
      if (commands && at.principal.kind !== "agent" && h.spec?.principals !== "all") return false;
      const match = h.spec?.match;
      return match === undefined || (at.names ?? []).some((n) => globMatch(match, n));
    });
  }

  /** The event index of a project (or of the server layers alone), re-resolved when a key changed. */
  #index(projectId: string | undefined): ReadonlyMap<HookEvent, readonly ConfiguredHook[]> {
    const slot = projectId ?? "";
    const src = this.#d.config.sources(projectId);
    const now = Date.now();
    let racy = false;
    const parts: string[] = [src.trusted ?? "", src.dir ?? "", this.#d.plugins?.digest() ?? ""];
    for (const p of src.paths) {
      let st: FileKey | undefined;
      try {
        st = this.#stat(p);
      } catch (e) {
        parts.push(`error:${String((e as { code?: unknown }).code ?? e)}`);
        continue;
      }
      if (st === undefined) {
        parts.push("absent");
        continue;
      }
      parts.push(`${st.ino}:${st.size}:${st.mtimeMs}:${st.ctimeMs}`);
      if (Math.max(st.mtimeMs, st.ctimeMs) >= now - RACY_MS) racy = true;
    }
    const key = parts.join("\0");
    const kept = this.#memo.get(slot);
    if (kept?.key === key) return kept.index;
    const resolved = this.#d.config.resolve(projectId);
    this.#d.credentials?.retain(projectId, resolved.hooks);
    const index = new Map<HookEvent, ConfiguredHook[]>();
    for (const h of resolved.hooks) {
      const list = index.get(h.event);
      if (list === undefined) index.set(h.event, [h]);
      else list.push(h);
    }
    if (this.#unreadable.size >= MEMO_MAX) this.#unreadable.clear();
    this.#unreadable.set(slot, [...new Set(resolved.hooks.filter((h) => h.unreadable === true).map((h) => h.layer))]);
    if (racy || resolved.projectChanged) {
      this.#memo.delete(slot);
    } else {
      if (this.#memo.size >= MEMO_MAX) this.#memo.clear();
      this.#memo.set(slot, { key, index });
    }
    return index;
  }

  /** Runs `hooks` for one event in order (see the module docs). */
  async #chain(event: HookEvent, at: HookAt, payload: unknown, hooks: readonly ConfiguredHook[]): Promise<HookOutcome> {
    const cls = HOOK_CLASSES[event];
    const counters = this.#counters[event];
    const dir = this.#d.config.dirOf(at.projectId);
    const req: HookRequest = { event, v: 1, ...(at.session === undefined ? {} : { session: sessionOf(at.session) }), project: { id: at.projectId, dir: dir ?? "" }, payload };
    const texts: string[] = [];
    const ids: string[] = [];
    let bytes = 0;
    let ran = 0;
    const outcome = (blocked?: HookBlock): HookOutcome => ({
      ran,
      ...(blocked === undefined ? {} : { blocked }),
      ...(texts.length === 0 ? {} : { context: texts.join("\n\n"), contextHooks: [...ids] }),
    });
    for (const h of hooks) {
      const tag = { event, id: h.id, layer: h.layer };
      let reason: string;
      if (h.unreadable === true) {
        reason = "unreadable";
      } else {
        const r = await this.#run(h, req, at, dir);
        if (r === undefined || isNotRun(r)) continue;
        if (!("kind" in r) || r.started === true) {
          ran += 1;
          counters.runs += 1;
        }
        if ("kind" in r) {
          this.#failed(h, at, r, tag);
          if (!failsClosed(h, event)) continue;
          reason = r.kind === "timeout" ? "timeout" : "failed";
        } else if (r.reply === "context") {
          const size = Buffer.byteLength(r.text);
          const sep = texts.length === 0 ? 0 : 2;
          if (bytes + sep + size > HOOK_LIMITS.contextBytes) {
            this.#d.notices?.notice(at, { hook: { ...tag, contextDropped: size } });
            continue;
          }
          bytes += sep + size;
          texts.push(r.text);
          ids.push(h.id);
          continue;
        } else if (r.reply === "block") {
          reason = r.reason;
        } else {
          continue;
        }
      }
      counters.blocks += 1;
      if (cls === "awaited" || cls === "observe") {
        this.#d.notices?.notice(at, { hook: { ...tag, ignoredBlock: reason } });
        continue;
      }
      this.#audit(h, at, event, cls === "blockingAfter");
      if (cls === "blockingAfter") this.#d.notices?.notice(at, { hook: { ...tag, blocked: reason } });
      return outcome({ hook: h.id, reason, layer: h.layer });
    }
    return outcome();
  }

  /**
   * One hook's run: its process, or the plugin layer's handler (none until Task 10: skipped).
   * A run that rejects is an `unavailable` failure (Decision 3), never a defect of the caller.
   */
  async #run(h: ConfiguredHook, req: HookRequest, at: HookAt, dir: string | undefined): Promise<HookRun | undefined> {
    // After `close`: nothing starts; the failure follows `onTimeout` (`PreMerge` blocks).
    if (this.#closed) return { kind: "unavailable", detail: "the server is stopping" } satisfies HookFailure;
    const run = this.#start(h, req, at, dir);
    this.#running.add(run);
    try {
      return await run;
    } catch (e) {
      return { kind: "unavailable", detail: e instanceof Error ? e.message : String(e) } satisfies HookFailure;
    } finally {
      this.#running.delete(run);
    }
  }

  #start(h: ConfiguredHook, req: HookRequest, at: HookAt, dir: string | undefined): Promise<HookRun | undefined> {
    if (!isCommandHook(h)) return this.#d.plugins?.run(h, req, at) ?? Promise.resolve(undefined);
    if (dir === undefined) return Promise.resolve({ kind: "unavailable", detail: "the project is not open on this server" } satisfies HookFailure);
    return Effect.runPromise(
      runCommandHook(h, req, {
        ...this.#d.process,
        serverData: this.#d.serverData,
        slots: this.#slots,
        signal: this.#stopping.signal,
        ...(h.spec?.fin === true && this.#d.credentials !== undefined ? { credential: () => this.#d.credentials!.forRun(h, at.projectId) } : {}),
        recheck: () => this.#d.config.applies(at.projectId, h),
        onStart: ({ warning }) => {
          if (warning === undefined || this.#warned) return;
          this.#warned = true;
          this.#d.log?.(`hooks: ${warning.code}: ${warning.message} (logged once per server start; server.status lists it)`);
        },
      }),
    );
  }

  #failed(h: ConfiguredHook, at: HookAt, r: HookFailure, tag: { event: HookEvent; id: string; layer: HookLayer }): void {
    const counters = this.#counters[tag.event];
    if (r.kind === "timeout") {
      counters.timeouts += 1;
      this.#d.notices?.notice(at, { code: "HOOK_TIMEOUT", hook: h.id, event: tag.event, layer: h.layer, timeoutS: h.spec?.timeoutS ?? HOOK_LIMITS.timeoutS.default });
      return;
    }
    counters.failures += 1;
    const stderr = r.stderr === undefined ? "" : Buffer.from(r.stderr.replace(CONTROL, "")).subarray(0, STDERR_BYTES).toString("utf8").replace(/�$/, "");
    this.#d.notices?.notice(at, { hook: { ...tag, failed: r.kind, detail: r.detail, ...(stderr === "" ? {} : { stderr }) } });
  }

  /** `hook.block` as the caller's: never the reason (the session's notice holds it). */
  #audit(h: ConfiguredHook, at: HookAt, event: HookEvent, after: boolean): void {
    try {
      this.#append(h, at, event, after);
    } catch (e) {
      this.#d.log?.(`hooks: hook.block not audited: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  #append(h: ConfiguredHook, at: HookAt, event: HookEvent, after: boolean): void {
    this.#d.audit.append({
      kind: "hook.block",
      principal: { kind: at.principal.kind, id: at.principal.id },
      outcome: "denied",
      code: "HOOK_BLOCKED",
      details: { event, hookId: h.id, layer: h.layer, projectId: at.projectId, ...(at.session === undefined ? {} : { session: at.session.id }), ...(after ? { after: true } : {}) },
    });
  }
}

export function makeHookEngine(d: HookEngineDeps): HookEngineImpl {
  return new Engine(d);
}
