// The session environment of spec §8.5 (plan 7C Task 11): `childEnv()` plus exactly the
// session's variables. `sessionEnv` builds it; `assertSessionEnv` is the contract a CLI
// adapter (7D, 7E) checks its launch environment against, so a harness that sets its own
// environment cannot drop `FIN_PROJECT_DIR` or pass a secret by accident.
import { posix, win32 } from "node:path";
import type { Harness, Session } from "@finisher/contracts";
import { childEnv, NEVER_PASSED, secretName } from "../env.ts";
import { finError } from "../errors.ts";
import { type LinkedStore, PROVIDER_DIR_ENV, type SessionDirs } from "./session-dirs.ts";

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
  if (harness === "claudeCode") keys.add("CLAUDE_SECURESTORAGE_CONFIG_DIR");
  if (platform === "win32") for (const k of ["TEMP", "TMP", "LOCALAPPDATA"]) keys.add(k);
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

/** The session environment (§8.5). */
export function sessionEnv(i: SessionEnvInput): Record<string, string> {
  const platform = i.platform ?? process.platform;
  const env = childEnv(
    { FIN_SERVER: i.server, FIN_SESSION: i.session.id, FIN_PROJECT_DIR: i.projectDir, FIN_CREDENTIAL: i.dirs.credential },
    i.parent ?? process.env,
    platform,
  );
  env.TMPDIR = i.dirs.tmp;
  if (platform === "win32") {
    env.TEMP = i.dirs.tmp;
    env.TMP = i.dirs.tmp;
    env.LOCALAPPDATA = i.dirs.localAppData;
  }
  const provider = PROVIDER_DIR_ENV[i.session.harness];
  if (provider !== undefined) env[provider] = i.dirs.provider;
  if (i.store?.env !== undefined) env[i.store.env.name] = i.store.env.value;
  env.GIT_CONFIG_GLOBAL = i.dirs.gitConfig;
  env.GIT_CONFIG_NOSYSTEM = "1";
  env.GIT_CONFIG_COUNT = String(GIT_OVERRIDES.length);
  GIT_OVERRIDES.forEach(([k, v], n) => {
    env[`GIT_CONFIG_KEY_${n}`] = k;
    env[`GIT_CONFIG_VALUE_${n}`] = v;
  });
  env.GNUPGHOME = i.dirs.gnupg;
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
  readonly platform?: NodeJS.Platform;
}

const ALLOW_LIST = new Set(["PATH", "HOME", "LANG", "TMPDIR", "Path", "USERPROFILE", "SYSTEMROOT", "SystemRoot", "TEMP", "TMP", "LOCALAPPDATA"]);
/** A key whatever an adapter declares: every `*_API_KEY` (§8.5). */
const isKey = (k: string) => /_API_KEY$/i.test(k);

/**
 * Throws `SANDBOX_ENV` (a `FinError`) unless `env` is a session environment of `s` (§8.5):
 * every name `sessionEnv` sets is present; `FIN_SESSION` is the session's id; the credential,
 * `TMPDIR`, the provider directory, the git configuration and `GNUPGHOME` are the session's own
 * paths under `agents/<id>/` (absolute); the git settings are exactly `GIT_OVERRIDES` with
 * `GIT_CONFIG_NOSYSTEM=1`; `FIN_PROJECT_DIR` is absolute; no name of `NEVER_IN_SESSION` and no
 * key; nothing outside the allow-list, `LC_*`, the session's names, `FIN_CALL` and
 * `opts.extra`. With `opts.expect` (the sandbox's `env`), every session name has its value
 * exactly. 7D and 7E call it on every launch environment.
 */
export function assertSessionEnv(env: Readonly<Record<string, string | undefined>>, s: Pick<Session, "id" | "harness">, opts: AssertOptions = {}): void {
  const platform = opts.platform ?? process.platform;
  const path = platform === "win32" ? win32 : posix;
  const fail = (reason: string, name: string): never => {
    throw finError("SANDBOX_ENV", `the session environment is wrong: ${reason}`, { sessionId: s.id, name, reason });
  };
  const keys = sessionKeys(s.harness, platform);
  const extra = new Set(opts.extra ?? []);
  for (const [k, v] of Object.entries(env)) {
    if (v === undefined) continue;
    if (NEVER_IN_SESSION.includes(k)) fail("a variable that never passes", k);
    if (isKey(k)) fail("a secret", k);
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
  if (need("FIN_SESSION") !== s.id) fail("another session's id", "FIN_SESSION");
  const own = (k: string, ...rel: string[]) => {
    const v = need(k);
    if (!path.isAbsolute(v) || path.normalize(v) !== v || !v.endsWith(path.sep + path.join("agents", s.id, ...rel))) fail("not the session's own path", k);
    return v;
  };
  const cred = own("FIN_CREDENTIAL", "credential.json");
  const base = cred.slice(0, cred.length - "credential.json".length - 1);
  const under = (k: string, ...rel: string[]) => {
    if (own(k, ...rel) !== path.join(base, ...rel)) fail("not the session's own path", k);
  };
  under("TMPDIR", "tmp");
  under("GIT_CONFIG_GLOBAL", "git", "config");
  under("GNUPGHOME", "tmp", "gnupg");
  const provider = PROVIDER_DIR_ENV[s.harness];
  if (provider !== undefined) under(provider, "provider");
  if (platform === "win32") {
    under("TEMP", "tmp");
    under("TMP", "tmp");
    under("LOCALAPPDATA", "localappdata");
  }
  if (s.harness === "claudeCode") {
    const store = need("CLAUDE_SECURESTORAGE_CONFIG_DIR");
    if (!path.isAbsolute(store) || !store.endsWith(path.sep + path.join("providers", "claudeCode", "home", "store"))) fail("not the harness's store", "CLAUDE_SECURESTORAGE_CONFIG_DIR");
  }
  if (need("GIT_CONFIG_NOSYSTEM") !== "1") fail("system git configuration read", "GIT_CONFIG_NOSYSTEM");
  if (need("GIT_CONFIG_COUNT") !== String(GIT_OVERRIDES.length)) fail("not the session's git settings", "GIT_CONFIG_COUNT");
  GIT_OVERRIDES.forEach(([k, v], n) => {
    if (need(`GIT_CONFIG_KEY_${n}`) !== k) fail("not the session's git settings", `GIT_CONFIG_KEY_${n}`);
    if (need(`GIT_CONFIG_VALUE_${n}`) !== v) fail("not the session's git settings", `GIT_CONFIG_VALUE_${n}`);
  });
  if (!path.isAbsolute(need("FIN_PROJECT_DIR"))) fail("not an absolute folder", "FIN_PROJECT_DIR");
  if (opts.expect !== undefined) {
    for (const k of keys) if (env[k] !== opts.expect[k]) fail("not the sandbox's value", k);
  }
}
