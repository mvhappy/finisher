// The job host's grants through the ACL broker (plan 7C Decision 18, the Windows side of Task
// 12): each spawn's table is granted before the spawn, its own entries under the process's
// owner revoked at its exit (and when the spawn fails), static entries kept; a job visiting a
// confined warm worker is granted to it for the attempt and revoked before it moves back. A
// recording broker and the scripted host, so every OS runs it; the Windows suites run the real
// broker (`host_sandbox_windows.rs`, `agent-sandbox.test.ts`).
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import type { AclGrant, HostMsg, JobSpawnRequest } from "@finisher/core-types";
import type { AclBrokerGrant } from "@finisher/fin-node";
import { afterEach, describe, expect, test } from "vitest";
import { finError } from "../../src/errors.ts";
import type { AclBroker } from "../../src/sandbox/broker.ts";
import { FakeHost } from "../support/fake-host.ts";
import { closeTestEngines, testEngine, until } from "../support/jobs.ts";

afterEach(async () => {
  await closeTestEngines();
});

const isWarm = (r: JobSpawnRequest) => r.args?.includes("--warm") === true;
const MODIFY = 0x1301bf;
const READ_EXECUTE = 0x1200a9;

/** A broker that records each call (and what exists at the time, through `probe`). */
function recordingBroker(active = true, probe?: (op: "grant" | "revoke", owner: string, path?: string) => void) {
  const ops: string[] = [];
  const held: AclBrokerGrant[] = [];
  const acl: AclBroker = {
    active,
    grant: async (g) => {
      probe?.("grant", g.owner, g.path);
      ops.push(`grant ${g.owner} ${g.path}`);
      held.push(g);
    },
    revoke: async (owner) => {
      probe?.("revoke", owner);
      ops.push(`revoke ${owner}`);
      const n = held.filter((g) => g.owner === owner).length;
      for (let i = held.length - 1; i >= 0; i--) if (held[i]!.owner === owner) held.splice(i, 1);
      return n;
    },
    sweep: async () => 0,
  };
  return { acl, ops, held };
}

/** The scripted `test` worker of `warm-sandbox.test.ts`, answering a table per spawn. */
function host(caps: { perJobGrants: boolean; enforced?: boolean }, ops: string[], table?: (r: JobSpawnRequest) => AclGrant[]) {
  const reqs = new Map<number, JobSpawnRequest>();
  const fake = new FakeHost((h, proc, req) => {
    reqs.set(proc, req);
    h.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "worker", family: "test", kinds: [{ kind: "test.warm", version: 1 }], warm: isWarm(req), pid: 1000 + proc } });
  });
  fake.capsOverride = { profiles: { worker: caps.enforced === false ? "disabled" : "enforced", plugin: "disabled" }, perJobGrants: caps.perJobGrants };
  const spawn = fake.spawn.bind(fake);
  fake.spawn = async (g, r) => {
    ops.push(`spawn ${r.label}`);
    return spawn(g, r);
  };
  if (table !== undefined) Object.assign(fake, { aclGrants: table });
  fake.onSend = (h, proc, msg: HostMsg) => {
    if (msg.t !== "start") return;
    const req = reqs.get(proc)!;
    h.emit({ type: "message", proc, msg: { t: "result", job: msg.job.id, value: { value: null, pid: 1000 + proc, served: 1 } } });
    if (isWarm(req)) h.emit({ type: "message", proc, msg: { t: "ready" } });
    else h.exit(proc, { reason: "exited", code: 0 });
  };
  return fake;
}

/** One owner entry on the request's directory, one static entry on its program. */
const TABLE = (r: JobSpawnRequest): AclGrant[] => [
  { path: r.cwd, trustee: { kind: "sid", value: "S-1-15-3-1" }, mask: MODIFY, scope: "owner" },
  { path: "/bin/fin-workers", trustee: { kind: "sid", value: "S-1-15-3-2" }, mask: READ_EXECUTE, scope: "static" },
];

