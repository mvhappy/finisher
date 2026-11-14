# Sub-project 6C: Audio — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the resampler and WSOLA, the mixer's sample-exact timing, filter and dynamics maths, loudness and the limiter, determinism across native and wasm32, the plan contract and the model bump, codec sessions and their failure modes, the cache recipes), `sonnet` for plumbing (gates, generators, kinds, RPCs, CLI, e2e, CI, docs). Tasks 15 and 16 need a macOS and a Windows 11 host; Task 23 is controller-run.

**Goal:** Finisher hears what the plan says. After 6C:
- a new pure crate **`fin-audio`** (native and `wasm32-unknown-unknown`) holds the mixer, the scalar polyphase resampler, the WSOLA time-stretch, the four built-in audio effects (`eq`, `compressor`, `gate`, `deEsser`), loudness measurement (BS.1770-4 through `ebur128`), the true-peak limiter with two-pass normalisation, and waveform peaks; its hashed output is **bit-identical native and on wasm32** (tier A0);
- **the plan grows** (spec §4.3 last row): the effect registry gains the four audio kinds, `effect.*`/`param.set`/`keyframe.*` edit them on audio-track clips, and `AudioItem.effects` carries each enabled effect with its numbers as piecewise segments over the item's span; the model version bumps once;
- **audio decodes** in the media worker: WAV, AIFF, FLAC, Ogg (Vorbis, Opus), MP3, ALAC, CAF and PCM through symphonia (MPL-2.0, unmodified); AAC-LC on Linux through symphonia (owner decision 1); Opus through a pinned libopus; AAC through AudioToolbox on macOS and Media Foundation on Windows; HE-AAC is `CODEC_UNAVAILABLE` with the codec plugin as the fix where the OS cannot decode it;
- **audio encodes** for the batches that need it: Opus (libopus) for proxy audio now, AAC-LC through vo-aacenc on Linux and the OS encoders elsewhere for 6E's export;
- **PCM feeds** are content-addressed cache entries: `pcm48` (48 kHz `f32` stereo, the mixer's input) and `pcm16m` (16 kHz `f32` mono, sub-project 8's analysis feed), plus `wave` peaks, made by `media.pcm` and `media.waveform` jobs; `pcm16m` is a library stage;
- proxies gain Opus audio (**proxy recipe v2**, the follow-up 6B-1 Decision 4 assigned to 6C);
- `fin listen` prints a range's loudness, true peak, clipping, silences and loudest moments, per track and for the mix (spec §8.7), from a `media.listen` job.

**Spec:** `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` (cited as *spec §n*): §1, §3.2–3.6, §4.3 (audio effects row), §4.4 (registry row), §6.1–6.4 (audio rows), §7.1 (proxy audio), §7.2 (recipes `pcm48`, `pcm16m`, `wave`), §8 (all), §9.1 item 3 and §9.3 (what 6E will call), §13.1–13.4 (A0, A1, audio fixtures, float rules), §14 (mixer and worklet rows), §15, §16, §17, §18 (`fin-audio` row), §19 row 6C, §21 decisions 1 and 10, §25 (6A as built). Plugins (*pl §n*): §5 (`media.pcm` host call, 8C's). Core (*core §n*): §5.2 (registry), §5.4 (eval). Parent: arch §12.7, §17.

**Builds on, as built** (integration branch `sp4c-edit-breadth`, read 2026-11-13):
- `fin-core`: `eval::eval_audio` → `AudioPlan {timeline, from, to, items: Vec<AudioItem>}`, `AudioItem {item, track, via, asset, from, to, sourceFrom, speed, gain: Vec<GainSegment>, pan: Vec<PanSegment>}`, `GainSegment {from, to, db, interp, u, shapes: Vec<Shape>}`, `Shape {kind: ShapeKind, curve: Curve, p}` (`crates/fin-core/src/eval/audio.rs`, 393 lines); piecewise curves `NumberCurve`/`Piece` (`eval/segments.rs`, `pub(crate)`); `bezier_y` (public, `eval/interp.rs`); the effect registry `params/effects.rs` (`KINDS = ["colorAdjust", "lut", "blur"]`, `Scope::Effect {kind}`, `effect_kinds()`); `effect.add` refuses every registry effect on an audio-track item (`check_visual_track`, `TRACK_KIND_MISMATCH`, `ops/effect_add.rs:101`); `SCHEMA_VERSION = 4` (`model/mod.rs:38`) with frozen identity steps `upgrade/{snapshot,record}_v3.rs`; store `DB_VERSION = 6` (`fin-store/src/migrations/mod.rs:38`).
- `fin-media`: `AudioCodec {Aac, Opus, Mp3, Flac, Alac, Pcm, Ac3, Eac3, Other}` (`format.rs:70`), `AudioTrackInfo {codec, fourcc, sampleRate, channels, priming, padding}` (`info.rs:199`), `backend.rs` (`VendorClass`, `EncodePurpose`, `encoder_allowed` over `VideoCodec` only, `BACKENDS` for video), `jobs/caps.rs` (`AudioCodecCaps`: "none decode or encode before 6C"), `RENDER_VERSION = 1`.
- `fin-container`: `Demuxer::audio() -> Option<&AudioTrack {track, timescale, config: CodecConfig, samples}>` (first audio track only), `read_sample`, `CodecConfig::{Aac {esds}, Opus {dops}}`, AAC AudioSpecificConfig parsing (`demux/aac.rs`), the MP4 muxer's `AudioTrackConfig {config, sampleRate, channels, priming}` (priming written as the edit list's media time).
- `fin-codecs`: video traits only (`traits.rs`), `registry.rs` (`open_decoder`/`open_encoder` under the policy), `hung.rs`; `scripts/check-encoder-call-sites.mjs` (6B-1 Task 1) lists the files allowed to open encoders.
- `fin-probe`: `PROBE_VERSION = 1`; sniffs MP4/MOV/M4A and WAV only (`lib.rs:154`, `wav.rs`).
- The `media` family (`crates/fin-workers/src/media/`, kinds `media.look` v1, `media.capabilities` v1), `fin-shm` (`JobDir`, `create_output`, `commit_output`), `fin-library`'s no-follow opener, 5C's `JobEngine` and kinds (`apps/finisherd/src/jobs/kinds/`), 5D's stages (`LibraryService.registerStage`, `apps/finisherd/src/library/service.ts:565`).
- Gates: `scripts/check-eval-floats.mjs` (render rules: `libm::` required, SIMD refused), `scripts/check-crate-deps.mjs` (`MEDIA` already lists `fin-audio`), `scripts/check-wasm-deps.mjs`, `scripts/check-native-deps.mjs` + `third_party/native.json`, `scripts/check-notices.mjs`, `libm = "=0.2.16"` pinned in the workspace.

**Builds on 6B-1, not yet on integration** (branch `home/sp6b1`, Tasks 1–3 merged, 9–13 done there, 14–22 open, read 2026-11-13): `fin-media::cache` (`Recipe {name, version}`, `RECIPES`, `KeyInput {fingerprint, recipe, recipeVersion, params, renderVersion, decoderClass}`, `CacheKey::compute`, `EntryMeta`), `fin-node::media::cache` (`MediaCache` napi: `lookup`, `commit`, `pin`, `entriesFor`, …) and `apps/finisherd/src/media/cache.ts` (Task 13); `media.proxy` and `encode_pipeline.rs` (Task 14, not built yet); `packages/contracts/src/media-rpcs.ts` (`MEDIA_RPCS`, spread into `rpc.ts`); model v5 (`m0007_video_display`).

**Architecture (6C slice):**
- **`crates/fin-audio`** (new, pure, `wasm32-unknown-unknown`; every file ≤ 600 lines): `lib.rs` (`RATE = 48_000`, `AUDIO_VERSION`), `buf.rs` (planar `Stereo`, block sizes), `channels.rs` (upmix, BS.775 downmix, mono fold), `time.rs` (rational time → sample index), `resample/{mod,kernel}.rs`, `stretch.rs` (WSOLA), `mix/{mod,item,curves,pan}.rs`, `effects/{mod,params,biquad,eq,detector,compressor,gate,deesser}.rs`, `loudness/{mod,meter,limiter,normalise}.rs`, `peaks.rs`, `listen.rs` (the `fin listen` summary over measured blocks), `testing/{mod,signals,analyse}.rs` (feature `testing`).
- **`fin-core`:** `params/audio_effects.rs` (the four kinds' specs), `params/effects.rs` (an `EffectDomain` per kind), `ops/effect_add.rs` (domain check), `eval/audio_effects.rs` (`AudioEffectPlan`, `ParamSegment`), `eval/audio.rs` (call site and the nested shift), the model bump.
- **`fin-codecs`:** `audio/{mod,traits,registry,symphonia,mp4,opus,aac_linux,reader}.rs`, `audio/os/{toolbox,mf}.rs` (target-gated); new sys crates `crates/fin-opus-sys` (libopus 1.6.1) and `crates/fin-aacenc-sys` (vo-aacenc, Linux only), each a featureless `cc` build behind a C shim, like `fin-dav1d-sys`.
- **`fin-media`:** `backend_audio.rs` (audio backends, `audio_encoder_allowed`), `jobs/{pcm,wave,listen}.rs`, recipes `PCM48`, `PCM16M`, `WAVE` and `PROXY_*` v2 in `cache.rs`, `AudioCodecCaps` rows filled.
- **`fin-workers`** (`media` family): `media/{pcm.rs, wave.rs, listen.rs, audio_src.rs}`; proxy audio in 6B-1's `media/proxy.rs`.
- **`fin-probe`:** `audio.rs` (AIFF, FLAC, Ogg, MP3, CAF through symphonia's format readers).
- **`finisherd`:** `jobs/kinds/media.{pcm,waveform,listen}.ts`, `media/{pcm.ts, listen.ts}`; **`@finisher/contracts`:** `audio.ts` (payloads), rows in `media-rpcs.ts`; **`fin`:** `commands/listen.rs`, `media pcm`, `media wave`.
- **Not in 6C:** export, templates, `media.render`, two-pass loudness *in an export* and the A/V offset check (6E calls 6C's APIs); the AudioWorklet mixer and `fin-wasm` (6F; 6C only proves the crate builds and hashes the same on wasm32); ducking (`compose::duck`, sub-project 8); offline processors such as de-noise (plugins); the `media.pcm` **host call** for analyzers (8C, which serves 6C's `pcm16m` entries); platform video codecs and proxies' video (6B-1); `media.frames` (6B-2).

**Tech stack** (licences checked on crates.io and the upstream repositories, 2026-11-13; every crate is ≥ 7 days old today):

| Dependency | Version | Licence | Published | Notes |
|---|---|---|---|---|
| `symphonia` | 0.6.1 | MPL-2.0 | 2026-08-13 | `default-features = false`; features `wav, aiff, flac, ogg, vorbis, mp3, alac, caf, pcm`; `aac` only under `cfg(target_os = "linux")` (Decision 9). Never `opt-simd*`: those are the only path to `rustfft` (`symphonia-core` 0.6.1 features: `opt-simd-sse = ["rustfft/sse"]`, …), so without them no FFT crate and no SIMD enters the tree |
| its subcrates | 0.6.1 each | MPL-2.0 | 2026-08-13 | `symphonia-core`, `-common`, `-metadata`, `-format-{riff,ogg,caf}`, `-codec-{vorbis,alac,pcm,aac}`, `-bundle-{flac,mp3}` |
| their new transitive crates | `extended` 0.1.0 (MIT), `regex-lite` 0.1.9 (MIT OR Apache-2.0) | — | 2022-05-27, 2026-02-03 | `bitflags` 2, `bytemuck`, `lazy_static`, `log`, `num-complex` 0.4, `smallvec` 1 are already in `Cargo.lock` |
| `ebur128` | 0.1.11 | MIT | 2026-10-26 | default features only (`c-tests`, which pulls `cc`, is off); new transitive `dasp_frame` 0.11.0, `dasp_sample` 0.11.0 (MIT OR Apache-2.0); `bitflags` 1.3 and `smallvec` 1 already in the tree |
| `objc2-audio-toolbox` (+ `objc2-core-audio-types`) | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 2025-10-04 | macOS only, the 0.3 line the tree's `objc2-*` already use |
| `windows` | 0.62 (in the tree) | MIT OR Apache-2.0 | — | `Win32_Media_MediaFoundation` (added by 6B-1 Task 1) |
| libopus | v1.6.1, commit `22244de5a79bd1d6d623c32e72bf1954b56235be` | BSD-3-Clause (`COPYING`) + the royalty-free IETF patent grants it lists | tag 2026-01-13 | submodule `third_party/opus`; `dnn/` and the deep-PLC/DRED/OSCE code are not compiled (Decision 11) |
| vo-aacenc | commit `a277487e051e92e99a532294eed3c673f4d879f2` (upstream `mstorsjo/vo-aacenc`, last commit 2017-06-28) | Apache-2.0 (patent grant covers VisualOn's contributions only; AAC pool: counsel item 6.1) | — | submodule `third_party/vo-aacenc`, Finisher's fork per spec §6.4 (Owner question 6); Linux only |

Rejected here: `rubato` and anything on `rustfft` (spec §8.1, SIMD); Rubber Band and SoundTouch (GPL/LGPL, spec §8.2); `fdk-aac` (no patent grant, spec §16.2); `audiopus`/`opus-sys` (unmaintained bindings; own FFI as for dav1d); symphonia's `isomp4` and `mkv` readers (MP4 is `fin-container`'s, WebM is 6B-1's `fin-container` demuxer). Node 24.22, Effect 4, vitest as the repository.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code; `pnpm gates` runs cargo-deny, the native build gate (libopus and vo-aacenc get `third_party/native.json` entries with their licence-file BLAKE3), the notices gate (symphonia's MPL-2.0 §3.2 statement with each 0.6.1 crate's name and source link, spec §16.1) and the run-time library gate (AudioToolbox and Media Foundation's AAC MFTs as `os`; nothing is loaded by name). No FFmpeg, libav\*, GStreamer, fdk-aac, Rubber Band or SoundTouch anywhere.
- **Patents (spec §16.4):** Finisher's own AAC code (symphonia's AAC-LC decoder, vo-aacenc) is compiled **only for Linux targets** (Decision 9); macOS and Windows use the OS codec. Counsel item 6.1 stays open; nothing here decides it.
- **Dependency rules** (`scripts/check-crate-deps.mjs`): `fin-audio` is linked only by `fin-workers` (and `fin-wasm` in 6F), **never by `fin-codecs`**: codec APIs hand out decoded audio at its native rate with an explicit channel layout, and the worker converts it with `fin-audio` (`crates/fin-workers/src/media/audio_src.rs`); the gate's `ONLY` check is transitive, so this is enforced; `fin-audio` links no Finisher crate but `fin-core` and `fin-media`; `fin-opus-sys` and `fin-aacenc-sys` only by `fin-codecs`/`fin-workers`; `fin-node` may link `fin-probe`'s symphonia readers (Decision 14) but never `fin-codecs` or `fin-audio`; `objc2-audio-toolbox` only under `cfg(target_os = "macos")`, `fin-aacenc-sys` and symphonia's `aac` feature only under `cfg(target_os = "linux")`.
- **`unsafe`** only in `fin-opus-sys`, `fin-aacenc-sys`, `fin-codecs/src/audio/opus.rs`, `audio/aac_linux.rs` and `audio/os/*.rs`, each block with `// SAFETY:`; `fin-audio` is `#![forbid(unsafe_code)]`.
- **Workers never start processes;** every new kind runs in `fin-workers media`. New kinds that read untrusted media are **cold** (spec decision 20: no new warm untrusted-input kind before 7C Task 12).
- **Primitive ops live only in `fin-core`,** one module per op: 6C adds no op kind; it changes `effect.add`'s validation in `ops/effect_add.rs`.
- **TypeScript model types are generated:** `AudioEffectPlan`, `ParamSegment`, `EffectDomain`, the job params and results (`PcmParams`, `PcmResult`, `WaveParams`, `WaveResult`, `ListenParams`, `ListenResult`), `AudioBackendEntry` and the filled `AudioCodecCaps` derive `JsonSchema`, go into `crates/fin-store/examples/export_schema.rs`, and are regenerated with `rtk pnpm --filter @finisher/core-types generate`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit today, so 6C adds call-site lines only or splits first: sizes after 6B-1 (`home/sp6b1`, 2026-11-13): `crates/fin/src/output_codes.rs` 573, `packages/contracts/src/rpc.ts` 599 (6C adds **nothing** there; its RPCs go into `media-rpcs.ts`), `packages/contracts/src/scopes.ts` 574, `apps/finisherd/src/main.ts` 591, `apps/finisherd/src/projects.ts` 595. **Every task re-counts a file with `wc -l` before editing it and splits it first (a new sibling module holding the moved code, no behaviour change, its own commit) if its change would pass 600**; `projects.ts` and `main.ts` have room for a handful of call-site lines only, so Tasks 18 and 21 put their hooks in `apps/finisherd/src/media/pcm.ts` and register them through one line each, or split those files first; `crates/fin-core/src/eval/audio.rs` (393: effects live in `eval/audio_effects.rs`).
- Errors are `{code, message, details, fix?}`; 6C adds no new error code: decode and encode failures are spec §17's `DECODE_FAILED`, `ENCODE_FAILED`, `CODEC_UNAVAILABLE`, `MEDIA_CORRUPT`, `MEDIA_LIMIT`; `LOUDNESS_TARGET_UNREACHABLE` is a **warning** in results (no exit code). If `CODEC_UNAVAILABLE` gains a job-level exit in `fin`, it goes into `output_codes.rs` and `console/exit-codes.ts` with the parity test.

**Determinism** (spec §8.2, §13.4)
- `fin-audio`'s mixer, curves, pan, effects, resampler, WSOLA, limiter and peaks are **scalar `f32`** (coefficient design in `f64`, cast once) **in a fixed order, `libm` for every transcendental (exception, owner-approved at Task 23: the dynamics detector's `log2`/`exp2` are fixed-order `+ − ×` polynomials, `effects/detector.rs`, for spec §14 throughput), no `mul_add`, no SIMD, no `f32::powf`/`.sin()`**. `scripts/check-eval-floats.mjs` gains a third pattern for `crates/fin-audio/src/**` minus `loudness/meter.rs` and `testing/**` (render rules: `libm::` required, `std::arch` refused).
- **What is gated bit-exact (A0):** given the same PCM input, the mixer with every effect, the resampler, WSOLA, the limiter and the peaks produce the same bits on macOS arm64, Windows x64, Linux x64 and `wasm32-wasip1`. **Not gated:** decoders (no audio decoder in the tree is normative across platforms, Decision 8) and `ebur128`'s measurements (reported, never hashed, spec §8.6); a normalisation gain derived from them is **quantised to 0.01 dB** before it is applied, so it is stable in practice.
- **`AUDIO_VERSION`** (`fin-audio`, starts at 1) is to audio what `RENDER_VERSION` is to frames: a change that moves an A0 hash bumps it in the same commit, and a test fails when A0 hashes change without a bump. Audio cache entries carry it in their key params (Decision 7).

