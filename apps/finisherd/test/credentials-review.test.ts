// The review of 5A Tasks 8 and 9: the credential file is private before it is minted and the
// credential is taken back when it cannot be written (M2); `credential.master` is bound to the
// environment, kept out of Time Machine and can be replaced (M3); changes outside `Store::write`
// check the credential (L1); a revocation survives a busy store (L2); an ended credential is
// refused by name after the server proved itself (L4); the owner's purge keeps its own trace.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Credential, CredentialFile, FinError } from "@finisher/contracts";
import { createPrivateFile, endCredential, type PrivateFile } from "@finisher/fin-node";
import { Effect, Exit, Fiber, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Credentials } from "../src/auth/credentials.ts";
import { b64u, canonicalEndpoint, helloProof, proveMac, sameMac } from "../src/auth/crypto.ts";
import { BACKUP_EXCLUDE_HEX, excludeFromBackups } from "../src/auth/keys.ts";
import { revokeCredential, sweepExpired } from "../src/auth/revoke.ts";
import { Tickets } from "../src/auth/tickets.ts";
import { ConnectionRegistry } from "../src/connections.ts";
import { describeMint } from "../src/console/commands/agent.ts";
import { runConsole } from "../src/console/main.ts";
import { person } from "./support/person.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import type { CallMeta } from "../src/diagnostics.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { CONSOLE } from "../src/principal.ts";
import { makeProjectRegistry, type ProjectRegistryShape } from "../src/projects.ts";
import { confirmed, connectProved, type Exit as Answer, hello, NdjsonClient, ok, type ServerInfo } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-cred-review-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  for (const c of clients.splice(0)) await c.close();
  await stopServer();
  rmSync(tmp, { recursive: true, force: true });
});

const code = (e: Answer) => (e.ok ? undefined : e.error.code);

// --- A server in this process, with a clock the test moves ---------------------------------

let running: { fiber: Fiber.Fiber<unknown, unknown>; info: ServerInfo } | undefined;
const clients: NdjsonClient[] = [];
let now = Date.now();

async function startInProcess(): Promise<ServerInfo> {
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  const fiber = Effect.runFork(
    serve({ serverData, credentials: { clock: () => now, sweepMs: 20 }, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)),
  );
  running = { fiber, info: await info };
  return running.info;
}

async function stopServer(): Promise<void> {
  if (running === undefined) return;
  await Effect.runPromise(Fiber.interrupt(running.fiber));
  running = undefined;
}

async function asConsole(): Promise<NdjsonClient> {
  const c = await connectProved({ info: running!.info, serverData }, "console");
  clients.push(c);
  return c;
}

async function mint(req: Record<string, unknown> = {}): Promise<{ credential: Credential; file: CredentialFile }> {
  return ok(await confirmed(await asConsole(), "auth.credential.mint", { kind: "agent", label: "test", ...req }));
}

/** Hello and prove as `file` does: whether the server proved itself, and the prove's exit. */
async function proveAs(file: CredentialFile): Promise<{ client: NdjsonClient; proved: boolean; exit: Answer }> {
  const info = running!.info;
  const client = await NdjsonClient.connect(info.socket);
  clients.push(client);
  const key = b64u.decode(file.key)!;
  const endpoint = canonicalEndpoint(info.socket);
  const { clientNonce, exit } = await hello(client, { env: file.env, endpoint, credential: { kind: file.kind, id: file.id } });
  const answer = ok<{ envId: string; endpoint: string; serverNonce: string; proof?: string }>(exit);
  const serverNonce = b64u.decode(answer.serverNonce)!;
  const proof = answer.proof === undefined ? null : b64u.decode(answer.proof);
  const proved = proof !== null && sameMac(proof, helloProof(key, answer.envId, answer.endpoint, clientNonce, serverNonce));
  const mac = proveMac(key, answer.envId, answer.endpoint, serverNonce, clientNonce);
  return { client, proved, exit: await client.call("auth.prove", { mac: b64u.encode(mac) }) };
}

