import { Effect, Schema } from "effect";
import { RpcSchema } from "effect/rpc";
import { expect, test } from "vitest";
import { FinError, FinRpcs, PROTOCOL_VERSION } from "./index.ts";

test("protocol version starts at 1", () => {
  expect(PROTOCOL_VERSION).toBe(1);
});

test("the group exposes exactly the 7 v1 RPC tags", () => {
  expect([...FinRpcs.requests.keys()].sort()).toEqual(
    [
      "asset.probe",
      "ops.subscribe",
      "project.create",
      "project.open",
      "server.hello",
      "snapshot.get",
      "tx.commit",
    ].sort(),
  );
});

test("ops.subscribe is a streaming rpc and the others are not", () => {
  for (const [tag, rpc] of FinRpcs.requests) {
    expect(RpcSchema.isStreamSchema(rpc.successSchema)).toBe(tag === "ops.subscribe");
  }
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
