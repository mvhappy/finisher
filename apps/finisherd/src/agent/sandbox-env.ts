// The session environment of spec §8.5 (plan 7C Task 11): `childEnv()` plus exactly the
// session's variables. `sessionEnv` builds it; `assertSessionEnv` is the contract a CLI
// adapter (7D, 7E) checks its launch environment against, so a harness that sets its own
// environment cannot drop `FIN_PROJECT_DIR` or pass a secret by accident.
import { existsSync } from "node:fs";
import { posix, win32 } from "node:path";
import type { Harness, Session } from "@finisher/contracts";
import { childEnv, NEVER_PASSED, secretName } from "../env.ts";
import { finError } from "../errors.ts";
import { type LinkedStore, PROVIDER_DIR_ENV, SIGN_IN_STORES, type SessionDirs } from "./session-dirs.ts";

/**
 * The session's git settings at command-line precedence (`GIT_CONFIG_COUNT`), because a
 * repository's own `.git/config` overrides the global file: no fsmonitor (a daemon, and a
 * program a repository could name) and no credential helper (credential-cache is a daemon on
 * a Unix socket). Signing stays off through the global file.
 */
export const GIT_OVERRIDES: ReadonlyArray<readonly [string, string]> = [
  ["core.fsmonitor", "false"],
  ["credential.helper", ""],
];

/**
 * The names a host sandbox (detection, the probe: no session) never has: the session's
 * connection to the server and its project, and the sign-in store variable (spec §8.6), which
 * only detection's host sandbox has (`AssertOptions.store`).
 */
export const SESSION_ONLY: ReadonlyArray<string> = ["FIN_SERVER", "FIN_SESSION", "FIN_CREDENTIAL", "FIN_PROJECT_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"];

/**
 * Claude Code's own switches a launch adds (plan 7D Decision 5): non-secret, documented, each
 * fixed by the adapter. Not `CLAUDE_CODE_GIT_BASH_PATH` (Decision 21), not
 * `CLAUDE_CODE_ENTRYPOINT` (Finisher is not the SDK), never a token.
 */
export const CLAUDE_CODE_ENV: ReadonlyArray<string> = [
  "CLAUDE_CODE_TMPDIR",
  "DISABLE_AUTOUPDATER",
  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
  "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
  "CLAUDE_CODE_DISABLE_CLAUDE_MDS",
  "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
  "BASH_DEFAULT_TIMEOUT_MS",
];

/**
 * The names a harness's adapter may add to one process (`SpawnOptions.env`, the `extra` of
 * `assertSessionEnv`): Claude Code's switches, and for every harness `FIN_CALL` (a per-call
 * process's call id, spec §12.3). Nothing else.
 */
export function harnessExtra(harness: Harness): ReadonlyArray<string> {
  return harness === "claudeCode" ? [...CLAUDE_CODE_ENV, "FIN_CALL"] : ["FIN_CALL"];
}

/** A call id as `fin` takes it from `FIN_CALL` (`agent/reach.ts`, `crates/fin/src/agent_env.rs`). */
const CALL_ID = /^[0-9A-Za-z._:-]{1,128}$/;

/**
 * The value each name a harness adds must have (plan 7D Decision 5): the switches exactly `1`,
 * the timeout a whole number of milliseconds, the CLI's temporary directory the sandbox's own
 * `TMPDIR`, the call id one `fin` accepts. `env` is the whole launch environment.
 */
const EXTRA_VALUES: Readonly<Record<string, (v: string, env: Readonly<Record<string, string | undefined>>) => boolean>> = {
  CLAUDE_CODE_TMPDIR: (v, env) => v !== "" && v === env.TMPDIR,
  DISABLE_AUTOUPDATER: (v) => v === "1",
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: (v) => v === "1",
  CLAUDE_CODE_DISABLE_AUTO_MEMORY: (v) => v === "1",
  CLAUDE_CODE_DISABLE_CLAUDE_MDS: (v) => v === "1",
  CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: (v) => v === "1",
  BASH_DEFAULT_TIMEOUT_MS: (v) => /^\d+$/.test(v),
  FIN_CALL: (v) => CALL_ID.test(v),
};