async function consoleRun(argv: string[], more: { createFile?: (path: string) => PrivateFile } = {}) {
  let out = "";
  let err = "";
  const exit = await runConsole([...argv, "--server-data", serverData], {
    prompt: person(),
    output: { out: (t) => void (out += `${t}\n`), err: (t) => void (err += `${t}\n`) },
    ...more,
  });
  return { exit, out, err };
}

// --- M2: the --out file ----------------------------------------------------------------------

describe("M2: the credential file", () => {
  test("createPrivateFile: new, owner-only from creation, written whole, removed on discard", () => {
    const path = join(tmp, "c.json");
    const f = createPrivateFile(path);
    expect(f.path).toBe(path);
    if (process.platform !== "win32") expect(statSync(path).mode & 0o777).toBe(0o600);
    // An existing path, or a symlink planted there, is refused and left alone.
    expect(() => createPrivateFile(path)).toThrow(expect.objectContaining({ code: "ALREADY_EXISTS" }));
    if (process.platform !== "win32") {
      const link = join(tmp, "link.json");
      symlinkSync(join(tmp, "target.json"), link);
      expect(() => createPrivateFile(link)).toThrow(expect.objectContaining({ code: "ALREADY_EXISTS" }));
      expect(existsSync(join(tmp, "target.json"))).toBe(false);
    }
    f.commit('{"v":1}\n');
    expect(readFileSync(path, "utf8")).toBe('{"v":1}\n');
    f.discard();
    expect(existsSync(path)).toBe(true);
    const g = createPrivateFile(join(tmp, "d.json"));
    g.discard();
    g.discard();
    expect(existsSync(join(tmp, "d.json"))).toBe(false);
  });

  test("a file that would not be private is refused before anything is minted", { timeout: 30_000 }, async () => {
    await startInProcess();
    const path = join(tmp, "unsafe.json");
    const unsafe = () => {
      throw Object.assign(new Error("unsafe"), { code: "PRIVATE_FILE_UNSAFE", details: { path, problems: ["mode 0644"] } });
    };
    const r = await consoleRun(["agent", "mint", "--label", "x", "--out", path], { createFile: unsafe });
    expect(r.exit).toBe(2);
    expect(r.err).toMatch(/would not be private \(mode 0644\)/);
    expect(ok<{ credentials: unknown[] }>(await (await asConsole()).call("auth.credential.list", { all: true })).credentials).toEqual([]);
  });

  test("a file that cannot be written: the credential is revoked on the same connection and the file removed", { timeout: 30_000 }, async () => {
    await startInProcess();
    const path = join(tmp, "full.json");
    let real: PrivateFile | undefined;
    const failing = (p: string): PrivateFile => {
      real = createPrivateFile(p);
      const inner = real;
      return {
        get path() {
          return inner.path;
        },
        commit: () => {
          throw Object.assign(new Error("No space left on device"), { code: "IO_ERROR" });
        },
        discard: () => inner.discard(),
      } as unknown as PrivateFile;
    };
    const r = await consoleRun(["agent", "mint", "--label", "full", "--out", path], { createFile: failing });
    expect(r.exit, r.err).toBe(6);
    expect(r.err).toMatch(/was revoked/);
    expect(existsSync(path)).toBe(false);
    const listed = ok<{ credentials: Credential[] }>(await (await asConsole()).call("auth.credential.list", { all: true })).credentials;
    expect(listed).toHaveLength(1);
    expect(listed[0]!.status).toBe("revoked");
    // Only the minting connection may take a credential back without a code.
    const other = await asConsole();
    const fresh = await mint();
    expect(code(await other.call("auth.credential.revoke", { id: fresh.credential.id }))).toBe("CONFIRMATION_REQUIRED");
  });

  test("L6: the mint's confirmation says plainly that no --project is every project and any path", () => {
    expect(describeMint("agent", "x", [], [], [])).toMatch(/ALL projects \(\*\): every project on this server, and any folder path/);
    expect(describeMint("agent", "x", [], ["01aaaaaaaaaaaaaaaaaaaaaaaa"], ["own"])).toMatch(/the projects 01aaaaaaaaaaaaaaaaaaaaaaaa and the branches own/);
  });
});

