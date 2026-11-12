// `agent.session.start`'s step 1 (plan 7A Decision 15.1; sub-project 7 §4.4, §4.7, §6.1, §6.2):
// what the session will be, before anything is made.
//
// - **Role** (`editor` by default; `ROLE_UNKNOWN`) and **mode** (the role's default, at most its
//   ceiling; `MODE_INVALID`).
// - **Harness** (`builtin` by default): one registered on this server (`HARNESS_UNKNOWN`; the
//   `fake` harness exists only in test builds with `FIN_TEST_HOOKS=1`).
// - **Preset and model** for the built-in agent: `openrouter` by default, the model an
//   OpenRouter id (`<maker>/<model>`, `MODEL_UNKNOWN` otherwise; `{reason: noTools}` for a
//   listed model without tool calls) with the project's key or the server's default
//   (`PROVIDER_AUTH`); `local` needs the owner's endpoint (`PROVIDER_UNAVAILABLE`) and a model
//   the server lists, a known open-weight family or one the owner describes (`MODEL_UNKNOWN`).
//   The price is the runtime's check (`PRICING_UNKNOWN`). OpenRouter's listing is refreshed
//   first when it is a day old (best effort: the cache stands when the network does not).
// - **Model default**: the role's per harness (`roleModel`); the CLI harnesses (7D, 7E) read the
//   same field, and the CLI lists its own models, so a bundled role names none for them.
// - **Tools**: the role's allow-set rides on the plan (7D builds `--disallowedTools` from it).
// - **Budget**: the role's, which the starter may lower or fill in.
// - **A child** (7B Decision 17, `parent` given): its role must be in the parent role's
//   `spawn.roles` (`ROLE_UNKNOWN` otherwise, also with no role named); its mode at most the
//   parent's and the role's ceiling (`MODE_INVALID`; the default is the role's, lowered to the
//   parent's); its harness and preset the parent's unless given. Its budget here is the role's,
//   lowered by the request; the runtime carves it from the parent's (Task 7, `carve.ts`).
import type { Harness, Mode, Principal, Session, Weights } from "@finisher/contracts";
import { finError } from "../errors.ts";
import type { Catalogue } from "./catalogue/catalogue.ts";
import { OPENROUTER_ID } from "./catalogue/prices.ts";
import { type ClaudeAccess, claudeModelOffered } from "./claude/models.ts";
import { budgetFor, MODE_RANK, type Role, roleModel, roleUnknown } from "./roles.ts";
import { harnessUnknown, type SessionPlan, type SessionPlanner, type StartRequest } from "./runtime-deps.ts";
import type { LocalEndpoint } from "./settings.ts";

export interface PlannerDeps {
  readonly roles: ReadonlyMap<string, Role>;
  readonly catalogue: Pick<Catalogue, "describe">;
  /** The harnesses this server runs. */
  readonly harnesses: () => ReadonlySet<Harness>;
  /** Whether an OpenRouter key would be found for the project (its own, else the server's). */
  readonly hasKey: (projectId: string) => boolean;
  readonly local: () => LocalEndpoint | undefined;
  /** The models the local server lists; rejects when it does not answer. */
  readonly localModels: (endpoint: LocalEndpoint) => Promise<readonly string[]>;
  /** Refreshes OpenRouter's listing when it is stale; never rejects. */
  readonly refreshListing?: () => Promise<void>;
  /**
   * Claude Code's discovered models (`claude/models.ts`), or `undefined` when the harness is not
   * usable here (not installed, or `agents.claudeCode.enabled` off): then `HARNESS_UNKNOWN`.
   * Below the floor: `PROVIDER_VERSION`.
   */
  readonly claude?: () => Promise<ClaudeAccess>;
}

const LOCAL_ID = /^[^\s\u0000-\u001f/\\][^\s\u0000-\u001f]{0,199}$/;

function modelUnknown(model: string | undefined, reason: string, fix: string): never {
  throw finError("MODEL_UNKNOWN", model === undefined ? "no model was named and the role has no default" : `${model} is not a model this session can use`, { ...(model === undefined ? {} : { model }), reason }, fix);
}

