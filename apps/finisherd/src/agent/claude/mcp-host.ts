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
//   its `tool_use_id` and the runtime's call id. A `tools/call` runs only by consuming a ticket
//   with the same input; otherwise it is refused (`isError`, a `shellRefused` notice) and
//   nothing starts. The call id becomes the process's `FIN_CALL`, so its `fin` is attributed
//   `exact`.
// - **Which ticket, failing closed** (review of Task 13): a call carrying `_meta
//   ["claudecode/toolUseId"]` takes only the ticket of that `tool_use_id` (or, failing one, an
//   unambiguous ticket that names none). A call without it takes a ticket only when the match is
//   unambiguous: exactly one live ticket for that input, and no other prompt of this turn for the
//   same input that is waiting, was denied, was withdrawn or was refused at once by the port.
//   Anything else is refused (`reason: "ambiguous"`), so a denied or never-asked call cannot take
//   another call's allow, and out-of-order calls never swap their `FIN_CALL`s.
// - **Turn boundaries** (review of Task 13): at the turn's `result` (`turnEnded`) and the
//   instance's end (`closed`) every ticket dies and every shell prompt still waiting is withdrawn
//   from the port (`turnEnded` returns its `requestCancelled`, so the runtime expires the card);
//   a decision for it arriving later is `PROVIDER_PROTOCOL` and makes no ticket.
// - **One CLI request id per request**: a `tools/call` of the shell reusing any id this host has
//   seen is JSON-RPC `-32600` with a `reusedRequestId` notice, before any ticket is looked at.
// - **Stopping**: `notifications/cancelled`, a `control_cancel_request` of the `tools/call`, the
//   turn's `result` (`turnEnded`: no answer is awaited after it) and the instance's end
//   (`closed`) abort a running shell (its process group killed); an aborted call is not answered.
//
// TODO(7D Task 13 Step 0, a Windows host): whether 2.1.321 sends `_meta["claudecode/toolUseId"]`
// in `tools/call` and `notifications/cancelled` on an interrupt (fake-claude does both).
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { finError } from "../../errors.ts";
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
  /** The turn's `result`: tickets dropped, running shells stopped; the shell prompts it withdrew. */
  turnEnded(): readonly ProviderEvent[];
  /** The instance ended: running shells are stopped, tickets dropped, waiting shell prompts withdrawn. */
  closed(): void;
}

export interface McpHostOptions {
  /** The instance's `mcp__finisher__*` tools (`LaunchPlan.mcpTools`): `tools/list` and what `tools/call` serves. */
  readonly mcpTools: readonly string[];
  /** The brokered shell (`shell-tool.ts`), when the instance serves `mcp__finisher__shell`. */
  readonly shell?: ShellTool;
}

/** A shell prompt of this turn: waiting, allowed (a ticket), used, or not allowed (denied, withdrawn, refused). */
interface ShellAsk {
  readonly cli: string;
  readonly key: string;
  readonly toolUseId?: string;
  state: "waiting" | "allowed" | "used" | "notAllowed";
  callId?: string;
}

/** The tool, input and `tool_use_id` a permission prompt asks about (`can_use_tool`, or the MCP `approve`). */
function promptOf(r: CliRequest): { tool: string; input: unknown; toolUseId?: string } | undefined {
  if (r.subtype === "can_use_tool") return { tool: r.tool_name, input: r.input, ...(r.tool_use_id === undefined ? {} : { toolUseId: r.tool_use_id }) };
  if (r.subtype !== "mcp_message" || r.server_name !== MCP_SERVER || !isMcpToolCall(r.message)) return undefined;
  const p = (r.message as { params?: unknown }).params;
  const args = isObject(p) && p.name === "approve" && isObject(p.arguments) ? p.arguments : undefined;
  if (args === undefined || typeof args.tool_name !== "string") return undefined;
  return { tool: args.tool_name, input: args.input, ...(typeof args.tool_use_id === "string" ? { toolUseId: args.tool_use_id } : {}) };
}

const clip = (s: string) => s.slice(0, 64);
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const rpcError = (id: unknown, code: number, message: string): McpReply => ({ mcp_response: { jsonrpc: "2.0", id, error: { code, message } } });
const toolResult = (id: unknown, text: string, isError: boolean): McpReply => ({ mcp_response: { jsonrpc: "2.0", id, result: { content: [{ type: "text", text }], isError } } });

