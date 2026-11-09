// The font manager (plan 4C Task 22): fetch triggers, the off switch, status warnings, the
// shared measurer over the cache and font asset files. Fonts come from a loopback server.
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Engine, FontManager, type FinNodeError, testHooks } from "../index.js";
import { type CatalogFile, catalogJson, catalogPin, FontServer, sha256, waitFor } from "./fontserver.ts";

const hooks = testHooks!;
let tmp: string;
let serverData: string;
let server: FontServer;
let engine: Engine | undefined;
const ENV = ["FIN_FONT_CATALOG", "FIN_FONTS_OFFLINE", "FIN_FONTS_TEST_BACKOFF_MS", "FIN_FONTS_TEST_RETRY_MS", "FIN_FONTS_TEST_NO_PROXY"];

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-fonts-"));
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

interface Status {
  fonts: { missing: Array<{ key: string; reason: string }>; fetching: Array<{ key: string; bytes: number; total: number }>; failed: unknown[] };
  warnings: Array<{ code: string; message: string; details?: Record<string, unknown>; fix?: string }>;
}

/** A catalog of `files` (served by the test server) for the next manager. */
function useCatalog(files: CatalogFile[]): void {
  const path = join(tmp, `catalog-${files.map((f) => f.id).join("-")}.json`);
  writeFileSync(path, catalogJson(files));
  process.env.FIN_FONT_CATALOG = path;
}

function file(id: string, bytes: Uint8Array, opts: { chunkDelayMs?: number } = {}): CatalogFile {
  return { id, bytes, url: server.serve(`${id}.ttf`, bytes, opts) };
}

async function open(): Promise<{ engine: Engine; events: Status[] }> {
  engine = await Engine.create(serverData, join(tmp, "p"), "Demo", "30");
  const events: Status[] = [];
  engine.onEvent((json) => {
    const e = JSON.parse(json) as { type: string; status?: Status };
    if (e.type === "status" && e.status) events.push(e.status);
  });
  return { engine, events };
}

let txs = 0;
/** Commits `font.pin` of each pin in `fonts` (a `Fonts` JSON: `{pins, fallback}`) on main. */
async function pinFonts(e: Engine, fonts: string): Promise<void> {
  const { pins } = JSON.parse(fonts) as { pins: Record<string, unknown> };
  const ops = Object.entries(pins).map(([key, pin]) => ({ op: "font.pin", key, pin }));
  await e.commit(JSON.stringify({ txId: `pin${++txs}`, author: "a", branch: "main", baseRev: await e.head("main"), schemaVersion: 2, ops }));
}

const status = async (e: Engine): Promise<Status> => JSON.parse(await e.status()) as Status;
const codes = (s: Status) => s.warnings.map((w) => w.code);

