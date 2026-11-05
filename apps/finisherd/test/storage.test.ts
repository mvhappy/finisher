// Storage RPCs (plan 4A Task 15): status, takeover, verify, rebuild, compact, recover,
// acknowledge, tx.get, project.events, generations on commit, and the engine events finisherd
// routes (imports to ops.subscribe, rewinds and rebuilds ending it, ENGINE_BUSY passed through).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { CommittedTx } from "@finisher/core-types";
import { Effect, Exit, Fiber, PubSub, Scope, Stream } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Hubs } from "../src/hubs.ts";
import { makeProjectRegistry, type ProjectRegistryShape, type RegistryOptions } from "../src/projects.ts";
import { addAsset, NdjsonClient, ok, type RunningServer, startServer, tx } from "./support/ndjson-client.ts";

let tmp: string;
const servers: RunningServer[] = [];
const clients: NdjsonClient[] = [];
let scope: Scope.Closeable | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-storage-"));
});

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  for (const s of servers.splice(0)) await s.stop();
  if (scope) await Effect.runPromise(Scope.close(scope, Exit.void));
  scope = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function server(name = "sd"): Promise<NdjsonClient> {
  const s = await startServer(join(tmp, name));
  servers.push(s);
  const c = await NdjsonClient.connect(s.info.socket);
  clients.push(c);
  return c;
}

async function until<T>(f: () => T | undefined, ms: number, what: string): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

type Status = { lease: { state: string }; readOnly: { reason: string } | null; busy: unknown; branches: { generation: number }[] };

