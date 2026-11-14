// The registry of libraries in `control.db`'s `libraries` chain (plan 5D Task 8, Decisions
// 3, 14, 15, 17, 48, 49). It validates and stores what a library *is*: its canonical root,
// names, filters and schedule, the volume it was measured on, and whether it is metadata-only.
// States, scans, polls and catalogs are the service's (Task 10); this module only keeps the
// rows they read and write.
//
// - `add` resolves the root through fin-node (`libraryCanonicalRoot`, off the JS thread):
//   absolute, existing, a directory, a string, not holding or inside the server data, no overlap
//   with another root by `rootKey` prefix in either direction (`LIBRARY_OVERLAP`);
// - names are unique case-insensitively (`LIBRARY_NAME_TAKEN`); a name left out is the root's
//   folder name, numbered when taken;
// - audit rows (`library.add|remove|set`) carry `{libraryId, storage, fields}`, never a name,
//   root, alias or path; `set` audits only the human-only fields (Decision 47, 49).
import { FinError } from "@finisher/contracts";
import type { RootState, VolumeIdentity } from "@finisher/core-types";
import { libraryCanonicalRoot, libraryProbeRoot } from "@finisher/fin-node";
import { Effect } from "effect";
import type { StatementSync } from "node:sqlite";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import { registerChain } from "../control/migrations.ts";
import { libraries0001 } from "../control/migrations/libraries/0001.ts";
import { ulid } from "../control/ulid.ts";
import { attempt, finError, toFinError } from "../errors.ts";

registerChain("libraries", [libraries0001], { formatVersion: 1, minReaderVersion: 1 });

export type LibraryStorage = "local" | "network" | "sync";
export type LibraryState = "online" | "offline" | "scanning" | "paused";
export interface LibraryScan {
  /** `demand`: Decision 28's cadence; a number: a fixed interval in seconds. */
  readonly poll: "demand" | number;
  readonly full: "daily" | "weekly" | "manual";
}

/** The extension groups a library probes by default (Decision 37). */
export const KIND_GROUPS = ["media", "image", "svg", "lottieTemplate", "font", "lut"] as const;
/** Fields only a human may change (`library.admin`, Decision 47); the only `library.set` audit rows. */
export const HUMAN_ONLY_FIELDS = ["hydrate", "bandwidthMiBs", "acceptVolume", "acceptUnmounted", "acceptEmptyRoot", "aliases"] as const;

export interface CanonicalRoot {
  readonly root: string;
  readonly rootKey: string;
  readonly identity: VolumeIdentity;
  /** The detected storage kind: `local`, `network` or `sync:<provider>`. */
  readonly storage: string;
}

/** What the registry needs of fin-node (tests may fake it). */
export interface RootTools {
  canonicalRoot(path: string, serverData: string, timeoutMs?: number): Promise<CanonicalRoot>;
  probeRoot(root: string, stored: VolumeIdentity, timeoutMs?: number): Promise<RootState>;
}

/** The real tools: fin-node's per-root threads. */
export const finNodeRootTools: RootTools = {
  canonicalRoot: async (path, serverData, timeoutMs) =>
    JSON.parse(await libraryCanonicalRoot(path, serverData, timeoutMs)) as CanonicalRoot,
  probeRoot: async (root, stored, timeoutMs) => JSON.parse(await libraryProbeRoot(root, JSON.stringify(stored), timeoutMs)) as RootState,
};

export interface LibraryRow {
  readonly id: string;
  readonly name: string;
  /** The canonical absolute root (user content: never journaled or put in audit rows). */
  readonly root: string;
  readonly rootKey: string;
  readonly aliases: ReadonlyArray<string>;
  readonly volume: VolumeIdentity;
  /** The effective kind: the owner's override, else the detected one. */
  readonly storage: LibraryStorage;
  readonly provider?: string;
  /** What the OS reported (`local`, `network`, `sync:<provider>`). */
  readonly storageDetected: string;
  readonly storageOverride?: LibraryStorage;
  readonly state: LibraryState;
  readonly offlineReason?: string;
  readonly include?: ReadonlyArray<string>;
  readonly exclude: ReadonlyArray<string>;
  readonly kinds: ReadonlyArray<string> | "all";
  readonly scan: LibraryScan;
  readonly hydrate: boolean;
  readonly bandwidthMiBs?: number;
  readonly stages: ReadonlyArray<string>;
  /** The last full-scan generation allocated (Decision 27). */
  readonly gen: number;
  readonly createdBy: string;
  readonly createdAt: number;
  readonly lastPollAt?: number;
  readonly lastFullAt?: number;
}

