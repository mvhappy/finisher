// The bundled agent roles (sub-project 7 §6.2; plan 7B Decision 14, role schema v1): data, not
// code, one file per role in `roles/`, named `<name>.json`; plugins contribute more later.
//
//   {name, title, prompt: {text, skills: [topic…]}, modeCeiling, defaultMode,
//    models: {builtin?, claudeCode?, codex?, local?}, rules: [{id, effect, command, where?}…],
//    tools: {allow: [tool…]}, budget, spawn?: {roles: [name…], maxChildren?}}
//
// - **Mode.** A role may narrow its mode, never widen it: `modeCeiling` caps what a starter may
//   ask for (`MODE_INVALID` above it) and `defaultMode` sits at or below it. A role holds no
//   scopes of its own, so it cannot widen what its mode grants.
// - **Rules.** A role's rules are the `role` layer of the rule store. In the file a rule is
//   `{id, effect, command, where?}`; `layer`, `source` and `createdAt` are not the file's to
//   say (a file that names them is `ROLE_INVALID`), the loader fills them: layer `role`,
//   source `role:<name>`, `createdAt` 0 (bundled data has no creation time). A role `allow`
//   never overrides another layer's `ask` or `deny` (evaluation, `rules.ts`), so it cannot widen;
//   a literal `allow` naming a human-only scope is refused here as an obvious authoring mistake
//   (scopes gate on their own). The bundled `default.asset-add` asks on an `asset.add` of a
//   download without a recorded licence (`where.unlicensedDownload`), which 7G resolves; until
//   then the condition never holds, so the rule ships inert (7B Task 2).
// - **Tools.** `tools.allow` is the allow-set of tool names: the built-in agent's (`fin`,
//   `workspace_*`) and the Claude Code vocabulary 7D builds `--tools` from (the CLI enables
//   exactly the named tools it maps, plan 7D Decision 2; a tool a newer CLI adds stays off). A
//   name outside that vocabulary (a scope, say) is refused.
// - **Skills** are `fin guide` topics (7G): slugs the server resolves; the prompt only names them.
// - **Spawn.** `spawn.roles` are the roles a session of this one may start as children
//   (Decision 17); every name must be another file in the directory (itself included).
// - **A bad file** fails the server's start: `ROLE_INVALID {file, field, reason}`.
import { readdirSync, readFileSync } from "node:fs";
import { Mode, type Harness, Rule, RuleCommand, RuleEffect, RuleWhere, type BudgetSpec } from "@finisher/contracts";
import { Schema } from "effect";
import { finError } from "../errors.ts";
import { NEVER_SESSION_SCOPES } from "./credentials.ts";

export interface Role {
  readonly name: string;
  readonly title: string;
  readonly prompt: { readonly text: string; readonly skills: readonly string[] };
  readonly modeCeiling: Mode;
  readonly defaultMode: Mode;
  /** The model a session of this role uses when the starter names none, per harness (and preset). */
  readonly models: { readonly builtin?: string; readonly claudeCode?: string; readonly codex?: string; readonly local?: string };
  /** The role layer of the rule store, `layer`, `source` and `createdAt` filled by the loader. */
  readonly rules: readonly Rule[];
  readonly tools: { readonly allow: readonly string[] };
  /** The role's budget: lower than the server's defaults, never higher. */
  readonly budget: BudgetSpec;
  readonly spawn?: { readonly roles: readonly string[]; readonly maxChildren?: number };
}

/** `plan` < `draft` < `auto`. */
export const MODE_RANK: Readonly<Record<Mode, number>> = { plan: 0, draft: 1, auto: 2 };

/** The tool names a role may allow: the built-in agent's, and Claude Code's (spec §4.5). */
export const KNOWN_TOOLS: ReadonlySet<string> = new Set([
  "fin", "workspace_read", "workspace_write", "workspace_list",
  "Read", "Edit", "Write", "Grep", "Glob", "Bash", "WebFetch", "WebSearch",
]);

const Slug = Schema.String.check(Schema.isPattern(/^[a-z][a-z0-9]*(-[a-z0-9]+)*$/), Schema.isMaxLength(64));
const Text = Schema.String.check(Schema.isMinLength(1));
const Positive = Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(1));
const Model = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(200));

/** A rule as a role file writes it. */
const RoleRule = Schema.Struct({
  id: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(128)),
  effect: RuleEffect,
  command: RuleCommand,
  where: Schema.optionalKey(RuleWhere),
});

const RoleFile = Schema.Struct({
  name: Slug,
  title: Text,
  prompt: Schema.Struct({ text: Text, skills: Schema.Array(Slug) }),
  modeCeiling: Mode,
  defaultMode: Mode,
  models: Schema.Struct({
    builtin: Schema.optionalKey(Model),
    claudeCode: Schema.optionalKey(Model),
    codex: Schema.optionalKey(Model),
    local: Schema.optionalKey(Model),
  }),
  rules: Schema.Array(RoleRule),
  tools: Schema.Struct({ allow: Schema.Array(Schema.String).check(Schema.isMinLength(1)) }),
  budget: Schema.Struct({
    tokens: Schema.optionalKey(Positive),
    moneyMicros: Schema.optionalKey(Positive),
    wallMs: Schema.optionalKey(Positive),
    modelCallsPerRun: Schema.optionalKey(Positive),
    toolCallsPerRun: Schema.optionalKey(Positive),
  }),
  spawn: Schema.optionalKey(Schema.Struct({ roles: Schema.Array(Slug), maxChildren: Schema.optionalKey(Schema.Number.check(Schema.isInt(), Schema.isBetween({ minimum: 1, maximum: 8 }))) })),
});

