// The Claude Code capability probe (plan 7D Task 5, Decision 8), per `(version, flags hash, OS)`
// and cached in `<server-data>/agents/claude-probe.json`. It runs the detected CLI with the
// session's flags through a **host sandbox** (`openHostSandbox("probe")`: a throwaway config
// dir, never the sign-in store variable) and costs nothing:
//
// 1. **`initialize` only:** it must list `models`, `commands: []`, `capabilities`, the detected
//    `claude_code_version`, and `account.tokenSource: "none"`. A CLI signed in by other means (a
//    managed `apiKeyHelper` or `env`, a cloud provider's variables, a keychain item) gets **no
//    user line**: the result is marked `initPending`, and the first real instance's init
//    completes it (`completeProbe`), never a paid prompt.
// 2. **Unsigned only:** one user line under `--max-budget-usd 0.01` (a second guard); the CLI
//    answers with `system/init` and the "Not logged in" `result {is_error: true}` at
//    `total_cost_usd: 0` (2.1.321 writes `subtype: "success"` there, so the probe keys on
//    `is_error`). The init must pass `checkInit` with the settings canary; then the CLI is killed.
//
// The permission path (Decision 10): `stdio` first; a version that refuses the value (it exits
// before answering `initialize`, naming `--permission-prompt-tool`) is probed again with the
// documented MCP form (`mcp__finisher__approve`); a version that refuses both is
// `PROVIDER_VERSION {missing: "permissionPrompt"}`. The `finisher` SDK server's handshake is
// answered at once (Task 0: it starts before the `initialize` answer).
//
// The cache holds no account data (no token source, label or provider): an Effect Schema with
// excess keys refused validates it on read, and an invalid file is probed over.
import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { Effect, Schema } from "effect";
import { finError, toFinError } from "../../errors.ts";
import type { Detection } from "../adapter.ts";
import { openHostSandbox, type SandboxedChild, type SessionSandbox, type SessionSandboxDeps } from "../sandbox.ts";
import { sessionDirs } from "../session-dirs.ts";
import { APPROVE_TOOL, CLI_TOOLS, cliTools, instancePlan, type LaunchPlan, writeLaunchFiles } from "./launch.ts";
import { checkInit, checkInitialize, type CheckOptions, mcpReply, versionError } from "./probe-check.ts";
import type { ProbeResult } from "./probe-types.ts";
import { type CliLine, decodeLine, encodeAnswer, encodeControl, encodeUser, lineFramer, type ResultMsg, type SystemInit } from "./protocol.ts";

export type { ProbeResult } from "./probe-types.ts";
export { checkInit, firstTurnGate, untestedVersionNotices } from "./probe-check.ts";

export interface ProbeDeps {
  readonly sandbox: SessionSandboxDeps;
  /** The detected CLI, absolute (`agents.claudeCode.path` or `PATH`, resolved). */
  readonly binary: string;
  readonly os?: NodeJS.Platform;
  /** The CLI's install tree (Decision 7: `extraRead`). */
  readonly extraRead?: readonly string[];
  /** How long each answer may take (default 30 s). */
  readonly timeoutMs?: number;
}

/** Where a probe result is cached. */
export interface ProbeKey {
  readonly version: string;
  readonly flagsHash: string;
  readonly os: NodeJS.Platform;
}

/** The probe's prompt: never answered (the CLI is not signed in), never paid (`--max-budget-usd`). */
const PROBE_PROMPT = "Reply with OK.";
const PROBE_BUDGET_USD = "0.01";
const INITIALIZE_ID = "probe-initialize";

// ---- the cache ----

const Str = Schema.String;
const CachedResult = Schema.Struct({
  version: Str,
  permissionPath: Schema.Literals(["stdio", "mcp"]),
  tools: Schema.Array(Str),
  models: Schema.Array(Schema.Struct({ id: Str, displayName: Str, isDefault: Schema.Boolean })),
  capabilities: Schema.Array(Str),
  restricted: Schema.optionalKey(Schema.Boolean),
  brokeredShell: Schema.optionalKey(Schema.Boolean),
  initPending: Schema.optionalKey(Schema.Literal(true)),
});
const CacheEntry = Schema.Struct({ version: Str, flagsHash: Str, os: Str, result: CachedResult });
const CacheFile = Schema.Struct({ entries: Schema.Array(CacheEntry) });
type CacheEntry = typeof CacheEntry.Type;
const decodeCache = Schema.decodeUnknownResult(CacheFile);

export const probeCachePath = (serverData: string): string => join(serverData, "agents", "claude-probe.json");

