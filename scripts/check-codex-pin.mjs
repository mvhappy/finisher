// The pinned Codex binary's licence gate (sub-project 7E, Task 2). `vendor/codex/pin.json` may
// keep only `bin/codex` and `codex-path/rg` (`.exe` on Windows): never the platform package's
// `codex-resources/bwrap` (LGPL-2.0-or-later, spec §17), its code-mode host or anything else.
// Every target has a sha512 integrity, the committed snapshot lives in `vendor/codex/<version>/`
// (the pin's version, and no other) with its schema bundles, `features.txt`, `LICENSE`
// (Apache-2.0) and `NOTICE`, and no file called `bwrap*` is in the repository's tree. Each
// finding is `CODEX_PIN <message>`.
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { EXPERIMENTAL_FILES, TARGETS } from "./codex-pin.mjs";
import { report, rootArg, walk } from "./walk.mjs";

const ALLOWED = /^(?:bin\/codex|codex-path\/rg)(?:\.exe)?$/;

export function findings(root) {
  const out = [];
  const bad = (m) => out.push(`CODEX_PIN ${m}`);
  for (const p of walk(root)) if (/(^|\/)bwrap[^/]*$/.test(p)) bad(`${p}: bwrap (LGPL) must never be in the repository`);
  const pinPath = join(root, "vendor/codex/pin.json");
  if (!existsSync(pinPath)) return [...out, "CODEX_PIN vendor/codex/pin.json is missing"];
  let pin;
  try {
    pin = JSON.parse(readFileSync(pinPath, "utf8"));
  } catch (e) {
    return [...out, `CODEX_PIN pin.json is not JSON: ${e.message}`];
  }
  if (!/^\d+\.\d+\.\d+$/.test(pin.version ?? "")) bad(`pin.json version ${JSON.stringify(pin.version)} is not x.y.z`);
  for (const t of TARGETS) {
    const e = pin.targets?.[t];
    if (!e) {
      bad(`pin.json has no target ${t}`);
      continue;
    }
    if (!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(e.integrity ?? "")) bad(`${t}: integrity must be a sha512 SRI string`);
    if (!Array.isArray(e.kept) || e.kept.length === 0) bad(`${t}: kept is empty`);
    for (const k of e.kept ?? []) {
      if (/bwrap/i.test(k)) bad(`${t}: kept names bwrap (LGPL): ${k}`);
      else if (!ALLOWED.test(k)) bad(`${t}: kept names ${k}, only bin/codex and codex-path/rg are Apache-2.0 files Finisher ships`);
      else if (t.startsWith("win32") !== k.endsWith(".exe")) bad(`${t}: kept ${k} has the wrong extension for the target`);
    }
  }
  for (const t of Object.keys(pin.targets ?? {})) if (!TARGETS.includes(t)) bad(`pin.json names an unknown target ${t}`);
  const base = join(root, "vendor/codex");
  for (const d of readdirSync(base, { withFileTypes: true }))
    if (d.isDirectory() && d.name !== pin.version) bad(`vendor/codex/${d.name} is not the pinned version ${pin.version}`);
  const snap = join(base, String(pin.version));
  const need = [
    "schema/codex_app_server_protocol.schemas.json",
    ...EXPERIMENTAL_FILES.map((f) => `schema-experimental/${f}`),
    "features.txt",
    "LICENSE",
    "NOTICE",
  ];
  for (const f of need) if (!existsSync(join(snap, f))) bad(`vendor/codex/${pin.version}/${f} is missing`);
  const lic = join(snap, "LICENSE");
  if (existsSync(lic) && !/Apache License/.test(readFileSync(lic, "utf8"))) bad("LICENSE is not the Apache License");
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) report(findings(rootArg()));
