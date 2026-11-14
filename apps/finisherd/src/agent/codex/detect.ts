// Codex's detection, its line in `agent.providers` and the sign-in commands (plan 7E Task 6,
// Decisions 1, 8, 11, 20, 21; Owner Q7(a)).
//
// - **The binary** is the bundled one (`pin.ts` `codexBinary()`): there is no `agents.codex.path`.
//   Not bundled is `PROVIDER_UNAVAILABLE {reason: notBundled}`.
// - **The version**: `codex --version` must print `codex-cli <x.y.z>` within 10 s (anything else is
//   `PROVIDER_PROTOCOL`) and equal the pin (`PROVIDER_VERSION {version, pinned}`).
// - **Signed in**: a short app-server (`initialize`, `initialized`, `account/read`, then stdin
//   closed), only through `rpc.ts`, so only `ALLOWED_METHODS`; `account/read` never carries
//   `refreshToken`. `chatgpt` is signed in (the plan type is shown; the e-mail only ever goes into
//   the keyed hash of `cli/account.ts`), `apiKey` is signed in and recorded by its method alone
//   with an `accountUnidentified` warning (Owner Q7(a)), no account is signed out, and `amazonBedrock`
//   and `gcp` are refused `PROVIDER_AUTH {reason: unsupportedAccount}`.
//
// Both run in a host sandbox (`openHostSandbox("detect", "codex")`), whose `CODEX_HOME` is the
// directory's throwaway `provider/`, holding only a link named `auth.json` to the sign-in
// store (read-only to the sandbox for detection). **Finisher opens nothing under `providers/`**:
// only the CLI, in the sandbox, reads the store; the sign-in commands are printed for a person.
import { realpathSync, statSync } from "node:fs";
import { posix, win32 } from "node:path";
import type { FinError, ProviderInfo } from "@finisher/contracts";
import { Cause, Effect, Exit } from "effect";
import { safe } from "../../console/safe.ts";
import { finError } from "../../errors.ts";
import type { Capabilities } from "../adapter.ts";
import { accountChanged, accountKey, accountProblem, accountsOf, type AuthStatus } from "../cli/account.ts";
import { openHostSandbox, type SandboxedChild, type SessionSandbox, type SessionSandboxDeps } from "../sandbox.ts";
import { type CodexSettings, codexSettings } from "../settings.ts";
import { type PinDeps, codexBinary, defaultPinDeps } from "./pin.ts";
import { codexRpc } from "./rpc.ts";
import type { ResultOf } from "./rpc.ts";

/** What `codex --version` prints (Task 0: `codex-cli 0.167.0`). */
export const VERSION_LINE = /^codex-cli (\d+\.\d+\.\d+)$/;
/** How long `--version` and the account read may each take. */
export const DETECT_TIMEOUT_MS = 10_000;
/** How long an `account/read` answer is reused (a sign-in or out is run by a person in another process). */
export const AUTH_TTL_MS = 30_000;
const OUTPUT_CAP = 64 * 1024;
const HARNESS = "codex";

/** What sessions on Codex can do (spec §4.1; the session tasks may refine it). */
export const CODEX_CAPABILITIES: Capabilities = {
  steer: true,
  rollback: true,
  resume: true,
  reasoning: true,
  images: false,
  parallelTools: false,
  costReported: false,
  exactCapture: true,
  nativeSubagents: false,
};

export interface CodexDetection {
  /** The bundled binary. */
  readonly path: string;
  readonly version: string;
  readonly pinned: string;
  /** `null` when `account/read` gave no usable answer (`problem` says why). */
  readonly signedIn: boolean | null;
  /** `chatgpt`, `apiKey`, `amazonBedrock` or `gcp`. */
  readonly authMethod?: string;
  readonly planType?: string;
  /** The account's keyed hash (`cli/account.ts`): never the e-mail. */
  readonly accountHash?: string;
  readonly warnings?: ProviderInfo["warnings"];
  readonly problem?: FinError;
}

export interface CodexDetectDeps {
  readonly sandbox: SessionSandboxDeps;
  /** `agents.codex` (default: read from `settings.json` at each call). */
  readonly settings?: () => CodexSettings;
  /** Where the binary is looked for (default: the server's; tests). */
  readonly pin?: PinDeps;
  readonly timeoutMs?: number;
  /** The clock for the account cache (default `Date.now`; tests). */
  readonly now?: () => number;
}

