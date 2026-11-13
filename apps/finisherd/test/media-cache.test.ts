// The media cache in finisherd (6B-1 plan Task 13): `media.cacheMaxGiB`, fin-node's `MediaCache`
// over napi (commits, the adoption after a crash, `CACHE_IO`), the service (project pins, the
// timer's work), and the RPCs on a running server: `media.cache.status` (`server.read`),
// `media.cache.gc` (`settings.admin`: the owner's console, never `fin`), the cap from
// `settings.json` and an open project's entries pinned again after a restart.
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { blake3Hex, MediaCache } from "@finisher/fin-node";
import { Effect, Exit, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { readMediaSettings } from "../src/config.ts";
import { UNAVAILABLE_CODES } from "../src/console/exit-codes.ts";
import { runConsole } from "../src/console/main.ts";
import { capFromSettings, GIB, makeMediaCache, type MediaCacheService } from "../src/media/cache.ts";
import { connectProved, type Exit as RpcExit, ok, type RunningServer, startServer } from "./support/ndjson-client.ts";
import { person } from "./support/person.ts";

let tmp: string;
let serverData: string;
let server: RunningServer | undefined;

beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "fin-media-cache-")));
  serverData = join(tmp, "sd");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

/** Canonical JSON (fin-media's `canonical_json`): keys sorted at every level, no whitespace. */
function canonical(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  if (v !== null && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}

/**
 * `<jobOut>/<key>/` with `files` and a `meta.json` for a `proxy-1080` of `fingerprint`; the key.
 * The key input's `renderVersion` is whatever `meta.json` says (it is hashed, not compared with
 * the build's), so the test needs no copy of `RENDER_VERSION`.
 */
function writeEntry(jobOut: string, fingerprint: string, files: Record<string, string>): string {
  const input = { fingerprint, recipe: "proxy-1080", recipeVersion: 1, params: { tier: "t1080" }, renderVersion: 1, decoderClass: "software" };
  const key = blake3Hex(Buffer.from(canonical(input)));
  const dir = join(jobOut, key);
  mkdirSync(dir, { recursive: true });
  const listed = Object.entries(files).map(([name, text]) => {
    writeFileSync(join(dir, name), text);
    return { name, bytes: Buffer.byteLength(text), blake3: blake3Hex(Buffer.from(text)) };
  });
  writeFileSync(join(dir, "meta.json"), JSON.stringify({ key, ...input, files: listed, createdAt: 1 }));
  return key;
}

let jobs = 0;
/** A job's `out/` under `tmp/jobs/` holding one entry; `[out, key]`. */
function job(fingerprint: string, files: Record<string, string> = { "proxy.mp4": "mp4", "table.bin": "table" }): [string, string] {
  const out = join(tmp, "jobs", `j${++jobs}`, "out");
  return [out, writeEntry(out, fingerprint, files)];
}

const failure = async <A>(e: Effect.Effect<A, FinError>): Promise<FinError> => {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) throw new Error(`expected a failure, got ${JSON.stringify(exit.value)}`);
  const reason = exit.cause.reasons.find((r) => r._tag === "Fail");
  if (reason === undefined || reason._tag !== "Fail") throw new Error("expected a FinError");
  return reason.error;
};

const settingsFile = (value: unknown) => writeFileSync(join(serverData, "settings.json"), JSON.stringify(value));

describe("media.cacheMaxGiB", () => {
  test("absent, set, unusable", () => {
    expect(readMediaSettings(serverData)).toEqual({});
    expect(capFromSettings(serverData)).toBeNull();
    settingsFile({ fonts: { autoFetch: true } });
    expect(capFromSettings(serverData)).toBeNull();
    settingsFile({ media: { cacheMaxGiB: 2.5 } });
    expect(readMediaSettings(serverData)).toEqual({ cacheMaxGiB: 2.5 });
    expect(capFromSettings(serverData)).toBe(2.5 * GIB);
    for (const bad of [0, -1, "10", null, Number.POSITIVE_INFINITY]) {
      settingsFile({ media: { cacheMaxGiB: bad } });
      expect(readMediaSettings(serverData).error, String(bad)).toMatch(/cacheMaxGiB/);
      expect(capFromSettings(serverData)).toBeUndefined();
    }
    writeFileSync(join(serverData, "settings.json"), "{ not json");
    expect(capFromSettings(serverData)).toBeUndefined();
    // Finite GiB whose bytes overflow (Infinity, or past 2^53): unusable, never a start failure.
    for (const huge of [1e300, 2 ** 24]) {
      settingsFile({ media: { cacheMaxGiB: huge } });
      expect(capFromSettings(serverData), String(huge)).toBeUndefined();
    }
  });
});

