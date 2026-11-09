import { Effect, Schema } from "effect";
import { RpcSchema } from "effect/rpc";
import { expect, test } from "vitest";
import { FinError, FinRpcs, MERGE_INCLUDE_MAX, PROTOCOL_VERSION, STREAM_RPCS } from "./index.ts";

test("protocol version is 2 (5A: the handshake, tickets, asset.probe {projectId})", () => {
  expect(PROTOCOL_VERSION).toBe(2);
});

test("generation_is_required, probe_requires_project_id (protocol 2)", () => {
  const decode = (tag: string, payload: unknown) => Schema.decodeUnknownExit(FinRpcs.requests.get(tag)!.payloadSchema as Schema.Codec<unknown>)(payload)._tag;
  const generation = { incarnation: "i", n: 0 };
  expect(decode("tx.commit", { projectId: "p", tx: {} })).toBe("Failure");
  expect(decode("tx.commit", { projectId: "p", tx: {}, generation })).toBe("Success");
  expect(decode("ops.subscribe", { projectId: "p", branch: "main", fromRev: 0 })).toBe("Failure");
  expect(decode("ops.subscribe", { projectId: "p", branch: "main", fromRev: 0, generation })).toBe("Success");
  expect(decode("asset.probe", { path: "/m/a.mov" })).toBe("Failure");
  expect(decode("asset.probe", { projectId: "p", path: "/m/a.mov" })).toBe("Success");
});

test("the group exposes exactly the protocol 2 RPC tags", () => {
  expect([...FinRpcs.requests.keys()].sort()).toEqual(
    [
      "agent.approval.answer",
      "agent.approval.list",
      "agent.approval.wait",
      "agent.key.list",
      "agent.key.remove",
      "agent.key.set",
      "agent.local.remove",
      "agent.local.set",
      "agent.local.show",
      "agent.local.test",
      "agent.project.trust",
      "agent.providers",
      "agent.rules.add",
      "agent.rules.list",
      "agent.rules.remove",
      "agent.savedPrompt.list",
      "agent.savedPrompt.run",
      "agent.self",
      "agent.self.try",
      "agent.session.delete",
      "agent.session.end",
      "agent.session.get",
      "agent.session.interrupt",
      "agent.session.list",
      "agent.session.prompt",
      "agent.session.resume",
      "agent.session.rollback",
      "agent.session.start",
      "agent.session.steer",
      "agent.session.wait",
      "agent.watch",
      "asset.probe",
      "auth.audit.list",
      "auth.audit.purge",
      "auth.credential.list",
      "auth.credential.mint",
      "auth.credential.revoke",
      "auth.credential.rotateMaster",
      "auth.prove",
      "auth.revokeSelf",
      "auth.ticket",
      "auth.whoami",
      "branch.create",
      "branch.list",
      "branch.set",
      "console.challenge",
      "diagnostics.consent",
      "diagnostics.export",
      "diagnostics.project",
      "diagnostics.purge",
      "diagnostics.record",
      "diagnostics.status",
      "eval.audio",
      "eval.frame",
      "fonts.catalog",
      "fonts.fetch",
      "fonts.plan",
      "fonts.status",
      "history.cherryPick",
      "history.checkpoint",
      "history.merge",
      "history.redo",
      "history.revert",
      "history.undo",
      "jobs.cancel",
      "jobs.get",
      "jobs.list",
      "jobs.pause",
      "jobs.resume",
      "jobs.submit",
      "jobs.wait",
      "jobs.watch",
      "jobs.workers",
      "library.add",
      "library.file",
      "library.files",
      "library.list",
      "library.lookup",
      "library.remove",
      "library.scan",
      "library.search",
      "library.set",
      "library.status",
      "library.watch",
      "media.capabilities",
      "media.look",
      "ops.subscribe",
      "plugin.content.get",
      "plugin.disable",
      "plugin.discard",
      "plugin.enable",
      "plugin.info",
      "plugin.install",
      "plugin.list",
      "plugin.plan",
      "plugin.remove",
      "plugin.search",
      "plugin.suggest",
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
      "registry.add",
      "registry.forget",
      "registry.list",
      "registry.refresh",
      "registry.remove",
      "registry.status",
      "server.hello",
      "server.status",
      "server.stop",
      "snapshot.get",
      "training.closedTargets.get",
      "training.closedTargets.set",
      "tx.commit",
      "tx.get",
    ].sort(),
  );
});

test("ops.subscribe, project.events, jobs.watch, library.watch and agent.watch are streaming rpcs and the others are not", () => {
  const streams = new Set(["ops.subscribe", "project.events", "jobs.watch", "library.watch", "agent.watch"]);
  for (const [tag, rpc] of FinRpcs.requests) {
    expect(RpcSchema.isStreamSchema(rpc.successSchema), tag).toBe(streams.has(tag));
  }
});

test("rpc_stats_excludes_streams: STREAM_RPCS is exactly the streaming rpcs", () => {
  const streams = [...FinRpcs.requests].filter(([, rpc]) => RpcSchema.isStreamSchema(rpc.successSchema)).map(([tag]) => tag);
  expect([...STREAM_RPCS].sort()).toEqual(streams.sort());
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

test("history payloads decode what finisherd needs and the results are not streams", async () => {
  const decode = (tag: string, v: unknown) =>
    Effect.runPromise(Schema.decodeUnknownEffect(FinRpcs.requests.get(tag)!.payloadSchema as PlainSchema)(v));
  expect(await decode("branch.create", { projectId: "p", txId: "t", author: "a", from: "main", at: { checkpoint: "v1" } })).toEqual({
    projectId: "p",
    txId: "t",
    author: "a",
    from: "main",
    at: { checkpoint: "v1" },
  });
  expect(await decode("history.undo", { projectId: "p", txId: "t", author: "a", branch: "main", to: "v1", dryRun: true })).toMatchObject({
    to: "v1",
    dryRun: true,
  });
  await expect(decode("history.merge", { projectId: "p", txId: "t", author: "a", from: "x" })).rejects.toThrow();
  // A limited merge (plan 5E Decision 6): `include` bounded, `sourceUpTo` a seq.
  const merge = { projectId: "p", txId: "t", author: "a", from: "x", into: "main" };
  expect(await decode("history.merge", { ...merge, include: [], sourceUpTo: 7 })).toMatchObject({ include: [], sourceUpTo: 7 });
  const many = Array.from({ length: MERGE_INCLUDE_MAX + 1 }, (_, n) => `t${n}`);
  await expect(decode("history.merge", { ...merge, include: many })).rejects.toThrow();
  await expect(decode("history.merge", { ...merge, sourceUpTo: -1 })).rejects.toThrow();
  await expect(decode("history.merge", { ...merge, sourceUpTo: 1.5 })).rejects.toThrow();
  // `project.recover` lists with just the project.
  expect(await decode("project.recover", { projectId: "p" })).toEqual({ projectId: "p" });
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