/** The cache's entries; an absent, unreadable or invalid file is empty (the probe runs again). */
export async function readProbeCache(serverData: string): Promise<readonly CacheEntry[]> {
  let raw: string;
  try {
    raw = await readFile(probeCachePath(serverData), "utf8");
  } catch {
    return [];
  }
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return [];
  }
  const r = decodeCache(value, { onExcessProperty: "error" });
  return r._tag === "Success" ? r.success.entries : [];
}

const sameKey = (e: Pick<CacheEntry, "version" | "flagsHash" | "os">, k: ProbeKey) => e.version === k.version && e.flagsHash === k.flagsHash && e.os === k.os;

/** Replaces `key`'s entry with `result` (temp-then-rename, 0600). Only the fields of `CachedResult` are written. */
async function writeProbeCache(serverData: string, key: ProbeKey, result: ProbeResult): Promise<void> {
  const r = Schema.decodeUnknownSync(CachedResult)(result, { onExcessProperty: "ignore" });
  const entries = [...(await readProbeCache(serverData)).filter((e) => !sameKey(e, key)), { ...key, result: r }];
  await mkdir(dirname(probeCachePath(serverData)), { recursive: true, mode: 0o700 });
  await writeLaunchFiles({ files: [{ path: probeCachePath(serverData), body: `${JSON.stringify({ entries }, null, 2)}\n` }] });
}

/**
 * The hash of the launch flags a probe vouches for: the argv and settings `instancePlan` builds
 * for `os`, with paths and the session id fixed. A Finisher release that changes a flag, the
 * tool lists or the settings probes again.
 */
export function probeFlagsHash(os: NodeJS.Platform = process.platform): string {
  const root = os === "win32" ? "C:\\fin" : "/fin";
  const plan = probePlan("claude", sessionDirs(root, "probe", os), os, "stdio", "00000000-0000-0000-0000-000000000000");
  return createHash("sha256").update(JSON.stringify({ argv: plan.argv.slice(1), files: plan.files.map((f) => f.body) })).digest("hex").slice(0, 16);
}

// ---- the process ----

/** The probe instance's plan: every CLI tool Finisher maps, the canary, a cent of budget. */
function probePlan(binary: string, dirs: ReturnType<typeof sessionDirs>, os: NodeJS.Platform, path: "stdio" | "mcp", uuid: string): Omit<LaunchPlan, "dropped"> {
  const { tools, shell } = cliTools([...CLI_TOOLS], false);
  return instancePlan({
    binary,
    dirs,
    os,
    uuid,
    resume: false,
    tools,
    shell,
    probe: { permissionPath: path },
    model: "default",
    system: "Finisher capability probe.",
    retentionDays: 1,
    budgetUsd: PROBE_BUDGET_USD,
    canary: true,
  });
}

/** The host sandbox's own directories, from its `TMPDIR` (`<agents>/host-<ulid>/tmp`). */
export function hostDirs(box: Pick<SessionSandbox, "env">, os: NodeJS.Platform): { id: string; dirs: ReturnType<typeof sessionDirs> } {
  const tmp = box.env.TMPDIR;
  if (tmp === undefined) throw finError("SANDBOX_ENV", "the host sandbox has no TMPDIR", { name: "TMPDIR" });
  const id = basename(dirname(tmp));
  const dirs = sessionDirs(dirname(dirname(tmp)), id, os);
  if (dirs.tmp !== tmp) throw finError("INTERNAL", "the host sandbox's directories do not match its TMPDIR", {});
  return { id, dirs };
}

/**
 * The lines of one instance, decoded, with the `finisher` server's handshake answered at once
 * (`mcpReply`) and every other CLI request answered `error` (the probe serves no tool). `next`
 * waits for a line matching `pred`; it rejects when the process exits first (with its stderr),
 * on a line that does not decode, or after `timeoutMs`.
 */
export class InstanceLines {
  readonly lines: CliLine[] = [];
  stderr = "";
  #exit: { code: number | null; signal: string | null } | null = null;
  #error: FinError | null = null;
  #wake: (() => void)[] = [];
  readonly #child: SandboxedChild;