type Path = ReadonlyArray<PropertyKey>;

/** The first failing path of a schema issue. */
function issuePath(issue: unknown, at: Path = []): Path {
  const i = issue as { _tag?: string; path?: Path; issue?: unknown; issues?: readonly unknown[] };
  if (i._tag === "Pointer") return issuePath(i.issue, [...at, ...(i.path ?? [])]);
  if (i.issues !== undefined && i.issues.length > 0) return issuePath(i.issues[0], at);
  if (i.issue !== undefined) return issuePath(i.issue, at);
  return at;
}

const fieldOf = (p: Path): string => p.reduce<string>((s, k) => (typeof k === "number" ? `${s}[${k}]` : s === "" ? String(k) : `${s}.${String(k)}`), "");

function invalid(file: string, field: string, reason: string): never {
  throw finError("ROLE_INVALID", `role file ${file}: ${field === "" ? "" : `${field}: `}${reason}`, { file, field, reason }, `fix ${file} (role schema v1, agent/roles.ts)`);
}

/** The first line of a schema error, without the path lines. */
const firstLine = (m: string): string => m.split("\n")[0] ?? m;

function decodeRole(raw: unknown, file: string): Role {
  let r: typeof RoleFile.Type;
  try {
    r = Schema.decodeUnknownSync(RoleFile)(raw, { onExcessProperty: "error" });
  } catch (e) {
    const err = e as { issue?: unknown; message?: string };
    return invalid(file, fieldOf(issuePath(err.issue)), firstLine(err.message ?? "invalid"));
  }
  if (file !== `${r.name}.json`) invalid(file, "name", `must match the file name (${r.name}.json)`);
  if (MODE_RANK[r.defaultMode] > MODE_RANK[r.modeCeiling]) invalid(file, "defaultMode", `above the role's modeCeiling (${r.modeCeiling})`);
  r.tools.allow.forEach((t, i) => {
    if (!KNOWN_TOOLS.has(t)) invalid(file, `tools.allow[${i}]`, `${t} is not a tool name (allowed: ${[...KNOWN_TOOLS].join(", ")})`);
  });
  const seen = new Set<string>();
  r.rules.forEach((x, i) => {
    if (seen.has(x.id)) invalid(file, `rules[${i}].id`, `${x.id} is used twice`);
    seen.add(x.id);
    if (x.effect === "allow" && [...NEVER_SESSION_SCOPES].some((s) => x.command === s || x.command.startsWith(`${s}.`))) {
      invalid(file, `rules[${i}].command`, `a role cannot allow ${x.command}: no session may hold it`);
    }
  });
  const rules: Rule[] = r.rules.map((x) => ({ ...x, layer: "role", source: `role:${r.name}`, createdAt: 0 }));
  return { ...r, rules } as Role;
}

/** The roles in `dir` (the bundled `roles/` by default), by name. */
export function loadRoles(dir: URL = new URL("./roles/", import.meta.url)): ReadonlyMap<string, Role> {
  const roles = new Map<string, Role>();
  for (const f of readdirSync(dir).filter((n) => n.endsWith(".json")).sort()) {
    let raw: unknown;
    try {
      raw = JSON.parse(readFileSync(new URL(f, dir), "utf8"));
    } catch (e) {
      invalid(f, "", `not JSON: ${(e as Error).message}`);
    }
    const role = decodeRole(raw, f);
    roles.set(role.name, role);
  }
  for (const role of roles.values()) {
    role.spawn?.roles.forEach((n, i) => {
      if (!roles.has(n)) invalid(`${role.name}.json`, `spawn.roles[${i}]`, `${n} is not a role in this directory`);
    });
  }
  return roles;
}

export const BUNDLED_ROLES: ReadonlyMap<string, Role> = loadRoles();

export function roleUnknown(name: string, roles: ReadonlyMap<string, Role>): never {
  throw finError("ROLE_UNKNOWN", `no agent role named ${name}`, { role: name, roles: [...roles.keys()] }, `use one of: ${[...roles.keys()].join(", ")}`);
}

/** The role's default model for a harness (the `local` preset has its own); none when it names none. */
export function roleModel(role: Role, harness: Harness, preset?: "openrouter" | "local"): string | undefined {
  if (harness === "claudeCode") return role.models.claudeCode;
  if (harness === "codex") return role.models.codex;
  return preset === "local" ? role.models.local : role.models.builtin;
}

/**
 * The effective budget: the role's, where the starter asks for more the role's wins (a role
 * may lower the server's defaults; the starter may set any limit the role leaves open, and
 * lower one the role set).
 */
export function budgetFor(role: Role, asked: BudgetSpec | undefined): BudgetSpec {
  const out: Record<string, number> = { ...(role.budget as Record<string, number>) };
  for (const [k, v] of Object.entries(asked ?? {})) {
    if (typeof v !== "number") continue;
    const cap = out[k];
    out[k] = cap === undefined ? v : Math.min(cap, v);
  }
  return out as BudgetSpec;
}