const pathOf = (platform: NodeJS.Platform) => (platform === "win32" ? win32 : posix);
const sync = <A>(f: () => A): Effect.Effect<A, FinError> => Effect.try({ try: f, catch: (e) => e as FinError });
const problemOf = (e: FinError) => ({ code: e.code, message: e.message, ...(e.fix === undefined ? {} : { fix: e.fix }) });
const reinstall = "reinstall Finisher";

/** The version in `--version`'s output; `PROVIDER_PROTOCOL` when it does not match. */
export function parseCodexVersion(stdout: string): string {
  const m = VERSION_LINE.exec(stdout.trim());
  if (m === null) throw finError("PROVIDER_PROTOCOL", "codex --version did not print a Codex version", { harness: HARNESS, reason: "version" }, reinstall);
  return m[1]!;
}

const timedOut = (step: string, ms: number) => finError("PROVIDER_PROTOCOL", `codex ${step} did not answer within ${Math.round(ms / 1000)} s`, { harness: HARNESS, reason: "timeout", step }, reinstall);

/** `work` against a clock; on the timeout the child is killed. */
async function within<A>(child: SandboxedChild, ms: number, step: string, work: Promise<A>): Promise<A> {
  let timer: NodeJS.Timeout | undefined;
  const late = new Promise<"timeout">((resolve) => (timer = setTimeout(() => resolve("timeout"), ms)));
  const r = await Promise.race([work, late]);
  clearTimeout(timer);
  if (r === "timeout") {
    child.kill("SIGKILL");
    await child.exited.catch(() => undefined);
    throw timedOut(step, ms);
  }
  return r;
}

/** `codex --version`'s stdout (at most `OUTPUT_CAP`); a non-zero exit is `PROVIDER_PROTOCOL`. */
async function runVersion(child: SandboxedChild, ms: number): Promise<string> {
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
  const exit = await within(child, ms, "--version", Promise.all([child.exited, drained]).then(([e]) => e));
  if (exit.code !== 0) throw finError("PROVIDER_PROTOCOL", "codex --version failed", { harness: HARNESS, reason: "version", exitCode: exit.code }, reinstall);
  return stdout.slice(0, OUTPUT_CAP);
}

type Account = NonNullable<ResultOf<"account/read">["account"]>;

/** `initialize`, `initialized`, `account/read` (no `refreshToken`), then stdin closed and the exit awaited. */
async function readAccount(child: SandboxedChild, ms: number): Promise<ResultOf<"account/read">> {
  const rpc = codexRpc({ stdin: child.stdin, stdout: child.stdout });
  child.stderr.resume();
  const exchange = Effect.runPromiseExit(
    Effect.gen(function* () {
      yield* rpc.request("initialize", { clientInfo: { name: "finisher", version: "detect" } });
      yield* rpc.notify("initialized");
      const r = yield* rpc.request("account/read", {});
      rpc.end();
      return r;
    }),
  );
  const exit = await within(child, ms, "account/read", exchange);
  if (Exit.isFailure(exit)) {
    const e = Cause.findErrorOption(exit.cause);
    throw e._tag === "Some" ? e.value : finError("INTERNAL", "the Codex account read failed", { harness: HARNESS });
  }
  // The server exits on its stdin's end; a late one is killed with the sandbox.
  await Promise.race([child.exited.catch(() => undefined), new Promise((r) => setTimeout(r, 2000).unref())]);
  return exit.value;
}

const unsupported = (type: string) =>
  finError(
    "PROVIDER_AUTH",
    "Codex is signed in with an account type Finisher does not support (only a ChatGPT plan or an OpenAI API key)",
    { harness: HARNESS, reason: "unsupportedAccount", authMethod: type },
    "sign it in again with the commands `finisherd provider signin codex` prints",
  );

const noLabel = () =>
  finError(
    "PROVIDER_ACCOUNT_CHANGED",
    "Codex is signed in with a ChatGPT account that names no e-mail, so Finisher cannot tell it apart from another",
    { harness: HARNESS, reason: "noAccountLabel" },
    "sign it in again with the commands `finisherd provider signin codex` prints",
  );

/** The unidentified-account warning of an API-key sign-in (Owner Q7(a)). */
const UNIDENTIFIED = {
  code: "accountUnidentified",
  message: "Codex is signed in with an API key, which names no account: Finisher records the method only, so a swap to another key is not seen",
  details: { harness: HARNESS, authMethod: "apiKey" },
  fix: "sign in with a ChatGPT account to have account changes noticed",
} as const;

