// The Claude Code launch plan (plan 7D Task 4; Decisions 1, 2, 4, 5, 7, 10, 12, 15, 21): the
// argv, the files under `agents/<id>/settings/` (the session settings, the MCP config, the role
// prompt) and the extra environment of one CLI instance. Pure: `writeLaunchFiles` writes the
// files, the adapter spawns `argv` through the session sandbox with `env` as `SpawnOptions.env`.
//
// - **Files, not argv,** carry the prompt and the settings: no prompt text in `ps`, and the
//   Windows command-line limit. `settings/` is a protected root of the `agent` profile, so the
//   CLI reads these files and cannot change them.
// - **`--tools` is the role's allow-set** intersected with the CLI tools Finisher maps (Decision
//   2): a tool a newer CLI adds stays off. `--disallowedTools` is a second layer naming the
//   agentic tools; it never holds an `mcp__*` wildcard (deny beats allow: it would also remove
//   Finisher's own `finisher` server's tools; `--strict-mcp-config` already admits only that).
// - **The settings file is written from a typed schema** (Task 0: one wrongly typed known key
//   makes `-p` drop the whole file silently, `disableAllHooks` and the `ask` list with it).
// - **A CLI reaches `fin` through its shell**: `fin` in the allow-set enables `Bash` (Decision 2);
//   every call still asks, and 7B's `permissions.ts` decides it.
// - **Windows** (Decision 21, owner Q2(a)): the brokered shell (Task 13: no `Bash`; the
//   `finisher` SDK MCP server's `shell` tool, which `finisherd` serves through the session
//   sandbox, `mcp-host.ts` and `shell-tool.ts`) is on where `ProbeResult.brokeredShell` is,
//   which nothing sets yet: until 7C Task 16's Windows validation, Windows sessions run `direct`
//   and use the CLI's own `Bash`, as on Linux and macOS. `LaunchContext.brokeredShell` turns the
//   shape on for any OS (tests, so it runs on Linux with `sh`). The server is declared on every
//   OS: the MCP permission fallback (Decision 10) and 7G's tools are served by it too.
// - **Paths come from the sandbox**: `settings/` and `CLAUDE_CODE_TMPDIR` are derived from the
//   session environment's `TMPDIR`, so they name the directories the sandbox protects and grants.
// - **The session id** is the session ULID's 128 bits written as a UUID (Decision 12), so a
//   resume is `--resume=<uuid>` with no table between them.
import { randomBytes } from "node:crypto";
import { chmod, rename, rm, writeFile } from "node:fs/promises";
import { posix, win32 } from "node:path";
import type { Session } from "@finisher/contracts";
import { Schema } from "effect";
import { finError } from "../../errors.ts";
import type { SessionSpec } from "../adapter.ts";
import { assertSpawnEnv } from "../sandbox-env.ts";
import { type SessionDirs, sessionDirs } from "../session-dirs.ts";
import { CANARY_STYLE, type ProbeResult } from "./probe-types.ts";

/** What one instance is started with: `argv` through the sandbox, `files` written first, `env` added. */
export interface LaunchPlan {
  readonly argv: string[];
  readonly files: { path: string; body: string }[];
  readonly env: Record<string, string>;
  /** The `--tools` set: the instance's allow-set for the init check (Decision 8). */
  readonly tools: readonly string[];
  /** The `finisher` SDK MCP server's tools this instance is served (`mcp__finisher__*`). */
  readonly mcpTools: readonly string[];
  /** Role tool names no CLI tool maps to: never passed; the caller records a notice. */
  readonly dropped: readonly string[];
}