/** The `finisher` server of one instance, over `port` (its permissions: `claudePermissions()`). */
export function mcpHost(port: RequestPort, o: McpHostOptions): McpHostPort {
  const served = o.mcpTools.includes(SHELL_TOOL) && o.shell !== undefined ? o.shell : undefined;
  /** This turn's shell prompts, by the runtime's id (a port refusal: by `!<cli>`). */
  const asks = new Map<string, ShellAsk>();
  /** The runtime's ids of shell prompts whose turn ended undecided: their decisions are refused. */
  const ended = new Set<string>();
  /** Every CLI request id this host has been given. */
  const seen = new Set<string>();
  /** Running calls by the CLI's request id, with their JSON-RPC id. */
  const running = new Map<string, { readonly rpc: unknown; readonly stop: AbortController }>();
  const stopAll = () => {
    for (const r of running.values()) r.stop.abort();
    running.clear();
  };
  /** Ends the turn's prompts: the waiting ones withdrawn from the port (its events), the rest dropped. */
  const endAsks = (): ProviderEvent[] => {
    const out: ProviderEvent[] = [];
    for (const [id, a] of asks) {
      if (a.state !== "waiting") continue;
      ended.add(id);
      out.push(...port.cancel(a.cli).filter((e) => e.type === "requestCancelled"));
    }
    asks.clear();
    return out;
  };
  /** The ticket a call takes (module docs), or why none. */
  const pick = (key: string, toolUseId: string | undefined): ShellAsk | "notAllowed" | "ambiguous" => {
    const same = [...asks.values()].filter((a) => a.key === key && a.state !== "used");
    const live = same.filter((a) => a.state === "allowed");
    if (toolUseId !== undefined) {
      const exact = live.find((a) => a.toolUseId === toolUseId);
      if (exact !== undefined) return exact;
    }
    const open = toolUseId === undefined ? live : live.filter((a) => a.toolUseId === undefined);
    if (open.length === 0) return "notAllowed";
    return open.length === 1 && same.length === 1 ? open[0]! : "ambiguous";
  };

  const shellCall = (cli: string, rpc: unknown, params: Record<string, unknown>) => {
    if (served === undefined) return { answer: encodeAnswer(cli, rpcError(rpc, -32602, "tool not served here: shell")) };
    const input = shellInput(params.arguments);
    if (typeof input === "string") return { answer: encodeAnswer(cli, toolResult(rpc, input, true)) };
    const meta = isObject(params._meta) ? params._meta["claudecode/toolUseId"] : undefined;
    const toolUseId = typeof meta === "string" ? meta : undefined;
    const ticket = pick(callKey(SHELL_TOOL, params.arguments), toolUseId);
    if (typeof ticket === "string") {
      const event: ProviderEvent = { type: "notice", name: "shellRefused", details: { harness: "claudeCode", reason: ticket, ...(toolUseId === undefined ? {} : { toolUseId: clip(toolUseId) }) } };
      const text = ticket === "ambiguous" ? "Finisher cannot tell which allowed shell call this is: send its tool_use_id." : "Finisher has not allowed this shell call.";
      return { answer: encodeAnswer(cli, toolResult(rpc, text, true)), events: [event] };
    }
    ticket.state = "used";
    const entry = { rpc, stop: new AbortController() };
    running.set(cli, entry);
    const later = served.run(input, ticket.callId, entry.stop.signal).then((r: ShellResult) => {
      const aborted = entry.stop.signal.aborted;
      if (running.get(cli) === entry) running.delete(cli);
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
      const reused = seen.has(id);
      seen.add(id);
      if (r.subtype === "mcp_message" && r.server_name === MCP_SERVER && isMcpToolCall(r.message)) {
        const m = r.message as { id?: unknown; params?: unknown };
        const params = isObject(m.params) ? m.params : {};
        if (params.name === "shell") {
          if (reused) {
            const event: ProviderEvent = { type: "notice", name: "reusedRequestId", details: { harness: "claudeCode", requestId: clip(id) } };
            return { answer: encodeAnswer(id, rpcError(m.id ?? null, -32600, "Finisher refuses a tools/call that reuses a request id.")), events: [event] };
          }
          return shellCall(id, m.id ?? null, params);
        }
      }
      const out = port.request(id, r);
      // A prompt for the brokered shell: its allow leaves a ticket (module docs).
      const prompt = promptOf(r);
      if (prompt?.tool === SHELL_TOOL) {
        const key = callKey(SHELL_TOOL, prompt.input);
        const ask = out.events?.find((e) => e.type === "request");
        const toolUseId = ask?.type === "request" ? ask.providerId : prompt.toolUseId;
        const base = { cli: id, key, ...(toolUseId === undefined ? {} : { toolUseId }) };
        // Refused at once by the port (a reused id, a re-run): a not-allowed prompt all the same.
        if (ask?.type === "request") asks.set(ask.id, { ...base, state: "waiting" });
        else asks.set(`!${id}#${asks.size}`, { ...base, state: "notAllowed" });
      }
      return out;
    },

    respond(id: string, a: RuntimeAnswer): string | FinError {
      if (ended.has(id)) return finError("PROVIDER_PROTOCOL", "this shell prompt's turn has ended: it was withdrawn", { id: clip(id) });
      const line = port.respond(id, a);
      const ask = asks.get(id);
      if (typeof line === "string" && ask?.state === "waiting") {
        ask.state = a.kind === "approval" && a.allow ? "allowed" : "notAllowed";
        if (a.kind === "approval" && a.callId !== undefined) ask.callId = a.callId;
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
      for (const e of events) {
        const ask = e.type === "requestCancelled" ? asks.get(e.id) : undefined;
        if (ask !== undefined) ask.state = "notAllowed";
      }
      return events;
    },

    ...(port.asked === undefined ? {} : { asked: port.asked }),

    turnEnded() {
      const withdrawn = endAsks();
      port.turnEnded?.();
      stopAll();
      return withdrawn;
    },

    closed() {
      endAsks();
      stopAll();
    },
  };
}
