import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
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
  return gzipSync(rawTar(Object.entries(files)));
}
/** The uncompressed ustar bytes of `[path, content]` entries (a path may repeat), with the end blocks. */
function rawTar(entries, { end = true } = {}) {
  const blocks = [];
  for (const [path, content] of entries) {
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
  if (end) blocks.push(Buffer.alloc(1024));
  return Buffer.concat(blocks);
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

test("extract_fails_closed_on_a_truncated_archive: a cut body, a cut header or no end blocks write nothing", async () => {
  const dir = tmp();
  const two = rawTar([[V + "bin/codex", "CODEX-BIN"], [V + "codex-path/rg", "RG-BIN"]], { end: false });
  const cuts = {
    // The codex entry's body cut short (its header promised 9 bytes).
    body: rawTar([[V + "codex-path/rg", "RG-BIN"], [V + "bin/codex", "CODEX-BIN"]], { end: false }).subarray(0, 1024 + 512 + 4),
    // Both kept files whole, then half a header.
    header: Buffer.concat([two, rawTar([["package/package.json", "{}"]], { end: false }).subarray(0, 100)]),
    // Both kept files whole, and no end-of-archive blocks.
    noEnd: two,
  };
  for (const [name, raw] of Object.entries(cuts)) {
    const file = join(dir, `${name}.tgz`);
    const buf = gzipSync(raw);
    writeFileSync(file, buf);
    const dest = join(dir, `out-${name}`);
    await assert.rejects(extractKept(file, dest, KEPT, { integrity: sri(buf) }), /truncated|end of the archive/, name);
    assert.deepEqual(existsSync(dest) ? readdirSync(dest) : [], [], name);
  }
});

test("extract_refuses_duplicate_destinations: a second codex entry is refused, and every opened file removed", async () => {
  const dir = tmp();
  const buf = gzipSync(rawTar([[V + "bin/codex", "CODEX-BIN"], [V + "codex-path/rg", "RG-BIN"], [V + "bin/codex", "TRUNC"]]));
  const file = join(dir, "dup.tgz");
  writeFileSync(file, buf);
  const dest = join(dir, "out");
  await assert.rejects(extractKept(file, dest, KEPT, { integrity: sri(buf) }), /more than once|duplicate/);
  assert.deepEqual(readdirSync(dest), []);
  // Another triple's copy of a kept file lands on the same destination: refused as well.
  const buf2 = gzipSync(rawTar([[V + "bin/codex", "CODEX-BIN"], ["package/vendor/aarch64-unknown-linux-musl/bin/codex", "OTHER"], [V + "codex-path/rg", "RG-BIN"]]));
  writeFileSync(file, buf2);
  await assert.rejects(extractKept(file, dest, KEPT, { integrity: sri(buf2) }), /more than once|duplicate/);
  assert.deepEqual(readdirSync(dest), []);
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

/** Every target as the real pin has it, with `over` replacing some. */
const allTargets = (over) => {
  const t = { integrity: "sha512-AAAA", kept: ["bin/codex", "codex-path/rg"] };
  const w = { integrity: "sha512-AAAA", kept: ["bin/codex.exe", "codex-path/rg.exe"] };
  return { targets: { "linux-x64": t, "linux-arm64": t, "darwin-x64": t, "darwin-arm64": t, "win32-x64": w, "win32-arm64": w, ...over } };
};

test("gate refuses a kept file that is not codex or rg (only the allow-list's finding), and a stray bwrap file", () => {
  assert.equal(gate(goodRoot({}, allTargets({}))).code, 0);
  const r = gate(goodRoot({}, allTargets({ "linux-x64": { integrity: "sha512-AAAA", kept: ["bin/codex", "bin/codex-code-mode-host"] } })));
  assert.equal(r.code, 1, r.out);
  const found = r.out.split("\n").filter((l) => l.includes("CODEX_PIN"));
  assert.equal(found.length, 1, r.out);
  assert.match(found[0], /CODEX_PIN linux-x64: kept names bin\/codex-code-mode-host, only bin\/codex and codex-path\/rg/);
  const r2 = gate(goodRoot({ "resources/codex/linux-x64/bwrap": "x" }));
  assert.equal(r2.code, 1, r2.out);
  assert.match(r2.out, /CODEX_PIN.*bwrap/);
});

test("gate_refuses_bwrap_any_case: a Bwrap.exe or BWRAP anywhere in the tree", () => {
  for (const f of ["resources/codex/win32-x64/Bwrap.exe", "tools/BWRAP", "x/bWrap-helper"]) {
    const r = gate(goodRoot({ [f]: "x" }));
    assert.equal(r.code, 1, `${f}: ${r.out}`);
    assert.match(r.out, new RegExp(`CODEX_PIN ${f.replace(/[.]/g, "\\.")}: bwrap`));
  }
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

test("check_mode_needs_the_host_target: --check over targets this host cannot run fails, before any download", () => {
  const host = `${process.platform}-${process.arch}`;
  const other = ["linux-x64", "darwin-arm64"].find((t) => t !== host);
  const r = spawnSync(process.execPath, [fileURLToPath(new URL("./codex-pin.mjs", import.meta.url)), "--check", "--target", other], { encoding: "utf8", env: { ...process.env, npm_config_offline: "true" } });
  assert.equal(r.status, 1, r.stdout + r.stderr);
  assert.match(r.stderr, /--check.*host/);
  assert.doesNotMatch(r.stdout, /integrity verified/);
});
