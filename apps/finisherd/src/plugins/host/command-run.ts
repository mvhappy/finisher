// `plugin.command.run` (sp8b plan Task 9; spec §3.8, Decision 7). In order:
// 1. the name resolves to an enabled plugin's command the caller may use (`PLUGIN_NOT_FOUND`),
//    and `args` pass the command's schema (`PLUGIN_COMMAND_ARGS {problems}`) before any job;
// 2. the branch resolves to its id and its snapshot is read **now** and held for the run
//    (`HostCalls.hold`): its rev is `CommandRun.rev`, every page the plugin reads, and the
//    transaction's `baseRev`, and its history generation goes with the commit, so a rewind in
//    between is `HISTORY_REWOUND` and a commit in between is rebased by fin-core from the rev
//    the plugin actually saw (never applied twice);
// 3. a `plugin.command` job owned by the invoker, `via` the plugin, with invoker ∩ grants as its
//    rights, runs on the key's warm process; the run is cancelled at the command's own
//    timeout (`PLUGIN_TIMEOUT`; counted from the submit, so time queued for a process counts)
//    or when the caller goes away;
// 4. `CommandOut::Data` is answered as is; `CommandOut::Tx` (only from a command whose manifest
//    says `writes`, and only with the `project:write` grant) is built as `Tx {baseRev, via,
//    author: the invoker}`, checked by fin-core against the held snapshot
//    (`PLUGIN_TX_INVALID {problems}`), then committed through the chain's `tx.commit` as the
//    live invoker (`commit`, wired in `handlers.ts`): its scopes, branch limits, write guard
//    and author apply as to any commit of the invoker's. `dryRun` takes the same path and
//    commits nothing. fin-core's `CONFLICT` is the outcome `conflict`.
// 5. every outcome and refusal reaches `onRun` (the journal's `plugin.command`, Task 15).
import { FinError, type JobView, type Principal } from "@finisher/contracts";
import type { CommandRun, CommittedTx, PluginCommandRunRequest, PluginCommandRunResult } from "@finisher/core-types";
import { pluginCommandOutcome } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { ulid } from "../../control/ulid.ts";
import { finError } from "../../errors.ts";
import type { JobEngine } from "../../jobs/api.ts";
import { COMMAND_KIND } from "../../jobs/kinds/plugin.command.ts";
import { type HostCalls, pinOf } from "./calls.ts";
import { type CommandEntry, type CommandRegistry, parseCommandName } from "./commands.ts";
import { scrubLogText } from "./log.ts";
import { type Invoker, requirePermission } from "./rights.ts";
import type { JobRights, Permission } from "@finisher/core-types";

/**
 * What a run did, for the journal (Task 15): never its data or the ops themselves. A name that
 * does not resolve (`PLUGIN_NOT_FOUND`, a disabled plugin) is reported with the plugin id and
 * command it names (no version), when it has that shape.
 */
export interface CommandRunEvent {
  /** `<id>@<version>`, or the bare id the name gave when it did not resolve. */
  readonly plugin: string;
  readonly command: string;
  readonly principal: { readonly kind: string; readonly id: string };
  /** The outcome, or the refusal's code. */
  readonly outcome: PluginCommandRunResult["outcome"] | "error";
  readonly code?: string;
  readonly dryRun: boolean;
  readonly ms: number;
  /** The arguments as validated (absent when they did not pass). */
  readonly args?: unknown;
  /** The committed transaction's id. */
  readonly txId?: string;
  /** Op kind → count of the ops the plugin returned (a commit, a conflict or a dry run). */
  readonly ops?: Readonly<Record<string, number>>;
}

/** Op kind (`op`, fin-core's wire name) → count. */
export function opCounts(ops: ReadonlyArray<unknown>): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const o of ops) {
    const k = (o as { op?: unknown } | null)?.op;
    if (typeof k === "string") counts[k] = (counts[k] ?? 0) + 1;
  }
  return counts;
}

/** A commit through the chain as the live invoker (`handlers.ts`); a dry run commits nothing. */
export type CommitAsInvoker = (projectId: string, tx: unknown, generation: unknown, opts: { readonly via: { readonly plugin: string; readonly command: string }; readonly dryRun: boolean }) => Effect.Effect<CommittedTx | { readonly dryRun: true }, FinError>;

