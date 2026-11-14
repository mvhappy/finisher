import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { gzipSync } from "node:zlib";
import { after } from "node:test";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";
import {
  EXPERIMENTAL_FILES,
  checkSnapshot,
  extractKept,
  generateArtifacts,
  integrityOf,
  throwawayEnv,
  verifyTarball,
} from "./codex-pin.mjs";

const dirs = [];
after(() => dirs.forEach((d) => rmSync(d, { recursive: true, force: true })));
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "fin-codexpin-"));
  dirs.push(d);
  return d;
};

/** A ustar archive of `{path: content}` (gzipped), the layout of a platform package. */
function tarball(files) {
  const blocks = [];
  for (const [path, content] of Object.entries(files)) {
    const body = Buffer.from(content);
    const h = Buffer.alloc(512);
    h.write(path, 0, "utf8");
    h.write("0000755\0", 100);
    h.write("0000000\0", 108);
    h.write("0000000\0", 116);
    h.write(body.length.toString(8).padStart(11, "0") + "\0", 124);
    h.write("00000000000\0", 136);
    h.write("        ", 148);
    h.write("0", 156);
    h.write("ustar\0" + "00", 257);
    let sum = 0;
    for (const b of h) sum += b;
    h.write(sum.toString(8).padStart(6, "0") + "\0 ", 148);
    blocks.push(h, body, Buffer.alloc((512 - (body.length % 512)) % 512));
  }
  blocks.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(blocks));
}
const sri = (buf) => "sha512-" + createHash("sha512").update(buf).digest("base64");
const V = "package/vendor/x86_64-unknown-linux-musl/";
const PACKAGE = {
  [V + "bin/codex"]: "CODEX-BIN",
  [V + "bin/codex-code-mode-host"]: "HOST",
  [V + "codex-path/rg"]: "RG-BIN",
  [V + "codex-resources/bwrap"]: "LGPL-BWRAP",
  [V + "codex-resources/voice/model.bin"]: "VOICE",
  [V + "codex-package.json"]: "{}",
  "package/package.json": "{}",
};
const KEPT = ["bin/codex", "codex-path/rg"];

test("pin_integrity_checked: a tampered tarball is refused and nothing is extracted", async () => {
  const dir = tmp();
  const good = tarball(PACKAGE);
  const file = join(dir, "pkg.tgz");
  writeFileSync(file, good);
  assert.equal(await integrityOf(file), sri(good));
  await verifyTarball(file, sri(good));
  const tampered = tarball({ ...PACKAGE, [V + "bin/codex"]: "EVIL-BIN" });
  writeFileSync(file, tampered);
  await assert.rejects(verifyTarball(file, sri(good)), /integrity/i);
  const dest = join(dir, "out");
  await assert.rejects(extractKept(file, dest, KEPT, { integrity: sri(good) }), /integrity/i);
  assert.equal(existsSync(dest) ? readdirSync(dest).length : 0, 0);
});

test("only_kept_files_extracted: bwrap, the code-mode host and voice never reach the cache", async () => {
  const dir = tmp();
  const file = join(dir, "pkg.tgz");
  const buf = tarball(PACKAGE);
  writeFileSync(file, buf);
  const dest = join(dir, "out");
  const written = await extractKept(file, dest, KEPT, { integrity: sri(buf) });
  assert.deepEqual(readdirSync(dest).sort(), ["codex", "rg"]);
  assert.deepEqual(written.sort(), ["codex", "rg"]);
  assert.equal(readFileSync(join(dest, "codex"), "utf8"), "CODEX-BIN");
  assert.equal(readFileSync(join(dest, "rg"), "utf8"), "RG-BIN");
});

test("a kept file missing from the tarball is an error", async () => {
  const dir = tmp();
  const file = join(dir, "pkg.tgz");
  const { [V + "codex-path/rg"]: _rg, ...rest } = PACKAGE;
  const buf = tarball(rest);
  writeFileSync(file, buf);
  await assert.rejects(extractKept(file, join(dir, "out"), KEPT, { integrity: sri(buf) }), /codex-path\/rg/);
});

function goodRoot(extra = {}, pin = {}) {
  const v = "0.167.0";
  const base = `vendor/codex/${v}/`;
  const target = { integrity: "sha512-AAAA", kept: ["bin/codex", "codex-path/rg"] };
  const files = {
    "vendor/codex/pin.json": JSON.stringify({
      version: v,
      targets: Object.fromEntries(
        ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64"].map((t) => [t, target]).concat(
          ["win32-x64", "win32-arm64"].map((t) => [t, { ...target, kept: ["bin/codex.exe", "codex-path/rg.exe"] }]),
        ),
      ),
      ...pin,
    }),
    [base + "schema/codex_app_server_protocol.schemas.json"]: "{}",
    [base + "schema/v2/Thing.json"]: "{}",
    ...Object.fromEntries(EXPERIMENTAL_FILES.map((f) => [base + "schema-experimental/" + f, "{}"])),
    [base + "features.txt"]: "apps stable true\n",
    [base + "LICENSE"]: "Apache License\nVersion 2.0\n",
    [base + "NOTICE"]: "OpenAI Codex\n",
    ...extra,
  };
  return fixtureRoot(files);
}
const gate = (root) => runGate("./check-codex-pin.mjs", ["--root", root]);

test("gate accepts a complete pin", () => {
  const r = gate(goodRoot());
  assert.equal(r.code, 0, r.out);
});

test("gate_refuses_bwrap_in_kept", () => {
  const target = { integrity: "sha512-AAAA", kept: ["bin/codex", "codex-path/rg", "codex-resources/bwrap"] };
  const r = gate(goodRoot({}, { targets: { "linux-x64": target } }));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /CODEX_PIN.*bwrap/);
});

