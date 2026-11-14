// Key files, private directories and `run/` (plan 5A Task 2, Decisions 5 and 7).
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { checkPrivateDir, checkServerDataLocal, lockExclusive, makePrivateDir } from "@finisher/fin-node";
import * as fs from "node:fs";
import { authDir, KEY_FILES, type KeyFs, readKeyFile, rotateKeys, runDir, writeKeyFile, writePrivateFile } from "../src/auth/keys.ts";
import { ensureServerData, socketPath } from "../src/config.ts";
import { lockPath } from "../src/lock.ts";
import { NdjsonClient, ok, startServer } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";
let tmp: string;
let serverData: string;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "keys-"));
  serverData = join(tmp, "sd");
  mkdirSync(serverData, { mode: 0o700 });
});

afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const keyFile = (name: keyof typeof KEY_FILES) => join(authDir(serverData), KEY_FILES[name]);

describe("key files", () => {
  test("keys_rotate_on_every_start", () => {
    const a = rotateKeys(serverData);
    const b = rotateKeys(serverData);
    expect(a.local.equals(b.local)).toBe(false);
    expect(a.console.equals(b.console)).toBe(false);
    expect(a.local.equals(a.console)).toBe(false);
    expect(readKeyFile(keyFile("local")).equals(b.local)).toBe(true);
    expect(readKeyFile(keyFile("console")).equals(b.console)).toBe(true);
    for (const k of [b.local, b.console, b.credentialMaster]) expect(k).toHaveLength(32);
    expect(readFileSync(keyFile("local"), "utf8")).toMatch(/^[A-Za-z0-9_-]{43}\n$/);
    // No temp files left behind; nothing else in auth/.
    expect(readdirSync(authDir(serverData)).sort()).toEqual(["console.key", "credential.master", "local.key"]);
  });

  test("credential_master_survives_restart", () => {
    const a = rotateKeys(serverData);
    const b = rotateKeys(serverData);
    expect(a.credentialMaster.equals(b.credentialMaster)).toBe(true);
    expect(readKeyFile(keyFile("credentialMaster")).equals(a.credentialMaster)).toBe(true);
  });

  test.skipIf(WIN)("key_files_are_0600_dirs_0700", () => {
    mkdirSync(authDir(serverData), { mode: 0o755 });
    rotateKeys(serverData);
    for (const d of [authDir(serverData), runDir(serverData)]) expect(statSync(d).mode & 0o777).toBe(0o700);
    for (const f of ["local", "console", "credentialMaster"] as const) expect(statSync(keyFile(f)).mode & 0o777).toBe(0o600);
  });

  test("malformed_key_file_is_refused", () => {
    rotateKeys(serverData);
    for (const bad of ["", "short\n", `${"A".repeat(43)}\n\n`, `${"A".repeat(42)}=\n`, `${"+".repeat(43)}\n`, "A".repeat(44)]) {
      writeFileSync(keyFile("local"), bad);
      expect(() => readKeyFile(keyFile("local")), JSON.stringify(bad)).toThrow(expect.objectContaining({ code: "KEY_FILE_INVALID" }));
    }
    // The error never echoes the file's contents.
    writeFileSync(keyFile("local"), "secret-looking-garbage\n");
    expect(() => readKeyFile(keyFile("local"))).toThrow(expect.not.objectContaining({ message: expect.stringContaining("secret") }));
    // A malformed master refuses the start instead of being replaced.
    writeFileSync(keyFile("credentialMaster"), "garbage\n");
    expect(() => rotateKeys(serverData)).toThrow(expect.objectContaining({ code: "KEY_FILE_INVALID" }));
    expect(readFileSync(keyFile("credentialMaster"), "utf8")).toBe("garbage\n");
  });

  test("key files are fsynced before and after the rename", () => {
    const calls: string[] = [];
    const fds = new Map<number, string>();
    const io: KeyFs = {
      openSync: ((p: fs.PathLike, flags: fs.OpenMode, mode?: fs.Mode) => {
        const fd = fs.openSync(p, flags, mode);
        fds.set(fd, String(p).endsWith(".tmp") ? "tmp" : String(p) === authDir(serverData) ? "dir" : String(p));
        calls.push(`open ${fds.get(fd)}`);
        return fd;
      }) as typeof fs.openSync,
      writeSync: (fd, data) => (calls.push(`write ${fds.get(fd)}`), fs.writeSync(fd, data)),
      fsyncSync: (fd) => (calls.push(`fsync ${fds.get(fd)}`), fs.fsyncSync(fd)),
      closeSync: (fd) => (calls.push(`close ${fds.get(fd)}`), fs.closeSync(fd)),
      renameSync: (a, b) => (calls.push("rename"), fs.renameSync(a, b)),
      rmSync: fs.rmSync,
      chmodSync: fs.chmodSync,
    };
    fs.mkdirSync(authDir(serverData));
    writeKeyFile(keyFile("credentialMaster"), Buffer.alloc(32, 1), process.platform, io);
    const expected = ["open tmp", "write tmp", "fsync tmp", "close tmp", "rename"];
    if (!WIN) expected.push("open dir", "fsync dir", "close dir");
    expect(calls).toEqual(expected);
    expect(readKeyFile(keyFile("credentialMaster")).equals(Buffer.alloc(32, 1))).toBe(true);
    // rotateKeys writes every key that way.
    calls.length = 0;
    rotateKeys(serverData, io);
    expect(calls.filter((c) => c === "fsync tmp")).toHaveLength(2); // the master is kept
    expect(calls.filter((c) => c === "rename")).toHaveLength(2);
  });

  test("a short write is completed; a write that makes no progress fails and leaves nothing (Task 10 second review)", () => {
    fs.mkdirSync(authDir(serverData));
    const path = join(authDir(serverData), "state.json");
    const text = `${JSON.stringify({ v: 1, processes: [{ instance: "01I", pid: 12345, start: "linux:1" }], note: "é ✓" })}\n`;
    // At most 10 bytes per call (a multi-byte character split across calls).
    const short = (limit: number): KeyFs => ({ ...fs, writeSync: (fd, data) => fs.writeSync(fd, data, 0, Math.min(limit, data.byteLength)) });
    writePrivateFile(path, text, process.platform, short(10));
    expect(readFileSync(path, "utf8")).toBe(text);
    // No progress: an error, the old file kept, no temp file left.
    expect(() => writePrivateFile(path, "{}\n", process.platform, short(0))).toThrow(/short write/);
    expect(readFileSync(path, "utf8")).toBe(text);
    expect(readdirSync(authDir(serverData)).filter((f) => f.endsWith(".tmp"))).toEqual([]);
  });

  test("socket_lives_in_run_dir", () => {
    if (!WIN) expect(socketPath(serverData)).toBe(join(runDir(serverData), "fin.sock"));
    else expect(socketPath(serverData)).toMatch(/^\\\\\.\\pipe\\finisher-/);
  });
});

