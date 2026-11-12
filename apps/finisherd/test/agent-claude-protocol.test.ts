// Claude Code's stream-JSON and control-protocol codec (plan 7D Task 2).
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { FinError } from "@finisher/contracts";
import fc from "fast-check";
import { describe, expect, it, test } from "vitest";
import {
  type CliLine,
  decodeLine,
  encodeAnswer,
  encodeControl,
  encodeUser,
  lineFramer,
  MAX_LINE_BYTES,
} from "../src/agent/claude/protocol.ts";

const fixture = (name: string): string[] =>
  readFileSync(new URL(`./fixtures/claude/${name}.jsonl`, import.meta.url), "utf8")
    .split("\n")
    .filter((l) => l !== "");
const objects = (name: string) => fixture(name).map((l) => JSON.parse(l) as Record<string, unknown>);

const ok = (raw: string | object): CliLine => {
  const r = decodeLine(typeof raw === "string" ? raw : JSON.stringify(raw));
  if (r instanceof FinError) throw new Error(`expected a line, got ${r.code}: ${r.message} ${JSON.stringify(r.details)}`);
  return r;
};
const protocolError = (raw: string | object) => {
  const r = decodeLine(typeof raw === "string" ? raw : JSON.stringify(raw));
  expect(r).toBeInstanceOf(FinError);
  expect((r as FinError).code).toBe("PROVIDER_PROTOCOL");
  return r as FinError;
};
const without = (o: Record<string, unknown>, path: string[]): Record<string, unknown> => {
  const copy = structuredClone(o);
  let at: Record<string, unknown> = copy;
  for (const k of path.slice(0, -1)) at = at[k] as Record<string, unknown>;
  delete at[path.at(-1)!];
  return copy;
};
const byKind = <K extends CliLine["kind"]>(lines: CliLine[], kind: K) => lines.filter((l): l is Extract<CliLine, { kind: K }> => l.kind === kind);