/** What the launch needs beyond the session, its spec and the probe. */
export interface LaunchContext {
  /** The detected CLI, absolute (`agents.claudeCode.path` or `PATH`, resolved). */
  readonly binary: string;
  /** `agents.retentionDays`: the CLI keeps its transcripts as long (`cleanupPeriodDays`). */
  readonly retentionDays: number;
  /** The money left in the session's budget, in micros, at this (re)start; null without a money limit. */
  readonly remainingMoneyMicros: number | null;
  /** The sign-in is a subscription plan: its money is shown, not charged, so no backstop (Decision 15). */
  readonly planSignIn: boolean;
  /**
   * On a resume, the money the CLI counts from earlier runs against `--max-budget-usd` (Facts:
   * "on `--resume` totals restored from earlier runs count"), so the flag is the remainder on
   * top of it. Verified 2.1.321, Task 0: the totals are restored after the last process's clean
   * exit (then the adapter sets this), restarted after a kill (then 0).
   */
  readonly restoredCostMicros?: number;
  /** The brokered shell on any OS (tests: `FIN_TEST_BROKERED_SHELL`); else Windows with `ProbeResult.brokeredShell`. */
  readonly brokeredShell?: boolean;
}

/** The CLI tools Finisher maps, in `--tools` order (Decision 2). */
export const CLI_TOOLS = ["Read", "Edit", "Write", "Glob", "Grep", "Bash", "WebFetch", "WebSearch"] as const;

/** The built-in agent's tool names: not CLI tools (a CLI reaches `fin` through its shell). */
const BUILTIN_ONLY: ReadonlySet<string> = new Set(["fin", "workspace_read", "workspace_write", "workspace_list"]);

/** The second layer (Decision 2, with Task 0's additions from the 2.1.321 default set). */
export const DISALLOWED_TOOLS = [
  "Task",
  "Agent",
  "Workflow",
  "SendMessage",
  "Skill",
  "Cron*",
  "ScheduleWakeup",
  "Monitor",
  "EnterWorktree",
  "ExitWorktree",
  "AskUserQuestion",
  "NotebookEdit",
  "ToolSearch",
  "EnterPlanMode",
  "ExitPlanMode",
  "ListAgents",
  "ReportFindings",
  "TaskStop",
] as const;

/** Finisher's SDK MCP server (Decision 21) and its tools as the CLI names them. */
export const MCP_SERVER = "finisher";
export const SHELL_TOOL = `mcp__${MCP_SERVER}__shell`;
export const APPROVE_TOOL = `mcp__${MCP_SERVER}__approve`;

const SETTINGS_FILE = "claude.json";
const MCP_FILE = "mcp.json";
const ROLE_FILE = "role.md";

// The session settings (Decision 4): every key typed to the one value a session may have.
const ClaudeSettings = Schema.Struct({
  disableAllHooks: Schema.Literal(true),
  autoMemoryEnabled: Schema.Literal(false),
  crossSessionInbound: Schema.Literal("refuse"),
  attribution: Schema.Struct({ commit: Schema.Literal(""), pr: Schema.Literal("") }),
  cleanupPeriodDays: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(1)),
  permissions: Schema.Struct({
    defaultMode: Schema.Literal("default"),
    blockWritesOutsideWorkingDirectories: Schema.Literal(true),
    ask: Schema.Array(Schema.String.check(Schema.isMinLength(1))),
  }),
  // Exactly `{}`: a settings `env` would reach the CLI's tools past the sandbox's environment.
  env: Schema.Record(Schema.String, Schema.Never),
  /** The probe's canary only (Decision 4): an applied file shows it in `system/init`. */
  outputStyle: Schema.optionalKey(Schema.Literal(CANARY_STYLE)),
});
export type ClaudeSettings = typeof ClaudeSettings.Type;

const decodeSettings = Schema.decodeUnknownResult(ClaudeSettings);

/**
 * `body` as the session settings, or throws `INTERNAL`: a key of another type (or an unknown
 * one) would make the CLI drop the whole file without a sign (Task 0).
 */
