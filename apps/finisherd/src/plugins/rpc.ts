// The plugin and registry RPCs (8A Tasks 9a and 9b): the handlers `handlers.ts` takes as call
// sites. Every one passes the chain first (scopes: `plugin.read` for the reads, refreshes and
// plans; `plugin.install` and `plugin.admin`, human-only, for the owner's changes; `plugin.suggest`
// with `project.read` for suggestions). Every change is audited with ids and words only
// (Decision 28): `registry.add|remove|forget`, `registry.verifyFail` for a refresh whose signed
// data did not verify, `plugin.install|update|remove|enable|disable|revoke|grant`.
//
// A registry's folder `path`, and every other location on the server, is shown to people
// (`human` principals) only: `local` is not a person (Decision 20; an agent in a shell
// authenticates as `local`), so neither it nor an `agent` or `mcp` principal learns where the
// registries live, in a status or in an error (owner decision, 2026-11-08; review L1, L2).
import type {
  FinError,
  PluginInfoResult,
  PluginInstallRequest,
  PluginInstallResult,
  PluginPlanRequest,
  PluginSearchRequest,
  RegistryAddRequest,
  RegistryEntry,
  RegistryListResult,
  RegistryRefreshResult,
} from "@finisher/contracts";
import type { InstallPlan } from "@finisher/core-types";
import { Effect } from "effect";
import { chainOf } from "../chain/chain.ts";
import type { Audit, AuditInput } from "../control/audit.ts";
import { requireAbsolute } from "../paths.ts";
import { ANONYMOUS } from "../principal.ts";
import { scrubError, scrubParts } from "./scrub.ts";
import type { Installed, Revoked } from "./lifecycle.ts";
import { type Asker, isVerifyFailure, type PluginsShape } from "./registries.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;
type Options = { readonly headers: unknown };

export interface PluginRpcDeps {
  readonly plugins: PluginsShape;
  readonly audit: Audit;
  /** `handlers.ts`' `guard`: defects become `INTERNAL`, the call is timed into `rpc.stats`. */
  readonly guard: Guard;
}

/** Who a handler serves (the chain's principal; `anonymous` outside the chain: fail closed). */
const askerOf = (options: object): Asker => {
  const p = chainOf(options)?.principal ?? ANONYMOUS;
  return { kind: p.kind, id: p.id, human: p.human };
};

/** Whether `by` may see where a registry lives, or where an error happened: a person. */
export const seesPaths = (by: Asker): boolean => by.human;

function withoutPath<T extends { readonly path?: string }>(e: T): Omit<T, "path"> {
  const { path: _path, ...rest } = e;
  return rest;
}

/** `entry` as `by` may see it. */
export const entryFor = (by: Asker, e: RegistryEntry): RegistryEntry => (seesPaths(by) ? e : withoutPath(e));

