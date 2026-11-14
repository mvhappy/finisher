// The brokered shell for Claude Code (plan 7D Task 13, Decisions 10, 18, 21; owner Q2a): the
// `finisher` SDK MCP server `finisherd` serves over `mcp_message` (the handshake, `tools/list`,
// the shell's `tools/call` and Decision 10's `approve` behind one host), and its `shell` tool:
// a fresh process through the session sandbox, in the workspace, with `FIN_CALL` (so `fin` is
// attributed `exact`), its output capped as the CLI's `Bash`, its process group killed at the
// timeout. Units for the host and the tool (`sh` here, `pwsh` on Windows, one interface), the
// launch shape, and the real runtime, store and `control.db` over `fake-claude` with the
// brokered shell switched on (`FIN_TEST_BROKERED_SHELL=1`, a test-hooks build: off on every OS
// otherwise, owner Q2a), so every case runs on Linux. The behavioural cases run on every OS
// in that OS's shell (`COMMANDS`, one entry per `Shell`); one is skipped, its reason in its
// name, only when the shell's binary (or the fake, the `fin` build) is absent.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Node, ProviderEvent, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { SessionSpec } from "../src/agent/adapter.ts";
import { BROKERED_SHELL_TEST_ENV } from "../src/agent/claude/adapter.ts";
import { launchPlan, SHELL_TOOL } from "../src/agent/claude/launch.ts";
import { mcpHost } from "../src/agent/claude/mcp-host.ts";
import { claudePermissions } from "../src/agent/claude/permissions.ts";
import type { ProbeResult } from "../src/agent/claude/probe-types.ts";
import { type CliRequest, decodeLine } from "../src/agent/claude/protocol.ts";
import type { RequestPort } from "../src/agent/claude/session.ts";
import { capOutput, PWSH, SH, SHELL_DEFAULT_TIMEOUT_MS, SHELL_MAX_TIMEOUT_MS, SHELL_OUTPUT_MAX, type ShellBox, shellFor, shellInput, shellTool } from "../src/agent/claude/shell-tool.ts";
import type { SpawnOptions } from "../src/agent/sandbox.ts";
import { toolNames } from "../src/agent/permissions.ts";
import { resolveFinPath } from "../src/agent/settings.ts";
import type { SessionTx } from "../src/agent/store-tables.ts";
import { startProcess } from "../src/sandbox/launch.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import { signInClaude } from "./support/claude-sign-in.ts";
import { addAsset, commit, confirmed, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";
const EXE = WIN ? ".exe" : "";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${EXE}`, import.meta.url));
const FIN = resolveFinPath({ models: [] } as unknown as Parameters<typeof resolveFinPath>[0]);
const REQUIRE = process.env.FIN_REQUIRE_SERVER === "1";
const VERSION = "2.1.321";

// ---- the shell each OS's tests run in (Step 1: every behavioural case on every OS) ----

/** This OS's shell (`shellFor`): `sh` here, `pwsh` on Windows. */
const SHELL = shellFor(process.platform);
/** Whether its binary exists: `/bin/sh`, or `pwsh.exe` on the `PATH`. */
const shellPresent = (): boolean => {
  if (SHELL.name === "sh") return existsSync(SH.argv("")[0]!);
  const dirs = (process.env.PATH ?? process.env.Path ?? "").split(delimiter).filter((d) => d !== "");
  return dirs.some((d) => existsSync(join(d, PWSH.argv("")[0]!)));
};
const SHELL_ABSENT = shellPresent() ? undefined : `${SHELL.argv("")[0]} not found`;
/** `test`, or skipped with its reason in the name when this OS's shell binary is absent. */
const skipNamed =
  (skip: string | undefined) =>
  (name: string, fn: () => Promise<void> | void, timeout?: number): void =>
    skip === undefined ? test(name, fn, timeout) : test.skip(`${name} [skipped: ${skip}]`, fn);
const withShell = skipNamed(SHELL_ABSENT);
const fakeAbsent = !existsSync(FAKE) && !REQUIRE ? "fake-claude not built" : undefined;
const finAbsent = (!existsSync(FAKE) || !existsSync(FIN)) && !REQUIRE ? "fake-claude or fin not built" : undefined;
const withFake = skipNamed(fakeAbsent);
const withFakeShell = skipNamed(fakeAbsent ?? SHELL_ABSENT);
const withFin = skipNamed(finAbsent ?? SHELL_ABSENT);

/** PowerShell: a string written as is (no newline added), on stdout or stderr. */
const psOut = (expr: string) => `[Console]::Out.Write(${expr})`;
const psErr = (expr: string) => `[Console]::Error.Write(${expr})`;
/** UTF-8 out, without a byte-order mark (Windows' console default is the OEM code page). */
const PS_UTF8 = "$u = [Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = $u";

/** Each case's command, per shell (one interface, `Shell`): the same output from both. */
const COMMANDS = {
  sh: {
    /** cwd, `$FIN_CALL` on stdout, `oops` on stderr, exit 3. */
    whereAndCall: 'pwd -P; printf "%s\\n" "$FIN_CALL"; echo oops >&2; exit 3',
    ok: "true",
    /** `n` a's then b on stdout, `n` c's then d on stderr. */
    floods: (n: number) => `head -c ${n} /dev/zero | tr '\\0' a; printf b; head -c ${n} /dev/zero | tr '\\0' c >&2; printf d >&2`,
    /** `n` a's, an emoji, bbb on stdout; xx, an emoji, `n` c's on stderr. */
    emojiAtCuts: (n: number) => `head -c ${n} /dev/zero | tr '\\0' a; printf '\\360\\237\\230\\200bbb'; printf 'xx\\360\\237\\230\\200' >&2; head -c ${n} /dev/zero | tr '\\0' c >&2`,
    /** A background child's pid in bg.pid, `started`, then a long wait. */
    groupThenWait: "sleep 30 & echo $! > bg.pid; echo started; sleep 30",
    wait: "sleep 30",
    /** cwd, `$FIN_CALL`, `$TMPDIR`, the parent's pid; oops on stderr; exit 3. */
    sessionView: 'pwd -P; printf "%s\\n" "$FIN_CALL" "$TMPDIR" "$PPID"; echo oops >&2; exit 3',
    fin: (fin: string, args: string, out: string) => `'${fin}' ${args} > ${out}`,
  },
  pwsh: {
    whereAndCall: `${psOut('(Get-Location).ProviderPath + "\`n" + $env:FIN_CALL + "\`n"')}; ${psErr('"oops\`n"')}; exit 3`,
    ok: "exit 0",
    floods: (n: number) => `${psOut(`('a' * ${n}) + 'b'`)}; ${psErr(`('c' * ${n}) + 'd'`)}`,
    emojiAtCuts: (n: number) => `${PS_UTF8}; ${psOut(`('a' * ${n}) + [char]::ConvertFromUtf32(0x1F600) + 'bbb'`)}; ${psErr(`'xx' + [char]::ConvertFromUtf32(0x1F600) + ('c' * ${n})`)}`,
    groupThenWait: `$p = Start-Process -FilePath pwsh.exe -ArgumentList '-NoProfile','-Command','Start-Sleep 30' -NoNewWindow -PassThru; Set-Content -Path bg.pid -Value $p.Id; ${psOut('"started\`n"')}; Start-Sleep 30`,
    wait: "Start-Sleep 30",
    sessionView: `${psOut('(Get-Location).ProviderPath + "\`n" + $env:FIN_CALL + "\`n" + $env:TMPDIR + "\`n" + (Get-Process -Id $PID).Parent.Id + "\`n"')}; ${psErr('"oops\`n"')}; exit 3`,
    fin: (fin: string, args: string, out: string) => `& '${fin}' ${args} > ${out}`,
  },
} as const;
const CMD = COMMANDS[SHELL.name];
/** The workspace path as this OS's shell prints it, ending in `/workspace/<id>` (or `\\`). */
const workspaceTail = (id: string) => new RegExp(`[\\\\/]workspace[\\\\/]${id}$`);

// ---- the host (no process) ----

const request = (raw: unknown): { id: string; r: CliRequest } => {
  const l = decodeLine(JSON.stringify(raw));
  if (!("kind" in l) || l.kind !== "controlRequest") throw new Error("not a control request");
  return { id: l.request_id, r: l.request };
};
const mcpLine = (id: string, message: Record<string, unknown>) => request({ type: "control_request", request_id: id, request: { subtype: "mcp_message", server_name: "finisher", message: { jsonrpc: "2.0", ...message } } });
const canUseTool = (id: string, tool: string, input: unknown, toolUseId?: string) =>
  request({ type: "control_request", request_id: id, request: { subtype: "can_use_tool", tool_name: tool, input, ...(toolUseId === undefined ? {} : { tool_use_id: toolUseId }) } });
const rpcOf = (line: string | undefined) => (JSON.parse(line!) as { response: { request_id: string; response: { mcp_response: { id?: unknown; result?: Record<string, unknown>; error?: { code: number; message: string } } } } }).response;
const idOf = (events: readonly ProviderEvent[] | undefined) => {
  const e = events?.find((x) => x.type === "request");
  if (e?.type !== "request") throw new Error("no request event");
  return e.id;
};

/** A shell that records each run and answers at once. */
function recordingShell() {
  const runs: { command: string; timeoutMs: number; callId: string | undefined }[] = [];
  return {
    runs,
    tool: { run: async (i: { command: string; timeoutMs: number }, callId: string | undefined) => (runs.push({ command: i.command, timeoutMs: i.timeoutMs, callId }), { exit: 0, stdout: "ran\n", stderrTail: "" }) },
  };
}

describe("the MCP host", () => {
  test("mcp_initialize_and_tools_list: initialize (the CLI's protocol version), notifications, tools/list of what this instance serves", () => {
    const host = mcpHost(claudePermissions(), { mcpTools: [SHELL_TOOL] });
    const init = host.handshake("finisher", { jsonrpc: "2.0", id: 0, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "claude-code" } } });
    expect(init).toEqual({ mcp_response: { jsonrpc: "2.0", id: 0, result: { protocolVersion: "2025-11-25", capabilities: { tools: {} }, serverInfo: { name: "finisher", version: "1.0.0" } } } });
    expect(host.handshake("finisher", { jsonrpc: "2.0", method: "notifications/initialized" })).toEqual({ mcp_response: { jsonrpc: "2.0", result: {} } });
    const list = host.handshake("finisher", { jsonrpc: "2.0", id: 1, method: "tools/list" }) as { mcp_response: { id: unknown; result: { tools: { name: string; inputSchema: { required: string[]; properties: Record<string, unknown> } }[] } } };
    expect(list.mcp_response.id).toBe(1);
    expect(list.mcp_response.result.tools.map((t) => t.name)).toEqual(["shell"]);
    expect(list.mcp_response.result.tools[0]!.inputSchema).toMatchObject({ required: ["command"], properties: { command: { type: "string" }, timeoutMs: { type: "integer" } } });
    // Off (owner Q2a): the server is declared and answers, with no tool; the MCP permission
    // path (Decision 10) lists `approve` beside it.
    const none = mcpHost(claudePermissions(), { mcpTools: [] }).handshake("finisher", { jsonrpc: "2.0", id: 1, method: "tools/list" });
    expect(none).toEqual({ mcp_response: { jsonrpc: "2.0", id: 1, result: { tools: [] } } });
    const both = mcpHost(claudePermissions(), { mcpTools: [SHELL_TOOL, "mcp__finisher__approve"] }).handshake("finisher", { jsonrpc: "2.0", id: 1, method: "tools/list" }) as typeof list;
    expect(both.mcp_response.result.tools.map((t) => t.name)).toEqual(["shell", "approve"]);
    expect(host.handshake("other", { jsonrpc: "2.0", id: 0, method: "initialize" })).toEqual({ error: "no MCP server named other" });
  });

  test("unknown_mcp_method_is_32601 (the request's id kept); an unknown tool is -32602", async () => {
    const host = mcpHost(claudePermissions(), { mcpTools: [SHELL_TOOL] });
    for (const method of ["resources/list", "prompts/get", "ping2", ""]) {
      expect(host.handshake("finisher", { jsonrpc: "2.0", id: 7, method })).toEqual({ mcp_response: { jsonrpc: "2.0", id: 7, error: { code: -32601, message: `method not served here: ${method}` } } });
    }
    const { id, r } = mcpLine("c1", { id: 9, method: "tools/call", params: { name: "rm", arguments: {} } });
    const a = host.request(id, r);
    expect(rpcOf(a.answer).response.mcp_response).toEqual({ jsonrpc: "2.0", id: 9, error: { code: -32602, message: "tool not served here: rm" } });
    // The shell is not served on an instance launched without it.
    const off = mcpHost(claudePermissions(), { mcpTools: [] });
    const s = mcpLine("c2", { id: 10, method: "tools/call", params: { name: "shell", arguments: { command: "ls" } } });
    expect(rpcOf(off.request(s.id, s.r).answer).response.mcp_response.error).toEqual({ code: -32602, message: "tool not served here: shell" });
  });

  test("approve stays behind the same host (Decision 10's fallback): the permissions port decides it", () => {
    const host = mcpHost(claudePermissions(), { mcpTools: ["mcp__finisher__approve"] });
    const { id, r } = mcpLine("a1", { id: 3, method: "tools/call", params: { name: "approve", arguments: { tool_name: "Bash", input: { command: "ls" }, tool_use_id: "toolu_1" } } });
    const asked = host.request(id, r);
    expect(asked.answer).toBeUndefined();
    expect(asked.events).toEqual([{ type: "request", id: expect.any(String), kind: "approval", tool: "Bash", input: { command: "ls" }, providerId: "toolu_1" }]);
    const line = host.respond(idOf(asked.events), { kind: "approval", allow: true });
    const text = (rpcOf(line as string).response.mcp_response.result as { content: { text: string }[] }).content[0]!.text;
    expect(JSON.parse(text)).toEqual({ behavior: "allow", updatedInput: { command: "ls" } });
  });

  test("a shell call runs only once the runtime allowed that call; FIN_CALL is the call the runtime named", async () => {
    const sh = recordingShell();
    const host = mcpHost(claudePermissions(), { mcpTools: [SHELL_TOOL], shell: sh.tool });
    const call = (cli: string, rpc: number, args: unknown, toolUseId?: string) => {
      const { id, r } = mcpLine(cli, { id: rpc, method: "tools/call", params: { name: "shell", arguments: args, ...(toolUseId === undefined ? {} : { _meta: { "claudecode/toolUseId": toolUseId } }) } });
      return host.request(id, r);
    };
    // Never asked: refused, nothing runs, a notice.
    const cold = call("c0", 1, { command: "ls" }, "toolu_1");
    const body = rpcOf(cold.answer).response.mcp_response.result as { isError: boolean; content: { text: string }[] };
    expect(body.isError).toBe(true);
    expect(body.content[0]!.text).toMatch(/not allowed/);
    expect(cold.events).toEqual([{ type: "notice", name: "shellRefused", details: { harness: "claudeCode", reason: "notAllowed", toolUseId: "toolu_1" } }]);
    // Asked and denied: still refused.
    const d = canUseTool("p1", SHELL_TOOL, { command: "ls" }, "toolu_1");
    host.respond(idOf(host.request(d.id, d.r).events), { kind: "approval", allow: false });
    expect(call("c1", 2, { command: "ls" }, "toolu_1").later).toBeUndefined();
    // Allowed: runs once, with the runtime's call id; the same input again is refused.
    const p = canUseTool("p2", SHELL_TOOL, { command: "ls", timeoutMs: 5000 }, "toolu_2");
    host.respond(idOf(host.request(p.id, p.r).events), { kind: "approval", allow: true, callId: "01CALL" });
    // Another call's input never matches the ticket.
    expect(call("c2", 3, { command: "rm -rf x" }, "toolu_2").later).toBeUndefined();
    const ran = call("c3", 4, { command: "ls", timeoutMs: 5000 }, "toolu_2");
    expect(ran.answer).toBeUndefined();
    const out = rpcOf(await ran.later);
    expect(out.request_id).toBe("c3");
    expect(out.response.mcp_response).toEqual({ jsonrpc: "2.0", id: 4, result: { content: [{ type: "text", text: JSON.stringify({ exit: 0, stdout: "ran\n", stderrTail: "" }) }], isError: false } });
    expect(sh.runs).toEqual([{ command: "ls", timeoutMs: 5000, callId: "01CALL" }]);
    expect(call("c4", 5, { command: "ls", timeoutMs: 5000 }, "toolu_2").later).toBeUndefined();
    // Without `_meta` the input alone names the allowed call; a ticket dies with its turn.
    const q = canUseTool("p3", SHELL_TOOL, { command: "pwd" }, "toolu_3");
    host.respond(idOf(host.request(q.id, q.r).events), { kind: "approval", allow: true, callId: "01CALL3" });
    host.turnEnded?.();
    expect(call("c5", 6, { command: "pwd" }).later).toBeUndefined();
    const q2 = canUseTool("p4", SHELL_TOOL, { command: "pwd" }, "toolu_4");
    host.respond(idOf(host.request(q2.id, q2.r).events), { kind: "approval", allow: true, callId: "01CALL4" });
    await call("c6", 7, { command: "pwd" }).later;
    expect(sh.runs.at(-1)).toEqual({ command: "pwd", timeoutMs: SHELL_DEFAULT_TIMEOUT_MS, callId: "01CALL4" });
    // A bad input is the tool's own error.
    const bad = call("c7", 8, { command: 3 });
    expect((rpcOf(bad.answer).response.mcp_response.result as { isError: boolean }).isError).toBe(true);
  });

  /** A host over a recording shell, with its prompt and call helpers. */
  const hostOf = () => {
    const sh = recordingShell();
    const host = mcpHost(claudePermissions(), { mcpTools: [SHELL_TOOL], shell: sh.tool });
    const ask = (cli: string, input: unknown, toolUseId?: string) => {
      const p = canUseTool(cli, SHELL_TOOL, input, toolUseId);
      return idOf(host.request(p.id, p.r).events);
    };
    const call = (cli: string, rpc: number, args: unknown, toolUseId?: string) => {
      const { id, r } = mcpLine(cli, { id: rpc, method: "tools/call", params: { name: "shell", arguments: args, ...(toolUseId === undefined ? {} : { _meta: { "claudecode/toolUseId": toolUseId } }) } });
      return host.request(id, r);
    };
    return { sh, host, ask, call };
  };
  const refusal = (out: { events?: readonly ProviderEvent[] }) => (out.events?.[0] as { details?: { reason?: string } } | undefined)?.details?.reason;

  test("a shell prompt still waiting when its turn ends is withdrawn: a later decision makes no ticket and nothing runs", async () => {
    const { sh, host, ask, call } = hostOf();
    const id = ask("p1", { command: "ls" }, "toolu_1");
    // The turn's end withdraws it (the runtime expires its card), and the port forgets it.
    expect(host.turnEnded?.()).toEqual([{ type: "requestCancelled", id }]);
    const late = host.respond(id, { kind: "approval", allow: true, callId: "01OLD" });
    expect(typeof late).not.toBe("string");
    expect(late).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    // The next turn's call with the same input (with or without its id) is refused.
    expect(call("c1", 1, { command: "ls" }).later).toBeUndefined();
    expect(call("c2", 2, { command: "ls" }, "toolu_1").later).toBeUndefined();
    // A prompt answered before the turn ended withdraws nothing.
    const a = ask("p2", { command: "pwd" }, "toolu_2");
    host.respond(a, { kind: "approval", allow: false });
    expect(host.turnEnded?.() ?? []).toEqual([]);
    // The instance's end does the same as the turn's end.
    const b = ask("p3", { command: "ls" }, "toolu_3");
    host.closed();
    expect(host.respond(b, { kind: "approval", allow: true, callId: "01OLD2" })).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    expect(call("c3", 3, { command: "ls" }, "toolu_3").later).toBeUndefined();
    expect(sh.runs).toEqual([]);
  });

  test("identical shell calls: a call id is matched exactly; without one only an unambiguous ticket is served (fail closed)", async () => {
    const { sh, host, ask, call } = hostOf();
    // Allow u1, deny an identical u2: an identity-free call may be u2's, so it is refused.
    host.respond(ask("p1", { command: "ls" }, "u1"), { kind: "approval", allow: true, callId: "C1" });
    host.respond(ask("p2", { command: "ls" }, "u2"), { kind: "approval", allow: false });
    const free = call("c1", 1, { command: "ls" });
    expect(free.later).toBeUndefined();
    expect(refusal(free)).toBe("ambiguous");
    expect(free.events).toEqual([{ type: "notice", name: "shellRefused", details: { harness: "claudeCode", reason: "ambiguous" } }]);
    // The denied call by its id is refused; another id never takes u1's ticket.
    expect(refusal(call("c2", 2, { command: "ls" }, "u2"))).toBe("notAllowed");
    expect(refusal(call("c3", 3, { command: "ls" }, "u9"))).toBe("notAllowed");
    // u1 by its id runs, as C1.
    await call("c4", 4, { command: "ls" }, "u1").later;
    expect(sh.runs).toEqual([{ command: "ls", timeoutMs: SHELL_DEFAULT_TIMEOUT_MS, callId: "C1" }]);
    host.turnEnded?.();
    // Two identical allows: identity-free is ambiguous; by id, out of order, each gets its own FIN_CALL.
    host.respond(ask("p3", { command: "pwd" }, "u3"), { kind: "approval", allow: true, callId: "C3" });
    host.respond(ask("p4", { command: "pwd" }, "u4"), { kind: "approval", allow: true, callId: "C4" });
    expect(refusal(call("c5", 5, { command: "pwd" }))).toBe("ambiguous");
    await call("c6", 6, { command: "pwd" }, "u4").later;
    await call("c7", 7, { command: "pwd" }, "u3").later;
    expect(sh.runs.slice(1).map((r) => r.callId)).toEqual(["C4", "C3"]);
    host.turnEnded?.();
    // A pending identical prompt makes it ambiguous too; once it is denied by the runtime, still so.
    host.respond(ask("p5", { command: "id" }, "u5"), { kind: "approval", allow: true, callId: "C5" });
    const pending = ask("p6", { command: "id" }, "u6");
    expect(refusal(call("c8", 8, { command: "id" }))).toBe("ambiguous");
    host.respond(pending, { kind: "approval", allow: false });
    expect(refusal(call("c9", 9, { command: "id" }))).toBe("ambiguous");
    // A prompt the port refused at once (a reused request id) counts as a denied one.
    host.turnEnded?.();
    host.respond(ask("p7", { command: "w" }, "u7"), { kind: "approval", allow: true, callId: "C7" });
    const reused = canUseTool("p7", SHELL_TOOL, { command: "w" }, "u8");
    expect(host.request(reused.id, reused.r).events).toEqual([expect.objectContaining({ name: "reusedRequestId" })]);
    expect(refusal(call("c10", 10, { command: "w" }))).toBe("ambiguous");
    expect(sh.runs).toHaveLength(3);
  });

  test("a tools/call reusing a request id is a JSON-RPC error before any ticket is used; a completion removes only its own entry", async () => {
    const aborted: string[] = [];
    const runs: string[] = [];
    const tool = {
      run: (i: { command: string }, _c: string | undefined, signal: AbortSignal) =>
        new Promise<{ exit: number | null; stdout: string; stderrTail: string }>((res) => {
          runs.push(i.command);
          signal.addEventListener("abort", () => (aborted.push(i.command), res({ exit: null, stdout: "", stderrTail: "" })));
        }),
    };
    const host = mcpHost(claudePermissions(), { mcpTools: [SHELL_TOOL], shell: tool });
    const allow = (cli: string, command: string, toolUseId: string) => {
      const p = canUseTool(cli, SHELL_TOOL, { command }, toolUseId);
      host.respond(idOf(host.request(p.id, p.r).events), { kind: "approval", allow: true, callId: `C-${command}` });
    };
    const call = (cli: string, rpc: number, command: string) => {
      const { id, r } = mcpLine(cli, { id: rpc, method: "tools/call", params: { name: "shell", arguments: { command } } });
      return host.request(id, r);
    };
    allow("p1", "a", "u1");
    allow("p2", "b", "u2");
    expect(call("m1", 1, "a").later).toBeDefined();
    const again = call("m1", 2, "b");
    expect(again.later).toBeUndefined();
    expect(rpcOf(again.answer).response.mcp_response).toEqual({ jsonrpc: "2.0", id: 2, error: { code: -32600, message: "Finisher refuses a tools/call that reuses a request id." } });
    expect(again.events).toEqual([{ type: "notice", name: "reusedRequestId", details: { harness: "claudeCode", requestId: "m1" } }]);
    // A prompt's id is not a new call's either.
    expect(call("p2", 3, "b").answer).toBeDefined();
    // b's ticket is still there, and m1's cancel still stops a.
    expect(call("m2", 4, "b").later).toBeDefined();
    expect(runs).toEqual(["a", "b"]);
    expect(host.cancel("m1")).toEqual([]);
    expect(host.cancel("m2")).toEqual([]);
    await new Promise((r) => setTimeout(r, 0));
    expect(aborted).toEqual(["a", "b"]);
  });

  test("a running shell is aborted by notifications/cancelled, a control cancel, the turn's end or the close: no answer after", async () => {
    const aborted: string[] = [];
    const tool = {
      run: (i: { command: string }, _c: string | undefined, signal: AbortSignal) =>
        new Promise<{ exit: number | null; stdout: string; stderrTail: string }>((res) => signal.addEventListener("abort", () => (aborted.push(i.command), res({ exit: null, stdout: "", stderrTail: "" })))),
    };
    const host = mcpHost(claudePermissions(), { mcpTools: [SHELL_TOOL], shell: tool });
    const start = (n: number) => {
      const p = canUseTool(`p${n}`, SHELL_TOOL, { command: `c${n}` }, `toolu_${n}`);
      host.respond(idOf(host.request(p.id, p.r).events), { kind: "approval", allow: true, callId: `C${n}` });
      const { id, r } = mcpLine(`m${n}`, { id: 100 + n, method: "tools/call", params: { name: "shell", arguments: { command: `c${n}` } } });
      return host.request(id, r).later!;
    };
    const a = start(1);
    expect(host.handshake("finisher", { jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: 101 } })).toEqual({ mcp_response: { jsonrpc: "2.0", result: {} } });
    expect(await a).toBeUndefined();
    const b = start(2);
    expect(host.cancel("m2")).toEqual([]);
    expect(await b).toBeUndefined();
    const c = start(3);
    host.turnEnded?.();
    expect(await c).toBeUndefined();
    const d = start(4);
    host.closed();
    expect(await d).toBeUndefined();
    expect(aborted).toEqual(["c1", "c2", "c3", "c4"]);
  });
});

// ---- the shell tool ----

/** A box that starts processes as the sandbox's direct mode does (`startProcess`), recording each spawn. */
function directBox(workspace: string) {
  const spawns: { argv: readonly string[]; opts: SpawnOptions }[] = [];
  const box: ShellBox = {
    workspace,
    spawn: (argv, opts) =>
      Effect.promise(() => {
        spawns.push({ argv, opts });
        // Windows: PowerShell needs `SystemRoot` to start.
        const sys = WIN && process.env.SystemRoot !== undefined ? { SystemRoot: process.env.SystemRoot } : {};
        return startProcess(undefined, argv, opts.cwd, { PATH: process.env.PATH ?? "/usr/bin:/bin", ...sys, ...opts.env }, process.platform, () => {});
      }),
  };
  return { box, spawns };
}
const never = new AbortController().signal;

let tmp: string;
beforeEach(() => {
  // `native`: Windows' short (8.3) names spelled out, as PowerShell prints the location.
  tmp = realpathSync.native(mkdtempSync(join(tmpdir(), "agent-claude-shell-")));
});

describe("the shell tool", () => {
  test("one interface: pwsh on Windows, sh elsewhere", () => {
    expect(shellFor("win32")).toBe(PWSH);
    expect(shellFor("linux")).toBe(SH);
    expect(shellFor("darwin")).toBe(SH);
    expect(SH.argv("echo hi")).toEqual(["/bin/sh", "-c", "echo hi"]);
    expect(PWSH.argv("Get-Date")).toEqual(["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "Get-Date"]);
  });

  test("shellInput: a command; timeoutMs defaults to the CLI's Bash default and is capped at its maximum", () => {
    expect(shellInput({ command: "ls" })).toEqual({ command: "ls", timeoutMs: SHELL_DEFAULT_TIMEOUT_MS });
    expect(SHELL_DEFAULT_TIMEOUT_MS).toBe(120_000);
    expect(shellInput({ command: "ls", timeoutMs: 10 ** 9 })).toEqual({ command: "ls", timeoutMs: SHELL_MAX_TIMEOUT_MS });
    expect(shellInput({ command: "ls", timeoutMs: 50 })).toEqual({ command: "ls", timeoutMs: 50 });
    for (const bad of [null, {}, { command: "" }, { command: 3 }, { command: "ls", timeoutMs: 0 }, { command: "ls", timeoutMs: 1.5 }, { command: "ls", timeoutMs: "9" }, { command: "ls", extra: 1 }]) {
      expect(typeof shellInput(bad), JSON.stringify(bad)).toBe("string");
    }
  });

  withShell("shell_call_runs_through_session_sandbox (unit): a fresh process of the box, cwd the workspace, FIN_CALL set; {exit, stdout, stderrTail}", async () => {
    const { box, spawns } = directBox(tmp);
    const sh = shellTool({ box, os: process.platform });
    const r = await sh.run({ command: CMD.whereAndCall, timeoutMs: 10_000 }, "01JCALL", never);
    expect(r).toEqual({ exit: 3, stdout: `${tmp}\n01JCALL\n`, stderrTail: "oops\n" });
    expect(spawns).toEqual([{ argv: SHELL.argv(CMD.whereAndCall), opts: { cwd: tmp, stdio: "pipe", env: { FIN_CALL: "01JCALL" } } }]);
    // No call id: no FIN_CALL at all (never an empty one).
    await sh.run({ command: CMD.ok, timeoutMs: 10_000 }, undefined, never);
    expect(spawns[1]!.opts.env).toEqual({});
  });

  withShell("output is capped like the CLI's Bash: stdout's head and stderr's tail, SHELL_OUTPUT_MAX characters each", async () => {
    const { box } = directBox(tmp);
    const n = SHELL_OUTPUT_MAX + 5_000;
    const r = await shellTool({ box, os: process.platform }).run({ command: CMD.floods(n), timeoutMs: 10_000 }, undefined, never);
    expect(r.exit).toBe(0);
    expect(r.stdout).toBe("a".repeat(SHELL_OUTPUT_MAX));
    expect(r.stderrTail).toBe(`${"c".repeat(SHELL_OUTPUT_MAX - 1)}d`);
    expect(r.truncated).toBe(true);
    expect(SHELL_OUTPUT_MAX).toBe(30_000);
  });

  withShell("a cut never splits a character: an emoji across stdout's and stderr's boundary is dropped whole", async () => {
    const { box } = directBox(tmp);
    const r = await shellTool({ box, os: process.platform }).run({ command: CMD.emojiAtCuts(SHELL_OUTPUT_MAX - 1), timeoutMs: 10_000 }, undefined, never);
    expect(r.exit).toBe(0);
    expect(r.stdout.isWellFormed()).toBe(true);
    expect(r.stderrTail.isWellFormed()).toBe(true);
    expect(r.stdout).toBe("a".repeat(SHELL_OUTPUT_MAX - 1));
    expect(r.stderrTail).toBe("c".repeat(SHELL_OUTPUT_MAX - 1));
    expect(r.truncated).toBe(true);
  });

  test("capOutput: SHELL_OUTPUT_MAX UTF-16 code units, cut at a code point", () => {
    const e = "\u{1F600}";
    const head = capOutput(`${"a".repeat(SHELL_OUTPUT_MAX - 1)}${e}bbb`, "head");
    expect(head).toBe("a".repeat(SHELL_OUTPUT_MAX - 1));
    const tail = capOutput(`xx${e}${"c".repeat(SHELL_OUTPUT_MAX - 1)}`, "tail");
    expect(tail).toBe("c".repeat(SHELL_OUTPUT_MAX - 1));
    // A whole pair inside the cap is kept.
    expect(capOutput(`${"a".repeat(SHELL_OUTPUT_MAX - 2)}${e}b`, "head")).toBe(`${"a".repeat(SHELL_OUTPUT_MAX - 2)}${e}`);
    expect(capOutput(`x${e}${"c".repeat(SHELL_OUTPUT_MAX - 2)}`, "tail")).toBe(`${e}${"c".repeat(SHELL_OUTPUT_MAX - 2)}`);
    for (const s of [head, tail]) expect(s.isWellFormed()).toBe(true);
    expect(capOutput("short", "head")).toBe("short");
  });

  withShell("shell_timeout_kills_group: the shell and what it started are gone at the timeout", async () => {
    const { box } = directBox(tmp);
    const t0 = Date.now();
    const r = await shellTool({ box, os: process.platform }).run({ command: CMD.groupThenWait, timeoutMs: WIN ? 3_000 : 400 }, undefined, never);
    expect(Date.now() - t0).toBeLessThan(10_000);
    expect(r).toMatchObject({ exit: null, timedOut: true, stdout: "started\n" });
    const bg = Number(readFileSync(join(tmp, "bg.pid"), "utf8"));
    expect(bg).toBeGreaterThan(0);
    const alive = () => {
      try {
        process.kill(bg, 0);
        return true;
      } catch {
        return false;
      }
    };
    const end = Date.now() + 5_000;
    while (alive() && Date.now() < end) await new Promise((res) => setTimeout(res, 20));
    expect(alive()).toBe(false);
  });

  withShell("an abort kills it the same way (cancelled)", async () => {
    const { box } = directBox(tmp);
    const ac = new AbortController();
    const running = shellTool({ box, os: process.platform }).run({ command: CMD.wait, timeoutMs: 60_000 }, undefined, ac.signal);
    setTimeout(() => ac.abort(), 200);
    expect(await running).toMatchObject({ exit: null, cancelled: true });
  });

  test("a spawn the sandbox refuses is the call's error result, never a throw", async () => {
    const box: ShellBox = { workspace: tmp, spawn: () => Effect.fail({ code: "SANDBOX_UNAVAILABLE", message: "no sandbox", details: {} } as never) };
    expect(await shellTool({ box, os: "linux" }).run({ command: "ls", timeoutMs: 1000 }, undefined, never)).toEqual({ exit: null, stdout: "", stderrTail: "SANDBOX_UNAVAILABLE: no sandbox" });
  });
});

// ---- the launch ----

describe("the launch", () => {
  const SID = "01JC8Z5Q3M4N6P7R8S9T0VWXYZ";
  const PROBE: ProbeResult = { version: "2.1.321", permissionPath: "stdio", tools: [], models: [], capabilities: [] };
  const spec: SessionSpec = {
    sessionId: SID,
    projectId: "P1",
    model: "default",
    role: "editor",
    mode: "draft",
    system: "role",
    cwd: `C:\\data\\workspace\\${SID}`,
    env: { TMPDIR: `C:\\data\\agents\\${SID}\\tmp` },
    providerRef: null,
    tools: ["fin", "Read", "Edit", "Bash"],
  };
  const plan = (brokeredShell: boolean | undefined) =>
    launchPlan({ id: SID, harness: "claudeCode" } as Session, spec, { ...PROBE, ...(brokeredShell === undefined ? {} : { brokeredShell }) }, "win32", { binary: "C:\\claude\\claude.exe", retentionDays: 90, remainingMoneyMicros: null, planSignIn: true });
  const flag = (argv: readonly string[], name: string) => argv[argv.indexOf(name) + 1];
  const body = (p: ReturnType<typeof plan>, name: string) => JSON.parse(p.files.find((f) => f.path.endsWith(name))!.body) as Record<string, unknown>;

  test("no_bash_in_tools_on_windows: brokeredShell on: no Bash in --tools, the shell served and asked; off (the default, owner Q2a): Bash as on Linux", () => {
    const on = plan(true);
    expect(flag(on.argv, "--tools")).toBe("Read,Edit");
    expect(on.tools).not.toContain("Bash");
    expect(on.mcpTools).toEqual([SHELL_TOOL]);
    expect((body(on, "claude.json").permissions as { ask: string[] }).ask).toEqual(["Read", "Edit", SHELL_TOOL]);
    for (const p of [plan(undefined), plan(false)]) {
      expect(flag(p.argv, "--tools")).toBe("Read,Edit,Bash");
      expect(p.mcpTools).toEqual([]);
    }
    // The `finisher` SDK server is declared either way (Decision 10's fallback and 7G use it).
    for (const p of [on, plan(undefined)]) {
      expect(body(p, "mcp.json")).toEqual({ mcpServers: { finisher: { type: "sdk", name: "finisher" } } });
      expect(p.argv).toContain("--strict-mcp-config");
    }
  });
});

// ---- through the runtime, over the fake ----

const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY, shell: process.env[BROKERED_SHELL_TEST_ENV] };
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
  process.env.FIN_TEST_SANDBOX_READY = "agent";
  process.env[BROKERED_SHELL_TEST_ENV] = "1";
});
afterAll(() => {
  for (const [k, v] of [["FIN_TEST_HOOKS", saved.hooks], ["FIN_TEST_SANDBOX_READY", saved.ready], [BROKERED_SHELL_TEST_ENV, saved.shell]] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
});

let server: AgentServer | undefined;
let restore: (() => SandboxStatus) | undefined;
afterEach(async () => {
  await server?.stop();
  server = undefined;
  if (restore !== undefined) setSandboxStatusSource(restore);
  restore = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

/** The `agent` profile not ready: `direct` mode, every shell call asks a person (owner Q3a). */
const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });

interface Running {
  readonly local: NdjsonClient;
  readonly owner: NdjsonClient;
  readonly projectId: string;
  readonly serverData: string;
  /** The rules added, by command. */
  readonly rules: Record<string, string>;
}

type Observed = {
  pid: number;
  argv: string[];
  answers?: { tool: string; answer: { behavior?: string } }[];
  mcpCalls?: { tool: string; toolUseId: string; result: { content: { type: string; text: string }[]; isError: boolean } }[];
  controlResponses?: { requestId: string; subtype: string; raw: string }[];
}[];

/** One turn of brokered shell calls (each announced, asked, called over MCP when allowed), then the turn's result. */
const shellTurn = (...inputs: Record<string, unknown>[]) => ({
  init: { apiKeySource: "/login managed key" },
  steps: [...inputs.map((input, i) => ({ ...(i === 0 ? { expect: "user" } : {}), ask: { tool: SHELL_TOOL, input, mcp: true } })), { cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }],
});

async function claudeServer(script: Record<string, unknown>, rules: { effect: string; command: string }[] = []): Promise<Running> {
  const previous = setSandboxStatusSource(NOT_READY);
  restore ??= previous;
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, `claude${EXE}`);
  copyFileSync(FAKE, binary);
  writeFileSync(join(dir, "script.json"), JSON.stringify(script));
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: "@config/observed.json", version: VERSION }));
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: binary }, ...(existsSync(FIN) ? { finPath: FIN } : {}) } }));
  signInClaude(serverData);
  server = new AgentServer(serverData, { timers: false });
  await server.start();
  const local = await server.local();
  const owner = await server.console();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaaP", 0, [addAsset()])));
  const ids: Record<string, string> = {};
  for (const r of rules) ids[r.command] = ok<{ rule: { id: string } }>(await confirmed(owner, "agent.rules.add", { ...r, projectId })).rule.id;
  return { local, owner, projectId, serverData, rules: ids };
}

const start = async (r: Running) => ok<{ session: Session }>(await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt: "go" })).session;

async function until<T>(f: () => T | undefined, ms = 20_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((res) => setTimeout(res, 20));
  }
}
const store = () => server!.core.store;
const settle = (id: string) => until(() => (["idle", "interrupted", "ended"].includes(store().get(id)!.status) ? store().get(id)! : undefined));
const nodes = (id: string): Node[] => store().nodes(id, 1, 1000) as Node[];
const callIds = (id: string) => nodes(id).filter((n) => n.kind === "toolCall").map((n) => n.callId!);
const results = (id: string) => nodes(id).filter((n) => n.kind === "toolResult").map((n) => n.payload as { ok: boolean; output: unknown });
const approvalNodes = (id: string) => nodes(id).filter((n) => n.kind === "approval").map((n) => n.payload as Record<string, unknown>);
const observed = (r: Running, id: string): Observed => {
  const p = join(r.serverData, "agents", id, "provider", "observed.json");
  return existsSync(p) ? (JSON.parse(readFileSync(p, "utf8")) as Observed) : [];
};
/** The shell's result as the CLI got it (`{exit, stdout, stderrTail}`). */
const shellResults = (r: Running, id: string): Record<string, unknown>[] => observed(r, id).flatMap((o) => o.mcpCalls ?? []).map((c) => ({ ...(JSON.parse(c.result.content[0]!.text) as Record<string, unknown>), isError: c.result.isError }));

/** Answers each shell card allow as it opens (owner Q3a: while the profile is off every shell call asks). */
async function allowCards(r: Running, id: string, n: number): Promise<void> {
  for (let i = 0; i < n; i++) {
    const card = await until(() => store().approvals(id).find((a) => a.status === "open"));
    ok(await confirmed(r.owner, "agent.approval.answer", { id: card.id, decision: "allow" }));
  }
}

describe("over fake-claude", () => {
  withFake("mcp_initialize_and_tools_list: the handshake is answered, the server connected, the shell listed and nothing else", async () => {
    const r = await claudeServer({ init: { apiKeySource: "/login managed key" }, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] });
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    const [o] = observed(r, s.id);
    expect(o!.argv[o!.argv.indexOf("--tools") + 1]).not.toMatch(/Bash/);
    // What finisherd answered the fake's handshake (every answer it sent, byte for byte).
    const rpcs = (o!.controlResponses ?? []).map((c) => (JSON.parse(c.raw) as { response: { response?: { mcp_response?: { result?: Record<string, unknown> } } } }).response.response?.mcp_response).filter((m) => m !== undefined);
    expect(rpcs[0]!.result).toMatchObject({ protocolVersion: "2025-11-25", capabilities: { tools: {} }, serverInfo: { name: "finisher" } });
    expect(rpcs.map((m) => (m.result?.tools as { name: string }[] | undefined)?.map((t) => t.name)).filter((t) => t !== undefined)).toEqual([["shell"]]);
    // The init passed the gate: `finisher` connected, its shell the instance's only MCP tool.
    expect(nodes(s.id).filter((n) => n.kind === "assistantMessage" || n.kind === "notice").map((n) => JSON.stringify(n.payload))).not.toContainEqual(expect.stringMatching(/PROVIDER_VERSION|isolation/));
  });

  withFakeShell("shell_call_runs_through_session_sandbox: a fresh process finisherd starts in the workspace, the session environment and FIN_CALL; the CLI gets {exit, stdout, stderrTail}", async () => {
    const r = await claudeServer(shellTurn({ command: CMD.sessionView }));
    const s = await start(r);
    await allowCards(r, s.id, 1);
    expect((await settle(s.id)).status).toBe("idle");
    const [call] = callIds(s.id);
    const [res] = shellResults(r, s.id);
    const [cwd, finCall, tmpDir, ppid] = String(res!.stdout).split("\n");
    expect(cwd).toMatch(workspaceTail(s.id));
    expect(finCall).toBe(call);
    expect(tmpDir).toBe(join(r.serverData, "agents", s.id, "tmp"));
    // Started by finisherd (this process, `direct` mode), not by the CLI.
    expect(Number(ppid)).toBe(process.pid);
    expect(Number(ppid)).not.toBe(observed(r, s.id)[0]!.pid);
    expect(res).toMatchObject({ exit: 3, stderrTail: "oops\n", isError: true });
    expect(results(s.id)).toEqual([expect.objectContaining({ ok: false })]);
  }, 60_000);

  withFin("shell_fin_call_is_exact: the shell's fin commit carries its call, attribution exact", async () => {
    const r = await claudeServer(shellTurn({ command: CMD.fin(FIN, "bin add --name brokered", "out.json") }));
    const s = await start(r);
    await allowCards(r, s.id, 1);
    expect((await settle(s.id)).status).toBe("idle");
    expect(shellResults(r, s.id)).toEqual([expect.objectContaining({ exit: 0, isError: false })]);
    const [call] = callIds(s.id);
    const txs = store().txsOfSession(s.id);
    expect(txs).toEqual([expect.objectContaining({ callId: call, attribution: "exact" })]);
    expect(store().txsOfCall(call!).map((t: SessionTx) => t.txId)).toEqual(txs.map((t) => t.txId));
  }, 60_000);

  withFake("shell_permission_named_like_bash: rules written for tool:Bash(...) govern mcp__finisher__shell(<command>)", async () => {
    // The request's names: its own first, then the CLI's Bash's (Decision 21).
    expect(toolNames(SHELL_TOOL, { command: "curl https://example.invalid" })).toEqual([
      "tool:mcp__finisher__shell",
      "tool:mcp__finisher__shell(curl https://example.invalid)",
      "tool:Bash",
      "tool:Bash(curl https://example.invalid)",
    ]);
    const r = await claudeServer(shellTurn({ command: "curl https://example.invalid" }, { command: "rm -rf scratch" }), [
      { effect: "ask", command: "tool:Bash(curl *)" },
      { effect: "deny", command: "tool:Bash(rm *)" },
    ]);
    const s = await start(r);
    const card = await until(() => store().approvals(s.id).find((a) => a.status === "open"));
    // The Bash rule's card, not the managed one every shell call gets while the profile is off.
    expect(card.subject).toMatchObject({ rule: r.rules["tool:Bash(curl *)"], layer: "local" });
    ok(await confirmed(r.owner, "agent.approval.answer", { id: card.id, decision: "deny" }));
    expect((await settle(s.id)).status).toBe("idle");
    expect(approvalNodes(s.id).filter((p) => p.tool === SHELL_TOOL && p.decision === "deny").map((p) => [p.by, p.reason ?? null])).toEqual([
      ["user", null],
      ["policy", "rule"],
    ]);
    // Neither ran: the CLI was refused both, no tools/call went out.
    expect(observed(r, s.id)[0]!.answers!.map((a) => [a.tool, a.answer.behavior])).toEqual([
      [SHELL_TOOL, "deny"],
      [SHELL_TOOL, "deny"],
    ]);
    expect(shellResults(r, s.id)).toEqual([]);
  }, 60_000);
});
