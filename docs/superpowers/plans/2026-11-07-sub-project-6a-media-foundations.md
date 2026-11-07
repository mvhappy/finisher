# Sub-project 6A: Media foundations and first pixels — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (container parsing, the sampling rule, colour and geometry maths, the CPU reference, the GPU compositor, the worker lifecycle, tolerance tooling), `sonnet` for plumbing (crate skeletons, gates, bindings, RPC, CLI, CI, docs).

**Goal:** the first frames rendered from frame plans, on every OS, through the architecture the rest of sub-project 6 builds on:
- `fin look --at <t>` asks `finisherd`, which starts a `fin-worker` process, which evaluates the plan from a pinned snapshot, decodes AV1 sources in software, composites them on the GPU and writes PNGs;
- the same frames rendered by a CPU reference compositor match within tolerance T1, and the reference's own output is byte-identical on macOS, Windows, Linux and wasm32 (T0);
- probing tells the truth about MP4/MOV files: display size, rotation, colour tags, edit lists, VFR.

6A is **OS-neutral**: it uses only software codecs (`rav1d` decode, SVT-AV1 encode), so every OS runs the same path. Platform codecs are 6B.

**Spec:** `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` (cited as *spec §n*): §2–§5.2, §6.1–6.4, §10.2, §13, §15–§18, §19 row 6A, §20. Parent: `docs/specs/2026-11-01-finisher-architecture-design.md` §4 rule 3, §12.1–12.3, §27.2–27.3. Core: `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §5.4–5.6.

**Branch:** `sp6a-media-foundations`, from `sp4c-edit-breadth` (6A consumes 4C's eval, `fin-text` and `fin-fonts`). Rebase onto `main` when 4C merges.

**Architecture (6A slice):**
- **New crates:** `fin-media` (pure types, sampling rule, protocol, test patterns), `fin-container` (ISOBMFF demux, MP4 mux), `fin-codecs` (traits, AV1 software backend), `fin-compositor` (wgpu core, CPU reference, comparison), `fin-workers` (the `fin-worker` binary).
- **Changed:** `fin-probe` (media through `fin-container`), `fin-fonts` (read-only cache without the fetcher), `fin-node` (job preparation, media index), `fin-core` (event kinds, `Source::Worker`, base-box and transition rules in docs), `finisherd` (`src/media/`), `@finisher/contracts` (`media.*`), `fin` (`fin look`, `fin media caps`, `fin wait` for media jobs).
- **Not in 6A:** platform codecs, proxies, the cache manager, audio, effects, masks, captions raster, images, SVG, Lottie, export, `fin-wasm`. Layers of kinds other than `clip` (and clip sources that are not AV1) are reported in `skipped`, never silently dropped (spec §2.7).

**Tech stack:**
- Rust 2024; `wgpu` 30 (MIT OR Apache-2.0); `rav1d` 1.1 (BSD-2-Clause); SVT-AV1 C sources as a git submodule at a pinned release tag (BSD-3-Clause-Clear + AOMedia Patent License 1.0), built with `cmake`; `png` (MIT OR Apache-2.0); `memmap2` (MIT OR Apache-2.0); `libm` (already in tree); `pollster` (Apache-2.0 OR MIT) for blocking on wgpu futures in the worker.
- Each new crate is checked with `cargo deny` in the task that adds it; native sources go into `third_party/native.json` (Task 1).
- Node 26, Effect 4, vitest; CI adds `cmake` and `nasm` (Windows, Linux) and Mesa's lavapipe (Linux).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny, the JS licence gate and (from Task 1) the native-library gate.
- **Purity:** `fin-media` and `fin-container` have no I/O beyond the `Read + Seek` / `Write + Seek` they are given, no clocks, no randomness, and build for `wasm32-unknown-unknown` (added to `scripts/check-wasm-deps.mjs` in Task 1). `fin-compositor` builds for `wasm32-unknown-unknown` too (wgpu's WebGPU backend), checked by `cargo check` only in 6A.
- **Dependency rules** (spec §18):
  - `fin` links no media crate;
  - `fin-node` links `fin-media` and `fin-container`, never `fin-codecs`, `fin-compositor` or `wgpu`;
  - only `fin-workers` links `fin-compositor` and `fin-codecs`;
  - `fin-workers` links `fin-fonts` only with `default-features = false` (no HTTP, no TLS: workers never fetch).

  Task 1 enforces them with a new gate, `scripts/check-crate-deps.mjs` (over `cargo metadata`).
- **Primitive ops live only in `fin-core`.** 6A adds no op. Rules that `fin-core` documents (the base box, transition looks) are doc comments and constants in `eval/plan.rs`, not new ops.
- **TypeScript model types are generated, never hand-written.** Every wire type here (`MediaInfo`, the worker protocol, `LookResult`, capability types, new event payloads) derives `JsonSchema` and is added to `crates/fin-store/examples/export_schema.rs`; then `pnpm --filter @finisher/core-types generate`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Split by stage from the start (`fin-container/src/isobmff/{boxes,sample_table,edit_list,codec_config,colour}.rs`, `fin-compositor/src/{device,pool,ingest,layer,blend,readback}.rs`, `reference/{ingest,layer,blend}.rs`).
- Errors are `{ code, message, details, fix? }`. Every new code (spec §17) goes into `crates/fin/src/output.rs` with its exit code: `WORKER_*`, `GPU_*`, `JOB_CANCELLED`, `INPUT_POISONED` → 5 (unavailable, retry); `CODEC_UNAVAILABLE`, `UNSUPPORTED_*`, `MEDIA_CORRUPT` → 2 (never succeeds as sent); `PLAN_MISMATCH` → internal.
- **No media files are committed.** Fixtures are generated at test time (Task 3, Task 6). Real footage tests read `FIN_MEDIA_FIXTURES` and skip without it; `FIN_REQUIRE_MEDIA=1` on validation hosts makes them fail instead.

**Determinism**
- `fin-media` (`colour`, `sample`), `fin-compositor` (`geometry`, `reference`) use `f64`/`f32` arithmetic, `sqrt`, and `libm` for anything transcendental; no `mul_add`, no SIMD intrinsics. Task 8 extends `check-eval-floats.mjs` to refuse method-call transcendentals (`x.sin()`, `x.powf()`) in those modules while allowing `libm::` (the eval rules forbid `libm`; the render rules require it).
- `RENDER_VERSION` starts at 1 (spec §2.5). A test compares the T0 hashes (Task 8) with committed goldens, and its failure message says to bump `RENDER_VERSION` when the change is intended.

**Workers**
- Every worker is spawned from `childEnv()` (`apps/finisherd/src/env.ts`). Workers get no token and no network, and write only into their job's temp directory under `<server-data>/cache/media/tmp/<jobId>/`; the supervisor moves results.
- Test hooks that make a worker crash, hang or emit garbage are behind a `test-hooks` feature of `fin-workers`, never in a default build (`scripts/check-test-hooks.mjs` gains the crate).

**Testing and process**
- **GPU tests** pick an adapter with `FIN_GPU=auto|software|<backend>`. Without any adapter they print a notice and skip; `FIN_REQUIRE_GPU=1` (set in CI on Linux with lavapipe and on Windows with WARP) makes them fail instead. macOS CI uses the runner's Metal device if one is exposed, else skips; the validation task (Task 16) covers real GPUs.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh` (Node 26). Builds may set `CARGO_TARGET_DIR` to a large volume.
- **The repo is public:** no host names, IPs, user names or key names in tracked files. Cross-host notes stay in `archive/hosts.md`; benchmarks go in `archive/`.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 6A: media foundations", in the same commit. Task 1 adds the section.
- **Batches** (each green before the next):
  - A, crates and media types: Tasks 1–3;
  - B, containers, probe and codecs: Tasks 4–7;
  - C, compositor: Tasks 8–9;
  - D, workers and server: Tasks 10–14;
  - E, e2e, CI, docs and validation: Tasks 15–16.

