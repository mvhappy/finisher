// `Libraries.locate` (plan 5D Task 11; Decision 45): a path under a root or an alias, a
// fingerprint, a full hash; ambiguous keys and cloud-only files. Real service, engine and roots.
import { rmSync, utimesSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { addLibrary, scanLibrary } from "../../src/library/admin.ts";
import type { FileRow } from "../../src/library/catalog/core/queries.ts";
import { foldKey, relUnder } from "../../src/library/locate.ts";
import { exec } from "../../src/library/service.ts";
import { PNG_1X1, overlay, tree } from "./support/kinds.ts";
import { closeLiveRigs, liveRig, liveRoots } from "./support/live.ts";

afterEach(closeLiveRigs, 60_000);

const ACTOR = { kind: "local", id: "test" } as const;
const WAIT = 60_000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function eventually<A>(f: () => Promise<A | undefined | false>, ms = WAIT): Promise<A> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() > end) throw new Error("condition not met in time");
    await sleep(50);
  }
}

describe("paths", () => {
  test("rel_under_roots_and_aliases", () => {
    expect(relUnder("/srv/media", "/srv/media/a/b.mov")).toEqual(["a", "b.mov"]);
    expect(relUnder("/srv/media/", "/srv/media//a/b.mov")).toEqual(["a", "b.mov"]);
    expect(relUnder("/srv/media", "/srv/media")).toBeUndefined();
    expect(relUnder("/srv/media", "/srv/mediaX/a")).toBeUndefined();
    expect(relUnder("/srv/media", "/SRV/media/a")).toBeUndefined(); // POSIX compares exactly
    expect(relUnder("\\\\server\\media", "\\\\SERVER\\Media\\a\\b.mov")).toEqual(["a", "b.mov"]);
    expect(relUnder("C:\\Media", "c:/media/x.mov")).toEqual(["x.mov"]);
    expect(() => relUnder("/srv/media", "/srv/media/a/../b")).toThrow(/\.\./);
    expect(foldKey("Café.MOV", true)).toBe("Café.MOV");
    expect(foldKey("Café.MOV", false)).toBe("café.mov");
  });
});

