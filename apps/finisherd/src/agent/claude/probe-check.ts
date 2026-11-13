// The capability predicates of plan 7D Decision 8 (Task 5), apart from the probe's process code:
// what an `initialize` answer must hold, what every instance's `system/init` must hold
// (`checkInit`), the first-turn gate that holds an instance's events back until its init passed,
// the `finisher` SDK MCP server's handshake answers, and the `untestedVersion` notice.
//
// Drift fails closed (Review Focus 6): a key the predicate reads that is absent is a failure
// naming it, never a pass. The isolation items (Decision 6, as Task 0 amended it):
// - `tools` ⊆ the instance's `--tools` ∪ the `finisher` server's tools, and every tool Finisher
//   asked for is there;
// - `mcp_servers` = Finisher's only, and `finisher` is `connected` (an unanswered handshake comes
//   up `failed`, Task 0);
// - `plugins` all built-in (`path: "builtin"`, `source` ending `@builtin`);
// - `skills` and `slash_commands` empty; `memory_paths` absent or `null` (2.1.321 leaves it out
//   when auto memory is off); `additional_directories` absent or empty;
// - `agents` only the five built-ins (listed even with `Task`/`Agent` disallowed);
// - `session_id` the UUID Finisher passed; `permissionMode` `default`;
// - `apiKeySource: "none"` while the CLI says it is signed in: the sign-in was ignored (`oauth`);
// - with the probe's settings canary, `output_style` shows it (a settings file that fails the
//   CLI's validation is dropped whole and silently, Task 0).
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { finError } from "../../errors.ts";
import { type CliModel, parseClaudeInitialize } from "../catalogue/cli-models.ts";
import { type LaunchPlan, MCP_SERVER } from "./launch.ts";
import { CANARY_STYLE } from "./probe-types.ts";
import type { CliLine, McpReply, SystemInit } from "./protocol.ts";

export { CANARY_STYLE } from "./probe-types.ts";

/** The subagents 2.1.321 lists whatever the flags (Task 0): any other name refuses. */
export const BUILTIN_AGENTS: ReadonlySet<string> = new Set(["claude", "Explore", "general-purpose", "Plan", "statusline-setup"]);

/** `PROVIDER_VERSION {missing, item?}`: what the installed CLI lacks or added. */
export function versionError(missing: string, message: string, extra: Record<string, unknown> = {}): FinError {
  return finError("PROVIDER_VERSION", message, { harness: "claudeCode", missing, ...extra });
}

const isolation = (item: string, version: string | undefined): FinError =>
  versionError("isolation", `the Claude Code instance is not isolated: ${item}`, { item, ...(version === undefined ? {} : { version }) });

/** The session UUID an instance's argv passes (`--session-id <uuid>` or `--resume=<uuid>`). */
export function planUuid(plan: Pick<LaunchPlan, "argv">): string | null {
  const i = plan.argv.indexOf("--session-id");
  if (i >= 0) return plan.argv[i + 1] ?? null;
  const r = plan.argv.find((a) => a.startsWith("--resume="));
  return r === undefined ? null : r.slice("--resume=".length);
}

export interface CheckOptions {
  /**
   * The CLI's sign-in was ignored if `apiKeySource` is `none`: set only when `auth status` reports
   * a sign-in **and** the instance's own `initialize.account` is exactly unsigned
   * (`unsignedAccount`). `apiKeySource: "none"` alone is no sign of it: a claude.ai sign-in reports
   * `none` too (published stream-JSON logs of Pro/Max runs).
   */
  readonly signedIn?: boolean;
  /** The settings carried the probe's canary: `output_style` must show it. */
  readonly canary?: boolean;
  /** The probe's unsigned turn (Decision 8 step 2): `apiKeySource` must be `none`, else `unsigned`. */
  readonly unsigned?: boolean;
}

/** Init keys the predicate reads that the decoder leaves optional: absent is a failure. */
const REQUIRED_KEYS = ["permissionMode", "apiKeySource", "mcp_servers", "plugins", "skills", "slash_commands", "agents"] as const;

