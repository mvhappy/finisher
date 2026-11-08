// Plugin packs' font catalogs in the font manager (8A Task 12a): they join the built-in
// catalog by entry id under its rules, leave it again without touching pins, and a pack that
// clashes is left out. Fonts come from a loopback server.
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Engine, FontManager, testHooks } from "../index.js";
import { type CatalogFile, catalogJson, catalogPin, FontServer, sha256, waitFor } from "./fontserver.ts";

const hooks = testHooks!;
let tmp: string;
let serverData: string;
let server: FontServer;
let engine: Engine | undefined;
const ENV = ["FIN_FONTS_TEST_BACKOFF_MS", "FIN_FONTS_TEST_NO_PROXY"];

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-fonts-packs-"));
  serverData = join(tmp, "server-data");
  server = await FontServer.start();
  process.env.FIN_FONTS_TEST_BACKOFF_MS = "0";
  process.env.FIN_FONTS_TEST_NO_PROXY = "1";
});

afterEach(async () => {
  await engine?.close();
  engine = undefined;
  await server.close();
  for (const k of ENV) delete process.env[k];
  rmSync(tmp, { recursive: true, force: true });
});

/** A catalog file served by the test server: the `sans` test font, or a padded one (`seed`). */
const file = (id: string, seed: number | "sans"): CatalogFile => {
  const bytes = hooks.testFont(seed === "sans" ? "sans" : `big:4096:${seed}`);
  return { id, bytes, url: server.serve(`${id}.ttf`, bytes) };
};
const pack = (id: string, ...files: CatalogFile[]) => ({ id, catalogs: [catalogJson(files)] });
type Rejected = { rejected: Array<{ id: string; code: string; details: { rule?: string; id?: string } }> };
const setPacks = (m: FontManager, ...packs: ReturnType<typeof pack>[]): Rejected => JSON.parse(m.setExtraCatalogs(JSON.stringify(packs))) as Rejected;
const catalogOf = (m: FontManager) => JSON.parse(m.catalog()) as { catalogVersion: string; fallback: string[]; entries: Array<{ id: string }> };
const cached = async (m: FontManager, sha: string) => {
  const s = JSON.parse(await m.status()) as { files: Array<{ sha256: string; entry?: string; state: { state: string } }> };
  return s.files.find((f) => f.sha256 === sha);
};