/**
 * The shells a Claude Code session's `SHELL` may name (plan 7D Decision 5), in order of choice.
 * Verified on 2.1.321 (7D Task 0, s3B): without `SHELL` the CLI's Bash tool starts the passwd
 * login shell, which sources the user's rc files and hands their exports to every tool process.
 */
export const SESSION_SHELLS: ReadonlyArray<string> = ["/bin/bash", "/bin/sh"];

/** The first of `SESSION_SHELLS` this host has (`/bin/sh` at worst). */
export function sessionShell(exists: (p: string) => boolean = existsSync): string {
  return SESSION_SHELLS.find((p) => exists(p)) ?? SESSION_SHELLS.at(-1)!;
}

/**
 * The harnesses whose sessions get their own `HOME` (`tmp/home`, Windows `USERPROFILE` too) and,
 * off Windows, an explicit `SHELL` (plan 7D Decision 5, Task 0 signed in): with the server's
 * `HOME` Claude Code's Bash sourced the user's rc files and the CLI read `~/.claude/`.
 */
const OWN_HOME: ReadonlySet<Harness> = new Set(["claudeCode"]);

/** The variable naming `harness`'s sign-in store directory, when its store is one (Claude Code). */
export function storeEnvName(harness: Harness): string | undefined {
  const store = SIGN_IN_STORES[harness];
  return store?.kind === "dir" ? store.env : undefined;
}

/** `names` holds `k` in any case (Windows reads environment names case-insensitively). */
const hasFolded = (names: Iterable<string>, k: string) => {
  const u = k.toUpperCase();
  for (const n of names) if (n.toUpperCase() === u) return true;
  return false;
};

/** Every name `sessionEnv` adds on top of `childEnv`'s allow-list, by harness. */
export function sessionKeys(harness: Harness, platform: NodeJS.Platform = process.platform): Set<string> {
  const keys = new Set<string>([
    "FIN_SERVER",
    "FIN_SESSION",
    "FIN_CREDENTIAL",
    "FIN_PROJECT_DIR",
    "TMPDIR",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_COUNT",
    "GNUPGHOME",
    ...GIT_OVERRIDES.flatMap((_, n) => [`GIT_CONFIG_KEY_${n}`, `GIT_CONFIG_VALUE_${n}`]),
  ]);
  const provider = PROVIDER_DIR_ENV[harness];
  if (provider !== undefined) keys.add(provider);
  const store = storeEnvName(harness);
  if (store !== undefined) keys.add(store);
  if (platform === "win32") for (const k of ["TEMP", "TMP", "LOCALAPPDATA"]) keys.add(k);
  if (OWN_HOME.has(harness)) for (const k of platform === "win32" ? ["HOME", "USERPROFILE"] : ["HOME", "SHELL"]) keys.add(k);
  return keys;
}

export interface SessionEnvInput {
  readonly session: Pick<Session, "id" | "harness">;
  /** `FIN_SERVER`: the local socket, the agent listener (`tcp:`) or the pipe. */
  readonly server: string;
  /** The session's project folder, links resolved. */
  readonly projectDir: string;
  readonly dirs: SessionDirs;
  readonly store?: LinkedStore | undefined;
  readonly parent?: NodeJS.ProcessEnv;
  readonly platform?: NodeJS.Platform;
}

/** The names a host sandbox's environment adds on top of `childEnv`'s allow-list. */
export function hostKeys(harness: Harness, platform: NodeJS.Platform = process.platform): Set<string> {
  const keys = sessionKeys(harness, platform);
  for (const k of SESSION_ONLY) keys.delete(k);
  return keys;
}

/** The session environment (§8.5). */
export function sessionEnv(i: SessionEnvInput): Record<string, string> {
  const platform = i.platform ?? process.platform;
  const env = childEnv(
    { FIN_SERVER: i.server, FIN_SESSION: i.session.id, FIN_PROJECT_DIR: i.projectDir, FIN_CREDENTIAL: i.dirs.credential },
    i.parent ?? process.env,
    platform,
  );
  if (i.store?.env !== undefined) env[i.store.env.name] = i.store.env.value;
  return ownDirs(env, i.session.harness, i.dirs, platform);
}