const label = (v: unknown): string => (typeof v === "string" ? v : JSON.stringify(v) ?? String(v)).slice(0, 80);

/**
 * `init` against the instance `plan` started (Decision 8): null when it holds, else
 * `PROVIDER_VERSION {missing, item}` — `missing` a key's name when the key is absent,
 * `isolation` (with `item`) for anything Finisher did not supply, `oauth` for an ignored
 * sign-in, `mcp` for a `finisher` server that is not connected, `settings` for a missing canary.
 */
export function checkInit(init: SystemInit, plan: Pick<LaunchPlan, "argv" | "tools" | "mcpTools">, opts: CheckOptions = {}): FinError | null {
  const v = init.claude_code_version;
  for (const k of REQUIRED_KEYS) {
    if (init[k] === undefined) return versionError(k, `the Claude Code init has no ${k}`, { item: k, ...(v === undefined ? {} : { version: v }) });
  }
  if (init.session_id !== planUuid(plan)) return isolation("session_id", v);
  if (init.permissionMode !== "default") return isolation(`permissionMode:${label(init.permissionMode)}`, v);
  const allowed = new Set([...plan.tools, ...plan.mcpTools]);
  for (const t of init.tools) if (!allowed.has(t)) return isolation(`tool:${label(t)}`, v);
  for (const s of init.mcp_servers ?? []) if (s.name !== MCP_SERVER) return isolation(`mcp_server:${label(s.name)}`, v);
  for (const p of init.plugins ?? []) {
    if (p.path !== "builtin" || p.source === undefined || !p.source.endsWith("@builtin")) return isolation(`plugin:${label(p.name)}`, v);
  }
  const skill = init.skills?.[0];
  if (skill !== undefined) return isolation(`skill:${label((skill as { name?: unknown } | null)?.name ?? skill)}`, v);
  const command = init.slash_commands?.[0];
  if (command !== undefined) return isolation(`slash_command:${label(command)}`, v);
  if (init.memory_paths !== undefined && init.memory_paths !== null) return isolation("memory_paths", v);
  if (init.additional_directories !== undefined && init.additional_directories.length > 0) return isolation("additional_directories", v);
  for (const a of init.agents ?? []) if (!BUILTIN_AGENTS.has(a)) return isolation(`agent:${label(a)}`, v);
  const finisher = init.mcp_servers?.find((s) => s.name === MCP_SERVER);
  if (finisher?.status !== "connected") {
    return versionError("mcp", "Finisher's MCP server did not connect to the Claude Code instance", { item: MCP_SERVER, status: finisher?.status ?? "absent", ...(v === undefined ? {} : { version: v }) });
  }
  const present = new Set(init.tools);
  for (const t of [...plan.tools, ...plan.mcpTools]) {
    if (!present.has(t)) return versionError("tools", `the Claude Code instance lacks the tool ${t}`, { item: t, ...(v === undefined ? {} : { version: v }) });
  }
  // TODO(7D Task 0 Step 1/Step 4): `apiKeySource` and `initialize.account` of a CLI signed in with
  // its own flow (claude.ai, Console); modelled by fake-claude as `none` with `tokenSource:
  // "claude.ai"` (a sign-in, passes) and a Console key's `/login managed key`.
  if (opts.signedIn === true && init.apiKeySource === "none") {
    return versionError("oauth", "the Claude Code instance ignored its sign-in", { item: "apiKeySource", ...(v === undefined ? {} : { version: v }) });
  }
  if (opts.unsigned === true && init.apiKeySource !== "none") {
    return versionError("unsigned", "the Claude Code probe instance is signed in by other means", { item: "apiKeySource", ...(v === undefined ? {} : { version: v }) });
  }
  if (opts.canary === true && init.output_style !== CANARY_STYLE) {
    return versionError("settings", "the Claude Code instance did not apply its settings file", { item: "outputStyle", ...(v === undefined ? {} : { version: v }) });
  }
  return null;
}

