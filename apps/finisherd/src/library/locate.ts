// `Libraries.locate` (plan 5D Decision 45, arch §20): where a file is, for sub-project 6's
// manifest. In order:
//
// 1. **a path** under a library's root, or under one of its **aliases** (other machines'
//    paths of the same root: `/Volumes/<share>/Media` ↔ `\\<server>\media` ↔ `/mnt/media`),
//    mapped below the root and looked up by `relKey` (NFC, case-folded on a case-insensitive
//    root); several live rows → `PATH_AMBIGUOUS`;
// 2. a **fingerprint**, then 3. a **full hash**, across this server's libraries.
//
// A path whose only row is missing is answered only when neither the fingerprint nor the hash
// finds a live file. `cloudOnly` rows are answered (6 refuses them at export with
// `FILE_CLOUD_ONLY`). Aliases serve this system-internal resolution only: they never grant
// reach (Decision 47). The `Located` carries the raw chain and the `Expect` (with `dev`) that
// `open_for_read` needs.
import { isAbsolute, win32 } from "node:path";
import { FinError } from "@finisher/contracts";
import type { FileState, Located } from "./api.ts";
import type { ReaderClient } from "./catalog/client.ts";
import type { LibraryRow, Registry } from "./registry.ts";
import { exec } from "./states.ts";

export interface LocateDeps {
  readonly registry: Pick<Registry, "list" | "get">;
  readonly reader: Pick<ReaderClient, "file" | "lookup">;
  /** Whether the library's catalog is open (others are skipped). */
  readonly attached: (id: string) => boolean;
}

export interface LocateRef {
  readonly path?: string;
  readonly fingerprint?: string;
  readonly fullHash?: string;
}

const invalidPath = (why: string): FinError => new FinError({ code: "INVALID_PATH", message: `the path is not usable: ${why}`, details: {} });

/** `nfc` as the catalog's key component: itself, or lower-cased and re-normalised on a case-insensitive root (`fin_library::names::fold`). */
export function foldKey(nfc: string, caseSensitive: boolean): string {
  return caseSensitive ? nfc : nfc.toLowerCase().normalize("NFC");
}

const windowsy = (p: string): boolean => p.includes("\\") || /^[A-Za-z]:/.test(p);

/**
 * The components of `path` below `base` (both absolute), or `undefined` when it is not under
 * it. A Windows-style base or path compares with `\` as `/` and without case (as Windows
 * does); any other compares exactly. `.` and `..` components are refused.
 */
export function relUnder(base: string, path: string): string[] | undefined {
  const win = windowsy(base) || windowsy(path);
  const norm = (p: string) => {
    const s = win ? p.replace(/\\/g, "/") : p;
    return s.length > 1 ? s.replace(/\/+$/, "") : s;
  };
  const b = norm(base);
  const p = norm(path);
  const eq = (x: string, y: string) => (win ? x.toLowerCase() === y.toLowerCase() : x === y);
  const prefix = b === "/" ? "/" : `${b}/`;
  if (eq(p, b) || p.length <= prefix.length || !eq(p.slice(0, prefix.length), prefix)) return undefined;
  const parts = p.slice(prefix.length).split("/").filter((c) => c !== "");
  if (parts.some((c) => c === "." || c === "..")) throw invalidPath("it has . or .. components");
  return parts.length === 0 ? undefined : parts;
}

/** What `locate` reads of a catalogued row (`ReaderClient.file`). */
interface FileMatch {
  readonly id: number;
  readonly rel: { components: string[] };
  readonly display: string;
  readonly size: number;
  readonly mtimeNs: string;
  readonly fileId: string | null;
  readonly dev: string | null;
  readonly state: string;
}

