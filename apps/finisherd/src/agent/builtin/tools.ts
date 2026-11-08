// The built-in agent's tools (sub-project 7 §4.3; plan 7A Decisions 11–13): one native `fin`
// tool (argv, no shell) and three workspace file tools confined to `workspace/<sessionId>/`.
// The definitions are static JSON, so `toolsHash` is identical across sessions; the executor
// is what the runtime runs each announced call with (`RuntimeDeps.tools.builtin`).
import { blake3Hex } from "@finisher/fin-node";
import { finError } from "../../errors.ts";
import type { ToolCall, ToolExecutor, ToolOutcome } from "../runtime-deps.ts";
import { type FinIndex, finIndex, PROJECT_DIR_ENV, runFinTool } from "./fin-tool.ts";
import type { ToolDef } from "./request.ts";
import { WORKSPACE_TOOLS, type WorkspaceToolName, workspaceTool } from "./workspace-tools.ts";

export const FIN_TOOL = "fin";

export const TOOL_DEFS: readonly ToolDef[] = [
  {
    name: FIN_TOOL,
    description:
      "Run one Finisher CLI command. `argv` is the words after `fin` (no shell, no quoting: one array item per argument, e.g. [\"item\", \"trim\", \"c2\", \"--end\", \"-1s\"]). The answer is the JSON `fin` printed, its exit code and the end of its stderr. `fin help <group>` and `fin schema <cmd>` describe the commands. Writes go to this session's branch unless `--branch` names one of its own.",
    parameters: {
      type: "object",
      properties: {
        argv: { type: "array", items: { type: "string" }, description: "The arguments after `fin`; the first is a command word, never a path." },
        stdin: { type: "string", description: "Text on fin's standard input (`fin batch` lines)." },
      },
      required: ["argv"],
      additionalProperties: false,
    },
  },
  {
    name: "workspace_read",
    description: "Read a file in this session's workspace (paths relative to it). At most 256 KiB per call: use offset and limit (bytes) for more.",
    parameters: {
      type: "object",
      properties: { path: { type: "string" }, offset: { type: "integer", minimum: 0 }, limit: { type: "integer", minimum: 1 } },
      required: ["path"],
      additionalProperties: false,
    },
  },
  {
    name: "workspace_write",
    description: "Write a text file in this session's workspace (paths relative to it; parent folders are made). `mode`: create (the default; fails if it exists), overwrite or append.",
    parameters: {
      type: "object",
      properties: { path: { type: "string" }, content: { type: "string" }, mode: { type: "string", enum: ["create", "overwrite", "append"] } },
      required: ["path", "content"],
      additionalProperties: false,
    },
  },
  {
    name: "workspace_list",
    description: "List a folder of this session's workspace (default: its top).",
    parameters: { type: "object", properties: { path: { type: "string" } }, additionalProperties: false },
  },
];

