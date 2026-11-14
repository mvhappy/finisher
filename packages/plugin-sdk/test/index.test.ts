// In-process units of the SDK: the line codec, error mapping, results, job files and the
// repository's bundler. The envelope over real stdio is `sdk.test.ts`.
import { chmodSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, truncateSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { decodeLine, EXIT_HOST_GONE, Envelope, encodeLine, type Incoming, LineTooLong, MAX_LINE_BYTES } from "../src/envelope.ts";
import { fromJobError } from "../src/errors.ts";
import { refuseLink } from "../src/files.ts";
import { ENVELOPE_VERSION, EXIT_PROTOCOL, InputFile, OutputFile, openGrant, PluginError, toCommandOut, toPluginError } from "../src/index.ts";
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

  test("a known host message of the wrong shape is a protocol error", () => {
    const start = { v: 1, seq: 1, t: "start", dirs: { job: "/j", tmp: "/j/t" }, limits: { aliveMs: 100 }, job: { id: "j1", kind: "plugin.command", kindVersion: 1, attempt: 1, params: {} } };
    expect(decodeLine(JSON.stringify(start), 1)).toMatchObject({ t: "start" });
    const bad: Array<Record<string, unknown>> = [
      { v: 1, seq: 1, t: "start" },
      { ...start, job: { ...start.job, id: 1 } },
      { ...start, job: { ...start.job, kindVersion: "1" } },
      { ...start, limits: {} },
      { ...start, dirs: null },
      { ...start, files: {} },
      { v: 1, seq: 1, t: "cancel" },
      { v: 1, seq: 1, t: "reply", id: "1", value: 1 },
      { v: 1, seq: 1, t: "reply", id: 1, error: { code: 1, message: "m" } },
      { v: 1, seq: 1, t: "attach", helper: "h" },
    ];
    for (const m of bad) expect([m, decodeLine(JSON.stringify(m), 1)]).toEqual([m, expect.stringMatching(new RegExp(`${m["t"]}`))]);
    expect(decodeLine('{"v":1,"seq":1,"t":"cancel","job":"j1","reason":"user"}', 1)).toMatchObject({ t: "cancel", job: "j1" });
    expect(decodeLine('{"v":1,"seq":1,"t":"reply","id":3,"error":{"code":"X","message":"m"}}', 1)).toMatchObject({ t: "reply", id: 3 });
  });
});

