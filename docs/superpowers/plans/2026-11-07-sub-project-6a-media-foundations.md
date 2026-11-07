# Sub-project 6A: Media foundations and first pixels — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (container interpretation, the sampling rule, colour and geometry maths, the CPU reference, the GPU compositor, the mapped-file channel, the media worker family, tolerance tooling), `sonnet` for plumbing (crate skeletons, gates, bindings, RPC, CLI, CI, docs).

**Revision 2** (2026-11-07), after the review of `e6f7e64`: sub-project 5C owns jobs and the worker binary, so this plan has no supervisor or job store of its own; the sampling rule is the nearest frame; the plan is split into **6A-1** (Tasks 1–8, ready now) and **6A-2** (Tasks 9–16, after 5C). Spec §24 maps every finding. **Owner decisions of 2026-11-07** (spec §21) are applied: contain is the base box, 5C-1 runs alongside 6A-1, GPU ids are recorded as numeric `meta`, and the repository targets Node 24.

**Goal:** the first frames rendered from frame plans, on every OS, through the architecture the rest of sub-project 6 builds on:
- `fin look --at <t>` makes `finisherd` submit a `media.look` job through 5C's `JobEngine`; 5C runs it on a warm `interactive:media` worker (`fin-workers media`), which evaluates the plan from a pinned snapshot, decodes AV1 sources in software, composites them on the GPU and writes PNGs;
- the same frames rendered by a CPU reference compositor match within tolerance T1, and the reference's own output is byte-identical on macOS, Windows, Linux and wasm32 (T0);
- probing tells the truth about MP4/MOV files: display size, rotation, colour tags, edit lists, VFR.

6A is **OS-neutral**: it uses only software codecs (`rav1d` decode, SVT-AV1 encode), so every OS runs the same path. Platform codecs are 6B.

**Spec:** `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` (cited as *spec §n*): §2–§5.2, §6.1–6.4, §10.2, §13, §15–§18, §19 rows 6A-1 and 6A-2, §20. Sub-project 5 (*cp §n*, branch `sp5-design`): §6 (5C jobs). Parent: `docs/specs/2026-11-01-finisher-architecture-design.md` §4 rule 3, §12.1–12.3, §27.2–27.3. Core: `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §5.4–5.6.

**Branches:** `sp6a1-media-foundations` from `sp4c-edit-breadth` (6A consumes 4C's eval, `fin-text` and `fin-fonts`); `sp6a2-first-pixels` stacked on 6A-1 and on 5C-1's branch (cp §6.8: the engine, `fin-workers` with the `test` family, `jobhost`, warm workers, mapped files and helpers, the `JobEngine` API). Rebase onto `main` as 4C and 5C merge.

**Architecture (6A slice):**
- **New crates:** `fin-media` (pure types, the sampling rule, job params and results, test patterns), `fin-container` (sample tables, edit lists, fragments and limits over `mp4-atom`; MP4 mux), `fin-codecs` (traits, the AV1 software backend), `fin-compositor` (CPU reference, wgpu core, comparison), `fin-shm` (the mapped-file channel).
- **Into 5C's crates:** the `media` family in `fin-workers` (`crates/fin-workers/src/media/`), the `media.look` kind in `finisherd` (`apps/finisherd/src/jobs/kinds/media.look.ts`).
- **Changed:** `fin-probe` (media through `fin-container`), `fin-fonts` (read-only cache without the fetcher), `fin-node` (media job inputs), `fin-core` (docs for the base box and transition looks), `@finisher/contracts` (`media.look`, `media.capabilities`), `fin` (`fin look`, `fin media caps`).
- **Not in 6A:** platform codecs, proxies, the cache and its index, audio, effects, masks, captions raster, images, SVG, Lottie, export, `fin-wasm`. Layers of kinds other than `clip` (and clip sources that are not AV1) are reported in `skipped`, never silently dropped (spec §2.7).

**Tech stack:**
- Rust 2024; `wgpu` 30 (MIT OR Apache-2.0); `rav1d` 1.1 (BSD-2-Clause); `mp4-atom` 0.17 (MIT OR Apache-2.0); SVT-AV1 C sources as a git submodule at a pinned release tag (BSD-3-Clause-Clear + AOMedia Patent License 1.0), built with `cmake`; `png` (MIT OR Apache-2.0); `memmap2` (MIT OR Apache-2.0); `libm` **pinned `=0.2.16`** (the version 4C resolves); `pollster` (Apache-2.0 OR MIT) to block on wgpu futures in the worker.
- Each new crate is checked with `cargo deny` in the task that adds it; native sources and run-time-loaded libraries go into `third_party/native.json` and `third_party/runtime.json` (Task 1).
- Node 24 (owner decision 2026-11-07, matching Electron's embedded Node; until TASKS.md's "switch repo Node 26 → 24" item lands, the version `.superpowers/env.sh` pins), Effect 4, vitest; CI adds `cmake` and `nasm` (Windows, Linux) and Mesa's lavapipe (Linux).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny, the JS licence gate and (from Task 1) the native build gate and the run-time library gate (spec §16.3).
- **Purity:** `fin-media` and `fin-container` have no I/O beyond the `Read + Seek` / `Write + Seek` they are given, no clocks, no randomness, and build for `wasm32-unknown-unknown` (added to `scripts/check-wasm-deps.mjs` in Task 1). `fin-compositor` builds for `wasm32-unknown-unknown` too (wgpu's WebGPU backend), checked by `cargo check` only in 6A.
- **Dependency rules** (spec §18), enforced by a new gate `scripts/check-crate-deps.mjs` over `cargo metadata`:
  - `fin` links no media crate;
  - `fin-node` links `fin-media` and `fin-container`, never `fin-codecs`, `fin-compositor`, `fin-shm` or `wgpu`;
  - only `fin-workers` links `fin-compositor` and `fin-codecs`;
  - `fin-workers` links `fin-fonts` only with `default-features = false` (no HTTP, no TLS: workers never fetch).
- **No job machinery here.** Spawning, the kill tree, liveness, retries, poison handling, job persistence, `jobs.*` and `fin wait` are 5C's (cp §6). This plan adds a family and a kind; if a needed 5C feature is missing, the task stops and records it in `TASKS.md` instead of building a local substitute.
- **Workers never start processes** (spec §2.9). A test in Task 11 checks that the `media` family starts no child process.
- **Primitive ops live only in `fin-core`.** 6A adds no op. Rules that `fin-core` documents (the base box, transition looks) are doc comments and constants in `eval/plan.rs`, not ops.
- **TypeScript model types are generated, never hand-written.** Every wire type here (`MediaInfo`, media job params and results, `LookResult`, capability types, the `job.done` media fields) derives `JsonSchema` and is added to `crates/fin-store/examples/export_schema.rs`; then `pnpm --filter @finisher/core-types generate`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Split by stage from the start (`fin-container/src/{table,edit_list,fragment,limits,codec_config,colour,matrix}.rs`, `fin-compositor/src/{device,pool,ingest,layer,blend,coverage,mips,readback}.rs`, `reference/{ingest,layer,blend,coverage,mips}.rs`).
- Errors are `{ code, message, details, fix? }`. Every new code (spec §17) goes into `crates/fin/src/output.rs` with its exit code: `CODEC_UNAVAILABLE`, `UNSUPPORTED_*`, `MEDIA_CORRUPT`, `MEDIA_LIMIT` → 2; `GPU_*`, `WORKER_OOM`, `SHM_REFUSED` → 5; `PLAN_MISMATCH` → internal. 5C's codes keep 5C's exit codes.
- **No media files are committed.** Fixtures are generated at test time (Tasks 3, 6). Real-footage tests read `FIN_MEDIA_FIXTURES` and skip without it; `FIN_REQUIRE_MEDIA=1` on validation hosts makes them fail instead.
- **`Probe.media` never reaches an `asset.add` record** (spec §4.4); Task 8 tests it.

**Determinism**
- `fin-media` (`colour`, `sample`), `fin-compositor` (`geometry`, `coverage`, `mips`, `reference`) use `f64`/`f32` arithmetic, `sqrt`, and the pinned `libm` for anything transcendental; no `mul_add`, no SIMD intrinsics. Task 7 extends `check-eval-floats.mjs` to refuse method-call transcendentals (`x.sin()`, `x.powf()`) in those modules while allowing `libm::` (the eval rules forbid `libm`; the render rules require it).
- **No NaN:** every division by alpha is `if a > 0.0 { c / a } else { 0.0 }`; reference output buffers are checked for NaN in debug builds.
- **The sampling rule is the nearest frame, ties to the earlier frame** (spec §10.2). Every test that expects a frame id computes it through `frame_at`; corpora include times exactly at midpoints and one tick either side.
- `RENDER_VERSION` starts at 1 (spec §2.5). A test compares the T0 hashes (Task 7) with committed goldens; its failure message says to bump `RENDER_VERSION` when the change is intended.

**Testing and process**
- **GPU tests** pick an adapter with `FIN_GPU=auto|software|<backend>`. Without any adapter they print a notice and skip; `FIN_REQUIRE_GPU=1` (set in CI on Linux with lavapipe and on Windows with WARP) makes them fail instead. macOS CI uses the runner's Metal device if one is exposed, else skips; the validation task (Task 16) covers real GPUs.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh` (Node 24 once the switch item lands). Builds may set `CARGO_TARGET_DIR` to a large volume.
- **The repo is public:** no host names, IPs, user names or key names in tracked files. Cross-host notes stay in `archive/hosts.md`; benchmarks go in `archive/`.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 6A", in the same commit. Task 1 adds the section.
- **Batches** (each green before the next):
  - **6A-1** (ready now; needs nothing from 5C):
    - A, crates, gates and media types: Tasks 1–3;
    - B, containers and codecs: Tasks 4–6;
    - C, the CPU reference and probing: Tasks 7–8.
  - **6A-2** (after 5C-1; Tasks 9 and 10 may start earlier since they do not touch jobs):
    - D, GPU compositor and the mapped-file channel: Tasks 9–10;
    - E, the media family, job inputs and `fin look`: Tasks 11–14;
    - F, e2e, CI, docs and validation: Tasks 15–16.

