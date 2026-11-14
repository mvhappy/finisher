// The Codex launch plan (plan 7E Task 7; Decisions 4–7, 9, 19, 21): the `app-server` argv, the
// one variable a Codex process adds to the session environment, and the `thread/start` and
// `thread/resume` parameters of one instance. Pure but for reading the committed pin files; the
// session (Task 10) spawns `argv` through the session sandbox with `cwd` = `startParams.cwd`.
//
// - **Overrides, not files**: every setting is a documented `-c` key under `--strict-config`, so
//   a misspelt key or a feature a re-pin drops fails the start instead of falling back to a
//   default (Task 0: exit 1 "unknown configuration field" / "Unknown feature flag").
// - **Every default-on feature is off unless `ALLOWED_FEATURES` names it** (Decision 6): the list
//   is read from the pin's committed `features.txt`, so a feature a newer pin turns on is off
//   until someone names it here.
// - **The role prompt goes over the protocol** (`developerInstructions`), never argv or a file in
//   the workspace (spec §4.6). Budget facts go in each turn's first input item (§4.1), not here.
// - **Commands are Finisher's** (Decision 9): Codex's shells are off; a role that may run
//   commands (`fin` or `Bash`) gets the one `shell` dynamic tool `finisherd` runs.
// - **No `forced_login_method`**: its values (`chatgpt | api`) would each refuse the other sign-in.
import { readFileSync } from "node:fs";
import { posix, win32 } from "node:path";
import type { Session } from "@finisher/contracts";
import { finError } from "../../errors.ts";
import type { SessionSpec } from "../adapter.ts";
import { assertSessionEnv, assertSpawnEnv, harnessExtra } from "../sandbox-env.ts";
import type { Closed, DynamicToolSpec, ThreadResumeParams, ThreadStartParams } from "./generated/protocol.ts";
import { codexBinary } from "./pin.ts";
import type { CodexProbe } from "./probe-types.ts";

export type { CodexProbe };

/** The role's allow-set as Codex's tools (`codexTools`). */
export interface CodexTools {
  /** The `shell` dynamic tool is declared (`fin` or `Bash`). */
  readonly shell: boolean;
  /**
   * The role may write files (`Edit`, `Write`, `MultiEdit`). No 0.167.0 key removes Codex's
   * `apply_patch` (Task 0), so `codex/permissions.ts` (Task 11) declines every patch when false.
   */
  readonly patches: boolean;
  /** `web_search="live"` (`WebSearch`), else `"disabled"`. */
  readonly webSearch: boolean;
  /** Role tool names no Codex tool maps to: never passed; the session records a notice. */
  readonly dropped: readonly string[];
}

/** One instance's launch (frozen in the plan; `tools` added for Tasks 10–11). */
export interface CodexLaunch {
  readonly argv: string[];
  /** What a Codex process adds to the session environment (`SpawnOptions.env`). */
  readonly env: Record<string, string>;
  /** What Finisher writes is `Closed`: no member this pin does not know. */
  readonly startParams: Closed<ThreadStartParams>;
  /** The `--disable` features, in `features.txt` order: the probe reads each back (Task 8). */
  readonly disabledFeatures: string[];
  readonly tools: CodexTools;
}

/** What the committed pin says beyond the binary (`vendor/codex/pin.json`). */
export interface CodexPinFacts {
  readonly version: string;
  /**
   * A resumed thread restores the dynamic `shell` declaration from its rollout (Decision 4),
   * observed signed in on this pin. Absent or unverified is `false`.
   */
  readonly resumeRestoresDynamicTools: boolean;
}

export interface CodexLaunchOptions {
  /** The bundled binary (default `codexBinary()`). */
  readonly binary?: string;
  /** The pin's `features list` output (default the committed `vendor/codex/<pin>/features.txt`). */
  readonly features?: string;
}

