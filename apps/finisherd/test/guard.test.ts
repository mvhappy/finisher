// The write guard end to end (plan 5A Task 5, Decision 17): the chain's guard rides in a write's
// `metaJson` to fin-node, and the store checks it against the branch it resolves inside its
// write transaction, so a rename between the policy stage and the write cannot open a window.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { BranchCreated, WriteGuard } from "@finisher/core-types";
import { Effect, Exit, Scope } from "effect";
import { afterEach, beforeEach, expect, test } from "vitest";
import type { CallMeta } from "../src/diagnostics.ts";
import { makeProjectRegistry, type ProjectRegistryShape } from "../src/projects.ts";
import { addAsset, insert, tx } from "./support/ndjson-client.ts";

let tmp: string;
let scope: Scope.Closeable | undefined;
let r: ProjectRegistryShape;

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-guard-"));
  scope = Effect.runSync(Scope.make());
  r = await Effect.runPromise(Scope.provide(scope)(makeProjectRegistry(join(tmp, "sd"))));
});

afterEach(async () => {
  if (scope) await Effect.runPromise(Scope.close(scope, Exit.void));
  scope = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const failure = (e: Effect.Effect<unknown, FinError>): Promise<FinError> => Effect.runPromise(Effect.flip(e));
const meta = (guard: WriteGuard): CallMeta => ({ actor: { kind: "agent" }, guard });
const author = "agent:a1";

test("rename_between_resolve_and_write_is_caught", async () => {
  const projectId = (await run(r.create(join(tmp, "p"), "Demo", "30"))).projectId;
  await run(r.commit(projectId, tx("t-asset", 0, [addAsset()])));
  const make = async (txId: string, name: string) =>
    ((await run(r.history(projectId, "branchCreate", { projectId, txId, author, name, from: "main" }))) as BranchCreated).branch.id;
  const x = await make("b-x", "x");
  const y = await make("b-y", "y");
  // The policy stage resolved "y" to its id and allowed only that branch...
  const guard: WriteGuard = { denyBranches: [], allowBranches: [await run(r.resolveBranch(projectId, "y"))] };
  expect(guard.allowBranches).toEqual([y]);
  // ...then, before the write, "y" comes to name another branch.
  await run(r.history(projectId, "branchSet", { projectId, txId: "s-1", author, branch: y, name: "old-y" }));
  await run(r.history(projectId, "branchSet", { projectId, txId: "s-2", author, branch: x, name: "y" }));
  const before = await run(r.snapshot(projectId, x));
  const commit = { ...tx("t-sneak", before.rev, [insert(0, 5)]), branch: "y" };
  const refused = await failure(r.commit(projectId, commit, undefined, meta(guard)));
  expect(refused).toMatchObject({ code: "BRANCH_NOT_ALLOWED", details: { branch: x } });
  expect((await run(r.snapshot(projectId, x))).rev).toBe(before.rev);
  expect(await run(r.txGet(projectId, "t-sneak"))).toBeNull();
  // The branch the guard is about still takes the write, by its new name.
  const ok = await run(r.commit(projectId, { ...tx("t-ok", (await run(r.snapshot(projectId, y))).rev, [insert(0, 5)]), branch: "old-y" }, undefined, meta(guard)));
  expect(JSON.stringify(ok)).toContain('"t-ok"');
  expect(await run(r.txGet(projectId, "t-ok"))).not.toBeNull();
});

test("a_denied_branch_refuses_history_writes", async () => {
  const projectId = (await run(r.create(join(tmp, "p"), "Demo", "30"))).projectId;
  await run(r.commit(projectId, tx("t-asset", 0, [addAsset()])));
  const deny = meta({ denyBranches: ["main"] });
  // `tx()` commits as author "test": undo is per author.
  const refused = await failure(r.history(projectId, "undo", { projectId, txId: "u-1", author: "test", branch: "main" }, deny));
  expect(refused.code).toBe("BRANCH_PROTECTED");
  expect((await run(r.snapshot(projectId, "main"))).rev).toBe(1);
  // No guard (an open one): the same undo works.
  await run(r.history(projectId, "undo", { projectId, txId: "u-2", author: "test", branch: "main" }, meta({ denyBranches: [] })));
  expect((await run(r.snapshot(projectId, "main"))).rev).toBe(2);
});
