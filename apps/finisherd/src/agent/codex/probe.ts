// The Codex capability probe (plan 7E Task 8, Decision 8), per `(pinned version, flags hash,
// OS)` and cached in `<server-data>/agents/codex-probe.json` (temp then rename, 0600, no account
// data). It runs the pinned binary with the session's flags (`codexLaunch` of a probe role)
// through a **host sandbox** (`openHostSandbox("probe", "codex")`: a throwaway `CODEX_HOME` with
// **no** `auth.json` link, `HOME` its empty `tmp/home`), and every method it sends is free:
//
// `initialize` (`userAgent` names the pin) · `initialized` · `config/read {includeLayers}` ·
// `configRequirements/read` · `skills/list` · `hooks/list` · `plugin/installed` ·
// `mcpServerStatus/list` · `app/installed` · `account/read` · `model/list` (paged; seeds the
// models) · `thread/start` (the session's parameters, `ephemeral: true`). **No `turn/start`, no
// login method**, ever (`rpc.ts` would refuse a login anyway). Each answer is checked as it comes
// (`probe-check.ts`), so the first failure stops the probe.
//
// **Signed in by other means** (`account/read` not `null`: a managed `CODEX_API_KEY`, a keyring):
// the probe stops there, sends nothing more, and caches `initPending`; the first real instance's
// `checkInstance` completes it (`completeProbe`), never a paid turn (7D Decision 8's rule).
//
// Decision 21 / owner Q8(b): the probe has no store either way (it never links `auth.json`), and
// no step here reads the store layout, so a rename finding changes nothing in this file.
import { createHash, randomBytes } from "node:crypto";
import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import type { FinError, Session } from "@finisher/contracts";
import { Effect, Schema } from "effect";
import { finError, toFinError } from "../../errors.ts";
import type { SessionSpec } from "../adapter.ts";
import { discoverCodexModels } from "../catalogue/cli-models.ts";
import { openHostSandbox, type SandboxedChild, type SessionSandbox, type SessionSandboxDeps } from "../sandbox.ts";
import { hostEnv } from "../sandbox-env.ts";
import { providerDirClean, sessionDirs } from "../session-dirs.ts";
import { type CodexLaunch, codexLaunch, codexPinFacts } from "./launch.ts";
import { checkConfig, checkInitialize, checkInstance, checkLists, checkRequirements, checkThread, type InstanceFacts, threadStartRefused, versionError } from "./probe-check.ts";
import type { CodexProbe } from "./probe-types.ts";
import { type CodexRpc, codexRpc } from "./rpc.ts";

export type { CodexProbe } from "./probe-types.ts";
export { checkInstance, type InstanceFacts, type InstanceItem, instanceGate } from "./probe-check.ts";

/** A probe's result: `initPending` when it stopped at a sign-in it did not make. */
export interface CodexProbeResult extends CodexProbe {
  readonly initPending?: true;
}

export interface CodexProbeDeps {
  readonly sandbox: SessionSandboxDeps;
  /** The pinned binary (default `codexBinary()`, through `codexLaunch`). */
  readonly binary?: string;
  readonly os?: NodeJS.Platform;
  /** How long the whole exchange may take (default 30 s). */
  readonly timeoutMs?: number;
}

export interface ProbeKey {
  readonly version: string;
  readonly flagsHash: string;
  readonly os: NodeJS.Platform;
}

/** The probe's role: a shell (so `dynamicTools` are declared and must be taken) and patches. */
const PROBE_TOOLS = ["Bash", "Edit"] as const;
const PROBE_PROMPT = "Finisher capability probe.";
const CLIENT = { name: "finisher", version: "probe" } as const;

// ---- the cache ----

const Str = Schema.String;
const CachedResult = Schema.Struct({
  version: Str,
  sandbox: Schema.Literals(["read-only", "danger-full-access"]),
  approval: Schema.Literals(["on-request", "granular"]),
  models: Schema.Array(Schema.Struct({ id: Str, displayName: Str, isDefault: Schema.Boolean, alias: Schema.optionalKey(Str) })),
  initPending: Schema.optionalKey(Schema.Literal(true)),
});
const CacheEntry = Schema.Struct({ version: Str, flagsHash: Str, os: Str, result: CachedResult });
const CacheFile = Schema.Struct({ entries: Schema.Array(CacheEntry) });
type CacheEntry = typeof CacheEntry.Type;
const decodeCache = Schema.decodeUnknownResult(CacheFile);

