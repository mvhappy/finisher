// Server-data location, local socket path and the `finisherd.json` discovery file.
import { createHash, randomBytes } from "node:crypto";
import { chmodSync, lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir, userInfo } from "node:os";
import { join, resolve } from "node:path";
import { PROTOCOL_VERSION } from "@finisher/contracts";

export const SERVER_VERSION = "0.0.0";

/** Plan decision 6: `FIN_SERVER_DATA`, else the per-OS application data directory. */
export function defaultServerData(
  env: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
  home: string = homedir(),
): string {
  if (env.FIN_SERVER_DATA) return resolve(env.FIN_SERVER_DATA);
  if (platform === "darwin") return join(home, "Library", "Application Support", "Finisher");
  if (platform === "win32") return join(env.LOCALAPPDATA ?? join(home, "AppData", "Local"), "Finisher");
  return join(env.XDG_DATA_HOME || join(home, ".local", "share"), "finisher");
}

/**
 * Plan decision 5: `<server-data>/fin.sock`, or on Windows the named pipe
 * `\\.\pipe\finisher-<username>` for the default server-data. Pipe names are global per
 * machine rather than per directory, so any other server-data gets its own pipe
 * `finisher-<username>-<hash>` (12 hex chars of SHA-256 of the lower-cased path): otherwise a
 * second server-data (tests, a temp dir) would collide with the default server's pipe.
 * Clients find the actual name in `finisherd.json`.
 */
export function socketPath(
  serverData: string,
  platform: NodeJS.Platform = process.platform,
  defaultDir: string = defaultServerData({ ...process.env, FIN_SERVER_DATA: undefined }, platform),
  username: string = userInfo().username,
): string {
  if (platform !== "win32") return join(serverData, "fin.sock");
  const pipe = `\\\\.\\pipe\\finisher-${username}`;
  const key = (dir: string) => dir.replace(/[\\/]+$/, "").toLowerCase();
  if (key(serverData) === key(defaultDir)) return pipe;
  return `${pipe}-${createHash("sha256").update(key(serverData)).digest("hex").slice(0, 12)}`;
}

export interface CliArgs {
  readonly serverData: string;
}

export function parseArgs(argv: ReadonlyArray<string>, env: NodeJS.ProcessEnv = process.env): CliArgs {
  let serverData: string | undefined;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--server-data") {
      const value = argv[++i];
      if (!value) throw new Error("--server-data needs a directory");
      serverData = resolve(value);
    } else if (arg?.startsWith("--server-data=")) {
      serverData = resolve(arg.slice("--server-data=".length));
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return { serverData: serverData ?? defaultServerData(env) };
}

/** Contents of `<server-data>/finisherd.json` (mode 0600). */
export interface ServerInfo {
  readonly pid: number;
  readonly wsPort: number;
  readonly socket: string;
  readonly token: string;
  readonly protocolVersion: number;
  /** Whether the server records diagnostics now (plan D, Decision 23: `fin` sends nothing when false). */
  readonly diagnostics: { readonly enabled: boolean };
}

export function newToken(): string {
  return randomBytes(32).toString("base64url");
}

export function infoPath(serverData: string): string {
  return join(serverData, "finisherd.json");
}

/** A startup failure with a stable code; `main` prints `finisherd: <code>: <message>`. */
export class ServerStartError extends Error {
  readonly code: string;
  readonly details: Record<string, unknown>;
  constructor(code: string, message: string, details: Record<string, unknown> = {}) {
    super(message);
    this.code = code;
    this.details = details;
  }
}

/**
 * Any startup failure as a `ServerStartError`: our own and fin-node's (`SERVER_RUNNING`,
 * `SERVER_START_FAILED`) keep their code; anything else (EACCES, ENOTDIR, …) becomes
 * `SERVER_START_FAILED` with the original code in `details.cause`.
 */
export function startFailure(e: unknown): ServerStartError {
  if (e instanceof ServerStartError) return e;
  const err = e as { code?: unknown; message?: unknown; details?: unknown; finNode?: unknown; name?: unknown } | null;
  const message = typeof err?.message === "string" ? err.message : String(e);
  if (err?.finNode === true && typeof err.code === "string") {
    return new ServerStartError(err.code, message, (err.details as Record<string, unknown>) ?? {});
  }
  const cause = typeof err?.code === "string" ? err.code : typeof err?.name === "string" ? err.name : "unknown";
  return new ServerStartError("SERVER_START_FAILED", message, { cause });
}

/**
 * Creates the server-data directory (0700) or checks an existing one: it must be a real
 * directory (not a symlink) owned by this user; group/other permissions are stripped.
 */
