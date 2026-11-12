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
import { autoUpdatable, baseOf, canonical, installedPlugin, type PlanDeps, precheck, registryFor, stagePlan } from "./plans.ts";
import { actorOf, type Asker } from "./registries.ts";
import type { PlanRow } from "./tables.ts";
import type { PluginHostHooks } from "./host/processes.ts";
import type { PluginSuggestions } from "../reviews/suggestions.ts";

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
  /**
   * Runs after every change to the installed plugins, under the same lock, whether it
   * succeeded or not (so it reads what really is installed): the content consumers follow
   * the rows (Task 12a).
   */
  readonly changed?: Effect.Effect<void>;
  /**
   * The code plugins' processes (sp8b Task 5): stopped on disable, revocation, removal and an
   * update; asked whether a code plugin can be enabled here. Without it (8A's tests), code
   * plugins cannot be enabled (`PLUGIN_RUNTIME_UNAVAILABLE`).
   */
  readonly host?: PluginHostHooks;
  /**
   * Where suggestions live (5E Decision 11): review items. Without it (8A's tests) they are rows
   * of `plugin_suggestions`.
   */
  readonly suggestions?: PluginSuggestions;
}

/** How many suggestions one principal may have for one project (review M2). */
export const MAX_SUGGESTIONS = 20;

/** Who installs the plugins that come with Finisher. */
export const BUNDLED_BY: Asker = { kind: "system", id: "bundled", human: false };

const notInstalled = (id: string) => finError("PLUGIN_NOT_INSTALLED", `${id} is not installed`, { id }, "Check the plugin's id in the list of installed plugins.");