test("gate refuses a kept file that is not codex or rg, and a stray bwrap file", () => {
  const r = gate(goodRoot({}, { targets: { "linux-x64": { integrity: "sha512-AAAA", kept: ["bin/codex-code-mode-host"] } } }));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /CODEX_PIN/);
  const r2 = gate(goodRoot({ "resources/codex/linux-x64/bwrap": "x" }));
  assert.equal(r2.code, 1, r2.out);
  assert.match(r2.out, /CODEX_PIN.*bwrap/);
});

test("gate refuses a snapshot that is not the pin's version, or lacks its files", () => {
  const r = gate(goodRoot({}, { version: "0.168.0" }));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /CODEX_PIN.*0\.168\.0/);
  const root = fixtureRoot({ "vendor/codex/pin.json": JSON.stringify({ version: "0.167.0", targets: {} }) });
  assert.equal(gate(root).code, 1);
});

test("gate refuses a missing or non-sha512 integrity", () => {
  const target = { integrity: "sha1-zzz", kept: ["bin/codex", "codex-path/rg"] };
  const r = gate(goodRoot({}, { targets: { "linux-x64": target } }));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /CODEX_PIN.*integrity/);
});

test("schema_and_features_written_under_throwaway_home: spawn args and env", () => {
  const dir = tmp();
  const calls = [];
  const run = (cmd, args, opts) => {
    calls.push({ cmd, args, env: opts.env });
    const out = args.indexOf("--out");
    if (out !== -1) {
      mkdirSync(args[out + 1], { recursive: true });
      writeFileSync(join(args[out + 1], "x.json"), "{}");
      for (const f of EXPERIMENTAL_FILES) writeFileSync(join(args[out + 1], f), "{}");
    }
    return { status: 0, stdout: args[0] === "features" ? "apps stable true\n" : "", stderr: "" };
  };
  const base = join(dir, "work");
  const outDir = join(dir, "snap");
  const hostHome = "/home/someone-else";
  const prev = process.env.HOME;
  process.env.HOME = hostHome;
  try {
    generateArtifacts({ bin: "/cache/codex", outDir, workDir: base, run });
  } finally {
    process.env.HOME = prev;
  }
  assert.deepEqual(
    calls.map((c) => c.args.slice(0, 2).join(" ")),
    ["app-server generate-json-schema", "app-server generate-json-schema", "features list"],
  );
  assert.ok(calls[1].args.includes("--experimental"));
  assert.ok(!calls[0].args.includes("--experimental"));
  assert.equal(calls[0].args[calls[0].args.indexOf("--out") + 1], join(outDir, "schema"));
  for (const c of calls) {
    assert.ok(!c.args.some((a) => /login|logout/.test(a)));
    assert.ok(c.env.HOME.startsWith(base) && c.env.CODEX_HOME.startsWith(base), "throwaway home");
    assert.notEqual(c.env.HOME, hostHome);
    for (const k of Object.keys(c.env)) assert.ok(["HOME", "USERPROFILE", "CODEX_HOME", "PATH", "TMPDIR", "SystemRoot", "TEMP", "TMP"].includes(k), k);
    assert.ok(!Object.values(c.env).some((v) => String(v).includes(hostHome)));
  }
  assert.equal(readFileSync(join(outDir, "features.txt"), "utf8"), "apps stable true\n");
  assert.ok(existsSync(join(outDir, "schema", "x.json")));
  assert.deepEqual(readdirSync(join(outDir, "schema-experimental")).sort(), [...EXPERIMENTAL_FILES].sort());
  assert.equal(throwawayEnv("/w", "linux").HOME, "/w/home");
});

test("check_mode_compares_fresh_schema: a fresh generation is compared with the committed snapshot", () => {
  const dir = tmp();
  const snap = join(dir, "committed");
  const fake = (content) => (cmd, args) => {
    const out = args.indexOf("--out");
    if (out !== -1) {
      const d = args[out + 1];
      mkdirSync(join(d, "v2"), { recursive: true });
      writeFileSync(join(d, "a.json"), content);
      writeFileSync(join(d, "v2", "b.json"), "{}");
      for (const f of EXPERIMENTAL_FILES) writeFileSync(join(d, f), args.includes("--experimental") ? content + "x" : content);
    }
    return { status: 0, stdout: args[0] === "features" ? "f stable true\n" : "", stderr: "" };
  };
  generateArtifacts({ bin: "/c/codex", outDir: snap, workDir: join(dir, "w1"), run: fake("{\"a\":1}") });
  assert.ok(existsSync(join(snap, "schema-experimental", EXPERIMENTAL_FILES[0])));
  assert.ok(!existsSync(join(snap, "schema-experimental", "a.json")), "experimental keeps only its bundle files");
  assert.deepEqual(checkSnapshot({ bin: "/c/codex", snapshotDir: snap, workDir: join(dir, "w2"), run: fake("{\"a\":1}") }), []);
  const diffs = checkSnapshot({ bin: "/c/codex", snapshotDir: snap, workDir: join(dir, "w3"), run: fake("{\"a\":2}") });
  assert.ok(diffs.some((d) => /schema\/a\.json/.test(d)), diffs.join("\n"));
  assert.ok(diffs.some((d) => /schema-experimental/.test(d)));
  writeFileSync(join(snap, "features.txt"), "other\n");
  assert.ok(checkSnapshot({ bin: "/c/codex", snapshotDir: snap, workDir: join(dir, "w4"), run: fake("{\"a\":1}") }).some((d) => /features\.txt/.test(d)));
});