export interface NewLibrary {
  readonly root: string;
  readonly name?: string;
  readonly storage?: LibraryStorage;
  readonly include?: ReadonlyArray<string>;
  readonly exclude?: ReadonlyArray<string>;
  readonly kinds?: ReadonlyArray<string> | "all";
  readonly scan?: Partial<LibraryScan>;
  readonly hydrate?: boolean;
  readonly bandwidthMiBs?: number;
  readonly aliases?: ReadonlyArray<string>;
}

/** `library.set` without `acceptVolume` (that is `setVolume`). `null` clears an optional field. */
export interface LibraryPatch {
  readonly name?: string;
  readonly include?: ReadonlyArray<string> | null;
  readonly exclude?: ReadonlyArray<string>;
  readonly kinds?: ReadonlyArray<string> | "all";
  readonly scan?: Partial<LibraryScan>;
  readonly paused?: boolean;
  readonly stages?: ReadonlyArray<string>;
  readonly aliases?: ReadonlyArray<string>;
  readonly hydrate?: boolean;
  readonly bandwidthMiBs?: number | null;
}

/** Who acts, for the audit row (`Principal`'s kind and id). */
export interface Actor {
  readonly kind: string;
  readonly id: string;
}

export interface RegistryOptions {
  /** The server data folder: no library root may hold it or lie inside it. */
  readonly serverData: string;
  readonly audit?: Audit;
  readonly tools?: RootTools;
  readonly clock?: () => number;
}

interface DbRow {
  id: string;
  name: string;
  nameKey: string;
  root: string;
  rootKey: string;
  aliases: string;
  volume: string;
  storage: string;
  storageOverride: string | null;
  state: LibraryState;
  offlineReason: string | null;
  include: string | null;
  exclude: string;
  kinds: string;
  scan: string;
  hydrate: number;
  bandwidthMiBs: number | null;
  stages: string;
  gen: number;
  createdBy: string;
  createdAt: number;
  lastPollAt: number | null;
  lastFullAt: number | null;
}

const MAX_NAME = 64;
export const STAGE_NAME = /^[a-z][a-zA-Z0-9.:-]{0,63}$/;

function invalid(field: string, why: string): FinError {
  return finError("INVALID_REQUEST", `${field}: ${why}`, { field });
}

const norm = (s: string): string => s.normalize("NFC");
const nameKeyOf = (name: string): string => norm(name).toLowerCase();

function cleanName(v: unknown): string {
  if (typeof v !== "string") throw invalid("name", "must be text");
  const name = norm(v.trim());
  // eslint-disable-next-line no-control-regex
  if (name.length === 0 || [...name].length > MAX_NAME || /[\u0000-\u001f\u007f]/.test(name)) {
    throw invalid("name", `must be 1–${MAX_NAME} characters without control characters`);
  }
  return name;
}

function textList(field: string, v: unknown, max: number, each: number): string[] {
  if (!Array.isArray(v) || v.length > max) throw invalid(field, `must be a list of at most ${max} texts`);
  for (const x of v) {
    if (typeof x !== "string" || x.length === 0 || x.length > each || x.includes("\u0000")) {
      throw invalid(field, `entries must be 1–${each} characters`);
    }
  }
  return [...new Set(v as string[])];
}

function cleanKinds(v: unknown): string[] | "all" {
  if (v === "all") return "all";
  const list = textList("kinds", v, KIND_GROUPS.length, 32);
  const unknown = list.find((k) => !(KIND_GROUPS as ReadonlyArray<string>).includes(k));
  if (unknown !== undefined || list.length === 0) throw invalid("kinds", `must be "all" or some of ${KIND_GROUPS.join(", ")}`);
  return list;
}

function cleanScan(v: unknown, base: LibraryScan): LibraryScan {
  if (typeof v !== "object" || v === null) throw invalid("scan", "must be an object");
  const { poll = base.poll, full = base.full } = v as Partial<LibraryScan>;
  if (poll !== "demand" && !(Number.isInteger(poll) && (poll as number) >= 30 && (poll as number) <= 604_800)) {
    throw invalid("scan.poll", 'must be "demand" or 30–604800 seconds');
  }
  if (full !== "daily" && full !== "weekly" && full !== "manual") throw invalid("scan.full", "must be daily, weekly or manual");
  return { poll, full };
}

