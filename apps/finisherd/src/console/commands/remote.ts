// `finisherd remote enable [--listen <host:port>] [--tls self-signed | --cert <file> --key
// <file>] [--public-url <url>]`, `remote disable` and `remote status [--json]` (plan 5B Task 12,
// spec §5.3): remote access is off until the owner turns it on here (`settings.write
// server.remote`, y/N). `enable` checks the setting before anything is asked: a listener off this
// computer without TLS is `LISTEN_NEEDS_TLS`. It says first that the server opens to the
// network, and for a self-signed certificate that browsers need trusted files or an SSH tunnel on
// the same port. With `FIN_LISTEN` set the server follows the environment, not the file.
import { resolve } from "node:path";
import { consoleError, valueOf } from "../client.ts";
import { confirmed, proceed, requireTty } from "../confirm.ts";
import { has, one, onlyFlags, pinGroups, usage } from "../format.ts";
import { safe } from "../safe.ts";
import { checkSetting, DEFAULT_LISTEN, parseListen, readServerSettings, type RemoteSettings } from "../../net/server-settings.ts";
import { type CommandContext, withConnection } from "./context.ts";

interface Listener {
  transport: string;
  endpoint: string;
  spki?: string;
  mode?: "selfSigned" | "files";
  source?: "env" | "settings" | "test";
}

export async function remote(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  if (rest.length > 0) usage(`remote ${sub ?? ""} takes no further arguments`);
  if (sub === "enable") return enable(ctx);
  if (sub === "disable") return disable(ctx);
  if (sub === "status") return status(ctx);
  return usage(sub === undefined ? "remote needs enable, disable or status" : `unknown remote command: ${sub}`);
}

/** The setting `remote enable`'s flags ask for, keeping the `origins` and `hosts` already set. */
function enabledSetting(ctx: CommandContext): RemoteSettings {
  const tls = one(ctx.args, "tls");
  const cert = one(ctx.args, "cert");
  const key = one(ctx.args, "key");
  if (tls !== undefined && tls !== "self-signed") usage("--tls takes self-signed (or give --cert and --key)");
  if (tls !== undefined && (cert !== undefined || key !== undefined)) usage("--tls self-signed and --cert/--key are either-or");
  if ((cert === undefined) !== (key === undefined)) usage("--cert and --key go together");
  const before = readServerSettings(ctx.serverData).remote;
  const listen = one(ctx.args, "listen") ?? before?.listen;
  const publicUrl = one(ctx.args, "public-url") ?? before?.publicUrl;
  const value = {
    enabled: true,
    ...(listen === undefined ? {} : { listen }),
    ...(cert !== undefined && key !== undefined
      ? { tls: { mode: "files" as const, certFile: resolve(cert), keyFile: resolve(key) } }
      : tls !== undefined
        ? { tls: { mode: "selfSigned" as const } }
        : before?.tls === undefined
          ? {}
          : { tls: before.tls }),
    ...(publicUrl === undefined ? {} : { publicUrl }),
    ...(before?.origins === undefined ? {} : { origins: before.origins }),
    ...(before?.hosts === undefined ? {} : { hosts: before.hosts }),
  };
  try {
    return checkSetting("server.remote", value) as RemoteSettings;
  } catch (e) {
    const f = e as { code?: string; message?: string; details?: Record<string, unknown>; fix?: string };
    if (f.code === "LISTEN_NEEDS_TLS") {
      throw consoleError("LISTEN_NEEDS_TLS", "a listener off this computer needs TLS", f.details ?? {}, "Add --tls self-signed, or --cert <file> --key <file> for your own certificate.");
    }
    throw consoleError(f.code ?? "SETTINGS_INVALID", f.message ?? String(e), f.details ?? {}, f.fix);
  }
}

async function enable(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "remote enable", ["listen", "tls", "cert", "key", "public-url"]);
  const value = enabledSetting(ctx);
  requireTty(ctx.prompt, "remote enable");
  const listen = value.listen ?? DEFAULT_LISTEN;
  const port = (parseListen(listen) as { port: number }).port;
  const lines = [`This opens the server to the network on ${listen}, over TLS: devices you pair reach it there.`];
  if ((value.tls?.mode ?? "selfSigned") === "selfSigned") {
    lines.push(
      "Its certificate is self-signed: the Finisher app checks it by its pin.",
      `Browsers need trusted certificate files (--cert, --key), or an SSH tunnel on the same port (ssh -L ${port}:127.0.0.1:${port} <this server>).`,
    );
  }
  await withConnection(ctx, (c) => confirmed(c, ctx.prompt, lines.join("\n"), proceed, "settings.write", { path: "server.remote", value }));
  return status(ctx, true);
}

async function disable(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "remote disable", []);
  const before = readServerSettings(ctx.serverData).remote;
  requireTty(ctx.prompt, "remote disable");
  await withConnection(ctx, (c) =>
    confirmed(
      c,
      ctx.prompt,
      "This closes the remote listener: paired devices cannot reach the server until it is enabled again. They stay paired.",
      proceed,
      "settings.write",
      { path: "server.remote", value: { ...(before ?? {}), enabled: false } },
    ),
  );
  // What runs now: with `FIN_LISTEN` the server follows the environment, not the file.
  return status(ctx, true);
}

async function status(ctx: CommandContext, quiet = false): Promise<number> {
  if (!quiet) onlyFlags(ctx.args, "remote status", ["json"]);
  const settings = readServerSettings(ctx.serverData).remote;
  const listeners = await withConnection(ctx, async (c) => valueOf<{ listeners: Listener[] }>(await c.call("server.status", {})).listeners);
  const wss = listeners.find((l) => l.transport === "wss");
  if (!quiet && has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify({ enabled: wss !== undefined, ...(wss === undefined ? {} : { listener: wss }), ...(settings === undefined ? {} : { settings }) }));
    return 0;
  }
  if (wss === undefined) {
    ctx.output.out(settings?.enabled === true ? "Remote access is enabled but not running: finisherd status says why." : "Remote access is off.");
    return 0;
  }
  if (wss.source === "env" && quiet) {
    ctx.output.out(`Remote access follows FIN_LISTEN, not the settings: ${safe(wss.endpoint)} stays on until the server runs without it.`);
    return 0;
  }
  ctx.output.out(`Remote access is on: ${safe(wss.endpoint)} (${wss.mode === "files" ? "your certificate files" : "self-signed"}, from ${wss.source ?? "settings"}).`);
  if (wss.spki !== undefined) ctx.output.out(`Pin: ${pinGroups(wss.spki)}`);
  return 0;
}
