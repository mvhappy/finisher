// Claude Code's permission prompts through the runtime (plan 7D Task 8; Decisions 4, 10, 18, 21;
// owner Q3a; Review Focus 3): every `can_use_tool` (and Decision 10's MCP `approve`) becomes a
// `request` 7B's `permissions.ts` and `provider-requests.ts` decide; the answer goes back with
// the input unchanged; a `control_cancel_request` withdraws the card; while the `agent` profile
// is not enforced a shell command is asked and a write outside the workspace denied in every
// mode. The real runtime, store and `control.db` over `fake-claude`, plus unit cases for the port,
// the gate and the managed rule.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Approval, Node, ProviderEvent, Rule, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect } from "effect";
import fc from "fast-check";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { claudePermissions } from "../src/agent/claude/permissions.ts";
import { firstTurnGate } from "../src/agent/claude/probe-check.ts";
import { type CliLine, decodeLine } from "../src/agent/claude/protocol.ts";
import { providerPermissions, toolNames, UNCONFINED_SHELL_RULE } from "../src/agent/permissions.ts";
import type { RuleSource } from "../src/agent/rules.ts";
import { resolveFinPath } from "../src/agent/settings.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";
import { signInClaude } from "./support/claude-sign-in.ts";

const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${WIN ? ".exe" : ""}`, import.meta.url));
const FIN = resolveFinPath({ models: [] } as unknown as Parameters<typeof resolveFinPath>[0]);
const REQUIRE = process.env.FIN_REQUIRE_SERVER === "1";
const withFake = test.skipIf(WIN || (!existsSync(FAKE) && !REQUIRE));
const VERSION = "2.1.321";

// ---- the port, the gate and the managed rule (no process) ----

const canUseTool = (id: string, tool: string, input: unknown, extra: Record<string, unknown> = {}): CliLine => {
  const l = decodeLine(JSON.stringify({ type: "control_request", request_id: id, request: { subtype: "can_use_tool", tool_name: tool, input, ...extra } }));
  if (!("kind" in l)) throw new Error(l.message);
  return l;
};
const requestOf = (l: CliLine) => {
  if (l.kind !== "controlRequest") throw new Error(l.kind);
  return l.request;
};
/** The runtime's id of the request a port turned into an event. */
const idOf = (r: { events?: readonly ProviderEvent[] }) => {
  const e = r.events?.[0];
  if (e?.type !== "request") throw new Error("no request event");
  return e.id;
};
const answerOf = (line: string | { code: string }) => {
  if (typeof line !== "string") throw new Error(line.code);
  return JSON.parse(line) as { response: { subtype: string; request_id: string; error?: string; response?: Record<string, unknown> } };
};

describe("the port", () => {
  test("never_rewrites_input: an allow sends the request's input back unchanged, byte for byte", () => {
    // Plain-object JSON (no `__proto__` key: `JSON.parse` would make it an own key the CLI never sends).
    const json = fc.jsonValue().filter((v) => !JSON.stringify(v).includes('"__proto__"'));
    const input = fc.dictionary(fc.string().filter((k) => k !== "__proto__"), json, { maxKeys: 8 });
    fc.assert(
      fc.property(fc.string({ minLength: 1, maxLength: 40 }), input, (tool, generated) => {
        // As the CLI's JSON carries it (no -0).
        const inp = JSON.parse(JSON.stringify(generated)) as Record<string, unknown>;
        const port = claudePermissions();
        const l = canUseTool("r1", tool, inp, { permission_suggestions: [{ type: "addRules", rules: [{ toolName: tool }] }] });
        const r = port.request("r1", requestOf(l));
        expect(r.answer).toBeUndefined();
        expect(r.events).toEqual([{ type: "request", id: idOf(r), kind: "approval", tool, input: inp }]);
        const a = answerOf(port.respond(idOf(r), { kind: "approval", allow: true }));
        expect(a.response.request_id).toBe("r1");
        expect(a.response.response).toEqual({ behavior: "allow", updatedInput: inp });
        expect(JSON.stringify(a.response.response!.updatedInput)).toBe(JSON.stringify(inp));
      }),
      { numRuns: 500 },
    );
  });

  test("never_sends_updated_permissions; a deny carries the server's message", () => {
    const port = claudePermissions();
    const r1 = idOf(port.request("r1", requestOf(canUseTool("r1", "Bash", { command: "ls" }, { permission_suggestions: [{ type: "addRules" }] }))));
    const r2 = idOf(port.request("r2", requestOf(canUseTool("r2", "Bash", { command: "rm -rf x" }))));
    const allow = answerOf(port.respond(r1, { kind: "approval", allow: true }));
    const deny = answerOf(port.respond(r2, { kind: "approval", allow: false, message: "Finisher's rule rm (local layer) denies this call." }));
    expect(Object.keys(allow.response.response!)).toEqual(["behavior", "updatedInput"]);
    expect(deny.response.response).toEqual({ behavior: "deny", message: "Finisher's rule rm (local layer) denies this call." });
    for (const raw of [allow, deny].map((x) => JSON.stringify(x))) expect(raw).not.toMatch(/updatedPermissions|permission_suggestions/);
  });

  test("nothing is answered the runtime did not decide: no answer at request time, none twice, none after a cancel", () => {
    const port = claudePermissions();
    const first = port.request("r1", requestOf(canUseTool("r1", "Read", { file_path: "a" })));
    expect(first.answer).toBeUndefined();
    const r1 = idOf(first);
    // A reused id while the first waits is refused (a deny), never allowed, and noticed.
    const again = port.request("r1", requestOf(canUseTool("r1", "Read", { file_path: "b" })));
    expect(answerOf(again.answer!).response.response).toMatchObject({ behavior: "deny" });
    expect(again.events).toEqual([{ type: "notice", name: "reusedRequestId", details: { harness: "claudeCode", requestId: "r1" } }]);
    expect(port.respond("nope", { kind: "approval", allow: true })).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    // The CLI's own id is not the runtime's: only the exact request instance is answered.
    expect(port.respond("r1", { kind: "approval", allow: true })).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    expect(port.respond(r1, { kind: "userInput", text: "x" })).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    expect(typeof port.respond(r1, { kind: "approval", allow: true })).toBe("string");
    expect(port.respond(r1, { kind: "approval", allow: true })).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    const r2 = idOf(port.request("r2", requestOf(canUseTool("r2", "Read", { file_path: "a" }))));
    expect(port.cancel("r2")).toEqual([{ type: "requestCancelled", id: r2 }]);
    expect(port.cancel("r2")).toEqual([]);
    expect(port.respond(r2, { kind: "approval", allow: true })).toMatchObject({ code: "PROVIDER_PROTOCOL" });
  });

  test("a stale answer never authorizes a later request that reuses a retired id (review repro)", () => {
    const port = claudePermissions();
    // In one burst: Read(r1), cancel r1, Write(r1) outside the workspace.
    const read = idOf(port.request("r1", requestOf(canUseTool("r1", "Read", { file_path: "safe.txt" }))));
    expect(port.cancel("r1")).toEqual([{ type: "requestCancelled", id: read }]);
    const write = port.request("r1", requestOf(canUseTool("r1", "Write", { file_path: "/tmp/outside", content: "x" })));
    // The reuse is refused at once: a deny for r1, nothing for the runtime to decide.
    expect(answerOf(write.answer!).response).toMatchObject({ request_id: "r1", response: { behavior: "deny" } });
    expect(write.events?.some((e) => e.type === "request")).toBe(false);
    // The runtime's allow of the Read reaches no one.
    expect(port.respond(read, { kind: "approval", allow: true })).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    // Answered ids are retired too.
    const r3 = idOf(port.request("r3", requestOf(canUseTool("r3", "Read", { file_path: "a" }))));
    port.respond(r3, { kind: "approval", allow: false });
    expect(answerOf(port.request("r3", requestOf(canUseTool("r3", "Write", { file_path: "/x" }))).answer!).response.response).toMatchObject({ behavior: "deny" });
    // MCP: a reused control id is refused with a deny body as well.
    const call = { subtype: "mcp_message" as const, server_name: "finisher", message: { jsonrpc: "2.0", id: 9, method: "tools/call", params: { name: "approve", arguments: { tool_name: "Write", input: { file_path: "/x" } } } } };
    const m = idOf(port.request("m1", call));
    port.cancel("m1");
    const reused = answerOf(port.request("m1", call).answer!);
    expect(JSON.parse((reused.response.response as { mcp_response: { result: { content: { text: string }[] } } }).mcp_response.result.content[0]!.text)).toMatchObject({ behavior: "deny" });
    expect(port.respond(m, { kind: "approval", allow: true })).toMatchObject({ code: "PROVIDER_PROTOCOL" });
  });

  test("hook_callback_answered_error (unit) and an unknown subtype", () => {
    const port = claudePermissions();
    const h = port.request("h1", { subtype: "hook_callback", callback_id: "cb_1", input: {} });
    expect(answerOf(h.answer!).response).toMatchObject({ subtype: "error", request_id: "h1" });
    expect(h.events).toEqual([{ type: "notice", name: "hookCallback", details: { harness: "claudeCode", callbackId: "cb_1" } }]);
    const u = port.request("u1", { subtype: "unknown", name: "future_thing" });
    expect(answerOf(u.answer!).response.subtype).toBe("error");
    expect(u.events).toEqual([{ type: "notice", name: "unservedRequest", details: { harness: "claudeCode", subtype: "future_thing" } }]);
  });

  test("the MCP fallback: `approve` is the same request; the answer is its text content; other tools are errors", () => {
    const port = claudePermissions();
    const call = (id: string, name: string, args: unknown) => ({ subtype: "mcp_message" as const, server_name: "finisher", message: { jsonrpc: "2.0", id: 7, method: "tools/call", params: { name, arguments: args } } });
    const r = port.request("m1", call("m1", "approve", { tool_name: "Bash", input: { command: "ls" }, tool_use_id: "t1" }));
    // The call it asks about (`tool_use_id`) rides along: an allowed shell call's window opens on it (Task 12).
    expect(r.events).toEqual([{ type: "request", id: idOf(r), kind: "approval", tool: "Bash", input: { command: "ls" }, providerId: "t1" }]);
    const a = answerOf(port.respond(idOf(r), { kind: "approval", allow: false, message: "no" }));
    expect(a.response.response).toEqual({ mcp_response: { jsonrpc: "2.0", id: 7, result: { content: [{ type: "text", text: JSON.stringify({ behavior: "deny", message: "no" }) }] } } });
    const shell = answerOf(port.request("m2", call("m2", "shell", { command: "ls" })).answer!);
    expect(shell.response.response).toMatchObject({ mcp_response: { id: 7, error: { code: -32602 } } });
    expect(answerOf(port.request("m3", call("m3", "approve", { tool_name: 1 })).answer!).response.response).toMatchObject({ mcp_response: { error: {} } });
  });

  test("asked: what a prompt named this turn, by id or (no id, the MCP path) by the call; forgotten at the turn's end (review of part B)", () => {
    const port = claudePermissions();
    const call = (args: unknown) => ({ subtype: "mcp_message" as const, server_name: "finisher", message: { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "approve", arguments: args } } });
    port.request("c1", requestOf(canUseTool("c1", "Read", { file_path: "a" }, { tool_use_id: "t1" })));
    port.request("c2", call({ tool_name: "Write", input: { file_path: "/x", content: "y" } }));
    const asked = (d: { tool_name: string; tool_use_id?: string; tool_input?: Record<string, unknown> }) => port.asked!(d);
    expect(asked({ tool_name: "Read", tool_use_id: "t1" })).toBe(true);
    expect(asked({ tool_name: "Write", tool_use_id: "t9", tool_input: { content: "y", file_path: "/x" } })).toBe(true);
    expect(asked({ tool_name: "Write", tool_use_id: "t9", tool_input: { file_path: "/elsewhere" } })).toBe(false);
    expect(asked({ tool_name: "Edit", tool_use_id: "t2" })).toBe(false);
    port.turnEnded!();
    expect(asked({ tool_name: "Read", tool_use_id: "t1" })).toBe(false);
    expect(asked({ tool_name: "Write", tool_input: { file_path: "/x", content: "y" } })).toBe(false);
  });
});

describe("the first-turn gate", () => {
  const UUID = "019311f2-dc74-254d-63e1-194e81be77df";
  const plan = { argv: ["claude", "-p", "--session-id", UUID], tools: ["Read"], mcpTools: ["mcp__finisher__approve"] };
  const init = (): CliLine => {
    const l = decodeLine(
      JSON.stringify({
        type: "system",
        subtype: "init",
        session_id: UUID,
        tools: ["Read", "mcp__finisher__approve"],
        model: "claude-opus-5-5",
        permissionMode: "default",
        apiKeySource: "none",
        claude_code_version: VERSION,
        mcp_servers: [{ name: "finisher", status: "connected", source: "sdk" }],
        plugins: [],
        skills: [],
        slash_commands: [],
        agents: ["claude", "Explore", "general-purpose", "Plan", "statusline-setup"],
        output_style: "default",
      }),
    );
    if (!("kind" in l) || l.kind !== "init") throw new Error(`bad fixture ${JSON.stringify(l)}`);
    return l;
  };
  const mcp = (id: string, method: string): CliLine => ({ kind: "controlRequest", request_id: id, request: { subtype: "mcp_message", server_name: "finisher", message: { jsonrpc: "2.0", id: 1, method, params: { name: "approve", arguments: { tool_name: "Read", input: {} } } } } });
  const ids = (lines: readonly CliLine[]) => lines.map((l) => (l.kind === "controlRequest" ? l.request_id : l.kind));

  test("a cancel of a held request drops it: once the gate opens only the others come out (no orphaned card)", () => {
    const gate = firstTurnGate(plan);
    expect(gate.push(canUseTool("r1", "Read", { file_path: "a" })).kind).toBe("hold");
    expect(gate.push(mcp("m1", "tools/call")).kind).toBe("hold");
    expect(gate.push(canUseTool("r2", "Read", { file_path: "b" })).kind).toBe("hold");
    expect(gate.push(mcp("m2", "tools/call")).kind).toBe("hold");
    expect(gate.push({ kind: "controlCancel", request_id: "r1" }).kind).toBe("drop");
    expect(gate.push({ kind: "controlCancel", request_id: "m1" }).kind).toBe("drop");
    // A cancel of a request that is not held goes on (the port ignores it when nothing waits).
    expect(gate.push({ kind: "controlCancel", request_id: "zz" })).toMatchObject({ kind: "forward" });
    const opened = gate.push(init());
    expect(opened.kind).toBe("forward");
    expect(ids(opened.kind === "forward" ? opened.lines : [])).toEqual(["r2", "m2", "init"]);
  });

  test("an MCP `tools/call` is held until the init passes; the handshake is not", () => {
    const gate = firstTurnGate(plan);
    expect(gate.push(mcp("l1", "tools/list")).kind).toBe("forward");
    expect(gate.push(mcp("c1", "tools/call")).kind).toBe("hold");
    const opened = gate.push(init());
    expect(ids(opened.kind === "forward" ? opened.lines : [])).toEqual(["c1", "init"]);
  });
});

describe("the managed rule while the agent profile is not enforced (owner Q3a)", () => {
  const rule = (id: string, effect: Rule["effect"], command: string, layer: Rule["layer"] = "local"): Rule => ({ id, effect, command, layer, source: "test", createdAt: 0 });
  const source = (rules: Rule[]): RuleSource => ({ layers: () => ({ rules, projects: new Map() }) });
  const s = (mode: Session["mode"]) => ({ id: "S1", projectId: "P1", mode, role: "editor", provider: "claudeCode" }) as unknown as Session;
  const explain = (rules: Rule[], mode: Session["mode"], tool: string, input: unknown, unconfined: boolean) =>
    Effect.runPromise(providerPermissions({ rules: source(rules), dirOf: () => "/proj" }).explain(s(mode), { tool, input, unconfined }));

  test("unconfined: shells ask under the managed rule; writes outside the workspace are denied in every mode", async () => {
    for (const mode of ["plan", "draft", "auto"] as const) {
      for (const tool of ["Bash", "mcp__finisher__shell"]) {
        expect(await explain([], mode, tool, { command: "git status" }, true), `${mode} ${tool}`).toMatchObject({ decision: "ask", reason: "unconfined", rule: UNCONFINED_SHELL_RULE, layer: "managed" });
      }
      for (const tool of ["Edit", "Write"]) {
        expect((await explain([], mode, tool, { file_path: "/proj/finisher.project.json" }, true)).decision, `${mode} ${tool}`).toBe("deny");
        // `/proj` is not on disk: an unresolvable workspace fails closed (real paths below).
        expect((await explain([], mode, tool, { file_path: "/proj/workspace/S1/notes.md" }, true)).decision, `${mode} ${tool}`).toBe("deny");
      }
      expect((await explain([], mode, "Read", { file_path: "/proj/x" }, true)).decision).toBe("allow");
    }
    expect(await explain([], "draft", "Write", { file_path: "/home/u/.bashrc" }, true)).toMatchObject({ decision: "deny", reason: "unconfined" });
    // A user allow does not lift a managed ask; a deny rule still denies first.
    expect((await explain([rule("ls", "allow", "tool:Bash(ls *)", "user")], "draft", "Bash", { command: "ls -la" }, true)).decision).toBe("ask");
    expect(await explain([rule("ls", "deny", "tool:Bash(ls *)")], "draft", "Bash", { command: "ls -la" }, true)).toMatchObject({ decision: "deny", reason: "rule" });
  });

  describe("symlinks never carry an unconfined write outside the workspace (review of Task 8)", () => {
    let root: string;
    let ws: string;
    const at = (p: string, platform?: NodeJS.Platform) =>
      Effect.runPromise(providerPermissions({ rules: source([]), dirOf: () => join(root, "P"), ...(platform === undefined ? {} : { platform }) }).explain(s("draft"), { tool: "Write", input: { file_path: p }, unconfined: true }));
    const decide = (tool: string, mode: Session["mode"], p: string) =>
      Effect.runPromise(providerPermissions({ rules: source([]), dirOf: () => join(root, "P") }).explain(s(mode), { tool, input: { [tool === "NotebookEdit" ? "notebook_path" : "file_path"]: p }, unconfined: true }));
    beforeEach(() => {
      root = realpathSync(mkdtempSync(join(tmpdir(), "agent-claude-links-")));
      ws = join(root, "P", "workspace", "S1");
      mkdirSync(join(ws, "sub", "deep"), { recursive: true });
      mkdirSync(join(root, "outside", "dir"), { recursive: true });
      writeFileSync(join(root, "outside", "secret.txt"), "x");
      writeFileSync(join(ws, "real.txt"), "x");
      symlinkSync(join(root, "outside", "secret.txt"), join(ws, "link.txt"));
      symlinkSync(join(root, "outside", "dir"), join(ws, "linkdir"));
      symlinkSync(join(ws, "real.txt"), join(ws, "inner.txt"));
      symlinkSync(join(root, "outside", "nowhere.txt"), join(ws, "dangling.txt"));
      symlinkSync(join(ws, "loop"), join(ws, "loop"));
    });
    afterEach(() => rmSync(root, { recursive: true, force: true }));

    test("each write tool, every mode: a symlinked file or parent dir that leads outside is denied", async () => {
      const outside = [
        "link.txt",
        join(ws, "link.txt"),
        "linkdir/new.txt",
        join(ws, "linkdir", "a", "b", "new.txt"),
        "sub/../link.txt",
        "linkdir/../escape.txt",
        "dangling.txt",
        "loop/x.txt",
        // `..` past a missing component: the kernel cannot resolve it, so neither does the check.
        "nothere/../real.txt",
      ];
      for (const tool of ["Edit", "Write", "MultiEdit", "NotebookEdit"]) {
        for (const mode of ["plan", "draft", "auto"] as const) {
          for (const p of outside) expect((await decide(tool, mode, p)).decision, `${tool} ${mode} ${p}`).toBe("deny");
          for (const p of ["real.txt", "inner.txt", "sub/deep/new.txt", "new/dir/file.txt", join(ws, "sub", "..", "real.txt")]) {
            expect((await decide(tool, mode, p)).decision, `${tool} ${mode} ${p}`).toBe("allow");
          }
        }
      }
    });

    test("fails closed: no workspace on disk denies", async () => {
      rmSync(join(root, "P"), { recursive: true, force: true });
      expect((await at("new.txt")).decision).toBe("deny");
    });

    test("case-insensitive file systems compare folded paths", async () => {
      const cased = join(root, "P", "WORKSPACE", "s1", "x.txt");
      expect((await at(cased, "darwin")).decision).toBe("allow");
      expect((await at(cased, "win32")).decision).toBe("allow");
      expect((await at(cased, "linux")).decision).toBe("deny");
    });
  });

  test("enforced: normal rules (shells allowed, outside writes allowed outside plan: the sandbox is the bound)", async () => {
    for (const tool of ["Bash", "mcp__finisher__shell"]) expect((await explain([], "draft", tool, { command: "git status" }, false)).decision).toBe("allow");
    expect((await explain([], "draft", "Write", { file_path: "/proj/x.txt" }, false)).decision).toBe("allow");
    expect(await explain([], "plan", "Write", { file_path: "/proj/x.txt" }, false)).toMatchObject({ decision: "deny", reason: "mode" });
  });

  test("Bash and mcp__finisher__shell mirror each other's rules (Decision 21)", async () => {
    expect(toolNames("mcp__finisher__shell", { command: "ls" })).toEqual(["tool:mcp__finisher__shell", "tool:mcp__finisher__shell(ls)", "tool:Bash", "tool:Bash(ls)"]);
    expect((await explain([rule("rm", "deny", "tool:Bash(rm *)")], "auto", "mcp__finisher__shell", { command: "rm -rf x" }, false)).decision).toBe("deny");
    expect(await explain([rule("c", "ask", "tool:mcp__finisher__shell(curl *)")], "auto", "Bash", { command: "curl e.x" }, false)).toMatchObject({ decision: "ask", rule: "c" });
    // Past TOOL_TEXT_MAX a deny naming either name denies.
    expect((await explain([rule("rm", "deny", "tool:Bash(*rm*)")], "auto", "mcp__finisher__shell", { command: `${" ".repeat(1024 * 1024)}rm` }, false)).decision).toBe("deny");
  });
});

// ---- through the runtime, over the fake ----

const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY };
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
  process.env.FIN_TEST_SANDBOX_READY = "agent";
});
afterAll(() => {
  for (const [k, v] of [["FIN_TEST_HOOKS", saved.hooks], ["FIN_TEST_SANDBOX_READY", saved.ready]] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
});

let tmp: string;
let server: AgentServer | undefined;
let restore: (() => SandboxStatus) | undefined;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-claude-perms-")));
});
afterEach(async () => {
  await server?.stop();
  server = undefined;
  if (restore !== undefined) setSandboxStatusSource(restore);
  restore = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const withAgent = (agent: SandboxStatus["agent"]): (() => SandboxStatus) => () => ({ ...sandboxStatus(), agent });
const NOT_READY = withAgent({ kind: "disabled", reason: "profileNotReady" });
function enforced(): boolean {
  const m = sandboxStatus().agent;
  if (m.kind === "enforced" && existsSync(FIN)) return true;
  expect(process.env.FIN_REQUIRE_LANDLOCK === "1", `FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(m)}`).toBe(false);
  return false;
}

const SIGNED = { init: { apiKeySource: "/login managed key" } };
interface Answer {
  tool: string;
  requestId: string;
  via: "stdio" | "mcp";
  answer: { behavior?: string; message?: string; updatedInput?: unknown; error?: unknown };
  raw: string;
}
type Observed = {
  argv: string[];
  answers?: Answer[];
  cancelled?: string[];
  controlResponses?: { requestId: string; subtype: string; raw: string }[];
  cliRefused?: { tool: string; toolUseId: string }[];
  bash?: { command: string; shell: string; code: number | null; stdout: string }[];
}[];

interface Running {
  readonly local: NdjsonClient;
  readonly owner: NdjsonClient;
  readonly projectId: string;
  readonly projectDir: string;
  readonly serverData: string;
}

/** Steps of one turn: an `ask` per entry, each its own step, then the turn's `result`. */
const asks = (...list: { tool: string; input: unknown; cancelAfterMs?: number; run?: boolean }[]) => ({
  steps: [...list.map((ask, i) => ({ ...(i === 0 ? { expect: "user" } : {}), ask })), { cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }],
});

async function claudeServer(script: Record<string, unknown>, opts: { status?: () => SandboxStatus; rules?: { effect: string; command: string }[] } = {}): Promise<Running> {
  const previous = setSandboxStatusSource(opts.status ?? NOT_READY);
  restore ??= previous;
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, "claude");
  copyFileSync(FAKE, binary);
  writeFileSync(join(dir, "script.json"), JSON.stringify({ ...SIGNED, ...script }));
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: "@config/observed.json", version: VERSION }));
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: binary } } }));
  signInClaude(serverData);
  server = new AgentServer(serverData, { timers: false });
  await server.start();
  const local = await server.local();
  const owner = await server.console();
  const projectDir = join(tmp, "P");
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: projectDir, name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaaP", 0, [addAsset()])));
  for (const r of opts.rules ?? []) ok(await confirmed(owner, "agent.rules.add", { ...r, projectId }));
  return { local, owner, projectId, projectDir, serverData };
}

const start = async (r: Running, mode?: Session["mode"]) =>
  ok<{ session: Session }>(await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt: "go", ...(mode === undefined ? {} : { mode }) })).session;

async function until<T>(f: () => T | undefined | Promise<T | undefined>, ms = 20_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((res) => setTimeout(res, 20));
  }
}
const statusOf = (id: string) => server!.core.store.get(id)!.status;
const settle = (id: string) => until(() => (["idle", "interrupted", "ended"].includes(statusOf(id)) ? server!.core.store.get(id)! : undefined));
const nodes = (id: string): Node[] => server!.core.store.nodes(id, 1, 1000) as Node[];
const approvalNodes = (id: string) => nodes(id).filter((n) => n.kind === "approval").map((n) => n.payload as Record<string, unknown>);
const observed = (r: Running, id: string): Observed => {
  const p = join(r.serverData, "agents", id, "provider", "observed.json");
  return existsSync(p) ? (JSON.parse(readFileSync(p, "utf8")) as Observed) : [];
};
const answers = (r: Running, id: string) => observed(r, id).flatMap((o) => o.answers ?? []);
const noticesOf = (id: string, name: string) => nodes(id).flatMap((n) => (n.kind === "notice" && Object.hasOwn(n.payload as object, name) ? [(n.payload as Record<string, unknown>)[name]] : []));
const openCard = (id: string) => until(() => server!.core.store.approvals(id).find((a: Approval) => a.status === "open"));

/** The runtime's id of a CLI request (`<CLI id>#<generation>`, review of Task 8). */
const ofCli = (cliId: string) => expect.stringMatching(new RegExp(`^${cliId.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}#\\d+$`));
const cliOf = (runtimeId: unknown) => String(runtimeId).replace(/#\d+$/, "");

/** Review Focus 3: every allow the CLI heard is an `allow` decision the runtime recorded for that request. */
function everyAllowDecided(r: Running, id: string): void {
  const decided = approvalNodes(id).filter((p) => p.decision === "allow").map((p) => cliOf(p.requestId));
  for (const a of answers(r, id)) if (a.answer.behavior === "allow") expect(decided, `allow of ${a.requestId}`).toContain(a.requestId);
}

describe("through the runtime", () => {
  withFake("read_and_ls_reach_runtime: each call is a request and an approval node; a deny rule refuses the ls", async () => {
    const r = await claudeServer(asks({ tool: "Read", input: { file_path: "cuts.txt" } }, { tool: "Bash", input: { command: "ls -la" } }), { rules: [{ effect: "deny", command: "tool:Bash(ls *)" }] });
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    const got = answers(r, s.id);
    expect(got.map((a) => [a.tool, a.via, a.answer.behavior])).toEqual([["Read", "stdio", "allow"], ["Bash", "stdio", "deny"]]);
    expect(approvalNodes(s.id)).toEqual([
      expect.objectContaining({ requestId: ofCli(got[0]!.requestId), kind: "provider", tool: "Read", decision: "allow", by: "policy", reason: "sandbox" }),
      expect.objectContaining({ requestId: ofCli(got[1]!.requestId), kind: "provider", tool: "Bash", decision: "deny", by: "policy", reason: "rule", layer: "local" }),
    ]);
    expect(got[1]!.answer.message).toMatch(/rule .* \(local layer\) denies/);
    everyAllowDecided(r, s.id);
    // The denied `ls` is in the result's `permission_denials`, but a prompt named it: no cliDenied.
    expect(noticesOf(s.id, "cliDenied")).toEqual([]);
  });

  withFake("can_use_tool_allowed_by_policy: by policy; updatedInput is the request's input byte for byte", async () => {
    const input = { file_path: "dir/ü.txt", offset: 10, limit: 2.5, nested: { z: [1, "two", null, true], a: " " } };
    const r = await claudeServer(asks({ tool: "Read", input }));
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    const [a] = answers(r, s.id);
    expect(a!.answer).toEqual({ behavior: "allow", updatedInput: input });
    const node = approvalNodes(s.id)[0]!;
    expect(node).toMatchObject({ decision: "allow", by: "policy" });
    expect(JSON.stringify((JSON.parse(a!.raw) as { response: { response: { updatedInput: unknown } } }).response.response.updatedInput)).toBe(JSON.stringify(node.input));
    expect(a!.raw).not.toMatch(/updatedPermissions/);
  });

  withFake("plan_mode_write_outside_workspace_denied: by the CLI itself, before any prompt; a cliDenied notice, no approval", async () => {
    // Verified on 2.1.321 (7D Task 0, s2a): `blockWritesOutsideWorkingDirectories` refuses a
    // write outside the working directory with no `can_use_tool`; it is only in the result's
    // `permission_denials`. Finisher's own plan-mode deny (the unit tests above) stays the
    // second layer for a path the CLI lets through.
    const r = await claudeServer(asks({ tool: "Write", input: { file_path: "/tmp/elsewhere.txt", content: "x" } }, { tool: "Write", input: { file_path: "notes.md", content: "x" } }));
    const s = await start(r, "plan");
    expect((await settle(s.id)).status).toBe("idle");
    const got = answers(r, s.id);
    expect(got.map((a) => [a.tool, a.answer.behavior])).toEqual([["Write", "allow"]]);
    expect(approvalNodes(s.id).map((p) => [p.tool, (p.input as { file_path?: string }).file_path, p.decision])).toEqual([["Write", "notes.md", "allow"]]);
    const refused = observed(r, s.id).flatMap((o) => o.cliRefused ?? []);
    expect(refused).toEqual([{ tool: "Write", toolUseId: expect.any(String) }]);
    expect(noticesOf(s.id, "cliDenied")).toEqual([{ harness: "claudeCode", tool: "Write", toolUseId: refused[0]!.toolUseId, path: "/tmp/elsewhere.txt" }]);
  });

  withFake("unconfined (direct): a shell command opens a managed card; the CLI refuses a write outside the workspace itself", async () => {
    const r = await claudeServer(asks({ tool: "Bash", input: { command: "git status" } }, { tool: "Edit", input: { file_path: "/tmp/x.txt", old_string: "a", new_string: "b" } }));
    const s = await start(r);
    expect(s.mode).not.toBe("plan");
    const card = await openCard(s.id);
    expect(card).toMatchObject({ kind: "provider", subject: expect.objectContaining({ rule: UNCONFINED_SHELL_RULE, layer: "managed" }) });
    expect(statusOf(s.id)).toBe("waitingApproval");
    ok(await confirmed(r.owner, "agent.approval.answer", { id: card.id, decision: "allow" }));
    expect((await settle(s.id)).status).toBe("idle");
    const got = answers(r, s.id);
    expect(got.map((a) => [a.tool, a.answer.behavior])).toEqual([["Bash", "allow"]]);
    expect(approvalNodes(s.id).find((p) => p.tool === "Edit")).toBeUndefined();
    expect(noticesOf(s.id, "cliDenied")).toEqual([expect.objectContaining({ tool: "Edit", path: "/tmp/x.txt" })]);
    everyAllowDecided(r, s.id);
  });

  withFake("enforced (launcher): the same calls follow the normal rules", async () => {
    if (!enforced()) return;
    const r = await claudeServer(asks({ tool: "Bash", input: { command: "git status" } }, { tool: "Edit", input: { file_path: "/tmp/x.txt", old_string: "a", new_string: "b" } }), { status: withAgent(sandboxStatus().agent) });
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    expect(answers(r, s.id).map((a) => [a.tool, a.answer.behavior])).toEqual([["Bash", "allow"]]);
    expect(noticesOf(s.id, "cliDenied")).toEqual([expect.objectContaining({ tool: "Edit", path: "/tmp/x.txt" })]);
    expect(server!.core.store.approvals(s.id)).toEqual([]);
  });

  describe("the session's Bash has its own HOME and SHELL (7D Decision 5, Task 0 signed in)", () => {
    // Verified on 2.1.321 (7D Task 0, s3B): with the server's `HOME` and no `SHELL` the CLI's Bash
    // started the passwd login shell, which sourced the user's rc files and passed on what they
    // export. fake-claude runs an allowed Bash call as the CLI does (`$SHELL -l -c`, `/bin/sh`
    // standing in for the passwd shell), under the session's sandbox.
    const LEAK = "FIN_RC_LEAK_PROBE";
    let userHome: string;
    let saved: string | undefined;
    beforeEach(() => {
      userHome = mkdtempSync(join(tmp, "user-home-"));
      for (const rc of [".profile", ".bash_profile", ".bashrc", ".zshenv"]) writeFileSync(join(userHome, rc), `export ${LEAK}=leaked\n`);
      saved = process.env.HOME;
      process.env.HOME = userHome;
    });
    afterEach(() => {
      if (saved === undefined) delete process.env.HOME;
      else process.env.HOME = saved;
    });

    const bashEnv = async (r: Running) => {
      const s = await start(r);
      if (statusOf(s.id) !== "idle") {
        const card = await until(() => (statusOf(s.id) === "idle" ? null : server!.core.store.approvals(s.id).find((a: Approval) => a.status === "open")));
        if (card !== null) ok(await confirmed(r.owner, "agent.approval.answer", { id: card.id, decision: "allow" }));
      }
      expect((await settle(s.id)).status).toBe("idle");
      const [run] = observed(r, s.id).flatMap((o) => o.bash ?? []);
      expect(run, "the Bash call ran").toBeDefined();
      const env = Object.fromEntries(run!.stdout.split("\n").filter((l) => l.includes("=")).map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]));
      return { s, run: run!, env };
    };
    const check = ({ s, run, env }: Awaited<ReturnType<typeof bashEnv>>, r: Running) => {
      const home = join(r.serverData, "agents", s.id, "tmp", "home");
      expect(run.code).toBe(0);
      expect(env.HOME).toBe(home);
      expect(["/bin/bash", "/bin/sh"]).toContain(env.SHELL);
      expect(run.shell).toBe(env.SHELL);
      expect(env).not.toHaveProperty(LEAK);
      expect(run.stdout).not.toContain(userHome);
    };

    withFake("direct: the login shell reads nothing of the user's home", async () => {
      const r = await claudeServer(asks({ tool: "Bash", input: { command: "env" }, run: true }));
      check(await bashEnv(r), r);
    });

    withFake("enforced (launcher): the same under the agent profile", async () => {
      if (!enforced()) return;
      const r = await claudeServer(asks({ tool: "Bash", input: { command: "env" }, run: true }), { status: withAgent(sandboxStatus().agent) });
      check(await bashEnv(r), r);
    });
  });

  withFake("rule_ask_opens_provider_card: waitingApproval; a person's allow answers the CLI; the card is consumed", async () => {
    const r = await claudeServer(asks({ tool: "WebFetch", input: { url: "https://example.com", prompt: "read" } }), { rules: [{ effect: "ask", command: "tool:WebFetch" }] });
    const s = await start(r);
    const card = await openCard(s.id);
    expect(statusOf(s.id)).toBe("waitingApproval");
    expect(answers(r, s.id)).toEqual([]);
    ok(await confirmed(r.owner, "agent.approval.answer", { id: card.id, decision: "allow" }));
    expect((await settle(s.id)).status).toBe("idle");
    expect(answers(r, s.id).map((a) => a.answer.behavior)).toEqual(["allow"]);
    expect(server!.core.store.approval(card.id)).toMatchObject({ status: "answered", decision: "allow", consumedAt: expect.any(Number) });
    expect(approvalNodes(s.id).at(-1)).toMatchObject({ approvalId: card.id, decision: "allow", by: "user" });
    everyAllowDecided(r, s.id);
  });

  withFake("card_expiry_denies", async () => {
    const r = await claudeServer(asks({ tool: "WebFetch", input: { url: "https://example.com" } }), { rules: [{ effect: "ask", command: "tool:WebFetch" }] });
    const s = await start(r);
    const card = await openCard(s.id);
    server!.time.advance(31 * 60_000);
    server!.agents.runtime.expireApprovals();
    expect((await settle(s.id)).status).toBe("idle");
    const [a] = answers(r, s.id);
    expect(a!.answer).toMatchObject({ behavior: "deny", message: expect.stringMatching(/in time/) });
    expect(server!.core.store.approval(card.id)!.status).toBe("expired");
    expect(approvalNodes(s.id).at(-1)).toMatchObject({ approvalId: card.id, decision: "deny", by: "policy", reason: "expired" });
  });

  withFake("control_cancel_withdraws_card: expired, reason withdrawn, no answer sent", async () => {
    // The turn goes on for a while after the cancel: the card is expired then, not by the run's end.
    const script = { steps: [{ expect: "user", ask: { tool: "WebFetch", input: { url: "https://example.com" }, cancelAfterMs: 800 } }, { sleep: 2500, cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] };
    const r = await claudeServer(script, { rules: [{ effect: "ask", command: "tool:WebFetch" }] });
    const s = await start(r);
    const card = await openCard(s.id);
    expect(statusOf(s.id)).toBe("waitingApproval");
    await until(() => (server!.core.store.approval(card.id)!.status === "expired" ? true : undefined), 2000);
    expect(statusOf(s.id)).toBe("running");
    expect((await settle(s.id)).status).toBe("idle");
    expect(server!.core.store.approval(card.id)!.status).toBe("expired");
    const obs = observed(r, s.id)[0]!;
    expect(obs.cancelled).toHaveLength(1);
    expect(answers(r, s.id)).toEqual([]);
    // Nothing was written for the withdrawn request (only the handshake's answers).
    expect((obs.controlResponses ?? []).filter((c) => c.requestId === obs.cancelled![0])).toEqual([]);
    expect(approvalNodes(s.id).at(-1)).toMatchObject({ approvalId: card.id, requestId: ofCli(obs.cancelled![0]!), status: "expired", reason: "withdrawn" });
    // A late answer reaches no one.
    expect((await confirmed(r.owner, "agent.approval.answer", { id: card.id, decision: "allow" })).ok).toBe(false);
  }, 20_000);

  withFake("deny_rule_beats_everything: an ask rule, an allow rule and the managed ask lose to a deny", async () => {
    const rules = [
      { effect: "allow", command: "tool:Bash(ls *)" },
      { effect: "ask", command: "tool:Bash" },
      { effect: "deny", command: "tool:mcp__finisher__shell(ls *)" },
    ];
    const r = await claudeServer(asks({ tool: "Bash", input: { command: "ls -la" } }), { rules });
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    expect(answers(r, s.id).map((a) => a.answer.behavior)).toEqual(["deny"]);
    expect(server!.core.store.approvals(s.id)).toEqual([]);
  });

  withFake("oversized_input_never_allowed: a command past 1 MiB is asked (tool.oversized)", async () => {
    const r = await claudeServer(asks({ tool: "Read", input: { file_path: `${"a/".repeat(600_000)}x` } }));
    const s = await start(r);
    const card = await openCard(s.id);
    expect(card.subject).toMatchObject({ rule: "tool.oversized", layer: "managed" });
    ok(await confirmed(r.owner, "agent.approval.answer", { id: card.id, decision: "deny" }));
    expect((await settle(s.id)).status).toBe("idle");
    expect(answers(r, s.id).map((a) => a.answer.behavior)).toEqual(["deny"]);
  });

  withFake("hook_callback_answered_error and noticed", async () => {
    const hook = { type: "control_request", request_id: "hook-1", request: { subtype: "hook_callback", callback_id: "cb_9", input: {} } };
    const r = await claudeServer({ steps: [{ expect: "user", emit: [hook] }, { sleep: 300, cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] });
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    const resp = (observed(r, s.id)[0]!.controlResponses ?? []).find((c) => c.requestId === "hook-1");
    expect(resp).toMatchObject({ subtype: "error" });
    expect(nodes(s.id).filter((n) => n.kind === "notice").map((n) => n.payload)).toContainEqual({ hookCallback: { harness: "claudeCode", callbackId: "cb_9" } });
  });

  withFake("a stale answer never authorizes a request that reuses a cancelled id (review repro)", async () => {
    const line = (tool: string, input: unknown) => ({ type: "control_request", request_id: "r1", request: { subtype: "can_use_tool", tool_name: tool, input, tool_use_id: "toolu_x", permission_suggestions: [], blocked_path: null, decision_reason: null } });
    const burst = [line("Read", { file_path: "safe.txt" }), { type: "control_cancel_request", request_id: "r1" }, line("Write", { file_path: "/tmp/outside.txt", content: "x" })];
    const r = await claudeServer({ steps: [{ expect: "user", emit: burst }, { sleep: 1000, cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] });
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    const toR1 = (observed(r, s.id)[0]!.controlResponses ?? []).filter((c) => c.requestId === "r1").map((c) => JSON.parse(c.raw) as { response: { response?: { behavior?: string; updatedInput?: unknown } } });
    // The refused reuse is r1's last answer; an allow can only be the Read's, had it been decided
    // before the cancel arrived (lines can come in separate chunks): never one with the Write's input.
    expect(toR1.at(-1)?.response.response?.behavior).toBe("deny");
    for (const c of toR1.slice(0, -1)) expect(c.response.response).toEqual({ behavior: "allow", updatedInput: { file_path: "safe.txt" } });
    expect(toR1.length).toBeLessThanOrEqual(2);
    expect(approvalNodes(s.id).some((p) => p.tool === "Write")).toBe(false);
    expect(nodes(s.id).filter((n) => n.kind === "notice").map((n) => n.payload)).toContainEqual({ reusedRequestId: { harness: "claudeCode", requestId: "r1" } });
  });

  withFake("mcp_prompt_tool_path_same_decisions (Decision 10's fallback)", async () => {
    const r = await claudeServer({ refuseFlagValues: { "--permission-prompt-tool": ["stdio"] }, ...asks({ tool: "Read", input: { file_path: "cuts.txt" } }, { tool: "Bash", input: { command: "ls -la" } }) }, { rules: [{ effect: "deny", command: "tool:Bash(ls *)" }] });
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    const got = answers(r, s.id);
    expect(got.map((a) => [a.tool, a.via, a.answer.behavior])).toEqual([["Read", "mcp", "allow"], ["Bash", "mcp", "deny"]]);
    expect(got[0]!.answer.updatedInput).toEqual({ file_path: "cuts.txt" });
    expect(approvalNodes(s.id).map((p) => [p.tool, p.decision, p.reason])).toEqual([["Read", "allow", "sandbox"], ["Bash", "deny", "rule"]]);
    everyAllowDecided(r, s.id);
  });
});

// Keeps the event type in the union the port emits.
const _cancelled: ProviderEvent = { type: "requestCancelled", id: "x" };
void _cancelled;
