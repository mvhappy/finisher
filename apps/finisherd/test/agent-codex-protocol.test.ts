// The Codex app-server protocol (sub-project 7E plan Task 3, Decisions 2 and 3; Review Focus 2
// and 6): the generated Effect Schemas decode every message the adapter uses, drift is a notice
// and never an error, a missing needed field is `PROVIDER_PROTOCOL`, and the vendored client
// frames, correlates, fails pending requests at EOF and sends only the allow-listed methods.
import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { PassThrough, Writable } from "node:stream";
import { fileURLToPath } from "node:url";
import { FinError } from "@finisher/contracts";
import { Cause, Effect, Exit, Fiber, Schema, Stream } from "effect";
import fc from "fast-check";
import { describe, expect, it } from "vitest";
import {
  CLIENT_REQUEST_METHODS,
  CLIENT_REQUESTS,
  SERVER_NOTIFICATIONS,
  SERVER_REQUESTS,
  ReasoningEffort,
  type TurnStatus,
  UnknownMember,
} from "../src/agent/codex/generated/protocol.ts";
import {
  ALLOWED_METHODS,
  type CodexRpc,
  codexRpc,
  decodeIncoming,
  decodeNotification,
  decodeResult,
  decodeServerRequest,
  type ServerNotification,
  type ServerReply,
  type ServerRequest,
} from "../src/agent/codex/rpc.ts";

const REPO = fileURLToPath(new URL("../../../", import.meta.url));
const FIXTURES = fileURLToPath(new URL("./fixtures/codex/", import.meta.url));
const MiB = 1024 * 1024;

const fixtureLines = (): { file: string; line: string }[] =>
  readdirSync(FIXTURES)
    .filter((f) => f.endsWith(".jsonl"))
    .sort()
    .flatMap((file) =>
      readFileSync(join(FIXTURES, file), "utf8")
        .split("\n")
        .filter((l) => l.trim() !== "")
        .map((line) => ({ file, line })),
    );

/** Every `UnknownMember` under `v`, by path. */
function unknownsIn(v: unknown, path = "$"): string[] {
  if (v instanceof UnknownMember) return [`${path}=${v.name}`];
  if (Array.isArray(v)) return v.flatMap((x, i) => unknownsIn(x, `${path}[${i}]`));
  if (v !== null && typeof v === "object") return Object.entries(v).flatMap(([k, x]) => unknownsIn(x, `${path}.${k}`));
  return [];
}

const isFin = (v: unknown): v is FinError => v instanceof FinError;
const T = "01a1d879-9522-7233-8cfb-c561a1b84de0";
const U = "01a1d879-9530-7482-9c4c-882985904a0c";
const turnCompleted = (turn: Record<string, unknown>) => ({ threadId: T, turn: { id: U, items: [], status: "completed", ...turn } });

// ---- a client over in-memory pipes ----

interface Harness {
  readonly rpc: CodexRpc;
  /** The lines the client wrote, parsed. */
  readonly sent: () => Record<string, unknown>[];
  /** Waits until the client wrote `n` lines. */
  readonly sentAtLeast: (n: number) => Promise<Record<string, unknown>[]>;
  readonly feed: (chunk: string | Buffer) => void;
  readonly line: (msg: unknown) => void;
  readonly eof: () => void;
  /** The server's stdout destroyed (its `close` without an `end`, as after a kill). */
  readonly destroy: () => void;
  readonly stdin: PassThrough;
}

function harness(): Harness {
  const stdin = new PassThrough();
  const stdout = new PassThrough();
  let text = "";
  const waiters: (() => void)[] = [];
  stdin.on("data", (b: Buffer) => {
    text += b.toString("utf8");
    for (const w of waiters.splice(0)) w();
  });
  const sent = () =>
    text
      .split("\n")
      .filter((l) => l !== "")
      .map((l) => JSON.parse(l) as Record<string, unknown>);
  const sentAtLeast = async (n: number) => {
    while (sent().length < n) await new Promise<void>((r) => waiters.push(r));
    return sent();
  };
  return {
    rpc: codexRpc({ stdin, stdout }),
    sent,
    sentAtLeast,
    feed: (c) => stdout.write(c),
    line: (msg) => stdout.write(`${JSON.stringify(msg)}\n`),
    eof: () => stdout.end(),
    destroy: () => stdout.destroy(),
    stdin,
  };
}

const run = <A, E>(e: Effect.Effect<A, E>) => Effect.runPromise(e);
const failure = async <A>(e: Effect.Effect<A, FinError>): Promise<FinError> => {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) throw new Error(`expected a failure, got ${JSON.stringify(exit.value)}`);
  const err = Cause.findErrorOption(exit.cause);
  if (err._tag !== "Some") throw new Error(`expected a typed failure: ${Cause.pretty(exit.cause)}`);
  return err.value;
};
/** The notifications until the stream ends: the values, and the error it failed with (if any). */
const drain = async (rpc: CodexRpc): Promise<{ items: ServerNotification[]; error: FinError | null }> => {
  const items: ServerNotification[] = [];
  const exit = await Effect.runPromiseExit(Stream.runForEach(rpc.notifications, (n) => Effect.sync(() => void items.push(n))));
  if (Exit.isSuccess(exit)) return { items, error: null };
  const err = Cause.findErrorOption(exit.cause);
  return { items, error: err._tag === "Some" ? err.value : null };
};