/** What step 1 learns from the `initialize` answer. Never the account itself: whether it is unsigned only. */
export interface InitializeFacts {
  readonly models: readonly CliModel[];
  readonly capabilities: readonly string[];
  /**
   * The account is exactly `{tokenSource: "none", apiProvider: "firstParty"}` and nothing else.
   * Anything more or other is a sign-in (2.1.321: an API key adds `apiKeySource`, an
   * `apiKeyHelper` is `tokenSource: "apiKeyHelper"`, Bedrock has `apiProvider: "bedrock"` and no
   * `tokenSource`), and the probe sends no user line.
   */
  readonly unsigned: boolean;
}

/** `initialize.account` is exactly the unsigned one (Task 0, 2.1.321): anything more or other is a sign-in. */
export const unsignedAccount = (account: unknown): boolean =>
  typeof account === "object" && account !== null && !Array.isArray(account) && isUnsigned(account as Record<string, unknown>);

const isUnsigned = (account: Record<string, unknown>): boolean =>
  Object.keys(account).length === 2 && account.tokenSource === "none" && account.apiProvider === "firstParty";

const strings = (v: unknown): v is string[] => Array.isArray(v) && v.every((x) => typeof x === "string");

/**
 * The `initialize` answer's body (`control_response.response.response`) against Decision 8's
 * step 1: `models`, `commands: []`, `capabilities`, `claude_code_version` equal to `version`,
 * `output_style` the canary (the probe always runs with it; a settings file the CLI dropped
 * answers `default`), an `account` object. A missing one is `PROVIDER_VERSION {missing}`; a
 * command is isolation.
 */
export function checkInitialize(body: unknown, version: string): InitializeFacts | FinError {
  if (typeof body !== "object" || body === null || Array.isArray(body)) return versionError("initialize", "the Claude Code initialize answer is not an object");
  const o = body as Record<string, unknown>;
  let models: CliModel[];
  try {
    models = parseClaudeInitialize(o);
  } catch {
    return versionError("models", "the Claude Code initialize answer has no models", { item: "models", version });
  }
  if (models.length === 0) return versionError("models", "the Claude Code initialize answer lists no models", { item: "models", version });
  if (!Array.isArray(o.commands)) return versionError("commands", "the Claude Code initialize answer has no commands", { item: "commands", version });
  if (o.commands.length > 0) {
    const c = o.commands[0] as { name?: unknown } | null;
    return isolation(`command:${label(c?.name ?? c)}`, version);
  }
  if (!strings(o.capabilities)) return versionError("capabilities", "the Claude Code initialize answer has no capabilities", { item: "capabilities", version });
  if (o.claude_code_version !== version) {
    return versionError("version", "the Claude Code CLI reports another version than detected", { item: "claude_code_version", version, got: label(o.claude_code_version) });
  }
  if (o.output_style !== CANARY_STYLE) return versionError("settings", "the Claude Code CLI did not apply the probe's settings file", { item: "outputStyle", version });
  const account = o.account;
  if (typeof account !== "object" || account === null || Array.isArray(account)) {
    return versionError("account", "the Claude Code initialize answer has no account", { item: "account", version });
  }
  return { models, capabilities: o.capabilities, unsigned: isUnsigned(account as Record<string, unknown>) };
}

// ---- the first-turn gate ----

/** What the caller does with a line: pass these on, nothing yet, or stop the instance. */
export type GateStep =
  | { readonly kind: "forward"; readonly lines: readonly CliLine[] }
  | { readonly kind: "hold" }
  /**
   * The init failed: fail the turn with `error` and kill the instance at once (SIGKILL of its
   * process group through the sandbox, `SandboxedChild.kill`); an `interrupt` control request
   * only where no kill is possible. An interrupt lets the turn play on, a kill does not.
   */
  | { readonly kind: "refuse"; readonly error: FinError; readonly kill: true }
  /** After a refusal: the line is dropped. */
  | { readonly kind: "drop" };