## Decisions taken here

1. **Owner questions 4, 5 and 6 (spec §21) proceed on their recommendations** unless the owner answers otherwise before Batch C: the base box is "contain"; media jobs are built in `finisherd`; workers run without an OS sandbox until 6G. Each is isolated (one function, one directory, one task) so a different answer is a contained change.
2. **Snapshot format for jobs:** the JSON of `fin_core::Project` at `(branch, rev)`, exactly what `fin-node`'s existing `snapshot` returns, plus `{branch, rev}` beside it. No new format.
3. **Measurer in workers:** the worker builds the same `fin-text` measurer `fin-node` uses, over `fin_fonts::FontCache` opened read-only on the server's font cache plus the project's font assets (paths from the manifest). The plan samples (spec §3.4) prove the two agree.
4. **Plan samples:** BLAKE3 of the compact JSON of `FramePlan`, as the 4C corpus hashes it (`crates/fin-core/tests/eval_corpus.rs`), at every requested `fin look` time.
5. **Asset paths in 6A:** the manifest takes the model's `asset.path` as stored and checks it exists. The library-root mapping (arch §20) arrives with sub-project 5's libraries.
6. **Media index in 6A:** a JSON file per fingerprint under `<server-data>/cache/media/index/<fingerprint>.json`, written by `fin-node` at probe time. 6B replaces it with the SQLite index of spec §7.2 and migrates these files.
7. **`fin look` output:** PNGs under `<project>/workspace/shared/look/` named `<timeline>-<t as n_d>-<plan hash 8 hex>.png` (deterministic, so repeated looks reuse files), or `--out <dir>`. The answer lists paths, sizes, plan hashes and `skipped`.
8. **Sources in 6A:** AV1 in MP4/MOV only. Every other codec is `CODEC_UNAVAILABLE` per layer, in `skipped`.
9. **Transition looks:** `crossfade` mixes encoded values, `dissolve` linear light (spec §4.3), documented on `TransitionKind` in `fin-core` and implemented in both compositors.

## Review focus

1. **The sampling rule** (`frame_at`) with edit lists, B-frame reordering (`ctts`), VFR and out-of-range times. → Tasks 2, 4, property `frame_at_matches_linear_scan`.
2. **Container robustness:** every malformed input is a typed error, never a panic, a hang or an allocation proportional to a declared (not actual) size. → Task 4 fuzz target and limits.
3. **Rotation and display size** are right for 90/180/270° and non-square pixels. → Tasks 4, 5, 8.
4. **The CPU reference is an independent oracle:** it does not share shader code or texture logic with the GPU path, only `geometry` and `fin-media::colour`. → Task 8.
5. **T1 holds on every adapter** in CI (lavapipe, WARP, Metal if present). → Tasks 9, 15.
6. **Worker lifecycle:** no partial outputs after a kill; retry then poisoning; orphan exit; protocol mismatch. → Tasks 10, 11.
7. **Plan parity** between `fin-node` and the worker, including captions measured with real fonts. → Tasks 12, 15.
8. **Licence gate** covers SVT-AV1's native sources. → Tasks 1, 7.

---

### Task 1: Crates, gates and the tracker — tier: sonnet

**Files:**
- Create: `crates/fin-media/`, `crates/fin-container/`, `crates/fin-codecs/`, `crates/fin-compositor/`, `crates/fin-workers/` (each with `Cargo.toml` using the workspace package fields and `publish = false`, and a `lib.rs`/`main.rs` stub with its module doc from spec §18).
- Modify: root `Cargo.toml` (`members` already globs `crates/*`; add the new crates to `default-members`).
- Create: `third_party/native.json`, `scripts/check-native-deps.mjs`, `scripts/check-native-deps.test.mjs`. Modify: `scripts/gates.mjs`, `THIRD_PARTY_NOTICES.md` (heading for native libraries).
- Modify: `scripts/check-wasm-deps.mjs` (add `fin-media`, `fin-container`). Create: `scripts/check-crate-deps.mjs` and its test (the dependency rules of Global Constraints, over `cargo metadata`).
- Modify: `TASKS.md` (new section "Sub-project 6A: media foundations" listing Tasks 1–16).

**`third_party/native.json`:**
```json
{ "libraries": [
  { "name": "SVT-AV1", "path": "third_party/svt-av1", "tag": "v<x.y.z>", "commit": "<sha>",
    "license": "BSD-3-Clause-Clear", "patents": "AOMedia-Patent-License-1.0",
    "files": ["LICENSE.md", "PATENTS.md"], "builtBy": "crates/fin-codecs/build.rs" } ],
  "allow": ["BSD-2-Clause", "BSD-3-Clause", "BSD-3-Clause-Clear", "MIT", "ISC", "Zlib", "Apache-2.0"],
  "patentLicences": ["AOMedia-Patent-License-1.0", "WebM-Patent-Grant"] }
```

**Gate rules (`check-native-deps.mjs`):** every `build.rs` that names a path under `third_party/` must match a listed `builtBy`; every listed library's `path` must be a git submodule at `commit`; `license` must be in `allow`; each `files` entry must exist in the submodule; `THIRD_PARTY_NOTICES.md` must contain the library's name and tag. Its test feeds a fake tree with an unlisted `build.rs`, a GPL licence and a missing notice, and expects three failures.

- [ ] **Step 1:** Create the crates and the gates with their tests (the SVT-AV1 entry is added in Task 7; the gate passes on an empty list).
- [ ] **Step 2:** `rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates` → PASS. **Commit** `build: media engine crates, native-library and crate-dependency gates`.

