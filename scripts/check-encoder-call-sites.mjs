// Encoder call-site gate (sub-project 6B-1 plan Decision 2, Global Constraints "Encoder
// policy"): one function, `fin_media::backend::encoder_allowed`, decides which encoder a purpose
// may use, and `fin_codecs::registry::open_encoder` is the only way to open one. So:
// - each backend's encoder primitives appear only in that backend's files (`PRIMITIVES`):
//   `VTCompressionSessionCreate` in `apple/encode.rs`; Microsoft's H.264 encoder MFT, the video
//   encoder MFT category and the encoder `CODECAPI_AVEnc*` properties in `mf/encode.rs` and
//   `mf/codecapi.rs`; SVT-AV1's bindings (`fin_svt_sys`) in `av1/svt_ffi.rs`, and its safe
//   wrapper (`svt_ffi`) in `av1/svt.rs` (the `fin-svt-sys` crate itself is not scanned), and
//   its opener (`open_av1_encoder`) in `av1/svt.rs` and the registry backend `av1/backends.rs`;
// - audio (6C plan Decision 10): `fin_media::backend_audio::audio_encoder_allowed` decides and
//   `fin_codecs::audio::registry::open_audio_encoder` opens; an audio encoder backend's `open`
//   takes an `AudioOpenToken`, which is made (`AudioOpenToken(())`) only in `audio/registry.rs`,
//   so that file is the only audio encoder opener. Tasks 13–16 add their encoder primitives here;
// - every file that calls `open_encoder(` or `open_audio_encoder(` names an `EncodePurpose::`
//   variant (its config's `purpose`, or the purpose argument); neither is renamed (`as`) or taken
//   as a value, so every call is seen; and `EncoderConfig` has no `Default`, so its `purpose`
//   field (required, no default) is always written out at the literal: the compiler holds the
//   per-call rule, this gate the per-file one.
// Rust sources under `crates/` and `apps/`, comments and string literals ignored. Each finding
// is `ENCODER_CALL_SITE <path>:<line>: <what>`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { code } from "./check-worker-spawn.mjs";
import { report, rootArg, walk } from "./walk.mjs";

const CODECS = "crates/fin-codecs/src/";
export const PRIMITIVES = [
  { name: "VTCompressionSessionCreate", re: /\bVTCompressionSessionCreate\b/, files: [`${CODECS}apple/encode.rs`] },
  { name: "Microsoft's H.264 encoder MFT", re: /\bCLSID_MSH264EncoderMFT\b/, files: [`${CODECS}mf/encode.rs`] },
  { name: "the video encoder MFT category", re: /\bMFT_CATEGORY_VIDEO_ENCODER\b/, files: [`${CODECS}mf/encode.rs`] },
  { name: "an encoder CODECAPI property", re: /\bCODECAPI_AVEnc\w*/, files: [`${CODECS}mf/encode.rs`, `${CODECS}mf/codecapi.rs`] },
  { name: "SVT-AV1's bindings", re: /\bfin_svt_sys\b/, files: [`${CODECS}av1/svt_ffi.rs`] },
  { name: "SVT-AV1's encoder wrapper", re: /\bsvt_ffi\b/, files: [`${CODECS}av1/svt.rs`, `${CODECS}av1/mod.rs`] },
  { name: "SVT-AV1's opener (use open_encoder)", re: /\bopen_av1_encoder\b/, files: [`${CODECS}av1/svt.rs`, `${CODECS}av1/backends.rs`] },
  { name: "the audio encoder token (use open_audio_encoder)", re: /\bAudioOpenToken\s*\(\s*\(\s*\)\s*\)/, files: [`${CODECS}audio/registry.rs`] },
];
/** The policy-filtered openers: video (6B-1) and audio (6C). */
export const OPENERS = ["open_encoder", "open_audio_encoder"];
const PURPOSE = /\bEncodePurpose\s*::\s*\w+/;
const DEFAULT = /#\[derive\([^)]*\bDefault\b[^)]*\)\]\s*(?:#\[[^\]]*\]\s*)*pub\s+struct\s+EncoderConfig\b|\bimpl\s+Default\s+for\s+EncoderConfig\b/;

/** Whether `path` (repo-relative, "/") is scanned. */
export function scanned(path) {
  return path.endsWith(".rs") && /^(crates|apps)\//.test(path) && !path.startsWith("crates/fin-svt-sys/") && !path.includes("/target/");
}

/** The findings in one file's text. */
export function findings(path, text) {
  const out = [];
  const body = code(text);
  const lines = body.split("\n");
  const lineOf = (i) => body.slice(0, i).split("\n").length;
  lines.forEach((line, n) => {
    for (const p of PRIMITIVES) if (p.re.test(line) && !p.files.includes(path)) out.push(`ENCODER_CALL_SITE ${path}:${n + 1}: ${p.name} outside ${p.files.join(", ")}`);
  });
  const noUse = body.replace(/\buse\b[^;]*;/g, (u) => u.replace(/[^\n]/g, " "));
  for (const name of OPENERS) {
    const call = body.search(new RegExp(`(?<!fn\\s+)\\b${name}\\s*\\(`));
    if (call !== -1 && !PURPOSE.test(body)) out.push(`ENCODER_CALL_SITE ${path}:${lineOf(call)}: ${name} without an EncodePurpose (name the config's purpose in this file)`);
    for (const [re, what, text] of [
      [new RegExp(`\\b${name}\\s+as\\b`), `${name} renamed (call it by its name)`, body],
      // Neither called nor defined (over the text without `use` items): taken as a value.
      [new RegExp(`(?<!fn\\s+)\\b${name}\\b(?!\\s*\\()`), `${name} taken as a value (call it directly)`, noUse],
    ]) {
      const at = text.search(re);
      if (at !== -1) out.push(`ENCODER_CALL_SITE ${path}:${lineOf(at)}: ${what}`);
    }
  }
  const at = body.search(new RegExp(DEFAULT.source));
  if (at !== -1) out.push(`ENCODER_CALL_SITE ${path}:${lineOf(at)}: EncoderConfig with a Default (its purpose must be written out)`);
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  const violations = [];
  for (const path of walk(root)) if (scanned(path)) violations.push(...findings(path, readFileSync(join(root, path), "utf8")));
  report(violations);
}