export function sessionPlanner(deps: PlannerDeps): SessionPlanner {
  return {
    async plan(req: StartRequest, _by: Principal, parent?: Session): Promise<SessionPlan> {
      const roleName = req.role ?? (parent === undefined ? "editor" : "");
      if (parent !== undefined) childRole(deps.roles, parent, roleName);
      const role = deps.roles.get(roleName) ?? roleUnknown(roleName, deps.roles);
      const mode = req.mode ?? (parent === undefined ? role.defaultMode : lower(role.defaultMode, parent.mode));
      if (parent !== undefined && MODE_RANK[mode] > MODE_RANK[parent.mode]) {
        throw finError("MODE_INVALID", `a child works in its parent's ${parent.mode} mode at most`, { mode, parentMode: parent.mode, role: role.name }, `spawn it in ${parent.mode} mode or below`);
      }
      if (MODE_RANK[mode] > MODE_RANK[role.modeCeiling]) {
        throw finError("MODE_INVALID", `the ${role.name} role works in ${role.modeCeiling} mode at most`, { mode, ceiling: role.modeCeiling, role: role.name }, `start it in ${role.modeCeiling} mode or below`);
      }
      const harness = req.harness ?? parent?.harness ?? "builtin";
      if (!deps.harnesses().has(harness)) harnessUnknown(harness);
      const budget = budgetFor(role, req.budget);
      const base = { harness, role: role.name, mode, budget, tools: role.tools.allow };
      if (harness === "fake") {
        // Tests only: no model, no key, priced as local (nothing is charged).
        return { ...base, provider: "local", model: req.model ?? "fake", weights: "unknown" };
      }
      if (harness === "claudeCode") return { ...base, provider: "anthropic", ...(await claudeModel(deps, req.model ?? roleModel(role, harness))) };
      if (harness !== "builtin") harnessUnknown(harness);
      const preset = req.provider ?? (parent?.provider === "local" ? "local" : "openrouter");
      if (preset === "local") return { ...base, provider: "local", ...(await localModel(deps, req.model ?? roleModel(role, harness, "local"))) };
      const model = req.model ?? roleModel(role, harness, "openrouter");
      if (model === undefined || !OPENROUTER_ID.test(model)) {
        modelUnknown(model, model === undefined ? "noDefault" : "notOpenrouterId", "name an OpenRouter model id such as anthropic/claude-sonnet-5.5, or use --provider local");
      }
      if (!deps.hasKey(req.projectId)) {
        throw finError("PROVIDER_AUTH", "no OpenRouter key is set for this project or the server", { provider: "openrouter" }, "a person sets one with `finisherd provider key set openrouter` (the key on stdin)");
      }
      await deps.refreshListing?.();
      const info = deps.catalogue.describe("openrouter", model);
      if ((info.source === "openrouter" || info.source === "settings") && !info.capabilities.tools) {
        modelUnknown(model, "noTools", "pick a model that supports tool calls");
      }
      return { ...base, provider: "openrouter", model, weights: info.weights as Weights };
    },
  };
}

const lower = (a: Mode, b: Mode): Mode => (MODE_RANK[a] <= MODE_RANK[b] ? a : b);

/** `ROLE_UNKNOWN` unless `parent`'s role may spawn `name` (7B Decision 17). */
function childRole(roles: ReadonlyMap<string, Role>, parent: Session, name: string): void {
  const allowed = roles.get(parent.role)?.spawn?.roles ?? [];
  if (allowed.includes(name) && roles.has(name)) return;
  throw finError(
    "ROLE_UNKNOWN",
    name === "" ? "a child needs a role" : `a ${parent.role} session may not spawn a ${name} child`,
    { role: name, parentRole: parent.role, roles: [...allowed] },
    allowed.length === 0 ? `a ${parent.role} session spawns no children` : `spawn one of: ${allowed.join(", ")}`,
  );
}

/** Claude Code's model: a discovered `value` (`default`, an alias) or a `resolvedModel`; its weights are closed. */
async function claudeModel(deps: PlannerDeps, model: string | undefined): Promise<{ model: string; weights: Weights }> {
  const access = await deps.claude?.();
  if (access === undefined) harnessUnknown("claudeCode");
  if ("tooOld" in access) {
    const { version, floor } = access.tooOld;
    throw finError("PROVIDER_VERSION", `Claude Code${version === undefined ? "" : ` ${version}`} is older than ${floor}, the oldest this Finisher supports`, { harness: "claudeCode", ...(version === undefined ? {} : { version }), floor }, `update Claude Code to ${floor} or later`);
  }
  const { models } = access;
  if (model === undefined || !claudeModelOffered(models, model)) {
    throw finError(
      "MODEL_UNKNOWN",
      model === undefined ? "no model was named and the role has no default" : `${model} is not a model the installed Claude Code offers`,
      { harness: "claudeCode", ...(model === undefined ? {} : { model }), reason: model === undefined ? "noDefault" : "notOffered" },
      `use one of: ${["default", ...models.map((m) => m.id)].filter((x, i, a) => a.indexOf(x) === i).join(", ")}`,
    );
  }
  return { model, weights: "closed" };
}

async function localModel(deps: PlannerDeps, model: string | undefined): Promise<{ model: string; weights: Weights }> {
  const endpoint = deps.local();
  if (endpoint === undefined) {
    throw finError("PROVIDER_UNAVAILABLE", "no local model endpoint is set", { provider: "local", reason: "notConfigured" }, "a person sets one with `finisherd provider local set <ollama|lmstudio|url>`");
  }
  if (model === undefined || !LOCAL_ID.test(model)) modelUnknown(model, model === undefined ? "noDefault" : "badName", "name the model with --model (`finisherd provider local test` lists the server's)");
  const info = deps.catalogue.describe("local", model);
  // A local model is never closed: a known family is open, anything else unknown.
  const weights: Weights = info.weights === "open" ? "open" : "unknown";
  if (info.source === "settings" || info.source === "family") return { model, weights };
  let listed: readonly string[];
  try {
    listed = await deps.localModels(endpoint);
  } catch {
    throw finError("PROVIDER_UNAVAILABLE", "the local model server does not answer", { provider: "local", reason: "unreachable" }, "start the model server, then try again (`finisherd provider local test`)");
  }
  if (!listed.includes(model)) modelUnknown(model, "notListed", "name a model the local server lists (`finisherd provider local test`), or describe it in agents.models");
  return { model, weights };
}