## Decisions taken here

1. **Owner decisions 4, 5 and 6 (spec §21, 2026-11-07):** the base box is "contain" (decision 4); media workers run with 5C's cleared environment and no OS sandbox in 6A, the Linux Landlock profile is 6B-1 and the full `worker` profile 6G (decision 6, spec §3.5.1); 5C-1 runs alongside 6A-1, which shapes the split above (decision 5).
2. **Snapshot format for jobs:** the JSON of `fin_core::Project` at `(branch, rev)`, exactly what `fin-node`'s existing `snapshot` returns, plus `{branch, rev}` beside it. No new format.
3. **Measurer in workers:** the worker builds the same `fin-text` measurer `fin-node` uses, over `fin_fonts::FontCache` opened read-only on the server's font cache plus the project's font assets (paths from the manifest). The plan samples (spec §3.3) prove the two agree.
4. **Plan samples:** BLAKE3 of the compact JSON of `FramePlan`, as the 4C corpus hashes it (`crates/fin-core/tests/eval_corpus.rs`), at every requested `fin look` time.
5. **Asset paths in 6A:** the manifest takes the model's `asset.path` as stored and checks it exists. The library-root mapping (arch §20) arrives with 5D's libraries.
6. **No media index:** the worker parses each source's container when it opens it (milliseconds) and a warm worker keeps the result in memory, keyed by fingerprint and file size and mtime. The SQLite cache index is 6B (spec §7.2).
7. **`fin look` output:** PNGs under `<project>/workspace/shared/look/` named `<timeline>-<t as n_d>-<plan hash 8 hex>.png` (deterministic, so repeated looks reuse files), or `--out <dir>`. The answer lists paths, sizes, plan hashes and `skipped`.
8. **Sources in 6A:** AV1 in MP4/MOV only. Every other codec is `CODEC_UNAVAILABLE` per layer, in `skipped`.
9. **Transition looks:** `crossfade` mixes encoded values, `dissolve` linear light (spec §4.3), documented on `TransitionKind` in `fin-core` and implemented in both compositors.
10. **`mp4-atom` under limits:** `fin-container` reads each box header itself (size, type, extended size), checks the size against the bytes left and the box's cap, and only then hands the bytes to `mp4-atom`. Counts inside sample-table boxes are checked against the box size before any `Vec` is allocated.

## Review focus

1. **The sampling rule** (`frame_at`): nearest frame, ties to the earlier, with edit lists, B-frame reordering (`ctts`), VFR and out-of-range times. → Tasks 2, 5, property `frame_at_matches_linear_scan`.
2. **Container robustness:** every malformed input is a typed error, never a panic, a hang or an allocation proportional to a declared (not actual) size, including inside `mp4-atom`. → Task 5 fuzz target and limits.
3. **Rotation and display size** are right for 90/180/270° and non-square pixels. → Tasks 2, 5, 7, 8.
4. **The CPU reference is an independent oracle:** it shares only `geometry`, `coverage`/`mips` level maths and `fin-media::colour` with the GPU path, never shader code or texture logic. → Task 7.
5. **T1 holds on every adapter** in CI (lavapipe, WARP, Metal if present), including rotated edges (coverage) and minified sources (mips). → Tasks 9, 15.
6. **`fin-shm` rules:** no host mapping of a file a less-trusted process can write; `O_NOFOLLOW`, `nlink == 1`, size caps. → Task 10.
7. **Plan parity** between `fin-node` and the worker, including captions measured with real fonts. → Tasks 12, 15.
8. **Licence gates** catch an unlisted native build and a changed licence file. → Task 1.

---

## 6A-1 (ready now)

### Task 1: Crates, licence gates and the tracker — tier: sonnet

**Files:**
- Create: `crates/fin-media/`, `crates/fin-container/`, `crates/fin-codecs/`, `crates/fin-compositor/`, `crates/fin-shm/` (each with `Cargo.toml` using the workspace package fields and `publish = false`, and a `lib.rs` stub with its module doc from spec §18).
- Modify: root `Cargo.toml` (`default-members`; `[workspace.dependencies] libm = "=0.2.16"`, and every crate that uses `libm` takes it from the workspace).
- Create: `third_party/native.json`, `third_party/runtime.json`, `scripts/check-native-deps.mjs` + test, `scripts/check-runtime-libs.mjs` + test, `scripts/check-crate-deps.mjs` + test. Modify: `scripts/gates.mjs`, `scripts/check-wasm-deps.mjs` (add `fin-media`, `fin-container`), `THIRD_PARTY_NOTICES.md` (a "Native libraries" heading and the MPL-2.0 §3.2 statement for symphonia when it arrives in 6C).
- Modify: `TASKS.md` (section "Sub-project 6A" listing Tasks 1–16 under 6A-1 and 6A-2).

