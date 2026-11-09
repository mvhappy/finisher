// The `server.*` sections of `settings.json` (5B Decisions 26–28): read with their schemas at
// start (an invalid section is ignored, with a `server.status` warning, never a crash) and
// checked the same way by `settings.write` before anything is written.
//   server.remote       {enabled, listen?, tls?: {mode: selfSigned} | {mode: files, certFile, keyFile},
//                        publicUrl?, origins?, hosts?}
//   server.projectRoots [absolute paths]   (default [<home>/Finisher]; Task 11 reads them)
//   server.origins      [origins]          (5A)
//   server.loopback     {port?}            (a fixed loopback port, read at start)
// `FIN_LISTEN=<host:port>` runs the remote listener whatever the file says (Docker), with
// `FIN_TLS_CERT` and `FIN_TLS_KEY` naming certificate files (else self-signed).
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";
import type { SettingsPath } from "@finisher/contracts";
import { Schema } from "effect";
import { settingsPath } from "../config.ts";
import { finError } from "../errors.ts";

export const DEFAULT_LISTEN = "0.0.0.0:7311";

const Tls = Schema.Union([
  Schema.Struct({ mode: Schema.Literal("selfSigned") }),
  Schema.Struct({ mode: Schema.Literal("files"), certFile: Schema.String, keyFile: Schema.String }),
]);
const Remote = Schema.Struct({
  enabled: Schema.Boolean,
  listen: Schema.optional(Schema.String),
  tls: Schema.optional(Tls),
  publicUrl: Schema.optional(Schema.String),
  origins: Schema.optional(Schema.Array(Schema.String)),
  hosts: Schema.optional(Schema.Array(Schema.String)),
});
const Paths = Schema.Array(Schema.String);
const Loopback = Schema.Struct({ port: Schema.optional(Schema.Number) });

export type TlsConfig = typeof Tls.Type;
export type RemoteSettings = typeof Remote.Type;

/** What the remote listener runs with. */
export interface RemoteConfig {
  readonly listen: string;
  readonly tls: TlsConfig;
  readonly publicUrl?: string | undefined;
  readonly origins?: ReadonlyArray<string> | undefined;
  readonly hosts?: ReadonlyArray<string> | undefined;
  readonly source: "env" | "settings" | "test";
}

export interface ServerSettings {
  readonly remote?: RemoteSettings;
  readonly projectRoots: ReadonlyArray<string>;
  readonly loopbackPort?: number;
  /** Sections ignored because they do not fit their schema (`server.status` warns with the code). */
  readonly invalid: ReadonlyArray<{ readonly key: SettingsPath; readonly code: string }>;
}

const invalidValue = (path: string, why: string) =>
  finError("SETTINGS_INVALID", `${path} is not a valid setting: ${why}`, { key: path, reason: why }, `Change ${path} as the message says and try again.`);

/** `host:port` (IPv6 in brackets) → its parts, or a reason it is not one. */
export function parseListen(listen: string): { host: string; port: number } | string {
  const m = /^(\[[0-9A-Fa-f:.]+\]|[^:[\]\s]+):(\d{1,5})$/.exec(listen.trim());
  if (m === null) return "it is not host:port";
  const port = Number(m[2]);
  if (port > 65535) return "the port is past 65535";
  return { host: m[1]!.replace(/^\[|\]$/g, ""), port };
}

/** Whether a listen host is loopback only (TLS is then not required). */
export const isLoopbackHost = (h: string) => h === "127.0.0.1" || h === "::1" || h === "localhost" || /^127\./.test(h);

