// The Claude Code adapter (plan 7D Task 7; Decisions 1, 8, 11, 12): `ClaudeCodeAdapter` opens
// one CLI process per instance, through the session sandbox only (`spec.sandbox`: the spawn
// gate, Review Focus 3), after detection and the capability probe, and hands it to
// `ClaudeInstance` (`session.ts`).
//
// Per instance: detect (cached), probe (cached per `(version, flags hash, OS)`), the launch plan
// and its files, the process (`--session-id` or `--resume=`, the CLI's install tree as
// `extraRead`), then `initialize` (its models replace the cache's list for the version and
// account, Decision 11) and the first-turn gate. The first init that passes completes a probe
// left `initPending` and gives the version's `untestedVersion` notice once (spec §4.5).
//
// The adapter is always registered; planning (live: detected and `agents.claudeCode.enabled`)
// decides whether a session may start on it, and each open checks the setting again. Nothing
// runs the CLI at boot.
import { realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import type { FinError, Node, ProviderEvent, Session } from "@finisher/contracts";
import { Effect } from "effect";
import type { KeyFs } from "../../auth/keys.ts";
import { finError, toFinError } from "../../errors.ts";
import type { Detection, ProviderAdapter, ProviderRef, ProviderSession, SessionSpec } from "../adapter.ts";
import { parseClaudeInitialize } from "../catalogue/cli-models.ts";
import type { Ids } from "../clock.ts";
import { systemIds } from "../clock.ts";
import { aliveProcesses, type CliBaseline, forgetProcess, lastBaseline, lastEnd, noteResult, recordProcess } from "../provider-pid.ts";
import type { SandboxedChild, SessionSandbox, SessionSandboxDeps } from "../sandbox.ts";
import { claudeCodeSettings } from "../settings.ts";
import { CLAUDE_CAPABILITIES, type ClaudeDetection, detectClaude } from "./detect.ts";
import { launchPlan, sessionUuid, writeLaunchFiles } from "./launch.ts";
import { instanceAccount, instanceAccountHash, labelOf } from "./account.ts";
import type { ClaudeModelCache } from "./models.ts";
import { completeProbe, probe, probeFlagsHash } from "./probe.ts";
import { signInIgnored, untestedVersionNotices } from "./probe-check.ts";
import { ClaudeSession, IDLE_CLOSE_MS } from "./instances.ts";
import { claudePermissions } from "./permissions.ts";
import { interruptedNote, resumeState, toldIds } from "./resume.ts";
import { ClaudeInstance, CLOSE_EXIT_WAIT_MS, type EventSink, type RequestPort } from "./session.ts";
import { baselineOf, ClaudeUsage, type ResumeTotals, restoredOffset, turnProjection, ZERO_TOTALS } from "./usage.ts";

export { sessionUuid } from "./launch.ts";

/** How long an instance's `initialize` answer may take (Task 0: under a second). */
export const INITIALIZE_TIMEOUT_MS = 30_000;

export interface ClaudeDeps {
  /** The host sandboxes' dependencies (detection, the probe); sessions' processes use `spec.sandbox`. */
  readonly sandbox: SessionSandboxDeps;
  /** The session records (`launchPlan` reads the session). */
  readonly sessions: { get(id: string): Session | undefined };
  readonly models: ClaudeModelCache;
  /** `agents.retentionDays` (default 90): the CLI keeps its transcripts as long. */
  readonly retentionDays?: number;
  /**
   * What is left of a session's budget (limit − used − reserved; null without a limit): money
   * for `--max-budget-usd` (Decision 15's backstop, set on each (re)start), tokens to cap the
   * per-turn projection the gate reserves (Task 9).
   */
  readonly remaining?: (sessionId: string) => { readonly tokens: number | null; readonly moneyMicros: number | null };
  /** `agents.claudeCode.chargePlanMoney`: a plan sign-in's money is charged too (spec §13.1). */
  readonly chargePlanMoney?: () => boolean;
  /** The per-turn projection, in tokens (default `TURN_ESTIMATE_TOKENS`). */
  readonly turnEstimateTokens?: number;
  readonly os?: NodeJS.Platform;
  readonly ids?: Ids;
  /** Detection (default `detectClaude` over `sandbox`; tests). */
  readonly detect?: () => Effect.Effect<ClaudeDetection, FinError>;
  /** Decision 16's waits (tests shorten them). */
  readonly interruptWaitMs?: number;
  readonly sigintGraceMs?: number;
  readonly closeGraceMs?: number;
  /** The file calls `provider.pid`'s record of a starting process makes (tests inject failures). */
  readonly pidIo?: KeyFs;
  /** `processStart` for `provider.pid` (tests inject a failure). */
  readonly processStart?: (pid: number) => string | null;
  /** How long a killed process may take to be gone (default `CLOSE_EXIT_WAIT_MS`). */
  readonly exitWaitMs?: number;
  /** How long the `initialize` answer may take (default `INITIALIZE_TIMEOUT_MS`; tests). */
  readonly initializeTimeoutMs?: number;
  /** `agents.claudeCode.enabled`, read at each open (default: `settings.json`); off refuses the open. */
  readonly enabled?: () => boolean;
  /** Where CLI requests go (default: `claudePermissions()`, through the runtime; tests). */
  readonly requests?: () => RequestPort;
  /** A session's nodes, oldest first: what a resumed instance is told and refused (Task 10, `resume.ts`). */
  readonly nodes?: (sessionId: string) => Promise<readonly Node[]> | readonly Node[];
  /** Decision 12's idle close (default: `agents.claudeCode.idleCloseS`, 300 s); ≤ 0 never. */
  readonly idleCloseMs?: number;
  readonly log?: (message: string, details: Record<string, unknown>) => void;
}

const asDetection = (d: ClaudeDetection): Detection => ({ installed: true, version: d.version, supported: `>= ${d.floor}`, signedIn: d.signedIn === true, capabilities: CLAUDE_CAPABILITIES });

/** `f`, a throw of it a failure (`FinError` kept, anything else `INTERNAL`). */
const attempt = <A>(f: () => A | Promise<A>) => Effect.tryPromise({ try: async () => f(), catch: toFinError });

export function ClaudeCodeAdapter(deps: ClaudeDeps): ProviderAdapter {
  const os = deps.os ?? deps.sandbox.platform ?? process.platform;
  const ids = deps.ids ?? systemIds();
  const detect = deps.detect ?? (() => detectClaude({ sandbox: deps.sandbox }));
  // One notice per version per server (spec §4.5).
  const untested = untestedVersionNotices();
  const log = deps.log ?? (() => {});
  const idleCloseMs = () => deps.idleCloseMs ?? (claudeCodeSettings(deps.sandbox.serverData).idleCloseS ?? IDLE_CLOSE_MS / 1000) * 1000;

  return {
    harness: "claudeCode",
    detect: () => Effect.map(detect(), asDetection),
    open: (spec: SessionSpec) =>
      Effect.gen(function* () {
        // Review Focus 3: the session sandbox is the only way this adapter starts a process.
        const box = spec.sandbox;
        if (box === undefined) return yield* Effect.fail(finError("SANDBOX_UNAVAILABLE", "a Claude Code session starts only in its session sandbox", { harness: "claudeCode", sessionId: spec.sessionId }));
        if (deps.sessions.get(spec.sessionId) === undefined) return yield* Effect.fail(finError("SESSION_NOT_FOUND", "no such session", { sessionId: spec.sessionId }));
        const uuid = sessionUuid(spec.sessionId);
        const ref = { kind: "claudeCode", id: uuid };
        // A resumed session (Task 10): the calls its CLI is told of, and refused again (`resume.ts`).
        const resumed = spec.providerRef !== null;
        const state = resumed && deps.nodes !== undefined ? resumeState(yield* Effect.promise(async () => deps.nodes!(spec.sessionId))) : undefined;
        // Its last process ended while no server watched (Decision 17; verified 2.1.321, Task 0
        // s5C: it may have finished the turn into its transcript): the CLI and a person are told.
        const unseen = resumed && lastEnd(sessionDir(spec.sessionId)) === "unseen";
        const proc = startProcess(spec, box, uuid, ref, ids.ulid(), unseen);
        const note = [interruptedNote(state?.interrupted ?? []), unseen ? UNSEEN_NOTE : undefined].filter((n) => n !== undefined).join("\n\n");
        const session = yield* ClaudeSession.open(
          { instance: proc.instance, ref, start: proc.start, idleCloseMs, log, ...(state === undefined ? {} : { known: state.known }), ...(note === "" ? {} : { note, told: toldIds(state?.interrupted ?? []) }) },
          resumed,
        );
        yield* Effect.addFinalizer(() => Effect.promise(() => session.close()));
        return session.session() satisfies ProviderSession;
      }),
  };

  /**
   * One process of the session's instance `instance` (Decision 12: the first, or one resumed
   * after an idle close): detection, the probe, the launch plan, the spawn through the session
   * sandbox (recorded in `provider.pid` for Decision 17's sweep), then the `initialize` answer
   * (the account, the models). A refused process is closed before the failure.
   */
  function startProcess(spec: SessionSpec, box: SessionSandbox, uuid: string, ref: ProviderRef, instance: string, unseen: boolean) {
    // The `unseenTail` notice goes with the first process only.
    let first = true;
    const start = ({ resume, sink, rerun }: { readonly resume: boolean; readonly sink: EventSink; readonly rerun: (id: string) => boolean }) =>
      Effect.gen(function* () {
        const s = deps.sessions.get(spec.sessionId);
        if (s === undefined) return yield* Effect.fail(finError("SESSION_NOT_FOUND", "no such session", { sessionId: spec.sessionId }));
        // Planning reads the setting live; so does each open (it may have been turned off since).
        if (!(deps.enabled ?? (() => claudeCodeSettings(deps.sandbox.serverData).enabled))()) {
          return yield* Effect.fail(finError("PROVIDER_UNAVAILABLE", "Claude Code is off on this server", { harness: "claudeCode", reason: "disabled" }, "set agents.claudeCode.enabled to true in settings.json"));
        }
        const d = yield* detect();
        // Decision 19 (as 7D Task 0 amended it): signed in, but the sign-in home names no account
        // (neither e-mail nor organization): a new session is refused before the CLI starts; a
        // running session's later process is not (it started on the earlier check).
        // TODO(7D Task 18): depends on the person's `auth login` filling the sign-in home's
        // `.claude.json` (assumed; only the owner's long-lived config dir was observed).
        if (d.problem?.code === "PROVIDER_AUTH" && spec.providerRef === null && !resume) return yield* Effect.fail(d.problem);
        const binary = yield* attempt(() => realpathSync(d.path));
        // Decision 7: the CLI's install tree, resolved, which its process reads.
        const extraRead = [dirname(binary)];
        const flagsHash = probeFlagsHash(os);
        const found = yield* probe(asDetection(d), flagsHash, { sandbox: deps.sandbox, binary, os, extraRead });
        // Decision 15: a plan sign-in's money is shown, not charged (unless the owner asked), so
        // it has no `--max-budget-usd` backstop. Verified on 2.1.321 (7D Task 0): a plan
        // sign-in's `auth status` prints `authMethod: "claude.ai"` (with `subscriptionType`).
        const planSignIn = d.authMethod === "claude.ai" && deps.chargePlanMoney?.() !== true;
        const dir = sessionDir(s.id);
        // Decision 17: never a second process beside one still alive on this transcript (a
        // sweep that could not kill it, or a close whose process outlived its wait).
        if (aliveProcesses(dir, deps.processStart).length > 0) {
          return yield* Effect.fail(finError("PROVIDER_UNAVAILABLE", "an earlier Claude Code process of this session is still running", { harness: "claudeCode", reason: "previousProcessAlive" }, "try again in a moment"));
        }
        const charged = baselineOf(s.usage);
        // Verified 2.1.321, Task 0 (s1d/s1e): `--resume` restores the CLI's totals after a clean
        // exit and restarts them after a kill. What a resumed process restored is checked against
        // the last process's own baseline (`provider.pid`), not the session's lifetime spend;
        // beyond it, or with none, it is charged (`usage.ts`). The flag adds the restored money
        // only after a clean end (`tracked`), when the CLI counts it.
        const offset = resume ? restoredOffset(lastBaseline(dir), charged) : ZERO_TOTALS;
        const totals: ResumeTotals = resume ? "restored" : "reset";
        const restoredCostMicros = resume && lastEnd(dir) === "clean" ? offset.costMicros : 0;
        // O2: the baseline at each `result` (an unseen end then re-charges only the unseen tail).
        let pid: number | null = null;
        const plan = yield* attempt(() =>
          launchPlan(s, { ...spec, providerRef: resume ? ref : null }, found, os, { binary, retentionDays: deps.retentionDays ?? 90, remainingMoneyMicros: deps.remaining?.(s.id).moneyMicros ?? null, planSignIn, restoredCostMicros }),
        );
        const usage: ClaudeUsage = new ClaudeUsage({
          baseline: offset,
          resumed: resume,
          basis: planSignIn ? "subscription" : "reported",
          resumeTotals: totals,
          onResult: (cli) => {
            if (pid === null) return;
            try {
              noteResult(dir, pid, { cli, offset: usage.offset, charged });
            } catch (e) {
              // A stale baseline only over-charges a later resume.
              log("claude result baseline not recorded", { pid, error: String(e).slice(0, 200) });
            }
          },
        });
        yield* attempt(() => writeLaunchFiles(plan, os));
        const spawned = yield* box.spawn(plan.argv, { cwd: box.workspace, stdio: "pipe", env: plan.env, extraRead });
        // Decision 17: recorded before the instance takes any work, or killed and refused.
        const child = yield* tracked(dir, instance, spawned, () => ({ cli: usage.cliTotals(), offset: usage.offset, charged }), () => usage.lastIsError());
        pid = child.pid;
        const notices: ProviderEvent[] = plan.dropped.map((tool) => ({ type: "notice", name: "toolDropped", details: { harness: "claudeCode", tool } }));
        if (unseen && first) notices.push({ type: "notice", name: "unseenTail", details: { harness: "claudeCode", reason: "exitedWhileServerDown" } });
        first = false;
        const key = { version: d.version, flagsHash, os };
        // Decision 8 (as 7D Task 0 amended it): the sign-in counts as ignored when `auth status`
        // reports one and this instance's own `initialize.account` shows none (`signInIgnored`);
        // verified on 2.1.321: `apiKeySource: "none"` is what a claude.ai sign-in reports, so it
        // decides nothing. Set from the `initialize` answer, before any init.
        let ignoredSignIn = false;
        const check = {
          get signInIgnored() {
            return ignoredSignIn;
          },
        };
        const i = yield* ClaudeInstance.start({
          instance,
          child,
          plan,
          uuid,
          ref,
          sink,
          check,
          notices,
          // Resume re-checks the init too (Task 5's predicate on every process).
          onInit: (init) => {
            if (found.initPending === true) {
              void Effect.runPromise(Effect.result(completeProbe(deps.sandbox.serverData, key, init, plan, { signInIgnored: ignoredSignIn }))).then((r) => {
                if (r._tag === "Failure") log("claude probe not completed", { sessionId: s.id, code: r.failure.code });
              });
            }
            const n = untested(init.claude_code_version ?? d.version);
            return n === null ? [] : [n];
          },
          usage,
          requests: deps.requests?.() ?? claudePermissions({ rerun }),
          ...(spec.gate === undefined ? {} : { gate: spec.gate, projection: () => turnProjection(deps.remaining?.(s.id).tokens ?? null, deps.turnEstimateTokens) }),
          ...(deps.interruptWaitMs === undefined ? {} : { interruptWaitMs: deps.interruptWaitMs }),
          ...(deps.sigintGraceMs === undefined ? {} : { sigintGraceMs: deps.sigintGraceMs }),
          ...(deps.closeGraceMs === undefined ? {} : { closeGraceMs: deps.closeGraceMs }),
          ...(deps.exitWaitMs === undefined ? {} : { exitWaitMs: deps.exitWaitMs }),
        });
        const refuse = (e: FinError) => Effect.andThen(Effect.promise(() => i.close()), Effect.fail(e));
        // The `initialize` answer: the account (Decision 19) and the models (Decision 11). A
        // process that ends first opens all the same (its lines say why), except a resume whose
        // transcript the CLI cannot find (Decision 17, Task 0): that one is refused. A live
        // process with no usable answer (none in time, an error) is refused: the sign-in and
        // account checks need it (fail closed).
        const late = Symbol("late");
        const ms = deps.initializeTimeoutMs ?? INITIALIZE_TIMEOUT_MS;
        const answered = yield* Effect.promise(() => Promise.race([i.initialized(), new Promise<typeof late>((r) => setTimeout(() => r(late), ms).unref())]));
        if (answered === late || ((answered === null || typeof answered !== "object") && !i.exited())) {
          return yield* refuse(finError("PROVIDER_PROTOCOL", "Claude Code gave no usable initialize answer, so its sign-in and account could not be checked", { harness: "claudeCode", reason: answered === late ? "initializeTimeout" : "initialize", ...(answered === late ? { timeoutMs: ms } : {}) }));
        }
        const body = answered;
        const early = i.early();
        if (body === null && resume && early !== null && early.subtype === "error_during_execution") {
          return yield* refuse(finError("PROVIDER_EXITED", "Claude Code has no transcript of this session to resume", { harness: "claudeCode", reason: "transcriptLost", detail: early.text }, "end this session and start a new one"));
        }
        if (body !== null && typeof body === "object") {
          const account = (body as { account?: unknown }).account;
          ignoredSignIn = d.signedIn === true && signInIgnored(account, d.authMethod);
          // A new session (no provider reference) may be refused; a resumed process never is.
          const seen = sessionAccount(deps.sandbox.serverData, d, account, spec.providerRef === null && !resume);
          if (seen.refused !== null) return yield* refuse(seen.refused);
          try {
            const models = parseClaudeInitialize(body);
            // Before Task 11's hash exists (signed out, or no account named) the version's own list.
            if (models.length > 0) deps.models.recordInstanceModels(d.version, seen.hash ?? UNKNOWN_ACCOUNT, models);
          } catch (e) {
            log("claude initialize without models", { sessionId: s.id, error: String(e).slice(0, 200) });
          }
        }
        return i;
      });
    return { instance, start };
  }

  function sessionDir(sessionId: string): string {
    return join(deps.sandbox.serverData, "agents", sessionId);
  }

  /**
   * Decision 17: the process is in `provider.pid` while it runs. A record that cannot be written
   * (or, outside Windows, a start time that cannot be read) refuses the process: its group is
   * killed (and waited for) before the failure, so nothing runs that a crash recovery could not
   * sweep; a process already gone by then (an unknown `--resume` exits at once) needs no record.
   * The child handed on settles `exited` only once the end is recorded, so a later process reads
   * it fresh. The end is `clean` when the CLI exited by itself with no signal: exit 0, or exit 1
   * after an error `result` (Task 0 s1b, s1f: stdin's end then exits 1). TODO(7D Task 0 Step 1):
   * whether the real CLI writes its `cost-state` (restores its totals) on that exit 1; modelled by
   * fake-claude so. Only the flag reads it: a resume's charges come from per-turn deltas, so a
   * wrong `clean` loosens only the CLI's own backstop, never the runtime's budget stop.
   */
  function tracked(dir: string, instance: string, child: SandboxedChild, baseline: () => CliBaseline, lastIsError: () => boolean): Effect.Effect<SandboxedChild, FinError> {
    const unrecorded = Effect.promise(async () => {
      try {
        recordProcess(dir, instance, child.pid, { ...(deps.pidIo === undefined ? {} : { io: deps.pidIo }), ...(deps.processStart === undefined ? {} : { start: deps.processStart }) });
        return null;
      } catch (e) {
        // Gone already (its start time unreadable because it exited): nothing to sweep.
        if (await Promise.race([child.exited.then(() => true), new Promise<boolean>((r) => setTimeout(() => r(false), GONE_WAIT_MS).unref())])) return null;
        child.kill("SIGKILL");
        await Promise.race([child.exited, new Promise((r) => setTimeout(r, deps.exitWaitMs ?? CLOSE_EXIT_WAIT_MS).unref())]);
        return String(e).slice(0, 200);
      }
    });
    return Effect.flatMap(unrecorded, (error) => {
      if (error !== null) return Effect.fail(finError("INTERNAL", "the Claude Code process could not be recorded for the orphan sweep", { harness: "claudeCode", reason: "processNotRecorded", error }));
      const exited = child.exited.then((e) => {
        try {
          const clean = (e.signal ?? null) === null && (e.code === 0 || (e.code === 1 && lastIsError()));
          forgetProcess(dir, child.pid, clean ? "clean" : "killed", baseline());
        } catch (err) {
          // The directory is gone (the session was deleted), or the disk failed: the record stays
          // and the next recovery counts the process as unseen (fail closed).
          log("claude process end not recorded", { pid: child.pid, error: String(err).slice(0, 200) });
        }
        return e;
      });
      return Effect.succeed({ pid: child.pid, stdin: child.stdin, stdout: child.stdout, stderr: child.stderr, exited, kill: (signal?: NodeJS.Signals) => child.kill(signal) });
    });
  }
}

/** How long a process whose record failed may take to show it had exited already. */
const GONE_WAIT_MS = 200;

/** What a resumed CLI is told when its last process ended unwatched (`provider-pid.ts`). */
const UNSEEN_NOTE =
  "Finisher: the session's previous process went on after the server stopped and ended unwatched; what it did after that point (its last turn may have finished) is in your transcript but was not recorded by Finisher.";

/** The models of an instance whose account has no hash (signed out, or naming none). */
export const UNKNOWN_ACCOUNT = "unknown";

/**
 * The account check of one instance (Decision 19, `account.ts`): the label its `initialize.account`
 * names, else the hash detection's `auth status` gave from the sign-in home (the same subject for
 * an e-mail, so the same hash), else none (`instanceAccount`: nothing recorded; a new session is
 * refused only by a change already waiting). A new session (no provider reference yet) is refused
 * while a change waits; a running session's respawn or resume never is. Verified on 2.1.321 (7D
 * Task 0): `initialize.account.email` names the account once the CLI holds the profile in the
 * session's config dir (a resumed process; a first start only when the profile fetch wins), so on
 * a first start detection's hash names it.
 */
function sessionAccount(serverData: string, d: ClaudeDetection, account: unknown, newSession: boolean): { hash?: string; refused: FinError | null } {
  const label = labelOf(account);
  if (label === undefined && d.accountHash !== undefined) return instanceAccountHash(serverData, d.accountHash, d.authMethod, { newSession });
  return instanceAccount(serverData, { signedIn: d.signedIn === true, ...(d.authMethod === undefined ? {} : { authMethod: d.authMethod }), ...(label === undefined ? {} : { label }) }, { newSession });
}