export interface FirstTurnGate {
  /** Feed every decoded line of the instance, in order. */
  push(line: CliLine): GateStep;
  readonly state: "waiting" | "open" | "refused";
}

/** Lines the caller always handles at once: the SDK MCP handshake and answers to its own requests. */
const passes = (l: CliLine): boolean =>
  (l.kind === "controlRequest" && l.request.subtype === "mcp_message") || l.kind === "controlResponse" || l.kind === "controlCancel";

/**
 * Holds every event of an instance back until its `system/init` passes `checkInit` (Decision 8,
 * Review Focus 1), so nothing reaches the runtime from an instance that is not isolated. Then it
 * releases what it held in arrival order, the init last, and forwards everything. The CLI sends
 * an init on every turn: each one is checked, in any state, and a failing one refuses (the
 * caller kills the instance and fails the turn). A `result` before any init releases that
 * `result` alone when it is an error (an unknown `--resume`), dropping what was held, and
 * refuses otherwise. Held lines include `can_use_tool` and every other CLI request. Control
 * lines the caller must answer at once (the MCP handshake, control
 * responses, cancels) always pass; a refused instance's other control requests too, so a caller
 * may deny them (Task 7's session instead kills the instance at once and drops every later line).
 */
export function firstTurnGate(plan: Pick<LaunchPlan, "argv" | "tools" | "mcpTools">, opts: CheckOptions = {}): FirstTurnGate {
  let state: FirstTurnGate["state"] = "waiting";
  let held: CliLine[] = [];
  const refuse = (error: FinError): GateStep => {
    state = "refused";
    held = [];
    return { kind: "refuse", error, kill: true };
  };
  return {
    get state() {
      return state;
    },
    push(line) {
      if (passes(line)) return { kind: "forward", lines: [line] };
      if (state === "refused") return line.kind === "controlRequest" ? { kind: "forward", lines: [line] } : { kind: "drop" };
      if (line.kind === "init") {
        const e = checkInit(line, plan, opts);
        if (e !== null) return refuse(e);
        if (state === "open") return { kind: "forward", lines: [line] };
        state = "open";
        const lines = [...held, line];
        held = [];
        return { kind: "forward", lines };
      }
      if (state === "open") return { kind: "forward", lines: [line] };
      if (line.kind === "result") {
        if (!line.is_error) return refuse(versionError("init", "the Claude Code instance ended a turn without its init", { item: "system/init" }));
        held = [];
        return { kind: "forward", lines: [line] };
      }
      held.push(line);
      return { kind: "hold" };
    },
  };
}

// ---- the `finisher` SDK MCP server's handshake ----

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
 * The answer to one `mcp_message` for the `finisher` server, as the Agent SDK's in-process
 * servers answer (Task 0: the CLI starts the handshake before it answers `initialize`, and waits
 * up to a minute for it): `initialize`, `notifications/*`, `tools/list` of `mcpTools`
 * (`mcp__finisher__<name>`); any other method is a JSON-RPC error (`tools/call` is the
 * session's, Tasks 8 and 13). A message for another server is an error.
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

// ---- untested versions ----

/**
 * The versions Task 15's recorded transcript suite passes. Empty until Task 15 records one: every
 * version then runs on the probe alone, with the notice below (spec §4.5).
 */
export const TESTED_VERSIONS: readonly string[] = [];

/**
 * A notice source for the session layer (Task 7): `notice {untestedVersion}` the first time a
 * version outside `tested` is used, null for a tested version and on every later use.
 */
export function untestedVersionNotices(tested: readonly string[] = TESTED_VERSIONS): (version: string) => ProviderEvent | null {
  const seen = new Set<string>();
  return (version) => {
    if (tested.includes(version) || seen.has(version)) return null;
    seen.add(version);
    return { type: "notice", name: "untestedVersion", details: { harness: "claudeCode", version } };
  };
}