async function rejection(p: Promise<unknown>): Promise<FinNodeError> {
  try {
    await p;
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a rejection");
}

function request(sha: string): string {
  return JSON.stringify({
    words: ["Hello", "world"],
    style: { family: "Test Sans", weight: 400, italic: false, size: 48, lineHeight: 1.2, letterSpacing: 0, align: "center" },
    faces: [{ sha256: sha, face: 0, family: "Test Sans", weight: 400, italic: false }],
    maxWidth: 1000,
    maxLines: 2,
  });
}

describe("font manager", () => {
  test("a commit with a new pin fetches it in the background; status events go missing → cached", async () => {
    const f = file("sans", hooks.testFont("sans"));
    useCatalog([f]);
    const { engine, events } = await open();
    const pin = catalogPin(f);
    await pinFonts(engine, pin.fonts);
    const fonts = FontManager.open(serverData);
    await waitFor(async () => (JSON.parse(await fonts.status()) as { files: Array<{ state: { state: string } }> }).files[0]?.state.state === "cached" || undefined, 10_000, "cached");
    expect(existsSync(join(serverData, "fonts", sha256(f.bytes)))).toBe(true);
    expect(server.hitsOf("sans.ttf")).toBe(1);
    const now = await status(engine);
    expect(now.fonts).toEqual({ missing: [], fetching: [], failed: [] });
    expect(codes(now)).not.toContain("FONT_MISSING");
    // The fetch was reported while it ran, then cleared.
    await waitFor(() => (events.at(-1)?.fonts.fetching.length === 0 ? true : undefined), 5_000, "a clear status event");
    expect(events.some((s) => s.fonts.fetching.some((x) => x.key === pin.key))).toBe(true);
    const files = JSON.parse(await fonts.status()) as { autoFetch: boolean; damaged: unknown; files: Array<{ keys: string[]; entry: string }> };
    expect(files.autoFetch).toBe(true);
    expect(files.damaged).toBeNull();
    expect(files.files[0]).toMatchObject({ entry: "sans", keys: [pin.key] });
  });

  test("an open fetches the missing pins its branches hold", async () => {
    const f = file("sans", hooks.testFont("sans"));
    useCatalog([f]);
    const fonts = FontManager.open(serverData);
    fonts.setAutoFetch(false);
    const { engine: first } = await open();
    const pin = catalogPin(f);
    await pinFonts(first, pin.fonts);
    expect((await status(first)).fonts.missing).toEqual([{ key: pin.key, sha256: sha256(f.bytes), reason: "offline" }]);
    await first.close();
    engine = undefined;
    expect(server.hitsOf("sans.ttf")).toBe(0);
    fonts.setAutoFetch(true);
    engine = await Engine.open(serverData, join(tmp, "p"));
    await waitFor(async () => (JSON.parse(await fonts.status()) as { files: Array<{ state: { state: string } }> }).files[0]?.state.state === "cached" || undefined, 10_000, "the open's fetch");
    expect(server.hitsOf("sans.ttf")).toBe(1);
    expect(existsSync(join(serverData, "fonts", sha256(f.bytes)))).toBe(true);
    expect((await status(engine)).fonts).toEqual({ missing: [], fetching: [], failed: [] });
  });

  test("fetch progress reaches status events", async () => {
    const f = file("big", hooks.testFont("big:400000"), { chunkDelayMs: 20 });
    useCatalog([f]);
    const { engine, events } = await open();
    await pinFonts(engine, catalogPin(f).fonts);
    const partial = await waitFor(
      () => events.flatMap((s) => s.fonts.fetching).find((x) => x.bytes > 0 && x.bytes < x.total),
      20_000,
      "a progress event",
    );
    expect(partial.total).toBe(400_000);
    await waitFor(async () => ((await status(engine)).fonts.fetching.length === 0 ? true : undefined), 30_000, "done");
  });

  test("a failed fetch warns FONT_FETCH_FAILED with its fix, and is retried", async () => {
    process.env.FIN_FONTS_TEST_RETRY_MS = "400";
    const bytes = hooks.testFont("sans");
    const f = file("sans", bytes);
    server.routes.set(`/${"0123456789abcdef0123456789abcdef01234567"}/sans.ttf`, { status: 500 });
    useCatalog([f]);
    const { engine } = await open();
    await pinFonts(engine, catalogPin(f).fonts);
    const failed = await waitFor(async () => (await status(engine)).warnings.find((w) => w.code === "FONT_FETCH_FAILED"), 10_000, "a failure");
    expect(failed.details).toMatchObject({ key: "Test sans|400|normal", entry: "sans", style: "regular", sha256: sha256(bytes) });
    expect(typeof failed.details?.retryAt).toBe("number");
    expect(failed.fix).toContain("HTTPS_PROXY");
    expect(failed.fix).toContain("fin fonts fetch sans");
    // An edit is never blocked by it.
    const marker = { op: "marker.add", id: "$m", timeline: "tl1", at: "1", label: "m" };
    await engine.commit(JSON.stringify({ txId: "t1", author: "a", branch: "main", baseRev: 0, schemaVersion: 2, ops: [marker] }));
    server.serve("sans.ttf", bytes);
    await waitFor(async () => ((await status(engine)).fonts.failed.length === 0 && (await status(engine)).fonts.missing.length === 0 ? true : undefined), 10_000, "the retry");
    expect(server.hitsOf("sans.ttf")).toBeGreaterThanOrEqual(2);
  });

  test("autoFetch off never fetches; an explicit fetch does; switching it on at run time fetches", async () => {
    const a = file("sans", hooks.testFont("sans"));
    const b = file("rtl", hooks.testFont("rtl"));
    useCatalog([a, b]);
    const fonts = FontManager.open(serverData);
    fonts.setAutoFetch(false);
    expect(fonts.autoFetch).toBe(false);
    const { engine } = await open();
    await pinFonts(engine, catalogPin(a).fonts);
    await new Promise((r) => setTimeout(r, 300));
    expect(server.hitsOf("sans.ttf")).toBe(0);
    const s = await status(engine);
    expect(s.fonts.missing).toEqual([{ key: "Test sans|400|normal", sha256: sha256(a.bytes), reason: "offline" }]);
    const w = s.warnings.find((x) => x.code === "FONT_MISSING");
    expect(w?.fix).toContain("fin fonts fetch --pinned");
    // A measurement missing bytes asks, but nothing is fetched while off.
    expect(JSON.parse(hooks.measure(serverData, request(sha256(b.bytes))))).toEqual({ kind: "unavailable", missing: [sha256(b.bytes)] });
    await new Promise((r) => setTimeout(r, 200));
    expect(server.hitsOf("rtl.ttf")).toBe(0);
    // `fin fonts fetch --pinned` fetches anyway.
    const after = JSON.parse(await fonts.fetch(JSON.stringify({ pinned: engine.projectId }))) as { files: Array<{ entry: string; state: { state: string } }> };
    expect(after.files.find((x) => x.entry === "sans")?.state.state).toBe("cached");
    expect((await status(engine)).fonts.missing).toEqual([]);
    // On again: what is pinned and missing is fetched at once.
    await pinFonts(engine, catalogPin(b).fonts);
    expect(server.hitsOf("rtl.ttf")).toBe(0);
    fonts.setAutoFetch(true);
    await waitFor(() => (server.hitsOf("rtl.ttf") > 0 ? true : undefined), 5_000, "fetch after switching on");
  });

  test("a summary computed before a newer one is never published after it", async () => {
    const f = file("sans", hooks.testFont("sans"));
    useCatalog([f]);
    FontManager.open(serverData).setAutoFetch(false);
    const { engine, events } = await open();
    // A refresh computes the summary without pins and pauses; meanwhile a pin is committed
    // and its refresh (on the jobs thread) must wait, then publish last.
    const { pins } = JSON.parse(catalogPin(f).fonts) as { pins: Record<string, unknown> };
    const ops = Object.entries(pins).map(([key, pin]) => ({ op: "font.pin", key, pin }));
    engine.fontPublishRaceForTest(JSON.stringify({ txId: "race", author: "a", branch: "main", baseRev: await engine.head("main"), schemaVersion: 2, ops }));
    await waitFor(() => (events.some((s) => s.fonts.missing.length === 1) ? true : undefined), 5_000, "the newer summary");
    await new Promise((r) => setTimeout(r, 300));
    // Without the publish lock the paused (older) summary lands after the newer one.
    const missing = events.map((s) => s.fonts.missing.length);
    expect(missing).toContain(0);
    expect(missing.slice(missing.indexOf(1))).not.toContain(0);
    expect((await status(engine)).fonts.missing).toHaveLength(1);
  });

  test("FIN_FONTS_OFFLINE=1 keeps background fetching off", async () => {
    process.env.FIN_FONTS_OFFLINE = "1";
    const f = file("sans", hooks.testFont("sans"));
    useCatalog([f]);
    const fonts = FontManager.open(serverData);
    fonts.setAutoFetch(true);
    expect(fonts.autoFetch).toBe(false);
    const { engine } = await open();
    await pinFonts(engine, catalogPin(f).fonts);
    await new Promise((r) => setTimeout(r, 300));
    expect(server.hitsOf("sans.ttf")).toBe(0);
    expect((await status(engine)).fonts.missing[0]?.reason).toBe("offline");
  });

  test("a measurement missing bytes queues their fetch; the next one measures", async () => {
    const f = file("sans", hooks.testFont("sans"));
    useCatalog([f]);
    FontManager.open(serverData);
    const sha = sha256(f.bytes);
    expect(JSON.parse(hooks.measure(serverData, request(sha))).kind).toBe("unavailable");
    await waitFor(() => (JSON.parse(hooks.measure(serverData, request(sha))).kind === "layout" ? true : undefined), 10_000, "measured");
    expect(server.hitsOf("sans.ttf")).toBe(1);
  });

  test("captions measure with pinned bytes: eval lays them out, an overflowing edit is CAPTION_OVERFLOW, unpinned text warns FONT_UNPINNED", async () => {
    const f = file("sans", hooks.testFont("sans"));
    useCatalog([f]);
    const { engine } = await open();
    await pinFonts(engine, catalogPin(f).fonts);
    await waitFor(async () => {
      const s = await status(engine);
      return s.fonts.missing.length === 0 && s.fonts.fetching.length === 0 ? true : undefined;
    }, 10_000, "the pinned file fetched");
    const caption = (id: string, at: string, text: string) => ({
      op: "item.insert", id, track: "t3", at, duration: "2", mode: "overwrite", kind: "caption", style: "st1",
      words: text.split(" ").map((t) => ({ text: t, start: "0", end: "0" })),
    });
    const commitOps = async (txId: string, ops: unknown[]) =>
      engine.commit(JSON.stringify({ txId, author: "a", branch: "main", baseRev: await engine.head("main"), schemaVersion: 2, ops }));
    await commitOps("c1", [
      { op: "track.add", id: "$k", timeline: "tl1", kind: "caption" },
      { op: "style.add", id: "$s", name: "Test", set: { "text.family": "Test sans", "text.weight": 400 } },
      caption("$c", "0", "hi there"),
    ]);
    const plan = JSON.parse(await engine.evalFrame("main", null, "1")) as { layers: Array<{ caption?: { layout: { faces: string[] } | null } }>; textEngine?: string };
    expect(plan.layers[0]?.caption?.layout?.faces).toEqual([sha256(f.bytes)]);
    expect(plan.textEngine).toContain("fin-text/");
    // A word far wider than the safe zone: refused with the measured numbers.
    const e = await rejection(commitOps("c2", [caption("$d", "3", "x".repeat(200))]));
    expect(e.code).toBe("CAPTION_OVERFLOW");
    expect(e.details?.item).toBe("cap2");
    // Hebrew text no pin covers: accepted, unmeasured, and the status says so.
    await commitOps("c3", [caption("$h", "6", "שלום")]);
    const w = (await status(engine)).warnings.find((x) => x.code === "FONT_UNPINNED");
    expect(w?.details).toEqual({ key: "script:Hebr", items: ["cap2"] });
    expect(w?.fix).toContain("fin fonts pin");
  });

  // Review of 2026-11-09, #23: a caption that goes with any removal leaves no stale warning.
  test("FONT_UNPINNED forgets a caption removed by a lift, a ripple, its track's or its timeline's removal", async () => {
    useCatalog([file("sans", hooks.testFont("sans"))]);
    const { engine } = await open();
    const commitOps = async (txId: string, ops: unknown[]) =>
      JSON.parse(await engine.commit(JSON.stringify({ txId, author: "a", branch: "main", baseRev: await engine.head("main"), schemaVersion: 2, ops }))) as { bindings: Record<string, string> };
    const hebrew = (track: string) => ({
      op: "item.insert", id: "$h", track, at: "0", duration: "2", mode: "overwrite", kind: "caption", style: "st1",
      words: [{ text: "שלום", start: "0", end: "0" }],
    });
    const unpinned = async () => (await status(engine)).warnings.find((x) => x.code === "FONT_UNPINNED")?.details?.items;
    const { bindings } = await commitOps("setup", [
      { op: "track.add", id: "$k", timeline: "tl1", kind: "caption" },
      { op: "style.add", id: "$s", name: "Test", set: { "text.family": "Test sans", "text.weight": 400 } },
      { op: "timeline.add", id: "$tl", name: "Second", rate: "30", resolution: [1920, 1080] },
    ]);
    const track = bindings.$k!;
    let n = 0;
    const removals: Array<[string, (item: string) => Promise<unknown[]>]> = [
      ["lift", async (item) => [{ op: "item.delete", item, mode: "lift" }]],
      ["ripple", async (item) => [{ op: "item.delete", item, mode: "ripple" }]],
    ];
    for (const [how, ops] of removals) {
      const item = (await commitOps(`h${++n}`, [hebrew(track)])).bindings.$h!;
      expect(await unpinned(), how).toEqual([item]);
      await commitOps(`r${n}`, await ops(item));
      expect(await unpinned(), how).toBeUndefined();
    }
    // The track, with its items.
    const t = (await commitOps("k2", [{ op: "track.add", id: "$k", timeline: "tl1", kind: "caption" }])).bindings.$k!;
    const onTrack = (await commitOps(`h${++n}`, [hebrew(t)])).bindings.$h!;
    expect(await unpinned()).toEqual([onTrack]);
    await commitOps("rk", [{ op: "track.remove", track: t, withItems: true }]);
    expect(await unpinned()).toBeUndefined();
    // The timeline, with its tracks and items.
    const k3 = (await commitOps("k3", [{ op: "track.add", id: "$k", timeline: bindings.$tl!, kind: "caption" }])).bindings.$k!;
    const onTimeline = (await commitOps(`h${++n}`, [hebrew(k3)])).bindings.$h!;
    expect(await unpinned()).toEqual([onTimeline]);
    await commitOps("rtl", [{ op: "timeline.remove", timeline: bindings.$tl! }]);
    expect(await unpinned()).toBeUndefined();
  });

  test("an asset font pin measures from the asset file; a changed file is a hash mismatch", async () => {
    useCatalog([file("other", hooks.testFont("rtl"))]);
    const { engine } = await open();
    const bytes = hooks.testFont("sans");
    const sha = sha256(bytes);
    const path = join(tmp, "TestSans.ttf");
    writeFileSync(path, bytes);
    const asset = {
      op: "asset.add",
      id: "$f",
      kind: "font",
      path,
      fingerprint: "fp-font",
      font: { sha256: sha, size: bytes.length, format: "ttf", faces: [{ index: 0, family: "Test Sans", weight: 400, italic: false, scripts: ["Latn", "Cyrl"] }] },
    };
    const c = JSON.parse(await engine.commit(JSON.stringify({ txId: "t1", author: "a", branch: "main", baseRev: 0, schemaVersion: 2, ops: [asset] }))) as { bindings: Record<string, string> };
    const key = "Test Sans|400|normal";
    const pin = { family: "Test Sans", weight: 400, italic: false, face: 0, sha256: sha, size: bytes.length, format: "ttf", scripts: ["Latn", "Cyrl"], source: { type: "asset", asset: c.bindings.$f } };
    await pinFonts(engine, JSON.stringify({ pins: { [key]: pin }, fallback: [key] }));
    expect((await status(engine)).fonts.missing).toEqual([]);
    const measured = JSON.parse(hooks.measure(serverData, request(sha)));
    expect(measured.kind).toBe("layout");
    expect(measured.faces).toEqual([sha]);

    const changed = Uint8Array.from(bytes);
    changed[changed.length - 1]! ^= 1;
    writeFileSync(path, changed);
    const s = await status(engine);
    expect(s.fonts.missing).toEqual([{ key, sha256: sha, reason: "hashMismatch" }]);
    expect(s.warnings.find((w) => w.code === "FONT_MISSING")?.fix).toContain("restore the font asset");
    // A face list measured before keeps the verified bytes it loaded (exactly the pinned
    // ones); a new face list reads the file again and finds it changed.
    const other = JSON.parse(request(sha)) as { faces: unknown[] };
    other.faces.push({ sha256: "0".repeat(64), face: 0, family: "X", weight: 400, italic: false });
    expect(JSON.parse(hooks.measure(serverData, JSON.stringify(other))).missing).toContain(sha);
  });

  test("an unusable catalog is FONTS_DAMAGED: a warning, and edits still commit", async () => {
    const path = join(tmp, "bad-catalog.json");
    writeFileSync(path, "{ not json");
    process.env.FIN_FONT_CATALOG = path;
    const { engine } = await open();
    const s = await status(engine);
    expect(codes(s)).toContain("FONTS_DAMAGED");
    const fonts = FontManager.open(serverData);
    const st = JSON.parse(await fonts.status()) as { damaged: { code: string } | null };
    expect(st.damaged?.code).toBe("FONT_CATALOG_INVALID");
    expect((await rejection(fonts.fetch(JSON.stringify({ entries: ["noto-sans"] })))).code).toBe("FONTS_DAMAGED");
  });

  test("fetch requests: unknown targets are FONT_NOT_IN_CATALOG; a failure is FONT_FETCH_FAILED", async () => {
    const f = file("sans", hooks.testFont("sans"));
    server.routes.set(`/0123456789abcdef0123456789abcdef01234567/sans.ttf`, { status: 500 });
    useCatalog([f]);
    const fonts = FontManager.open(serverData);
    fonts.setAutoFetch(false);
    expect((await rejection(fonts.fetch(JSON.stringify({ entries: ["nope"] })))).code).toBe("FONT_NOT_IN_CATALOG");
    expect((await rejection(fonts.fetch(JSON.stringify({ keys: ["Nope|400|normal"] })))).code).toBe("FONT_NOT_IN_CATALOG");
    expect((await rejection(fonts.fetch(JSON.stringify({ pinned: "01nope" })))).code).toBe("PROJECT_NOT_FOUND");
    expect((await rejection(fonts.fetch(JSON.stringify({ bogus: 1 })))).code).toBe("INVALID_OP");
    const e = await rejection(fonts.fetch(JSON.stringify({ keys: ["Test sans|400|normal"] })));
    expect(e.code).toBe("FONT_FETCH_FAILED");
    expect(e.details).toMatchObject({ entry: "sans", style: "regular", reason: "http500" });
    expect(e.fix).toContain("HTTPS_PROXY");
    expect(JSON.parse(fonts.catalog()).catalogVersion).toBe("2026.11.1");
  });
});
