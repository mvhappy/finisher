// `finisherd library add|rm|set` (plan 5D Task 14; spec §4.7, §7): the owner's libraries. Every
// one changes what the server indexes or trusts (a root to read, the fields only a person
// decides, a catalog to delete), so each needs a terminal and the typed code, like
// `agent mint`. Reading libraries is `fin library ls|status|files|show|search`.
//
// - `library add <dir> [--name <n>] [--storage local|network|sync] [--include <glob>]…
//   [--exclude <glob>]… [--kinds all|<kind>,…] [--poll demand|<s>] [--full daily|weekly|manual]
//   [--hydrate] [--bandwidth <MiB/s>] [--alias <path>]…`: adds the root and starts its first
//   full scan; the job's id is printed to wait on (`fin wait <job>`).
// - `library rm <lib>`: forgets the library and deletes its catalog; the files under the root
//   are never touched.
// - `library set <lib> …`: `--hydrate on|off`, `--bandwidth <MiB/s>|off`, `--alias <path>`… or
//   `--no-aliases`, `--accept-volume`, `--accept-unmounted <dir>`, `--accept-empty-root`.
//   `fin library set` changes the other fields (`--name`, `--include`, …).
import { resolve } from "node:path";
import type { LibraryAddResult, LibraryViewResult } from "@finisher/contracts";
import { LIBRARY_STORAGES } from "@finisher/contracts";
import { confirmed, requireTty } from "../confirm.ts";
import { all, has, one, onlyFlags, usage } from "../format.ts";
import { safe } from "../safe.ts";
import { type CommandContext, withConnection } from "./context.ts";

export async function library(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  if (sub === "add") return add(ctx, rest);
  if (sub === "rm" || sub === "remove") return rm(ctx, rest);
  if (sub === "set") return set(ctx, rest);
  return usage(
    sub === undefined ? "library needs a command: add, rm or set" : `unknown library command: ${sub}`,
    "Run `finisherd help` to list the commands; `fin library ls` lists the libraries.",
  );
}

const FULLS = ["daily", "weekly", "manual"] as const;

/** `--poll demand` or a whole number of seconds. */
function pollOf(value: string): "demand" | number {
  if (value === "demand") return "demand";
  const n = Number(value);
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(n) || n < 1) usage("--poll needs demand or a whole number of seconds");
  return n;
}

/** `--bandwidth <MiB/s>`: a positive number. */
function bandwidthOf(value: string): number {
  const n = Number(value);
  if (!Number.isFinite(n) || n <= 0) usage("--bandwidth needs a number of MiB per second above 0 (set also takes off)");
  return n;
}

/** `--hydrate` alone is on; `--hydrate on|off` says it. */
function hydrateOf(value: string | undefined): boolean {
  if (value === "" || value === "on") return true;
  if (value === "off") return false;
  return usage("--hydrate takes on or off");
}

function libraryArg(command: string, rest: ReadonlyArray<string>): string {
  const [lib, ...more] = rest;
  if (lib === undefined || more.length > 0) usage(`${command} takes one library, by its id or name`);
  return lib;
}

async function add(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "library add", ["name", "storage", "include", "exclude", "kinds", "poll", "full", "hydrate", "bandwidth", "alias"]);
  const [dir, ...more] = rest;
  if (dir === undefined || more.length > 0) usage("library add takes one folder: the library's root");
  const storage = one(ctx.args, "storage");
  if (storage !== undefined && !(LIBRARY_STORAGES as ReadonlyArray<string>).includes(storage)) usage(`--storage takes ${LIBRARY_STORAGES.join(", ")}`);
  const full = one(ctx.args, "full");
  if (full !== undefined && !(FULLS as ReadonlyArray<string>).includes(full)) usage(`--full takes ${FULLS.join(", ")}`);
  const poll = one(ctx.args, "poll");
  const scan = { ...(poll === undefined ? {} : { poll: pollOf(poll) }), ...(full === undefined ? {} : { full }) };
  const kinds = one(ctx.args, "kinds");
  const bandwidth = one(ctx.args, "bandwidth");
  const include = all(ctx.args, "include");
  const exclude = all(ctx.args, "exclude");
  const aliases = all(ctx.args, "alias");
  const hydrate = has(ctx.args, "hydrate") ? hydrateOf(one(ctx.args, "hydrate")) : undefined;
  const name = one(ctx.args, "name");
  const root = resolve(dir);
  const payload = {
    root,
    ...(name === undefined ? {} : { name }),
    ...(storage === undefined ? {} : { storage }),
    ...(include.length === 0 ? {} : { include }),
    ...(exclude.length === 0 ? {} : { exclude }),
    ...(kinds === undefined ? {} : { kinds: kinds === "all" ? "all" : kinds.split(",").filter((k) => k !== "") }),
    ...(Object.keys(scan).length === 0 ? {} : { scan }),
    ...(hydrate === undefined ? {} : { hydrate }),
    ...(bandwidth === undefined ? {} : { bandwidthMiBs: bandwidthOf(bandwidth) }),
    ...(aliases.length === 0 ? {} : { aliases }),
  };
  requireTty(ctx.prompt, "library add");
  const added = await withConnection(ctx, (c) =>
    confirmed<LibraryAddResult>(
      c,
      ctx.prompt,
      `This adds ${safe(root)} as a library and starts indexing it: Finisher reads the names, sizes and times of everything under it, and the media files' contents to tell what they are` +
        `${hydrate === true ? ", including files a sync service keeps in the cloud (they are downloaded to be read)" : ", but never downloads files a sync service keeps in the cloud"}. Nothing under it is ever changed.`,
      "library.add",
      payload,
    ),
  );
  const lib = added.library;
  ctx.output.out(`Added library ${safe(lib.name)} (${lib.id}), ${lib.storage} storage, root ${safe(lib.root)}.`);
  if (added.job === undefined) ctx.output.out("Its first scan could not be queued yet; the library scans again on its own.");
  else ctx.output.out(`Its first scan is job ${added.job.id}: \`fin wait ${added.job.id}\` waits for it, \`fin library status ${lib.id}\` shows how it goes.`);
  return 0;
}