**`third_party/native.json`:**
```json
{ "libraries": [
  { "name": "SVT-AV1", "package": "fin-codecs", "path": "third_party/svt-av1", "tag": "v<x.y.z>", "commit": "<sha>",
    "license": "BSD-3-Clause-Clear", "patents": "AOMedia-Patent-License-1.0",
    "files": { "LICENSE.md": "<blake3>", "PATENTS.md": "<blake3>" } } ],
  "allow": ["BSD-2-Clause", "BSD-3-Clause", "BSD-3-Clause-Clear", "MIT", "ISC", "Zlib", "Apache-2.0"],
  "patentLicences": ["AOMedia-Patent-License-1.0", "WebM-Patent-Grant"] }
```

**Native build gate (`check-native-deps.mjs`), keyed on `cargo metadata`** (spec §16.3 item 1):
- every package in the shipped graph (the `deny.toml` targets, normal and build edges of the shipped binaries) that has a `links` key, or a build-dependency on `cc`, `cmake`, `nasm-rs`, `bindgen` or `pkg-config`, must appear in `native.json` by `package` (crates whose native code is pure-Rust-bundled, e.g. `ring`, are listed with their bundled licences);
- a listed `path` must be a git submodule at `commit`; each `files` entry must exist and its BLAKE3 must match (a mismatch says "licence file changed: re-review and update the hash");
- `license` must be in `allow`; `THIRD_PARTY_NOTICES.md` must name the library and its tag;
- MPL-2.0 packages in the graph must be named in `THIRD_PARTY_NOTICES.md`'s MPL §3.2 section with their version.

**Run-time library gate (`check-runtime-libs.mjs`)** (spec §16.3 item 2): `runtime.json` lists `{name, provider: os|driver|bundled, license, note}`; the gate greps Rust and TS sources for `libloading::Library::new`, `dlopen`, `LoadLibrary` and `process.dlopen` and fails on a name not listed. Initial entries: none loaded by Finisher in 6A; Electron's `ffmpeg` entry (the open-codecs build, spec §16.1) is added in 6F, `libva` in 6B-2.

**Tests:** the native gate on a fake metadata document with an unlisted `links` package, a GPL licence, a changed licence hash and a missing notice → four failures; the run-time gate on a fake source with an unlisted `dlopen("libfoo.so")` → one failure; the crate-deps gate on a fake graph where `fin` depends on `fin-codecs` → one failure.

