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
// The adapter is registered only when the CLI is detected and `agents.claudeCode.enabled` is on
// (`claudeAdapterFor`, called by `wiring.ts` at boot).
import { realpathSync } from "node:fs";
import { dirname } from "node:path";
import type { FinError, ProviderEvent, ProviderInfo, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import type { Detection, ProviderAdapter, ProviderSession, SessionSpec } from "../adapter.ts";
import { parseClaudeInitialize } from "../catalogue/cli-models.ts";
import type { Ids } from "../clock.ts";
import { systemIds } from "../clock.ts";
import type { SessionSandboxDeps } from "../sandbox.ts";
import { CLAUDE_CAPABILITIES, type ClaudeDetection, detectClaude } from "./detect.ts";
import { launchPlan, sessionUuid, writeLaunchFiles } from "./launch.ts";
import { type ClaudeModelCache, claudeHarnessAvailable } from "./models.ts";
import { completeProbe, probe, probeFlagsHash } from "./probe.ts";
import { untestedVersionNotices } from "./probe-check.ts";
import { ClaudeInstance, type RequestPort } from "./session.ts";
import { baselineOf, ClaudeUsage, type ResumeTotals, turnProjection } from "./usage.ts";

export { sessionUuid } from "./launch.ts";

/**
 * The account an instance's models are recorded under until Task 11 hashes the signed-in
 * account (`agent.providers.accountHash` is unset until then, so planning reads the version's
 * newest instance list, `ClaudeModelCache.models`). TODO(7D Task 11): the keyed account hash.
 */
export const PENDING_ACCOUNT = "pending";

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
  /** What `--resume` does with the CLI's totals (`usage.ts`; default `reset`, fail closed, until Task 0). */
  readonly resumeTotals?: ResumeTotals;
  /** The per-turn projection, in tokens (default `TURN_ESTIMATE_TOKENS`). */
  readonly turnEstimateTokens?: number;
  readonly os?: NodeJS.Platform;
  readonly ids?: Ids;
  /** Detection (default `detectClaude` over `sandbox`; tests). */
  readonly detect?: () => Effect.Effect<ClaudeDetection, FinError>;
  /** Decision 16's waits (tests shorten them). */
  readonly interruptWaitMs?: number;
  readonly sigintGraceMs?: number;
  /** Where CLI requests go (Task 8 fills it; default: refused). */
  readonly requests?: () => RequestPort;
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

  return {
    harness: "claudeCode",
    detect: () => Effect.map(detect(), asDetection),
    open: (spec: SessionSpec) =>
      Effect.gen(function* () {
        // Review Focus 3: the session sandbox is the only way this adapter starts a process.
        const box = spec.sandbox;
        if (box === undefined) return yield* Effect.fail(finError("SANDBOX_UNAVAILABLE", "a Claude Code session starts only in its session sandbox", { harness: "claudeCode", sessionId: spec.sessionId }));
        const s = deps.sessions.get(spec.sessionId);
        if (s === undefined) return yield* Effect.fail(finError("SESSION_NOT_FOUND", "no such session", { sessionId: spec.sessionId }));
        const d = yield* detect();
        const binary = yield* attempt(() => realpathSync(d.path));
        // Decision 7: the CLI's install tree, resolved, which its process reads.
        const extraRead = [dirname(binary)];
        const flagsHash = probeFlagsHash(os);
        const found = yield* probe(asDetection(d), flagsHash, { sandbox: deps.sandbox, binary, os, extraRead });
        // Decision 15: a plan sign-in's money is shown, not charged (unless the owner asked), so
        // it has no `--max-budget-usd` backstop. TODO(7D Task 0 Step 4): the `auth status`
        // `authMethod` of a plan sign-in; modelled by fake-claude as `claude.ai` (its default).
        const planSignIn = d.authMethod === "claude.ai" && deps.chargePlanMoney?.() !== true;
        const baseline = baselineOf(s.usage);
        const plan = yield* attempt(() =>
          launchPlan(s, spec, found, os, { binary, retentionDays: deps.retentionDays ?? 90, remainingMoneyMicros: deps.remaining?.(s.id).moneyMicros ?? null, planSignIn, restoredCostMicros: deps.resumeTotals === "restored" ? baseline.costMicros : 0 }),
        );
        const usage = new ClaudeUsage({ baseline, resumed: spec.providerRef !== null, basis: planSignIn ? "subscription" : "reported", ...(deps.resumeTotals === undefined ? {} : { resumeTotals: deps.resumeTotals }) });
        yield* attempt(() => writeLaunchFiles(plan, os));
        const child = yield* box.spawn(plan.argv, { cwd: box.workspace, stdio: "pipe", env: plan.env, extraRead });
        const notices: ProviderEvent[] = plan.dropped.map((tool) => ({ type: "notice", name: "toolDropped", details: { harness: "claudeCode", tool } }));
        const key = { version: d.version, flagsHash, os };
        const instance = yield* ClaudeInstance.start({
          instance: ids.ulid(),
          child,
          plan,
          uuid: sessionUuid(s.id),
          ref: { kind: "claudeCode", id: sessionUuid(s.id) },
          // TODO(7D Task 0 Step 1): `system/init.apiKeySource` of a CLI signed in with its own
          // flow; modelled by fake-claude as whatever the script's `init` sets (`none` refuses a
          // signed-in instance as an ignored sign-in, Decision 8).
          check: { signedIn: d.signedIn === true },
          notices,
          onInitialize: (body) => {
            try {
              const models = parseClaudeInitialize(body);
              if (models.length > 0) deps.models.recordInstanceModels(d.version, PENDING_ACCOUNT, models);
            } catch (e) {
              log("claude initialize without models", { sessionId: s.id, error: String(e).slice(0, 200) });
            }
          },
          onInit: (init) => {
            if (found.initPending === true) {
              void Effect.runPromise(Effect.result(completeProbe(deps.sandbox.serverData, key, init, plan, { signedIn: d.signedIn === true }))).then((r) => {
                if (r._tag === "Failure") log("claude probe not completed", { sessionId: s.id, code: r.failure.code });
              });
            }
            const n = untested(init.claude_code_version ?? d.version);
            return n === null ? [] : [n];
          },
          usage,
          ...(spec.gate === undefined ? {} : { gate: spec.gate, projection: () => turnProjection(deps.remaining?.(s.id).tokens ?? null, deps.turnEstimateTokens) }),
          ...(deps.requests === undefined ? {} : { requests: deps.requests() }),
          ...(deps.interruptWaitMs === undefined ? {} : { interruptWaitMs: deps.interruptWaitMs }),
          ...(deps.sigintGraceMs === undefined ? {} : { sigintGraceMs: deps.sigintGraceMs }),
        });
        yield* Effect.addFinalizer(() => Effect.sync(() => instance.close()));
        return instance.session() satisfies ProviderSession;
      }),
  };
}

/** The adapter when Claude Code may run sessions here (detected, `agents.claudeCode.enabled`), else none. */
export function claudeAdapterFor(info: Pick<ProviderInfo, "detected" | "enabled">, deps: ClaudeDeps): ProviderAdapter | undefined {
  return claudeHarnessAvailable(info) ? ClaudeCodeAdapter(deps) : undefined;
}
