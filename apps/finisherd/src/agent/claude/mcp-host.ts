// The `finisher` SDK MCP server (plan 7D Decisions 10 and 21, Task 13): the one host `finisherd`
// serves over `mcp_message` control requests, as the Agent SDK's in-process servers are served.
// The launch declares it on every OS (`--mcp-config`, type `sdk`); what it lists is the
// instance's `mcpTools`.
//
// - **The handshake** (`handshake`, answered at once by the instance, before and after the
//   first-turn gate): `initialize` (the CLI's `protocolVersion` echoed: verified on 2.1.321, 7D
//   Task 0, "2025-11-25"), every `notifications/*`, `tools/list` of the instance's tools; any
//   other method is JSON-RPC `-32601`, a message for another server an `error` answer.
//   `notifications/cancelled {requestId}` also stops that running call.
// - **`tools/call`** (`RequestPort.request`, behind the gate like any CLI request): `approve`
//   (Decision 10's documented fallback) goes to the permissions port (`permissions.ts`) as
//   before; `shell` (Decision 21's brokered shell) runs `shell-tool.ts`; any other name is
//   `-32602` (MCP: an unknown tool). The shell's answer comes when the process ends (`later`).
// - **A shell call runs only once the runtime allowed it** (Review Focus 3): the CLI asks for
//   `mcp__finisher__shell` with `can_use_tool` like any tool (Decision 4's `ask` list), the
//   runtime decides it through 7B's `permissions.ts` (where `tool:Bash(...)` rules hold for it
//   too), and its allow leaves a one-use ticket: the call's input (compared as canonical JSON),
//   its `tool_use_id` and the runtime's call id. A `tools/call` runs only by consuming the ticket
//   with the same input (and the same `tool_use_id` when the CLI sends `_meta
//   ["claudecode/toolUseId"]`); otherwise it is refused (`isError`, a `shellRefused` notice) and
//   nothing starts. Tickets die with their turn (`turnEnded`). The call id becomes the process's
//   `FIN_CALL`, so its `fin` is attributed `exact`.
// - **Stopping**: `notifications/cancelled`, a `control_cancel_request` of the `tools/call`, the
//   turn's `result` (`turnEnded`: no answer is awaited after it) and the instance's end
//   (`closed`) abort a running shell (its process group killed); an aborted call is not answered.
//
// TODO(7D Task 13 Step 0, a Windows host): whether 2.1.321 sends `_meta["claudecode/toolUseId"]`
// in `tools/call` and `notifications/cancelled` on an interrupt (fake-claude does both).
import type { FinError, ProviderEvent } from "@finisher/contracts";
import type { RuntimeAnswer } from "../adapter.ts";
import { MCP_SERVER, SHELL_TOOL } from "./launch.ts";
import { type CliRequest, encodeAnswer, type McpReply } from "./protocol.ts";
import type { RequestPort } from "./session.ts";
import { type ShellResult, type ShellTool, shellInput } from "./shell-tool.ts";

/** An MCP `tools/call`: a CLI request like `can_use_tool`, never part of the handshake. */
export const isMcpToolCall = (message: unknown): boolean => typeof message === "object" && message !== null && (message as { method?: unknown }).method === "tools/call";