export const codexProbeCachePath = (serverData: string): string => join(serverData, "agents", "codex-probe.json");

/** The cache's entries; an absent, unreadable or invalid file (an excess key included) is empty. */
export async function readCodexProbeCache(serverData: string): Promise<readonly CacheEntry[]> {
  let value: unknown;
  try {
    value = JSON.parse(await readFile(codexProbeCachePath(serverData), "utf8"));
  } catch {
    return [];
  }
  const r = decodeCache(value, { onExcessProperty: "error" });
  return r._tag === "Success" ? r.success.entries : [];
}

const sameKey = (e: Pick<CacheEntry, "version" | "flagsHash" | "os">, k: ProbeKey) => e.version === k.version && e.flagsHash === k.flagsHash && e.os === k.os;

/** One read-modify-write at a time in this process, so concurrent probes keep every entry. */
let cacheWrites: Promise<unknown> = Promise.resolve();

/** Replaces `key`'s entry with `result`, temp then rename (0600). Only `CachedResult`'s fields are written. */
function writeCodexProbeCache(serverData: string, key: ProbeKey, result: CodexProbeResult): Promise<void> {
  const write = async () => {
    const r = Schema.decodeUnknownSync(CachedResult)({ ...result, models: result.models.map(({ id, displayName, isDefault, alias }) => ({ id, displayName, isDefault, ...(alias === undefined ? {} : { alias }) })) }, { onExcessProperty: "ignore" });
    const entries = [...(await readCodexProbeCache(serverData)).filter((e) => !sameKey(e, key)), { ...key, result: r }];
    const path = codexProbeCachePath(serverData);
    await mkdir(dirname(path), { recursive: true, mode: 0o700 });
    const tmp = `${path}.${randomBytes(6).toString("hex")}.tmp`;
    try {
      await writeFile(tmp, `${JSON.stringify({ entries }, null, 2)}\n`, { mode: 0o600, flag: "wx" });
      await rename(tmp, path);
    } catch (e) {
      await rm(tmp, { force: true }).catch(() => 0);
      throw e;
    }
  };
  const next = cacheWrites.then(write);
  cacheWrites = next.catch(() => 0);
  return next;
}

// ---- the launch and its hash ----

/**
 * The hash of the flags a probe vouches for: the argv without the binary (the per-session
 * `shell_environment_policy.set` table masked: it is the session environment, not a flag), what
 * the process adds to the environment, and the thread parameters that are not per session.
 * Changing a `-c` key, a `--disable`, the sandbox or the dynamic tools probes again.
 */
export function flagsHashOf(l: Pick<CodexLaunch, "argv" | "env" | "startParams">): string {
  const argv = l.argv.slice(1).map((a) => (a.startsWith("shell_environment_policy.set=") ? "shell_environment_policy.set=<session>" : a));
  const { cwd: _c, model: _m, developerInstructions: _d, ...thread } = l.startParams;
  const env = Object.entries(l.env).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return createHash("sha256").update(JSON.stringify({ argv, env, thread })).digest("hex").slice(0, 16);
}

const SEED: CodexProbe = { version: "", sandbox: "read-only", approval: "on-request", models: [] };

/**
 * The probe instance's launch in `box` (a host sandbox): the probe role's `codexLaunch`, held to
 * the host environment contract. Decision 7's fallback (`danger-full-access` with granular
 * approvals) is not probed: TODO(7E Task 0 Step 3/5) — whether Codex's read-only sandbox starts
 * inside Finisher's decides it.
 */