/**
 * The default-on features of the 0.167.0 pin Finisher keeps on, each for a reason. Every other
 * `true` row of `features.txt` gets `--disable` (when unsure: off; the probe reads every value
 * back, Task 8). Task 0 (unsigned) on 0.167.0: all 66 `--disable`s are accepted under
 * `--strict-config` and `config/read` reports each `false`.
 *
 * - `api_key_model_discovery` (stable): `model/list` lists an API-key account's models from the
 *   API instead of only the bundled catalogue; Decision 10 discovers models from that list. It
 *   loads nothing and adds no tool.
 * - `content_item_kinds` (stable): typed content items in tool outputs; Finisher's `shell`
 *   answer is a `contentItems` list (`inputText`, Decision 9). Kept so that answer reaches the
 *   model as built. TODO(7E Task 0 Step 3): confirm the signed-in `shell` round trip with it on.
 * - `item_ids` (removed): items carry ids, which the mapper keys every event on (Decision 14);
 *   the pin ignores `--disable` for it (`experimentalFeature/list` still reports it on).
 * - `sqlite` (removed): the state database under `CODEX_HOME` that thread persistence and
 *   `thread/resume` use (Decisions 12, 17); a removed switch gates nothing to turn off.
 * - `steer` (removed): `turn/steer` joining a running turn (Decision 13, owner Q4(a)); a removed
 *   switch gates nothing to turn off, and turning it off could only lose steer.
 *
 * Every other removed row (`collaboration_modes`, `realtime_conversation`, `resize_all_images`,
 * `terminal_resize_reflow`, `tool_search_always_defer_mcp_tools`, `tui_app_server`,
 * `worktrees`) is disabled anyway: harmless, and off if a re-pin revives it.
 *
 * Read back on 0.167.0 (unsigned, Task 7): `config/read` reports every `--disable` as `false`,
 * but `experimentalFeature/list` still reports `unified_exec` and six removed rows enabled.
 * TODO(7E Task 0 Step 3): whether Codex offers `unified_exec` to the model with `shell_tool`
 * off; Task 8's probe must not take `config/read` alone as proof for it.
 */
export const ALLOWED_FEATURES: ReadonlySet<string> = new Set(["api_key_model_discovery", "content_item_kinds", "item_ids", "sqlite", "steer"]);

/** The role tool names that map to a Codex tool. */
const SHELL_NAMES: ReadonlySet<string> = new Set(["fin", "Bash"]);
const PATCH_NAMES: ReadonlySet<string> = new Set(["Edit", "Write", "MultiEdit"]);
/** The built-in agent's own tools: not CLI tools, neither mapped nor noticed (7D's rule). */
const BUILTIN_ONLY: ReadonlySet<string> = new Set(["workspace_read", "workspace_write", "workspace_list"]);

/** Finisher's `shell` (Decision 9): run by `finisherd` through the session sandbox (Task 12). */
export const SHELL_DYNAMIC_TOOL: Extract<Closed<DynamicToolSpec>, { type: "function" }> = {
  type: "function",
  name: "shell",
  description:
    "Run one shell command in the session's workspace through the session's shell and return its exit status, " +
    "standard output and standard error (the tail, capped). `fin` is on PATH. Every command is checked against the " +
    "session's rules and may be asked of the person first.",
  inputSchema: {
    type: "object",
    properties: {
      command: { type: "string", description: "The command line, run by the session's shell." },
      timeoutMs: { type: "integer", minimum: 1, description: "Stop the command after this many milliseconds." },
    },
    required: ["command"],
    additionalProperties: false,
  },
};

/** Decision 7's fallback policy: patches still ask (`sandbox_approval`), nothing else is granted. */
const GRANULAR = { granular: { sandbox_approval: true, rules: true, mcp_elicitations: false, request_permissions: false, skill_approval: false } } as const;

/** The role's allow-set as Codex's tools; `undefined` (no allow-set) maps to none. */
export function codexTools(allow: readonly string[] | undefined): CodexTools {
  const named = [...new Set(allow ?? [])];
  const known = (t: string) => SHELL_NAMES.has(t) || PATCH_NAMES.has(t) || t === "WebSearch" || BUILTIN_ONLY.has(t);
  return {
    shell: named.some((t) => SHELL_NAMES.has(t)),
    patches: named.some((t) => PATCH_NAMES.has(t)),
    webSearch: named.includes("WebSearch"),
    dropped: named.filter((t) => !known(t)),
  };
}

/**
 * The features of a `features list` output (`name  stage  default`, the stage one or two words)
 * that are on by default and not in `ALLOWED_FEATURES`, in the file's order. Throws `INTERNAL`
 * for a row whose default is neither `true` nor `false`, or a text with no rows: a list that
 * cannot be read cannot prove anything off.
 */