describe("locate", () => {
  test("locate_by_path_alias_fp_hash", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "lib");
    tree(root, { sub: ["a.png", "b.png"] });
    utimesSync(join(root, "sub", "b.png"), 1_700_000_100, 1_700_000_100);
    const rig = await liveRig();
    const { library, job } = await addLibrary(rig.svc, { root, aliases: ["/mnt/media", "\\\\server\\media"] }, ACTOR);
    expect((await rig.t.wait(job!.id, WAIT)).state).toBe("succeeded");
    const lib = library.id;
    const rows = await eventually(async () => {
      const dir = (await exec(rig.catalogs.reader.files(lib, { dir: "sub", limit: 10 }))).files;
      return dir.length === 2 && dir.every((f) => f.fp !== null) ? dir : undefined;
    });
    const a = rows.find((r) => r.nameNfc === "a.png") as FileRow;
    const located = await Effect.runPromise(rig.svc.locate({ path: join(root, "sub", "a.png") }));
    expect(located).toMatchObject({
      library: lib, fileId: a.id, root, display: "sub/a.png", match: "path", state: "present", offline: false, hydrate: false,
      rel: { components: [Buffer.from("sub").toString("base64"), Buffer.from("a.png").toString("base64")] },
      expect: { size: a.size, mtimeNs: a.mtimeNs, fileId: a.fileId },
    });
    expect(await Effect.runPromise(rig.svc.locate({ path: "/mnt/media/sub/a.png" }))).toMatchObject({ fileId: a.id, match: "alias" });
    expect(await Effect.runPromise(rig.svc.locate({ path: "\\\\SERVER\\media\\sub\\a.png" }))).toMatchObject({ fileId: a.id, match: "alias" });
    expect(await Effect.runPromise(rig.svc.locate({ fingerprint: a.fp! }))).toMatchObject({ fileId: a.id, match: "fingerprint" });
    // A caller's reach filters the libraries searched: by path, by alias and by content alike.
    const none = () => false;
    expect(await Effect.runPromise(rig.svc.locate({ path: join(root, "sub", "a.png") }, none))).toBeUndefined();
    expect(await Effect.runPromise(rig.svc.locate({ path: "/mnt/media/sub/a.png" }, none))).toBeUndefined();
    expect(await Effect.runPromise(rig.svc.locate({ fingerprint: a.fp! }, none))).toBeUndefined();
    expect(await Effect.runPromise(rig.svc.locate({ fingerprint: a.fp! }, (id) => id === lib))).toMatchObject({ fileId: a.id });
    // A path nobody holds falls through to the fingerprint.
    expect(await Effect.runPromise(rig.svc.locate({ path: "/elsewhere/a.png", fingerprint: a.fp! }))).toMatchObject({ fileId: a.id, match: "fingerprint" });
    // A full hash, once `library.hash` stored it.
    const hashJob = await Effect.runPromise(rig.svc.hash(lib, [a.id]));
    expect((await rig.t.wait(hashJob.id, WAIT)).state).toBe("succeeded");
    const hashed = await eventually(async () => (await exec(rig.catalogs.reader.file(lib, { fileId: a.id }))).matches[0]?.fullHash ?? undefined);
    expect(await Effect.runPromise(rig.svc.locate({ fullHash: hashed }))).toMatchObject({ fileId: a.id, match: "hash" });
    expect(await Effect.runPromise(rig.svc.locate({ path: join(root, "sub", "none.png") }))).toBeUndefined();
    expect(await Effect.runPromise(rig.svc.locate({ fingerprint: "fp1:none" }))).toBeUndefined();
    for (const path of ["sub/a.png", `${root}/sub/../sub/a.png`]) {
      expect((await Effect.runPromise(Effect.flip(rig.svc.locate({ path })))).code).toBe("INVALID_PATH");
    }
    // A file gone from its path: the path still answers its missing row; its moved copy wins by fingerprint.
    const b = rows.find((r) => r.nameNfc === "b.png") as FileRow;
    writeFileSync(join(root, "b-moved.png"), PNG_1X1);
    utimesSync(join(root, "b-moved.png"), 1_700_000_100, 1_700_000_100);
    rmSync(join(root, "sub", "b.png"));
    const scan = await scanLibrary(rig.svc, lib, { full: true });
    expect((await rig.t.wait(scan.id, WAIT)).state).toBe("succeeded");
    const moved = await eventually(async () => {
      const r = await Effect.runPromise(rig.svc.locate({ path: join(root, "sub", "b.png"), fingerprint: b.fp! }));
      return r?.display === "b-moved.png" ? r : undefined;
    });
    expect(moved).toMatchObject({ fileId: b.id, match: "fingerprint", state: "present" });
    rmSync(join(root, "b-moved.png"));
    const scan2 = await scanLibrary(rig.svc, lib, { full: true });
    expect((await rig.t.wait(scan2.id, WAIT)).state).toBe("succeeded");
    expect(await Effect.runPromise(rig.svc.locate({ path: join(root, "b-moved.png") }))).toMatchObject({ fileId: b.id, match: "path", state: "missing" });
  });

  test("locate_ambiguous_and_cloud_only", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "lib");
    tree(root, { "": ["cloud.png"] });
    // NFC and NFD twins share one key (`PATH_AMBIGUOUS`); macOS cannot hold both names in one directory.
    const twins = process.platform !== "darwin";
    if (twins) {
      writeFileSync(join(root, "caf\u00e9.png"), PNG_1X1);
      writeFileSync(join(root, "cafe\u0301.png"), PNG_1X1);
    } else {
      console.warn("locate_ambiguous_and_cloud_only: NFC/NFD twins need a filesystem that keeps both names (not APFS); the ambiguity part is skipped");
    }
    overlay(root, { paths: { "cloud.png": { placeholder: "dataless" } } });
    const rig = await liveRig();
    const { library, job } = await addLibrary(rig.svc, { root }, ACTOR);
    expect((await rig.t.wait(job!.id, WAIT)).state).toBe("succeeded");
    const cloud = await Effect.runPromise(rig.svc.locate({ path: join(root, "cloud.png") }));
    expect(cloud).toMatchObject({ library: library.id, state: "cloudOnly", match: "path", offline: false });
    if (twins) {
      const err = await Effect.runPromise(Effect.flip(rig.svc.locate({ path: join(root, "caf\u00e9.png") })));
      expect(err.code).toBe("PATH_AMBIGUOUS");
      expect((err.details as { fileIds: number[] }).fileIds).toHaveLength(2);
    }
  });
});