function cleanBandwidth(v: unknown): number {
  if (typeof v !== "number" || !Number.isFinite(v) || v <= 0 || v > 100_000) throw invalid("bandwidthMiBs", "must be a number above 0");
  return v;
}

function cleanStages(v: unknown): string[] {
  const list = textList("stages", v, 64, 64);
  if (list.some((s) => s === "probe" || !STAGE_NAME.test(s))) throw invalid("stages", "must be stage names (`probe` is built in)");
  return list;
}

function cleanFlag(field: string, v: unknown): boolean {
  if (typeof v !== "boolean") throw invalid(field, "must be true or false");
  return v;
}

function cleanOverride(v: unknown): LibraryStorage {
  if (v !== "local" && v !== "network" && v !== "sync") throw invalid("storage", "must be local, network or sync");
  return v;
}

/** The default schedule of a storage kind (Decisions 28, 29). */
export function defaultScan(storage: LibraryStorage): LibraryScan {
  if (storage === "network") return { poll: "demand", full: "daily" };
  if (storage === "sync") return { poll: 600, full: "daily" };
  return { poll: "demand", full: "weekly" };
}

function effectiveStorage(detected: string, override: LibraryStorage | undefined): { storage: LibraryStorage; provider?: string } {
  const [kind, provider] = detected.split(":") as [string, string | undefined];
  const storage = override ?? (kind === "network" || kind === "sync" ? kind : "local");
  return storage === "sync" && provider !== undefined && kind === "sync" ? { storage, provider } : { storage };
}

/**
 * Decision 14: a Linux FUSE sync root without a provider marker (the detected provider is
 * `unknown`) is catalogued from metadata only (`skipped`, `METADATA_ONLY`) unless the owner
 * sets `hydrate`: a FUSE client downloads on any read, so even a small budget would download.
 * Task 9 passes this to the scan and probe workers.
 */
export function metadataOnly(
  lib: Pick<LibraryRow, "storage" | "provider" | "volume" | "hydrate">,
  platform: NodeJS.Platform = process.platform,
): boolean {
  return (
    platform === "linux" &&
    lib.storage === "sync" &&
    (lib.provider === undefined || lib.provider === "unknown") &&
    lib.volume.kind.startsWith("fuse") &&
    !lib.hydrate
  );
}

/** True when `outer` is `inner` or a directory above it (both `rootKey`s). */
export function keyContains(outer: string, inner: string): boolean {
  return inner === outer || (inner.startsWith(outer) && (outer.endsWith("/") || inner[outer.length] === "/"));
}

function parseRow(r: DbRow): LibraryRow {
  const override = r.storageOverride === null ? undefined : (r.storageOverride as LibraryStorage);
  const { storage, provider } = effectiveStorage(r.storage, override);
  return {
    id: r.id,
    name: r.name,
    root: r.root,
    rootKey: r.rootKey,
    aliases: JSON.parse(r.aliases) as string[],
    volume: JSON.parse(r.volume) as VolumeIdentity,
    storage,
    ...(provider === undefined ? {} : { provider }),
    storageDetected: r.storage,
    ...(override === undefined ? {} : { storageOverride: override }),
    state: r.state,
    ...(r.offlineReason === null ? {} : { offlineReason: r.offlineReason }),
    ...(r.include === null ? {} : { include: JSON.parse(r.include) as string[] }),
    exclude: JSON.parse(r.exclude) as string[],
    kinds: JSON.parse(r.kinds) as string[] | "all",
    scan: JSON.parse(r.scan) as LibraryScan,
    hydrate: r.hydrate !== 0,
    ...(r.bandwidthMiBs === null ? {} : { bandwidthMiBs: r.bandwidthMiBs }),
    stages: JSON.parse(r.stages) as string[],
    gen: r.gen,
    createdBy: r.createdBy,
    createdAt: r.createdAt,
    ...(r.lastPollAt === null ? {} : { lastPollAt: r.lastPollAt }),
    ...(r.lastFullAt === null ? {} : { lastFullAt: r.lastFullAt }),
  };
}