describe("storage RPCs", () => {
  test("status/takeover/verify RPCs", async () => {
    const c = await server();
    const dir = join(tmp, "Мій проект");
    const { projectId } = ok<{ projectId: string }>(await c.call("project.create", { dir, name: "Demo", rate: "30" }));
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]) }));

    const status = ok<Status>(await c.call("project.status", { projectId }));
    expect(status.lease.state).toBe("held");
    expect(status.readOnly).toBeNull();
    expect(status.busy).toBeNull();
    expect(ok<Status>(await c.call("project.takeover", { projectId, confirm: false })).lease.state).toBe("held");
    const report = ok<{ ok: boolean; checks: unknown[] }>(await c.call("project.verify", { projectId }));
    expect(report.ok).toBe(true);
    expect(report.checks.length).toBeGreaterThan(0);
    expect(ok<{ skipped: unknown }>(await c.call("project.compact", { projectId }))).toHaveProperty("skipped");
    expect(ok<{ manual: string }>(await c.call("project.recover", { projectId })).manual).toContain("CHAIN_FORK");
    expect(ok(await c.call("project.acknowledgeRewind", { projectId, branch: "main", generation: 1 }))).toEqual({ acknowledged: false });
    expect(ok(await c.call("project.acknowledgeSalvage", { projectId, copy: "nope" }))).toEqual({ acknowledged: false });

    const got = ok<{ rev: number; tx: { txId: string } }>(await c.call("tx.get", { projectId, txId: "tx-1" }));
    expect([got.rev, got.tx.txId]).toEqual([1, "tx-1"]);
    expect(ok(await c.call("tx.get", { projectId, txId: "tx-unknown" }))).toBeNull();

    const missing = await c.call("project.status", { projectId: "nope" });
    expect(missing.ok ? undefined : missing.error.code).toBe("PROJECT_NOT_FOUND");
  });

  test("a commit sent with a stale generation is refused with HISTORY_REWOUND", async () => {
    const c = await server();
    const { projectId } = ok<{ projectId: string }>(await c.call("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" }));
    const snap = ok<{ rev: number; generation: { incarnation: string; n: number } }>(
      await c.call("snapshot.get", { projectId, branch: "main" }),
    );
    expect(snap.generation).toEqual({ incarnation: expect.any(String), n: 0 });
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]), generation: snap.generation }));
    const stale = { incarnation: "an-older-database", n: 0 };
    const refused = await c.call("tx.commit", { projectId, tx: tx("tx-2", 1, [addAsset("$b")]), generation: stale });
    expect(refused.ok ? undefined : refused.error.code).toBe("HISTORY_REWOUND");
    // After a rebuild, the generation read before it is stale too.
    ok(await c.call("project.rebuild", { projectId }));
    const old = await c.call("tx.commit", { projectId, tx: tx("tx-3", 1, [addAsset("$c")]), generation: snap.generation });
    expect(old.ok ? undefined : old.error.code).toBe("HISTORY_REWOUND");
  });

  test("ops.subscribe with a stale generation ends at once with HISTORY_REWOUND", async () => {
    const c = await server();
    const { projectId } = ok<{ projectId: string }>(await c.call("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" }));
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]) }));
    const snap = ok<{ rev: number; generation: unknown }>(await c.call("snapshot.get", { projectId, branch: "main" }));
    const revs: number[] = [];
    const live = c.stream("ops.subscribe", { projectId, branch: "main", fromRev: 0, generation: snap.generation }, (v) =>
      revs.push((v as { rev: number }).rev),
    );
    await until(() => (revs.length === 1 ? true : undefined), 5_000, "rev 1");
    c.interrupt(live.id);
    await live.done;
    const stale = c.stream(
      "ops.subscribe",
      { projectId, branch: "main", fromRev: 1, generation: { incarnation: "an-older-database", n: 0 } },
      () => undefined,
    );
    const exit = await stale.done;
    expect(exit.ok ? undefined : exit.error.code).toBe("HISTORY_REWOUND");
    expect(exit.ok ? undefined : exit.error.details).toMatchObject({ branch: "main", rev: 0, reason: "GENERATION" });
  });

  test("events stream emits status on lease change", async () => {
    const c = await server();
    const dir = join(tmp, "p");
    const { projectId } = ok<{ projectId: string }>(await c.call("project.create", { dir, name: "Demo", rate: "30" }));
    const events: Array<{ type: string; status?: Status }> = [];
    const sub = c.stream("project.events", { projectId }, (v) => events.push(v as (typeof events)[number]));
    const first = await until(() => events[0], 5_000, "the first status");
    expect(first.type).toBe("status");
    expect(first.status!.lease.state).toBe("held");
    // The lease record disappears: the next heartbeat (≤ 5 s) loses the lease.
    rmSync(join(dir, ".finisher", "lease.json"));
    const lost = await until(
      () => events.find((e) => e.type === "status" && e.status!.lease.state !== "held"),
      15_000,
      "a status without the lease",
    );
    // Read-only now: LEASE_LOST, or LEASE_HELD (this server's own claim, still fresh, is seen
    // as another holder's until it goes stale).
    expect(lost.status!.readOnly?.reason).toMatch(/^LEASE_(LOST|HELD)$/);
    c.interrupt(sub.id);
    await sub.done;
  }, 30_000);

  test("follower ops.subscribe receives imported edits", async () => {
    const writer = await server("sd-writer");
    const follower = await server("sd-follower");
    const dir = join(tmp, "shared");
    const { projectId } = ok<{ projectId: string }>(await writer.call("project.create", { dir, name: "Demo", rate: "30" }));
    expect(ok<{ projectId: string }>(await follower.call("project.open", { dir })).projectId).toBe(projectId);
    const status = ok<Status>(await follower.call("project.status", { projectId }));
    expect(status.readOnly?.reason).toBe("LEASE_HELD");

    const revs: number[] = [];
    const sub = follower.stream("ops.subscribe", { projectId, branch: "main", fromRev: 0 }, (v) => revs.push((v as { rev: number }).rev));
    ok(await writer.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]) }));
    ok(await writer.call("tx.commit", { projectId, tx: tx("tx-2", 1, [addAsset("$b")]) }));
    await until(() => (revs.length >= 2 ? revs : undefined), 15_000, "imported revs");
    expect(revs).toEqual([1, 2]);
    const got = ok<{ rev: number }>(await follower.call("tx.get", { projectId, txId: "tx-2" }));
    expect(got.rev).toBe(2);
    // The follower takes no edits.
    const refused = await follower.call("tx.commit", { projectId, tx: tx("tx-f", 2, [addAsset("$c")]) });
    expect(refused.ok ? undefined : refused.error.code).toBe("PROJECT_READ_ONLY");
    follower.interrupt(sub.id);
    await sub.done;
  }, 30_000);
});

