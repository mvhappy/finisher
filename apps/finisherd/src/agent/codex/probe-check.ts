// The Codex capability predicate (plan 7E Decision 8, Task 8; Review Focus 1 and 6), apart from
// the probe's process code: what an instance's `initialize`, `config/read {includeLayers: true}`,
// `configRequirements/read`, five lists and `thread/start` answer must hold, and the gate that
// holds its events until they did. The probe (`probe.ts`) runs it step by step on a throwaway
// home; **every real instance re-runs all of it** (`checkInstance`) before its first event reaches
// the runtime (`instanceGate`, on `cli/gate.ts`).
//
// Drift fails closed: a key the predicate reads that is absent is a failure naming it, never a
// pass. What each check reads (0.167.0, Task 0 and Task 7 on the real binary, unsigned):
//
// - **Version**: `initialize.userAgent` is `<originator>/<codex version> (…)`; another version than
//   the pin is `PROVIDER_VERSION {version, pinned}`. `codexHome` is the instance's own `provider/`.
// - **Decision 4's keys**: the *effective* `config` value of each (`expectedConfig`; `web_search`
//   per role; `shell_environment_policy.set` equal to the argv's table) **and** every scalar's
//   origin `sessionFlags`. `forced_login_method` never set.
// - **Decision 6's features**: for each `--disable F` of the plan, the effective
//   `config.features.F` is `false` (`sleep_tool` is `{enabled: false}`-shaped in its origin:
//   `features.sleep_tool.enabled`) **and** its origin is `sessionFlags`; and no `features` entry is
//   on unless `ALLOWED_FEATURES` names it. **`experimentalFeature/list` is not read**: on 0.167.0 it
//   still reports `unified_exec` (and six removed rows) `enabled` under `--disable` while
//   `config/read` reports `false` (Task 7), so neither list alone proves a feature off and the
//   list cannot be a predicate the real binary passes. `config/read` effective value plus origin
//   is what Finisher can check unsigned; whether the model is still offered `unified_exec` is
//   TODO(7E Task 0 Step 3) (signed in: list the model's tools with `shell_tool` and `unified_exec`
//   off; if it is offered, a session must refuse the pin).
// - **Origins and layers**: every origin is `sessionFlags` (nothing from a file); the layers are
//   `sessionFlags`, `user` (exactly `<codexHome>/config.toml`, empty) and `system` (empty: a
//   `/etc/codex/config.toml` with content is refused); a layer of any other type (project, MDM,
//   managed, packaged defaults, one this pin does not know) is refused unless it is disabled
//   (`disabledReason`), since a disabled layer contributes nothing (Task 0: an untrusted project
//   `.codex/` is not even listed).
// - **Managed requirements** (`configRequirements/read`, read as sent): `null`, or only keys that
//   restrict and still admit Finisher's settings (`requirementProblem`); anything that turns an
//   approval off, a feature or a capability on, injects instructions, or is not known is
//   `PROVIDER_VERSION {missing: "managedConfig", item}`.
// - **Lists**: `skills/list`, `hooks/list`, `plugin/installed` and `app/installed` empty,
//   `mcpServerStatus/list` ⊆ Finisher's servers (none today): else `isolation` naming the item.
// - **Thread**: `thread/start`'s `approvalPolicy`, `approvalsReviewer`, `sandbox` and `cwd` are the
//   plan's, and `instructionSources` is present and empty (`$CODEX_HOME/AGENTS.md` and the
//   workspace's are sources otherwise).
import type { FinError } from "@finisher/contracts";
import { posix, win32 } from "node:path";
import { Effect } from "effect";
import { finError } from "../../errors.ts";
import { type EventGate, eventGate } from "../cli/gate.ts";
import { ALLOWED_FEATURES, type CodexLaunch, codexPinFacts } from "./launch.ts";
import type { CodexRpc, ResultOf } from "./rpc.ts";

