// Claude Code's permission prompts through the runtime (plan 7D Task 8; Decisions 4, 10, 18, 21;
// Review Focus 3): the `RequestPort` every Claude Code instance dispatches its CLI requests to.
//
// - **`can_use_tool`** (`--permission-prompt-tool stdio`, Decision 10's default path) becomes a
//   `request {id: request_id, kind: approval, tool: tool_name, input}` for the runtime, which
//   decides through 7B's `permissions.ts` and `provider-requests.ts` (policy, a rule's `ask` card,
//   owner Q3a's managed rule while the `agent` profile is not enforced). Nothing is answered here
//   until the runtime has decided.
// - **The MCP fallback** (Decision 10: `--permission-prompt-tool mcp__finisher__approve`): an
//   `mcp_message` `tools/call` of the `finisher` server's `approve` tool is the same `request`;
//   the answer is the tool's text content holding the same `{behavior, ...}` body.
// - **Answers.** `allow` is `{behavior: "allow", updatedInput: <the request's input, the same
//   object>}`: **Finisher never rewrites a tool's input** (spec §12.3) and never sends
//   `updatedPermissions` (a remembered allow is a Finisher rule, 7B Decision 12; `encodeAnswer`
//   copies the fields one by one). `deny` is `{behavior: "deny", message: <the server's reason>}`.
//   An answer for no waiting request (answered, withdrawn, unknown) writes nothing.
// - **Withdrawn.** A `control_cancel_request` for a waiting request is `requestCancelled {id}`:
//   the runtime expires its card and nothing is answered.
// - **One answer per request instance** (review of Task 8). The runtime's id of a request is
//   `<CLI request_id>#<generation>`, unique per port, and `respond` answers only that exact
//   instance. A CLI `request_id` is used once: a permission prompt reusing one (still waiting,
//   answered or withdrawn) is denied at once with a `reusedRequestId` notice and never reaches
//   the runtime, so a late decision on a withdrawn request can never answer a later one.
// - **No re-run on resume** (Task 10, Review Focus 5): a prompt for a call an earlier instance
//   of the session announced (`tool_use_id`, `PortOptions.rerun`) is denied at once with a
//   `rerunRefused` notice; the runtime already gave that call its (synthetic) result.
// - **Refused by the CLI itself.** Verified on 2.1.321 (7D Task 0, s2a): a Write or Edit outside
//   the working directory never sends `can_use_tool`: `blockWritesOutsideWorkingDirectories`
//   refuses it first (an `is_error` `tool_result`), and it shows up only in the turn's
//   `result.permission_denials`. No `approval` node exists for it; `asked` tells the mapper
//   which denials no prompt named, and each becomes a `notice {cliDenied}` (`map.ts`).
// - **Anything else** is answered `error` at once: a `hook_callback` (no SDK hooks are registered,
//   Decision 18) and a request subtype this host does not know, each with a notice; an MCP
//   `tools/call` of another tool (the brokered shell is Task 13's) a JSON-RPC error.
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { finError } from "../../errors.ts";
import type { RuntimeAnswer } from "../adapter.ts";
import { MCP_SERVER } from "./launch.ts";
import { isMcpToolCall, mcpReply } from "./probe-check.ts";
import { type CanUseToolAnswer, type CliRequest, encodeAnswer } from "./protocol.ts";
import type { RequestPort } from "./session.ts";

/** The `finisher` server's tool that answers permission prompts on the MCP path (Decision 10). */
const APPROVE = "approve";
/** The deny message when the runtime gave none. */
const DENIED = "Finisher denied this call.";

