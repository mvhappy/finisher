// In-process units of the SDK: the line codec, error mapping, results, job files and the
// repository's bundler. The envelope over real stdio is `sdk.test.ts`.
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { decodeLine, encodeLine, LineTooLong, MAX_LINE_BYTES } from "../src/envelope.ts";
import { fromJobError } from "../src/errors.ts";
import { ENVELOPE_VERSION, InputFile, OutputFile, openGrant, PluginError, toCommandOut, toPluginError } from "../src/index.ts";
import { bundle } from "../tools/bundle.mjs";

let tmp: string;
beforeAll(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-plugin-sdk-units-"));
});
afterAll(() => rmSync(tmp, { recursive: true, force: true }));

describe("the line codec", () => {
  test("adds v and seq, keeps hello's v, refuses lines over 1 MiB", () => {
    expect(ENVELOPE_VERSION).toBe(1);
    expect(JSON.parse(encodeLine(3, { t: "ready" }))).toEqual({ v: 1, t: "ready", seq: 3 });
    expect(encodeLine(1, { t: "alive" }).endsWith("\n")).toBe(true);
    expect(() => encodeLine(1, { t: "result", job: "j", value: "x".repeat(MAX_LINE_BYTES) })).toThrow(LineTooLong);
  });

  test("decodes host lines, checks v and seq, hands unknown types on", () => {
    expect(decodeLine('{"v":1,"t":"shutdown","seq":1}', 1)).toEqual({ v: 1, t: "shutdown", seq: 1 });
    expect(decodeLine('{"v":1,"t":"zap","id":4,"seq":2}', 2)).toEqual({ t: "unknown", type: "zap", id: 4 });
    expect(decodeLine('{"v":1,"t":"zap","seq":2}', 2)).toEqual({ t: "unknown", type: "zap" });
    expect(decodeLine('{"v":2,"t":"shutdown","seq":1}', 1)).toMatch(/v 2/);
    expect(decodeLine('{"v":1,"t":"shutdown","seq":2}', 1)).toMatch(/seq/);
    expect(decodeLine("nope", 1)).toMatch(/JSON/);
    expect(decodeLine("[1]", 1)).toMatch(/object/);
  });
});

describe("errors", () => {
  test("a PluginError keeps its code and carries fix in details", () => {
    expect(new PluginError("X", "m", { a: 1 }, "do y").toJobError()).toEqual({ code: "X", message: "m", details: { a: 1, fix: "do y" } });
    expect(new PluginError("X", "m").toJobError()).toEqual({ code: "X", message: "m" });
    const back = fromJobError({ code: "X", message: "m", details: { a: 1, fix: "do y" } });
    expect([back.code, back.details, back.fix]).toEqual(["X", { a: 1 }, "do y"]);
  });

  test("Node's ERR_ACCESS_DENIED is PLUGIN_PERMISSION_DENIED without the resource", () => {
    const e = Object.assign(new Error("Access to this API has been restricted"), { code: "ERR_ACCESS_DENIED", permission: "FileSystemWrite", resource: "/home/x" });
    expect(toPluginError(e).toJobError()).toEqual({ code: "PLUGIN_PERMISSION_DENIED", message: expect.any(String), details: { perm: "fs", nodePermission: "FileSystemWrite" } });
    const child = Object.assign(new Error("denied"), { code: "ERR_ACCESS_DENIED", permission: "ChildProcess" });
    expect(toPluginError(child).details).toEqual({ perm: "process", nodePermission: "ChildProcess" });
  });

  test("Node's own errors are not taken for plugin errors", () => {
    const enoent = Object.assign(new Error("ENOENT: no such file"), { code: "ENOENT", errno: -2, syscall: "open" });
    expect(toPluginError(enoent).toJobError()).toMatchObject({ code: "PLUGIN_COMMAND_FAILED", details: { name: "Error", nodeCode: "ENOENT" } });
    expect(toPluginError(Object.assign(new Error("x"), { code: "ERR_INVALID_ARG_TYPE" })).code).toBe("PLUGIN_COMMAND_FAILED");
    expect(toPluginError("a string").toJobError()).toEqual({ code: "PLUGIN_COMMAND_FAILED", message: "a string", details: { name: "string" } });
    expect(toPluginError({ code: "MINE", message: "m", details: { k: 1 }, fix: "f" }).toJobError()).toEqual({ code: "MINE", message: "m", details: { k: 1, fix: "f" } });
  });
});

describe("results", () => {
  test("{tx} and {data} become a CommandOut; nothing is null data", () => {
    expect(toCommandOut({ tx: [] })).toEqual({ kind: "tx", ops: [] });
    expect(toCommandOut({ data: { a: 1 } })).toEqual({ kind: "data", value: { a: 1 } });
    expect(toCommandOut(undefined)).toEqual({ kind: "data", value: null });
    expect(() => toCommandOut(7 as never)).toThrow(PluginError);
  });
});

