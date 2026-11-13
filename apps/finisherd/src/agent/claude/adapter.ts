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
import { dirname } from "node:path";
import type { FinError, ProviderEvent, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import type { Detection, ProviderAdapter, ProviderSession, SessionSpec } from "../adapter.ts";
import { parseClaudeInitialize } from "../catalogue/cli-models.ts";
import type { Ids } from "../clock.ts";
import { systemIds } from "../clock.ts";
import type { SessionSandboxDeps } from "../sandbox.ts";
import { claudeCodeSettings } from "../settings.ts";
import { CLAUDE_CAPABILITIES, type ClaudeDetection, detectClaude } from "./detect.ts";
import { launchPlan, sessionUuid, writeLaunchFiles } from "./launch.ts";
import { instanceAccount, instanceAccountHash, labelOf } from "./account.ts";
import type { ClaudeModelCache } from "./models.ts";
import { completeProbe, probe, probeFlagsHash } from "./probe.ts";
import { unsignedAccount, untestedVersionNotices } from "./probe-check.ts";
import { ClaudeInstance, type RequestPort } from "./session.ts";

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
   * The money left in a session's budget, in micros, for `--max-budget-usd` (Decision 15's
   * backstop); null without a money limit. TODO(7D Task 9): the runtime's remaining budget and
   * the plan sign-in that turns the flag off.
   */
  readonly remainingMoneyMicros?: (sessionId: string) => number | null;
  readonly os?: NodeJS.Platform;
  readonly ids?: Ids;
  /** Detection (default `detectClaude` over `sandbox`; tests). */
  readonly detect?: () => Effect.Effect<ClaudeDetection, FinError>;
  /** Decision 16's waits (tests shorten them). */
  readonly interruptWaitMs?: number;
  readonly sigintGraceMs?: number;
  readonly closeGraceMs?: number;
  /** `agents.claudeCode.enabled`, read at each open (default: `settings.json`); off refuses the open. */
  readonly enabled?: () => boolean;
  /** Where CLI requests go (default: `claudePermissions()`, through the runtime; tests). */
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
        // Planning reads the setting live; so does each open (it may have been turned off since).
        if (!(deps.enabled ?? (() => claudeCodeSettings(deps.sandbox.serverData).enabled))()) {
          return yield* Effect.fail(finError("PROVIDER_UNAVAILABLE", "Claude Code is off on this server", { harness: "claudeCode", reason: "disabled" }, "set agents.claudeCode.enabled to true in settings.json"));
        }
        const d = yield* detect();
        const binary = yield* attempt(() => realpathSync(d.path));
        // Decision 7: the CLI's install tree, resolved, which its process reads.
        const extraRead = [dirname(binary)];
        const flagsHash = probeFlagsHash(os);
        const found = yield* probe(asDetection(d), flagsHash, { sandbox: deps.sandbox, binary, os, extraRead });
        const plan = yield* attempt(() =>
          launchPlan(s, spec, found, os, { binary, retentionDays: deps.retentionDays ?? 90, remainingMoneyMicros: deps.remainingMoneyMicros?.(s.id) ?? null, planSignIn: false }),
        );
        yield* attempt(() => writeLaunchFiles(plan, os));
        const child = yield* box.spawn(plan.argv, { cwd: box.workspace, stdio: "pipe", env: plan.env, extraRead });
        const notices: ProviderEvent[] = plan.dropped.map((tool) => ({ type: "notice", name: "toolDropped", details: { harness: "claudeCode", tool } }));
        const key = { version: d.version, flagsHash, os };
        // Decision 8: the sign-in counts as ignored only when `auth status` reports one and this
        // instance's own `initialize.account` is exactly unsigned (`apiKeySource: "none"` alone
        // is what a claude.ai sign-in reports). Set from the `initialize` answer, before any init.
        // TODO(7D Task 0 Step 1/Step 4): `initialize.account` and `apiKeySource` signed in with the
        // CLI's own flow; modelled by fake-claude as `tokenSource: "claude.ai"` with `none`.
        let ignoredSignIn = false;
        const check = {
          get signedIn() {
            return ignoredSignIn;
          },
        };
        const instance = yield* ClaudeInstance.start({
          instance: ids.ulid(),
          child,
          plan,
          uuid: sessionUuid(s.id),
          ref: { kind: "claudeCode", id: sessionUuid(s.id) },
          check,
          notices,
          onInit: (init) => {
            if (found.initPending === true) {
              void Effect.runPromise(Effect.result(completeProbe(deps.sandbox.serverData, key, init, plan, { signedIn: ignoredSignIn }))).then((r) => {
                if (r._tag === "Failure") log("claude probe not completed", { sessionId: s.id, code: r.failure.code });
              });
            }
            const n = untested(init.claude_code_version ?? d.version);
            return n === null ? [] : [n];
          },
          ...(deps.requests === undefined ? {} : { requests: deps.requests() }),
          ...(deps.interruptWaitMs === undefined ? {} : { interruptWaitMs: deps.interruptWaitMs }),
          ...(deps.sigintGraceMs === undefined ? {} : { sigintGraceMs: deps.sigintGraceMs }),
          ...(deps.closeGraceMs === undefined ? {} : { closeGraceMs: deps.closeGraceMs }),
        });
        yield* Effect.addFinalizer(() => Effect.promise(() => instance.close()));
        // The `initialize` answer: the account (Decision 19) and the models (Decision 11). A
        // process that ends first (an unknown `--resume`) opens all the same: its lines say why.
        const body = yield* Effect.promise(() => Promise.race([instance.initialized(), new Promise((r) => setTimeout(() => r(null), INITIALIZE_TIMEOUT_MS).unref())]));
        if (body !== null && typeof body === "object") {
          const account = (body as { account?: unknown }).account;
          ignoredSignIn = d.signedIn === true && unsignedAccount(account);
          const seen = sessionAccount(deps.sandbox.serverData, d, account, spec.providerRef === null);
          if (seen.refused !== null) return yield* Effect.fail(seen.refused);
          try {
            const models = parseClaudeInitialize(body);
            // Before Task 11's hash exists (signed out, or no account named) the version's own list.
            if (models.length > 0) deps.models.recordInstanceModels(d.version, seen.hash ?? UNKNOWN_ACCOUNT, models);
          } catch (e) {
            log("claude initialize without models", { sessionId: s.id, error: String(e).slice(0, 200) });
          }
        }
        return instance.session() satisfies ProviderSession;
      }),
  };
}

/** The models of an instance whose account has no hash (signed out, or naming none). */
export const UNKNOWN_ACCOUNT = "unknown";

/**
 * The account check of one instance (Decision 19, `account.ts`): the label its `initialize.account`
 * names, else the hash detection's `auth status` gave. A new session (no provider reference yet)
 * is refused while a change waits; a running session's respawn or resume never is.
 * TODO(7D Task 0 Step 4): whether `initialize.account` names the account signed in; modelled by
 * fake-claude as `email` there when its sidecar `account` is set.
 */
function sessionAccount(serverData: string, d: ClaudeDetection, account: unknown, newSession: boolean): { hash?: string; refused: FinError | null } {
  const label = labelOf(account);
  if (label === undefined && d.accountHash !== undefined) return instanceAccountHash(serverData, d.accountHash, d.authMethod, { newSession });
  return instanceAccount(serverData, { signedIn: d.signedIn === true, ...(d.authMethod === undefined ? {} : { authMethod: d.authMethod }), ...(label === undefined ? {} : { label }) }, { newSession });
}
