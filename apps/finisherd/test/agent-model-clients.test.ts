// The built-in agent's OpenAI-compatible client against the fake model server (plan 7A Task 9).
import { afterEach, describe, expect, it } from "vitest";
import { blake3 } from "../src/agent/blake3.ts";
import {
  checkBaseUrl,
  type KeySource,
  localClient,
  type ModelClient,
  type ModelResult,
  type ModelStreamEvent,
  noKey,
  openrouterClient,
  retryAfterMs,
} from "../src/agent/builtin/compat.ts";
import { chatBody, type ModelRequest } from "../src/agent/builtin/request.ts";
import { manualClock } from "../src/agent/clock.ts";
import { type FakeModelServer, type FakeResponse, sse, startFakeModelServer } from "./support/fake-model-server.ts";

const KEY = "sk-or-v1-test-0000000000000000";
const key: KeySource = (f) => f(KEY);

const req = (over: Partial<ModelRequest> = {}): ModelRequest => ({
  system: "You edit video with fin.",
  tools: [{ name: "fin", description: "Run fin.", parameters: { type: "object", properties: { argv: { type: "array", items: { type: "string" } } }, required: ["argv"] } }],
  messages: [{ role: "user", content: [{ type: "text", text: "tighten the intro" }] }],
  model: "test/closed-explicit",
  maxTokens: 1000,
  ...over,
});

let server: FakeModelServer | undefined;
afterEach(async () => {
  await server?.close();
  server = undefined;
});

async function serve(...scripts: FakeResponse[]) {
  server = await startFakeModelServer(scripts);
  return server;
}

const sleeps: number[] = [];
const fakeSleep = async (ms: number) => {
  sleeps.push(ms);
};

function client(s: FakeModelServer, caching: "explicit" | "automatic" | "none" = "explicit"): ModelClient {
  sleeps.length = 0;
  return openrouterClient(s.baseUrl, { cachingFor: () => caching, sleep: fakeSleep, clock: manualClock() });
}

async function collect(c: ModelClient, r: ModelRequest, k: KeySource = key): Promise<{ events: ModelStreamEvent[]; result: ModelResult }> {
  const events: ModelStreamEvent[] = [];
  for await (const e of c.stream(r, k)) events.push(e);
  const done = events.at(-1);
  if (done?.type !== "done") throw new Error("no result");
  return { events, result: done.result };
}

async function failure(c: ModelClient, r: ModelRequest): Promise<{ code: string; details: unknown; events: ModelStreamEvent[] }> {
  const events: ModelStreamEvent[] = [];
  try {
    for await (const e of c.stream(r, key)) events.push(e);
  } catch (e) {
    return { code: (e as { code: string }).code, details: (e as { details: unknown }).details, events };
  }
  throw new Error("no failure");
}