### Task 2: `fin-media` types and the sampling rule — tier: opus

**Files:**
- Create: `crates/fin-media/src/{lib.rs, format.rs, colour.rs, info.rs, sample.rs, version.rs, error.rs}`, `crates/fin-media/tests/sample.rs`.
- Modify: `crates/fin-store/examples/export_schema.rs`; regenerate `@finisher/core-types`.

**Interfaces:**
```rust
pub const RENDER_VERSION: u32 = 1;

pub enum PixelFormat { Nv12, P010, I420, I420p10, I444, Rgba8, Rgba16f }
pub enum Primaries { Bt709, Bt601_625, Bt601_525, Bt2020, DisplayP3, Unknown }
pub enum Transfer { Bt709, Srgb, Pq, Hlg, Linear, Unknown }
pub enum Matrix { Bt709, Bt601, Bt2020Ncl, Identity, Unknown }
pub enum Range { Limited, Full }
pub enum ChromaSiting { Left, Center, TopLeft, Unknown }
pub struct ColourTags { pub primaries: Primaries, pub transfer: Transfer, pub matrix: Matrix,
                        pub range: Range, pub siting: ChromaSiting }
impl ColourTags { pub fn resolve_defaults(self, height: u32) -> ColourTags } // spec §10.1 untagged rule

pub enum Rotation { R0, R90, R180, R270 }
pub struct Size { pub width: u32, pub height: u32 }
pub struct VideoTrackInfo { pub codec: VideoCodec, pub profile: Option<u8>, pub level: Option<u8>,
    pub bit_depth: u8, pub chroma: Chroma, pub coded: Size, pub display: Size, pub rotation: Rotation,
    pub pixel_aspect: Rational, pub colour: ColourTags, pub hdr: Option<HdrInfo>,
    pub nominal_rate: Rational, pub vfr: bool, pub frames: u64 }
pub struct AudioTrackInfo { pub codec: AudioCodec, pub sample_rate: u32, pub channels: u16,
    pub priming: u32, pub padding: u32 }
pub struct MediaInfo { pub container: ContainerKind, pub duration: Rational,
    pub video: Option<VideoTrackInfo>, pub audio: Option<AudioTrackInfo> }

/// One frame in presentation order: `pts` in `timescale` ticks after edit lists.
pub struct FrameEntry { pub pts: i64, pub duration: u32, pub keyframe: bool,
                        pub decode_index: u32, pub offset: u64, pub size: u32 }
pub struct SampleTable { pub timescale: u32, pub frames: Vec<FrameEntry> } // sorted by pts
pub enum FrameAt { Frame(usize), BeforeStart, AfterEnd }
/// Spec §10.2: the last frame with pts ≤ s. Exact rational comparison; O(log n).
pub fn frame_at(table: &SampleTable, s: Rational) -> FrameAt;
/// The keyframe (in decode order) a decoder must start from to output frame `i`.
pub fn decode_start(table: &SampleTable, i: usize) -> usize;
```

- `display` is `coded` rotated by `rotation` and scaled by `pixel_aspect` (width scaled, rounded to even).
- `BeforeStart`/`AfterEnd` map to the first/last frame in the renderer, which then reports `SOURCE_OUT_OF_RANGE` (spec §10.2).

**Tests:** `frame_at` on CFR, VFR (random durations), a negative first pts (edit list offset), duplicates refused at construction (`MEDIA_CORRUPT`), times exactly on a boundary; property `frame_at_matches_linear_scan` (proptest, 256 cases); `decode_start` with open and closed GOPs; `resolve_defaults` (576 → BT.601, 720 → BT.709); `display` for each rotation and a 4:3 pixel aspect.

- [ ] **Step 1: Write** the tests (they fail), then the types and functions.
- [ ] **Step 2:** Schema export and generated types; three gates → PASS. **Commit** `feat(media): media info, colour tags and the frame sampling rule`.

### Task 3: Frame-id test patterns — tier: opus

**Files:**
- Create: `crates/fin-media/src/testing/{mod.rs, pattern.rs, read.rs}` behind a `testing` feature; `crates/fin-media/tests/pattern.rs`.

**Interfaces:**
```rust
pub struct Yuv420 { pub width: u32, pub height: u32, pub y: Vec<u8>, pub u: Vec<u8>, pub v: Vec<u8> }
/// A picture whose content varies with `index` (gradients, bars, a moving edge) and carries
/// `index` and a CRC-8 as a 4×8 grid of 2-level blocks in the top-left corner.
pub fn pattern(index: u32, width: u32, height: u32, seed: u64) -> Yuv420;
/// Reads the block code back from a decoded Y plane or an RGB(A) image; None if unreadable.
pub fn read_id_y(y: &[u8], width: u32, height: u32) -> Option<u32>;
pub fn read_id_rgba(rgba: &[u8], width: u32, height: u32, at: Rect) -> Option<u32>;
```
- Blocks are at least 16×16 px at 540p and scale with the height; the reader samples block centres and thresholds at mid-grey, so it survives AV1 at CRF 40 and limited-range conversion.
- `read_id_rgba` takes the rectangle where the source landed in a composited frame (from `geometry`, Task 8), so frame ids can be read from `fin look` output.

**Tests:** round trip for indices 0…10,000 (sampled) at 320×180, 960×540 and 1920×1080; a corrupted block fails the CRC; `pattern` is deterministic for a seed.

- [ ] **Step 1–2:** tests, implementation, gates → PASS. **Commit** `test(media): frame-id test patterns`.

### Task 4: ISOBMFF demuxer — tier: opus

**Files:**
- Create: `crates/fin-container/src/{lib.rs, error.rs, limits.rs}`, `crates/fin-container/src/isobmff/{mod.rs, reader.rs, boxes.rs, sample_table.rs, edit_list.rs, codec_config.rs, colour.rs, matrix.rs}`.
- Create: `crates/fin-container/tests/{demux.rs, malformed.rs}`, `crates/fin-container/fuzz/` (`cargo fuzz` target `demux`, not a workspace member).

**Interfaces:**
```rust
pub struct Demuxer<R: Read + Seek> { /* … */ }
impl<R: Read + Seek> Demuxer<R> {
    pub fn open(r: R, limits: &Limits) -> Result<Self, CoreError>;
    pub fn info(&self) -> &MediaInfo;
    pub fn video(&self) -> Option<&VideoTrack>;   // its SampleTable and CodecConfig
    pub fn audio(&self) -> Option<&AudioTrack>;
    /// Reads sample `decode_index` of a track into `buf` (resized).
    pub fn read_sample(&mut self, track: TrackRef, decode_index: u32, buf: &mut Vec<u8>) -> Result<(), CoreError>;
}
pub enum CodecConfig { Av1 { av1c: Vec<u8> }, Avc { avcc: Vec<u8> }, Hevc { hvcc: Vec<u8> },
                       Vp9 { vpcc: Vec<u8> }, Aac { esds: Vec<u8> }, Opus { dops: Vec<u8> }, Other { fourcc: [u8; 4] } }
pub struct Limits { pub max_boxes: u32, pub max_samples: u32, pub max_box_bytes: u64 } // defaults: 1M, 10M, 64 MiB
```