export interface CommandRunDeps {
  readonly registry: CommandRegistry;
  readonly host: {
    rightsOf(invoker: Invoker, grants: ReadonlyArray<Permission>, sessionRoot?: string): JobRights;
    readonly calls: Pick<HostCalls, "hold">;
  };
  readonly engine: Pick<JobEngine, "submit" | "wait" | "cancel">;
  readonly projects: {
    resolveBranch(projectId: string, branch: string): Effect.Effect<string, FinError>;
    snapshot(projectId: string, branch: string): Effect.Effect<{ readonly rev: number; readonly project: unknown; readonly generation?: unknown }, FinError>;
  };
  readonly commit: CommitAsInvoker;
  /** An agent session's root branch (its rights' branch reach). */
  readonly sessionRootOf?: (p: Principal) => string | undefined;
  /** The author the invoker's records carry (the chain stamps a credential's own anyway). */
  readonly authorOf: (p: Principal) => string;
  readonly onRun?: (e: CommandRunEvent) => void;
  readonly clock?: () => number;
}

export function commandTimeout(entry: CommandEntry): FinError {
  return finError(
    "PLUGIN_TIMEOUT",
    `${entry.info.name} did not finish within ${entry.info.timeoutS} s`,
    { name: entry.info.name, timeoutS: entry.info.timeoutS },
    "Run it again; if it keeps timing out, its plugin is too slow for this project.",
  );
}

function txInvalid(name: string, problems: ReadonlyArray<unknown>): FinError {
  return finError("PLUGIN_TX_INVALID", `the transaction ${name} returned is not valid`, { name, problems }, "The plugin proposed a change Finisher refuses: report it to the plugin's publisher.");
}

const readOnly = (name: string): FinError =>
  txInvalid(name, [{ code: "PLUGIN_TX_INVALID", message: "the command's manifest does not say it writes", details: { reason: "readOnlyCommand" } }]);

/**
 * The job's error as the caller's. The SDKs carry a plugin error's `fix` in `details.fix` (the
 * envelope's `JobError` has no `fix`); it becomes `FinError.fix`, scrubbed like the plugin's log
 * text. Only host-known codes keep details at all (`mapPluginError`: a plugin's own code is
 * `PLUGIN_COMMAND_FAILED` without any), so no other plugin's fix reaches the caller.
 */
const asError = (e: { code: string; message: string; details?: unknown; fix?: string }): FinError => {
  const details = e.details !== null && typeof e.details === "object" ? { ...(e.details as Record<string, unknown>) } : {};
  const given = details["fix"];
  delete details["fix"];
  const fix = e.fix ?? (typeof given === "string" && given.trim() !== "" ? scrubLogText(given) : undefined);
  return new FinError({ code: e.code, message: e.message, details, ...(fix === undefined ? {} : { fix }) });
};

