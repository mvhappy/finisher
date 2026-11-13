// Claude Code's detection, its line in `agent.providers` and the sign-in commands (plan 7D
// Decisions 9, 22, 23; spec §4.5, §8.6).
//
// - **The binary**: `agents.claudeCode.path` (an absolute path), else `claude` on the server's
//   `PATH`, resolved through absolute entries only (a relative entry names whatever folder the
//   server happens to run in). On Windows only the native build (`claude.exe`) is used: an npm
//   shim (`claude.cmd`, `.bat`, `.ps1`) or a Node script needs a shell, and Node in the container
//   hits `WINDOWS_NODE_PIPES`, so it is refused `PROVIDER_UNAVAILABLE` with the fix "install the
//   native build".
// - **The version**: `claude --version` must print `<x.y.z> (Claude Code)` (Task 0: `2.1.321
//   (Claude Code)`) within 10 s; anything else is `PROVIDER_PROTOCOL`. Below the floor
//   (`CLAUDE_FLOOR`, data changed per Finisher release) is `PROVIDER_VERSION {version, floor}`.
// - **Signed in**: `claude auth status --json`, whose JSON answer Finisher reads (the CLI reads
//   its own store). Task 0: it exits 1 when signed out, which is an answer, not a failure. Only
//   `loggedIn` and a one-word `authMethod` are kept: never a label, an e-mail or a token.
//
// Both commands run in a host sandbox (`openHostSandbox("detect")`): a throwaway
// `CLAUDE_CONFIG_DIR` (`auth status` writes `.claude.json` and `backups/` there) and the sign-in
// store variable, so the CLI reads its own store. **Finisher opens nothing under `providers/`**:
// the sign-in commands below are printed for a person to run, and run nothing here.
import { realpathSync, statSync, accessSync, constants } from "node:fs";
import { posix, win32 } from "node:path";
import type { FinError, ProviderInfo } from "@finisher/contracts";
import { Effect, Exit } from "effect";
import { safe } from "../../console/safe.ts";
import { finError } from "../../errors.ts";
import type { Capabilities } from "../adapter.ts";
import { openHostSandbox, type SandboxedChild, type SessionSandbox, type SessionSandboxDeps } from "../sandbox.ts";
import { type ClaudeCodeSettings, claudeCodeSettings } from "../settings.ts";
import { accountChanged, accountCheck, accountKey, ClaudeAccounts, labelOfOutput } from "./account.ts";

/**
 * The lowest supported version (Decision 9): at least 2.1.314 (the `--setting-sources` leak fix)
 * and the version Task 0 validated, 2.1.321. Task 15's contract suite may raise it.
 */
export const CLAUDE_FLOOR = "2.1.321";

/** What `claude --version` prints (Decision 22). */
export const VERSION_LINE = /^(\d+\.\d+\.\d+) \(Claude Code\)$/;

/** How long `--version` and `auth status` may each take. */
export const DETECT_TIMEOUT_MS = 10_000;

/** What detection reads of a command's output at most. */
const OUTPUT_CAP = 64 * 1024;

/**
 * The sign-in methods kept by name; any other answer is `"other"`, never the CLI's text (it could
 * be a label). `claude.ai` and `console` are what the CLI documents. ASSUMPTION (plan Task 0's
 * signed-in half confirms the real values): `oauth` and `apiKey` are not known to be printed, so
 * they are not listed; add a value here once Task 0 sees it.
 */
const AUTH_METHODS: ReadonlySet<string> = new Set(["claude.ai", "console"]);

/** How long an `auth status` answer is reused. A sign-in/out is printed for a person to run in a separate console process, which cannot reach this cache: this TTL covers it. */
export const AUTH_TTL_MS = 30_000;

/** What sessions on Claude Code can do (spec §4.1; the session tasks may refine it). */
export const CLAUDE_CAPABILITIES: Capabilities = {
  steer: true,
  rollback: false,
  resume: true,
  reasoning: true,
  images: false,
  parallelTools: true,
  costReported: true,
  exactCapture: false,
  nativeSubagents: false,
};

export interface ClaudeDetection {
  /** The CLI as found (a `PATH` entry or the setting), links not resolved: what a person runs. */
  readonly path: string;
  readonly version: string;
  readonly floor: string;
  /** `null` when `auth status` gave no usable answer (`problem` says why). */
  readonly signedIn: boolean | null;
  readonly authMethod?: string;
  /** The signed-in account's keyed hash (`account.ts`); the label itself is never kept. */
  readonly accountHash?: string;
  readonly problem?: FinError;
}

