// Crate-age gate (supply-chain rule): every crate in Cargo.lock, and in each committed
// `crates/*/fuzz/Cargo.lock` (the nightly fuzz targets are outside the workspace and resolve
// on their own), must have been published at least N days ago (default 7). Needs network access (crates.io API), so it is NOT part of
// `pnpm gates`; run it with `pnpm check:crate-age` before merging dependency changes.
//
//   node scripts/check-crate-age.mjs [--days N] [--delay-ms MS] [--root DIR]
//
// Publication dates never change, so every answer is cached in `target/crate-age-cache.json`
// (git-ignored); only new name@version pairs hit the network, about one request per second.
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const DEFAULT_MIN_DAYS = 7;
export const DEFAULT_DELAY_MS = 1000;
const DAY_MS = 86_400_000;
const CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index";
const USER_AGENT = "finisher-crate-age-check";

/** Registry packages of a Cargo.lock as `{name, version}` (path, git and other-registry
 * packages are not crates.io publications and are skipped), sorted and de-duplicated. */
export function parseLock(text) {
  const out = new Map();
  for (const block of text.split(/^\[\[package\]\]\s*$/m).slice(1)) {
    const name = block.match(/^name = "([^"]+)"/m)?.[1];
    const version = block.match(/^version = "([^"]+)"/m)?.[1];
    const source = block.match(/^source = "([^"]+)"/m)?.[1];
    if (name && version && source === CRATES_IO) out.set(`${name}@${version}`, { name, version });
  }
  return [...out.values()].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
}

/** Age in whole days of a crates.io `created_at` timestamp at `now` (a Date). */
export function ageDays(createdAt, now) {
  const t = Date.parse(createdAt);
  if (Number.isNaN(t)) throw new Error(`unparseable created_at: ${createdAt}`);
  return Math.floor((now.getTime() - t) / DAY_MS);
}

/** True when `createdAt` is less than `minDays` full days before `now`. Exactly `minDays`
 * days (to the millisecond) old passes; a timestamp in the future is always too young. */
export function isTooYoung(createdAt, now, minDays = DEFAULT_MIN_DAYS) {
  const t = Date.parse(createdAt);
  if (Number.isNaN(t)) throw new Error(`unparseable created_at: ${createdAt}`);
  return now.getTime() - t < minDays * DAY_MS;
}

/** Violation lines for the crates younger than `minDays`. `dates` maps "name@version" to
 * `created_at`; a crate without a date is itself a violation (it cannot be proven old). */
export function findViolations(crates, dates, now, minDays = DEFAULT_MIN_DAYS) {
  const out = [];
  for (const { name, version } of crates) {
    const at = dates[`${name}@${version}`];
    if (!at) out.push(`${name} ${version}: no publication date`);
    else if (isTooYoung(at, now, minDays)) out.push(`${name} ${version}: published ${at} (${ageDays(at, now)} days ago, need >= ${minDays})`);
  }
  return out;
}

/** Dates for `crates`: from `cache` where known, else `fetchCreated(name, version)` (spaced
 * `delayMs` apart). Returns the merged map; `onFetch` is called after each lookup. */
export async function resolveDates(crates, cache, fetchCreated, { delayMs = DEFAULT_DELAY_MS, sleep = defaultSleep, onFetch = () => {} } = {}) {
  const dates = { ...cache };
  let first = true;
  for (const { name, version } of crates) {
    const key = `${name}@${version}`;
    if (dates[key]) continue;
    if (!first && delayMs > 0) await sleep(delayMs);
    first = false;
    dates[key] = await fetchCreated(name, version);
    onFetch(key, dates[key]);
  }
  return dates;
}