async function rm(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "library rm", []);
  const lib = libraryArg("library rm", rest);
  requireTty(ctx.prompt, "library rm");
  await withConnection(ctx, (c) =>
    confirmed<object>(
      c,
      ctx.prompt,
      `This removes the library ${safe(lib)}: its jobs are cancelled and its catalog is deleted. The files under its root are never touched; adding the root again indexes it from scratch.`,
      "library.remove",
      { id: lib },
    ),
  );
  ctx.output.out(`Removed library ${safe(lib)}.`);
  return 0;
}

async function set(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "library set", ["hydrate", "bandwidth", "alias", "no-aliases", "accept-volume", "accept-unmounted", "accept-empty-root"]);
  const lib = libraryArg("library set", rest);
  const aliases = all(ctx.args, "alias");
  if (aliases.length > 0 && has(ctx.args, "no-aliases")) usage("library set takes --alias <path>… or --no-aliases, not both");
  const bandwidth = one(ctx.args, "bandwidth");
  const unmounted = one(ctx.args, "accept-unmounted");
  const hydrate = has(ctx.args, "hydrate") ? hydrateOf(one(ctx.args, "hydrate")) : undefined;
  const changes: string[] = [];
  const payload: Record<string, unknown> = { id: lib };
  if (hydrate !== undefined) {
    payload.hydrate = hydrate;
    changes.push(hydrate ? "lets its scans download files a sync service keeps in the cloud, to read them" : "stops its scans downloading files a sync service keeps in the cloud");
  }
  if (bandwidth !== undefined) {
    payload.bandwidthMiBs = bandwidth === "off" ? null : bandwidthOf(bandwidth);
    changes.push(bandwidth === "off" ? "removes its read bandwidth limit" : `limits its reads to ${bandwidthOf(bandwidth)} MiB per second`);
  }
  if (aliases.length > 0 || has(ctx.args, "no-aliases")) {
    payload.aliases = aliases;
    changes.push(aliases.length === 0 ? "forgets its aliases (the paths other machines know the root by)" : `sets its aliases to ${aliases.map(safe).join(", ")} (the paths other machines know the root by)`);
  }
  if (has(ctx.args, "accept-volume")) {
    payload.acceptVolume = true;
    changes.push("accepts the volume now under its root as the library's (the folder moved or was mounted from somewhere else)");
  }
  if (unmounted !== undefined) {
    payload.acceptUnmounted = { dir: unmounted };
    changes.push(`accepts that the nested mount at ${safe(unmounted)} is gone for good: what is there now is what the library holds (files it listed from the mount go missing)`);
  }
  if (has(ctx.args, "accept-empty-root")) {
    payload.acceptEmptyRoot = true;
    changes.push("accepts that its root is empty for good: the library comes back online and, once a later scan sees the root empty again, its files go missing");
  }
  if (changes.length === 0) {
    usage("library set needs something to change: --hydrate, --bandwidth, --alias, --no-aliases, --accept-volume, --accept-unmounted or --accept-empty-root", "`fin library set` changes the library's name, filters, kinds, scan schedule and pause.");
  }
  requireTty(ctx.prompt, "library set");
  const done = await withConnection(ctx, (c) =>
    confirmed<{ library: LibraryViewResult; unmounted?: number }>(c, ctx.prompt, `This ${changes.join(", and ")}, for the library ${safe(lib)}.`, "library.set", payload),
  );
  ctx.output.out(`Updated library ${safe(done.library.name)} (${done.library.id}), now ${done.library.state}.`);
  if (done.unmounted !== undefined) {
    ctx.output.out(done.unmounted === 0 ? "No nested mount was offline there, so nothing was forgotten." : `Forgot ${done.unmounted} nested mount point${done.unmounted === 1 ? "" : "s"}.`);
  }
  return 0;
}