**Covers:**
- `ftyp`, `moov`/`mvhd`/`trak`/`tkhd`/`mdia`/`mdhd`/`hdlr`/`minf`/`stbl` (`stsd`, `stts`, `ctts` v0/v1, `stss`, `stsc`, `stsz`/`stz2`, `stco`/`co64`), `edts`/`elst` (v0/v1, empty edits, `media_rate` 1 only), fragmented files (`mvex`/`trex`, `moof`/`traf`/`tfhd`/`tfdt`/`trun`).
- The first video and first audio track (others ignored, listed in `info`).
- The `tkhd` matrix → `Rotation` (anything else `UNSUPPORTED_TRANSFORM`); `pasp`; `colr` (`nclx`, `nclc`; `prof`/`rICC` reported as ICC, primaries from it are 6B); `mdcv`, `clli`, `dvcC`/`dvvC` → `HdrInfo`; codec configuration boxes; `btrt` ignored.
- Edit lists applied to presentation times (spec §10.2): frames entirely outside the edit are dropped from the table, partially covered first frames kept with their pts clamped to the edit's start.
- **VFR:** `vfr = true` when sample durations differ by more than 0.1 % (ignoring the last sample); `nominal_rate` = the most common duration's rate.
- Audio priming from `elst` media time or `sgpd` `roll`, Opus pre-skip from `dOps`.

**Limits and safety:** box sizes checked against the file length before any allocation; declared counts bounded by `Limits` and by the bytes available; recursion depth ≤ 16; every error is `MEDIA_CORRUPT {at: offset, box}` or `UNSUPPORTED_CONTAINER`. No `unsafe`.

**Tests:**
- Synthetic files built by the Task 5 muxer *and* by a hand-written box builder in the test (so the demuxer is not only tested against our own writer): CFR, VFR, B-frames with `ctts`, an edit list with an empty edit, a fragmented file, 90/180/270° matrices, a 4:3 `pasp`, `colr` variants, an HLG file with `mdcv`/`clli`.
- `malformed.rs`: truncated at every box boundary of a small file, sizes past EOF, `co64` offsets past EOF, `stsz` counts of `u32::MAX`, a box loop — each a typed error within 50 ms.
- Real files from `FIN_MEDIA_FIXTURES` (skip without): an iPhone HEVC portrait clip (rotation 90, HLG, Dolby Vision 8.4), a screen recording (VFR), an edited export with an edit list.
- The fuzz target runs 60 s in a nightly CI job (Task 15), seeded with the synthetic files.

- [ ] **Step 1:** box builder and tests (fail); **Step 2:** demuxer; **Step 3:** fuzz target, 10 min local run with no findings; gates → PASS. **Commit** `feat(container): ISOBMFF demuxer with edit lists, fragments, rotation and colour boxes`.

### Task 5: MP4 muxer — tier: opus

**Files:**
- Create: `crates/fin-container/src/mux/{mod.rs, mp4.rs, boxes.rs}`, `crates/fin-container/tests/mux.rs`.

**Interfaces:**
```rust
pub struct Mp4Writer<W: Write + Seek> { /* … */ }
impl<W: Write + Seek> Mp4Writer<W> {
    pub fn new(w: W, opts: Mp4Options) -> Result<Self, CoreError>;     // faststart: reserve moov space
    pub fn add_video(&mut self, t: VideoTrackConfig) -> Result<TrackRef, CoreError>;
    pub fn add_audio(&mut self, t: AudioTrackConfig) -> Result<TrackRef, CoreError>;
    pub fn write(&mut self, track: TrackRef, data: &[u8], pts: i64, dts: i64, duration: u32, key: bool) -> Result<(), CoreError>;
    pub fn finish(self) -> Result<W, CoreError>; // writes moov first (into the reserve, or rewrites)
}
pub struct VideoTrackConfig { pub config: CodecConfig, pub coded: Size, pub timescale: u32,
    pub rotation: Rotation, pub pixel_aspect: Rational, pub colour: ColourTags, pub hdr: Option<HdrInfo> }
pub struct AudioTrackConfig { pub config: CodecConfig, pub sample_rate: u32, pub channels: u16, pub priming: u32 }
```
- Sample entries in 6A: `av01` (+`av1C`), `avc1`/`hvc1` (config passthrough, for 6B), `Opus` (+`dOps`), `mp4a` (+`esds`). `colr` nclx always written; `pasp` when not square; the matrix for rotation; an edit list for audio priming and for a first video pts ≠ 0.
- Test-only knobs (feature `testing`): write VFR durations, custom matrices, `ctts`, an empty edit, fragmented output — used by Task 4's and later tests to produce edge cases.

**Tests:** mux → demux round trip of every field; byte-identical output for identical input (determinism); moov before mdat; a 10,000-sample file.

- [ ] **Steps:** tests, implementation, gates → PASS. **Commit** `feat(container): MP4 muxer with fast start, colour, rotation and priming edit lists`.

### Task 6: Probe through `fin-container`; the media index — tier: sonnet