  constructor(child: SandboxedChild, mcpTools: readonly string[], answerOthers = true) {
    this.#child = child;
    const framer = lineFramer();
    const take = (raws: (string | FinError)[]) => {
      for (const raw of raws) {
        const l = typeof raw === "string" ? decodeLine(raw) : raw;
        if (!("kind" in l)) {
          this.#error ??= typeInitError(raw) ?? l;
          continue;
        }
        if (l.kind === "controlRequest") {
          const r = l.request;
          if (r.subtype === "mcp_message") child.stdin.write(encodeAnswer(l.request_id, mcpReply(r.server_name, r.message, mcpTools)));
          else if (answerOthers) child.stdin.write(encodeAnswer(l.request_id, { error: "not served by this host" }));
        }
        this.lines.push(l);
      }
      this.#notify();
    };
    child.stdout.on("data", (b: Buffer) => take(framer.push(b)));
    child.stdout.on("end", () => take(framer.end()));
    child.stderr.on("data", (b: Buffer) => {
      if (this.stderr.length < 8192) this.stderr += String(b);
    });
    child.stdin.on("error", () => 0);
    void child.exited.then((e) => {
      this.#exit = { code: e.code, signal: e.signal ?? null };
      this.#notify();
    });
  }

  #notify() {
    for (const w of this.#wake.splice(0)) w();
  }

  write(line: string): void {
    this.#child.stdin.write(line);
  }

  /** The first line (from the start) matching `pred`. */
  async next<L extends CliLine>(pred: (l: CliLine) => l is L, timeoutMs: number): Promise<L | { exited: { code: number | null; signal: string | null } }> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const hit = this.lines.find(pred);
      if (hit !== undefined) return hit;
      if (this.#error !== null) throw this.#error;
      if (this.#exit !== null) return { exited: this.#exit };
      const left = deadline - Date.now();
      if (left <= 0) throw finError("PROVIDER_VERSION", "the Claude Code CLI did not answer the probe in time", { harness: "claudeCode", missing: "answer", timeoutMs });
      await new Promise<void>((resolve) => {
        const t = setTimeout(resolve, left);
        this.#wake.push(() => {
          clearTimeout(t);
          resolve();
        });
      });
    }
  }
}

/** A `system/init` line that fails to decode for want of a key: `PROVIDER_VERSION {missing: <key>}`. */
function typeInitError(raw: string | FinError): FinError | null {
  if (typeof raw !== "string") return null;
  let o: Record<string, unknown>;
  try {
    o = JSON.parse(raw) as Record<string, unknown>;
  } catch {
    return null;
  }
  if (o?.type !== "system" || o.subtype !== "init") return null;
  for (const k of ["tools", "model", "session_id"]) if (o[k] === undefined) return versionError(k, `the Claude Code init has no ${k}`, { item: k });
  return null;
}

const isInitAnswer = (l: CliLine): l is Extract<CliLine, { kind: "controlResponse" }> => l.kind === "controlResponse" && l.response.request_id === INITIALIZE_ID;
const isInit = (l: CliLine): l is SystemInit => l.kind === "init";
const isResult = (l: CliLine): l is ResultMsg => l.kind === "result";

type RunOutcome = { readonly kind: "refusedPath" } | { readonly kind: "done"; readonly result: ProbeResult };

/** One probe instance in `box` with permission path `path`. */
async function runInstance(box: SessionSandbox, detected: string, path: "stdio" | "mcp", deps: ProbeDeps): Promise<RunOutcome> {
  const os = deps.os ?? process.platform;
  const timeoutMs = deps.timeoutMs ?? 30_000;
  const { dirs } = hostDirs(box, os);
  const uuid = randomUUID();
  const plan = probePlan(deps.binary, dirs, os, path, uuid);
  await writeLaunchFiles(plan, os);
  const child = await Effect.runPromise(
    Effect.result(box.spawn(plan.argv, { cwd: box.workspace, stdio: "pipe", env: plan.env, ...(deps.extraRead === undefined ? {} : { extraRead: deps.extraRead }) })),
  ).then((r) => {
    if (r._tag === "Failure") throw r.failure;
    return r.success;
  });
  const io = new InstanceLines(child, plan.mcpTools);
  try {
    io.write(encodeControl(INITIALIZE_ID, { subtype: "initialize" }));
    const answer = await io.next(isInitAnswer, timeoutMs);
    if ("exited" in answer) {
      // A version without this path names the flag and its value as it exits (Decision 10).
      if (/permission-prompt-tool/i.test(io.stderr) && io.stderr.includes(path === "stdio" ? "stdio" : APPROVE_TOOL)) return { kind: "refusedPath" };
      throw versionError("initialize", "the Claude Code CLI exited before answering initialize", { version: detected, code: answer.exited.code, signal: answer.exited.signal });
    }
    if (answer.response.subtype !== "success") throw versionError("initialize", "the Claude Code CLI refused initialize", { version: detected });
    const facts = checkInitialize(answer.response.response, detected);
    if (!("models" in facts)) throw facts;
    const base = { version: detected, permissionPath: path, models: facts.models };
    // Signed in by other means: no user line, ever; the first real instance completes the probe.
    if (facts.tokenSource !== "none") return { kind: "done", result: { ...base, tools: [], capabilities: [...facts.capabilities], initPending: true } };
    io.write(encodeUser([PROBE_PROMPT], uuid));
    const init = await io.next(isInit, timeoutMs);
    if ("exited" in init) throw versionError("init", "the Claude Code CLI exited before its init", { version: detected, code: init.exited.code });
    const bad = checkInit(init, plan, { signedIn: false, canary: true });
    if (bad !== null) throw bad;
    const result = await io.next(isResult, timeoutMs);
    if ("exited" in result) throw versionError("result", "the Claude Code CLI exited before its result", { version: detected, code: result.exited.code });
    if (!result.is_error || result.total_cost_usd !== 0) {
      throw versionError("unsigned", "the Claude Code probe turn was answered or charged: the CLI is signed in by other means", { version: detected, cost: result.total_cost_usd });
    }
    const capabilities = [...new Set([...facts.capabilities, ...(init.capabilities ?? [])])];
    return { kind: "done", result: { ...base, tools: [...init.tools], capabilities } };
  } finally {
    child.stdin.end();
    child.kill("SIGKILL");
  }
}

