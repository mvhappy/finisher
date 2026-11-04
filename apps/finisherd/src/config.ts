// Server-data location, local socket path and the `finisherd.json` discovery file.
import { createHash, randomBytes } from "node:crypto";
import { chmodSync, lstatSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
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

export function removeServerInfo(serverData: string): void {
  rmSync(infoPath(serverData), { force: true });
}