describe("OpenRouter preset", () => {
  it("streams text, ends the turn and reports usage, cost and the upstream provider", async () => {
    const s = await serve({ sse: sse("text") });
    const { events, result } = await collect(client(s), req());
    expect(events.filter((e) => e.type === "textDelta").map((e) => (e as { text: string }).text)).toEqual(["The intro ", "runs long."]);
    expect(result).toMatchObject({ text: "The intro runs long.", stopReason: "endTurn", toolCalls: [], upstream: "Anthropic", generationId: "gen-text-1" });
    // prompt_tokens includes the cache reads and writes; input does not.
    expect(result.usage).toEqual({ input: 500, output: 30, cacheRead: 4000, cacheWrite: 700, reasoning: 0 });
    expect(result.costMicros).toBe(4215);
    const sent = s.requests()[0]!;
    expect(sent.headers.authorization).toBe(`Bearer ${KEY}`);
    expect(Object.keys(sent.headers).filter((h) => h.startsWith("x-stainless"))).toEqual([]);
  });

  it("assembles a tool call split across deltas", async () => {
    const s = await serve({ sse: sse("tool-split") });
    const { result } = await collect(client(s), req());
    expect(result.stopReason).toBe("toolUse");
    expect(result.toolCalls).toEqual([{ id: "call_a1", name: "fin", arguments: '{"argv":["item","trim","c1","--end","-1s"]}' }]);
    expect(JSON.parse(result.toolCalls[0]!.arguments)).toEqual({ argv: ["item", "trim", "c1", "--end", "-1s"] });
  });

  it("keeps two parallel tool calls apart", async () => {
    const s = await serve({ sse: sse("parallel-tools") });
    const { result } = await collect(client(s), req());
    expect(result.toolCalls).toEqual([
      { id: "call_r1", name: "workspace_read", arguments: '{"path":"notes.md"}' },
      { id: "call_r2", name: "workspace_list", arguments: "{}" },
    ]);
    expect(result.usage).toEqual({ input: 400, output: 20, cacheRead: 0, cacheWrite: 0, reasoning: 0 });
  });

  it("counts reasoning tokens and sends the reasoning details back unchanged on the next call", async () => {
    const s = await serve({ sse: sse("reasoning") }, { sse: sse("text") });
    const c = client(s);
    const r1 = req({ reasoning: { effort: "high" } });
    const { result, events } = await collect(c, r1);
    expect(result.usage.reasoning).toBe(90);
    expect(result.reasoningText).toBe("The intro is 4 s.");
    expect(events.filter((e) => e.type === "reasoningDelta")).toHaveLength(2);
    const details = [
      { type: "reasoning.text", text: "The intro is 4 s.", format: "anthropic-claude-v1", index: 0, signature: "c2lnbmF0dXJl" },
      { type: "reasoning.encrypted", data: "ZW5jcnlwdGVk", format: "anthropic-claude-v1", index: 1 },
    ];
    expect(result.reasoning).toEqual(details);
    expect(s.requests()[0]!.json.reasoning).toEqual({ effort: "high" });
    const r2 = req({
      messages: [...r1.messages, { role: "assistant", text: result.text, reasoning: result.reasoning }, { role: "user", content: [{ type: "text", text: "go on" }] }],
    });
    await collect(c, r2);
    const assistant = (s.requests()[1]!.json.messages as Record<string, unknown>[])[2]!;
    expect(assistant).toEqual({ role: "assistant", content: "Trim it.", reasoning_details: details });
  });

  it("places cache_control breakpoints for an explicit-caching model only", async () => {
    const s = await serve({ sse: sse("text") }, { sse: sse("text") });
    const history = req({
      messages: [
        { role: "user", content: [{ type: "text", text: "a" }] },
        { role: "assistant", toolCalls: [{ id: "call_1", name: "fin", arguments: "{}" }] },
        { role: "tool", toolCallId: "call_1", content: "{\"ok\":true}" },
      ],
    });
    await collect(client(s, "explicit"), history);
    const explicit = s.requests()[0]!.json;
    const msgs = explicit.messages as Record<string, unknown>[];
    expect(msgs[0]).toEqual({ role: "system", content: [{ type: "text", text: "You edit video with fin.", cache_control: { type: "ephemeral" } }] });
    expect(msgs[3]).toEqual({ role: "tool", tool_call_id: "call_1", content: [{ type: "text", text: '{"ok":true}', cache_control: { type: "ephemeral" } }] });
    expect(JSON.stringify(explicit).match(/cache_control/g)).toHaveLength(2);
    await collect(client(s, "automatic"), history);
    const automatic = s.requests()[1]!.json;
    expect(JSON.stringify(automatic)).not.toContain("cache_control");
    expect((automatic.messages as Record<string, unknown>[])[0]).toEqual({ role: "system", content: "You edit video with fin." });
  });

  it("asks for usage on the stream and sends Finisher's body", async () => {
    const s = await serve({ sse: sse("text") });
    await collect(client(s), req({ temperature: 0.2 }));
    const body = s.requests()[0]!.json;
    expect(body.stream).toBe(true);
    expect(body.stream_options).toEqual({ include_usage: true });
    expect(body.max_tokens).toBe(1000);
    expect(body.temperature).toBe(0.2);
    expect(body.tools).toEqual([{ type: "function", function: { name: "fin", description: "Run fin.", parameters: req().tools[0]!.parameters } }]);
  });

  it("wire_body_equals_recorded_request and body_hash_matches", async () => {
    const s = await serve({ sse: sse("tool-split") });
    const c = client(s);
    const r = req({ messages: [{ role: "user", content: [{ type: "text", text: "Ünïcødé ✂ and a quote \" here" }, { type: "image", mediaType: "image/png", data: "iVBORw0KGgo=" }] }] });
    const { result } = await collect(c, r);
    const received = s.requests()[0]!.body;
    expect(result.capture.request).toBe(received.toString("utf8"));
    expect(JSON.parse(result.capture.request)).toEqual(c.wireBody(r));
    expect(c.wireBody(r)).toEqual(chatBody(r, { preset: "openrouter", caching: "explicit" }));
    expect(result.bodyHash).toBe(blake3(received).toString("hex"));
    expect(result.capture.response).toHaveLength(4);
  });

  it("retries 429 with retry-after, then 503 with backoff, re-sending the same body", async () => {
    const s = await serve(
      { status: 429, body: { error: { code: 429, message: "slow down" } }, headers: { "retry-after": "2" } },
      { status: 503, body: { error: { code: 503, message: "overloaded" } } },
      { sse: sse("text") },
    );
    const { events, result } = await collect(client(s), req());
    const retries = events.filter((e) => e.type === "retry");
    expect(retries).toEqual([
      { type: "retry", attempt: 1, code: "PROVIDER_RATE_LIMITED", delayMs: 2000 },
      { type: "retry", attempt: 2, code: "PROVIDER_OVERLOADED", delayMs: 2000 },
    ]);
    expect(sleeps).toEqual([2000, 2000]);
    expect(result.text).toBe("The intro runs long.");
    const bodies = s.requests().map((r) => r.body.toString("hex"));
    expect(new Set(bodies).size).toBe(1);
    expect(s.requests()).toHaveLength(3);
  });

  it("retries a stream cut short once, with a notice-able retry event", async () => {
    const text = sse("text");
    const s = await serve({ sse: text, cutAfter: text.indexOf("runs long") }, { sse: text });
    const { events, result } = await collect(client(s), req());
    expect(events.filter((e) => e.type === "retry")).toEqual([{ type: "retry", attempt: 1, code: "PROVIDER_OVERLOADED", delayMs: 1000 }]);
    expect(result.text).toBe("The intro runs long.");
  });

  it("retries an error chunk inside a 200 stream", async () => {
    const s = await serve({ sse: sse("midstream-error") }, { sse: sse("text") });
    const { events } = await collect(client(s), req());
    expect(events.filter((e) => e.type === "retry").map((e) => (e as { code: string }).code)).toEqual(["PROVIDER_OVERLOADED"]);
  });

  it("gives up after five attempts with the last code", async () => {
    const s = await serve({ status: 429, body: { error: { message: "no" } }, headers: { "retry-after": "1" } });
    const f = await failure(client(s), req());
    expect(f.code).toBe("PROVIDER_RATE_LIMITED");
    expect(f.details).toMatchObject({ retryAfterMs: 1000 });
    expect(f.events.filter((e) => e.type === "retry")).toHaveLength(4);
    expect(s.requests()).toHaveLength(5);
  });

  it("maps a malformed event, 401, 402, a context overflow and an unknown model", async () => {
    const cases: [FakeResponse, string][] = [
      [{ sse: sse("malformed") }, "PROVIDER_PROTOCOL"],
      [{ status: 401, body: { error: { code: 401, message: "No auth credentials found" } } }, "PROVIDER_AUTH"],
      [{ status: 402, body: { error: { code: 402, message: "Insufficient credits" } } }, "PROVIDER_AUTH"],
      [{ status: 400, body: { error: { code: 400, message: "This endpoint's maximum context length is 8192 tokens" } } }, "MODEL_CONTEXT_EXCEEDED"],
      [{ status: 404, body: { error: { code: 404, message: "No endpoints found for test/x" } } }, "MODEL_UNKNOWN"],
      [{ status: 400, body: { error: { code: 400, message: "invalid tools" } } }, "PROVIDER_PROTOCOL"],
    ];
    for (const [script, code] of cases) {
      const s = await serve(script);
      const f = await failure(client(s), req());
      expect(f.code, JSON.stringify(script).slice(0, 80)).toBe(code);
      expect(s.requests()).toHaveLength(1);
      expect(JSON.stringify(f)).not.toContain(KEY);
      await s.close();
      server = undefined;
    }
  });
});

