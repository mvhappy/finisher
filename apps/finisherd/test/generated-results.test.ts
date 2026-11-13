// The RPC results whose Effect schemas are generated (`packages/contracts/src/generated/schemas.ts`)
// keep every field of what the engine answers: each raw answer decodes and encodes back to itself
// (a `Struct` drops a field it does not list, so a schema behind its Rust type would fail here).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FinRpcs } from "@finisher/contracts";
import { Engine } from "@finisher/fin-node";
import { Schema } from "effect";
import { RpcSchema } from "effect/rpc";
import { afterEach, beforeEach, expect, test } from "vitest";

let tmp: string;
let engines: Engine[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-generated-"));
});

afterEach(async () => {
  for (const e of engines) await e.close().catch(() => undefined);
  engines = [];
  rmSync(tmp, { recursive: true, force: true });
});

type Plain = Schema.Codec<unknown, unknown, never, never>;

/** The success schema of `tag` (a stream's item). */
function success(tag: string): Plain {
  const s = FinRpcs.requests.get(tag)!.successSchema;
  return (RpcSchema.isStreamSchema(s) ? s.success : s) as Plain;
}

/** `raw` (an engine answer, JSON) through `tag`'s success schema and back: unchanged. */
function roundTrips(tag: string, raw: string): unknown {
  const value: unknown = JSON.parse(raw);
  const decoded = Schema.decodeUnknownSync(success(tag))(value);
  expect(JSON.parse(JSON.stringify(Schema.encodeSync(success(tag))(decoded))), tag).toStrictEqual(value);
  return value;
}

const req = (o: object) => JSON.stringify({ author: "test", ...o });
const tx = (txId: string, branch: string, baseRev: number, ops: unknown[]) =>
  JSON.stringify({ txId, author: "test", branch, baseRev, schemaVersion: 2, ops });
const addAsset = {
  op: "asset.add",
  id: "$a",
  path: "/media/a.mov",
  duration: "600",
  video: { width: 1920, height: 1080, rate: "30" },
  fingerprint: "fp",
};
const insert = (at: number) => ({ op: "clip.insert", id: "$c", track: "t1", at: String(at), asset: "a1", sourceIn: "0", duration: "5" });

test("status, takeover, verify, rebuild, compact and the status event keep every field", async () => {
  const e = await Engine.create(join(tmp, "sd"), join(tmp, "Project"), "Demo", "30");
  engines.push(e);
  const events: string[] = [];
  e.onEvent((json) => events.push(json));
  await e.commit(tx("t-asset", "main", 0, [addAsset]));

  const status = roundTrips("project.status", await e.status()) as { busy: unknown; fonts?: unknown; warnings: unknown[] };
  expect(status.busy).toBeNull();
  const taken = roundTrips("project.takeover", await e.takeover(false)) as object;
  expect(taken).not.toHaveProperty("busy");
  const verify = roundTrips("project.verify", await e.verify()) as { checks: unknown[] };
  expect(verify.checks.length).toBeGreaterThan(0);
  roundTrips("project.rebuild", await e.rebuild());
  roundTrips("project.compact", await e.compact());

  // A status `project.events` item as the engine emits it (finisherd passes it on).
  await expect.poll(() => events.some((j) => JSON.parse(j).type === "status")).toBe(true);
  for (const j of events.filter((j) => JSON.parse(j).type === "status")) roundTrips("project.events", j);
});

test("branch create, undo, redo, merge, cherry-pick, revert and recover keep every field", async () => {
  const e = await Engine.create(join(tmp, "sd"), join(tmp, "Project"), "Demo", "30");
  engines.push(e);
  await e.commit(tx("t-asset", "main", 0, [addAsset]));
  await e.commit(tx("t-c1", "main", 1, [insert(0)]));

  roundTrips("branch.create", await e.branchCreate(req({ txId: "b1", name: "feature", from: "main" })));
  await e.commit(tx("t-b1", "br1", 2, [insert(10)]));
  await e.commit(tx("t-b2", "br1", 3, [insert(20)]));
  const merged = roundTrips("history.merge", await e.merge(req({ txId: "m1", from: "feature", into: "main", include: ["t-b1"] }))) as {
    notIncluded: string[];
    placements: unknown[];
  };
  expect(merged.notIncluded).toEqual(["t-b2"]);
  roundTrips("history.undo", await e.undo(req({ txId: "u1", branch: "main" })));
  roundTrips("history.redo", await e.redo(req({ txId: "r1", branch: "main" })));
  roundTrips("history.revert", await e.revert(req({ txId: "v1", mergeId: "m1", branch: "main" })));
  const twice = roundTrips("history.revert", await e.revert(req({ txId: "v2", mergeId: "m1", branch: "main" })));
  expect(twice).toMatchObject({ noOp: "ALREADY_REVERTED" });
  roundTrips("history.cherryPick", await e.cherryPick(req({ txId: "p1", origin: "t-b2", into: "main" })));
  const again = roundTrips("history.cherryPick", await e.cherryPick(req({ txId: "p2", origin: "t-b2", into: "main" })));
  expect(again).toMatchObject({ noOp: "ALREADY_PRESENT" });
  roundTrips("project.recover", await e.recover(req({})));
});

test("a field the schema does not list is dropped, a missing required one refused", () => {
  const verify = success("project.verify");
  expect(Schema.encodeSync(verify)(Schema.decodeUnknownSync(verify)({ ok: true, checks: [], extra: 1 }))).toEqual({ ok: true, checks: [] });
  expect(() => Schema.decodeUnknownSync(verify)({ checks: [] })).toThrow();
});