export const defaultSleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** `created_at` of one crates.io version, retrying a rate-limited (429) answer. */
export async function fetchCreatedAt(name, version, { fetchFn = fetch, sleep = defaultSleep, retries = 5 } = {}) {
  const url = `https://crates.io/api/v1/crates/${encodeURIComponent(name)}/${encodeURIComponent(version)}`;
  for (let attempt = 0; ; attempt++) {
    const res = await fetchFn(url, { headers: { "User-Agent": USER_AGENT, Accept: "application/json" } });
    if (res.status === 429 && attempt < retries) {
      const wait = Number(res.headers?.get?.("retry-after"));
      await sleep((Number.isFinite(wait) && wait > 0 ? wait : 2 ** attempt) * 1000);
      continue;
    }
    if (!res.ok) throw new Error(`crates.io ${name} ${version}: HTTP ${res.status}`);
    const at = (await res.json())?.version?.created_at;
    if (typeof at !== "string") throw new Error(`crates.io ${name} ${version}: no version.created_at in the response`);
    return at;
  }
}

/** The lockfiles under `root` the gate covers: the workspace `Cargo.lock`, then every
 * `crates/<name>/fuzz/Cargo.lock` in name order (relative paths, `/`-separated). */
export function lockfiles(root) {
  const out = ["Cargo.lock"];
  const crates = join(root, "crates");
  if (existsSync(crates)) {
    for (const entry of readdirSync(crates, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      if (entry.isDirectory() && existsSync(join(crates, entry.name, "fuzz", "Cargo.lock"))) out.push(`crates/${entry.name}/fuzz/Cargo.lock`);
    }
  }
  return out;
}

/** Registry packages of all `locks` merged, sorted and de-duplicated by name and version. */
export function parseLocks(texts) {
  const out = new Map();
  for (const text of texts) for (const c of parseLock(text)) out.set(`${c.name}@${c.version}`, c);
  return [...out.values()].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
}

export function readCache(path) {
  try {
    const cache = JSON.parse(readFileSync(path, "utf8"));
    return cache && typeof cache === "object" && !Array.isArray(cache) ? cache : {};
  } catch {
    return {};
  }
}

export function writeCache(path, cache) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(cache, null, 1) + "\n");
}

export function option(argv, flag, fallback) {
  const i = argv.indexOf(flag);
  if (i === -1) return fallback;
  const n = Number(argv[i + 1]);
  if (!Number.isFinite(n) || n < 0) throw new Error(`${flag} needs a non-negative number`);
  return n;
}

export async function main(argv = process.argv.slice(2), env = process.env) {
  const i = argv.indexOf("--root");
  const root = i === -1 ? process.cwd() : argv[i + 1];
  const days = option(argv, "--days", env.FIN_CRATE_MIN_AGE_DAYS ? Number(env.FIN_CRATE_MIN_AGE_DAYS) : DEFAULT_MIN_DAYS);
  const delayMs = option(argv, "--delay-ms", DEFAULT_DELAY_MS);
  const lock = join(root, "Cargo.lock");
  if (!existsSync(lock)) throw new Error(`no Cargo.lock under ${root}`);
  const crates = parseLocks(lockfiles(root).map((f) => readFileSync(join(root, f), "utf8")));
  const cachePath = join(root, "target", "crate-age-cache.json");
  const cache = readCache(cachePath);
  const missing = crates.filter((c) => !cache[`${c.name}@${c.version}`]).length;
  if (missing) console.error(`crate-age: ${missing} of ${crates.length} crates to look up on crates.io (cached: ${crates.length - missing})`);
  let dates = cache;
  try {
    dates = await resolveDates(crates, cache, (n, v) => fetchCreatedAt(n, v), {
      delayMs,
      onFetch: (key, at) => {
        cache[key] = at;
      },
    });
  } finally {
    writeCache(cachePath, cache); // keep what was learned even if a lookup failed midway
  }
  const violations = findViolations(crates, dates, new Date(), days);
  for (const v of violations) console.log(v);
  if (violations.length) console.log(`crate-age: ${violations.length} crate(s) younger than ${days} days; downgrade with \`cargo update -p <name> --precise <version>\``);
  else console.error(`crate-age: all ${crates.length} crates are at least ${days} days old`);
  process.exitCode = violations.length ? 1 : 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((err) => {
    console.error(`crate-age: ${err.message}`);
    process.exitCode = 2;
  });
}