describe("start order", () => {
  test("keys_rotate_before_listen", { timeout: 30_000 }, async () => {
    // lock → store → keys → socket: an unwritable auth/ fails the start after the store
    // opened and before the socket exists.
    writeFileSync(authDir(serverData), "not a directory");
    await expect(startServer(serverData, 15_000)).rejects.toThrow(/SERVER_DATA_UNSAFE/);
    expect(existsSync(join(serverData, "control.db"))).toBe(true);
    expect(existsSync(join(runDir(serverData), "fin.sock"))).toBe(false);
    expect(existsSync(join(serverData, "finisherd.json"))).toBe(false);
    rmSync(authDir(serverData));
    // A held server lock: nothing else is touched.
    rmSync(join(serverData, "control.db"));
    const lock = lockExclusive(lockPath(serverData));
    try {
      await expect(startServer(serverData, 15_000)).rejects.toThrow(/SERVER_RUNNING/);
      expect(existsSync(join(serverData, "control.db"))).toBe(false);
      expect(existsSync(authDir(serverData))).toBe(false);
    } finally {
      lock.release();
    }
  });

  test("run_dir_holds_only_the_socket", { timeout: 30_000 }, async () => {
    const server = await startServer(serverData);
    try {
      if (!WIN) expect(readdirSync(runDir(serverData))).toEqual(["fin.sock"]);
      else expect(readdirSync(runDir(serverData))).toEqual([]);
      expect(server.info.socket).toBe(socketPath(serverData));
      expect(server.info.envId).toMatch(/^[0-9a-z]{26}$/);
      // The keys on disk are the ones this server holds: both rotated before it listened.
      const before = readFileSync(keyFile("local"), "utf8");
      const c = await NdjsonClient.connect(server.info.socket);
      ok(await c.call("server.hello", {}));
      await c.close();
      expect(readFileSync(keyFile("local"), "utf8")).toBe(before);
    } finally {
      await server.stop();
    }
    // A restart rotates them.
    const old = readFileSync(keyFile("local"), "utf8");
    const again = await startServer(serverData);
    try {
      expect(readFileSync(keyFile("local"), "utf8")).not.toBe(old);
    } finally {
      await again.stop();
    }
  });
});

