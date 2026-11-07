// The server's key files (plan 5A Decision 5, spec §4.1):
//
//   <server-data>/auth/   (0700; protected owner-only DACL on Windows)
//     local.key            `fin`'s key        — new at every start
//     console.key          the console's key  — new at every start
//     credential.master    derives agent/MCP credential keys — created once, kept
//   <server-data>/run/    (0700) only the Unix socket `fin.sock`
//
// Each file is base64url (no padding) of 32 random bytes plus `\n`, mode 0600, written to a
// temp file in the same private directory (so it inherits the DACL on Windows), fsynced,
// renamed over the old one, and the directory fsynced (`writeKeyFile`). `rotateKeys` runs after the server lock and before any listener opens, so
// a key read from disk while a server listens is always that server's. Keys are never logged.
import { randomBytes } from "node:crypto";
import * as fs from "node:fs";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { Context } from "effect";
import { ensurePrivateDir, ServerStartError } from "../config.ts";
import { b64u, KEY_BYTES } from "./crypto.ts";

export interface Keys {
  readonly local: Buffer;
  readonly console: Buffer;
  readonly credentialMaster: Buffer;
}

export const KEY_FILES = { local: "local.key", console: "console.key", credentialMaster: "credential.master" } as const;

/** `<server-data>/auth`. */
export function authDir(serverData: string): string {
  return join(serverData, "auth");
}

/** `<server-data>/run`: the only server-data directory a sandbox may reach (spec §4.11). */
export function runDir(serverData: string): string {
  return join(serverData, "run");
}

const KEY_TEXT = /^[A-Za-z0-9_-]{43}\n?$/;

/** A key file's 32 bytes; throws (without echoing the contents) if it is malformed. */
export function readKeyFile(path: string): Buffer {
  const text = readFileSync(path, "utf8");
  const key = KEY_TEXT.test(text) ? b64u.decode(text.trimEnd()) : null;
  if (key === null || key.length !== KEY_BYTES) {
    throw new ServerStartError("KEY_FILE_INVALID", `${path} is not a key file (base64url of ${KEY_BYTES} bytes)`, { path });
  }
  return key;
}

const RETRYABLE = new Set(["EPERM", "EBUSY", "EACCES"]);

function sleepSync(ms: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

/** The `node:fs` calls a key write makes (tests inject a recording one). */
export interface KeyFs {
  openSync: typeof fs.openSync;
  writeSync: (fd: number, data: string) => number;
  fsyncSync: typeof fs.fsyncSync;
  closeSync: typeof fs.closeSync;
  renameSync: typeof fs.renameSync;
  rmSync: typeof fs.rmSync;
  chmodSync: typeof fs.chmodSync;
}

const REAL_FS: KeyFs = fs;

/**
 * Writes `key` to `path` (0600) durably: a temp file in the same directory, written and
 * fsynced, renamed over `path` (retried on Windows sharing violations), then the directory
 * fsynced so the rename survives a crash. On Windows `fsync` is `FlushFileBuffers` on the
 * temp file's write descriptor; Node cannot open a directory for flushing there, and NTFS
 * journals the rename itself.
 */
export function writeKeyFile(path: string, key: Buffer, platform: NodeJS.Platform = process.platform, io: KeyFs = REAL_FS): void {
  const tmp = `${path}.${process.pid}.tmp`;
  io.rmSync(tmp, { force: true });
  const fd = io.openSync(tmp, "wx", 0o600);
  try {
    io.writeSync(fd, `${b64u.encode(key)}\n`);
    io.fsyncSync(fd);
  } finally {
    io.closeSync(fd);
  }
  if (platform !== "win32") io.chmodSync(tmp, 0o600);
  for (let attempt = 0; ; attempt++) {
    try {
      io.renameSync(tmp, path);
      break;
    } catch (e) {
      const code = (e as { code?: unknown }).code;
      if (platform !== "win32" || typeof code !== "string" || !RETRYABLE.has(code) || attempt >= 20) {
        io.rmSync(tmp, { force: true });
        throw e;
      }
      sleepSync(25);
    }
  }
  if (platform === "win32") return;
  const dir = io.openSync(dirname(path), "r");
  try {
    io.fsyncSync(dir);
  } finally {
    io.closeSync(dir);
  }
}

/**
 * Makes `auth/` and `run/` private, writes new `local.key` and `console.key`, and creates
 * `credential.master` if it is missing (a present but malformed master refuses the start:
 * replacing it would silently invalidate every credential). Returns the keys in memory.
 */
export function rotateKeys(serverData: string, io: KeyFs = REAL_FS): Keys {
  const auth = authDir(serverData);
  ensurePrivateDir(auth);
  ensurePrivateDir(runDir(serverData));
  const local = randomBytes(KEY_BYTES);
  const console = randomBytes(KEY_BYTES);
  writeKeyFile(join(auth, KEY_FILES.local), local, process.platform, io);
  writeKeyFile(join(auth, KEY_FILES.console), console, process.platform, io);
  const masterPath = join(auth, KEY_FILES.credentialMaster);
  let credentialMaster: Buffer;
  try {
    credentialMaster = readKeyFile(masterPath);
  } catch (e) {
    if ((e as { code?: unknown }).code !== "ENOENT") throw e;
    credentialMaster = randomBytes(KEY_BYTES);
    writeKeyFile(masterPath, credentialMaster, process.platform, io);
  }
  return { local, console, credentialMaster };
}

/** The keys this server rotated at start (5A Task 3's handshake reads them). */
export class KeysTag extends Context.Service<KeysTag, Keys>()("finisherd/Keys") {}