/** What the account read means; the e-mail goes into the keyed hash and no further. */
function answerOf(serverData: string, a: Account | null | undefined): Omit<CodexDetection, "path" | "version" | "pinned"> {
  if (a === null || a === undefined) return { signedIn: false };
  const type = String((a as { type?: unknown }).type);
  if (type === "chatgpt") {
    const c = a as Extract<Account, { type: "chatgpt" }>;
    const planType = typeof c.planType === "string" && /^[a-z0-9_]{1,40}$/.test(c.planType) ? c.planType : "other";
    const base = { signedIn: true, authMethod: "chatgpt", planType } as const;
    if (typeof c.email !== "string" || c.email === "") return { ...base, problem: noLabel() };
    try {
      const status: AuthStatus = { signedIn: true, authMethod: "chatgpt", label: c.email };
      return { ...base, accountHash: accountsOf(serverData, HARNESS).hashOf(status, accountKey(serverData)) };
    } catch (e) {
      return { ...base, problem: e as FinError };
    }
  }
  if (type === "apiKey") {
    const base = { signedIn: true, authMethod: "apiKey", warnings: [UNIDENTIFIED] } as const;
    try {
      return { ...base, accountHash: accountsOf(serverData, HARNESS).hashOf({ signedIn: true, authMethod: "apiKey" }, accountKey(serverData)) };
    } catch (e) {
      return { ...base, problem: e as FinError };
    }
  }
  return { signedIn: true, authMethod: /^[A-Za-z]{1,24}$/.test(type) ? type : "other", problem: unsupported(type) };
}

// Detection runs on every `agent.providers` call, so its answers are kept: the version while the
// binary is the same file (realpath, mtime, size), the account for `AUTH_TTL_MS`; concurrent
// callers share one run.
type Answer = Omit<CodexDetection, "path" | "pinned">;
let generation = 0;
const versions = new Map<string, string>();
const accounts = new Map<string, { at: number; answer: Omit<Answer, "version"> }>();
const flights = new Map<string, Promise<Exit.Exit<Answer, FinError>>>();

/** Forgets what detection has kept (a settings change; a sign-in or sign-out finishing in this process; tests). */
export function invalidateCodexDetection(): void {
  generation++;
  versions.clear();
  accounts.clear();
  flights.clear();
}

function probe(deps: CodexDetectDeps, real: string, key: string | undefined, pinned: string): Effect.Effect<Answer, FinError> {
  const ms = deps.timeoutMs ?? DETECT_TIMEOUT_MS;
  const now = deps.now ?? Date.now;
  const accountKeyOf = key === undefined ? undefined : `${deps.sandbox.serverData}\0${key}`;
  const spawn = (box: SessionSandbox, args: readonly string[]) => box.spawn([real, ...args], { cwd: box.workspace, stdio: "pipe", extraRead: [real] });
  return Effect.scoped(
    Effect.gen(function* () {
      const gen = generation;
      let version = key === undefined ? undefined : versions.get(key);
      const hit = accountKeyOf === undefined ? undefined : accounts.get(accountKeyOf);
      let answer = hit !== undefined && now() - hit.at < AUTH_TTL_MS ? hit.answer : undefined;
      if (version === undefined || answer === undefined) {
        const box = yield* openHostSandbox("detect", HARNESS, deps.sandbox);
        if (version === undefined) {
          const child = yield* spawn(box, ["--version"]);
          const out = yield* Effect.tryPromise({ try: () => runVersion(child, ms), catch: (e) => e as FinError });
          version = yield* sync(() => parseCodexVersion(out));
          if (key !== undefined && gen === generation) versions.set(key, version);
        }
        if (version === pinned && answer === undefined) {
          const argv = ["app-server", "--listen", "stdio://", "--strict-config", "-c", 'cli_auth_credentials_store="file"'];
          const read = yield* Effect.result(
            spawn(box, argv).pipe(
              Effect.flatMap((child) => Effect.tryPromise({ try: () => readAccount(child, ms), catch: (e) => e as FinError })),
              Effect.flatMap((r) => sync(() => answerOf(deps.sandbox.serverData, r.account))),
            ),
          );
          answer = read._tag === "Failure" ? { signedIn: null, problem: read.failure } : read.success;
          if (accountKeyOf !== undefined && gen === generation) accounts.set(accountKeyOf, { at: now(), answer });
        }
      }
      if (version !== pinned) {
        return yield* Effect.fail(finError("PROVIDER_VERSION", `Codex ${version} is not the pinned ${pinned}`, { harness: HARNESS, version, pinned }, reinstall));
      }
      return { version, ...answer! };
    }),
  );
}