export function checkSettings(body: unknown): ClaudeSettings {
  const r = decodeSettings(body, { onExcessProperty: "error" });
  if (r._tag === "Success") return r.success;
  throw finError("INTERNAL", "the Claude Code session settings do not match their schema", { issue: String(r.failure) });
}

/**
 * The session settings for an instance whose `--tools` are `tools` and whose `finisher` server
 * serves `mcpTools` (Decision 4): every enabled tool asks; of the server's, only `shell` does
 * (`approve` is how asking happens).
 */
export function sessionSettings(tools: readonly string[], mcpTools: readonly string[], retentionDays: number, canary = false): ClaudeSettings {
  return checkSettings({
    disableAllHooks: true,
    autoMemoryEnabled: false,
    crossSessionInbound: "refuse",
    attribution: { commit: "", pr: "" },
    cleanupPeriodDays: retentionDays,
    // No `deny`/`allow` path rules: deny beats allow, and a single-`/` path is relative to the
    // settings file. The `ask` list makes every call of every enabled tool a `can_use_tool`.
    permissions: { defaultMode: "default", blockWritesOutsideWorkingDirectories: true, ask: [...tools, ...mcpTools.filter((t) => t === SHELL_TOOL)] },
    env: {},
    ...(canary ? { outputStyle: CANARY_STYLE } : {}),
  });
}

/** The `--mcp-config` body: Finisher's SDK server only, served over `mcp_message`. */
export function mcpConfig(): { mcpServers: Record<string, { type: "sdk"; name: string }> } {
  return { mcpServers: { [MCP_SERVER]: { type: "sdk", name: MCP_SERVER } } };
}

/**
 * The role's allow-set as `--tools` (Decision 2): the mapped CLI tools it names, in `CLI_TOOLS`
 * order, with `Bash` when it names `Bash` or `fin` (a CLI reaches `fin` through its shell). With
 * the brokered shell (Decision 21, Windows only) that shell is `mcp__finisher__shell`, not
 * `Bash`. Names that map to nothing are `dropped`.
 */
export function cliTools(allow: readonly string[] | undefined, brokered: boolean): { tools: string[]; shell: boolean; dropped: string[] } {
  const named = new Set(allow ?? []);
  const shell = named.has("Bash") || named.has("fin");
  const tools = CLI_TOOLS.filter((t) => (t === "Bash" ? shell && !brokered : named.has(t)));
  const known = new Set<string>(CLI_TOOLS);
  const dropped = [...named].filter((t) => !known.has(t) && !BUILTIN_ONLY.has(t));
  return { tools, shell: shell && brokered, dropped };
}

/** The extra environment of every instance (Decision 5): non-secret switches, never a token. */
export function launchEnv(tmp: string): Record<string, string> {
  return {
    CLAUDE_CODE_TMPDIR: tmp,
    DISABLE_AUTOUPDATER: "1",
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    CLAUDE_CODE_DISABLE_AUTO_MEMORY: "1",
    CLAUDE_CODE_DISABLE_CLAUDE_MDS: "1",
    CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: "1",
    BASH_DEFAULT_TIMEOUT_MS: "120000",
  };
}

/**
 * The money left as the dollars `--max-budget-usd` takes, or `BUDGET_EXCEEDED`: 2.1.321 drops a
 * value <= 0 (`maxBudgetUsd>0`), which would be no cap at all, so a spent budget (or a remainder
 * that is no whole number of micros) never launches.
 */
function maxBudgetUsd(s: Session, micros: number, restored: number): string {
  if (!Number.isSafeInteger(micros) || micros <= 0) {
    throw finError("BUDGET_EXCEEDED", "the session's money budget is used up", { sessionId: s.id, kind: "money", remaining: String(micros) });
  }
  return String((micros + (Number.isSafeInteger(restored) && restored > 0 ? restored : 0)) / 1_000_000);
}

/**
 * The session's directories as its sandbox spells them, from the session environment's
 * `TMPDIR` (`<agents>/<id>/tmp`), or `SANDBOX_ENV`: absolute, normal, this session's.
 */
