// An agent session's directories in server data (spec §8.5, §8.6; plan 7C Task 11):
// `<server-data>/agents/<sessionId>/` holds `credential.json` (written by `credentials.ts`,
// 0600), `settings/` (what Finisher writes for the provider; read-only to the sandbox),
// `tmp/` (`TMPDIR`), `provider/` (the session's own CLI configuration directory), `git/` (the
// session's git configuration) and `blobs/` (product state, denied to the sandbox). Every
// directory is private (0700); an existing one is made private again.
//
// The CLI's sign-in store stays in the harness's sign-in home `<server-data>/providers/
// <harness>/home/` and is reached in place (§8.6): `linkSignInStore` makes a link to it, or
// names a store-only directory, and **never opens anything under `providers/`** (no read, no
// stat of the target: `lstat` and `symlink` on the link's own name only).
//
// Windows: the ACL broker (plan 7C Task 10) grants these directories to the session's
// capability SID; `ensureSessionDirs` takes that step as `grant` so this module stays free of
// OS code.
import { chmod, lstat, mkdir, readlink, rm, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import type { Harness } from "@finisher/contracts";
import { sandboxSessionGitConfig } from "@finisher/fin-node";

export interface SessionDirs {
  /** `<server-data>/agents/<sessionId>`. */
  readonly dir: string;
  readonly credential: string;
  readonly settings: string;
  readonly tmp: string;
  readonly provider: string;
  readonly git: string;
  /** `git/config`: the file `GIT_CONFIG_GLOBAL` names. */
  readonly gitConfig: string;
  readonly blobs: string;
  /** `tmp/gnupg`: a private `GNUPGHOME` (spec §8.3), so no user's keyring is reached. */
  readonly gnupg: string;
  /** Windows: `localappdata/`, the session's `LOCALAPPDATA` (S9 point 7). */
  readonly localAppData: string;
}

/** The directory names under `<server-data>/agents/<sessionId>/`. */
export function sessionDirs(agentsRoot: string, sessionId: string): SessionDirs {
  const dir = join(agentsRoot, sessionId);
  const git = join(dir, "git");
  const tmp = join(dir, "tmp");
  return {
    dir,
    credential: join(dir, "credential.json"),
    settings: join(dir, "settings"),
    tmp,
    provider: join(dir, "provider"),
    git,
    gitConfig: join(git, "config"),
    blobs: join(dir, "blobs"),
    gnupg: join(tmp, "gnupg"),
    localAppData: join(dir, "localappdata"),
  };
}

export interface EnsureOptions {
  readonly platform?: NodeJS.Platform;
  /** Windows: the broker's grant of one directory to the session (Task 10). */
  readonly grant?: (dir: string) => Promise<void>;
}

/** Makes the session's directories (0700) and its git configuration; answers their names. */
export async function ensureSessionDirs(agentsRoot: string, sessionId: string, opts: EnsureOptions = {}): Promise<SessionDirs> {
  const platform = opts.platform ?? process.platform;
  const d = sessionDirs(agentsRoot, sessionId);
  const dirs = [d.dir, d.settings, d.tmp, d.provider, d.git, d.blobs, d.gnupg, ...(platform === "win32" ? [d.localAppData] : [])];
  for (const p of dirs) {
    await mkdir(p, { recursive: true, mode: 0o700 });
    // `mkdir` leaves an existing directory's mode as it was.
    if (platform !== "win32") await chmod(p, 0o700);
  }
  await writeFile(d.gitConfig, sandboxSessionGitConfig(), { mode: 0o600 });
  if (platform !== "win32") await chmod(d.gitConfig, 0o600);
  if (opts.grant !== undefined) for (const p of dirs) await opts.grant(p);
  return d;
}

/**
 * How a harness's CLI reaches its sign-in store (spec §8.6, spike S9 B7):
 * - `link`: the store is a file of the sign-in home; the session's `provider/` holds a link of
 *   that name to it (Codex's `auth.json` under `CODEX_HOME`);
 * - `dir`: the CLI writes its store by temp file and rename and refuses a link (Claude Code
 *   2.1.319), so the store lives in a directory of its own, named by an environment variable
 *   (`CLAUDE_SECURESTORAGE_CONFIG_DIR`; undocumented, so 7D re-checks it per CLI version), and
 *   the sandbox grants that directory, which holds nothing the CLI loads as configuration.
 */
export type SignInStore =
  | { readonly kind: "link"; readonly name: string }
  | { readonly kind: "dir"; readonly name: string; readonly env: string };

/** Each CLI harness's store (the built-in agent has none). */
export const SIGN_IN_STORES: Readonly<Partial<Record<Harness, SignInStore>>> = {
  claudeCode: { kind: "dir", name: "store", env: "CLAUDE_SECURESTORAGE_CONFIG_DIR" },
  codex: { kind: "link", name: "auth.json" },
  fake: { kind: "link", name: "sign-in.json" },
};

/** The variable naming the session's provider directory (§8.5), per CLI harness. */
export const PROVIDER_DIR_ENV: Readonly<Partial<Record<Harness, string>>> = {
  claudeCode: "CLAUDE_CONFIG_DIR",
  codex: "CODEX_HOME",
  fake: "FIN_FAKE_PROVIDER_DIR",
};

/** What the session's sandbox is given for the store. */
export interface LinkedStore {
  /** The path the sandbox grants read and write on: the store file, or the store directory. */
  readonly grant: string;
  /** For a `dir` store: the variable and its value. */
  readonly env?: { readonly name: string; readonly value: string };
}

/**
 * Connects `provider/` to `harness`'s sign-in store in `<server-data>/providers/<harness>/home/`
 * (`serverData` canonical). `link`: makes (or keeps) the link `provider/<name>` → the store
 * file, replacing anything else of that name; `dir`: creates the store directory if missing,
 * never looking inside it. Never opens, reads or follows anything under `providers/`.
 */
export async function linkSignInStore(harness: Harness, serverData: string, dirs: SessionDirs): Promise<LinkedStore | undefined> {
  const store = SIGN_IN_STORES[harness];
  if (store === undefined) return undefined;
  const home = join(serverData, "providers", harness, "home");
  const target = join(home, store.name);
  if (store.kind === "dir") {
    await mkdir(target, { recursive: true, mode: 0o700 });
    return { grant: target, env: { name: store.env, value: target } };
  }
  const link = join(dirs.provider, store.name);
  const existing = await lstat(link).catch(() => undefined);
  if (existing?.isSymbolicLink() === true && (await readlink(link)) === target) return { grant: target };
  if (existing !== undefined) await rm(link, { recursive: true, force: true });
  await symlink(target, link, "file");
  return { grant: target };
}
