# Sub-project 6: Media engine — Design

- **Status:** draft for review (2026-11-07). Nothing is implemented yet. The plan for the first sub-batch (6A) is `docs/superpowers/plans/2026-11-07-sub-project-6a-media-foundations.md`.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §4 (rule 3), §7, §12, §17 (performance budgets), §18, §21.1, §22 and §27.2–27.3 and §27.10 for sub-project 6. Where the two disagree, this document wins for sub-project 6, and arch §27 gets a pointer when a sub-batch merges.
- **Builds on:** sub-project 4 (`docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`, cited as *core §n*), in particular `fin-core::eval` (core §5.4), captions and `fin-text` (core §5.5–5.6), the asset kinds (core §5.7) and the diagnostics journal (arch §21.1).
- **Licence facts** in §16 were checked against primary sources on 2026-11-07 (each project's LICENSE/COPYING/PATENTS file, crates.io metadata). They are not legal advice; arch §26 open question 1 (counsel on codec patents) still stands.

## 1. Goal, scope and success

Sub-project 6 turns plans into pixels and sound. It delivers:
- decoding and encoding of the formats creators use, through OS and hardware codecs plus royalty-free software codecs (D13);
- probing that tells the truth about a file: display size, rotation, colour, HDR, variable frame rate, edit lists;
- proxies, PCM, waveforms, thumbnails and filmstrips, cached locally and content-addressed (arch §12.2);
- one wgpu compositor, native for `fin look`, QA renders and export, and WASM/WebGPU for client preview (D11);
- `fin-audio`, the mixer, native and WASM (arch §12.7), with loudness measurement and normalisation on export;
- rasterised captions, images, SVG, Lottie templates and masks;
- the colour and frame-rate policy of arch §12.8;
- export to the format templates social platforms expect;
- the preview engine (decoder pool, scheduler, AudioWorklet mixer) as a headless TS package, without UI.

**The contract:** the engine renders exactly what `eval_frame` and `eval_audio` say (arch §12.3, core §5.4). It never reads the timeline model to decide *what* to draw or play. Where a plan leaves something out that rendering needs, the plan grows (§4.3); the engine does not reach around it.

**Success, for every sub-batch:**
- every capability is driven and tested through `fin` → `finisherd` (`fin look`, `fin render`, `fin listen`, `fin media`, `fin cache`), and the preview through a headless Playwright harness;
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS arm64, Windows 11 x64 and Linux x64;
- the golden-frame and audio suites (§13) pass at their tolerances on every OS and on wasm32;
- the licence gate covers native libraries too (§16.3).

**Non-goals** (each has an owner elsewhere):
- UI: the editor, the timeline, the inspector, the preview widget (sub-project 9). Sub-project 6 ships the preview engine and a test harness page only.
- The general job system, scopes and the library indexer (sub-project 5). Sub-project 6 builds media jobs only (§3.5, owner question 5).
- Analysis: transcription, alignment, scenes, embeddings, voice activity, mattes, beats (sub-project 8). The engine only feeds them PCM and frames (arch §13).
- Ducking. It is compiled into gain keyframes at edit time (§8.5), so the mixer never needs analysis data.
- Audio effects beyond gain, pan, fades, crossfades and the master limiter (plugins, sub-project 8).
- HDR export and a 10-bit working pipeline (arch §12.8: types are prepared, not built).
- Raw agent-authored Lottie (after v1, arch §12.5).
- An FFmpeg codec backend. Sub-project 6 defines the process-based backend interface and its conformance suite; the plugin itself lives in the copyleft repo (arch §18).
- WebCodecs encoding (arch §27.2: it hangs on Windows; encoding stays in `fin-codecs`).
- Background removal (after v1); sub-project 6 renders mattes that plugins produce.

## 2. Principles

1. **Plans in, pixels out.** A render is a pure function of (frame plan, render manifest, source pixels, render version). The render manifest (§4.2) holds only what an asset *is* (its file, fingerprint and media info), never what the timeline does with it.
2. **Native work that can crash runs in a worker process** (arch §4 rule 3). `finisherd` and `fin-node` never link a codec, a GPU driver, thorvg or resvg. A worker crash, hang or out-of-memory is a retryable job failure, never a server failure.
3. **One sampling rule everywhere.** Preview, `fin look`, QA and export choose the same source frame for the same source time (§10.2), whether they read a proxy or the original.
4. **Determinism has tiers** (§13.2). CPU rasters (text, the CPU reference compositor) are byte-identical on every OS and on wasm32. GPU output is checked against the CPU reference within a stated tolerance. Encoded output is checked after decoding.
5. **Every intentional change to output bumps `RENDER_VERSION`.** It is part of every cache key and every golden, as `TextMeasurer::fingerprint` is for layouts.
6. **Copy first, zero-copy later.** Every GPU path has a CPU-copy fallback that is correct and measured. Zero-copy interop (IOSurface, D3D11 shared handles, DMA-BUF) is an optimisation behind it (§5.4).
7. **Nothing renders silently wrong.** A layer the engine cannot draw (an unsupported codec, a missing font, a missing matte) is reported by item id and code, in the job result and in `fin look`'s answer. Export refuses; preview and `fin look` draw the rest and say what they left out.
8. **No media in the repository.** Test media is generated in code at test time (§13.3), as `fin-probe` fixtures already are. Real footage for validation comes from a local, rights-cleared fixture cache and is never committed.

## 3. Architecture

### 3.1 Processes

```
 finisherd (Node, TS) ─────────────────────────────────────────────────────────────
  RPC gateway · media jobs · MediaSupervisor · cache HTTP endpoints · journal forwarding
        │ napi (fin-node)                          │ stdio NDJSON + shared-memory files
 fin-node (Rust, in-process)                       ▼
  project engines · eval · snapshots for jobs   fin-worker processes (Rust, one binary)
  media cache index · capability cache            roles: probe-deep, decode, proxy, look,
  plan-hash samples · journal writer               render/export, audio-cache, waveform,
                                                    template-check, preview-frames
                                                  links fin-core (eval), fin-text, fin-container,
                                                  fin-codecs, fin-compositor (wgpu), fin-audio,
                                                  fin-captions, fin-graphics (thorvg, resvg)
 Client (browser / Electron renderer)
  fin-wasm: fin-core + fin-container (demux) + fin-compositor (WebGPU) + fin-audio
  @finisher/preview: WebCodecs decoder pool, scheduler, AudioWorklet glue, dotlottie-web
```

- **`fin-worker` is one binary with roles**, chosen per job. Pools keep warm workers per class:
  - `interactive`: `fin look`, thumbnails, server-rendered preview frames. One or two warm workers keep a GPU device and decoders open between requests.
  - `batch`: proxies, PCM caching, waveforms, exports, analysis feeds. Concurrency is capped by cores, GPU memory and the hardware encoder's session limit from the capability report.
- **A worker runs one job at a time** (an interactive worker serves short requests one after another). It is recycled after 50 jobs, after 30 minutes, or when its resident memory passes a high-water mark (default 4 GiB).
- **No GPU in `finisherd`.** A GPU device loss or driver crash kills only a worker.
- **Remote media and the cloud.** A job is self-contained (§3.4), so the same job can later run on another machine's worker pool (`WorkerPool`, arch §5) without a change to its format.

### 3.2 Crash isolation and the worker lifecycle

- **Spawn:** `finisherd` starts every worker from `childEnv()` (arch §27.7). Workers get no `FIN_TOKEN`, no project write path and no network: they read sources and the font cache, and write only into a per-job temp directory under `<server-data>/cache/media/tmp/<jobId>/`.
- **Liveness:** a worker sends `heartbeat` at least every second while busy. The supervisor kills a worker that is silent for 10 s, or that passes its job's deadline (from the job's size: e.g. 3× the expected real time, at least 60 s).
- **Cancel:** `cancel {id}`, then a 2 s grace, then a hard kill (`SIGKILL`; `TerminateProcess` on Windows).
- **Orphans:** a worker exits when its stdin reaches EOF, and a watchdog thread exits it when its parent process is gone (`getppid` change on Unix; a wait on the parent's process handle on Windows). A hung driver call can still outlive the parent; that is accepted and logged (`worker.exit` reason `orphaned` on the next start's sweep, which kills stale pids recorded in `<server-data>/cache/media/workers.json`).
- **Outputs are invisible until committed:** a worker writes into its temp directory. Only the supervisor moves results into the cache (rename) or to their destination (temp-then-rename, arch §6.7), after the worker reported success. A crash leaves nothing behind but temp files, which the next start sweeps.
- **Retries:** `WORKER_CRASHED`, `WORKER_TIMEOUT` and `GPU_DEVICE_LOST` are retryable: the job is retried once on a fresh worker, then fails with the code and `details.attempts`. A second crash on the same input marks the input as poisoned for that recipe (no further automatic retries until the asset or `RENDER_VERSION` changes), so a malformed file cannot crash-loop the indexer.
- **OS sandbox:** workers get the same OS sandbox mechanism as agents (arch §8.4) once sub-project 7 builds the broker; until then they run with the env allow-list, no credentials and the write restriction above (owner question 6).

### 3.3 Worker protocol

- **Control channel:** NDJSON over the worker's stdin/stdout, one JSON object per line, the same framing as `fin`'s local transport. stderr carries logs, scrubbed by the supervisor before they reach the server log.
- **Types:** the messages are Rust types in `fin-media::protocol`, exported to JSON Schema and generated into `@finisher/core-types` like every model type. The supervisor never hand-writes them.
- **Messages:**

  | Direction | Message | Fields |
  |---|---|---|
  | worker → | `hello` | `protocol`, `workerVersion`, `renderVersion`, `pid`, `roles` |
  | → worker | `config` | diagnostics gate, cache root, font cache root, limits, GPU preference |
  | → worker | `job` | `id`, `kind`, `spec` (one variant per job kind, §3.4) |
  | worker → | `progress` | `id`, `done`, `total`, `unit` (`frames`, `samples`, `bytes`), `rate` |
  | worker → | `heartbeat` | `id?`, `rss` (its resident bytes) |
  | worker → | `frame` | `id`, `shm` (file name), `width`, `height`, `format`, `plan` (BLAKE3) — interactive frames |
  | worker → | `event` | a diagnostics payload (§15) |
  | worker → | `result` | `id`, `ok` with the job's result, or `error` (`{code, message, details, fix?}`) |
  | → worker | `cancel` / `shutdown` | |