/** JSON with object keys sorted, so the same input matches whatever order its keys came in. */
const stable = (v: unknown): string => {
  if (Array.isArray(v)) return `[${v.map(stable).join(",")}]`;
  if (typeof v === "object" && v !== null) {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o).sort().map((k) => `${JSON.stringify(k)}:${stable(o[k])}`).join(",")}}`;
  }
  return JSON.stringify(v) ?? "null";
};
/** One call as a key: its tool and input. */
export const callKey = (tool: string, input: unknown) => `${tool}\u0000${stable(input)}`;

const MCP_TOOL_SCHEMAS: Readonly<Record<string, { description: string; inputSchema: Record<string, unknown> }>> = {
  shell: {
    description: "Runs a command in the session's shell.",
    inputSchema: { type: "object", properties: { command: { type: "string" }, timeoutMs: { type: "integer", minimum: 1 } }, required: ["command"] },
  },
  approve: {
    description: "Asks Finisher whether a tool call may run.",
    inputSchema: { type: "object", properties: { tool_name: { type: "string" }, input: { type: "object" }, tool_use_id: { type: "string" } }, required: ["tool_name", "input"] },
  },
};

/**
 * The answer to one handshake `mcp_message` for the `finisher` server (module docs): the
 * probe's and every instance's. `tools/call` is the session's (`mcpHost`).
 */
export function mcpReply(serverName: string, message: unknown, mcpTools: readonly string[]): McpReply | { readonly error: string } {
  if (serverName !== MCP_SERVER) return { error: `no MCP server named ${serverName.slice(0, 64)}` };
  const m = (typeof message === "object" && message !== null ? message : {}) as { method?: unknown; id?: unknown; params?: { protocolVersion?: unknown } };
  const id = m.id ?? null;
  const ok = (result: unknown): McpReply => ({ mcp_response: { jsonrpc: "2.0", id, result } });
  const method = typeof m.method === "string" ? m.method : "";
  if (method === "initialize") {
    const protocolVersion = typeof m.params?.protocolVersion === "string" ? m.params.protocolVersion : "2025-11-25";
    return ok({ protocolVersion, capabilities: { tools: {} }, serverInfo: { name: MCP_SERVER, version: "1.0.0" } });
  }
  if (method.startsWith("notifications/")) return { mcp_response: { jsonrpc: "2.0", result: {} } };
  if (method === "tools/list") {
    const prefix = `mcp__${MCP_SERVER}__`;
    const tools = mcpTools.filter((t) => t.startsWith(prefix)).map((t) => t.slice(prefix.length)).filter((t) => Object.hasOwn(MCP_TOOL_SCHEMAS, t));
    return ok({ tools: tools.map((name) => ({ name, ...MCP_TOOL_SCHEMAS[name]! })) });
  }
  return { mcp_response: { jsonrpc: "2.0", id, error: { code: -32601, message: `method not served here: ${method.slice(0, 64)}` } } };
}

/** The host as an instance uses it: a `RequestPort` that also answers the handshake. */
export interface McpHostPort extends RequestPort {
  handshake(serverName: string, message: unknown): McpReply | { readonly error: string };
  /** The instance ended: running shells are stopped, tickets dropped. */
  closed(): void;
}

export interface McpHostOptions {
  /** The instance's `mcp__finisher__*` tools (`LaunchPlan.mcpTools`): `tools/list` and what `tools/call` serves. */
  readonly mcpTools: readonly string[];
  /** The brokered shell (`shell-tool.ts`), when the instance serves `mcp__finisher__shell`. */
  readonly shell?: ShellTool;
}

interface Ticket {
  readonly key: string;
  readonly toolUseId?: string;
  readonly callId?: string;
}

const clip = (s: string) => s.slice(0, 64);
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const rpcError = (id: unknown, code: number, message: string): McpReply => ({ mcp_response: { jsonrpc: "2.0", id, error: { code, message } } });
const toolResult = (id: unknown, text: string, isError: boolean): McpReply => ({ mcp_response: { jsonrpc: "2.0", id, result: { content: [{ type: "text", text }], isError } } });

/** The `finisher` server of one instance, over `port` (its permissions: `claudePermissions()`). */
export function mcpHost(port: RequestPort, o: McpHostOptions): McpHostPort {
  const served = o.mcpTools.includes(SHELL_TOOL) && o.shell !== undefined ? o.shell : undefined;
  /** The runtime's ids of the shell prompts waiting for a decision. */
  const asked = new Map<string, { readonly key: string; readonly toolUseId?: string }>();
  let tickets: Ticket[] = [];
  /** Running calls by the CLI's request id, with their JSON-RPC id. */
  const running = new Map<string, { readonly rpc: unknown; readonly stop: AbortController }>();
  const stopAll = () => {
    for (const r of running.values()) r.stop.abort();
    running.clear();
  };

  const shellCall = (cli: string, rpc: unknown, params: Record<string, unknown>) => {
    if (served === undefined) return { answer: encodeAnswer(cli, rpcError(rpc, -32602, "tool not served here: shell")) };
    const input = shellInput(params.arguments);
    if (typeof input === "string") return { answer: encodeAnswer(cli, toolResult(rpc, input, true)) };
    const meta = isObject(params._meta) ? params._meta["claudecode/toolUseId"] : undefined;
    const toolUseId = typeof meta === "string" ? meta : undefined;
    const key = callKey(SHELL_TOOL, params.arguments);
    const i = tickets.findIndex((t) => t.key === key && (toolUseId === undefined || t.toolUseId === undefined || t.toolUseId === toolUseId));
    if (i < 0) {
      const event: ProviderEvent = { type: "notice", name: "shellRefused", details: { harness: "claudeCode", reason: "notAllowed", ...(toolUseId === undefined ? {} : { toolUseId: clip(toolUseId) }) } };
      return { answer: encodeAnswer(cli, toolResult(rpc, "Finisher has not allowed this shell call.", true)), events: [event] };
    }
    const [ticket] = tickets.splice(i, 1);
    const stop = new AbortController();
    running.set(cli, { rpc, stop });
    const later = served.run(input, ticket!.callId, stop.signal).then((r: ShellResult) => {
      const aborted = stop.signal.aborted;
      running.delete(cli);
      return aborted ? undefined : encodeAnswer(cli, toolResult(rpc, JSON.stringify(r), r.exit !== 0));
    });
    return { later };
  };

  return {
    handshake(serverName, message) {
      const m = isObject(message) ? message : {};
      if (serverName === MCP_SERVER && m.method === "notifications/cancelled" && isObject(m.params)) {
        const target = m.params.requestId;
        for (const r of running.values()) if (r.rpc === target) r.stop.abort();
      }
      return mcpReply(serverName, message, o.mcpTools);
    },

    request(id: string, r: CliRequest) {
      if (r.subtype === "mcp_message" && r.server_name === MCP_SERVER && isMcpToolCall(r.message)) {
        const m = r.message as { id?: unknown; params?: unknown };
        const params = isObject(m.params) ? m.params : {};
        if (params.name === "shell") return shellCall(id, m.id ?? null, params);
      }
      const out = port.request(id, r);
      // A prompt for the brokered shell: its allow leaves a ticket (module docs).
      for (const e of out.events ?? []) {
        if (e.type === "request" && e.tool === SHELL_TOOL) asked.set(e.id, { key: callKey(SHELL_TOOL, e.input), ...(e.providerId === undefined ? {} : { toolUseId: e.providerId }) });
      }
      return out;
    },

    respond(id: string, a: RuntimeAnswer): string | FinError {
      const line = port.respond(id, a);
      const prompt = asked.get(id);
      if (typeof line === "string" && prompt !== undefined) {
        asked.delete(id);
        if (a.kind === "approval" && a.allow) tickets.push({ ...prompt, ...(a.callId === undefined ? {} : { callId: a.callId }) });
      }
      return line;
    },

    cancel(requestId: string) {
      const r = running.get(requestId);
      if (r !== undefined) {
        r.stop.abort();
        return [];
      }
      const events = port.cancel(requestId);
      for (const e of events) if (e.type === "requestCancelled") asked.delete(e.id);
      return events;
    },

    ...(port.asked === undefined ? {} : { asked: port.asked }),

    turnEnded() {
      port.turnEnded?.();
      tickets = [];
      stopAll();
    },

    closed() {
      tickets = [];
      asked.clear();
      stopAll();
    },
  };
}