describe("job files", () => {
  test("an input is read by position", () => {
    const path = join(tmp, "in.bin");
    writeFileSync(path, Buffer.from("0123456789"));
    const f = new InputFile({ name: "in/pcm", path, mode: "read", bytes: 10 });
    expect(Buffer.from(f.read(3, 4)).toString()).toBe("3456");
    expect(Buffer.from(f.read(8, 10)).toString()).toBe("89");
    expect(f.read(20, 4).length).toBe(0);
    f.close();
    expect(() => f.read(0, 1)).toThrow(/closed/);
  });

  test("an output takes writes inside its declared size only", () => {
    const path = join(tmp, "out.bin");
    writeFileSync(path, Buffer.alloc(8));
    const f = openGrant([{ name: "out/a", path, mode: "write", bytes: 8 }], "out/a", "output");
    f.write(2, Buffer.from("abc"));
    expect(() => f.write(6, Buffer.from("xyz"))).toThrow(expect.objectContaining({ code: "PLUGIN_OUTPUT_INVALID", details: expect.objectContaining({ reason: "size" }) }));
    f.close();
    const back = new InputFile({ name: "out/a", path, mode: "read" });
    expect(Buffer.from(back.read(0, 8))).toEqual(Buffer.from([0, 0, 97, 98, 99, 0, 0, 0]));
    back.close();
  });

  test("a file of another size, a missing grant, a wrong mode and (unix) a symlink are refused", () => {
    const path = join(tmp, "sized.bin");
    writeFileSync(path, Buffer.alloc(4));
    expect(() => new OutputFile({ name: "out/b", path, mode: "write", bytes: 5 })).toThrow(expect.objectContaining({ code: "PLUGIN_OUTPUT_INVALID" }));
    expect(() => new OutputFile({ name: "out/b", path, mode: "write" })).toThrow(/declared size/);
    expect(() => new InputFile({ name: "in/b", path, mode: "read", bytes: 5 })).toThrow(expect.objectContaining({ code: "PLUGIN_PROTOCOL" }));
    expect(() => new OutputFile({ name: "in/c", path, mode: "read", bytes: 4 })).toThrow(/not writable/);
    expect(() => openGrant([], "in/x", "input")).toThrow(expect.objectContaining({ code: "PLUGIN_PROTOCOL" }));
    if (process.platform !== "win32") {
      const link = join(tmp, "link.bin");
      symlinkSync(path, link);
      expect(() => new InputFile({ name: "in/l", path: link, mode: "read" })).toThrow(expect.objectContaining({ code: "ELOOP" }));
    }
  });
});

describe("the bundler", () => {
  const write = (name: string, text: string): string => {
    const p = join(tmp, name);
    writeFileSync(p, text);
    return p;
  };

  test("joins modules into one that imports only node:", async () => {
    write("b.ts", 'import { join } from "node:path";\nimport type { X } from "./nope.ts";\nexport const B: string = join("b", "c");\nexport function twice(n: number): number { return n * 2; }\n');
    write("c.ts", 'export * from "./b.ts";\nexport { twice as double } from "./b.ts";\n');
    const entry = write("a.ts", 'import { B, double } from "./c.ts";\nimport * as fs from "node:fs";\nexport { B };\nglobalThis.__bundleTest = [B, double(2), typeof fs.readSync];\n');
    const out = bundle({ entry, outfile: join(tmp, "out", "a.mjs") });
    expect(out.match(/^import .*$/gm)).toEqual(['import * as __fin_node_0 from "node:path";', 'import * as __fin_node_1 from "node:fs";']);
    await import(join(tmp, "out", "a.mjs"));
    expect((globalThis as { __bundleTest?: unknown }).__bundleTest).toEqual(["b/c", 4, "function"]);
  });

  test("refuses what it does not understand", () => {
    const cases: Array<[string, RegExp]> = [
      ['import x from "left-pad";\n', /only node:, relative and aliased/],
      ["export default 1;\n", /export default/],
      ["export const { a } = { a: 1 };\n", /destructuring/],
      ['const m = await import("./x.ts");\n', /dynamic import/],
    ];
    for (const [i, [text, why]] of cases.entries()) expect(() => bundle({ entry: write(`bad${i}.ts`, text) })).toThrow(why);
    write("cy1.ts", 'import { b } from "./cy2.ts";\nexport const a = 1;\n');
    write("cy2.ts", 'import { a } from "./cy1.ts";\nexport const b = 2;\n');
    expect(() => bundle({ entry: join(tmp, "cy1.ts") })).toThrow(/cycle/);
  });
});
