// The library registry in the `libraries` chain (5D plan Task 8): root validation through
// fin-node's real canonicaliser, names, storage, audit privacy (Decisions 3, 14, 15, 17, 48, 49).
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FinError, auditDetailsValid } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Audit } from "../../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../../src/control/db.ts";
import { CONTROL_CHAINS } from "../../src/control/migrations.ts";
import {
  type Actor,
  type LibraryRow,
  type NewLibrary,
  Registry,
  defaultScan,
  keyContains,
  metadataOnly,
} from "../../src/library/registry.ts";

let base: string;
let serverData: string;
let store: ControlStore;
let audit: Audit;
let registry: Registry;
let now = 1_000_000;
const clock = (): number => now;
const human: Actor = { kind: "local", id: "console" };

beforeEach(() => {
  base = realpathSync(mkdtempSync(join(tmpdir(), "libraries-")));
  serverData = join(base, "data");
  mkdirSync(serverData);
  mkdirSync(join(base, "libs"));
  now = 1_000_000;
  open();
});

afterEach(async () => {
  await store.close();
  rmSync(base, { recursive: true, force: true });
});

function open(): void {
  store = openControlStore(serverData, { serverId: "sid-1", checkpoint: false, clock });
  audit = new Audit(store, clock);
  registry = new Registry(store, { serverData, audit, clock });
}

function dir(...parts: string[]): string {
  const p = join(base, "libs", ...parts);
  mkdirSync(p, { recursive: true });
  return p;
}

const run = <A>(e: Effect.Effect<A, FinError>): Promise<A> => Effect.runPromise(e);

async function failure(e: Effect.Effect<unknown, FinError>): Promise<FinError> {
  const r = await Effect.runPromise(Effect.flip(e));
  expect(r).toBeInstanceOf(FinError);
  return r;
}

const add = (input: NewLibrary): Promise<LibraryRow> => run(registry.add(input, human));