// ---- the decoders ----

describe("decoding", () => {
  it("decodes_every_used_message", () => {
    const seen = { result: new Set<string>(), notification: new Set<string>(), request: new Set<string>() };
    const lines = fixtureLines();
    expect(lines.length).toBeGreaterThan(80);
    for (const { file, line } of lines) {
      const raw = JSON.parse(line) as { _method?: string; id?: unknown; method?: string; result?: unknown; params?: unknown };
      const where = `${file}: ${line.slice(0, 120)}`;
      let decoded: unknown;
      if (raw._method !== undefined) {
        decoded = decodeResult(raw._method as keyof typeof CLIENT_REQUESTS, raw.result);
        seen.result.add(raw._method);
      } else if (raw.id !== undefined) {
        decoded = decodeServerRequest(raw.id as number, raw.method!, raw.params);
        expect((decoded as ServerRequest).method, where).toBe(raw.method);
        seen.request.add(raw.method!);
      } else {
        decoded = decodeNotification(raw.method!, raw.params);
        expect((decoded as ServerNotification).method, where).toBe(raw.method);
        seen.notification.add(raw.method!);
      }
      expect(isFin(decoded) ? `${decoded.message} ${JSON.stringify(decoded.details)}` : "ok", where).toBe("ok");
      expect(unknownsIn(decoded), where).toEqual([]);
      // the whole line through the router too (a response's `_method` is a fixture's note, ignored)
      const routed = decodeIncoming(line);
      expect(isFin(routed) ? routed.message : routed.kind, where).not.toBe("PROVIDER_PROTOCOL");
    }
    expect([...seen.result].sort()).toEqual(Object.keys(CLIENT_REQUESTS).sort());
    expect([...seen.notification].sort()).toEqual(Object.keys(SERVER_NOTIFICATIONS).sort());
    expect([...seen.request].sort()).toEqual(Object.keys(SERVER_REQUESTS).sort());
  });

  it("keeps_the_fields_the_adapter_reads", () => {
    const done = decodeNotification("turn/completed", turnCompleted({ status: "failed", error: { message: "no", codexErrorInfo: { httpConnectionFailed: { httpStatusCode: 401 } } } }));
    expect(done).toMatchObject({ method: "turn/completed", params: { turn: { status: "failed", error: { codexErrorInfo: { httpConnectionFailed: { httpStatusCode: 401 } } } } } });
    // the types narrow by method (a compile-time check as much as a run-time one)
    if (isFin(done) || done.method !== "turn/completed") throw new Error("not decoded");
    const status: TurnStatus = done.params.turn.status;
    expect(status).toBe("failed");
    const call = decodeServerRequest(5, "item/tool/call", { threadId: T, turnId: U, callId: "c", tool: "shell", arguments: { command: "ls" }, extra: 1 });
    expect(call).toEqual({ id: 5, method: "item/tool/call", params: { threadId: T, turnId: U, callId: "c", tool: "shell", arguments: { command: "ls" } } });
  });

  it("unknown_notification_is_unknown", () => {
    for (const method of ["thread/realtime/started", "item/autoApprovalReview/started", "brand/new/thing"]) {
      expect(decodeNotification(method, { anything: [1, 2] })).toEqual({ method: "unknown", name: method, params: { anything: [1, 2] } });
    }
    expect(decodeServerRequest(3, "currentTime/read", {})).toEqual({ id: 3, method: "unknown", name: "currentTime/read", params: {} });
  });

  it("unknown_enum_member_is_unknown_not_error", () => {
    // a new CodexErrorInfo value, string or object
    for (const info of ["quantumOverload", { gpuMelted: { retryAfterMs: 5 } }, { someNew: 1, withTwoKeys: 2 }]) {
      const n = decodeNotification("turn/completed", turnCompleted({ status: "failed", error: { message: "x", codexErrorInfo: info } }));
      expect(isFin(n) ? n.message : "decoded").toBe("decoded");
      const got = (n as { params: { turn: { error: { codexErrorInfo: unknown } } } }).params.turn.error.codexErrorInfo;
      expect(got).toBeInstanceOf(UnknownMember);
      expect((got as UnknownMember).name).toBe(typeof info === "string" ? info : Object.keys(info)[0]);
    }
    // a new ThreadItem type
    const item = decodeNotification("item/completed", { threadId: T, turnId: U, completedAtMs: 1, item: { type: "hologram", id: "h1", frames: 3 } });
    expect((item as { params: { item: unknown } }).params.item).toEqual(new UnknownMember("hologram"));
    // a new TurnStatus and a new plan-step status
    const st = decodeNotification("turn/completed", turnCompleted({ status: "paused" }));
    expect((st as { params: { turn: { status: unknown } } }).params.turn.status).toEqual(new UnknownMember("paused"));
    const plan = decodeNotification("turn/plan/updated", { threadId: T, turnId: U, plan: [{ step: "a", status: "skipped" }] });
    expect((plan as { params: { plan: { status: unknown }[] } }).params.plan[0]!.status).toEqual(new UnknownMember("skipped"));
    // a known member's broken body is not drift
    const broken = decodeNotification("item/completed", { threadId: T, turnId: U, completedAtMs: 1, item: { type: "agentMessage", id: "m" } });
    expect(isFin(broken) && broken.code).toBe("PROVIDER_PROTOCOL");
    const badInfo = decodeNotification("turn/completed", turnCompleted({ status: "failed", error: { message: "x", codexErrorInfo: { httpConnectionFailed: { httpStatusCode: "x" } } } }));
    expect(isFin(badInfo) && badInfo.code).toBe("PROVIDER_PROTOCOL");
    // a value of the wrong JSON type is not an enum member
    const num = decodeNotification("turn/completed", turnCompleted({ status: 7 }));
    expect(isFin(num) && num.code).toBe("PROVIDER_PROTOCOL");
  });

  it("missing_needed_field_is_protocol", () => {
    const noStatus = turnCompleted({});
    delete (noStatus.turn as Record<string, unknown>).status;
    const a = decodeNotification("turn/completed", noStatus);
    expect(isFin(a) && a.code).toBe("PROVIDER_PROTOCOL");
    expect(isFin(a) && (a.details as { method: string }).method).toBe("turn/completed");
    const b = decodeServerRequest(1, "item/tool/call", { threadId: T, turnId: U, tool: "shell", arguments: {} });
    expect(isFin(b) && b.code).toBe("PROVIDER_PROTOCOL");
    const c = decodeResult("thread/start", { model: "m" });
    expect(isFin(c) && c.code).toBe("PROVIDER_PROTOCOL");
    const d = decodeNotification("thread/tokenUsage/updated", { threadId: T, turnId: U, tokenUsage: { last: {}, total: {} } });
    expect(isFin(d) && d.code).toBe("PROVIDER_PROTOCOL");
    const e = decodeNotification("item/agentMessage/delta", undefined);
    expect(isFin(e) && e.code).toBe("PROVIDER_PROTOCOL");
  });

  it("routes_lines", () => {
    expect(decodeIncoming('{"id":3,"result":{"a":1}}')).toEqual({ kind: "response", id: 3, result: { a: 1 } });
    expect(decodeIncoming('{"error":{"code":-32600,"message":"Not initialized"},"id":1}')).toEqual({ kind: "error", id: 1, error: { code: -32600, message: "Not initialized" } });
    expect(decodeIncoming('{"method":"thread/closed","params":{"threadId":"t"},"emittedAtMs":1}')).toEqual({ kind: "notification", notification: { method: "thread/closed", params: { threadId: "t" } } });
    expect(decodeIncoming('{"id":"r1","method":"attestation/generate","params":{}}')).toMatchObject({ kind: "request", request: { id: "r1", method: "attestation/generate" } });
    for (const bad of ["not json", "[1,2]", "null", '{"id":true,"result":1}', '{"hello":1}', '{"id":1}', '{"method":5}']) {
      const r = decodeIncoming(bad);
      expect(isFin(r) && r.code, bad).toBe("PROVIDER_PROTOCOL");
    }
  });

  it("decode_never_throws", () => {
    const corpus = fixtureLines().map((l) => l.line);
    const methods = [...Object.keys(SERVER_NOTIFICATIONS), ...Object.keys(SERVER_REQUESTS), "x/y"];
    const check = (raw: string) => {
      const r = decodeIncoming(raw);
      expect(isFin(r) ? r.code : typeof r.kind).toBe(isFin(r) ? "PROVIDER_PROTOCOL" : "string");
    };
    fc.assert(fc.property(fc.string({ unit: "binary", maxLength: 256 }), check), { numRuns: 500 });
    fc.assert(fc.property(fc.uint8Array({ maxLength: 512 }), (b) => check(Buffer.from(b).toString("utf8"))), { numRuns: 300 });
    fc.assert(
      fc.property(fc.constantFrom(...methods), fc.jsonValue({ maxDepth: 4 }), fc.option(fc.oneof(fc.integer(), fc.string())), (method, params, id) =>
        check(JSON.stringify(id === null ? { method, params } : { id, method, params })),
      ),
      { numRuns: 1500 },
    );
    const mutate = (v: unknown, path: readonly number[], replacement: unknown): unknown => {
      if (path.length === 0 || v === null || typeof v !== "object") return replacement;
      const keys = Object.keys(v);
      if (keys.length === 0) return replacement;
      const k = keys[path[0]! % keys.length]!;
      const copy: Record<string, unknown> = Array.isArray(v) ? ([...v] as unknown as Record<string, unknown>) : { ...v };
      if (path.length === 1 && replacement === "__delete__") delete copy[k];
      else copy[k] = mutate((v as Record<string, unknown>)[k], path.slice(1), replacement);
      return copy;
    };
    fc.assert(
      fc.property(
        fc.constantFrom(...corpus),
        fc.array(fc.nat(), { minLength: 1, maxLength: 8 }),
        fc.oneof(fc.constant("__delete__"), fc.jsonValue({ maxDepth: 2 }), fc.constant(null), fc.constant({ type: "zzz" })),
        (line, path, replacement) => {
          const raw = JSON.parse(line) as Record<string, unknown>;
          delete raw._method;
          check(JSON.stringify(mutate(raw, path, replacement)));
          if ("result" in raw) {
            const method = (JSON.parse(line) as { _method: keyof typeof CLIENT_REQUESTS })._method;
            const r = decodeResult(method, mutate(raw.result, path, replacement));
            expect(isFin(r) ? r.code : "ok").toMatch(/^(ok|PROVIDER_PROTOCOL)$/);
          }
        },
      ),
      { numRuns: 2000 },
    );
    check(`{"method":"turn/completed","params":${"[".repeat(200_000)}}`);
    check(`${"[".repeat(1_000_000)}${"]".repeat(1_000_000)}`);
  });
});

