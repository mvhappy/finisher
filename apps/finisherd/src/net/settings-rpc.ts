// `settings.write {path, value}` (5B Decision 27; `settings.admin`, human-only, confirmed on the
// console): the value is checked against its section's schema (`SETTINGS_INVALID`,
// `LISTEN_NEEDS_TLS`; a files-mode pair is checked before anything is written,
// `TLS_FILES_INVALID`), written into `settings.json`'s `server` section keeping every other key
// (`patchSection`: a file that does not parse is never overwritten), audited `settings.write
// {path}` (never the value), then applied live: the remote listener starts, restarts or stops;
// `server.origins` is read as the file changes; `server.projectRoots` is read by each path
// check; `server.loopback` takes effect at the next start. With `FIN_LISTEN` set, the remote
// listener follows the environment, not the file.
import type { FinError, SettingsPath } from "@finisher/contracts";
import { Effect } from "effect";
import { chainOf } from "../chain/chain.ts";
import { readFileSync } from "node:fs";
import { ServerStartError, settingsPath } from "../config.ts";
import type { Audit } from "../control/audit.ts";
import { finError, toFinError } from "../errors.ts";
import { patchSection } from "../settings-write.ts";
import { filesPair, type RemoteListener } from "./remote.ts";
import { checkSetting, readServerSettings, type RemoteSettings, remoteConfigOf } from "./server-settings.ts";

type Guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => Effect.Effect<A, FinError, R>;

const KEYS: Record<SettingsPath, string> = {
  "server.remote": "remote",
  "server.projectRoots": "projectRoots",
  "server.origins": "origins",
  "server.loopback": "loopback",
};

/** The `server` section's `key` now, as stored. */
function readSection(serverData: string, key: string): unknown {
  try {
    return (JSON.parse(readFileSync(settingsPath(serverData), "utf8")) as { server?: Record<string, unknown> })?.server?.[key];
  } catch {
    return undefined;
  }
}

const asFinError = (e: unknown): FinError =>
  e instanceof ServerStartError ? finError(e.code, e.message, e.details, e.fix) : toFinError(e);

/** Writes `path` (checked) into `settings.json` and audits it; the stored value. */
export function patchSettings(serverData: string, path: SettingsPath, value: unknown): unknown {
  const checked = checkSetting(path, value);
  if (path === "server.remote") {
    const r = checked as RemoteSettings;
    if (r.enabled && r.tls?.mode === "files") filesPair(r.tls.certFile, r.tls.keyFile);
  }
  patchSection(serverData, "server", { [KEYS[path]]: checked });
  return checked;
}

export function settingsHandlers(d: { readonly guard: Guard; readonly serverData: string; readonly remote: RemoteListener; readonly audit: Audit; readonly env?: NodeJS.ProcessEnv }) {
  return {
    "settings.write": ({ path, value }: { path: SettingsPath; value: unknown }, options: object) =>
      d.guard(
        "settings.write",
        Effect.gen(function* () {
          // A device cannot widen its own reach (review of Task 11): its roots, or TLS files it
          // would have the server read. The owner sets those from the console.
          if (chainOf(options)?.principal.kind === "device") {
            const files = path === "server.remote" && (value as { tls?: { mode?: unknown } } | null)?.tls?.mode === "files";
            if (path === "server.projectRoots" || files) {
              return yield* Effect.fail(
                finError("PATH_NOT_ALLOWED", "a device cannot change the server's roots or certificate files", { reason: "deviceSettings", path }, "Change it from the console on the server: finisherd console."),
              );
            }
          }
          const before = readSection(d.serverData, KEYS[path]);
          yield* Effect.try({ try: () => patchSettings(d.serverData, path, value), catch: asFinError });
          const env = d.env ?? process.env;
          if (path === "server.remote" && (env.FIN_LISTEN ?? "") === "") {
            const previous = d.remote.config();
            const config = remoteConfigOf(readServerSettings(d.serverData), env);
            const applied = config === undefined ? Effect.as(d.remote.stop(), undefined) : Effect.mapError(d.remote.start(config), asFinError);
            const failed = yield* Effect.flip(Effect.as(applied, undefined)).pipe(Effect.option);
            if (failed._tag === "Some") {
              // Not applied: the file and the listener go back to what they were.
              patchSection(d.serverData, "server", { [KEYS[path]]: before ?? null });
              if (previous !== undefined) yield* Effect.ignore(d.remote.start(previous));
              return yield* Effect.fail(failed.value);
            }
          }
          const by = chainOf(options)?.principal;
          d.audit.append({ kind: "settings.write", principal: { kind: by?.kind ?? "anonymous", id: by?.id ?? "" }, outcome: "ok", details: { path } });
          return { path };
        }),
      ),
  };
}