describe("registry", () => {
  test("libraries_chain_registers", () => {
    expect(CONTROL_CHAINS.get("libraries")?.formatVersion).toBe(1);
    const row = store.db.prepare("SELECT value FROM meta WHERE key = 'formatVersion.libraries'").get() as { value: string } | undefined;
    expect(row?.value).toBe("1");
  });

  test("add_validates_root", async () => {
    const reason = async (root: string): Promise<unknown> => {
      const e = await failure(registry.add({ root }, human));
      expect(e.code).toBe("LIBRARY_ROOT_INVALID");
      return (e.details as { reason?: string }).reason;
    };
    expect(await reason("relative/dir")).toBe("relative");
    expect(await reason(join(base, "libs", "missing"))).toBe("notFound");
    const file = join(base, "libs", "a-file");
    writeFileSync(file, "x");
    expect(await reason(file)).toBe("notDirectory");
    // The server data, and anything holding it, is never a library.
    expect(await reason(serverData)).toBe("serverData");
    mkdirSync(join(serverData, "inner"));
    expect(await reason(join(serverData, "inner"))).toBe("serverData");
    expect(await reason(base)).toBe("serverData");
    expect(registry.list()).toEqual([]);
  });

  test("symlinked_roots_are_canonicalised", async () => {
    const real = dir("real");
    // A junction on Windows: a directory symlink needs a privilege a non-elevated user lacks.
    symlinkSync(real, join(base, "libs", "link"), process.platform === "win32" ? "junction" : "dir");
    const lib = await add({ root: join(base, "libs", "link") });
    expect(lib.root).toBe(real);
    // The same folder through its link overlaps itself.
    expect((await failure(registry.add({ root: join(base, "libs", "link") }, human))).code).toBe("LIBRARY_OVERLAP");
  });

  test("overlap_is_refused_in_both_directions", async () => {
    await add({ root: dir("media", "a"), name: "A" });
    const above = await failure(registry.add({ root: dir("media"), name: "Above" }, human));
    expect(above.code).toBe("LIBRARY_OVERLAP");
    await add({ root: dir("other", "x"), name: "X" });
    const below = await failure(registry.add({ root: dir("other", "x", "y"), name: "Below" }, human));
    expect(below.code).toBe("LIBRARY_OVERLAP");
    // A sibling whose name merely starts the same is not an overlap.
    const sibling = await add({ root: dir("media", "ab"), name: "AB" });
    expect(sibling.root).toBe(join(base, "libs", "media", "ab"));
    expect(registry.list()).toHaveLength(3);
  });

  test("overlap_is_found_across_case_variants_on_case_insensitive_volumes", async () => {
    const lib = await add({ root: dir("Media", "A"), name: "A" });
    // The display root keeps the case it was resolved with; only the lookup key is folded.
    expect(lib.root.endsWith(join("Media", "A"))).toBe(true);
    if (lib.volume.caseSensitive) return; // `MEDIA` is another folder there: nothing to compare.
    for (const variant of [join(base, "libs", "MEDIA"), join(base, "libs", "media", "a"), join(base, "libs", "MEDIA", "a", "..")]) {
      expect((await failure(registry.add({ root: variant, name: "V" }, human))).code).toBe("LIBRARY_OVERLAP");
    }
    expect(registry.list()).toHaveLength(1);
  });

  test("key_containment_is_by_component", () => {
    expect(keyContains("/a/b", "/a/b/c")).toBe(true);
    expect(keyContains("/a/b", "/a/bc")).toBe(false);
    expect(keyContains("/", "/a")).toBe(true);
    expect(keyContains("/a/b/c", "/a/b")).toBe(false);
  });

  test("names_unique_case_insensitive", async () => {
    await add({ root: dir("one"), name: "Footage" });
    const e = await failure(registry.add({ root: dir("two"), name: "fOOTAGE" }, human));
    expect(e.code).toBe("LIBRARY_NAME_TAKEN");
    // NFC and NFD spellings are one name.
    await add({ root: dir("three"), name: "Café" });
    expect((await failure(registry.add({ root: dir("four"), name: "Café" }, human))).code).toBe("LIBRARY_NAME_TAKEN");
    // A name left out is the folder's, numbered while taken.
    const a = await add({ root: dir("x", "Shots") });
    const b = await add({ root: dir("y", "Shots") });
    expect([a.name, b.name]).toEqual(["Shots", "Shots 2"]);
    // Commands take an id or a name.
    expect(registry.resolve(b.id)?.id).toBe(b.id);
    expect(registry.resolve("shots 2")?.id).toBe(b.id);
    expect(registry.resolve("nope")).toBeUndefined();
    // Renaming into a taken name fails; into its own (any case) works.
    expect((await failure(registry.set(b.id, { name: "SHOTS" }, human))).code).toBe("LIBRARY_NAME_TAKEN");
    expect((await run(registry.set(b.id, { name: "SHOTS 2" }, human))).name).toBe("SHOTS 2");
  });

  test("storage_detected_with_override", async () => {
    const plain = await add({ root: dir("plain") });
    expect(plain.storageDetected).toBe("local");
    expect(plain.storage).toBe("local");
    expect(plain.storageOverride).toBeUndefined();
    expect(plain.scan).toEqual(defaultScan("local"));
    const net = await add({ root: dir("net"), storage: "network" });
    // The override wins; the detection stays on record.
    expect(net.storage).toBe("network");
    expect(net.storageDetected).toBe("local");
    expect(net.storageOverride).toBe("network");
    expect(net.scan).toEqual({ poll: "demand", full: "daily" });
    const sync = await add({ root: dir("sync"), storage: "sync" });
    expect(sync.scan).toEqual({ poll: 600, full: "daily" });
    expect((await failure(registry.add({ root: dir("bad"), storage: "cloud" as never }, human))).code).toBe("INVALID_REQUEST");
  });

  test("linux_fuse_sync_is_metadata_only", () => {
    const fuse = (kind: string) => ({ kind }) as never;
    const base = { storage: "sync", provider: "unknown", hydrate: false } as const;
    expect(metadataOnly({ ...base, volume: fuse("fuse.rclone") }, "linux")).toBe(true);
    // The owner's `hydrate` opts in; a known provider's marker, another OS or a local
    // filesystem keep the normal behaviour.
    expect(metadataOnly({ ...base, hydrate: true, volume: fuse("fuse.rclone") }, "linux")).toBe(false);
    expect(metadataOnly({ ...base, provider: "dropbox", volume: fuse("fuse.rclone") }, "linux")).toBe(false);
    expect(metadataOnly({ ...base, volume: fuse("fuse.rclone") }, "darwin")).toBe(false);
    expect(metadataOnly({ ...base, volume: fuse("ext4") }, "linux")).toBe(false);
    expect(metadataOnly({ ...base, storage: "network", volume: fuse("fuse.sshfs") }, "linux")).toBe(false);
  });

  test("defaults_and_validation", async () => {
    const lib = await add({ root: dir("d") });
    expect(lib.kinds).toEqual(["media", "image", "svg", "lottieTemplate", "font", "lut"]);
    expect(lib).toMatchObject({ exclude: [], aliases: [], stages: [], hydrate: false, state: "online", gen: 0, createdAt: now });
    expect(lib.createdBy).toBe("local:console");
    expect(lib.include).toBeUndefined();
    expect(lib.volume.key).toMatch(/^[0-9a-f]{32}$/);
    const bad: Array<[Partial<NewLibrary>, string]> = [
      [{ name: "" }, "name"],
      [{ name: "x\u0000y" }, "name"],
      [{ kinds: ["audio"] }, "kinds"],
      [{ kinds: [] }, "kinds"],
      [{ scan: { poll: 5 } }, "scan.poll"],
      [{ scan: { full: "hourly" as never } }, "scan.full"],
      [{ bandwidthMiBs: 0 }, "bandwidthMiBs"],
      [{ exclude: [""] }, "exclude"],
      [{ aliases: "x" as never }, "aliases"],
    ];
    for (const [over, field] of bad) {
      const e = await failure(registry.add({ root: dir("v"), ...over }, human));
      expect(e).toMatchObject({ code: "INVALID_REQUEST", details: { field } });
    }
    expect(await add({ root: dir("v"), kinds: "all", scan: { poll: 90, full: "manual" }, bandwidthMiBs: 20 })).toMatchObject({
      kinds: "all",
      scan: { poll: 90, full: "manual" },
      bandwidthMiBs: 20,
    });
  });

  test("set_changes_fields_and_pause_keeps_the_offline_reason", async () => {
    const lib = await add({ root: dir("s"), name: "S" });
    const set = (p: Parameters<Registry["set"]>[1]): Promise<LibraryRow> => run(registry.set(lib.id, p, human));
    expect(await set({ include: ["**/*.mov"], exclude: ["tmp/**"], kinds: ["media"], stages: ["proxy540"] })).toMatchObject({
      include: ["**/*.mov"],
      exclude: ["tmp/**"],
      kinds: ["media"],
      stages: ["proxy540"],
    });
    expect((await set({ include: null })).include).toBeUndefined();
    expect((await set({ scan: { poll: 120 } })).scan).toEqual({ poll: 120, full: "weekly" });
    expect((await set({ paused: true })).state).toBe("paused");
    expect((await set({ paused: false })).state).toBe("online");
    registry.setState(lib.id, "offline", "ROOT_UNMOUNTED");
    expect(await set({ paused: true })).toMatchObject({ state: "paused", offlineReason: "ROOT_UNMOUNTED" });
    expect(await set({ paused: false })).toMatchObject({ state: "offline", offlineReason: "ROOT_UNMOUNTED" });
    expect((await failure(registry.set(lib.id, { stages: ["probe"] }, human))).code).toBe("INVALID_REQUEST");
    expect((await failure(registry.set("missing-id", { name: "Z" }, human))).code).toBe("LIBRARY_NOT_FOUND");
    expect(await set({})).toMatchObject({ id: lib.id });
  });

  test("service_writes_persist_across_a_restart", async () => {
    const lib = await add({ root: dir("p"), name: "P" });
    expect(registry.nextGen(lib.id)).toBe(1);
    expect(registry.nextGen(lib.id)).toBe(2);
    registry.touch(lib.id, { lastPollAt: 5 });
    registry.touch(lib.id, { lastFullAt: 7 });
    registry.setVolume(lib.id, { ...lib.volume, key: "k".repeat(32) });
    registry.setState(lib.id, "offline", "ROOT_VOLUME_CHANGED");
    await store.close();
    open();
    expect(registry.get(lib.id)).toMatchObject({
      gen: 2,
      lastPollAt: 5,
      lastFullAt: 7,
      state: "offline",
      offlineReason: "ROOT_VOLUME_CHANGED",
      volume: { key: "k".repeat(32) },
    });
    registry.setState(lib.id, "online");
    expect(registry.get(lib.id)?.offlineReason).toBeUndefined();
    expect(() => registry.nextGen("missing-id")).toThrow(/no such library/);
  });

  test("remove_deletes_the_row_and_frees_name_and_root", async () => {
    const root = dir("r");
    const lib = await add({ root, name: "R" });
    await run(registry.remove(lib.id, human));
    expect(registry.get(lib.id)).toBeUndefined();
    expect((await failure(registry.remove(lib.id, human))).code).toBe("LIBRARY_NOT_FOUND");
    expect((await add({ root, name: "R" })).id).not.toBe(lib.id);
  });

  test("audit_rows_carry_no_names_or_paths", async () => {
    const secretRoot = dir("Client Footage 2026");
    const lib = await add({ root: secretRoot, name: "Wedding Smith", aliases: ["/Volumes/secret-share/Media"] });
    await run(registry.set(lib.id, { name: "Renamed Wedding", include: ["**/*.mov"] }, human)); // not human-only: no row
    await run(registry.set(lib.id, { hydrate: true, bandwidthMiBs: 10, aliases: ["/mnt/secret"] }, human));
    registry.setVolume(lib.id, lib.volume, human);
    await run(registry.remove(lib.id, human));
    const rows = audit.list().entries.filter((e) => e.kind.startsWith("library.")).reverse();
    expect(rows.map((r) => r.kind)).toEqual(["library.add", "library.set", "library.set", "library.remove"]);
    expect(rows.map((r) => r.details)).toEqual([
      { libraryId: lib.id, storage: "local" },
      { libraryId: lib.id, storage: "local", fields: ["aliases", "hydrate", "bandwidthMiBs"] },
      { libraryId: lib.id, storage: "local", fields: ["acceptVolume"] },
      { libraryId: lib.id, storage: "local" },
    ]);
    const text = JSON.stringify(rows);
    for (const secret of ["Wedding", "Client", "Footage", "secret", "Renamed", base, "/mnt"]) expect(text).not.toContain(secret);
    expect(audit.failures()).toBe(0);
    // The closed schema itself refuses names and paths.
    expect(auditDetailsValid("library.add", { libraryId: lib.id, storage: "local" })).toBe(true);
    expect(auditDetailsValid("library.add", { libraryId: lib.id, name: "x" })).toBe(false);
    expect(auditDetailsValid("library.set", { libraryId: lib.id, root: "/x" })).toBe(false);
    expect(auditDetailsValid("library.set", { libraryId: lib.id, fields: ["not a word"] })).toBe(false);
  });
});
