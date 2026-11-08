// Content consumers (8A plan Task 12a): the enabled content packs' font catalogs join the
// server's catalog (and leave it with the pack), and `plugin.content.get` serves a pack's
// caption styles. Over fin-node's `Registry` and `FontManager` and the real
// `resources/plugins/bundled`; fonts come from a loopback server.
import { chmodSync, cpSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Effect, Exit, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { catalogJson, FontServer, sha256 } from "../../../packages/fin-node/test/fontserver.ts";
import { type FontsShape, makeFonts } from "../src/fonts.ts";
import type { FontSink } from "../src/plugins/content.ts";
import { advisory, AGENT, as, CONSOLE, code, codeOf, failure, hooks, plugins, run, type Setup } from "./support/plugins-fixture.ts";

const f = plugins();
const REAL = fileURLToPath(new URL("../../../resources/plugins/bundled", import.meta.url));
/** A style file of the bundled pack: the built-in preset of that name without its `id` (`fin-node`'s `bundled.rs` pins it). */
const presetFile = (name: string) => JSON.parse(readFileSync(join(REAL, "captions-basic", "styles", `${name}.json`), "utf8")) as Record<string, unknown>;

let scope: Scope.Closeable;
let fonts: FontsShape;
let server: FontServer;

beforeEach(async () => {
  process.env.FIN_FONTS_TEST_BACKOFF_MS = "0";
  process.env.FIN_FONTS_TEST_NO_PROXY = "1";
  server = await FontServer.start();
  scope = Effect.runSync(Scope.make());
  fonts = await Effect.runPromise(Scope.provide(scope)(makeFonts(f.serverData, { settingsPollMs: 60_000 })));
});

afterEach(async () => {
  await Effect.runPromise(Scope.close(scope, Exit.void));
  await server.close();
  delete process.env.FIN_FONTS_TEST_BACKOFF_MS;
  delete process.env.FIN_FONTS_TEST_NO_PROXY;
});

/** The font manager as the plugins service takes it. */
const sink = (): FontSink => (packs) => fonts.setExtraCatalogs(packs);
const options = () => ({ fonts: sink() });

/** A font catalog file of one entry `id` for the `sans` test font, served by the loopback server. */
const catalogOf = (id: string, seed = "sans") => {
  const bytes = hooks.testFont(seed === "sans" ? "sans" : `big:4096:${seed}`);
  return { bytes, text: catalogJson([{ id, bytes, url: server.serve(`${id}.ttf`, bytes) }]) };
};
const fontPack = (id: string, version: string, ...catalogs: string[]) => ({
  id,
  version,
  files: Object.fromEntries(catalogs.map((c, i) => [`fonts/catalog${i}.json`, c])),
  contributes: { fontCatalogs: catalogs.map((_, i) => ({ file: `fonts/catalog${i}.json` })) },
});

const catalogIds = async () => (await run(fonts.catalog())).entries.map((e) => e.id);
const BUILTIN = async () => (await catalogIds()).filter((id) => !id.startsWith("acme") && !id.startsWith("other"));

async function install(s: Setup, id: string) {
  const plan = (await run(s.life.plan({ id }, CONSOLE))).plan;
  return run(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
}
const content = (s: Setup, id: string, name: string, by = CONSOLE) => s.h["plugin.content.get"]({ id, kind: "captionStyle", name }, as(by));

describe("font catalogs", () => {
  test("pack_fonts_join_the_catalog", async () => {
    const builtin = await catalogIds();
    const a = catalogOf("acme-sans");
    const s = await f.setup({ packages: [fontPack("@acme/fonts", "1.0.0", a.text)] }, options());
    expect(await catalogIds()).toEqual(builtin);

    await install(s, "@acme/fonts");
    expect(await catalogIds()).toEqual([...builtin, "acme-sans"]);
    // Fetched like any catalog file: the pinned SHA-256 is checked.
    const status = await run(fonts.fetch({ entries: ["acme-sans"] }));
    expect(status.files.find((x) => x.entry === "acme-sans")?.state.state).toBe("cached");
    expect(existsSync(join(f.serverData, "fonts", sha256(a.bytes)))).toBe(true);

    // Disabling the pack takes its entries out for new pins; the cached bytes stay.
    await run(s.life.disable("@acme/fonts"));
    expect(await catalogIds()).toEqual(builtin);
    expect(existsSync(join(f.serverData, "fonts", sha256(a.bytes)))).toBe(true);
    await run(s.life.enable("@acme/fonts"));
    expect(await catalogIds()).toEqual([...builtin, "acme-sans"]);
    await run(s.life.remove("@acme/fonts", false));
    expect(await catalogIds()).toEqual(builtin);
    expect(existsSync(join(f.serverData, "fonts", sha256(a.bytes)))).toBe(true);
    expect(s.plugins.problems()).toEqual([]);
  });

  test("a revoked pack's fonts leave the catalog, and a restart finds the installed packs", async () => {
    const builtin = await catalogIds();
    const a = catalogOf("acme-sans");
    const s = await f.setup({ packages: [fontPack("@acme/fonts", "1.0.0", a.text)] }, options());
    await install(s, "@acme/fonts");
    expect(await catalogIds()).toEqual([...builtin, "acme-sans"]);
    f.publish(s.reg, { advisories: [advisory("@acme/fonts", "1.0.0")] });
    await run(s.h["registry.refresh"]({ id: "team" }, as(CONSOLE)));
    expect(s.tables.installedOne("@acme/fonts")?.state).toBe("revoked");
    expect(await catalogIds()).toEqual(builtin);

    // A new service over the same folders (a restart) merges what is enabled at its start.
    await run(Effect.sync(() => s.tables.setState("@acme/fonts", true, "active", f.now)));
    await run(fonts.setExtraCatalogs([]));
    expect(await catalogIds()).toEqual(builtin);
    const again = f.service(options());
    await run(again.plugins.content.sync());
    expect(await catalogIds()).toEqual([...builtin, "acme-sans"]);
  });

  test("merge_refuses_taken_ids: the plan refuses a built-in id, a clash that slips through leaves the pack out", async () => {
    const builtin = await catalogIds();
    const taken = catalogOf(builtin[0]!, "1");
    const first = catalogOf("acme-one", "2");
    // The same font file under another id: no id clashes, but one file may not be listed twice.
    const twin = { ...first, text: catalogJson([{ id: "acme-twin", bytes: first.bytes, url: server.serve("acme-one.ttf", first.bytes) }]) };
    const s = await f.setup({ packages: [fontPack("@acme/shadow", "1.0.0", taken.text), fontPack("@acme/one", "1.0.0", first.text), fontPack("@acme/twin", "1.0.0", twin.text)] }, options());

    const refused = await failure(s.life.plan({ id: "@acme/shadow" }, CONSOLE));
    expect(refused?.code).toBe("PLUGIN_MANIFEST_INVALID");
    expect(refused?.details).toMatchObject({ reason: "catalogIdTaken" });

    await install(s, "@acme/one");
    await install(s, "@acme/twin");
    expect(await catalogIds()).toEqual([...builtin, "acme-one"]);
    expect(s.plugins.problems()).toEqual([{ id: "@acme/twin", version: "1.0.0", reason: "fontCatalogRejected", code: "FONT_CATALOG_INVALID" }]);
    // The owner sees it in the list.
    const list = await run(s.h["plugin.list"]());
    expect(list.problems).toEqual([{ id: "@acme/twin", version: "1.0.0", reason: "fontCatalogRejected", code: "FONT_CATALOG_INVALID" }]);
    // Removing the pack that holds the file lets the twin in at the next change.
    await run(s.life.remove("@acme/one", false));
    expect(await catalogIds()).toEqual([...builtin, "acme-twin"]);
    expect(s.plugins.problems()).toEqual([]);
  });

  test("a pack whose files no longer validate is left out and listed", async () => {
    const builtin = await catalogIds();
    const a = catalogOf("acme-sans");
    const s = await f.setup({ packages: [fontPack("@acme/fonts", "1.0.0", a.text)] }, options());
    await install(s, "@acme/fonts");
    const dir = join(f.serverData, "plugins", s.tables.installedFull("@acme/fonts")!.dir, "fonts", "catalog0.json");
    chmodSync(dir, 0o644);
    writeFileSync(dir, "{ not a catalog");
    const again = f.service(options());
    await run(fonts.setExtraCatalogs([]));
    await run(again.plugins.content.sync());
    expect(await catalogIds()).toEqual(builtin);
    expect(again.plugins.problems()).toMatchObject([{ id: "@acme/fonts", reason: "contentInvalid", code: "PLUGIN_MANIFEST_INVALID" }]);
  });
});

describe("caption styles", () => {
  test("content_get_returns_style", async () => {
    const s = await f.setup({ packages: [{ id: "@acme/captions", version: "1.2.0", styles: ["punchy", "calm"] }] }, options());
    await install(s, "@acme/captions");
    const got = await run(content(s, "@acme/captions", "punchy"));
    expect(got).toMatchObject({ id: "@acme/captions", version: "1.2.0", kind: "captionStyle", name: "punchy" });
    // The test registry's styles are the built-in `clean` preset named after the style and version.
    expect(got.style).toEqual({ ...presetFile("clean"), name: "punchy 1.2.0" });
    expect("id" in got.style).toBe(false);
    expect((await run(content(s, "@acme/captions", "calm"))).style.name).toBe("calm 1.2.0");
    // Not a name of the pack: what the pack has is in the error.
    const missing = await failure(content(s, "@acme/captions", "loud"));
    expect(missing).toMatchObject({ code: "NOT_FOUND", details: { available: ["punchy", "calm"] } });
    // An agent reads too: it is data the pack ships (the scope table gives it `plugin.read`).
    expect((await run(content(s, "@acme/captions", "calm", AGENT))).name).toBe("calm");
  });

  test("content_get_respects_enabled_state", async () => {
    const s = await f.setup({ packages: [{ id: "@acme/captions", version: "1.0.0", styles: ["punchy"] }, code("@acme/tool", "1.0.0")] }, options());
    expect(await codeOf(content(s, "@acme/captions", "punchy"))).toBe("PLUGIN_NOT_INSTALLED");
    await install(s, "@acme/captions");
    expect((await run(content(s, "@acme/captions", "punchy"))).name).toBe("punchy");

    await run(s.life.disable("@acme/captions"));
    expect(await codeOf(content(s, "@acme/captions", "punchy"))).toBe("PLUGIN_DISABLED");
    await run(s.life.enable("@acme/captions"));
    expect((await run(content(s, "@acme/captions", "punchy"))).name).toBe("punchy");

    // A revoked version serves nothing.
    f.publish(s.reg, { advisories: [advisory("@acme/captions", "1.0.0")] });
    await run(s.h["registry.refresh"]({ id: "team" }, as(CONSOLE)));
    expect(await codeOf(content(s, "@acme/captions", "punchy"))).toBe("PLUGIN_REVOKED");

    // A code plugin has no caption styles.
    const toolPlan = (await run(s.life.plan({ id: "@acme/tool" }, CONSOLE))).plan;
    await run(s.life.install({ planId: toolPlan.planId, digest: toolPlan.digest, accept: { thirdPartyCode: true } }, CONSOLE));
    expect(await codeOf(content(s, "@acme/tool", "punchy"))).toBe("NOT_FOUND");
  });

  test("a damaged store file serves nothing", async () => {
    const s = await f.setup({ packages: [{ id: "@acme/captions", version: "1.0.0", styles: ["punchy"] }] }, options());
    await install(s, "@acme/captions");
    const file = join(f.serverData, "plugins", s.tables.installedFull("@acme/captions")!.dir, "styles", "punchy.json");
    chmodSync(file, 0o644);
    const style = JSON.parse(readFileSync(file, "utf8")) as Record<string, unknown>;
    style.colour = "#fff"; // a key the model would drop
    writeFileSync(file, JSON.stringify(style));
    const e = await failure(content(s, "@acme/captions", "punchy"));
    expect(e).toMatchObject({ code: "PLUGIN_MANIFEST_INVALID", details: { reason: "contentInvalid" } });
  });

  test("the bundled pack serves the built-in presets, and serves nothing once revoked", async () => {
    const root = join(f.tmp, "bundled");
    cpSync(REAL, root, { recursive: true });
    const s = f.service({ ...options(), bundledRoot: root });
    await run(s.plugins.syncBundled());
    for (const name of ["bold", "clean", "punch"]) {
      const got = await run(content(s as unknown as Setup, "@finisher/captions-basic", name));
      expect(got.style).toEqual(presetFile(name));
      expect(got.title).toBeTypeOf("string");
    }
    // The files stop matching the app: revoked, nothing served.
    const style = join(root, "captions-basic", "styles", "bold.json");
    writeFileSync(style, readFileSync(style, "utf8").replace("72.0", "73.0"));
    await run(s.plugins.syncBundled());
    expect(await codeOf(content(s as unknown as Setup, "@finisher/captions-basic", "bold"))).toBe("PLUGIN_BUNDLED_MISMATCH");
  });
});
