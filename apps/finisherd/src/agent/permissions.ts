// Provider permission requests (sub-project 7B Task 5, Decision 13; spec §7.4): what the runtime
// answers when a harness asks before running one of its own tools (Claude Code's `can_use_tool`,
// Codex's `requestApproval`; 7D and 7E map their protocols onto `decideProviderRequest`).
//
// - **Names.** A request is named `tool:<Tool>` and, when its input reads as text,
//   `tool:<Tool>(<text>)` (a shell command, a URL, a path, a query; else the input's canonical
//   JSON), so a rule `ask "tool:WebFetch"` or `ask "tool:Bash(curl *)"` in the same rule store
//   (every layer, `rules.ts`) marks it. Both names are evaluated and the strictest wins.
// - **Order.** A `deny` rule denies; then **what the mode forbids** is denied: in `plan` a write
//   (a file tool that writes: `WRITE_TOOLS`) whose path is outside `workspace/<session>/` (a
//   relative path is the workspace's; a write tool naming no path is outside); then an `ask`
//   rule routes the request to a person (a `provider` approval, `provider-requests.ts`); else
//   **allow**: what the sandbox contains (file tools inside the workspace, reads, shell commands:
//   7C's sandbox is the bound, not this check).
// - **Length.** A name holds the input's whole text (the glob matcher is linear: no padding can
//   push a command past a `*…*` rule). An input whose text passes `TOOL_TEXT_MAX` (1 MiB) is
//   not matched at all and is never allowed: denied when a `deny` rule names the tool (its
//   pattern before `(` matches `tool:<Tool>`), else asked (the `ask` rule that names the tool,
//   else the server's `tool.oversized`).
// - **The shell's two names** (7D Decision 21): Claude Code's `Bash` and Finisher's brokered
//   `mcp__finisher__shell` are one shell, so a request of either is evaluated under both tool
//   names: a rule written for `tool:Bash(...)` holds for the brokered shell and the reverse.
// - **While the `agent` profile is not enforced** (owner Q3a, 2026-11-12; the request's
//   `unconfined`: its session sandbox runs `direct`, or under a launcher that confines nothing),
//   a managed-layer rule applies: a write tool's path outside `workspace/<session>/` is denied in
//   every mode (as `plan` denies it always), and a shell command is asked (`UNCONFINED_SHELL_RULE`,
//   layer `managed`: no remembered allow lifts it), so a person sees every shell command until
//   the sandbox is on. A `deny` rule still denies first. Once the profile is enforced the
//   normal order above applies.
// - A layer that cannot be read denies (its synthetic `deny **`, `trust.ts`), so a broken file
//   never opens a tool.
import { isAbsolute, join, relative, resolve } from "node:path";
import type { FinError, Rule, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { canonicalJson } from "./approvals.ts";
import { globMatch } from "./glob.ts";
import { type Layer, NOTHING_RESOLVED, type RuleSource, RuleSet } from "./rules.ts";

/** The file tools that write (Claude Code's, Codex's patch, the built-in loop's). */
export const WRITE_TOOLS: ReadonlySet<string> = new Set(["Write", "Edit", "MultiEdit", "NotebookEdit", "apply_patch", "workspace_write"]);
/** The shell's names (7D Decision 21): the CLI's own and Finisher's brokered one. */
export const SHELL_TOOLS: readonly string[] = ["Bash", "mcp__finisher__shell"];
/** The managed rule a shell command asks under while the `agent` profile is not enforced (module docs). */
export const UNCONFINED_SHELL_RULE = "sandbox.notEnforced";
/** The input fields that hold a request's text, in order. */
const TEXT_FIELDS = ["command", "url", "file_path", "notebook_path", "path", "pattern", "query", "prompt"] as const;
/** The input fields that name the file a write tool writes. */
const PATH_FIELDS = ["file_path", "notebook_path", "path"] as const;
/** The most text of an input that is matched (characters); past it, module docs. */
export const TOOL_TEXT_MAX = 1024 * 1024;
/** The rule an oversized input with no `ask` rule of its own asks under (module docs). */
export const OVERSIZED_RULE = "tool.oversized";

export type ProviderDecision = "allow" | "deny" | { readonly ask: string };

/** One provider request: its tool, input, and whether its session's `agent` profile confines nothing (module docs). */
export interface ProviderRequest {
  readonly tool: string;
  readonly input: unknown;
  readonly unconfined?: boolean;
}

/** A decision and why: the rule and layer that decided, or the mode. */
export interface Explained {
  readonly decision: "allow" | "deny" | "ask";
  readonly reason: "rule" | "mode" | "unconfined" | "sandbox" | "size";
  readonly rule?: string;
  readonly layer?: Layer;
  /** The name the rules decided on (`tool:<Tool>…`), for a card. */
  readonly command: string;
}

const field = (input: unknown, key: string): unknown => (input !== null && typeof input === "object" && !Array.isArray(input) ? (input as Record<string, unknown>)[key] : undefined);

/** The text of a request's input (module docs), whole. */
function textOf(input: unknown): string {
  if (typeof input === "string") return input;
  for (const k of TEXT_FIELDS) {
    const v = field(input, k);
    if (typeof v === "string") return v;
  }
  return input === undefined || input === null ? "" : canonicalJson(input);
}

/** The tool names a request of `tool` is evaluated under: both of the shell's, else its own. */
const aliasesOf = (tool: string): readonly string[] => (SHELL_TOOLS.includes(tool) ? [tool, ...SHELL_TOOLS.filter((t) => t !== tool)] : [tool]);

/** Every name a request is evaluated under (module docs), the plain tool name first. */
export function toolNames(tool: string, input: unknown): string[] {
  return namesOf(tool, textOf(input));
}

const namesOf = (tool: string, text: string): string[] => aliasesOf(tool).flatMap((t) => (text === "" ? [`tool:${t}`] : [`tool:${t}`, `tool:${t}(${text})`]));

/** Whether rule pattern `command` names `tool` at all: its part before `(` matches `tool:<Tool>`. */
export function namesTool(command: string, tool: string): boolean {
  const open = command.indexOf("(");
  return globMatch(open < 0 ? command : command.slice(0, open), `tool:${tool}`) || globMatch(command, `tool:${tool}`);
}

/**
 * The rules an input past `TOOL_TEXT_MAX` is decided by (module docs): the deny and ask rules
 * that name the tool, as rules of its plain name (`RuleSet` keeps their scope).
 */
function naming(rules: readonly Rule[], tool: string): Rule[] {
  const names = aliasesOf(tool);
  return rules.filter((r) => (r.effect === "deny" || r.effect === "ask") && names.some((t) => namesTool(r.command, t))).map((r) => ({ ...r, command: `tool:${tool}` }));
}

/** Whether `path` (absolute, or relative to the workspace) lies inside `workspace`. */
export function insideWorkspace(workspace: string, path: string): boolean {
  const rel = relative(workspace, resolve(workspace, path));
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

export interface PermissionsDeps {
  readonly rules: RuleSource;
  /** The folder of a project (its session's workspace is `workspace/<sessionId>/` in it). */
  readonly dirOf: (projectId: string) => string | undefined;
}

export interface ProviderPermissions {
  /** The decision on one request (plan 7B's frozen interface). */
  decideProviderRequest(s: Session, req: ProviderRequest): Effect.Effect<ProviderDecision, FinError>;
  /** The same, with the rule, layer or mode that decided. */
  explain(s: Session, req: ProviderRequest): Effect.Effect<Explained, FinError>;
}

/** Decides provider requests over the session's rule layers and mode (module docs). */
export function providerPermissions(d: PermissionsDeps): ProviderPermissions {
  const explain = (s: Session, req: ProviderRequest) =>
    Effect.gen(function* () {
      const layered = yield* Effect.try({ try: () => d.rules.layers({ session: s }), catch: (e) => finError("INTERNAL", `command rules cannot be read: ${String(e)}`) });
      const text = textOf(req.input);
      const big = text.length > TOOL_TEXT_MAX;
      const names = namesOf(req.tool, big ? "" : text);
      const o = new RuleSet(big ? naming(layered.rules, req.tool) : layered.rules).decide(names, s, NOTHING_RESOLVED);
      const command = o.command ?? names[0]!;
      if (o.effect === "deny") return { decision: "deny", reason: "rule", rule: o.rule.id, layer: o.layer, command } satisfies Explained;
      // A textual check (7C's sandbox is the real bound): it does not resolve symlinks inside the
      // workspace (a link there to a file outside passes), and compares paths with the case they
      // are given (on a case-insensitive file system `Workspace/S1` names the same folder and is
      // refused; a differently cased path into another folder could pass).
      const unconfined = req.unconfined === true;
      if ((s.mode === "plan" || unconfined) && WRITE_TOOLS.has(req.tool)) {
        const dir = d.dirOf(s.projectId);
        const path = PATH_FIELDS.map((k) => field(req.input, k)).find((v): v is string => typeof v === "string" && v !== "");
        if (dir === undefined || path === undefined || !insideWorkspace(join(dir, "workspace", s.id), path)) return { decision: "deny", reason: s.mode === "plan" ? "mode" : "unconfined", command } satisfies Explained;
      }
      if (o.effect === "ask") return { decision: "ask", reason: "rule", rule: o.rule.id, layer: o.layer, command } satisfies Explained;
      if (big) return { decision: "ask", reason: "size", rule: OVERSIZED_RULE, layer: "managed", command } satisfies Explained;
      if (unconfined && SHELL_TOOLS.includes(req.tool)) return { decision: "ask", reason: "unconfined", rule: UNCONFINED_SHELL_RULE, layer: "managed", command } satisfies Explained;
      return { decision: "allow", reason: "sandbox", command } satisfies Explained;
    });
  return {
    explain,
    decideProviderRequest: (s, req) => Effect.map(explain(s, req), (e): ProviderDecision => (e.decision === "ask" ? { ask: e.rule! } : e.decision)),
  };
}