export interface ResolveInput {
  /** `agents.claudeCode.path`. */
  readonly setting?: string | undefined;
  /** The server's `PATH`. */
  readonly pathEnv?: string | undefined;
  readonly platform: NodeJS.Platform;
  /** Whether `p` is a file this account may run (tests: a fake file system). */
  readonly isFile?: (p: string) => boolean;
}

export interface ClaudeDetectDeps {
  /** The host sandbox's dependencies (`serverData` is the server data, links resolved). */
  readonly sandbox: SessionSandboxDeps;
  /** `agents.claudeCode` (default: read from `settings.json` at each call). */
  readonly settings?: () => ClaudeCodeSettings;
  /** The server's `PATH` (default: this process's). */
  readonly pathEnv?: string;
  readonly platform?: NodeJS.Platform;
  readonly isFile?: (p: string) => boolean;
  /** Resolves links of the found path (default `realpathSync`; tests). */
  readonly realpath?: (p: string) => string;
  readonly timeoutMs?: number;
  /** The clock for the `auth status` cache (default `Date.now`; tests). */
  readonly now?: () => number;
}

const HARNESS = "claudeCode";
const NATIVE_FIX = "install the native build";
const INSTALL_FIX = "install Claude Code's native build, or set agents.claudeCode.path in settings.json to its absolute path";

const pathOf = (platform: NodeJS.Platform) => (platform === "win32" ? win32 : posix);

/** Absolute for `platform`; on Windows with a drive or a UNC share (a rooted `\x` depends on the current drive). */
function absolute(p: string, platform: NodeJS.Platform): boolean {
  return platform === "win32" ? /^[A-Za-z]:[\\/]/.test(p) || /^\\\\[^\\]/.test(p) : posix.isAbsolute(p);
}

function defaultIsFile(platform: NodeJS.Platform): (p: string) => boolean {
  return (p) => {
    try {
      if (!statSync(p).isFile()) return false;
      if (platform !== "win32") accessSync(p, constants.X_OK);
      return true;
    } catch {
      return false;
    }
  };
}

const notInstalled = (reason: string) => finError("PROVIDER_UNAVAILABLE", "Claude Code is not installed on this server", { harness: HARNESS, reason }, INSTALL_FIX);

/** Windows: only `claude.exe` (the native build); a shim or a script is refused. */
function checkNative(p: string, platform: NodeJS.Platform): void {
  if (platform !== "win32") return;
  if (win32.extname(p).toLowerCase() !== ".exe") {
    throw finError("PROVIDER_UNAVAILABLE", "this Claude Code is an npm shim or a Node script, which Finisher cannot run in its sandbox", { harness: HARNESS, reason: "npmShim" }, NATIVE_FIX);
  }
}

/** On Windows, what a `PATH` folder may hold under the name `claude`, in `PATHEXT`'s order. */
const WINDOWS_NAMES = ["claude.com", "claude.exe", "claude.bat", "claude.cmd", "claude.ps1", "claude.js"];

/**
 * The CLI to run: `setting` (absolute, else refused), else the first `claude` in the absolute
 * entries of `pathEnv`. Throws `PROVIDER_UNAVAILABLE`.
 */
export function resolveClaudePath(i: ResolveInput): string {
  const isFile = i.isFile ?? defaultIsFile(i.platform);
  const p = pathOf(i.platform);
  if (i.setting !== undefined) {
    if (!absolute(i.setting, i.platform)) {
      throw finError("PROVIDER_UNAVAILABLE", "agents.claudeCode.path must be an absolute path", { harness: HARNESS, reason: "relativePath" }, "set agents.claudeCode.path in settings.json to the CLI's absolute path");
    }
    checkNative(i.setting, i.platform);
    if (!isFile(i.setting)) throw notInstalled("pathMissing");
    return i.setting;
  }
  for (const raw of (i.pathEnv ?? "").split(p.delimiter)) {
    const dir = i.platform === "win32" ? raw.replace(/^"(.*)"$/, "$1") : raw;
    // A relative entry (`.`, `bin`, empty) is never searched.
    if (dir === "" || !absolute(dir, i.platform)) continue;
    const names = i.platform === "win32" ? WINDOWS_NAMES : ["claude"];
    const found = names.map((n) => p.join(dir, n)).find((c) => isFile(c));
    if (found === undefined) continue;
    checkNative(found, i.platform);
    return found;
  }
  throw notInstalled("notOnPath");
}

