// The plugin and registry RPCs (8A Task 9a): the handlers `handlers.ts` takes as call sites.
// Every one passes the chain first (scopes: `plugin.read` for the reads and refreshes,
// `plugin.admin`, human-only, for adding, removing and forgetting registries). The registry
// administration is audited (`registry.add|remove|forget`, and `registry.verifyFail` for a
// refresh whose signed data did not verify), with ids and words only.
import type {
  FinError,
  PluginInfoResult,
  PluginSearchRequest,
  RegistryAddRequest,
  RegistryEntry,
  RegistryRefreshResult,
} from "@finisher/contracts";
import { Effect } from "effect";
import { chainOf } from "../chain/chain.ts";
import type { Audit, AuditInput } from "../control/audit.ts";
import { requireAbsolute } from "../paths.ts";
import { LOCAL } from "../principal.ts";
import { type Asker, isVerifyFailure, type PluginsShape } from "./registries.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;
type Options = { readonly headers: unknown };

export interface PluginRpcDeps {
  readonly plugins: PluginsShape;
  readonly audit: Audit;
  /** `handlers.ts`' `guard`: defects become `INTERNAL`, the call is timed into `rpc.stats`. */
  readonly guard: Guard;
}

/** Who a handler serves (the chain's principal; `local` outside the chain). */
const askerOf = (options: object): Asker => {
  const p = chainOf(options)?.principal ?? LOCAL;
  return { kind: p.kind, id: p.id, human: p.human };
};

export function pluginHandlers({ plugins, audit, guard }: PluginRpcDeps) {
  const record = (by: Asker, kind: AuditInput["kind"], outcome: AuditInput["outcome"], details: Record<string, unknown>, code?: string): void =>
    audit.append({ kind, principal: { kind: by.kind, id: by.id }, outcome, ...(code === undefined ? {} : { code }), details: code === undefined ? details : { ...details, code } });

  /** Audits the end of an administration call and passes its result on. */
  const audited = <A>(by: Asker, kind: AuditInput["kind"], details: Record<string, unknown>, self: Effect.Effect<A, FinError>): Effect.Effect<A, FinError> =>
    self.pipe(
      Effect.tapError((e) => Effect.sync(() => record(by, kind, e.code === "REGISTRY_UNTRUSTED" ? "denied" : "failed", details, e.code))),
      Effect.tap(() => Effect.sync(() => record(by, kind, "ok", details))),
    );

  const verifyFailures = (by: Asker, results: ReadonlyArray<RegistryRefreshResult["results"][number]>): void => {
    for (const r of results) if (!r.ok && isVerifyFailure(r.error.code)) record(by, "registry.verifyFail", "failed", { registryId: r.id }, r.error.code);
  };

  return {
    "registry.list": () => guard("registry.list", plugins.registries()),
    "registry.status": ({ id }: { readonly id: string }) => guard("registry.status", plugins.status(id)),
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
            return yield* plugins.add({ cfg: { ...req.cfg, path }, root }, by);
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
          Effect.tap((r) => Effect.sync(() => verifyFailures(by, r.results))),
        ),
      );
    },
    "plugin.list": () => guard("plugin.list", Effect.map(plugins.installed(), (list) => ({ plugins: list }))),
    "plugin.search": (q: PluginSearchRequest) => guard("plugin.search", plugins.search(q)),
    "plugin.info": ({ id }: { readonly id: string }): Effect.Effect<PluginInfoResult, FinError> => guard("plugin.info", plugins.info(id)),
  };
}