describe("engine events in the registry", () => {
  async function registry(options?: RegistryOptions): Promise<ProjectRegistryShape> {
    scope = Effect.runSync(Scope.make());
    return Effect.runPromise(Scope.provide(scope)(makeProjectRegistry(join(tmp, "sd"), options)));
  }

  /** `exit`, or `undefined` when it has not settled within `ms`. */
  async function within<A>(p: Promise<A>, ms: number): Promise<A | undefined> {
    return Promise.race([p, new Promise<undefined>((r) => setTimeout(() => r(undefined), ms))]);
  }
  const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);

  /** Runs `ops.subscribe` in the background, collecting revs; resolves with its exit. */
  function subscribe(r: ProjectRegistryShape, projectId: string, fromRev: number) {
    const revs: number[] = [];
    const fiber = Effect.runFork(
      Stream.runForEach(r.subscribe(projectId, "main", fromRev), (c) => Effect.sync(() => revs.push(c.rev))),
    );
    return { revs, exit: () => Effect.runPromise(Fiber.await(fiber)) };
  }

  const failureOf = (exit: Exit.Exit<unknown, FinError>): FinError | undefined =>
    Exit.isFailure(exit) ? (exit.cause.reasons.find((r) => r._tag === "Fail") as { error: FinError } | undefined)?.error : undefined;

  test("ops.subscribe ends with HISTORY_REWOUND once history it reported is rewound", async () => {
    const r = await registry();
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    for (let i = 0; i < 3; i++) await run(r.commit(projectId, tx(`tx-${i}`, i, [addAsset(`$a${i}`)])));
    const saw = subscribe(r, projectId, 0);
    const behind = subscribe(r, projectId, 0);
    await until(() => (saw.revs.length === 3 ? true : undefined), 5_000, "the backlog");
    r.engineOf(projectId)!.emitRewoundForTest("main", 2);
    const e = failureOf(await saw.exit());
    expect(e?.code).toBe("HISTORY_REWOUND");
    expect(e?.details).toMatchObject({ branch: "main", rev: 2 });
    expect(behind.revs).toEqual([1, 2, 3]);
    expect(failureOf(await behind.exit())?.code).toBe("HISTORY_REWOUND");
  });

  test("a subscriber before the rewound revs carries on", async () => {
    const r = await registry();
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    await run(r.commit(projectId, tx("tx-0", 0, [addAsset("$a0")])));
    // Started at rev 1 and seen nothing past it; a rewind from rev 5 does not touch it.
    const sub = subscribe(r, projectId, 1);
    await new Promise((res) => setTimeout(res, 100));
    r.engineOf(projectId)!.emitRewoundForTest("main", 5);
    await new Promise((res) => setTimeout(res, 100));
    await run(r.commit(projectId, tx("tx-1", 1, [addAsset("$a1")])));
    await until(() => (sub.revs.length === 1 ? true : undefined), 5_000, "rev 2");
    expect(sub.revs).toEqual([2]);
  });

  test("a rebuild ends every subscriber that read a rev, with reason REBUILT", async () => {
    const r = await registry();
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    await run(r.commit(projectId, tx("tx-0", 0, [addAsset("$a0")])));
    const sub = subscribe(r, projectId, 0);
    await until(() => (sub.revs.length === 1 ? true : undefined), 5_000, "rev 1");
    await run(r.rebuild(projectId));
    const e = failureOf(await sub.exit());
    expect(e?.code).toBe("HISTORY_REWOUND");
    expect(e?.details).toMatchObject({ branch: "main", rev: 0, reason: "REBUILT" });
  });

  test("ENGINE_BUSY is passed through and keeps the engine", async () => {
    const r = await registry();
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    const engine = r.engineOf(projectId)!;
    engine.delayNextLongCallForTest(1_500);
    const verifying = run(r.verify(projectId));
    // Wait until the verify has the store (status answers at once, with `busy`).
    for (const end = Date.now() + 5_000; ((await run(r.status(projectId))) as Status).busy === null; ) {
      if (Date.now() > end) throw new Error("the verify never started");
      await new Promise((res) => setTimeout(res, 20));
    }
    const busy = failureOf(await Effect.runPromise(Effect.exit(r.commit(projectId, tx("tx-1", 0, [addAsset()])))));
    expect(busy?.code).toBe("ENGINE_BUSY");
    expect(r.engineOf(projectId)).toBe(engine);
    expect((await verifying) as { ok: boolean }).toMatchObject({ ok: true });
    const status = (await run(r.status(projectId))) as Status;
    expect(status.busy).toBeNull();
  });
  test("a subscriber behind a hub smaller than the burst still ends on a rewind whose marker it lost", async () => {
    const r = await registry({ hubCapacity: 2 });
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    for (let i = 0; i < 3; i++) await run(r.commit(projectId, tx(`tx-${i}`, i, [addAsset(`$a${i}`)])));
    const revs: number[] = [];
    // A slow consumer: the 2-item hub drops the rewind's marker under the burst below.
    const fiber = Effect.runFork(
      r.subscribe(projectId, "main", 0).pipe(
        Stream.mapEffect((c) => Effect.as(Effect.sleep("200 millis"), c.rev)),
        Stream.runForEach((rev) => Effect.sync(() => revs.push(rev))),
      ),
    );
    r.engineOf(projectId)!.emitRewoundForTest("main", 2);
    // The marker is on the hub (events arrive within milliseconds) while the backlog is
    // still being consumed (3 × 200 ms); the commits push it out.
    await new Promise((res) => setTimeout(res, 100));
    for (let i = 3; i < 10; i++) await run(r.commit(projectId, tx(`tx-${i}`, i, [addAsset(`$a${i}`)])));
    const exit = await within(Effect.runPromise(Fiber.await(fiber)), 5_000);
    expect(exit, "the subscriber never ended").toBeDefined();
    expect(failureOf(exit!)?.code).toBe("HISTORY_REWOUND");
    expect(revs.every((rev) => rev <= 3)).toBe(true);
  }, 15_000);

  test("the hub publishes every rev, also one below a rev it published before", async () => {
    const hubs = new Hubs(8);
    const committed = (rev: number) => ({ rev, tx: { branch: "main" } }) as unknown as CommittedTx;
    const got = await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const sub = yield* PubSub.subscribe(hubs.ops("p", "main").pubsub);
          // The new history's rev 2 can be published before its rewind is known.
          hubs.publishTx("p", committed(3));
          hubs.publishTx("p", committed(2));
          return yield* PubSub.takeAll(sub);
        }),
      ),
    );
    expect(got.map((i) => i.value?.rev)).toEqual([3, 2]);
  });

  test("an evicted engine ends its subscribers with ENGINE_CLOSED; subscribing again reopens it", async () => {
    const r = await registry();
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    await run(r.commit(projectId, tx("tx-0", 0, [addAsset("$a0")])));
    const sub = subscribe(r, projectId, 0);
    const events = Effect.runFork(Stream.runDrain(r.events(projectId)));
    await until(() => (sub.revs.length === 1 ? true : undefined), 5_000, "rev 1");
    const engine = r.engineOf(projectId)!;
    engine.panicNextTickForTest();
    const exit = await within(sub.exit(), 5_000);
    expect(exit, "the subscriber never ended").toBeDefined();
    expect(failureOf(exit!)?.code).toBe("ENGINE_CLOSED");
    const ev = await within(Effect.runPromise(Fiber.await(events)), 5_000);
    expect(ev && failureOf(ev)?.code).toBe("ENGINE_CLOSED");
    expect(r.engineOf(projectId)).toBeUndefined();
    const again = subscribe(r, projectId, 0);
    await until(() => (again.revs.length === 1 ? true : undefined), 5_000, "rev 1 again");
    expect(r.engineOf(projectId)).not.toBe(engine);
  });
});