const COLUMNS =
  "id, name, nameKey, root, rootKey, aliases, volume, storage, storageOverride, state, offlineReason, include, exclude, kinds, scan, " +
  "hydrate, bandwidthMiBs, stages, gen, createdBy, createdAt, lastPollAt, lastFullAt";

function validateNew(input: NewLibrary) {
  const override = input.storage === undefined ? undefined : cleanOverride(input.storage);
  const name = input.name === undefined ? undefined : cleanName(input.name);
  const exclude = input.exclude === undefined ? [] : textList("exclude", input.exclude, 256, 512);
  const include = input.include === undefined ? undefined : textList("include", input.include, 256, 512);
  const kinds = input.kinds === undefined ? [...KIND_GROUPS] : cleanKinds(input.kinds);
  const aliases = input.aliases === undefined ? [] : textList("aliases", input.aliases, 32, 4096);
  const bandwidth = input.bandwidthMiBs === undefined ? undefined : cleanBandwidth(input.bandwidthMiBs);
  const hydrate = input.hydrate === undefined ? false : cleanFlag("hydrate", input.hydrate);
  if (input.scan !== undefined) cleanScan(input.scan, defaultScan("local"));
  return { override, name, exclude, include, kinds, aliases, bandwidth, hydrate };
}

export class Registry {
  readonly #store: ControlStore;
  readonly #serverData: string;
  /** The server data folder (`removeLibrary` deletes a catalog below it). */
  get serverData(): string {
    return this.#serverData;
  }
  readonly #audit: Audit | undefined;
  readonly #tools: RootTools;
  readonly #clock: () => number;
  readonly #stmts = new Map<string, StatementSync>();

  constructor(store: ControlStore, opts: RegistryOptions) {
    this.#store = store;
    this.#serverData = opts.serverData;
    this.#audit = opts.audit;
    this.#tools = opts.tools ?? finNodeRootTools;
    this.#clock = opts.clock ?? Date.now;
  }