// --- M3: credential.master -------------------------------------------------------------------

describe("M3: credential.master", () => {
  test("auth/ is excluded from Time Machine on macOS only, and a failure is only a warning", () => {
    const calls: Array<[string, ReadonlyArray<string>]> = [];
    const run = (status: number | null) => (c: string, a: ReadonlyArray<string>) => (calls.push([c, a]), status);
    expect(excludeFromBackups("/sd/auth", "darwin", run(0))).toBe(true);
    expect(calls).toEqual([["/usr/bin/xattr", ["-wx", "com.apple.metadata:com_apple_backup_excludeItem", BACKUP_EXCLUDE_HEX, "/sd/auth"]]]);
    expect(excludeFromBackups("/sd/auth", "darwin", run(1))).toBe(false);
    expect(excludeFromBackups("/sd/auth", "linux", run(0))).toBe(false);
    expect(excludeFromBackups("/sd/auth", "win32", run(0))).toBe(false);
    expect(calls).toHaveLength(2);
  });

  test("the real exclusion is quick and Time Machine reads it as excluded", { skip: process.platform !== "darwin" }, () => {
    // `tmutil addexclusion` takes 11 s on some hosts (it waits on backupd), and the start waited
    // for it; the sticky exclusion is the attribute it writes, so the start writes that itself.
    const dir = join(tmp, "auth");
    mkdirSync(dir);
    const t0 = Date.now();
    expect(excludeFromBackups(dir)).toBe(true);
    expect(Date.now() - t0).toBeLessThan(2000);
    const check = spawnSync("/usr/bin/tmutil", ["isexcluded", dir], { encoding: "utf8" });
    expect(check.stdout).toMatch(/^\[Excluded\]/);
  });

  test("auth rotate-master needs a terminal, ends every credential, and old files never prove again", { timeout: 30_000 }, async () => {
    await startInProcess();
    const a = await mint();
    const b = await mint({ kind: "mcp" });
    const before = readFileSync(join(serverData, "auth", "credential.master"), "utf8");
    let err = "";
    const refused = await runConsole(["auth", "rotate-master", "--server-data", serverData], {
      prompt: { isInteractive: () => false, show: () => undefined, ask: async () => "" },
      output: { out: () => undefined, err: (t) => void (err += t) },
    });
    expect([refused, err]).toEqual([7, expect.stringMatching(/CONSOLE_NEEDS_TTY/)]);
    const r = await consoleRun(["auth", "rotate-master"]);
    expect(r.exit, r.err).toBe(0);
    expect(r.out).toMatch(/2 credential\(s\) ended/);
    expect(readFileSync(join(serverData, "auth", "credential.master"), "utf8")).not.toBe(before);
    for (const file of [a.file, b.file]) {
      const p = await proveAs(file);
      // Its row has ended, but the key it holds is not the one the new master derives.
      expect(p.proved, file.kind).toBe(false);
      expect(code(p.exit)).toBe("UNAUTHENTICATED");
    }
    const audit = ok<{ entries: Array<{ details: Record<string, unknown> }> }>(
      await (await asConsole()).call("auth.audit.list", { kind: "credential.rotateMaster" }),
    ).entries;
    expect(audit).toHaveLength(1);
    expect(audit[0]!.details).toMatchObject({ count: 2 });
    // New credentials work with the new master, also after a restart.
    const c = await mint();
    await stopServer();
    await startInProcess();
    const p = await proveAs(c.file);
    expect([p.proved, code(p.exit)]).toEqual([true, undefined]);
  });
});

// --- L4: ended credentials are refused by name -----------------------------------------------

