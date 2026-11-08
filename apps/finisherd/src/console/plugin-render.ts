// How the console shows a plan, an installed plugin and a registry to the person who decides
// (plan 8A Task 12b; spec §5.3): the warnings that matter first and in plain words, every
// permission on its own line with its risk, and what an update adds. Text only: nothing here
// connects or asks.
import type { InstallPlan, Listing, Permission, PluginInfo } from "@finisher/core-types";
import type { InstalledRow, RegistryEntry } from "@finisher/contracts";

/** The warnings the person must see before anything else, with the words they are shown in. */
const LOUD: Record<string, (w: { details?: Record<string, unknown> }) => string[]> = {
  thirdPartyCode: () => [
    "!! THIRD-PARTY CODE — HIGH RISK",
    "   This plugin is code written by someone else. Nothing confines it: once it can run, it acts with",
    "   your rights on this computer, whatever permissions it lists. This version of Finisher cannot run",
    "   code plugins yet, so it installs switched off.",
  ],
  registryChanged: () => [
    "!! REGISTRY CHANGED",
    "   This plugin was installed from another registry, or from one whose root has changed since.",
    "   Go ahead only if you trust the registry named below.",
  ],
  yanked: () => ["!! YANKED", "   The publisher withdrew this version."],
};

const asText = (v: unknown): string => (Array.isArray(v) ? v.map(asText).join(", ") : typeof v === "object" && v !== null ? JSON.stringify(v) : String(v));

/** `net (origins: https://api.example.org)` — the permission's key and what it grants. */
export function permissionText(p: Permission): string {
  const { perm, ...rest } = p as { perm: string } & Record<string, unknown>;
  const detail = Object.entries(rest).map(([k, v]) => `${k}: ${asText(v)}`);
  return detail.length === 0 ? perm : `${perm} (${detail.join("; ")})`;
}

const bytesText = (n: number): string => (n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KiB` : `${(n / 1024 / 1024).toFixed(1)} MiB`);

const quieter = (w: { code: string; details?: Record<string, unknown> }): string => {
  const d = w.details ?? {};
  switch (w.code) {
    case "copyleft":
      return `Licence: ${asText(d.spdx)} (${asText(d.class)}); installing it means accepting its terms (add --accept-copyleft)`;
    case "licenceUnknown":
      return `Licence: ${asText(d.spdx)} is not a licence Finisher recognises`;
    case "lookalikeId":
      return `Its id looks like ${asText(d.similar)}: check it is the plugin you meant`;
    case "registryStale":
      return `Its registry's data is ${asText(d.days)} days past its expiry`;
    case "advisory":
      return `Advisory: ${asText(d.summary ?? d.id ?? "see the registry")}`;
    default:
      return Object.keys(d).length === 0 ? w.code : `${w.code}: ${JSON.stringify(d)}`;
  }
};

/** What a plan says about the permissions an update adds, widens or drops. */
function diffLines(plan: InstallPlan): string[] {
  const { added, widened, removed, riskUp } = plan.diff;
  if (plan.action === "install") return [];
  const out: string[] = [`Compared with ${plan.from ?? "the installed version"}:`];
  if (added.length + widened.length + removed.length === 0) out.push("  No permission changes.");
  for (const p of added) out.push(`  + adds ${permissionText(p)}`);
  for (const w of widened) out.push(`  ~ widens ${permissionText(w.from)}  →  ${permissionText(w.to)}`);
  for (const p of removed) out.push(`  - drops ${permissionText(p)}`);
  if (riskUp) out.push("  The overall risk goes up.");
  return out;
}

/** Whether `plan` asks for more than the installed version had. */
export const widens = (plan: InstallPlan): boolean => plan.diff.added.length + plan.diff.widened.length > 0 || plan.diff.riskUp;

export interface Rendered {
  readonly lines: string[];
  readonly thirdPartyCode: boolean;
  readonly copyleft: boolean;
}