- **Bulk data** never goes over the control channel: frames and PCM go through memory-mapped files in the job's temp directory (`memmap2`), named in the message; finished artefacts go to files.
- **Versioning:** `hello.protocol` must equal the supervisor's; workers ship with the app and are version-locked to it (arch §21 "Updates"). A mismatch is `WORKER_PROTOCOL` and the worker is killed.

### 3.4 Jobs and their inputs

A job is self-contained: given its spec and the files it names, any worker of the same version produces the same output.

- **Render input:**
  - `snapshot`: the project state at a fixed `(branch, rev)`, written by `fin-node` as JSON to the job's temp directory. The worker runs `fin-core::eval` on it itself, with a `fin-text` measurer over the font cache (read-only).
  - `manifest`: the render manifest (§4.2).
  - `planSamples`: BLAKE3 hashes of the frame plans `fin-node` computed for a few times (first, last, every 10 s of output). The worker compares its own plans at those times and fails with `PLAN_MISMATCH` (an `INTERNAL`-class error) if any differs. This catches a version skew between `fin-node` and the worker, or a font cache that changed under the job.
  - Why the worker evaluates instead of receiving a stream of plans: the job can be replayed alone (`fin-worker run job.json` reproduces a bug report), there is no per-frame IPC, and a remote worker pool needs nothing but the job. Eval is deterministic across platforms (core §5.4), and the samples prove it per job.
- **Job kinds:** `probeDeep` (decode-level probe: first frame, HDR SEI, true VFR), `proxy`, `audioCache`, `waveform`, `thumbs` (thumbnails and filmstrips), `look` (one frame or a contact sheet), `render` (a range to a file: QA renders and export), `templateCheck` (render sample frames of a new Lottie template in a throwaway worker before it is accepted, §11.1), `previewFrames` (server-rendered preview, §12.6).
- **Media jobs live in `finisherd`** (`apps/finisherd/src/media/`): queued, running, done, failed or cancelled, persisted under `<server-data>/media-jobs/` so a restart reports what was running, with the RPCs `media.jobs.get|list|cancel` and `fin wait <job>`. Sub-project 5's job system absorbs them later without changing their wire shape (owner question 5).

## 4. Data flow

### 4.1 From an edit to a frame

```
 tx.commit ──► fin-node (op log) ──► snapshot @ rev ──┐
                                                     ├──► job ──► fin-worker
 asset table + server's library mapping ──► manifest ┘             │
                                                                   ▼
             eval_frame(snapshot, timeline, t) ──► FramePlan ──► render graph
             for each layer: SourceRef(asset, time) ──► manifest ──► media info + sample table
                         ──► frame index (§10.2) ──► decoder (proxy or original) ──► texture
             captions ──► fin-captions raster ──► texture
             graphics ──► thorvg / resvg raster ──► texture
             graph ──► composite (rgba16float) ──► output (8-bit) ──► PNG | encoder ──► muxer
```

In the client, the same graph runs in `fin-wasm` on committed ops replayed locally (arch §6.6), with WebCodecs decoding proxies.

### 4.2 The render manifest

For every asset a snapshot references, the manifest holds:
- `asset`, `kind`, `fingerprint` (as in the model);
- `original`: the absolute path on this machine (resolved by `finisherd` through the library roots, arch §20), or `missing`;
- `proxies`: the cache keys of available proxies per tier;
- `media`: the `MediaInfo` (§6.3) from the media index, keyed by fingerprint;
- for fonts and templates, the cache paths of their bytes.

The manifest says what an asset *is*; the plan says what to do with it. It is built by `fin-node` from the snapshot's asset table and the media index, never by the worker.

### 4.3 Plan contract gaps closed in sub-project 6

Reading the 4C plans against what rendering needs found these gaps. Each is closed in `fin-core::eval` (the plan grows), never by the engine reading the model:

| Gap | Today | Change | Sub-batch |
|---|---|---|---|
| Caption style | `CaptionPlan.style` is the style id only | `CaptionPlan.paint`: the resolved colour, stroke, background box, highlight colour, and the layout's origin in frame pixels (from `position` and the safe zone) | 6D |
| Caption animation | the model has no animation fields; arch §1 promises animated captions | `CaptionStyle.animation` (a closed set of presets, e.g. `none`, `pop`, `karaoke`, `fade`, `slideUp`) added to the model; eval resolves each word's transform, opacity and colour into `WordPlan` | 6D |
| Matte time | `MaskPlan` has the matte id but no time | rule, documented in `fin-core`: a matte is read at the masked layer's source time (mattes are produced aligned to their source) | 6D |
| Base box | how a source maps into the frame at scale 1 is undefined | rule in `fin-core` docs and a `fit` parameter (owner question 4) | 6A (rule), 6D (param) |
| Transition looks | `crossfade` and `dissolve` have no defined difference | `crossfade`: both layers mixed in encoded (display) space; `dissolve`: mixed in linear light. Both use `progress` through `curve` | 6A |

## 5. GPU compositor (`fin-compositor`)

### 5.1 Render graph