describe("L4: an ended credential", () => {
  test("revoked and expired ones get the server's proof, then CREDENTIAL_REVOKED or CREDENTIAL_EXPIRED; unknown ones no proof", { timeout: 30_000 }, async () => {
    now = Date.now();
    await startInProcess();
    const revoked = await mint();
    ok(await confirmed(await asConsole(), "auth.credential.revoke", { id: revoked.credential.id }));
    const r = await proveAs(revoked.file);
    expect([r.proved, code(r.exit)]).toEqual([true, "CREDENTIAL_REVOKED"]);
    const expiring = await mint({ idleS: 60 });
    now += 61_000;
    const e = await proveAs(expiring.file);
    expect([e.proved, code(e.exit)]).toEqual([true, "CREDENTIAL_EXPIRED"]);
    expect(e.exit.ok ? undefined : e.exit.error.fix).toMatch(/Ask the owner of this Finisher server for a new one/);
    const unknown = await proveAs({ ...revoked.file, id: "01zzzzzzzzzzzzzzzzzzzzzzzz" });
    expect([unknown.proved, code(unknown.exit)]).toEqual([false, "UNAUTHENTICATED"]);
    // Each is an `auth.fail` (aggregated per credential kind per minute, Task 5 M5).
    const fails = ok<{ entries: Array<{ code?: string; count: number }> }>(await (await asConsole()).call("auth.audit.list", { kind: "auth.fail" })).entries;
    expect(fails.reduce((n, f) => n + f.count, 0)).toBe(3);
    expect(fails[fails.length - 1]!.code).toBe("CREDENTIAL_REVOKED");
  });
});

// --- L2: a busy store never stops a revocation ----------------------------------------------

describe("L2: revocation and a failing store", () => {
  let store: ControlStore;
  afterEach(async () => {
    await store?.close();
  });

  test("the credential ends in memory at once; the write is retried until it lands", async () => {
    store = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
    const audit = new Audit(store);
    const creds = new Credentials(store, Buffer.alloc(32, 7), audit);
    const { credential } = creds.mint(CONSOLE, { kind: "agent", label: "a" });
    const ref = { kind: "agent" as const, id: credential.id };
    // A writer holding the store: every write of ours is busy.
    const other = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
    other.db.exec("BEGIN IMMEDIATE");
    const tickets = new Tickets();
    tickets.mint({ kind: "agent", id: credential.id, scopes: [], projects: "*", branches: "*", human: false }, 30);
    const deps = { audit, tickets, connections: new ConnectionRegistry(), revoke: (r: { kind: "agent" | "mcp"; id: string }) => creds.markEnded(r, "owner"), persist: () => creds.persist() };
    const report = revokeCredential(deps, ref, { reason: "owner" });
    expect(report).toMatchObject({ revoked: true, tickets: 1 });
    expect(creds.isLive(ref)).toBe(false);
    expect(creds.unsaved()).toBe(1);
    other.db.exec("COMMIT");
    await other.close();
    // The retry (after about a second) writes it.
    for (let i = 0; i < 60 && creds.unsaved() > 0; i++) await new Promise((r) => setTimeout(r, 50));
    expect(creds.unsaved()).toBe(0);
    creds.dispose();
    expect(new Credentials(store, Buffer.alloc(32, 7), audit).isLive(ref)).toBe(false);
  });

  test("sweepExpired goes on past a credential it cannot end", () => {
    const ended: string[] = [];
    const deps = {
      audit: { append: () => undefined } as unknown as Audit,
      tickets: new Tickets(),
      connections: new ConnectionRegistry(),
      revoke: (r: { id: string }) => {
        if (r.id === "bad") throw new Error("store busy");
        ended.push(r.id);
        return true;
      },
    };
    const refs = [{ kind: "agent" as const, id: "bad" }, { kind: "agent" as const, id: "good" }];
    expect(sweepExpired(deps, () => refs)).toBe(1);
    expect(ended).toEqual(["good"]);
  });
});

// --- L1: changes outside Store::write check the credential ----------------------------------