describe("decoding", () => {
  it("decodes_every_known_line", () => {
    const lines = [...fixture("session"), ...fixture("control")].map(ok);
    expect(lines.every((l) => l.kind !== "unknown")).toBe(true);

    const [init] = byKind(lines, "init");
    expect(init).toMatchObject({ session_id: "01890a5d-ac96-774b-bcce-b302099a8057", model: "test-claude-big", permissionMode: "default", apiKeySource: "none" });
    expect(init!.tools).toContain("mcp__finisher__shell");
    expect(init!.memory_paths).toBeNull();
    expect(init!.plugins).toEqual([{ name: "cc-plugin-example@builtin", path: "builtin", source: "cc-plugin-example@builtin" }]);
    expect(init!.capabilities).toEqual(["interrupt_receipt_v1"]);

    const [assistant] = byKind(lines, "assistant");
    expect(assistant!.message.content).toEqual([
      { type: "thinking", thinking: "The timeline needs a trim.", signature: "c2lnbmF0dXJl" },
      { type: "text", text: "I will read the cut list." },
      { type: "tool_use", id: "toolu_01", name: "Read", input: { file_path: "/srv/finisher/workspace/s1/cuts.txt" } },
    ]);
    expect(assistant!.parent_tool_use_id).toBeNull();
    expect(assistant!.message.usage).toEqual({ input_tokens: 12, cache_creation_input_tokens: 0, cache_read_input_tokens: 3400, output_tokens: 87 });

    const users = byKind(lines, "user");
    expect(users.map((u) => u.message.content)).toEqual([
      [{ type: "tool_result", tool_use_id: "toolu_01", content: "00:00:01.000 00:00:04.500\n", is_error: false }],
      [{ type: "tool_result", tool_use_id: "toolu_02", content: [{ type: "text", text: "permission denied" }], is_error: true }],
    ]);

    const events = byKind(lines, "stream").map((s) => s.event);
    expect(events.map((e) => e.type)).toEqual([
      "message_start",
      "content_block_start",
      "content_block_delta",
      "content_block_delta",
      "content_block_stop",
      "content_block_delta",
      "content_block_delta",
      "message_delta",
      "message_stop",
    ]);
    expect(events.flatMap((e) => (e.type === "content_block_delta" ? [e.delta] : []))).toEqual([
      { type: "thinking_delta", thinking: "The timeline needs a trim." },
      { type: "signature_delta", signature: "c2lnbmF0dXJl" },
      { type: "text_delta", text: "I will read the cut list." },
      { type: "input_json_delta", partial_json: '{"file_path":' },
    ]);
    expect(events[0]).toMatchObject({ type: "message_start", message: { usage: { input_tokens: 12, cache_read_input_tokens: 3400 } } });
    expect(events[7]).toEqual({ type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 87 } });

    const [success, failed] = byKind(lines, "result");
    expect(success).toMatchObject({ subtype: "success", is_error: false, total_cost_usd: 0.0213, queued_turn_count: 0, stop_reason: "end_turn" });
    expect(success!.usage).toEqual({ input_tokens: 24, cache_creation_input_tokens: 0, cache_read_input_tokens: 6800, output_tokens: 160 });
    expect(success!.modelUsage).toEqual({ "test-claude-big": { inputTokens: 24, outputTokens: 160, cacheReadInputTokens: 6800, cacheCreationInputTokens: 0, costUSD: 0.0213 } });
    expect(failed).toMatchObject({ is_error: true, result: "Not logged in · Please run /login", total_cost_usd: 0, api_error_status: null });

    expect(byKind(lines, "apiRetry")).toEqual([{ kind: "apiRetry", attempt: 1, max_retries: 10, retry_delay_ms: 1200, error_status: 529, error: "overloaded_error" }]);
    expect(byKind(lines, "compactBoundary")).toEqual([{ kind: "compactBoundary", compact_metadata: { trigger: "auto", pre_tokens: 158000 } }]);

    const responses = byKind(lines, "controlResponse");
    expect(responses[0]!.response).toMatchObject({ subtype: "success", request_id: "init-1" });
    expect((responses[0]!.response as { response: { models: unknown[] } }).response.models).toHaveLength(1);
    expect(responses[1]!.response).toEqual({ subtype: "error", request_id: "req-7", error: "Unknown request subtype" });

    const requests = byKind(lines, "controlRequest");
    expect(requests.map((r) => [r.request_id, r.request.subtype])).toEqual([
      ["cli-1", "can_use_tool"],
      ["cli-2", "mcp_message"],
      ["cli-3", "hook_callback"],
    ]);
    expect(requests[0]!.request).toMatchObject({ tool_name: "Bash", input: { command: "git status", description: "Show the status" }, tool_use_id: "toolu_03", blocked_path: null });
    expect(requests[1]!.request).toEqual({ subtype: "mcp_message", server_name: "finisher", message: { jsonrpc: "2.0", id: 3, method: "tools/list" } });
    expect(byKind(lines, "controlCancel")).toEqual([{ kind: "controlCancel", request_id: "cli-1" }]);
  });

  it("unknown_type_is_unknown_not_error", () => {
    const lines = fixture("unknown").map(ok);
    expect(lines[0]).toEqual({ kind: "unknown", type: "keep_alive" });
    expect(lines[1]).toEqual({ kind: "unknown", type: "rate_limit_event" });
    expect(lines[2]).toEqual({ kind: "system", subtype: "status" });
    expect(lines[3]).toEqual({ kind: "system", subtype: "permission_denied" });
    expect(lines[4]).toEqual({ kind: "controlRequest", request_id: "cli-9", request: { subtype: "unknown", name: "elicitation" } });
    expect((lines[5] as Extract<CliLine, { kind: "assistant" }>).message.content).toEqual([{ type: "unknown", name: "server_tool_use" }]);
    expect((lines[6] as Extract<CliLine, { kind: "stream" }>).event).toEqual({ type: "content_block_delta", index: 0, delta: { type: "unknown", name: "citations_delta" } });
    expect((lines[7] as Extract<CliLine, { kind: "stream" }>).event).toEqual({ type: "unknown", name: "ping" });
    // A name is kept short: the mapper puts it in a notice.
    expect(ok({ type: "x".repeat(500) })).toEqual({ kind: "unknown", type: "x".repeat(64) });
  });

  it("known_type_missing_needed_field_is_protocol_error", () => {
    const [init] = objects("session");
    const result = objects("session").find((o) => o.type === "result")!;
    const canUse = objects("control").find((o) => (o.request as { subtype?: string } | undefined)?.subtype === "can_use_tool")!;
    const assistant = objects("session").find((o) => o.type === "assistant")!;
    expect(protocolError(without(init!, ["tools"])).details).toMatchObject({ what: "system/init" });
    expect(protocolError(without(result, ["usage"])).details).toMatchObject({ what: "result" });
    expect(protocolError(without(canUse, ["request", "tool_name"])).details).toMatchObject({ what: "control_request can_use_tool" });
    // More of the same rule: a field the adapter reads, gone or of the wrong type.
    protocolError(without(init!, ["session_id"]));
    protocolError(without(result, ["total_cost_usd"]));
    protocolError(without(canUse, ["request_id"]));
    protocolError({ ...init!, tools: "Read" });
    protocolError({ ...assistant, message: { content: [{ type: "tool_use", name: "Read", input: {} }] } });
    protocolError({ type: "control_response", response: { subtype: "error", request_id: "r" } });
    protocolError({ type: "control_cancel_request" });
    protocolError({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta" } } });
    protocolError({ type: "system" });
    // Not a line at all.
    protocolError("not json");
    protocolError("[1,2]");
    protocolError("null");
    protocolError({ no: "type" });
  });

  it("extra_fields_are_ignored", () => {
    const grow = (o: Record<string, unknown>) => ({ ...o, a_newer_field: { nested: [1, 2, 3] }, another: "x" });
    for (const o of [...objects("session"), ...objects("control")]) {
      expect(ok(grow(o))).toEqual(ok(o));
    }
    const line = ok(grow(objects("control")[2]!)) as Extract<CliLine, { kind: "controlRequest" }>;
    expect(Object.keys(line)).toEqual(["kind", "request_id", "request"]);
    expect(line.request).not.toHaveProperty("another");
    // Extra fields are dropped, so nothing a newer CLI adds reaches the runtime unchecked.
    expect(ok(objects("session")[0]!)).not.toHaveProperty("uuid");
  });

  it("line_over_cap_is_protocol_error", () => {
    const pad = (n: number) => JSON.stringify({ type: "keep_alive", pad: "a".repeat(n) });
    const at = pad(0).length;
    expect(ok(pad(MAX_LINE_BYTES - at))).toEqual({ kind: "unknown", type: "keep_alive" });
    expect(protocolError(pad(MAX_LINE_BYTES - at + 1)).details).toEqual({ maxBytes: MAX_LINE_BYTES });
    // Bytes, not UTF-16 units: three-byte characters cross the cap at a third of the length.
    expect(protocolError(pad(Math.ceil(MAX_LINE_BYTES / 3) + 1).replace(/a/g, "€")).code).toBe("PROVIDER_PROTOCOL");
  });
});

describe("framing", () => {
  const drain = (chunks: (string | Uint8Array)[], cap?: number) => {
    const f = lineFramer(cap);
    return [...chunks.flatMap((c) => f.push(c)), ...f.end()];
  };

  it("framing_splits_on_newline_only", () => {
    const a = JSON.stringify({ type: "keep_alive", text: "line separator paragraph\rreturn" });
    expect(a).toContain(" ");
    expect(drain([`${a}\n`])).toEqual([a]);

    // A partial line across chunks, a UTF-8 sequence split between two of them, CRLF, blank lines.
    const b = JSON.stringify({ type: "keep_alive", s: "€ü日本" });
    const bytes = Buffer.from(`${a}\r\n\n${b}\r\n`, "utf8");
    const cut = bytes.indexOf(Buffer.from("€")) + 1;
    expect(drain([bytes.subarray(0, 5), bytes.subarray(5, cut), bytes.subarray(cut)])).toEqual([a, b]);
    for (let i = 0; i <= bytes.length; i++) expect(drain([bytes.subarray(0, i), bytes.subarray(i)])).toEqual([a, b]);

    // A last line without its newline is delivered at the end.
    expect(drain(["{\"type\":\"x\"}\n{\"type\":", "\"y\"}"])).toEqual(['{"type":"x"}', '{"type":"y"}']);

    // The cap holds while buffering: an endless line is one error, later lines still arrive.
    const f = lineFramer(16);
    const out: unknown[] = [];
    for (let i = 0; i < 1000; i++) out.push(...f.push("x".repeat(64)));
    out.push(...f.push("still the long one\nshort\n"), ...f.end());
    expect(out.length).toBe(2);
    expect((out[0] as FinError).code).toBe("PROVIDER_PROTOCOL");
    expect(out[1]).toBe("short");
    // Exactly at the cap is fine, with or without a CR.
    expect(drain(["0123456789abcdef\n0123456789abcdef\r\n0123456789abcdefg\n"], 16).map((x) => (typeof x === "string" ? x : x.code))).toEqual([
      "0123456789abcdef",
      "0123456789abcdef",
      "PROVIDER_PROTOCOL",
    ]);
    expect(drain(["0123456789abcdef\r"], 16)).toEqual(["0123456789abcdef"]);
    expect(drain(["0123456789abcdefg"], 16).map((x) => (typeof x === "string" ? x : x.code))).toEqual(["PROVIDER_PROTOCOL"]);
  });
});

describe("encoding", () => {
  it("encodes host lines exactly", () => {
    const user = encodeUser(["budget: 1000 tokens left", "Trim the first clip\nplease"], "01890a5d-ac96-774b-bcce-b302099a8057");
    expect(user.endsWith("\n")).toBe(true);
    expect(user.slice(0, -1)).not.toContain("\n");
    expect(JSON.parse(user)).toEqual({
      type: "user",
      message: {
        role: "user",
        content: [
          { type: "text", text: "budget: 1000 tokens left" },
          { type: "text", text: "Trim the first clip\nplease" },
        ],
      },
      parent_tool_use_id: null,
      session_id: "01890a5d-ac96-774b-bcce-b302099a8057",
    });
    expect(JSON.parse(encodeControl("r1", { subtype: "initialize" }))).toEqual({ type: "control_request", request_id: "r1", request: { subtype: "initialize", hooks: null } });
    expect(JSON.parse(encodeControl("r2", { subtype: "interrupt" }))).toEqual({ type: "control_request", request_id: "r2", request: { subtype: "interrupt" } });
    expect(JSON.parse(encodeControl("r3", { subtype: "set_permission_mode", mode: "plan" }))).toEqual({
      type: "control_request",
      request_id: "r3",
      request: { subtype: "set_permission_mode", mode: "plan" },
    });

    const answer = (a: Parameters<typeof encodeAnswer>[1]) => JSON.parse(encodeAnswer("cli-1", a)) as unknown;
    const input = { command: "git status" };
    // An allow that carries `updatedPermissions` (a caller's slip) never sends it.
    expect(answer({ behavior: "allow", updatedInput: input, updatedPermissions: [{ type: "addRules" }] } as never)).toEqual({
      type: "control_response",
      response: { subtype: "success", request_id: "cli-1", response: { behavior: "allow", updatedInput: input } },
    });
    expect(answer({ behavior: "deny", message: "denied by rule tool:Bash" })).toEqual({
      type: "control_response",
      response: { subtype: "success", request_id: "cli-1", response: { behavior: "deny", message: "denied by rule tool:Bash" } },
    });
    expect(answer({ behavior: "deny", message: "stop", interrupt: true })).toMatchObject({ response: { response: { behavior: "deny", message: "stop", interrupt: true } } });
    expect(answer({ mcp_response: { jsonrpc: "2.0", id: 3, result: { tools: [] } } })).toEqual({
      type: "control_response",
      response: { subtype: "success", request_id: "cli-1", response: { mcp_response: { jsonrpc: "2.0", id: 3, result: { tools: [] } } } },
    });
    expect(answer({ error: "hook callbacks are not supported" })).toEqual({
      type: "control_response",
      response: { subtype: "error", request_id: "cli-1", error: "hook callbacks are not supported" },
    });
  });

  const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
  /** CI (`FIN_REQUIRE_SERVER=1`, as the other Rust-binary tests): a missing fake fails instead of skipping. */
  const REQUIRE_FAKE = process.env.FIN_REQUIRE_SERVER === "1";

  test.skipIf(!existsSync(FAKE) && !REQUIRE_FAKE)("encode_user_round_trips_through_the_fake", async () => {
    if (!existsSync(FAKE)) throw new Error(`FIN_REQUIRE_SERVER=1 but ${FAKE} is missing: run \`cargo build -p fin-agent-fakes\``);
    const uuid = "01890a5d-ac96-774b-bcce-b302099a8057";
    const config = mkdtempSync(join(tmpdir(), "fin-claude-proto-"));
    try {
      const child = spawn(FAKE, ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--permission-mode", "manual", "--session-id", uuid], {
        cwd: config,
        env: { PATH: process.env.PATH ?? "", CLAUDE_CONFIG_DIR: config },
        stdio: ["pipe", "pipe", "pipe"],
      });
      const framer = lineFramer();
      const lines: CliLine[] = [];
      const waiters: { pred: (l: CliLine) => boolean; resolve: (l: CliLine) => void }[] = [];
      child.stdout.on("data", (chunk: Buffer) => {
        for (const raw of framer.push(chunk)) {
          const l = typeof raw === "string" ? decodeLine(raw) : raw;
          if (l instanceof FinError) throw new Error(`fake wrote a bad line: ${l.message}`);
          lines.push(l);
          for (const w of waiters.splice(0).filter((w) => !w.pred(l) || (w.resolve(l), false))) waiters.push(w);
        }
      });
      const next = (pred: (l: CliLine) => boolean) =>
        new Promise<CliLine>((resolve, reject) => {
          const seen = lines.find(pred);
          if (seen) return resolve(seen);
          waiters.push({ pred, resolve });
          setTimeout(() => reject(new Error(`timed out; lines so far: ${JSON.stringify(lines)}`)), 10_000).unref();
        });
      const exited = new Promise<number | null>((resolve) => child.on("exit", (code) => resolve(code)));

      child.stdin.write(encodeControl("init-1", { subtype: "initialize" }));
      const init = await next((l) => l.kind === "controlResponse");
      expect(init).toMatchObject({ kind: "controlResponse", response: { subtype: "success", request_id: "init-1" } });
      expect(lines.some((l) => l.kind === "init")).toBe(false);

      const user = encodeUser(["facts: none", "Trim the first clip"], uuid);
      child.stdin.write(user);
      expect(await next((l) => l.kind === "init")).toMatchObject({ kind: "init", session_id: uuid });
      expect(await next((l) => l.kind === "result")).toMatchObject({ kind: "result", is_error: false });

      child.stdin.end();
      expect(await exited).toBe(0);
      // The fake keeps the user line in its transcript as it parsed it: the same JSON value.
      const projects = join(config, "projects");
      const [dir] = readdirSync(projects);
      const transcript = readFileSync(join(projects, dir!, `${uuid}.jsonl`), "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l) as unknown);
      expect(transcript).toContainEqual(JSON.parse(user));
    } finally {
      rmSync(config, { recursive: true, force: true });
    }
  });
});

describe("robustness", () => {
  const corpus = [...fixture("session"), ...fixture("control"), ...fixture("unknown")];

  /** A JSON value with one field removed, retyped or replaced, anywhere in the tree. */
  const mutate = (v: unknown, path: number[], replacement: unknown): unknown => {
    if (path.length === 0 || typeof v !== "object" || v === null) return replacement;
    const keys = Object.keys(v);
    if (keys.length === 0) return replacement;
    const k = keys[path[0]! % keys.length]!;
    const copy: Record<string, unknown> = Array.isArray(v) ? ([...v] as unknown as Record<string, unknown>) : { ...(v as Record<string, unknown>) };
    if (path.length === 1 && replacement === "__delete__") delete copy[k];
    else copy[k] = mutate((v as Record<string, unknown>)[k], path.slice(1), replacement);
    return copy;
  };

  it("decode_never_throws", () => {
    const check = (raw: string) => {
      const r = decodeLine(raw);
      expect(r instanceof FinError ? r.code : typeof r.kind).toBe(r instanceof FinError ? "PROVIDER_PROTOCOL" : "string");
    };
    fc.assert(
      fc.property(fc.uint8Array({ maxLength: 512 }), (bytes) => check(Buffer.from(bytes).toString("utf8"))),
      { numRuns: 500 },
    );
    fc.assert(fc.property(fc.string({ unit: "binary", maxLength: 256 }), check), { numRuns: 500 });
    fc.assert(
      fc.property(
        fc.constantFrom(...corpus),
        fc.array(fc.nat(), { minLength: 1, maxLength: 6 }),
        fc.oneof(fc.constant("__delete__"), fc.jsonValue({ maxDepth: 2 }), fc.constant(null), fc.constant({ type: "text" })),
        (line, path, replacement) => check(JSON.stringify(mutate(JSON.parse(line), path, replacement))),
      ),
      { numRuns: 3000 },
    );
    // Truncated and spliced fixture lines.
    fc.assert(
      fc.property(fc.constantFrom(...corpus), fc.nat(), fc.string({ maxLength: 8 }), (line, at, junk) => check(line.slice(0, at % line.length) + junk + line.slice(at % line.length))),
      { numRuns: 2000 },
    );
    // Deep nesting cannot overflow anything into a throw.
    check(`{"type":"assistant","message":{"content":${"[".repeat(200_000)}}}`);
    check(`${"[".repeat(1_000_000)}${"]".repeat(1_000_000)}`);
  });
});