export function disabledFeatures(text: string): string[] {
  const out: string[] = [];
  let rows = 0;
  for (const line of text.split("\n")) {
    const cols = line.trim().split(/\s+/);
    if (cols.length === 1 && cols[0] === "") continue;
    const [name, last] = [cols[0]!, cols.at(-1)!];
    if (cols.length < 3 || !/^[a-z0-9_.]+$/.test(name) || (last !== "true" && last !== "false")) {
      throw finError("INTERNAL", "the pinned Codex feature list has a row that is not `name stage default`", { row: line.slice(0, 120) });
    }
    rows++;
    if (last === "true" && !ALLOWED_FEATURES.has(name)) out.push(name);
  }
  if (rows === 0) throw finError("INTERNAL", "the pinned Codex feature list is empty", {});
  return out;
}

const VENDOR = new URL("../../../../../vendor/codex/", import.meta.url);

/** `vendor/codex/pin.json`'s version and `resumeRestoresDynamicTools` (absent: false). */
export function codexPinFacts(): CodexPinFacts {
  const pin = JSON.parse(readFileSync(new URL("pin.json", VENDOR), "utf8")) as { version: string; resumeRestoresDynamicTools?: unknown };
  // TODO(7E Task 0 Step 3): the signed-in resume run records `resumeRestoresDynamicTools` here.
  return { version: pin.version, resumeRestoresDynamicTools: pin.resumeRestoresDynamicTools === true };
}

const featuresOf = (version: string) => readFileSync(new URL(`${version}/features.txt`, VENDOR), "utf8");

/** A TOML basic string (`-c` values are TOML): JSON's escapes are TOML's, plus DEL. */
export function tomlString(v: string): string {
  if (!v.isWellFormed()) throw finError("INTERNAL", "a session variable is not valid Unicode", {});
  return JSON.stringify(v).replaceAll("\u007f", "\\u007F");
}

/** `{K="v", …}`, keys sorted: an inline table, bare keys where TOML allows them. */
export function tomlTable(env: Readonly<Record<string, string>>): string {
  const key = (k: string) => (/^[A-Za-z0-9_-]+$/.test(k) ? k : tomlString(k));
  const rows = Object.keys(env)
    .sort()
    .map((k) => `${key(k)}=${tomlString(env[k]!)}`);
  return `{${rows.join(", ")}}`;
}

/** The thread sandbox and approval policy (Decision 7), from the probe. */
function threadSandbox(probe: CodexProbe): Pick<Closed<ThreadStartParams>, "sandbox" | "approvalPolicy"> {
  if (probe.sandbox === "danger-full-access" && probe.approval !== "granular") {
    // Full access with `on-request` lets every patch through unasked: never launched.
    throw finError(
      "PROVIDER_VERSION",
      "Codex's patches would not ask on this system",
      { harness: "codex", version: probe.version, missing: "patchApproval" },
      "use a Finisher build whose Codex asks before every patch on this system",
    );
  }
  return { sandbox: probe.sandbox, approvalPolicy: probe.approval === "granular" ? GRANULAR : "on-request" };
}

/**
 * `spec.cwd`, checked to be exactly `<FIN_PROJECT_DIR>/workspace/<sessionId>` of the session's
 * own environment (absolute, normal): not merely a folder ending in `workspace/<sessionId>`.
 * Throws `INTERNAL`.
 */
function workspaceCwd(s: Session, cwd: string, env: Readonly<Record<string, string>>, os: NodeJS.Platform): string {
  const path = os === "win32" ? win32 : posix;
  const project = env.FIN_PROJECT_DIR;
  if (project === undefined || !path.isAbsolute(project) || !path.isAbsolute(cwd) || path.normalize(cwd) !== cwd || cwd !== path.join(project, "workspace", s.id)) {
    throw finError("INTERNAL", "the session's working directory is not its own workspace", { sessionId: s.id });
  }
  return cwd;
}

/**
 * One instance's launch: the argv (Decision 4's order), `NO_COLOR` on top of the session
 * environment, and `thread/start`'s parameters. Throws a `FinError`: `PROVIDER_VERSION` for a
 * probe of another version than the pin or a sandbox under which patches would not ask,
 * `SANDBOX_ENV` for a session environment that is not the session's own (`HOME`, `CODEX_HOME`,
 * a secret…), `INTERNAL` for a cwd outside `workspace/<sessionId>/` or an unreadable feature list,
 * `PROVIDER_UNAVAILABLE` when no binary is bundled.
 */