describe("local preset", () => {
  it("sends no key, no reasoning and no breakpoints, and reports no cost", async () => {
    const s = await serve({ sse: sse("local") });
    const c = localClient(s.baseUrl, { cachingFor: () => "explicit", sleep: fakeSleep });
    const { result } = await collect(c, req({ model: "llama3.3:70b", reasoning: { effort: "low" } }), noKey);
    expect(result.text).toBe("Local answer.");
    expect(result.usage).toEqual({ input: 50, output: 3, cacheRead: 0, cacheWrite: 0, reasoning: 0 });
    expect(result.costMicros).toBeUndefined();
    const sent = s.requests()[0]!;
    expect(sent.headers.authorization).toBeUndefined();
    expect(sent.json.reasoning).toBeUndefined();
    expect(JSON.stringify(sent.json)).not.toContain("cache_control");
    expect(sent.json.model).toBe("llama3.3:70b");
  });
});

describe("base URLs and retry-after", () => {
  it("takes https, or http on loopback; the local preset also takes http elsewhere", () => {
    const code = (f: () => unknown) => {
      try {
        f();
        return "ok";
      } catch (e) {
        return (e as { code: string }).code;
      }
    };
    expect(code(() => checkBaseUrl("openrouter", "https://openrouter.ai/api/v1"))).toBe("ok");
    expect(code(() => checkBaseUrl("openrouter", "http://127.0.0.1:9/api/v1"))).toBe("ok");
    expect(code(() => checkBaseUrl("openrouter", "http://[::1]:9/v1"))).toBe("ok");
    expect(code(() => checkBaseUrl("openrouter", "http://models.example/api/v1"))).toBe("PROVIDER_UNAVAILABLE");
    expect(code(() => checkBaseUrl("openrouter", "https://user:pw@openrouter.ai/api/v1"))).toBe("PROVIDER_UNAVAILABLE");
    expect(code(() => checkBaseUrl("local", "http://localhost:11434/v1"))).toBe("ok");
    expect(code(() => checkBaseUrl("local", "http://192.0.2.10:1234/v1"))).toBe("ok");
    expect(code(() => checkBaseUrl("local", "file:///etc/passwd"))).toBe("PROVIDER_UNAVAILABLE");
    expect(code(() => checkBaseUrl("local", "not a url"))).toBe("PROVIDER_UNAVAILABLE");
  });

  it("reads seconds or an HTTP date", () => {
    const now = Date.UTC(2026, 10, 7, 12, 0, 0);
    expect(retryAfterMs("3", now)).toBe(3000);
    expect(retryAfterMs("0.5", now)).toBe(500);
    expect(retryAfterMs(new Date(now + 7000).toUTCString(), now)).toBe(7000);
    expect(retryAfterMs("soon", now)).toBeUndefined();
    expect(retryAfterMs(null, now)).toBeUndefined();
  });
});
