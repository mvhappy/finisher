// One attempt of a warm job (spec §6.3, 5C-1 plan Decisions 20, 23, 35, 36; Task 11): the job's
// directory, files and `prepare` as for a cold job, then the helpers in a group of their own
// (the kind's memory rule; the worker's group has the family's interactive rule), then `start`
// to a `ready` worker the pool lent. The attempt ends with the worker's next `ready` (the worker
// stays in the pool) or its exit (it is lost). Before the outcome counts, in test-hooks builds,
// the worker must hold nothing under the job directory (`WORKER_CRASHED {heldFiles}`), and the
// helpers shut down. A cancel the worker answers within the grace (`JOB_CANCELLED`, then
// `ready`) keeps it; otherwise its group is killed with the job's helpers. A helper that fails
// during the job gets the worker a `cancel` too, not a kill (Task 11 review L4): a `ready`
// within the grace keeps it, and the attempt fails with the helper's error.
import type { HelperGrant, JobHostKillReason } from "@finisher/core-types";
import type { HelperSpec } from "@finisher/contracts";
import { resolveMemoryRule } from "@finisher/fin-node";
import { finError, toFinError } from "../errors.ts";
import { AttemptBase, type AttemptSpec, type KillReason, type Outcome, stopped } from "./attempt.ts";
import { heldFiles } from "./held-files.ts";
import { exitVerdict, type JobHostExitInfo, type ProcEvent } from "./host.ts";
import { type FamilySpec, type KindSpec, resolveOrFn } from "./kinds.ts";
import type { RunControl } from "./run-control.ts";
import { type AttemptOutcome, attemptWith, EXIT_GRACE_MS, type RunnerEnv } from "./runner.ts";
import type { JobRow } from "./store.ts";

/** How long a worker has to answer the `cancel` a failed helper sent it (interactive, Decision 23). */
export const HELPER_CANCEL_GRACE_MS = 500;

/** What the pool lends an attempt of its worker. */
export interface WarmLink {
  readonly proc: number;
  readonly pid: number | undefined;
  /** The worker group's limit (the family's interactive rule, Decision 20). */
  readonly memoryBytes: number | undefined;
  /** The worker served jobs before this one. */
  readonly reused: boolean;
  /** Routes the worker's events to `on` (`undefined`: back to the pool). */
  bind(on: ((e: ProcEvent) => void) | undefined): void;
  /** The `start` was written: the worker serves one more job. */
  started(): void;
  /** Kills the worker's group: the worker is lost to the pool. */
  lose(reason: JobHostKillReason): void;
}

export type WarmOutcome = AttemptOutcome & {
  /** The worker is still good for the next job (it sent `ready` and holds nothing). */
  readonly kept: boolean;
  /** The worker group's figure attached to its `ready` (Decision 20). */
  readonly memory?: number;
};

/** One attempt of `row` on the pool's worker behind `link`. */
export async function runWarmAttempt(
  env: RunnerEnv,
  row: JobRow,
  kind: KindSpec,
  family: FamilySpec,
  params: unknown,
  control: RunControl,
  link: WarmLink,
): Promise<WarmOutcome> {
  let attempt: WarmAttempt | undefined;
  const o = await attemptWith(env, row, kind, params, control, (p) => {
    attempt = new WarmAttempt({ env, row, kind, family, params, control, ...p }, link);
    return attempt.run(p.helpers);
  });
  // A failure before the worker's part (files, `prepare`) leaves the worker untouched.
  const kept = attempt === undefined ? true : attempt.kept;
  const memory = attempt?.memory;
  return { ...o, warmReused: link.reused, kept, ...(memory === undefined ? {} : { memory }) };
}

class WarmAttempt extends AttemptBase {
  readonly #link: WarmLink;
  #helperGroup = -1;
  #cancelReady: (() => void) | undefined;
  /** The attempt is ending (after `ready` or the exit): later events change nothing. */
  #ending = false;
  #lost = false;
  #held: string[] = [];
  #heldError: string | undefined;
  /** A failed helper's `cancel` went to the worker; its kill waits for the grace. */
  #helperCancel: (() => void) | undefined;
  kept = false;
  memory: number | undefined;