**Files:**
- Modify: `crates/fin-probe/src/lib.rs` (the media path), `crates/fin-probe/Cargo.toml` (drop `mp4`, add `fin-container`, `fin-media`), `crates/fin-probe/src/fixtures.rs` (MOV fixtures written by `fin-container`'s muxer).
- Create: `crates/fin-node/src/media/{mod.rs, index.rs}`; modify `crates/fin-node/src/lib.rs` (probe writes the index).
- Tests: `crates/fin-probe/tests/media.rs`, `crates/fin-node` unit tests for the index.

**Behaviour:**
- `Probe` (the model's `VideoInfo`/`AudioInfo`) is **unchanged in meaning** in 6A: width and height stay the coded track size, the rate the average (core-format change is 6B, spec §6.3). `Probe` gains an optional `media: MediaInfo` field (generated TS type updated) so callers can see the truth.
- `fin-node`'s probe writes `MediaInfo` to `<server-data>/cache/media/index/<fingerprint>.json` (temp-then-rename; Decision 6) and reads it back by fingerprint (`media_info(fingerprint)`).
- Existing probe tests must pass unchanged, except the ones pinning `mp4`-crate behaviour on broken files, which now expect `MEDIA_CORRUPT` or `UNSUPPORTED_CONTAINER` with the same `details.path` shape.

- [ ] **Steps:** move the media path, keep the old tests green, add tests for rotation, VFR and edit lists surfacing in `media`; gates; `rtk pnpm e2e` (the reel's probe goldens must not change) → PASS. **Commit** `feat(probe): media probing through fin-container; a media index keyed by fingerprint`.

### Task 7: Software AV1: `rav1d` decode, SVT-AV1 encode — tier: opus

**Files:**
- Create: `crates/fin-codecs/src/{lib.rs, traits.rs, frame.rs, reader.rs, caps.rs}`, `crates/fin-codecs/src/av1/{mod.rs, decode.rs, svt.rs, svt_ffi.rs}`, `crates/fin-codecs/build.rs`.
- Create: git submodule `third_party/svt-av1` at the latest release tag; entry in `third_party/native.json`; `THIRD_PARTY_NOTICES.md` (SVT-AV1 licence, AOMedia Patent License 1.0, `rav1d`).
- Create: `crates/fin-codecs/tests/{av1.rs, reader.rs}`, `crates/fin-codecs/examples/make_fixture.rs`.

**Interfaces:**
```rust
pub trait VideoDecoder: Send {
    fn send(&mut self, data: &[u8], pts: i64) -> Result<(), CoreError>;
    fn receive(&mut self) -> Result<Option<DecodedFrame>, CoreError>;
    fn flush(&mut self) -> Result<(), CoreError>;
    fn backend(&self) -> BackendId;
}
pub trait VideoEncoder: Send {
    fn send(&mut self, frame: &RawFrame, pts: i64, force_key: bool) -> Result<(), CoreError>;
    fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError>;
    fn finish(&mut self) -> Result<(), CoreError>;
    fn config(&self) -> CodecConfig; // av1C after the first packet
}
pub struct DecodedFrame { pub pts: i64, pub format: PixelFormat, pub size: Size, pub planes: [Plane; 3], pub colour: ColourTags }
pub fn open_decoder(track: &VideoTrack, caps: &Capabilities) -> Result<Box<dyn VideoDecoder>, CoreError>; // AV1 only in 6A; else CODEC_UNAVAILABLE
pub fn open_av1_encoder(cfg: &Av1EncoderConfig) -> Result<Box<dyn VideoEncoder>, CoreError>; // preset, crf, keyint, threads
/// Random access by presentation index: decodes from `decode_start`, caches the last GOP.
pub struct FrameReader<R: Read + Seek> { /* demuxer + decoder + table */ }
impl<R: Read + Seek> FrameReader<R> { pub fn frame(&mut self, index: usize) -> Result<&DecodedFrame, CoreError>; }
pub fn capabilities() -> Capabilities; // 6A: av1 decode sw, av1 encode sw, others none
```
- **Build:** `build.rs` runs `cmake` on `third_party/svt-av1` (encoder only, static, `-DBUILD_APPS=OFF -DBUILD_DEC=OFF`, `nasm` on x86_64) into `OUT_DIR`; a clear error names missing `cmake`/`nasm`. `rav1d` from crates.io with its `asm` feature on.
- **FFI:** only the encoder calls used (`svt_av1_enc_init_handle`, `_set_parameter`, `_init`, `_send_picture`, `_get_packet`, `_release_out_buffer`, `_deinit`, `_deinit_handle`), each wrapped once, no `unsafe` outside `svt_ffi.rs`.
- `make_fixture` writes a pattern clip (`--frames`, `--size`, `--rate`, `--vfr`, `--rotate`, `--keyint`) as AV1 in MP4, used by tests and e2e.

**Tests:** encode 120 pattern frames at 960×540, decode them back: frame ids exact, PSNR ≥ 35 dB on Y; `FrameReader` random access in shuffled order equals sequential decode (bit-exact planes); a truncated sample is `DECODE_FAILED` without panic; encoder determinism (same input and settings → identical bytes with 1 thread).

**Benchmark (archive only):** `rav1d` vs dav1d (`archive/sp6/av1-decode.md`): 1080p and 540p decode fps on each OS. If `rav1d` is more than 15 % slower or fails any test, record it and open the dav1d FFI follow-up in TASKS (spec §6.4).

- [ ] **Steps:** submodule and build, tests, implementation, `rtk cargo deny check`, native gate, three gates → PASS. **Commit** `feat(codecs): software AV1 decode (rav1d) and encode (SVT-AV1 from pinned source)`.

### Task 8: Geometry, colour and the CPU reference compositor — tier: opus

**Files:**
- Create: `crates/fin-compositor/src/{lib.rs, geometry.rs, plan_walk.rs, source.rs, output.rs}`, `crates/fin-compositor/src/reference/{mod.rs, ingest.rs, layer.rs, blend.rs}` (feature `reference`), `crates/fin-media/src/colour/convert.rs` (matrices and transfer functions shared by both compositors).
- Create: `crates/fin-compositor/tests/{geometry.rs, reference.rs}`, `crates/fin-compositor/tests/golden/t0.json` (BLAKE3 goldens), `crates/fin-compositor/tests/corpus/*.json` (plans + synthetic source descriptions).
- Modify: `crates/fin-core/src/eval/plan.rs` and `model/timeline.rs` (doc comments: the base box, transition looks; no code change), `scripts/check-eval-floats.mjs` and its test (render modules: refuse method transcendentals, allow `libm::`).

**Interfaces:**
```rust
/// Spec §5.1: the source's display size fitted inside the frame, centred ("contain").
pub fn base_box(display: Size, frame: Resolution) -> RectF64;
/// Item → frame affine matrix (row-major 3×3) from the plan's transform, in frame pixels with
/// the origin at the top-left; rotation degrees clockwise; `libm::sin/cos`.
pub fn layer_matrix(t: &PlanTransform, base: RectF64, frame: Resolution, rotation: Rotation) -> [f64; 9];
pub trait SourceProvider {
    /// The decoded frame for a layer, or why there is none (`CODEC_UNAVAILABLE`, `ORIGINALS_MISSING`, …).
    fn frame(&mut self, layer: &Layer) -> Result<SourceFrame<'_>, LayerIssue>;
}
pub struct Skipped { pub item: String, pub kind: ItemKind, pub code: String }
pub struct RenderOutput { pub size: Size, pub rgba8: Vec<u8>, pub skipped: Vec<Skipped> }
pub mod reference { pub fn render(plan: &FramePlan, src: &mut dyn SourceProvider, out: Size) -> RenderOutput; }
```

**Reference semantics (the oracle):**
- Ingest: limited/full range expansion, BT.601/709/2020 matrices, bilinear chroma upsampling with the source's siting, to `f32` RGB in display-encoded space; then premultiplied alpha 1.
- Per layer: crop in the base box, the matrix, bilinear sampling of the source at each output pixel centre (inverse mapping), opacity, blend into an `f32` RGBA accumulation buffer (premultiplied) with the seven modes (W3C compositing formulas), transitions per Decision 9.
- Output: clamp, round half to even to 8 bits, alpha 255 (opaque black background).
- Layers it cannot draw in 6A (non-clip kinds, `children` of nested items, effects, masks) are listed in `skipped` with `code: "LAYER_UNAVAILABLE"` and `details.reason: "notYet"`; effects and masks on a drawn clip are skipped the same way and named.

**Tests:**
- `geometry.rs`: contain for 16:9 in 9:16 and the reverse, rotations, anchors at corners, negative scale (mirroring), crop.
- `reference.rs`: ~15 corpus cases (single clip; opacity keyframe sample; each blend mode over a gradient; crossfade and dissolve at progress 0, 0.5, 1; rotated portrait source; transform off-frame; two tracks): output BLAKE3 equals `t0.json`; the same cases under `wasm32-wasip1` (wasmtime, as the 4C corpus does) give the same hashes.
- Frame ids are readable (`read_id_rgba`) from the composited output where the source is unrotated and at scale ≥ 0.5.

- [ ] **Step 1:** geometry tests and code; **Step 2:** reference tests (empty goldens print hashes), implementation, inspect 5 outputs by eye (written to `archive/sp6/reference/`), fill `t0.json` from a Linux run; **Step 3:** wasm32 run; float gate; three gates → PASS. **Commit** `feat(compositor): geometry, colour conversion and the CPU reference compositor`.

### Task 9: The wgpu compositor core and the T1 comparator — tier: opus

**Files:**
- Create: `crates/fin-compositor/src/{device.rs, pool.rs, ingest.rs, layer.rs, blend.rs, readback.rs, gpu.rs, compare.rs}`, `crates/fin-compositor/src/shaders/{ingest.wgsl, layer.wgsl, blend.wgsl}`.
- Create: `crates/fin-compositor/tests/gpu.rs`.

**Interfaces:**
```rust
pub enum AdapterChoice { Auto, Software, Backend(wgpu::Backends) } // from FIN_GPU
pub struct Gpu { /* device, queue, adapter info */ }
impl Gpu { pub fn new(choice: AdapterChoice) -> Result<Gpu, CoreError>; pub fn info(&self) -> AdapterSummary; } // GPU_UNAVAILABLE
pub struct Compositor { /* pipelines, texture pool */ }
impl Compositor {
    pub fn new(gpu: &Gpu) -> Compositor;
    pub fn render(&mut self, gpu: &Gpu, plan: &FramePlan, src: &mut dyn SourceProvider, out: Size) -> Result<RenderOutput, CoreError>;
}
pub struct AdapterSummary { pub backend: String, pub device_type: String, pub vendor: u32, pub device: u32, pub software: bool }
pub mod compare { pub struct T1 { pub psnr: f64, pub max_abs: u8, pub over_2: f64 } pub fn t1(a: &[u8], b: &[u8]) -> T1; pub fn passes(t: &T1) -> bool; }
```
- **Pipeline (spec §5.1, 6A subset):** upload Y/U/V planes as `r8unorm`/`rg8unorm` textures (copy path); ingest shader → `rgba16float` source texture; layer shader samples with the inverse matrix (bilinear, clamp-to-border transparent) and the crop, writes to a scratch target; blend shader reads the accumulation target and the scratch and writes the other ping-pong target; output pass converts to `rgba8unorm`; readback with 256-byte row alignment.
- Uniforms are `f32`; matrices come from `geometry` (Rust), never computed in WGSL.
- Same `skipped` behaviour as the reference.
- `device.lost` → `GPU_DEVICE_LOST`; allocation failure → `GPU_OUT_OF_MEMORY` (wgpu error scopes around each frame).

**Tests:** every Task 8 corpus case rendered on the GPU passes `compare::passes` against the reference (T1: ≥ 99.9 % of channels within 2/255, max ≤ 8/255, PSNR ≥ 50 dB); a failing case writes both images and a diff to `target/fin-compositor-t1/` for inspection; pipelines are created once per (stage, mode) (counted); 100 consecutive renders do not grow the texture pool beyond its first frame's set.

- [ ] **Steps:** tests, shaders and code, run on the local GPU and on the software adapter (`FIN_GPU=software`), three gates → PASS. **Commit** `feat(compositor): wgpu compositor core with ping-pong blending and the T1 comparator`.

### Task 10: The worker binary and protocol — tier: opus

**Files:**
- Create: `crates/fin-media/src/protocol/{mod.rs, jobs.rs, messages.rs}` (types; generated TS), `crates/fin-workers/src/{main.rs, io.rs, watchdog.rs, runner.rs, look.rs, sources.rs, events.rs}`, `crates/fin-workers/src/hooks.rs` (feature `test-hooks`).
- Modify: `crates/fin-fonts/Cargo.toml` and `src/lib.rs` (feature `fetch`, default on, gating `ureq`, `rustls`, `Fetcher` and `download`; `FontCache` and the catalog types build without it).
- Create: `crates/fin-workers/tests/{protocol.rs, look.rs, lifecycle.rs}`.

**Protocol types** (spec §3.3): `ToWorker::{Config, Job, Cancel, Shutdown}`, `FromWorker::{Hello, Progress, Heartbeat, Frame, Event, Result}`, serde tag `"type"`, camelCase. `PROTOCOL: u32 = 1`.

**`LookSpec`:** `{ snapshot: path, branch, rev, manifest: path, timeline, at: [Rational], size?: Size, planSamples: [{at, blake3}], outDir: path }` → result `{ frames: [{at, file, width, height, plan}], skipped: [Skipped], adapter: AdapterSummary }`.

**Worker loop:**
- Writes `hello` at start; reads one message per line; refuses a second `job` while busy (`WORKER_PROTOCOL`).
- A heartbeat thread sends `heartbeat` every 500 ms while a job runs.
- The watchdog thread exits the process (`std::process::exit(70)`) on stdin EOF or when the parent pid is gone (Unix: `getppid()` changed; Windows: `WaitForSingleObject` on a handle to the parent opened at start).
- `look`: read snapshot and manifest; build the measurer (Decision 3); for each time: `eval_frame`; compare its BLAKE3 with the plan sample (`PLAN_MISMATCH` on a difference); render with `Compositor` and a `SourceProvider` over `FrameReader`s (one per asset, opened lazily, `frame_at` per layer); encode PNG (`png` crate, no compression tuning, deterministic) into `outDir`; `progress` per frame.
- Panics are caught at the job boundary, reported as `result.error` `INTERNAL` and as an `error.panic` `event`, then the worker exits (a panicked worker is not reused).
- `test-hooks`: `FIN_WORKER_HOOK=crash|hang|garbage|slowHeartbeat` at a named step.

**Tests:**
- `protocol.rs`: every message round-trips; unknown `type` is refused; a JSON line over 1 MiB is refused.
- `look.rs`: a fixture project (built in the test with `fin-core` ops on a `make_fixture` AV1 asset) rendered at 5 times: PNGs decode to the T1-reference within tolerance; frame ids read back equal `frame_at`'s choice; a wrong plan sample → `PLAN_MISMATCH`; an H.264 asset → `skipped` `CODEC_UNAVAILABLE`.
- `lifecycle.rs` (spawns the built binary with `test-hooks`): stdin closed → exits within 1 s; `hang` → no heartbeat; `crash` mid-PNG → no file in `outDir` except temp names.

- [ ] **Steps:** protocol types and schema; `fin-fonts` feature split (its tests still pass with and without `fetch`); worker; tests; three gates → PASS. **Commit** `feat(workers): fin-worker with the NDJSON protocol, watchdogs and the look job`.

### Task 11: The media supervisor and media jobs in `finisherd` — tier: opus

**Files:**
- Create: `apps/finisherd/src/media/{supervisor.ts, pool.ts, jobs.ts, protocol.ts, sweep.ts, index.ts}`, `apps/finisherd/test/media-supervisor.test.ts`, `apps/finisherd/test/media-jobs.test.ts`.
- Modify: `apps/finisherd/src/main.ts` (start and stop the supervisor), `apps/finisherd/src/config.ts` (`FIN_WORKER_BIN` override for development and tests; default: `fin-worker` beside the `finisherd` entry point / in the app's resources).

**Behaviour** (spec §3.2, §3.4):
- `Supervisor` spawns workers with `childEnv()`, `stdio: ["pipe", "pipe", "pipe"]`, `windowsHide: true`; parses NDJSON with the generated protocol types (validated with the Effect schemas generated from them, or a thin decoder over the generated TS types; no hand-written shapes).
- **Pools:** `interactive` (min 0, max 2, kept warm 5 minutes) and `batch` (max = `min(4, cores / 2)` in 6A). A worker serves one job at a time; recycled after 50 jobs, 30 minutes, or 4 GiB resident (the worker reports its own resident size in `heartbeat.rss`; Node cannot read a child's memory).
- **Timeouts:** 10 s without a message; the job's deadline; `cancel` → 2 s grace → `kill("SIGKILL")` (Node maps it to `TerminateProcess` on Windows).
- **Retry and poisoning:** one retry on `WORKER_CRASHED`/`WORKER_TIMEOUT`/`GPU_DEVICE_LOST`; then failed with `details.attempts`; a second crash on the same `(input fingerprint, recipe)` records it in `<server-data>/media-jobs/poisoned.json` and further jobs fail fast with `INPUT_POISONED` until the fingerprint or `RENDER_VERSION` changes.
- **Jobs:** `{id (ULID), kind, state: queued|running|done|failed|cancelled, progress, result?, error?, attempts, createdAt, endedAt}`, persisted as one JSON file per job under `<server-data>/media-jobs/` (temp-then-rename), loaded at start (running ones become `failed` with `WORKER_CRASHED`, reason `serverRestart`), pruned after 7 days.
- **Sweep at start:** kill pids left in `cache/media/workers.json` whose start time matches; delete `cache/media/tmp/*` older than the server start.
- **Commit of outputs:** for `look`, the supervisor renames each PNG from the job's temp directory to the requested directory, refusing destinations inside `.finisher/` or `assets/` (`EXPORT_DESTINATION`).

**Tests** (vitest, with a real `fin-worker` built with `test-hooks`; skipped with a notice when `FIN_WORKER_BIN` is unset locally, required in CI): spawn and `hello`; protocol mismatch kills; `hang` → `WORKER_TIMEOUT` then retry; two crashes → `INPUT_POISONED`; cancel mid-job; a server restart marks the running job failed; outputs appear only after `result.ok`; the env passed to the child contains only allow-listed keys.

- [ ] **Steps:** tests, implementation, `rtk pnpm test`, three gates → PASS. **Commit** `feat(server): media supervisor, worker pools and persisted media jobs`.

### Task 12: Job preparation in `fin-node` — tier: sonnet

**Files:**
- Modify: `crates/fin-node/src/media/{mod.rs, prepare.rs}`, `crates/fin-node/src/lib.rs` (binding), `packages/fin-node` (TS declaration).
- Tests: `crates/fin-node` unit tests; `apps/finisherd/test/media-prepare.test.ts`.

**Interface (napi, async on the engine's worker thread):**
```ts
prepareMediaJob(projectId: string, branch: string, timeline: string, at: string[] /* rationals */, dir: string)
  : Promise<{ snapshot: string; manifest: string; rev: number; planSamples: { at: string; blake3: string }[] }>
```
- Writes `snapshot.json` (Decision 2) and `manifest.json` (spec §4.2; Decision 5: `original` = the stored path if it exists, else `null`; `media` from the index; fonts: the cache root plus the project's font asset paths) into `dir`.
- Computes `eval_frame` at each time with the shared measurer and returns the hashes (Decision 4).
- Errors: `NOT_FOUND` (timeline), `INVALID_RANGE`, and the eval errors unchanged.

**Tests:** the manifest lists every asset the plans reference and nothing else; a missing original is `null`; hashes equal a direct `eval_frame` in Rust; a snapshot round-trips into `fin_core::Project` equal to the engine's state.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(node): prepare media jobs: snapshot, render manifest and plan samples`.

### Task 13: `media.*` RPCs and `fin look` — tier: sonnet

**Files:**
- Modify: `packages/contracts/src/rpc.ts` (+ tests), `apps/finisherd/src/handlers.ts`, `apps/finisherd/src/media/look.ts` (new).
- Create: `crates/fin/src/commands/look.rs`, `crates/fin/src/commands/media_jobs.rs`; modify `crates/fin/src/commands/mod.rs`, `crates/fin/src/registry.rs`, `crates/fin/src/output.rs` (new codes and exit codes).
- Tests: `apps/finisherd/test/media-look.test.ts`, `crates/fin/src/commands/tests.rs` (parsing, echo), RPC fixtures (`scripts/capture-rpc-fixtures.ts`).

**RPCs:**
- `media.look {projectId, branch?, timeline?, at: Rational[1..64], out?, size?}` → `LookResult` (Task 10's result with final paths) — runs on the `interactive` pool and waits for the job.
- `media.jobs.get {job}`, `media.jobs.list {state?, limit?}`, `media.jobs.cancel {job}`.
- `media.capabilities {}` → `{codecs: Capabilities, gpu: AdapterSummary | null, worker: {version, renderVersion, protocol}}` (asks an interactive worker once per server run and caches the answer).

**`fin`:**
- `fin look --at <t>[,<t>…] [--timeline <id>] [--out <dir>] [--size WxH]`: times in every `fin` time form (`12.4s`, `00:12.4`, `f300`), echoed resolved; output `{frames: [...], skipped: [...]}`; exit 0 with a non-empty `skipped` (it is information, spec §2.7).
- `fin media caps`; `fin wait <job>` gains media job ids (`media.jobs.get` polling every 250 ms until done; prints the result).
- `fin` stays free of media crates (gate from Task 1).

**Tests:** contracts envelope test lists the new RPCs; `fin look` against a live server renders 3 times of a fixture project; `--at` past the timeline end is `INVALID_RANGE` from eval; an unknown timeline is `NOT_FOUND`; `skipped` reported for a caption item.

- [ ] **Steps:** tests, implementation, fixtures, three gates, `rtk pnpm e2e` → PASS. **Commit** `feat(fin): fin look, fin media caps and media jobs over media.* RPCs`.

### Task 14: Diagnostics kinds for the media engine — tier: sonnet

**Files:**
- Modify: `crates/fin-core/src/events/{mod.rs, kinds.rs}`, `crates/fin-core/src/events/payloads/` (new `media.rs`), the kind registry tests; `crates/fin-workers/src/events.rs`; `apps/finisherd/src/media/supervisor.ts` (forwarding).
- Regenerate types.

**Kinds** (spec §15): `media.capabilities` v1, `media.job` v1, `worker.exit` v1, `render.stats` v1 (per-minute histograms of decode, composite and readback micros by backend), `preview.stats` v0 and `render.sample` v0 (`training_only`) reserved. `Source::Worker` added; `media.job`, `worker.exit` and `render.stats` list `[Finisherd, Worker]`, `media.capabilities` `[Worker]`.
- Every field `meta` except `error.panic`'s existing content field; codec and backend names come from `fin-media` static tables; GPU vendor and device ids are included only if owner question 7 is answered yes, else the fields are absent (`Option`), decided by a constant in `fin-media`.
- **Forwarding:** the supervisor sends a worker's `event` to the journal with `source: "worker"`, through the journal binding from the diagnostics plan's D4. **If D4 has not merged when this task runs,** the task adds the kinds and the worker side only, keeps forwarding behind a `TODO(D4)` that is a TASKS follow-up, and says so in its commit.
- The `config` message carries the gate (`diagnostics: bool`); with it false the worker builds no payload (a test counts zero `event` lines).

**Tests:** the registry's schema tests (no free-form `meta` string); payload round trips; `strip_content` on the new kinds; the worker emits `media.job` for a look job; the supervisor emits `worker.exit` for a killed worker.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(core): media engine diagnostics kinds and the worker source`.

### Task 15: e2e scenario and CI — tier: sonnet

**Files:**
- Create: `scripts/e2e/look.mjs`, `scripts/e2e/look-golden/{plans.json, t0.json}`; modify `scripts/e2e.mjs`.
- Modify: `.github/workflows/ci.yml`: install `cmake` and `nasm` (pinned actions or the runner's package manager), Mesa lavapipe on ubuntu (`mesa-vulkan-drivers`), `FIN_REQUIRE_GPU=1` on ubuntu and windows, `git submodule update --init --depth 1 third_party/svt-av1`; a nightly job running the `fin-container` fuzz target for 60 s. Every action pinned to a commit SHA.

**Scenario `look`:**
1. `cargo run -p fin-codecs --example make_fixture` writes two AV1 clips (540p30 CFR, and a 540p VFR rotated 90° clip) into the e2e temp folder.
2. `fin project create`, `fin asset add` both, two clips on V1 with a crossfade, the rotated clip on V2 with keyframed opacity and a `multiply` blend, a caption from a word list (to check `skipped`).
3. `fin look --at 0,0.5,1.0,1.5,2.0` → PNGs.
4. Checks: plan hashes against `plans.json`; frame ids read from each PNG equal `frame_at`'s choice; each PNG passes T1 against `cargo run -p fin-compositor --example reference_render` over the same job files (kept by `FIN_KEEP_JOB_DIRS=1`), whose hashes must equal `t0.json`; `skipped` lists the caption with `notYet`.
5. Kill test: `FIN_WORKER_HOOK=crash` on the second look → `WORKER_CRASHED` retried, then success; no stray files.

- [ ] **Steps:** scenario, goldens blessed locally (`E2E_LOOK_BLESS=1`, refused under CI like the reel's), CI changes, a green CI run on all three OSes → **Commit** `test(e2e): fin look scenario; ci: media build dependencies, lavapipe and nightly fuzzing`.

### Task 16: Spec, tracker and cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

**Files:**
- Modify: `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` (status line, "as built" notes for §3, §6.3, §6.4, §13.2 with measured T1 numbers per adapter), `docs/specs/2026-11-01-finisher-architecture-design.md` §27 (a pointer item for 6A), `TASKS.md`.
- Archive only: `archive/sp6/6a-validation.md` (numbers, hosts by role, never by name).

**Validation rows** (each on macOS arm64, Windows 11 x64 with a discrete GPU, Linux x64 with an Intel iGPU, plus the software adapters):
1. `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` green.
2. T0 hashes identical on all three OSes and wasm32.
3. T1 per adapter: PSNR, max |Δ| and the share of channels over 2/255, for the corpus and the e2e frames; the worst case named.
4. `fin look` latency: cold and warm, p50/p95 over 50 runs at 1080p (budget spec §14: warm ≤ 300 ms p95 — from originals in 6A, which is the worse case).
5. Worker spawn to `hello` p95 (budget ≤ 150 ms).
6. `rav1d` vs dav1d decision recorded (Task 7).
7. Real-footage probe rows from `FIN_MEDIA_FIXTURES`: an iPhone portrait HEVC clip reports rotation 90, HLG and Dolby Vision 8.4; a screen recording reports VFR; an edit-list export reports the trimmed start.

- [ ] **Steps:** run the rows, record them, update the spec and tracker, decide follow-ups → **Commit** `docs: 6A as built and validated`. **6A is done** when every row passes or has an accepted, recorded exception.

---

## Questions for the owner

6A proceeds on the recommendations of spec §21 questions 4, 5 and 6 (Decision 1) and records question 7's answer as a constant (Task 14). Questions 1, 2, 3 and 8 do not block 6A (they decide 6B, 6C, 6E and 6F) but should be answered before 6B's plan is written.
