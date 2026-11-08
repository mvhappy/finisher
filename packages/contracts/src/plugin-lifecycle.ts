// The plugin lifecycle over RPC (sub-project 8A Task 9b; spec §5.3–5.4): plan → approve →
// install, update, enable, disable, remove, and an agent's suggestion. The plan is fin-registry's
// `InstallPlan` (a generated type of `@finisher/core-types`); the rows are `control.db`'s.
import type { InstallPlan } from "@finisher/core-types";
import { Schema } from "effect";
import { InstalledRowSchema, RegistryIdSchema } from "./plugins.ts";

export type { InstallPlan } from "@finisher/core-types";

/** `@<publisher>/<name>` (8A Decision 2's shape; fin-registry checks the rest). */
export const PluginIdSchema = Schema.String.check(Schema.isPattern(/^@[a-z0-9][a-z0-9-]{1,38}\/[a-z0-9][a-z0-9-]{1,63}$/));
/** Semver 2.0 without build metadata (Decision 3; fin-registry parses it). */
const VersionSchema = Schema.String.check(Schema.isMaxLength(64), Schema.isPattern(/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/));

/**
 * `plugin.plan` (`plugin.read`): a plan for `id` from `registry` (by default the registry it is
 * installed from, or the only one that lists it). `version` names an exact version;
 * `allowDowngrade` (with `version`, a person only: it needs `plugin.install`) plans an older one.
 * Nothing runs; the plan expires after 10 minutes.
 */
export const PluginPlanPayload = Schema.Struct({
  id: PluginIdSchema,
  version: Schema.optional(VersionSchema),
  registry: Schema.optional(RegistryIdSchema),
  update: Schema.optional(Schema.Boolean),
  allowDowngrade: Schema.optional(Schema.Boolean),
});
export type PluginPlanRequest = typeof PluginPlanPayload.Type;
export const PluginPlanSuccess = Schema.Unknown;
export type PluginPlanResult = InstallPlan;

/** What a person accepted beyond the permissions: copyleft terms, third-party code (spec §5.3). */
export const PluginAcceptSchema = Schema.Struct({
  copyleft: Schema.optional(Schema.Boolean),
  thirdPartyCode: Schema.optional(Schema.Boolean),
});

/**
 * `plugin.install` (`plugin.install`, owner): installs the plan `planId` exactly as `digest`
 * names it, from a plan a person made. `autoUpdate` turns automatic updates on or off for a
 * content pack (kept from the installed version when absent; off for a first install).
 */
export const PluginInstallPayload = Schema.Struct({
  planId: Schema.String.check(Schema.isPattern(/^p[0-9a-f]{24}$/)),
  digest: Schema.String.check(Schema.isPattern(/^[0-9a-f]{64}$/)),
  accept: Schema.optional(PluginAcceptSchema),
  autoUpdate: Schema.optional(Schema.Boolean),
});
export type PluginInstallRequest = typeof PluginInstallPayload.Type;
export const PluginInstallSuccess = Schema.Struct({
  action: Schema.Literals(["install", "update", "downgrade"]),
  plugin: InstalledRowSchema,
});
export type PluginInstallResult = typeof PluginInstallSuccess.Type;

/** `plugin.remove` (`plugin.install`, owner): deletes the store directory, its data with `purgeData`. */
export const PluginRemovePayload = Schema.Struct({ id: PluginIdSchema, purgeData: Schema.optional(Schema.Boolean) });
export const PluginRemoveSuccess = Schema.Struct({ removed: Schema.Boolean });

/** `plugin.enable` / `plugin.disable` (`plugin.admin`, owner), per server. */
export const PluginIdOnlyPayload = Schema.Struct({ id: PluginIdSchema });
export const PluginStateSuccess = Schema.Struct({ plugin: InstalledRowSchema });

/**
 * `plugin.suggest` (`plugin.suggest` and `project.read`): an agent suggests a plugin for a
 * project (arch §8.3). It is recorded as a suggestion for a person, never as an approval.
 */
export const PluginSuggestPayload = Schema.Struct({
  id: PluginIdSchema,
  reason: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(2000)),
  projectId: Schema.String,
});
export const PluginSuggestSuccess = Schema.Struct({ suggestionId: Schema.String });

/** What a refresh changed in the installed plugins (Decision 23, spec §6.5). */
export interface RefreshApplied {
  /** Versions a signed `disable` advisory revoked (any principal's refresh). */
  readonly revoked: ReadonlyArray<{ readonly id: string; readonly version: string; readonly advisory: string }>;
  /** Content packs updated automatically (a person's refresh only, nothing widening). */
  readonly updated: ReadonlyArray<{ readonly id: string; readonly from: string; readonly to: string }>;
}