function sandboxDirs(s: Session, env: Readonly<Record<string, string>>, os: NodeJS.Platform): SessionDirs {
  const path = os === "win32" ? win32 : posix;
  const tmp = env.TMPDIR;
  const own = path.sep + path.join("agents", s.id, "tmp");
  if (tmp === undefined || !path.isAbsolute(tmp) || path.normalize(tmp) !== tmp || !tmp.endsWith(own)) {
    throw finError("SANDBOX_ENV", "the session environment is wrong: TMPDIR is not the session's own", { sessionId: s.id, name: "TMPDIR", reason: "not the session's own path" });
  }
  const dirs = sessionDirs(path.dirname(path.dirname(tmp)), s.id, os);
  // `sessionDirs` and the check above spell `tmp/` alike; a drift between them must not launch.
  if (dirs.tmp !== tmp) throw finError("INTERNAL", "the session's directories do not match its TMPDIR", { sessionId: s.id });
  return dirs;
}

/**
 * One instance's launch (Decision 1's order): a new session starts with `--session-id <uuid>`,
 * one with a provider reference resumes with `--resume=<uuid>`. Throws a `FinError`:
 * `PROVIDER_PROTOCOL` for a reference that is not this session's Claude Code session,
 * `INTERNAL` for settings that fail their schema, `SANDBOX_ENV` for a `TMPDIR` that is not the
 * session's own or an environment name outside the harness's, `BUDGET_EXCEEDED` for a money
 * budget with nothing left.
 */
export function launchPlan(s: Session, spec: SessionSpec, probe: ProbeResult, os: NodeJS.Platform, ctx: LaunchContext): LaunchPlan {
  const dirs = sandboxDirs(s, spec.env, os);
  const uuid = sessionUuid(s.id);
  const ref = spec.providerRef;
  if (ref !== null && (ref.kind !== "claudeCode" || ref.id !== uuid)) {
    throw finError("PROVIDER_PROTOCOL", "the session's provider reference is not its Claude Code session", { sessionId: s.id, kind: ref.kind });
  }
  const { tools, shell, dropped } = cliTools(spec.tools, ctx.brokeredShell === true || (os === "win32" && probe.brokeredShell === true));
  const budget = ctx.remainingMoneyMicros !== null && !ctx.planSignIn ? maxBudgetUsd(s, ctx.remainingMoneyMicros, ref === null ? 0 : (ctx.restoredCostMicros ?? 0)) : null;
  const plan = instancePlan({ binary: ctx.binary, dirs, os, uuid, resume: ref !== null, tools, shell, probe, model: spec.model, system: spec.system, retentionDays: ctx.retentionDays, budgetUsd: budget });
  // The CLI's temporary directory is the sandbox's own (`assertSessionEnv` holds it to `TMPDIR`).
  assertSpawnEnv(plan.env, s, os);
  return { ...plan, dropped };
}

/** One instance's inputs once the session is resolved (`launchPlan`, and the probe's host instance). */
export interface InstanceArgs {
  readonly binary: string;
  /** The instance's directories as its sandbox spells them (`settings/`, `tmp/`). */
  readonly dirs: SessionDirs;
  readonly os: NodeJS.Platform;
  readonly uuid: string;
  /** `--resume=<uuid>` instead of `--session-id <uuid>`. */
  readonly resume: boolean;
  /** The `--tools` set (`cliTools`). */
  readonly tools: readonly string[];
  /** The brokered shell is served (`cliTools`' `shell`). */
  readonly shell: boolean;
  readonly probe: Pick<ProbeResult, "permissionPath" | "restricted">;
  readonly model: string;
  readonly system: string;
  readonly retentionDays: number;
  /** `--max-budget-usd`'s value, or null for none. */
  readonly budgetUsd: string | null;
  /** The probe's settings canary (Decision 4): `outputStyle` the init must show. */
  readonly canary?: boolean;
}