**Testing and process**
- **No media files are committed.** Every fixture is synthetic, generated in code at test time (`fin-audio::testing` signals; test-only WAV, AIFF, CAF-`lpcm`, FLAC-verbatim and Ogg writers; Opus and AAC fixtures encoded at test time by our own encoders or the OS's). MP3, Vorbis and ALAC decode rows, which no encoder in the tree can make, read the local fixture cache `FIN_MEDIA_FIXTURES` (permissively licensed or owner-made clips only, AGENTS.md; no NC or share-alike corpora) and skip with a notice; `FIN_REQUIRE_MEDIA=1` fails them instead. Loudness conformance uses EBU Tech 3341/3342-*style* signals generated from their published parameters, not EBU's files.
- **Numeric tolerances are stated in each test** (dB, LU, samples, Hz); A0 hashes are BLAKE3 over the little-endian `f32` bytes.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`; tasks touching `fin-audio` **from Task 11 on** also run `rtk cargo test -p fin-audio --features testing --target wasm32-wasip1 --test a0` (with `CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime --dir ."`; before Task 11 they run `rtk cargo build -p fin-audio --target wasm32-unknown-unknown`); **tasks that add or update a dependency run `rtk pnpm check:dep-age` after `Cargo.lock` is resolved** (AGENTS.md: every crate, transitive ones included, published ≥ 7 days before it is locked), and again before the batch merges; tasks touching TS, RPCs or `fin` also run `rtk pnpm e2e`. Platform tasks (15, 16) run `rtk cargo test -p fin-codecs --features testing` on their OS before the commit; Linux CI stays green (the code is target-gated).
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Shell commands are prefixed with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files; measurements go to `archive/sp6/6c-*.md`.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 6C" (Task 1 adds the section under "Sub-project 6: media engine", replacing the one-line `- [ ] 6C: audio (incl. built-in audio effects, PCM feeds)`).

**Batches and branches**

| Batch | Tasks | Needs | Branch | Can run beside |
|---|---|---|---|---|
| **A** Foundations and DSP core | 1–4 | integration (now) | `sp6c-a` | B, 6B-1 |
| **B** The plan grows | 5–6 | integration (now) | `sp6c-b` | A, 6B-1 |
| **C** Mixer, effects, loudness, A0 | 7–11 | A, B | `sp6c-c` | D |
| **D** Decode, encode, probe | 12–14 (Linux; 13 also adds the gapless muxer contract); 15 (macOS host, after 6B-1 Tasks 4–5); 16 (Windows host, after 6B-1 Tasks 6–7); 17 (after 12–14 and **6B-1 Task 8**, which creates `probe/guidance.rs`) | A (Tasks 2–3: channels, layouts, resampler; no Batch C dependency: codec tests measure RMS, not loudness) | `sp6c-d`, `sp6c-d-mac`, `sp6c-d-win` | C |
| **E** PCM feeds, peaks, proxy audio, `fin listen` | 18–21 | C, D (12–14), **6B-1 Task 13 (cache) merged**; Task 20 also **6B-1 Task 14 (`media.proxy`)** | `sp6c-e` | — |
| **F** Diagnostics, e2e, validation, docs | 22–24 | all | `sp6c-f` | — |

Each batch merges into `sp6c-audio` (from integration) green on Linux; the batch's macOS and Windows rows run before the next batch starts (owner decision 8); `sp6c-audio` merges into integration after Batch F. Batches A and B touch disjoint crates and run in parallel; C and D likewise.

## Decisions taken here

1. **Scope and the 6B-1 boundary.** 6C owns every audio codec and audio byte path: audio traits and backends in `fin-codecs/src/audio/` (a new directory 6B-1 never touches), `fin-opus-sys`, `fin-aacenc-sys`, the audio backend table and audio encoder policy in `fin-media/src/backend_audio.rs` (beside, not inside, 6B-1's `backend.rs`), the `pcm48`/`pcm16m`/`wave` recipes, the proxy's audio track. 6B-1 owns video decode/encode (VideoToolbox, Media Foundation video, `apple/`, `mf/`), the capability probe and its guidance table, the cache (index, keys, eviction, HTTP), proxies' video, thumbnails, deep probe, WebM demux and the Landlock profile. **Merge points**, each named in its task: `fin-media/src/cache.rs` (6C adds recipe constants to `RECIPES`), `fin-media/src/jobs/{mod,caps,manifest}.rs` (6C adds modules, audio rows and the manifest's `pcm` field), `fin-workers/src/media/mod.rs` (`KINDS`), `fin-workers/src/media/proxy.rs` (Task 20 adds the audio track after 6B-1 Task 14), `fin-codecs/src/probe/guidance.rs` (Task 17 replaces 6B-1's "arrives with the audio batch" line), `fin-codecs/src/mf/com.rs` (Task 16 reuses its COM/`MFStartup` guard, made `pub(crate)` by a one-line change), `fin-media/src/format.rs` (6B-1 adds `ContainerKind::{Webm, Matroska}`, 6C adds `AudioCodec::Vorbis`), `fin-container/src/mux/{mod,mp4}.rs` (Decision 22; untouched by 6B-1 on `home/sp6b1`), `fin-core/src/events/{names_media.rs, payloads/media.rs}` (Decision 24), `scripts/check-encoder-call-sites.mjs` (6C adds its encoder files), `packages/contracts/src/media-rpcs.ts`, `TASKS.md`. 6C never edits `fin-codecs/src/{registry,hung,traits}.rs` or `fin-media/src/backend.rs`.
2. **The spec's dependency row is too short (spec vs code).** Spec §19 says 6C needs "6A-2 (6B-1 for OS AAC)". In the code, `pcm48`/`pcm16m`/`wave` are cache recipes and the cache (`fin-media::cache`, `fin-node::media::cache`) is 6B-1 Task 13, not on integration yet; proxy audio needs 6B-1 Task 14. So Batch E waits for those; Batches A–D do not. OS AAC on macOS and Windows reuses 6B-1's session plumbing (Batches B and C of 6B-1).
3. **Internal format, channel layouts and mono identity** (spec §8.1, §8.2, arch §12.7): 48 kHz, `f32`, planar inside `fin-audio`, interleaved little-endian on disk.
   - **Explicit layouts.** `fin_media::audio_layout::{ChannelPos, ChannelLayout}` (new, shared by `fin-codecs` and `fin-audio`, which both already link `fin-media`): positions `L, R, C, Lfe, Ls, Rs, Lb, Rb, Cb, Lw, Rw, Unknown`. **Every decoder backend returns its decoded frames with the layout in its own channel order, and normalises nothing itself**: symphonia's `Channels` bitmask; WAV `WAVEFORMATEXTENSIBLE` `dwChannelMask` (absent: the WAVE default order for the count); Opus mapping family 0 (mono/stereo) and family 1 in **Vorbis order** (RFC 7845 §5.1.1.2: 3 = L C R, 5 = L C R Ls Rs, 6 = L C R Ls Rs LFE, 8 = L C R Ls Rs Lb Rb LFE); AAC `channelConfiguration` per ISO/IEC 14496-3 Table 1.19 (3 = C L R, 6 = C L R Ls Rs LFE, 7 = C L R Ls Rs Lb Rb LFE); AudioToolbox's `AudioChannelLayout` and MF's `MF_MT_AUDIO_CHANNEL_MASK`. `AudioTrackInfo` (the wire type) still records only a count (spec §6.3 says "channels and layout": **spec vs code**; the layout is a run-time value of the decoder, not a model or `MediaInfo` field).
   - **Canonical mixer sources** are `Mono` or `Stereo` (`fin_audio::channels::Canonical`): a 1-channel source stays **mono** (it is *not* upmixed at decode); a 2-channel source is stereo (also when L = R: "dual mono" is stereo); 3–8 channels are mapped by position (never by index) and downmixed to stereo with BS.775 coefficients (C and Ls/Rs at −3 dB, Lb/Rb folded into Ls/Rs, LFE dropped); a layout with an `Unknown` position or more than 8 channels is `CODEC_UNAVAILABLE {reason: channels}`.
   - **Mono's −3 dB upmix (spec §8.1) and mono's constant-power pan (§8.2) are one operation**, applied once in the mixer: a mono source at pan `p` gives `L = x·sqrt((1 − p)/2)`, `R = x·sqrt((1 + p)/2)`, so at the centre each side is `x × 0.70710677` (the §8.1 upmix) and nothing attenuates it again; a stereo source takes balance (Decision 16). So `pcm48` keeps a mono source as **one channel** (Decision 7) and `PcmSource::layout()` tells the mixer which law to use (`AudioItem` has no channel field and needs none: the source carries it).
   - The 16 kHz analysis feed is the canonical form folded to mono: mono as is, stereo as `0.5·(L + R)`.
4. **Sample-exact time.** A rational time `t` maps to sample `round_half_up(t × 48000)` computed exactly on `fin_core::time::Rational` (`fin-audio::time::sample_at`); every segment, shape, effect-parameter and chunk boundary is computed from its own rational time, never by adding lengths (spec §9.1 item 3: ≤ ½ sample from exact, no drift). An item's source position at output sample `n` is `sourceFrom + (n − sample_at(from))/48000 × speed`.
5. **The resampler** (spec §8.1): own scalar polyphase windowed-sinc. Rate ratio reduced by gcd to `L/M`. The prototype is specified in units of `fs_min = min(fin, fout)`: **passband edge `0.4535·fs_min`** (20.0 kHz at 44.1 kHz, 7.256 kHz at 16 kHz), **stopband edge `0.5·fs_min`**, `−6 dB` cutoff `fc = 0.4768·fs_min` (the band's midpoint), **Kaiser window β = 10.06** (the Kaiser formula for 100 dB), half-length **70 samples at `fs_min`**, so **140 taps a phase when upsampling** and `140·fin/fout` input taps when downsampling (420 for 48 → 16 kHz). Computed for this plan from the continuous prototype in `f64` (script in `archive/sp6/6c-resampler.py`): passband deviation ≤ 3.1·10⁻⁵ dB up to 0.4535·fs_min, −0.05 dB at 0.46, −6.02 dB at 0.4768, −30.7 dB at 0.49, and **≥ 102.0 dB rejection from 0.5·fs_min to fs_min** (worst at the edge region; ≥ 105 dB beyond 0.52). The tests (Task 3) assert these with margins for `f32` coefficients and arithmetic: ±0.01 dB passband, ≤ −95 dB stopband. Bessel I₀ by its power series in `f64`; coefficients designed once per ratio in `f64` with `libm`, normalised to unit DC gain per phase, cast to `f32`; the inner loop is a fixed-order `f32` dot product. Spec §8.1 fixes no numbers; the earlier draft's 64-tap, β 8.6, 0.94-Nyquist design reached only ~21 dB at the stopband edge and is not used. Ratios with `L > 1024` (non-standard rates) quantise the phase to 1/1024 and interpolate linearly between adjacent phase tables (still deterministic). The filter's group delay is removed, so sample 0 of the output is source time 0, and the output length is exactly `ceil(n_in × fout / fin)`.
6. **WSOLA** (spec §8.2): mono analysis on the channel sum, stereo synthesis with one offset for both channels (no phasiness between channels); frame 1,024 samples at 48 kHz (21.3 ms), Hann window, 50 % overlap, search ±512 samples by normalised cross-correlation (scalar `f32`, fixed order, ties to the smaller offset). `speed = 1` bypasses it bit-exactly; speeds the model allows but WSOLA cannot do well (below 1/4 or above 4) still produce output of exactly the right length, and `fin listen` warns `STRETCH_EXTREME {item, speed}` in its notes (no error).
7. **Cache recipes** (spec §7.2; 6B-1's `fin-media::cache`): `pcm48` v1 (`chunk-NNNNNN.f32`, interleaved LE at 48 kHz in the source's canonical layout of Decision 3: **1 channel for a mono source, else 2**; 1 MiB a chunk, i.e. 131,072 stereo frames or 262,144 mono ones, the last one short; `meta.json`'s `params {audioVersion, rate: 48000, layout: mono|stereo, frames}`; spec §7.2 says "stereo": **spec vs code**, because a mono source must keep its pan law), `pcm16m` v1 (same, mono at 16 kHz, 262,144 samples a chunk), `wave` v1 (`peaks.bin`, below). `KeyInput.params` carries `audioVersion`, so an `AUDIO_VERSION` bump invalidates audio entries without touching video ones; `renderVersion` stays in the key as 6B-1 defined it (**spec vs code**: spec §7.2 names only `RENDER_VERSION`). `decoderClass` is the vendor class of the backend that decoded the source (Decision 8).
8. **Decoders are not normative across platforms.** symphonia's AAC/MP3/Vorbis use platform float maths and libopus is a float build; AudioToolbox and MF differ from both. So `pcm48` of the same file may differ between machines; caches are local, the key names the decoder class, and A0 covers everything after decode. Spec §8.1's "bit-identical on every OS" holds for the resampler and below given the same decoded input (**spec vs code**, recorded in §8.1 by Task 24). PCM and FLAC decode are bit-exact everywhere and a test pins it.
9. **AAC is OS-first, and Finisher's AAC code is Linux-only** (owner decision 1 as the spec states it, spec §6.2 "AAC-LC: symphonia" in the Linux column, §9.2 "vo-aacenc on Linux"). macOS: AudioToolbox decode (LC and HE-AAC) and encode (LC); Windows: MF's AAC decoder and encoder MFTs (absent on N editions without the Media Feature Pack → `CODEC_UNAVAILABLE` with that fix); Linux: symphonia's AAC-LC decoder (backend name `symphonia`) and vo-aacenc. HE-AAC on Linux is `CODEC_UNAVAILABLE {profile: he-aac}` with `fix: codecPlugin` (symphonia refuses it, spec §21 item 1). Neither `symphonia-codec-aac` nor vo-aacenc is compiled into macOS or Windows binaries (target-gated features, checked by `check-crate-deps`).
10. **Audio encoder policy.** `fin_media::backend_audio::audio_encoder_allowed(class, AudioCodec, purpose)` is the one function: `Export | QaRender | Matte` allow `Os | Software | Plugin`, `Proxy` allows every class (there is no `gpu` audio encoder today; the rule mirrors decision 9 so one appearing later is refused for exports). `fin-codecs::audio::registry::open_audio_encoder` filters through it, and `check-encoder-call-sites.mjs` gains `audio/registry.rs` as the only opener. 6C opens audio encoders for `Proxy` (Opus) and in tests; `Export` is 6E's.
11. **libopus build.** `fin-opus-sys` compiles libopus 1.6.1's `celt_sources.mk`, `silk_sources.mk` + `SILK_SOURCES_FLOAT` and `opus_sources.mk` + `OPUS_SOURCES_FLOAT` with the `cc` crate (float build, `-ffp-contract=off`, no `dnn/`, no `lpcnet_sources.mk`, `ENABLE_DEEP_PLC`, `ENABLE_DRED`, `ENABLE_OSCE` undefined; x86 SSE/AVX and ARM NEON intrinsics files **not** compiled, run-time CPU detection off), so the build needs no model download, no Python and no assembler; `build.rs` refuses any other libopus version until its source lists are re-checked (the dav1d rule). A C shim owns `OpusEncoder`/`OpusDecoder` so Rust mirrors no struct.
12. **vo-aacenc build.** `fin-aacenc-sys` (Linux only) compiles the library's `common/` and `aacenc/src/` C files with `cc` (no autotools), C-only paths (its ARMv5/ARMv7 assembly not built), behind a C shim; fed only mixer PCM (48 kHz stereo), so its input is never untrusted. AAC-LC, 48 kHz, 2 channels, CBR at the requested bitrate (6E's templates name 192 kb/s); output wrapped as an MP4 `esds` with the AudioSpecificConfig `fin-container`'s muxer already writes. Encoder priming (1,024 samples for AAC-LC) is reported for the edit list.
13. **The audio effect registry** (spec §8.4): kinds `eq`, `compressor`, `gate`, `deEsser`, each with `enabled`. The registry's one-level rule for effect params (`Resolver::names`, review batch J #51) forbids dotted names, so EQ bands are flat: `band1Type` … `band8Type` (enum `off|lowShelf|highShelf|peak|lowPass|highPass`, default `off`, not animatable), `band<n>Freq` (20–20,000 Hz, defaults 80, 160, 315, 630, 1250, 2500, 5000, 10000), `band<n>Gain` (−24…24 dB, 0), `band<n>Q` (0.1–18, 0.707). `compressor`: `detector` (`rms|peak`, `rms`), `threshold` (−60…0 dB, −18), `ratio` (1–20, 4), `knee` (0–24 dB, 6), `attack` (0.1–200 ms, 10), `release` (5–2,000 ms, 100), `makeup` (0–24 dB, 0). `gate`: `threshold` (−90…0 dB, −50), `range` (−90…0 dB, −80), `attack` (0.1–50 ms, 1), `hold` (0–500 ms, 50), `release` (5–2,000 ms, 100). `deEsser`: `frequency` (2,000–12,000 Hz, 6,500), `q` (0.5–8, 2), `threshold` (−60…0 dB, −30), `ratio` (1–20, 6), `range` (0–24 dB max reduction, 12). Numbers are animatable, enums not. **Spec vs code:** the spec says `scope: audio`; the code's `Scope` has one `Effect {kind}` variant, so 6C adds `EffectDomain {visual, audio}` per kind (`params::effect_domain(kind)`, shown by `fin schema param`) instead of a new scope. `effect.add` refuses a visual kind on an audio-track item (as today) **and an audio kind on any other track** (`TRACK_KIND_MISMATCH`, policy, `fix: add audio effects to clips on audio tracks`); the `fin` command and schema list both domains.
14. **Probing audio containers** (spec §6.2 rows WAV…CAF; `fin-probe` today sniffs MP4/MOV/M4A and WAV only, **spec vs code**): `fin-probe` sniffs `FORM…AIFF/AIFC`, `fLaC`, `OggS`, `ID3`/MPEG frame sync, `caff` by magic and reads sample rate, channels and duration with symphonia's format readers only (no codec features in `fin-probe`), bounded by the file length and inside `fin-probe`'s existing panic guard; an MP3 without a Xing/VBRI/LAME frame count is scanned (frames counted, bytes bounded by the file). `PROBE_VERSION` takes the next number when Task 17 lands (6B-1 takes 2 for WebM on `home/sp6b1`, so 3 after it; TASKS line "bump `PROBE_VERSION` whenever probe output changes"), which re-queues the probe stage for every catalogued file (5D). The model's `AudioInfo {sampleRate, channels}` is unchanged.
15. **The plan shape** (spec §8.4 says `params: {path: segments}`; enums are not segments, **spec vs code**): `AudioItem.effects: Vec<AudioEffectPlan>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so every existing plan hash, the 4C eval corpus and `scripts/e2e/reel-golden/audio.json` are unchanged), `AudioEffectPlan {id, kind, numbers: BTreeMap<String, Vec<ParamSegment>>, values: BTreeMap<String, PlanValue>}`, `ParamSegment {from, to, v: [f64; 2], interp, u: [f64; 2]}` read like `GainSegment` (`lerp(v0, v1, ease(lerp(u0, u1, x)))`). Every registry param of the kind is present (defaults filled), numbers cover `[from, to)` without gaps; nested items' effect segments are shifted with their gain and pan.
16. **Processing order** (spec §8.2, §8.4): per item, read source → WSOLA (speed ≠ 1) → effects in plan order → gain × shapes → pan → sum into the master bus in plan order. Gain, shapes and pan are evaluated at **control points every 32 samples** (and at every segment edge) and linearly ramped in amplitude between them; effect coefficients are recomputed every **64 samples** from the parameters at the block's first sample (spec §8.4). `equalPower` is `sqrt(x)`; a mono source pans with constant power (`L = x·sqrt((1 − p)/2)`, `R = x·sqrt((1 + p)/2)`: Decision 3's single upmix), a stereo source with balance (`p < 0`: `R × (1 + p)`, `p > 0`: `L × (1 − p)`, the other side at unity; spec §8.2). Pan is clamped to [−1, 1] and applied after effects, which therefore run on 1 channel for a mono source and 2 for stereo. Dynamics detectors and filter states carry across blocks and are reset only at an item's first sample.
17. **Streaming, offline only in 6C.** `Mixer::render(&mut self, out: &mut Stereo)` fills any block length (128 for 6F's worklet quantum, 48,000 for jobs) and the output does not depend on the block length (a test renders the same plan in blocks of 128, 1,000 and 48,000 and compares bits). 6C runs it only offline in the worker; the AudioWorklet glue is 6F's.
18. **Loudness and the limiter** (spec §8.6): `loudness::Meter` wraps `ebur128::EbuR128` (modes `I | LRA | TRUE_PEAK | SAMPLE_PEAK`); integrated (LUFS), range (LU), true peak (dBTP), sample peak, short-term max. `Limiter`: look-ahead 5 ms, peak detection on a 4× oversampled signal (the resampler's kernel at ratio 4/1), instant attack over the look-ahead, 50 ms release, ceiling in dBTP. `normalise(measured, target) -> GainDecision {gainDb (quantised 0.01 dB), limit: bool, warning?}`: unreachable when the input is silent (integrated below −70 LUFS) or when the limiter would need more than 1 LU of reduction (`LOUDNESS_TARGET_UNREACHABLE {measured, target}`, a warning). 6E runs the two passes in an export; 6C provides and tests them.
19. **Waveform peaks** (`wave` v1): per channel of the 48 kHz stereo signal, min/max pairs as `i16` (`round(x × 32767)`, clamped) at 256, 1,024, 4,096, 16,384 and 65,536 samples per peak; `peaks.bin` = a 32-byte header (`FINWAVE1`, levels, channels, frames) then the levels in order. Computed from `pcm48` when its entry exists, else from the decode.
20. **`media.pcm` / `media.waveform` / `media.listen`** are cold kinds on `worker:media`: `media.pcm {asset, fingerprint, recipe: pcm48|pcm16m, key}` → `PcmResult {key, frames, chunks, decoder: {backend, class}, sourceRate, sourceChannels}` (priority `background`, pool `cpu`), `media.waveform {asset, fingerprint, key}` → `WaveResult {key, levels}` (`background`, `cpu`), `media.listen {projectId, branch, rev, timeline, from, to}` → `ListenResult` (`normal`, `cpu`). Memory: `batch`'s `min(4 GiB, 25 % of RAM)` through 5C's `MemoryRule`. `pcm16m` registers as a library stage (`pcm16m`, after `probe`); `pcm48` is submitted when an asset is first used on an audio track of an open project (beside 6B-1's `1080` proxy hook) and on demand (`fin media pcm`).
21. **`fin listen`** (spec §8.7): `media.listen` evaluates the audio plan in the worker (from the pinned snapshot, plan samples as `media.look` does: BLAKE3 of the plan's compact JSON at the range start and every 10 s), mixes from `pcm48` entries the manifest names (manifest field `pcm: {asset → key}`) or decodes originals when an entry is missing, measures per track and for the mix, and answers `ListenResult {timeline, from, to, mix: ListenStats, tracks: [{track, stats}], notes}` with `ListenStats {integrated, range, truePeak, samplePeak, clipped: {samples, firstAt?}, silences: [{from, to}], loudest: [{at, shortTerm}] (top 5 short-term (3 s) loudness maxima, ≥ 3 s apart)}`. **Silence is its own detector, not short-term loudness** (whose 3 s window cannot resolve a 0.7 s gap): the signal is cut into **10 ms blocks** (480 samples, aligned to the range start); a block is silent when the larger of its two channels' RMS is below **−60 dBFS**; a silence is a maximal run of silent blocks lasting **≥ 0.5 s (50 blocks)**; `from` is the first silent block's start and `to` the last one's end (10 ms resolution), clipped to the range; a run touching a range edge is reported clipped, not dropped. Times are rational strings. Speech/music labels join with sub-project 8.
22. **Gapless length contract** (spec §8.1, §9.1 item 4). The muxer today writes the audio edit as `media_time = priming` and `segment_duration` to the **end of the encoded samples** (`fin-container/src/mux/mp4.rs:152–170`), and `AudioTrackConfig` (`mux/mod.rs:59`) has no padding or presented length, so a round trip gains up to one codec frame of padding. 6C defines:
    - an encoder's `finish()` answers `EncodedLength {priming, valid, padding}` with **`valid` = the samples sent**, `priming` = its delay (Opus: the encoder lookahead, also written as `dOps` pre-skip; AAC-LC: 1,024 for vo-aacenc, `kAudioConverterPrimeInfo.leadingFrames` for AudioToolbox, MF's reported delay), and `padding = decoded total − priming − valid` (the last frame's fill, `< frame_size` plus any codec tail);
    - `AudioTrackConfig` gains **`presented: Option<u64>`** (samples at the track rate); when set, the edit list's `segment_duration` is exactly `presented` (instead of "to the encoded end") and `media_time` stays `priming`;
    - **exactness in the movie timescale:** when a file has an audio track, `movie_timescale` becomes the least common multiple of the audio rate and the video track's timescale (48,000 alone for audio-only files; e.g. 240,000 for a 30,000 video timescale), falling back to the audio rate when the LCM passes `u32::MAX`, so both tracks' edits are exact; files without audio keep today's rule, so no 6A or 6B-1 output changes (their proxies are video-only until Task 20);
    - the demuxer already derives `padding = total − priming − shown` from the edit (`demux/mod.rs:484–503`); 6C adds the test that it reads back exactly the `padding` the encoder reported.
    These are the only `fin-container` edits 6C makes (`mux/mod.rs` 455 lines, `mux/mp4.rs` 461, and their tests). **6B-1 does not edit `fin-container/src/mux/`** on `home/sp6b1` (its container work is `demux/webm/` and `codec_config/sei.rs`); if 6B-1 Task 14 needs a muxer change after all, it is a merge point.
23. **A failed `media.pcm` attempt restarts from sample 0.** 5C empties a kind's `write` directories before every attempt (`apps/finisherd/src/jobs/files.ts:162`), 6B-1's commit runs only after the worker exits (`fin-node/src/media/cache/commit.rs`), and item outcomes and checkpoints are separate, at-least-once writes (`apps/finisherd/src/jobs/writers.ts:1`). So chunks cannot survive an attempt, and rebuilding decoder and resampler state mid-stream (decoder pre-roll, the resampler's history, priming) would need a second, riskier code path. `media.pcm` therefore reports **progress, not items, and writes no checkpoint**: a retry after a worker crash, a hang or a `finisherd` restart decodes from the first sample again, which is deterministic for a given decoder (Decision 8), so the committed entry is byte-identical to an uninterrupted run. A 10-minute source decodes in seconds (Task 23 measures it); if that measurement shows hour-long sources need resumption, it is a follow-up with 5C (an attempt-surviving directory), not 6C.
24. **Backend and codec names** for diagnostics are the closed tables in `crates/fin-core/src/events/names_media.rs`: `MEDIA_BACKENDS` already holds `symphonia`, `libopus`, `vo-aacenc`, `audiotoolbox` and `mediafoundation`, and `backend_class` already maps them (`software`, `os`), so the audio backends use **those names** (Linux AAC decode is `symphonia`, Windows AAC is `mediafoundation`), and no backend name is added. `MEDIA_CODECS` (video codecs and `none` today) gains `aac`, `opus`, `mp3`, `flac`, `alac`, `vorbis`, `pcm`; `RenderPhase` (`events/payloads/media.rs:103`) gains `decodeAudio`, `mix`, `encodeAudio`. `fin-media`'s `AudioCodec` (`format.rs:70`) has no `Vorbis` (WebM/Ogg Vorbis read as `Other` today) and gains it (Task 12).

## Review focus

1. **A0 holds:** the mixer, every effect, the resampler, WSOLA, the limiter and the peaks hash the same native and on `wasm32-wasip1`, and block length never changes output. → Tasks 3, 4, 7–11 (`a0_corpus_native_matches_wasm`, `block_size_invariant`).
2. **Sample-exact timing:** item edges, fades, crossfades and chunk boundaries land on `round_half_up(t × 48000)` from exact rationals, never accumulated. → Task 7 (`ntsc_rate_item_edges_exact`), Task 18.
3. **The plan is the whole truth:** effects come only from `AudioItem.effects`; the mixer never reads the model; nested effects shift with their item; existing plan hashes unchanged; a mono source is upmixed exactly once. → Tasks 5, 6, 7 (`mono_vs_dual_mono_pan`).
4. **Licences and patents:** no AAC code of ours outside Linux binaries; MPL notices; the native gate covers libopus and vo-aacenc; no SIMD FFT crate in the tree. → Tasks 1, 13, 14, 24.
5. **Gapless:** a round trip through every encoder, muxer and reader returns exactly the samples sent, for lengths that are and are not codec-frame multiples. → Tasks 13, 14 (`*_gapless_lengths`).
6. **Decoders fail typed:** truncated, corrupt, oversized-channel and HE-AAC-on-Linux inputs are typed errors, never panics; a decode bounded by the container's limits. → Tasks 12–16.
7. **Cache entries are complete and keyed right:** `audioVersion` in the key, the decoder class recorded, chunks whose count and sizes match `meta.json`. → Tasks 18–20.

## Frozen interfaces

```rust
// crates/fin-audio
pub const RATE: u32 = 48_000;
pub const AUDIO_VERSION: u32 = 1;
pub struct Stereo { pub l: Vec<f32>, pub r: Vec<f32> }
pub fn sample_at(t: Rational) -> Result<i64, CoreError>;                  // round_half_up(t × 48000), exact
// crates/fin-media/src/audio_layout.rs (shared by fin-codecs and fin-audio)
pub enum ChannelPos { L, R, C, Lfe, Ls, Rs, Lb, Rb, Cb, Lw, Rw, Unknown }
pub struct ChannelLayout(pub Vec<ChannelPos>);   // in the decoder's channel order
pub mod channels { pub enum Canonical { Mono(Vec<f32>), Stereo(Stereo) }
                   pub fn canonical(interleaved: &[f32], layout: &ChannelLayout) -> Result<Canonical, CoreError>;  // by position; BS.775 for 3–8
                   pub fn fold_mono(c: &Canonical, out: &mut Vec<f32>); }
pub struct Resampler;  // new(from_hz, to_hz) -> Result<Self, CoreError>; process(&mut self, input: &[f32], out: &mut Vec<f32>); finish(&mut self, out: &mut Vec<f32>);
pub fn resampled_len(n_in: u64, from_hz: u32, to_hz: u32) -> u64;          // ceil(n_in × to / from)
pub struct Wsola;      // new(speed: Rational) -> Self; process(&mut self, input: &Stereo, out: &mut Stereo); finish(..)
pub enum SourceLayout { Mono, Stereo }
pub trait PcmSource { fn layout(&self) -> SourceLayout; fn read(&mut self, at: i64, frames: usize, out: &mut Canonical) -> Result<(), CoreError>; } // 48 kHz frames from `at`, in its own layout; zeros outside
pub struct Mixer;      // new(plan: &AudioPlan, sources: BTreeMap<String, Box<dyn PcmSource>>) -> Result<Self, CoreError>; render(&mut self, out: &mut Stereo) -> Result<(), CoreError>; position(&self) -> i64
pub struct Meter;      // new(channels: u16) -> Self; push(&mut self, block: &Stereo); measured(&self) -> Measured {integrated, range, true_peak, sample_peak, short_term_max}
pub struct Limiter;    // new(ceiling_dbtp: f64) -> Self; process(&mut self, block: &mut Stereo); finish(&mut self, out: &mut Stereo)
pub fn normalise(m: &Measured, target_lufs: f64, ceiling_dbtp: f64) -> GainDecision;
pub fn peaks(pcm: &Stereo) -> Peaks; impl Peaks { pub fn to_bytes(&self) -> Vec<u8>; pub fn parse(b: &[u8]) -> Result<Peaks, CoreError>; }
// crates/fin-codecs/src/audio
pub trait AudioDecoder: Send { fn send(&mut self, packet: &[u8], pts: i64) -> Result<(), CoreError>; fn receive(&mut self) -> Result<Option<DecodedAudio>, CoreError>; fn finish(&mut self) -> Result<(), CoreError>; fn flush(&mut self) -> Result<(), CoreError>; fn backend(&self) -> BackendId; }
pub trait AudioEncoder: Send { fn send(&mut self, pcm: &[f32], pts: i64) -> Result<(), CoreError>; fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError>; fn finish(&mut self) -> Result<EncodedLength, CoreError>; fn config(&self) -> CodecConfig; fn priming(&self) -> u32; fn frame_size(&self) -> u32; }
pub struct EncodedLength { pub priming: u32, pub valid: u64, pub padding: u32 }   // Decision 22: decoded total = priming + valid + padding
pub struct DecodedAudio { pub interleaved: Vec<f32>, pub layout: ChannelLayout, pub rate: u32, pub pts: i64 }
pub fn open_audio_decoder(info: &AudioTrackInfo, config: &AudioConfig) -> Result<Box<dyn AudioDecoder>, CoreError>;      // CODEC_UNAVAILABLE
pub fn open_audio_encoder(cfg: &AudioEncoderConfig, purpose: EncodePurpose) -> Result<Box<dyn AudioEncoder>, CoreError>;  // policy-filtered
pub struct AudioReader;  // open(file, limits) -> Result<Self, CoreError>; info() -> &AudioTrackInfo; next(&mut self) -> Result<Option<DecodedAudio>, CoreError> (priming and padding already trimmed, native rate and layout); decoder_class() -> VendorClass
// crates/fin-workers/src/media/audio_src.rs (the only place decoded audio meets fin-audio)
pub struct DecodedSource;  // over AudioReader: canonical layout (fin_audio::channels) → Resampler to 48 kHz or (folded) to 16 kHz; impl PcmSource
pub struct CachedSource;   // over a pcm48 entry's chunks; impl PcmSource
```

---

## Batch A: foundations and the DSP core

### Task 1: Tracker, the `fin-audio` crate, dependencies and gates — tier: sonnet — A

**Depends on:** nothing.

**Files:**
- Modify: `TASKS.md` ("Sub-project 6C" with Tasks 1–24, the batch table and the merge points of Decision 1).
- Create: `crates/fin-audio/{Cargo.toml, clippy.toml, src/lib.rs, src/buf.rs, src/time.rs}` (`#![forbid(unsafe_code)]`, deps `fin-core`, `libm` from the workspace; feature `testing`). `clippy.toml` identical to the shared one (`scripts/check-clippy-config.mjs`).
- Modify: root `Cargo.toml` (`members`/`default-members` gain `crates/fin-audio`; `[workspace.dependencies]` `symphonia = { version = "=0.6.1", default-features = false }`, `ebur128 = "=0.1.11"`), `deny.toml` only if a new licence text appears (none expected: MPL-2.0, MIT, Apache-2.0 are listed).
- Modify: `scripts/check-eval-floats.mjs` (+ test): an `AUDIO` pattern `crates/fin-audio/src/**` excluding `loudness/meter.rs` and `testing/**`, with the render rules.
- Modify: `scripts/check-crate-deps.mjs` (+ test): `ONLY` gains `{crate: "fin-audio", by: ["fin-workers", "fin-wasm"]}` (transitive, so `fin-codecs` may not take it: Global Constraints), `{crate: "fin-opus-sys", by: ["fin-codecs", "fin-workers"]}`, `{crate: "fin-aacenc-sys", by: ["fin-codecs", "fin-workers"]}`; `CONFINED` gains `{crate: "fin-audio", may: ["fin-core", "fin-media"]}`; `TARGETED` gains `objc2-audio-toolbox`/`objc2-core-audio-types` under `cfg(target_os = "macos")` and `fin-aacenc-sys` under `cfg(target_os = "linux")`; a new rule refuses `rustfft` anywhere in the shipped graph and `symphonia`'s `aac` feature outside a Linux-gated section.
- Modify: `scripts/gates.mjs` (`cargo build -q -p fin-audio --target wasm32-unknown-unknown`), (the wasm32-wasip1 test step is Task 11's, which creates `tests/a0.rs`).

**Failing tests first:** `check-eval-floats.test.mjs` `audio_module_refuses_powf` (a fake `fin-audio/src/mix/curves.rs` with `x.powf(y)` → one finding) and `audio_meter_exempt`; `check-crate-deps.test.mjs` `fin_node_linking_fin_audio_fails`, `fin_codecs_linking_fin_audio_fails`, `rustfft_in_graph_fails`, `aac_feature_ungated_fails`; `fin-audio` `tests/time.rs`: `sample_at_rounds_half_up` (`1/96000` → 1, `1001/30000` → 1,602 (1601.6 rounds up), `-1/96000` → 0), `sample_at_overflow_is_time_overflow`.

- [ ] **Step 1:** the tests above → FAIL. **Step 2:** crate skeleton, workspace entries, gate changes. **Step 3:** `rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates` → PASS. `rtk pnpm check:dep-age` → PASS. **Commit** `build(audio): 6C tracker, the fin-audio crate and its determinism and dependency gates` (licences of `symphonia` 0.6.1 and `ebur128` 0.1.11 in the body; the crates themselves are added by the tasks that use them).

### Task 2: Channels, test signals and analysis helpers — tier: sonnet — A

**Depends on:** Task 1.

**Files:** Create `crates/fin-media/src/audio_layout.rs` (`ChannelPos`, `ChannelLayout`, and the per-format order tables of Decision 3: `wave_default(n)`, `wave_mask(mask)`, `vorbis_order(n)` (Opus family 1), `aac_config(n)`), `crates/fin-media/tests/audio_layout.rs`, `crates/fin-audio/src/channels.rs`, `src/testing/{mod.rs, signals.rs, analyse.rs}`, `tests/channels.rs`, `tests/testing.rs`. Modify `crates/fin-media/src/lib.rs`.

**Behaviour:** Decision 3's canonical forms (mono kept mono, stereo, BS.775 by position) and mono fold; `testing::signals`: `sine(hz, dbfs, secs, rate)`, `sweep(from_hz, to_hz, secs, rate)` (log), `impulse(at, len)`, `noise(seed, secs, rate)` (xorshift64*, uniform → deterministic, no `rand`), `pink(seed, ..)` (Voss-McCartney), `tone_burst`, `silence`, `ebu3341_case(n)` and `ebu3342_case(n)` (the documented sine/noise sequences and levels, generated); `testing::analyse`: `rms_dbfs`, `peak_dbfs`, `goertzel_db(signal, hz, rate)`, `thd_n_db(signal, fundamental_hz, rate)` (notch by fitting the fundamental with least squares), `dominant_hz` (zero crossings with interpolation), `null_db(a, b)` (residual of a − b).

**Failing tests first:** `mono_stays_mono` (a 1-channel layout → `Canonical::Mono`, bit-exact), `stereo_passthrough_bit_exact`, `each_channel_isolated_per_layout` (for every table: WAVE default 3–8, WAVE masks, Vorbis/Opus-family-1 3–8, AAC configurations 3–7: an impulse on one channel index at a time lands where its **position** says: C → 0.70710677 on L and R, Ls → L × 0.70710677, Rs → R × 0.70710677, Lb → L × 0.70710677, LFE → nothing, L → L only), `vorbis_and_wave_51_differ_by_index_not_by_result` (the same positions in the two orders give bit-identical stereo), `unknown_position_unavailable` and `nine_channels_unavailable` (`CODEC_UNAVAILABLE {reason: channels}`), `mono_fold_is_half_sum`; `sine_rms_matches_level` (−20 dBFS sine → RMS −23.01 dB ± 0.01), `goertzel_finds_tone` (±0.05 dB), `thd_n_of_pure_sine_below_minus_120db`, `noise_is_reproducible` (same seed → same bytes).

- [ ] Tests → FAIL; implement; gates → PASS. **Commit** `feat(audio): channel conversion, generated test signals and analysis helpers`.

### Task 3: The scalar polyphase resampler — tier: opus — A

**Depends on:** Task 2.

**Files:** Create `crates/fin-audio/src/resample/{mod.rs, kernel.rs}`, `tests/resample.rs`.

**Behaviour:** Decision 5. Streaming (`process` any chunk sizes, `finish` flushes the tail); output identical whatever the chunking.

**Failing tests first** (each at 44,100 → 48,000, 48,000 → 16,000, 22,050 → 48,000, 96,000 → 48,000, 8,000 → 48,000, 47,999 → 48,000 unless stated):
- `length_is_exact` (`resampled_len` and actual length for n = 0, 1, 1,000, 441,000);
- `impulse_lands_on_mapped_sample` (impulse at input 4,410 of 44.1 kHz → peak at output 4,800, |offset| = 0);
- `passband_flat` (Decision 5: sines from 100 Hz to the passband edge `0.4535·fs_min`: 20.0 kHz for 44.1 → 48, 7.25 kHz for 48 → 16, 10.0 kHz for 22.05 → 48, 21.7 kHz for 96 → 48, 3.62 kHz for 8 → 48: gain within ±0.01 dB);
- `stopband_rejects` (each tone at or beyond the stopband edge `0.5·fs_min`, measured at the frequency it would alias or image to, relative to a 1 kHz reference at the same level: 48 → 16 kHz, 9 kHz in → 7 kHz ≤ −95 dB; 96 → 48 kHz, 26 kHz in → 22 kHz ≤ −95 dB; 44.1 → 16 kHz, 18 kHz in → 2 kHz ≤ −95 dB; 44.1 → 48 kHz, 15 kHz in, its image at 29.1 kHz folding to 18.9 kHz ≤ −95 dB). The prototype computes ≥ 102 dB there (Decision 5); 7 dB is the margin for `f32`;
- `transition_band_shape` (0.46·fs_min −0.05 ± 0.05 dB, 0.4768·fs_min −6.0 ± 0.1 dB);
- `thd_n_1khz` (1 kHz at −1 dBFS: THD+N ≤ −100 dB);
- `chunking_invariant` (1, 7, 480 and 48,000-sample chunks → identical bytes);
- `identity_ratio_bit_exact` (48 → 48 copies);
- `golden_hash` (a 1 s sweep 44.1 → 48 kHz, BLAKE3 pinned; re-blessed only with an `AUDIO_VERSION` bump).

- [ ] Tests → FAIL; implement; gates and the wasm32 build → PASS. **Commit** `feat(audio): scalar polyphase windowed-sinc resampler`.

### Task 4: WSOLA time-stretch — tier: opus — A

**Depends on:** Task 2.

**Files:** Create `crates/fin-audio/src/stretch.rs`, `tests/stretch.rs`.

**Behaviour:** Decision 6. Input frames needed for `n` output frames at speed `s` are exactly those the mixer computes from Decision 4 (the stretcher is driven by source position, not by its own counter, so it never drifts).

**Failing tests first:** `speed_one_bypasses_bit_exact`; `length_exact_at_speeds` (1/2, 3/2, 2, 1001/1000, 1/100, 100: output frames = `sample_at(duration)` exactly); `pitch_preserved` (440 Hz sine at 1/2, 3/2 and 2: `dominant_hz` within ±0.5 %); `level_preserved` (RMS within ±0.5 dB at 3/2); `no_clicks_on_sine` (max sample-to-sample step ≤ 1.2 × the input's at 3/2); `stereo_offsets_shared` (L and R identical in → identical out); `chunking_invariant`; `golden_hash` (speech-like pulsed noise at 3/2).

- [ ] Tests → FAIL; implement; gates → PASS. **Commit** `feat(audio): pitch-preserving WSOLA time-stretch`.

## Batch B: the plan grows

### Task 5: The audio effect kinds in the registry, `effect.add` by domain, the model bump — tier: opus — B

**Depends on:** nothing (parallel with Batch A).

**Files:**
- Create: `crates/fin-core/src/params/audio_effects.rs` (Decision 13's specs; a `const fn` helper per shape keeps it under 600 lines), `crates/fin-core/tests/audio_effects_registry.rs`.
- Modify: `crates/fin-core/src/params/effects.rs` (`KINDS` gains the four; `EffectDomain {Visual, Audio}`, `pub fn effect_domain(kind) -> Option<EffectDomain>`; `of`/`params_of`/`find` cover both spec tables), `params/mod.rs` (re-export), `crates/fin-core/src/ops/effect_add.rs` (`check_visual_track` → `check_track_domain`: Decision 13's two refusals; module doc), `crates/fin/src/commands/{schema.rs, effect.rs}` (`fin schema param` prints the domain; `fin effect add` help lists audio kinds), `crates/fin-store/examples/export_schema.rs`, regenerate `@finisher/core-types`.
- **Model version** (Owner question 1): `crates/fin-core/src/model/mod.rs` `SCHEMA_VERSION` = **integration's value + 1 when this task lands** (read `model/mod.rs` on integration just before the merge; streams that land later renumber, as review batch J and 6B-1 did; never a hard-coded number); the DB version likewise is integration's `DB_VERSION` + 1; `crates/fin-core/src/upgrade/{mod.rs, snapshot_v<N-1>.rs, record_v<N-1>.rs}` (identity steps: the runner refuses a version with no step, `upgrade/mod.rs` `upcast`; no data is rewritten: a model with no audio effects is already valid), `crates/fin-store/src/migrations/{mod.rs, m<next>_audio_effects.rs}` (the version-only migration, as `m0006_fade_offsets` does: `formatVersion`/`minReaderVersion` to the next DB version, no table or row changes), `crates/fin-core/tests/upgrade_golden.rs` (the new step's identity case), eval corpus relabelled to the new schema (hashes unchanged), `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §5.2 (one line: the audio kinds) and the schema-number notes in spec 6 §4.4 and spec 8 if the reservation shifts.

**Failing tests first:** `audio_kinds_listed_with_domain`; `eq_band_params_flat_and_typed` (`band3Freq` number 20–20,000 Hz, `band3Type` enum, `band9Freq` → `PARAM_UNKNOWN` with alternatives); `defaults_match_decision_13`; `audio_effect_on_audio_clip_ok`; `audio_effect_on_video_track_refused` (`TRACK_KIND_MISMATCH`, policy, `fix`); `visual_effect_on_audio_track_still_refused`; `keyframe_on_compressor_threshold` (animatable); `keyframe_on_band_type_refused` (`PARAM_NOT_ANIMATABLE`); `param_set_out_of_range` (`INVALID_RANGE {path: effects.fx1.ratio, min: 1, max: 20}`); `split_copies_audio_effects_with_ids`; `undo_effect_add_exact`; `older_schema_refused_by_older_reader` (a record stamped with the new version → `NEWER_FORMAT` for a reader at N−1, using the existing `upcast` test helper); `upgrade_identity_step` (golden).

- [ ] Tests → FAIL; implement; regenerate types; `rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates`, `rtk pnpm e2e` → PASS. **Commit** `feat(core)!: audio effect kinds eq, compressor, gate and deEsser; model v<N>` (the number taken).

### Task 6: `AudioItem.effects` in `eval_audio` — tier: opus — B

**Depends on:** Task 5.

**Files:**
- Create: `crates/fin-core/src/eval/audio_effects.rs` (`AudioEffectPlan`, `ParamSegment`; `effects_of(item, from, to) -> Result<Vec<AudioEffectPlan>, CoreError>` over `NumberCurve` per number path `effects.<fx>.<param>`, breakpoints at keyframes and the range ends; enums/bools resolved by `interp::resolve`), `crates/fin-core/tests/eval_audio_effects.rs`, `crates/fin-core/tests/eval_corpus/projects/21-audio-effects.json` (an EQ with a keyframed band gain, a compressor, a gate and a de-esser on two clips, one inside a nested timeline) and its rows in `expected.json`.
- Modify: `crates/fin-core/src/eval/audio.rs` (the `effects` field per Decision 15; `audio_item` fills it; `Mix::nested` shifts each `ParamSegment`), `eval/mod.rs` (re-exports; `check_audio` checks every segment value is finite, `INTERNAL` otherwise), `export_schema.rs`, `@finisher/core-types`.

**Failing tests first:** `disabled_effect_absent`; `effects_in_item_order`; `all_params_present_with_defaults`; `keyframed_gain_segments_cut_at_keys` (keys at 1 s and 3 s over `[0, 4)` → 3 segments with exact `u`); `segments_clipped_to_range`; `nested_effect_segments_shifted` (inner item at 2 s inside a nested item at 10 s, `sourceIn` 1 → segments at 11 s); `no_effects_hash_unchanged` (the 20 existing corpus projects' `expected.json` untouched); `corpus_21_hash` (blessed here, and on `wasm32-wasip1` by the CI step).

- [ ] Tests → FAIL; implement; regenerate types; gates and `rtk cargo test -p fin-core --target wasm32-wasip1 --test eval_corpus` → PASS. **Commit** `feat(core): audio plans carry each item's effects as piecewise parameters`.

## Batch C: mixer, effects, loudness and A0

### Task 7: The mixer: timing, gain, shapes, pan, speed — tier: opus — C

**Depends on:** Tasks 3, 4, 6.

**Files:** Create `crates/fin-audio/src/mix/{mod.rs, item.rs, curves.rs, pan.rs}`, `crates/fin-audio/src/testing/source.rs` (`MemSource`: a `PcmSource` over a `Canonical`, mono or stereo), `crates/fin-audio/tests/mix.rs`.

**Behaviour:** Decisions 4, 16, 17; the `fin-core` `AudioPlan` is the only input; effect slots are a no-op hook until Task 8 (`effects::Chain::process`). Gain `10^(dB/20)` through `libm::powf`; bézier ease through `fin_core::eval::bezier_y`.

**Failing tests first:** `single_item_unity_passthrough` (0 dB, centre-panned stereo source → output equals input bit-exactly); `item_edges_sample_exact` (item `[1, 2)` s → nonzero exactly from 48,000 to 95,999); `ntsc_rate_item_edges_exact` (edges at `1001/30000 × k` land on `sample_at`, no accumulated drift over 10,000 frames); `gain_db_curve` (linear −20 → 0 dB keyframes: amplitude at the midpoint −10 dB ± 0.01); `fade_in_linear` and `crossfade_equal_power_constant_power` (sum of squares of both sides within ±0.01 dB across the window); `mono_vs_dual_mono_pan` (the same signal `x` as a mono source and as a stereo source with L = R: at pan 0 mono gives L = R = 0.70710677·x and dual mono gives L = R = x, both bit-exact; at pan −1 mono gives L = x, R = 0 and dual mono gives L = x, R = 0; at pan 0.5 mono gives L = 0.5·x, R = 0.8660254·x and dual mono L = 0.5·x, R = x); `mono_upmixed_once` (a mono source through an EQ with every band off at pan 0: −3.01 dB, never −6.02); `cached_and_decoded_mono_agree` (a mono WAV through `DecodedSource` and through a `pcm48` 1-channel `CachedSource` mix to the same bits; the 48 kHz sources are faked with `MemSource` here, the real ones are Task 18's); `two_items_sum_in_plan_order`; `speed_three_halves_uses_wsola` (length and pitch as Task 4); `null_test_inverted_copy` (an item and its −1 copy at the same gain → output all zeros); `block_size_invariant` (128, 1,000, 48,000); `missing_source_is_silent_and_reported` (`Mixer::missing()` lists the asset).

- [ ] Tests → FAIL; implement; gates and wasm32 build → PASS. **Commit** `feat(audio): sample-exact mixer with gain curves, fades, pan laws and speed`.

### Task 8: Parameter interpolation and the EQ — tier: opus — C

**Depends on:** Task 7.

**Files:** Create `crates/fin-audio/src/effects/{mod.rs, params.rs, biquad.rs, eq.rs}`, `crates/fin-audio/tests/eq.rs`. Modify `mix/item.rs` (the chain runs per item before gain, Decision 16).

**Behaviour:** `effects::params::ParamsAt` evaluates an `AudioEffectPlan`'s numbers at a block's first sample (`ParamSegment` read like gain); RBJ cookbook biquads (`lowShelf`, `highShelf`, `peak`, `lowPass`, `highPass`; shelf slope 1, `q` for the others) designed in `f64` with `libm`, run as transposed direct form II in `f32`; coefficients recomputed every 64 samples; a band whose `type` is `off` is skipped; an unknown kind in the plan (a newer model) is skipped and reported (`Mixer::skipped_effects()`).

**Failing tests first:** `peak_band_gain_at_centre` (+6 dB at 1 kHz, Q 1: Goertzel at 1 kHz +6.00 ± 0.05 dB, at 100 Hz and 10 kHz within ±0.3 dB of 0); `low_shelf_and_high_shelf_levels` (±0.1 dB at 20 Hz / 20 kHz); `low_pass_minus_3db_at_cutoff` (Q 0.707: −3.01 ± 0.05 dB); `high_pass_rejects_dc`; `all_bands_off_is_bit_exact_passthrough`; `keyframed_gain_moves_once_per_64_samples` (coefficients change only at block starts); `stable_at_extremes` (20 Hz and 20 kHz, Q 18, ±24 dB: no NaN or ∞ over 10 s of noise); `golden_hash`.

- [ ] Tests → FAIL; implement; gates → PASS. **Commit** `feat(audio): the eq effect: eight RBJ biquad bands with block-rate automation`.

### Task 9: Compressor, gate and de-esser — tier: opus — C

**Depends on:** Task 8.

**Files:** Create `crates/fin-audio/src/effects/{detector.rs, compressor.rs, gate.rs, deesser.rs}`, `crates/fin-audio/tests/dynamics.rs`.

**Behaviour:** linked stereo detection (max of L/R for `peak`, mean square of L/R over a 10 ms window for `rms`), one-pole attack/release smoothing of the gain in dB with coefficients `exp(−1/(τ·48000))` via `libm::expf`; soft knee as the quadratic of Giannoulis et al.; the gate opens above `threshold`, holds `hold` ms, then releases to `range`; the de-esser is a compressor whose detector reads a band-pass (RBJ, `frequency`, `q`) and whose gain applies only to that band (split: band-pass + residual, recombined), reduction capped at `range`.

Levels in the parameters and the tests are **dBFS of the detector's measure**: peak (sample peak of the linked channels) for `detector: peak`, RMS for `detector: rms`; a sine's RMS is its peak − 3.01 dB.

**Failing tests first:** `compressor_static_curve_peak` (`detector: peak`, steady 1 kHz sine at −6 dBFS peak, threshold −18, ratio 4, knee 0, makeup 0: overshoot 12 dB, reduction 12 × (1 − 1/4) = 9 dB → output peak −15.00 ± 0.1 dBFS after settling); `compressor_static_curve_rms` (`detector: rms`, the same sine: RMS −9.03 dBFS, overshoot 8.97 dB, reduction 6.73 dB → output RMS −15.76 ± 0.1 dBFS, peak −12.73 ± 0.1 dBFS); `compressor_knee_midpoint`; `attack_time_constant` (step +20 dB: gain reaches 63 % of its change within attack ± 1 ms); `release_time_constant`; `rms_vs_peak_detector_differ_on_crest`; `gate_closes_below_threshold` (−70 dBFS noise → attenuated by `range` ± 0.5 dB), `gate_hold_keeps_open` (the gain stays open `hold` ms after the signal drops); `deesser_reduces_band_only` (6.5 kHz burst + 200 Hz tone: 6.5 kHz down ≥ 6 dB, 200 Hz within ±0.1 dB); `deesser_range_caps_reduction`; `no_nan_on_silence`; `golden_hash` per effect.

- [ ] Tests → FAIL; implement; gates → PASS. **Commit** `feat(audio): compressor, gate and de-esser`.

### Task 10: Loudness measurement, the true-peak limiter, normalisation, peaks — tier: opus — C

**Depends on:** Task 7.

**Files:** Create `crates/fin-audio/src/loudness/{mod.rs, meter.rs, limiter.rs, normalise.rs}`, `crates/fin-audio/src/peaks.rs`, `crates/fin-audio/tests/{loudness.rs, limiter.rs, peaks.rs}`. Modify `crates/fin-audio/Cargo.toml` (`ebur128` from the workspace).

**Behaviour:** Decisions 18 and 19.

**Failing tests first:** `ebu3341_cases` (generated cases 1–5: integrated within ±0.1 LU of −23 LUFS, cases 7–8 within ±0.1 LU, case 9 true peak), `ebu3342_cases` (LRA within ±1 LU of the documented values), `silence_is_minus_inf` (integrated `null`, no NaN); `limiter_holds_ceiling` (a +6 dBFS 997 Hz sine and inter-sample-peak test tones: true peak by `Meter` ≤ −1.0 + 0.1 dBTP), `limiter_transparent_below_ceiling` (−10 dBFS sine: bit-exact delayed by the look-ahead), `limiter_latency_compensated` (`finish` returns the tail; total length unchanged); `normalise_reaches_target` (pink noise at −30 LUFS, target −14 / −1 dBTP: measured after gain and limiter −14 ± 0.2 LUFS), `normalise_gain_quantised_to_hundredths`, `normalise_silence_warns`, `normalise_unreachable_beyond_1lu_warns` (`LOUDNESS_TARGET_UNREACHABLE {measured, target}`); `peaks_levels_and_header` (5 levels; min/max of a known ramp exact), `peaks_parse_round_trip`, `peaks_parse_refuses_truncated`; `limiter_golden_hash`, `peaks_golden_hash`.

- [ ] Tests → FAIL; implement; gates and `rtk pnpm check:dep-age` (`ebur128` 0.1.11, `dasp_frame`, `dasp_sample` enter the lock here) → PASS. **Commit** `feat(audio): BS.1770 loudness, true-peak limiter, normalisation and waveform peaks`.

### Task 11: The A0 corpus: native and wasm32 bit-identical — tier: opus — C

**Depends on:** Tasks 7–10.

**Files:** Create `crates/fin-audio/tests/a0.rs`, `crates/fin-audio/tests/a0/{plans/*.json, expected.json}` (10 plans: each effect alone and chained, fades and crossfades both curves, a nested item, speeds 1/2, 3/2, 1001/1000, a 29.97 timeline, the limiter after a hot mix). Modify `.github/workflows/ci.yml` (the wasm32-wasip1 step gains `cargo test -p fin-audio --features testing --target wasm32-wasip1 --test a0`; from this task on every `fin-audio` task runs it, Global Constraints).

**Behaviour:** each plan's sources are generated signals (`testing::signals`, fixed seeds); output BLAKE3 per plan in `expected.json`; a mismatch prints the plan name and the first differing sample. `AUDIO_VERSION` is asserted against a constant in `expected.json` (`a0_hash_change_requires_audio_version_bump`). The mixer's throughput is measured informationally here (`cargo test --release -p fin-audio --test a0 -- --ignored perf`, 16 stereo items each with EQ + compressor, reporting × real time; the gate is Task 23's).

**Failing tests first:** `a0_corpus_native_matches_expected`, `a0_corpus_native_matches_wasm` (the same test under `wasm32-wasip1`, CI), `a0_hash_change_requires_audio_version_bump`, `block_size_invariant_corpus`.

- [ ] Tests → FAIL; bless on Linux x64; `rtk cargo test -p fin-audio --features testing --target wasm32-wasip1 --test a0` → PASS; the macOS arm64 run before the batch merges. **Commit** `test(audio): A0 corpus, bit-identical native and on wasm32`.

## Batch D: decode, encode and probe

### Task 12: Audio decoder traits, the symphonia backend and MP4 audio — tier: opus — D

**Depends on:** Task 2 (`fin-media::audio_layout`); not on the resampler, which only the worker applies.

**Files:**
- Create: `crates/fin-codecs/src/audio/{mod.rs, traits.rs, registry.rs, symphonia.rs, mp4.rs, reader.rs}`, `crates/fin-codecs/src/audio/testing/{wav.rs, aiff.rs, caf.rs, flac.rs, ogg.rs}` (test-only writers under `testing`), `crates/fin-codecs/tests/audio_decode.rs`.
- Create: `crates/fin-media/src/backend_audio.rs` (`AudioBackendEntry {name, class, decode: &[AudioCodec], encode: &[AudioCodec]}`, `AUDIO_BACKENDS` with `symphonia` (`software`) now and rows added by Tasks 13–16, `audio_encoder_allowed` per Decision 10), `crates/fin-media/tests/audio_policy.rs`.
- Modify: `crates/fin-codecs/{Cargo.toml (symphonia; **not** `fin-audio`), src/lib.rs (pub use audio::*)}`, `crates/fin-media/src/{lib.rs, format.rs}` (**merge point**, 6B-1 edits `ContainerKind` in the same file: `AudioCodec::Vorbis`, mapped from Ogg/WebM Vorbis and symphonia's `CODEC_TYPE_VORBIS`; generated types regenerated), `crates/fin-container/src/demux/webm/codec_private.rs` only if 6B-1's WebM demuxer maps `A_VORBIS` to `Other` (then one line to `Vorbis`, a merge point), `crates/fin-store/examples/export_schema.rs`, `@finisher/core-types`, `scripts/check-encoder-call-sites.mjs` (+ test: `audio/registry.rs` is the only audio opener).

**Behaviour:** `AudioReader::open` sniffs MP4 (through `fin-container`'s `Demuxer`, its limits and its first audio track; AAC and Opus configurations are routed to Tasks 13–16's backends, other MP4 audio to symphonia's codec by codec type) or a symphonia container (format readers of the features above); trims priming and padding from `AudioTrackInfo` (spec §8.1) so sample 0 is the first audible sample; returns `DecodedAudio` at the source's own rate with its `ChannelLayout` (Decision 3). **It never resamples or downmixes:** `fin-codecs` does not link `fin-audio`; the worker's `audio_src.rs` (Task 18) and the tests (through `fin-audio` as a dev-dependency only, which `check-crate-deps` ignores) do. Reads are bounded (`MEDIA_LIMIT` above 24 h of audio or 8 channels × 384 kHz). symphonia's decoders run inside `catch_unwind`; a panic is `DECODE_FAILED {backend: symphonia}`.

**Failing tests first:** `wav_pcm16_24_32f_bit_exact` (generated sine written as each, decoded back exactly), `aiff_and_caf_lpcm_bit_exact`, `flac_verbatim_bit_exact` (test writer), `wav_layout_from_mask` (a 5.1 `WAVEFORMATEXTENSIBLE` with mask `0x60F` → `[L, R, C, Lfe, Ls, Rs]`), `wav_44100_native_rate` (rate 44,100 reported, frames exact), `mono_wav_reports_mono`, `vorbis_codec_mapped` (`AudioCodec::Vorbis` from an Ogg Vorbis header; probe and caps mapping in Task 17), (MP4 priming and padding are Task 13's, which has the first MP4 audio encoder), `truncated_files_typed` (each writer's output cut at 10 points → `MEDIA_CORRUPT` or `DECODE_FAILED`, never a panic), `decoder_class_is_software`; `mp3_vorbis_alac_from_fixture_cache` (skipped with a notice without `FIN_MEDIA_FIXTURES`; with it: duration within one frame of the probe, level within ±0.5 dB of the recorded value); `audio_policy_matrix` (`Export` refuses a hypothetical `Gpu` class; `Proxy` allows all).

- [ ] Tests → FAIL; implement; gates and `rtk pnpm check:dep-age` (symphonia 0.6.1 and its transitives `extended`, `regex-lite` enter `Cargo.lock` here) → PASS. **Commit** `feat(codecs): audio decoder traits, the symphonia backend and the native-rate reader`.

### Task 13: libopus decode and encode — tier: opus — D

**Depends on:** Task 12.

**Files:** Create `crates/fin-opus-sys/{Cargo.toml, build.rs, shim/fin_opus_shim.{c,h}, src/lib.rs}`, `third_party/opus` (submodule at `22244de5a79bd1d6d623c32e72bf1954b56235be`, `.gitmodules`), `crates/fin-codecs/src/audio/opus.rs`, `crates/fin-codecs/tests/audio_opus.rs`. Modify `third_party/native.json` (libopus 1.6.1, BSD-3-Clause, `COPYING` BLAKE3), `THIRD_PARTY_NOTICES.md`, root `Cargo.toml` (members), `crates/fin-media/src/backend_audio.rs` (`libopus`, `software`, decode and encode `Opus`), and the gapless contract of Decision 22 in `crates/fin-container/src/mux/{mod.rs, mp4.rs}` (`AudioTrackConfig.presented`, the LCM movie timescale when audio is present) with `crates/fin-container/tests/audio_gapless.rs`.

**Behaviour:** Decision 11. Decoder at 48 kHz, channel mapping family 0 and 1 (≤ 8 channels; family 255 → `CODEC_UNAVAILABLE`); pre-skip from `dOps`/`OpusHead` honoured through `AudioTrackInfo.priming`; encoder `OPUS_APPLICATION_AUDIO`, 20 ms frames, VBR at the requested bitrate (proxies: 128 kb/s stereo), reports its lookahead as `priming`; `config()` writes `dOps`.

**Failing tests first:** `opus_round_trip_sine` (1 kHz −6 dBFS through encode→decode: level within ±0.3 dB, `dominant_hz` ±1 Hz, null residual ≤ −25 dB after aligning by `priming`); `opus_round_trip_level` (pink noise: RMS within ±0.3 dB, through `fin-audio::testing::analyse`, a dev-dependency; Batch C's loudness meter is not needed); `opus_gapless_lengths` (inputs of 0, 1, 959, 960, 961, 48,000 and 48,001 samples (20 ms frames are 960): `EncodedLength.valid` equals the input, `priming + valid + padding` equals the decoded total, and muxed with `presented = valid` then read back by `AudioReader` the output has exactly `valid` samples whose first equals the input's first after priming); `mux_edit_exact_with_video` (an audio track at 48 kHz beside a 30,000-timescale video track: `mvhd` timescale 240,000, the audio edit's `segment_duration` equals `presented × 5`, the demuxer's `padding` equals the encoder's); `mux_without_audio_unchanged` (a video-only file's bytes equal today's for the 6A fixtures); `opus_in_ogg_test_writer`; `opus_mono_and_51` (family 1 six-channel downmixed per Decision 3); `opus_corrupt_packet_typed`; `opus_wrong_version_refused_by_build` (a `build.rs` unit test on the version check); `native_gate_lists_libopus`.

- [ ] Tests → FAIL; implement; gates (native and notices included) → PASS on Linux (Global Constraints' merge note: `fin-container/src/mux/` is not edited by 6B-1 on `home/sp6b1`; re-check with `git diff` against integration before the commit), then macOS and Windows builds (`rtk cargo test -p fin-codecs --features testing --test audio_opus`) before the batch merges. **Commit** `feat(codecs): libopus 1.6.1 decode and encode through a pinned build`.

### Task 14: AAC-LC on Linux: symphonia decode and vendored vo-aacenc — tier: opus — D (Linux)

**Depends on:** Task 12.

**Files:** Create `crates/fin-aacenc-sys/{Cargo.toml, build.rs, shim/fin_aacenc_shim.{c,h}, src/lib.rs}` (Linux only: on other targets the crate is empty and `build.rs` does nothing), `third_party/vo-aacenc` (submodule, Decision 12, Owner question 6), `crates/fin-codecs/src/audio/aac_linux.rs` (`#[cfg(target_os = "linux")]`), `crates/fin-codecs/tests/audio_aac_linux.rs` (`#![cfg(target_os = "linux")]`). Modify `crates/fin-codecs/Cargo.toml` (`[target.'cfg(target_os = "linux")'.dependencies]` `symphonia` with `aac`, `fin-aacenc-sys`), `third_party/native.json` (vo-aacenc, Apache-2.0, `COPYING`/`NOTICE` BLAKE3, the pinned commit), `THIRD_PARTY_NOTICES.md`, `crates/fin-media/src/backend_audio.rs` (`symphonia` gains decode `Aac` and `vo-aacenc` encode `Aac`, both `software`, Linux rows only; Decision 24's names).

**Merge prerequisite:** the submodule points at **Finisher's maintained fork** (spec §6.4, Owner question 6). The working branch may pin upstream `mstorsjo/vo-aacenc` at the same commit, but Task 14 does not merge into `sp6c-audio` until the fork exists and `.gitmodules` names it.

**Behaviour:** Decisions 9 and 12; HE-AAC (SBR/PS signalled in the AudioSpecificConfig `fin-container/src/demux/aac.rs` already parses) is `CODEC_UNAVAILABLE {codec: aac, profile: he-aac, os: linux, fix: codecPlugin}` before any decode.

**Failing tests first:** `aac_lc_round_trip` (vo-aacenc at 192 kb/s → MP4 → symphonia: level ±0.3 dB, null residual ≤ −20 dB after `priming` 1,024, RMS within ±0.2 dB on pink noise); `aac_gapless_lengths` (inputs of 1, 1,023, 1,024, 1,025, 48,000 and 48,001 samples (AAC frames are 1,024): output length exactly the input's through `presented`, edit media time 1,024, the demuxer's `padding` equal to `EncodedLength.padding`); `he_aac_refused_with_plugin_fix` (a config with explicit SBR); `aac_corrupt_typed`; `aac_only_on_linux` (a gate test over `cargo metadata --filter-platform` for `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`: neither `symphonia-codec-aac` nor `fin-aacenc-sys` objects are in the graph).

- [ ] Tests → FAIL; implement; gates → PASS. **Commit** `feat(codecs): AAC-LC on Linux through symphonia and a vendored vo-aacenc` (body: counsel item 6.1 unchanged).

### Task 15: AudioToolbox AAC decode and encode — tier: opus — D (macOS host, after 6B-1 Batch B)

**Depends on:** Task 12; 6B-1 Tasks 4–5 merged (the `objc2` setup and `apple/` conventions; this task touches no file under `apple/`).

**Files:** Create `crates/fin-codecs/src/audio/os/{mod.rs, toolbox.rs}` (`#[cfg(target_os = "macos")]`), `crates/fin-codecs/tests/audio_toolbox.rs`. Modify root `Cargo.toml` (`objc2-audio-toolbox`, `objc2-core-audio-types` 0.3.2, macOS-gated), `crates/fin-codecs/Cargo.toml`, `crates/fin-media/src/backend_audio.rs` (`audiotoolbox`, `os`, decode `Aac` (LC, HE, HEv2), encode `Aac` (LC)), `third_party/runtime.json` (AudioToolbox, `os`).

**Behaviour:** `AudioConverterNew` from the `esds` AudioSpecificConfig to interleaved `f32`, fed packet by packet through the input callback; encoder AAC-LC 48 kHz stereo at the requested bitrate, its `kAudioConverterPrimeInfo` reported as `priming`. Errors are typed with the `OSStatus` as a number, never a string.

**Failing tests first:** `toolbox_aac_round_trip` (its own encoder → MP4 → its decoder: as Task 14's thresholds); `toolbox_gapless_lengths` (as Task 14's, priming from `kAudioConverterPrimeInfo`); `caps_audio_rows_mac` (AAC decode and encode `os` through `audiotoolbox`, HE-AAC decode `os`; moved here from Task 17); `toolbox_he_aac_decodes` (an HE-AAC stream encoded by AudioToolbox's `kAudioFormatMPEG4AAC_HE`: rate 48 kHz out, level ±0.5 dB); `toolbox_corrupt_typed`; `toolbox_class_os`.

- [ ] Tests → FAIL on the macOS host; implement; `rtk cargo test -p fin-codecs --features testing` on macOS, the three gates and `rtk pnpm check:dep-age` (`objc2-audio-toolbox`, `objc2-core-audio-types`) on Linux → PASS. **Commit** `feat(codecs): AAC decode and encode through AudioToolbox`.

### Task 16: Media Foundation AAC decode and encode — tier: opus — D (Windows 11 host, after 6B-1 Batch C)

**Depends on:** Task 12; 6B-1 Tasks 6–7 merged (`mf/com.rs`'s COM and `MFStartup` guard, made `pub(crate)` here: the one edit under `mf/`).

**Files:** Create `crates/fin-codecs/src/audio/os/mf.rs` (`#[cfg(windows)]`), `crates/fin-codecs/tests/audio_mf.rs`. Modify `crates/fin-codecs/src/mf/com.rs` (visibility only), `crates/fin-media/src/backend_audio.rs` (`mediafoundation`, `os` (Decision 24's name), decode `Aac` (LC, HE), encode `Aac` (LC)), `third_party/runtime.json` (Microsoft's AAC MFTs, `os`).

**Behaviour:** `CLSID_CMSAACDecMFT` / `CLSID_AACMFTEncoder` (enumerated with `MFTEnumEx`, Microsoft's own only), `MF_MT_USER_DATA` from the AudioSpecificConfig, float PCM out; absent MFT (N editions) → `CODEC_UNAVAILABLE {fix: mediaFeaturePack}`.

**Failing tests first:** `mf_aac_round_trip` (Task 14's thresholds); `mf_gapless_lengths` (as Task 14's); `caps_audio_rows_win` (AAC `os` through `mediafoundation`; moved here from Task 17); `mf_he_aac_decodes` (from the fixture cache when present, else skipped with a notice); `mf_missing_mft_guidance` (enumeration stubbed empty → the fix); `mf_corrupt_typed`.

- [ ] Tests → FAIL on the Windows host; implement; `rtk cargo test -p fin-codecs --features testing` on Windows and the three gates on Linux → PASS. **Commit** `feat(codecs): AAC decode and encode through Media Foundation`.

### Task 17: Probing audio containers; audio rows in the capability report — tier: sonnet — D

**Depends on:** Tasks 12–14 (the backend table and its Linux rows) and **6B-1 Task 8 merged** (it creates `fin-codecs/src/probe/guidance.rs` and capability types v2; not on integration today). The macOS and Windows rows are asserted by Tasks 15 and 16, which add them.

**Files:** Create `crates/fin-probe/src/audio.rs`, `crates/fin-probe/tests/audio_containers.rs`. Modify `crates/fin-probe/{Cargo.toml (symphonia, readers only), src/lib.rs (sniffing, `PROBE_VERSION` = **the next number when this task lands**: 6B-1 takes 2 for WebM on `home/sp6b1`, so 3 if that has merged; re-read the constant, never hard-code; the unsupported-format fix text)}`, `apps/finisherd/test/library/` (the re-queue test), `crates/fin-probe/src/fixtures.rs` (AIFF, FLAC, CAF, Ogg writers reused from Task 12's test writers through a `fin-codecs`-free copy: `fin-probe` must not link `fin-codecs`), `crates/fin-media/src/jobs/caps.rs` (`AudioCodecCaps` rows from `AUDIO_BACKENDS`), `crates/fin-workers/src/media/caps.rs`, `crates/fin-codecs/src/probe/guidance.rs` (**merge point** with 6B-1 Task 8: Linux AAC is `software` now; HE-AAC on Linux → `codecPlugin`; Windows N → `mediaFeaturePack`), `export_schema.rs`, `@finisher/core-types`.

**Failing tests first:** `probe_aiff_flac_caf_ogg` (rate, channels, duration within one sample of what was written), `probe_mp3_from_fixture_cache` (skipped without it), `probe_mp3_without_xing_scans` (fixture cache), `probe_truncated_audio_typed`, `probe_version_bumped` (the constant is one above the integration value it found); `probed_assets_requeued` (finisherd, 5D: a library file catalogued at the previous `PROBE_VERSION` is re-queued for the probe stage only, and an `.flac` file that was `UNSUPPORTED_CONTAINER` before becomes a media asset with duration); `caps_audio_rows_linux` (`#[cfg(target_os = "linux")]`: Opus decode/encode `sw` through `libopus`, AAC-LC decode `sw` through `symphonia`, encode `sw` through `vo-aacenc`, HE-AAC `none` with `codecPlugin`, Vorbis/MP3/FLAC/ALAC decode `sw`).

- [ ] Tests → FAIL; implement; gates and `rtk pnpm e2e` → PASS. **Commit** `feat(probe): audio containers through symphonia's readers; audio rows in media.capabilities`.

## Batch E: PCM feeds, peaks, proxy audio and `fin listen`

**Before starting:** 6B-1 Task 13 (`fin-media::cache`, `fin-node::media::cache`, `apps/finisherd/src/media/cache.ts`) merged into integration and `sp6c-audio` rebased on it; Task 20 also needs 6B-1 Task 14.

### Task 18: `pcm48` and `pcm16m`: recipes, `media.pcm`, the library stage — tier: opus — E

**Depends on:** Tasks 12–14.

**Files:**
- Create: `crates/fin-media/src/jobs/pcm.rs` (`PcmParams`, `PcmResult`, `PcmRecipe {pcm48, pcm16m}`, chunk constants of Decision 7), `crates/fin-workers/src/media/{pcm.rs, audio_src.rs}` (`audio_src`: opening an original through `fin-library`'s no-follow opener and the manifest's checks, as `sources.rs` does, then `AudioReader` → `fin_audio::channels::canonical` → `Resampler`: the frozen `DecodedSource` and `CachedSource`; `crates/fin-workers/Cargo.toml` gains `fin-audio`), `apps/finisherd/src/jobs/kinds/media.pcm.ts`, `apps/finisherd/src/media/pcm.ts` (submit, dedupe by key, first-use hook, the `pcm16m` stage spec), `crates/fin-workers/tests/media_pcm.rs`, `apps/finisherd/test/media-pcm.test.ts`.
- Modify (merge points): `crates/fin-media/src/cache.rs` (`PCM48`, `PCM16M` in `RECIPES`), `crates/fin-media/src/jobs/mod.rs`, `crates/fin-workers/src/media/mod.rs` (`KINDS` gains `media.pcm` v1), `apps/finisherd/src/projects.ts` (595 lines on `home/sp6b1`: one call-site line beside 6B-1's proxy first-use hook, the logic in `media/pcm.ts`; re-count first and split `projects.ts` before this task if one line would pass 600), `apps/finisherd/src/main.ts` (591: one line registering the stage, same rule), `packages/contracts/src/{audio.ts (new), media-rpcs.ts}` (`media.pcm {projectId, asset, recipe?, wait?}` → `{job}` or `PcmResult`), `packages/contracts/src/scopes.ts` (`project.read` + `jobs.submit`), `crates/fin/src/commands/media.rs` (`fin media pcm <asset> [--recipe pcm48|pcm16m]`), `crates/fin/src/registry_rpcs.rs`, `export_schema.rs`, `@finisher/core-types`.

**Behaviour:** Decisions 7, 8, 20. The worker writes chunks and `meta.json` into `out/` with `fin-shm::create_output`; `onSucceeded` commits with 6B-1's `MediaCache.commit` (rename, then insert); the key is computed by both sides (`PLAN_MISMATCH {reason: cacheKey}` on a difference). **Retries restart from sample 0** (Decision 23): the kind reports `progress` (samples done of the probe's total), no items and no checkpoint; the worker writes each chunk complete before starting the next (a partial file can only be the last one of a dead attempt, and 5C empties `out/` before the next attempt, `files.ts:162`); the commit checks every listed size and BLAKE3, so no half-written entry is ever indexed. The 8C host call `media.pcm` (pl §5) will read these entries; 6C exposes nothing to plugins.

**Failing tests first:** `pcm48_chunks_and_meta` (a 10 s WAV → 4 chunks: 3 × 131,072 frames + 86,784; sizes and BLAKE3 in `meta.json`); `pcm16m_is_mono_16k` (length `resampled_len`); `pcm48_equals_decoded_source` (concatenated chunks bit-equal `audio_src::DecodedSource`'s output); `key_has_audio_version_and_decoder_class`; `same_key_dedupes`; `worker_death_retry_is_byte_identical` (the worker killed after chunk 2 is written: the retry starts from sample 0 with an empty `out/` and the committed entry's chunks and `meta.json` equal an uninterrupted run's byte for byte); `daemon_death_retry_is_byte_identical` (`finisherd` stopped mid-job and restarted: the job is `JOB_INTERRUPTED`-retried by 5C, same result, and no entry is indexed in between); `mono_source_stays_one_channel` (a mono WAV's `pcm48` has `layout: mono` and 262,144-sample chunks); `missing_original_typed` (`ORIGINALS_MISSING`); `pcm16m_stage_registered_and_runs_after_probe` (finisherd); `first_use_submits_pcm48` (an audio clip inserted in an open project → one `media.pcm` job).

- [ ] Tests → FAIL; implement; regenerate types; three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(media): pcm48 and pcm16m cache recipes, the media.pcm kind and the pcm16m library stage`.

### Task 19: `media.waveform` — tier: sonnet — E

**Depends on:** Tasks 10, 18.

**Files:** Create `crates/fin-media/src/jobs/wave.rs`, `crates/fin-workers/src/media/wave.rs`, `apps/finisherd/src/jobs/kinds/media.waveform.ts`, `crates/fin-workers/tests/media_wave.rs`. Modify `crates/fin-media/src/cache.rs` (`WAVE`), `crates/fin-workers/src/media/mod.rs` (`KINDS`), `apps/finisherd/src/media/pcm.ts` (submitted after `pcm48` commits), `packages/contracts/src/{audio.ts, media-rpcs.ts}` (`media.waveform {projectId, asset, wait?}`), `crates/fin/src/commands/media.rs` (`fin media wave <asset>` prints the levels and the entry key), `export_schema.rs`, `@finisher/core-types`.

**Failing tests first:** `wave_from_pcm48_entry_matches_direct` (bytes equal whether read from the entry or decoded); `wave_levels_header`; `wave_entry_meta_valid` (6B-1's `EntryMeta::check`); `wave_after_pcm48_commit` (finisherd).

- [ ] Tests → FAIL; implement; gates and `rtk pnpm e2e` → PASS. **Commit** `feat(media): waveform peaks as a cache recipe and the media.waveform kind`.

### Task 20: Proxy recipe v2: Opus audio in proxies — tier: opus — E (after 6B-1 Task 14)

**Depends on:** Tasks 13, 18; 6B-1 Task 14 merged.

**Files:** Modify (merge point) `crates/fin-workers/src/media/proxy.rs` (an audio track: the source's audio through `audio_src::DecodedSource` at 48 kHz in its canonical layout (mono stays 1 channel) → libopus 128 kb/s stereo or 64 kb/s mono through `open_audio_encoder(.., EncodePurpose::Proxy)` → `Mp4Writer::add_audio` with the encoder's `priming` and `presented = EncodedLength.valid`, Decision 22), `crates/fin-media/src/cache.rs` (`PROXY_540`, `PROXY_1080` → version 2), `crates/fin-media/src/jobs/proxy.rs` (`ProxyResult.audio?: {codec: opus, bitrate, priming}`), `crates/fin-workers/tests/media_proxy.rs` (new cases), `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` §7.1's note (the amendment §21 item 10 promised).

**Failing tests first:** `proxy_v2_has_opus_audio` (a source with AAC or WAV audio → the proxy's audio track decodes to the source's audio within ±0.3 dB, length equal ± 0 samples after `priming`); `proxy_without_audio_stays_video_only`; `proxy_v1_keys_invalidated` (same source, new key); `proxy_audio_starts_with_video` (a flash/beep pattern from `fin-media::testing` + `fin-audio::testing`: offset ≤ 1 ms after decode).

- [ ] Tests → FAIL; implement; gates and `rtk pnpm e2e` → PASS. **Commit** `feat(media): proxy recipe v2 carries Opus audio`.

### Task 21: `media.listen` and `fin listen` — tier: opus — E

**Depends on:** Tasks 7–10, 18.

**Files:**
- Create: `crates/fin-audio/src/listen.rs` (per-track and mix statistics from `Meter` blocks: Decision 21), `crates/fin-media/src/jobs/listen.rs` (`ListenParams`, `ListenResult`, `ListenStats`), `crates/fin-workers/src/media/listen.rs`, `apps/finisherd/src/jobs/kinds/media.listen.ts`, `apps/finisherd/src/media/listen.ts`, `crates/fin/src/commands/listen.rs`, `crates/fin-workers/tests/media_listen.rs`, `apps/finisherd/test/media-listen.test.ts`, `crates/fin/tests/cli_listen.rs`.
- Modify: `crates/fin-media/src/jobs/manifest.rs` (**merge point**: `pcm: BTreeMap<asset, key>` beside 6B-1's `proxies`), `crates/fin-node/src/media/inputs.rs` (fills `pcm` from the cache index), `crates/fin-workers/src/media/mod.rs` (`KINDS`), `packages/contracts/src/{audio.ts, media-rpcs.ts}` (`media.listen {projectId, branch?, timeline?, from?, to?, wait?}`: the whole timeline when the range is absent; `INVALID_RANGE` past its end), `scopes.ts` (`project.read`, `jobs.submit`), `crates/fin/src/{registry.rs, registry_rpcs.rs}` (`fin listen [--timeline t] [--from a] [--to b] [--json] [--no-wait]`, text summary by default), `export_schema.rs`, `@finisher/core-types`.

**Behaviour:** Decision 21. Per-track statistics mix only that track's items (same mixer, a filtered plan); the mix is the master bus **without** normalisation (`fin listen` reports, it does not change). Notes carry `missingSource {asset}`, `STRETCH_EXTREME {item, speed}`, `skippedEffect {item, effect, kind}`.

**Failing tests first:** `listen_mix_and_tracks` (two tracks of generated tones at −20 and −26 dBFS RMS: per-track integrated within ±0.2 LU of the computed values, the mix's as their power sum); `listen_detects_clipping` (a +3 dB gain on a full-scale tone → `clipped.samples > 0`, `firstAt` exact); `listen_silences_over_half_second` (Decision 21's detector: a 0.7 s gap starting at 2.000 s is reported as `[2, 27/10)` exactly at 10 ms resolution; a 0.4 s gap is not; a gap of 0.495 s (49.5 blocks) is not; a silence running into the range end is reported clipped; −59 dBFS noise is not silence, −61 dBFS is); `listen_loudest_moments_top5_spaced`; `listen_uses_pcm48_when_present` (`stats.sourcesFromCache`); `listen_decodes_when_missing`; `listen_plan_samples_checked` (`PLAN_MISMATCH` on a tampered sample); `fin_listen_text_and_json` (CLI); `listen_range_past_end_invalid`.

- [ ] Tests → FAIL; implement; regenerate types; three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(media): fin listen: loudness, peaks, clipping and silences per track and for the mix`.

## Batch F: diagnostics, e2e, validation, docs

### Task 22: Diagnostics, the e2e `listen` scenario and CI — tier: sonnet — F

**Depends on:** Tasks 18–21.

**Files:** Create `scripts/e2e/{listen.mjs, listen-lib.mjs}`, `scripts/e2e/listen-golden/listen.json`. Modify `scripts/e2e.mjs` (register `listen`), `crates/fin-workers/src/media/diag.rs` (the audio kinds fill `job.done.media {codec, backend, hw: false, frames (samples ÷ 1,024), rtFactorMilli, cacheHit}`), and the **closed core schema** (Decision 24): `crates/fin-core/src/events/names_media.rs` (`MEDIA_CODECS` gains `aac, opus, mp3, flac, alac, vorbis, pcm`; `MEDIA_BACKENDS` and `backend_class` already hold `symphonia, libopus, vo-aacenc, audiotoolbox, mediafoundation` and stay as they are, with a test that every `AUDIO_BACKENDS` name is in the table and maps to its `VendorClass`), `crates/fin-core/src/events/payloads/media.rs` (`RenderPhase` gains `DecodeAudio`, `Mix`, `EncodeAudio`), the journal's kind schema and `packages/core-types` regenerated, `crates/fin-core/tests/` journal round-trip cases for a `job.done` with an audio `media` object and a `render.stats` entry per new phase, and `crates/fin-journal` tests that a payload naming an unknown codec reads as `other`, `.github/workflows/ci.yml` (the wasm32 A0 step on Linux; `cargo test -p fin-codecs --features testing --test audio_opus` on all three OSes; nothing new to install: libopus and vo-aacenc build with `cc`).

**Scenario:** generate a WAV (tones and a silence) and an MP4 with Opus audio (Task 13's encoder) and, on Linux, one with AAC-LC; `fin asset add` both; insert them on two audio tracks with an EQ, a compressor and a fade; `fin media pcm` and `fin media wave` both assets (entries appear in `fin cache status`); `fin listen --json`: integrated, true peak and silences within the golden's tolerances (±0.2 LU, ±0.2 dB, silence edges exact at the detector's 10 ms resolution); `fin view` shows the effects; the journal's `job.done` carries the audio `media` fields; no path in any diagnostics payload.

**Failing tests first:** the scenario fails until the pieces exist (run it first against the branch to see it fail at its first unmet step).

- [ ] Scenario → FAIL; implement; `rtk pnpm e2e listen` and all scenarios, three gates → PASS. **Commit** `test(e2e): listen scenario; audio diagnostics and CI rows`.

### Task 23: Performance and cross-platform validation (controller-run) — tier: opus (controller)

**Depends on:** Tasks 1–22.

**Hosts:** macOS arm64, Windows 11 x64, Linux x64 (kernel ≥ 6.2), and the D31-minimum machine when available (spec §14). Results by role in `archive/sp6/6c-validation.md`; durable conclusions into spec §26 by Task 24.

- [ ] 1. `FIN_REQUIRE_SERVER=1 rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates`, `rtk pnpm e2e` green on each OS.
- [ ] 2. A0: `a0` and every `golden_hash` equal on macOS arm64, Windows x64, Linux x64 and `wasm32-wasip1`.
- [ ] 3. Mixer throughput (spec §14): 16 stereo items each with EQ + compressor, release build: ≥ 100× real time on the reference machines, ≥ 30× on the D31-minimum. A miss is fixed (scalar restructuring only; no SIMD) or recorded as an owner question, never waived silently.
- [ ] 4. `media.pcm` of a 10-minute and a 2-hour AAC source per OS (seconds, × real time: this decides whether Decision 23's restart-from-zero needs a resumption follow-up) and `media.listen` of a 60 s two-track timeline.
- [ ] 5. AAC round trips through each OS's encoder decode within Task 14's thresholds on the other OSes' decoders where available (an AudioToolbox file on Linux's symphonia, a vo-aacenc file on MF and AudioToolbox).
- [ ] 6. `FIN_MEDIA_FIXTURES` rows with `FIN_REQUIRE_MEDIA=1`: MP3 (CBR, VBR with and without Xing), Vorbis, ALAC, an iPhone AAC-LC and an HE-AAC file (refused on Linux with the plugin fix, decoded elsewhere).
- [ ] 7. CI: one green run of `ci.yml` on all three OSes with the new rows.

### Task 24: Spec as built, notices and tracker — tier: sonnet — F

**Depends on:** Task 23.

**Files:** Modify `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` (status line; §8 "As built" notes; §7.1 proxy audio; §7.2 recipes and the `audioVersion` key param; §6.2 Linux-only AAC code; §16.2 versions: symphonia 0.6.1, `ebur128` 0.1.11, libopus 1.6.1, vo-aacenc at its commit; §19 6C's real dependency on 6B-1 Tasks 13–14; a new **§26 "6C as built"** with every Decision marked As built or Deviation, the spec-vs-code items of Decisions 2, 3, 7, 8, 13, 14, 15, the measured numbers of Task 23 and the residuals), `docs/specs/2026-11-01-finisher-architecture-design.md` §27 (a pointer), `THIRD_PARTY_NOTICES.md` (checked complete by the notices gate), `TASKS.md` (6C done; open residuals; the 8C note that `pcm16m` entries exist for its `media.pcm` host call; counsel item 6.1 unchanged).

- [ ] Docs; three gates → PASS. **Commit** `docs: sub-project 6C as built`.

---

## Owner questions

Each has a recommended default, and the plan uses it until the owner answers.

1. **Does adding the four audio effect kinds bump the model version?** Spec §4.4 says "format bump (new registry kinds)". The effect's stored shape does not change, but an older binary replaying an `effect.add eq` refuses it (`PARAM_UNKNOWN`), and `effect.add`'s domain rule changes.
   - **(a, recommended)** Yes, once: `SCHEMA_VERSION` = **integration's value + 1 at the moment Task 5 lands** (reservations by unmerged streams do not count; a stream that lands later renumbers), with the identity steps the upgrade runner needs and a version-only store migration; no data rewritten, no legacy rows (there are no users).
   - (b) No bump: older readers would then fail op by op instead of refusing the project up front.
2. **Which built-in effects, with which parameters?** The spec fixes the four kinds (§8.4); this plan fixes their parameters (Decision 13: an 8-band EQ with flat names `band<n>…`, compressor with RMS/peak detector and soft knee, gate with hold and range, de-esser as a split-band compressor).
   - **(a, recommended)** As Decision 13.
   - (b) Fewer EQ bands (4) for a smaller registry and UI; or add a `limiter` kind per item (the master limiter stays export-only either way).
3. **PCM feed transport for analyzers.** Sub-project 8's `media.pcm` host call (8C) needs the 16 kHz mono feed.
   - **(a, recommended)** Cache entries of 1 MiB `f32` little-endian chunks with `meta.json` (Decision 7); 8C copies the requested range into the plugin job's `in/` as one file (`fin-shm::write_input`), no ring. Simple, replayable, and the feed is small (64 KB per second).
   - (b) A `fin-shm` ring streamed by a media worker during the plugin call (lower latency for very long files, a live sibling worker per call).
4. **Real-time or offline in 6C?**
   - **(a, recommended)** Offline only (jobs), with a block-size-invariant streaming API that 6F wraps in the AudioWorklet (Decision 17).
   - (b) Also a native real-time output for desktop preview in 6C (needs an audio device crate and its licence review; spec puts preview in the browser).
5. **Normalisation defaults and the limiter.** 6E's templates name targets (spec §9.2: −14 LUFS / −1 dBTP for `social-9x16-1080p`).
   - **(a, recommended)** No default target in 6C: `fin listen` only reports; `normalise` takes the template's target; unreachable means silence or > 1 LU of limiting (spec §8.6), quantised gains of 0.01 dB; look-ahead 5 ms, release 50 ms.
   - (b) `fin listen --target <LUFS>` also reports the gain normalisation would apply (cheap to add; tell us if agents need it now).
6. **Where does Finisher's vo-aacenc fork live?** Spec §6.4 says "a git submodule of Finisher's own maintained fork". Creating a repository is an external action this plan does not take.
   - **(a, recommended)** You create the fork (for example beside the main repository on GitHub, mirrored to the local Gitea) at upstream commit `a277487e051e92e99a532294eed3c673f4d879f2`. **The fork is a prerequisite for merging Task 14**: its working branch may pin upstream at that commit, but `.gitmodules` must name the fork before Task 14 merges into `sp6c-audio` (spec §6.4 requires Finisher's maintained fork).
   - (b) Vendor the sources as plain files under `third_party/vo-aacenc` (no submodule; patches tracked in-tree).
7. **Finisher's own AAC code only in Linux builds?** Owner decision 1 is about Linux; compiling symphonia's AAC decoder and vo-aacenc into macOS and Windows builds would cover Windows N editions without the Media Feature Pack, at the cost of shipping AAC code on every OS.
   - **(a, recommended)** Linux only (Decision 9); Windows N gets the Media Feature Pack guidance, as for video.
   - (b) Every OS, as a fallback when the OS codec is missing (widens counsel item 6.1).
8. **Probing audio files in `fin-node` with symphonia's readers.** Shallow probes run in-process in `fin-node` today (MP4 through `fin-container`, WAV by hand).
   - **(a, recommended)** symphonia's format readers only (no codecs), safe Rust, inside `fin-probe`'s panic guard and bounded by the file length (Decision 14).
   - (b) Hand-written header readers per format (more code, no MPL in `fin-node`; MP3 duration still needs a frame scan).
   - (c) Probe audio-only containers in the worker (`media.probeDeep`), leaving `asset.add` without duration until it runs.

**Decided here, not asked:** the internal format and the resampler and WSOLA designs (the spec fixes them; Decisions 3, 5, 6 fill the numbers); `AUDIO_VERSION` in the audio recipes' key params (Decision 7); decoders are not cross-platform-normative (Decision 8); libopus as a float build without DNN or intrinsics (Decision 11); wave peak levels (Decision 19). Counsel item 6.1 (Linux AAC-LC decode and encode) stays open and blocks release, not this plan.
