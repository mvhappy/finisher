// A brute-force reference model of the catalog diff (plan 5D Task 7b), for
// `diff.property.test.ts`. It restates Decisions 11, 13, 18, 22 and 25 over plain arrays: every
// rule scans every row, nothing is indexed, cached or incremental, and no code is shared with
// `src/library/catalog/core/`. Rows are compared with the catalog's field for field.
import type { DirDone, ScanBatch, ScanEntry } from "@finisher/core-types";

export interface MDir {
  readonly id: number;
  readonly parent: number | null;
  readonly name: string; // hex of the raw bytes
  nameNfc: string;
  relKey: string;
  state: string;
  code: string | null;
  placeholder: string | null;
  mtimeNs: string | null;
  listedGen: number | null;
  listedAt: number | null;
  emptySince: number | null;
  emptyJob: string | null;
  collision: number;
  lastSeen: number;
}

export interface MFile {
  readonly id: number;
  dirId: number;
  name: string; // hex
  nameNfc: string;
  nameLossy: boolean;
  relKey: string;
  ext: string | null;
  size: number;
  mtimeNs: string;
  fileId: string | null; // hex, usable ids only
  dev: string | null;
  placeholder: string | null;
  fp: string | null;
  probeVersion: number | null;
  kind: string | null;
  state: string;
  code: string | null;
  renameOf: number | null;
  collision: number;
  hardlink: boolean;
  firstSeen: number;
  lastSeen: number;
  lastChange: number;
  missingSince: number | null;
  missingGen: number | null;
}

export interface ProbeResult {
  readonly fileId: number;
  readonly size: number;
  readonly mtimeNs: string;
  readonly fp: string | null;
  readonly probeVersion: number;
  readonly state: "present" | "probeFailed" | "skipped";
  readonly kind?: string | null;
}

const hex = (b64: string): string => Buffer.from(b64, "base64").toString("hex");
const lossyDecode = (h: string): string => new TextDecoder().decode(Buffer.from(h, "hex"));
const live = (r: { state: string }): boolean => r.state !== "missing";
const EXCLUDED = "EXCLUDED";

interface Saved {
  readonly dirs: MDir[];
  readonly files: MFile[];
  readonly nextDir: number;
  readonly nextFile: number;
  readonly deleted: number;
}

interface MListing {
  start: number;
  seen: number;
  excludedDirs: number[];
  excludedFiles: number[];
}

/** Who applies: the job, its generation, the transaction time and the attempt's start. */
interface Ctx {
  readonly job: string;
  readonly gen: number;
  readonly at: number;
  readonly startedAt: number;
}

function extOf(nfc: string): string | null {
  const i = nfc.lastIndexOf(".");
  if (i <= 0 || i === nfc.length - 1) return null;
  const e = nfc.slice(i + 1).toLowerCase();
  return e.length > 16 ? null : e;
}

export class Model {
  dirs: MDir[] = [];
  files: MFile[] = [];
  offline = false;
  /** Model file ids deleted (merged away), for the id pairing in the test. */
  readonly deleted: number[] = [];
  #nextDir = 2;
  #nextFile = 1;
  #lastAt = 0;
  #listings = new Map<string, MListing>();

  constructor() {
    this.dirs.push({
      id: 1, parent: null, name: "", nameNfc: "", relKey: "", state: "present", code: null, placeholder: null, mtimeNs: null,
      listedGen: null, listedAt: null, emptySince: null, emptyJob: null, collision: 0, lastSeen: 0,
    });
  }

  /** `attemptStart`: strictly after every time handed out or written so far. */
  attemptStart(now: number): number {
    this.#lastAt = Math.max(now, this.#lastAt + 1);
    return this.#lastAt;
  }

  /** The indexer restarted (or the catalog was reopened): every open listing is lost. */
  restart(): void {
    this.#listings = new Map();
  }

  dir(id: number): MDir {
    return this.dirs.find((d) => d.id === id)!;
  }