/** What one instance answered: the checked facts. `config` and `requirements` are as sent. */
export interface InstanceFacts {
  readonly initialize: ResultOf<"initialize">;
  /** `config/read {includeLayers: true}` as Codex sent it (`requestRaw`: the generated `Config` drops `features`…). */
  readonly config: unknown;
  /** `configRequirements/read` as sent (an unknown requirement must be seen to be refused). */
  readonly requirements: unknown;
  readonly skills: ResultOf<"skills/list">;
  readonly hooks: ResultOf<"hooks/list">;
  readonly plugins: ResultOf<"plugin/installed">;
  readonly mcpServers: ResultOf<"mcpServerStatus/list">;
  readonly apps: ResultOf<"app/installed">;
  readonly thread: ResultOf<"thread/start">;
  /**
   * The instance's `CODEX_HOME` (its `provider/`, from the sandbox environment); `initialize.codexHome`
   * must be it, and the user config layer `<codexHome>/config.toml`. Compared as plain strings:
   * a canonicalised path (macOS `/private/var/…`) would refuse falsely (Task 20 checks it).
   */
  readonly codexHome: string;
}

/** The MCP servers Finisher configures for Codex: none (`-c mcp_servers={}`, Decision 6). */
export const FINISHER_MCP_SERVERS: ReadonlySet<string> = new Set();

/** `PROVIDER_VERSION {harness: codex, missing, …}`. */
export function versionError(missing: string, message: string, extra: Record<string, unknown> = {}): FinError {
  return finError("PROVIDER_VERSION", message, { harness: "codex", missing, ...extra }, "reinstall Finisher, or remove the configuration that reaches Codex");
}

const isolation = (item: string, why: string): FinError => versionError("isolation", `the Codex instance is not isolated: ${why}`, { item: item.slice(0, 200) });
const managed = (item: string, why: string): FinError => versionError("managedConfig", `a managed Codex requirement conflicts with Finisher's settings: ${why}`, { item });

const plain = (u: unknown): u is Record<string, unknown> => typeof u === "object" && u !== null && !Array.isArray(u);
const label = (v: unknown): string => (typeof v === "string" ? v : (JSON.stringify(v) ?? String(v))).slice(0, 80);

/** Structural equality of JSON values (object key order ignored). */
export function jsonEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) || Array.isArray(b)) return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((x, i) => jsonEqual(x, b[i]));
  if (!plain(a) || !plain(b)) return false;
  const ka = Object.keys(a);
  return ka.length === Object.keys(b).length && ka.every((k) => Object.hasOwn(b, k) && jsonEqual(a[k], b[k]));
}

// ---- initialize ----

/** `<originator>/<x.y.z…> (…)`: the Codex version `userAgent` names. */
const USER_AGENT = /^[^/\s]+\/(\d+\.\d+\.\d+[^\s]*)\s/;

/** The version and `CODEX_HOME` an `initialize` answer names. */
export function checkInitialize(r: ResultOf<"initialize">, pinned: string, codexHome: string): FinError | null {
  const m = USER_AGENT.exec(r.userAgent);
  if (m === null) return versionError("userAgent", "Codex's initialize names no version", { userAgent: label(r.userAgent) });
  if (m[1] !== pinned) return finError("PROVIDER_VERSION", `Codex ${m[1]} is not the pinned ${pinned}`, { harness: "codex", version: m[1], pinned }, "reinstall Finisher");
  if (r.codexHome !== codexHome) return isolation("codexHome", "it runs on another CODEX_HOME than its own");
  return null;
}

// ---- config/read ----

/** Decision 4's `-c` keys and the effective value each must have (`web_search` is the role's). */
export function expectedConfig(plan: Pick<CodexLaunch, "tools">): ReadonlyArray<readonly [string, unknown]> {
  return [
    ["cli_auth_credentials_store", "file"],
    ["project_doc_max_bytes", 0],
    ["project_doc_fallback_filenames", []],
    ["check_for_update_on_startup", false],
    ["analytics.enabled", false],
    ["history.persistence", "none"],
    ["web_search", plan.tools.webSearch ? "live" : "disabled"],
    ["shell_environment_policy.inherit", "none"],
    ["skills.bundled.enabled", false],
    ["mcp_servers", {}],
    ["agents.enabled", false],
  ];
}

const at = (o: unknown, path: string): unknown => path.split(".").reduce<unknown>((n, k) => (plain(n) ? n[k] : undefined), o);
const hasPath = (o: unknown, path: string): boolean => {
  const parts = path.split(".");
  const parent = parts.slice(0, -1).reduce<unknown>((n, k) => (plain(n) ? n[k] : undefined), o);
  return plain(parent) && Object.hasOwn(parent, parts.at(-1)!);
};

