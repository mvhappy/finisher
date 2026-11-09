// `history.drop` through the server (5E plan Task 5, Decisions 5, 16, 17, 26): the record, the
// `argsHash` the server always sets, the audit row (a count, never ids or review text) and the
// scopes the chain asks for.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { BranchCreated, CommittedTx, MergeReport } from "@finisher/core-types";
import { afterEach, beforeEach, expect, test } from "vitest";
import { dropPayload } from "../src/history.ts";
import { addAsset, commit, connectProved, insert, type NdjsonClient, ok, type RunningServer, startServer, tx } from "./support/ndjson-client.ts";

let tmp: string;
let server: RunningServer;
const clients: NdjsonClient[] = [];
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-drop-"));
});
afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  await server?.stop();
  rmSync(tmp, { recursive: true, force: true });
});

async function connect(name: "local" | "console"): Promise<NdjsonClient> {
  const c = await connectProved(server, name);
  clients.push(c);
  return c;
}

/** A project whose branch `feature` holds two commits main lacks: `f1` and `f2`. */
async function setup(local: NdjsonClient) {
  const { projectId } = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" }));
  ok(await commit(local, projectId, tx("t-asset", 0, [addAsset()])));
  const made = ok<BranchCreated>(await local.call("branch.create", { projectId, txId: "b1", author: "ann", name: "feature", from: "main" }));
  const branch = made.branch.id;
  const f1 = ok<CommittedTx>(await commit(local, projectId, { ...tx("f1", 1, [insert(0, 5)]), branch }));
  ok(await commit(local, projectId, { ...tx("f2", f1.rev, [insert(5, 5)]), branch }));
  return { projectId, branch };
}

const write = (projectId: string, txId: string, extra: object) => ({ projectId, txId, author: "ann", from: "feature", into: "main", ...extra });

test("drop_audited_without_text", async () => {
  server = await startServer(join(tmp, "sd"));
  const local = await connect("local");
  const { projectId } = await setup(local);
  // A drop into `main` (protected by default) is a reviewer's: `local` holds `review.decide`.
  const dropped = ok<CommittedTx>(await local.call("history.drop", write(projectId, "d1", { drops: ["f1"] })));
  expect(dropped.tx.txId).toBe("d1");
  // A dry run plans and audits nothing.
  const dry = await local.call("history.drop", write(projectId, "d-dry", { drops: ["f2"], dryRun: true }));
  expect(dry.ok).toBe(true);
  const rows = ok<{ entries: Array<{ kind: string; outcome: string; details: Record<string, unknown> }> }>(
    await (await connect("console")).call("auth.audit.list", { kind: "history.drop" }),
  );
  expect(rows.entries).toHaveLength(1);
  expect(rows.entries[0]).toMatchObject({ kind: "history.drop", outcome: "ok", details: { count: 1 } });
  // Ids, branches and review text are not in it.
  const text = JSON.stringify(rows.entries);
  for (const secret of ["f1", "f2", "feature", "main", "ann"]) expect(text).not.toContain(`"${secret}"`);
});

test("history_drop_always_sets_an_args_hash_and_a_reused_key_is_refused", async () => {
  server = await startServer(join(tmp, "sd"));
  const local = await connect("local");
  const { projectId } = await setup(local);
  const first = ok<CommittedTx>(await local.call("history.drop", write(projectId, "d1", { drops: ["f1"] })));
  expect(first.tx.argsHash).toMatch(/^blake3:[0-9a-f]{64}$/);
  // The same key and request answers the record again (the order of the ids does not matter)...
  const again = ok<CommittedTx>(await local.call("history.drop", write(projectId, "d1", { drops: ["f1"] })));
  expect(again.rev).toBe(first.rev);
  // ...another request under the key is IDEM_KEY_REUSED, never the first record.
  const reused = await local.call("history.drop", write(projectId, "d1", { drops: ["f2"] }));
  expect(reused.ok).toBe(false);
  expect(JSON.stringify(reused)).toContain("IDEM_KEY_REUSED");
  // A client's own hash is kept as sent.
  const mine = `blake3:${"a".repeat(64)}`;
  expect(dropPayload({ from: "a", into: "b", drops: ["x"], argsHash: mine }).argsHash).toBe(mine);
  expect(dropPayload({ from: "a", into: "b", drops: ["y", "x"] }).argsHash).toBe(dropPayload({ from: "a", into: "b", drops: ["x", "y", "x"] }).argsHash);
  expect(dropPayload({ from: "a", into: "b", drops: ["x"] }).argsHash).not.toBe(dropPayload({ from: "a", into: "b", undrops: ["x"] }).argsHash);
});

test("a_drop_keeps_the_origin_out_of_a_plain_merge_and_undrop_brings_it_back", async () => {
  server = await startServer(join(tmp, "sd"));
  const local = await connect("local");
  const { projectId } = await setup(local);
  ok(await local.call("history.drop", write(projectId, "d1", { drops: ["f1"] })));
  const merged = ok<MergeReport>(await local.call("history.merge", { projectId, txId: "m1", author: "ann", from: "feature", into: "main", skipConflicting: true }));
  expect(merged.excluded?.map((e) => [e.txId, e.reason])).toContainEqual(["f1", "DROPPED"]);
  expect(merged.committed.map((c) => c.tx.txId.replace(/-\d+$/, ""))).not.toContain("f1");
  // Lift the drop and a merge copies it.
  ok(await local.call("history.drop", write(projectId, "d2", { undrops: ["f1"] })));
  const again = ok<MergeReport>(await local.call("history.merge", { projectId, txId: "m2", author: "ann", from: "feature", into: "main", skipConflicting: true }));
  expect(again.merged).toContain("f1");
});