export function codexLaunch(s: Session, spec: SessionSpec, probe: CodexProbe, os: NodeJS.Platform, o: CodexLaunchOptions = {}): CodexLaunch {
  const pinned = codexPinFacts().version;
  if (probe.version !== pinned) {
    throw finError("PROVIDER_VERSION", `Codex ${probe.version} is not the pinned ${pinned}`, { harness: "codex", version: probe.version, pinned }, "reinstall Finisher");
  }
  const cwd = workspaceCwd(s, spec.cwd, spec.env, os);
  const tools = codexTools(spec.tools);
  const thread = threadSandbox(probe);
  // The session environment (Decision 5: `CODEX_HOME` = `provider/`, `HOME` = `tmp/home`, `SHELL`)
  // is the sandbox's; a Codex process adds `NO_COLOR` only, and the whole is held to the contract.
  // TODO(7E Task 0 Step 4): Q8(b) fallback if refresh renames. As built, `CODEX_HOME` is the
  // session's own `provider/`, whose `auth.json` links the shared store (`SIGN_IN_STORES.codex`);
  // if a refresh replaces `auth.json` by rename, spec §8.6's fallback branches here.
  const env = { NO_COLOR: "1" };
  assertSpawnEnv(env, s, os);
  assertSessionEnv({ ...spec.env, ...env }, s, { extra: harnessExtra("codex"), platform: os });
  const disabled = disabledFeatures(o.features ?? featuresOf(pinned));
  const config = [
    'cli_auth_credentials_store="file"',
    "project_doc_max_bytes=0",
    "project_doc_fallback_filenames=[]",
    "check_for_update_on_startup=false",
    "analytics.enabled=false",
    'history.persistence="none"',
    `web_search="${tools.webSearch ? "live" : "disabled"}"`,
    'shell_environment_policy.inherit="none"',
    // Only for commands Codex would run itself, which Decision 9 removes: §8.5's environment.
    `shell_environment_policy.set=${tomlTable(spec.env)}`,
    "skills.bundled.enabled=false",
    "mcp_servers={}",
    "agents.enabled=false",
  ];
  const argv = [
    o.binary ?? codexBinary(),
    ...["app-server", "--listen", "stdio://", "--strict-config"],
    ...config.flatMap((c) => ["-c", c]),
    ...disabled.flatMap((f) => ["--disable", f]),
  ];
  const startParams: Closed<ThreadStartParams> = {
    cwd,
    model: spec.model,
    ...thread,
    approvalsReviewer: "user",
    developerInstructions: spec.system,
    ephemeral: false,
    serviceName: "finisher",
    ...(tools.shell ? { dynamicTools: [SHELL_DYNAMIC_TOOL] } : {}),
    config: {},
  };
  return { argv, env, startParams, disabledFeatures: disabled, tools };
}

/**
 * `thread/resume`'s parameters (Decision 4): the settings common with `l.startParams`, never the
 * start-only `dynamicTools`, `ephemeral` or `serviceName` (the pin's `ThreadResumeParams` has
 * none). A thread that declared `shell` resumes only on a pin that recorded it restores the
 * declaration from its rollout, else `PROVIDER_VERSION {missing: "resumeDynamicTools"}`.
 *
 * The decision reads today's role (`l`, built from the session's current allow-set), not the
 * tools the thread was started with: a thread started with `shell` whose role no longer allows it
 * resumes here, and its rollout may still declare `shell`.
 * TODO(7E Task 12): the `shell` handler refuses every `item/tool/call` for `shell` when
 * `l.tools.shell` is false (a call the role does not allow is never run, whatever the thread says).
 */
export function threadResumeParams(l: CodexLaunch, threadId: string, pin: CodexPinFacts = codexPinFacts()): Closed<ThreadResumeParams> {
  if (threadId === "") throw finError("INVALID_REQUEST", "a Codex thread id is empty", {});
  const s = l.startParams;
  if ((s.dynamicTools ?? []).some((t) => t.type === "function" && t.name === SHELL_DYNAMIC_TOOL.name) && !pin.resumeRestoresDynamicTools) {
    throw finError(
      "PROVIDER_VERSION",
      `Codex ${pin.version} is not known to restore the session's shell tool on resume`,
      { harness: "codex", version: pin.version, missing: "resumeDynamicTools" },
      "start a new session",
    );
  }
  return {
    threadId,
    cwd: s.cwd ?? null,
    model: s.model ?? null,
    sandbox: s.sandbox ?? null,
    approvalPolicy: s.approvalPolicy ?? null,
    approvalsReviewer: s.approvalsReviewer ?? null,
    developerInstructions: s.developerInstructions ?? null,
    config: s.config ?? null,
  };
}