export interface HostEnvInput {
  readonly harness: Harness;
  readonly dirs: SessionDirs;
  /** Detection's host sandbox only (Decision 22): the sign-in store `linkSignInStore` answered. */
  readonly store?: LinkedStore | undefined;
  readonly parent?: NodeJS.ProcessEnv;
  readonly platform?: NodeJS.Platform;
}

/**
 * A host sandbox's own `HOME` (Task 5 review), and a Claude Code session's (Decision 5):
 * `tmp/home`, which `ensureSessionDirs` makes.
 */
export function hostHome(dirs: SessionDirs, platform: NodeJS.Platform = process.platform): string {
  return (platform === "win32" ? win32 : posix).join(dirs.tmp, "home");
}

/**
 * A host sandbox's environment (plan 7D Task 3b): `childEnv()` and the directory's own `TMPDIR`,
 * provider directory, git settings and `GNUPGHOME`; no `FIN_*` connection, no project, no
 * credential; the sign-in store variable only with `store` (detection's). `HOME` (and Windows'
 * `USERPROFILE`) is the throwaway `hostHome`, never the server's: what a CLI reads under the
 * user's home (`$HOME/.config/anthropic/…`) cannot sign the probe in. Sessions keep the server's,
 * but for Claude Code's (`OWN_HOME`).
 */
export function hostEnv(i: HostEnvInput): Record<string, string> {
  const platform = i.platform ?? process.platform;
  const env = childEnv({}, i.parent ?? process.env, platform);
  if (i.store?.env !== undefined) env[i.store.env.name] = i.store.env.value;
  env.HOME = hostHome(i.dirs, platform);
  if (platform === "win32") env.USERPROFILE = env.HOME;
  return ownDirs(env, i.harness, i.dirs, platform);
}

/** The directory-derived names of a session or host environment. */
function ownDirs(env: Record<string, string>, harness: Harness, dirs: SessionDirs, platform: NodeJS.Platform): Record<string, string> {
  env.TMPDIR = dirs.tmp;
  if (platform === "win32") {
    env.TEMP = dirs.tmp;
    env.TMP = dirs.tmp;
    env.LOCALAPPDATA = dirs.localAppData;
  }
  const provider = PROVIDER_DIR_ENV[harness];
  if (provider !== undefined) env[provider] = dirs.provider;
  env.GIT_CONFIG_GLOBAL = dirs.gitConfig;
  env.GIT_CONFIG_NOSYSTEM = "1";
  env.GIT_CONFIG_COUNT = String(GIT_OVERRIDES.length);
  GIT_OVERRIDES.forEach(([k, v], n) => {
    env[`GIT_CONFIG_KEY_${n}`] = k;
    env[`GIT_CONFIG_VALUE_${n}`] = v;
  });
  env.GNUPGHOME = dirs.gnupg;
  if (OWN_HOME.has(harness)) {
    env.HOME = hostHome(dirs, platform);
    if (platform === "win32") env.USERPROFILE = env.HOME;
    else env.SHELL = sessionShell();
  }
  return env;
}

/**
 * Never in a session's environment (§8.5), whatever an adapter adds: `NEVER_PASSED`, Claude
 * Code's token and `FIN_CALL_IDEM` (a shell runs several `fin`s, §3.4); with `secretName`, what
 * `assertSessionEnv` holds an adapter's own environment to.
 */
export const NEVER_IN_SESSION: ReadonlyArray<string> = [...NEVER_PASSED, "CLAUDE_CODE_OAUTH_TOKEN", "FIN_CALL_IDEM"];

export interface AssertOptions {
  /**
   * The sandbox's own session environment (`SessionSandbox.env`): every session name must have
   * exactly its value there (the strict check; without it only the shape is checked).
   */
  readonly expect?: Readonly<Record<string, string>>;
  /**
   * Names the adapter may add beyond the session's (a CLI's own documented switches, Claude
   * Code's non-secret `CLAUDE_CODE_*` among them). Never a name of `NEVER_IN_SESSION` or a key.
   */
  readonly extra?: ReadonlyArray<string>;
  /**
   * A host sandbox's environment (`hostEnv`; `s.id` is its `host-<ulid>`): the names of
   * `SESSION_ONLY` are refused instead of required, the own paths are checked from `TMPDIR`, and
   * `HOME` (Windows: `USERPROFILE` too) must be its throwaway `tmp/home`.
   */
  readonly host?: boolean;
  /**
   * The harness's sign-in store variable (a `dir` store: Claude Code's) is required and must name
   * `providers/<harness>/home/<store>`; without it the variable is refused. Default: a session
   * has it, a host sandbox not (only detection's has it, plan 7D Decision 22; the probe never).
   */
  readonly store?: boolean;
  readonly platform?: NodeJS.Platform;
}