  constructor(a: AttemptSpec, link: WarmLink) {
    super(a, () => this.#helperGroup);
    this.#link = link;
    this.proc = link.proc;
  }

  /** The helpers' group, and the worker once it has the job (before its `start` it is not the job's). */
  protected kill(reason: KillReason): void {
    if (this.#helperGroup >= 0) void this.a.env.host.killGroup(this.#helperGroup, reason).catch(() => {});
    if (!this.started) return;
    if (reason === "helperFailed" && !this.#lost && this.#helperCancel === undefined) {
      // The worker is not at fault: it may stop the job and stay (Task 11 review L4).
      const { env, row } = this.a;
      if (env.host.send(this.#link.proc, { t: "cancel", job: row.id, reason: "cancel" }) === "ok") {
        this.#helperCancel = env.timers.after(HELPER_CANCEL_GRACE_MS, () => this.#lose("helperFailed"));
        return;
      }
    }
    this.#lose(reason);
  }

  #lose(reason: JobHostKillReason): void {
    if (this.#lost) return;
    this.#lost = true;
    this.#link.lose(reason);
  }

  async run(helpers: ReadonlyArray<HelperSpec>): Promise<Outcome> {
    const { env, row, kind, control } = this.a;
    const done = new Promise<Outcome>((resolve) => {
      this.settle = resolve;
    });
    this.#link.bind((e) => this.#onEvent(e));
    this.unbind = control.bind((reason, graceMs) => this.stop(reason, graceMs));
    if (helpers.length > 0) {
      const rule = kind.memory === undefined ? undefined : resolveOrFn(kind.memory, row.params as never);
      const memoryBytes = rule === undefined ? undefined : resolveMemoryRule(rule, env.totalMem);
      this.#helperGroup = env.host.openGroup({ label: `${kind.kind} ${row.id} helpers`, qos: "normal", ...(memoryBytes === undefined ? {} : { memoryBytes }) });
    }
    let grants: HelperGrant[];
    try {
      grants = await this.helpers.start(helpers);
    } catch (e) {
      this.a.facts.helpers = this.helpers.started;
      const o: Outcome =
        control.requested !== undefined
          ? { ok: false, error: stopped(), cause: "stopped" }
          : this.fault === undefined
            ? { ok: false, error: toFinError(e), cause: "helper" }
            : { ok: false, ...this.fault };
      return this.#early(o);
    }
    this.a.facts.helpers = this.helpers.started;
    if (this.#lost) return this.#early({ ok: false, error: finError("WORKER_UNAVAILABLE", "the warm worker exited before the job started", { reason: "warmExited" }), cause: "spawn" });
    if (control.requested !== undefined || this.fault !== undefined) {
      return this.#early(this.fault === undefined ? { ok: false, error: stopped(), cause: "stopped" } : { ok: false, ...this.fault });
    }
    this.#link.started();
    this.sendStart(this.#link.memoryBytes, grants);
    return done;
  }

  /** Ends an attempt that never wrote its `start`: the worker is still the pool's. */
  async #early(o: Outcome): Promise<Outcome> {
    this.#ending = true;
    await this.#endHelpers();
    this.kept = !this.#lost;
    this.#detach();
    return o;
  }

  #detach(): void {
    this.unbind();
    this.#link.bind(undefined);
  }

  async #endHelpers(): Promise<void> {
    if (this.#helperGroup >= 0) {
      try {
        await this.a.env.host.killGroup(this.#helperGroup, "jobEnded");
      } catch (e) {
        this.a.env.log("warning", `job ${this.a.row.id}: ending its helpers' group failed: ${toFinError(e).message}`);
      }
    }
    await this.helpers.closeCalls();
    if (this.#helperGroup >= 0) this.a.env.host.closeGroup(this.#helperGroup);
  }

  #onEvent(e: ProcEvent): void {
    switch (e.type) {
      case "message":
        if (this.started) this.onMessage(e.msg, e.memory);
        return;
      case "stderr":
        return this.a.env.log("debug", `[job ${this.a.row.id} ${this.a.family.family}] ${e.line}`);
      case "exit":
        this.#lost = true;
        if (this.started) void this.#onExit(e.info);
        return;
      default:
        return;
    }
  }

  protected afterAnswer(): void {
    // A warm worker sends `ready` once it answered; one that does not is lost.
    this.#cancelReady = this.a.env.timers.after(EXIT_GRACE_MS, () => this.#lose("jobEnded"));
  }

  protected onReady(memory: number | undefined): void {
    if (this.#ending) return;
    if (this.fault !== undefined) {
      // Only a worker a failed helper cancelled ends its attempt by `ready`.
      if (this.#helperCancel === undefined || this.#lost) return;
      this.#helperCancel();
      this.a.env.host.watch(this.#link.proc, null);
      this.memory = memory;
      void this.#afterReady().catch((e) => this.#finish({ ok: false, error: finError("INTERNAL", `the attempt's end failed: ${toFinError(e).message}`), cause: "exit" }));
      return;
    }
    if (this.answer === undefined) {
      this.failWith(finError("WORKER_CRASHED", "the warm worker sent ready before its answer", { protocol: "readyBeforeAnswer" }), "protocol", "jobEnded");
      return;
    }
    this.#cancelReady?.();
    this.memory = memory;
    void this.#afterReady().catch((e) => this.#finish({ ok: false, error: finError("INTERNAL", `the attempt's end failed: ${toFinError(e).message}`), cause: "exit" }));
  }

  async #afterReady(): Promise<void> {
    this.#ending = true;
    const { env, dirs } = this.a;
    await this.calls.close();
    if (this.answer !== undefined && "value" in this.answer) await this.helpers.shutdown();
    await this.#endHelpers();
    // Decision 36: the worker closed every file and mapping under the job directory before
    // `ready` (checked once the helpers, which live in it, are gone).
    if (env.heldFilesCheck && !this.#lost) {
      try {
        this.#held = await heldFiles(this.#link.pid, dirs.job);
      } catch (e) {
        // A check that cannot be made fails (Task 11 review L3).
        this.#heldError = toFinError(e).message;
      }
    }
    const held = this.#held.length > 0 || this.#heldError !== undefined;
    if (held) this.#lose("jobEnded");
    this.kept = !this.#lost;
    if (held) {
      const error =
        this.#heldError === undefined
          ? finError("WORKER_CRASHED", "the warm worker held files of the job directory past its ready", { heldFiles: this.#held })
          : finError("WORKER_CRASHED", "the warm worker's open files could not be checked", { heldFiles: [], checkFailed: this.#heldError });
      return this.#finish({ ok: false, error, cause: "heldFiles" });
    }
    this.#finish(this.outcomeOf(undefined, undefined));
  }

  async #onExit(info: JobHostExitInfo): Promise<void> {
    const { env, row } = this.a;
    if (this.#ending) return;
    this.#ending = true;
    let outcome: Outcome | undefined;
    try {
      this.#cancelReady?.();
      this.#helperCancel?.();
      this.cancelKill?.();
      if (info.reason === "abandoned") env.abandoned(this.#link.proc, row.id);
      await this.calls.close();
      const verdict = exitVerdict(info, this.answer !== undefined);
      if (this.answer !== undefined && "value" in this.answer) await this.helpers.shutdown();
      await this.#endHelpers();
      outcome = this.outcomeOf(info, verdict.error);
    } catch (e) {
      outcome = { ok: false, error: finError("INTERNAL", `the attempt's end failed: ${toFinError(e).message}`), cause: "exit", exit: info };
    } finally {
      this.kept = false;
      this.#finish(outcome ?? { ok: false, error: finError("INTERNAL", "the attempt ended without an outcome"), cause: "exit", exit: info });
    }
  }

  #finish(o: Outcome): void {
    this.#helperCancel?.();
    this.#cancelReady?.();
    this.#detach();
    this.flush();
    this.settle(o);
  }
}