export function pluginHandlers({ plugins, audit, guard }: PluginRpcDeps) {
  const { lifecycle } = plugins;
  const record = (by: Asker, kind: AuditInput["kind"], outcome: AuditInput["outcome"], details: Record<string, unknown>, code?: string): void =>
    audit.append({ kind, principal: { kind: by.kind, id: by.id }, outcome, ...(code === undefined ? {} : { code }), details: code === undefined ? details : { ...details, code } });

  const outcomeOf = (e: FinError): AuditInput["outcome"] => (e.code === "REGISTRY_UNTRUSTED" || e.code === "HUMAN_ONLY" || e.code.endsWith("_NOT_ACCEPTED") ? "denied" : "failed");

  /** Audits the end of a change and passes its result on. */
  const audited = <A>(by: Asker, kind: AuditInput["kind"], details: Record<string, unknown>, self: Effect.Effect<A, FinError>): Effect.Effect<A, FinError> =>
    self.pipe(
      Effect.tapError((e) => Effect.sync(() => record(by, kind, outcomeOf(e), details, e.code))),
      Effect.tap(() => Effect.sync(() => record(by, kind, "ok", details))),
    );

  const verifyFailures = (by: Asker, results: ReadonlyArray<RegistryRefreshResult["results"][number]>): void => {
    for (const r of results) if (!r.ok && isVerifyFailure(r.error.code)) record(by, "registry.verifyFail", "failed", { registryId: r.id }, r.error.code);
  };

  const revocations = (by: Asker, revoked: ReadonlyArray<Revoked>, registryId: string | undefined): void => {
    for (const r of revoked) {
      const reg = registryId ?? plugins.tables.installedOne(r.id)?.registryId;
      record(by, "plugin.revoke", "ok", { pluginId: r.id, version: r.version, ...(reg === undefined ? {} : { registryId: reg }) }, "PLUGIN_REVOKED");
    }
  };

  /** The audit rows of an approval: the install or update, and the grants it added. */
  const installs = (by: Asker, done: Installed, auto = false): void => {
    const p = done.plan;
    const added = p.diff.added.length + p.diff.widened.length;
    const base = { pluginId: p.plugin.id, version: p.plugin.version, ...(p.source.registryId == null ? {} : { registryId: p.source.registryId }) };
    record(by, done.action === "install" ? "plugin.install" : "plugin.update", "ok", {
      ...base,
      permissionsAdded: added,
      risk: p.risk,
      ...(auto ? { auto: true } : {}),
      ...(done.thirdPartyCode === undefined ? {} : { thirdPartyCode: done.thirdPartyCode }),
    });
    if (added > 0) record(by, "plugin.grant", "ok", { ...base, permissionsAdded: added, ...(auto ? { auto: true } : {}) });
  };

  const handlers = {
    "registry.list": (_: unknown, options: Options): Effect.Effect<RegistryListResult, FinError> => {
      const by = askerOf(options);
      return guard(
        "registry.list",
        Effect.map(plugins.registries(), (l) => ({
          registries: l.registries.map((e) => entryFor(by, e)),
          problems: seesPaths(by) ? l.problems : l.problems.map((p) => ({ id: p.id, code: p.code, message: scrubParts(p).message })),
        })),
      );
    },
    "registry.status": ({ id }: { readonly id: string }, options: Options) => guard("registry.status", Effect.map(plugins.status(id), (e) => entryFor(askerOf(options), e))),
    "registry.add": (req: RegistryAddRequest, options: Options): Effect.Effect<RegistryEntry, FinError> => {
      const by = askerOf(options);
      const details = { registryId: req.cfg.id };
      return guard(
        "registry.add",
        audited(
          by,
          "registry.add",
          details,
          Effect.gen(function* () {
            const path = yield* requireAbsolute("cfg.path", req.cfg.path);
            const root = req.root.kind === "file" ? { ...req.root, path: yield* requireAbsolute("root.path", req.root.path) } : req.root;
            return entryFor(by, yield* plugins.add({ cfg: { ...req.cfg, path }, root }, by));
          }),
        ),
      );
    },
    "registry.remove": ({ id }: { readonly id: string }, options: Options) =>
      guard(
        "registry.remove",
        Effect.as(audited(askerOf(options), "registry.remove", { registryId: id }, plugins.remove(id)), { removed: true }),
      ),
    "registry.forget": ({ rootSha256 }: { readonly rootSha256: string }, options: Options) => {
      const sha = rootSha256.toLowerCase();
      return guard("registry.forget", Effect.as(audited(askerOf(options), "registry.forget", { rootSha256: sha }, plugins.forget(sha)), { forgotten: true }));
    },
    "registry.refresh": ({ id }: { readonly id?: string | undefined }, options: Options): Effect.Effect<RegistryRefreshResult, FinError> => {
      const by = askerOf(options);
      return guard(
        "registry.refresh",
        plugins.refresh(id, by).pipe(
          Effect.tapError((e) =>
            Effect.sync(() => {
              if (id !== undefined && isVerifyFailure(e.code)) record(by, "registry.verifyFail", "failed", { registryId: id }, e.code);
            }),
          ),
          Effect.tap((r) =>
            Effect.sync(() => {
              verifyFailures(by, r.results);
              revocations(by, r.applied.revoked, undefined);
              for (const u of r.updates) installs(by, u, true);
            }),
          ),
          Effect.map(({ updates: _updates, ...r }) => ({
            ...r,
            results: r.results.map((x) => (x.ok ? { ...x, status: entryFor(by, x.status) } : seesPaths(by) ? x : { ...x, error: scrubParts(x.error) })),
          })),
        ),
      );
    },
    "plugin.list": () =>
      guard(
        "plugin.list",
        Effect.map(plugins.installed(), (list) => ({ plugins: list, bundled: plugins.bundled(), ...(plugins.problems().length === 0 ? {} : { problems: plugins.problems() }) })),
      ),
    "plugin.search": (q: PluginSearchRequest) => guard("plugin.search", plugins.search(q)),
    "plugin.info": ({ id }: { readonly id: string }): Effect.Effect<PluginInfoResult, FinError> => guard("plugin.info", plugins.info(id)),
    "plugin.plan": (req: PluginPlanRequest, options: Options): Effect.Effect<InstallPlan, FinError> => {
      const by = askerOf(options);
      return guard(
        "plugin.plan",
        lifecycle.plan(req, by).pipe(
          Effect.tap(({ revoked }) => Effect.sync(() => revocations(by, revoked, undefined))),
          Effect.map(({ plan }) => plan),
        ),
      );
    },
    "plugin.install": (req: PluginInstallRequest, options: Options): Effect.Effect<PluginInstallResult, FinError> => {
      const by = askerOf(options);
      // Read before the install: a failed one may delete its plan.
      const pluginOfPlan = plugins.tables.plan(req.planId)?.pluginId;
      return guard(
        "plugin.install",
        lifecycle.install(req, by).pipe(
          Effect.tapError((e) => Effect.sync(() => record(by, "plugin.install", outcomeOf(e), { planId: req.planId, ...(pluginOfPlan === undefined ? {} : { pluginId: pluginOfPlan }) }, e.code))),
          Effect.tap((done) => Effect.sync(() => installs(by, done))),
          Effect.map((done) => ({ action: done.action, plugin: done.plugin })),
        ),
      );
    },
    "plugin.remove": ({ id, purgeData }: { readonly id: string; readonly purgeData?: boolean | undefined }, options: Options) => {
      const by = askerOf(options);
      return guard(
        "plugin.remove",
        audited(
          by,
          "plugin.remove",
          { pluginId: id },
          Effect.map(lifecycle.remove(id, purgeData === true), () => ({ removed: true })),
        ),
      );
    },
    "plugin.enable": ({ id }: { readonly id: string }, options: Options) =>
      guard("plugin.enable", Effect.map(audited(askerOf(options), "plugin.enable", { pluginId: id }, lifecycle.enable(id)), (plugin) => ({ plugin }))),
    "plugin.disable": ({ id }: { readonly id: string }, options: Options) =>
      guard("plugin.disable", Effect.map(audited(askerOf(options), "plugin.disable", { pluginId: id }, lifecycle.disable(id)), (plugin) => ({ plugin }))),
    "plugin.suggest": (req: { readonly id: string; readonly reason: string; readonly projectId: string }, options: Options) =>
      guard("plugin.suggest", lifecycle.suggest(req, askerOf(options))),
  };
  // Whatever fails for someone who may not see locations fails without them (review L2).
  const scrubbed = Object.fromEntries(
    Object.entries(handlers).map(([name, handler]) => [
      name,
      (payload: unknown, options: Options) => Effect.mapError((handler as (p: unknown, o: Options) => Effect.Effect<unknown, FinError>)(payload, options), (e) => (seesPaths(askerOf(options)) ? e : scrubError(e))),
    ]),
  );
  return scrubbed as unknown as typeof handlers;
}
