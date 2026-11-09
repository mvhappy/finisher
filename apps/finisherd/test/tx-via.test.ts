// `Tx.via` (plan 8B Task 3, spec §3.8): a client's `tx.commit` never stores a `via` (only the
// plugin host, which commits through the registry, sets it), and a stored `via` reaches the
// records the history RPCs and `tx.get` answer with.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { BranchCreated, CommittedTx, HistoryResult } from "@finisher/core-types";
import { Effect, Exit, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { makeProjectRegistry } from "../src/projects.ts";
import { addAsset, commit, connectProved, insert, type NdjsonClient, ok, type RunningServer, startServer, tx } from "./support/ndjson-client.ts";

let tmp: string;
const servers: RunningServer[] = [];
const clients: NdjsonClient[] = [];
let scope: Scope.Closeable | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-tx-via-"));
});

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  for (const s of servers.splice(0)) await s.stop();
  if (scope) await Effect.runPromise(Scope.close(scope, Exit.void));
  scope = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const via = { plugin: "@acme/tools@1.2.0", command: "trim-silence" };
const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);

describe("Tx.via", () => {
  test("client_via_is_stripped", async () => {
    const s = await startServer(join(tmp, "sd"));
    servers.push(s);
    const c = await connectProved(s);
    clients.push(c);
    const { projectId } = ok<{ projectId: string }>(await c.call("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" }));
    const committed = ok<CommittedTx>(await commit(c, projectId, { ...tx("t-1", 0, [addAsset()]), via }));
    expect(committed.tx.via).toBeUndefined();
    // A `null` or malformed one is dropped the same way (never stored, never an error).
    ok(await commit(c, projectId, { ...tx("t-2", 1, [insert(0, 5)]), via: null }));
    ok(await commit(c, projectId, { ...tx("t-3", 2, [insert(5, 5)]), via: "x" }));
    for (const id of ["t-1", "t-2", "t-3"]) {
      const got = ok<CommittedTx>(await c.call("tx.get", { projectId, txId: id }));
      expect(got.tx.txId).toBe(id);
      expect("via" in got.tx).toBe(false);
    }
  });

  test("a stored via reaches tx.get and the history records", async () => {
    scope = Effect.runSync(Scope.make());
    const r = await Effect.runPromise(Scope.provide(scope)(makeProjectRegistry(join(tmp, "sd"))));
    const { projectId } = await run(r.create(join(tmp, "p"), "Demo", "30"));
    await run(r.commit(projectId, tx("t-asset", 0, [addAsset()])));
    const made = (await run(
      r.history(projectId, "branchCreate", { projectId, txId: "b1", author: "ann", name: "feature", from: "main" }),
    )) as BranchCreated;
    // What the plugin host does (Task 9): the registry, not the RPC, so `via` is kept.
    const p1 = await run(r.commit(projectId, { ...tx("p1", 1, [insert(0, 5)]), via }));
    expect(p1.tx.via).toEqual(via);
    const got = (await run(r.txGet(projectId, "p1"))) as CommittedTx;
    expect(got.tx.via).toEqual(via);

    const picked = (await run(
      r.history(projectId, "cherryPick", { projectId, txId: "k1", author: "dave", origin: "p1", into: made.branch.id }),
    )) as HistoryResult;
    expect(picked.committed.map((x) => [x.tx.author, x.tx.via])).toEqual([["dave", via]]);
  });
});