/**
 * Finds the bundled Codex, reads its version (`PROVIDER_VERSION {version, pinned}` unless it is
 * the pin) and whether it is signed in. Runs it in a host sandbox (twice at most; answers are
 * reused); reads nothing under `providers/`.
 */
export function detectCodex(deps: CodexDetectDeps): Effect.Effect<CodexDetection, FinError> {
  return Effect.gen(function* () {
    const pin = deps.pin ?? defaultPinDeps();
    const path = yield* sync(() => codexBinary(pin));
    const real = yield* sync(() => {
      try {
        return realpathSync(path);
      } catch {
        throw finError("PROVIDER_UNAVAILABLE", "Codex is not bundled with this build: its binary is gone", { harness: HARNESS, provider: HARNESS, reason: "notBundled" });
      }
    });
    let key: string | undefined;
    try {
      const st = statSync(real);
      key = `${real}\0${st.mtimeMs}\0${st.size}`;
    } catch {
      key = undefined; // Unreadable here: run it every time.
    }
    const flightKey = key === undefined ? undefined : `${deps.sandbox.serverData}\0${key}`;
    let flight = flightKey === undefined ? undefined : flights.get(flightKey);
    if (flight === undefined) {
      const started = Effect.runPromiseExit(probe(deps, real, key, pin.version));
      flight = started;
      if (flightKey !== undefined) {
        flights.set(flightKey, started);
        void started.finally(() => {
          if (flights.get(flightKey) === started) flights.delete(flightKey);
        });
      }
    }
    const r = yield* Effect.promise(() => flight);
    const d = yield* r;
    return { path, pinned: pin.version, ...d } satisfies CodexDetection;
  });
}

/**
 * Codex's line in `agent.providers`: what detection found, the pin, whether it is signed in and
 * how (one word), the plan type, the account's keyed hash and whether sessions may start on it.
 * Never a label or token. Off (`agents.codex.enabled`, the default): the binary is looked for but
 * not run. Each answer is recorded (`cli/account.ts`): an account other than the recorded one,
 * unconfirmed, is the problem `PROVIDER_ACCOUNT_CHANGED`.
 */
export function codexProviderInfo(deps: CodexDetectDeps): Effect.Effect<ProviderInfo> {
  return Effect.suspend(() => {
    const settings = (deps.settings ?? (() => codexSettings(deps.sandbox.serverData)))();
    const base = { harness: HARNESS, capabilities: CODEX_CAPABILITIES, enabled: settings.enabled } as const;
    if (!settings.enabled) {
      let found = false;
      try {
        codexBinary(deps.pin ?? defaultPinDeps());
        found = true;
      } catch {
        found = false;
      }
      return Effect.succeed<ProviderInfo>({
        ...base,
        detected: found,
        problem: { code: "PROVIDER_UNAVAILABLE", message: "Codex is off on this server", fix: "set agents.codex.enabled to true in settings.json" },
      });
    }
    return detectCodex({ ...deps, settings: () => settings }).pipe(
      Effect.map((d): ProviderInfo => {
        let problem = d.problem;
        let waiting: string | undefined;
        try {
          const record = accountsOf(deps.sandbox.serverData, HARNESS);
          const seen = d.accountHash === undefined || d.problem?.code === "PROVIDER_AUTH" ? undefined : record.record(d.accountHash, d.authMethod);
          waiting = seen?.changed === true ? seen.hash : record.pending();
          // An unsupported account is the more specific refusal: it stays.
          if (waiting !== undefined && d.problem?.code !== "PROVIDER_AUTH") problem = accountChanged(waiting, HARNESS);
        } catch (e) {
          problem = e as FinError;
        }
        const accountHash = d.accountHash ?? waiting;
        return {
          ...base,
          detected: true,
          version: d.version,
          supported: true,
          signedIn: d.signedIn,
          ...(d.authMethod === undefined ? {} : { authMethod: d.authMethod }),
          ...(d.planType === undefined ? {} : { planType: d.planType }),
          ...(accountHash === undefined ? {} : { accountHash }),
          ...(d.warnings === undefined ? {} : { warnings: d.warnings }),
          ...(problem === undefined ? {} : { problem: problemOf(problem) }),
        };
      }),
      Effect.catch((e: FinError) => {
        const version = (e.details as { version?: unknown } | null)?.version;
        const detected = e.code === "PROVIDER_VERSION" || e.code === "PROVIDER_PROTOCOL";
        const account = detected && typeof version !== "string" ? accountProblem(deps.sandbox.serverData, HARNESS) : undefined;
        return Effect.succeed<ProviderInfo>({
          ...base,
          detected,
          ...(typeof version === "string" ? { version, supported: false } : {}),
          problem: problemOf(account ?? e),
        });
      }),
      Effect.catchCause((c) => Effect.succeed<ProviderInfo>({ ...base, detected: false, problem: { code: "INTERNAL", message: `detection failed: ${String(c).slice(0, 200)}` } })),
    );
  });
}

