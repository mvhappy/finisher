// In-process ProjectRegistry tests (fin-node `test-hooks` build via vitest.config.ts).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { Engine } from "@finisher/fin-node";
import { Effect, Exit, Fiber, Scope, Stream } from "effect";
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