describe("pack font catalogs", () => {
  test("pack_fonts_join_the_catalog", async () => {
    const m = FontManager.open(serverData);
    const builtin = catalogOf(m);
    const sans = file("acme-sans", "sans");
    expect(setPacks(m, pack("@acme/fonts", sans))).toEqual({ rejected: [] });

    const merged = catalogOf(m);
    expect(merged.entries.map((e) => e.id)).toEqual([...builtin.entries.map((e) => e.id), "acme-sans"]);
    // Pins name the version they were planned at: a pack does not move it.
    expect(merged.catalogVersion).toBe(builtin.catalogVersion);
    expect((await cached(m, sha256(sans.bytes)))?.entry).toBe("acme-sans");

    // The pack's file is fetched like a built-in one: pinned SHA-256, same checks.
    const after = JSON.parse(await m.fetch(JSON.stringify({ entries: ["acme-sans"] }))) as { files: Array<{ entry?: string; state: { state: string } }> };
    expect(after.files.find((f) => f.entry === "acme-sans")?.state.state).toBe("cached");
    expect(existsSync(join(serverData, "fonts", sha256(sans.bytes)))).toBe(true);
    expect(server.hitsOf("acme-sans.ttf")).toBe(1);
  });

  test("a pin made before the pack arrived is fetched once the pack is merged", async () => {
    const m = FontManager.open(serverData);
    const sans = file("late-sans", "sans");
    engine = await Engine.create(serverData, join(tmp, "p"), "Demo", "30");
    const pin = catalogPin(sans);
    const ops = Object.entries((JSON.parse(pin.fonts) as { pins: Record<string, unknown> }).pins).map(([key, p]) => ({ op: "font.pin", key, pin: p }));
    await engine.commit(JSON.stringify({ txId: "pin1", author: "a", branch: "main", baseRev: await engine.head("main"), schemaVersion: 2, ops }));
    await new Promise((r) => setTimeout(r, 150));
    expect(server.hitsOf("late-sans.ttf")).toBe(0);

    setPacks(m, pack("@acme/fonts", sans));
    await waitFor(async () => ((await cached(m, sha256(sans.bytes)))?.state.state === "cached" ? true : undefined), 10_000, "the pack's pin fetched");
    expect(server.hitsOf("late-sans.ttf")).toBe(1);
  });

  test("taking a pack away keeps the fonts a project pinned usable", async () => {
    const m = FontManager.open(serverData);
    const sans = file("acme-sans", "sans");
    setPacks(m, pack("@acme/fonts", sans));
    engine = await Engine.create(serverData, join(tmp, "p"), "Demo", "30");
    const pin = catalogPin(sans);
    const ops = Object.entries((JSON.parse(pin.fonts) as { pins: Record<string, unknown> }).pins).map(([key, p]) => ({ op: "font.pin", key, pin: p }));
    await engine.commit(JSON.stringify({ txId: "pin1", author: "a", branch: "main", baseRev: await engine.head("main"), schemaVersion: 2, ops }));
    await waitFor(async () => ((await cached(m, sha256(sans.bytes)))?.state.state === "cached" ? true : undefined), 10_000, "cached");

    expect(setPacks(m)).toEqual({ rejected: [] });
    // No new pin can name it now…
    expect(catalogOf(m).entries.map((e) => e.id)).not.toContain("acme-sans");
    expect(JSON.parse(await m.fetch(JSON.stringify({ pinned: engine.projectId }))).files.length).toBeGreaterThan(0);
    await expect(m.fetch(JSON.stringify({ entries: ["acme-sans"] }))).rejects.toMatchObject({ code: "FONT_NOT_IN_CATALOG" });
    // …but the pinned bytes are still in the cache, listed as the project's own pin, and measure.
    expect(existsSync(join(serverData, "fonts", sha256(sans.bytes)))).toBe(true);
    const row = await cached(m, sha256(sans.bytes));
    expect(row).toMatchObject({ state: { state: "cached" } });
    expect(row?.entry).toBeUndefined();
    const status = JSON.parse(await engine.status()) as { fonts: { missing: unknown[] }; warnings: Array<{ code: string }> };
    expect(status.fonts.missing).toEqual([]);
    expect(status.warnings.map((w) => w.code)).not.toContain("FONT_MISSING");
    const request = JSON.stringify({
      words: ["Hello", "world"],
      style: { family: "Test acme-sans", weight: 400, italic: false, size: 48, lineHeight: 1.2, letterSpacing: 0, align: "center" },
      faces: [{ sha256: sha256(sans.bytes), face: 0, family: "Test acme-sans", weight: 400, italic: false }],
      maxWidth: 1000,
      maxLines: 2,
    });
    expect(JSON.parse(hooks.measure(serverData, request)).kind).toBe("layout");
  });

  test("merge_refuses_taken_ids: a pack that clashes is left out whole, the others stay", async () => {
    const m = FontManager.open(serverData);
    const builtinIds = catalogOf(m).entries.map((e) => e.id);
    const a = file("pack-a", 5);
    const impostor = file(builtinIds[0]!, 1);
    const twin = file("pack-a", 3);
    const fine = file("pack-b", 4);
    const answer = setPacks(m, pack("@acme/a", a), pack("@acme/impostor", impostor, file("pack-c", 2)), pack("@acme/twin", twin), pack("@acme/b", fine));
    expect(answer.rejected.map((r) => [r.id, r.code, r.details.rule])).toEqual([
      ["@acme/impostor", "FONT_CATALOG_INVALID", "catalogIdTaken"],
      ["@acme/twin", "FONT_CATALOG_INVALID", "catalogIdTaken"],
    ]);
    // Whole: the impostor's other entry is not merged either; the built-in entry is not replaced.
    expect(catalogOf(m).entries.map((e) => e.id)).toEqual([...builtinIds, "pack-a", "pack-b"]);
    expect((await cached(m, sha256(impostor.bytes)))).toBeUndefined();
    // Setting the same packs again changes nothing; an empty set takes them all away.
    expect(setPacks(m, pack("@acme/a", a), pack("@acme/b", fine)).rejected).toEqual([]);
    expect(setPacks(m).rejected).toEqual([]);
    expect(catalogOf(m).entries.map((e) => e.id)).toEqual(builtinIds);
  });

  test("a pack catalog is held to the built-in catalog's rules", async () => {
    const m = FontManager.open(serverData);
    const bad = JSON.parse(catalogJson([file("nc-font", 6)])) as { entries: Array<{ licence: { spdx: string } }> };
    bad.entries[0]!.licence.spdx = "CC-BY-NC-4.0";
    const mutable = JSON.parse(catalogJson([file("moving", 7)])) as { entries: Array<{ styles: Array<{ sources: Array<{ url: string }> }> }> };
    mutable.entries[0]!.styles[0]!.sources[0]!.url = "http://127.0.0.1:1/latest/moving.ttf";
    const answer = JSON.parse(
      m.setExtraCatalogs(JSON.stringify([{ id: "@acme/nc", catalogs: [JSON.stringify(bad)] }, { id: "@acme/moving", catalogs: [JSON.stringify(mutable)] }, { id: "@acme/junk", catalogs: ["{ nope"] }])),
    ) as Rejected;
    expect(answer.rejected.map((r) => [r.id, r.details.rule])).toEqual([
      ["@acme/nc", "licence"],
      ["@acme/moving", "immutableUrl"],
      ["@acme/junk", "schema"],
    ]);
    expect(catalogOf(m).entries.some((e) => ["nc-font", "moving"].includes(e.id))).toBe(false);
  });
});