const SET_PREFIX = "shell_environment_policy.set=";
const TABLE_ROW = /\s*(?:([A-Za-z0-9_-]+)|("(?:[^"\\]|\\.)*"))=("(?:[^"\\]|\\.)*")\s*(,|$)/y;

/** The argv's `shell_environment_policy.set={…}` table (as `launch.ts` writes it), or `undefined`. */
export function planShellSet(plan: Pick<CodexLaunch, "argv">): Record<string, string> | undefined {
  const raw = plan.argv.find((a, i) => plan.argv[i - 1] === "-c" && a.startsWith(SET_PREFIX))?.slice(SET_PREFIX.length);
  if (raw === undefined || !raw.startsWith("{") || !raw.endsWith("}")) return undefined;
  const body = raw.slice(1, -1);
  const out: Record<string, string> = {};
  TABLE_ROW.lastIndex = 0;
  while (TABLE_ROW.lastIndex < body.length) {
    const m = TABLE_ROW.exec(body);
    if (m === null) return undefined;
    out[m[1] ?? (JSON.parse(m[2]!) as string)] = JSON.parse(m[3]!) as string;
    if (m[4] === "") break;
  }
  return out;
}

const sessionFlags = (o: unknown): boolean => plain(o) && plain(o.name) && o.name.type === "sessionFlags";

/** A feature's effective value is off: `false`, or a table with `enabled: false`. */
const featureOff = (v: unknown): boolean => v === false || (plain(v) && v.enabled === false);
/** Unset (`null`): the pin's default, which is off for every feature not in `ALLOWED_FEATURES` and not disabled (a disabled one is checked `false` above). */
const featureUnset = (v: unknown): boolean => v === null;

/** `config/read {includeLayers: true}` (as sent) against the plan. */
export function checkConfig(raw: unknown, plan: Pick<CodexLaunch, "argv" | "tools" | "disabledFeatures">, codexHome: string): FinError | null {
  if (!plain(raw) || !plain(raw.config)) return versionError("config", "Codex's config/read has no config");
  const { config } = raw;
  const origins = raw.origins;
  if (!plain(origins)) return versionError("configOrigins", "Codex's config/read has no origins");
  if (!Array.isArray(raw.layers)) return versionError("configLayers", "Codex's config/read has no layers");
  // Decision 4: each key's effective value, and a scalar's origin (an empty table or list has none).
  for (const [key, want] of expectedConfig(plan)) {
    if (!hasPath(config, key)) return versionError(`config.${key}`, `Codex's config/read has no ${key}`);
    const got = at(config, key);
    if (!jsonEqual(got, want)) return isolation(`config:${key}`, `${key} is ${label(got)}, not ${label(want)}`);
    if (typeof want !== "object" && !sessionFlags(origins[key])) return isolation(`config:${key}`, `${key} does not come from Finisher's flags`);
  }
  const set = planShellSet(plan);
  const gotSet = at(config, "shell_environment_policy.set");
  if (set === undefined) return versionError("shellEnvironment", "the launch plan has no shell_environment_policy.set");
  if (!jsonEqual(gotSet, set)) return isolation("config:shell_environment_policy.set", "the shell environment is not the session's");
  const forced = config.forced_login_method;
  if (forced !== undefined && forced !== null) return isolation("config:forced_login_method", `forced_login_method is ${label(forced)}`);
  // Decision 6: every `--disable F` off, from Finisher's flags; nothing on that Finisher did not name.
  const features = config.features;
  if (!plain(features)) return versionError("config.features", "Codex's config/read has no features");
  for (const f of plan.disabledFeatures) {
    if (!Object.hasOwn(features, f) || !featureOff(features[f])) return isolation(`feature:${f}`, `feature ${f} is not off`);
    if (!sessionFlags(origins[`features.${f}`]) && !sessionFlags(origins[`features.${f}.enabled`])) return isolation(`feature:${f}`, `feature ${f} is not off by Finisher's flags`);
  }
  for (const [f, v] of Object.entries(features)) {
    if (!featureOff(v) && !featureUnset(v) && !ALLOWED_FEATURES.has(f)) return isolation(`feature:${f}`, `feature ${f} is ${label(v)}, not off`);
  }
  // Nothing from a file: every origin is a flag, the user layer is the empty throwaway one.
  for (const [key, o] of Object.entries(origins)) {
    if (!sessionFlags(o)) return isolation(`configOrigin:${key}`, `${key} comes from ${label(plain(o) && plain(o.name) ? o.name.type : o)}`);
  }
  const path = posix.isAbsolute(codexHome) ? posix : win32;
  for (const layer of raw.layers as unknown[]) {
    if (!plain(layer) || !plain(layer.name)) return versionError("configLayers", "a Codex config layer has no name");
    const type = layer.name.type;
    const empty = plain(layer.config) && Object.keys(layer.config).length === 0;
    if (type === "sessionFlags") continue;
    if (typeof layer.disabledReason === "string" && layer.disabledReason !== "") continue;
    if (type === "user") {
      if (layer.name.file !== path.join(codexHome, "config.toml")) return isolation("configLayer:user", `the user config is ${label(layer.name.file)}`);
      if (!empty) return isolation("configLayer:user", "the user config is not empty");
      continue;
    }
    if (type === "system" && empty) continue;
    return isolation(`configLayer:${label(type)}`, `a ${label(type)} config layer${typeof layer.name.file === "string" ? ` (${label(layer.name.file)})` : ""} applies`);
  }
  return null;
}

// ---- configRequirements/read ----

/** Requirement keys that only restrict: accepted while they still admit Finisher's setting. */
type Rule = (v: unknown, plan: Pick<CodexLaunch, "startParams" | "tools">) => boolean;
const admits = (want: (plan: Pick<CodexLaunch, "startParams" | "tools">) => unknown): Rule => (v, plan) => Array.isArray(v) && v.some((x) => jsonEqual(x, want(plan)));
const offOnly: Rule = (v) => v === false;
const REQUIREMENT_RULES: Readonly<Record<string, Rule>> = {
  allowedApprovalPolicies: admits((p) => p.startParams.approvalPolicy),
  allowedSandboxModes: admits((p) => p.startParams.sandbox),
  allowedWebSearchModes: admits((p) => (p.tools.webSearch ? "live" : "disabled")),
  // Sign-in methods and Windows sandbox implementations: a restriction, nothing reaches a session.
  allowedLoginMethods: (v) => Array.isArray(v),
  allowedWindowsSandboxImplementations: (v) => Array.isArray(v),
  // Only switching a feature off; a required `true` is allowed only for one Finisher keeps on.
  featureRequirements: (v) => plain(v) && Object.entries(v).every(([f, on]) => on === false || (on === true && ALLOWED_FEATURES.has(f))),
  cliAuthCredentialsStore: (v) => v === "file",
  checkForUpdateOnStartup: offOnly,
  allowLoginShell: offOnly,
  allowRemoteControl: offOnly,
  allowAppshots: offOnly,
  allowBrowserAndComputerUse: offOnly,
  // `true` keeps managed hooks only: there are managed hooks to run.
  allowManagedHooksOnly: offOnly,
};

/**
 * `configRequirements/read` (as sent): `null` when no requirement conflicts, else
 * `PROVIDER_VERSION {missing: "managedConfig", item}`. Every key not in `REQUIREMENT_RULES`
 * (`additionalDeveloperInstructions`, `models`, `modelProvider(s)`, `chatgptBaseUrl`,
 * `defaultPermissions`, `computerUse`, `browserUse`, `autoReview`, `otel`, `logDir`,
 * `sqliteHome`, … and any a re-pin adds) is refused when set: it adds or redirects something.
 */
export function checkRequirements(raw: unknown, plan: Pick<CodexLaunch, "startParams" | "tools">): FinError | null {
  if (!plain(raw)) return versionError("configRequirements", "Codex's configRequirements/read is not an object");
  const req = raw.requirements;
  if (req === null || req === undefined) return null;
  if (!plain(req)) return managed("requirements", "the requirements are not a table");
  for (const [key, v] of Object.entries(req)) {
    if (v === null) continue;
    const rule = Object.hasOwn(REQUIREMENT_RULES, key) ? REQUIREMENT_RULES[key] : undefined;
    if (rule === undefined) return managed(key, `${key} is set`);
    if (!rule(v, plan)) return managed(key, `${key} is ${label(v)}`);
  }
  return null;
}

// ---- the lists ----

/** The five lists: nothing loaded that Finisher did not configure. */
export function checkLists(f: Pick<InstanceFacts, "skills" | "hooks" | "plugins" | "mcpServers" | "apps">, mcp: ReadonlySet<string> = FINISHER_MCP_SERVERS): FinError | null {
  const skill = f.skills.data.flatMap((e) => e.skills)[0];
  if (skill !== undefined) return isolation(`skill:${skill.name}`, `the skill ${label(skill.name)} loads (${label(skill.path)})`);
  const hook = f.hooks.data.flatMap((e) => e.hooks)[0] as { eventName?: unknown; sourcePath?: unknown } | undefined;
  if (hook !== undefined) return isolation(`hook:${label(hook.eventName)}`, `a ${label(hook.eventName)} hook is installed (${label(hook.sourcePath)})`);
  const market = f.plugins.marketplaces[0];
  if (market !== undefined) {
    const p = market.plugins[0];
    return isolation(p === undefined ? `marketplace:${market.name}` : `plugin:${p.id}`, p === undefined ? `the plugin marketplace ${label(market.name)} is configured` : `the plugin ${label(p.id)} is installed`);
  }
  const server = f.mcpServers.data.find((s) => !mcp.has(s.name));
  if (server !== undefined) return isolation(`mcpServer:${server.name}`, `the MCP server ${label(server.name)} is configured`);
  if (f.mcpServers.nextCursor !== undefined && f.mcpServers.nextCursor !== null) return isolation("mcpServer:more", "Codex lists more MCP servers");
  const app = f.apps.apps[0];
  if (app !== undefined) return isolation(`app:${app.id}`, `the app ${label(app.id)} is installed`);
  return null;
}

// ---- thread/start ----

/** The sandbox policy `thread/start` answers for the plan's sandbox mode (Task 0: camelCase `readOnly`). */
const sandboxMatches = (got: unknown, mode: unknown): boolean => {
  if (!plain(got)) return false;
  if (mode === "read-only") return got.type === "readOnly" && got.networkAccess !== true;
  if (mode === "danger-full-access") return got.type === "dangerFullAccess";
  return false;
};

/** `thread/start`'s answer against the plan's parameters. */
export function checkThread(r: ResultOf<"thread/start">, plan: Pick<CodexLaunch, "startParams">): FinError | null {
  const s = plan.startParams;
  const fallback = checkSandboxPlan(plan);
  if (fallback !== null) return fallback;
  const wrong = (field: string, got: unknown) => versionError(field, `Codex's thread runs with ${field} ${label(got)}, not Finisher's`, { item: field });
  if (!jsonEqual(r.approvalPolicy, s.approvalPolicy)) return wrong("approvalPolicy", r.approvalPolicy);
  if (r.approvalsReviewer !== s.approvalsReviewer) return wrong("approvalsReviewer", r.approvalsReviewer);
  if (!sandboxMatches(r.sandbox, s.sandbox)) return wrong("sandbox", r.sandbox);
  if (r.cwd !== s.cwd) return wrong("cwd", r.cwd);
  if (r.instructionSources === undefined) return versionError("instructionSources", "Codex's thread/start names no instruction sources");
  const source = r.instructionSources[0];
  if (source !== undefined) return isolation(`instructions:${source}`, `${label(source)} is an instruction source`);
  return null;
}

/**
 * Only `read-only` with `on-request` is accepted (Task 7 review): nothing unsigned proves Codex's
 * own exec tool off (`experimentalFeature/list` reports `unified_exec` enabled under `--disable`,
 * `config/read` reports it `false`), and under Decision 7's fallback (`danger-full-access` with a
 * granular policy) a command Codex ran itself would not be confined by Codex's sandbox.
 * TODO(7E Task 0 Step 3): once a signed-in run shows the model is not offered Codex's exec tool
 * (and patches still ask under the fallback), admit the fallback here.
 */
export function checkSandboxPlan(plan: Pick<CodexLaunch, "startParams">): FinError | null {
  const s = plan.startParams;
  if (s.sandbox === "read-only" && s.approvalPolicy === "on-request") return null;
  return versionError("patchApproval", "Codex's fallback sandbox is not accepted: Finisher cannot yet prove Codex runs no command of its own", { reason: "unverifiedExec", sandbox: label(s.sandbox), approvalPolicy: label(s.approvalPolicy) });
}

/** A refused `thread/start`: `dynamicTools` when the pin does not take them (Decision 9). */
export function threadStartRefused(e: FinError): FinError {
  const d = e.details as { reason?: unknown; rpcError?: { message?: unknown } } | null;
  if (e.code !== "PROVIDER_PROTOCOL" || d?.reason !== "rpcError") return e;
  const message = String(d.rpcError?.message ?? "");
  if (/dynamic_?tools/i.test(message)) return versionError("dynamicTools", "this Codex does not take Finisher's shell tool (dynamic tools)", { rpcError: message.slice(0, 200) });
  return versionError("threadStart", "Codex refused Finisher's thread parameters", { rpcError: message.slice(0, 200) });
}

// ---- the whole ----

/**
 * The predicate on one instance's facts against the plan it started with (Decision 8): null when
 * it holds, else the `PROVIDER_VERSION` the turn fails with (the caller kills the group).
 */
export function checkInstance(facts: InstanceFacts, plan: CodexLaunch): FinError | null {
  const pinned = codexPinFacts().version;
  return (
    checkInitialize(facts.initialize, pinned, facts.codexHome) ??
    checkConfig(facts.config, plan, facts.codexHome) ??
    checkRequirements(facts.requirements, plan) ??
    checkLists(facts) ??
    checkThread(facts.thread, plan)
  );
}

/** What an instance's gate holds: its events, and the facts it is checked on. */
export type InstanceItem<E> = { readonly kind: "event"; readonly event: E } | { readonly kind: "facts"; readonly facts: InstanceFacts };

/**
 * The gate of one instance: `push` each event (`{kind: "event"}`), `verify` the facts once
 * collected; nothing is forwarded before `checkInstance` passed, and a failure refuses (kill).
 */
export function instanceGate<E extends { readonly method: string; readonly name?: string }>(plan: CodexLaunch): EventGate<InstanceItem<E>> {
  const gate = eventGate<InstanceItem<E>>((item) => (item.kind === "facts" ? checkInstance(item.facts, plan) : versionError("instanceFacts", "an event was checked in place of the instance's facts")));
  return {
    get state() {
      return gate.state;
    },
    // A skill planted mid-instance under `$CODEX_HOME/skills`: the pin announces it with
    // `skills/changed` (not decoded: it arrives as `unknown`), so the checked `skills/list == []`
    // no longer holds: refuse. A skill planted in the workspace's `.agents/skills` raises **no**
    // notification on 0.167.0 (review, real binary: none within 4 s, `skills/list` stays cached
    // at 0, `{forceReload: true}` lists it): `recheckSkills` before each turn covers that.
    push: (item) => (item.kind === "event" && skillsChanged(item.event) ? gate.refuse(isolation("skills:changed", "a skill appeared while it ran")) : gate.push(item)),
    verify: (item) => gate.verify(item),
    refuse: (e) => gate.refuse(e),
    withdraw: (m) => gate.withdraw(m),
    discard: () => gate.discard(),
  };
}

/**
 * Before each turn of an instance (TODO(7E Task 10): the session calls it ahead of every
 * `turn/start` and refuses the turn, killing the group, on a failure): `skills/list {cwds,
 * forceReload: true}` must still be empty, else `PROVIDER_VERSION {missing: "isolation", item:
 * "skill:<name>"}`. `forceReload` because the pin caches the list and a workspace skill planted
 * mid-instance raises no `skills/changed`. TODO(7E Task 0 Step 3): whether a later turn of the
 * same instance actually offers such a skill to the model (if never, this is defence in depth).
 */
export function recheckSkills(rpc: Pick<CodexRpc, "request">, cwds: readonly string[]): Effect.Effect<void, FinError> {
  return Effect.flatMap(rpc.request("skills/list", { cwds: [...cwds], forceReload: true }), (skills) => {
    const skill = skills.data.flatMap((e) => e.skills)[0];
    return skill === undefined ? Effect.void : Effect.fail(isolation(`skill:${skill.name}`, `the skill ${label(skill.name)} appeared (${label(skill.path)})`));
  });
}

/** `skills/changed`, decoded or not. */
export const skillsChanged = (n: { readonly method: string; readonly name?: string }): boolean => n.method === "skills/changed" || (n.method === "unknown" && n.name === "skills/changed");
