// The ACL broker (plan 7C Task 10, Decision 18) from finisherd's side. fin-node's broker runs
// its log on every OS (off Windows it applies nothing), so the log, the crash points (a Node
// child aborted by the `test-hooks` hook `FIN_TEST_ACL_CRASH`), the sweep and the protected
// roots are tested here through the napi surface; `aclBroker` itself is a no-op off Windows.
// The session wiring (`openSessionSandbox` on Windows) runs against a fake broker: the agent
// table granted before any process, the static entries under the static owner, the project's
// container SID allowed on the pipe once, the session's grants revoked at scope close.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import type { Session } from "@finisher/contracts";
import type { AclGrant } from "@finisher/core-types";
import { type AclBrokerGrant, aclGrant, aclGrantsForTest, aclRevoke, aclSweep, sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { openSessionSandbox } from "../src/agent/sandbox.ts";
import { makePipeSocketServer } from "../src/pipe.ts";
import { type AclBroker, aclBroker, grantTable, STATIC_OWNER, sessionOwner } from "../src/sandbox/broker.ts";

const MODIFY = 0x1301bf;
const READ = 0x120089;
const SID = "S-1-15-3-1024-1-2-3-4-5-6-7-8";
const FIN_NODE = pathToFileURL(resolve(import.meta.dirname, "../../../packages/fin-node/index.js")).href;

let tmp: string;
let data: string;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "acl-broker-")));
  data = join(tmp, "data");
  for (const d of ["run", "auth", "agents/s1/tmp", "agents/s1/settings", "agents/s1/blobs"]) mkdirSync(join(data, d), { recursive: true });
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const wal = () => (existsSync(join(data, "run", "acl-broker.wal")) ? readFileSync(join(data, "run", "acl-broker.wal"), "utf8") : "");
const tmpGrant = (owner = "session:s1"): AclBrokerGrant => ({ path: join(data, "agents", "s1", "tmp"), sid: SID, mask: MODIFY, inherit: true, owner });

describe("aclBroker", () => {
  test("is_a_no_op_off_windows", async () => {
    const calls: string[] = [];
    const native = {
      aclGrant: async () => void calls.push("grant"),
      aclRevoke: async () => (calls.push("revoke"), 0),
      aclSweep: async () => (calls.push("sweep"), 0),
    };
    const off = aclBroker(data, { platform: "linux", native });
    expect(off.active).toBe(false);
    await off.grant(tmpGrant());
    expect(await off.revoke("session:s1")).toBe(0);
    expect(await off.sweep([])).toBe(0);
    expect(calls).toEqual([]);
    const on = aclBroker(data, { platform: "win32", native });
    await on.grant(tmpGrant());
    await on.revoke("session:s1");
    await on.sweep([]);
    expect(calls).toEqual(["grant", "revoke", "sweep"]);
  });
});

