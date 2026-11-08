// Install plans (8A Task 9b, spec §5.3 step 1, Decision 18): which registry a plan comes from,
// the refresh before it, fin-registry's plan, and the `plugin_plans` row that records who made
// it (review M4) and the installed state it was made against (review M2).
//
// Nothing a package ships runs here: fin-registry fetches the target with its signed length as
// the cap, checks its signed SHA-256, unpacks it strictly and validates it into staging.
import type { FinError, PluginPlanRequest } from "@finisher/contracts";
import type { InstallPlan, InstalledPlugin, Permission, PluginInfo } from "@finisher/core-types";
import type { Registry } from "@finisher/fin-node";
import { Effect } from "effect";
import { attempt, attemptJson, finError } from "../errors.ts";
import type { Asker } from "./registries.ts";
import type { InstalledFull, PluginTables } from "./tables.ts";

/** JSON with object keys sorted at every level and no whitespace (fin-registry's `canonical_json`). */
export function canonical(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  if (v !== null && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .filter((k) => o[k] !== undefined)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}

/**
 * The installed state a plan is made against, as `plugin_plans.base` stores it: the version and
 * its grants (sorted), or `null` when nothing is installed. A commit installs only while the
 * state is still this (review M2).
 */
export function baseOf(tables: PluginTables, id: string): string | null {
  const row = tables.installedOne(id);
  if (row === undefined) return null;
  return canonical({ version: row.version, grants: [...tables.grants(id)].sort() });
}

/** fin-registry's `InstalledPlugin` for a plan's `installed`: the row and its grants. */
export function installedPlugin(row: InstalledFull, grants: ReadonlyArray<string>): InstalledPlugin {
  return {
    id: row.id,
    version: row.version,
    kind: row.kind,
    source: row.source,
    ...(row.registryId === undefined ? {} : { registryId: row.registryId }),
    permissions: grants.map((g) => JSON.parse(g) as Permission),
    treeHash: row.treeHash,
    ...(row.targetSha256 === undefined ? {} : { targetSha256: row.targetSha256 }),
    dir: row.dir,
  };
}

export interface PlanDeps {
  readonly native: Registry;
  readonly tables: PluginTables;
  readonly clock: () => number;
  /** Test builds only: plans are marked agent-facing (8A Decision 23's test-only kind flag). */
  readonly testAgentFacing?: (id: string) => boolean;
}

/** How many plans one principal may have open at once (review M2). */
export const MAX_OPEN_PLANS = 4;

/**
 * Whether the plan's registry is not where the installed version came from (review M1): another
 * registry id, or the same id under a root the version was not verified under.
 */
export function registryChanged(deps: PlanDeps, installed: InstalledFull | undefined, registry: string): boolean {
  if (installed === undefined || installed.source !== "registry") return false;
  return installed.registryId !== registry || !deps.tables.rootMatches(installed);
}

/**
 * The registry a plan for `req` comes from (see `PluginPlanPayload`). An update follows the
 * registry the installed version came from only while that registry still has the root the
 * version was verified under; after a registry is removed and another folder added under its
 * id, the person names the registry (review M1).
 */
export function registryFor(deps: PlanDeps, req: PluginPlanRequest, installed: InstalledFull | undefined): Effect.Effect<string, FinError> {
  return Effect.gen(function* () {
    if (req.registry !== undefined) return req.registry;
    if (installed?.registryId !== undefined) {
      const now = deps.tables.rootOf(installed.registryId);
      if (now !== undefined && now !== installed.rootSha256) {
        return yield* Effect.fail(
          finError(
            "REGISTRY_AMBIGUOUS",
            `registry ${installed.registryId} is not the registry ${req.id} was installed from: it has another root`,
            { id: req.id, reason: "rootChanged", registry: installed.registryId },
            "Name the registry to take the update from.",
          ),
        );
      }
      return installed.registryId;
    }
    const listed = (yield* attemptJson<PluginInfo[]>(() => deps.native.info(req.id))).map((i) => i.registryId);
    if (listed.length === 1) return listed[0]!;
    if (listed.length === 0) {
      return yield* Effect.fail(finError("PLUGIN_NOT_FOUND", `no registry lists ${req.id}`, { id: req.id }, "Check the plugin's id, or add and refresh the registry that offers it."));
    }
    return yield* Effect.fail(
      finError("REGISTRY_AMBIGUOUS", `${listed.length} registries list ${req.id}`, { id: req.id, registries: listed }, "Name the registry to install it from."),
    );
  });
}

/** Refusals before any plan is made: downgrades are a person's, bundled plugins update with the app. */
export function precheck(req: PluginPlanRequest, by: Asker, installed: InstalledFull | undefined): FinError | undefined {
  if (req.allowDowngrade === true && !by.human) {
    return finError("HUMAN_ONLY", "only a person may plan a downgrade", { tag: "plugin.plan", reason: "downgrade", principal: by.kind, where: "console" }, "Only you, the owner of this Finisher server, can downgrade a plugin: use the Finisher app or the server's console.");
  }
  if (req.allowDowngrade === true && req.version === undefined) {
    return finError("PLUGIN_DOWNGRADE_REFUSED", `a downgrade of ${req.id} needs the exact version`, { id: req.id, reason: "noVersion" }, "Name the version to go back to.");
  }
  if (req.update === true && installed === undefined) {
    return finError("PLUGIN_NOT_INSTALLED", `${req.id} is not installed`, { id: req.id }, "Install it first.");
  }
  if (installed?.source === "bundled") {
    return finError("PLUGIN_BUNDLED", `${req.id} comes with Finisher and updates with it`, { id: req.id }, "Update Finisher to update it.");
  }
  return undefined;
}

/** `by` may not have another plan open (review M2). */
const tooMany = (by: Asker) =>
  finError("TOO_MANY_PLANS", `${by.kind} principals may have ${MAX_OPEN_PLANS} plans open at once`, { limit: MAX_OPEN_PLANS }, "Wait for your plans to expire (10 minutes) or install or discard one.");

/**
 * fin-registry's plan for `req` from `registry` (already refreshed), recorded with its maker and
 * base. Every other installed or listed id is a look-alike candidate.
 *
 * A principal has at most `MAX_OPEN_PLANS` plans open (`internal` plans, an automatic update's
 * own, are not counted). A plan a non-human principal makes is returned for information only:
 * its row is stored as unusable and its staged files are deleted at once, so an agent cannot
 * fill the disk with staged packages nobody can install (review M2).
 */
export function stagePlan(deps: PlanDeps, req: PluginPlanRequest, by: Asker, registry: string, internal = false): Effect.Effect<InstallPlan, FinError> {
  return Effect.gen(function* () {
    const { native, tables } = deps;
    if (!internal && tables.openPlans(by, deps.clock()) >= MAX_OPEN_PLANS) return yield* Effect.fail(tooMany(by));
    const installed = tables.installedFull(req.id);
    const base = baseOf(tables, req.id);
    const listed = yield* attemptJson<Array<{ id: string }>>(() => native.search("{}"));
    const known = [...new Set([...tables.installed().map((p) => p.id), ...listed.map((l) => l.id)])].filter((id) => id !== req.id).sort();
    const body = {
      id: req.id,
      source: { kind: "registry", registry, ...(req.version === undefined ? {} : { version: req.version }) },
      update: req.update === true,
      allowDowngrade: req.allowDowngrade === true,
      ...(installed === undefined ? {} : { installed: installedPlugin(installed, tables.grants(req.id)) }),
      knownIds: known,
      ...(registryChanged(deps, installed, registry) ? { registryChanged: true } : {}),
      ...(deps.testAgentFacing?.(req.id) === true ? { testAgentFacing: true } : {}),
    };
    const plan = yield* attemptJson<InstallPlan>(() => native.plan(JSON.stringify(body)));
    yield* Effect.try({
      try: () =>
        tables.addPlan({
          id: plan.planId,
          digest: plan.digest,
          pluginId: plan.plugin.id,
          version: plan.plugin.version,
          source: plan.source.kind,
          ...(plan.source.registryId === undefined || plan.source.registryId === null ? {} : { registryId: plan.source.registryId }),
          action: plan.action,
          byKind: by.kind,
          byId: by.id,
          human: by.human,
          base,
          ...(tables.rootOf(registry) === undefined ? {} : { rootSha256: tables.rootOf(registry)! }),
          unusable: !by.human,
          createdAt: deps.clock(),
          expiresAt: plan.expiresAt,
        }),
      catch: (e) => finError("INTERNAL", `the plan could not be recorded: ${String(e)}`),
    }).pipe(Effect.tapError(() => Effect.ignore(attempt(() => native.discardPlan(plan.planId)))));
    // Plans race each other (making one takes seconds, outside the lock): the cap holds after the fact.
    if (!by.human || (!internal && tables.openPlans(by, deps.clock()) > MAX_OPEN_PLANS)) {
      if (by.human) tables.deletePlan(plan.planId);
      yield* Effect.ignore(attempt(() => native.discardPlan(plan.planId)));
      if (by.human) return yield* Effect.fail(tooMany(by));
    }
    return plan;
  });
}

/** Whether a plan may install without a person approving it (Decision 23, spec §5.3). */
export function autoUpdatable(plan: InstallPlan): boolean {
  const d = plan.diff;
  const widens = d.added.length > 0 || d.widened.length > 0 || d.riskUp;
  const warned = plan.warnings?.some((w) => w.code === "thirdPartyCode" || w.code === "yanked" || w.code === "registryChanged" || (w.code === "advisory" && w.details?.action === "disable")) ?? false;
  return plan.action === "update" && plan.plugin.kind === "content" && plan.agentFacing !== true && !widens && !warned && plan.licence.class === "permissive";
}