export function probeLaunch(box: Pick<SessionSandbox, "env" | "workspace">, os: NodeJS.Platform, binary?: string): CodexLaunch {
  const id = box.workspace.split(/[\\/]/).at(-1)!;
  const session = { id, harness: "codex" } as Session;
  const spec: SessionSpec = { sessionId: id, projectId: "probe", model: "", role: "probe", mode: "plan", system: PROBE_PROMPT, cwd: box.workspace, env: box.env, providerRef: null, tools: [...PROBE_TOOLS] };
  return codexLaunch(session, spec, { ...SEED, version: codexPinFacts().version }, os, { host: true, ...(binary === undefined ? {} : { binary }) });
}

/** The flags hash of this build's probe launch for `os` (paths and the host id fixed). */
export function codexProbeFlagsHash(os: NodeJS.Platform = process.platform): string {
  const root = os === "win32" ? "C:\\fin" : "/fin";
  const id = `host-${"0".repeat(26)}`;
  const dirs = sessionDirs(os === "win32" ? `${root}\\agents` : `${root}/agents`, id, os);
  const parent = os === "win32" ? { Path: "C:\\Windows\\System32", SystemRoot: "C:\\Windows" } : { PATH: "/usr/bin:/bin" };
  const env = hostEnv({ harness: "codex", dirs, platform: os, parent });
  const workspace = os === "win32" ? `${root}\\project\\workspace\\${id}` : `${root}/project/workspace/${id}`;
  return flagsHashOf(probeLaunch({ env, workspace }, os, os === "win32" ? "codex.exe" : "codex"));
}

// ---- the exchange ----

/** What one probe instance found: the facts and models, or a sign-in it stopped at. */
export type ProbeOutcome = { readonly kind: "deferred" } | { readonly kind: "done"; readonly facts: InstanceFacts; readonly models: CodexProbe["models"] };

const run = <A>(e: Effect.Effect<A, FinError>): Promise<A> =>
  Effect.runPromise(Effect.result(e)).then((r) => {
    if (r._tag === "Failure") throw r.failure;
    return r.success;
  });
const ok = (e: FinError | null): void => {
  if (e !== null) throw e;
};

/**
 * The probe's methods over `rpc`, each answer checked before the next is sent (throws the first
 * failure). `codexHome` is the instance's `CODEX_HOME`. Sends no `turn/start` and no login.
 */
export async function probeInstance(rpc: CodexRpc, launch: CodexLaunch, codexHome: string): Promise<ProbeOutcome> {
  const pinned = codexPinFacts().version;
  const initialize = await run(rpc.request("initialize", { clientInfo: CLIENT, capabilities: { experimentalApi: true } }));
  ok(checkInitialize(initialize, pinned, codexHome));
  await run(rpc.notify("initialized"));
  const config = (await run(rpc.requestRaw("config/read", { includeLayers: true }))).raw;
  ok(checkConfig(config, launch, codexHome));
  const requirements = (await run(rpc.requestRaw("configRequirements/read", undefined))).raw;
  ok(checkRequirements(requirements, launch));
  const cwds = [launch.startParams.cwd!];
  const lists = {
    skills: await run(rpc.request("skills/list", { cwds })),
    hooks: await run(rpc.request("hooks/list", { cwds })),
    plugins: await run(rpc.request("plugin/installed", {})),
    mcpServers: await run(rpc.request("mcpServerStatus/list", {})),
    apps: await run(rpc.request("app/installed", {})),
  };
  ok(checkLists(lists));
  const account = await run(rpc.request("account/read", {}));
  // Signed in by something Finisher did not link: nothing more is sent (no thread, no turn).
  if (account.account !== null && account.account !== undefined) return { kind: "deferred" };
  const models = await discoverCodexModels((_m, p) => run(rpc.request("model/list", p)));
  const model = models.find((m) => m.isDefault)?.id;
  const { model: _, ...start } = launch.startParams;
  const thread = await run(rpc.request("thread/start", { ...start, ...(model === undefined ? {} : { model }), ephemeral: true }).pipe(Effect.mapError(threadStartRefused)));
  ok(checkThread(thread, launch));
  return { kind: "done", facts: { initialize, config, requirements, ...lists, thread, codexHome }, models };
}