/** Checks `value` against `path`'s schema and rules: the value as stored, or `SETTINGS_INVALID` / `LISTEN_NEEDS_TLS`. */
export function checkSetting(path: SettingsPath, value: unknown): unknown {
  const decode = <A>(schema: Schema.Codec<A>) => {
    const r = Schema.decodeUnknownExit(schema)(value, { onExcessProperty: "error" });
    if (r._tag !== "Success") throw invalidValue(path, "its shape does not fit");
    return r.value;
  };
  switch (path) {
    case "server.remote": {
      const r = decode(Remote);
      const listen = parseListen(r.listen ?? DEFAULT_LISTEN);
      if (typeof listen === "string") throw invalidValue(path, `listen: ${listen}`);
      if (r.enabled && r.tls === undefined && !isLoopbackHost(listen.host)) {
        throw finError(
          "LISTEN_NEEDS_TLS",
          "a listener off this computer needs TLS",
          { listen: "nonLoopback" },
          "Choose a self-signed certificate or your own certificate files for remote access.",
        );
      }
      if (r.tls?.mode === "files" && (!isAbsolute(r.tls.certFile) || !isAbsolute(r.tls.keyFile))) throw invalidValue(path, "certFile and keyFile are absolute paths");
      if (r.publicUrl !== undefined && !/^https:\/\//.test(r.publicUrl)) throw invalidValue(path, "publicUrl is an https:// URL");
      return r;
    }
    case "server.projectRoots": {
      const roots = decode(Paths);
      if (roots.some((p) => !isAbsolute(p))) throw invalidValue(path, "every root is an absolute path");
      return roots;
    }
    case "server.origins": {
      const origins = decode(Paths);
      for (const o of origins) {
        try {
          if (new URL(o).origin === "null") throw new Error("null");
        } catch {
          throw invalidValue(path, "every entry is an origin (scheme://host[:port])");
        }
      }
      return origins;
    }
    case "server.loopback": {
      const l = decode(Loopback);
      if (l.port !== undefined && (!Number.isInteger(l.port) || l.port < 1 || l.port > 65535)) throw invalidValue(path, "port is 1–65535");
      return l;
    }
  }
}

export const defaultProjectRoots = (home: string = homedir()) => [join(home, "Finisher")];

/** Reads the `server.*` sections; a section that does not fit is ignored and named in `invalid`. */
export function readServerSettings(serverData: string): ServerSettings {
  let server: Record<string, unknown> = {};
  try {
    const raw = JSON.parse(readFileSync(settingsPath(serverData), "utf8")) as { server?: unknown };
    if (raw?.server !== null && typeof raw?.server === "object" && !Array.isArray(raw.server)) server = raw.server as Record<string, unknown>;
  } catch {
    // No file, or one that does not parse (diagnostics warns about that already): defaults.
  }
  const invalid: Array<{ key: SettingsPath; code: string }> = [];
  const read = <A>(path: SettingsPath, key: string): A | undefined => {
    if (server[key] === undefined) return undefined;
    try {
      return checkSetting(path, server[key]) as A;
    } catch (e) {
      // A hand-edited listener off loopback without TLS is `LISTEN_NEEDS_TLS` (Decision 29).
      invalid.push({ key: path, code: (e as { code?: string }).code ?? "SETTINGS_INVALID" });
      return undefined;
    }
  };
  const remote = read<RemoteSettings>("server.remote", "remote");
  const roots = read<ReadonlyArray<string>>("server.projectRoots", "projectRoots");
  const loopback = read<{ port?: number }>("server.loopback", "loopback");
  read<ReadonlyArray<string>>("server.origins", "origins");
  return {
    ...(remote === undefined ? {} : { remote }),
    projectRoots: roots ?? defaultProjectRoots(),
    ...(loopback?.port === undefined ? {} : { loopbackPort: loopback.port }),
    invalid,
  };
}

/** The remote listener's config: `FIN_LISTEN` first, else enabled settings, else none. */
export function remoteConfigOf(settings: ServerSettings, env: NodeJS.ProcessEnv = process.env): RemoteConfig | undefined {
  const listen = env.FIN_LISTEN;
  if (listen !== undefined && listen !== "") {
    const cert = env.FIN_TLS_CERT;
    const key = env.FIN_TLS_KEY;
    // Either variable asks for files; one without the other fails as unreadable files.
    const tls: TlsConfig = cert || key ? { mode: "files", certFile: cert ?? "", keyFile: key ?? "" } : { mode: "selfSigned" };
    return { listen, tls, source: "env", publicUrl: settings.remote?.publicUrl, origins: settings.remote?.origins, hosts: settings.remote?.hosts };
  }
  const r = settings.remote;
  if (r === undefined || !r.enabled) return undefined;
  return { listen: r.listen ?? DEFAULT_LISTEN, tls: r.tls ?? { mode: "selfSigned" }, source: "settings", publicUrl: r.publicUrl, origins: r.origins, hosts: r.hosts };
}
