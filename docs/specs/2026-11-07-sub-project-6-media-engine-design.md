# Sub-project 6: Media engine — Design

- **Status:** revision 2 (2026-11-07), after an independent review of revision 1 (`e6f7e64`); §24 maps each finding to its resolution. 6A-1 is built (branch `sp6a1-media-foundations`; "As built (6A-1)" notes in §5.1, §6.1, §6.4, §10.1, §13.4, §17). The plan for the first sub-batch (6A) is `docs/superpowers/plans/2026-11-07-sub-project-6a-media-foundations.md`.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §4 (rule 3), §7, §12, §17 (performance budgets), §18, §21.1, §22 and §27.2–27.3 and §27.10 for sub-project 6. Where the two disagree, this document wins for sub-project 6, and arch §27 gets a pointer when a sub-batch merges.
- **Builds on:** sub-project 4 (`docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`, cited as *core §n*): `fin-core::eval` (core §5.4), captions and `fin-text` (core §5.5–5.6), the asset kinds (core §5.7), and the diagnostics journal (arch §21.1).
- **Siblings** (all in revision; cited by branch): sub-project 5 (*cp §n*, `sp5-design`, `docs/specs/2026-11-07-sub-project-5-control-plane-design.md`): jobs, the worker binary, the library stage framework; sub-project 7 (*ar §n*, `sp7-design`): `fin-sandbox`; sub-project 8 (*pl §n*, `sp8-design`): plugins, analyzers, codec and matte interfaces. **Ownership splits fixed by the coordinator** are stated where they apply and summarised in §3.6.
- **Licence facts** in §16 were checked against primary sources on 2026-11-07 (each project's LICENSE/COPYING/PATENTS file, crates.io metadata). Facts still to verify are marked *(verify)*. They are not legal advice; arch §26 open question 1 (counsel on codec patents) still stands.

## 1. Goal, scope and success

Sub-project 6 turns plans into pixels and sound. It delivers:
- decoding and encoding of the formats creators use, through OS and hardware codecs plus royalty-free software codecs (D13);
- probing that tells the truth about a file: display size, rotation, colour, HDR, variable frame rate, edit lists;
- proxies, PCM (48 kHz for mixing, 16 kHz mono for analysis), waveforms, thumbnails, filmstrips and analysis frame feeds, cached locally and content-addressed (arch §12.2);
- one wgpu compositor, native for `fin look`, QA renders and export, and WASM/WebGPU for client preview (D11);
- `fin-audio`, the mixer, native and WASM (arch §12.7), with the built-in audio effects (EQ, compressor, gate, de-esser) and loudness measurement and normalisation on export;
- rasterised captions, images, SVG, Lottie templates and masks;
- the colour and frame-rate policy of arch §12.8;
- export to the format templates social platforms expect;
- the preview engine (decoder pool, scheduler, AudioWorklet mixer) as a headless TS package, without UI;
- the **mapped-file bulk channel** (`fin-shm`) that media workers, codec plugins and analyzers share.

**The contract:** the engine renders exactly what `eval_frame` and `eval_audio` say (arch §12.3, core §5.4). It never reads the timeline model to decide *what* to draw or play. Where a plan leaves something out that rendering needs, the plan grows (§4.3); the engine does not reach around it.

**Success, for every sub-batch:**
- every capability is driven and tested through `fin` → `finisherd` (`fin look`, `fin render`, `fin listen`, `fin media`, `fin cache`), and the preview through a headless Playwright harness;
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS arm64, Windows 11 x64 and Linux x64;
- the golden-frame and audio suites (§13) pass at their tolerances on every OS and on wasm32;
- the licence gates cover native libraries, run-time-loaded libraries and the Docker image (§16.3).

**Non-goals** (each has an owner elsewhere):
- UI: the editor, the timeline, the inspector, the preview widget (sub-project 9). Sub-project 6 ships the preview engine and a test harness page only.
- **The job model, scheduler, worker binary, kill tree, retries and poison handling, `jobs.*` RPCs and `fin wait`** (sub-project 5C, cp §6). Sub-project 6 contributes the `media` worker family and its job kinds (§3).
- **The OS sandbox** (sub-project 7, `fin-sandbox`). Sub-project 6 states what the `worker` profile must allow (§3.5).
- Analysis: transcription, alignment, scenes, embeddings, voice activity, matte producers, beats (sub-project 8). The engine feeds them PCM and frames (§7.2) and renders the mattes they produce.
- Ducking. It is compiled into gain keyframes at edit time (§8.5), so the mixer never needs analysis data.
- Audio effects beyond the built-in four (plugins, sub-project 8: offline processors such as de-noise).
- HDR export and a 10-bit working pipeline (arch §12.8: types are prepared, not built).
- Raw agent-authored Lottie (after v1, arch §12.5).
- An FFmpeg codec backend. Sub-project 6 defines the process codec backend protocol and its conformance suite; the plugin lives in the copyleft repo (arch §18), its plugin packaging in pl §10.1.
- WebCodecs encoding (arch §27.2: it hangs on Windows; encoding stays in `fin-codecs`).
- Background removal (after v1); sub-project 6 renders mattes that plugins produce.

**Cut from v1 (accepted after review):** the WebM muxer and libvpx encoding (VP9 decode stays); Dolby Vision profile 5; the playback half of server-rendered preview (scrub frames stay, §12); hardware AV1 encoding (SVT-AV1 is the only AV1 encoder in v1, §6.2); the per-fingerprint media index files (`MediaInfo` is computed where it is needed, §6.3).

## 2. Principles

1. **Plans in, pixels out.** A render is a pure function of (frame plan, render manifest, source pixels, render version). The render manifest (§4.2) holds only what an asset *is* (its file, fingerprint and media facts), never what the timeline does with it.
2. **Native work that can crash runs in a worker process** (arch §4 rule 3), under sub-project 5's job system. `finisherd` and `fin-node` never link a codec, a GPU driver, thorvg or resvg. A worker crash, hang or out-of-memory is a job failure handled by 5C, never a server failure.
3. **One sampling rule everywhere.** Preview, `fin look`, QA and export choose the same source frame for the same source time (§10.2): the **nearest frame**, whether they read a proxy or the original.
4. **Determinism has tiers** (§13.2). CPU rasters (text, the CPU reference compositor) are byte-identical on every OS and on wasm32. GPU output is checked against the CPU reference within a stated tolerance. Encoded output is checked after decoding.
5. **Every intentional change to output bumps `RENDER_VERSION`.** It is part of every cache key and every golden, as `TextMeasurer::fingerprint` is for layouts.
6. **Copy first, zero-copy later.** Every GPU path has a CPU-copy fallback that is correct and measured. Zero-copy interop (IOSurface, D3D11 shared handles, DMA-BUF) is an optimisation behind it (§5.4).
7. **Nothing renders silently wrong.** A layer the engine cannot draw (an unsupported codec, a missing font, a missing matte) is reported by item id and code, in the job result and in `fin look`'s answer. Export refuses; preview and `fin look` draw the rest and say what they left out.
8. **No media in the repository.** Test media is generated in code at test time (§13.3), as `fin-probe` fixtures already are. Real footage for validation comes from a local, rights-cleared fixture cache and is never committed.
9. **Workers never start processes.** A codec plugin or other helper a media job needs is started by 5C as a sibling of the worker, connected only through the job's mapped files (§3.4).

## 3. Architecture

### 3.1 Processes and ownership

```
 finisherd (Node, TS) ───────────────────────────────────────────────────────────────
  RPC gateway · 5C job scheduler (jobs.*, fin wait) · media job kinds (sp6, jobs/kinds/media.*)
  media HTTP endpoints · journal forwarding
        │ napi (fin-node)                     │ 5C: spawn, kill tree, liveness, retries, poison
 fin-node (Rust, in-process)                  ▼
  project engines · eval · job inputs     fin-workers media   (5C's binary, sp6's family)
  plan samples · media cache index         ├─ links fin-core (eval), fin-text, fin-container, fin-codecs,
  5C host side (job objects, cgroups)      │  fin-compositor (wgpu), fin-audio, fin-captions, fin-graphics
                                           └─ job dir: in/ (host-written, read-only) · out/ (worker-written)
                                        sibling helper (started by 5C when a job needs one):
                                           a codec plugin process, mapped rings in the same job dir
 Client (browser / Electron renderer)
  fin-wasm: fin-core + fin-container (demux) + fin-compositor (WebGPU) + fin-audio
  @finisher/preview: WebCodecs decoder pool, scheduler, AudioWorklet glue, dotlottie-web
```

- **5C owns** (cp §6): the job model and store (`control.db`), the scheduler and its pools, the single `fin-workers <family>` binary and its envelope (the `fin-jobs` crate: `hello`, `start`, `cancel`, `shutdown`; `begin`, `item`, `progress`, `checkpoint`, `alive`, `result`, `error`, `ready`), spawning through `fin-node`'s `jobhost` with the host-owned kill tree on every OS, warm workers (the `interactive:<family>` pools), helpers and the lifecycle of each job's mapped files, liveness (`WORKER_HUNG`) and stall detection (`JOB_STALLED`), retries, the isolation of a crashing batch (`POISON_ITEM`), restarts (`JOB_INTERRUPTED`), the `JobEngine` TypeScript API, `jobs.*` RPCs, `fin job …`, `fin wait`, and the `job.done` and `worker.exit` journal kinds (cp §6.0).
- **Sub-project 6 owns:** the `media` worker family (`fin-workers media`), the media job kinds and their params and results (§3.3), the format of the mapped files of media jobs and the crate that reads and writes them, `fin-shm` (§3.4; 5C creates, sizes, hands over and deletes the files, never reads them), the media crates, the media cache and its HTTP endpoints, the `media.capabilities` and `render.stats` journal kinds, and the media fields of `job.done` (its `kv` 2, §15).
- **No GPU in `finisherd`.** A GPU device loss or driver crash kills only a worker.
- **Remote media and the cloud.** A media job is self-contained (§3.3), so the same job can later run on another machine's worker pool (`WorkerPool`, arch §5) without a change to its format.

### 3.2 What sub-project 6 uses from 5C, and what it still asks

5C's revision 3 (cp §6.0–6.4) provides what media needs; sub-project 6 builds against it as follows:
1. **Warm workers** (cp §6.3): `media.look`, `media.previewFrames` and `media.capabilities` are `warm` kinds with priority `interactive` on the `interactive:media` pool (1 worker, at most 2), so a long export never delays them. The `fin look` budget (≤ 300 ms p95, §14) assumes a warm worker; `finisherd` calls `jobs.warm {family: media}` when a project opens. Between jobs a media worker keeps only caches keyed by content (the GPU device, compiled pipelines, open decoders and parsed `MediaInfo` keyed by fingerprint, size and mtime), as cp §6.3 requires, and a test runs the same `media.look` on a fresh and a reused worker and compares the PNG bytes.
2. **Pools:** batch media kinds take `gpu` (cp §6.3: 1) and, when they encode with hardware, **`hwenc`**, a pool sub-project 6 asks 5C to add, sized from the capability report's encoder session limit (2 where unknown).
3. **Mapped files** (cp §6.4): each media kind declares its files (`MappedFile {name, bytes, mode}`); 5C creates them in `<server-data>/jobs/<jobId>/` and deletes the directory when the job ends. Sub-project 6's layout inside it is `in/` (inputs written by `fin-node` before `start`: `snapshot.json`, `manifest.json`), `out/` (the worker's outputs, and the packet rings it writes for a helper) and `helpers/<id>/out/` (each helper's only writable place). **Ask:** a kind-level `onSucceeded` hook that runs before 5C deletes the directory, in which sub-project 6's handler moves outputs out of `out/` with `fin-shm::commit_output` (§3.4).
4. **Helpers** (cp §6.4): a codec plugin is a `HelperSpec {kind: codec, profile: worker}` sibling of the media worker, started by 5C, connected only through the job's mapped files; a helper that exits early fails the job with `HELPER_FAILED`. Workers never start processes (§2.9).
5. **Ask: memory limits that scale with RAM:** per family and priority, a resident limit enforced by the Windows job object (`JOB_OBJECT_LIMIT_PROCESS_MEMORY`), a cgroup `memory.max` where cgroup v2 is delegated, and the worker's own reported RSS elsewhere (`WORKER_OOM`). Media's defaults are in §14.
6. **Items:** `media.look`, `media.thumbs` and `media.frames` report each output as an `item`, so a crash keeps what was done and the in-flight item is retried alone (cp §6.5).
7. **Inside `finisherd`,** sub-project 6's RPC handlers submit through `JobEngine.submit(owner, spec)` (cp §6.7); only `--no-wait` forms of `fin` commands, which print a job id for `fin wait`, need 5C-2's RPC surface.

### 3.3 Media job kinds

Declared in `finisherd` (`jobs/kinds/media.*.ts`) with params and results generated from Rust (`fin-media::jobs`). Every kind runs on `worker:media`.

| Kind | Priority | Pools | Does |
|---|---|---|---|
| `media.look` | interactive (warm) | `interactive:media` | frames or a contact sheet of a timeline at given times, to PNG |
| `media.probeDeep` | normal | cpu | decode-level probe: the first frame, in-band HDR SEI |
| `media.proxy` | background | gpu, hwenc | a `540` or `1080` proxy (§7.1) |
| `media.pcm` | background | cpu | the 48 kHz `f32` stereo cache, or the 16 kHz mono analysis feed (§7.2) |
| `media.waveform`, `media.thumbs` | background | cpu (thumbs: gpu) | peaks; thumbnails and filmstrips |
| `media.frames` | normal | gpu | an analysis frame feed: frames at given times or a stride, at `maxSide`, `rgb8` or `gray8`, into a mapped ring (pl §5 `media.frames`) |
| `media.render` | normal | gpu, hwenc | a range to a file: QA renders and export (§9) |
| `media.templateCheck` | normal | cpu | renders sample frames of a new Lottie template in a throwaway worker (§11.1) |
| `media.previewFrames` | interactive (warm) | `interactive:media` | server-rendered scrub frames (§12) |
| `media.matteEncode` | normal | gpu, hwenc | encodes a matte producer's alpha frames as a `matte` asset (§11.3) |

- **Inputs** (written into the job directory's `in/` by `fin-node` before `start`; the paths reach the worker in `start.files`):
  - `snapshot.json`: the project state at a fixed `(branch, rev)`. The worker runs `fin-core::eval` itself, with a `fin-text` measurer over the font cache (read-only).
  - `manifest.json`: the render manifest (§4.2).
  - `planSamples`: BLAKE3 hashes of the frame plans `fin-node` computed for a few times (first, last, every 10 s of output; every time for `media.look`). The worker compares its own plans at those times and fails with `PLAN_MISMATCH` (internal) on any difference, which catches a version skew between `fin-node` and the worker or a font cache that changed under the job.
  - Why the worker evaluates instead of receiving plans: the job can be replayed alone (`fin-workers media --replay <jobdir>` reproduces a bug report), there is no per-frame IPC, and a remote worker pool needs nothing but the job directory. Eval is deterministic across platforms (core §5.4), and the samples prove it per job.
- **Outputs** go to `out/` only; sub-project 6's `onSucceeded` handler moves them to the cache (by rename, after the `fin-shm` checks) or to their destination (temp-then-rename, arch §6.7) before 5C deletes the directory.
- **Retries and poison** are 5C's (cp §6.4). A media input that is a poison item (`POISON_ITEM`) stays failed until its fingerprint or `RENDER_VERSION` changes, which 5C sees through the kind's `dedupe(params)` key.

### 3.4 The mapped-file bulk channel (`fin-shm`)

Frames and PCM never go over the stdio envelope. `fin-shm` is a small crate (sub-project 6) used by media workers, codec plugin helpers and sub-project 8's analyzers:
- **Layout:** each party has an `in/` written by the side that trusts it, read-only to the reader, and an `out/` it writes. A ring is a header (magic, version, slot size, slot count, a sequence per slot) plus fixed-size slots, in one file created at its final size.
- **Mapping rules:**
  - the **host never maps a file a less-trusted process can still write** (a truncation would fault the host, and the contents could change under a check): a host reads such outputs with bounded `pread` into its own buffers, or maps them only after the writer has exited;
  - files are opened with `O_NOFOLLOW` (Windows: `FILE_FLAG_OPEN_REPARSE_POINT` and a check that the handle is not a reparse point), must be regular files with `nlink == 1`, and are refused above the size declared in the ring header or the kind's cap;
  - a writer maps its own `out/` files; a reader maps another party's file read-only only when that party cannot write it any more (`in/` files are written before the reader starts).
- **Between a worker and a codec plugin helper:** the worker writes packet rings in its own `out/`, which the helper may map read-only; the helper writes frame rings in `helpers/<id>/out/`, which the worker, the more trusted side, reads by `pread` only (a copy: about 3 MB per 1080p NV12 frame, well within budget, §14). All ring files are 5C-created `MappedFile`s of fixed size (cp §6.4).
- **Slot handoff:** where the two sides share a control channel (the host and a plugin, pl §3.5), slot ownership moves by `slot {ring, index, seq, len}` and `release {ring, index, seq}` messages. Between a worker and a helper, which share only files (cp §6.4), each slot carries a sequence word written after the payload; the reader copies the payload with `pread`, then re-reads the sequence and discards the copy if it changed. Either way a wrong `seq`, an out-of-range index or a length over the slot size is a protocol error.
- **Copies are validated, not files:** an output read from a less-trusted writer is copied into private memory or a staging file first and validated as the copy (pl §3.5 rule 1); a short read is a typed error, never a `SIGBUS`.
- **Size caps:** per kind (e.g. `media.frames` ≤ 256 MiB per ring), checked before creation and at every read.
- **One crate, one rule set:** these are the rules of pl §3.5 (rings fixed-size and host-created, outputs validated after the writer exits or as copies, directories swept at start); `fin-shm` implements them once for media jobs, helpers and the plugin host.

### 3.5 Requirements on sub-project 7's `worker` sandbox profile

Sub-project 7 (`fin-sandbox`, ar §7C) owns the sandbox and applies a `worker` profile to media workers and their helpers (6G turns it on; owner question 6). The profile must allow, and only allow:
- **GPU and media engines:**
  - macOS: Metal and the IOKit user clients it opens (`IOSurfaceRootUserClient`, the GPU's accelerator client), IOSurface, and the CoreMedia/VideoToolbox services hardware decode and encode use;
  - Linux: `/dev/dri/renderD*` (read/write), `/dev/dri/card*` read-only for enumeration where Mesa needs it, the Vulkan ICD and driver files, `/sys/class/drm` and `/sys/dev/char` reads; `/dev/nvidia*` in 6G;
  - Windows: D3D11/D3D12 and Media Foundation inside a normal AppContainer (not LPAC), including the hardware MFTs; to be proven in 6B's validation, since S8 tested an AppContainer without GPU work.
- **Reads:** library roots and project `sources/` (only the files the manifest names, where the OS allows per-file grants; else the roots), the font cache, the media cache (read), the job's `in/`.
- **Writes:** the job's `out/` only; a private temp directory.
- **Denied:** network (all), process creation (`process-exec`/`process-fork` on macOS, a seccomp filter on `execve`/`clone` without `CLONE_THREAD` on Linux, the job object's active-process limit of 1 on Windows), the rest of server data, credentials, the agent listener.

### 3.6 Ownership splits (coordinator decisions)

| Topic | Owner | Sub-project 6's part |
|---|---|---|
| Jobs, scheduler, worker binary, envelope, kill tree, warm workers, helpers, mapped-file lifecycle, retries, poison, `jobs.*`, `fin wait` | 5C | the `media` family and kinds; uses and asks in §3.2 |
| Sandbox | 7 (`fin-sandbox`, `worker` profile) | requirements §3.5 |
| Shared-memory channel | 6 (`fin-shm`) | used by sub-project 8 plugins too |
| Built-in audio effects (EQ, compressor, gate, de-esser) | 6 (6C) | `AudioPlan` gains `effects` (§8.4) |
| 16 kHz mono PCM feed, `media.frames` | 6 (cache recipes) | consumed by sub-project 8's analyzers |
| Codec plugins | 8 (packaging), 5C (starts them as sibling helpers) | the process codec protocol and conformance suite (§6.5, §13.5) |
| Mattes | asset kind `matte` (matches pl §10.3) | encoding and rendering (§11.3) |
| `job.done`, `worker.exit` | 5C | media fields in `job.done` as its `kv` 2 (§15) |
| `media.capabilities`, `render.stats` | 6 | §15 |
| The `Source` enum | the diagnostics spec (arch §21.1) | none |
| Capability URLs for media bytes | 6 (cp §1.2 moved them here) | §7.3 |

## 4. Data flow

### 4.1 From an edit to a frame

```
 tx.commit ──► fin-node (op log) ──► snapshot @ rev ──┐
                                                     ├──► jobs.submit(media.*) ──► 5C ──► fin-workers media
 asset table + server's library mapping ──► manifest ┘                                    │
                                                                                          ▼
             eval_frame(snapshot, timeline, t) ──► FramePlan ──► render graph
             for each layer: SourceRef(asset, time) ──► manifest ──► the file's MediaInfo + sample table
                         ──► frame index (§10.2, nearest frame) ──► decoder (proxy or original) ──► texture
             captions ──► fin-captions raster ──► texture
             graphics ──► thorvg / resvg raster ──► texture
             graph ──► composite (rgba16float) ──► output (8-bit) ──► PNG | encoder ──► muxer
```

In the client, the same graph runs in `fin-wasm` on committed ops replayed locally (arch §6.6), with WebCodecs decoding proxies.

### 4.2 The render manifest

For every asset a snapshot references, the manifest holds:
- `asset`, `kind`, `fingerprint` (as in the model);
- `original`: the absolute path on this machine (resolved by `finisherd` through the library roots, arch §20, cp §7), or `missing`;
- `proxies`: the cache keys of available proxies per tier (from 6B);
- for fonts, templates and mattes, the paths of their bytes.

The manifest says what an asset *is*; the plan says what to do with it. It is built by `fin-node` from the snapshot's asset table and the cache index, never by the worker. Media facts (`MediaInfo`, §6.3) are read by the worker from the file itself (a container parse costs milliseconds) and kept in memory by warm workers; proxy cache entries carry their source's `MediaInfo` in their `meta.json`.

### 4.3 Plan contract gaps closed in sub-project 6

Each is closed in `fin-core` (the plan grows), never by the engine reading the model:

| Gap | Today | Change | Sub-batch |
|---|---|---|---|
| Caption style | `CaptionPlan.style` is the style id only | `CaptionPlan.paint`: the resolved colour, stroke, background box, highlight colour, and the layout's origin in frame pixels (from `position` and the safe zone) | 6D |
| Caption animation | the model has no animation fields; arch §1 promises animated captions | `CaptionStyle.animation` (a closed set of presets: `none`, `pop`, `karaoke`, `fade`, `slideUp`); eval resolves each word's transform, opacity and colour into `WordPlan` | 6D |
| Matte | `MaskPlan.matte` is a free string with no time | `matte` names an asset of kind `matte` (pl §10.3); rule documented in `fin-core`: a matte is read at the masked layer's source time (mattes are produced one frame per source frame) | 6D |
| Base box | how a source maps into the frame at scale 1 is undefined | rule in `fin-core` docs (contain) and a `fit` parameter (owner question 4) | 6A (rule), 6D (param) |
| Transition looks | `crossfade` and `dissolve` have no defined difference | `crossfade`: both layers mixed in encoded (display) space; `dissolve`: mixed in linear light. Both use `progress` through `curve` | 6A |
| Audio effects | `AudioItem` has gain, pan and shapes only | `AudioItem.effects`: the item's enabled audio effects with parameters resolved over the item's span (piecewise like gain), in order | 6C |

### 4.4 Schema changes and their versioning

Every model change follows core §3.2 (`formatVersion`, `minReaderVersion`, frozen upcasters). Sub-project 6 makes these:

| Change | Batch | Versioning |
|---|---|---|
| `VideoInfo.display`, `rotation`, `pixelAspect` (optional) | 6B | format bump; no upcaster can know an old asset's rotation, so absent means "unknown" and the engine uses the file's `MediaInfo`; `asset.reprobe` (a new op) fills them |
| `fit` item parameter | 6D | format bump (older readers refuse `param.set fit`); absent = contain, no upcaster |
| Asset kind `matte`; `Mask.matte` an asset id | 6D (or 8H) | **one `SCHEMA_VERSION` 2 → 3 bump shared with sub-project 8's `Project.plugins`** (pl §4), made by whichever lands first, with frozen upgrade steps and goldens; old free-string mattes stay unresolved (`MATTE_MISSING` until relinked) |
| `mask.set`, `mask.remove` ops (4C Decision 48) | 6D | format bump (new op kinds) |
| `nested` insert edits (4C Decision 48) | 6D | format bump (new op kinds) |
| `CaptionStyle.animation` | 6D | format bump; absent = `none`, no upcaster |
| Audio effect kinds in the param registry (`eq`, `compressor`, `gate`, `deEsser`) | 6C | format bump (new registry kinds) |

- **Plan additions** (`CaptionPlan.paint`, `WordPlan` animation fields, `AudioItem.effects`) change the 4C eval corpus hashes (`crates/fin-core/tests/eval_corpus/expected.json`) and the reel goldens (`scripts/e2e/reel-golden/frames.json`, `audio.json`). The task that adds a field re-blesses them in the same commit and says why.
- **`Probe.media` never reaches an `asset.add` record.** `asset.probe` answers it for callers to read; the op's payload keeps only the model's fields, and a test checks that a committed `asset.add` has no `media` key.

## 5. GPU compositor (`fin-compositor`)

### 5.1 Render graph

For each frame plan, bottom layer first:
1. **Source:** a decoded frame, a caption raster, a graphic raster, an image, or a group (a nested item's children rendered into an offscreen target at the nested timeline's resolution).
2. **Ingest:** YUV planes (NV12, P010, I420/I444 for software decoders) are converted to RGB in a shader: range expansion, the source matrix (BT.601, BT.709, BT.2020 NCL), chroma siting, then the colour policy of §10.1.
3. **Effects in plan order:** `colorAdjust`, `lut`, `blur` (core §5.2), each a WGSL module with a uniform block generated from its `ParamSpec`s.
4. **Crop** (fractions of the item's base box).
5. **Mask:** alpha from the matte (§11.3).
6. **Transform and composite** onto the frame target with opacity and the blend mode:
   - `normal`, `screen` and `add` use **fixed-function blending** on premultiplied colour (`normal`: src + dst·(1 − αs); `screen`: src + dst·(1 − src); `add`: src + dst), and the layer pass draws straight onto the accumulation target, **fused** with the blend (one pass per layer);
   - `multiply`, `overlay`, `darken`, `lighten` are computed in the shader, reading the destination from a ping-pong target (two passes per layer), because they are not expressible as blend factors on premultiplied colour;
   - a layer with effects or a mask renders to a scratch target first (an extra pass), whatever its blend mode.
7. **Adjustment layers** apply their effects to the composite below them, mixed with the unadjusted composite by the layer's opacity (their transform and crop are defaults, as the plan says).
8. **Transitions:** the outgoing layer (`role: out`) is drawn, then the incoming one with weight `curve(progress)`. `crossfade` mixes in encoded space, `dissolve` in linear light (§4.3).
9. **Output:** the frame target is converted to 8-bit Rec.709 (BT.1886) for PNG, the encoder or the preview canvas.

- **Precision:** sources are 8-bit (or 10-bit where the codec allows, arch §12.8); intermediate targets are `rgba16float` with premultiplied alpha; the output is 8-bit. *Amendment to arch §12.8 "8-bit internally":* 8-bit intermediates band under stacked opacity and gradients, `rgba16float` render targets with blending are core WebGPU, and the 10-bit/HDR path needs float anyway.
- **Premultiplied alpha and division:** wherever a stage needs straight colour (effects, LUTs, blend formulas), it un-premultiplies as `a > 0 ? c / a : 0` with an explicit branch, in both compositors, so no NaN is ever produced (NaN payload bits differ between wasm32 and native, and a NaN would propagate differently on GPUs).
- **Base box:** a source's base box is its display size (after rotation and pixel aspect) fitted inside the timeline frame ("contain"), centred. `transform` then applies: `anchor` is item-normalised in the base box, `position` is pixels from the frame centre, `scale` multiplies the base box, `rotation` is degrees clockwise. **The anchor convention:** the anchor point is placed at the frame centre plus `position`, and the item scales and rotates about it, so the defaults (anchor ½, ½; position 0) leave the base box centred, and changing the anchor alone moves the item (as Premiere and After Effects do). The matrix is built in Rust (`f64`, `libm` for `sin`/`cos`, deterministic) and passed to shaders as `f32`. Owner question 4 asks whether "contain" is the right default.
- **Sampling and downscaling** (defined identically in both compositors, §13.4):
  - magnification and scale ≥ ½: bilinear sampling of the source at the output pixel centre;
  - minification: each source gets a **mip chain by exact 2×2 box averaging** (our own pass; WebGPU generates no mipmaps), and the layer samples with an **explicit level** `L = log2(1 / s)` where `s` is the layer's smaller axis scale in output pixels per source pixel (constant per layer, since transforms are affine; computed in Rust with `libm`), bilinear in levels `⌊L⌋` and `⌈L⌉` blended by the fraction. No derivative-based LOD, which differs between GPUs.
- **Edges without clamp-to-border:** WebGPU has no border colour, so samplers clamp to edge and the shader computes **coverage** per output pixel: for each of the quad's four edges (after crop), `clamp(d + 0.5, 0, 1)` where `d` is the signed distance in output pixels from the pixel centre to the edge (positive inside); coverage is their product and multiplies the layer's alpha. The CPU reference computes the same coverage, so rotated and fractional edges match.
- **Pipelines and textures:** one pipeline per (stage, blend mode, target format), created on first use; a texture pool keyed by (size, format, usage) recycles intermediates; decoded frames live in a per-asset ring.
- **Plugin effects later:** a WGSL effect (sub-project 8, pl §10.4) is a fragment function `fn effect(uv: vec2f, src: vec4f) -> vec4f` plus a uniform block generated from the plugin's `ParamSpec`s. The built-ins use the same ABI.

### 5.2 Native

- **Backends:** Metal on macOS; D3D12 on Windows (Vulkan as an alternative); Vulkan on Linux. The adapter is chosen by the capability report: the discrete GPU for batch work unless the user picks another (Chromium ignores `powerPreference`, arch §27.2, but native wgpu does not).
- **Shader compiler on D3D12:** wgpu's FXC path (`d3dcompiler_47.dll`, part of Windows) by default; DXC only if a measured need appears, and then its DLLs go through the run-time library gate (§16.3).
- **Without a GPU** (a NAS without one, a VM): lavapipe (Mesa, Vulkan) on Linux and WARP (D3D12) on Windows. The server Docker image includes Mesa's lavapipe (covered by the image's SBOM gate, §16.3). They are slow but correct, and the capability report says so.
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

The copy path stays as the fallback and is chosen automatically when an import fails; the capability report records which path a machine uses. Encoding takes the same route backwards.

## 6. Codecs and containers

### 6.1 Strategy

- **Containers are ours, in Rust** (`fin-container`), on every OS: one reading of edit lists, sample tables, rotation and colour boxes everywhere. Media Foundation ignores MP4 edit lists (arch §27.3), and AVFoundation hides sample tables, so relying on OS demuxers would give three behaviours.
  - **The box layer is `mp4-atom`** (0.17.1, 2026-11-04, MIT OR Apache-2.0; it encodes and decodes the boxes we need, `stts`, `ctts`, `stsc`, `stsz`, `stco`, `co64`, `stss`, `elst` and the fragment boxes `moof`, `traf`, `tfhd`, `tfdt`, `trun`, `mvex`, `trex`, but validates and interprets nothing): it parses and serialises the ISOBMFF boxes. **Sample tables, edit lists, fragments-to-samples, limits and the codec-configuration readers are ours.** Why: box encoding is large, boring and well covered by a maintained crate that both reads and writes, which roughly halves the container work; the parts that decide which frame is shown and what a hostile file can make us allocate stay in code we test and fuzz. `fin-container` checks every box's size against the bytes available and caps declared counts **before** handing a box to `mp4-atom`, and the fuzz target covers the combination. If fuzzing finds an unbounded allocation inside `mp4-atom` that the pre-checks cannot prevent, the fallback is our own box layer for the affected boxes.
- **Codecs are the OS's or hardware's for patent-encumbered formats** (H.264, HEVC, AAC) and **royalty-free software** for AV1, VP9 and Opus (D13, arch §18). Finisher ships no H.264 or HEVC implementation of its own.
- **OS codecs are driven at the codec level:**
  - macOS: `VTDecompressionSession` / `VTCompressionSession` (stateful: the OS parses the bitstream);
  - Windows: Media Foundation transforms (`IMFTransform`; the hardware MFT bound explicitly by adapter, arch §27.3), stateful; and **D3D11VA directly** for HEVC without the HEVC Video Extensions (arch §27.3), which is **stateless**;
  - Linux: **VA-API, which is stateless**: the application parses the bitstream, fills picture and slice parameters and manages reference frames.
- **Stateless decoding uses `cros-codecs`** (ChromeOS; 0.0.6 of 2025-06-18, BSD-3-Clause in both crates.io metadata and the repo LICENSE; the repo is active) for bitstream parsing, DPB and reference management and the decoder state machines. Its VA-API backend decodes H.264, HEVC, VP8, VP9 and AV1 and encodes H.264, VP9 and AV1 (no HEVC encode); it is Linux-only by design ("support for systems other than Linux" is a stated non-goal) and reaches libva through `cros-libva` (0.0.13, BSD-3-Clause), whose `build.rs` **links** `libva` and `libva-drm` through `pkg-config`.
  - **libva is loaded at run time, never linked**, so the binary starts on machines without libva and the capability report says what is missing. That needs either a `cros-libva` fork whose FFI table is resolved with `libloading`, or Finisher's own VA-API backend over `cros-codecs`' parsers and state machines; the 6B-2 spike decides which.
  - **D3D11VA** gets Finisher's own backend; it reuses `cros-codecs`' parsers and state machines if the crate builds on Windows without its `vaapi` feature *(verify in the spike)*, else Finisher writes the DXVA parameter filling on its own parsers for H.264 and HEVC only (AV1 and VP9 go through MF there).
  - **HEVC encoding on VA-API** is not in `cros-codecs`; it is out of v1 unless owner question 9 wants Linux HEVC export (it recommends none).
- *As built (6A-1), `fin-container`'s limits* apply per file (`maxBoxes` 1 M, `maxBoxBytes` 64 MiB, `maxDepth` 16) and per track: `maxSamples` (10 M) bounds a track's samples, which are also at most the file's length in bytes (a fragment run with no per-sample fields costs no bytes a sample), and bounds the **presented frames** after edit lists (`maxPresentedFrames`; up to 1,024 edits may repeat the media), counted before any frame is stored, with the samples an edit looks at bounded by twice that (`maxEditWork`). A video sample in a fragment must have at least one byte. Times that overflow 64 bits are `MEDIA_CORRUPT`.
- **Process-based backends** (the FFmpeg plugin, others) implement the same `Decoder`/`Encoder` traits through a helper process that 5C starts as the job's sibling, with packets and frames in `fin-shm` rings (§3.4, §6.5).

### 6.2 Formats per OS

**Containers**

| Container | Read | Write | Notes |
|---|---|---|---|
| MP4 / MOV (ISOBMFF), incl. fragmented | ✓ | ✓ (MP4) | edit lists, `colr`, `pasp`, `tkhd` matrix, `mdcv`/`clli`, `dvcC`/`dvvC`, `sgpd`; fast start |
| WebM / Matroska | ✓ (6B) | — (cut from v1) | |
| WAV, AIFF, FLAC, Ogg, MP3, CAF | ✓ audio | — | via symphonia's readers (§6.4) |
| PNG, JPEG, WebP | ✓ images | PNG (`fin look`, thumbnails) | |
| MXF, AVI, MPEG-TS, others | — | — | `UNSUPPORTED_CONTAINER`, fix: the codec plugin |

**Video decode**

| Codec | macOS 14+ (Apple Silicon) | Windows 10 22H2 / 11 | Linux | Software |
|---|---|---|---|---|
| H.264 (8-bit, High) | VideoToolbox (HW) | MF + hardware MFT | VA-API (Intel, AMD) through `cros-codecs`; NVDEC (6G) | **none** (patents) |
| HEVC Main / Main10 | VideoToolbox (HW) | MF with the HEVC Video Extensions, else D3D11VA through `cros-codecs` | VA-API through `cros-codecs`; NVDEC (6G) | **none** |
| AV1 | VideoToolbox on M3 and later | MF (AV1 extension) or D3D11VA | VA-API | `rav1d` (all OSes) |
| VP9 | VideoToolbox after `VTRegisterSupplementalVideoDecoderIfAvailable(kCMVideoCodecType_VP9)` (verified on an M4) | MF / D3D11VA | VA-API | libvpx |
| ProRes 422/4444 | VideoToolbox | — | — | none (plugin) |

**Video encode**

| Codec | macOS | Windows | Linux | Software |
|---|---|---|---|---|
| H.264 | VideoToolbox | hardware MFT by adapter (AMD, NVIDIA, Intel) | VA-API (Intel low-power VDEnc, AMD) | **none** |
| HEVC | VideoToolbox | hardware MFT | VA-API | **none** |
| AV1 | — | — (the hardware MFT path is cut from v1) | — | **SVT-AV1** |
| VP9 | — | — | — | — (libvpx encoding cut from v1) |

Owner question 9 asks which of these H.264/HEVC encoders Finisher may use for export, given that GPU vendors' encoders and Linux drivers generally come without a patent licence for the application.

**Audio**

| Codec | macOS | Windows | Linux |
|---|---|---|---|
| AAC-LC / HE-AAC decode | AudioToolbox | MF AAC decoder (absent on N editions without the Media Feature Pack) | **open** (owner question 1; symphonia: AAC-LC only) |
| AAC-LC encode | AudioToolbox | MF AAC encoder (absent on N editions) | **open** (owner question 1) |
| Opus | libopus (decode and encode, all OSes) | | |
| MP3, FLAC, ALAC, Vorbis, PCM | symphonia (decode, all OSes) | | |

- **Gaps get guidance, not failure** (arch §12.1): the capability report lists each missing decoder with its fix (the HEVC Video Extensions, the Media Feature Pack on Windows N, a GPU driver, the codec plugin). An asset that cannot be decoded is still registered; its items render as "unavailable" in preview and `fin look`, and export refuses with `CODEC_UNAVAILABLE`.
- **Proxies remove most gaps for editing:** a machine that can decode an original once writes AV1 or H.264 proxies that every client can play.

### 6.3 Probing

- **Shallow probe** (in `fin-node`, at `asset.add`, as today; and in 5D's `library.probe` worker, cp §7.3): `fin-container` parses the container (safe Rust, panics caught). It reports the model's `VideoInfo`/`AudioInfo` and, in the `asset.probe` answer only, `MediaInfo`:
  - coded and display size, the `tkhd` rotation (0/90/180/270; other matrices refused as `UNSUPPORTED_TRANSFORM`), pixel aspect;
  - codec, profile, level, bit depth, chroma format;
  - colour tags (`colr` nclx/ICC, else the codec configuration's VUI), range, chroma siting;
  - HDR: transfer (PQ, HLG), `mdcv`/`clli`, Dolby Vision configuration (profile, compatibility id);
  - the sample table with edit lists applied (§10.2), the nominal rate, and whether the source is VFR (frame durations differ by more than 0.1 %);
  - audio: codec, rate, channels and layout, priming (encoder delay) and padding.
- **No media index files.** Workers parse the file when they open it; proxy cache entries carry `MediaInfo` in `meta.json`; 6B's cache index (§7.2) records what the server needs to answer quickly.
- **Deep probe** (`media.probeDeep`): decodes the first frame to confirm decodability and reads in-band HDR SEI when the container has no HDR boxes.
- **Known 4C gaps this fixes:** `fin-probe` today reports `tkhd` width and height without the rotation matrix (a portrait iPhone clip reads as 1920×1080), averages the frame rate over the track (VFR is not detected) and ignores edit lists. 6A puts the truth in `MediaInfo`; 6B adds display size and rotation to the model (§4.4).

### 6.4 Software codecs and container code

| Library | Use | Integration |
|---|---|---|
| `rav1d` (Rust port of dav1d) | AV1 decode | crate; **maintenance risk** (§16.2): dav1d through a minimal FFI is the fallback if `rav1d` stalls, is > 15 % slower, or fails conformance (6A measures it) |
| SVT-AV1 | AV1 encode: proxies without a hardware encoder, every AV1 export, test fixtures | own minimal FFI, built from source at a pinned tag (arch §27.3), never a prebuilt download |
| libvpx | VP9 decode where the OS has none | own minimal FFI, built from source at a pinned tag (decoder only) |
| libopus | Opus decode/encode | own minimal FFI, built from source at a pinned tag |
| symphonia | MP3, FLAC, ALAC, Vorbis, PCM, WAV/AIFF/Ogg/CAF readers | crate, unmodified (MPL-2.0, §16.1) |
| `mp4-atom` | ISOBMFF box encode/decode | crate, under `fin-container`'s limits (§6.1) |
| `cros-codecs` | stateless decoding state machines and parsers (6B) | crate, with Finisher's VA-API and D3D11VA backends |
| `fin-container` | sample tables, edit lists, fragments, limits, MP4 mux, WebM demux | ours; fuzzed (§13.5) |

*As built (6A-1):* `rav1d` 1.1.0's crate package leaves out its `*.h` files, so its aarch64 assembly cannot build: the assembly is enabled on x86_64 only, and aarch64 runs rav1d's Rust paths, 2.7–4.2× slower than dav1d 1.5.3 on an Apple M4 (627 against ~1,710 fps at 540p, 257 against ~1,090 at 1080p, frame delay 1). That is past the 15 % threshold: the dav1d FFI follow-up is open in `TASKS.md`. After the 6A-1 review, rav1d decodes with **one thread**: with worker threads its debug-only `DisjointMut` overlap checker panicked once on test input, which in release builds (unchecked) would be a data race on untrusted media. Single-threaded on aarch64 it decodes 215 fps at 540p and 22 fps at 1080p, which makes the dav1d decision urgent for 6A-2's latency budgets (§14). SVT-AV1 v4.3.0 is built once by `fin-svt-sys` (a featureless crate) through a small C shim of Finisher's that owns its configuration struct; `fin-codecs` and `fin-svt-sys` are not default workspace members until 6A-2 needs them.

### 6.5 The process codec backend

The protocol a codec plugin implements (packaged by sub-project 8, pl §10.1): `codec.open {kind, stream, config}` → `{session, caps}`; packets in and frames out through `fin-shm` rings; `codec.flush`, `codec.close`; `demux.open`/`demux.read` on a read-only grant of one source file. The media worker is the client; the plugin is the job's sibling helper (§3.2 item 4). Conformance: §13.5.

### 6.6 Estimate for 6B

Revision 1 treated VA-API and D3D11VA like VideoToolbox. They are stateless, so 6B carries a decoder backend per API on top of `cros-codecs`, D3D11VA-specific DXVA parameter structures (`DXVA_PicParams_H264`, `_HEVC`, …) and reference-surface management, besides the stateful VideoToolbox and MF paths. **6B is re-estimated at about 1.6× revision 1's size** (more if the D3D11VA backend cannot reuse `cros-codecs`), and is split into **6B-1** (VideoToolbox, MF stateful decode and encode, the capability report, HDR tone mapping, proxies, the cache and HTTP serving) and **6B-2** (VA-API and D3D11VA through `cros-codecs`, VA-API encode). 6B-2's first task is a spike that proves a `cros-codecs` backend over run-time-loaded libva, and the D3D11VA backend's feasibility, before the rest is planned.

## 7. Proxies and caching

### 7.1 Proxies

- **Tiers** (arch §12.2):
  - `540`: scrub and remote clients. All-intra where the hardware encoder supports intra-only at ≥ 8× real time; otherwise a keyframe every 0.25 s.
  - `1080`: default preview. Closed GOP with a keyframe at least every 0.5 s.
  - Originals for export.
- **Codec:** OS/hardware H.264 → SVT-AV1 (preset 10, CRF, closed GOP every 15 frames; arch §27.3). A remote client whose WebCodecs cannot decode the chosen codec gets the other one, generated on demand. If owner question 2's answer removes software H.264 from the desktop app, preview proxies default to AV1 (§12).
- **Audio:** Opus 128 kb/s stereo at 48 kHz inside the proxy (for remote clients and preview), plus the PCM cache (§7.2) for native mixing.
- **Frames:** a proxy holds **exactly one frame per original frame**, in order, stamped at a constant rate (the original's nominal rate), and its cache entry keeps the original's sample table. Preview finds the original frame for a source time with the same nearest-frame rule as export (§10.2) and fetches that frame from the proxy by index, so preview and export show the same frame, also for VFR sources and for times that fall between frames. This is how "conformed to CFR with a PTS map" (arch §12.2) is realised.
- **HDR sources** are tone-mapped to SDR when the proxy is made (§10.1), with the same algorithm export uses on that OS, so preview never relies on Chromium's own tone mapping.
- **Rotation** is not baked: proxies keep the coded orientation, and the compositor rotates, so proxies and originals are interchangeable.
- **When:** sub-project 5D's indexer registers `proxy540` as a library stage (cp §7.3) for every asset; `1080` is made when an asset is first used in a project; both can be requested explicitly (`fin media proxy <asset>`).

### 7.2 The cache

- **Location:** `<server-data>/cache/media/`, local disk only (arch §6.7).
- **Recipes** (each a cache kind with a versioned recipe):

  | Recipe | Content | Consumers |
  |---|---|---|
  | `proxy-540`, `proxy-1080` | MP4 + the original's sample table + `MediaInfo` | preview, `fin look` |
  | `pcm48` | 48 kHz `f32` stereo in 1 MiB chunks | the mixer, waveforms |
  | `pcm16m` | **16 kHz `f32` mono analysis feed**, resampled by `fin-audio`'s scalar resampler | sub-project 8 analyzers (`media.pcm` host call, pl §5) |
  | `wave` | min/max peaks at several zoom levels | timeline (sub-project 9) |
  | `thumbs`, `strip` | thumbnails, filmstrips | sub-project 9 |
  | `frames` | analysis frame feeds at `maxSide` and stride (`media.frames`) | sub-project 8 (scenes, embeddings, reframe, mattes) |
  | `raster` | Lottie, SVG and caption rasters worth keeping | the compositor |

- **Keys:** BLAKE3 of (source fingerprint, recipe name and version, recipe parameters, `RENDER_VERSION`, decoder class). Analysis recipes (pl §8 "Recipes and caching") name the PCM or frame feed's recipe version and decoder class in theirs, so a change to the feed invalidates the analyses built on it.
- **Entries** are directories: the payload and `meta.json` (recipe, versions, sizes, creation, the source's `MediaInfo`). Workers write into their job's `out/`; 5C's move (with `fin-shm`'s checks) puts the directory in place.
- **Index:** a small SQLite database `cache/media/index.db` on local disk, written by `fin-node` only: entries, sizes, last use, pins, the project each entry serves (for reach checks, §7.3). Built in 6B.
- **Eviction:** least recently used, above a cap (default the smaller of 50 GiB and 15 % of the volume, `media.cacheMaxGiB`). Entries a running job reads, or an open project's timelines use, are pinned. `fin cache status|gc`.
- **Offline libraries:** proxies and PCM keep editing and preview working; export waits and lists the missing originals (`ORIGINALS_MISSING`, arch §6.7).

### 7.3 Serving the client

- `finisherd` serves cache entries over HTTP with range requests (`GET /media/<key>/<file>`). A request carries the connection's credential (cp §4.3); the server maps the key to the projects it serves (index, §7.2) and requires the principal's reach over one of them plus the `project.read` scope. Clients address media by key, never by path.
- **Capability URLs** (moved here from sub-project 5, cp §1.2) for elements that cannot send headers (`<video>`, the share page later): `GET /media/cap/<token>/<file>`, where the token is an opaque random string held in memory, bound to `{key, principal, expiresAt ≤ 10 min}`, and revoked with the principal.
- The preview demuxes proxies itself (`fin-wasm` runs `fin-container`) and feeds WebCodecs; it fetches GOP-sized byte ranges ahead of the playhead.

## 8. Audio engine (`fin-audio`)

### 8.1 Format and decoding

- 48 kHz, 32-bit float, stereo internally (arch §12.7). Mono sources are upmixed by duplication with −3 dB; multichannel sources are downmixed with ITU-R BS.775 coefficients.
- **Resampling** at decode with **`fin-audio`'s own scalar polyphase windowed-sinc resampler** (fixed taps and window, `f32`, fixed summation order), so the 48 kHz and 16 kHz caches are bit-identical on every OS and in WASM. `rubato` is not used: its FFT path (`rustfft`) uses SIMD and would fall outside the determinism gate (§13.4).
- Encoder priming and padding from the container (`elst`, `sgpd` roll, Opus pre-skip) are removed at decode, so sample 0 of the cache is the first audible sample at source time 0.

### 8.2 Mixing the plan

- For each `AudioItem`: read PCM from `sourceFrom` at `speed`; run the item's effects (§8.4); apply gain segments (dB → amplitude, `10^(dB/20)`), shapes and pan; sum into the master bus in plan order.
- **Curves:** `linear` is `x`; `equalPower` is `sqrt(x)` (a crossfade's two sides sum to constant power). This fixes "as the mixer defines it" in `eval::audio`.
- **Pan:** a mono source uses constant-power panning (`L = sqrt((1 − p)/2)`, `R = sqrt((1 + p)/2)`, so the centre is −3 dB on each side); a stereo source uses balance (the side panned away from is attenuated linearly, the other side stays at unity).
- **Speed ≠ 1** keeps pitch, with a WSOLA time-stretch written in Rust in `fin-audio` (deterministic, the same native and in WASM). C++ stretchers do not build for `wasm32-unknown-unknown`, and the permissive options that do not exist at speech quality (Rubber Band and SoundTouch are GPL/LGPL and excluded).
- **Master bus:** the sum, then (on export only) the loudness gain and the true-peak limiter (§8.6).
- **Determinism:** scalar `f32` in a fixed order, an exactly pinned `libm` for `pow`, `sin`, `cos` and `sqrt`, no SIMD intrinsics in the mixer, effects or resampler. Native and wasm32 output is bit-identical, so null tests are exact (§13.4).

### 8.3 Preview

- The same mixer runs in an AudioWorklet as a WASM instance. PCM comes from the Opus proxy audio, decoded by WebCodecs `AudioDecoder` into a `SharedArrayBuffer` ring the worklet reads. That needs cross-origin isolation (COOP/COEP headers from `finisherd` and in Electron); the preview harness checks for it.
- **The audio clock drives video** (arch §12.7): the scheduler of arch §27.2 (cadence-aware, audio clock sampled at rAF plus the measured presentation offset, nearest frame, per-platform display constant, interpolated audio clock) is implemented in `@finisher/preview`.

### 8.4 Built-in audio effects (6C)

- **Kinds:** `eq` (up to 8 bands: low/high shelf, peaking, low/high pass; RBJ biquads), `compressor` (feed-forward, RMS or peak detector, threshold, ratio, knee, attack, release, make-up), `gate` (threshold, range, attack, hold, release), `deEsser` (a split-band compressor on a band around a chosen frequency). They are `ParamSpec`s in `fin-core::params` (`scope: audio`), edited with the existing `effect.*` and `param.set`/`keyframe.*` ops on audio-bearing items, which needs the registry change of §4.4.
- **Plan:** `AudioItem.effects: [{id, kind, params: {path: segments}}]`, parameters as piecewise segments over the item's span like gain, so automation stays in the plan.
- **Processing:** per item, before gain and pan, in plan order; coefficients recomputed per 64-sample block from the interpolated parameters; `f32` scalar, `libm`, fixed order (bit-identical native and WASM).
- Offline processors (de-noise, voice isolation) remain plugins (sub-project 8); they produce new audio assets rather than running in the mixer.

### 8.5 Ducking (amendment to arch §12.7)

Ducking is compiled into ordinary gain keyframes at edit time by a `compose::duck` builder in `fin-core` (from voice-activity artifacts and a ducking preset), never computed by the mixer. Agents and users see and edit the result; the plan stays the whole truth; the mixer stays free of analysis data. The builder arrives with voice activity in sub-project 8.

### 8.6 Loudness

- **Measurement:** ITU-R BS.1770-4 integrated loudness, loudness range and true peak (4× oversampling), via the `ebur128` crate (MIT, a port of libebur128), measured on every export and on `fin listen`. Its results are reported, not hashed, so its internal float use is outside the bit-exactness gate.
- **Normalisation:** a format template names a target (e.g. −14 LUFS integrated, −1 dBTP). Export runs two passes over the audio: mix and measure, then apply one static gain and a look-ahead true-peak limiter on the master bus. The export report records the measured values before and after.
- **Unreachable targets** (silence, or a gain the limiter cannot hold within 1 LU) produce the warning `LOUDNESS_TARGET_UNREACHABLE`, not a failure.

### 8.7 `fin listen`

A text summary of a range for agents (arch §9.6): per track and for the mix, integrated loudness, true peak, clipping, silences longer than 0.5 s, and the loudest moments. Speech and music labels join when sub-project 8's analysis exists.

## 9. Export pipeline

### 9.1 Flow

1. **Preflight** (in `finisherd` before the job is submitted): the template is supported here (capability report), every original is reachable and unchanged (fast fingerprint), fonts, templates and mattes are present, nothing in the range is unmeasured or unavailable. Failures are typed (`ORIGINALS_MISSING`, `ASSET_CHANGED`, `CODEC_UNAVAILABLE`, `CAPTIONS_UNMEASURED`, `LAYER_UNAVAILABLE`), each listing the items or assets.
2. **Video:** for output frame `n`, `t = from + n / rate` exactly (rational). Eval, decode originals by §10.2, composite at the output resolution, convert to NV12 on the GPU, encode. Bounded queues between decode, composite and encode keep memory flat (§14).
3. **Audio:** eval over 1 s chunks, mix, measure (pass 1); then mix again with the normalisation gain and limiter and encode (pass 2). The audio of a range starts at sample `round(from × 48000)`. For rates whose frame duration is not a whole number of samples (29.97 fps: 1601.6 samples per frame) that start is off the exact time by **at most ½ sample (10.4 µs)**; it never accumulates, because every chunk boundary is computed from the exact rational time, not by adding chunk lengths.
4. **Mux** (`fin-container`): MP4 with `moov` first (space reserved, rewritten at the end), `colr` nclx BT.709 limited range, the encoder delay in an edit list so audio and video start together.
5. **Verify:** reopen the file with `fin-container`, decode the first and last frames, count frames and samples, decode the audio and measure its loudness. The result goes into the export report.
6. **Commit:** temp-then-rename to the destination (default `workspace/shared/exports/<name>.mp4`).

### 9.2 Format templates

- Templates are data (a content pack, arch §14.1): container, video codec, profile, resolution, rate (always the timeline's), rate control and bitrate, GOP, audio codec and bitrate, loudness target and true-peak ceiling, safe-zone preset.
- **First-party templates:** `social-9x16-1080p` (H.264 High, ~12 Mb/s VBR, AAC-LC 192 kb/s, −14 LUFS / −1 dBTP), `master-16x9-1080p`, `master-16x9-2160p`, `av1-web` (AV1 + Opus, the royalty-free reserve). Whether an HEVC template ships at all is owner question 9.
- A template asks for capabilities, never for a specific backend; the export report names the backend that was used.

### 9.3 Export report

`{file, bytes, duration, frames, rate, video: {codec, profile, backend: hw|sw, encoder}, audio: {codec, rate, loudness: {integrated, range, truePeak, before}}, avOffsetSamples (≤ 0.5), warnings, renderVersion, textEngine, planSamples}`. `fin render` prints it; the reviewer's QA gate (sub-project 10) reads it.

### 9.4 Not in v1

Chunked parallel export (GOP-aligned segments encoded on several workers and concatenated) is designed for but not built: the job format already names its range, so a later version can split it. It needs identical encoder parameter sets across sessions, which not every hardware encoder guarantees.

## 10. Colour and frame rate

### 10.1 Colour

- **Working space:** Rec.709 primaries, display-referred, BT.1886 encoded values. sRGB images, SVG, Lottie and caption colours are treated as the same encoding (the usual practice in editors and browsers). Output is tagged BT.709 / BT.709 / BT.709, limited range.
- **Untagged sources:** BT.601 below 720 lines, BT.709 otherwise; images without a profile are sRGB.
- **Wide-gamut SDR** (Display P3 images and video, BT.2020 SDR): linearised, converted to Rec.709 with a 3×3 matrix, soft-clipped, re-encoded.
- **HDR** (HLG, PQ, Dolby Vision profile 8.x from phones), tone-mapped to SDR at decode (arch §12.8):
  - macOS: the OS tone mapping (VideoToolbox / `VTPixelTransferSession`), which also uses Dolby Vision metadata;
  - Windows and Linux: Finisher's shader. HLG is first taken to display light at a 1,000-nit reference; then a BT.2390 EETF on PQ luminance maps it to SDR, followed by BT.2020 → Rec.709 gamut mapping (matrix plus soft clip). Dolby Vision profile 8 uses its HLG/HDR10 base layer and ignores the RPU. **Profile 5 is cut from v1:** it is `CODEC_UNAVAILABLE` on every OS.
  - The source's HDR metadata stays in `MediaInfo` for HDR export later.
  - Because the two algorithms differ, HDR goldens are compared within an OS only (§13.2, tier T4).
- **10-bit sources** are decoded at 10 bits (P010) and converted in the shader. The working space stays SDR.
- **LUTs** (`.cube`) apply in encoded space as 3D textures (`rgba16float`, trilinear), as `.cube` LUTs for Rec.709 expect.
- *As built (6A-1), the CPU reference:* wide-gamut SDR (BT.2020, Display P3) is converted to Rec.709 and **hard-clipped** to 0…1 (the soft clip is 6B's); the BT.601 primaries (525 and 625 lines) are treated as Rec.709 (no primaries conversion, only the BT.601 matrix); HDR transfers are not tone-mapped yet (6B). Identity-matrix (GBR) sources expand every channel as luma (limited range `(v − 16)/219`).

### 10.2 Frame rate, VFR and the sampling rule

- **The sample table:** every video track's frames as `(pts, duration, keyframe, offset, size)`, with edit lists applied, so `pts` is presentation time on the source's own timeline in exact rationals.
- **The sampling rule is the nearest frame** (arch §12.8 "nearest-frame"), one function, `fin-media::frame_at`:
  - Each frame `i` owns the interval `(m(i−1), m(i)]`, where `m(i) = (pts(i) + pts(i+1)) / 2` is the midpoint to the next frame; the first frame owns everything up to `m(0)`, the last everything after `m(last−1)`.
  - The frame shown at source time `s` is the one whose interval contains `s`. For a constant-rate source with frame duration `d` this is **the last frame with `pts < s + d/2`**.
  - **Ties** (`s` exactly at a midpoint) **go to the earlier frame.** Why: at half speed a 24 fps source on a 24 fps timeline then shows every frame exactly twice (0, 0, 1, 1, …) instead of showing the first frame once (0, 1, 1, 2, 2, …), and a later-wins rule would also make a timeline time that lands exactly halfway look ahead of the sound.
  - All comparisons are exact rationals; there is no epsilon.
  - Times outside the source's extent (a transition handle past the asset is policy, core §5.4) render the first or last frame and report `SOURCE_OUT_OF_RANGE` in the result.
- **Rate conversion:** the timeline rate is fixed; every source is sampled by that rule, so 30 fps footage on a 25 fps timeline drops frames and 24 fps footage on a 30 fps timeline repeats them, each at the frame closest in time. Frame blending (arch §12.8, "chosen per item") has no model parameter yet; a `frameBlend` parameter is a 6D candidate if the owner wants it before v1.
- **VFR:** the table is used as is; nothing is resampled. Proxies keep one frame per original frame (§7.1), so preview and export agree.
- **Mattes** follow the same rule on their own table, which matches the source's one frame for one frame (§11.3).

## 11. Captions, Lottie, SVG and masks

### 11.1 Lottie templates (`fin-graphics::lottie`)

- **Native:** thorvg's C API through a small FFI, built from source at the version inside the shipped dotlottie-web (v1.1.x; arch §27.10), with only its Lottie loader (plus the PNG/JPEG loaders templates embed) and the software raster engine. Frames render to premultiplied RGBA at the layer's pixel size and are uploaded as textures. A golden checks native thorvg against dotlottie-web (pixel-identical at v1.1.1 vs dotlottie-web 0.81.0, arch §27.10).
- **Preview:** dotlottie-web (MIT) in a Web Worker in the client.
- **Text slots** are laid out and rasterised by `fin-captions` and injected as image slots, oversampled for animated scale (arch §27.10). thorvg's text engine is never used.
- **Safety:** a template is validated against the slot schema at `asset.add` (core §5.7) and then rendered at sample times by a `media.templateCheck` job, because thorvg segfaults on malformed `chars` (arch §27.10). A template that crashes or hangs the check (5C reports `WORKER_CRASHED`/`WORKER_HUNG`) is refused (`TEMPLATE_INVALID`). Export renders templates in the render worker, so a crash there fails only that job.
- **Fonts:** every font a template names must be registered (arch §27.10); text slots use the project's pinned fonts.

### 11.2 SVG and images (`fin-graphics::svg`, `image`)

- **Sanitising** at `asset.add` (the asset file is never changed; the sanitised tree is a cache entry): `usvg` (≥ 0.45, Apache-2.0 OR MIT) parses with external references, `<script>`, event attributes, `foreignObject` and system fonts disabled; `data:` URLs only for images; size and nesting limits. A file that needs anything refused is `SVG_UNSAFE` with the reason.
- **Text in SVG** uses the project's pinned fonts through `fin-text`, never system fonts; an unpinned family is `FONT_UNPINNED` at add.
- **Raster:** `resvg` (tiny-skia) at the layer's pixel size; re-rasterised when the effective scale changes by more than 10 %; cached.
- **Images:** PNG, JPEG and WebP decoded in the worker (pure Rust decoders), EXIF orientation applied, ICC profiles honoured for sRGB and Display P3 (§10.1), others treated as sRGB with a warning.

### 11.3 Masks and mattes

- **A matte is an asset of kind `matte`** (pl §10.3, owner question 4 of sub-project 8): a luma-encoded greyscale video in `assets/`, write-once and synced, so export on another machine never depends on a cache. `Mask.matte` names that asset (§4.4).
- **Producing one:** a matte producer (a plugin, sub-project 8) returns alpha frames, exactly one per source frame, in order, through a `fin-shm` ring; a `media.matteEncode` job encodes them losslessly-enough (H.264 4:0:0 where the OS encoder offers it, else SVT-AV1 at high quality, 8-bit luma) at the source's rate and sample table, and registers the asset.
- **Rendering:** the engine decodes the matte like any source, at the masked layer's source time by the nearest-frame rule (§10.2), and takes alpha from range-expanded luma.
- **Order:** `choke` (erosion by `choke` pixels at the timeline resolution, a separable min filter), then `feather` (Gaussian blur with radius `feather` pixels), then `invert`, then multiply into the layer's alpha after crop and edge coverage.
- **Mask edits** (`mask.set`, `mask.remove`, keyframes on `mask.*`) are added to `fin-core` in 6D (4C Decision 48), one module per op, with inverses and footprints like every op.
- A matte asset whose file is unavailable renders the layer unmasked in preview with `MATTE_MISSING` and refuses export.

### 11.4 Captions (`fin-captions`)

- **Input:** the `CaptionPlan`: words, active flags, layout, and (from 6D) the resolved paint and per-word animation (§4.3).
- **Glyphs:** `fin-text` gains a glyph-level twin of `measure` (`shape` → glyph ids and positions per word) over the same code path, so the renderer's word boxes are exactly the plan's. The renderer checks this and fails with `PLAN_MISMATCH` if they differ.
- **Raster:** glyph outlines through `swash`'s scaler on the CPU, the path the S11b spike proved byte-identical across x86-64, arm64 and wasm32 when fonts are pinned (arch §27.10). Stroke, background box (rounded rectangle) and highlight colour are rasterised in the same pass, into one premultiplied RGBA bitmap per block and animation state, cached by (layout hash, paint hash, state).
- **Animation:** per-word transforms and opacity from the plan are applied when the block bitmap is built (small motions) or as per-word quads on the GPU (scale animations, where the words are rasterised oversampled).
- **Colour emoji:** bitmap emoji (CBDT/sbix) from the catalog's Noto Color Emoji (OFL) bitmap build, scaled with the box mip chain of §5.1. COLRv1 needs a painter `swash` lacks (arch §27.10); it is a later follow-up, and the catalog pins the bitmap build until then.
- **Missing fonts:** an unmeasured caption (core §5.4 `unmeasured`) is not drawn in preview or `fin look`, which report it; export refuses with `CAPTIONS_UNMEASURED` (fix: `fin fonts fetch`).

## 12. Preview engine (`@finisher/preview`, headless)

- **What it is:** a TS package with no UI: a player core that a client (sub-project 9's editor, the share page later) mounts on a canvas.
- **Pieces:**
  - the `fin-wasm` instance (op replay and eval, compositor, demux);
  - a WebCodecs decoder pool, one decoder per active asset and tier, fed GOPs ahead of the playhead, with frames kept in a ring keyed by original frame index;
  - the scheduler (§8.3) and the AudioWorklet mixer;
  - dotlottie-web for graphics;
  - a capability probe (`VideoDecoder.isConfigSupported` per proxy codec, including `prefer-hardware`) sent to the server in the handshake, so it serves the right proxy codec.
- **Proxy codec in the desktop app** depends on owner question 2: if Electron ships with the open-codecs-only ffmpeg, H.264 proxies are used only where WebCodecs reports hardware H.264 decode *(verify that hardware H.264 decoding still works in WebCodecs with that ffmpeg build, per pinned Electron version)*; otherwise preview proxies default to AV1 (decoded in hardware where available and by Chromium's dav1d elsewhere).
- **Scrub:** seeks decode from the nearest keyframe of the `1080` proxy, or of the `540` proxy while dragging fast; the last good frame stays up until the new one arrives.
- **Server-rendered fallback** (arch §12.3, D31: no WebGPU): an interactive `media.previewFrames` job renders frames on demand (WebP over the binary channel) for scrubbing. **Playback through the server is cut from v1:** without WebGPU the client plays the `540` proxy in a `<video>` element with no compositing (captions and graphics are absent, and the player says so).
- **Test harness:** a page under `packages/preview/harness/` loaded by Playwright in Chromium and in Electron on the three OSes, for parity goldens, the A/V latency measurement of arch §27.2, and the budgets of §14. It also serves as the walking skeleton's "bare web client" (arch §24 row 3), which has not been built yet.

## 13. Determinism and tests

### 13.1 Layers of testing

| Layer | What | Where |
|---|---|---|
| Unit | colour matrices, transfer functions, the sampling rule, gain/pan laws, the resampler, WSOLA, audio effects, effect formulas, coverage and mip levels, each against hand-computed values | each crate |
| Containers | demux/mux round trips, edit lists, fragmented files, rotation and colour boxes, broken files (typed errors, never panics) | `fin-container` |
| Codec conformance | each backend decodes generated fixtures to the expected frame ids and checksums; encoders round-trip through decoders within T5 | `fin-codecs`, per OS |
| Golden frames | plans from a corpus rendered by the CPU reference (hashes committed) and by the GPU (compared to the reference within T1) | `fin-compositor`, native and wasm |
| Audio | null tests, native vs wasm bit-exactness, loudness against EBU Tech 3341/3342-style signals generated in code, A/V offset | `fin-audio`, export |
| Media jobs | each kind's params and results against 5C's harness (`test.*` kinds); `fin-shm` refusals (symlinks, hard links, oversize, truncation while reading); the in-flight-item retry of `media.frames` | `fin-workers`, `finisherd` |
| e2e | the reel scenario (core §5.9) extended: `fin look` samples, `fin render` with its report, `fin listen` | `pnpm e2e` |
| Preview | parity with native, A/V latency, scrub and playback budgets, in Chromium and Electron | Playwright harness |
| Fuzzing | `fin-container` (with `mp4-atom` underneath), the SVG sanitiser, the `.cube` parser, `fin-shm` ring headers | `cargo fuzz`, nightly CI job |

### 13.2 Tolerance tiers

| Tier | Compared | Tolerance |
|---|---|---|
| **T0** | CPU rasters (captions, the CPU reference compositor, SVG where tiny-skia proves deterministic) across macOS, Windows, Linux and wasm32 | byte-identical; BLAKE3 goldens committed |
| **T1** | a GPU render against the CPU reference render of the same plan and source pixels, any adapter (Metal, D3D12, Vulkan, lavapipe, WARP, WebGPU) | per channel \|Δ\| ≤ 2/255 on ≥ 99.9 % of pixels, max \|Δ\| ≤ 8/255, PSNR ≥ 50 dB |
| **T2** | native thorvg against dotlottie-web for the same template frame | PSNR ≥ 60 dB (measured 77.6 dB, arch §27.10) |
| **T3** | the same 8-bit H.264, HEVC, AV1 or VP9 bitstream decoded by different backends | bit-exact YUV planes (decoding is normative); a backend that differs is a bug or is excluded |
| **T4** | HDR tone mapping, within one OS against its own reference | T1 within the OS; across OSes informational only (mean ΔE2000 reported, not gated) |
| **T5** | an encoded export, decoded, against the CPU reference render | PSNR ≥ 38 dB and SSIM ≥ 0.97 at the template bitrate; frame count and frame ids exact |
| **A0** | the mixer, effects and resampler, native against wasm32 | bit-identical samples |
| **A1** | export audio decoded against the mixer's output | integrated loudness within ±0.1 LU, cross-correlation lag 0, A/V start offset ≤ ½ sample by construction and ≤ 1 ms measured by flash/beep patterns |

- **The CPU reference compositor** (`fin-compositor::reference`, a test-only feature) implements every stage of §5.1 (blend formulas, coverage, the mip chain and explicit-level sampling, premultiplied division) in `f32` with the pinned `libm`, scalar and in a fixed order. It is the oracle: GPU output is never compared with stored images, so no golden images are committed and a golden is portable across adapters. Its own output is pinned by BLAKE3 hashes, which catch drift in the oracle itself.
- **Goldens and the sampling rule:** every golden that names a source frame (the frame-id checks in `fin look` tests, the e2e scenario, the proxy tests) is computed through `frame_at`'s nearest-frame rule, and the corpus includes times at exact midpoints, just before and just after them, so the tie rule is pinned.
- **Adapters in CI:** lavapipe on the Linux runners, WARP on Windows, and the runner's Metal device on macOS where the runner exposes one (unverified for GitHub's arm64 runners; if absent, the macOS GPU rows run in host validation, as 4A/4B's cross-host rows do). Hardware GPU rows (Apple M-series, an AMD/NVIDIA desktop GPU, an Intel iGPU, and the D31-minimum machine of §14) run in each sub-batch's validation task.

### 13.3 Fixtures

- **Generated sources:** `fin-media::testing` draws frames with a **frame-id pattern**: a block code carrying the frame index and a checksum, robust to lossy coding, in a corner of an otherwise varied picture (gradients, colour bars, moving edges, text). Decoders, the sampling rule, edit lists and VFR are checked by reading ids back from decoded frames.
- **Encoded at test time:** SVT-AV1 (every OS) and the OS encoders (platform tests) encode the patterns into MP4 with `fin-container`'s muxer. VFR, edit-list, rotation, HDR-tagged and 10-bit variants are written by tweaking the muxed boxes.
- **Audio:** tones, sweeps, silences and beeps in sync with flash frames, generated in code.
- **Real footage:** a local fixture cache (`FIN_MEDIA_FIXTURES`), filled from rights-cleared or owner-made clips (arch §26 question 10), required on validation hosts (`FIN_REQUIRE_MEDIA=1`) and skipped elsewhere with a notice. Never committed; permissive licences only (AGENTS.md).

### 13.4 Determinism rules

- **`libm` is pinned exactly** (`libm = "=0.2.x"` in the workspace, the version 4C already resolves) in every crate whose output is hashed; an update is a deliberate change that re-runs the T0 and A0 suites and bumps `RENDER_VERSION` if any hash moves.
- `fin-captions`, `fin-audio` (mixer, effects, resampler, WSOLA) and the CPU reference use `libm` for transcendental functions, scalar code in a fixed order, no `mul_add`, no SIMD intrinsics, and no dependency that uses them on the hashed path. A gate like 4C's float gate (`check-eval-floats.mjs`) refuses platform float calls (`x.sin()`, `f32::powf`) in those modules while allowing `libm::`.
- **No NaN on the hashed path:** divisions by alpha are guarded (§5.1), and a debug assertion checks every reference output buffer for NaN.
- GPU shaders may use any WGSL built-in; their variance is what T1 absorbs. Both compositors share the definitions that decide *which* samples are taken (coverage, mip level, base box, matrix), computed in Rust.
- `RENDER_VERSION` is bumped by any change that alters T0 hashes or T1 references on purpose; a test fails when T0 hashes change without a bump.
- *As built (6A-1):* the T0 goldens (33 cases) were blessed on macOS arm64 and match on x86_64 Linux natively and on `wasm32-wasip1`. **Encoded output is deterministic on one machine only:** SVT-AV1 with one thread gives byte-identical output run to run, and across its C, SSE4.2 and AVX2 kernels on one x86_64 machine, but its C code is compiled with the platform's floating-point contraction (arm64 clang contracts to FMA), so bytes may differ between machine classes. `fin-codecs/tests/golden/encode.json` records one hash per machine class for comparison and is asserted only on request.

### 13.5 Conformance suite for codec backends

A backend (built-in or a plugin, pl §10.1) passes when it: decodes the generated fixture set to the right frame ids (T3 where it claims bit-exactness); seeks to any frame within its declared cost; reports its capabilities truthfully; survives truncated and corrupt input with typed errors; handles cancel within 2 s; respects `fin-shm`'s ring rules; and, for encoders, round-trips within T5 with exact frame counts and the requested GOP. The suite is the `codec` profile of `fin-plugin-test` (pl §7) and runs against every built-in backend in CI.

## 14. Performance budgets

Budgets are CI gates where CI hardware can measure them, and validation-task gates on the reference machines otherwise (an Apple M-series Mac, a Windows desktop with an AMD RX-class GPU, a Linux Intel Gen12 machine) and on a **D31-minimum machine**: Windows 10 22H2 or 11 x64, 4 cores, 8 GB RAM, an integrated GPU of the oldest class that offers WebGPU in Chromium (an Intel UHD 620-class iGPU), or an 8 GB M1 Mac.

| Area | Reference machines | D31-minimum machine |
|---|---|---|
| Preview playback, 1080p proxy, 5 layers + captions + 1 Lottie | 60 fps, ≤ 0.1 % drops (arch §17, measured 0.06 %) | 30 fps on the `1080` proxy or 60 fps on `540`, ≤ 0.5 % drops |
| Preview composite (GPU time per frame, same scene) | ≤ 4 ms p95 on an iGPU | ≤ 12 ms p95 |
| Scrub to a new frame | ≤ 50 ms p95 local (`1080`); ≤ 100 ms remote (`540`) | ≤ 120 ms p95 (`540`) |
| A/V sync, preview | \|offset\| p95 ≤ 16 ms compensated (measured 7.4 / 9.1 ms, arch §27.2); CI budget ±1 frame at 30 fps (arch §12.7) | same |
| A/V sync, export | ≤ ½ sample start offset, no drift | same |
| `fin look`, one 1080p frame, **warm** worker (§3.2 item 1), from proxy | ≤ 300 ms p95; cold worker ≤ 1.5 s | ≤ 600 ms p95; cold ≤ 3 s |
| Worker spawn to `hello` | ≤ 150 ms | ≤ 300 ms |
| Proxy `540`, hardware H.264 | ≥ 10× real time (measured 18–27×) | ≥ 4× |
| Proxy `540`, SVT-AV1, 4 threads | ≥ 5× real time (measured 9–22×) | ≥ 2× |
| Export 1080p30, hardware H.264, 60 s reel with captions and one Lottie | ≥ 4× real time | ≥ 1.5× |
| Export 2160p30, hardware H.264 | ≥ 1× real time | not required |
| Mixer, native, 16 stereo items, each with EQ + compressor | ≥ 100× real time | ≥ 30× |
| AudioWorklet render quantum (128 frames, 8 items with effects) | ≤ 0.5 ms | ≤ 1.5 ms |
| Lottie frame, 1080p | ≤ 5 ms (measured ≤ 2.9 ms) | ≤ 12 ms |
| Caption block raster, 2 lines at 1080p | ≤ 2 ms, cached thereafter | ≤ 5 ms |

**Worker memory limits** (§3.2 item 5, asked of 5C) scale with the machine's RAM: `interactive` the smaller of 2 GiB and 15 % of RAM; `batch` 1080p work the smaller of 4 GiB and 25 %; `media.render` at 2160p the smaller of 6 GiB and 35 %. A job that needs more than its limit allows fails `WORKER_OOM` with `details.limit` rather than swapping the machine.

## 15. Diagnostics (arch §21.1)

- **Path:** workers send diagnostics through 5C's envelope; `finisherd` forwards them to the `fin-node` journal, which stays the only writer. The `source` label is the diagnostics spec's to define (it owns the `Source` enum); sub-project 6 adds no variant.
- **Owned by 5C:** `job.done` (cp §3.4) and `worker.exit`.
- **Media fields in `job.done`** (the kind's `kv` 2, cp §3.4: an optional `media` object, all `meta`): `codec` and `backend` (names from static tables), `hw: bool`, `resolution` (bucket), `frames`, `rtFactor`, `cacheHit`, `copyPath: copy|zeroCopy`.
- **Owned by sub-project 6:**

  | Kind | v | Payload (all `meta`) |
  |---|---|---|
  | `media.capabilities` | 1 | once per server run: per codec and profile, decode/encode `hw|sw|none`; backend names; GPU backend, adapter type, and vendor/device ids if owner question 7 allows; copy or zero-copy path |
  | `render.stats` | 1 | per-minute histograms (like `eval.stats`): decode, composite, readback, encode micros per backend |
  | `preview.stats` | 0 | reserved for sub-project 9: drops, A/V offset histogram, scrub latency |
  | `render.sample` | 0 | reserved, training only: a sampled output frame as a `frame` blob (arch §21.1: frames are always blobs; a later sub-project adds the source) |

- **Never recorded:** file names or paths (a source is `{root, ext, hash}`), adapter name strings from drivers, decoder error strings that may hold paths (they are `scrubbed` and `content` like every error message), frames or audio outside `render.sample`.
- Worker panics become `error.panic` through 5C's forwarding, as arch §21 "Crash reporting" says.

## 16. Licensing

### 16.1 Rules applied

- No GPL/LGPL/AGPL/SSPL/EUPL in shipped code (AGENTS.md, arch §18). FFmpeg/libav*, x264, x265, libde265 (LGPL), GStreamer (LGPL), Rubber Band (GPL) and SoundTouch (LGPL) are excluded from Finisher's code. FFmpeg is reachable only as a user-installed process plugin from `finisher-plugins-copyleft`. Electron's own ffmpeg library is owner question 2.
- **MPL-2.0** is already on the `deny.toml` allow list. It is file-level copyleft: Finisher uses MPL crates unmodified, and any patch to an MPL file is published as that file. **MPL-2.0 §3.2** applies to symphonia (and any MPL crate we ship): distributing the executable obliges Finisher to tell recipients how to get the MPL files' source and that they are under the MPL. `THIRD_PARTY_NOTICES.md` carries that statement with the crate versions and links to their source, and the notices gate (§16.3) checks it is present for every MPL package in the shipped graph.
- H.264, HEVC and AAC only through OS or hardware codecs (arch §18), subject to owner questions 1, 3 and 9.

### 16.2 Components

| Component | Licence (SPDX) | Patent terms | Use |
|---|---|---|---|
| wgpu, naga | MIT OR Apache-2.0 | — | compositor |
| `rav1d` | BSD-2-Clause | AOMedia Patent License 1.0 (AV1) | AV1 decode. **Maintenance risk:** a single-sponsor port (Prossimo/ISRG); the last release is 1.1.0 of 2025-05-06, though the repository had commits in late October 2026 and no maintenance-mode announcement was found; dav1d, upstream for every fix, is the fallback |
| dav1d (fallback) | BSD-2-Clause | AOMedia Patent License 1.0 | AV1 decode if `rav1d` falls short |
| SVT-AV1 | BSD-3-Clause-Clear (since v0.9) | AOMedia Patent License 1.0 (the BSD-3-Clause-Clear text itself grants no patent rights; the grant is in `PATENTS.md`) | AV1 encode |
| libvpx | BSD-3-Clause | Google's WebM patent grant (`PATENTS`) | VP9 decode |
| libopus | BSD-3-Clause | royalty-free grants listed in its `COPYING` | Opus |
| symphonia 0.6 | MPL-2.0 | MP3 patents expired (widely reported, not re-verified); FLAC, Vorbis, ALAC royalty-free | audio decode |
| `mp4-atom` | MIT OR Apache-2.0 | — | ISOBMFF boxes |
| `cros-codecs` 0.0.6, `cros-libva` 0.0.13 | BSD-3-Clause | — | stateless decoding (6B-2); `cros-libva` only as a run-time-loading fork, if at all |
| libva (system library, loaded at run time) | MIT | drivers are the system's (owner questions 3, 9) | VA-API |
| `libloading` | ISC | — | run-time loading of libva |
| vo-aacenc (candidate, owner question 1) | Apache-2.0 (patent grant covers VisualOn's contributions only) | AAC pool (Via LA) | Linux AAC-LC encode; dormant since 2017, would be vendored and maintained by Finisher |
| `ebur128` | MIT | — | loudness |
| thorvg (C++) | MIT | — | Lottie (native); its optional bundled loaders' licences are checked in 6D |
| dotlottie-web | MIT | — | Lottie (preview) |
| `resvg`, `usvg` ≥ 0.45 | Apache-2.0 OR MIT (MPL-2.0 up to 0.44; never use those) | — | SVG |
| `tiny-skia` | BSD-3-Clause | — | SVG raster |
| `swash` | Apache-2.0 OR MIT | — | glyph raster |
| `cosmic-text` | MIT OR Apache-2.0 | — | shaping (4C) |
| `objc2`; `objc2-av-foundation`, `objc2-video-toolbox` | MIT; Zlib OR Apache-2.0 OR MIT | OS codec, licensed by Apple (to confirm with counsel) | macOS codecs |
| `windows`, `windows-sys` | MIT OR Apache-2.0 | OS codec (to confirm with counsel) | Media Foundation, D3D11VA |
| Mesa lavapipe (Docker image only) | MIT (Mesa) | — | software Vulkan without a GPU |
| `memmap2` | MIT OR Apache-2.0 | — | `fin-shm` |
| `wasm-bindgen` | MIT OR Apache-2.0 | — | `fin-wasm` |

**Considered and not used**

| Component | Why |
|---|---|
| openh264 (BSD-2-Clause) | the source carries no patent licence; Cisco covers royalties only for its own binary downloaded at install or run time (openh264.org FAQ). Its decoder targets Constrained Baseline, which phone footage is not |
| fdk-aac | its own licence, not OSI-approved, with an explicit clause that grants no patent rights |
| `rubato` | its FFT path uses SIMD (`rustfft`), outside the determinism gate; own scalar resampler instead (§8.1) |
| `mp4parse` (MPL-2.0) | demux only |
| `mp4` 0.14 (MIT) | last release 2023; no edit lists. `fin-probe` uses it today and moves to `fin-container` in 6A |
| `muxide`, `re_mp4` | young or demux-only |
| `svt-av1-sys`, `vpx-sys`, `audiopus` | unmaintained bindings; own FFI instead |

### 16.3 Licence gates

`cargo deny` sees a crate's licence, not the C library it builds or the DLL it loads. Sub-project 6 adds three gates: the first two in `pnpm gates` from 6A's Task 1, the third in the release pipeline once the server image exists (6G at the latest):
1. **Native build gate** (`scripts/check-native-deps.mjs`). It is **keyed on `cargo metadata`**: every package in the shipped graph that has a `links` field, or a build-dependency on `cc`, `cmake`, `nasm-rs`, `bindgen` or `pkg-config`, must have an entry in `third_party/native.json` (name, version or pinned tag and commit, SPDX licence, patent-licence file, notice path, and the **BLAKE3 of each licence file**). A changed licence file hash fails the gate until a person re-reviews and updates the entry. A listed licence must be on the gate's allow list (the `deny.toml` list plus `BSD-3-Clause-Clear` and the AOMedia Patent License 1.0); `THIRD_PARTY_NOTICES.md` must name the library and version.
2. **Run-time library gate** (`third_party/runtime.json`): every library Finisher loads by name at run time is listed with who provides it (`os`, `driver`, `bundled`) and its licence: libva and its drivers (`driver`, never bundled in the desktop app; the Docker image per owner question 3), `d3dcompiler_47.dll` (`os`), DXC's DLLs if ever used (`bundled`, licences to verify), Electron's `ffmpeg` library (`bundled`, owner question 2), CoreMedia/VideoToolbox and Media Foundation (`os`). A grep over the source for `libloading::Library::new`, `dlopen` and `LoadLibrary` must find only listed names.
3. **Docker image SBOM gate:** the release pipeline generates an SPDX SBOM of the server image (a build-time tool, not shipped) and fails on any package whose licence is outside the allow list, except entries in a reviewed exceptions file that names the package, version and reason (e.g. the base image's GPL system packages, which are separate programs in the image, not linked into Finisher, if counsel agrees; arch §26 question 1).

Vendored native sources are git submodules at pinned commits under `third_party/`.

### 16.4 Patents (for counsel; arch §26 question 1)

- AV1 (AOMedia Patent License 1.0), VP9 (Google's grant) and Opus (its contributors' grants) are royalty-free with defensive termination clauses.
- H.264 is not patent-free in 2026: community tracking lists patents in force in the US and Brazil until about 2030. HEVC has two pools (Access Advance, Via LA) and active litigation. AAC is licensed through Via LA; no primary source on its expiry was found. All three reach users only through codecs their OS or GPU driver provides.
- **OS codecs vs GPU vendors' encoders:** Apple and Microsoft license the codecs they ship with their OS; whether that covers an application that drives them is for counsel. GPU vendors' encoders (NVENC, AMD's and Intel's MFTs, Linux VA-API drivers) generally come with no patent licence for the application at all, which is owner question 9.

## 17. Error codes

All errors use the one shape `{code, message, details, fix?}` (arch §19). Job-level codes are 5C's (`WORKER_CRASHED`, `WORKER_HUNG`, `JOB_STALLED`, `JOB_TIMEOUT`, `POISON_ITEM`, `HELPER_FAILED`, `JOB_CANCELLED`, `JOB_INTERRUPTED`, `WORKER_UNAVAILABLE`, `WORKER_PROTOCOL_MISMATCH`; cp §6.9). Sub-project 6 adds these, each with its `fin` exit code in `crates/fin/src/output.rs`:

| Code | When | `details` / `fix` |
|---|---|---|
| `UNSUPPORTED_CONTAINER` | (exists) a container `fin-container` cannot read | `format`; fix: the codec plugin |
| `UNSUPPORTED_TRANSFORM` | a track matrix other than a multiple of 90° | |
| `MEDIA_CORRUPT` | a structurally broken file or sample | `asset`, `at` (byte offset or frame) |
| `MEDIA_LIMIT` | a file exceeds a container limit (boxes, samples, box bytes) | `limit`, `value` |
| `CODEC_UNAVAILABLE` | no decoder or encoder for codec/profile/bit depth on this machine | `codec`, `profile`, `os`, `direction`; fix: the capability report's advice |
| `DECODE_FAILED`, `ENCODE_FAILED` | the backend reported an error | `backend`, `frame` |
| `ENCODER_HUNG` | a hardware encoder produced no output within its deadline (arch §27.3) | `backend`, `adapter`; the job falls back to the next backend |
| `GPU_UNAVAILABLE` | no adapter, not even a software one | fix: install Mesa / use server-rendered preview |
| `GPU_DEVICE_LOST`, `GPU_OUT_OF_MEMORY` | device loss or allocation failure | the job reports it; 5C retries it (an interactive job once, on a fresh worker) |
| `WORKER_OOM` | the worker passed its memory limit (§14) | `limit` |
| `SHM_REFUSED` | a mapped file failed `fin-shm`'s checks (link, size, type) | `file` (job-relative), `reason` |
| `ORIGINALS_MISSING` | export or a QA render needs originals that are unreachable | `assets` |
| `ASSET_CHANGED` | an original's fingerprint no longer matches | `asset`; fix: relink |
| `PROXY_UNAVAILABLE` | preview needs a proxy that does not exist yet | `asset`, `tier`, `job` (the job making it) |
| `CACHE_IO` | the cache volume failed or is full | `freeBytes` |
| `LAYER_UNAVAILABLE` | a layer cannot be drawn (summary for export preflight) | `items: [{item, code}]` |
| `CAPTIONS_UNMEASURED` | a caption in range has no layout | `items`, `keys`; fix: `fin fonts fetch` |
| `MATTE_MISSING` | a mask's matte asset is absent or unreadable | `item`, `matte` |
| `TEMPLATE_INVALID` | a Lottie template failed validation or crashed the check | `asset`, `reason` |
| `SVG_UNSAFE`, `SVG_INVALID` | sanitising refused or parsing failed | `reason` |
| `LUT_INVALID` | a `.cube` failed to parse | `line` |
| `SOURCE_OUT_OF_RANGE` | (warning in results) a source time past the asset | `item`, `time` |
| `EXPORT_TEMPLATE_UNSUPPORTED` | a template's codec or level cannot be produced here | `missing` |
| `EXPORT_DESTINATION` | the destination is not writable or is inside `.finisher/` or `assets/` | |
| `LOUDNESS_TARGET_UNREACHABLE` | (warning) normalisation could not reach the target | `measured`, `target` |
| `PLAN_MISMATCH` | a worker's plan or caption layout differs from `fin-node`'s | `at`, `expected`, `got`; internal |
| `UNSUPPORTED_PIXEL_FORMAT` | a decoded picture Finisher does not take (monochrome, 12-bit) or a compositor cannot read | `layout`, `bitDepth` or `format`, `reason` |

*As built (6A-1):* `fin` maps `UNSUPPORTED_CONTAINER`, `UNSUPPORTED_TRANSFORM`, `MEDIA_CORRUPT` and `MEDIA_LIMIT` to exit 3 (validation), beside the existing `UNSUPPORTED_CONTAINER`; the plan's "2" is `USAGE`. The codec and GPU codes join `fin`'s table with `fin look` (6A-2).

## 18. Crate and file map

| Crate / package | New? | Responsibility | wasm32 |
|---|---|---|---|
| `fin-media` | new | pixel formats, colour tags and matrices, `MediaInfo`, the sample table and `frame_at`, `RENDER_VERSION`, media job params and results, capability and template types; test pattern generators (`testing`) | yes, pure |
| `fin-container` | new | sample tables, edit lists, fragments, limits and codec configurations over `mp4-atom`; MP4 mux; WebM demux (6B) | yes, pure |
| `fin-shm` | new | the mapped-file channel: job-directory layout, rings, open and mapping rules (§3.4) | no |
| `fin-codecs` | new | `Decoder`/`Encoder` traits; backends `apple`, `mf`, `stateless` (`cros-codecs` with `vaapi` and `d3d11va`), `nv` (6G), `av1` (rav1d, SVT-AV1), `vpx` (decode), `opus`, `audio` (symphonia, OS AAC); the process backend client; the capability probe | no |
| `fin-compositor` | new | render graph, WGSL, effects, blend, masks, ingest, coverage, mips, readback; `reference` (CPU oracle) | yes (WebGPU) |
| `fin-audio` | new | mixer, resampler, WSOLA, audio effects, loudness, limiter, waveform peaks | yes, pure |
| `fin-captions` | new | caption and text-slot raster over `fin-text` and `swash` | yes, pure |
| `fin-graphics` | new | `lottie` (thorvg FFI, native only), `svg` (sanitise, raster), `image` | partly (svg, image) |
| `fin-workers` | **5C's crate** | sub-project 6 adds the `media` family module (`src/media/`): job runners for every kind of §3.3 | no |
| `fin-jobs` | 5C's crate | sub-project 6 adds nothing to the envelope; media params and results live in `fin-media` | — |
| `jobhost` (in `fin-node`) | 5C's module | sub-project 6 adds nothing; the `onSucceeded` move uses `fin-shm` | — |
| `fin-wasm` | new (6F) | `wasm-bindgen` exports: replay and eval, compositor, mixer, demux | yes |
| `fin-text` | changed | glyph-level `shape` beside `measure` (6D) | yes |
| `fin-fonts` | changed | a read-only `FontCache` usable without the fetcher (feature split), for workers | — |
| `fin-probe` | changed | media probing through `fin-container` (6A); other kinds unchanged | — |
| `fin-core` | changed | plan growth (§4.3), the schema changes of §4.4, mask and nested edits (6D), audio effect specs (6C), `compose::duck` (sub-project 8) | yes |
| `fin-node` | changed | media job inputs (snapshot, manifest, plan samples), the media cache index (6B) | — |
| `fin` | changed | `look`, `render`, `listen`, `media caps|proxy|status`, `cache status|gc`; waiting is 5C's `fin wait` | — |
| `finisherd` | changed | `jobs/kinds/media.*.ts` (with their `onSucceeded` handlers), media HTTP endpoints and capability URLs, `media.*` RPCs that submit through `JobEngine` | — |
| `@finisher/contracts` | changed | `media.look`, `media.render`, `media.listen`, `media.capabilities`, `media.proxy.*`, `media.cache.*` (each answers `{job}` or a result, per arch §9.9) | — |
| `@finisher/preview` | new (6F) | decoder pool, scheduler, AudioWorklet glue, dotlottie-web, harness | — |

**Dependency rules:** `fin` links no media crate. `fin-node` links `fin-media` and `fin-container` (probe, job inputs) but never `fin-codecs`, `fin-compositor`, `fin-graphics` or wgpu. Only `fin-workers` (its `media` family) and `fin-wasm` link the compositor.

## 19. Sub-batches and order

Each sub-batch gets its own plan, is green on all three OSes before the next starts, and ends with a validation task on real hardware.

| Batch | Delivers | Depends on |
|---|---|---|
| **6A-1 Foundations** | `fin-media` (types, nearest-frame `frame_at`), frame-id patterns, the MP4 muxer, `fin-container` over `mp4-atom`, software AV1 (`rav1d`, SVT-AV1), the CPU reference compositor, probing through `fin-container`, the native-library gates | 4C |
| **6A-2 First pixels** | the wgpu compositor core, `fin-shm`, the `media` worker family with `media.look`, job inputs in `fin-node`, `fin look`, media fields in `job.done`, `media.capabilities`, e2e and CI, validation | 6A-1, **5C-1** (the engine, warm workers, mapped files; 5C-2 only for `fin look --no-wait`) |
| **6B-1 Stateful platform codecs and proxies** | VideoToolbox, Media Foundation (stateful) decode and encode; the capability report; HDR → SDR tone mapping; `VideoInfo` display/rotation (§4.4); proxies, thumbnails, filmstrips; the cache, its index and eviction; HTTP serving and capability URLs; WebM demux; deep probe | 6A-2 |
| **6B-2 Stateless platform codecs** | `cros-codecs` backends for VA-API (run-time libva) and D3D11VA; VA-API encode; `media.frames` | 6B-1 |
| **6C Audio** | audio demux and decode (symphonia, OS AAC, Opus), the scalar resampler, `pcm48` and `pcm16m`, waveforms, the native mixer with pan, curves and WSOLA, the built-in audio effects and `AudioPlan.effects`, loudness and the limiter, `fin listen` | 6A-2 (6B-1 for OS AAC) |
| **6D Layers** | effects (`colorAdjust`, `lut`, `blur`), adjustment layers, nested items (with their edits), masks and `matte` assets (with their edits and `media.matteEncode`), images, SVG, Lottie and `media.templateCheck`, captions raster with the plan growth of §4.3 and caption animation, `fit` | 6A-2 |
| **6E Export** | format templates, MP4 mux of encoded streams with priming edit lists, two-pass loudness, the export report and verification, `fin render`, encoder fallbacks | 6B-1, 6C, 6D |
| **6F Preview** | `fin-wasm`, `@finisher/preview` (decoder pool, scheduler, worklet mixer, dotlottie-web), server-rendered scrub frames, the harness, parity goldens and latency in Chromium and Electron | 6B-1, 6C, 6D |
| **6G Hardening** | zero-copy interop, NVDEC/NVENC on Linux, fuzzing at full scale, performance gates on reference and D31-minimum hardware, the worker sandbox profile turned on (sub-project 7C), the cross-platform validation matrix | all |

**Order:** 6A-1 runs now. **5C-1 runs alongside 6A-1** (cp §6.8: it needs only 5A Tasks 1–2); 6A-2 starts when both are in. 6C and 6D can run in parallel after 6A-2 (they touch different crates); 6B-2 can run beside 6C/6D; 6E and 6F can run in parallel after their inputs.

## 20. Decisions taken here

| # | Decision | Where |
|---|---|---|
| 1 | Workers evaluate plans themselves from a pinned snapshot; `fin-node` sends plan-hash samples that the worker must match | §3.3 |
| 2 | Sub-project 6 is a 5C worker family (`fin-workers media`) with its own job kinds; no supervisor or job store of its own | §3.1–3.3 |
| 3 | `fin-shm`: rings in job directories; the host never maps a file a less-trusted process can still write; `O_NOFOLLOW`, `nlink == 1`, size caps | §3.4 |
| 4 | Own sample tables, edit lists and limits on top of `mp4-atom`'s box layer | §6.1 |
| 5 | Stateless decoding (VA-API, D3D11VA) on `cros-codecs` with run-time-loaded libva | §6.1, §6.6 |
| 6 | `rav1d` for AV1 decode, dav1d as the measured fallback; SVT-AV1, libvpx (decode), libopus through own FFI from pinned sources | §6.4 |
| 7 | The sampling rule is the nearest frame; ties go to the earlier frame; proxies keep one frame per original frame | §7.1, §10.2 |
| 8 | `rgba16float` intermediates, 8-bit sources and output (amends arch §12.8); fixed-function and fused passes for `normal`, `screen`, `add` | §5.1 |
| 9 | Coverage-based edges and an explicit-level box mip chain, identical in both compositors | §5.1 |
| 10 | Blending and `crossfade` in display-encoded space; `dissolve` in linear light | §4.3, §5.1 |
| 11 | Ducking is gain keyframes made at edit time, not a mixer feature (amends arch §12.7) | §8.5 |
| 12 | `equalPower` = `sqrt`; constant-power pan for mono, balance for stereo; pitch-preserving WSOLA for speed ≠ 1; own scalar resampler | §8.1, §8.2 |
| 13 | Built-in audio effects in the mixer (EQ, compressor, gate, de-esser); `AudioItem.effects` | §8.4 |
| 14 | The CPU reference compositor is the golden oracle; no golden images are committed; `libm` pinned exactly | §13.2, §13.4 |
| 15 | Colour emoji from a bitmap (CBDT) build until a COLRv1 painter exists | §11.4 |
| 16 | Mattes are `matte` assets encoded by `media.matteEncode` | §11.3 |

## 21. Owner questions

1. **Linux AAC.** Linux has no OS AAC decoder or encoder, and AAC is the audio of nearly all phone footage and what TikTok and Instagram ask for. Facts (checked 2026-11-07): symphonia decodes **AAC-LC only** and **refuses** an HE-AAC stream ("aac too complex") rather than decoding its core (HE-AAC support is an open, unmerged pull request); **vo-aacenc** (VisualOn's AAC-LC encoder from Android) is Apache-2.0, whose patent grant covers only VisualOn's own contributions, and it has had no commit since 2017; **Opus in MP4** is listed by YouTube's upload settings (AAC-LC or Opus, MP4), Instagram's Reels spec lists AAC only, and TikTok's spec could not be read *(verify)*. Options: (a) decode AAC-LC with symphonia (MPL-2.0, unmodified); export AAC-LC on Linux with a vendored vo-aacenc; HE-AAC sources need the codec plugin; (b) as (a) but export Opus-in-MP4 instead of AAC on Linux, with AAC from the codec plugin when installed; (c) ship no AAC at all on Linux (AAC footage needs the plugin even to be edited); (d) license AAC from Via LA. **Recommendation: (a)**, with counsel checking the patent exposure of a shipped AAC-LC decoder and encoder. (b) is not recommended: Instagram does not list Opus. Trade-off: (a) makes Linux and the NAS Docker image usable for phone footage at some patent risk, with a dormant encoder Finisher must maintain itself and quality a little below the OS encoders'; (b) avoids an AAC encoder but risks refused or re-encoded uploads; (c) is the cleanest posture but makes the headless/NAS server nearly useless for the main use case; (d) costs money.
2. **Electron's bundled ffmpeg.** Electron builds Chromium's FFmpeg as a **shared library** (`is_component_ffmpeg = true` in its `release.gn`, "for the LGPL requirement") that the app **loads at start**, so removing it is likely impossible. The **default build includes the H.264 and AAC decoders** (`proprietary_codecs = true`, `ffmpeg_branding = "Chrome"` in `all.gn`), and every release also publishes `ffmpeg-v<version>-<platform>-<arch>.zip` built with `proprietary_codecs = false` (open codecs only). *(Verify against the pinned Electron version: the library's file names and licence file (LGPL-2.1+ expected), the codec list of the open-codecs zip, and that WebCodecs **hardware** H.264 decode still works after the swap; `proprietary_codecs` is compiled into the Electron binary itself, so it may.)* Options: (a) a documented exception for this one dynamically linked browser-runtime component: replaceable, never called by Finisher's code, notice shipped, **plus** shipping Electron's open-codecs-only build so the app carries no software H.264/AAC decoder; (b) the exception with the default build (software H.264/AAC decoders included); (c) no exception (likely impossible, since Electron does not start without it). **Recommendation: (a).** If WebCodecs hardware H.264 decoding does not survive the swap, preview proxies default to AV1 (§12). Trade-off: (a) changes arch §18's rule from "no LGPL in the app" to "no LGPL in Finisher's code", and costs H.264 preview on machines without hardware decode (they get AV1 proxies); (b) keeps software H.264 everywhere but ships patent-encumbered decoders Finisher did not license.
3. **Patent posture of hardware codec drivers in the Docker image.** On Linux, VA-API drivers (Intel's media driver, Mesa) implement H.264/HEVC, and some distributions build Mesa without them for patent reasons. If the server Docker image includes these drivers, Finisher distributes codec implementations. Options: (a) the image includes no hardware codec drivers; users mount the host's or build a documented derived image; (b) the image includes them. **Recommendation: (a) until counsel answers arch §26 question 1.** Trade-off: (a) adds a setup step for NAS users with GPUs; (b) is one-click but takes on the distribution question.
4. **Default base box: contain, cover or pixel size.** A clip whose aspect differs from the timeline (16:9 footage on a 9:16 timeline) can be (a) fitted inside with bars, (b) scaled to fill and cropped, or (c) drawn at its pixel size. **Recommendation: (a) contain**, as CapCut and Premiere do by default, with a `fit` parameter (contain/cover/none) added in 6D. Trade-off: reframe (sub-project 8) then keyframes a larger scale to fill 9:16; (b) would suit 9:16 shorts better out of the box but crops silently. The choice is hard to change later: it fixes how every existing project looks.
5. **5C runs alongside 6A batches A–C.** 6A-1 (batches A–C of the 6A plan: crates, types, containers, AV1, the CPU reference, probing) needs nothing from 5C; 6A-2 (the `media` family, `fin look`) needs 5C-1 (the engine, warm workers, mapped files and helpers), which needs only 5A Tasks 1–2 (cp §6.8). Confirm that 5C-1 is scheduled in parallel with 6A-1, so 6A-2 starts when both finish. **Recommendation: yes.** Trade-off: two batches in flight that meet at the envelope and mapped-file boundary (§3.2), checked against 5C's `test` family as soon as it exists; the alternative (5C first, then all of 6A) delays first pixels by 5C-1's length.
6. **When workers get an OS sandbox.** Workers parse untrusted media (agents download files). The sandbox is sub-project 7's (`fin-sandbox`, a `worker` profile, §3.5). Options: (a) media workers run with 5C's cleared environment, no credentials and the job-directory write rule until 7C ships, and 6G turns on the `worker` profile; (b) (a), but on Linux apply a **Landlock-only** profile in 6B already (cheap: no broker, no ACLs, the S8 code path; it confines file writes to `out/` and reads to the manifest's roots, while GPU and driver files stay readable), since headless Linux servers and NAS boxes are the most exposed; (c) block media workers on 7C. **Recommendation: (b).** Trade-off: (b) costs a few days in 6B and covers only Linux file access (no network or exec filtering until 7C's seccomp); macOS and Windows workers stay unsandboxed until 6G; (c) delays every media feature behind sub-project 7.
7. **GPU identifiers in diagnostics.** The diagnostics plan deferred this to sub-project 6. Options: (a) record GPU vendor id, device id (PCI numbers), backend, adapter type and driver major version as `meta`, never the driver's name string; (b) record nothing about the GPU. **Recommendation: (a)**: GPU bugs dominate media-engine failures, and the ids are numbers from a fixed space. Trade-off: device ids narrow down a machine more than OS and core count do.
8. **Platform scope of 6B, and NVIDIA on Linux.** Options: (a) 6B-1 ships VideoToolbox and Media Foundation, 6B-2 VA-API (Intel, AMD) and D3D11VA, all three OSes required for each merge; NVDEC/NVENC on Linux (arch §12.1) moves to 6G; (b) macOS first, merging 6B on macOS alone and following with Windows and Linux; (c) NVDEC/NVENC in 6B too. **Recommendation: (a)**, which keeps sub-project 4's "green on all three OSes" rule; the spike validated exactly these paths (arch §27.3), and NVIDIA on Linux was not measured. Trade-off: (b) gets the owner's own machine to pixels sooner but lets the other paths drift; (c) serves NAS boxes with NVIDIA cards (common on Unraid) earlier at the cost of an untested path in 6B.
9. **H.264/HEVC export: which encoders, which templates, which Windows.** Apple's VideoToolbox and Microsoft's own Media Foundation codecs come with the OS vendor's codec licence; GPU vendors' encoders (NVENC, AMD's and Intel's MFTs on Windows, VA-API drivers on Linux) generally carry **no patent licence** for the application that drives them. Windows N editions ship without Microsoft's "media-related technologies" until the Media Feature Pack is installed, and LTSC editions lack the Store codec extensions (HEVC, AV1) *(verify the LTSC part)*. And Windows 10 22H2 has been **out of support since 14 October 2025** (paid Extended Security Updates run to October 2027 for consumers and October 2028 for organisations), yet D31 lists it as a minimum. Options for export: (a) H.264 export on macOS through VideoToolbox and on Windows through Microsoft's MF encoder where the OS provides one, GPU-vendor encoders only for proxies (which never leave the machine) until counsel answers; AV1 (SVT-AV1) export everywhere; Linux H.264 export only through the codec plugin; **no HEVC export template in v1**; (b) H.264 and HEVC export through any hardware encoder found, as revision 1 assumed; (c) (a) plus HEVC export on macOS only. **Recommendation: (a)**, and revisit D31: drop Windows 10 22H2 as a supported minimum (keep it best-effort) and state the N/LTSC requirement (the Media Feature Pack) in the capability report. Trade-off: (a) makes Linux and some Windows machines export H.264 only through the plugin or fall back to AV1, which some platforms re-encode with quality loss; (b) is the best user experience and the largest unexamined exposure; (c) serves iPhone-centric creators at little extra exposure.

Question numbers are kept stable across revisions (sibling specs cite them); question 9 is new in revision 2.

## 22. Contradictions and gaps found in earlier documents

1. **Arch §18 vs Electron:** Electron's bundled `libffmpeg` is LGPL-2.1 (owner question 2).
2. **Arch §12.7 vs the plan contract:** ducking in the mixer would need analysis data that `AudioPlan` does not carry (resolved by §8.5).
3. **Arch §12.3 "consumes eval output only" vs `FramePlan`/`AudioPlan`:** caption paint and placement, caption animation, matte identity and time, transition looks and audio effects are missing or undefined (resolved by §4.3); the engine also needs asset facts, which the render manifest (§4.2) provides without timeline semantics.
4. **Arch §12.8 "8-bit internally"** vs float intermediates (amended by §5.1).
5. **Arch §12.8 "frame blending or nearest-frame, chosen per item"** has no model parameter; v1 uses nearest frame (§10.2).
6. **Arch §1 "animated captions"** vs a model with no caption animation fields (§4.3).
7. **Core §5.6 and arch §27.10 say fonts are "always bundled"**; the 4C plan (Decisions 29–39) and the code fetch them on demand into a pinned, content-addressed cache and bundle none. The condition that matters (pinned bytes, never system fonts) holds either way; core §5.6 and arch §27.10 should be reworded when 4C's Task 28 records its decisions.
8. **`fin-probe` vs arch §27.3:** rotation, VFR and edit lists are not handled (§6.3).
9. **Arch §24 order:** sub-project 3 (walking skeleton with proxy preview) and sub-project 5 (jobs) are not built, yet sub-project 6 needs jobs (now from 5C, owner question 5) and absorbs the skeleton's preview client (§12).
10. **Arch §12.1 lists NVDEC/NVENC for Linux** without spike evidence (owner question 8).
11. **Core §5.1 transitions** name `crossfade` and `dissolve` without a visual difference (§4.3).
12. **Arch §6.2 lists mattes among regenerable analysis artifacts**; they are now `matte` assets (§11.3, pl §10.3).
13. **Arch §12.7 lists EQ and compressor as plugins;** the coordinator moved the built-in four into the mixer (§8.4).
14. **D31 lists Windows 10 22H2,** which has been out of support since 14 October 2025 (owner question 9).
15. **cp §6.4** deletes a job's directory when the job ends, with no step in which the family moves its outputs; sub-project 6 asks for an `onSucceeded` hook (§3.2 item 3).
16. **cp §6.3** has no pool for hardware encoder sessions and no RAM-scaled memory limits; sub-project 6 asks for `hwenc` and the limits (§3.2 items 2 and 5).
17. **Cross-design naming:** pl revision 2 (§2, §3.5) calls the one worker binary `fin-worker` with roles `library` and `media`; cp revision 3 and the coordinator's decision call it **`fin-workers <family>`**, which this spec uses. Sub-project 8 should rename.
18. **Cross-design references:** pl cites media §8.4 for loudness; in this revision loudness is §8.6 (§8.4 is the built-in audio effects).

## 23. Risks

| Risk | Mitigation |
|---|---|
| GPU variance across vendors breaks T1 | the CPU oracle isolates shader bugs from tolerance issues; per-stage tolerance tests; WARP/lavapipe as CI baselines; hardware rows in each validation task |
| `rav1d` stalls or lags dav1d | 6A measures both; dav1d FFI is the fallback |
| `cros-codecs` is shaped for ChromeOS and its VA-API backend assumes a linked libva | 6B-2 starts with a spike; our own backends over its parsers and state machines if its backend cannot use run-time loading |
| `mp4-atom` allocates on hostile input | `fin-container`'s pre-checks and caps; fuzzing of the combination; own box layer for affected boxes as the fallback |
| 5C slips and blocks 6A-2 | 6A-1 needs nothing from 5C; the `media` family is tested against 5C's `test.*` harness as soon as it exists |
| Hardware encoders hang or pick the wrong adapter (arch §27.3) | explicit adapter binding; `ENCODER_HUNG` with a deadline and fallback to the next backend; capability probe encodes a test clip |
| HEVC on Windows without the extensions is unverified (arch §27.11) | 6B-2 validation row on a machine without them; D3D11VA path; guidance |
| Linux desktop WebGPU and real vsync are unverified (arch §27.11) | server-rendered scrub fallback; 6F validation on a real Linux desktop |
| Zero-copy interop needs wgpu hal APIs that change between versions | copy path first and always available; interop behind a feature, pinned wgpu |
| thorvg crashes on malformed templates | `media.templateCheck` in a throwaway worker; export in the render worker; 5C's retries and poison handling |
| Patent exposure (H.264/HEVC/AAC) | OS codecs first; owner questions 1, 3 and 9; counsel before launch |
| The engine grows into many crates and long files | crate map fixed here; 600-line limit; one module per stage |

## 24. Review findings (revision 2) and where they are resolved

| Finding | Resolution |
|---|---|
| **B1** own supervisor, job store, `media.jobs.*`, `fin wait` duplicate 5C | §3.1–3.3, §3.6, §18: the `media` family of 5C's `fin-workers`, media kinds and `fin-shm` only; 5C's names and codes (`fin-jobs`, `WORKER_HUNG`, `POISON_ITEM`, `HELPER_FAILED`, `jobs.*`, `JobEngine`); warm `interactive:media` workers for `fin look` (§3.2, aligned with cp revision 3); remaining asks: `hwenc`, RAM-scaled limits, `onSucceeded`; owner question 5 rewritten |
| **B2** sample-and-hold sampling | §10.2: nearest frame, ties to the earlier frame, exact rationals; §7.1 proxies and §13.2 goldens follow it |
| **H1** ownership | §3.6 table; audio effects §8.4 and `AudioItem.effects` §4.3; `pcm16m` and `media.frames` §3.3, §7.2; codec plugins as 5C sibling helpers §3.2, §6.5; `matte` assets §11.3, §4.4; `job.done`/`worker.exit` to 5C, media fields §15; `Source` to the diagnostics spec §15; sandbox requirements §3.5; `fin-shm` rules §3.4 |
| **H2** VA-API/D3D11VA are stateless | §6.1, §6.6: `cros-codecs`, libva via `libloading`; 6B re-estimated (≈1.6×) and split into 6B-1/6B-2 with a spike first |
| **H3** owner question 2 | rewritten (Electron loads ffmpeg at start; default build has H.264/AAC; recommend the exception plus the open-codecs zip; WebCodecs hardware H.264 to verify; AV1 proxies otherwise; facts marked to verify) |
| **H4** H.264/HEVC export | owner question 9 (OS vendor vs GPU vendor encoders, Windows N/LTSC, HEVC template, D31 and Windows 10's end of support); §6.2, §9.2, §16.4 |
| **H5** licence gate | §16.3: keyed on `cargo metadata` (`links`, native build-deps), licence-file hashes, a run-time library gate, the Docker SBOM gate |
| Licences: rav1d, MPL §3.2, `mp4-atom` | §16.2 (rav1d risk), §16.1 (MPL §3.2 in `THIRD_PARTY_NOTICES.md`, gated), §6.1 (`mp4-atom` for boxes; tables, edit lists and limits ours; justified) |
| **M1** determinism | §13.4 (`libm` pinned exactly), §5.1 (guarded division), §8.1 (own scalar resampler), §5.1 (box mip chain, explicit level, in both compositors) |
| **M2** no clamp-to-border | §5.1 coverage edges in shader and reference |
| **M3** budgets | §14: D31-minimum column; fixed-function `normal`/`screen`/`add` with fused passes (§5.1); RAM-scaled worker limits |
| **M4** schema | §4.4: every model change and its versioning; plan additions re-bless the 4C corpus and reel goldens; `Probe.media` never in `asset.add` records |
| **M5** 6A plan | the plan: muxer before demuxer, CRF-40 check after AV1, split into 6A-1 and 6A-2, no JSON media index |
| **M6** v1 cuts | §1 "Cut from v1"; §6.2, §10.1, §12 |
| **M7** owner questions 1 and 6 | question 1 (vo-aacenc, AAC-LC only, Opus-in-MP4 acceptance); question 6 option (b) Landlock-only in 6B |
| Small: 29.97 fps offset; VP9 on macOS | §9.1 (≤ ½ sample, no drift), §6.2 (`VTRegisterSupplementalVideoDecoderIfAvailable`) |