describe("the envelope's reader", () => {
  function open(): { input: PassThrough; output: PassThrough; got: Incoming[]; warned: string[]; exits: number[]; ended: () => number } {
    const input = new PassThrough();
    const output = new PassThrough();
    const got: Incoming[] = [];
    const warned: string[] = [];
    const exits: number[] = [];
    let ends = 0;
    new Envelope({ input, output, exit: (c) => exits.push(c), warn: (l) => warned.push(l) }, (m) => got.push(m), () => ends++);
    output.resume();
    return { input, output, got, warned, exits, ended: () => ends };
  }
  const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 20));

  test("splits lines across chunks, takes CRLF, keeps multibyte characters whole", async () => {
    const e = open();
    const line = Buffer.from('{"v":1,"seq":1,"t":"reply","id":1,"value":"é€"}\r\n{"v":1,"seq":2,"t":"shutdown"}\n');
    for (let i = 0; i < line.length; i += 7) e.input.write(line.subarray(i, i + 7));
    await tick();
    expect(e.got).toEqual([{ v: 1, seq: 1, t: "reply", id: 1, value: "é€" }, { v: 1, seq: 2, t: "shutdown" }]);
    e.input.end();
    await tick();
    expect(e.ended()).toBe(1);
    expect(e.exits).toEqual([]);
  });

  test("a host line over 1 MiB is refused before it is buffered whole: exit 65", async () => {
    const e = open();
    e.input.write("x".repeat(MAX_LINE_BYTES));
    await tick();
    expect(e.exits).toEqual([]);
    e.input.write("xx");
    await tick();
    expect(e.exits).toEqual([EXIT_PROTOCOL]);
    expect(e.warned.join("\n")).toMatch(/1048576/);
  });

  test("an unknown host type is handed on and described once per type", async () => {
    const e = open();
    e.input.write('{"v":1,"seq":1,"t":"zap"}\n{"v":1,"seq":2,"t":"zap"}\n{"v":1,"seq":3,"t":"zop"}\n');
    await tick();
    expect(e.got.map((m) => m.t)).toEqual(["unknown", "unknown", "unknown"]);
    expect(e.warned).toHaveLength(2);
    expect(e.exits).toEqual([]);
  });

  test("a failing output (EPIPE) ends the process with 70", async () => {
    const e = open();
    e.output.destroy(Object.assign(new Error("write EPIPE"), { code: "EPIPE" }));
    await tick();
    expect(e.exits).toEqual([EXIT_HOST_GONE]);
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
    // `perm` is a manifest permission name (only `fs` maps to one) or "unknown".
    const child = Object.assign(new Error("denied"), { code: "ERR_ACCESS_DENIED", permission: "ChildProcess" });
    expect(toPluginError(child).details).toEqual({ perm: "unknown", nodePermission: "ChildProcess" });
    expect(toPluginError({ code: "ERR_ACCESS_DENIED" }).details).toEqual({ perm: "unknown", nodePermission: "unknown" });
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
  const file = (name: string, bytes: number | Buffer): string => {
    const p = join(tmp, name);
    writeFileSync(p, typeof bytes === "number" ? Buffer.alloc(bytes) : bytes);
    return p;
  };

  test("an input is read by position", () => {
    const path = file("in.bin", Buffer.from("0123456789"));
    const f = openGrant([{ name: "in/pcm", path, mode: "read", bytes: 10 }], "in/pcm", "input");
    expect([f.name, f.size]).toEqual(["in/pcm", 10]);
    expect(Buffer.from(f.read(3, 4)).toString()).toBe("3456");
    expect(Buffer.from(f.read(8, 10)).toString()).toBe("89");
    expect(f.read(20, 4).length).toBe(0);
    expect(() => f.read(-1, 1)).toThrow(RangeError);
    f.close();
    expect(() => f.read(0, 1)).toThrow(/closed/);
  });

  test("an input that shrinks under the reader is truncated", () => {
    const path = file("shrinks.bin", 10);
    const f = new InputFile({ name: "in/s", path, mode: "read", bytes: 10 });
    truncateSync(path, 4);
    expect(() => f.read(0, 10)).toThrow(expect.objectContaining({ code: "PLUGIN_PROTOCOL", details: { file: "in/s", reason: "truncated" } }));
    f.close();
  });

  test("an output takes writes inside its declared size only", () => {
    const path = file("out.bin", 8);
    const f = openGrant([{ name: "out/a", path, mode: "write", bytes: 8 }], "out/a", "output");
    f.write(2, Buffer.from("abc"));
    expect(() => f.write(6, Buffer.from("xyz"))).toThrow(expect.objectContaining({ code: "PLUGIN_OUTPUT_INVALID", details: expect.objectContaining({ file: "out/a", reason: "size" }) }));
    f.close();
    const back = new InputFile({ name: "out/a", path, mode: "read" });
    expect(Buffer.from(back.read(0, 8))).toEqual(Buffer.from([0, 0, 97, 98, 99, 0, 0, 0]));
    back.close();
  });

  test("an output needs write access only (O_WRONLY)", () => {
    const path = file("wo.bin", 4);
    chmodSync(path, 0o200);
    try {
      const f = new OutputFile({ name: "out/w", path, mode: "write", bytes: 4 });
      f.write(0, Buffer.from("ab"));
      f.close();
    } finally {
      chmodSync(path, 0o600);
    }
  });

  test("an output truncated before close is PLUGIN_OUTPUT_INVALID {truncated}", () => {
    const path = file("cut.bin", 6);
    const f = new OutputFile({ name: "out/c", path, mode: "write", bytes: 6 });
    truncateSync(path, 2);
    expect(() => f.close()).toThrow(expect.objectContaining({ code: "PLUGIN_OUTPUT_INVALID", details: { file: "out/c", reason: "truncated", declared: 6, got: 2 } }));
  });

  test("a mode mismatch is PLUGIN_PERMISSION_DENIED {perm: fs, access}", () => {
    const path = file("mode.bin", 4);
    expect(() => new OutputFile({ name: "in/c", path, mode: "read", bytes: 4 })).toThrow(expect.objectContaining({ code: "PLUGIN_PERMISSION_DENIED", details: { perm: "fs", access: "write", file: "in/c" } }));
    expect(() => new InputFile({ name: "out/c", path, mode: "write", bytes: 4 })).toThrow(expect.objectContaining({ code: "PLUGIN_PERMISSION_DENIED", details: { perm: "fs", access: "read", file: "out/c" } }));
  });

  test("size, link and non-regular outputs are PLUGIN_OUTPUT_INVALID; inputs PLUGIN_PROTOCOL", () => {
    const path = file("sized.bin", 4);
    expect(() => new OutputFile({ name: "out/b", path, mode: "write", bytes: 5 })).toThrow(expect.objectContaining({ code: "PLUGIN_OUTPUT_INVALID", details: expect.objectContaining({ reason: "size" }) }));
    expect(() => new OutputFile({ name: "out/b", path, mode: "write" })).toThrow(expect.objectContaining({ code: "PLUGIN_OUTPUT_INVALID", details: expect.objectContaining({ reason: "size" }) }));
    expect(() => new InputFile({ name: "in/b", path, mode: "read", bytes: 5 })).toThrow(expect.objectContaining({ code: "PLUGIN_PROTOCOL", details: expect.objectContaining({ reason: "size" }) }));
    const dir = join(tmp, "a-dir");
    mkdirSync(dir, { recursive: true });
    expect(() => new InputFile({ name: "in/d", path: dir, mode: "read" })).toThrow(expect.objectContaining({ code: "PLUGIN_PROTOCOL", details: { file: "in/d", reason: "notRegular" } }));
    expect(() => openGrant([], "in/x", "input")).toThrow(expect.objectContaining({ code: "PLUGIN_PROTOCOL", details: { reason: "noFile", file: "in/x" } }));
    if (process.platform !== "win32") {
      const link = join(tmp, "link.bin");
      symlinkSync(path, link);
      expect(() => new InputFile({ name: "in/l", path: link, mode: "read" })).toThrow(expect.objectContaining({ code: "PLUGIN_PROTOCOL", details: { file: "in/l", reason: "link" } }));
      expect(() => new OutputFile({ name: "out/l", path: link, mode: "write", bytes: 4 })).toThrow(expect.objectContaining({ code: "PLUGIN_OUTPUT_INVALID", details: { file: "out/l", reason: "link" } }));
      // Windows' pre-check (no O_NOFOLLOW there), run here on the same link.
      expect(() => refuseLink(link, "output", "out/l")).toThrow(expect.objectContaining({ code: "PLUGIN_OUTPUT_INVALID", details: { file: "out/l", reason: "link" } }));
      expect(() => refuseLink(path, "output", "out/l")).not.toThrow();
    }
  });

  test("an I/O failure is PLUGIN_COMMAND_FAILED {reason: io}, without the path", () => {
    const missing = join(tmp, "nope", "missing.bin");
    let caught: unknown;
    try {
      new InputFile({ name: "in/m", path: missing, mode: "read" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toMatchObject({ code: "PLUGIN_COMMAND_FAILED", details: { file: "in/m", reason: "io", nodeCode: "ENOENT" } });
    expect(JSON.stringify((caught as PluginError).toJobError())).not.toContain(tmp);
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
      ["export const a = 1, b = 2;\n", /more than one binding/],
      ["export let a = f(1, 2),\n  b = 3;\n", /more than one binding/],
      ["export var a = 1\n  , b = 2;\n", /more than one binding/],
    ];
    for (const [i, [text, why]] of cases.entries()) expect(() => bundle({ entry: write(`bad${i}.ts`, text) })).toThrow(why);
    write("cy1.ts", 'import { b } from "./cy2.ts";\nexport const a = 1;\n');
    write("cy2.ts", 'import { a } from "./cy1.ts";\nexport const b = 2;\n');
    expect(() => bundle({ entry: join(tmp, "cy1.ts") })).toThrow(/cycle/);
  });

  test("commas inside one binding's initialiser are not a second binding", async () => {
    const entry = write(
      "one.ts",
      [
        "export const pair = [1, 2];",
        "export const add = (a: number, b: number): number => a + b;",
        "export const re = /a,b/g;",
        "export const s = \"a,b\" + 'c,d';",
        "export const t = `x${[1, 2].join(\",\")},y`;",
        "export const o = { a: 1, b: 2 }; // a, comment",
        "export const m = new Map<string, number>([[\"k\", 1]]);",
        "export const n = 3",
        "export const k = /* x, y */ 4;",
        "globalThis.__oneTest = [pair, add(1, 2), re.source, s, t, o, m.get(\"k\"), n, k];",
        "",
      ].join("\n"),
    );
    bundle({ entry, outfile: join(tmp, "out", "one.mjs") });
    await import(join(tmp, "out", "one.mjs"));
    expect((globalThis as { __oneTest?: unknown }).__oneTest).toEqual([[1, 2], 3, "a,b", "a,bc,d", "x1,2,y", { a: 1, b: 2 }, 1, 3, 4]);
  });
});
