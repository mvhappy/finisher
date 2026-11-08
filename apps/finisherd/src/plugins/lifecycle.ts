// The plugin lifecycle (8A Task 9b, spec §5.3; Decisions 19–23, 26): approve and install a
// plan, update, enable, disable, remove, suggest, and what a refresh changes in the installed
// plugins (signed `disable` advisories for anyone's refresh, automatic content updates for a
// person's only).
//
// Every change to the installed plugins runs under one lock, so `control.db`'s plugin rows
// cannot change between an approval's re-check and its write (review M2). An approval:
//   1. reads the plan's row: missing or expired is `PLAN_EXPIRED`; a plan a non-human principal
//      made is `HUMAN_ONLY`, whoever submits it (review M4), before anything is committed;
//   2. re-checks that the installed version and grants are still the plan's base
//      (`PLAN_MISMATCH {reason: installed}`);
//   3. commits through fin-registry, which re-checks the digest, the expiry, the acceptances,
//      the staged tree's hash and that the pack's font ids are still free (the installed rows'
//      directories are passed in);
//   4. in one `control.db` transaction re-checks the base again and writes the row, the grants
//      exactly as planned and the plan's deletion. If that transaction fails, the version just
//      placed in the store is removed again: the store keeps no index, so nothing is installed.
// The Rust commit runs on fin-node's worker thread; holding a SQLite transaction open across
// it would sweep unrelated writers on the connection into it, so the lock stands in for the
// transaction's isolation between steps 2 and 4.
import type { FinError, InstalledRow, PluginInstallRequest, PluginPlanRequest, RefreshApplied } from "@finisher/contracts";
import type { InstallPlan, InstalledPlugin } from "@finisher/core-types";
import { Effect, Semaphore } from "effect";
import { ulid } from "../control/ulid.ts";
import { attempt, attemptJson, finError, toFinError } from "../errors.ts";
import { autoUpdatable, baseOf, canonical, type PlanDeps, precheck, registryFor, stagePlan } from "./plans.ts";
import { actorOf, type Asker } from "./registries.ts";

/** A version a refresh revoked. */
export type Revoked = RefreshApplied["revoked"][number];

/** What an approval did, for the audit log. */
export interface Installed {
  readonly action: InstallPlan["action"];
  readonly plugin: InstalledRow;
  readonly plan: InstallPlan;
  /** Set for third-party code (spec §3.1). */
  readonly thirdPartyCode?: { readonly accepted: boolean; readonly confined: boolean };
}

export interface LifecycleDeps extends PlanDeps {
  /** Refreshes one registry (recording it); fails with the refresh's error. */
  readonly refreshOne: (id: string, by: Asker) => Effect.Effect<void, FinError>;
}

const notInstalled = (id: string) => finError("PLUGIN_NOT_INSTALLED", `${id} is not installed`, { id }, "Check the plugin's id in the list of installed plugins.");