// ---- the client ----

describe("client", () => {
  it("framing", async () => {
    const h = harness();
    const got = drain(h.rpc);
    h.feed('{"method":"thread/closed","params":{"threadId":"crlf"}}\r\n');
    // a line split across chunks, the split inside a multi-byte character
    const split = Buffer.from('{"method":"thread/name/updated","params":{"threadId":"t","threadName":"€uro"}}\n', "utf8");
    const at = split.indexOf(Buffer.from("€")) + 1;
    h.feed(split.subarray(0, 10));
    h.feed(split.subarray(10, at));
    h.feed(split.subarray(at));
    // two lines in one chunk, and blank lines
    h.feed('{"method":"thread/closed","params":{"threadId":"a"}}\n\n{"method":"thread/closed","params":{"threadId":"b"}}\n');
    // a line one byte over 16 MiB
    const pad = 16 * MiB - '{"method":"warning","params":{"message":""}}'.length + 1;
    h.feed(`{"method":"warning","params":{"message":"${"a".repeat(pad)}"}}\n`);
    const { items, error } = await got;
    expect(items).toEqual([
      { method: "thread/closed", params: { threadId: "crlf" } },
      { method: "thread/name/updated", params: { threadId: "t", threadName: "€uro" } },
      { method: "thread/closed", params: { threadId: "a" } },
      { method: "thread/closed", params: { threadId: "b" } },
    ]);
    expect(error?.code).toBe("PROVIDER_PROTOCOL");
    expect(error?.details).toMatchObject({ maxBytes: 16 * MiB });
  });

  it("framing_accepts_a_line_of_exactly_16_mib", async () => {
    const h = harness();
    const got = drain(h.rpc);
    const shell = '{"method":"warning","params":{"message":""}}';
    h.feed(`{"method":"warning","params":{"message":"${"a".repeat(16 * MiB - shell.length)}"}}\n`);
    h.eof();
    const { items, error } = await got;
    expect(error).toBeNull();
    expect(items).toHaveLength(1);
    expect(items[0]!.method).toBe("warning");
  });

  it("writes_requests_and_notifications", async () => {
    const h = harness();
    const pending = run(h.rpc.request("initialize", { clientInfo: { name: "finisher", version: "1.0.0" }, capabilities: { experimentalApi: true } }));
    await run(h.rpc.notify("initialized"));
    const req = run(h.rpc.request("configRequirements/read", undefined));
    const sent = await h.sentAtLeast(3);
    expect(sent[0]).toEqual({ id: 1, method: "initialize", params: { clientInfo: { name: "finisher", version: "1.0.0" }, capabilities: { experimentalApi: true } } });
    expect(sent[1]).toEqual({ method: "initialized" });
    expect(sent[2]).toEqual({ id: 2, method: "configRequirements/read" });
    h.line({ id: 1, result: { userAgent: "finisher/0.167.0", codexHome: "/p", platformFamily: "unix", platformOs: "linux" } });
    h.line({ id: 2, result: { supportsIndependentSpeedModes: true, requirements: null } });
    expect(await pending).toEqual({ userAgent: "finisher/0.167.0", codexHome: "/p", platformFamily: "unix", platformOs: "linux" });
    expect(await req).toEqual({ supportsIndependentSpeedModes: true, requirements: null });
  });

  it("responses_correlate_by_id", async () => {
    const h = harness();
    const a = run(h.rpc.request("turn/steer", { threadId: T, expectedTurnId: U, input: [{ type: "text", text: "a" }] }));
    const b = run(h.rpc.request("app/installed", {}));
    const c = run(h.rpc.request("account/read", {}));
    const sent = await h.sentAtLeast(3);
    const idOf = (m: string) => sent.find((s) => s.method === m)!.id;
    // answered last-first, with a stale id and a notification in between
    h.line({ id: idOf("account/read"), result: { account: null, requiresOpenaiAuth: true } });
    h.line({ method: "thread/closed", params: { threadId: T } });
    h.line({ id: idOf("app/installed"), result: { apps: [] } });
    h.line({ id: idOf("turn/steer"), result: { turnId: "steered" } });
    expect(await a).toEqual({ turnId: "steered" });
    expect(await b).toEqual({ apps: [] });
    expect(await c).toEqual({ account: null, requiresOpenaiAuth: true });
  });

  it("an_error_response_fails_its_request_only", async () => {
    const h = harness();
    const a = h.rpc.request("thread/resume", { threadId: T });
    const b = run(h.rpc.request("app/installed", {}));
    const fa = failure(a);
    const sent = await h.sentAtLeast(2);
    const idOf = (m: string) => sent.find((s) => s.method === m)!.id;
    h.line({ id: idOf("thread/resume"), error: { code: -32600, message: `no rollout found for thread id ${T}` } });
    h.line({ id: idOf("app/installed"), result: { apps: [] } });
    const e = await fa;
    expect(e.code).toBe("PROVIDER_PROTOCOL");
    expect(e.details).toEqual({ method: "thread/resume", reason: "rpcError", rpcError: { code: -32600, message: `no rollout found for thread id ${T}` } });
    expect(await b).toEqual({ apps: [] });
  });

  it("a_result_missing_a_needed_field_fails_its_request", async () => {
    const h = harness();
    const p = failure(h.rpc.request("thread/start", { cwd: "/w" }));
    const sent = await h.sentAtLeast(1);
    h.line({ id: sent[0]!.id, result: { model: "m" } });
    const e = await p;
    expect(e.code).toBe("PROVIDER_PROTOCOL");
    expect((e.details as { method: string }).method).toBe("thread/start");
  });

  it("eof_fails_pending_with_provider_exited", async () => {
    const h = harness();
    const got = drain(h.rpc);
    const p = failure(h.rpc.request("model/list", {}));
    const q = failure(h.rpc.request("thread/read", { threadId: T }));
    await h.sentAtLeast(2);
    h.line({ method: "thread/closed", params: { threadId: T } });
    h.eof();
    for (const [e, method] of [
      [await p, "model/list"],
      [await q, "thread/read"],
    ] as const) {
      expect(e.code).toBe("PROVIDER_EXITED");
      expect(e.details).toEqual({ method, reason: "eof" });
    }
    // the stream ends cleanly after what came before the end
    expect(await got).toEqual({ items: [{ method: "thread/closed", params: { threadId: T } }], error: null });
    // a request after the end fails at once, and writes nothing
    const before = h.sent().length;
    expect((await failure(h.rpc.request("account/read", {}))).code).toBe("PROVIDER_EXITED");
    expect(h.sent()).toHaveLength(before);
  });

  it("a_protocol_error_fails_pending_and_the_stream", async () => {
    const h = harness();
    const got = drain(h.rpc);
    const p = failure(h.rpc.request("model/list", {}));
    await h.sentAtLeast(1);
    h.line({ method: "turn/completed", params: { threadId: T, turn: { id: U, items: [] } } });
    const e = await p;
    expect(e.code).toBe("PROVIDER_PROTOCOL");
    expect((await got).error?.code).toBe("PROVIDER_PROTOCOL");
    expect((await failure(h.rpc.request("account/read", {}))).code).toBe("PROVIDER_PROTOCOL");
  });

  it("a_response_to_an_id_never_sent_is_protocol", async () => {
    const h = harness();
    const got = drain(h.rpc);
    h.line({ id: 99, result: {} });
    expect((await got).error?.code).toBe("PROVIDER_PROTOCOL");
  });

  it("an_interrupted_request_leaves_nothing_pending", async () => {
    const h = harness();
    const fiber = Effect.runFork(h.rpc.request("model/list", {}));
    const sent = await h.sentAtLeast(1);
    await run(Fiber.interrupt(fiber));
    // its late answer is dropped, not a protocol error
    const got = drain(h.rpc);
    h.line({ id: sent[0]!.id, result: { data: [], nextCursor: null } });
    h.line({ method: "thread/closed", params: { threadId: T } });
    h.eof();
    expect(await got).toEqual({ items: [{ method: "thread/closed", params: { threadId: T } }], error: null });
  });

  it("only_allowed_methods_sent", async () => {
    // the plan's allow-list (Decision 3), exactly: widening it is a review, not an edit
    expect([...ALLOWED_METHODS].sort()).toEqual(
      [
        "initialize", "thread/start", "thread/resume", "thread/revert", "thread/read", "turn/start", "turn/steer", "turn/interrupt",
        "model/list", "account/read", "config/read", "configRequirements/read", "skills/list", "hooks/list", "plugin/installed",
        "mcpServerStatus/list", "app/installed", "experimentalFeature/list",
      ].sort(),
    );
    const forbidden = /^(account\/(login|logout|gatewayOAuth|bedrock|gcp|rateLimitResetCredit)|accountSession\/|feedback\/upload|mcpServer\/oauth|plugin\/(reconcile|share)|fs\/|command\/exec|thread\/fork|config\/(value|batchWrite))/;
    for (const m of ALLOWED_METHODS) {
      expect(CLIENT_REQUEST_METHODS as readonly string[]).toContain(m);
      expect(m).not.toMatch(forbidden);
    }
    expect(CLIENT_REQUEST_METHODS.filter((m) => forbidden.test(m)).length).toBeGreaterThan(15);
    // every other method of the generated union is refused at `request`, and nothing is written
    const h = harness();
    const others = CLIENT_REQUEST_METHODS.filter((m) => !ALLOWED_METHODS.has(m));
    expect(others.length).toBe(CLIENT_REQUEST_METHODS.length - ALLOWED_METHODS.size);
    for (const m of others) {
      const e = await failure(h.rpc.request(m as never, {} as never));
      expect(e.code, m).toBe("PROVIDER_PROTOCOL");
      expect(e.details, m).toEqual({ method: m, reason: "methodNotAllowed" });
    }
    const strangers = ["account/login/start ", "Account/logout", "thread/rollback", ""];
    for (const m of strangers) expect((await failure(h.rpc.request(m as never, {} as never))).details).toMatchObject({ reason: "methodNotAllowed" });
    expect(h.sent()).toEqual([]);
  });

  it("no_other_method_literal_in_the_adapter", () => {
    const dir = join(REPO, "apps/finisherd/src/agent/codex");
    const refused = new Set<string>(CLIENT_REQUEST_METHODS.filter((m) => !ALLOWED_METHODS.has(m)));
    const hits: string[] = [];
    const files = readdirSync(dir, { recursive: true, withFileTypes: true }).filter((d) => d.isFile() && /\.ts$/.test(d.name));
    expect(files.length).toBeGreaterThan(1);
    for (const d of files) {
      const path = join(d.parentPath, d.name);
      if (path.includes("/generated/")) continue;
      // comments may name a refused method (rpc.ts's header does); code may not
      const text = readFileSync(path, "utf8")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/^\s*\/\/.*$/gm, "");
      for (const m of text.matchAll(/(["'`])([A-Za-z][A-Za-z0-9_]*(?:\/[A-Za-z0-9_]+)+)\1/g)) if (refused.has(m[2]!)) hits.push(`${path}: ${m[2]}`);
    }
    expect(hits).toEqual([]);
  });

  it("serves_server_requests_through_one_port", async () => {
    const h = harness();
    const asked: ServerRequest[] = [];
    h.rpc.serve({
      request: (r): Effect.Effect<ServerReply> => {
        asked.push(r);
        switch (r.method) {
          case "item/tool/call":
            return Effect.succeed({ method: "item/tool/call", result: { contentItems: [{ type: "inputText", text: "exit 0" }], success: true } });
          case "item/fileChange/requestApproval":
            return Effect.succeed({ method: "item/fileChange/requestApproval", result: { decision: "decline" } });
          case "account/chatgptAuthTokens/refresh":
            return Effect.succeed({ error: { code: -32601, message: "Finisher holds no tokens" } });
          case "attestation/generate":
            // a reply for another method is never written as this one's answer
            return Effect.succeed({ method: "item/fileChange/requestApproval", result: { decision: "decline" } });
          default:
            return Effect.succeed({ error: { code: -32601, message: `not handled: ${r.method === "unknown" ? r.name : r.method}` } });
        }
      },
    });
    h.line({ id: 0, method: "item/tool/call", params: { threadId: T, turnId: U, callId: "c1", tool: "shell", arguments: { command: "ls" } } });
    h.line({ id: "s1", method: "item/fileChange/requestApproval", params: { threadId: T, turnId: U, itemId: "fc", startedAtMs: 1 } });
    h.line({ id: 2, method: "account/chatgptAuthTokens/refresh", params: { reason: "unauthorized" } });
    h.line({ id: 3, method: "currentTime/read", params: {} });
    h.line({ id: 4, method: "attestation/generate", params: {} });
    const sent = await h.sentAtLeast(5);
    const byId = new Map(sent.map((s) => [s.id, s]));
    expect(byId.get(0)).toEqual({ id: 0, result: { contentItems: [{ type: "inputText", text: "exit 0" }], success: true } });
    expect(byId.get("s1")).toEqual({ id: "s1", result: { decision: "decline" } });
    expect(byId.get(2)).toEqual({ id: 2, error: { code: -32601, message: "Finisher holds no tokens" } });
    expect(byId.get(3)).toEqual({ id: 3, error: { code: -32601, message: "not handled: currentTime/read" } });
    expect(byId.get(4)).toMatchObject({ id: 4, error: { code: -32603 } });
    expect(asked.map((r) => r.method)).toEqual(["item/tool/call", "item/fileChange/requestApproval", "account/chatgptAuthTokens/refresh", "unknown", "attestation/generate"]);
  });

  it("holds_server_requests_until_served", async () => {
    const h = harness();
    h.line({ id: 7, method: "item/tool/call", params: { threadId: T, turnId: U, callId: "c", tool: "shell", arguments: {} } });
    await new Promise((r) => setTimeout(r, 20));
    expect(h.sent()).toEqual([]);
    h.rpc.serve({ request: () => Effect.succeed({ method: "item/tool/call", result: { contentItems: [], success: false } }) });
    expect(await h.sentAtLeast(1)).toEqual([{ id: 7, result: { contentItems: [], success: false } }]);
  });

  it("a_resolved_request_stops_its_handler", async () => {
    const h = harness();
    let interrupted = false;
    h.rpc.serve({ request: () => Effect.never.pipe(Effect.onInterrupt(() => Effect.sync(() => void (interrupted = true)))) });
    const got = drain(h.rpc);
    h.line({ id: 11, method: "item/fileChange/requestApproval", params: { threadId: T, turnId: U, itemId: "fc", startedAtMs: 1 } });
    await new Promise((r) => setTimeout(r, 20));
    h.line({ method: "serverRequest/resolved", params: { threadId: T, requestId: 11 } });
    h.eof();
    expect(await got).toEqual({ items: [{ method: "serverRequest/resolved", params: { threadId: T, requestId: 11 } }], error: null });
    await new Promise((r) => setTimeout(r, 20));
    expect(interrupted).toBe(true);
    expect(h.sent()).toEqual([]);
  });

  it("a_server_request_missing_a_needed_field_is_refused_and_protocol", async () => {
    const h = harness();
    const asked: ServerRequest[] = [];
    h.rpc.serve({ request: (r) => (asked.push(r), Effect.succeed({ error: { code: -32601, message: "x" } })) });
    const got = drain(h.rpc);
    h.line({ id: 1, method: "item/tool/call", params: { threadId: T, turnId: U, tool: "shell", arguments: {} } });
    expect((await got).error?.code).toBe("PROVIDER_PROTOCOL");
    expect(await h.sentAtLeast(1)).toEqual([{ id: 1, error: { code: -32602, message: expect.stringContaining("item/tool/call") } }]);
    expect(asked).toEqual([]);
  });
});

describe("client: exactly one answer, and every end", () => {
  const tick = (ms = 20) => new Promise((r) => setTimeout(r, ms));
  const approval = (id: number | string) => ({ id, method: "item/fileChange/requestApproval", params: { threadId: T, turnId: U, itemId: "fc", startedAtMs: 1 } });
  const decline: ServerReply = { method: "item/fileChange/requestApproval", result: { decision: "decline" } };

  it("stdout_close_without_end_is_eof", async () => {
    const h = harness();
    const got = drain(h.rpc);
    const p = failure(h.rpc.request("model/list", {}));
    await h.sentAtLeast(1);
    h.line({ method: "thread/closed", params: { threadId: T } });
    await tick();
    h.destroy();
    const e = await p;
    expect(e.code).toBe("PROVIDER_EXITED");
    expect(e.details).toEqual({ method: "model/list", reason: "eof" });
    expect(await got).toEqual({ items: [{ method: "thread/closed", params: { threadId: T } }], error: null });
  });

  it("notify_refuses_any_other_method_at_run_time", async () => {
    const h = harness();
    for (const m of ["account/logout", "initialize", "Initialized", ""]) {
      const e = await failure(h.rpc.notify(m as never));
      expect(e.code, m).toBe("PROVIDER_PROTOCOL");
      expect(e.details, m).toMatchObject({ reason: "methodNotAllowed" });
    }
    await tick();
    expect(h.sent()).toEqual([]);
    await run(h.rpc.notify("initialized"));
    expect(await h.sentAtLeast(1)).toEqual([{ method: "initialized" }]);
  });

  it("a_handler_that_fails_dies_or_interrupts_itself_is_answered_32603", async () => {
    const h = harness();
    h.rpc.serve({
      request: (r): Effect.Effect<ServerReply> =>
        r.id === 1 ? (Effect.fail(new Error("typed")) as never) : r.id === 2 ? Effect.die(new Error("defect")) : r.id === 3 ? Effect.interrupt : Effect.succeed(decline),
    });
    for (const id of [1, 2, 3, 4]) h.line(approval(id));
    const sent = await h.sentAtLeast(4);
    const byId = new Map(sent.map((s) => [s.id, s]));
    for (const id of [1, 2, 3]) expect(byId.get(id), String(id)).toEqual({ id, error: { code: -32603, message: expect.any(String) } });
    expect(byId.get(4)).toEqual({ id: 4, result: { decision: "decline" } });
    await tick();
    expect(h.sent()).toHaveLength(4);
  });

  it("a_resolved_request_stops_its_handler_before_any_end", async () => {
    const h = harness();
    let interrupted = false;
    h.rpc.serve({ request: () => Effect.never.pipe(Effect.onInterrupt(() => Effect.sync(() => void (interrupted = true)))) });
    h.line(approval(11));
    await tick();
    expect(interrupted).toBe(false);
    h.line({ method: "serverRequest/resolved", params: { threadId: T, requestId: 11 } });
    await tick();
    expect(interrupted).toBe(true);
    expect(h.sent()).toEqual([]);
    // the client is still open: a later request is served and answered
    h.rpc.serve({ request: () => Effect.succeed(decline) });
    h.line(approval(12));
    expect(await h.sentAtLeast(1)).toEqual([{ id: 12, result: { decision: "decline" } }]);
  });

  it("a_resolved_handler_that_still_succeeds_is_not_answered", async () => {
    const h = harness();
    h.rpc.serve({ request: () => Effect.uninterruptible(Effect.as(Effect.sleep("30 millis"), decline)) });
    h.line(approval(13));
    await tick(5);
    h.line({ method: "serverRequest/resolved", params: { threadId: T, requestId: 13 } });
    await tick(60);
    expect(h.sent()).toEqual([]);
  });

  it("a_held_request_resolved_before_serve_is_dropped", async () => {
    const h = harness();
    const asked: ServerRequest[] = [];
    h.line(approval(21));
    h.line(approval(22));
    h.line({ method: "serverRequest/resolved", params: { threadId: T, requestId: 21 } });
    await tick();
    h.rpc.serve({ request: (r) => (asked.push(r), Effect.succeed(decline)) });
    expect(await h.sentAtLeast(1)).toEqual([{ id: 22, result: { decision: "decline" } }]);
    await tick();
    expect(asked.map((r) => r.id)).toEqual([22]);
    expect(h.sent()).toHaveLength(1);
  });

  it("a_duplicate_id_while_open_is_protocol_and_never_answered_twice", async () => {
    const h = harness();
    let interrupted = 0;
    h.rpc.serve({ request: () => Effect.never.pipe(Effect.onInterrupt(() => Effect.sync(() => void interrupted++))) });
    const got = drain(h.rpc);
    h.line(approval(31));
    h.line(approval(31));
    const { error } = await got;
    expect(error?.code).toBe("PROVIDER_PROTOCOL");
    expect(error?.details).toMatchObject({ reason: "duplicateRequestId" });
    await tick();
    expect(interrupted).toBe(1);
    expect(h.sent()).toEqual([]);
    // an id reused once its request was answered is a new request
    const h2 = harness();
    h2.rpc.serve({ request: () => Effect.succeed(decline) });
    h2.line(approval(41));
    await h2.sentAtLeast(1);
    h2.line(approval(41));
    expect(await h2.sentAtLeast(2)).toEqual([
      { id: 41, result: { decision: "decline" } },
      { id: 41, result: { decision: "decline" } },
    ]);
  });

  it("a_stop_interrupts_handlers_and_writes_nothing_after", async () => {
    const h = harness();
    let interrupted = false;
    h.rpc.serve({
      request: (r) =>
        r.id === 51
          ? Effect.never.pipe(Effect.onInterrupt(() => Effect.sync(() => void (interrupted = true))))
          : Effect.uninterruptible(Effect.as(Effect.sleep("30 millis"), decline)),
    });
    const got = drain(h.rpc);
    h.line(approval(51));
    h.line(approval(52));
    await tick(5);
    h.feed("not json\n");
    expect((await got).error?.code).toBe("PROVIDER_PROTOCOL");
    await tick(60);
    expect(interrupted).toBe(true);
    expect(h.sent()).toEqual([]);
  });

  it("an_eof_writes_no_answer_for_interrupted_handlers", async () => {
    const h = harness();
    h.rpc.serve({ request: () => Effect.never });
    h.line(approval(61));
    await tick(5);
    h.eof();
    await tick();
    expect(h.sent()).toEqual([]);
  });

  it("end_closes_stdin_and_later_requests_fail_stdin_closed", async () => {
    const h = harness();
    let finished = false;
    let writes = 0;
    const write = h.stdin.write.bind(h.stdin) as (...a: unknown[]) => boolean;
    h.stdin.write = ((...a: unknown[]) => (writes++, write(...a))) as never;
    h.stdin.on("finish", () => (finished = true));
    h.rpc.end();
    await tick();
    expect(finished).toBe(true);
    const e = await failure(h.rpc.request("account/read", {}));
    expect(e.code).toBe("PROVIDER_EXITED");
    expect(e.details).toEqual({ method: "account/read", reason: "stdinClosed" });
    await run(h.rpc.notify("initialized"));
    await tick();
    // refused before any write: nothing is even tried after the end
    expect(writes).toBe(0);
    expect(h.sent()).toEqual([]);
  });

  it("a_failed_write_fails_its_request_at_once", async () => {
    const stdout = new PassThrough();
    const stdin = new Writable({ write: (_c, _e, cb) => cb(new Error("EPIPE")) });
    const rpc = codexRpc({ stdin, stdout });
    const e = await failure(rpc.request("account/read", {}));
    expect(e.code).toBe("PROVIDER_EXITED");
    expect(e.details).toEqual({ method: "account/read", reason: "stdinClosed" });
  });

  it("a_schema_min_length_is_decoded", () => {
    // `ReasoningEffort` is `minLength: 1` in the pin: the generator keeps the constraint
    expect(Schema.decodeUnknownResult(ReasoningEffort)("high")._tag).toBe("Success");
    expect(Schema.decodeUnknownResult(ReasoningEffort)("")._tag).toBe("Failure");
  });

  it("an_error_response_for_no_request_says_so", async () => {
    const h = harness();
    const got = drain(h.rpc);
    h.line({ id: null, error: { code: -32700, message: "Parse error" } });
    const { error } = await got;
    expect(error?.code).toBe("PROVIDER_PROTOCOL");
    expect(error?.message).toMatch(/no request/);
    expect(error?.message).not.toMatch(/never sent/);
    expect(error?.details).toMatchObject({ rpcError: { code: -32700, message: "Parse error" } });
  });
});

// ---- the gate ----

describe("generated", () => {
  it("generated_file_is_current", () => {
    const r = spawnSync(process.execPath, [join(REPO, "scripts/check-codex-protocol.mjs"), "--root", REPO], { encoding: "utf8" });
    expect(`${r.stdout}${r.stderr}`.trim()).toBe("");
    expect(r.status).toBe(0);
  });
});
