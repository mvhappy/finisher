import { Effect, Schema } from "effect";
import { RpcSchema } from "effect/rpc";
import { expect, test } from "vitest";
import { FinError, FinRpcs, PROTOCOL_VERSION } from "./index.ts";

test("protocol version starts at 1", () => {
  expect(PROTOCOL_VERSION).toBe(1);
});

test("the group exposes exactly the v1 RPC tags", () => {
  expect([...FinRpcs.requests.keys()].sort()).toEqual(
    [
      "asset.probe",
      "ops.subscribe",
      "project.acknowledgeRewind",
      "project.acknowledgeSalvage",
      "project.compact",
      "project.create",
      "project.events",
      "project.open",
      "project.rebuild",
      "project.recover",
      "project.status",
      "project.takeover",
      "project.verify",
      "server.hello",
      "snapshot.get",
      "tx.commit",
      "tx.get",
    ].sort(),
  );
});

test("ops.subscribe and project.events are streaming rpcs and the others are not", () => {
  for (const [tag, rpc] of FinRpcs.requests) {
    expect(RpcSchema.isStreamSchema(rpc.successSchema)).toBe(tag === "ops.subscribe" || tag === "project.events");
  }
});

/** The RPC schemas here use no services; `Schema.Top` leaves the requirements open. */
type PlainSchema = Schema.Codec<unknown, unknown, never, never>;

test("tx.get encodes a committed tx, a set-aside record and null", async () => {
  const success = FinRpcs.requests.get("tx.get")!.successSchema;
  const encode = (v: unknown) => Effect.runPromise(Schema.encodeUnknownEffect(success as PlainSchema)(v));
  const setAside = { setAside: { txId: "t", branch: "main", reason: "SUPERSEDED", origin: "local" } };
  expect(await encode(setAside)).toEqual(setAside);
  const committed = { rev: 1, tx: { txId: "t" }, bindings: {}, diff: {} };
  expect(await encode(committed)).toEqual(committed);
  expect(await encode(null)).toBeNull();
});

test("project.events items keep every field of each event kind", async () => {
  const success = FinRpcs.requests.get("project.events")!.successSchema;
  const item = RpcSchema.isStreamSchema(success) ? success.success : success;
  const encode = (v: unknown) => Effect.runPromise(Schema.encodeUnknownEffect(item as PlainSchema)(v));
  const rewound = { type: "rewound", branch: "main", fromRev: 3, quarantined: 2, own: false, generation: 1, incarnation: "i" };
  expect(await encode(rewound)).toEqual(rewound);
  expect(await encode({ type: "status", status: { lease: { state: "held" } } })).toEqual({
    type: "status",
    status: { lease: { state: "held" } },
  });
  expect(await encode({ type: "rebuilt", incarnation: "i2" })).toEqual({ type: "rebuilt", incarnation: "i2" });
});

test("FinError round-trips through encode/decode", async () => {
  const err = new FinError({
    code: "ASSET_NOT_FOUND",
    message: "no such file",
    details: { path: "/tmp/x" },
    fix: "check the path",
  });
  const encoded = await Effect.runPromise(Schema.encodeEffect(FinError)(err));
  expect(encoded).toEqual({
    _tag: "FinError",
    code: "ASSET_NOT_FOUND",
    message: "no such file",
    details: { path: "/tmp/x" },
    fix: "check the path",
  });
  const decoded = await Effect.runPromise(Schema.decodeUnknownEffect(FinError)(encoded));
  expect(decoded).toBeInstanceOf(FinError);
  expect(decoded.code).toBe("ASSET_NOT_FOUND");
  expect(decoded.fix).toBe("check the path");
});