describe("fin-node's broker (its log runs on every OS)", () => {
  test("grants_are_logged_revoked_and_swept", async () => {
    await aclGrant(data, tmpGrant("session:a"));
    await aclGrant(data, { ...tmpGrant(STATIC_OWNER), path: join(data, "agents", "s1", "settings"), mask: READ });
    expect(aclGrantsForTest(data).map((g) => g.owner).sort()).toEqual(["session:a", STATIC_OWNER]);
    expect(wal().trim().split("\n")).toHaveLength(4);
    expect(await aclSweep(data, ["session:a"])).toBe(0);
    expect(await aclSweep(data, [])).toBe(1);
    expect(aclGrantsForTest(data).map((g) => g.owner)).toEqual([STATIC_OWNER]);
    expect(await aclRevoke(data, STATIC_OWNER)).toBe(1);
    expect(aclGrantsForTest(data)).toEqual([]);
  });

  test("a_protected_root_is_refused_before_the_log", async () => {
    await aclSweep(data, []);
    const before = wal();
    for (const [path, mask] of [
      [data, READ],
      [join(data, "auth"), READ],
      [join(data, "agents", "s1"), READ],
      [join(data, "agents", "s1", "blobs"), READ],
      [join(data, "agents", "s1", "settings"), MODIFY],
    ] as const) {
      await expect(aclGrant(data, { ...tmpGrant(), path, mask }), path).rejects.toMatchObject({ code: "PATH_NOT_ALLOWED", details: { reason: "aclProtected" } });
    }
    await expect(aclGrant(data, { ...tmpGrant(), path: join(data, "agents", "s1", "tmp", "missing") })).rejects.toMatchObject({ code: "NOT_FOUND" });
    await expect(aclGrant(data, { ...tmpGrant(), mask: MODIFY | 0x40000 })).rejects.toMatchObject({ code: "INVALID_ARGS" });
    expect(wal()).toBe(before);
  });

  /** Runs one broker call in a Node child that the test hook aborts at `point`. */
  function crashChild(op: "grant" | "revoke", point: "afterAppend" | "afterApply"): void {
    const call = op === "grant" ? `m.aclGrant(${JSON.stringify(data)}, ${JSON.stringify(tmpGrant())})` : `m.aclRevoke(${JSON.stringify(data)}, "session:s1")`;
    const r = spawnSync(process.execPath, ["--input-type=module", "-e", `const m = await import(${JSON.stringify(FIN_NODE)}); await ${call}; console.log("CHILD_DONE");`], {
      env: { ...process.env, FIN_TEST_ACL_CRASH: `${op}:${point}` },
      encoding: "utf8",
      timeout: 30_000,
    });
    expect(r.stdout, r.stderr).not.toContain("CHILD_DONE");
    expect(r.status === 0, `${op}:${point} aborted the child`).toBe(false);
  }

  const pendingOps = () => {
    const lines = wal().trim().split("\n").filter(Boolean);
    return lines.filter((l) => !l.includes('"done"')).length - lines.filter((l) => l.includes('"done"')).length;
  };

  test.each(["afterAppend", "afterApply"] as const)("a_crash_in_a_grant_%s_is_completed_at_the_next_start", async (point) => {
    crashChild("grant", point);
    expect(pendingOps()).toBe(1);
    // The next start (no owner live) revokes it; the log is left with nothing pending.
    expect(await aclSweep(data, [])).toBe(0);
    expect(aclGrantsForTest(data)).toEqual([]);
    expect(pendingOps()).toBe(0);
  });

  test("a_crashed_grant_of_a_live_owner_is_completed", async () => {
    crashChild("grant", "afterApply");
    await aclSweep(data, ["session:s1"]);
    expect(aclGrantsForTest(data).map((g) => g.owner)).toEqual(["session:s1"]);
    expect(pendingOps()).toBe(0);
  });

  test.each(["afterAppend", "afterApply"] as const)("a_crash_in_a_revoke_%s_is_completed_at_the_next_start", async (point) => {
    // Granted by a child that ends normally, then revoked by one aborted at `point`.
    const ok = spawnSync(process.execPath, ["--input-type=module", "-e", `const m = await import(${JSON.stringify(FIN_NODE)}); await m.aclGrant(${JSON.stringify(data)}, ${JSON.stringify(tmpGrant())});`], { encoding: "utf8" });
    expect(ok.status, ok.stderr).toBe(0);
    crashChild("revoke", point);
    expect(pendingOps()).toBe(1);
    // Completed even with the owner live: the revoke was asked for.
    await aclSweep(data, ["session:s1"]);
    expect(aclGrantsForTest(data)).toEqual([]);
    expect(pendingOps()).toBe(0);
  });
});

/** A broker that records what it is asked. */
function fakeBroker(): AclBroker & { grants: AclBrokerGrant[]; revoked: string[] } {
  const grants: AclBrokerGrant[] = [];
  const revoked: string[] = [];
  return {
    active: true,
    grants,
    revoked,
    grant: async (g) => {
      if (!existsSync(g.path)) throw Object.assign(new Error("missing"), { code: "NOT_FOUND" });
      grants.push(g);
    },
    revoke: async (owner) => (revoked.push(owner), 0),
    sweep: async () => 0,
  };
}

