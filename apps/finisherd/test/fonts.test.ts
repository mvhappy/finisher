// The fonts and eval RPCs through a real finisherd (plan 4C Task 22): a test catalog through
// `FIN_FONT_CATALOG` (test-hooks build) over a loopback font server; no internet.
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { testHooks } from "@finisher/fin-node";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { catalogJson, FontServer, sha256, waitFor } from "../../../packages/fin-node/test/fontserver.ts";
import { commit, addAsset, connectProved, insert, NdjsonClient, ok, type RunningServer, startServer, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
let fonts: FontServer;
let server: RunningServer | undefined;
let client: NdjsonClient | undefined;

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-fonts-"));
  serverData = join(tmp, "server-data");
  fonts = await FontServer.start();
});

afterEach(async () => {
  await client?.close().catch(() => undefined);
  client = undefined;
  await server?.stop();
  server = undefined;
  await fonts.close();
  rmSync(tmp, { recursive: true, force: true });
});

interface FileStatus {
  sha256: string;
  entry?: string;
  state: { state: string; reason?: string };
}
interface Status {
  catalogVersion: string;
  autoFetch: boolean;
  damaged: { code: string } | null;
  files: FileStatus[];
}

async function start(catalog: string, env: Record<string, string> = {}): Promise<NdjsonClient> {
  const path = join(tmp, "catalog.json");
  writeFileSync(path, catalog);
  server = await startServer(serverData, 15_000, {
    ...process.env,
    FIN_FONT_CATALOG: path,
    FIN_FONTS_TEST_BACKOFF_MS: "0",
    FIN_FONTS_TEST_NO_PROXY: "1",
    ...env,
  });
  client = await connectProved(server);
  return client;
}