describe("MediaCache over napi", () => {
  test("a commit is checked, moved and indexed; the key JS computes is the one Rust recomputes", async () => {
    const cache = await MediaCache.open(serverData);
    expect(cache.openReport()).toEqual({ adopted: 0, deleted: 0, dropped: 0, indexReplaced: false, warnings: [] });
    const [out, key] = job("fp1");
    const meta = await cache.commit(out, key, ["p1"]);
    expect(meta).toMatchObject({ key, fingerprint: "fp1", recipe: "proxy-1080", decoderClass: "software" });
    expect(readFileSync(join(cache.entryDir(key)!, "proxy.mp4"), "utf8")).toBe("mp4");
    // Canonical (Windows: a verbatim path), so compared by its tail.
    expect(cache.entryDir(key)?.endsWith(join("cache", "media", key.slice(0, 2), key))).toBe(true);
    expect(cache.entryDir("../x")).toBeNull();
    expect(await cache.lookup(key)).toEqual(meta);
    expect(await cache.lookup("0".repeat(64))).toBeNull();
    expect(await cache.projectsOf(key)).toEqual(["p1"]);
    expect((await cache.entriesFor({ fingerprint: "fp1" })).map((m) => m.key)).toEqual([key]);
    const s = await cache.status();
    expect(s).toMatchObject({ entries: 1, pinned: 0, byRecipe: { "proxy-1080": { entries: 1 } } });
    expect(s.capBytes).toBeGreaterThan(0);
    expect(s.capBytes).toBeLessThanOrEqual(50 * GIB);
    // A tampered output is refused, typed.
    const [out2, key2] = job("fp2");
    writeFileSync(join(out2, key2, "table.bin"), "TABLE");
    await expect(cache.commit(out2, key2, [])).rejects.toMatchObject({ code: "PLAN_MISMATCH", details: { reason: "cacheEntry", file: "table.bin" } });
    await expect(cache.pin(key, "nobody")).rejects.toMatchObject({ code: "INVALID_OP" });
    // A hit for another project: `serve` records it and pins the entry while it is open.
    await cache.pinProject("p2");
    expect(await cache.serve(key, ["p2"])).toBe(true);
    expect(await cache.projectsOf(key)).toEqual(["p1", "p2"]);
    expect(await cache.pinsOf(key)).toEqual(["project:p2"]);
    expect(await cache.serve("0".repeat(64), ["p2"])).toBe(false);
    await expect(cache.serve(key, ["../x"])).rejects.toMatchObject({ code: "INVALID_OP" });
  });

  test("a full volume is CACHE_IO {freeBytes} (exit 5) and moves nothing", async () => {
    const cache = await MediaCache.open(serverData);
    const [out, key] = job("fp1");
    await cache.setFaultsForTest({ freeBytes: 4096 });
    const e = await cache.commit(out, key, []).catch((x: unknown) => x as { code: string; details: Record<string, unknown> });
    expect(e).toMatchObject({ code: "CACHE_IO", details: { reason: "full", freeBytes: 4096 } });
    expect(UNAVAILABLE_CODES).toContain("CACHE_IO");
    expect(readFileSync(join(out, key, "proxy.mp4"), "utf8")).toBe("mp4");
    expect((await cache.status()).entries).toBe(0);
  });

  test("a crash between the rename and the insert: the next open adopts the entry", async () => {
    const first = await MediaCache.open(serverData);
    const [out, key] = job("fp1");
    await first.setFaultsForTest({ stopAfterRename: true });
    await expect(first.commit(out, key, [])).rejects.toMatchObject({ code: "INTERNAL" });
    expect(await first.lookup(key)).toBeNull();
    const next = await MediaCache.open(serverData);
    expect(next.openReport()).toMatchObject({ adopted: 1, deleted: 0 });
    expect((await next.lookup(key))?.key).toBe(key);
    // Adopted entries serve no project until a commit or a hit's `serve` names one.
    expect(await next.projectsOf(key)).toEqual([]);
    expect(await next.serve(key, ["p1"])).toBe(true);
    expect(await next.projectsOf(key)).toEqual(["p1"]);
  });
});

