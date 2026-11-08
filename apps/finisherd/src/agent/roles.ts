// The bundled agent roles (sub-project 7 §6.2): data, not code, one file per role in `roles/`
// (`{name, title, prompt, modeCeiling, defaultMode, models: {builtin?, local?}, budget}`); plugins
// contribute more later. A role may narrow its mode, never widen it: `modeCeiling` caps what a
// starter may ask for (`MODE_INVALID` above it). The prompt is the role's whole system prompt
// before the `fin` map, identical across its sessions (Decision 13).
import { readdirSync, readFileSync } from "node:fs";
import type { BudgetSpec, Mode } from "@finisher/contracts";
import { finError } from "../errors.ts";

export interface Role {
  readonly name: string;
  readonly title: string;
  readonly prompt: string;
  readonly modeCeiling: Mode;
  readonly defaultMode: Mode;
  /** The model a session of this role uses when the starter names none, per preset. */
  readonly models: { readonly builtin?: string; readonly local?: string };
  /** The role's budget: lower than the server's defaults, never higher. */
  readonly budget: BudgetSpec;
}

/** `plan` < `draft` < `auto`. */
export const MODE_RANK: Readonly<Record<Mode, number>> = { plan: 0, draft: 1, auto: 2 };

const MODES = new Set(Object.keys(MODE_RANK));
const BUDGET_KEYS = new Set(["tokens", "moneyMicros", "wallMs", "modelCallsPerRun", "toolCallsPerRun"]);

function checkRole(raw: unknown, file: string): Role {
  const r = raw as Record<string, unknown> | null;
  const bad = (what: string) => new Error(`role file ${file}: ${what}`);
  if (r === null || typeof r !== "object") throw bad("not an object");
  if (typeof r.name !== "string" || !/^[a-z][a-z-]{0,63}$/.test(r.name)) throw bad("name");
  if (typeof r.title !== "string" || typeof r.prompt !== "string" || r.prompt === "") throw bad("title and prompt");
  if (!MODES.has(r.modeCeiling as string) || !MODES.has(r.defaultMode as string)) throw bad("modeCeiling and defaultMode");
  if (MODE_RANK[r.defaultMode as Mode] > MODE_RANK[r.modeCeiling as Mode]) throw bad("defaultMode above modeCeiling");
  const models = (r.models ?? {}) as Record<string, unknown>;
  for (const k of Object.keys(models)) if ((k !== "builtin" && k !== "local") || typeof models[k] !== "string") throw bad(`models.${k}`);
  const budget = (r.budget ?? {}) as Record<string, unknown>;
  for (const [k, v] of Object.entries(budget)) if (!BUDGET_KEYS.has(k) || !Number.isSafeInteger(v) || (v as number) <= 0) throw bad(`budget.${k}`);
  return r as unknown as Role;
}

/** The roles in `dir` (the bundled `roles/` by default), by name. */
export function loadRoles(dir: URL = new URL("./roles/", import.meta.url)): ReadonlyMap<string, Role> {
  const roles = new Map<string, Role>();
  for (const f of readdirSync(dir).filter((n) => n.endsWith(".json")).sort()) {
    const role = checkRole(JSON.parse(readFileSync(new URL(f, dir), "utf8")), f);
    if (roles.has(role.name)) throw new Error(`role ${role.name} is defined twice`);
    roles.set(role.name, role);
  }
  return roles;
}

export const BUNDLED_ROLES: ReadonlyMap<string, Role> = loadRoles();

export function roleUnknown(name: string, roles: ReadonlyMap<string, Role>): never {
  throw finError("ROLE_UNKNOWN", `no agent role named ${name}`, { role: name, roles: [...roles.keys()] }, `use one of: ${[...roles.keys()].join(", ")}`);
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