const canonical = (v: unknown): string =>
  JSON.stringify(v, (_k, x: unknown) =>
    x !== null && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(Object.entries(x as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : x,
  );

/** `blake3:<hex>` of the tool definitions (Decision 13): the same for every session. */
export const toolsHash = `blake3:${blake3Hex(Buffer.from(canonical(TOOL_DEFS), "utf8"))}`;

/** `blake3:<hex>` of a system prompt. */
export const systemHash = (system: string) => `blake3:${blake3Hex(Buffer.from(system, "utf8"))}`;

export const TOOL_NAMES: ReadonlySet<string> = new Set(TOOL_DEFS.map((t) => t.name));

/**
 * Whether a call changes something, so runs alone and in order (spec §4.4): `workspace_write`,
 * and a `fin` command the index marks as a write, does not know or cannot read. `--dry-run`
 * makes a read only as a flag (`FinIndex.parse`: not a value, not after `--`). Reads may run
 * in parallel.
 */
export function isWrite(tool: string, input: unknown, index: FinIndex | undefined): boolean {
  if (tool === "workspace_read" || tool === "workspace_list") return false;
  if (tool !== FIN_TOOL) return true;
  const argv = (input as { argv?: unknown } | null)?.argv;
  if (!Array.isArray(argv) || !argv.every((a) => typeof a === "string")) return true;
  if (index === undefined) return argv.length > 0;
  let parsed: ReturnType<FinIndex["parse"]>;
  try {
    parsed = index.parse(argv as string[]);
  } catch {
    return true;
  }
  if (parsed.command === undefined) return argv.length > 0;
  return parsed.command.write && !parsed.dryRun;
}

export interface BuiltinToolsDeps {
  /** The `fin` binary (`agents.finPath`). */
  readonly finPath: string;
  /** `agents.toolTimeoutS` in ms (default 120 s). */
  readonly timeoutMs?: number;
  /** The project folder of a session (the second root of `fin`'s path check, besides the workspace). */
  readonly projectDirOf?: (projectId: string) => string | undefined;
}

const interrupted = () => finError("SESSION_INTERRUPTED", "the tool call was stopped", {});

/**
 * `start()`, or `SESSION_INTERRUPTED` as soon as `signal` aborts (a budget stop past its grace,
 * an interrupt). An aborted signal starts nothing. A workspace operation already under way
 * finishes on its own, unheard (each write is temp-then-rename, so it lands whole or not at
 * all); its late failure is dropped, never an unhandled rejection.
 */
function abortable<A>(start: () => Promise<A>, signal: AbortSignal): Promise<A> {
  if (signal.aborted) return Promise.reject(interrupted());
  const p = start();
  return new Promise<A>((resolve, reject) => {
    const onAbort = () => {
      p.catch(() => {});
      reject(interrupted());
    };
    signal.addEventListener("abort", onAbort, { once: true });
    p.then(resolve, reject).finally(() => signal.removeEventListener("abort", onAbort));
  });
}

/**
 * The executor the runtime runs the built-in loop's calls with (`RuntimeDeps.tools.builtin`). It
 * honours the call's abort signal: `fin` is killed with its process tree (`runFinTool`), and
 * every call answers `SESSION_INTERRUPTED` at once.
 */
export function builtinTools(deps: BuiltinToolsDeps): ToolExecutor {
  return {
    run: (call: ToolCall, signal: AbortSignal) => abortable(() => runOne(deps, call, signal), signal),
  };
}

async function runOne(deps: BuiltinToolsDeps, call: ToolCall, signal: AbortSignal): Promise<ToolOutcome> {
  if (call.tool === FIN_TOOL) {
    const input = (call.input ?? {}) as { argv?: unknown; stdin?: unknown };
    if (!Array.isArray(input.argv) || !input.argv.every((a) => typeof a === "string")) {
      throw finError("USAGE", "the fin tool takes {argv: string[], stdin?: string}", { tool: call.tool });
    }
    if (input.stdin !== undefined && typeof input.stdin !== "string") throw finError("USAGE", "stdin must be a string", { tool: call.tool });
    const project = deps.projectDirOf?.(call.projectId);
    // Fail closed: a call is never run without the index its paths are checked with (a failed
    // `fin schema commands` is tried again by the next call).
    const index = await finIndex(deps.finPath).catch((e: unknown) => {
      throw finError("PROVIDER_UNAVAILABLE", `the fin tool cannot read fin's command index: ${e instanceof Error ? e.message : String(e)}`, { finPath: deps.finPath }, "check agents.finPath");
    });
    // The project folder `fin` trusts comes from the runtime, never from the call.
    const env: Record<string, string> = { ...call.env };
    if (project === undefined) delete env[PROJECT_DIR_ENV];
    else env[PROJECT_DIR_ENV] = project;
    return runFinTool(
      { callId: call.callId, argv: input.argv as string[], ...(input.stdin === undefined ? {} : { stdin: input.stdin as string }) },
      {
        finPath: deps.finPath,
        cwd: call.cwd,
        env,
        signal,
        ...(deps.timeoutMs === undefined ? {} : { timeoutMs: deps.timeoutMs }),
        roots: project === undefined ? [call.cwd] : [call.cwd, project],
        index,
      },
    );
  }
  if ((WORKSPACE_TOOLS as ReadonlyArray<string>).includes(call.tool)) {
    return workspaceTool(call.tool as WorkspaceToolName, call.input, call.cwd);
  }
  throw finError("USAGE", `no tool named ${call.tool}`, { tool: call.tool, tools: [...TOOL_NAMES] });
}