/**
 * The capability probe for the detected CLI (Decision 8): the cached result for `(version,
 * flagsHash, OS)`, else a probe run and its result cached. `PROVIDER_VERSION {missing}` when the
 * CLI lacks what Finisher needs; `PROVIDER_UNAVAILABLE` when nothing is detected.
 */
export function probe(detected: Detection, flagsHash: string, deps: ProbeDeps): Effect.Effect<ProbeResult, FinError> {
  const os = deps.os ?? process.platform;
  const version = detected.version;
  if (!detected.installed || version === null) {
    return Effect.fail(finError("PROVIDER_UNAVAILABLE", "Claude Code is not installed", { harness: "claudeCode" }));
  }
  const key: ProbeKey = { version, flagsHash, os };
  const serverData = deps.sandbox.serverData;
  const attempt = <A>(f: () => Promise<A>) => Effect.tryPromise({ try: f, catch: toFinError });
  return Effect.gen(function* () {
    const cached = (yield* attempt(() => readProbeCache(serverData))).find((e) => sameKey(e, key));
    if (cached !== undefined) return cached.result as ProbeResult;
    const result = yield* Effect.scoped(
      Effect.gen(function* () {
        const box = yield* openHostSandbox("probe", "claudeCode", deps.sandbox);
        const first = yield* attempt(() => runInstance(box, version, "stdio", deps));
        if (first.kind === "done") return first.result;
        // Decision 10: this version refuses `stdio`; the documented MCP form is the path.
        const second = yield* attempt(() => runInstance(box, version, "mcp", deps));
        if (second.kind === "done") return second.result;
        return yield* Effect.fail(versionError("permissionPrompt", "the Claude Code CLI accepts no permission prompt path", { version }));
      }),
    );
    yield* attempt(() => writeProbeCache(serverData, key, result));
    return result;
  });
}

/**
 * Completes a probe left `initPending` (a CLI signed in by other means) from the first real
 * instance's init: `checkInit` against that instance's plan, then the cached entry gains its
 * tools. A probe already complete is returned as cached; a missing entry is `INTERNAL`.
 */
export function completeProbe(serverData: string, key: ProbeKey, init: SystemInit, plan: Pick<LaunchPlan, "argv" | "tools" | "mcpTools">, opts: CheckOptions = {}): Effect.Effect<ProbeResult, FinError> {
  return Effect.tryPromise({
    try: async () => {
      const entry = (await readProbeCache(serverData)).find((e) => sameKey(e, key));
      if (entry === undefined) throw finError("INTERNAL", "no Claude Code probe to complete", { version: key.version });
      const bad = checkInit(init, plan, opts);
      if (bad !== null) throw bad;
      if (entry.result.initPending !== true) return entry.result as ProbeResult;
      const { initPending: _, ...rest } = entry.result;
      const capabilities = [...new Set([...rest.capabilities, ...(init.capabilities ?? [])])];
      const result: ProbeResult = { ...rest, tools: [...init.tools], capabilities };
      await writeProbeCache(serverData, key, result);
      return result;
    },
    catch: toFinError,
  });
}