For each frame plan, bottom layer first:
1. **Source:** a decoded frame, a caption raster, a graphic raster, an image, or a group (a nested item's children rendered into an offscreen target at the nested timeline's resolution).
2. **Ingest:** YUV planes (NV12, P010, I420/I444 for software decoders) are converted to RGB in a shader: range expansion, the source matrix (BT.601, BT.709, BT.2020 NCL), chroma siting, then the colour policy of §10.1.
3. **Effects in plan order:** `colorAdjust`, `lut`, `blur` (core §5.2), each a WGSL module with a uniform block generated from its `ParamSpec`s.
4. **Crop** (fractions of the item's base box).
5. **Mask:** alpha from the matte (§11.3).
6. **Transform and composite** onto the frame target with opacity and the blend mode: `normal`, `multiply`, `screen`, `overlay`, `add`, `darken`, `lighten`. Blending runs in the shader with the destination read from a ping-pong target, because `overlay` cannot be fixed-function and every mode must match the CPU reference.
7. **Adjustment layers** apply their effects to the composite below them, mixed with the unadjusted composite by the layer's opacity (their transform and crop are defaults, as the plan says).
8. **Transitions:** the outgoing layer (`role: out`) is drawn, then the incoming one with weight `curve(progress)`. `crossfade` mixes in encoded space, `dissolve` in linear light (§4.3).
9. **Output:** the frame target is converted to 8-bit Rec.709 (BT.1886) for PNG, the encoder or the preview canvas.

- **Precision:** sources are 8-bit (or 10-bit where the codec allows, arch §12.8); intermediate targets are `rgba16float` with premultiplied alpha; the output is 8-bit. *Amendment to arch §12.8 "8-bit internally":* 8-bit intermediates band under stacked opacity and gradients, `rgba16float` render targets with blending are core WebGPU, and the 10-bit/HDR path needs float anyway.
- **Base box:** a source's base box is its display size (after rotation and pixel aspect) fitted inside the timeline frame ("contain"), centred. `transform` then applies: `anchor` is item-normalised in the base box, `position` is pixels from the frame centre, `scale` multiplies the base box, `rotation` is degrees clockwise. The matrix is built in Rust (`f64`, `libm` for `sin`/`cos`, deterministic) and passed to shaders as `f32`. Owner question 4 asks whether "contain" is the right default.
- **Pipelines and textures:** one pipeline per (stage, blend mode, target format), created on first use; a texture pool keyed by (size, format, usage) recycles intermediates; decoded frames live in a per-asset ring.
- **Plugin effects later:** a WGSL effect (sub-project 8) is a fragment function `fn effect(uv: vec2f, src: vec4f) -> vec4f` plus a uniform block generated from the plugin's `ParamSpec`s. The built-ins use the same ABI so plugin effects need no special path.

### 5.2 Native

- **Backends:** Metal on macOS; D3D12 on Windows (Vulkan as an alternative); Vulkan on Linux. The adapter is chosen by the capability report: the discrete GPU for batch work unless the user picks another (Chromium ignores `powerPreference`, arch §27.2, but native wgpu does not).
- **Without a GPU** (a NAS without one, a VM): wgpu's software adapters, lavapipe (Mesa, Vulkan) on Linux and WARP (D3D12) on Windows. The server Docker image includes Mesa's lavapipe. They are slow but correct, and the capability report says so.
- **Readback** for PNG and software encoders: a staging buffer with 256-byte row alignment, mapped asynchronously; double-buffered so the GPU renders frame *n+1* while frame *n* is read.

### 5.3 WASM (preview)

- `fin-wasm` builds `fin-compositor` for `wasm32-unknown-unknown` with wgpu's WebGPU backend. Shaders are the same WGSL.
- **Frame ingest:** WebCodecs `VideoFrame` → `copyExternalImageToTexture`, one GPU copy (0.4 ms p95, arch §27.2). `importExternalTexture` is not used: it needs a separate shader path and the copy is cheap.
- **Graphics in preview:** Lottie frames come from dotlottie-web (§11.1) as `ImageBitmap`s and are copied the same way; captions and SVG are rasterised by `fin-wasm` itself (pure Rust).

### 5.4 Zero-copy interop (later, 6G)

| OS | Decoder output | Import into wgpu |
|---|---|---|
| macOS | `CVPixelBuffer` on an IOSurface | Metal texture from the IOSurface, wrapped with `create_texture_from_hal` |
| Windows | D3D11 texture from Media Foundation / D3D11VA | shared NT handle opened on D3D12, wrapped from hal |
| Linux | VA surface exported as DMA-BUF | Vulkan external memory with DRM format modifiers, wrapped from hal |

The copy path (download to CPU, upload) stays as the fallback and is chosen automatically when an import fails; the capability report records which path a machine uses. Encoding takes the same route backwards (IOSurface-backed render targets into VideoToolbox; NV12 conversion on the GPU before download elsewhere).

## 6. Codecs and containers

### 6.1 Strategy

- **Containers are ours, in Rust** (`fin-container`), on every OS: one reading of edit lists, sample tables, rotation and colour boxes everywhere. Media Foundation ignores MP4 edit lists (arch §27.3), and AVFoundation hides sample tables, so relying on OS demuxers would give three behaviours.
- **Codecs are the OS's or hardware's for patent-encumbered formats** (H.264, HEVC, AAC) and **royalty-free software** for AV1, VP9 and Opus, as D13 and arch §18 say. Finisher ships no H.264 or HEVC implementation of its own, encoder or decoder.
- **OS codecs are driven at the codec level:** `VTDecompressionSession` / `VTCompressionSession`, Media Foundation transforms (`IMFTransform`, the hardware MFT bound explicitly by adapter, arch §27.3) and D3D11VA, and VA-API through libva. Never through the OS's source readers or players.
- **Process-based backends** (the FFmpeg plugin, others) implement the same `Decoder`/`Encoder` traits over the worker protocol's shared-memory frames, and pass the conformance suite (§13.5).

### 6.2 Formats per OS

**Containers**

| Container | Read | Write | Notes |
|---|---|---|---|
| MP4 / MOV (ISOBMFF), incl. fragmented | ✓ | ✓ (MP4) | edit lists, `colr`, `pasp`, `tkhd` matrix, `mdcv`/`clli`, `dvcC`/`dvvC`, `sgpd`; fast start |
| WebM / Matroska | ✓ (6B) | ✓ WebM (6E, the VP9/AV1 + Opus reserve) | |
| WAV, AIFF, FLAC, Ogg, MP3, CAF | ✓ audio | — | via symphonia's readers (§6.4) |
| PNG, JPEG, WebP | ✓ images | PNG (`fin look`, thumbnails) | |
| MXF, AVI, MPEG-TS, others | — | — | `UNSUPPORTED_CONTAINER`, fix: the codec plugin |

**Video decode**

| Codec | macOS 14+ (Apple Silicon) | Windows 10 22H2 / 11 | Linux | Software |
|---|---|---|---|---|
| H.264 (8-bit, High) | VideoToolbox (HW) | MF + D3D11VA (HW) | VA-API (Intel, AMD); NVDEC (6G) | **none** (patents) |
| HEVC Main / Main10 | VideoToolbox (HW) | MF with the HEVC Video Extensions, else D3D11VA directly (arch §27.3) | VA-API; NVDEC (6G) | **none** |
| AV1 | VideoToolbox on M3 and later | MF (AV1 extension) or D3D11VA | VA-API | `rav1d`/dav1d (all OSes) |
| VP9 | VideoToolbox | MF / D3D11VA | VA-API | libvpx |
| ProRes 422/4444 | VideoToolbox | — | — | none (plugin) |

**Video encode**

| Codec | macOS | Windows | Linux | Software |
|---|---|---|---|---|
| H.264 | VideoToolbox (HW) | hardware MFT by adapter (AMD, NVIDIA, Intel) | VA-API (Intel low-power VDEnc, AMD) | **none** |
| HEVC | VideoToolbox | hardware MFT | VA-API | **none** |
| AV1 | — (no Apple encoder) | hardware MFT (RTX 40+, RX 7000+, Arc) | VA-API where offered | **SVT-AV1** |
| VP9 | — | — | VA-API (Intel) | libvpx `--rt` (reserve) |

**Audio**

| Codec | macOS | Windows | Linux |
|---|---|---|---|
| AAC-LC / HE-AAC decode | AudioToolbox | MF AAC decoder | **open** (owner question 1) |
| AAC-LC encode | AudioToolbox | MF AAC encoder | **open** (owner question 1) |
| Opus | libopus (decode and encode, all OSes) | | |
| MP3, FLAC, ALAC, Vorbis, PCM | symphonia (decode, all OSes) | | |

- **Gaps get guidance, not failure** (arch §12.1): the capability report lists each missing decoder with its fix (the HEVC Video Extensions, a GPU driver, the codec plugin). An asset that cannot be decoded is still registered; its items render as "unavailable" in preview and `fin look`, and export refuses with `CODEC_UNAVAILABLE`.
- **Proxies remove most gaps for editing:** a machine that can decode an original once (or another machine sharing the library, later) writes AV1 or H.264 proxies that every client can play.

### 6.3 Probing

- **Shallow probe** (in `fin-node`, at `asset.add`, as today): `fin-container` parses the container, which is safe Rust with panics caught, so it stays in process. It reports the model's `VideoInfo`/`AudioInfo` and fills the **media index** (in the cache, keyed by fingerprint) with `MediaInfo`:
  - coded and display size, the `tkhd` rotation (0/90/180/270; other matrices refused as `UNSUPPORTED_TRANSFORM`), pixel aspect;
  - codec, profile, level, bit depth, chroma format;
  - colour tags (`colr` nclx/ICC, else the codec configuration's VUI), range, chroma siting;
  - HDR: transfer (PQ, HLG), `mdcv`/`clli`, Dolby Vision configuration (profile, compatibility id);
  - the sample table with edit lists applied (§10.2), the nominal rate, and whether the source is VFR (frame durations differ by more than 0.1 %);
  - audio: codec, rate, channels and layout, priming (encoder delay) and padding.
- **Deep probe** (a worker job, after add): decodes the first frame to confirm decodability and reads in-band HDR SEI when the container has no HDR boxes.
- **Known 4C gaps this fixes:** `fin-probe` today reports `tkhd` width and height without the rotation matrix (a portrait iPhone clip reads as 1920×1080), averages the frame rate over the track (VFR is not detected) and ignores edit lists. 6A puts the truth in `MediaInfo`; 6B adds display size and rotation to the model's `VideoInfo` with a format bump under core §3.2.

### 6.4 Software codecs and container code

| Library | Use | Integration |
|---|---|---|
| `rav1d` (Rust port of dav1d) | AV1 decode | crate; dav1d itself through a minimal FFI is the fallback if `rav1d` is > 15 % slower or fails conformance (6A measures it) |
| SVT-AV1 | AV1 encode: proxies without a hardware encoder, exports where no AV1 HW exists, and test fixtures | own minimal FFI, built from source at a pinned tag (arch §27.3), never a prebuilt download |
| libvpx | VP9 decode and the `--rt` encode reserve | own minimal FFI, built from source at a pinned tag |
| libopus | Opus decode/encode | own minimal FFI, built from source at a pinned tag |
| symphonia | MP3, FLAC, ALAC, Vorbis, PCM, WAV/AIFF/Ogg/CAF readers | crate, unmodified |
| `fin-container` | ISOBMFF and WebM demux, MP4 and WebM mux | ours; fuzzed (§13.5) |

`svt-av1-sys`, `vpx-sys` and `audiopus` exist but are years old, so Finisher keeps small FFI modules of its own (a few hundred lines each), as arch §27.3 already decided for SVT-AV1.

## 7. Proxies and caching

### 7.1 Proxies

- **Tiers** (arch §12.2):
  - `540`: scrub and remote clients. All-intra where the hardware encoder supports intra-only at ≥ 8× real time; otherwise a keyframe every 0.25 s.
  - `1080`: default preview. Closed GOP with a keyframe at least every 0.5 s.
  - Originals for export.
- **Codec:** OS/hardware H.264 → hardware AV1 → SVT-AV1 (preset 10, CRF, closed GOP every 15 frames; arch §27.3). Remote clients whose WebCodecs cannot decode the chosen codec get the other one, generated on demand.
- **Audio:** Opus 128 kb/s stereo at 48 kHz inside the proxy (for remote clients and preview), plus the PCM cache (§7.2) for native mixing.
- **Frames:** a proxy holds **exactly one frame per original frame**, in order, stamped at a constant rate (the original's nominal rate). With the original's sample table kept beside it, preview finds the original frame for a source time by the rule in §10.2 and fetches that frame from the proxy by index. Preview and export therefore show the same frame, also for VFR sources. This is how "conformed to CFR with a PTS map" (arch §12.2) is realised.
- **HDR sources** are tone-mapped to SDR when the proxy is made (§10.1), with the same algorithm export uses on that OS, so preview never relies on Chromium's own tone mapping.
- **Rotation** is not baked: proxies keep the coded orientation, and the compositor rotates, so proxies and originals are interchangeable.
- **When:** library indexing (sub-project 5) asks for `540` for every asset; `1080` is made when an asset is first used in a project; both can be requested explicitly (`fin media proxy <asset>`).

### 7.2 The cache

- **Location:** `<server-data>/cache/media/`, local disk only (arch §6.7). Kinds: `proxy-540`, `proxy-1080`, `pcm` (48 kHz `f32` stereo in 1 MiB chunks), `wave` (min/max peaks at several zoom levels), `thumbs`, `strip` (filmstrips), `index` (the media index), `raster` (Lottie, SVG and caption rasters worth keeping).
- **Keys:** BLAKE3 of (source fingerprint, recipe name, recipe parameters, `RENDER_VERSION`, codec backend class). A changed source (new fingerprint) or a new render version makes new keys; old entries age out.
- **Entries** are directories: the payload, the sample table where relevant, and `meta.json` (recipe, versions, sizes, creation). Workers write into their temp directory; the supervisor renames the directory into place and records it.
- **Index:** a small SQLite database `cache/media/index.db` on local disk, written by `fin-node` only: entries, sizes, last use, pins.
- **Eviction:** least recently used, above a cap (default the smaller of 50 GiB and 15 % of the volume, `media.cacheMaxGiB`). Entries that a running job reads, or that an open project's timelines use, are pinned. `fin cache status|gc`.
- **Offline libraries:** proxies and PCM keep editing and preview working; export waits and lists the missing originals (`ORIGINALS_MISSING`, arch §6.7).

### 7.3 Serving the client

- `finisherd` serves cache entries over HTTP with range requests (`GET /media/<key>/<file>`), authorised like the WebSocket. Clients address media by key, never by path.
- The preview demuxes proxies itself (`fin-wasm` runs `fin-container`) and feeds WebCodecs; it fetches GOP-sized byte ranges ahead of the playhead.

## 8. Audio engine (`fin-audio`)

### 8.1 Format and decoding

- 48 kHz, 32-bit float, stereo internally (arch §12.7). Mono sources are upmixed by duplication with −3 dB; multichannel sources are downmixed with ITU-R BS.775 coefficients.
- Resampling at decode with `rubato` (sinc, fixed parameters, deterministic), so the PCM cache is always 48 kHz.
- Encoder priming and padding from the container (`elst`, `sgpd` roll, Opus pre-skip) are removed at decode, so sample 0 of the cache is the first audible sample at source time 0.

### 8.2 Mixing the plan

- For each `AudioItem`: read PCM from `sourceFrom` at `speed`; apply gain segments (dB → amplitude, `10^(dB/20)`), shapes and pan; sum into the master bus in plan order.
- **Curves:** `linear` is `x`; `equalPower` is `sqrt(x)` (so a crossfade's two sides sum to constant power). This fixes "as the mixer defines it" in `eval::audio`.
- **Pan:** a mono source uses constant-power panning (`L = sqrt((1 − p)/2)`, `R = sqrt((1 + p)/2)`, so the centre is −3 dB on each side); a stereo source uses balance (the side panned away from is attenuated linearly, the other side stays at unity).
- **Speed ≠ 1** keeps pitch, with a WSOLA time-stretch written in Rust in `fin-audio` (deterministic, the same native and in WASM). C++ stretchers do not build for `wasm32-unknown-unknown`, and the permissive options that do (Rubber Band and SoundTouch are GPL/LGPL and excluded) are not good enough for speech at 0.5–2×.
- **Master bus:** the sum, then (on export only) the loudness gain and the true-peak limiter (§8.4).
- **Determinism:** scalar `f32` in a fixed order, `libm` for `pow` and `sqrt`, no SIMD intrinsics in the mixer core. Native and wasm32 output is bit-identical, so null tests are exact (§13.4).

### 8.3 Preview

- The same mixer runs in an AudioWorklet as a WASM instance. PCM comes from the Opus proxy audio, decoded by WebCodecs `AudioDecoder` into a `SharedArrayBuffer` ring the worklet reads. That needs cross-origin isolation (COOP/COEP headers from `finisherd` and in Electron); the preview harness checks for it.
- **The audio clock drives video** (arch §12.7): the scheduler of arch §27.2 (cadence-aware, audio clock sampled at rAF plus the measured presentation offset, nearest frame, per-platform display constant, interpolated audio clock) is implemented in `@finisher/preview`.

### 8.4 Loudness

- **Measurement:** ITU-R BS.1770-4 integrated loudness, loudness range and true peak (4× oversampling), via the `ebur128` crate (MIT, a port of libebur128). Measured on every export and on `fin listen`.
- **Normalisation:** a format template names a target (e.g. −14 LUFS integrated, −1 dBTP). Export runs two passes over the audio: mix and measure, then apply one static gain and a look-ahead true-peak limiter on the master bus. The export report records the measured values before and after.
- **Unreachable targets** (silence, or a gain the limiter cannot hold within 1 LU) produce the warning `LOUDNESS_TARGET_UNREACHABLE`, not a failure.

### 8.5 Ducking (amendment to arch §12.7)

Ducking is compiled into ordinary gain keyframes at edit time by a `compose::duck` builder in `fin-core` (from voice-activity artifacts and a ducking preset), never computed by the mixer. Agents and users see and edit the result; the plan stays the whole truth; the mixer stays free of analysis data. The builder arrives with voice activity in sub-project 8.

### 8.6 `fin listen`

A text summary of a range for agents (arch §9.6): per track and for the mix, integrated loudness, true peak, clipping, silences longer than 0.5 s, and the loudest moments. Speech and music labels join when sub-project 8's analysis exists.

## 9. Export pipeline

### 9.1 Flow

1. **Preflight** (in `finisherd` before a worker starts): the template is supported here (capability report), every original is reachable and unchanged (fast fingerprint), fonts and templates are present, nothing in the range is unmeasured or unavailable. Failures are typed (`ORIGINALS_MISSING`, `ASSET_CHANGED`, `CODEC_UNAVAILABLE`, `CAPTIONS_UNMEASURED`, `LAYER_UNAVAILABLE`), each listing the items or assets.
2. **Video:** for output frame `n`, `t = from + n / rate` exactly (rational). Eval, decode originals by §10.2, composite at the output resolution, convert to NV12 on the GPU, encode. Bounded queues between decode, composite and encode keep memory flat (§14).
3. **Audio:** eval over 1 s chunks, mix, measure (pass 1); then mix again with the normalisation gain and limiter and encode (pass 2). The audio of a range starts at sample `round(from × 48000)`, which is exact for every frame rate in use because both are rationals.
4. **Mux** (`fin-container`): MP4 with `moov` first (space reserved, rewritten at the end), `colr` nclx BT.709 limited range, the encoder delay in an edit list so audio and video start together exactly.
5. **Verify:** reopen the file with `fin-container`, decode the first and last frames, count frames and samples, decode the audio and measure its loudness. The result goes into the export report.
6. **Commit:** temp-then-rename to the destination (default `workspace/shared/exports/<name>.mp4`).

### 9.2 Format templates

- Templates are data (a content pack, arch §14.1): container, video codec, profile, resolution, rate (always the timeline's), rate control and bitrate, GOP, audio codec and bitrate, loudness target and true-peak ceiling, safe-zone preset.
- First-party templates: `social-9x16-1080p` (H.264 High, ~12 Mb/s VBR, AAC-LC 192 kb/s, −14 LUFS / −1 dBTP), `social-9x16-1080p-hevc`, `master-16x9-1080p`, `master-16x9-2160p`, `av1-web` (AV1 + Opus, the royalty-free reserve).
- A template asks for capabilities, never for a specific backend; the export report names the backend that was used.

### 9.3 Export report

`{file, bytes, duration, frames, rate, video: {codec, profile, backend: hw|sw, encoder}, audio: {codec, rate, loudness: {integrated, range, truePeak, before}}, avOffsetSamples: 0, warnings, renderVersion, textEngine, planSamples}`. `fin render` prints it; the reviewer's QA gate (sub-project 10) reads it.

### 9.4 Not in v1

Chunked parallel export (GOP-aligned segments encoded on several workers and concatenated) is designed for but not built: the job format already names its range, so a later version can split it. It needs identical encoder parameter sets across sessions, which is not guaranteed by every hardware encoder.

## 10. Colour and frame rate

### 10.1 Colour

- **Working space:** Rec.709 primaries, display-referred, BT.1886 encoded values. sRGB images, SVG, Lottie and caption colours are treated as the same encoding (the usual practice in editors and browsers). Output is tagged BT.709 / BT.709 / BT.709, limited range.
- **Untagged sources:** BT.601 below 720 lines, BT.709 otherwise; images without a profile are sRGB.
- **Wide-gamut SDR** (Display P3 images and video, BT.2020 SDR): linearised, converted to Rec.709 with a 3×3 matrix, soft-clipped, re-encoded.
- **HDR** (HLG, PQ, Dolby Vision profiles 8.x and 5 from phones), tone-mapped to SDR at decode (arch §12.8):
  - macOS: the OS tone mapping (VideoToolbox / `VTPixelTransferSession`), which also uses Dolby Vision metadata;
  - Windows and Linux: Finisher's shader. HLG is first taken to display light at a 1,000-nit reference; then a BT.2390 EETF on PQ luminance maps it to SDR, followed by BT.2020 → Rec.709 gamut mapping (matrix plus soft clip). Dolby Vision profile 8 uses its HLG/HDR10 base layer and ignores the RPU; profile 5 (no compatible base layer) is decodable only on macOS and otherwise `CODEC_UNAVAILABLE`.
  - The source's HDR metadata stays in `MediaInfo` for HDR export later.
  - Because the two algorithms differ, HDR goldens are compared within an OS only (§13.2, tier T4).
- **10-bit sources** are decoded at 10 bits (P010) and converted in the shader. The working space stays SDR.
- **LUTs** (`.cube`) apply in encoded space as 3D textures (`rgba16float`, trilinear), as `.cube` LUTs for Rec.709 expect.

### 10.2 Frame rate and VFR

- **The sample table:** every video track's frames as `(pts, duration, keyframe, offset, size)`, with edit lists applied, so `pts` is presentation time on the source's own timeline in exact rationals.
- **The sampling rule** (one function, `fin-media::frame_at`): the frame shown at source time `s` is the last frame whose `pts ≤ s` (sample-and-hold). Before the first frame, the first; after the last, the last; outside the source's extent the layer reports `SOURCE_OUT_OF_RANGE` in the result (a transition handle past the asset is policy, core §5.4, so it renders the last frame and says so).
- **Rate conversion:** the timeline rate is fixed; every source is sampled by that rule, so 30 fps footage on a 25 fps timeline drops frames and 24 fps footage on a 30 fps timeline repeats them. *Arch §12.8 also names frame blending, chosen per item:* the model has no such parameter today, so v1 renders sample-and-hold, and a `frameBlend` parameter is a 6D candidate if the owner wants it before v1.
- **VFR:** the table is used as is; nothing is resampled. Proxies keep one frame per original frame (§7.1), so preview and export agree.

## 11. Captions, Lottie, SVG and masks

### 11.1 Lottie templates (`fin-graphics::lottie`)

- **Native:** thorvg's C API through a small FFI, built from source at the version inside the shipped dotlottie-web (v1.1.x; arch §27.10), with only its Lottie loader (plus the PNG/JPEG loaders templates embed) and the software raster engine. Frames render to premultiplied RGBA at the layer's pixel size and are uploaded as textures. A golden checks native thorvg against dotlottie-web (pixel-identical at v1.1.1 vs dotlottie-web 0.81.0, arch §27.10).
- **Preview:** dotlottie-web (MIT) in a Web Worker in the client.
- **Text slots** are laid out and rasterised by `fin-captions` and injected as image slots, oversampled for animated scale (arch §27.10). thorvg's text engine is never used.
- **Safety:** a template is validated against the slot schema at `asset.add` (core §5.7) and then rendered at sample times by a `templateCheck` job in a throwaway worker, because thorvg segfaults on malformed `chars` (arch §27.10). A template that crashes or hangs the check is refused (`TEMPLATE_INVALID`). Export renders templates in the render worker, so a crash there fails only that job.
- **Fonts:** every font a template names must be registered (arch §27.10); text slots use the project's pinned fonts.

### 11.2 SVG and images (`fin-graphics::svg`, `image`)

- **Sanitising** at `asset.add` (the asset file is never changed; the sanitised tree is a cache entry): `usvg` (≥ 0.45, Apache-2.0 OR MIT) parses with external references, `<script>`, event attributes, `foreignObject` and system fonts disabled; `data:` URLs only for images; size and nesting limits. A file that needs anything refused is `SVG_UNSAFE` with the reason.
- **Text in SVG** uses the project's pinned fonts through `fin-text`, never system fonts; an unpinned family is `FONT_UNPINNED` at add.
- **Raster:** `resvg` (tiny-skia) at the layer's pixel size; re-rasterised when the effective scale changes by more than 10 %; cached.
- **Images:** PNG, JPEG and WebP decoded in the worker (pure Rust decoders), EXIF orientation applied, ICC profiles honoured for sRGB and Display P3 (§10.1), others treated as sRGB with a warning.

### 11.3 Masks

- A matte (arch §12.6) is a luma-encoded greyscale video artifact. The engine decodes it like any source, at the masked layer's source time (§4.3), and takes alpha from range-expanded luma.
- **Order:** `choke` (erosion by `choke` pixels at the timeline resolution, a separable min filter), then `feather` (Gaussian blur with radius `feather` pixels), then `invert`, then multiply into the layer's alpha after crop.
- **Mask edits** (`mask.set`, `mask.remove`, keyframes on `mask.*`) are added to `fin-core` in 6D (4C Decision 48), one module per op, with inverses and footprints like every op.
- Producers are plugins (sub-project 8). Missing mattes render the layer unmasked in preview with `MATTE_MISSING` and refuse export.

### 11.4 Captions (`fin-captions`)

- **Input:** the `CaptionPlan`: words, active flags, layout, and (from 6D) the resolved paint and per-word animation (§4.3).
- **Glyphs:** `fin-text` gains a glyph-level twin of `measure` (`shape` → glyph ids and positions per word) over the same code path, so the renderer's word boxes are exactly the plan's. The renderer checks this and fails with `PLAN_MISMATCH` if they differ.
- **Raster:** glyph outlines through `swash`'s scaler on the CPU, the path the S11b spike proved byte-identical across x86-64, arm64 and wasm32 when fonts are pinned (arch §27.10). Stroke, background box (rounded rectangle) and highlight colour are rasterised in the same pass, into one premultiplied RGBA bitmap per block and animation state, cached by (layout hash, paint hash, state).
- **Animation:** per-word transforms and opacity from the plan are applied when the block bitmap is built (small motions) or as per-word quads on the GPU (scale animations, where the words are rasterised oversampled).
- **Colour emoji:** bitmap emoji (CBDT/sbix) from the catalog's Noto Color Emoji (OFL) bitmap build, scaled with a box filter. COLRv1 needs a painter `swash` lacks (arch §27.10); it is a later follow-up, and the catalog pins the bitmap build until then.
- **Missing fonts:** an unmeasured caption (core §5.4 `unmeasured`) is not drawn in preview or `fin look`, which report it; export refuses with `CAPTIONS_UNMEASURED` (fix: `fin fonts fetch`).

## 12. Preview engine (`@finisher/preview`, headless)

- **What it is:** a TS package with no UI: a player core that a client (sub-project 9's editor, the share page later) mounts on a canvas.
- **Pieces:**
  - the `fin-wasm` instance (op replay and eval, compositor, demux);
  - a WebCodecs decoder pool, one decoder per active asset and tier, fed GOPs ahead of the playhead, with frames kept in a ring keyed by original frame index;
  - the scheduler (§8.3) and the AudioWorklet mixer;
  - dotlottie-web for graphics;
  - a capability probe (`VideoDecoder.isConfigSupported` per proxy codec) sent to the server in the handshake, so it serves the right proxy codec.
- **Scrub:** seeks decode from the nearest keyframe of the `1080` proxy, or of the `540` proxy while dragging fast; the last good frame stays up until the new one arrives.
- **Server-rendered fallback** (arch §12.3, D31: no WebGPU): an interactive worker renders frames on demand (`previewFrames`, WebP over the binary channel) for scrubbing. For playback it renders the visible range into a temporary `540` H.264/AV1 file played through WebCodecs or a `<video>` element. It is slower and it says so.
- **Test harness:** a page under `packages/preview/harness/` loaded by Playwright in Chromium and in Electron on the three OSes, for parity goldens, the A/V latency measurement of arch §27.2, and the budgets of §14. It also serves as the walking skeleton's "bare web client" (arch §24 row 3), which has not been built yet.

## 13. Determinism and tests

### 13.1 Layers of testing

| Layer | What | Where |
|---|---|---|
| Unit | colour matrices, transfer functions, the sampling rule, gain/pan laws, WSOLA, effect formulas, each against hand-computed values | each crate |
| Containers | demux/mux round trips, edit lists, fragmented files, rotation and colour boxes, broken files (typed errors, never panics) | `fin-container` |
| Codec conformance | each backend decodes generated fixtures to the expected frame ids and checksums; encoders round-trip through decoders within T5 | `fin-codecs`, per OS |
| Golden frames | plans from a corpus rendered by the CPU reference (hashes committed) and by the GPU (compared to the reference within T1) | `fin-compositor`, native and wasm |
| Audio | null tests, native vs wasm bit-exactness, loudness against EBU Tech 3341/3342-style signals generated in code, A/V offset | `fin-audio`, export |
| Workers | crash, hang, cancel, kill mid-write (no partial output visible), retry and poisoning, protocol mismatch, orphan sweep | `fin-workers`, `finisherd` |
| e2e | the reel scenario (core §5.9) extended: `fin look` samples, `fin render` with its report, `fin listen` | `pnpm e2e` |
| Preview | parity with native, A/V latency, scrub and playback budgets, in Chromium and Electron | Playwright harness |
| Fuzzing | the demuxers, the SVG sanitiser, the `.cube` parser, the worker protocol decoder | `cargo fuzz`, nightly CI job |

### 13.2 Tolerance tiers

| Tier | Compared | Tolerance |
|---|---|---|
| **T0** | CPU rasters (captions, the CPU reference compositor, SVG where tiny-skia proves deterministic) across macOS, Windows, Linux and wasm32 | byte-identical; BLAKE3 goldens committed |
| **T1** | a GPU render against the CPU reference render of the same plan and source pixels, any adapter (Metal, D3D12, Vulkan, lavapipe, WARP, WebGPU) | per channel \|Δ\| ≤ 2/255 on ≥ 99.9 % of pixels, max \|Δ\| ≤ 8/255, PSNR ≥ 50 dB |
| **T2** | native thorvg against dotlottie-web for the same template frame | PSNR ≥ 60 dB (measured 77.6 dB, arch §27.10) |
| **T3** | the same 8-bit H.264, HEVC, AV1 or VP9 bitstream decoded by different backends | bit-exact YUV planes (decoding is normative); a backend that differs is a bug or is excluded |
| **T4** | HDR tone mapping, within one OS against its own reference | T1 within the OS; across OSes informational only (mean ΔE2000 reported, not gated) |
| **T5** | an encoded export, decoded, against the CPU reference render | PSNR ≥ 38 dB and SSIM ≥ 0.97 at the template bitrate; frame count and frame ids exact |
| **A0** | the mixer, native against wasm32 | bit-identical samples |
| **A1** | export audio decoded against the mixer's output | integrated loudness within ±0.1 LU, cross-correlation lag 0, A/V offset 0 samples by construction and ≤ 1 ms measured by flash/beep patterns |

- **The CPU reference compositor** (`fin-compositor::reference`, a test-only feature) implements every stage of §5.1 in `f32` with `libm`, scalar and in a fixed order. It is the oracle: GPU output is never compared with stored images, so no golden images are committed and a golden is portable across adapters. Its own output is pinned by BLAKE3 hashes, which catch drift in the oracle itself.
- **Adapters in CI:** lavapipe on the Linux runners, WARP on Windows, and the runner's Metal device on macOS where the runner exposes one (unverified for GitHub's arm64 runners; if absent, the macOS GPU rows run in host validation, as 4A/4B's cross-host rows do). Hardware GPU rows (Apple M-series, an AMD/NVIDIA desktop GPU, an Intel iGPU) run in each sub-batch's validation task.

### 13.3 Fixtures

- **Generated sources:** `fin-media::testing` draws frames with a **frame-id pattern**: a block code carrying the frame index and a checksum, robust to lossy coding, in a corner of an otherwise varied picture (gradients, colour bars, moving edges, text). Decoders, the sampling rule, edit lists and VFR are checked by reading ids back from decoded frames.
- **Encoded at test time:** SVT-AV1 (every OS) and the OS encoders (platform tests) encode the patterns into MP4 with `fin-container`'s muxer. VFR, edit-list, rotation, HDR-tagged and 10-bit variants are written by tweaking the muxed boxes.
- **Audio:** tones, sweeps, silences and beeps in sync with flash frames, generated in code.
- **Real footage:** a local fixture cache (`FIN_MEDIA_FIXTURES`), filled from rights-cleared or owner-made clips (arch §26 question 10), required on validation hosts (`FIN_REQUIRE_MEDIA=1`) and skipped elsewhere with a notice. Never committed; permissive licences only (AGENTS.md).

### 13.4 Determinism rules

- `fin-captions`, `fin-audio`'s mixer and the CPU reference use `libm` for transcendental functions, scalar code in a fixed order, and no `mul_add`. A gate like 4C's float gate (`check-eval-floats.mjs`) refuses platform float calls in those modules.
- GPU shaders may use any WGSL built-in; their variance is what T1 absorbs.
- `RENDER_VERSION` is bumped by any change that alters T0 hashes or T1 references on purpose; a test fails when T0 hashes change without a bump.

### 13.5 Conformance suite for codec backends

A backend (built-in or a plugin, arch §14.4) passes when it: decodes the generated fixture set to the right frame ids (T3 where it claims bit-exactness); seeks to any frame within its declared cost; reports its capabilities truthfully; survives truncated and corrupt input with typed errors; handles cancel within 2 s; and, for encoders, round-trips within T5 with exact frame counts and the requested GOP. The suite is a `fin-plugin-test` profile (arch §14.4) and runs against every built-in backend in CI.

## 14. Performance budgets

Budgets are CI gates where CI hardware can measure them, and validation-task gates on the reference machines otherwise (an Apple M-series Mac, a Windows desktop with an AMD RX-class GPU, a Linux Intel Gen12 machine).

| Area | Budget |
|---|---|
| Preview playback, 1080p proxy, 5 layers + captions + 1 Lottie | 60 fps, ≤ 0.1 % drops (arch §17, measured 0.06 %) |
| Preview composite (GPU time per frame, same scene) | ≤ 4 ms p95 on an iGPU |
| Scrub to a new frame (1080p short-GOP proxy) | ≤ 50 ms p95 local; ≤ 100 ms p95 remote on `540` |
| A/V sync, preview | \|offset\| p95 ≤ 16 ms compensated (arch §27.2 measured 7.4 / 9.1 ms); CI budget ±1 frame at 30 fps (arch §12.7) |
| A/V sync, export | 0 samples |
| `fin look` one 1080p frame, warm worker, from proxy | ≤ 300 ms p95; cold worker ≤ 1.5 s |
| Worker spawn to `hello` | ≤ 150 ms |
| Proxy `540`, hardware H.264 | ≥ 10× real time (measured 18–27×) |
| Proxy `540`, SVT-AV1, 4 threads | ≥ 5× real time (measured 9–22×) |
| Export 1080p30, hardware H.264, 60 s reel with captions and one Lottie | ≥ 4× real time |
| Export 2160p30, hardware HEVC/H.264 | ≥ 1× real time |
| Mixer, native, 16 stereo items | ≥ 200× real time |
| AudioWorklet render quantum (128 frames, 8 items) | ≤ 0.5 ms |
| Lottie frame, 1080p | ≤ 5 ms (measured ≤ 2.9 ms) |
| Caption block raster, 2 lines at 1080p | ≤ 2 ms, cached thereafter |
| Export memory | ≤ 1.5 GiB resident per 2160p export worker |

## 15. Diagnostics (arch §21.1)

- **Path:** workers send `event` messages (§3.3) with typed payloads; the supervisor forwards them to the `fin-node` journal, which stays the only writer. The supervisor sets `source: worker` (a new `Source` variant, additive as arch §21.1 allows) from the transport, never from the payload. The `config` message carries the diagnostics gate, so a worker builds no payload when diagnostics are off.
- **Kinds:**

  | Kind | v | Payload (all `meta` unless noted) |
  |---|---|---|
  | `media.capabilities` | 1 | once per session: per codec and profile, decode/encode `hw|sw|none`; backend names from a static table; GPU backend, adapter type, vendor and device ids (owner question 7); copy or zero-copy path |
  | `media.job` | 1 | job kind, outcome (`ok|failed|cancelled|retried|poisoned`), error code, codec and backend names, resolution bucket, frames, duration, real-time factor, attempts, cache hit |
  | `worker.exit` | 1 | role class, reason (`ok|crash|timeout|killed|oom|orphaned|protocol`), exit code or signal number, uptime, jobs served |
  | `render.stats` | 1 | per-minute histograms (like `eval.stats`): decode, composite, readback, encode micros per backend |
  | `preview.stats` | 0 | reserved for sub-project 9: drops, A/V offset histogram, scrub latency |
  | `render.sample` | 0 | reserved, training only: a sampled output frame as a `frame` blob (arch §21.1: frames are always blobs; a later sub-project adds the source) |

- **Never recorded:** file names or paths (a source is `{root, ext, hash}`), adapter name strings from drivers, decoder error strings that may hold paths (they are `scrubbed` and `content` like every error message), frames or audio outside `render.sample`.
- Worker panics become `error.panic` with `source: worker`, as arch §21 "Crash reporting" says.

## 16. Licensing

### 16.1 Rules applied

- No GPL/LGPL/AGPL/SSPL/EUPL in shipped code (AGENTS.md, arch §18). FFmpeg/libav*, x264, x265, libde265 (LGPL), GStreamer (LGPL), Rubber Band (GPL) and SoundTouch (LGPL) are excluded. FFmpeg is reachable only as a user-installed process plugin from `finisher-plugins-copyleft`.
- MPL-2.0 is already on the `deny.toml` allow list. It is file-level copyleft: Finisher uses MPL crates unmodified, and any patch to an MPL file is published as that file.
- H.264, HEVC and AAC only through OS or hardware codecs (arch §18).

### 16.2 Components

| Component | Licence (SPDX) | Patent terms | Use |
|---|---|---|---|
| wgpu, naga | MIT OR Apache-2.0 | — | compositor |
| `rav1d` | BSD-2-Clause | AOMedia Patent License 1.0 (AV1) | AV1 decode |
| dav1d (fallback) | BSD-2-Clause | AOMedia Patent License 1.0 | AV1 decode if `rav1d` falls short |
| SVT-AV1 | BSD-3-Clause-Clear (since v0.9) | AOMedia Patent License 1.0 (the BSD-3-Clause-Clear text itself grants no patent rights; the grant is in `PATENTS.md`) | AV1 encode |
| libvpx | BSD-3-Clause | Google's WebM patent grant (`PATENTS`) | VP9 |
| libopus | BSD-3-Clause | royalty-free grants listed in its `COPYING` | Opus |
| symphonia 0.6 | MPL-2.0 | MP3 patents expired (widely reported, not re-verified); FLAC, Vorbis, ALAC royalty-free | audio decode |
| `rubato` | MIT OR Apache-2.0 | — | resampling |
| `ebur128` | MIT | — | loudness |
| thorvg (C++) | MIT | — | Lottie (native); its optional bundled loaders' licences are checked in 6D |
| dotlottie-web | MIT | — | Lottie (preview) |
| `resvg`, `usvg` ≥ 0.45 | Apache-2.0 OR MIT (MPL-2.0 up to 0.44; never use those) | — | SVG |
| `tiny-skia` | BSD-3-Clause | — | SVG raster |
| `swash` | Apache-2.0 OR MIT | — | glyph raster |
| `cosmic-text` | MIT OR Apache-2.0 | — | shaping (4C) |
| `objc2`; `objc2-av-foundation`, `objc2-video-toolbox` | MIT; Zlib OR Apache-2.0 OR MIT | OS codec, licensed by Apple to the device (to confirm with counsel) | macOS codecs |
| `windows`, `windows-sys` | MIT OR Apache-2.0 | OS codec (to confirm with counsel) | Media Foundation, D3D11VA |
| libva (system library, loaded at run time) | MIT | drivers are the system's (owner question 3) | VA-API |
| `cros-libva` (or our own FFI) | BSD-3-Clause | — | VA-API binding |
| Mesa lavapipe (Docker image only) | MIT (Mesa) | — | software Vulkan without a GPU |
| `memmap2` | MIT OR Apache-2.0 | — | shared-memory frames |
| `wasm-bindgen` | MIT OR Apache-2.0 | — | `fin-wasm` |

**Considered and not used**

| Component | Why |
|---|---|
| openh264 (BSD-2-Clause) | the source carries no patent licence; Cisco covers royalties only for its own binary downloaded at install or run time (openh264.org FAQ). Its decoder targets Constrained Baseline, which phone footage is not |
| fdk-aac | its own licence, not OSI-approved, with an explicit clause that grants no patent rights |
| `mp4parse` (MPL-2.0) | demux only, and Finisher needs one container reader that also writes |
| `mp4` 0.14 (MIT) | last release 2023; no edit lists. `fin-probe` uses it today and moves to `fin-container` in 6A |
| `muxide`, `mp4-atom`, `re_mp4` | young or demux-only; `fin-container` is small enough to own and must be fuzzed anyway |
| `svt-av1-sys`, `vpx-sys`, `audiopus` | unmaintained bindings; own FFI instead |

### 16.3 Native-library gate

`cargo deny` sees a `-sys` crate's licence, not the C library it builds. 6A adds `third_party/native.json`: each vendored native library's name, pinned tag and commit, SPDX licence, patent-licence file and notice path. `scripts/check-native-deps.mjs` (in `pnpm gates`) fails when a `build.rs` builds a source tree that is not listed, when a listed licence is not allowed, or when `THIRD_PARTY_NOTICES.md` lacks an entry. Vendored sources are git submodules at pinned commits under `third_party/`. `BSD-3-Clause-Clear` and the AOMedia Patent License 1.0 are added to the allow list of that gate (not to `deny.toml`, which never sees them).

### 16.4 Patents (for counsel; arch §26 question 1)

- AV1 (AOMedia Patent License 1.0), VP9 (Google's grant) and Opus (its contributors' grants) are royalty-free with defensive termination clauses.
- H.264 is not patent-free in 2026: community tracking lists patents in force in the US and Brazil until about 2030. HEVC has two pools (Access Advance, Via LA) and active litigation. AAC is licensed through Via LA; Finisher found no primary source on its expiry. All three reach users only through codecs their OS or GPU driver provides.
- Whether an OS vendor's or GPU vendor's codec licence covers an application that drives it, and whether a Docker image that includes VA-API drivers distributes codecs, are questions for counsel (owner question 3).

## 17. Error codes

All errors use the one shape `{code, message, details, fix?}` (arch §19). Retryable codes carry `details.retryable: true`. Each code gets its `fin` exit code in `crates/fin/src/output.rs`.

| Code | When | `details` / `fix` |
|---|---|---|
| `UNSUPPORTED_CONTAINER` | (exists) a container `fin-container` cannot read | `format`; fix: the codec plugin |
| `UNSUPPORTED_TRANSFORM` | a track matrix other than a multiple of 90° | |
| `MEDIA_CORRUPT` | a structurally broken file or sample | `asset`, `at` (byte offset or frame) |
| `CODEC_UNAVAILABLE` | no decoder or encoder for codec/profile/bit depth on this machine | `codec`, `profile`, `os`, `direction`; fix: the capability report's advice |
| `DECODE_FAILED`, `ENCODE_FAILED` | the backend reported an error | `backend`, `frame`; retryable once |
| `ENCODER_HUNG` | a hardware encoder produced no output within its deadline (arch §27.3) | `backend`, `adapter`; retryable on another backend |
| `GPU_UNAVAILABLE` | no adapter, not even a software one | fix: install Mesa / use server-rendered preview |
| `GPU_DEVICE_LOST`, `GPU_OUT_OF_MEMORY` | device loss or allocation failure | retryable |
| `WORKER_CRASHED`, `WORKER_TIMEOUT` | the worker died or went silent | `reason`, `attempts`; retryable |
| `WORKER_PROTOCOL` | a version or message mismatch | `expected`, `got` |
| `INPUT_POISONED` | the same input crashed two workers | `asset`, `recipe`; fix: relink or report the file |
| `JOB_CANCELLED` | cancelled by the user or a shutdown | |
| `ORIGINALS_MISSING` | export or a QA render needs originals that are unreachable | `assets` |
| `ASSET_CHANGED` | an original's fingerprint no longer matches | `asset`; fix: relink |
| `PROXY_UNAVAILABLE` | preview needs a proxy that does not exist yet | `asset`, `tier`, `job` (the job making it) |
| `CACHE_IO` | the cache volume failed or is full | `freeBytes` |
| `LAYER_UNAVAILABLE` | a layer cannot be drawn (summary for export preflight) | `items: [{item, code}]` |
| `CAPTIONS_UNMEASURED` | a caption in range has no layout | `items`, `keys`; fix: `fin fonts fetch` |
| `MATTE_MISSING` | a mask's matte artifact is absent | `item`, `matte` |
| `TEMPLATE_INVALID` | a Lottie template failed validation or crashed the check | `asset`, `reason` |
| `SVG_UNSAFE`, `SVG_INVALID` | sanitising refused or parsing failed | `reason` |
| `LUT_INVALID` | a `.cube` failed to parse | `line` |
| `SOURCE_OUT_OF_RANGE` | (warning in results) a source time past the asset | `item`, `time` |
| `EXPORT_TEMPLATE_UNSUPPORTED` | a template's codec or level cannot be produced here | `missing` |
| `EXPORT_DESTINATION` | the destination is not writable or is inside `.finisher/` or `assets/` | |
| `LOUDNESS_TARGET_UNREACHABLE` | (warning) normalisation could not reach the target | `measured`, `target` |
| `PLAN_MISMATCH` | a worker's plan or caption layout differs from `fin-node`'s | `at`, `expected`, `got`; internal |

## 18. Crate and file map

| Crate / package | New? | Responsibility | wasm32 |
|---|---|---|---|
| `fin-media` | new | pixel formats, colour tags and matrices, `MediaInfo`, the sample table and `frame_at`, `RENDER_VERSION`, the worker protocol and job specs, capability and template types; test pattern generators (`testing`) | yes, pure |
| `fin-container` | new | ISOBMFF and WebM demux, MP4 and WebM mux | yes, pure |
| `fin-codecs` | new | `Decoder`/`Encoder` traits; backends `apple`, `mf`, `vaapi`, `nv` (6G), `av1` (rav1d, SVT-AV1), `vpx`, `opus`, `audio` (symphonia, OS AAC); the process backend; the capability probe | no |
| `fin-compositor` | new | render graph, WGSL, effects, blend, masks, ingest, readback; `reference` (CPU oracle) | yes (WebGPU) |
| `fin-audio` | new | mixer, resampling, WSOLA, loudness, limiter, waveform peaks | yes, pure |
| `fin-captions` | new | caption and text-slot raster over `fin-text` and `swash` | yes, pure |
| `fin-graphics` | new | `lottie` (thorvg FFI, native only), `svg` (sanitise, raster), `image` | partly (svg, image) |
| `fin-workers` | new | the `fin-worker` binary: roles, protocol loop, job runners, watchdogs | no |
| `fin-wasm` | new | `wasm-bindgen` exports: replay and eval, compositor, mixer, demux | yes |
| `fin-text` | changed | glyph-level `shape` beside `measure` (6D) | yes |
| `fin-fonts` | changed | a read-only `FontCache` usable without the fetcher (feature split), for workers | — |
| `fin-probe` | changed | media probing through `fin-container` (6A); other kinds unchanged | — |
| `fin-core` | changed | plan growth (§4.3), mask and nested edits (6D), `compose::duck` (sub-project 8), new event kinds and `Source::Worker` | yes |
| `fin-node` | changed | snapshots and manifests for jobs, plan samples, the media cache index, the capability cache, journal forwarding | — |
| `fin` | changed | `look`, `render`, `listen`, `media caps|proxy|status`, `cache status|gc`, `wait` for media jobs | — |
| `finisherd` | changed | `src/media/`: supervisor, pools, jobs, cache HTTP endpoints, RPCs `media.*` | — |
| `@finisher/contracts` | changed | `media.look`, `media.render`, `media.listen`, `media.jobs.*`, `media.capabilities`, `media.proxy.*`, `media.cache.*` | — |
| `@finisher/preview` | new (6F) | decoder pool, scheduler, AudioWorklet glue, dotlottie-web, harness | — |

**Dependency rules:** `fin` links no media crate. `fin-node` links `fin-media` and `fin-container` (probe, manifests) but never `fin-codecs`, `fin-compositor`, `fin-graphics` or wgpu. Only `fin-workers` and `fin-wasm` link the compositor.

## 19. Sub-batches and order

Each sub-batch gets its own plan, is green on all three OSes before the next starts, and ends with a validation task on real hardware.

| Batch | Delivers | Depends on |
|---|---|---|
| **6A Foundations and first pixels** | `fin-media`, `fin-container` (ISOBMFF demux and MP4 mux), the rich shallow probe and media index, software AV1 (`rav1d` decode, SVT-AV1 encode), the worker binary and protocol, the supervisor and media jobs in `finisherd`, the CPU reference and the wgpu compositor core (ingest, base box, transform, crop, opacity, blend modes, transitions), `fin look`, the native-library gate, the first diagnostics kinds. OS-neutral: every OS runs the same software path | 4C |
| **6B Platform codecs and proxies** | VideoToolbox, Media Foundation/D3D11VA and VA-API decode and encode; the capability report; HDR → SDR tone mapping; `MediaInfo` display size and rotation in the model; proxies, thumbnails, filmstrips; the cache manager and eviction; HTTP serving; WebM demux; deep probe | 6A |
| **6C Audio** | audio demux and decode (symphonia, OS AAC, Opus), resampling, PCM cache, waveforms, the native mixer with pan, curves and WSOLA, loudness and the limiter, `fin listen` | 6A (6B for OS AAC) |
| **6D Layers** | effects (`colorAdjust`, `lut`, `blur`), adjustment layers, nested items (with their edits), masks (with their edits), images, SVG, Lottie and `templateCheck`, captions raster with the plan growth of §4.3 and caption animation | 6A |
| **6E Export** | format templates, MP4/WebM mux of encoded streams with priming edit lists, two-pass loudness, the export report and verification, `fin render`, encoder fallbacks | 6B, 6C, 6D |
| **6F Preview** | `fin-wasm`, `@finisher/preview` (decoder pool, scheduler, worklet mixer, dotlottie-web), server-rendered fallback, the harness, parity goldens and latency in Chromium and Electron | 6B, 6C, 6D |
| **6G Hardening** | zero-copy interop, NVDEC/NVENC on Linux, Dolby Vision profile 5 on macOS, fuzzing at full scale, performance gates on reference hardware, the worker OS sandbox (with sub-project 7's broker), the cross-platform validation matrix | all |

6C and 6D can run in parallel after 6A (they touch different crates); 6E and 6F can run in parallel after both.

## 20. Decisions taken here

| # | Decision | Where |
|---|---|---|
| 1 | Workers evaluate plans themselves from a pinned snapshot; `fin-node` sends plan-hash samples that the worker must match | §3.4 |
| 2 | One `fin-worker` binary with roles; pools per class; one job per worker at a time | §3.1 |
| 3 | NDJSON control over stdio; bulk data in memory-mapped files; protocol types generated from Rust | §3.3 |
| 4 | Own Rust containers on every OS; OS APIs only at the codec level | §6.1 |
| 5 | `rav1d` for AV1 decode, with dav1d as the measured fallback; SVT-AV1, libvpx, libopus through own FFI from pinned sources | §6.4 |
| 6 | Proxies keep one frame per original frame, stamped at a constant rate; the sampling rule is sample-and-hold on the original's table | §7.1, §10.2 |
| 7 | `rgba16float` intermediates, 8-bit sources and output (amends arch §12.8) | §5.1 |
| 8 | Blending and `crossfade` in display-encoded space; `dissolve` in linear light | §4.3, §5.1 |
| 9 | Ducking is gain keyframes made at edit time, not a mixer feature (amends arch §12.7) | §8.5 |
| 10 | `equalPower` = `sqrt`; constant-power pan for mono, balance for stereo; pitch-preserving WSOLA for speed ≠ 1 | §8.2 |
| 11 | The CPU reference compositor is the golden oracle; no golden images are committed | §13.2 |
| 12 | Colour emoji from a bitmap (CBDT) build until a COLRv1 painter exists | §11.4 |
| 13 | Media jobs in `finisherd` now, absorbed by sub-project 5 later (pending owner question 5) | §3.4 |

## 21. Owner questions

1. **Linux AAC.** Linux has no OS AAC decoder or encoder, and AAC is the audio of nearly all phone footage and what TikTok and Instagram ask for. Options: (a) decode AAC-LC with symphonia (MPL-2.0, unmodified) and, for export on Linux, use AAC from the codec plugin when installed, else Opus-in-MP4 with a capability warning; (b) ship no AAC at all on Linux (AAC footage needs the plugin even to be edited); (c) license AAC from Via LA and ship an encoder. **Recommendation: (a)**, with counsel checking the patent exposure of a shipped AAC-LC *decoder*. Trade-off: (a) makes Linux and the NAS Docker image usable for editing phone footage at some patent risk; (b) is the cleanest posture but makes the headless/NAS server nearly useless for the main use case; (c) costs money and a non-permissive encoder.
2. **Electron's bundled ffmpeg.** Electron ships Chromium's media library as a separate dynamic library (`libffmpeg.so`, `ffmpeg.dll`, `libffmpeg.dylib`), built from FFmpeg under LGPL-2.1. This conflicts with arch §18 "no LGPL in the shipped app", and no Electron build avoids it (to verify against the pinned Electron version in 6F). Options: (a) a documented exception for that one dynamically linked browser-runtime component: replaceable, never called by Finisher code, notice shipped; (b) remove it, which breaks WebCodecs software fallback and `<video>` in the desktop app. **Recommendation: (a)**; the licence gate gets an exact-path exception and a test that no Finisher binary links it. Trade-off: the rule becomes "no LGPL in Finisher's code" rather than "in the app".
3. **Patent posture of OS and hardware codecs, and the Docker image.** H.264/HEVC/AAC through the OS (VideoToolbox, Media Foundation) leans on the OS vendor's licences. On Linux, VA-API drivers (Intel's media driver, Mesa) implement those codecs, and some distributions build Mesa without them for patent reasons. If the server Docker image includes these drivers, Finisher distributes codec implementations. Options: (a) the image includes no hardware codec drivers; users mount the host's or build a documented derived image; (b) the image includes them. **Recommendation: (a) until counsel answers arch §26 question 1.** Trade-off: (a) adds a setup step for NAS users with GPUs; (b) is one-click but takes on the distribution question.
4. **Default base box: contain, cover or pixel size.** A clip whose aspect differs from the timeline (16:9 footage on a 9:16 timeline) can be (a) fitted inside with bars, (b) scaled to fill and cropped, or (c) drawn at its pixel size. **Recommendation: (a) contain**, as CapCut and Premiere do by default, with a `fit` parameter (contain/cover/none) added in 6D. Trade-off: reframe (sub-project 8) then keyframes a larger scale to fill 9:16; (b) would suit 9:16 shorts better out of the box but crops silently. The choice is hard to change later: it fixes how every existing project looks.
5. **Media jobs before sub-project 5.** The engine needs jobs (queue, progress, cancel, `fin wait`), which arch §24 places in sub-project 5, not built yet. Options: (a) sub-project 6 builds media jobs in `finisherd` with the wire shape of arch §9 item 9, and sub-project 5 generalises them; (b) build sub-project 5's job system first. **Recommendation: (a).** Trade-off: some rework when sub-project 5 lands, against blocking 6A on another sub-project.
6. **When workers get an OS sandbox.** Workers parse untrusted media (agents download files). Options: (a) ship 6A–6F with the env allow-list, no credentials and a write restriction by convention, and add the OS sandbox in 6G with sub-project 7's broker; (b) build a worker sandbox (Seatbelt, Landlock, AppContainer) in 6A. **Recommendation: (a).** Trade-off: a decoder exploit in a worker before 6G can write what the user can; (b) adds several weeks to 6A and duplicates sub-project 7's broker.
7. **GPU identifiers in diagnostics.** The diagnostics plan deferred this to sub-project 6. Options: (a) record GPU vendor id, device id (PCI numbers), backend, adapter type and driver major version as `meta`, never the driver's name string; (b) record nothing about the GPU. **Recommendation: (a)**: GPU bugs dominate media-engine failures, and the ids are numbers from a fixed space. Trade-off: device ids narrow down a machine more than OS and core count do.
8. **Platform scope of 6B, and NVIDIA on Linux.** Options: (a) 6B ships VideoToolbox, Media Foundation (AMD, NVIDIA and Intel hardware MFTs by adapter) and VA-API (Intel, AMD), all three required for the merge; NVDEC/NVENC on Linux (arch §12.1) moves to 6G; (b) macOS first, merging 6B on macOS alone and following with Windows and Linux; (c) NVDEC/NVENC in 6B too. **Recommendation: (a)**, which keeps sub-project 4's "green on all three OSes" rule; the spike validated exactly these paths (arch §27.3), and NVIDIA on Linux was not measured. Trade-off: (b) gets the owner's own machine to pixels sooner but lets the other paths drift; (c) serves NAS boxes with NVIDIA cards (common on Unraid) earlier at the cost of an untested path in 6B.

## 22. Contradictions and gaps found in earlier documents

1. **Arch §18 vs Electron:** Electron's bundled `libffmpeg` is LGPL-2.1 (owner question 2).
2. **Arch §12.7 vs the plan contract:** ducking in the mixer would need analysis data that `AudioPlan` does not carry (resolved by §8.5).
3. **Arch §12.3 "consumes eval output only" vs `FramePlan`:** caption paint and placement, caption animation, matte time and transition looks are missing or undefined (resolved by §4.3); the engine also needs asset facts, which the render manifest (§4.2) provides without timeline semantics.
4. **Arch §12.8 "8-bit internally"** vs float intermediates (amended by §5.1).
5. **Arch §12.8 "frame blending or nearest-frame, chosen per item"** has no model parameter (§10.2).
6. **Arch §1 "animated captions"** vs a model with no caption animation fields (§4.3).
7. **Core §5.6 and arch §27.10 say fonts are "always bundled"**; the 4C plan (Decisions 29–39) and the code fetch them on demand into a pinned, content-addressed cache and bundle none. The condition that matters (pinned bytes, never system fonts) holds either way; core §5.6 and arch §27.10 should be reworded when 4C's Task 28 records its decisions.
8. **`fin-probe` vs arch §27.3:** rotation, VFR and edit lists are not handled (§6.3).
9. **Arch §24 order:** sub-project 3 (walking skeleton with proxy preview) and sub-project 5 (jobs) are not built, yet sub-project 6 depends on jobs and absorbs the skeleton's preview client (owner question 5, §12).
10. **Arch §12.1 lists NVDEC/NVENC for Linux** without spike evidence (owner question 8).
11. **Core §5.1 transitions** name `crossfade` and `dissolve` without a visual difference (§4.3).
12. **Arch §21.1 `source`** has no variant for workers (additive `worker`, §15).

## 23. Risks

| Risk | Mitigation |
|---|---|
| GPU variance across vendors breaks T1 | the CPU oracle isolates shader bugs from tolerance issues; per-stage tolerance tests; WARP/lavapipe as CI baselines; hardware rows in each validation task |
| `rav1d` slower or less mature than dav1d | 6A measures both; dav1d FFI is the fallback |
| Hardware encoders hang or pick the wrong adapter (arch §27.3) | explicit adapter binding; `ENCODER_HUNG` with a deadline and fallback to the next backend; capability probe encodes a test clip |
| HEVC on Windows without the extensions is unverified (arch §27.11) | 6B validation row on a machine without them; D3D11VA path; guidance |
| Linux desktop WebGPU and real vsync are unverified (arch §27.11) | server-rendered fallback; 6F validation on a real Linux desktop |
| Zero-copy interop needs wgpu hal APIs that change between versions | copy path first and always available; interop behind a feature, pinned wgpu |
| thorvg crashes on malformed templates | `templateCheck` in a throwaway worker; export in the render worker; retries and poisoning |
| Patent exposure (H.264/HEVC/AAC) | OS/hardware only; owner questions 1 and 3; counsel before launch |
| The engine grows into many crates and long files | crate map fixed here; 600-line limit; one module per stage |
| Media jobs diverge from sub-project 5's design | wire shape from arch §9 item 9; sub-project 5 owns the generalisation |