describe("grantTable", () => {
  test("static_entries_are_kept_owner_entries_are_the_sessions_and_containers_resolve_once", async () => {
    const b = fakeBroker();
    const tmpDir = join(data, "agents", "s1", "tmp");
    const table: AclGrant[] = [
      { path: tmp, trustee: { kind: "container", value: "finisher-p-x" }, mask: 0x1200a9, scope: "static" },
      { path: join(tmp, "data"), trustee: { kind: "container", value: "finisher-p-x" }, mask: 0x1200a9, scope: "static" },
      { path: tmpDir, trustee: { kind: "sid", value: SID }, mask: MODIFY, scope: "owner" },
      { path: join(data, "agents", "s1", "credential.json"), trustee: { kind: "sid", value: SID }, mask: READ, scope: "owner" },
    ];
    const resolved: string[] = [];
    const allowed: string[] = [];
    await grantTable(b, table, {
      owner: "session:s1",
      containerSid: (n) => (resolved.push(n), "S-1-15-2-9"),
      onContainer: (s) => allowed.push(s),
    });
    expect(b.grants).toEqual([
      { path: tmp, sid: "S-1-15-2-9", mask: 0x1200a9, inherit: true, owner: STATIC_OWNER },
      { path: join(tmp, "data"), sid: "S-1-15-2-9", mask: 0x1200a9, inherit: true, owner: STATIC_OWNER },
      { path: tmpDir, sid: SID, mask: MODIFY, inherit: true, owner: "session:s1" },
    ]);
    expect(resolved).toEqual(["finisher-p-x"]);
    expect(allowed).toEqual(["S-1-15-2-9"]);
    // Any other failure is the caller's.
    const failing = { ...fakeBroker(), grant: async () => Promise.reject(Object.assign(new Error("x"), { code: "PATH_NOT_ALLOWED" })) };
    await expect(grantTable(failing, table.slice(2), { owner: "session:s1" })).rejects.toMatchObject({ code: "PATH_NOT_ALLOWED" });
    // An inactive broker grants nothing.
    const off = aclBroker(data, { platform: "linux" });
    await grantTable(off, table, { owner: "session:s1", containerSid: () => null });
  });
});

describe("the session sandbox on Windows", () => {
  test("grants_the_agent_table_before_any_process_and_revokes_it_at_scope_close", async () => {
    const dir = join(tmp, "P");
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "finisher.project.json"), "{}");
    const s = { id: "01S0000000000000000000000A", projectId: "01P0000000000000000000000A", harness: "fake", mode: "draft" } as unknown as Session;
    const b = fakeBroker();
    const allowed: string[] = [];
    const scope = await Effect.runPromise(Scope.make());
    const box = await Effect.runPromise(
      Scope.provide(scope)(
        openSessionSandbox(s, "fake", {
          serverData: data,
          projectDirOf: () => dir,
          finPath: "/bin/sh",
          localEndpoint: "\\\\.\\pipe\\finisher-test",
          status: () => ({ ...sandboxStatus(), agent: { kind: "enforced", detail: "test" } }),
          launcher: () => "/nonexistent/fin-sandbox-exec",
          platform: "win32",
          acl: b,
          allowPipe: (sid) => allowed.push(sid),
          containerSid: (name) => (name.startsWith("finisher-p-") ? "S-1-15-2-7" : null),
        }),
      ),
    );
    expect(box.mode).toBe("launcher");
    const owner = sessionOwner(s.id);
    const by = (p: string) => b.grants.filter((g) => g.path === p);
    const session = join(data, "agents", s.id);
    // The masks follow the policy: write roots modify, settings and git read, blobs nothing.
    expect(by(join(session, "tmp"))).toMatchObject([{ mask: MODIFY, owner }]);
    expect(by(join(session, "settings"))).toMatchObject([{ mask: READ, owner }]);
    expect(by(join(session, "git"))).toMatchObject([{ mask: READ, owner }]);
    expect(by(join(session, "blobs"))).toEqual([]);
    expect(by(session)).toEqual([]);
    expect(by(join(dir, "workspace", s.id))).toMatchObject([{ mask: MODIFY, owner }]);
    // The project folder: static (granted once; its trustee is the table's, a container or a per-view capability).
    expect(by(dir)).toMatchObject([{ owner: STATIC_OWNER }]);
    expect(b.grants.every((g) => g.owner === owner || g.owner === STATIC_OWNER)).toBe(true);
    expect(allowed).toEqual(["S-1-15-2-7"]);
    expect(b.revoked).toEqual([]);
    await Effect.runPromise(Scope.close(scope, Exit.void));
    expect(b.revoked).toEqual([owner]);
  });
});

describe("the pipe's allow", () => {
  test("on_allow_hands_over_the_listeners_allow", async () => {
    const allowed: string[] = [];
    const fake = { onEvent: () => {}, read: async () => null, write: async () => {}, close: () => {}, shutdown: () => {}, allow: (sid: string) => void allowed.push(sid) };
    let allow: ((sid: string) => void) | undefined;
    await Effect.runPromise(Effect.scoped(makePipeSocketServer("\\\\.\\pipe\\t", { native: () => fake, onAllow: (a) => (allow = a) })));
    allow!("S-1-15-2-5");
    expect(allowed).toEqual(["S-1-15-2-5"]);
  });
});