describe("the service", () => {
  async function service(): Promise<{ svc: MediaCacheService; cache: MediaCache; scope: Scope.Closeable }> {
    const scope = await Effect.runPromise(Scope.make());
    const svc = await Effect.runPromise(Scope.provide(scope)(makeMediaCache(serverData, { everyMs: 3_600_000 })));
    if (svc.cache === undefined) throw new Error("the cache did not open");
    return { svc, cache: svc.cache, scope };
  }

  test("an open project's entries are pinned until it closes; gc spares them", async () => {
    const { svc, cache, scope } = await service();
    const [out, key] = job("fp1");
    await cache.commit(out, key, ["p1"]);
    const held = svc.holdProject("p1");
    expect(await cache.pinsOf(key)).toEqual(["project:p1"]);
    const kept = await Effect.runPromise(svc.gc(0));
    expect(kept).toMatchObject({ evicted: 0 });
    expect(kept.bytes).toBeGreaterThan(0);
    held.release();
    held.release();
    expect(await cache.pinsOf(key)).toEqual([]);
    expect(await Effect.runPromise(svc.gc(0))).toMatchObject({ evicted: 1, bytes: 0 });
    expect((await failure(svc.gc(-1))).code).toBe("INVALID_OP");
    await Effect.runPromise(Scope.close(scope, Exit.void));
  });

  test("the timer's work reads media.cacheMaxGiB and evicts down to it; an unusable file keeps the cap", async () => {
    const { svc, cache, scope } = await service();
    for (const fp of ["a", "b"]) {
      const [out, key] = job(fp);
      await cache.commit(out, key, []);
    }
    const before = (await cache.status()).capBytes;
    settingsFile({ media: { cacheMaxGiB: 1 / GIB } });
    await svc.tick();
    expect(await cache.status()).toMatchObject({ capBytes: 1, entries: 0 });
    writeFileSync(join(serverData, "settings.json"), "{");
    await svc.tick();
    expect((await cache.status()).capBytes).toBe(1);
    rmSync(join(serverData, "settings.json"));
    await svc.tick();
    expect((await cache.status()).capBytes).toBe(before);
    await Effect.runPromise(Scope.close(scope, Exit.void));
  });

  test("a cache that cannot open leaves the service up: logged, every call CACHE_IO", async () => {
    // `cache/media` is a file: the directory cannot be made.
    mkdirSync(join(serverData, "cache"));
    writeFileSync(join(serverData, "cache", "media"), "not a directory");
    const logs: string[] = [];
    const scope = await Effect.runPromise(Scope.make());
    const svc = await Effect.runPromise(Scope.provide(scope)(makeMediaCache(serverData, { everyMs: 3_600_000, log: (m) => logs.push(m) })));
    expect(svc.cache).toBeUndefined();
    expect(logs.join("\n")).toMatch(/media cache: unavailable \(CACHE_IO: /);
    expect(await failure(svc.status())).toMatchObject({ code: "CACHE_IO", details: { reason: "io" } });
    expect((await failure(svc.gc())).code).toBe("CACHE_IO");
    svc.holdProject("p1").release();
    await svc.tick();
    await Effect.runPromise(Scope.close(scope, Exit.void));
  });
});

describe("the RPCs on a running server", () => {
  const code = (e: RpcExit) => (e.ok ? undefined : e.error.code);

  async function consoleRun(argv: string[]): Promise<{ code: number; out: string; err: string }> {
    let out = "";
    let err = "";
    const c = await runConsole([...argv, "--server-data", serverData], {
      prompt: person(),
      output: { out: (t) => void (out += `${t}\n`), err: (t) => void (err += `${t}\n`) },
    });
    return { code: c, out, err };
  }

  test("status is server.read; gc is the owner's; the cap comes from settings.json; open projects pin again after a restart", { timeout: 120_000 }, async () => {
    server = await startServer(serverData);
    const local = await connectProved(server);
    const projectDir = join(tmp, "P");
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: projectDir, name: "P", rate: "30" })).projectId;
    expect(ok(await local.call("media.cache.status", {}))).toMatchObject({ bytes: 0, entries: 0, pinned: 0 });
    // `fin` (a `local` principal) never holds `settings.admin`.
    expect(code(await local.call("media.cache.gc", {}))).toBe("HUMAN_ONLY");
    await local.close();
    await server.stop();

    // While the server is down: an entry serving the project, and a 3 GiB cap.
    const offline = await MediaCache.open(serverData);
    const [out, key] = job("fp1");
    await offline.commit(out, key, [projectId]);
    const [out2, key2] = job("fp2");
    await offline.commit(out2, key2, []);
    settingsFile({ media: { cacheMaxGiB: 3 } });

    server = await startServer(serverData);
    const again = await connectProved(server);
    let s = ok<{ entries: number; pinned: number; capBytes: number }>(await again.call("media.cache.status", {}));
    expect(s).toMatchObject({ entries: 2, pinned: 0, capBytes: 3 * GIB });
    ok(await again.call("project.open", { dir: projectDir }));
    for (let i = 0; i < 200 && s.pinned === 0; i++) {
      await new Promise((r) => setTimeout(r, 25));
      s = ok(await again.call("media.cache.status", {}));
    }
    expect(s.pinned).toBe(1);

    // The owner's console: `cache status`, then `cache gc --to 0` spares the open project's entry.
    const st = await consoleRun(["cache", "status", "--json"]);
    expect(st.code, st.err).toBe(0);
    expect(JSON.parse(st.out)).toMatchObject({ entries: 2, pinned: 1 });
    const gc = await consoleRun(["cache", "gc", "--to", "0", "--json"]);
    expect(gc.code, gc.err).toBe(0);
    expect(JSON.parse(gc.out)).toMatchObject({ evicted: 1 });
    expect(ok(await again.call("media.cache.status", {}))).toMatchObject({ entries: 1, pinned: 1 });
    expect((await consoleRun(["cache", "gc", "--to", "-1"])).code).toBe(2);
    await again.close();
    expect(key2).not.toBe(key);
  });

  test("a cache that cannot open does not stop the server: media.cache.* answer CACHE_IO", { timeout: 120_000 }, async () => {
    mkdirSync(join(serverData, "cache"));
    writeFileSync(join(serverData, "cache", "media"), "not a directory");
    server = await startServer(serverData);
    const local = await connectProved(server);
    expect(code(await local.call("media.cache.status", {}))).toBe("CACHE_IO");
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
    expect(projectId).toBeTruthy();
    await local.close();
  });
});
