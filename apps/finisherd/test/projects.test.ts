// In-process ProjectRegistry tests (fin-node `test-hooks` build via vitest.config.ts).
import { cpSync, mkdtempSync, renameSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { Engine } from "@finisher/fin-node";
import { Cause, Effect, Exit, Fiber, Scope, Stream } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { toFinError } from "../src/errors.ts";
import { makeProjectRegistry, type ProjectRegistryShape, type RegistryOptions } from "../src/projects.ts";
import { addAsset, tx } from "./support/ndjson-client.ts";

let tmp: string;
let scope: Scope.Closeable | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-registry-"));
});

afterEach(async () => {
  if (scope) await Effect.runPromise(Scope.close(scope, Exit.void));
  scope = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function registry(options?: RegistryOptions): Promise<ProjectRegistryShape> {
  scope = Effect.runSync(Scope.make());
  return Effect.runPromise(Scope.provide(scope)(makeProjectRegistry(join(tmp, "sd"), options)));
}

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const failure = (e: Effect.Effect<unknown, FinError>): Promise<FinError> => Effect.runPromise(Effect.flip(e));

describe("ProjectRegistry", () => {
  test("a subscriber far behind a small sliding hub still gets every rev once, in order", async () => {
    const r = await registry({ hubCapacity: 4 });
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    await run(r.commit(projectId, tx("tx-0", 0, [addAsset("$a0")])));

    const N = 40;
    const collected = Effect.runFork(
      r.subscribe(projectId, "main", 1).pipe(
        // A slow consumer: the hub (capacity 4) overflows and drops the oldest items.
        Stream.mapEffect((c) => Effect.as(Effect.sleep("15 millis"), c.rev)),
        Stream.take(N),
        Stream.runCollect,
      ),
    );
    await new Promise((res) => setTimeout(res, 20));
    for (let i = 1; i <= N; i++) await run(r.commit(projectId, tx(`tx-${i}`, i, [addAsset(`$a${i}`)])));
    const revs = await Effect.runPromise(Fiber.join(collected));
    expect(revs).toEqual(Array.from({ length: N }, (_, i) => i + 2));
  });

  test("open checks the project it was told to expect, before registering it (review of Task 5, H2)", async () => {
    const r = await registry();
    const [dirA, dirB] = [join(tmp, "a"), join(tmp, "b")];
    const a = (await run(r.create(dirA, "A", "30"))).projectId;
    const b = (await run(r.create(dirB, "B", "30"))).projectId;
    // Open here already: the folder's engine is not the expected project.
    expect(await failure(r.open(dirB, false, { projectId: a }))).toMatchObject({ code: "PROJECT_NOT_FOUND", details: { reason: "changed" } });
    await Effect.runPromise(Scope.close(scope!, Exit.void));
    scope = undefined;
    const r2 = await registry();
    // Loaded from disk: refused before it is registered (its lease is not kept).
    expect(await failure(r2.open(dirB, false, { projectId: a }))).toMatchObject({ code: "PROJECT_NOT_FOUND", details: { reason: "changed" } });
    expect(r2.isOpen(b)).toBe(false);
    // Attach-only answers only from an engine open here.
    expect((await failure(r2.open(dirA, false, { projectId: a, attachOnly: true }))).code).toBe("PROJECT_NOT_FOUND");
    expect(r2.isOpen(a)).toBe(false);
    expect((await run(r2.open(dirA, false, { projectId: a }))).projectId).toBe(a);
    expect((await run(r2.open(dirA, false, { projectId: a, attachOnly: true }))).projectId).toBe(a);
  });

  test("opening the same dir twice (even concurrently) yields one engine", async () => {
    const r = await registry();
    const dir = join(tmp, "p");
    const { projectId } = await run(r.create(dir, "Demo", "30"));
    const engine = r.engineOf(projectId);
    const [a, b] = await Promise.all([run(r.open(dir)), run(r.open(`${dir}/.`))]);
    expect(a.projectId).toBe(projectId);
    expect(b.projectId).toBe(projectId);
    expect(r.engineOf(projectId)).toBe(engine);
  });

  test("create retries with one idemKey (even concurrent) share one writable engine", async () => {
    const r = await registry();
    const dir = join(tmp, "p");
    const [a, b] = await Promise.all([run(r.create(dir, "Demo", "30", "k1")), run(r.create(dir, "Demo", "30", "k1"))]);
    expect(b.projectId).toBe(a.projectId);
    const engine = r.engineOf(a.projectId);
    // A later retry is answered by the open engine.
    expect((await run(r.create(`${dir}/.`, "Demo", "30", "k1"))).projectId).toBe(a.projectId);
    expect(r.engineOf(a.projectId)).toBe(engine);
    // The kept engine holds the lease: it can commit.
    await run(r.commit(a.projectId, tx("tx-0", 0, [addAsset("$a0")])));
    expect((await failure(r.create(dir, "Other", "30", "k1"))).code).toBe("IDEM_KEY_REUSED");
    expect((await failure(r.create(dir, "Demo", "30", "k2"))).code).toBe("PROJECT_EXISTS");
    expect((await failure(r.create(dir, "Demo", "30"))).code).toBe("PROJECT_EXISTS");
  });

  test("a dead engine is evicted and the project reopens on the next call", async () => {
    const r = await registry();
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    await run(r.commit(projectId, tx("tx-1", 0, [addAsset()])));
    const engine = r.engineOf(projectId)!;
    await engine.panicForTest().catch(() => undefined);

    const dead = await failure(r.snapshot(projectId, "main"));
    expect(dead.code).toBe("ENGINE_CLOSED");
    expect(r.engineOf(projectId)).toBeUndefined();

    const snap = await run(r.snapshot(projectId, "main"));
    expect(snap.rev).toBe(1);
    expect(r.engineOf(projectId)).not.toBe(engine);
  });

  test("opening a moved project takes it on at its new folder, writable, its streams going on (review #32)", async () => {
    const r = await registry();
    const [from, to] = [join(tmp, "a"), join(tmp, "b")];
    const { projectId } = await run(r.create(from, "Demo", "30"));
    await run(r.commit(projectId, tx("tx-0", 0, [addAsset("$a0")])));
    const old = r.engineOf(projectId);
    const ops = Effect.runFork(r.subscribe(projectId, "main", 1).pipe(Stream.take(1), Stream.runCollect));
    const seen: Array<{ type: string; fromRev?: number }> = [];
    const events = Effect.runFork(r.events(projectId).pipe(Stream.runForEach((e) => Effect.sync(() => seen.push(e as never)))));
    while (seen.length === 0) await new Promise((res) => setTimeout(res, 20));
    renameSync(from, to);
    expect((await run(r.open(to))).projectId).toBe(projectId);
    // The events subscriber goes on and hears the new engine.
    r.engineOf(projectId)!.emitRewoundForTest("main", 7);
    while (!seen.some((e) => e.type === "rewound")) await new Promise((res) => setTimeout(res, 20));
    expect(seen.find((e) => e.type === "rewound")).toMatchObject({ fromRev: 7 });
    expect(events.pollUnsafe()).toBeUndefined();
    expect(r.engineOf(projectId)).not.toBe(old);
    expect(r.dirOf(projectId)).toBe(to);
    expect(r.indexedDirOf(projectId)).toBe(to);
    // Writable at once (the old engine, whose folder went, had lost its lease).
    await run(r.commit(projectId, tx("tx-1", 1, [addAsset("$a1")])));
    expect((await Effect.runPromise(Fiber.join(ops))).map((c) => c.rev)).toEqual([2]);
    // A link to the new folder is the same project, the same engine.
    symlinkSync(to, join(tmp, "link"));
    const now = r.engineOf(projectId);
    expect((await run(r.open(join(tmp, "link")))).projectId).toBe(projectId);
    expect(r.engineOf(projectId)).toBe(now);
    await run(r.commit(projectId, tx("tx-2", 2, [addAsset("$a2")])));
  });

  test("a moved project is taken on when another project now sits at its old folder (review #32)", async () => {
    const r = await registry();
    const [from, to] = [join(tmp, "a"), join(tmp, "b")];
    const { projectId } = await run(r.create(from, "Demo", "30"));
    renameSync(from, to);
    const other = (await run(r.create(from, "Other", "30"))).projectId;
    expect((await run(r.open(to))).projectId).toBe(projectId);
    expect(r.dirOf(projectId)).toBe(to);
    await run(r.commit(projectId, tx("tx-0", 0, [addAsset("$a0")])));
    await run(r.commit(other, tx("tx-o", 0, [addAsset("$o")])));
  });

  test("two paths to a moved project's new folder, opened at once, share one writable engine (review #32)", async () => {
    const r = await registry();
    const [from, to, link] = [join(tmp, "a"), join(tmp, "b"), join(tmp, "link")];
    const { projectId } = await run(r.create(from, "Demo", "30"));
    renameSync(from, to);
    symlinkSync(to, link);
    const [x, y] = await Promise.all([run(r.open(to)), run(r.open(link))]);
    expect([x.projectId, y.projectId]).toEqual([projectId, projectId]);
    await run(r.commit(projectId, tx("tx-0", 0, [addAsset("$a0")])));
  });

  test("a copy whose original is still there is refused; the engine stays at the original (review #32)", async () => {
    const r = await registry();
    const [from, copy] = [join(tmp, "a"), join(tmp, "copy")];
    const { projectId } = await run(r.create(from, "Demo", "30"));
    const engine = r.engineOf(projectId);
    cpSync(from, copy, { recursive: true });
    // fin-store refuses the copy (it is the original's to keep); the engine stays as it was.
    expect((await failure(r.open(copy))).code).toBe("PROJECT_COPY");
    expect(r.engineOf(projectId)).toBe(engine);
    expect(r.dirOf(projectId)).toBe(from);
    await run(r.commit(projectId, tx("tx-0", 0, [addAsset("$a0")])));
  });

  test("a project.events subscriber that falls behind its buffer ends WATCH_LAGGING (review #38)", async () => {
    const r = await registry({ hubCapacity: 2 });
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    const engine = r.engineOf(projectId)!;
    const got: unknown[] = [];
    // Paused on its first item (the status) while four rewinds arrive.
    const paused = Effect.runFork(
      r.events(projectId).pipe(
        Stream.mapEffect((e) => Effect.as(got.length === 0 ? Effect.sleep("600 millis") : Effect.void, e)),
        Stream.runForEach((e) => Effect.sync(() => got.push(e))),
      ),
    );
    await new Promise((res) => setTimeout(res, 200));
    for (let i = 1; i <= 4; i++) engine.emitRewoundForTest("main", i);
    const exit = await Effect.runPromise(Fiber.await(paused));
    expect(Exit.isFailure(exit) ? toFinError(Cause.squash(exit.cause)) : undefined).toMatchObject({
      code: "WATCH_LAGGING",
      details: { projectId, buffer: 2 },
    });
    // Nothing after the gap was handed on as if none had been lost.
    expect(got.filter((e) => (e as { type: string }).type === "rewound")).toEqual([]);
  });

  test("events emitted before project.events read its status are not replayed after it (review #39)", async () => {
    const r = await registry();
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    const engine = r.engineOf(projectId)!;
    // Queued for the JS thread before the subscriber reads the status: that status covers them.
    engine.emitRewoundForTest("main", 1);
    engine.emitRewoundForTest("main", 2);
    const two = Effect.runFork(r.events(projectId).pipe(Stream.take(2), Stream.runCollect));
    await new Promise((res) => setTimeout(res, 300));
    engine.emitRewoundForTest("main", 9);
    const items = (await Effect.runPromise(Fiber.join(two))) as ReadonlyArray<{ type: string; fromRev?: number }>;
    expect(items[0]?.type).toBe("status");
    expect(items[1]).toMatchObject({ type: "rewound", fromRev: 9 });
  });

  test("an engine that finishes opening after shutdown started is closed, not registered", async () => {
    const dir = join(tmp, "p");
    await (await Engine.create(join(tmp, "sd"), dir, "Demo", "30")).close();
    const r = await registry();
    // Engine.open runs on the libuv pool; shutdown begins before it resolves.
    const pending = run(r.open(dir));
    await Effect.runPromise(Scope.close(scope!, Exit.void));
    scope = undefined;
    const late = await pending.then(
      () => undefined,
      (e: FinError) => e,
    );
    expect(late?.code).toBe("SERVER_SHUTTING_DOWN");
    expect((await failure(r.open(dir))).code).toBe("SERVER_SHUTTING_DOWN");
  });
});

describe("toFinError", () => {
  test("passes through fin-node codes only", () => {
    const fin = Object.assign(new Error("conflict"), { code: "CONFLICT", details: { a: 1 }, fix: "retry", finNode: true });
    expect(toFinError(fin)).toMatchObject({ code: "CONFLICT", details: { a: 1 }, fix: "retry" });

    const enoent = Object.assign(new Error("no such file"), { code: "ENOENT" });
    expect(toFinError(enoent)).toMatchObject({ code: "INTERNAL", details: { cause: "ENOENT" } });
    const sqlite = Object.assign(new Error("database is locked"), { code: "ERR_SQLITE_ERROR" });
    expect(toFinError(sqlite)).toMatchObject({ code: "INTERNAL", details: { cause: "ERR_SQLITE_ERROR" } });
    expect(toFinError("boom")).toMatchObject({ code: "INTERNAL" });
  });
});