/** The argv, files and environment of one instance (Decision 1's order). Throws `INTERNAL` for settings that fail their schema. */
export function instancePlan(a: InstanceArgs): Omit<LaunchPlan, "dropped"> {
  const at = (name: string) => (a.os === "win32" ? win32 : posix).join(a.dirs.settings, name);
  const tools = [...a.tools];
  const mcpTools = [...(a.shell ? [SHELL_TOOL] : []), ...(a.probe.permissionPath === "mcp" ? [APPROVE_TOOL] : [])];
  const files = [
    { path: at(SETTINGS_FILE), body: `${JSON.stringify(sessionSettings(tools, mcpTools, a.retentionDays, a.canary === true), null, 2)}\n` },
    { path: at(MCP_FILE), body: `${JSON.stringify(mcpConfig(), null, 2)}\n` },
    { path: at(ROLE_FILE), body: a.system },
  ];
  const argv = [
    a.binary,
    "-p",
    ...["--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"],
    ...["--permission-mode", "manual", "--permission-prompts", "host"],
    ...["--permission-prompt-tool", a.probe.permissionPath === "mcp" ? APPROVE_TOOL : "stdio"],
    ...(a.probe.restricted === true ? ["--restricted"] : ["--setting-sources", "user"]),
    ...["--settings", at(SETTINGS_FILE), "--strict-mcp-config", "--mcp-config", at(MCP_FILE)],
    ...["--tools", tools.join(","), "--disallowedTools", DISALLOWED_TOOLS.join(","), "--disable-slash-commands"],
    ...["--append-system-prompt-file", at(ROLE_FILE), "--model", a.model],
    ...(a.resume ? [`--resume=${a.uuid}`] : ["--session-id", a.uuid]),
    ...(a.budgetUsd === null ? [] : ["--max-budget-usd", a.budgetUsd]),
  ];
  return { argv, files, env: launchEnv(a.dirs.tmp), tools, mcpTools };
}

/**
 * Writes `plan.files` (into `settings/`, which `ensureSessionDirs` made): each to a fresh
 * temporary name beside it (0600, exclusive), then renamed over the old one, so the CLI never
 * reads a half-written file.
 */
export async function writeLaunchFiles(plan: Pick<LaunchPlan, "files">, os: NodeJS.Platform = process.platform): Promise<void> {
  for (const f of plan.files) {
    const tmp = `${f.path}.${randomBytes(6).toString("hex")}.tmp`;
    try {
      await writeFile(tmp, f.body, { mode: 0o600, flag: "wx" });
      if (os !== "win32") await chmod(tmp, 0o600);
      await rename(tmp, f.path);
    } catch (e) {
      await rm(tmp, { force: true });
      throw e;
    }
  }
}

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ULID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** A session ULID's 128 bits written as a UUID (Decision 12): lossless, no table. */
export function sessionUuid(ulid: string): string {
  const u = ulid.toUpperCase();
  if (!ULID.test(u)) throw finError("INVALID_REQUEST", "not a ULID", { ulid: ulid.slice(0, 32) });
  let n = 0n;
  for (const c of u) n = (n << 5n) | BigInt(CROCKFORD.indexOf(c));
  const h = n.toString(16).padStart(32, "0");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

/** The inverse of `sessionUuid`: the ULID a UUID's 128 bits spell (upper case). */
export function sessionUlid(uuid: string): string {
  const id = uuid.toLowerCase();
  if (!UUID.test(id)) throw finError("INVALID_REQUEST", "not a UUID", { uuid: uuid.slice(0, 40) });
  let n = BigInt(`0x${id.replaceAll("-", "")}`);
  let out = "";
  for (let i = 0; i < 26; i++) {
    out = CROCKFORD[Number(n & 31n)]! + out;
    n >>= 5n;
  }
  return out;
}