- [ ] **Step 1:** Create the crates and the gates with their tests (SVT-AV1's entry is added in Task 6; the gates pass on empty lists).
- [ ] **Step 2:** `rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates` → PASS. **Commit** `build: media engine crates; native, run-time and crate-dependency gates; libm pinned`.

### Task 2: `fin-media` types and the nearest-frame rule — tier: opus

**Files:**
- Create: `crates/fin-media/src/{lib.rs, format.rs, colour.rs, info.rs, sample.rs, version.rs}`, `crates/fin-media/tests/sample.rs`.
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
pub struct SampleTable { timescale: u32, frames: Vec<FrameEntry> } // sorted by pts, strictly increasing
impl SampleTable { pub fn new(timescale: u32, frames: Vec<FrameEntry>) -> Result<Self, CoreError>; } // MEDIA_CORRUPT on duplicates
pub struct FrameAt { pub index: usize, pub out_of_range: bool }
/// Spec §10.2: frame i owns (m(i−1), m(i)] with m(i) = (pts(i) + pts(i+1)) / 2; ties go to the
/// earlier frame; before the first midpoint the first frame, after the last the last frame
/// (`out_of_range` when `s` is before pts(0) or at/after the last frame's end). Exact rationals; O(log n).
pub fn frame_at(table: &SampleTable, s: Rational) -> FrameAt;
/// The keyframe (in decode order) a decoder must start from to output frame `i`.
pub fn decode_start(table: &SampleTable, i: usize) -> usize;
```
- `display` is `coded` rotated by `rotation`, with the width scaled by `pixel_aspect` and rounded to even.
- `frame_at` compares `2·s` with `pts(i) + pts(i+1)` in integer ticks after scaling `s` to the table's timescale as an exact rational (no rounding of `s`).

**Tests:**
- CFR at 24, 25, 30000/1001 and 60: `s` exactly on a pts, at a midpoint (→ earlier), one tick after the midpoint (→ later), one tick before;
- the half-speed case of spec §10.2: a 24 fps table sampled at `k/48` gives 0, 0, 1, 1, 2, 2, …;
- VFR with random durations; a negative first pts (edit-list offset); duplicates refused at construction (`MEDIA_CORRUPT`); `out_of_range` before the first pts and after the last frame's end;
- property `frame_at_matches_linear_scan` (proptest, 256 cases, `PROPTEST_CASES` honoured): the binary search equals a linear nearest-pts scan with the earlier-wins tie rule;
- `decode_start` with open and closed GOPs; `resolve_defaults` (576 → BT.601, 720 → BT.709); `display` for each rotation and a 4:3 pixel aspect.

- [ ] **Step 1: Write** the tests (they fail), then the types and functions.
- [ ] **Step 2:** Schema export and generated types; three gates → PASS. **Commit** `feat(media): media info, colour tags and the nearest-frame sampling rule`.

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
- Blocks are at least 16×16 px at 540p and scale with the height; the reader samples block centres and thresholds at mid-grey, so the code is meant to survive lossy coding and limited-range conversion. **The lossy check is in Task 6** (it needs the AV1 encoder).
- `read_id_rgba` takes the rectangle where the source landed in a composited frame (from `geometry`, Task 7), so frame ids can be read from `fin look` output.

**Tests:** uncompressed round trip for indices 0…10,000 (sampled) at 320×180, 960×540 and 1920×1080; the same after a limited-range conversion and ±3 code values of noise; a corrupted block fails the CRC; `pattern` is deterministic for a seed.

- [ ] **Steps:** tests, implementation, gates → PASS. **Commit** `test(media): frame-id test patterns`.

### Task 4: MP4 muxer over `mp4-atom` — tier: opus

The muxer comes first so the demuxer (Task 5) can be tested against real files from the start, alongside hand-built boxes.

**Files:**
- Create: `crates/fin-container/src/{lib.rs, error.rs, limits.rs}`, `crates/fin-container/src/mux/{mod.rs, mp4.rs, entries.rs}`, `crates/fin-container/tests/mux.rs`.

**Interfaces:**
```rust
pub struct Mp4Writer<W: Write + Seek> { /* … */ }
impl<W: Write + Seek> Mp4Writer<W> {
    pub fn new(w: W, opts: Mp4Options) -> Result<Self, CoreError>;     // faststart: reserve moov space
    pub fn add_video(&mut self, t: VideoTrackConfig) -> Result<TrackRef, CoreError>;
    pub fn add_audio(&mut self, t: AudioTrackConfig) -> Result<TrackRef, CoreError>;
    pub fn write(&mut self, track: TrackRef, data: &[u8], pts: i64, dts: i64, duration: u32, key: bool) -> Result<(), CoreError>;
    pub fn finish(self) -> Result<W, CoreError>; // moov first (into the reserve, or rewritten)
}
pub struct VideoTrackConfig { pub config: CodecConfig, pub coded: Size, pub timescale: u32,
    pub rotation: Rotation, pub pixel_aspect: Rational, pub colour: ColourTags, pub hdr: Option<HdrInfo> }
pub struct AudioTrackConfig { pub config: CodecConfig, pub sample_rate: u32, pub channels: u16, pub priming: u32 }
pub enum CodecConfig { Av1 { av1c: Vec<u8> }, Avc { avcc: Vec<u8> }, Hevc { hvcc: Vec<u8> },
                       Vp9 { vpcc: Vec<u8> }, Aac { esds: Vec<u8> }, Opus { dops: Vec<u8> }, Other { fourcc: [u8; 4] } }
```
- Boxes are built and serialised with `mp4-atom`; the sample tables (`stts`, `ctts`, `stss`, `stsc`, `stsz`, `co64`/`stco`), chunking (one chunk per second per track) and the edit lists are computed by `fin-container`.
- Sample entries in 6A: `av01` (+`av1C`), `avc1`/`hvc1` (config passthrough, for 6B), `Opus` (+`dOps`), `mp4a` (+`esds`). `colr` nclx always written; `pasp` when not square; the matrix for rotation; an edit list for audio priming and for a first video pts ≠ 0.
- Test-only knobs (feature `testing`): VFR durations, custom matrices, `ctts`, an empty edit, fragmented output, so Task 5 and later tests can produce edge cases.

**Tests:** structural checks of the written boxes (decoded back with `mp4-atom` directly: Task 5's demuxer does not exist yet); byte-identical output for identical input; `moov` before `mdat`; a 10,000-sample file; every knob produces the box it names.

- [ ] **Steps:** tests, implementation, `rtk cargo deny check`, gates → PASS. **Commit** `feat(container): MP4 muxer over mp4-atom with fast start, colour, rotation and priming edit lists`.

### Task 5: The demuxer: sample tables, edit lists, fragments and limits over `mp4-atom` — tier: opus

**Files:**
- Create: `crates/fin-container/src/demux/{mod.rs, reader.rs, table.rs, edit_list.rs, fragment.rs, codec_config.rs, colour.rs, matrix.rs}`, `crates/fin-container/tests/{demux.rs, malformed.rs, boxes.rs}` (a small hand-written box builder used only by tests), `crates/fin-container/fuzz/` (`cargo fuzz` target `demux`, not a workspace member).

**Interfaces:**
```rust
pub struct Demuxer<R: Read + Seek> { /* … */ }
impl<R: Read + Seek> Demuxer<R> {
    pub fn open(r: R, limits: &Limits) -> Result<Self, CoreError>;
    pub fn info(&self) -> &MediaInfo;
    pub fn video(&self) -> Option<&VideoTrack>;   // its SampleTable and CodecConfig
    pub fn audio(&self) -> Option<&AudioTrack>;
    pub fn read_sample(&mut self, track: TrackRef, decode_index: u32, buf: &mut Vec<u8>) -> Result<(), CoreError>;
}
pub struct Limits { pub max_boxes: u32, pub max_samples: u32, pub max_box_bytes: u64, pub max_depth: u8 } // 1M, 10M, 64 MiB, 16
```

**Covers** (interpretation is ours; `mp4-atom` decodes the boxes, Decision 10):
- `moov`/`trak`/`tkhd`/`mdhd`/`hdlr`/`stbl` sample tables (`stts`, `ctts` v0/v1, `stss`, `stsc`, `stsz`/`stz2`, `stco`/`co64`) expanded into `SampleTable`s; `edts`/`elst` (v0/v1, empty edits, `media_rate` 1 only) applied to presentation times: frames entirely outside the edit are dropped, a partially covered first frame keeps its pts clamped to the edit's start;
- fragmented files (`mvex`/`trex`, `moof`/`traf`/`tfhd`/`tfdt`/`trun`) appended to the tables;
- the first video and first audio track (others ignored, listed in `info`);
- the `tkhd` matrix → `Rotation` (anything else `UNSUPPORTED_TRANSFORM`); `pasp`; `colr` (`nclx`, `nclc`; ICC reported as ICC); `mdcv`, `clli`, `dvcC`/`dvvC` → `HdrInfo`; codec configuration boxes;
- **VFR:** `vfr = true` when sample durations differ by more than 0.1 % (ignoring the last sample); `nominal_rate` = the most common duration's rate;
- audio priming from `elst` media time or `sgpd` `roll`, Opus pre-skip from `dOps`.

**Limits and safety:** box headers read by `fin-container`, sizes checked against the bytes left and `max_box_bytes` before `mp4-atom` sees the box; declared counts checked against the box size before allocation; depth ≤ `max_depth`; every error is `MEDIA_CORRUPT {at: offset, box}`, `MEDIA_LIMIT` or `UNSUPPORTED_CONTAINER`. No `unsafe`.

**Tests:**
- files from the Task 4 muxer (CFR, VFR, B-frames with `ctts`, an edit list with an empty edit, fragmented, 90/180/270° matrices, a 4:3 `pasp`, `colr` variants, HLG with `mdcv`/`clli`) round-trip every field;
- the hand-built box builder covers what our muxer never writes (an `nclc` `colr`, `stz2`, `ctts` v1 with negative offsets, a moov after mdat, an unknown box between known ones), so the demuxer is not tested only against our own writer;
- `malformed.rs`: truncated at every box boundary of a small file, sizes past EOF, `co64` offsets past EOF, `stsz` counts of `u32::MAX`, a box loop, nesting past 16 — each a typed error within 50 ms and with bounded memory;
- real files from `FIN_MEDIA_FIXTURES` (skip without): an iPhone HEVC portrait clip (rotation 90, HLG, Dolby Vision 8.4), a screen recording (VFR), an edited export with an edit list;
- the fuzz target, seeded with the muxer's files, runs 10 minutes locally with no findings (and 60 s nightly in CI, Task 15).

- [ ] **Steps:** tests (fail), demuxer, fuzz run, gates → PASS. **Commit** `feat(container): demuxer with our sample tables, edit lists, fragments and limits over mp4-atom`.

### Task 6: Software AV1: `rav1d` decode, SVT-AV1 encode; the lossy pattern check — tier: opus

**Files:**
- Create: `crates/fin-codecs/src/{lib.rs, traits.rs, frame.rs, reader.rs, caps.rs}`, `crates/fin-codecs/src/av1/{mod.rs, decode.rs, svt.rs, svt_ffi.rs}`, `crates/fin-codecs/build.rs`.
- Create: git submodule `third_party/svt-av1` at the latest release tag; its entry (with licence-file hashes) in `third_party/native.json`; `THIRD_PARTY_NOTICES.md` (SVT-AV1 licence, AOMedia Patent License 1.0, `rav1d`).
- Create: `crates/fin-codecs/tests/{av1.rs, reader.rs, pattern_lossy.rs}`, `crates/fin-codecs/examples/make_fixture.rs`.

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
pub fn open_decoder(track: &VideoTrack, caps: &Capabilities) -> Result<Box<dyn VideoDecoder>, CoreError>; // AV1 only in 6A
pub fn open_av1_encoder(cfg: &Av1EncoderConfig) -> Result<Box<dyn VideoEncoder>, CoreError>; // preset, crf, keyint, threads
/// Random access by presentation index: decodes from `decode_start`, caches the last GOP.
pub struct FrameReader<R: Read + Seek> { /* demuxer + decoder + table */ }
impl<R: Read + Seek> FrameReader<R> { pub fn frame(&mut self, index: usize) -> Result<&DecodedFrame, CoreError>; }
pub fn capabilities() -> Capabilities; // 6A: av1 decode sw, av1 encode sw, others none
```
- **Build:** `build.rs` runs `cmake` on `third_party/svt-av1` (encoder only, static, `-DBUILD_APPS=OFF -DBUILD_DEC=OFF`, `nasm` on x86_64) into `OUT_DIR`; a clear error names missing `cmake`/`nasm`. `rav1d` from crates.io with its `asm` feature on.
- **FFI:** only the encoder calls used (`svt_av1_enc_init_handle`, `_set_parameter`, `_init`, `_send_picture`, `_get_packet`, `_release_out_buffer`, `_deinit`, `_deinit_handle`), each wrapped once, no `unsafe` outside `svt_ffi.rs`.
- `make_fixture` writes a pattern clip (`--frames`, `--size`, `--rate`, `--vfr`, `--rotate`, `--keyint`, `--crf`) as AV1 in MP4, used by tests and e2e.

**Tests:**
- encode 120 pattern frames at 960×540, decode them back: frame ids exact, PSNR ≥ 35 dB on Y;
- **`pattern_lossy.rs` (moved from Task 3):** the frame-id code reads back for every frame at CRF 40 at 540p and 1080p, and at CRF 50 for at least 99 % of frames (the threshold is recorded; a failure means the block size grows in Task 3's code);
- `FrameReader` random access in shuffled order equals sequential decode (bit-exact planes);
- a truncated sample is `DECODE_FAILED` without panic; encoder determinism (same input and settings → identical bytes with 1 thread).

**Benchmark (archive only):** `rav1d` vs dav1d (`archive/sp6/av1-decode.md`): 1080p and 540p decode fps on each OS. If `rav1d` is more than 15 % slower or fails any test, record it and open the dav1d FFI follow-up in TASKS (spec §6.4).

- [ ] **Steps:** submodule and build, tests, implementation, `rtk cargo deny check`, native gate, three gates → PASS. **Commit** `feat(codecs): software AV1 decode (rav1d) and encode (SVT-AV1 from pinned source)`.

### Task 7: Geometry, coverage, mips, colour and the CPU reference compositor — tier: opus

**Files:**
- Create: `crates/fin-compositor/src/{lib.rs, geometry.rs, coverage.rs, mips.rs, source.rs, output.rs}`, `crates/fin-compositor/src/reference/{mod.rs, ingest.rs, layer.rs, blend.rs, mipchain.rs}` (feature `reference`), `crates/fin-media/src/colour/convert.rs` (matrices and transfer functions shared by both compositors).
- Create: `crates/fin-compositor/tests/{geometry.rs, reference.rs}`, `crates/fin-compositor/tests/golden/t0.json` (BLAKE3 goldens), `crates/fin-compositor/tests/corpus/*.json` (plans + synthetic source descriptions).
- Modify: `crates/fin-core/src/eval/plan.rs` and `model/timeline.rs` (doc comments: the base box, transition looks; no code change), `scripts/check-eval-floats.mjs` and its test (render modules: refuse method transcendentals, allow `libm::`).

**Interfaces:**
```rust
/// Spec §5.1: the source's display size fitted inside the frame, centred ("contain").
pub fn base_box(display: Size, frame: Resolution) -> RectF64;
/// Item → frame affine matrix (row-major 3×3), frame pixels, origin top-left; rotation degrees
/// clockwise; `libm::sin/cos`.
pub fn layer_matrix(t: &PlanTransform, base: RectF64, frame: Resolution, rotation: Rotation) -> [f64; 9];
/// Spec §5.1 edges: per output pixel, the product over the quad's four edges of clamp(d + 0.5, 0, 1).
pub fn coverage(quad: &[[f64; 2]; 4], px: [f64; 2]) -> f64;
/// Spec §5.1 sampling level: L = max(0, log2(1 / s)) for the layer's smaller axis scale s.
pub fn mip_level(matrix: &[f64; 9], source: Size) -> f64;
pub trait SourceProvider { fn frame(&mut self, layer: &Layer) -> Result<SourceFrame<'_>, LayerIssue>; }
pub struct Skipped { pub item: String, pub kind: ItemKind, pub code: String }
pub struct RenderOutput { pub size: Size, pub rgba8: Vec<u8>, pub skipped: Vec<Skipped> }
pub mod reference { pub fn render(plan: &FramePlan, src: &mut dyn SourceProvider, out: Size) -> RenderOutput; }
```

**Reference semantics (the oracle; spec §5.1, §13.2):**
- Ingest: limited/full range expansion, BT.601/709/2020 matrices, bilinear chroma upsampling with the source's siting, to `f32` RGB in display-encoded space, premultiplied alpha 1.
- Sampling: `mip_level` is `max(0, log2(1/s))` (pinned `libm::log2`), so 0 for magnification; the mip chain is exact 2×2 box averages (odd sizes: the last column/row averaged with itself); sampling is bilinear in levels `⌊L⌋` and `⌈L⌉` blended by the fraction, at the inverse-mapped output pixel centre, clamped to edge.
- Per layer: crop in the base box, the matrix, the sample, `coverage`, opacity, then the blend into an `f32` RGBA accumulation buffer (premultiplied) with the seven modes (W3C compositing formulas; straight colour via the guarded division), transitions per Decision 9.
- Output: clamp, round half to even to 8 bits, alpha 255 (opaque black background).
- Layers it cannot draw in 6A (non-clip kinds, nested `children`, effects, masks) are listed in `skipped` with `code: "LAYER_UNAVAILABLE"` and `details.reason: "notYet"`; effects and masks on a drawn clip are skipped the same way and named.

**Tests:**
- `geometry.rs`: contain for 16:9 in 9:16 and the reverse, rotations, anchors at corners, negative scale (mirroring), crop; `coverage` on axis-aligned, half-pixel and 30°-rotated edges against hand values; `mip_level` at scales 2, 1, 0.7, ½, ¼, 0.3.
- `reference.rs`: about 18 corpus cases (single clip; opacity keyframe sample; each blend mode over a gradient; crossfade and dissolve at progress 0, 0.5, 1; rotated portrait source; a 30° rotation (edges); a 4K source at 1080p (mips); transform off-frame; two tracks; a layer with alpha 0 regions (no NaN)): output BLAKE3 equals `t0.json`; the same cases under `wasm32-wasip1` (wasmtime, as the 4C corpus does) give the same hashes.
- Frame ids are readable (`read_id_rgba`) from the composited output where the source is unrotated and at scale ≥ 0.5, and equal `frame_at`'s choice for the layer's source time.

- [ ] **Step 1:** geometry, coverage and mip tests and code; **Step 2:** reference tests (empty goldens print hashes), implementation, inspect 5 outputs by eye (written to `archive/sp6/reference/`), fill `t0.json` from a Linux run; **Step 3:** wasm32 run; float gate; three gates → PASS. **Commit** `feat(compositor): geometry, coverage, mips, colour conversion and the CPU reference compositor`.

### Task 8: Probe through `fin-container` — tier: sonnet

**Files:**
- Modify: `crates/fin-probe/src/lib.rs` (the media path), `crates/fin-probe/Cargo.toml` (drop `mp4`, add `fin-container`, `fin-media`), `crates/fin-probe/src/fixtures.rs` (MOV fixtures written by `fin-container`'s muxer).
- Modify: the `asset.add` path in `fin` (`crates/fin/src/commands/asset.rs`) and `fin-node` so the committed op never carries `media`.
- Tests: `crates/fin-probe/tests/media.rs`; a `fin-node` test on the committed record.

**Behaviour:**
- `Probe` keeps the model's `VideoInfo`/`AudioInfo` **unchanged in meaning** in 6A: width and height stay the coded track size, the rate the average (the model change is 6B, spec §4.4). `Probe` gains an optional `media: MediaInfo` field (generated TS type updated) in the `asset.probe` answer only.
- **No media index** (Decision 6): nothing is written to the cache.
- Existing probe tests pass unchanged, except those pinning `mp4`-crate behaviour on broken files, which now expect `MEDIA_CORRUPT`, `MEDIA_LIMIT` or `UNSUPPORTED_CONTAINER` with the same `details.path` shape.
- **`asset.add` records never contain `media`:** `fin` builds the op from the probe's model fields only, and `fin-node` refuses an `asset.add` whose payload has a `media` key (`INVALID_OP`, so a third-party client cannot store it either).

**Tests:** rotation, VFR and edit lists surface in `media`; a committed `asset.add` (through `fin` against a live server) has no `media` key; the reel e2e's probe goldens do not change.

- [ ] **Steps:** move the media path, keep the old tests green, add the new tests; gates; `rtk pnpm e2e` → PASS. **Commit** `feat(probe): media probing through fin-container; media facts only in the probe answer`. **6A-1 is done.**

---

## 6A-2 (after 5C)

**Entry check** (the controller runs it before Task 11): on the branch 6A-2 stacks on, 5C-1 provides the `fin-jobs` envelope (`hello`, `start` with `files` and `helpers`, `ready`), `fin-workers <family>`, `jobhost`, the `interactive:<family>` warm pools with `jobs.warm`, kind-declared `MappedFile`s in `<server-data>/jobs/<jobId>/`, helpers, `JobEngine.submit`, and the `test` family. Sub-project 6's asks (spec §3.2: the `onSucceeded` hook, the `hwenc` pool, RAM-scaled limits) are checked too; `onSucceeded` is needed by Task 12, the others not before 6B. Missing pieces are raised with 5C, not built here.

### Task 9: The wgpu compositor core and the T1 comparator — tier: opus

**Files:**
- Create: `crates/fin-compositor/src/{device.rs, pool.rs, ingest.rs, layer.rs, blend.rs, mipgen.rs, readback.rs, gpu.rs, compare.rs}`, `crates/fin-compositor/src/shaders/{ingest.wgsl, layer.wgsl, blend.wgsl, mip.wgsl}`.
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
- **Pipeline (spec §5.1, 6A subset):**
  - upload Y/U/V planes as `r8unorm`/`rg8unorm` textures (copy path); ingest shader → `rgba16float` source texture; `mip.wgsl` builds the 2×2 box chain when `mip_level` > 0;
  - `normal`, `screen`, `add`: the layer shader draws onto the accumulation target with fixed-function blend states (premultiplied), **one pass per layer**;
  - `multiply`, `overlay`, `darken`, `lighten`: the layer renders to a scratch target, then `blend.wgsl` reads the accumulation and the scratch and writes the other ping-pong target;
  - the layer shader samples with `textureSampleLevel` at the explicit level (never derivative LOD), clamp-to-edge, and multiplies by the analytic coverage of spec §5.1;
  - output pass converts to `rgba8unorm`; readback with 256-byte row alignment.
- Uniforms are `f32`; matrices, quads and levels come from `geometry`, `coverage` and `mips` (Rust), never computed in WGSL.
- `device.lost` → `GPU_DEVICE_LOST`; allocation failure → `GPU_OUT_OF_MEMORY` (wgpu error scopes around each frame).

**Tests:** every Task 7 corpus case rendered on the GPU passes `compare::passes` against the reference (T1); a failing case writes both images and a diff to `target/fin-compositor-t1/`; a fixed-function and a shader blend of the same mode-equivalent scene agree within T1; pipelines are created once per (stage, mode) (counted); 100 consecutive renders do not grow the texture pool beyond its first frame's set.

- [ ] **Steps:** tests, shaders and code, run on the local GPU and on the software adapter (`FIN_GPU=software`), three gates → PASS. **Commit** `feat(compositor): wgpu compositor core with fused fixed-function blends, coverage edges and explicit-level mips`.

### Task 10: `fin-shm`, the mapped-file channel — tier: opus

**Files:**
- Create: `crates/fin-shm/src/{lib.rs, layout.rs, ring.rs, open.rs, checks.rs}`, `crates/fin-shm/tests/{ring.rs, refusals.rs}`, `crates/fin-shm/fuzz/` (ring header target).

**Interfaces:**
```rust
pub struct JobDir { pub root: PathBuf } // <server-data>/jobs/<jobId>/
impl JobDir { pub fn input(&self) -> PathBuf; pub fn output(&self) -> PathBuf; } // in/, out/
pub struct RingSpec { pub slot_bytes: u64, pub slots: u32 }
pub struct RingWriter { /* maps its own file */ }   // create_new at final size, then map
pub struct RingReader { /* pread-based unless the writer is known gone */ }
impl RingReader {
    pub fn open_untrusted(path: &Path, cap: u64) -> Result<RingReader, CoreError>; // pread only
    pub fn open_sealed(path: &Path, cap: u64) -> Result<RingReader, CoreError>;    // writer exited: may map read-only
    pub fn read_slot(&mut self, i: u32, buf: &mut Vec<u8>) -> Result<SlotState, CoreError>;
}
/// The checks every open performs (spec §3.4): O_NOFOLLOW (Windows: no reparse point), regular
/// file, nlink == 1, size == the header's declared size ≤ cap.
pub fn checked_open(path: &Path, cap: u64) -> Result<File, CoreError>; // SHM_REFUSED {file, reason}
/// The move of a finished output from out/ to its destination: checked_open, then rename (same volume) or copy.
pub fn commit_output(from: &Path, to: &Path, cap: u64) -> Result<(), CoreError>;
```
- Header: magic `FINSHM01`, version, slot size, slot count, then per slot a `u64` sequence written last (release ordering) so a reader never takes a half-written slot.
- The host side (`fin-node`, used by 5C's move step) calls `commit_output`; workers and helpers use `RingWriter` on their own `out/`.

**Tests:** ring round trip across two processes (a test binary); `open_untrusted` never maps (checked by a test hook counting `mmap` calls); refusals: a symlink, a hard link (`nlink == 2`), a FIFO, a file larger than its header, a file truncated while being read (typed error, no SIGBUS because nothing untrusted is mapped), a header with `slots × slot_bytes` overflowing; Windows: a junction and a reparse point. Fuzz the header decoder 10 minutes locally.

- [ ] **Steps:** tests, implementation, gates → PASS. **Commit** `feat(shm): mapped-file rings for job directories with link, size and mapping rules`.

### Task 11: The `media` worker family and the `media.look` runner — tier: opus

**Files:**
- Create: `crates/fin-workers/src/media/{mod.rs, look.rs, sources.rs, open_cache.rs, replay.rs}` (in 5C's crate); `crates/fin-media/src/jobs.rs` (params and results).
- Modify: `crates/fin-workers/src/main.rs` (register the `media` family), `crates/fin-workers/Cargo.toml`; `crates/fin-fonts/Cargo.toml` and `src/lib.rs` (feature `fetch`, default on, gating `ureq`, `rustls`, `Fetcher` and `download`; `FontCache` and the catalog types build without it).
- Create: `crates/fin-workers/tests/media_look.rs`.

**`LookParams`** (generated): `{ timeline, at: [Rational], size?: Size, planSamples: [{at, blake3}], outName }` with the job directory's `in/snapshot.json` and `in/manifest.json`. **`LookResult`:** `{ frames: [{at, file (out/-relative), width, height, plan}], skipped: [Skipped], adapter: AdapterSummary }`.

**Runner** (on 5C's envelope: `hello {family: media, kinds, warm: true}`; `start` → `begin`/`item` per frame → `result` → `ready`; `alive` every 5 s from the envelope's own thread):
- read the snapshot and manifest from the paths in `start.files`; build the measurer (Decision 3);
- for each time: `begin {item: i}`; `eval_frame`; compare its BLAKE3 with the plan sample (`PLAN_MISMATCH`); render with `Compositor` and a `SourceProvider` over `FrameReader`s (one per asset, opened lazily, `frame_at` per layer); write the PNG (`png` crate, deterministic settings) into `out/`; `item {item: i, value: frame}`;
- **warm reuse:** the GPU device, compiled pipelines, open `FrameReader`s and parsed `MediaInfo` (keyed by fingerprint, size and mtime; Decision 6) live across jobs in the same warm worker, which cp §6.3 allows because they are caches keyed by content; 5C's idle expiry and recycling end them;
- `fin-workers media --replay <jobdir>` runs a job directory without a host (bug reports, tests).

**Tests:** `media_look.rs` drives the family through `jobhost` and the envelope as 5C's `test` family is driven (or `--replay` where no host is needed): a fixture project (built with `fin-core` ops on a `make_fixture` AV1 asset) rendered at 5 times including a midpoint tie: PNGs within T1 of the reference; frame ids equal `frame_at`'s choice; a wrong plan sample → `PLAN_MISMATCH`; an H.264 asset → `skipped` `CODEC_UNAVAILABLE`; a second job on the same warm worker reuses the device (counted) and is faster, and its PNG bytes equal those of the same job on a fresh worker (cp §6.3's fresh-vs-reused rule); the worker starts no child process.

- [ ] **Steps:** `fin-fonts` feature split (its tests pass with and without `fetch`); params/results and schema; runner; tests; three gates → PASS. **Commit** `feat(workers): media family with the media.look runner, warm reuse and replay`.

### Task 12: Media job inputs in `fin-node` and the `media.look` kind — tier: sonnet

**Files:**
- Create: `crates/fin-node/src/media/{mod.rs, inputs.rs}`; modify `crates/fin-node/src/lib.rs` (binding), `packages/fin-node` (TS declaration).
- Create: `apps/finisherd/src/jobs/kinds/media.look.ts` (5C's kind format: `{kind: "media.look", version: 1, family: "media", scopes: ["project.read"], params (generated), resources: [], warm: true, defaultPriority: "interactive", timeoutS: 60, stallS: 30, maxAttempts: 2, files: [{name: "in/snapshot.json", mode: "read"}, {name: "in/manifest.json", mode: "read"}, {name: "out/", mode: "write"}], dedupe}`) and its `onSucceeded` handler.
- Tests: `crates/fin-node` unit tests; `apps/finisherd/test/media-look-kind.test.ts`.

**Interface (napi, async on the engine's worker thread):**
```ts
prepareMediaJob(projectId: string, branch: string, timeline: string, at: string[] /* rationals */, inputDir: string)
  : Promise<{ rev: number; planSamples: { at: string; blake3: string }[] }>
```
- Writes `snapshot.json` (Decision 2) and `manifest.json` (spec §4.2; Decision 5: `original` = the stored path if it exists, else `null`; fonts: the cache root plus the project's font asset paths) into the job's `in/` (created by 5C), and returns the plan hashes (Decision 4).
- The kind's `prepare` hook calls it before `start`; its `dedupe` is `(projectId, branch, rev, timeline, at, size)`, so repeated identical looks coalesce.
- Outputs leave `out/` only through `fin-shm::commit_output` (via a `fin-node` binding) in the kind's `onSucceeded` handler (spec §3.2 item 3), to Decision 7's location, before 5C deletes the job directory; destinations inside `.finisher/` or `assets/` are refused (`EXPORT_DESTINATION`). If 5C-1 lacks `onSucceeded`, the handler runs on the `JobEngine` completion event and the task records the race as a 5C follow-up.

**Tests:** the manifest lists every asset the plans reference and nothing else; a missing original is `null`; hashes equal a direct `eval_frame` in Rust; the snapshot round-trips into `fin_core::Project` equal to the engine's state; the kind's params schema is the generated one.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(node): media job inputs and the media.look job kind`.

### Task 13: `media.look`, `media.capabilities` and `fin look` — tier: sonnet

**Files:**
- Modify: `packages/contracts/src/rpc.ts` (+ tests), `apps/finisherd/src/handlers.ts`, `apps/finisherd/src/media/look.ts` (new).
- Create: `crates/fin/src/commands/look.rs`, `crates/fin/src/commands/media_caps.rs`; modify `crates/fin/src/commands/mod.rs`, `crates/fin/src/registry.rs`, `crates/fin/src/output.rs`.
- Tests: `apps/finisherd/test/media-look.test.ts`, `crates/fin/src/commands/tests.rs`, RPC fixtures (`scripts/capture-rpc-fixtures.ts`).

**RPCs:**
- `media.look {projectId, branch?, timeline?, at: Rational[1..64], out?, size?, wait?: bool = true}` (scope `project.read`, reach over the project): the handler submits a `media.look` job with `JobEngine.submit(owner, spec)` (cp §6.7) and, with `wait`, awaits it (≤ 60 s) and answers `LookResult` with final paths; with `wait: false` it answers `{job}` (arch §9.9), which needs 5C-2's `jobs.*` for `fin wait`.
- `media.capabilities {}` → `{codecs: Capabilities, gpu: AdapterSummary | null, worker: {version, renderVersion}}`, answered by a warm `media.capabilities` job once per server run and cached. `finisherd` calls `jobs.warm {family: "media"}` when a project opens.

**`fin`:**
- `fin look --at <t>[,<t>…] [--timeline <id>] [--out <dir>] [--size WxH] [--no-wait]`: times in every `fin` time form (`12.4s`, `00:12.4`, `f300`), echoed resolved; output `{frames: [...], skipped: [...]}`; exit 0 with a non-empty `skipped` (information, spec §2.7). `--no-wait` prints `{job}`; waiting is 5C's `fin wait <job>`.
- `fin media caps`.
- `fin` stays free of media crates (gate from Task 1).

**Tests:** the contracts envelope test lists the new RPCs; `fin look` against a live server renders 3 times of a fixture project; `--at` past the timeline end is `INVALID_RANGE` from eval; an unknown timeline is `NOT_FOUND`; `skipped` reported for a caption item; `--no-wait` then `fin wait` returns the same result (skipped with a notice until 5C-2 is in).

- [ ] **Steps:** tests, implementation, fixtures, three gates, `rtk pnpm e2e` → PASS. **Commit** `feat(fin): fin look and fin media caps over media jobs`.

### Task 14: Diagnostics: media fields in `job.done`, `media.capabilities`, `render.stats` — tier: sonnet

**Files:**
- Modify: `crates/fin-core/src/events/{kinds.rs, payloads/…}` (5C's `job.done` goes to `kv` 2 with an optional `media` object, cp §3.4; new kinds `media.capabilities` v1, `render.stats` v1; `preview.stats` v0 and `render.sample` v0 (`training_only`) reserved); the kind registry tests; `crates/fin-workers/src/media/` (emission); regenerate types.

**Fields** (spec §15, all `meta`): `job.done.media = {codec, backend, hw, resolution (bucket), frames, rtFactor, cacheHit, copyPath}`; `media.capabilities` per codec/profile `hw|sw|none`, backend names from `fin-media` static tables, GPU backend and adapter type, and (owner decision 7) the PCI vendor id, device id and driver major version as numbers, never a driver's name string; the `sandbox` field (`none` in 6A; spec §15); `render.stats` per-minute histograms of decode, composite and readback micros by backend.
- **No new `Source` variant** (the diagnostics spec owns the enum); emission goes through 5C's forwarding of worker diagnostics.
- With diagnostics off (5C passes the gate to workers), the family builds no payload (a test counts zero emissions).

**Tests:** the registry's schema tests (no free-form `meta` string); payload round trips; `strip_content` on the new kinds; a `media.look` job's `job.done` carries the media object.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(core): media fields in job.done; media.capabilities and render.stats kinds`.

### Task 15: e2e scenario and CI — tier: sonnet

**Files:**
- Create: `scripts/e2e/look.mjs`, `scripts/e2e/look-golden/{plans.json, t0.json}`; modify `scripts/e2e.mjs`.
- Modify: `.github/workflows/ci.yml`: install `cmake` and `nasm` (pinned actions or the runner's package manager), Mesa lavapipe on ubuntu (`mesa-vulkan-drivers`), `FIN_REQUIRE_GPU=1` on ubuntu and windows, `git submodule update --init --depth 1 third_party/svt-av1`; a nightly job running the `fin-container` and `fin-shm` fuzz targets for 60 s each. Every action pinned to a commit SHA.

**Scenario `look`:**
1. `cargo run -p fin-codecs --example make_fixture` writes two AV1 clips (540p30 CFR, and a 540p VFR clip rotated 90°) into the e2e temp folder.
2. `fin project create`, `fin asset add` both, two clips on V1 with a crossfade, the rotated clip on V2 with keyframed opacity, a 30° rotation keyframe and a `multiply` blend, a caption from a word list (to check `skipped`).
3. `fin look --at 0,0.5,1.0,1.5,2.0` plus a time exactly at a frame midpoint of the VFR clip → PNGs.
4. Checks: plan hashes against `plans.json`; frame ids read from each PNG equal `frame_at`'s nearest-frame choice (the midpoint picks the earlier frame); each PNG passes T1 against `cargo run -p fin-compositor --example reference_render` over the same job directory (copied aside by the e2e's `onSucceeded` test hook, or replayed with `fin-workers media --replay`), whose hashes must equal `t0.json`; `skipped` lists the caption with `notYet`.
5. Crash handling is 5C's and tested there; this scenario only checks that a `media.look` whose worker is killed (5C's `test-hooks` kill switch) is retried by 5C and then succeeds with no stray files in the look directory.

- [ ] **Steps:** scenario, goldens blessed locally (`E2E_LOOK_BLESS=1`, refused under CI like the reel's), CI changes, a green CI run on all three OSes → **Commit** `test(e2e): fin look scenario; ci: media build dependencies, lavapipe and nightly fuzzing`.

### Task 16: Spec, tracker and cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

**Files:**
- Modify: `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` (status line, "as built" notes for §3, §6.1, §6.3, §6.4, §10.2, §13.2 with measured T1 numbers per adapter), `docs/specs/2026-11-01-finisher-architecture-design.md` §27 (a pointer item for 6A), `TASKS.md`.
- Archive only: `archive/sp6/6a-validation.md` (numbers, hosts by role, never by name).

**Validation rows** (each on macOS arm64, Windows 11 x64 with a discrete GPU, Linux x64 (kernel ≥ 6.2, D31) with an Intel iGPU, the software adapters, and the D31-minimum machine of spec §14 where available):
1. `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` green.
2. T0 hashes identical on all three OSes and wasm32.
3. T1 per adapter: PSNR, max |Δ| and the share of channels over 2/255, for the corpus and the e2e frames; the worst case named (expect it on rotated edges or mips).
4. `fin look` latency on a **warm** worker and cold, p50/p95 over 50 runs at 1080p (budgets spec §14: warm ≤ 300 ms p95 on reference machines, ≤ 600 ms on the D31-minimum; from originals in 6A, the worse case).
5. Worker spawn to `hello` p95 (≤ 150 ms) and warm dispatch (`start` to the first `begin`) p95.
6. The `rav1d` vs dav1d decision recorded (Task 6).
7. Real-footage probe rows from `FIN_MEDIA_FIXTURES`: an iPhone portrait HEVC clip reports rotation 90, HLG and Dolby Vision 8.4; a screen recording reports VFR; an edit-list export reports the trimmed start.

- [ ] **Steps:** run the rows, record them, update the spec and tracker, decide follow-ups → **Commit** `docs: 6A as built and validated`. **6A is done** when every row passes or has an accepted, recorded exception.

---

## Mapping from revision 1's task numbers

| Revision 1 | Revision 2 |
|---|---|
| 1 crates and gates | 1 (gates keyed on `cargo metadata`, licence hashes, run-time gate) |
| 2 types and sampling | 2 (nearest frame) |
| 3 patterns | 3 (lossy check moved to 6) |
| 4 demuxer, 5 muxer | 4 muxer first, 5 demuxer over `mp4-atom` |
| 6 probe + JSON media index | 8 (no index) |
| 7 AV1 | 6 |
| 8 CPU reference | 7 (coverage, mips, guarded division) |
| 9 wgpu | 9 (fused fixed-function blends) |
| 10 worker binary and protocol | 11 (a 5C family; protocol is 5C's) |
| 11 supervisor and media jobs | removed (5C) |
| 12 job preparation | 12 (plus the `media.look` kind) |
| 13 RPCs and `fin look` | 13 (`media.jobs.*` and its `fin wait` removed) |
| 14 diagnostics | 14 (fields in `job.done`; no `Source` change) |
| — | 10 `fin-shm` (new) |
| 15, 16 | 15, 16 |

## Owner decisions (2026-11-07)

The owner answered spec §21's nine questions on 2026-11-07, all as recommended (question 9 also drops Windows 10 from D31). For 6A: decisions 4 and 6 are Decision 1 above; decision 5 schedules 5C-1 alongside 6A-1, so 6A-2 starts when both are in; decision 7 records GPU ids as numeric `meta` (Task 14). Decisions 1, 2, 3, 8 and 9 shape 6B–6G (Linux AAC-LC through symphonia and vo-aacenc in 6C; Electron's open-codecs ffmpeg in 6F; no codec drivers in the Docker image; 6B on all three OSes with NVIDIA on Linux in 6G; export through OS vendors' encoders only and no built-in HEVC template in 6B-1 and 6E). None changes a 6A task.
