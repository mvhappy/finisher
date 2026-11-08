// What enabled content packs contribute (8A Task 12a, Decisions 22 and 26, spec §4.2).
//
// - **Font catalogs** merge into the server's catalog (`fonts.catalog`, `fonts.fetch`, new
//   pins) by entry id, through fin-fonts: the same pinning, SHA-256, licence, https and
//   source-host checks as the built-in catalog, and no pack replaces a built-in id or another
//   pack's (`catalogIdTaken`; a pack that clashes is left out whole and listed in
//   `plugin.list` `problems`). `sync()` computes the set from the `plugins` rows (enabled,
//   active, content), so it runs after every change to them and at start; disabling, removing
//   or revoking a pack takes its entries out of the catalog for new pins, while the font files
//   a project pinned stay in the cache and keep working (fonts are content-addressed).
// - **Caption styles** are read by `plugin.content.get` and copied into a project by
//   `fin style add --from @pub/pack:<name>` (Task 12c) through `style.add`. A pack that is not
//   installed, is disabled or is revoked (a bundled pack whose files no longer match the app
//   included) serves nothing.
//
// Everything is read from the pack's store folder by fin-node (`Registry.packContent`), which
// validates each file again as the install did.
import type { FinError, PluginContentGetRequest, PluginContentResult } from "@finisher/contracts";
import type { Registry } from "@finisher/fin-node";
import { Effect } from "effect";
import { attemptJson, finError } from "../errors.ts";
import type { InstalledFull, PluginTables } from "./tables.ts";

/** A pack's font catalogs, as the font manager takes them. */
export interface FontPack {
  readonly id: string;
  /** The text of each `catalog.json` the pack ships. */
  readonly catalogs: ReadonlyArray<string>;
}

/** A pack the font manager left out. */
export interface PackRejected {
  readonly id: string;
  readonly code: string;
  readonly message: string;
}

/** Where the enabled packs' font catalogs go (the font manager's `setExtraCatalogs`). */
export type FontSink = (packs: ReadonlyArray<FontPack>) => Effect.Effect<ReadonlyArray<PackRejected>, FinError>;

/** A pack whose contributions are not in use, listed in `plugin.list` `problems`. */
export interface ContentProblem {
  readonly id: string;
  readonly version: string;
  readonly reason: "contentInvalid" | "fontCatalogRejected";
  readonly code?: string;
}

interface PackContent {
  readonly id: string;
  readonly version: string;
  readonly fontCatalogs: ReadonlyArray<string>;
  readonly captionStyles: ReadonlyArray<{ readonly name: string; readonly title?: string; readonly style: PluginContentResult["style"] }>;
}

export interface ContentDeps {
  readonly native: Registry;
  readonly tables: PluginTables;
  /** Absent: no font manager to feed (tests of the lifecycle alone). */
  readonly fonts?: FontSink | undefined;
}

/** Whether the pack's contributions are served: a content pack that is enabled and active. */
const serving = (r: { readonly kind: string; readonly enabled: boolean; readonly state: string }): boolean => r.kind === "content" && r.enabled && r.state === "active";

export function makeContent({ native, tables, fonts }: ContentDeps) {
  let problems: ReadonlyArray<ContentProblem> = [];
  // What the font manager has now: the packs and their folders, so an unchanged set costs nothing.
  let applied: string | undefined;

  /** The font catalogs of the serving packs go to the font manager. Never fails: a damaged pack is a problem. */
  const sync = (): Effect.Effect<void> =>
    Effect.gen(function* () {
      if (fonts === undefined) return;
      const rows: InstalledFull[] = tables.installed().flatMap((r) => {
        const full = tables.installedFull(r.id);
        return full !== undefined && serving(full) ? [full] : [];
      });
      rows.sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
      const key = JSON.stringify(rows.map((r) => [r.id, r.version, r.dir]));
      if (key === applied) return;
      const found: ContentProblem[] = [];
      const packs: FontPack[] = [];
      for (const r of rows) {
        const read = yield* Effect.result(attemptJson<PackContent>(() => native.packContent(r.dir)));
        if (read._tag === "Failure") {
          found.push({ id: r.id, version: r.version, reason: "contentInvalid", code: read.failure.code });
          continue;
        }
        if (read.success.fontCatalogs.length > 0) packs.push({ id: r.id, catalogs: read.success.fontCatalogs });
      }
      const sent = yield* Effect.result(fonts(packs));
      if (sent._tag === "Failure") {
        yield* Effect.logWarning(`finisherd: plugin font catalogs could not be applied: ${sent.failure.code}`);
        return;
      }
      for (const rej of sent.success) {
        found.push({ id: rej.id, version: rows.find((r) => r.id === rej.id)?.version ?? "", reason: "fontCatalogRejected", code: rej.code });
      }
      problems = found;
      applied = key;
    });

  /** `plugin.content.get`: why a pack serves nothing, or the style. */
  const get = (req: PluginContentGetRequest): Effect.Effect<PluginContentResult, FinError> =>
    Effect.gen(function* () {
      const row = tables.installedFull(req.id);
      if (row === undefined) {
        return yield* Effect.fail(finError("PLUGIN_NOT_INSTALLED", `${req.id} is not installed`, { id: req.id }, "Check the plugin's id in the list of installed plugins."));
      }
      if (row.kind !== "content") {
        return yield* Effect.fail(finError("NOT_FOUND", `${req.id} is a code plugin and has no caption styles`, { kind: "captionStyle", id: req.id, name: req.name }, "Name a content pack."));
      }
      if (row.state === "revoked") {
        const bundled = row.source === "bundled";
        return yield* Effect.fail(
          finError(
            bundled ? "PLUGIN_BUNDLED_MISMATCH" : "PLUGIN_REVOKED",
            bundled ? `${req.id} does not match the app it came with` : `${req.id} ${row.version} was revoked by a signed advisory`,
            { id: req.id, version: row.version },
            bundled ? "Reinstall Finisher." : "Install a version the advisory does not name.",
          ),
        );
      }
      if (!row.enabled) {
        return yield* Effect.fail(finError("PLUGIN_DISABLED", `${req.id} is disabled`, { id: req.id }, "An owner can enable it."));
      }
      const content = yield* attemptJson<PackContent>(() => native.packContent(row.dir));
      const found = content.captionStyles.find((s) => s.name === req.name);
      if (found === undefined) {
        return yield* Effect.fail(
          finError("NOT_FOUND", `${req.id} has no caption style ${req.name}`, { kind: "captionStyle", id: req.id, name: req.name, available: content.captionStyles.map((s) => s.name) }, "Name one of the pack's styles."),
        );
      }
      return {
        id: content.id,
        version: content.version,
        kind: "captionStyle" as const,
        name: found.name,
        ...(found.title === undefined ? {} : { title: found.title }),
        style: found.style,
      };
    });

  return { sync, get, problems: (): ReadonlyArray<ContentProblem> => problems };
}

export type Content = ReturnType<typeof makeContent>;