export function ensureServerData(serverData: string, platform: NodeJS.Platform = process.platform): void {
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  const st = lstatSync(serverData);
  if (st.isSymbolicLink()) throw new ServerStartError("SERVER_DATA_UNSAFE", `${serverData} is a symlink`);
  if (!st.isDirectory()) throw new ServerStartError("SERVER_DATA_UNSAFE", `${serverData} is not a directory`);
  if (platform === "win32") return;
  if (process.getuid && st.uid !== process.getuid()) {
    throw new ServerStartError("SERVER_DATA_UNSAFE", `${serverData} is owned by uid ${st.uid}, not this user`);
  }
  if (st.mode & 0o077) chmodSync(serverData, 0o700);
}

/** Writes the discovery file atomically: a private `<file>.<pid>.tmp` (0600, `wx`) renamed into place. */
export function writeServerInfo(serverData: string, info: Omit<ServerInfo, "protocolVersion">): ServerInfo {
  const full: ServerInfo = { ...info, protocolVersion: PROTOCOL_VERSION };
  const file = infoPath(serverData);
  const tmp = `${file}.${process.pid}.tmp`;
  rmSync(tmp, { force: true });
  writeFileSync(tmp, `${JSON.stringify(full, null, 2)}\n`, { mode: 0o600, flag: "wx" });
  renameSync(tmp, file);
  return full;
}

/**
 * finisherd's settings, `<server-data>/settings.json`, re-read while the server runs:
 * - `fonts.autoFetch` (default `true`): fetch the fonts projects pin without being asked
 *   (plan 4C Decision 34). `false` leaves fetching to `fin fonts fetch`; so does
 *   `FIN_FONTS_OFFLINE=1`, whatever this says.
 */
export interface Settings {
  readonly fonts: { readonly autoFetch: boolean };
}

export const DEFAULT_SETTINGS: Settings = { fonts: { autoFetch: true } };

export function settingsPath(serverData: string): string {
  return join(serverData, "settings.json");
}

/** What an unusable `settings.json` means before any good read: fetch nothing unasked. */
export const UNREADABLE_SETTINGS: Settings = { fonts: { autoFetch: false } };

/**
 * The settings: defaults for a missing file. A present file that can't be used (unreadable,
 * invalid or truncated JSON — an editor mid-save, a trailing comma — or a wrongly typed value)
 * keeps `last`, the last good settings, or before any is known `UNREADABLE_SETTINGS`, so a bad
 * save never switches fetching on; `error` says why the file was not used.
 */
export function readSettings(serverData: string, last?: Settings): { settings: Settings; error?: string } {
  const fallback = last ?? UNREADABLE_SETTINGS;
  let text: string;
  try {
    text = readFileSync(settingsPath(serverData), "utf8");
  } catch (e) {
    const code = (e as { code?: unknown }).code;
    return code === "ENOENT" ? { settings: DEFAULT_SETTINGS } : { settings: fallback, error: String(e) };
  }
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    return { settings: fallback, error: (e as Error).message };
  }
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    return { settings: fallback, error: "settings must be a JSON object" };
  }
  const fonts = (raw as { fonts?: unknown }).fonts;
  if (fonts !== undefined && (fonts === null || typeof fonts !== "object" || Array.isArray(fonts))) {
    return { settings: fallback, error: "fonts must be an object" };
  }
  const autoFetch = (fonts as { autoFetch?: unknown } | undefined)?.autoFetch;
  if (autoFetch !== undefined && typeof autoFetch !== "boolean") {
    return { settings: fallback, error: "fonts.autoFetch must be true or false" };
  }
  return { settings: { fonts: { autoFetch: autoFetch ?? DEFAULT_SETTINGS.fonts.autoFetch } } };
}

/**
 * `settings.json`'s raw `privacy` section (plan D, Decision 8): `privacy` is what the file
 * holds (`undefined` when the file or the key is absent); `error` says why a present file can't
 * be used (unreadable, invalid JSON, not an object), which keeps the last good privacy
 * settings, or before any is known means recording nothing. The section's own validation
 * (`on`/`off`, ranges) is the journal's, with the same last-good rule.
 */
export function readPrivacy(serverData: string): { privacy?: unknown; error?: string; missing?: true } {
  let text: string;
  try {
    text = readFileSync(settingsPath(serverData), "utf8");
  } catch (e) {
    return (e as { code?: unknown }).code === "ENOENT" ? { missing: true } : { error: String(e) };
  }
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    return { error: (e as Error).message };
  }
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return { error: "settings must be a JSON object" };
  return { privacy: (raw as { privacy?: unknown }).privacy };
}

/**
 * The `privacy` argument of fin-node's `Journal.open`/`applySettings`: the section's JSON,
 * `null` when absent, or text that is not JSON for an unusable file (the journal then keeps
 * the last good settings, or records nothing before any).
 */
export function privacyArg(read: { privacy?: unknown; error?: string; missing?: true }): string | null {
  if (read.error !== undefined) return "<unusable settings.json>";
  return read.privacy === undefined ? null : JSON.stringify(read.privacy);
}

export function removeServerInfo(serverData: string): void {
  rmSync(infoPath(serverData), { force: true });
}