function toLocated(lib: LibraryRow, f: FileMatch, match: Located["match"]): Located {
  return {
    library: lib.id,
    fileId: f.id,
    root: lib.root,
    rel: f.rel,
    display: f.display,
    expect: { size: f.size, mtimeNs: f.mtimeNs, ...(f.fileId === null ? {} : { fileId: f.fileId }), ...(f.dev === null ? {} : { dev: f.dev }) },
    hydrate: lib.hydrate,
    state: f.state as FileState,
    match,
    offline: lib.state === "offline" || f.state === "offline",
  };
}

/** The live (else missing) row at `relKey` in `lib`; several live ones → `PATH_AMBIGUOUS`. */
async function byKey(d: LocateDeps, lib: LibraryRow, relKey: string): Promise<{ live?: FileMatch; missing?: FileMatch }> {
  const { matches } = await exec(d.reader.file(lib.id, { relKey }));
  const live = matches.filter((m) => m.state !== "missing");
  if (live.length > 1) {
    throw new FinError({ code: "PATH_AMBIGUOUS", message: "several files in the library have this path", details: { library: lib.id, fileIds: live.map((m) => m.id) } });
  }
  return { ...(live[0] === undefined ? {} : { live: live[0] }), ...(matches[0] === undefined ? {} : { missing: matches[0] }) };
}

const RANK: Record<string, number> = { present: 0, probeFailed: 1, skipped: 1, unreadable: 2, cloudOnly: 3, offline: 4 };

/** The best live row with `fp` or `fullHash` across libraries: present first, then by library order and id. */
async function byContent(d: LocateDeps, key: { fingerprints: string[] } | { hashes: string[] }, match: "fingerprint" | "hash"): Promise<Located | undefined> {
  let best: { lib: LibraryRow; id: number; rank: number } | undefined;
  for (const lib of d.registry.list()) {
    if (!d.attached(lib.id)) continue;
    const { matches } = await exec(d.reader.lookup(lib.id, key)).catch(() => ({ matches: [] }));
    for (const m of matches) {
      const rank = RANK[m.file.state];
      if (rank === undefined) continue;
      if (best === undefined || rank < best.rank) best = { lib, id: m.file.id, rank };
    }
    if (best?.rank === 0) break;
  }
  if (best === undefined) return undefined;
  const { matches } = await exec(d.reader.file(best.lib.id, { fileId: best.id }));
  return matches[0] === undefined ? undefined : toLocated(best.lib, matches[0], match);
}

export async function locate(d: LocateDeps, ref: LocateRef): Promise<Located | undefined> {
  let fallback: Located | undefined;
  if (ref.path !== undefined) {
    if (typeof ref.path !== "string" || !(isAbsolute(ref.path) || win32.isAbsolute(ref.path))) throw invalidPath("it is not absolute");
    let hit: { lib: LibraryRow; parts: string[]; match: "path" | "alias"; depth: number } | undefined;
    for (const lib of d.registry.list()) {
      if (!d.attached(lib.id)) continue;
      for (const [base, match] of [[lib.root, "path"], ...lib.aliases.map((a) => [a, "alias"])] as Array<[string, "path" | "alias"]>) {
        const parts = relUnder(base, ref.path);
        const depth = base.length;
        if (parts !== undefined && (hit === undefined || depth > hit.depth)) hit = { lib, parts, match, depth };
      }
    }
    if (hit !== undefined) {
      const key = hit.parts.map((c) => foldKey(c.normalize("NFC"), hit.lib.volume.caseSensitive)).join("/");
      const found = await byKey(d, hit.lib, key);
      if (found.live !== undefined) return toLocated(hit.lib, found.live, hit.match);
      if (found.missing !== undefined) fallback = toLocated(hit.lib, found.missing, hit.match);
    }
  }
  if (ref.fingerprint !== undefined) {
    const r = await byContent(d, { fingerprints: [ref.fingerprint] }, "fingerprint");
    if (r !== undefined) return r;
  }
  if (ref.fullHash !== undefined) {
    const r = await byContent(d, { hashes: [ref.fullHash] }, "hash");
    if (r !== undefined) return r;
  }
  return fallback;
}