/** The plan as the person reads it, loud warnings first. */
export function renderPlan(plan: InstallPlan): Rendered {
  const warnings = plan.warnings ?? [];
  const lines: string[] = [];
  for (const [code, say] of Object.entries(LOUD)) {
    const w = warnings.find((x) => x.code === code);
    if (w !== undefined) lines.push(...say(w), "");
  }
  const { plugin, source } = plan;
  const verb = plan.action === "install" ? "Install" : plan.action === "update" ? "Update" : "DOWNGRADE";
  const move = plan.from == null ? plugin.version : `${plan.from} → ${plugin.version}`;
  const from = source.kind === "registry" && source.registryId != null ? ` from registry ${source.registryId}` : ` (${source.kind})`;
  lines.push(`${verb} ${plugin.id} ${move}${from}`);
  lines.push(`  ${plugin.kind === "code" ? "a code plugin" : "a content pack"} by ${plugin.publisher}, ${bytesText(plan.bytes)} unpacked`);
  lines.push(`  Licence: ${plan.licence.spdx} (${plan.licence.class})`);
  lines.push(`  Risk: ${plan.risk}`);
  if (plan.permissions.length === 0) lines.push("Permissions: none");
  else {
    lines.push(`Permissions (${plan.permissions.length}):`);
    for (const p of plan.permissions) lines.push(`  [${p.risk}] ${permissionText(p.permission)}`);
  }
  lines.push(...diffLines(plan));
  for (const m of plan.models ?? []) lines.push(`Model: ${m.id} (${bytesText(m.bytes)}, ${m.licence})`);
  const rest = warnings.filter((w) => !(w.code in LOUD));
  if (rest.length > 0) {
    lines.push("Also:");
    for (const w of rest) lines.push(`  ${quieter(w)}`);
  }
  if (plan.registryStaleDays != null && !rest.some((w) => w.code === "registryStale")) lines.push(`  Its registry's data is ${plan.registryStaleDays} days past its expiry`);
  lines.push(`Plan ${plan.planId} expires ${new Date(plan.expiresAt).toISOString()}`);
  return { lines, thirdPartyCode: warnings.some((w) => w.code === "thirdPartyCode"), copyleft: warnings.some((w) => w.code === "copyleft") };
}

/** One installed plugin as a line. */
export function installedLine(p: InstalledRow): string {
  const state = p.state === "active" ? (p.enabled ? "enabled" : "disabled") : p.state;
  const from = p.source === "registry" && p.registryId !== undefined ? ` from ${p.registryId}` : ` (${p.source})`;
  return `${p.id} ${p.version}  ${p.kind}  ${state}${p.autoUpdate ? "  auto-update" : ""}${from}${p.registryStaleDays === undefined ? "" : `  registry ${p.registryStaleDays} days stale`}`;
}

/** A search hit as a line. */
export const listingLine = (l: Listing): string => `${l.id} ${l.latest}  ${l.kind}  ${l.license}  registry ${l.registryId}`;

/** A registry as lines for `registry ls` and `registry status`. */
export function registryLines(e: RegistryEntry): string[] {
  const last = e.lastRefresh === undefined ? "never refreshed" : `refreshed ${new Date(e.lastRefresh.at).toISOString()}${e.lastRefresh.ok ? "" : ` (failed: ${e.lastRefresh.code ?? "error"})`}`;
  const stale = e.staleDays == null ? "" : `, ${e.staleDays} days past expiry (up to ${e.maxStaleDays} allowed)`;
  return [
    `${e.id}  ${e.path ?? ""}`,
    `  root ${e.rootSha256}`,
    `  ${e.plugins} plugins, ${e.advisories} advisories, ${last}${stale}`,
  ];
}

/** What a plugin's registries sign about it, for `plugin info`. */
export function infoLines(id: string, installed: InstalledRow | undefined, registries: ReadonlyArray<PluginInfo>): string[] {
  const out = [id];
  out.push(installed === undefined ? "  not installed" : `  installed: ${installedLine(installed)}`);
  for (const r of registries) {
    out.push(`  registry ${r.registryId}:`);
    for (const v of r.versions) out.push(`    ${v.version}  ${v.kind}  ${v.license}  ${bytesText(v.bytesUnpacked)}${v.yanked ? "  YANKED" : ""}`);
    for (const a of r.advisories) out.push(`    advisory ${a.id} (${a.severity}, ${a.action}): ${a.summary}`);
  }
  if (registries.length === 0) out.push("  no registry lists it");
  return out;
}