describe.skipIf(!WIN)("private directories on Windows", () => {
  const icacls = (...args: string[]) => {
    const r = spawnSync("icacls", args, { encoding: "utf8" });
    if (r.status !== 0) throw new Error(`icacls ${args.join(" ")}: ${r.stdout}${r.stderr}`);
  };

  test("made_private_dir_passes_check", () => {
    const dir = join(tmp, "private");
    makePrivateDir(dir);
    expect(checkPrivateDir(dir)).toMatchObject({ ok: true, problems: [] });
    // Children inherit it.
    writeFileSync(join(dir, "f"), "x");
    mkdirSync(join(dir, "sub"));
    expect(checkPrivateDir(join(dir, "sub")).ok).toBe(true);
  });

  test("private_dir_refuses_everyone_ace", () => {
    const dir = join(tmp, "open");
    makePrivateDir(dir);
    icacls(dir, "/grant", "*S-1-1-0:(R)");
    const r = checkPrivateDir(dir);
    expect(r.ok).toBe(false);
    expect(r.problems).toContain("allows S-1-1-0");
    icacls(dir, "/grant", "*S-1-5-32-545:(R)");
    expect(checkPrivateDir(dir).problems).toContain("allows S-1-5-32-545");
  });

  test("private_dir_refuses_other_owner", () => {
    // The Windows directory belongs to TrustedInstaller, not this user.
    const r = checkPrivateDir(process.env.SystemRoot ?? "C:\\Windows");
    expect(r.ok).toBe(false);
    expect(r.problems.some((p) => p.startsWith("owner "))).toBe(true);
  });

  test("server_data_unsafe_on_windows", () => {
    const dir = join(tmp, "unsafe");
    makePrivateDir(dir);
    icacls(dir, "/grant", "*S-1-5-11:(R)");
    expect(() => ensureServerData(dir)).toThrow(
      expect.objectContaining({ code: "SERVER_DATA_UNSAFE", fix: expect.stringContaining("only your account") }),
    );
    // A new one is created private.
    const fresh = join(tmp, "fresh");
    ensureServerData(fresh);
    expect(checkPrivateDir(fresh).ok).toBe(true);
  });
});

describe.skipIf(WIN)("private directories elsewhere", () => {
  test("check reports ok; make is mkdir -p and 0700", () => {
    const dir = join(tmp, "a", "b");
    makePrivateDir(dir);
    expect(statSync(dir).mode & 0o777).toBe(0o700);
    expect(checkPrivateDir(dir)).toEqual({ ok: true, problems: [] });
    expect(() => checkPrivateDir(join(tmp, "missing"))).toThrow(expect.objectContaining({ code: "SERVER_DATA_UNSAFE" }));
  });
});

describe.skipIf(process.platform !== "linux")("server data locality (review #4)", () => {
  test("server data that resolves off a local disk is refused before anything is created, unless overridden", () => {
    const link = join(tmp, "elsewhere");
    fs.symlinkSync("/proc/self", link);
    const dir = join(link, "finisher");
    expect(() => ensureServerData(dir)).toThrow(
      expect.objectContaining({
        code: "SERVER_DATA_NOT_LOCAL",
        details: expect.objectContaining({ filesystem: "proc", override: "FIN_ALLOW_NONLOCAL_SERVER_DATA" }),
        fix: expect.stringContaining("FIN_ALLOW_NONLOCAL_SERVER_DATA=1"),
      }),
    );
    process.env.FIN_ALLOW_NONLOCAL_SERVER_DATA = "1";
    try {
      // (Not `ensureServerData`: Node's recursive mkdir never returns on procfs.)
      expect(() => checkServerDataLocal(dir)).not.toThrow();
    } finally {
      delete process.env.FIN_ALLOW_NONLOCAL_SERVER_DATA;
    }
    ensureServerData(join(tmp, "local"));
  });
});
