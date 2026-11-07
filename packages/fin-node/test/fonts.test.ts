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
  test("a new pin is fetched in the background; status events go missing → cached", async () => {
    const f = file("sans", hooks.testFont("sans"));
    useCatalog([f]);
    const { engine, events } = await open();
    const pin = catalogPin(f);
    engine.injectFontPinsForTest(pin.fonts);
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

  test("fetch progress reaches status events", async () => {
    const f = file("big", hooks.testFont("big:400000"), { chunkDelayMs: 20 });
    useCatalog([f]);
    const { engine, events } = await open();
    engine.injectFontPinsForTest(catalogPin(f).fonts);
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
    engine.injectFontPinsForTest(catalogPin(f).fonts);
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
    engine.injectFontPinsForTest(catalogPin(a).fonts);
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
    engine.injectFontPinsForTest(catalogPin(b).fonts);
    expect(server.hitsOf("rtl.ttf")).toBe(0);
    fonts.setAutoFetch(true);
    await waitFor(() => (server.hitsOf("rtl.ttf") > 0 ? true : undefined), 5_000, "fetch after switching on");
  });

  test("FIN_FONTS_OFFLINE=1 keeps background fetching off", async () => {
    process.env.FIN_FONTS_OFFLINE = "1";
    const f = file("sans", hooks.testFont("sans"));
    useCatalog([f]);
    const fonts = FontManager.open(serverData);
    fonts.setAutoFetch(true);
    expect(fonts.autoFetch).toBe(false);
    const { engine } = await open();
    engine.injectFontPinsForTest(catalogPin(f).fonts);
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
    engine.injectFontPinsForTest(JSON.stringify({ pins: { [key]: pin }, fallback: [key] }));
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