/** `work` against a clock; on the timeout the child is killed. */
async function within<A>(child: SandboxedChild, ms: number, work: Promise<A>): Promise<A> {
  let timer: NodeJS.Timeout | undefined;
  const late = new Promise<"timeout">((resolve) => (timer = setTimeout(() => resolve("timeout"), ms)));
  try {
    const r = await Promise.race([work, late]);
    if (r === "timeout") {
      child.kill("SIGKILL");
      throw versionError("answer", "Codex did not answer the capability probe in time", { timeoutMs: ms });
    }
    return r;
  } finally {
    clearTimeout(timer);
  }
}

/** One probe instance in `box`: hygiene, spawn, the exchange, then the group killed. */
async function runInstance(box: SessionSandbox, deps: CodexProbeDeps): Promise<CodexProbeResult> {
  const os = deps.os ?? process.platform;
  const codexHome = box.env.CODEX_HOME;
  if (codexHome === undefined) throw finError("SANDBOX_ENV", "the probe sandbox has no CODEX_HOME", { name: "CODEX_HOME" });
  await providerDirClean({ provider: codexHome });
  const launch = probeLaunch(box, os, deps.binary);
  const binary = launch.argv[0]!;
  const child = await run(box.spawn(launch.argv, { cwd: box.workspace, stdio: "pipe", env: launch.env, extraRead: [binary] }));
  child.stderr.resume();
  const rpc = codexRpc({ stdin: child.stdin, stdout: child.stdout });
  try {
    const out = await within(child, deps.timeoutMs ?? 30_000, probeInstance(rpc, launch, codexHome));
    const base = { version: codexPinFacts().version, sandbox: "read-only", approval: "on-request" } as const;
    return out.kind === "deferred" ? { ...base, models: [], initPending: true } : { ...base, models: out.models };
  } finally {
    rpc.end();
    child.kill("SIGKILL");
  }
}

/**
 * The capability probe (Decision 8): the cached result for `(pinned version, flagsHash, OS)`, else
 * a probe run and cached. `PROVIDER_VERSION {missing}` when the binary lacks what Finisher needs
 * or loads what it did not name; `{version, pinned}` for another version than the pin.
 */
export function probeCodex(flagsHash: string, deps: CodexProbeDeps): Effect.Effect<CodexProbeResult, FinError> {
  const os = deps.os ?? process.platform;
  const key: ProbeKey = { version: codexPinFacts().version, flagsHash, os };
  const serverData = deps.sandbox.serverData;
  const attempt = <A>(f: () => Promise<A>) => Effect.tryPromise({ try: f, catch: toFinError });
  return Effect.gen(function* () {
    const cached = (yield* attempt(() => readCodexProbeCache(serverData))).find((e) => sameKey(e, key));
    if (cached !== undefined) return cached.result as CodexProbeResult;
    const result = yield* Effect.scoped(
      Effect.gen(function* () {
        const box = yield* openHostSandbox("probe", "codex", deps.sandbox);
        return yield* attempt(() => runInstance(box, deps));
      }),
    );
    yield* attempt(() => writeCodexProbeCache(serverData, key, result));
    return result;
  });
}

/**
 * Completes a probe left `initPending` from the first real instance: `checkInstance` on its facts
 * against its plan, then the entry loses the mark and takes the instance's models (Decision 10:
 * an account may list others). A complete probe is returned as cached; a missing one is `INTERNAL`.
 */
export function completeProbe(serverData: string, key: ProbeKey, facts: InstanceFacts, plan: CodexLaunch, models: CodexProbe["models"]): Effect.Effect<CodexProbeResult, FinError> {
  return Effect.tryPromise({
    try: async () => {
      const entry = (await readCodexProbeCache(serverData)).find((e) => sameKey(e, key));
      if (entry === undefined) throw finError("INTERNAL", "no Codex probe to complete", { version: key.version });
      ok(checkInstance(facts, plan));
      if (entry.result.initPending !== true) return entry.result as CodexProbeResult;
      const { initPending: _, ...rest } = entry.result;
      const result: CodexProbeResult = { ...rest, models };
      await writeCodexProbeCache(serverData, key, result);
      return result;
    },
    catch: toFinError,
  });
}