const ALLOW_LIST = new Set(["PATH", "HOME", "LANG", "TMPDIR", "Path", "USERPROFILE", "SYSTEMROOT", "SystemRoot", "TEMP", "TMP", "LOCALAPPDATA"]);
/** A key whatever an adapter declares: every `*_API_KEY` (§8.5). */
const isKey = (k: string) => /_API_KEY$/i.test(k);

/**
 * Throws `SANDBOX_ENV` (a `FinError`) unless `env` is a session environment of `s` (§8.5):
 * every name `sessionEnv` sets is present; `FIN_SESSION` is the session's id; the credential,
 * `TMPDIR`, the provider directory, the git configuration and `GNUPGHOME` are the session's own
 * paths under `agents/<id>/` (absolute), and for Claude Code `HOME` (Windows `USERPROFILE`) its
 * `tmp/home` and `SHELL` one of `SESSION_SHELLS`; the git settings are exactly `GIT_OVERRIDES` with
 * `GIT_CONFIG_NOSYSTEM=1`; `FIN_PROJECT_DIR` is absolute; no name of `NEVER_IN_SESSION` and no
 * key (names in any case); nothing outside the allow-list, `LC_*`, the session's names,
 * `FIN_CALL` and `opts.extra`; `FIN_CALL` and the extra names with their fixed values
 * (`EXTRA_VALUES`); the sign-in store variable as `opts.store` says. With `opts.expect` (the
 * sandbox's `env`), every session name has its value exactly. 7D and 7E call it on every launch
 * environment.
 */