  /** The directory a raw chain names, if catalogued. */
  findDir(components: ReadonlyArray<string>): MDir | undefined {
    let cur: MDir | undefined = this.dir(1);
    for (const c of components) {
      cur = this.dirs.find((d) => d.parent === cur!.id && d.name === hex(c));
      if (cur === undefined) return undefined;
    }
    return cur;
  }

  /** Every directory below `id` (not `id` itself). */
  #below(id: number): MDir[] {
    const out: MDir[] = [];
    const walk = (p: number) => {
      for (const d of this.dirs) {
        if (d.parent === p) {
          out.push(d);
          walk(d.id);
        }
      }
    };
    walk(id);
    return out;
  }

  /**
   * Applies one batch file in transactions of `txRows` rows: answers the error code when the
   * file is refused (nothing written) or when transaction `fault` fails (the earlier ones stay,
   * every open listing is lost).
   */
  applyFile(
    batches: ReadonlyArray<ScanBatch>,
    job: { job: string; gen: number; startedAt: number },
    at0: number,
    txRows: number,
    fault?: number,
  ): string | undefined {
    const rows: Array<{ e?: ScanEntry; d?: DirDone }> = [];
    for (const b of batches) {
      for (const e of b.entries) rows.push({ e });
      for (const d of b.dirsDone) rows.push({ d });
    }
    // Every chain must be catalogued or created earlier in the file.
    const made: string[] = [];
    for (const r of rows) {
      const comps = (r.e ?? r.d)!.dir.components;
      if (this.findDir(comps) === undefined && !made.includes(comps.join("/"))) return "LIBRARY_BATCH_INVALID";
      if (r.e?.entryType === "dir") made.push([...comps, r.e.name].join("/"));
    }
    const rootEmpty = rows.some((r) => r.d !== undefined && r.d.dir.components.length === 0 && r.d.complete && r.d.entries === 0);
    if (!this.offline && rootEmpty && this.files.some(live)) return "ROOT_EMPTY_SUSPECT";
    const at = Math.max(at0, this.#lastAt);
    this.#lastAt = at;
    const ctx: Ctx = { ...job, at };
    for (let i = 0, n = 1; i < rows.length; i += txRows, n++) {
      const saved = n === fault ? this.#save() : undefined;
      for (const r of rows.slice(i, i + txRows)) {
        if (r.e !== undefined) this.#entry(r.e, ctx);
        else this.#done(r.d!, ctx);
      }
      this.#hardlinks();
      if (saved !== undefined) {
        this.#restore(saved);
        this.restart();
        return "CATALOG_TEST_FAULT";
      }
    }
    return undefined;
  }

  #save(): Saved {
    return {
      dirs: this.dirs.map((d) => ({ ...d })), files: this.files.map((f) => ({ ...f })), nextDir: this.#nextDir, nextFile: this.#nextFile,
      deleted: this.deleted.length,
    };
  }

  #restore(s: Saved): void {
    this.dirs = s.dirs;
    this.files = s.files;
    this.#nextDir = s.nextDir;
    this.#nextFile = s.nextFile;
    this.deleted.length = s.deleted;
  }

  #listing(ctx: Ctx, dirId: number): MListing {
    const key = `${ctx.job}\u0000${dirId}`;
    let l = this.#listings.get(key);
    if (l === undefined) {
      l = { start: Math.min(ctx.at, ctx.startedAt), seen: 0, excludedDirs: [], excludedFiles: [] };
      this.#listings.set(key, l);
    }
    return l;
  }

  #entry(e: ScanEntry, ctx: Ctx): void {
    const at = ctx.at;
    const dir = this.findDir(e.dir.components)!;
    this.#listing(ctx, dir.id).seen++;
    if (dir.emptySince !== null) {
      Object.assign(dir, { emptySince: null, emptyJob: null, code: dir.code === "DIR_EMPTY_SUSPECT" ? null : dir.code });
    }
    if (e.entryType === "dir") return this.#dirEntry(dir, e, ctx);
    if (e.entryType !== "file") return;
    const name = hex(e.name);
    if (e.excluded != null) {
      const row = this.files.find((f) => f.dirId === dir.id && f.name === name && live(f));
      if (row !== undefined) this.#listing(ctx, dir.id).excludedFiles.push(row.id);
      return;
    }
    const id = e.fileId == null || /^0*$/.test(hex(e.fileId)) ? null : hex(e.fileId);
    const dev = e.dev ?? null;
    const placeholder = e.placeholder ?? null;
    const fresh = placeholder === null ? "present" : "cloudOnly";
    const names = { dirId: dir.id, name, nameNfc: e.nameNfc, nameLossy: e.nameLossy, relKey: e.relKey, ext: extOf(e.nameNfc) };
    const known = this.files.find((f) => f.dirId === dir.id && f.name === name);
    if (known === undefined) {
      const same = id === null ? [] : this.files.filter((f) => f.fileId === id && f.dev === dev && f.size === e.size && f.mtimeNs === e.mtimeNs);
      const gone = same
        .filter((f) => !live(f) && f.code !== EXCLUDED)
        .sort((a, b) => (b.missingSince ?? 0) - (a.missingSince ?? 0) || a.id - b.id)[0];
      if (gone !== undefined) {
        Object.assign(gone, names, {
          fileId: id, dev, placeholder, state: fresh, code: null, lastSeen: at, missingSince: null, missingGen: null, renameOf: null,
        });
        if (gone.fp === null) gone.probeVersion = null;
        return;
      }
      const was = same.filter(live).sort((a, b) => a.id - b.id)[0];
      this.files.push({
        id: this.#nextFile++, ...names, size: e.size, mtimeNs: e.mtimeNs, fileId: id, dev, placeholder, fp: null, probeVersion: null,
        kind: null, state: fresh, code: null, renameOf: was?.id ?? null, collision: 0, hardlink: false, firstSeen: at, lastSeen: at,
        lastChange: at, missingSince: null, missingGen: null,
      });
      return;
    }
    const wasMissing = known.state === "missing";
    const sameStat = known.size === e.size && known.mtimeNs === e.mtimeNs;
    const idDiffers = known.fileId !== null && id !== null && known.fileId !== id;
    if (!sameStat || (!wasMissing && idDiffers)) {
      Object.assign(known, names, {
        size: e.size, mtimeNs: e.mtimeNs, fileId: id, dev, placeholder, state: fresh, code: null, fp: null, probeVersion: null, kind: null,
        renameOf: null, lastSeen: at, lastChange: at, missingSince: null, missingGen: null,
      });
      return;
    }
    const keep = !wasMissing && placeholder === null && (known.state === "probeFailed" || known.state === "skipped");
    Object.assign(known, names, {
      fileId: id, dev, placeholder, state: keep ? known.state : fresh, code: keep ? known.code : null, lastSeen: at, missingSince: null,
      missingGen: null,
    });
    if (wasMissing && known.fp === null) known.probeVersion = null;
  }

  #dirEntry(parent: MDir, e: ScanEntry, ctx: Ctx): void {
    const at = ctx.at;
    const state = e.excluded != null ? "excluded" : e.placeholder != null ? "cloudOnly" : "present";
    const fields = {
      nameNfc: e.nameNfc, relKey: e.relKey, state, code: e.excluded ?? null, placeholder: e.placeholder ?? null, lastSeen: at,
    };
    const known = this.dirs.find((d) => d.parent === parent.id && d.name === hex(e.name));
    if (known === undefined) {
      this.dirs.push({
        id: this.#nextDir++, parent: parent.id, name: hex(e.name), ...fields, mtimeNs: null, listedGen: null, listedAt: null,
        emptySince: null, emptyJob: null, collision: 0,
      });
      return;
    }
    Object.assign(known, fields);
    if (state === "excluded") this.#listing(ctx, parent.id).excludedDirs.push(known.id);
    if (state === "cloudOnly") {
      for (const d of this.#below(known.id)) if (!["missing", "excluded", "cloudOnly"].includes(d.state)) d.state = "cloudOnly";
      const ids = new Set([known.id, ...this.#below(known.id).map((d) => d.id)]);
      for (const f of this.files) if (ids.has(f.dirId) && live(f) && f.state !== "cloudOnly") f.state = "cloudOnly";
    }
  }

  /** Marks every directory below `id` and every file in or below it missing; answers the files. */
  #goneBelow(id: number, ctx: Ctx, code: string | null): MFile[] {
    const below = this.#below(id);
    for (const d of below) if (live(d)) d.state = "missing";
    const ids = new Set([id, ...below.map((d) => d.id)]);
    const files = this.files.filter((f) => ids.has(f.dirId) && live(f));
    for (const f of files) this.#gone(f, ctx, code);
    return files;
  }

  #gone(f: MFile, ctx: Ctx, code: string | null): void {
    Object.assign(f, { state: "missing", missingSince: ctx.at, missingGen: ctx.gen, code: code ?? f.code, renameOf: null, hardlink: false });
  }

  #resolve(gone: ReadonlyArray<MFile>): void {
    for (const x of gone) {
      if (!this.files.includes(x)) continue;
      const n = this.files.filter((f) => f.renameOf === x.id && live(f)).sort((a, b) => a.id - b.id)[0];
      if (n !== undefined) this.merge(x, n, x.fp !== null);
    }
  }

  /** `target` takes `source`'s path and metadata; `source` is dropped (Decision 25). */
  merge(target: MFile, source: MFile, keepProbe: boolean): void {
    this.files = this.files.filter((f) => f !== source);
    this.deleted.push(source.id);
    for (const f of this.files) if (f.renameOf === source.id || f.renameOf === target.id) f.renameOf = null;
    const p = keepProbe ? { ...target } : source;
    Object.assign(target, {
      dirId: source.dirId, name: source.name, nameNfc: source.nameNfc, nameLossy: source.nameLossy, relKey: source.relKey,
      ext: source.ext, collision: source.collision, size: source.size, mtimeNs: source.mtimeNs, fileId: source.fileId, dev: source.dev,
      placeholder: source.placeholder, lastSeen: source.lastSeen,
      state: keepProbe ? (source.placeholder !== null ? "cloudOnly" : "present") : source.state,
      code: keepProbe ? null : source.code, fp: p.fp, probeVersion: p.probeVersion, kind: p.kind, renameOf: null, missingSince: null,
      missingGen: null,
    });
  }

  #done(d: DirDone, ctx: Ctx): void {
    const at = ctx.at;
    const dir = this.findDir(d.dir.components)!;
    const key = `${ctx.job}\u0000${dir.id}`;
    const l = this.#listings.get(key);
    this.#listings.delete(key);
    if (this.offline) return;
    if (!d.complete) {
      dir.code = d.code ?? null;
      if (dir.state === "present") dir.state = "unreadable";
      dir.lastSeen = at;
      return;
    }
    const start = l?.start ?? Math.min(at, ctx.startedAt);
    const kids = this.files.some((f) => f.dirId === dir.id && live(f)) || this.dirs.some((c) => c.parent === dir.id && live(c));
    if (d.entries === 0 && kids) {
      const ok = dir.emptySince !== null && dir.emptyJob !== ctx.job && start - dir.emptySince >= 60_000;
      if (!ok) {
        dir.emptySince ??= at;
        dir.emptyJob ??= ctx.job;
        dir.code = "DIR_EMPTY_SUSPECT";
        dir.lastSeen = at;
        return;
      }
    } else if (d.entries > 0 && (l === undefined || l.seen < d.entries)) {
      dir.lastSeen = at;
      return;
    }
    // An exclusion takes its rows out once the listing that reported it closes.
    for (const id of l?.excludedFiles ?? []) {
      const f = this.files.find((x) => x.id === id);
      if (f !== undefined && live(f) && f.lastSeen < start) this.#gone(f, ctx, EXCLUDED);
    }
    for (const id of l?.excludedDirs ?? []) if (this.dir(id).state === "excluded") this.#goneBelow(id, ctx, EXCLUDED);
    const gone = this.files.filter((f) => f.dirId === dir.id && live(f) && f.lastSeen < start);
    for (const f of gone) this.#gone(f, ctx, null);
    for (const c of this.dirs.filter((c) => c.parent === dir.id && live(c) && c.lastSeen < start)) {
      c.state = "missing";
      gone.push(...this.#goneBelow(c.id, ctx, null));
    }
    const seen = new Set(this.files.filter((f) => f.dirId === dir.id && live(f) && f.lastSeen >= start).map((f) => f.id));
    for (const f of this.files) if (f.renameOf !== null && seen.has(f.renameOf)) f.renameOf = null;
    this.#resolve(gone);
    this.#collisions(dir.id);
    if (dir.listedAt === null || start >= dir.listedAt) {
      if (d.mtimeNs != null) dir.mtimeNs = d.mtimeNs;
      Object.assign(dir, { listedGen: ctx.gen, listedAt: start, code: d.code ?? null });
      if (dir.state === "unreadable") dir.state = "present";
    } else if (dir.code === "DIR_EMPTY_SUSPECT") dir.code = null;
    Object.assign(dir, { emptySince: null, emptyJob: null, lastSeen: at });
  }

  #collisions(dirId: number): void {
    const rows: Array<MFile | MDir> = [...this.files.filter((f) => f.dirId === dirId), ...this.dirs.filter((d) => d.parent === dirId)];
    for (const r of rows) {
      r.collision = 0;
      if (!live(r)) continue;
      for (const o of rows) {
        if (o === r || !live(o) || o.relKey !== r.relKey) continue;
        if (lossyDecode(o.name) === lossyDecode(r.name)) r.collision |= 4;
        else if (o.nameNfc === r.nameNfc) r.collision |= 1;
        else r.collision |= 2;
      }
    }
  }

  #hardlinks(): void {
    for (const f of this.files) {
      f.hardlink = live(f) && f.fileId !== null && this.files.some((o) => o !== f && live(o) && o.fileId === f.fileId && o.dev === f.dev);
    }
  }

  /** Present files a probe at `version` would take. */
  pending(version: number): number[] {
    return this.files.filter((f) => f.state === "present" && (f.probeVersion === null || f.probeVersion < version)).map((f) => f.id);
  }

  /** Lands probe results; answers the moves `{from, to}` in model ids. */
  probe(results: ReadonlyArray<ProbeResult>): Array<{ from: number; to: number }> {
    const moved: Array<{ from: number; to: number }> = [];
    for (const r of results) {
      const f = this.files.find((x) => x.id === r.fileId);
      if (f === undefined || f.size !== r.size || f.mtimeNs !== r.mtimeNs || ["missing", "cloudOnly", "offline"].includes(f.state)) continue;
      const first = f.fp === null && f.renameOf === null && f.firstSeen === f.lastChange;
      Object.assign(f, { fp: r.fp, probeVersion: r.probeVersion, state: r.state, code: null, kind: r.kind ?? null });
      if (!first || r.fp === null) continue;
      const m = this.files
        .filter((x) => x.state === "missing" && x.code !== EXCLUDED && x.fp === r.fp && x.id !== f.id)
        .sort((a, b) => (b.missingSince ?? 0) - (a.missingSince ?? 0) || a.id - b.id)[0];
      if (m !== undefined) {
        this.merge(m, f, false);
        moved.push({ from: f.id, to: m.id });
      }
    }
    this.#hardlinks();
    return moved;
  }

  /** What `writeKnownDirs` writes: present and unreadable directories by id. */
  knownDirs(gen: number): Array<{ id: number; mtimeNs: string | null; listed: boolean }> {
    return this.dirs
      .filter((d) => d.state === "present" || d.state === "unreadable")
      .sort((a, b) => a.id - b.id)
      .map((d) => ({ id: d.id, mtimeNs: d.mtimeNs, listed: d.listedGen === gen }));
  }

  /** The raw chain of a directory, as hex components. */
  chain(id: number): string[] {
    const out: string[] = [];
    for (let d = this.dir(id); d.parent !== null; d = this.dir(d.parent)) out.unshift(d.name);
    return out;
  }
}
