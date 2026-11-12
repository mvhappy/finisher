// Helpers of the "plugins" e2e scenario: the publisher's side (`fin-plugin`: packs, keys and a
// local-folder registry, once as a plain folder and once as a git clone) and the server's
// audit log. Nothing here touches the network.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { FIN_PLUGIN, log, socketCall } from "./lib.mjs";

/** A commit-shaped path segment: catalog source URLs must be immutable. */
const COMMIT = "0123456789abcdef0123456789abcdef01234567";

/** Runs `fin-plugin`; `{status, json}` (stdout, or the error JSON on stderr when it failed). */
export function finPlugin(args, cwd) {
  const out = spawnSync(FIN_PLUGIN, args, { cwd, encoding: "utf8", timeout: 60_000 });
  if (out.error) throw out.error;
  const text = (out.status === 0 ? out.stdout : out.stderr).trim();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    json = undefined;
  }
  log(`fin-plugin ${args[0]} ${args[1] ?? ""} (exit ${out.status}) → ${text.length > 120 ? `${text.slice(0, 120)}…` : text}`);
  return { status: out.status, json, text };
}

/** `fin-plugin`, which must succeed. */
export function mustPlugin(args, cwd) {
  const r = finPlugin(args, cwd);
  if (r.status !== 0) throw new Error(`fin-plugin ${args.join(" ")} exited ${r.status}: ${r.text}`);
  return r.json;
}

const STYLE = {
  name: "Default",
  text: { family: "Inter", weight: 700, italic: false, size: 64.0, lineHeight: 1.2, letterSpacing: 0.0, align: "center" },
  color: "#ffffff",
  position: { anchor: "bottom", offset: 0.0 },
  safeZone: "generic-9x16",
  maxChars: 32,
  maxLines: 2,
  gap: "1/2",
};

/**
 * A font catalog of one entry `id` under its own family (a pack may not reuse a family the
 * built-in catalog or another pack has), over bytes derived from the id. `family` overrides the
 * face's family, to try a pack that shadows a built-in one.
 */
export function fontCatalog(id, family = `E2E ${id}`) {
  const sha256 = createHash("sha256").update(`e2e font ${id}`).digest("hex");
  return {
    catalogVersion: "2026.11.1",
    fallback: [],
    entries: [
      {
        id,
        family: `E2E ${id}`,
        licence: { spdx: "OFL-1.1", url: `https://example.com/${COMMIT}/OFL.txt` },
        homepage: "https://example.com/",
        scripts: ["Latn"],
        styles: [
          {
            id: "regular",
            weight: 400,
            italic: false,
            file: { sha256, size: 4096, format: "ttf", faces: [{ index: 0, family, weight: 400, italic: false, scripts: ["Latn"] }] },
            sources: [{ url: `https://raw.githubusercontent.com/example/fonts/${COMMIT}/${id}.ttf` }],
          },
        ],
      },
    ],
  };
}

const PLATFORMS = ["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64", "win32-x64", "win32-arm64"];