describe("fonts RPCs", () => {
  test("fonts.catalog, fonts.status, and fonts.fetch from a loopback server", async () => {
    const bytes = testHooks!.testFont("sans");
    const c = await start(catalogJson([{ id: "sans", bytes, url: fonts.serve("sans.ttf", bytes) }]));
    const catalog = ok<{ catalogVersion: string; entries: Array<{ id: string }> }>(await c.call("fonts.catalog", {}));
    expect(catalog.entries.map((e) => e.id)).toEqual(["sans"]);
    const before = ok<Status>(await c.call("fonts.status", {}));
    expect(before).toMatchObject({ catalogVersion: "2026.11.1", autoFetch: true, damaged: null });
    expect(before.files.map((f) => [f.entry, f.state.state])).toEqual([["sans", "missing"]]);

    const after = ok<Status>(await c.call("fonts.fetch", { entries: ["sans"] }));
    expect(after.files[0]?.state.state).toBe("cached");
    expect(existsSync(join(serverData, "fonts", sha256(bytes)))).toBe(true);
    expect(fonts.hitsOf("sans.ttf")).toBe(1);
    // Cached: a second fetch downloads nothing.
    ok(await c.call("fonts.fetch", { keys: ["Test sans|400|normal"] }));
    expect(fonts.hitsOf("sans.ttf")).toBe(1);
  });

  test("a failed fetch is FONT_FETCH_FAILED with its fix; an unknown entry FONT_NOT_IN_CATALOG", async () => {
    const bytes = testHooks!.testFont("sans");
    const url = fonts.serve("sans.ttf", bytes, { status: 500 });
    const c = await start(catalogJson([{ id: "sans", bytes, url }]));
    const failed = await c.call("fonts.fetch", { entries: ["sans"] });
    if (failed.ok) throw new Error("expected a failure");
    expect(failed.error.code).toBe("FONT_FETCH_FAILED");
    expect(failed.error.details).toMatchObject({ key: "Test sans|400|normal", entry: "sans", style: "regular", reason: "http500" });
    expect(failed.error.fix).toContain("HTTPS_PROXY");
    const status = ok<Status>(await c.call("fonts.status", {}));
    expect(status.files[0]?.state).toMatchObject({ state: "failed", reason: "http500" });
    const unknown = await c.call("fonts.fetch", { entries: ["nope"] });
    expect(unknown.ok ? undefined : unknown.error.code).toBe("FONT_NOT_IN_CATALOG");
    const project = await c.call("fonts.fetch", { pinned: "01nope" });
    expect(project.ok ? undefined : project.error.code).toBe("PROJECT_NOT_FOUND");
  });

  test("fonts.autoFetch in settings.json switches background fetching at run time", async () => {
    const bytes = testHooks!.testFont("sans");
    const c = await start(catalogJson([{ id: "sans", bytes, url: fonts.serve("sans.ttf", bytes) }]));
    const autoFetch = async () => ok<Status>(await c.call("fonts.status", {})).autoFetch;
    expect(await autoFetch()).toBe(true);
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ fonts: { autoFetch: false } }));
    await waitFor(async () => ((await autoFetch()) === false ? true : undefined), 10_000, "autoFetch off");
    // An explicit fetch still fetches.
    ok(await c.call("fonts.fetch", { entries: ["sans"] }));
    expect(fonts.hitsOf("sans.ttf")).toBe(1);
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ fonts: { autoFetch: true } }));
    await waitFor(async () => ((await autoFetch()) === true ? true : undefined), 10_000, "autoFetch on");
  });

  test("FIN_FONTS_OFFLINE=1 wins over the setting", async () => {
    const bytes = testHooks!.testFont("sans");
    const c = await start(catalogJson([{ id: "sans", bytes, url: fonts.serve("sans.ttf", bytes) }]), { FIN_FONTS_OFFLINE: "1" });
    expect(ok<Status>(await c.call("fonts.status", {})).autoFetch).toBe(false);
  });

  test("project.status carries the font summary; a damaged catalog warns and never blocks edits", async () => {
    const c = await start("{ not a catalog");
    const dir = join(tmp, "p");
    const { projectId } = ok<{ projectId: string }>(await c.call("project.create", { dir, name: "Demo", rate: "30" }));
    ok(await commit(c, projectId, tx("tx-1", 0, [addAsset()])));
    const status = ok<{ fonts: unknown; warnings: Array<{ code: string; fix?: string }> }>(await c.call("project.status", { projectId }));
    expect(status.fonts).toEqual({ missing: [], fetching: [], failed: [] });
    const damaged = status.warnings.find((w) => w.code === "FONTS_DAMAGED");
    expect(damaged?.fix).toBeDefined();
    expect(ok<Status>(await c.call("fonts.status", {})).damaged?.code).toBe("FONT_CATALOG_INVALID");
  });
});

describe("eval RPCs", () => {
  test("eval.frame, eval.audio and fonts.plan", async () => {
    const c = await start(catalogJson([]));
    const dir = join(tmp, "p");
    const { projectId } = ok<{ projectId: string }>(await c.call("project.create", { dir, name: "Demo", rate: "30" }));
    ok(await commit(c, projectId, tx("tx-1", 0, [addAsset(), insert(0, 5)])));
    const frame = ok<{ timeline: string; t: string; layers: Array<{ kind: string }> }>(
      await c.call("eval.frame", { projectId, at: "1" }),
    );
    expect(frame).toMatchObject({ timeline: "tl1", t: "1" });
    expect(frame.layers.map((l) => l.kind)).toEqual(["clip"]);
    ok(await c.call("eval.frame", { projectId, branch: "main", timeline: "tl1", at: "1001/30000" }));
    const audio = ok<{ timeline: string }>(await c.call("eval.audio", { projectId, from: "0", to: "5" }));
    expect(audio.timeline).toBe("tl1");
    const bad = await c.call("eval.audio", { projectId, from: "5", to: "1" });
    expect(bad.ok ? undefined : bad.error.code).toBe("INVALID_RANGE");
    const plan = ok(await c.call("fonts.plan", { projectId, branch: "main", ops: [insert(5, 1)] }));
    expect(plan).toEqual({ ops: [], notes: [] });
    const head = ok<{ rev: number }>(await c.call("snapshot.get", { projectId, branch: "main" }));
    expect(head.rev).toBe(1);
  });
});