// The sign-in commands -------------------------------------------------------------------------

const shQuote = (s: string) => `'${s.replaceAll("'", `'\\''`)}'`;
/** PowerShell treats `'` and U+2018 to U+201B alike as a single quote: each is doubled. */
const psQuote = (s: string) => `'${s.replace(/['‘-‛]/g, (c) => c + c)}'`;
/** Variables that make Codex use a credential or endpoint other than its own store. */
const CRED_VARS = ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN", "OPENAI_BASE_URL", "OPENAI_ORG_ID", "OPENAI_PROJECT_ID"];
const STORE_CONFIG = 'cli_auth_credentials_store="file"';

/**
 * The commands a person runs in a terminal on the server to sign the bundled Codex in (`login`,
 * and its `--device-auth` variant for a headless server) or out (`logout`), into
 * `<server-data>/providers/codex/home` (Decision 21). Unix: a subshell with `umask 077`, the home
 * made 0700, the CLI run without any of `CRED_VARS`; Windows: the PowerShell form (server data's
 * owner-only DACL is inherited). Lines starting with `#` are comments in both shells.
 */
export function codexSigninCommands(verb: "login" | "logout", i: { path: string; serverData: string; platform: NodeJS.Platform; enabled?: boolean }): string[] {
  const exe = safe(i.path);
  const home = safe(pathOf(i.platform).join(i.serverData, "providers", HARNESS, "home"));
  const off = i.enabled === false ? ["# Codex is off on this server: sessions start on it once agents.codex.enabled is true in settings.json."] : [];
  const variants = verb === "login" ? [["# Sign in with a browser (a ChatGPT account, or an API key it offers):", ""], ["# Or, on a headless server, with a device code:", " --device-auth"]] : [["", ""]];
  if (i.platform === "win32") {
    return [
      "# Run in a new PowerShell window on this server, as the account that runs finisherd (it changes that window's environment).",
      ...off,
      ...(verb === "login" ? [`New-Item -ItemType Directory -Force -Path ${psQuote(home)} | Out-Null`] : []),
      `Remove-Item ${CRED_VARS.map((v) => `Env:${v}`).join(", ")} -ErrorAction SilentlyContinue`,
      `$env:CODEX_HOME = ${psQuote(home)}`,
      ...variants.flatMap(([note, flag]) => [...(note === "" ? [] : [note!]), `& ${psQuote(exe)} -c ${psQuote(STORE_CONFIG)} ${verb}${flag}`]),
    ];
  }
  const run = (flag: string) => `  env ${CRED_VARS.map((v) => `-u ${v}`).join(" ")} CODEX_HOME=${shQuote(home)} ${shQuote(exe)} -c ${shQuote(STORE_CONFIG)} ${verb}${flag}`;
  const block = (note: string, flag: string) => [
    ...(note === "" ? [] : [note]),
    "(",
    "  umask 077",
    ...(verb === "login" ? [`  mkdir -p ${shQuote(home)}`, `  chmod 700 ${shQuote(home)}`] : []),
    run(flag),
    ")",
  ];
  return [
    "# Run in sh, bash or zsh on this server, as the account that runs finisherd.",
    ...off,
    ...variants.flatMap(([note, flag]) => block(note!, flag!)),
  ];
}

/**
 * `finisherd provider signin|signout codex`: the bundled binary and the commands for it. Runs
 * nothing and opens nothing under `providers/`; throws `PROVIDER_UNAVAILABLE` when not bundled.
 */
export function codexSigninLines(serverData: string, verb: "login" | "logout", o: { platform?: NodeJS.Platform; pin?: PinDeps; settings?: CodexSettings } = {}): string[] {
  const platform = o.platform ?? process.platform;
  let data = serverData;
  try {
    data = realpathSync(serverData);
  } catch {
    // Not made yet: as given.
  }
  const settings = o.settings ?? codexSettings(serverData);
  const path = codexBinary(o.pin ?? defaultPinDeps());
  return codexSigninCommands(verb, { path, serverData: data, platform, enabled: settings.enabled });
}