  #stmt(sql: string): StatementSync {
    let s = this.#stmts.get(sql);
    if (s === undefined) {
      s = this.#store.db.prepare(sql);
      this.#stmts.set(sql, s);
    }
    return s;
  }

  get(id: string): LibraryRow | undefined {
    const r = this.#stmt(`SELECT ${COLUMNS} FROM libraries WHERE id = ?`).get(id) as DbRow | undefined;
    return r === undefined ? undefined : parseRow(r);
  }

  /** Case-insensitive (`LIBRARY_NAME_TAKEN` keeps names unique by this key). */
  byName(name: string): LibraryRow | undefined {
    const r = this.#stmt(`SELECT ${COLUMNS} FROM libraries WHERE nameKey = ?`).get(nameKeyOf(name)) as DbRow | undefined;
    return r === undefined ? undefined : parseRow(r);
  }

  /** Commands take an id or a name (Decision 48): the id wins. */
  resolve(ref: string): LibraryRow | undefined {
    return this.get(ref) ?? this.byName(ref);
  }

  list(): LibraryRow[] {
    return (this.#stmt(`SELECT ${COLUMNS} FROM libraries ORDER BY createdAt, id`).all() as unknown as DbRow[]).map(parseRow);
  }

  #require(id: string): LibraryRow {
    const row = this.get(id);
    if (row === undefined) throw finError("LIBRARY_NOT_FOUND", "there is no such library", { id });
    return row;
  }

  /** Adds a library (Decision 48); the caller starts its first scan. Audits `library.add`. */
  add(input: NewLibrary, actor: Actor): Effect.Effect<LibraryRow, FinError> {
    return Effect.try({ try: () => validateNew(input), catch: toFinError }).pipe(
      Effect.flatMap((fields) =>
        attempt(() => this.#tools.canonicalRoot(input.root, this.#serverData)).pipe(
          Effect.flatMap((canon) => Effect.try({ try: () => this.#insert(input, fields, canon, actor), catch: toFinError })),
        ),
      ),
    );
  }

  #insert(input: NewLibrary, f: ReturnType<typeof validateNew>, canon: CanonicalRoot, actor: Actor): LibraryRow {
    const { storage } = effectiveStorage(canon.storage, f.override);
    const scan = cleanScan(input.scan ?? {}, defaultScan(storage));
    const now = this.#clock();
    const id = ulid(now);
    const row = this.#store.tx(() => {
      for (const other of this.list()) {
        if (keyContains(other.rootKey, canon.rootKey) || keyContains(canon.rootKey, other.rootKey)) {
          throw finError("LIBRARY_OVERLAP", "the folder overlaps another library", { libraryId: other.id });
        }
      }
      const name = f.name ?? this.#derivedName(canon.root);
      if (this.byName(name) !== undefined) throw finError("LIBRARY_NAME_TAKEN", "a library with this name exists", { name });
      this.#stmt(
        "INSERT INTO libraries (id, name, nameKey, root, rootKey, aliases, volume, storage, storageOverride, state, include, exclude, kinds, " +
          "scan, hydrate, bandwidthMiBs, stages, gen, createdBy, createdAt) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'online', ?, ?, ?, ?, ?, ?, '[]', 0, ?, ?)",
      ).run(
        id,
        name,
        nameKeyOf(name),
        canon.root,
        canon.rootKey,
        JSON.stringify(f.aliases),
        JSON.stringify(canon.identity),
        canon.storage,
        f.override ?? null,
        f.include === undefined ? null : JSON.stringify(f.include),
        JSON.stringify(f.exclude),
        JSON.stringify(f.kinds),
        JSON.stringify(scan),
        f.hydrate ? 1 : 0,
        f.bandwidth ?? null,
        `${actor.kind}:${actor.id}`,
        now,
      );
      return this.#require(id);
    });
    this.#audit?.append({ kind: "library.add", principal: actor, outcome: "ok", details: { libraryId: id, storage: row.storage } });
    return row;
  }

  /** The root's folder name, numbered (`Name 2`) while taken. */
  #derivedName(root: string): string {
    const base = norm(root.split(/[\\/]/).filter((s) => s.length > 0).pop() ?? "").trim();
    const stem = base.length === 0 || [...base].length > MAX_NAME - 4 ? "Library" : base;
    for (let n = 1; ; n++) {
      const candidate = n === 1 ? stem : `${stem} ${n}`;
      if (this.byName(candidate) === undefined) return candidate;
    }
  }

  /** Changes registry-owned fields. Audits `library.set` when a human-only field changed. */
  set(id: string, patch: LibraryPatch, actor: Actor): Effect.Effect<LibraryRow, FinError> {
    return Effect.try({ try: () => this.#set(id, patch, actor), catch: toFinError });
  }

  #set(id: string, p: LibraryPatch, actor: Actor): LibraryRow {
    const sets: string[] = [];
    const args: Array<string | number | null> = [];
    const put = (column: string, value: string | number | null): void => {
      sets.push(`${column} = ?`);
      args.push(value);
    };
    const row = this.#require(id);
    const touched: string[] = [];
    if (p.name !== undefined) {
      const name = cleanName(p.name);
      const clash = this.byName(name);
      if (clash !== undefined && clash.id !== id) throw finError("LIBRARY_NAME_TAKEN", "a library with this name exists", { name });
      put("name", name);
      put("nameKey", nameKeyOf(name));
    }
    if (p.include !== undefined) put("include", p.include === null ? null : JSON.stringify(textList("include", p.include, 256, 512)));
    if (p.exclude !== undefined) put("exclude", JSON.stringify(textList("exclude", p.exclude, 256, 512)));
    if (p.kinds !== undefined) put("kinds", JSON.stringify(cleanKinds(p.kinds)));
    if (p.scan !== undefined) put("scan", JSON.stringify(cleanScan(p.scan, row.scan)));
    if (p.stages !== undefined) put("stages", JSON.stringify(cleanStages(p.stages)));
    if (p.paused !== undefined) {
      // Pausing keeps an offline reason; resuming returns to what the library was.
      put("state", cleanFlag("paused", p.paused) ? "paused" : row.offlineReason === undefined ? "online" : "offline");
    }
    if (p.aliases !== undefined) {
      put("aliases", JSON.stringify(textList("aliases", p.aliases, 32, 4096)));
      touched.push("aliases");
    }
    if (p.hydrate !== undefined) {
      put("hydrate", cleanFlag("hydrate", p.hydrate) ? 1 : 0);
      touched.push("hydrate");
    }
    if (p.bandwidthMiBs !== undefined) {
      put("bandwidthMiBs", p.bandwidthMiBs === null ? null : cleanBandwidth(p.bandwidthMiBs));
      touched.push("bandwidthMiBs");
    }
    if (sets.length === 0) return row;
    const updated = this.#store.tx(() => {
      this.#store.db.prepare(`UPDATE libraries SET ${sets.join(", ")} WHERE id = ?`).run(...args, id);
      return this.#require(id);
    });
    if (touched.length > 0) {
      this.#audit?.append({
        kind: "library.set",
        principal: actor,
        outcome: "ok",
        details: { libraryId: id, storage: updated.storage, fields: touched },
      });
    }
    return updated;
  }

  /** Deletes the row (the service cancels jobs and removes catalog files first). Audits `library.remove`. */
  remove(id: string, actor: Actor): Effect.Effect<void, FinError> {
    return Effect.try({
      try: () => {
        const row = this.#require(id);
        this.#stmt("DELETE FROM libraries WHERE id = ?").run(id);
        this.#audit?.append({ kind: "library.remove", principal: actor, outcome: "ok", details: { libraryId: id, storage: row.storage } });
      },
      catch: toFinError,
    });
  }

  // The service's writes (Task 10): state, the measured volume, generations and poll times.

  setState(id: string, state: LibraryState, reason?: string): void {
    this.#stmt("UPDATE libraries SET state = ?, offlineReason = ? WHERE id = ?").run(state, state === "offline" ? (reason ?? null) : null, id);
  }

  /**
   * Stores the volume measured now (each full scan; a confirmed harmless key change, Decision
   * 17). With `actor`, the owner accepted a changed volume: audited as `library.set {fields:
   * ["acceptVolume"]}`.
   */
  setVolume(id: string, volume: VolumeIdentity, actor?: Actor): void {
    const row = this.#require(id);
    this.#stmt("UPDATE libraries SET volume = ? WHERE id = ?").run(JSON.stringify(volume), id);
    if (actor !== undefined) {
      this.#audit?.append({
        kind: "library.set",
        principal: actor,
        outcome: "ok",
        details: { libraryId: id, storage: row.storage, fields: ["acceptVolume"] },
      });
    }
  }

  /**
   * Audits an owner's accept as `library.set {fields: [field]}` (`acceptUnmounted`,
   * `acceptEmptyRoot`): the field's name only, never the directory.
   */
  auditAccept(id: string, field: "acceptUnmounted" | "acceptEmptyRoot", actor: Actor): void {
    const row = this.#require(id);
    this.#audit?.append({ kind: "library.set", principal: actor, outcome: "ok", details: { libraryId: id, storage: row.storage, fields: [field] } });
  }

  /** Allocates the generation of the next full scan (Decision 27). */
  nextGen(id: string): number {
    const r = this.#stmt("UPDATE libraries SET gen = gen + 1 WHERE id = ? RETURNING gen").get(id) as { gen: number } | undefined;
    if (r === undefined) throw finError("LIBRARY_NOT_FOUND", "there is no such library", { id });
    return r.gen;
  }

  touch(id: string, at: { readonly lastPollAt?: number; readonly lastFullAt?: number }): void {
    this.#stmt("UPDATE libraries SET lastPollAt = COALESCE(?, lastPollAt), lastFullAt = COALESCE(?, lastFullAt) WHERE id = ?").run(
      at.lastPollAt ?? null,
      at.lastFullAt ?? null,
      id,
    );
  }

  /**
   * The root's volume measured now (fin-node's `libraryCanonicalRoot`), for an owner's
   * `acceptVolume`: the root must still resolve to itself.
   */
  measure(row: LibraryRow): Effect.Effect<VolumeIdentity, FinError> {
    return attempt(() => this.#tools.canonicalRoot(row.root, this.#serverData)).pipe(
      Effect.flatMap((c) =>
        c.rootKey === row.rootKey ? Effect.succeed(c.identity) : Effect.fail(finError("LIBRARY_ROOT_INVALID", "the folder now resolves elsewhere", { id: row.id })),
      ),
    );
  }

  /** A library's root state now (fin-node's per-root probe thread, Decision 3). */
  probe(row: LibraryRow, timeoutMs?: number): Effect.Effect<RootState, FinError> {
    return attempt(() => this.#tools.probeRoot(row.root, row.volume, timeoutMs));
  }
}