export function makeCommandRun(d: CommandRunDeps) {
  const clock = d.clock ?? Date.now;

  /**
   * Runs the job and answers its result (`CommandOut` as the plugin sent it). The job is
   * cancelled unless it ended: at the command's timeout (`PLUGIN_TIMEOUT`), on any failure of
   * the wait, and when the caller goes away (an interrupt, even right after the submit).
   */
  const runJob = (principal: Principal, entry: CommandEntry, params: CommandRun, rights: JobRights): Effect.Effect<unknown, FinError> =>
    Effect.acquireUseRelease(
      d.engine.submit(
        { kind: principal.kind as JobView["owner"]["kind"], id: principal.id },
        { kind: COMMAND_KIND, params, project: params.project, via: { plugin: entry.info.plugin }, rights, priority: "interactive", maxAttempts: 1 },
      ),
      (job) =>
        Effect.gen(function* () {
          const ended = yield* d.engine
            .wait([job.id], { timeoutMs: entry.info.timeoutS * 1000 })
            .pipe(Effect.catchIf((e) => e.code === "WAIT_TIMEOUT", () => Effect.fail(commandTimeout(entry))));
          const done = ended[0];
          if (done === undefined) return yield* Effect.fail(finError("INTERNAL", "the command's job vanished", { job: job.id }));
          if (done.state === "succeeded") return done.result;
          if (done.error !== undefined) return yield* Effect.fail(asError(done.error));
          return yield* Effect.fail(finError("JOB_CANCELLED", `${entry.info.name} was cancelled`, { name: entry.info.name, job: job.id }));
        }),
      (job, exit) => (Exit.isSuccess(exit) ? Effect.void : Effect.ignore(d.engine.cancel(job.id))),
    );

  const run = (principal: Principal, req: PluginCommandRunRequest): Effect.Effect<PluginCommandRunResult, FinError> => {
    const started = clock();
    const dryRun = req.dryRun === true;
    let entry: CommandEntry | undefined;
    // What the journal gets beyond the outcome (set as the run gets there).
    let seen: { args?: unknown; txId?: string; ops?: Record<string, number> } = {};
    const body = Effect.gen(function* () {
      const e = yield* d.registry.resolve(principal, req.name);
      entry = e;
      const args = req.args ?? {};
      const checked = e.args.validate(args);
      if (!checked.ok) return yield* Effect.fail(finError("PLUGIN_COMMAND_ARGS", `the arguments of ${e.info.name} do not match its schema`, { name: e.info.name, problems: checked.problems }, "Check the command's arguments with `fin help`."));
      seen = { args };
      const sessionRoot = d.sessionRootOf?.(principal);
      const rights = d.host.rightsOf({ kind: principal.kind, id: principal.id, scopes: principal.scopes, projects: principal.projects }, e.grants, sessionRoot);
      // A writing command that could never commit is refused before it runs.
      if (e.info.writes) {
        const denied = requirePermission(rights, "project:write");
        if (denied !== undefined) return yield* Effect.fail(denied);
      }
      const branch = yield* d.projects.resolveBranch(req.projectId, req.branch ?? "main");
      const snap = yield* d.projects.snapshot(req.projectId, branch);
      const pin = pinOf(snap);
      return yield* Effect.acquireUseRelease(
        Effect.sync(() => d.host.calls.hold(req.projectId, branch, pin)),
        (held) => Effect.gen(function* () {
          const params: CommandRun = { command: e.info.command, args, project: req.projectId, branch, rev: pin.rev, snapshot: held.id };
          const out = yield* runJob(principal, e, params, rights);
          // A read-only command's transaction is refused before fin-core applies anything.
          if (!e.info.writes && (out as { kind?: unknown } | null)?.kind === "tx") return yield* Effect.fail(readOnly(e.info.name));
          const via = { plugin: e.info.plugin, command: e.info.command };
          const checkedOut = pluginCommandOutcome(pin.json, out, { txId: ulid(clock()), author: d.authorOf(principal), branch, baseRev: pin.rev, via });
          if (!checkedOut.ok) {
            if ("protocol" in checkedOut) return yield* Effect.fail(finError("PLUGIN_PROTOCOL", `${e.info.name} returned something that is not a command result: ${checkedOut.protocol}`, { name: e.info.name, error: checkedOut.protocol }));
            return yield* Effect.fail(txInvalid(e.info.name, [checkedOut.invalid]));
          }
          if (checkedOut.kind === "data") return { outcome: "data", data: checkedOut.value as PluginCommandRunResult["data"] & {} } satisfies PluginCommandRunResult;
          const denied = requirePermission(rights, "project:write");
          if (denied !== undefined) return yield* Effect.fail(denied);
          const ops = checkedOut.tx.ops;
          seen = { ...seen, ops: opCounts(ops) };
          const committed = yield* d.commit(req.projectId, checkedOut.tx, pin.generation, { via, dryRun }).pipe(
            Effect.map((c) => ({ c }) as const),
            Effect.catchIf(
              (x) => x.code === "CONFLICT",
              (x) => Effect.succeed({ conflict: x } as const),
            ),
          );
          if ("conflict" in committed) return { outcome: "conflict", rev: pin.rev, ops } satisfies PluginCommandRunResult;
          const c = committed.c;
          if ("dryRun" in c) return { outcome: "data", dryRun: true, rev: pin.rev, ops } satisfies PluginCommandRunResult;
          seen = { ...seen, txId: c.tx.txId };
          return { outcome: c.rev === pin.rev + 1 ? "accepted" : "rebased", txId: c.tx.txId, rev: c.rev } satisfies PluginCommandRunResult;
        }),
        (held) => Effect.sync(held.release),
      );
    });
    const report = (outcome: CommandRunEvent["outcome"], code?: string) =>
      Effect.sync(() => {
        if (d.onRun === undefined) return;
        // A refusal before the name resolved names what the caller asked for (no version).
        const named = entry === undefined ? parseCommandName(req.name) : { id: entry.info.plugin, command: entry.info.command };
        if (named === undefined) return;
        d.onRun({ plugin: named.id, command: named.command, principal: { kind: principal.kind, id: principal.id }, outcome, ...(code === undefined ? {} : { code }), dryRun, ms: clock() - started, ...seen });
      });
    return body.pipe(
      Effect.tap((r) => report(r.outcome)),
      Effect.tapError((e) => report("error", e.code)),
    );
  };

  return { run };
}

export type CommandRunner = ReturnType<typeof makeCommandRun>;
