// Catalog extensions by module URL (plan 5D Task 7a, Decision 24).
import { threadId } from "node:worker_threads";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { assertCloneable } from "../../src/library/catalog/protocol.ts";
import { CatalogExtensionRegistry, CORE_EXTENSION } from "../../src/library/catalog/extensions.ts";
import { NOTES_URL, type Scratch, failCode, newLib, run, scratch, stopAll, threads } from "./support/catalog.ts";

let s: Scratch;
beforeEach(() => {
  s = scratch();
});
afterEach(async () => {
  await stopAll();
  s.cleanup();
});

describe("the registry", () => {
  test("core is always first; others by name; same pair twice is a no-op", () => {
    const r = new CatalogExtensionRegistry();
    r.register({ name: "zeta", module: new URL("file:///z.ts") });
    r.register({ name: "notes", module: NOTES_URL });
    r.register({ name: "notes", module: NOTES_URL });
    expect(r.refs().map((e) => e.name)).toEqual(["core", "notes", "zeta"]);
    expect(r.refs()[0]).toEqual(CORE_EXTENSION);
  });

  test("names, URLs and late registration are checked", () => {
    const r = new CatalogExtensionRegistry();
    expect(() => r.register({ name: "Bad-Name", module: NOTES_URL })).toThrow(/must match/);
    expect(() => r.register({ name: "web", module: new URL("https://example.invalid/x.ts") })).toThrow(/file: URL/);
    expect(() => r.register({ name: "fn", module: (() => NOTES_URL) as unknown as URL })).toThrow(/file: URL/);
    r.register({ name: "notes", module: NOTES_URL });
    expect(() => r.register({ name: "notes", module: new URL("file:///other.ts") })).toThrow(/another URL/);
    r.start();
    expect(() => r.register({ name: "late", module: NOTES_URL })).toThrow(/before the catalog threads start/);
  });
});

test("extension_module_loaded_in_both_threads", async () => {
  const c = await threads(s);
  const lib = newLib();
  await run(c.indexer.open(lib));
  const w = (await run(c.indexer.exec(lib, "notes", "whoami", {}))) as { threadId: number };
  const r = (await run(c.reader.query(lib, "notes", "whoami", {}))) as { threadId: number };
  expect(w.threadId).not.toBe(threadId);
  expect(r.threadId).not.toBe(threadId);
  expect(w.threadId).not.toBe(r.threadId);
  await run(c.indexer.exec(lib, "notes", "add", { text: "hello" }));
  expect(await run(c.reader.query(lib, "notes", "list", {}))).toEqual([{ id: 1, text: "hello" }]);
});

test("exec_runs_in_a_transaction_and_query_is_read_only", async () => {
  const c = await threads(s);
  const lib = newLib();
  await run(c.indexer.open(lib));
  expect(await run(c.indexer.exec(lib, "notes", "inTransaction", {}))).toEqual({ inTransaction: true });
  expect(await failCode(c.indexer.exec(lib, "notes", "addThenFail", { text: "lost" }))).toBe("CATALOG_OP_FAILED");
  expect(await run(c.reader.query(lib, "notes", "list", {}))).toEqual([]);
  const code = await failCode(c.reader.query(lib, "notes", "tryWrite", {}));
  expect(code).toBe("CATALOG_OP_FAILED");
  expect(await run(c.reader.query(lib, "notes", "list", {}))).toEqual([]);
  // Unknown extensions, ops and queries, and a catalog that is not open.
  expect(await failCode(c.indexer.exec(lib, "nope", "add", {}))).toBe("CATALOG_EXTENSION_UNKNOWN");
  expect(await failCode(c.indexer.exec(lib, "notes", "toString", {}))).toBe("CATALOG_OP_UNKNOWN");
  expect(await failCode(c.reader.query(lib, "notes", "constructor", {}))).toBe("CATALOG_OP_UNKNOWN");
  expect(await failCode(c.reader.query(newLib(), "notes", "list", {}))).toBe("CATALOG_NOT_OPEN");
  expect(await failCode(c.indexer.exec("../../etc", "notes", "add", {}))).toBe("CATALOG_ARGS_INVALID");
});

describe("functions_never_cross_ports", () => {
  test("a non-cloneable argument is refused before posting", async () => {
    const c = await threads(s);
    const lib = newLib();
    await run(c.indexer.open(lib));
    const before = c.posted.indexer.count;
    const readerBefore = c.posted.reader.count;
    expect(await failCode(c.indexer.exec(lib, "notes", "add", { text: "x", f: () => 1 }))).toBe("CATALOG_ARG_NOT_CLONEABLE");
    expect(await failCode(c.indexer.exec(lib, "notes", "add", { nested: [{ s: Symbol("x") }] }))).toBe("CATALOG_ARG_NOT_CLONEABLE");
    expect(await failCode(c.indexer.exec(lib, "notes", "add", { when: new (class Point {})() }))).toBe("CATALOG_ARG_NOT_CLONEABLE");
    expect(await failCode(c.reader.query(lib, "notes", "list", { buf: Buffer.from("x") }))).toBe("CATALOG_ARG_NOT_CLONEABLE");
    expect(c.posted.indexer.count).toBe(before);
    expect(c.posted.reader.count).toBe(readerBefore);
  });

  test("a non-cloneable result is an error, not a crash", async () => {
    const c = await threads(s);
    const lib = newLib();
    await run(c.indexer.open(lib));
    expect(await failCode(c.indexer.exec(lib, "notes", "returnsFunction", {}))).toBe("CATALOG_RESULT_NOT_CLONEABLE");
    expect(await run(c.indexer.exec(lib, "notes", "whoami", {}))).toHaveProperty("threadId");
  });

  test("assertCloneable names the offending path and accepts plain data", () => {
    expect(() => assertCloneable({ a: [1, { b: () => 0 }] })).toThrow(expect.objectContaining({ details: { at: "args.a.1.b", type: "function" } }));
    const cyclic: Record<string, unknown> = { n: 1n, d: new Date(0), u: new Uint8Array(2), m: new Map([[1, "x"]]), o: Object.create(null) };
    cyclic.self = cyclic;
    expect(() => assertCloneable(cyclic)).not.toThrow();
    expect(() => assertCloneable({ get x() { return 1; } })).toThrow(/plain data/);
  });
});

test("a thread that cannot load an extension fails the start", async () => {
  const r = new CatalogExtensionRegistry();
  r.register({ name: "missing", module: new URL("./fixtures/does-not-exist.ts", import.meta.url) });
  await expect(threads(s, { extensions: r })).rejects.toMatchObject({ code: "CATALOG_OP_FAILED" });
});