/** `x.y.z` against `floor`: negative when below. */
export function compareVersions(a: string, b: string): number {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let n = 0; n < 3; n++) {
    const d = (pa[n] ?? 0) - (pb[n] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/** The version in `--version`'s output; `PROVIDER_PROTOCOL` when it does not match. */
export function parseVersion(stdout: string): string {
  const m = VERSION_LINE.exec(stdout.trim());
  if (m === null) throw finError("PROVIDER_PROTOCOL", "claude --version did not print a Claude Code version", { harness: HARNESS, reason: "version" }, "reinstall Claude Code's native build");
  return m[1]!;
}

/**
 * `auth status --json`'s answer: `loggedIn`, and `authMethod` (allow-listed, else `other`) when signed in.
 * Exit 0 or 1 with a JSON object holding a boolean `loggedIn` is an answer; anything else is
 * `PROVIDER_PROTOCOL`. Nothing else of the answer is kept.
 */
export function parseAuthStatus(code: number | null, stdout: string): { signedIn: boolean; authMethod?: string } {
  let v: unknown;
  try {
    v = code === 0 || code === 1 ? JSON.parse(stdout) : undefined;
  } catch {
    v = undefined;
  }
  const o = v !== null && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined;
  if (o === undefined || typeof o.loggedIn !== "boolean") {
    throw finError("PROVIDER_PROTOCOL", "claude auth status did not answer whether it is signed in", { harness: HARNESS, reason: "authStatus", ...(code === null ? {} : { exitCode: code }) });
  }
  const method = o.loggedIn && typeof o.authMethod === "string" ? (AUTH_METHODS.has(o.authMethod) ? o.authMethod : "other") : undefined;
  return { signedIn: o.loggedIn, ...(method === undefined ? {} : { authMethod: method }) };
}

interface Ran {
  readonly code: number | null;
  readonly stdout: string;
}

/** Waits for `child` to exit with its output read (at most `OUTPUT_CAP`), or kills it after `ms`. */
async function finish(child: SandboxedChild, ms: number, step: string): Promise<Ran> {
  child.stdin.end();
  let stdout = "";
  child.stdout.on("data", (b: Buffer) => {
    if (stdout.length < OUTPUT_CAP) stdout += b.toString("utf8");
  });
  child.stderr.resume();
  const drained = new Promise<void>((resolve) => {
    child.stdout.once("close", resolve);
    child.stdout.once("end", resolve);
  });
  let timer: NodeJS.Timeout | undefined;
  const timedOut = new Promise<"timeout">((resolve) => (timer = setTimeout(() => resolve("timeout"), ms)));
  const r = await Promise.race([Promise.all([child.exited, drained]).then(([e]) => e), timedOut]);
  clearTimeout(timer);
  if (r === "timeout") {
    child.kill("SIGKILL");
    await child.exited.catch(() => undefined);
    throw finError("PROVIDER_PROTOCOL", `claude ${step} did not answer within ${Math.round(ms / 1000)} s`, { harness: HARNESS, reason: "timeout", step });
  }
  return { code: r.code, stdout: stdout.slice(0, OUTPUT_CAP) };
}

/** Runs the CLI once in `box` (its folder readable), to its end. */
function run(box: SessionSandbox, real: string, args: readonly string[], ms: number, platform: NodeJS.Platform): Effect.Effect<Ran, FinError> {
  const env = { DISABLE_AUTOUPDATER: "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1" };
  return box.spawn([real, ...args], { cwd: box.workspace, stdio: "pipe", env, extraRead: [pathOf(platform).dirname(real)] }).pipe(
    Effect.flatMap((child) => Effect.tryPromise({ try: () => finish(child, ms, args.join(" ")), catch: (e) => e as FinError })),
  );
}

const sync = <A>(f: () => A): Effect.Effect<A, FinError> => Effect.try({ try: f, catch: (e) => e as FinError });

interface AuthAnswer {
  readonly signedIn: boolean | null;
  readonly authMethod?: string;
  readonly accountHash?: string;
  readonly problem?: FinError;
}

/** `auth status`'s answer with the account hashed at once (`account.ts`): the label goes no further. */
function authAnswer(serverData: string, r: Ran): AuthAnswer {
  const a = parseAuthStatus(r.code, r.stdout);
  if (!a.signedIn) return a;
  const label = labelOfOutput(r.stdout);
  const { hash } = accountCheck({ ...a, ...(label === undefined ? {} : { label }) }, accountKey(serverData), null);
  return { ...a, accountHash: hash };
}

// Detection runs on every `agent.providers` call and every claudeCode plan, so its answers are
// kept: the version while the binary is the same file (realpath, mtime, size); `auth status` for
// `AUTH_TTL_MS`; concurrent callers share one run.
let generation = 0;
const versions = new Map<string, string>();
const auths = new Map<string, { at: number; answer: AuthAnswer }>();
const flights = new Map<string, Promise<Exit.Exit<AuthAnswer & { version: string }, FinError>>>();

/** Forgets what detection has kept (a settings change; a sign-in or sign-out finishing in this process). */
export function invalidateClaudeDetection(): void {
  generation++;
  versions.clear();
  auths.clear();
  flights.clear();
}

/** Runs the CLI (version unless known, `auth status` unless fresh) and keeps the answers. */
function probe(deps: ClaudeDetectDeps, real: string, key: string | undefined, platform: NodeJS.Platform): Effect.Effect<AuthAnswer & { version: string }, FinError> {
  const ms = deps.timeoutMs ?? DETECT_TIMEOUT_MS;
  const now = deps.now ?? Date.now;
  const authKey = key === undefined ? undefined : `${deps.sandbox.serverData}\0${key}`;
  return Effect.scoped(
    Effect.gen(function* () {
      const gen = generation;
      let version = key === undefined ? undefined : versions.get(key);
      const hit = authKey === undefined ? undefined : auths.get(authKey);
      let auth = hit !== undefined && now() - hit.at < AUTH_TTL_MS ? hit.answer : undefined;
      if (version === undefined || auth === undefined) {
        const box = yield* openHostSandbox("detect", HARNESS, deps.sandbox);
        if (version === undefined) {
          const v = yield* run(box, real, ["--version"], ms, platform);
          if (v.code !== 0) {
            return yield* Effect.fail(finError("PROVIDER_PROTOCOL", "claude --version failed", { harness: HARNESS, reason: "version", exitCode: v.code }, "reinstall Claude Code's native build"));
          }
          version = yield* sync(() => parseVersion(v.stdout));
          if (key !== undefined && gen === generation) versions.set(key, version);
        }
        if (compareVersions(version, CLAUDE_FLOOR) >= 0 && auth === undefined) {
          const a = yield* Effect.result(run(box, real, ["auth", "status", "--json"], ms, platform).pipe(Effect.flatMap((r) => sync(() => authAnswer(deps.sandbox.serverData, r)))));
          auth = a._tag === "Failure" ? { signedIn: null, problem: a.failure } : a.success;
          if (authKey !== undefined && gen === generation) auths.set(authKey, { at: now(), answer: auth });
        }
      }
      if (compareVersions(version, CLAUDE_FLOOR) < 0) {
        return yield* Effect.fail(
          finError("PROVIDER_VERSION", `Claude Code ${version} is older than ${CLAUDE_FLOOR}, the oldest this Finisher supports`, { harness: HARNESS, version, floor: CLAUDE_FLOOR }, `update Claude Code to ${CLAUDE_FLOOR} or later`),
        );
      }
      return { version, ...auth! };
    }),
  );
}

/**
 * Finds the CLI, reads its version (`PROVIDER_VERSION {version, floor}` below the floor) and
 * whether it is signed in. Runs the CLI in a host sandbox (twice at most; answers are reused, see
 * above); reads nothing under `providers/`.
 */
export function detectClaude(deps: ClaudeDetectDeps): Effect.Effect<ClaudeDetection, FinError> {
  const platform = deps.platform ?? deps.sandbox.platform ?? process.platform;
  return Effect.gen(function* () {
    const settings = (deps.settings ?? (() => claudeCodeSettings(deps.sandbox.serverData)))();
    const path = yield* sync(() =>
      resolveClaudePath({ setting: settings.path, pathEnv: deps.pathEnv ?? process.env.PATH ?? process.env.Path, platform, ...(deps.isFile === undefined ? {} : { isFile: deps.isFile }) }),
    );
    const real = yield* sync(() => {
      try {
        return (deps.realpath ?? realpathSync)(path);
      } catch {
        throw notInstalled("pathMissing");
      }
    });
    let key: string | undefined;
    try {
      const st = statSync(real);
      key = `${real}\0${st.mtimeMs}\0${st.size}`;
    } catch {
      key = undefined; // Unreadable here (a fake path): run it every time.
    }
    const flightKey = key === undefined ? undefined : `${deps.sandbox.serverData}\0${key}`;
    let flight = flightKey === undefined ? undefined : flights.get(flightKey);
    if (flight === undefined) {
      const started = Effect.runPromiseExit(probe(deps, real, key, platform));
      flight = started;
      if (flightKey !== undefined) {
        flights.set(flightKey, started);
        const mine = started;
        void started.finally(() => {
          if (flights.get(flightKey) === mine) flights.delete(flightKey);
        });
      }
    }
    const r = yield* Effect.promise(() => flight);
    const d = yield* r;
    const { version, ...auth } = d;
    return { path, version, floor: CLAUDE_FLOOR, ...auth } satisfies ClaudeDetection;
  });
}

const problemOf = (e: FinError) => ({ code: e.code, message: e.message, ...(e.fix === undefined ? {} : { fix: e.fix }) });

/**
 * Claude Code's line in `agent.providers`: what detection found, the floor, whether it is
 * signed in and how (one word), the account's keyed hash, and whether sessions may start on it.
 * Never a label or token (`account` stays unset). Each answer is recorded (`account.ts`): an
 * account other than the recorded one, unconfirmed, is the problem `PROVIDER_ACCOUNT_CHANGED`,
 * which stops new sessions. Off (`agents.claudeCode.enabled`, the default): the CLI is looked
 * for but not run.
 */
export function claudeProviderInfo(deps: ClaudeDetectDeps): Effect.Effect<ProviderInfo> {
  const platform = deps.platform ?? deps.sandbox.platform ?? process.platform;
  return Effect.suspend(() => {
    const settings = (deps.settings ?? (() => claudeCodeSettings(deps.sandbox.serverData)))();
    const base = { harness: HARNESS, capabilities: CLAUDE_CAPABILITIES, floor: CLAUDE_FLOOR, enabled: settings.enabled } as const;
    if (!settings.enabled) {
      let found = false;
      try {
        resolveClaudePath({ setting: settings.path, pathEnv: deps.pathEnv ?? process.env.PATH ?? process.env.Path, platform, ...(deps.isFile === undefined ? {} : { isFile: deps.isFile }) });
        found = true;
      } catch {
        found = false;
      }
      return Effect.succeed<ProviderInfo>({
        ...base,
        detected: found,
        problem: { code: "PROVIDER_UNAVAILABLE", message: "Claude Code is off on this server", fix: "set agents.claudeCode.enabled to true in settings.json" },
      });
    }
    return detectClaude({ ...deps, settings: () => settings }).pipe(
      Effect.map((d): ProviderInfo => {
        const seen = d.accountHash === undefined ? undefined : new ClaudeAccounts(deps.sandbox.serverData).record(d.accountHash, d.authMethod);
        const problem = d.problem ?? (seen?.changed === true ? accountChanged(seen.hash) : undefined);
        return {
          ...base,
          detected: true,
          version: d.version,
          supported: true,
          signedIn: d.signedIn,
          ...(d.authMethod === undefined ? {} : { authMethod: d.authMethod }),
          ...(d.accountHash === undefined ? {} : { accountHash: d.accountHash }),
          ...(problem === undefined ? {} : { problem: problemOf(problem) }),
        };
      }),
      Effect.catch((e: FinError) => {
        const version = (e.details as { version?: unknown } | null)?.version;
        return Effect.succeed<ProviderInfo>({
          ...base,
          // Found but unusable (too old, garbled, a shim) is detected; not found or no sandbox is not.
          detected: e.code === "PROVIDER_VERSION" || e.code === "PROVIDER_PROTOCOL" || (e.details as { reason?: unknown } | null)?.reason === "npmShim",
          ...(typeof version === "string" ? { version, supported: false } : {}),
          problem: problemOf(e),
        });
      }),
      // A sandbox that cannot open, or any other failure, is a problem on the line, never a failed RPC.
      Effect.catchCause((c) => Effect.succeed<ProviderInfo>({ ...base, detected: false, problem: { code: "INTERNAL", message: `detection failed: ${String(c).slice(0, 200)}` } })),
    );
  });
}

// The sign-in commands -------------------------------------------------------------------------

const shQuote = (s: string) => `'${s.replaceAll("'", `'\\''`)}'`;
/** PowerShell treats `'` and U+2018 to U+201B alike as a single quote: each is doubled. */
const psQuote = (s: string) => `'${s.replace(/['\u2018-\u201b]/g, (c) => c + c)}'`;
/** Variables that make the CLI use a credential, endpoint or cloud route other than its own store. */
const CRED_VARS = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_OAUTH_REFRESH_TOKEN", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"];

/**
 * The commands a person runs in a terminal on the server to sign the CLI in (`login`) or out
 * (`logout`) with its own flow, into `<server-data>/providers/claudeCode/home` (Decision 23).
 * Unix: a subshell with `umask 077`, the store folder made 0700, and the CLI run without
 * any of `CRED_VARS` (API key, tokens, base URL, cloud routes); Windows: the PowerShell form (server data's
 * owner-only DACL is inherited). Lines starting with `#` are comments in both shells.
 */
export function signinCommands(verb: "login" | "logout", i: { path: string; serverData: string; platform: NodeJS.Platform; enabled?: boolean; source?: "setting" | "path" }): string[] {
  const p = pathOf(i.platform);
  // Printed text never carries a control character (a path could hold an escape sequence).
  const exe = safe(i.path);
  const home = safe(p.join(i.serverData, "providers", HARNESS, "home"));
  const store = safe(p.join(i.serverData, "providers", HARNESS, "home", "store"));
  const off = i.enabled === false ? ["# Claude Code is off on this server: sessions start on it once agents.claudeCode.enabled is true in settings.json."] : [];
  const where =
    i.source === undefined
      ? []
      : [i.source === "setting" ? "# The CLI is the one agents.claudeCode.path names." : "# The CLI was found on this console's PATH, which can differ from the server's; agents.claudeCode.path in settings.json overrides the search."];
  const alt = `# Or run /${verb} inside an interactive claude started with the same variables.`;
  if (i.platform === "win32") {
    return [
      `# Run in a new PowerShell window on this server, as the account that runs finisherd (it changes that window's environment).`,
      ...off,
      ...where,
      ...(verb === "login" ? [`New-Item -ItemType Directory -Force -Path ${psQuote(store)} | Out-Null`] : []),
      `Remove-Item ${CRED_VARS.map((v) => `Env:${v}`).join(", ")} -ErrorAction SilentlyContinue`,
      `$env:CLAUDE_CONFIG_DIR = ${psQuote(home)}`,
      `$env:CLAUDE_SECURESTORAGE_CONFIG_DIR = ${psQuote(store)}`,
      "$env:DISABLE_AUTOUPDATER = '1'",
      `& ${psQuote(exe)} auth ${verb}`,
      alt,
    ];
  }
  return [
    "# Run in sh, bash or zsh on this server, as the account that runs finisherd.",
    ...off,
    ...where,
    "(",
    "  umask 077",
    ...(verb === "login" ? [`  mkdir -p ${shQuote(store)}`, `  chmod 700 ${shQuote(store)}`] : []),
    `  env ${CRED_VARS.map((v) => `-u ${v}`).join(" ")} CLAUDE_CONFIG_DIR=${shQuote(home)} CLAUDE_SECURESTORAGE_CONFIG_DIR=${shQuote(store)} DISABLE_AUTOUPDATER=1 ${shQuote(exe)} auth ${verb}`,
    ")",
    alt,
  ];
}

/**
 * `finisherd provider signin|signout claudeCode`: the CLI as detection would find it, and the
 * commands for it. Runs nothing and opens nothing under `providers/`; throws `PROVIDER_UNAVAILABLE`.
 */
export function signinLines(
  serverData: string,
  verb: "login" | "logout",
  o: { platform?: NodeJS.Platform; pathEnv?: string; isFile?: (p: string) => boolean; settings?: ClaudeCodeSettings } = {},
): string[] {
  const platform = o.platform ?? process.platform;
  // The server's own spelling of server data (links resolved), as its sandboxes name the store.
  let data = serverData;
  try {
    data = realpathSync(serverData);
  } catch {
    // Not made yet: as given.
  }
  const settings = o.settings ?? claudeCodeSettings(serverData);
  const path = resolveClaudePath({ setting: settings.path, pathEnv: o.pathEnv ?? process.env.PATH ?? process.env.Path, platform, ...(o.isFile === undefined ? {} : { isFile: o.isFile }) });
  return signinCommands(verb, { path, serverData: data, platform, enabled: settings.enabled, source: settings.path === undefined ? "path" : "setting" });
}