export function makeLifecycle(deps: LifecycleDeps) {
  const { native, tables, clock } = deps;
  const lock = Semaphore.makeUnsafe(1);
  const locked = <A, E>(self: Effect.Effect<A, E>) => lock.withPermits(1)(deps.changed === undefined ? self : Effect.ensuring(self, deps.changed));
  /** A code plugin's processes stop (none run for content packs). */
  const stopCode = (id: string, opts?: Parameters<PluginHostHooks["stop"]>[1]) => (deps.host === undefined ? Effect.void : deps.host.stop(id, opts));

  /**
   * Revokes the versions of registry `id` a signed `disable` advisory names (any principal's
   * refresh). Only rows verified under the root `id` has now: another root's advisories say
   * nothing about a package that root never signed (review M1).
   */
  const revoke = (id: string): Effect.Effect<Revoked[], FinError> =>
    Effect.gen(function* () {
      const rows = tables.installed().filter((p) => p.registryId === id && p.state !== "revoked" && tables.installedRootMatches(p.id));
      if (rows.length === 0) return [];
      const hits = yield* attemptJson<Revoked[]>(() => native.revocations(id, JSON.stringify(rows.map((p) => ({ id: p.id, version: p.version })))));
      for (const h of hits) {
        tables.setState(h.id, false, "revoked", clock());
        yield* stopCode(h.id, { fail: finError("PLUGIN_REVOKED", `${h.id} is revoked`, { plugin: h.id }) });
      }
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
      if (row.registryId !== undefined && tables.rootOf(row.registryId) !== row.rootSha256) {
        return yield* Effect.fail(finError("PLAN_MISMATCH", `registry ${row.registryId} is not the registry the plan was made from`, { reason: "registry" }, "Make a new plan."));
      }
      const accept = { copyleft: req.accept?.copyleft === true, thirdPartyCode: req.accept?.thirdPartyCode === true };
      return yield* place(row, accept, req.autoUpdate, by);
    });

  /**
   * Steps 2–4 of an approval for a plan row that passed the checks of its maker: re-checks the
   * installed state, places the version in the store and writes the row and grants in one
   * transaction. Bundled installs at start come here too (Task 10), with the same lock and
   * the same rows; the caller holds the lock.
   */
  const place = (row: PlanRow, accept: { copyleft: boolean; thirdPartyCode: boolean }, autoUpdateReq: boolean | undefined, by: Asker): Effect.Effect<Installed, FinError> =>
    Effect.gen(function* () {
      const changed = () => finError("PLAN_MISMATCH", `${row.pluginId}'s installed version or grants changed since the plan was made`, { reason: "installed" }, "Make a new plan.");
      if (baseOf(tables, row.pluginId) !== row.base) return yield* Effect.fail(changed());
      const staged = yield* attemptJson<InstallPlan>(() => native.stagedPlan(row.id));
      const dirs = tables.installed().flatMap((p) => {
        const full = p.id === row.pluginId ? undefined : tables.installedFull(p.id);
        return full === undefined ? [] : [full.dir];
      });
      const placed = yield* attemptJson<InstalledPlugin>(() => native.commit(row.id, row.digest, JSON.stringify(accept), JSON.stringify({ installedDirs: dirs }))).pipe(
        Effect.tapError((e) => Effect.sync(() => (e.code === "PLAN_EXPIRED" || (e.code === "PLAN_MISMATCH" && (e.details as { reason?: unknown } | undefined)?.reason !== "digest") ? tables.deletePlan(row.id) : undefined))),
      );
      const prev = tables.installedFull(row.pluginId);
      const now = clock();
      // A `disable` advisory that landed after the plan was made still stops this install from
      // enabling anything: the trusted metadata is read again here, under the lock (review M3).
      const advised = placed.registryId == null ? [] : yield* attemptJson<Revoked[]>(() => native.revocations(placed.registryId!, JSON.stringify([{ id: placed.id, version: placed.version }])));
      const revokedNow = advised.length > 0 || staged.warnings?.some((w) => w.code === "advisory" && w.details?.action === "disable") === true;
      const content = placed.kind === "content";
      // The owner's switch survives an update: a pack they turned off stays off, also a bundled
      // one that was revoked meanwhile (its `enabled` is still their choice, review M2).
      const keepOff = content && prev !== undefined && (prev.state === "disabled" || (prev.state === "revoked" && prev.source === "bundled" && !prev.enabled));
      // A code plugin installs disabled (8A); an approved update of one keeps what it had: an
      // enabled one stays enabled (the person approved the new version's plan), one in a crash
      // loop stays there until its owner enables it (sp8b Task 5).
      const codeOn = !content && prev !== undefined && prev.kind === "code" && prev.enabled && prev.state !== "revoked" && !revokedNow;
      const enabled = (content && !keepOff && !revokedNow) || codeOn;
      const state: InstalledRow["state"] = revokedNow ? "revoked" : codeOn && prev?.state === "crashLoop" ? "crashLoop" : enabled ? "active" : "disabled";
      const autoUpdate = content && (autoUpdateReq ?? prev?.autoUpdate ?? false);
      const fields = {
        id: placed.id,
        version: placed.version,
        source: placed.source,
        ...(placed.registryId == null ? {} : { registryId: placed.registryId }),
        ...(placed.targetSha256 == null ? {} : { targetSha256: placed.targetSha256 }),
        ...(row.rootSha256 === undefined || placed.registryId == null ? {} : { rootSha256: row.rootSha256 }),
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
      // An installed plugin is no longer a suggestion (5E Decision 11).
      yield* Effect.ignore(Effect.try({ try: () => deps.suggestions?.installed(placed.id), catch: toFinError }));
      // The old version's processes drain and stop: new jobs name the new version, so its key (sp8b Task 5).
      if (prev !== undefined && prev.kind === "code" && (prev.version !== placed.version || revokedNow)) yield* stopCode(prev.id, { version: prev.version, drain: !revokedNow });
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
      // Only what was verified under the root `id` has now updates by itself (review M1).
      const rows = tables
        .installed()
        .filter((p) => p.registryId === id && p.source === "registry" && p.kind === "content" && p.autoUpdate && p.state !== "revoked")
        .filter((p) => tables.installedRootMatches(p.id));
      for (const p of rows) {
        const staged = yield* Effect.option(stagePlan(deps, { id: p.id, update: true, registry: id }, by, id, true));
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
        if (row.state === "revoked") {
          return yield* Effect.fail(finError("PLUGIN_REVOKED", `${id} ${row.version} was revoked by a signed advisory`, { id, version: row.version }, "Install a version the advisory does not name."));
        }
        // There is no sandbox gate (owner decision 1); the host checks platform, host API,
        // Node and `models` (sp8b Task 5, Decision 18). Without a host there is no runtime.
        if (row.kind === "code") {
          if (deps.host === undefined) {
            return yield* Effect.fail(
              finError("PLUGIN_RUNTIME_UNAVAILABLE", `${id} is a code plugin, and this server runs no code plugins`, { id, kind: "code" }, "Keep it installed: a later Finisher update can run it."),
            );
          }
          yield* deps.host.enableCheck(tables.installedFull(id)!);
        }
        // The owner's enable also clears a crash loop and its count.
        tables.store.tx(() => {
          tables.setState(id, true, "active", clock());
          tables.setProblem(id, null);
        });
        deps.host?.enabled(id);
      } else {
        // A crash-looped plugin keeps its `problem`: only the owner's enable clears it (Decision 15).
        if (row.state !== "revoked") tables.setState(id, false, "disabled", clock());
        if (row.kind === "code") yield* stopCode(id, { fail: finError(row.state === "revoked" ? "PLUGIN_REVOKED" : "PLUGIN_DISABLED", `${id} is ${row.state === "revoked" ? "revoked" : "disabled"}`, { plugin: id }) });
      }
      return tables.installedOne(id)!;
    });

  const remove = (id: string, purgeData: boolean): Effect.Effect<InstalledRow, FinError> =>
    Effect.gen(function* () {
      const row = tables.installedOne(id);
      if (row === undefined) return yield* Effect.fail(notInstalled(id));
      if (row.source === "bundled") return yield* Effect.fail(finError("PLUGIN_BUNDLED", `${id} comes with Finisher and cannot be removed`, { id }, "Disable it instead."));
      if (row.kind === "code") yield* stopCode(id, { fail: finError("PLUGIN_NOT_FOUND", `${id} is not installed`, { plugin: id }) });
      // A row whose store directory is already gone (a removal interrupted after it) is removed too.
      yield* attempt(() => native.removePlugin(id, row.version, purgeData)).pipe(Effect.catchIf((e) => e.code === "PLUGIN_NOT_INSTALLED", () => Effect.void));
      yield* Effect.try({ try: () => tables.store.tx(() => tables.deletePlugin(id)), catch: toFinError });
      return row;
    });

  /**
   * Installs or updates the bundled plugin `entry` from the app's resources `root` without a
   * person's approval: the app update was the approval, and the compiled-in hash the check
   * (Task 10, Decision 24). Same lock, plan row and place as an approval, made by `system:bundled`.
   * A widening is applied; the caller audits it with the plan's diff. Resolves to nothing when
   * the installed row already is this version and tree.
   */
  const installBundled = (root: string, entry: { readonly id: string; readonly version: string; readonly treeHash: string }): Effect.Effect<Installed | undefined, FinError> =>
    locked(
      Effect.gen(function* () {
        let prev = tables.installedFull(entry.id);
        if (prev !== undefined && prev.source !== "bundled") {
          return yield* Effect.fail(finError("PLUGIN_ALREADY_INSTALLED", `${entry.id} is installed from elsewhere`, { id: entry.id }, "Remove it to use the one that comes with Finisher."));
        }
        // Another start of the same app got here first: nothing to do.
        if (prev !== undefined && prev.version === entry.version && prev.treeHash === entry.treeHash) return undefined;
        // The same version with other bytes (the app was rebuilt) is replaced, not updated: it
        // is planned as a fresh install and the old copy goes only once that plan exists, so a
        // plan that fails leaves the pack as it was (review M2). What the owner chose is kept.
        const rebuilt = prev !== undefined && prev.version === entry.version ? prev : undefined;
        const body = {
          id: entry.id,
          source: { kind: "bundled", root },
          update: prev !== undefined && rebuilt === undefined,
          // An older app than the installed version (a rollback) installs its own version.
          allowDowngrade: prev !== undefined && rebuilt === undefined,
          ...(prev === undefined || rebuilt !== undefined ? {} : { installed: installedPlugin(prev, tables.grants(entry.id)) }),
        };
        const plan = yield* attemptJson<InstallPlan>(() => native.plan(JSON.stringify(body)));
        if (rebuilt !== undefined) {
          yield* Effect.ignore(attempt(() => native.removePlugin(rebuilt.id, rebuilt.version, false)));
          yield* Effect.try({ try: () => tables.store.tx(() => tables.deletePlugin(rebuilt.id)), catch: toFinError });
        }
        const row: PlanRow = {
          id: plan.planId,
          digest: plan.digest,
          pluginId: plan.plugin.id,
          version: plan.plugin.version,
          source: "bundled",
          action: plan.action,
          byKind: BUNDLED_BY.kind,
          byId: BUNDLED_BY.id,
          human: false,
          base: baseOf(tables, entry.id),
          unusable: false,
          createdAt: clock(),
          expiresAt: plan.expiresAt,
        };
        yield* Effect.try({ try: () => tables.addPlan(row), catch: toFinError });
        // `undefined` keeps the owner's automatic-update choice of the installed version.
        const done = yield* place(row, { copyleft: false, thirdPartyCode: false }, rebuilt?.autoUpdate, BUNDLED_BY).pipe(
          Effect.tapError(() =>
            Effect.sync(() => tables.deletePlan(row.id)).pipe(Effect.andThen(Effect.ignore(attempt(() => native.discardPlan(row.id))))),
          ),
        );
        // A rebuilt pack the owner had switched off stays off.
        if (rebuilt !== undefined && !rebuilt.enabled && done.plugin.enabled) {
          tables.setState(entry.id, false, "disabled", clock());
          return { ...done, plugin: tables.installedOne(entry.id)! };
        }
        return done;
      }),
    );

  /** A bundled plugin whose resources no longer match the app: revoked, disabled. */
  const revokeBundled = (id: string): Effect.Effect<boolean> =>
    locked(
      Effect.sync(() => {
        const row = tables.installedOne(id);
        if (row === undefined || row.source !== "bundled" || row.state === "revoked") return false;
        // Only `state` changes: `enabled` stays the owner's choice, so the restore below puts
        // back what they had (review M2). Nothing is served while the state is `revoked`.
        return tables.setState(id, row.enabled, "revoked", clock());
      }),
    );

  /** A revoked bundled plugin whose resources match the app again: back to what the owner had (on, unless they switched it off). */
  const restoreBundled = (id: string): Effect.Effect<"enabled" | "kept" | false> =>
    locked(
      Effect.sync(() => {
        const row = tables.installedOne(id);
        if (row === undefined || row.source !== "bundled" || row.state !== "revoked") return false;
        const on = row.kind === "content" && row.enabled;
        if (!tables.setState(id, on, on ? "active" : "disabled", clock())) return false;
        return on ? "enabled" : "kept"; // "kept": the owner had it off, and it stays off
      }),
    );

  return {
    plan,
    installBundled,
    revokeBundled,
    restoreBundled,
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
          if (deps.suggestions !== undefined) return deps.suggestions.suggest({ pluginId: req.id, projectId: req.projectId, reason: req.reason }, by);
          // One suggestion per plugin, project and principal, and a bounded number per project:
          // an agent cannot fill the table by asking again (review M2).
          const mine = tables.suggestedBy(by, req.projectId);
          const same = mine.find((x) => x.pluginId === req.id);
          if (same !== undefined) return { suggestionId: same.id };
          if (mine.length >= MAX_SUGGESTIONS) {
            throw finError("TOO_MANY_SUGGESTIONS", `${by.kind} principals may have ${MAX_SUGGESTIONS} suggestions per project`, { limit: MAX_SUGGESTIONS }, "Wait for the owner to look at the ones you made.");
          }
          const now = clock();
          const suggestionId = ulid(now);
          tables.addSuggestion({ id: suggestionId, pluginId: req.id, projectId: req.projectId, reason: req.reason, byKind: by.kind, byId: by.id, createdAt: now });
          return { suggestionId };
        },
        catch: toFinError,
      }),
    /**
     * The maker of plan `planId` gives it up: its row and staged files go, and its slot of
     * `TOO_MANY_PLANS` is free again (review M3). Only the principal who made it may; anyone
     * else, and a plan that is gone, gets `PLAN_EXPIRED` alike.
     */
    discard: (planId: string, by: Asker): Effect.Effect<{ discarded: true }, FinError> =>
      Effect.gen(function* () {
        const row = tables.plan(planId);
        if (row === undefined || row.byKind !== by.kind || row.byId !== by.id) {
          return yield* Effect.fail(finError("PLAN_EXPIRED", `no plan ${planId}: it expired, was used or was never made`, { reason: "missing" }, "Make a new plan."));
        }
        tables.deletePlan(row.id);
        yield* Effect.ignore(attempt(() => native.discardPlan(row.id)));
        return { discarded: true as const };
      }),
    /** Deletes expired plans' rows and fin-registry's expired staging; returns how many. */
    sweep: (): Effect.Effect<number, FinError> =>
      Effect.map(attempt(() => native.sweep()), (n) => n + tables.deleteExpiredPlans(clock())),
  };
}

export type Lifecycle = ReturnType<typeof makeLifecycle>;
