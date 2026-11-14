import { readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";
import { rootArg } from "./walk.mjs";

// The font catalog gate (4C Decision 31), the JS half of `FontCatalog::validate`: both read
// the rule list `crates/fin-fonts/catalog/rules.json` and run the cases in
// `crates/fin-fonts/tests/catalog_gate_cases.json`. Only permissive licences, immutable https
// sources on the listed hosts, sane hashes and sizes, unique ids and keys. Output: `FONT_CATALOG_INVALID: <rule>:
// <path>: <message>` per violation.

const RULES = JSON.parse(readFileSync(fileURLToPath(new URL("../crates/fin-fonts/catalog/rules.json", import.meta.url)), "utf8"));

const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const isStr = (v) => typeof v === "string";
const isInt = (v) => Number.isInteger(v) && v >= 0;
const present = (v) => v !== undefined && v !== null;
const SHA = /^[0-9a-f]{64}$/;
const COMMIT = /^[0-9a-f]{40}$/;
const ID = /^[a-z0-9][a-z0-9-]*$/;
const SCRIPT = /^[A-Z][a-z]{3}$/;
const VERSION = /^\d{4}\.\d{1,2}\.\d+$/;

/** The same rule as `fin_fonts::is_immutable_url`. */
export function isImmutableUrl(url) {
  const i = url.indexOf("://");
  if (i < 0) return false;
  const path = url.slice(i + 3).split(/[?#]/)[0];
  const segs = path.split("/").slice(1);
  const pinned = segs.some((s) => {
    if (COMMIT.test(s)) return true;
    const at = s.lastIndexOf("@");
    return at > 0 && COMMIT.test(s.slice(at + 1));
  });
  const release = segs.some((s, k) => s === "releases" && segs[k + 1] === "download" && segs[k + 2] && segs[k + 3]);
  return pinned || release;
}

/** JSON with sorted keys, to compare two declarations of one file. */
function canonical(v) {
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  if (isObj(v)) return `{${Object.keys(v).filter((k) => present(v[k])).sort().map((k) => `${JSON.stringify(k)}:${canonical(v[k])}`).join(",")}}`;
  return JSON.stringify(v);
}

/** The host of `url`, or null when it has user info. */
function hostOf(url) {
  const i = url.indexOf("://");
  if (i < 0) return null;
  const authority = url.slice(i + 3).split(/[/?#]/)[0];
  if (authority.includes("@")) return null;
  return authority.startsWith("[") ? authority.slice(1).split("]")[0] : authority.split(":")[0];
}

/** Every violation of `catalog`, as `{ rule, path, message }`. */
export function checkCatalog(catalog, rules = RULES) {
  const out = [];
  const bad = (rule, path, message) => out.push({ rule, path, message });
  const keys = (obj, path, allowed, required) => {
    if (!isObj(obj)) return bad("schema", path, "not an object"), false;
    for (const k of Object.keys(obj)) if (!allowed.includes(k)) bad("schema", `${path}/${k}`, "unknown field");
    for (const k of required) if (!present(obj[k])) bad("schema", `${path}/${k}`, "missing");
    return true;
  };
  const https = (path, url) => {
    if (!isStr(url) || !url.startsWith("https://") || url.length <= 8) bad("https", path, `${url} is not https`);
  };
  const sha = (path, v) => SHA.test(v ?? "") || bad("sha256", path, "not 64 lowercase hex digits");
  const size = (path, v) => (isInt(v) && v >= 1 && v <= rules.maxSize) || bad("size", path, `${v} bytes is outside 1..=${rules.maxSize}`);
  const weight = (path, v) => (Number.isInteger(v) && v >= 1 && v <= 1000) || bad("weight", path, `${v} is outside 1..=1000`);
  const scripts = (path, v, needOne) => {
    if (!Array.isArray(v)) return bad("schema", path, "not an array");
    if (needOne && v.length === 0) bad("scripts", path, "none");
    for (const s of v) if (!SCRIPT.test(s)) bad("scripts", path, `${s} is not an ISO 15924 code`);
  };

  if (!keys(catalog, "", ["catalogVersion", "fallback", "entries"], ["catalogVersion", "fallback", "entries"])) return out;
  if (!VERSION.test(catalog.catalogVersion ?? "")) bad("version", "/catalogVersion", "not YYYY.MM.N");
  const ids = new Set();
  const shas = new Map();
  const faceKeys = new Set();
  (Array.isArray(catalog.entries) ? catalog.entries : []).forEach((e, i) => {
    const p = `/entries/${i}`;
    if (!keys(e, p, ["id", "family", "licence", "homepage", "scripts", "styles"], ["id", "family", "licence", "homepage", "scripts", "styles"])) return;
    if (!ID.test(e.id ?? "")) bad("id", `${p}/id`, JSON.stringify(e.id));
    if (ids.has(e.id)) bad("unique", `${p}/id`, "listed twice");
    ids.add(e.id);
    if (keys(e.licence, `${p}/licence`, ["spdx", "url"], ["spdx", "url"])) {
      if (!rules.licences.includes(e.licence.spdx)) bad("licence", `${p}/licence/spdx`, `${e.licence.spdx} is not on the permissive allow list`);
      https(`${p}/licence/url`, e.licence.url);
    }
    https(`${p}/homepage`, e.homepage);
    scripts(`${p}/scripts`, e.scripts, true);
    const styles = Array.isArray(e.styles) ? e.styles : [];
    if (styles.length === 0) bad("faces", `${p}/styles`, "no styles");
    const styleIds = new Set();
    styles.forEach((s, j) => checkStyle(s, `${p}/styles/${j}`, styleIds));
  });
  const seen = new Set();
  (Array.isArray(catalog.fallback) ? catalog.fallback : []).forEach((id, i) => {
    if (!ids.has(id) || seen.has(id)) bad("fallback", `/fallback/${i}`, `${id} is unknown or listed twice`);
    seen.add(id);
  });
  return out;

  function checkStyle(s, p, styleIds) {
    if (!keys(s, p, ["id", "weight", "italic", "defaultFace", "file", "sources"], ["id", "weight", "italic", "file", "sources"])) return;
    if (!ID.test(s.id ?? "")) bad("id", `${p}/id`, JSON.stringify(s.id));
    if (styleIds.has(s.id)) bad("unique", `${p}/id`, "listed twice");
    styleIds.add(s.id);
    weight(`${p}/weight`, s.weight);
    const f = s.file;
    if (keys(f, `${p}/file`, ["sha256", "size", "format", "faces", "variable"], ["sha256", "size", "format", "faces"])) {
      sha(`${p}/file/sha256`, f.sha256);
      size(`${p}/file/size`, f.size);
      if (!rules.formats.includes(f.format)) bad("format", `${p}/file/format`, `${f.format} is not a font format`);
      const variable = present(f.variable);
      if (variable && keys(f.variable, `${p}/file/variable`, ["wght"], ["wght"])) {
        const w = f.variable.wght;
        const wp = `${p}/file/variable/wght`;
        if (!Array.isArray(w) || w.length !== 2) bad("schema", wp, "not [min, max]");
        else {
          weight(`${wp}/0`, w[0]);
          weight(`${wp}/1`, w[1]);
          if (!(w[0] <= w[1] && w[0] <= s.weight && s.weight <= w[1])) bad("variable", wp, `${JSON.stringify(w)} does not cover the style's weight ${s.weight}`);
        }
      }
      // One file may serve several styles only as a variable font, declared identically.
      const seen = shas.get(f.sha256);
      const repeat = seen !== undefined;
      if (repeat && !variable) bad("unique", `${p}/file/sha256`, "listed twice");
      else if (repeat && canonical(seen) !== canonical(f)) bad("variable", `${p}/file`, "a variable file listed again must be declared identically");
      if (!repeat) shas.set(f.sha256, f);
      const faces = Array.isArray(f.faces) ? f.faces : [];
      if (faces.length === 0) bad("faces", `${p}/file/faces`, "no faces");
      const indexes = new Set();
      faces.forEach((face, k) => {
        const fp = `${p}/file/faces/${k}`;
        if (!keys(face, fp, ["index", "family", "weight", "italic", "scripts"], ["index", "family", "weight", "italic", "scripts"])) return;
        weight(`${fp}/weight`, face.weight);
        scripts(`${fp}/scripts`, face.scripts, false);
        if (indexes.has(face.index)) bad("unique", `${fp}/index`, "listed twice");
        indexes.add(face.index);
        const key = `${face.family}|${face.weight}|${face.italic}`;
        if (!face.family || (!repeat && faceKeys.has(key))) bad("unique", fp, `face key ${key} is listed twice`);
        faceKeys.add(key);
      });
      const df = present(s.defaultFace) ? s.defaultFace : 0;
      const d = faces.find((x) => x.index === df);
      if (faces.length && !(d && (d.weight === s.weight || variable) && d.italic === s.italic)) {
        bad("faces", `${p}/defaultFace`, "not a face of the file with the style's weight (or a variable one) and italic");
      }
    }
    const sources = Array.isArray(s.sources) ? s.sources : [];
    if (sources.length === 0) bad("sources", `${p}/sources`, "no sources");
    sources.forEach((src, k) => checkSource(src, `${p}/sources/${k}`));
  }

  function checkSource(src, p) {
    if (!keys(src, p, ["url", "sha256", "size", "archive"], ["url"])) return;
    https(`${p}/url`, src.url);
    if (isStr(src.url) && !isImmutableUrl(src.url)) bad("immutableUrl", `${p}/url`, `${src.url} names no commit or release tag`);
    const host = isStr(src.url) ? hostOf(src.url) : null;
    if (!host || !rules.sourceHosts.some((h) => h.toLowerCase() === host.toLowerCase())) {
      bad("sourceHost", `${p}/url`, `${src.url} is not on a catalog source host`);
    }
    if (present(src.archive)) {
      if (!keys(src.archive, `${p}/archive`, ["format", "member"], ["format", "member"])) return;
      if (!rules.archiveFormats.includes(src.archive.format)) bad("format", `${p}/archive/format`, `${src.archive.format} is not an archive format`);
      if (!present(src.sha256) || !present(src.size)) return bad("archive", p, "an archive source states the archive's sha256 and size");
      sha(`${p}/sha256`, src.sha256);
      size(`${p}/size`, src.size);
      const m = src.archive.member;
      if (!isStr(m) || !m || m.startsWith("/") || m.includes("\\") || m.split("/").some((x) => x === ".." || x === "")) {
        bad("archive", `${p}/archive/member`, "not a relative path");
      }
    } else if (present(src.sha256) || present(src.size)) {
      bad("archive", p, "only an archive source states a sha256 or size");
    }
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  const i = process.argv.indexOf("--catalog");
  const rel = i === -1 ? "crates/fin-fonts/catalog/catalog.json" : process.argv[i + 1];
  const path = isAbsolute(rel) ? rel : join(root, rel);
  let violations;
  try {
    violations = checkCatalog(JSON.parse(readFileSync(path, "utf8")));
  } catch (e) {
    violations = [{ rule: "schema", path: "", message: `${path}: ${e.message}` }];
  }
  for (const v of violations) console.log(`FONT_CATALOG_INVALID: ${v.rule}: ${v.path || "/"}: ${v.message}`);
  process.exitCode = violations.length ? 1 : 0;
}