describe("L1: acknowledgeRewind, acknowledgeSalvage and setCollect", () => {
  let scope: Scope.Closeable | undefined;
  let r: ProjectRegistryShape;
  afterEach(async () => {
    if (scope) await Effect.runPromise(Scope.close(scope, Exit.void));
    scope = undefined;
  });

  test("refuse a revoked credential inside the store, like a write", async () => {
    scope = Effect.runSync(Scope.make());
    r = await Effect.runPromise(Scope.provide(scope)(makeProjectRegistry(join(tmp, "sd"))));
    const projectId = (await Effect.runPromise(r.create(join(tmp, "p"), "Demo", "30"))).projectId;
    const live = "agent:01liveliveliveliveliveliv";
    const gone = "agent:01gonegonegonegonegonego";
    endCredential(gone);
    const meta = (credential: string): CallMeta => ({ actor: { kind: "agent" }, guard: { denyBranches: [], credential } });
    const failure = (e: Effect.Effect<unknown, FinError>) => Effect.runPromise(Effect.flip(e));
    expect(await Effect.runPromise(r.acknowledgeSalvage(projectId, "none", meta(live)))).toBe(false);
    expect(await failure(r.acknowledgeSalvage(projectId, "none", meta(gone)))).toMatchObject({ code: "CREDENTIAL_REVOKED" });
    expect(await Effect.runPromise(r.acknowledgeRewind(projectId, "main", 1, meta(live)))).toBe(false);
    expect(await failure(r.acknowledgeRewind(projectId, "main", 1, meta(gone)))).toMatchObject({ code: "CREDENTIAL_REVOKED" });
    const engine = r.engineOf(projectId)!;
    await expect(engine.setCollect("never", JSON.stringify(meta(gone)))).rejects.toMatchObject({ code: "CREDENTIAL_REVOKED" });
    expect(JSON.parse(await engine.setCollect("never", JSON.stringify(meta(live))))).toMatchObject({ collect: "never" });
  });
});

// --- Task 8: the owner's audit purge ---------------------------------------------------------

describe("Task 8: auth.audit.purge", () => {
  let store: ControlStore;
  afterEach(async () => {
    await store?.close();
  });

  test("never purges audit.purge rows, records the cutoff, and is one transaction", async () => {
    store = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
    let at = 1_000;
    const audit = new Audit(store, () => at);
    const by = { kind: "console", id: "console" };
    for (let i = 0; i < 5; i++) audit.append({ kind: "auth.ticket", principal: by, outcome: "ok", details: {} });
    at = 2_000;
    expect(audit.ownerPurge({ before: 1_500 }, by, "socket")).toBe(5);
    at = 3_000;
    audit.append({ kind: "auth.ticket", principal: by, outcome: "ok", details: {} });
    expect(audit.ownerPurge({ all: true }, by, "socket")).toBe(1);
    const rows = audit.list().entries;
    expect(rows.map((e) => e.kind)).toEqual(["audit.purge", "audit.purge"]);
    expect(rows[1]!.details).toEqual({ reason: "before", before: 1_500, count: 5, transport: "socket" });
    expect(rows[0]!.details).toEqual({ reason: "all", count: 1, transport: "socket" });
    // A busy store changes nothing: neither the delete nor its record.
    audit.append({ kind: "auth.ticket", principal: by, outcome: "ok", details: {} });
    const other = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
    other.db.exec("BEGIN IMMEDIATE");
    expect(() => audit.ownerPurge({ all: true }, by, "socket")).toThrow(expect.objectContaining({ code: "CONTROL_DB_BUSY" }));
    other.db.exec("COMMIT");
    await other.close();
    expect(audit.list().entries.map((e) => e.kind)).toEqual(["auth.ticket", "audit.purge", "audit.purge"]);
  });
});

test("the credential file written by the console parses as a credential file", () => {
  // A guard for the M2 rewrite: what `commit` writes is `JSON.stringify(file)` and a newline.
  const path = join(tmp, "f.json");
  const f = createPrivateFile(path);
  const file: CredentialFile = { v: 1, kind: "agent", id: "01aaaaaaaaaaaaaaaaaaaaaaaa", key: "k", env: "e" };
  f.commit(`${JSON.stringify(file)}\n`);
  expect(JSON.parse(readFileSync(path, "utf8"))).toEqual(file);
});
