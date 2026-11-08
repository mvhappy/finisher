// Bundled plugins without TUF (8A Task 10, Decision 24): the plugins that come with the app are
// unpacked in its resources, and the binary carries a compiled-in list of their ids, versions
// and tree hashes (`fin-node`'s `build.rs`). At start each tree is re-hashed:
//   - it matches and nothing is installed: installed, enabled (content packs), no approval;
//   - it matches a newer version than the installed one (the app was updated): replaced in
//     place by the same lock, plan and rows as an approval; a widening is applied (the app
//     update was the approval) and audited with its permission count;
//   - it matches what is installed: nothing changes (a revoked one is restored);
//   - it does not match (a changed, added, missing or unreadable file): never installed; an
//     installed one is revoked and disabled with `PLUGIN_BUNDLED_MISMATCH` in the status.
// A bundled plugin can be disabled but not removed (`PLUGIN_BUNDLED`), and updates only with
// the app (`plugin.plan` refuses it).
//
// Before sub-project 11's code signing nothing signs the resources: the compiled-in hashes
// detect corruption and partial updates, not someone who rewrites both the resources and the
// binary (Decision 25). The status says so: `integrity: "hashesOnly"`.
import { fileURLToPath } from "node:url";
import type { FinError } from "@finisher/contracts";
import type { Registry } from "@finisher/fin-node";
import { Effect } from "effect";
import type { Audit } from "../control/audit.ts";
import { attemptJson } from "../errors.ts";
import { BUNDLED_BY, type Installed, type Lifecycle } from "./lifecycle.ts";
import { actorOf } from "./tables.ts";
import type { PluginTables } from "./tables.ts";

/** What `finisherd` can say about the integrity of the resources it shipped with. */
export type BundledIntegrity = "hashesOnly" | "signed";

/**
 * `hashesOnly` on every build before sub-project 11 (TASKS: its code-signature check over the
 * bundled resources reports `signed`): no build is signed yet.
 */
export const bundledIntegrity = (): BundledIntegrity => "hashesOnly";

/** One bundled plugin as the status shows it: never a hash. */
export interface BundledItem {
  readonly id: string;
  readonly version: string;
  readonly ok: boolean;
  /** `PLUGIN_BUNDLED_MISMATCH` when the resources do not match the app. */
  readonly code?: string;
}

export interface BundledStatus {
  readonly integrity: BundledIntegrity;
  readonly plugins: ReadonlyArray<BundledItem>;
}

interface Verified {
  readonly id: string;
  readonly version: string;
  readonly treeHash: string;
  readonly ok: boolean;
  readonly code?: string;
}

/** `<repository>/resources/plugins/bundled`, or `FIN_BUNDLED_PLUGINS_DIR` (packaged apps set it). */
export function defaultBundledRoot(env: NodeJS.ProcessEnv = process.env): string {
  const set = env.FIN_BUNDLED_PLUGINS_DIR;
  if (set !== undefined && set !== "") return set;
  return fileURLToPath(new URL("../../../../resources/plugins/bundled", import.meta.url));
}

export interface BundledDeps {
  readonly native: Registry;
  readonly tables: PluginTables;
  readonly lifecycle: Lifecycle;
  readonly audit: Audit;
  /** Where the app's bundled plugin folders are. */
  readonly root: string;
}

export function makeBundled({ native, tables, lifecycle, audit, root }: BundledDeps) {
  let status: BundledStatus = { integrity: bundledIntegrity(), plugins: [] };
  const by = BUNDLED_BY;
  const record = (kind: "plugin.install" | "plugin.update" | "plugin.grant" | "plugin.revoke" | "plugin.enable", details: Record<string, unknown>, code?: string) =>
    audit.append({ kind, principal: { kind: by.kind, id: by.id }, outcome: "ok", ...(code === undefined ? {} : { code }), details: code === undefined ? details : { ...details, code } });

  /** The audit rows of an install or update: the permission count is the diff. */
  const audited = (done: Installed): void => {
    const p = done.plan;
    const added = p.diff.added.length + p.diff.widened.length;
    const base = { pluginId: p.plugin.id, version: p.plugin.version };
    record(done.action === "install" ? "plugin.install" : "plugin.update", { ...base, permissionsAdded: added, risk: p.risk, auto: true });
    if (added > 0) record("plugin.grant", { ...base, permissionsAdded: added, auto: true });
  };

  const one = (v: Verified): Effect.Effect<BundledItem, FinError> =>
    Effect.gen(function* () {
      const item: BundledItem = { id: v.id, version: v.version, ok: v.ok, ...(v.code === undefined ? {} : { code: v.code }) };
      const row = tables.installedFull(v.id);
      if (!v.ok) {
        if (yield* lifecycle.revokeBundled(v.id)) record("plugin.revoke", { pluginId: v.id, version: row?.version ?? v.version }, "PLUGIN_BUNDLED_MISMATCH");
        return item;
      }
      if (row !== undefined && row.source === "bundled" && row.version === v.version && row.treeHash === v.treeHash) {
        if (yield* lifecycle.restoreBundled(v.id)) record("plugin.enable", { pluginId: v.id });
        return item;
      }
      const done = yield* lifecycle.installBundled(root, v);
      if (done !== undefined) audited(done);
      return item;
    });

  /** Checks and installs every bundled plugin; one failing does not stop the others. */
  const sync = (): Effect.Effect<BundledStatus, FinError> =>
    Effect.gen(function* () {
      const verified = yield* attemptJson<Verified[]>(() => native.bundledVerify(root));
      const items: BundledItem[] = [];
      for (const v of verified) {
        const item = yield* one(v).pipe(
          Effect.catch((e) => Effect.as(Effect.logWarning(`finisherd: bundled plugin ${v.id} could not be installed: ${e.code}`), { id: v.id, version: v.version, ok: false, code: e.code } as BundledItem)),
        );
        items.push(item);
      }
      status = { integrity: bundledIntegrity(), plugins: items };
      return status;
    });

  return { sync, status: (): BundledStatus => status, actor: actorOf(by) };
}

export type Bundled = ReturnType<typeof makeBundled>;