function writeJson(path, value) {
  mkdirSync(join(path, ".."), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

/**
 * Writes the plugin source `spec` = `{id, version, kind, styles?, fonts?, permissions?}` under
 * `<work>/src-<id>-<version>`; returns `{src, out}`, the folder and where its `.finpkg` goes.
 */
export function writePlugin(work, spec) {
  const src = join(work, `src-${spec.id.replace(/[@/]/g, "_")}-${spec.version}`);
  const manifest = {
    manifestVersion: 1,
    id: spec.id,
    version: spec.version,
    kind: spec.kind,
    hostApi: "^1.0",
    license: spec.license ?? "MIT",
    publisher: { name: spec.id.slice(1).split("/")[0] },
  };
  if (spec.permissions) manifest.permissions = spec.permissions;
  if (spec.kind === "code") {
    manifest.platforms = PLATFORMS;
    manifest.runtime = { type: "node", main: "dist/plugin.mjs", node: ">=24.0" };
    mkdirSync(join(src, "dist"), { recursive: true });
    writeFileSync(join(src, "dist", "plugin.mjs"), `// ${spec.id} ${spec.version}\nexport {};\n`);
  } else {
    manifest.contributes = {};
    for (const name of spec.styles ?? []) {
      (manifest.contributes.captionStyles ??= []).push({ name, file: `styles/${name}.json` });
      writeJson(join(src, "styles", `${name}.json`), { ...STYLE, name: `${name} ${spec.version}` });
    }
    for (const [i, c] of (spec.fonts ?? []).entries()) {
      (manifest.contributes.fontCatalogs ??= []).push({ file: `fonts/${i}.json` });
      writeJson(join(src, "fonts", `${i}.json`), c);
    }
  }
  writeJson(join(src, "finisher-plugin.json"), manifest);
  return { src, out: join(work, `${spec.id.replace(/[@/]/g, "_")}-${spec.version}.finpkg`) };
}

/** Writes and packs `spec`; returns the `.finpkg` path. */
export function packPlugin(work, spec) {
  const { src, out } = writePlugin(work, spec);
  mustPlugin(["pack", src, "-o", out]);
  return out;
}

/**
 * A local-folder registry on `dir` with `rootKeys` root keys (all of which must sign: the
 * threshold equals their number) and `publishers` delegated; keys live in `<work>/keys-<name>`.
 */
export function makeRegistry(work, name, dir, publishers, { rootKeys = 1 } = {}) {
  const keys = join(work, `keys-${name}`);
  const key = (k) => join(work, `${name}-${k}.pem`);
  mkdirSync(keys, { recursive: true, mode: 0o700 }); // fin-plugin refuses a key folder others can read
  const roots = Array.from({ length: rootKeys }, (_, i) => key(i === 0 ? "root" : `root${i + 1}`));
  for (const r of roots) mustPlugin(["keygen", r]);
  for (const p of publishers) mustPlugin(["keygen", key(p), "--public", key(`${p}.pub`)]);
  const init = mustPlugin(["registry", "init", dir, "--keys", keys, ...roots.flatMap((r) => ["--root-keys", r]), "--threshold", String(rootKeys)]);
  for (const p of publishers) mustPlugin(["registry", "delegate", dir, "--keys", keys, "--publisher", p, "--key", key(`${p}.pub`)]);
  return {
    dir,
    keys,
    /** A key file of this registry (`root`, `acme`, …). */
    key,
    rootSha256: init.root.sha256,
    /** Packs and publishes `spec` as its publisher (the id's first part). */
    publish(spec) {
      const pkg = packPlugin(work, spec);
      return mustPlugin(["registry", "publish", dir, pkg, "--keys", keys, "--key", key(spec.id.slice(1).split("/")[0])]);
    },
    /** Packs the plugin folder `src` (its manifest's `id`) and publishes it as its publisher. */
    publishDir(src, id) {
      const out = join(work, `${id.replace(/[@/]/g, "_")}.finpkg`);
      mustPlugin(["pack", src, "-o", out]);
      return mustPlugin(["registry", "publish", dir, out, "--keys", keys, "--key", key(id.slice(1).split("/")[0])]);
    },
    /** A signed advisory by the plugin's publisher. */
    advise(args) {
      const publisher = args.plugin.slice(1).split("/")[0];
      return finPlugin(["registry", "advise", dir, "--keys", keys, "--plugin", args.plugin, "--versions", args.versions, "--action", args.action, "--summary", args.summary, "--id", args.id, "--key", key(publisher)]);
    },
    /** A fresh timestamp: the registry's metadata is as fresh as the owner's last signing. */
    resign() {
      return mustPlugin(["registry", "resign-timestamp", dir, "--keys", keys]);
    },
  };
}

/** `git` in `dir` (a local clone: no network, no hooks, no signing, a fixed identity). */
export function git(dir, ...args) {
  const out = spawnSync("git", ["-c", "user.name=e2e", "-c", "user.email=e2e@example.invalid", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args], { cwd: dir, encoding: "utf8" });
  if (out.status !== 0) throw new Error(`git ${args.join(" ")} exited ${out.status}: ${out.stderr}`);
  return out.stdout.trim();
}

/** The audit rows of `kind` (every kind without it), newest first, as the console reads them. */
export async function auditRows(server, kind) {
  const rows = [];
  let cursor;
  for (;;) {
    const page = await socketCall(server, "auth.audit.list", { ...(kind ? { kind } : {}), limit: 1000, ...(cursor ? { cursor } : {}) }, "console");
    rows.push(...page.entries);
    if (page.next === undefined) return rows;
    cursor = page.next;
  }
}
