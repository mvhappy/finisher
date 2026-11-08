// Library roots in fin-node (5D plan Task 8, Decision 3): the canonical root and the per-root
// probe threads. Needs the `test-hooks` binary (`build:test`): a hung stat is a test hook.
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { libraryCanonicalRoot, libraryProbeRoot, libraryProbeStatus, testHooks } from "../index.js";

interface Canonical {
  root: string;
  rootKey: string;
  storage: string;
  identity: { key: string; kind: string; caseSensitive: boolean; sample: unknown[]; rootFileId?: string | null };
}

let base: string;
let data: string;

beforeEach(() => {
  base = realpathSync(mkdtempSync(join(tmpdir(), "fin-node-library-")));
  data = join(base, "data");
  mkdirSync(data);
});

afterEach(() => {
  rmSync(base, { recursive: true, force: true });
});

const hooks = (): NonNullable<typeof testHooks> => {
  if (testHooks === undefined) throw new Error("these tests need the test-hooks build");
  return testHooks;
};

async function canon(path: string): Promise<Canonical> {
  return JSON.parse(await libraryCanonicalRoot(path, data)) as Canonical;
}

function root(name: string): string {
  const p = join(base, name);
  mkdirSync(p);
  writeFileSync(join(p, "a.mov"), "x");
  return p;
}

const probe = async (r: string, stored: Canonical["identity"], ms?: number): Promise<{ state: string; code?: string }> =>
  JSON.parse(await libraryProbeRoot(r, JSON.stringify(stored), ms)) as { state: string; code?: string };

describe("libraryCanonicalRoot", () => {
  test("resolves links and reports identity, key and storage without opening files", async () => {
    const real = root("real");
    // A junction on Windows: a directory symlink needs a privilege a non-elevated user lacks.
    symlinkSync(real, join(base, "link"), process.platform === "win32" ? "junction" : "dir");
    const c = await canon(join(base, "link"));
    expect(c.root).toBe(real);
    // `root` is the display path and keeps its case; `rootKey` is a lookup and overlap key,
    // lower-cased on a case-insensitive volume (macOS APFS default, Windows) and as-is elsewhere.
    // Keys use `/` separators on Windows too (Decision 10).
    const unified = process.platform === "win32" ? real.replaceAll("\\", "/") : real;
    expect(c.rootKey).toBe(c.identity.caseSensitive ? unified : unified.toLowerCase());
    if (process.platform === "darwin" && !c.identity.caseSensitive) expect(c.rootKey).not.toBe(real);
    expect(c.storage).toBe("local");
    expect(c.identity.key).toMatch(/^[0-9a-f]{32}$/);
    expect(c.identity.sample).toHaveLength(1);
    expect(readFileSync(join(real, "a.mov"), "utf8")).toBe("x");
  });

  test("refuses what is not a usable root", async () => {
    const reason = async (p: string): Promise<unknown> => {
      const e = await libraryCanonicalRoot(p, data).then(
        () => undefined,
        (err: Error & { code?: string; details?: { reason?: string } }) => err,
      );
      expect(e?.code).toBe("LIBRARY_ROOT_INVALID");
      return e?.details?.reason;
    };
    expect(await reason("not/absolute")).toBe("relative");
    expect(await reason(join(base, "nope"))).toBe("notFound");
    expect(await reason(join(root("r"), "a.mov"))).toBe("notDirectory");
    expect(await reason(data)).toBe("serverData");
    expect(await reason(base)).toBe("serverData");
  });
});

describe("libraryProbeRoot", () => {
  test("judges a root against its stored identity", async () => {
    const r = root("probe");
    const c = await canon(r);
    expect((await probe(r, c.identity)).state).toBe("online");
    // A different key whose root id and sample no longer match: a changed volume.
    const other = { ...c.identity, key: "f".repeat(32), rootFileId: "AAAAAAAAAAA=", sample: [] };
    expect((await probe(r, other)).state).toBe("volumeChanged");
    rmSync(r, { recursive: true });
    expect(await probe(r, c.identity)).toMatchObject({ state: "unreachable", code: "NOT_FOUND" });
  });

  test("probe_runs_off_the_libuv_pool", async () => {
    // libuv's pool has four threads: four probes that ran on it would starve `readFile`.
    const roots = await Promise.all(["h1", "h2", "h3", "h4"].map(async (n) => ({ path: root(n), ...(await canon(join(base, n))) })));
    const file = join(base, "h1", "a.mov");
    for (const r of roots) hooks().libraryProbeHang(r.path, true);
    const pending = roots.map((r) => probe(r.path, r.identity, 10_000));
    await new Promise((resolve) => setTimeout(resolve, 50));
    const t0 = performance.now();
    await Promise.all([readFile(file), readFile(file), readFile(file), readFile(file), readFile(file)]);
    expect(performance.now() - t0).toBeLessThan(500);
    for (const r of roots) hooks().libraryProbeHang(r.path, false);
    expect((await Promise.all(pending)).map((s) => s.state)).toEqual(["online", "online", "online", "online"]);
  });

  test("one_probe_per_root_outstanding", async () => {
    const r = root("single");
    const c = await canon(r);
    hooks().libraryProbeHang(r, true);
    const before = hooks().libraryProbeStarted(r);
    const calls = [probe(r, c.identity, 10_000), probe(r, c.identity, 10_000), probe(r, c.identity, 10_000)];
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(hooks().libraryProbeStarted(r) - before).toBe(1);
    hooks().libraryProbeHang(r, false);
    // All three callers share the one measurement, each judged against its own identity.
    expect((await Promise.all(calls)).map((s) => s.state)).toEqual(["online", "online", "online"]);
    expect(hooks().libraryProbeStarted(r) - before).toBe(1);
    await probe(r, c.identity);
    expect(hooks().libraryProbeStarted(r) - before).toBe(2);
  });

  test("timeout_leaves_thread_and_counts_it", async () => {
    const r = root("stuck");
    const c = await canon(r);
    const stuck0 = libraryProbeStatus().stuckProbes;
    hooks().libraryProbeHang(r, true);
    const before = hooks().libraryProbeStarted(r);
    expect((await probe(r, c.identity, 50)).state).toBe("timeout");
    expect(libraryProbeStatus().stuckProbes).toBe(stuck0 + 1);
    // Asked again while the thread is stuck, it waits on that thread: nothing new starts.
    expect((await probe(r, c.identity, 50)).state).toBe("timeout");
    expect(hooks().libraryProbeStarted(r) - before).toBe(1);
    expect(libraryProbeStatus().stuckProbes).toBe(stuck0 + 1);
    // The mount comes back: the thread returns, and the next probe clears the count.
    hooks().libraryProbeHang(r, false);
    expect((await probe(r, c.identity, 10_000)).state).toBe("online");
    expect(libraryProbeStatus().stuckProbes).toBe(stuck0);
  });
});
