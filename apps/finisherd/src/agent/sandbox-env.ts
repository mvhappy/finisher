// The session environment of spec §8.5 (plan 7C Task 11): `childEnv()` plus exactly the
// session's variables. `sessionEnv` builds it; `assertSessionEnv` is the contract a CLI
// adapter (7D, 7E) checks its launch environment against, so a harness that sets its own
// environment cannot drop `FIN_PROJECT_DIR` or pass a secret by accident.
import { isAbsolute, join } from "node:path";
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
  /** The exact values to hold the session's variables to (the runtime's own check). */
  readonly expect?: { readonly projectDir?: string; readonly credential?: string; readonly server?: string };
  /** Names the adapter may add beyond the session's (a CLI's own documented switches). */
  readonly extra?: ReadonlyArray<string>;
  readonly platform?: NodeJS.Platform;
}

const ALLOW_LIST = new Set(["PATH", "HOME", "LANG", "TMPDIR", "Path", "USERPROFILE", "SYSTEMROOT", "SystemRoot", "TEMP", "TMP", "LOCALAPPDATA"]);

/**
 * Throws `SANDBOX_ENV` (a `FinError`) unless `env` is a session environment of `s` (§8.5):
 * `FIN_SESSION` is the session's id, `FIN_CREDENTIAL` its `agents/<id>/credential.json`,
 * `FIN_PROJECT_DIR` an absolute folder, `FIN_SERVER` and `TMPDIR` set; no name of
 * `NEVER_IN_SESSION` and no secret; nothing outside the allow-list, `LC_*`, the session's
 * names, `FIN_CALL` and `opts.extra`. 7D and 7E call it on every launch environment.
 */
export function assertSessionEnv(env: Readonly<Record<string, string | undefined>>, s: Pick<Session, "id" | "harness">, opts: AssertOptions = {}): void {
  const fail = (reason: string, name: string): never => {
    throw finError("SANDBOX_ENV", `the session environment is wrong: ${reason}`, { sessionId: s.id, name, reason });
  };
  const keys = sessionKeys(s.harness, opts.platform);
  const extra = new Set(opts.extra ?? []);
  for (const [k, v] of Object.entries(env)) {
    if (v === undefined) continue;
    if (NEVER_IN_SESSION.includes(k)) fail("a variable that never passes", k);
    if (secretName(k)) fail("a secret", k);
    if (!(ALLOW_LIST.has(k) || k.startsWith("LC_") || keys.has(k) || k === "FIN_CALL" || extra.has(k))) fail("a variable outside the session's", k);
  }
  const need = (k: string): string => {
    const v = env[k];
    if (v === undefined || v === "") fail("a missing variable", k);
    return v!;
  };
  if (need("FIN_SESSION") !== s.id) fail("another session's id", "FIN_SESSION");
  const cred = need("FIN_CREDENTIAL");
  if (!cred.endsWith(join("agents", s.id, "credential.json"))) fail("not the session's credential", "FIN_CREDENTIAL");
  const dir = need("FIN_PROJECT_DIR");
  if (!isAbsolute(dir)) fail("not an absolute folder", "FIN_PROJECT_DIR");
  const server = need("FIN_SERVER");
  need("TMPDIR");
  const x = opts.expect;
  if (x?.projectDir !== undefined && dir !== x.projectDir) fail("another project's folder", "FIN_PROJECT_DIR");
  if (x?.credential !== undefined && cred !== x.credential) fail("not the session's credential", "FIN_CREDENTIAL");
  if (x?.server !== undefined && server !== x.server) fail("another endpoint", "FIN_SERVER");
}