export function assertSessionEnv(env: Readonly<Record<string, string | undefined>>, s: Pick<Session, "id" | "harness">, opts: AssertOptions = {}): void {
  const platform = opts.platform ?? process.platform;
  const path = platform === "win32" ? win32 : posix;
  const fail = (reason: string, name: string): never => {
    throw finError("SANDBOX_ENV", `the session environment is wrong: ${reason}`, { sessionId: s.id, name, reason });
  };
  const host = opts.host === true;
  const keys = host ? hostKeys(s.harness, platform) : sessionKeys(s.harness, platform);
  const storeName = storeEnvName(s.harness);
  const withStore = opts.store ?? !host;
  if (storeName !== undefined) {
    if (withStore) keys.add(storeName);
    else keys.delete(storeName);
  }
  const extra = new Set(opts.extra ?? []);
  for (const [k, v] of Object.entries(env)) {
    if (v === undefined) continue;
    if (hasFolded(NEVER_IN_SESSION, k)) fail("a variable that never passes", k);
    if (isKey(k)) fail("a secret", k);
    if (host && hasFolded(SESSION_ONLY, k) && !(withStore && k === storeName)) fail("a session variable in a host sandbox", k);
    if (!host && storeName !== undefined && !withStore && k.toUpperCase() === storeName) fail("a sign-in store without one", k);
    const value = EXTRA_VALUES[k];
    if (value !== undefined && (extra.has(k) || k === "FIN_CALL") && !value(v, env)) fail("a wrong value", k);
    if (extra.has(k) && !keys.has(k)) continue;
    if (secretName(k)) fail("a secret", k);
    if (!(ALLOW_LIST.has(k) || k.startsWith("LC_") || keys.has(k) || k === "FIN_CALL")) fail("a variable outside the session's", k);
  }
  const need = (k: string): string => {
    const v = env[k];
    if (v === undefined || (v === "" && !k.startsWith("GIT_CONFIG_VALUE_"))) fail("a missing variable", k);
    return v!;
  };
  for (const k of keys) need(k);
  if (!host && need("FIN_SESSION") !== s.id) fail("another session's id", "FIN_SESSION");
  const own = (k: string, ...rel: string[]) => {
    const v = need(k);
    if (!path.isAbsolute(v) || path.normalize(v) !== v || !v.endsWith(path.sep + path.join("agents", s.id, ...rel))) fail("not the session's own path", k);
    return v;
  };
  // The session's (or host sandbox's) directory, from the credential (or `TMPDIR`).
  const [anchor, rel] = host ? ["TMPDIR", "tmp"] : ["FIN_CREDENTIAL", "credential.json"];
  const first = own(anchor, rel);
  const base = first.slice(0, first.length - rel.length - 1);
  const under = (k: string, ...rel: string[]) => {
    if (own(k, ...rel) !== path.join(base, ...rel)) fail("not the session's own path", k);
  };
  under("TMPDIR", "tmp");
  under("GIT_CONFIG_GLOBAL", "git", "config");
  under("GNUPGHOME", "tmp", "gnupg");
  if (host || OWN_HOME.has(s.harness)) {
    under("HOME", "tmp", "home");
    if (platform === "win32") under("USERPROFILE", "tmp", "home");
  }
  if (keys.has("SHELL") && !SESSION_SHELLS.includes(need("SHELL"))) fail("not a session shell", "SHELL");
  const provider = PROVIDER_DIR_ENV[s.harness];
  if (provider !== undefined) under(provider, "provider");
  if (platform === "win32") {
    under("TEMP", "tmp");
    under("TMP", "tmp");
    under("LOCALAPPDATA", "localappdata");
  }
  const storeOf = SIGN_IN_STORES[s.harness];
  if (storeName !== undefined && withStore && storeOf !== undefined) {
    const store = need(storeName);
    if (!path.isAbsolute(store) || path.normalize(store) !== store || !store.endsWith(path.sep + path.join("providers", s.harness, "home", storeOf.name))) fail("not the harness's store", storeName);
  }
  if (need("GIT_CONFIG_NOSYSTEM") !== "1") fail("system git configuration read", "GIT_CONFIG_NOSYSTEM");
  if (need("GIT_CONFIG_COUNT") !== String(GIT_OVERRIDES.length)) fail("not the session's git settings", "GIT_CONFIG_COUNT");
  GIT_OVERRIDES.forEach(([k, v], n) => {
    if (need(`GIT_CONFIG_KEY_${n}`) !== k) fail("not the session's git settings", `GIT_CONFIG_KEY_${n}`);
    if (need(`GIT_CONFIG_VALUE_${n}`) !== v) fail("not the session's git settings", `GIT_CONFIG_VALUE_${n}`);
  });
  if (!host && !path.isAbsolute(need("FIN_PROJECT_DIR"))) fail("not an absolute folder", "FIN_PROJECT_DIR");
  if (opts.expect !== undefined) {
    for (const k of keys) if (env[k] !== opts.expect[k]) fail("not the sandbox's value", k);
  }
}

/**
 * Throws `SANDBOX_ENV` unless `add` (one process's `SpawnOptions.env`) only names what `harness`
 * may add (`harnessExtra`): never a name of `NEVER_IN_SESSION` or a key, never a session name
 * (`sessionKeys`, `SESSION_ONLY`: a process cannot move its credential, server, project,
 * temporary, provider or git directories), nothing outside the harness's list; names compared in
 * any case. The merged environment is then held to `assertSessionEnv` as a whole, which checks
 * the values (`EXTRA_VALUES`).
 */
export function assertSpawnEnv(add: Readonly<Record<string, string>>, s: Pick<Session, "id" | "harness">, platform: NodeJS.Platform = process.platform): void {
  const fail = (reason: string, name: string): never => {
    throw finError("SANDBOX_ENV", `a process's environment is wrong: ${reason}`, { sessionId: s.id, name, reason });
  };
  const session = sessionKeys(s.harness, platform);
  const allowed = harnessExtra(s.harness);
  for (const k of Object.keys(add)) {
    // Names in any case: Windows reads `fin_session` as `FIN_SESSION`.
    if (hasFolded(NEVER_IN_SESSION, k)) fail("a variable that never passes", k);
    if (isKey(k)) fail("a secret", k);
    if (hasFolded(session, k) || hasFolded(SESSION_ONLY, k)) fail("a session variable", k);
    if (!allowed.includes(k)) fail("a variable outside the harness's", k);
  }
}
