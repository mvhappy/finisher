// npm-age gate (supply-chain rule, the npm half of `check:dep-age`): every package in
// pnpm-lock.yaml must have been published at least N days ago (default 7), the same rule the
// machines' registry proxy enforces. Needs network access (the npm registry), so it is NOT part
// of `pnpm gates`; run it with `pnpm check:npm-age` before merging dependency changes.
//
//   node scripts/check-npm-age.mjs [--days N] [--delay-ms MS] [--registry URL] [--root DIR]
//
// Publication dates never change, so every answer is cached in `target/npm-age-cache.json`
// (git-ignored); only package names with an uncached locked version hit the network (one
// metadata request per name, `--delay-ms` apart, 200 ms by default). Set
// FIN_NPM_MIN_AGE_DAYS / FIN_NPM_REGISTRY instead of the flags if you prefer.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { defaultSleep, findViolations, option, readCache, writeCache, DEFAULT_MIN_DAYS } from "./check-crate-age.mjs";

export const DEFAULT_NPM_DELAY_MS = 200;
export const DEFAULT_REGISTRY = "https://registry.npmjs.org";
const USER_AGENT = "finisher-dep-age-check (npm publication-date gate)";

/** Registry packages of a pnpm-lock.yaml (`packages:` section) as `{name, version}`. Packages
 * resolved from a tarball URL, git or a directory are not registry publications and are
 * skipped. Sorted and de-duplicated. */
export function parseLock(text) {
  const out = new Map();
  let inPackages = false;
  let key = null;
  const flush = (resolution) => {
    if (key && resolution && !/\b(tarball|commit|directory|repo|type):/.test(resolution)) {
      const at = key.lastIndexOf("@");
      if (at > 0) out.set(key, { name: key.slice(0, at), version: key.slice(at + 1) });
    }
    key = null;
  };
  for (const line of text.split("\n")) {
    if (/^\S/.test(line)) {
      inPackages = line.startsWith("packages:");
      key = null;
      continue;
    }
    if (!inPackages) continue;
    const entry = line.match(/^ {2}(?:'([^']+)'|"([^"]+)"|([^\s'"][^\s]*)):\s*$/);
    if (entry) {
      key = entry[1] ?? entry[2] ?? entry[3];
      continue;
    }
    const resolution = line.match(/^ {4}resolution:\s*(.*)$/);
    if (resolution && key) flush(resolution[1]);
  }
  return [...out.values()].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
}

/** The `time` map (`version` -> ISO timestamp) of one package's registry metadata, retrying a
 * rate-limited (429) answer. Includes the `created`/`modified` keys; callers index by version. */
export async function fetchPublishTimes(name, { registry = DEFAULT_REGISTRY, fetchFn = fetch, sleep = defaultSleep, retries = 5 } = {}) {
  const url = `${registry.replace(/\/+$/, "")}/${name.replace("/", "%2f")}`;
  for (let attempt = 0; ; attempt++) {
    const res = await fetchFn(url, { headers: { "User-Agent": USER_AGENT, Accept: "application/json" } });
    if (res.status === 429 && attempt < retries) {
      const wait = Number(res.headers?.get?.("retry-after"));
      await sleep((Number.isFinite(wait) && wait > 0 ? wait : 2 ** attempt) * 1000);
      continue;
    }
    if (!res.ok) throw new Error(`npm registry ${name}: HTTP ${res.status}`);
    const time = (await res.json())?.time;
    if (!time || typeof time !== "object") throw new Error(`npm registry ${name}: no \`time\` map in the metadata`);
    return time;
  }
}

/** Dates for `packages` ("name@version" -> timestamp): from `cache` where known, else one
 * `fetchTimes(name)` per package name (spaced `delayMs` apart) that has an uncached version.
 * A version missing from the registry's `time` map stays undefined (a violation). */
export async function resolveDates(packages, cache, fetchTimes, { delayMs = DEFAULT_NPM_DELAY_MS, sleep = defaultSleep, onFetch = () => {} } = {}) {
  const dates = { ...cache };
  const byName = new Map();
  for (const { name, version } of packages) {
    if (dates[`${name}@${version}`]) continue;
    byName.set(name, [...(byName.get(name) ?? []), version]);
  }
  let first = true;
  for (const [name, versions] of byName) {
    if (!first && delayMs > 0) await sleep(delayMs);
    first = false;
    const time = await fetchTimes(name);
    for (const version of versions) {
      const at = time[version];
      if (typeof at !== "string") continue;
      dates[`${name}@${version}`] = at;
      onFetch(`${name}@${version}`, at);
    }
  }
  return dates;
}

export async function main(argv = process.argv.slice(2), env = process.env) {
  const i = argv.indexOf("--root");
  const root = i === -1 ? process.cwd() : argv[i + 1];
  const r = argv.indexOf("--registry");
  const registry = r === -1 ? (env.FIN_NPM_REGISTRY ?? DEFAULT_REGISTRY) : argv[r + 1];
  const days = option(argv, "--days", env.FIN_NPM_MIN_AGE_DAYS ? Number(env.FIN_NPM_MIN_AGE_DAYS) : DEFAULT_MIN_DAYS);
  const delayMs = option(argv, "--delay-ms", DEFAULT_NPM_DELAY_MS);
  const lock = join(root, "pnpm-lock.yaml");
  if (!existsSync(lock)) throw new Error(`no pnpm-lock.yaml under ${root}`);
  const packages = parseLock(readFileSync(lock, "utf8"));
  const cachePath = join(root, "target", "npm-age-cache.json");
  const cache = readCache(cachePath);
  const missing = packages.filter((p) => !cache[`${p.name}@${p.version}`]).length;
  if (missing) console.error(`npm-age: ${missing} of ${packages.length} packages to look up on ${registry} (cached: ${packages.length - missing})`);
  let dates = cache;
  try {
    dates = await resolveDates(packages, cache, (name) => fetchPublishTimes(name, { registry }), {
      delayMs,
      onFetch: (key, at) => {
        cache[key] = at;
      },
    });
  } finally {
    writeCache(cachePath, cache); // keep what was learned even if a lookup failed midway
  }
  const violations = findViolations(packages, dates, new Date(), days);
  for (const v of violations) console.log(v);
  if (violations.length) console.log(`npm-age: ${violations.length} package(s) younger than ${days} days; hold them back by pinning the previous version in package.json`);
  else console.error(`npm-age: all ${packages.length} packages are at least ${days} days old`);
  process.exitCode = violations.length ? 1 : 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((err) => {
    console.error(`npm-age: ${err.message}`);
    process.exitCode = 2;
  });
}
