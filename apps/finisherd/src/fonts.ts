// The server's fonts (plan 4C Task 22): fin-node's `FontManager` for this server data, which
// the project engines share, and the `fonts.autoFetch` setting applied at run time.
import { unwatchFile, watchFile } from "node:fs";
import type { FontCatalog, FontsStatus } from "@finisher/core-types";
import type { FinError } from "@finisher/contracts";
import { FontManager } from "@finisher/fin-node";
import { Context, Effect, Layer, type Scope } from "effect";
import { readSettings, type Settings, settingsPath } from "./config.ts";
import { attempt, attemptJson } from "./errors.ts";
import type { FontPack, PackRejected } from "./plugins/content.ts";

/** `fonts.fetch`'s payload. */
export interface FontsFetchRequest {
  readonly sha256?: ReadonlyArray<string> | undefined;
  readonly entries?: ReadonlyArray<string> | undefined;
  readonly keys?: ReadonlyArray<string> | undefined;
  readonly pinned?: string | undefined;
}

export interface FontsShape {
  readonly catalog: () => Effect.Effect<FontCatalog, FinError>;
  readonly status: () => Effect.Effect<FontsStatus, FinError>;
  readonly fetch: (req: FontsFetchRequest) => Effect.Effect<FontsStatus, FinError>;
  /**
   * The enabled plugin packs' font catalogs (8A Task 12a), replacing the previous set: merged
   * into the catalog under the built-in rules; a pack that breaks them or clashes is left out
   * and returned.
   */
  readonly setExtraCatalogs: (packs: ReadonlyArray<FontPack>) => Effect.Effect<ReadonlyArray<PackRejected>, FinError>;
  /** Whether background fetching is on now (the setting and `FIN_FONTS_OFFLINE`). */
  readonly autoFetch: () => boolean;
}

export class Fonts extends Context.Service<Fonts, FontsShape>()("finisherd/Fonts") {}

export interface FontsOptions {
  /** How often `<server-data>/settings.json` is looked at (default 1 s). */
  readonly settingsPollMs?: number;
}

/**
 * Opens the manager, applies `fonts.autoFetch` from `settings.json` and keeps watching it
 * (owner answer 1: fetching is automatic unless switched off; `FIN_FONTS_OFFLINE=1` keeps it
 * off whatever the file says).
 */
export function makeFonts(serverData: string, { settingsPollMs = 1000 }: FontsOptions = {}): Effect.Effect<FontsShape, never, Scope.Scope> {
  return Effect.gen(function* () {
    const manager = FontManager.open(serverData);
    // The last good settings, kept while the file is unusable (see `readSettings`).
    let last: Settings | undefined;
    const apply = () => {
      const { settings, error } = readSettings(serverData, last);
      if (error === undefined) last = settings;
      else {
        const kept = last === undefined ? "auto-fetch off until it is fixed" : "keeping the last good settings";
        process.stderr.write(`finisherd: warning: settings.json ignored (${kept}): ${error}\n`);
      }
      manager.setAutoFetch(settings.fonts.autoFetch);
    };
    apply();
    const path = settingsPath(serverData);
    yield* Effect.acquireRelease(
      Effect.sync(() => watchFile(path, { interval: settingsPollMs, persistent: false }, apply)),
      () => Effect.sync(() => unwatchFile(path, apply)),
    );
    return {
      catalog: () => attempt(async () => JSON.parse(manager.catalog()) as FontCatalog),
      status: () => attemptJson<FontsStatus>(() => manager.status()),
      fetch: (req) => attemptJson<FontsStatus>(() => manager.fetch(JSON.stringify(req))),
      setExtraCatalogs: (packs) => Effect.map(attempt(async () => JSON.parse(manager.setExtraCatalogs(JSON.stringify(packs))) as { rejected: PackRejected[] }), (r) => r.rejected),
      autoFetch: () => manager.autoFetch,
    } satisfies FontsShape;
  });
}

export function layerFonts(serverData: string, options?: FontsOptions): Layer.Layer<Fonts> {
  return Layer.effect(Fonts, makeFonts(serverData, options));
}