interface Waiting {
  /** The CLI's `request_id` the answer goes to. */
  readonly cli: string;
  readonly tool: string;
  readonly input: Readonly<Record<string, unknown>>;
  /** The JSON-RPC id of an MCP `approve` call (absent on the `stdio` path). */
  readonly rpc?: { readonly id: unknown };
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const clip = (s: string) => s.slice(0, 64);

export interface PortOptions {
  /** Whether `tool_use_id` is a call an earlier instance of the session announced. */
  readonly rerun?: (toolUseId: string) => boolean;
}

/** The answer to a prompt that is never asked of the runtime: `body` on the prompt's own path. */
const answerAt = (cli: string, body: CanUseToolAnswer, rpc?: { id: unknown }) =>
  encodeAnswer(cli, rpc === undefined ? body : { mcp_response: { jsonrpc: "2.0", id: rpc.id, result: { content: [{ type: "text", text: JSON.stringify(body) }] } } });

/** One instance's port (each instance has its own waiting requests). */
export function claudePermissions(opts: PortOptions = {}): RequestPort {
  /** By the runtime's id (`<cli>#<generation>`). */
  const waiting = new Map<string, Waiting>();
  /** The runtime's id of each CLI id still waiting. */
  const byCli = new Map<string, string>();
  /** Every CLI id a permission prompt has used (module docs). */
  const used = new Set<string>();
  /** Every `tool_use_id` a permission prompt named, answered or not. */
  const asked = new Set<string>();
  let generation = 0;
  const ask = (cli: string, tool: string, input: Readonly<Record<string, unknown>>, toolUseId: unknown, rpc?: { id: unknown }) => {
    if (typeof toolUseId === "string") asked.add(toolUseId);
    if (used.has(cli)) {
      const body: CanUseToolAnswer = { behavior: "deny", message: "Finisher refuses a permission prompt that reuses a request id." };
      return {
        answer: answerAt(cli, body, rpc),
        events: [{ type: "notice", name: "reusedRequestId", details: { harness: "claudeCode", requestId: clip(cli) } } satisfies ProviderEvent],
      };
    }
    used.add(cli);
    if (typeof toolUseId === "string" && opts.rerun?.(toolUseId) === true) {
      const body: CanUseToolAnswer = { behavior: "deny", message: "Finisher does not run this call again: it already has its result (the session was interrupted while it ran)." };
      return {
        answer: answerAt(cli, body, rpc),
        events: [{ type: "notice", name: "rerunRefused", details: { harness: "claudeCode", toolUseId: clip(toolUseId), tool: clip(tool) } } satisfies ProviderEvent],
      };
    }
    const id = `${cli}#${++generation}`;
    waiting.set(id, rpc === undefined ? { cli, tool, input } : { cli, tool, input, rpc });
    byCli.set(cli, id);
    return { events: [{ type: "request", id, kind: "approval", tool, input } satisfies ProviderEvent] };
  };
  const mcpError = (id: unknown, message: string) => ({ mcp_response: { jsonrpc: "2.0", id, error: { code: -32602, message } } });

  return {
    request(id: string, r: CliRequest) {
      switch (r.subtype) {
        case "can_use_tool":
          // Verified on 2.1.321 (7D Task 0, s2a): with Decision 4's `ask` list every tested call
          // sent `can_use_tool` (Read in and outside the workspace, Glob, Grep, Bash, WebFetch,
          // Edit and Write in the workspace); a write outside it never does (module docs).
          return ask(id, r.tool_name, r.input, r.tool_use_id);
        case "mcp_message": {
          const m = r.message as { id?: unknown; params?: { name?: unknown; arguments?: unknown } };
          if (r.server_name !== MCP_SERVER || !isMcpToolCall(r.message)) return { answer: encodeAnswer(id, mcpReply(r.server_name, r.message, [])) };
          const rpcId = m.id ?? null;
          if (m.params?.name !== APPROVE) return { answer: encodeAnswer(id, mcpError(rpcId, `tool not served here: ${clip(String(m.params?.name))}`)) };
          // Verified on 2.1.321 (7D Task 0, s2c): `tools/call` of `approve` with `arguments
          // {tool_name, input, tool_use_id}`; the CLI honours the JSON in `content[0].text`.
          const args = m.params.arguments;
          if (!isObject(args) || typeof args.tool_name !== "string" || !isObject(args.input)) return { answer: encodeAnswer(id, mcpError(rpcId, "approve takes {tool_name, input}")) };
          return ask(id, args.tool_name, args.input, args.tool_use_id, { id: rpcId });
        }
        case "hook_callback":
          // No SDK hooks are registered (`initialize {hooks: null}`): one cannot arrive.
          return {
            answer: encodeAnswer(id, { error: "no hooks are registered by this host" }),
            events: [{ type: "notice", name: "hookCallback", details: { harness: "claudeCode", callbackId: clip(r.callback_id) } }],
          };
        case "unknown":
          return {
            answer: encodeAnswer(id, { error: "not served by this host" }),
            events: [{ type: "notice", name: "unservedRequest", details: { harness: "claudeCode", subtype: clip(r.name) } }],
          };
      }
    },

    respond(id: string, a: RuntimeAnswer): string | FinError {
      const w = waiting.get(id);
      if (w === undefined) return finError("PROVIDER_PROTOCOL", "no Claude Code request is waiting for this answer", { id: clip(id) });
      if (a.kind !== "approval") return finError("PROVIDER_PROTOCOL", "a Claude Code permission prompt takes an approval", { id: clip(id), kind: a.kind });
      waiting.delete(id);
      byCli.delete(w.cli);
      // The input as the CLI sent it, the same object: never rewritten.
      const body: CanUseToolAnswer = a.allow ? { behavior: "allow", updatedInput: w.input } : { behavior: "deny", message: a.message ?? DENIED };
      return answerAt(w.cli, body, w.rpc);
    },

    /** `id` is the CLI's `request_id`; the event names the runtime's. */
    cancel(id: string): readonly ProviderEvent[] {
      // Verified on 2.1.321 (7D Task 0, s2b): sent when the host interrupts while a prompt
      // waits, before the interrupt's `control_response`; none came in 5 s of holding one.
      // TODO(7D Task 0 Step 2): whether the CLI ever withdraws a prompt on a timeout of its own;
      // modelled by fake-claude's `cancelAfterMs` for an unanswered `ask`.
      const runtimeId = byCli.get(id);
      if (runtimeId === undefined) return [];
      byCli.delete(id);
      waiting.delete(runtimeId);
      return [{ type: "requestCancelled", id: runtimeId }];
    },

    asked: (toolUseId: string) => asked.has(toolUseId),
  };
}
