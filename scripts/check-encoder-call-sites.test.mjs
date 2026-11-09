import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-encoder-call-sites.mjs", ["--root", root]);
const repo = fileURLToPath(new URL("..", import.meta.url));

test("a bare VTCompressionSessionCreate in fin-workers is one failure", () => {
  const r = gate(fixtureRoot({ "crates/fin-workers/src/media/proxy.rs": "fn f() {\n    unsafe { VTCompressionSessionCreate(a, b) };\n}\n" }));
  assert.equal(r.code, 1, r.out);
  assert.equal(r.out.trim().split("\n").filter((l) => l.startsWith("ENCODER_CALL_SITE")).length, 1, r.out);
  assert.match(r.out, /ENCODER_CALL_SITE crates\/fin-workers\/src\/media\/proxy\.rs:2: VTCompressionSessionCreate/);
});

test("each encoder primitive is allowed only in its backend's file", () => {
  const ok = fixtureRoot({
    "crates/fin-codecs/src/apple/encode.rs": "fn f() { VTCompressionSessionCreate(a); }\n",
    "crates/fin-codecs/src/mf/encode.rs": "const C: GUID = CLSID_MSH264EncoderMFT; const K: GUID = MFT_CATEGORY_VIDEO_ENCODER;\n",
    "crates/fin-codecs/src/mf/codecapi.rs": "fn g() { api.SetValue(&CODECAPI_AVEncMPVGOPSize, &v); }\n",
    "crates/fin-codecs/src/av1/svt_ffi.rs": "use fin_svt_sys::Encoder;\n",
    "crates/fin-codecs/src/av1/svt.rs": "use super::svt_ffi::Encoder;\n",
    "crates/fin-svt-sys/src/lib.rs": "pub fn svt_av1_enc_init() {}\n",
  });
  assert.equal(gate(ok).code, 0, gate(ok).out);
  const bad = fixtureRoot({
    "crates/fin-codecs/src/mf/decode.rs": "const C: GUID = CLSID_MSH264EncoderMFT;\n",
    "crates/fin-codecs/src/probe/mf.rs": "fn g() { api.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &v); }\n",
    "crates/fin-codecs/src/reader.rs": "use fin_svt_sys::Encoder;\n",
    "crates/fin-workers/src/media/x.rs": "use fin_codecs::av1::svt_ffi::Encoder;\n",
  });
  const r = gate(bad);
  assert.equal(r.code, 1);
  assert.equal(r.out.trim().split("\n").filter((l) => l.startsWith("ENCODER_CALL_SITE")).length, 4, r.out);
});

test("open_encoder callers name an EncodePurpose", () => {
  const ok = fixtureRoot({
    "crates/fin-codecs/src/registry.rs": "pub fn open_encoder(cfg: &EncoderConfig) {}\n",
    "crates/fin-workers/src/media/proxy.rs": "fn f() { let cfg = EncoderConfig { purpose: EncodePurpose::Proxy, ..x }; open_encoder(&cfg, &caps); }\n",
  });
  assert.equal(gate(ok).code, 0, gate(ok).out);
  const r = gate(fixtureRoot({ "crates/fin-workers/src/media/proxy.rs": "fn f(cfg: &EncoderConfig) {\n    fin_codecs::open_encoder(cfg, &caps);\n}\n" }));
  assert.equal(r.code, 1);
  assert.match(r.out, /ENCODER_CALL_SITE crates\/fin-workers\/src\/media\/proxy\.rs:2: open_encoder without an EncodePurpose/);
});

test("renamed, value-taken and multi-line open_encoder calls are caught", () => {
  const cases = {
    renamed: ["use fin_codecs::open_encoder as oe;\nfn f() { oe(&c); }\n", /:1: open_encoder renamed/],
    value: ["fn f() {\n    let o = fin_codecs::open_encoder;\n    o(&c);\n}\n", /:2: open_encoder taken as a value/],
    multiline: ["fn f() {\n    open_encoder\n        (&c, &caps);\n}\n", /:2: open_encoder without an EncodePurpose/],
  };
  for (const [name, [src, re]] of Object.entries(cases)) {
    const r = gate(fixtureRoot({ "crates/fin-workers/src/a.rs": src }));
    assert.equal(r.code, 1, `${name}: ${r.out}`);
    assert.match(r.out, re, name);
  }
  const ok = "use fin_codecs::{open_encoder, EncodePurpose};\nuse fin_codecs::open_encoder;\nfn f() { open_encoder(&EncoderConfig { purpose: EncodePurpose::Proxy }, &caps); }\n";
  assert.equal(gate(fixtureRoot({ "crates/fin-workers/src/a.rs": ok })).code, 0);
});

test("EncoderConfig may not have a Default", () => {
  for (const src of ["#[derive(Debug, Clone, Default)]\npub struct EncoderConfig { pub purpose: EncodePurpose }\n", "impl Default for EncoderConfig { fn default() -> Self { todo!() } }\n"]) {
    const r = gate(fixtureRoot({ "crates/fin-codecs/src/config.rs": src }));
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /EncoderConfig with a Default/);
  }
});

test("comments and strings are ignored", () => {
  const r = gate(fixtureRoot({ "crates/fin-workers/src/a.rs": '// VTCompressionSessionCreate is not called here\nconst S: &str = "CLSID_MSH264EncoderMFT open_encoder(";\n' }));
  assert.equal(r.code, 0, r.out);
});

test("passes on the real tree", () => {
  const r = gate(repo);
  assert.equal(r.code, 0, r.out);
});