export function makeLifecycle(deps: LifecycleDeps) {
  const { native, tables, clock } = deps;
  const lock = Semaphore.makeUnsafe(1);
  const locked = <A, E>(self: Effect.Effect<A, E>) => lock.withPermits(1)(self);

  /** Revokes the versions of registry `id` a signed `disable` advisory names (any principal's refresh). */
  const revoke = (id: string): Effect.Effect<Revoked[], FinError> =>
    Effect.gen(function* () {
      const rows = tables.installed().filter((p) => p.registryId === id && p.state !== "revoked");
      if (rows.length === 0) return [];
      const hits = yield* attemptJson<Revoked[]>(() => native.revocations(id, JSON.stringify(rows.map((p) => ({ id: p.id, version: p.version })))));
      for (const h of hits) tables.setState(h.id, false, "revoked", clock());
      return hits;
    });

  /** Refreshes `registry` for a plan and applies its advisories (never an automatic update). */
  const refreshForPlan = (registry: string, by: Asker) =>
    Effect.flatMap(deps.refreshOne(registry, by), () => locked(revoke(registry)));

  const plan = (req: PluginPlanRequest, by: Asker): Effect.Effect<{ plan: InstallPlan; revoked: Revoked[] }, FinError> =>
    Effect.gen(function* () {
      const installed = tables.installedFull(req.id);
      const refused = precheck(req, by, installed);
      if (refused !== undefined) return yield* Effect.fail(refused);
      const registry = yield* registryFor(deps, req, installed);
      const revoked = yield* refreshForPlan(registry, by);
      return { plan: yield* stagePlan(deps, req, by, registry), revoked };
    });

  /** Steps 1–4 of an approval (see the file comment); the caller holds the lock. */
  const commit = (req: PluginInstallRequest, by: Asker): Effect.Effect<Installed, FinError> =>
    Effect.gen(function* () {
      const row = tables.plan(req.planId);
      if (row === undefined) return yield* Effect.fail(finError("PLAN_EXPIRED", `no plan ${req.planId}: it expired, was used or was never made`, { reason: "missing" }, "Make a new plan."));
      if (clock() >= row.expiresAt) {
        tables.deletePlan(row.id);
        yield* Effect.ignore(attempt(() => native.discardPlan(row.id)));
        return yield* Effect.fail(finError("PLAN_EXPIRED", `plan ${row.id} expired`, { reason: "expired", expiresAt: row.expiresAt }, "Make a new plan."));
      }
      if (!row.human || !by.human) {
        const who = row.human ? by.kind : row.byKind;
        return yield* Effect.fail(
          finError(
            "HUMAN_ONLY",
            row.human ? `${by.kind} principals cannot install plugins` : `this plan was made by a ${row.byKind} principal; only a plan a person made installs`,
            { tag: "plugin.install", reason: row.human ? "approver" : "planMaker", principal: who, where: "console" },
            "Only you, the owner of this Finisher server, can install plugins, from a plan you made: use the Finisher app or the server's console. If a tool or an agent asked for this, check that you want it before you do it yourself.",
          ),
        );
      }
      if (req.digest !== row.digest) return yield* Effect.fail(finError("PLAN_MISMATCH", "the digest is not this plan's", { reason: "digest" }, "Make a new plan."));
      const changed = () => finError("PLAN_MISMATCH", `${row.pluginId}'s installed version or grants changed since the plan was made`, { reason: "installed" }, "Make a new plan.");
      if (baseOf(tables, row.pluginId) !== row.base) return yield* Effect.fail(changed());
      const staged = yield* attemptJson<InstallPlan>(() => native.stagedPlan(row.id));
      const accept = { copyleft: req.accept?.copyleft === true, thirdPartyCode: req.accept?.thirdPartyCode === true };
      const dirs = tables.installed().flatMap((p) => {
        const full = p.id === row.pluginId ? undefined : tables.installedFull(p.id);
        return full === undefined ? [] : [full.dir];
      });
      const placed = yield* attemptJson<InstalledPlugin>(() => native.commit(row.id, req.digest, JSON.stringify(accept), JSON.stringify({ installedDirs: dirs }))).pipe(
        Effect.tapError((e) => Effect.sync(() => (e.code === "PLAN_EXPIRED" || (e.code === "PLAN_MISMATCH" && (e.details as { reason?: unknown } | undefined)?.reason !== "digest") ? tables.deletePlan(row.id) : undefined))),
      );
      const prev = tables.installedFull(row.pluginId);
      const now = clock();
      const revokedNow = staged.warnings?.some((w) => w.code === "advisory" && w.details?.action === "disable") === true;
      const content = placed.kind === "content";
      const keepOff = content && prev !== undefined && prev.state === "disabled";
      const enabled = content && !keepOff && !revokedNow;
      const state: InstalledRow["state"] = revokedNow ? "revoked" : enabled ? "active" : "disabled";
      const autoUpdate = content && (req.autoUpdate ?? prev?.autoUpdate ?? false);
      const fields = {
        id: placed.id,
        version: placed.version,
        source: placed.source,
        ...(placed.registryId == null ? {} : { registryId: placed.registryId }),
        ...(placed.targetSha256 == null ? {} : { targetSha256: placed.targetSha256 }),
        treeHash: placed.treeHash,
        dir: placed.dir,
        kind: placed.kind,
        enabled,
        state,
        autoUpdate,
      };
      const grants = (placed.permissions ?? []).map(canonical);
      yield* Effect.try({
        try: () =>
          tables.store.tx(() => {
            if (baseOf(tables, row.pluginId) !== row.base) throw changed();
            if (prev === undefined) tables.addPlugin({ ...fields, installedBy: actorOf(by), installedAt: now });
            else tables.replacePlugin(fields, now);
            tables.setGrants(placed.id, placed.version, grants, actorOf(by), now);
            tables.deletePlan(row.id);
          }),
        catch: toFinError,
      }).pipe(Effect.tapError(() => Effect.ignore(attempt(() => native.removePlugin(placed.id, placed.version, false)))));
      // The version it replaced stays until the switch is recorded; its data stays too.
      if (prev !== undefined && prev.dir !== placed.dir) yield* Effect.ignore(attempt(() => native.removePlugin(prev.id, prev.version, false)));
      const third = staged.warnings?.find((w) => w.code === "thirdPartyCode");
      return {
        action: staged.action,
        plugin: tables.installedOne(placed.id)!,
        plan: staged,
        ...(third === undefined ? {} : { thirdPartyCode: { accepted: accept.thirdPartyCode, confined: third.details?.confined === true } }),
      };
    });

  /** A person's refresh: content packs with automatic updates on take a newer version that widens nothing. */
  const autoUpdate = (id: string, by: Asker): Effect.Effect<Installed[], never> =>
    Effect.gen(function* () {
      const out: Installed[] = [];
      if (!by.human) return out;
      const rows = tables.installed().filter((p) => p.registryId === id && p.source === "registry" && p.kind === "content" && p.autoUpdate && p.state !== "revoked");
      for (const p of rows) {
        const staged = yield* Effect.option(stagePlan(deps, { id: p.id, update: true, registry: id }, by, id));
        if (staged._tag === "None") continue;
        const plan = staged.value;
        if (!autoUpdatable(plan)) {
          tables.deletePlan(plan.planId);
          yield* Effect.ignore(attempt(() => native.discardPlan(plan.planId)));
          continue;
        }
        const done = yield* Effect.option(commit({ planId: plan.planId, digest: plan.digest }, by));
        if (done._tag === "Some") out.push(done.value);
      }
      return out;
    });

  const changeState = (id: string, enable: boolean): Effect.Effect<InstalledRow, FinError> =>
    Effect.gen(function* () {
      const row = tables.installedOne(id);
      if (row === undefined) return yield* Effect.fail(notInstalled(id));
      if (enable) {
        // No runtime exists before 8B; there is no sandbox gate (owner decision 1).
        if (row.kind === "code") {
          return yield* Effect.fail(
            finError("PLUGIN_RUNTIME_UNAVAILABLE", `${id} is a code plugin, and this version of Finisher cannot run code plugins yet`, { id, kind: "code" }, "Keep it installed: a later Finisher update can run it."),
          );
        }
        if (row.state === "revoked") {
          return yield* Effect.fail(finError("PLUGIN_REVOKED", `${id} ${row.version} was revoked by a signed advisory`, { id, version: row.version }, "Install a version the advisory does not name."));
        }
        tables.setState(id, true, "active", clock());
      } else if (row.state !== "revoked") {
        tables.setState(id, false, "disabled", clock());
      }
      return tables.installedOne(id)!;
    });

  const remove = (id: string, purgeData: boolean): Effect.Effect<InstalledRow, FinError> =>
    Effect.gen(function* () {
      const row = tables.installedOne(id);
      if (row === undefined) return yield* Effect.fail(notInstalled(id));
      if (row.source === "bundled") return yield* Effect.fail(finError("PLUGIN_BUNDLED", `${id} comes with Finisher and cannot be removed`, { id }, "Disable it instead."));
      // A row whose store directory is already gone (a removal interrupted after it) is removed too.
      yield* attempt(() => native.removePlugin(id, row.version, purgeData)).pipe(Effect.catchIf((e) => e.code === "PLUGIN_NOT_INSTALLED", () => Effect.void));
      yield* Effect.try({ try: () => tables.store.tx(() => tables.deletePlugin(id)), catch: toFinError });
      return row;
    });

  return {
    plan,
    install: (req: PluginInstallRequest, by: Asker) => locked(commit(req, by)),
    remove: (id: string, purgeData: boolean) => locked(remove(id, purgeData)),
    enable: (id: string) => locked(changeState(id, true)),
    disable: (id: string) => locked(changeState(id, false)),
    /** After a successful refresh of `id`: revocations for anyone, automatic updates for a person. */
    afterRefresh: (id: string, by: Asker): Effect.Effect<{ revoked: Revoked[]; updated: Installed[] }, FinError> =>
      locked(Effect.gen(function* () {
        const revoked = yield* revoke(id);
        return { revoked, updated: yield* autoUpdate(id, by) };
      })),
    suggest: (req: { readonly id: string; readonly reason: string; readonly projectId: string }, by: Asker): Effect.Effect<{ suggestionId: string }, FinError> =>
      Effect.try({
        try: () => {
          const now = clock();
          const suggestionId = ulid(now);
          tables.addSuggestion({ id: suggestionId, pluginId: req.id, projectId: req.projectId, reason: req.reason, byKind: by.kind, byId: by.id, createdAt: now });
          return { suggestionId };
        },
        catch: toFinError,
      }),
    /** Deletes expired plans' rows and fin-registry's expired staging; returns how many. */
    sweep: (): Effect.Effect<number, FinError> =>
      Effect.map(attempt(() => native.sweep()), (n) => n + tables.deleteExpiredPlans(clock())),
  };
}

export type Lifecycle = ReturnType<typeof makeLifecycle>;