describe("the job host's grants (plan 7C Decision 18)", () => {
  test("a_spawns_table_is_granted_before_it_and_its_own_entries_revoked_at_its_exit", async () => {
    const { acl, ops, held } = recordingBroker();
    const fake = host({ perJobGrants: false }, ops, TABLE);
    const t = await testEngine({ jobHost: fake.factory, acl });
    const v = await t.runJob("test.warm", { value: 1 }, { project: "A" });
    expect([v.state, v.warm]).toEqual(["succeeded", false]);
    const cold = fake.spawns.find((r) => !isWarm(r))!;
    await until(() => (ops.includes("revoke proc:1") ? true : undefined));
    expect(ops).toEqual([`grant proc:1 ${cold.cwd}`, "grant static /bin/fin-workers", `spawn ${cold.label}`, "revoke proc:1"]);
    // The static entry stays; the process's own is gone.
    expect(held.map((g) => [g.owner, g.path, g.mask, g.inherit])).toEqual([["static", "/bin/fin-workers", READ_EXECUTE, true]]);
    // The next process is another owner.
    await t.runJob("test.warm", { value: 2 }, { project: "A" });
    await until(() => (ops.includes("revoke proc:2") ? true : undefined));
  });

  test("a_table_that_cannot_be_granted_starts_nothing_and_leaves_nothing", async () => {
    const { acl, ops, held } = recordingBroker();
    const fake = host({ perJobGrants: false }, ops, () => {
      throw finError("SANDBOX_UNAVAILABLE", "no grant table for the spawn: x", {});
    });
    const t = await testEngine({ jobHost: fake.factory, acl, testKinds: { overrides: { "test.warm": { maxAttempts: 1 } } } });
    const v = await t.runJob("test.warm", { value: 1 }, { project: "A" });
    expect([v.state, v.error?.code]).toEqual(["failed", "SANDBOX_UNAVAILABLE"]);
    expect(fake.spawns).toEqual([]);
    expect(ops).toEqual(["revoke proc:1"]);
    expect(held).toEqual([]);
  });

  test("a_spawn_that_fails_revokes_what_it_was_granted", async () => {
    const { acl, ops, held } = recordingBroker();
    const fake = host({ perJobGrants: false }, ops, TABLE);
    fake.spawn = async (_g, r) => {
      ops.push(`spawn ${r.label}`);
      throw finError("SANDBOX_UNAVAILABLE", "spawn refused", {});
    };
    const t = await testEngine({ jobHost: fake.factory, acl, testKinds: { overrides: { "test.warm": { maxAttempts: 1 } } } });
    const v = await t.runJob("test.warm", { value: 1 }, { project: "A" });
    expect(v.state).toBe("failed");
    expect(ops.at(-1)).toBe("revoke proc:1");
    expect(held.filter((g) => g.owner !== "static")).toEqual([]);
  });

  test("off_windows_or_without_a_table_nothing_is_granted", async () => {
    for (const [active, table] of [
      [false, TABLE],
      [true, undefined],
    ] as const) {
      const { acl, ops } = recordingBroker(active);
      const fake = host({ perJobGrants: false }, ops, table);
      const t = await testEngine({ jobHost: fake.factory, acl });
      expect((await t.runJob("test.warm", { value: 1 }, { project: "A" })).state).toBe("succeeded");
      expect(ops.filter((o) => !o.startsWith("spawn "))).toEqual([]);
      await closeTestEngines();
    }
  });

  test("a_job_visiting_a_confined_warm_worker_is_granted_to_it_for_the_attempt", async () => {
    // Where each call finds the job's directory: at its visit place for both.
    const seen: string[] = [];
    let root = "";
    const { acl, ops } = recordingBroker(true, (op, owner, path) => {
      if (!owner.startsWith("job:")) return;
      const id = owner.slice(4);
      const warm = readdirSync(root).filter((n) => n.startsWith("warm-"));
      const visiting = warm.some((w) => existsSync(join(root, w, "jobs", id)));
      seen.push(`${op} ${visiting ? "visiting" : "home"}${path === undefined ? "" : ` ${path.endsWith(join("jobs", id)) ? "jobDir" : path}`}`);
    });
    const fake = host({ perJobGrants: true }, ops, TABLE);
    const t = await testEngine({ jobHost: fake.factory, acl });
    root = join(t.serverData, "jobs");
    const v = await t.runJob("test.warm", { value: 1 }, { project: "A" });
    expect([v.state, v.warm]).toEqual(["succeeded", true]);
    const w = fake.spawns.find(isWarm)!;
    expect(ops).toContain(`grant job:${v.id} ${join(w.cwd, "jobs", v.id)}`);
    expect(seen).toEqual(["grant visiting jobDir", "revoke visiting"]);
    // The worker's own grants stay while it is in the pool.
    expect(ops.filter((o) => o.startsWith("revoke proc:"))).toEqual([]);
  });

  test("an_unconfined_warm_worker_gets_no_visit_grant", async () => {
    const { acl, ops } = recordingBroker();
    const fake = host({ perJobGrants: false, enforced: false }, ops, TABLE);
    const t = await testEngine({ jobHost: fake.factory, acl });
    const v = await t.runJob("test.warm", { value: 1 }, { project: "A" });
    expect([v.state, v.warm]).toEqual(["succeeded", true]);
    expect(ops.filter((o) => o.includes("job:"))).toEqual([]);
  });
});
