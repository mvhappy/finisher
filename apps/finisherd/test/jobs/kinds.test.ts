// The kind, family and helper registry and the test kinds (5C-1 plan Task 7, Decisions 2,
// 27, 38).
import * as finNode from "@finisher/fin-node";
import { Effect } from "effect";
import { describe, expect, test } from "vitest";
import { type KindSpec, makeKindRegistry, mappedFileNameError, resolveOrFn, validateMappedFiles } from "../../src/jobs/kinds.ts";
import { registerTestKinds, testOnCall } from "../../src/jobs/kinds/test.ts";

const fam = { family: "fam", profile: "worker", program: { binary: "fin-workers", args: ["fam"] } } as const;
const spec = (over: Partial<KindSpec> = {}): KindSpec => ({
  kind: "fam.run",
  version: 1,
  family: "fam",
  scopes: [],
  resources: ["cpu"],
  defaultPriority: "normal",
  qos: "normal",
  timeoutS: 60,
  ...over,
});

describe("job kinds", () => {
  test("registry_refuses_duplicate_kinds_and_unknown_families", () => {
    const r = makeKindRegistry();
    expect(() => r.registerKind(spec())).toThrow(/unknown family fam/);
    r.registerFamily(fam);
    expect(() => r.registerFamily(fam)).toThrow(/already registered/);
    r.registerKind(spec());
    expect(() => r.registerKind(spec({ version: 2 }))).toThrow(/already registered/);
    expect(r.kind("fam.run")?.version).toBe(1);
    expect(r.family("fam")?.profile).toBe("worker");
    expect(r.kind("nope")).toBeUndefined();
    expect(() => r.registerKind(spec({ kind: "Bad.kind" }))).toThrow(/must match/);
    expect(() => r.registerKind(spec({ kind: "fam.zero", version: 0 }))).toThrow(/version/);
    expect(() => r.registerKind(spec({ kind: "fam.never", timeoutS: 0 }))).toThrow(/timeoutS/);
    expect(() => r.registerFamily({ ...fam, family: "Fam" })).toThrow(/must match/);
    expect(() => r.registerFamily({ ...fam, family: "warm3", warm: { slots: 3 } })).toThrow(/slots/);
    expect(() => r.registerFamily({ ...fam, family: "warmarg", program: { binary: "fin-workers", args: ["x", "--warm"] } })).toThrow(/engine appends --warm/);
    r.registerFamily({ ...fam, family: "dyn", program: (job) => ({ binary: "fin-workers", args: ["dyn", String(job.id)] }) });
    expect(r.program("dyn", { id: "j9" } as never, "warm")).toEqual({ binary: "fin-workers", args: ["dyn", "j9", "--warm"] });
    r.registerFamily({ ...fam, family: "dynbad", program: () => ({ binary: "fin-workers", args: ["--warm"] }) });
    expect(() => r.program("dynbad", { id: "j" } as never, "cold")).toThrow(/engine appends/);
    const h = { kind: "codec", id: "x", profile: "worker", resolve: () => ({ binary: "fin-workers" as const, args: [] }) } as const;
    r.registerHelper(h);
    expect(() => r.registerHelper(h)).toThrow(/already registered/);
    expect(r.helper("codec", "x")).toBe(h);
    expect(r.helper("plugin", "x")).toBeUndefined();
    expect(resolveOrFn<{ n: number }, number>((p) => p.n * 2, { n: 2 })).toBe(4);
    expect(resolveOrFn(["cpu"], {})).toEqual(["cpu"]);
  });

  test("mapped_file_names_validated", () => {
    for (const ok of ["a", "out/", "in/snapshot.json", "a/b/c/d.bin", "x_y-z.1", "console", "com0", "con_x", ".hidden"]) {
      expect(mappedFileNameError(ok), ok).toBeUndefined();
    }
    const bad: Array<[string, RegExp]> = [
      ["", /empty/],
      ["/abs", /relative/],
      ["C:/x", /relative/],
      ["a\\b", /relative/],
      ["../up", /".."/],
      ["a/../b", /".."/],
      ["./a", /"\."/],
      ["a/b/c/d/e", /deeper/],
      ["sp ace", /segment/],
      ["a//b", /segment/],
      ["ünï", /segment/],
      ["out.", /dot or a space/],
      ["a./b", /dot or a space/],
      ["CON", /device name/],
      ["in/nul.txt", /device name/],
      ["Com1", /device name/],
      ["lpt9.bin/x", /device name/],
    ];
    for (const [name, why] of bad) expect(mappedFileNameError(name), name).toMatch(why);
    const gib = 1 << 30;
    expect(validateMappedFiles([{ name: "a", bytes: 4096, mode: "write" }, { name: "out/", mode: "write" }])).toEqual([]);
    expect(validateMappedFiles([{ name: "a", mode: "read" }, { name: "a", mode: "read" }])).toEqual([`"a": listed twice`]);
    expect(validateMappedFiles([{ name: "d/", bytes: 1, mode: "write" }])[0]).toMatch(/directory has no size/);
    expect(validateMappedFiles([{ name: "big", bytes: gib + 1, mode: "write" }])[0]).toMatch(/1 GiB/);
    expect(validateMappedFiles([{ name: "neg", bytes: -1, mode: "write" }])[0]).toMatch(/bad size/);
    // Names, or directories on their paths, that differ only in case are one file on Windows
    // and macOS.
    expect(validateMappedFiles([{ name: "Out.bin", mode: "write" }, { name: "out.bin", mode: "write" }])[0]).toMatch(/only in case/);
    expect(validateMappedFiles([{ name: "In/a", mode: "read" }, { name: "in/b", mode: "read" }])[0]).toMatch(/"in" differs from "In" only in case/);
    expect(validateMappedFiles([{ name: "out/", mode: "write" }, { name: "OUT/x", mode: "write" }])[0]).toMatch(/only in case/);
    expect(validateMappedFiles([{ name: "in", mode: "read" }, { name: "in/a", mode: "read" }])[0]).toMatch(/both a file and a directory/);
    expect(validateMappedFiles([{ name: "in/a", mode: "read" }, { name: "in/b", mode: "read" }, { name: "out/", mode: "write" }, { name: "out/x", mode: "write" }])).toEqual([]);
    const five = [1, 2, 3, 4, 5].map((i) => ({ name: `f${i}`, bytes: gib, mode: "write" }) as const);
    expect(validateMappedFiles(five)).toEqual(["the files pass 4 GiB per job"]);
    const r = makeKindRegistry();
    r.registerFamily(fam);
    expect(() => r.registerKind(spec({ files: [{ name: "../x", mode: "read" }] }))).toThrow(/".."/);
  });

  test("test_kinds_register", async () => {
    const r = makeKindRegistry();
    registerTestKinds(r, { binaryDir: "/bin-dir" });
    expect(r.kinds().length).toBe(18);
    expect(r.kind("test.crash")?.batch?.restrict({ items: ["a", "b"], how: "abort" } as never, ["b"])).toEqual({ items: ["b"], how: "abort" });
    expect(r.kind("test.checkpoint")?.resumable).toBe(true);
    expect(r.kind("test.warm")).toMatchObject({ warm: true, defaultPriority: "interactive" });
    expect(r.kind("test.files")?.files).toEqual([
      { name: "in/data.txt", mode: "read" },
      { name: "out.bin", bytes: 4096, mode: "write" },
      { name: "out/", mode: "write" },
    ]);
    expect(resolveOrFn(r.kind("test.spawn")!.files!, { out: "pids.json" })).toEqual([{ name: "pids.json", bytes: 4096, mode: "write" }]);
    expect(r.kind("test.helper")?.helpers).toEqual([{ kind: "codec", id: "test", profile: "worker", files: ["helper.bin"] }]);
    expect(r.family("testplugin")?.profile).toBe("plugin");
    const program = r.family("testplugin")?.program;
    expect(program).toMatchObject({ args: ["testplugin"] });
    expect((program as { path: string }).path).toMatch(/^\/bin-dir[\\/]fin-workers(\.exe)?$/);
    const job = { params: { key: "A" } } as never;
    expect(r.family("testplugin")?.warmKey?.(job)).toBe("A");
    expect(r.helper("codec", "test")?.resolve({ params: { helper: "spawn" } } as never)).toEqual({ binary: "fin-workers", args: ["--helper", "test:spawn"] });
    expect(r.family("test")?.diagKinds).toEqual(["test.diag"]);
    // The engine decides warmness: argv per spawn mode, `--warm` appended (paths included).
    const any = { params: { key: "A" } } as never;
    expect(r.program("test", any, "cold")).toEqual({ binary: "fin-workers", args: ["test"] });
    expect(r.program("test", any, "warm")).toEqual({ binary: "fin-workers", args: ["test", "--warm"] });
    expect(r.program("testplugin", any, "warm")).toMatchObject({ args: ["testplugin", "--warm"] });
    expect((r.program("testplugin", any, "warm") as { path: string }).path).toMatch(/fin-workers(\.exe)?$/);
    expect(() => r.program("nope", any, "cold")).toThrow(/unknown family/);
    const ctx = { job: { id: "j1" }, role: "worker", owner: { kind: "system", id: "x" } } as never;
    expect(await Effect.runPromise(testOnCall(ctx, "test.echo", { a: 1 }))).toMatchObject({ echo: { a: 1 }, job: "j1", role: "worker" });
    const fail = await Effect.runPromise(Effect.flip(testOnCall(ctx, "test.fail", { b: 2 })));
    expect(fail).toMatchObject({ code: "TEST_FAIL", details: { b: 2 } });
    const unknown = await Effect.runPromise(Effect.flip(testOnCall(ctx, "nope", {})));
    expect(unknown).toMatchObject({ code: "INVALID_REQUEST", details: { method: "nope" } });
    expect(await Effect.runPromise(testOnCall(ctx, "test.slow", { ms: 5 }))).toEqual({ ms: 5 });
  });

  const validate = (finNode as unknown as Record<string, unknown>).validateJobParams as
    | ((kind: string, version: number, params: unknown) => { ok: boolean })
    | undefined;
  if (validate === undefined) console.warn("test_kinds_match_rust_params: skipped until Task 6 adds fin-node's validateJobParams");
  test.skipIf(validate === undefined)("test_kinds_match_rust_params", () => {
    const r = makeKindRegistry();
    registerTestKinds(r, { binaryDir: "/bin-dir" });
    const known = (finNode as unknown as { jobKindVersions(): Array<{ kind: string; version: number }> }).jobKindVersions();
    for (const k of r.kinds()) expect(known, k.kind).toContainEqual({ kind: k.kind, version: k.version });
  });
});
