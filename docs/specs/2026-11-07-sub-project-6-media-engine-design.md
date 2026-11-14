# Sub-project 6: Media engine — Design

- **Status:** revision 4 (2026-11-08). **Sub-batch 6A (6A-1 and 6A-2) is built**; §25 records it as built, each decision marked **As built** or **Deviation**, with the review fixes, the owner decisions as applied, the measured timings and the residuals; the "As built" notes in §3.3, §3.4, §5.1, §5.2, §6.1, §6.4, §10.1, §13.4, §15, §17 and §20 point there. Verified on Linux x64 (lavapipe), with compositor and `fin-shm` runs on Apple Silicon (Metal) and Windows 11 (WARP, D3D12); the cross-platform run of the whole of 6A (plan Task 16) is the checklist of §25.6, not yet run. 6B onward is not implemented. Revision 3 (2026-11-07) recorded the owner's decisions of 2026-11-07 in §21 and applied them to the body; three follow-up decisions of the same day (the proxy encoder scope, Electron's ffmpeg made provisional, Landlock-disabled hosts) are in §21 and applied in §3.5.1, §6.1, §6.2, §7.1, §16.1 and §16.3. Revision 2 followed an independent review of revision 1 (`e6f7e64`); §24 maps each finding to its resolution. The plan for the first sub-batch (6A) is `docs/superpowers/plans/2026-11-07-sub-project-6a-media-foundations.md`.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §4 (rule 3), §7, §12, §17 (performance budgets), §18, §21.1, §22 and §27.2–27.3 and §27.10 for sub-project 6. Where the two disagree, this document wins for sub-project 6, and arch §27 gets a pointer when a sub-batch merges.
- **Builds on:** sub-project 4 (`docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`, cited as *core §n*): `fin-core::eval` (core §5.4), captions and `fin-text` (core §5.5–5.6), the asset kinds (core §5.7), and the diagnostics journal (arch §21.1).
- **Siblings** (all in revision; cited by branch): sub-project 5 (*cp §n*, `sp5-design`, `docs/specs/2026-11-07-sub-project-5-control-plane-design.md`): jobs, the worker binary, the library stage framework; sub-project 7 (*ar §n*, `sp7-design`): `fin-sandbox`; sub-project 8 (*pl §n*, `sp8-design`): plugins, analyzers, codec and matte interfaces. **Ownership splits fixed by the coordinator** are stated where they apply and summarised in §3.6.
- **Licence facts** in §16 were checked against primary sources on 2026-11-07 (each project's LICENSE/COPYING/PATENTS file, crates.io metadata). Facts still to verify are marked *(verify)*. They are not legal advice; arch §26 open question 1 (counsel on codec patents) still stands, and decisions 1, 2 and 9 (§21) are on TASKS.md's "Counsel before release/upload" item.

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
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS arm64, Windows 11 x64 and Linux x64 (kernel ≥ 6.2, D31);
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

**Cut from v1 by owner decision (§21):** built-in HEVC encoding and any built-in HEVC export template (HEVC export only through the codec plugin, decision 9); H.264 export through GPU vendors' encoders (export uses OS vendors' encoders only, decision 9); built-in H.264 export on Linux (the codec plugin, or AV1); hardware codec drivers in the Docker image (decision 3); Windows 10 (D31 is Windows 11 only, decision 9).

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
- *As built (6A-2):* only `media.look` and `media.capabilities` exist (both warm, on `interactive:media`); the `media` family forwards the one environment variable `FIN_GPU` (`FAMILY_ENV` refuses any other name, and any `env` for a plugin family); its memory rule is `{ramFraction 0.5, capMiB 6144}`. The job's params carry `projectId`, `branch`, `rev` and `outDir` beside the worker's (the worker ignores them), because `dedupe` and `prepare` see only the params. `prepare` snapshots the head it finds and reports that rev. Frames are named `<timeline>-<n>_<d>-<w>x<h>-<plan hash, 8 hex>.png`; the size is in the name because the plan hash does not cover it. Details and deviations: §25.1.

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
- *As built (6A-2), `fin-shm`:* every path is walked segment by segment without following links (unix `openat(O_NOFOLLOW)`, Linux `O_PATH`; Windows `FILE_FLAG_OPEN_REPARSE_POINT` per segment, with the final path re-checked); a FIFO or device is refused before it is opened. A warm worker outlives its job, so `media.look` moves a PNG by **copy** (`commit_output_copy`: the checked handle is copied into a host-created `O_EXCL` temporary beside the destination, synced and renamed over it), never by renaming a file the worker may still hold. Workers write their outputs with `create_output` (an entry of that name, a link too, is unlinked first), the host writes `in/` with `write_input`; `--replay` uses `create_new_output`. Refusals are `SHM_REFUSED {file, reason}` with `file` job-relative before it reaches a client. Rings are fixed-size seqlock slots (a 32-byte header, a `{seq, len}` table, slots from a 64-byte-aligned offset); 6A uses no ring yet. Details: §25.1.

### 3.5 Requirements on sub-project 7's `worker` sandbox profile

Sub-project 7 (`fin-sandbox`, ar §7C) owns the sandbox and applies a `worker` profile to media workers and their helpers. **Owner decision 6:** a Linux **Landlock-only** profile ships in 6B (§3.5.1); the full `worker` profile below is turned on in 6G once 7C lands. The full profile must allow, and only allow:
- **GPU and media engines:**
  - macOS: Metal and the IOKit user clients it opens (`IOSurfaceRootUserClient`, the GPU's accelerator client), IOSurface, and the CoreMedia/VideoToolbox services hardware decode and encode use;
  - Linux: `/dev/dri/renderD*` (read/write), `/dev/dri/card*` read-only for enumeration where Mesa needs it, the Vulkan ICD and driver files, `/sys/class/drm` and `/sys/dev/char` reads; `/dev/nvidia*` in 6G;
  - Windows: D3D11/D3D12 and Media Foundation inside a normal AppContainer (not LPAC), including the hardware MFTs; to be proven in 6B's validation, since S8 tested an AppContainer without GPU work.
- **Reads:** library roots and project `sources/` (only the files the manifest names, where the OS allows per-file grants; else the roots), the font cache, the media cache (read), the job's `in/`.
- **Writes:** the job's `out/` only; a private temp directory.
- **Denied:** network (all), process creation (`process-exec`/`process-fork` on macOS, a seccomp filter on `execve`/`clone` without `CLONE_THREAD` on Linux, the job object's active-process limit of 1 on Windows), the rest of server data, credentials, the agent listener.

#### 3.5.1 Before 7C: the baseline and the Linux Landlock profile (6B)

- **Every OS, from 6A:** media workers and their helpers run with 5C's cleared environment, no credentials, and the job-directory write rule (`out/` only, §3.4). macOS and Windows workers have no OS sandbox until 6G.
- **Linux, from 6B-1:** the worker applies a Landlock ruleset to itself before it opens any input, through `fin-sandbox`'s Landlock policy code if 7C's crate is far enough along, else a small module in `fin-workers` that 7C later replaces. A one-job worker restricts itself to that job: writes to its `out/` and a private temp directory; reads to the manifest's sources, the font and media caches and its `in/`. A warm worker (§3.2 item 1), which serves several jobs and whose rules can only narrow, restricts itself once at start to the library roots, the caches and `<server-data>/jobs/`. Both add the GPU and driver files of §3.5 (reads, plus read/write on `/dev/dri/renderD*`). No seccomp: network and process creation stay open until 7C.
- *As built (7C, agent §25.1): the Linux Landlock phase is delivered by `fin-sandbox`* (no module in `fin-workers`): `fin-workers` registers `fin_sandbox::self_apply` first in `main` and applies the `worker` profile before `hello`. A cold worker reads its job's `in/`, each source and font file its job names (per-file grants from `KindSpec.reads`), the caches and the GPU rows of `data/linux/gpu.txt` (render nodes read-write, the Vulkan ICD and driver files, `/sys/class/drm`, `/sys/dev/char`), and writes `out/` and its private temp; it gets `EXECUTE` on nothing (not even its own binary: the suite asserts the denial). **A warm worker is not granted `<server-data>/jobs/`** (7C Task 12): it gets its own directory's `tmp/` and `jobs/`, and under an applied profile its project's `sources/`, the libraries' roots and the font cache; each job visits it for its attempt, and a job reading outside those roots (a source outside `sources/`, outside every library) runs cold with per-file grants. `media.capabilities.sandbox` is `landlock` once the self-sandbox applied (also on Windows, after the token check), `worker` on macOS (the full Seatbelt profile, Task 8), and `none` whenever nothing was applied: Landlock disabled, or the profile not in `READY` (every host today). TCP is confined only at Landlock ABI ≥ 4 (kernel 6.7). **No seccomp yet** (deferred, agent §25.2): network beyond TCP (UDP) and `fork` stay open on Linux. The profile applies on an OS only once `worker` is in `READY` (agent §25.3); until then media workers run with the baseline above and `SANDBOX_DISABLED {profile: worker, reason: profileNotReady}`.
- *As built (6B-1 Task 14, review fixes): a Vulkan driver named by the environment.* The `media` family passes `FIN_GPU` and the Vulkan loader's ICD variables `VK_ICD_FILENAMES`, `VK_DRIVER_FILES` and `VK_ADD_DRIVER_FILES` from `finisherd`'s environment (set by whoever started it, never by a client; `FAMILY_ENV`). When they name ICD manifests, the worker profile's GPU rows (`fin_sandbox::linux::gpu::devices`, used by the spawner's grants and the self-sandbox alike) add read access to each manifest and to the driver library its `ICD.library_path` names, canonical, the files only and never their directories: an entry that is a directory grants the `.json` manifests directly in it; a relative library path resolves against the manifest's directory, as the loader does; a bare library name is the dynamic loader's and is covered by the system rows; an entry that is missing, relative, unreadable or malformed grants nothing more. So a host whose only driver is a user-space one (SwiftShader under `~/.cache`) renders under Landlock.
- **Landlock ABI:** D31's Linux floor is kernel ≥ 6.2 (owner decision 2026-11-07), so every supported kernel offers ABI ≥ 3 (`REFER`, `TRUNCATE`); the profile uses ABI 3 and has no code path for older ABIs.
- **Hosts where Landlock is disabled** (kernel ≥ 6.2 with Landlock turned off: a kernel built without it, `landlock` missing from the `lsm=` list, or a container runtime whose seccomp profile blocks the Landlock syscalls) are still supported, and on them **everything runs, with warnings** (follow-up decision F2 of 2026-11-07, ar §8.9): the worker runs with the baseline above; the host shows a **persistent status warning** (`SANDBOX_DISABLED {profile: worker}` in `server.status` and `fin status`) and records a **diagnostics event** (`sandbox.unconfined`); `media.capabilities` records `sandbox: none` (§15), and the capability report says how to enable Landlock. Media jobs are never refused for this, and no setting overrides or silences the warning; the sandbox is defence in depth for media parsing, as it is for the other profiles on such a host.

### 3.6 Ownership splits (coordinator decisions)

| Topic | Owner | Sub-project 6's part |
|---|---|---|
| Jobs, scheduler, worker binary, envelope, kill tree, warm workers, helpers, mapped-file lifecycle, retries, poison, `jobs.*`, `fin wait` | 5C | the `media` family and kinds; uses and asks in §3.2 |
| Sandbox | 7 (`fin-sandbox`, `worker` profile) | requirements §3.5; the Linux Landlock-only profile of 6B (§3.5.1) |
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
| Base box | how a source maps into the frame at scale 1 is undefined | rule in `fin-core` docs (contain, the default by owner decision 4) and a `fit` parameter (contain/cover/none) | 6A (rule), 6D (param) |
| Transition looks | `crossfade` and `dissolve` have no defined difference | `crossfade`: both layers mixed in encoded (display) space; `dissolve`: mixed in linear light. Both use `progress` through `curve` | 6A |
| Audio effects | `AudioItem` has gain, pan and shapes only | `AudioItem.effects`: the item's enabled audio effects with parameters resolved over the item's span (piecewise like gain), in order | 6C |

### 4.4 Schema changes and their versioning

Every model change follows core §3.2 (`formatVersion`, `minReaderVersion`, frozen upcasters). Sub-project 6 makes these:

| Change | Batch | Versioning |
|---|---|---|
| `VideoInfo.display`, `rotation`, `pixelAspect` (optional) | 6B | format bump; no upcaster can know an old asset's rotation, so absent means "unknown" and the engine uses the file's `MediaInfo`; `asset.reprobe` (a new op) fills them |
| `fit` item parameter | 6D | format bump (older readers refuse `param.set fit`); absent = contain, no upcaster |
| Asset kind `matte`; `Mask.matte` an asset id | 6D (or 8H) | **one `SCHEMA_VERSION` bump (6 → 7; written 2 → 3 before the keyframe-id bump to 3, review batch J's content-time fields at 4, 6B-1's video fields at 5 and 6C's audio effect kinds at 6) shared with sub-project 8's `Project.plugins`** (pl §4), made by whichever lands first, with frozen upgrade steps and goldens; old free-string mattes stay unresolved (`MATTE_MISSING` until relinked) |
| `mask.set`, `mask.remove` ops (4C Decision 48) | 6D | format bump (new op kinds) |
| `nested` insert edits (4C Decision 48) | 6D | format bump (new op kinds) |
| `CaptionStyle.animation` | 6D | format bump; absent = `none`, no upcaster |
| Audio effect kinds in the param registry (`eq`, `compressor`, `gate`, `deEsser`) | 6C | format bump (new registry kinds) |

- **Plan additions** (`CaptionPlan.paint`, `WordPlan` animation fields, `AudioItem.effects`) change the 4C eval corpus hashes (`crates/fin-core/tests/eval_corpus/expected.json`) and the reel goldens (`scripts/e2e/reel-golden/frames.json`, `audio.json`). The task that adds a field re-blesses them in the same commit and says why.
- **`Probe.media` never reaches an `asset.add` record.** `asset.probe` answers it for callers to read; the op's payload keeps only the model's fields, and a test checks that a committed `asset.add` has no `media` key.

*As built (6B-1 Task 11):* the `VideoInfo` change is **`SCHEMA_VERSION` 5** (owner decision 2026-11-09, "bump per change"; v3 is stable keyframe ids, v4 review batch J's content-time fields, which landed first):
- **Model.** `VideoInfo {width, height, rate, display?: DisplaySize {width, height}, rotation?: Rotation (0|90|180|270, clockwise), pixelAspect?: Rational}`; `width`/`height` stay the coded size. Each new field is omitted when absent (absent = unknown), so an asset written before schema 5 keeps its bytes. `Rotation` moved from `fin-media` into `fin-core` (`fin_media::Rotation` re-exports it), so the model and `MediaInfo` share one type and one generated schema (`DisplaySize` is its own name, apart from `fin-media`'s `Size`).
- **Upcast.** Frozen identity steps `upgrade/snapshot_v4.rs` and `upgrade/record_v4.rs` (a record labelled v4 holding a v5-only shape is `INVALID_FORMAT`): every v4 value is a valid v5 value, and no rotation is guessed for an old asset; the bump only makes an older reader refuse v5 values (`NEWER_FORMAT`) instead of dropping the fields. A schema-4 client tx is committed as schema 5. DB migration `m0007_video_display` (frozen, literal values) raises `meta` `formatVersion`/`minReaderVersion` to `"7"` (`DB_VERSION` 7), so a DB-6 binary refuses the database with `DB_TOO_NEW`. The eval corpus is relabelled to v5 (plan hashes unchanged); store fixtures and the `fin view` golden stay older and upcast on read.
- **`asset.add`** fills the three fields from `fin-probe` (the demuxer's `MediaInfo.video`); `MediaInfo` itself is still never stored.
- **`asset.reprobe {asset, video?, audio?}`** (a client op, `ops/asset_reprobe.rs`): replaces a `media` asset's `video` and `audio`; the inverse is an `asset.reprobe` with the previous values; the diff and footprint are the asset's `video` and `audio` params. `NOT_FOUND` for an unknown asset, `INVALID_OP` for a non-media asset or (policy) neither stream, `TRACK_KIND_MISMATCH` when an item showing it would lose its track's stream.
- **RPC `asset.reprobe {projectId, asset, branch, deep?, txId, author, dryRun?, argsHash?}`** (`project.write`, `jobs.submit` with `deep`; a branch write) and **`fin asset reprobe <aN> [--deep]`**: the server probes the asset's file through `asset.probe`'s probe, refuses a moved fingerprint (`ASSET_CHANGED`, exit 3), with `deep` runs `media.probeDeep` first and refuses an undecodable asset with **`CODEC_UNAVAILABLE`** (exit 5, `details {asset, codec, direction: decode, failure}`), then commits one `asset.reprobe` record at the head it read, under the caller's `txId`. It answers `{asset, video?, audio?, deep?, rev, txId}`. A `txId` already in the log is answered from its record before anything is probed (a retry meets neither a moved file nor a full queue; another command's record under that key is `IDEM_KEY_REUSED`). A probe that finds the streams the model holds commits nothing and answers `unchanged: true` with the head's `rev`; so does a dry run (`dryRun: true`). A file that is no longer media is `ASSET_CHANGED` with `reason: kind`.

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
- **Base box:** a source's base box is its display size (after rotation and pixel aspect) fitted inside the timeline frame ("contain"), centred. `transform` then applies: `anchor` is item-normalised in the base box, `position` is pixels from the frame centre, `scale` multiplies the base box, `rotation` is degrees clockwise. **The anchor convention:** the anchor point is placed at the frame centre plus `position`, and the item scales and rotates about it, so the defaults (anchor ½, ½; position 0) leave the base box centred, and changing the anchor alone moves the item (as Premiere and After Effects do). The matrix is built in Rust (`f64`, `libm` for `sin`/`cos`, deterministic) and passed to shaders as `f32`. "Contain" is the default by owner decision 4; 6D adds the `fit` parameter (contain/cover/none) for items that want another rule.
- **Sampling and downscaling** (defined identically in both compositors, §13.4):
  - magnification and scale ≥ ½: bilinear sampling of the source at the output pixel centre;
  - minification: each source gets a **mip chain by exact 2×2 box averaging** (our own pass; WebGPU generates no mipmaps), and the layer samples with an **explicit level** `L = log2(1 / s)` where `s` is the layer's smaller axis scale in output pixels per source pixel (constant per layer, since transforms are affine; computed in Rust with `libm`), bilinear in levels `⌊L⌋` and `⌈L⌉` blended by the fraction. No derivative-based LOD, which differs between GPUs.
  - *As built (6A-2), deviation:* the mip levels are **separate textures**, sized like the reference's chain (`⌈w/2⌉`; WebGPU's own mip levels round down, which would change the average), and the layer shader mixes `textureSampleLevel(…, 0)` of `⌊L⌋` and `⌈L⌉` by the fraction. Both compositors share `placement` (quad, inverse, level, bounds, transition weight) and the layer walk with its skips, so they cannot disagree on which samples are taken.
- **Edges without clamp-to-border:** WebGPU has no border colour, so samplers clamp to edge and the shader computes **coverage** per output pixel: for each of the quad's four edges (after crop), `clamp(d + 0.5, 0, 1)` where `d` is the signed distance in output pixels from the pixel centre to the edge (positive inside); coverage is their product and multiplies the layer's alpha. The CPU reference computes the same coverage, so rotated and fractional edges match.
- **Pipelines and textures:** one pipeline per (stage, blend mode, target format), created on first use; a texture pool keyed by (size, format, usage) recycles intermediates; decoded frames live in a per-asset ring.
- **Plugin effects later:** a WGSL effect (sub-project 8, pl §10.4) is a fragment function `fn effect(uv: vec2f, src: vec4f) -> vec4f` plus a uniform block generated from the plugin's `ParamSpec`s. The built-ins use the same ABI.

### 5.2 Native

- **Backends:** Metal on macOS; D3D12 on Windows (Vulkan as an alternative); Vulkan on Linux. The adapter is chosen by the capability report: the discrete GPU for batch work unless the user picks another (Chromium ignores `powerPreference`, arch §27.2, but native wgpu does not).
- **Shader compiler on D3D12:** wgpu's FXC path (`d3dcompiler_47.dll`, part of Windows) by default; DXC only if a measured need appears, and then its DLLs go through the run-time library gate (§16.3).
  - *As built (6A-2):* wgpu 30.0.1 with FXC pinned; a loaded-module listing of the test processes shows `d3dcompiler_47.dll` and never `dxcompiler.dll`. `FIN_GPU=auto` prefers D3D12 on Windows, then the primary backends (it chose Vulkan before this was fixed, and FXC refused `blend.wgsl` for `overlay` once the blend mode was substituted, which a test now builds on every backend). `FIN_GPU=software` is `force_fallback_adapter` plus a CPU device type: lavapipe on Linux, WARP on Windows; macOS has no software adapter, so `software` skips there with a notice (`FIN_REQUIRE_GPU=1` fails it instead). Device loss and out-of-memory are typed (`GPU_DEVICE_LOST`, `GPU_OUT_OF_MEMORY`), never a panic.
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
- **Codecs are the OS's or hardware's for patent-encumbered formats** (H.264, HEVC, AAC) and **royalty-free software** for AV1, VP9 and Opus (D13, arch §18). Finisher ships no H.264 or HEVC implementation of its own. Two owner decisions refine this:
  - **Linux AAC-LC** (decision 1): Linux has no OS AAC codec, so AAC-LC is decoded with symphonia and encoded with a vendored vo-aacenc (§6.4); HE-AAC sources need the codec plugin. Counsel checks the exposure before release.
  - **Export encoders** (decision 9, scope stated by follow-up decision F3 of 2026-11-07): the rule "**OS vendors' encoders only**" for H.264 and HEVC **applies to exports only**. H.264 export (and QA renders and mattes, which follow it, §6.2) uses VideoToolbox on macOS or Microsoft's own H.264 encoder MFT on Windows, never a GPU vendor's encoder (NVENC, AMD's or Intel's MFTs, VA-API drivers). **Internal preview proxies are exempt: they are never distributed, stay inside the user's installation, and may use any hardware encoder, GPU-vendor ones included.** Elsewhere export is AV1 (SVT-AV1), or H.264 through the codec plugin. There is no built-in HEVC encoding; HEVC export comes only from the codec plugin.
  - *As built (6A):* the royalty-free software codecs are C libraries built from pinned submodules (§6.4): SVT-AV1 with cmake, **dav1d with the `cc` crate** (`fin-dav1d-sys`, no meson or ninja), each behind a small C shim of Finisher's so Rust mirrors no library struct; nasm is the one extra tool, for their x86_64 assembly.
- **OS codecs are driven at the codec level:**
  - macOS: `VTDecompressionSession` / `VTCompressionSession` (stateful: the OS parses the bitstream);
  - Windows: Media Foundation transforms (`IMFTransform`; the hardware MFT bound explicitly by adapter, arch §27.3), stateful; and **D3D11VA directly** for HEVC without the HEVC Video Extensions (arch §27.3), which is **stateless**;
  - Linux: **VA-API, which is stateless**: the application parses the bitstream, fills picture and slice parameters and manages reference frames.
- **Stateless decoding uses `cros-codecs`** (ChromeOS; 0.0.6 of 2025-06-18, BSD-3-Clause in both crates.io metadata and the repo LICENSE; the repo is active) for bitstream parsing, DPB and reference management and the decoder state machines. Its VA-API backend decodes H.264, HEVC, VP8, VP9 and AV1 and encodes H.264, VP9 and AV1 (no HEVC encode); it is Linux-only by design ("support for systems other than Linux" is a stated non-goal) and reaches libva through `cros-libva` (0.0.13, BSD-3-Clause), whose `build.rs` **links** `libva` and `libva-drm` through `pkg-config`.
  - **libva is loaded at run time, never linked**, so the binary starts on machines without libva and the capability report says what is missing. That needs either a `cros-libva` fork whose FFI table is resolved with `libloading`, or Finisher's own VA-API backend over `cros-codecs`' parsers and state machines; the 6B-2 spike decides which.
  - **D3D11VA** gets Finisher's own backend; it reuses `cros-codecs`' parsers and state machines if the crate builds on Windows without its `vaapi` feature *(verify in the spike)*, else Finisher writes the DXVA parameter filling on its own parsers for H.264 and HEVC only (AV1 and VP9 go through MF there).
  - **HEVC encoding on VA-API** is not in `cros-codecs`, and none is needed: there is no built-in HEVC encoding (decision 9).
- *As built (6A-1), `fin-container`'s limits* apply per file (`maxBoxes` 1 M, `maxBoxBytes` 64 MiB, `maxDepth` 16) and per track: `maxSamples` (10 M) bounds a track's samples, which are also at most the file's length in bytes (a fragment run with no per-sample fields costs no bytes a sample), and bounds the **presented frames** after edit lists (`maxPresentedFrames`; up to 1,024 edits may repeat the media), counted before any frame is stored, with the samples an edit looks at bounded by twice that (`maxEditWork`). A video sample in a fragment must have at least one byte. Times that overflow 64 bits are `MEDIA_CORRUPT`.
- **Process-based backends** (the FFmpeg plugin, others) implement the same `Decoder`/`Encoder` traits through a helper process that 5C starts as the job's sibling, with packets and frames in `fin-shm` rings (§3.4, §6.5).
- **Every backend has a vendor class** in `fin-media`'s static tables: `os` (VideoToolbox, AudioToolbox, Microsoft's MFTs), `gpu` (a GPU vendor's MFT, VA-API, NVDEC/NVENC), `software` (dav1d, SVT-AV1, libvpx, libopus, symphonia, vo-aacenc) or `plugin`. The export encoder policy (§9.2) and the capability report are keyed on it.

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

| Codec | macOS 14+ (Apple Silicon) | Windows 11 | Linux (kernel ≥ 6.2) | Software |
|---|---|---|---|---|
| H.264 (8-bit, High) | VideoToolbox (HW) | MF + hardware MFT | VA-API (Intel, AMD) through `cros-codecs`; NVDEC (6G) | **none** (patents) |
| HEVC Main / Main10 | VideoToolbox (HW) | MF with the HEVC Video Extensions, else D3D11VA through `cros-codecs` | VA-API through `cros-codecs`; NVDEC (6G) | **none** |
| AV1 | VideoToolbox on M3 and later | MF (AV1 extension) or D3D11VA | VA-API | dav1d (all OSes) |
| VP9 | VideoToolbox after `VTRegisterSupplementalVideoDecoderIfAvailable(kCMVideoCodecType_VP9)` (verified on an M4) | MF / D3D11VA | VA-API | libvpx |
| ProRes 422/4444 | VideoToolbox | — | — | none (plugin) |

**Video encode**

| Codec | macOS | Windows 11 | Linux | Software |
|---|---|---|---|---|
| H.264, export | VideoToolbox (`os`) | Microsoft's H.264 encoder MFT (`os`, a software encoder; absent on N editions without the Media Feature Pack) | — (the codec plugin) | **none** |
| H.264, proxies | VideoToolbox | a hardware MFT by adapter (AMD, NVIDIA, Intel; `gpu`), else Microsoft's MFT | VA-API (Intel low-power VDEnc, AMD; `gpu`); NVENC (6G) | **none** (SVT-AV1 proxies instead) |
| HEVC | — (only for test fixtures: VideoToolbox, §13.3) | — | — | **none**; HEVC export only through the codec plugin |
| AV1 | — | — (the hardware MFT path is cut from v1) | — | **SVT-AV1** (export everywhere; proxies without a hardware encoder) |
| VP9 | — | — | — | — (libvpx encoding cut from v1) |

**Encoder policy (owner decision 9; scope stated by follow-up decision F3 of 2026-11-07):** the "OS vendors' encoders only" rule applies to **exports and what follows the export path**. Export and QA renders (`media.render`, §9) and mattes (`media.matteEncode`, §11.3) use only `os` and `software` encoders and the codec plugin; QA renders follow export so the QA gate checks the encoder that ships, and a matte is a synced asset that can leave the machine. **Internal preview proxies (`media.proxy`) are exempt:** they are never distributed and never leave the cache of the user's installation, so they may use any hardware encoder, `os` or `gpu`; a `gpu` encoder is never chosen for a file that leaves the cache.

**Audio**

| Codec | macOS | Windows | Linux |
|---|---|---|---|
| AAC-LC / HE-AAC decode | AudioToolbox | MF AAC decoder (absent on N editions without the Media Feature Pack) | AAC-LC: symphonia (decision 1); HE-AAC: the codec plugin (symphonia refuses it) |
| AAC-LC encode | AudioToolbox | MF AAC encoder (absent on N editions) | vendored vo-aacenc (decision 1) |
| Opus | libopus (decode and encode, all OSes) | | |
| MP3, FLAC, ALAC, Vorbis, PCM | symphonia (decode, all OSes) | | |

- **Gaps get guidance, not failure** (arch §12.1): the capability report lists each missing decoder or export encoder with its fix (the HEVC Video Extensions, the Media Feature Pack on Windows N, the Store codec extensions that LTSC editions lack *(verify)*, a GPU driver, the codec plugin). An asset that cannot be decoded is still registered; its items render as "unavailable" in preview and `fin look`, and export refuses with `CODEC_UNAVAILABLE`.
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
- *As built (6B-1 Task 11):* the model's `VideoInfo` carries `display`, `rotation` and `pixelAspect` from schema 5 (§4.4): `asset.add` records them from the shallow probe, and `asset.reprobe` (`fin asset reprobe [--deep]`) records them for an asset added before. A portrait clip (1920×1080 coded, a 90° `tkhd` matrix) reads `display` 1080×1920, `rotation` 90.

### 6.4 Software codecs and container code

| Library | Use | Integration |
|---|---|---|
| dav1d (VideoLAN) | AV1 decode | own minimal FFI through a small C shim, built from source at a pinned tag (arch §27.3) by `fin-dav1d-sys` with the `cc` crate (no meson or ninja), never a prebuilt download. Replaced `rav1d` after 6A-1 (as built below) |
| SVT-AV1 | AV1 encode: proxies without a hardware encoder, every AV1 export, test fixtures | own minimal FFI, built from source at a pinned tag (arch §27.3), never a prebuilt download |
| libvpx | VP9 decode where the OS has none (Linux; 6B-2, not 6B-1: 6B-1 plan Decision 16, 2026-11-08) | own minimal FFI, built from source at a pinned tag (decoder only) |
| libopus | Opus decode/encode | own minimal FFI, built from source at a pinned tag |
| symphonia | MP3, FLAC, ALAC, Vorbis, PCM, WAV/AIFF/Ogg/CAF readers; AAC-LC decode on Linux (decision 1) | crate, unmodified (MPL-2.0, §16.1) |
| vo-aacenc | AAC-LC encode on Linux (decision 1; 6C) | vendored at a pinned commit under `third_party/vo-aacenc` (git submodule of Finisher's own maintained fork), own minimal FFI, built from source; dormant upstream since 2017, so Finisher owns its fixes; fed only PCM the mixer produced |
| `mp4-atom` | ISOBMFF box encode/decode | crate, under `fin-container`'s limits (§6.1) |
| `cros-codecs` | stateless decoding state machines and parsers (6B) | crate, with Finisher's VA-API and D3D11VA backends |
| `fin-container` | sample tables, edit lists, fragments, limits, MP4 mux, WebM demux | ours; fuzzed (§13.5) |

*As built (6A-1, then the dav1d switch):* 6A-1 shipped `rav1d` 1.1.0, whose crate package leaves out its `*.h` files, so its aarch64 assembly could not build (aarch64 ran its Rust paths, 2.7–4.2× slower than dav1d), and whose debug-only `DisjointMut` checker caught two worker threads writing one buffer region on test input (a data race in release builds on untrusted media), so it was forced to one thread: 215 fps at 540p and 22 fps at 1080p on an Apple M4. It was replaced before 6A-2 by **dav1d 1.5.4** (the submodule `third_party/dav1d`) in `fin-dav1d-sys`, a featureless crate whose `build.rs` does what dav1d's meson build does for the shipped targets with the `cc` crate alone: it writes `config.h`/`config.asm` (probing the assembler for `.func`, `.arch` and the aarch64 extensions as meson does), compiles the bit-depth templates once per depth, assembles the x86_64 assembly with nasm (already needed by SVT-AV1) and the aarch64 `.S` with the C compiler, always at `-O3` with `NDEBUG` and logging compiled out, and refuses any other dav1d version until its source lists are re-checked. So no meson, ninja or Python on any OS. aarch64 Windows (MSVC, whose `cl` cannot assemble GNU `.S`) builds dav1d's C paths only. A ~170-line C shim of Finisher's owns every dav1d struct, so Rust mirrors none of their layouts. Decoding is **multi-threaded**: `DecodeThreads {threads, frame_delay}` (dav1d 1.x runs tile, frame and post-filter work on one pool; 0/0 is a worker per logical processor and ⌈√threads⌉ frames in flight). With frames in flight, a second `dav1d_get_picture` with no packet in between drains, so `receive` asks once per packet and drains only after the new `VideoDecoder::finish` (end of stream); `FrameReader` resumes a run whose target is still in flight. Decodes with frames in flight are bit-exact with one thread (sequential, backwards, shuffled), and 50 repeated decodes four at a time agree; the same suite is clean under ThreadSanitizer (dav1d's C instrumented) and passes on Linux aarch64 (GCC) and x86_64 (nasm). Throughput on an Apple M4 (pattern clips of ~2 kB a frame, planes copied out; the machine was shared, so best observed): one thread, frame delay 1: ~1,000 fps at 540p and ~490 at 1080p, on par with dav1d 1.5.3's own CLI on the same input; all cores, frame delay 1: ~1,190 and ~670; all cores, frame delay 4: ~1,900 and ~1,280. A cold random access 30 frames into a 1080p GOP takes ~31 ms on all cores (84 ms on one). SVT-AV1 v4.3.0 is built once by `fin-svt-sys` (a featureless crate) through a small C shim of Finisher's that owns its configuration struct; `fin-codecs`, `fin-svt-sys` and `fin-dav1d-sys` are not default workspace members until 6A-2 needs them.

### 6.5 The process codec backend

The protocol a codec plugin implements (packaged by sub-project 8, pl §10.1): `codec.open {kind, stream, config}` → `{session, caps}`; packets in and frames out through `fin-shm` rings; `codec.flush`, `codec.close`; `demux.open`/`demux.read` on a read-only grant of one source file. The media worker is the client; the plugin is the job's sibling helper (§3.2 item 4). Conformance: §13.5.

**Availability (sub-project 8's owner decision 1, pl §3.1, §16.1):** third-party code plugins, the codec plugin among them, may be installed before 7C: installing one shows a high-risk warning, its permissions need a person's explicit approval, and nothing auto-updates. Until 7C lands and its profile is enforced automatically, its helper runs with the env allow-list, no credentials, brokered egress and the `in/`/`out/` rules. So the plugin paths this spec names as fixes (HE-AAC, HEVC export, Linux H.264 export, other containers) exist from 8B, not only after 7C.

### 6.6 Estimate for 6B

Revision 1 treated VA-API and D3D11VA like VideoToolbox. They are stateless, so 6B carries a decoder backend per API on top of `cros-codecs`, D3D11VA-specific DXVA parameter structures (`DXVA_PicParams_H264`, `_HEVC`, …) and reference-surface management, besides the stateful VideoToolbox and MF paths. **6B is re-estimated at about 1.6× revision 1's size** (more if the D3D11VA backend cannot reuse `cros-codecs`), and is split into **6B-1** (VideoToolbox, MF stateful decode and encode, the capability report, HDR tone mapping, proxies, the cache and HTTP serving, the Linux Landlock worker profile) and **6B-2** (VA-API and D3D11VA through `cros-codecs`, VA-API encode for proxies). Both cover all three OSes (owner decision 8); NVIDIA on Linux is 6G. 6B-2's first task is a spike that proves a `cros-codecs` backend over run-time-loaded libva, and the D3D11VA backend's feasibility, before the rest is planned.

## 7. Proxies and caching

### 7.1 Proxies

- **Tiers** (arch §12.2):
  - `540`: scrub and remote clients. All-intra where the hardware encoder supports intra-only at ≥ 8× real time; otherwise a keyframe every 0.25 s.
  - `1080`: default preview. Closed GOP with a keyframe at least every 0.5 s.
  - Originals for export.
- **Codec:** OS/hardware H.264 (any vendor class, GPU vendors' included: preview proxies are never distributed and stay inside the user's installation, §6.2) → SVT-AV1 (preset 10, CRF, closed GOP every 15 frames; arch §27.3). A remote client whose WebCodecs cannot decode the chosen codec gets the other one, generated on demand. If the desktop app ships Electron's open-codecs-only ffmpeg (owner decision 2, provisional until sub-project 9 decides the desktop shell), it has no software H.264 decoder, so its preview uses H.264 proxies only where WebCodecs reports hardware H.264 decode, else AV1 (§12).
- **Audio:** Opus 128 kb/s stereo at 48 kHz inside the proxy (for remote clients and preview), plus the PCM cache (§7.2) for native mixing. As built (proxy recipe v2, 6C; v1 was video only, §21 item 10): the source's first audio track in its canonical layout (6C plan Decision 3), so a mono source stays one channel at 64 kb/s and 3–8 channels are downmixed to stereo; the audio edit list's media time is the encoder's priming and its segment exactly the source's samples (6C plan Decision 22), so the decoded proxy audio has the source's length to the sample and starts with frame 0. A source without audio, or with audio that is not decoded (no decoder here for its codec or its container's audio, a layout or profile the decoder refuses, past the decode limits, or a corrupt track), gives a video-only proxy, and `ProxyResult.audioSkipped {code}` says why; the mix reads `pcm48`, which reports a corrupt track itself. The key's params carry what this build reads of the audio (`audio`: `none`, `unavailable`, or the decoding backend's vendor class, computed by the host from the probe and again by the worker from the file, `PLAN_MISMATCH` when they differ), so a build or plugin that gains the decoder makes a new entry rather than serving a silent one.
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
- **Eviction:** least recently used, above a cap (default the smaller of 50 GiB and 15 % of the volume, `media.cacheMaxGiB`). Entries a running job reads, or an open project's timelines use, are pinned. `fin cache status` (`media.cache.status`, `server.read`); eviction on demand is the owner's `finisherd cache gc [--to <GiB>]` (`media.cache.gc` needs `settings.admin`, which is human-only, so `fin cache gc` is refused `HUMAN_ONLY`) (6B-1 Task 13, as built).
- **Offline libraries:** proxies and PCM keep editing and preview working; export waits and lists the missing originals (`ORIGINALS_MISSING`, arch §6.7).

### 7.3 Serving the client

- `finisherd` serves cache entries over HTTP with range requests, **only through capability URLs** (6B-1 plan Decision 13, 2026-11-08; the header-authenticated `GET /media/<key>/<file>` this section first described is dropped, because 5A removed bearer tokens from the HTTP side and a header form would need a new credential on every range request). A client calls `media.url {key, file}` over its authenticated RPC connection; the server maps the key to the projects it serves (index, §7.2) and requires the principal's reach over one of them plus the `project.read` scope. Clients address media by key, never by path.
- **Capability URLs** (moved here from sub-project 5, cp §1.2), the one way to fetch media bytes (they also serve elements that cannot send headers, `<video>` and the share page later): `GET /media/cap/<token>/<file>`, where the token is 32 random bytes (base64url) held in memory, bound to `{key, file, principal, expiresAt ≤ 10 min}`, revoked with the principal, with `Host` and `Origin` checked as on the WebSocket upgrade.
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
- **First-party templates:** `social-9x16-1080p` (H.264 High, ~12 Mb/s VBR, AAC-LC 192 kb/s, −14 LUFS / −1 dBTP), `master-16x9-1080p`, `master-16x9-2160p`, `av1-web` (AV1 + Opus, the royalty-free reserve). **No built-in HEVC template** (owner decision 9): a template that names HEVC is supported only where the codec plugin is installed.
- A template asks for capabilities, never for a specific backend; the export report names the backend that was used.
- **Backend choice** follows the encoder policy of §6.2: for H.264, an `os` encoder, else the codec plugin; never a `gpu` encoder. Where neither exists (Linux without the plugin, Windows N without the Media Feature Pack), preflight fails with `EXPORT_TEMPLATE_UNSUPPORTED`, whose `fix` names the codec plugin and offers `av1-web`. AAC-LC comes from the OS encoder, or from vo-aacenc on Linux (decision 1).

### 9.3 Export report

`{file, bytes, duration, frames, rate, video: {codec, profile, backend: hw|sw, encoder}, audio: {codec, rate, loudness: {integrated, range, truePeak, before}}, avOffsetSamples (≤ 0.5), warnings, renderVersion, textEngine, planSamples}`. `fin render` prints it; the reviewer's QA gate (sub-project 10) reads it.

### 9.4 Not in v1

Chunked parallel export (GOP-aligned segments encoded on several workers and concatenated) is designed for but not built: the job format already names its range, so a later version can split it. It needs identical encoder parameter sets across sessions, which not every hardware encoder guarantees.

## 10. Colour and frame rate

### 10.1 Colour

- **Working space:** Rec.709 primaries, display-referred, BT.1886 encoded values. sRGB images, SVG, Lottie and caption colours are treated as the same encoding (the usual practice in editors and browsers). Output is tagged BT.709 / BT.709 / BT.709, limited range.
- **Untagged sources:** BT.601 below 720 lines, BT.709 otherwise; images without a profile are sRGB.
- **Wide-gamut SDR** (Display P3 images and video, BT.2020 SDR): linearised, converted to Rec.709 with a 3×3 matrix, gamut-compressed at the edge only (decision 2026-11-09 below), re-encoded.
- **HDR** (HLG, PQ, Dolby Vision profile 8.x from phones), tone-mapped to SDR at decode (arch §12.8):
  - macOS: the OS tone mapping (VideoToolbox / `VTPixelTransferSession`), which also uses Dolby Vision metadata;
  - Windows and Linux: Finisher's shader. HLG is first taken to display light at a 1,000-nit reference; then a BT.2390 EETF on PQ luminance maps it to SDR, followed by BT.2020 → Rec.709 gamut mapping (matrix plus the edge-only gamut compression). Dolby Vision profile 8 uses its HLG/HDR10 base layer and ignores the RPU. **Profile 5 is cut from v1:** it is `CODEC_UNAVAILABLE` on every OS.
  - The source's HDR metadata stays in `MediaInfo` for HDR export later.
  - Because the two algorithms differ, HDR goldens are compared within an OS only (§13.2, tier T4).
- **10-bit sources** are decoded at 10 bits (P010) and converted in the shader. The working space stays SDR.
- **LUTs** (`.cube`) apply in encoded space as 3D textures (`rgba16float`, trilinear), as `.cube` LUTs for Rec.709 expect.
- *As built (6A-1), the CPU reference:* wide-gamut SDR (BT.2020, Display P3) is converted to Rec.709 and **hard-clipped** to 0…1 (the soft clip is 6B's); the BT.601 primaries (525 and 625 lines) are treated as Rec.709 (no primaries conversion, only the BT.601 matrix); HDR transfers are not tone-mapped yet (6B). Identity-matrix (GBR) sources expand every channel as luma (limited range `(v − 16)/219`).
- *As built (6B-1 Task 10, `RENDER_VERSION` 2):* `fin_media::colour::tonemap::to_sdr` is the one definition of the HDR path (the CPU reference calls it, `shaders/tonemap.wgsl` ports it line for line; SwiftShader Vulkan within T1 at max |Δ| 1). PQ source peak: `clli` MaxCLL, else `mdcv` maximum, else 1,000 nits, within 100…10,000; HLG always 1,000 nits; target 100 nits, black 0 for both (no black lift). Values brighter than the declared peak clip at it (BT.2390's E1 is clamped to 0…1). The OOTF uses the BT.2020 luminance weights whatever the primaries. The gamut step is the compression in the decision below (it replaced a per-channel soft clip, `k = 0.8`, that also dimmed in-gamut white to 249/255 before `RENDER_VERSION` 2 was released). Rec.709 and BT.601 SDR sources are unchanged.
- **Decision (2026-11-09, owner): match professional editors.** What the standards and tools do, from their primary sources:
  - *Wide-gamut → Rec.709.* Report ITU-R BT.2407-0 (2017) §2 (BT.2087 covers only 709 → 2020) converts with the inverse BT.2087 matrix and hard-clips; §2.4: it "does not alter colours within the BT.709 gamut", but clipping "can lead to significant hue shifts and loss of spatial detail", and §1: "There is no universal gamut mapping method". The ACES 1.3 Reference Gamut Compression (docs.acescentral.com/rgc/specification) leaves a "zone of trust" unaltered and compresses distances from the achromatic `max(R, G, B)` above a per-channel threshold, the limit landing on the boundary (`p` = 1.2). DaVinci Resolve 19's Saturation Mapping (Reference Manual, colour management) likewise leaves everything below its Saturation Knee untouched; its Clip option hard-clips. BT.2446-1 Method B (§5.2.4) desaturates only out-of-gamut colours, in ICtCp. Premiere Pro and Final Cut Pro document no gamut compression (FCP's HDR Tools and Color Conform state no out-of-gamut rule; Adobe's pages were not readable for verbatim checks).
  - *HDR → SDR reference white.* BT.2408-9 (2026) §2.1, Table 1: HDR reference white is 203 cd/m², 58 % PQ, 75 % HLG, and "is not directly related to the signal level of SDR 'peak white'"; §5.2 prefers display-light down-mapping and leaves the methods to BT.2446. BT.2446-1 keeps headroom: Method B maps reference white to "approximately 90%" (86 % after tone mapping), Method C to "approximately 95% SDR signal level". No ITU text maps it to 100 %. BT.2390-8 §5.4.1 is the EETF used here.
  - *As built (`RENDER_VERSION` 2, unreleased, re-blessed):* `fin_media::colour::softclip` is an RGC-style compression on display-encoded values (signed BT.1886, `±|v|^(1/2.4)`): `a = max(R′, G′, B′)`, `d = (a − c)/a`, kept up to the threshold `t`, above it `t + s·x/(1 + x^1.2)^(1/1.2)`, `x = (d − t)/s`, `s` such that the source's reach lands on 1; `a` never changes; then a per-channel clip to 0…1. Encoded rather than linear distance because a linear band reaches deep into Rec.709 after BT.1886 (a test orange's blue moved 51 → 56). The limits are each source gamut's furthest reach (Display P3 R 1.527 93, G 1.245 41, B 1.380 35; BT.2020 1.804 84, 1.362 39, 1.408 41), the threshold `t = 1 − (limit − 1)/10`. So white, greys and every colour whose smallest encoded channel is above 2.5–8 % of its largest pass through to the byte (Display P3 white is 255, equal to Rec.709's), and a source with Rec.709 primaries (PQ or HLG included) is not compressed; only the saturated edge moves, Rec.709's own primaries sent in P3 or BT.2020 by at most 10 code values. A smooth compression cannot keep the whole Rec.709 gamut exactly (only BT.2407's clip does); this is the narrow-band compromise. Neutrals are untouched, so HDR reference white is the EETF's alone: a 1,000-nit source (HLG always; PQ with no or 1,000-nit metadata) puts 203 cd/m² at **0.949 signal (242/255), BT.2446 Method C's ≈ 95 %**; the source peak reaches 1.0; brighter masters keep more highlight range (4,000 nits: 0.875). This target is chosen over 100 % because the standards keep headroom and the BT.2390 EETF already lands there; no separate gain. Tests: `fin-media/tests/gamut.rs`, `tonemap.rs` `hdr_reference_white_lands_near_sdr_white`, `fin-compositor/tests/reference.rs` `wide_gamut_keeps_what_rec709_shows`.

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

- **A matte is an asset of kind `matte`** (pl §10.3, sub-project 8's owner decision 3): a luma-encoded greyscale video in `assets/`, write-once and synced, so export on another machine never depends on a cache. `Mask.matte` names that asset (§4.4).
- **Producing one:** a matte producer (a plugin, sub-project 8) returns alpha frames, exactly one per source frame, in order, through a `fin-shm` ring; a `media.matteEncode` job encodes them losslessly-enough (H.264 4:0:0 where an `os` encoder offers it, else SVT-AV1 at high quality, 8-bit luma; a matte is a synced asset, so the export encoder policy of §6.2 applies and no `gpu` encoder is used) at the source's rate and sample table, and registers the asset.
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
- **Proxy codec in the desktop app** (owner decision 2, provisional until sub-project 9 decides the desktop shell): if the app ships Electron's open-codecs-only ffmpeg build, H.264 proxies are used only where WebCodecs reports hardware H.264 decode *(verify that hardware H.264 decoding still works in WebCodecs with that ffmpeg build, per pinned Electron version)*; everywhere else preview proxies default to AV1 (decoded in hardware where available and by Chromium's dav1d elsewhere). If the verification fails, the desktop app uses AV1 proxies only.
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
- **Encoded at test time:** SVT-AV1 (every OS) and the OS encoders (platform tests) encode the patterns into MP4 with `fin-container`'s muxer. HEVC fixtures are encoded by VideoToolbox on macOS only (there is no built-in HEVC encoder elsewhere, §6.2); HEVC decode rows on Windows and Linux use clips from the local fixture cache. VFR, edit-list, rotation, HDR-tagged and 10-bit variants are written by tweaking the muxed boxes.
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

Budgets are CI gates where CI hardware can measure them, and validation-task gates on the reference machines otherwise (an Apple M-series Mac, a Windows desktop with an AMD RX-class GPU, a Linux Intel Gen12 machine) and on a **D31-minimum machine**: Windows 11 x64 (D31 is Windows 11 only, owner decision 9), 4 cores, 8 GB RAM, an integrated GPU of the oldest class that offers WebGPU in Chromium (an Intel UHD 620-class iGPU), or an 8 GB M1 Mac.

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
| Export 1080p30, H.264 through VideoToolbox, 60 s reel with captions and one Lottie | ≥ 4× real time | ≥ 1.5× (8 GB M1) |
| Export 1080p30, H.264 through Microsoft's (software) MFT, same reel | ≥ 2× real time *(provisional; measured in 6B-1)* | ≥ 1× *(provisional)* |
| Export 1080p30, SVT-AV1 (Linux and the royalty-free reserve), same reel | ≥ 1.5× real time *(provisional; measured in 6E)* | ≥ 0.5× *(provisional)* |
| Export 2160p30, H.264 through VideoToolbox | ≥ 1× real time | not required |
| Mixer, native, 16 stereo items, each with EQ + compressor | ≥ 100× real time | ≥ 30× |
| AudioWorklet render quantum (128 frames, 8 items with effects) | ≤ 0.5 ms | ≤ 1.5 ms |
| Lottie frame, 1080p | ≤ 5 ms (measured ≤ 2.9 ms) | ≤ 12 ms |
| Caption block raster, 2 lines at 1080p | ≤ 2 ms, cached thereafter | ≤ 5 ms |

**Worker memory limits** (§3.2 item 5, asked of 5C) scale with the machine's RAM: `interactive` the smaller of 2 GiB and 15 % of RAM; `batch` 1080p work the smaller of 4 GiB and 25 %; `media.render` at 2160p the smaller of 6 GiB and 35 %. A job that needs more than its limit allows fails `WORKER_OOM` with `details.limit` rather than swapping the machine.

## 15. Diagnostics (arch §21.1)

- **Path:** workers send diagnostics through 5C's envelope; `finisherd` forwards them to the `fin-node` journal, which stays the only writer. The `source` label is the diagnostics spec's to define (it owns the `Source` enum); sub-project 6 adds no variant.
- **Owned by 5C:** `job.done` (cp §3.4) and `worker.exit`.
- **Media fields in `job.done`** (the kind's `kv` 2, cp §3.4: an optional `media` object, all `meta`): `codec` and `backend` (names from static tables), `hw: bool`, `resolution` (bucket), `frames`, `rtFactor`, `cacheHit`, `copyPath: copy|zeroCopy`.
  - *As built (6A-2):* `job.done` is `kv` 2 with `media {codec, backend, hw, resolution, frames, rtFactorMilli, cacheHit, copyPath}`; **deviation:** `rtFactorMilli` (integer thousandths: payloads carry no floats) replaces `rtFactor`. §25.1.
- **Owned by sub-project 6:**

  | Kind | v | Payload (all `meta`) |
  |---|---|---|
  | `media.capabilities` | 1 | once per server run: per codec and profile, decode/encode `hw|sw|none` and the backend's vendor class (§6.1); backend names; GPU backend, adapter type, and (owner decision 7) the PCI vendor id, device id and driver major version as numbers; copy or zero-copy path; the worker sandbox in force (`none`, `landlock`, later `worker`, §3.5.1) |
  | `render.stats` | 1 | per-minute histograms (like `eval.stats`): decode, composite, readback, encode micros per backend |
  | `preview.stats` | 0 | reserved for sub-project 9: drops, A/V offset histogram, scrub latency |
  | `render.sample` | 0 | reserved, training only: a sampled output frame as a `frame` blob (arch §21.1: frames are always blobs; a later sub-project adds the source) |

- **Never recorded:** file names or paths (a source is `{root, ext, hash}`), adapter and driver name strings (only the numeric ids above), decoder error strings that may hold paths (they are `scrubbed` and `content` like every error message), frames or audio outside `render.sample`.
- Worker panics become `error.panic` through 5C's forwarding, as arch §21 "Crash reporting" says.

## 16. Licensing

### 16.1 Rules applied

- No GPL/LGPL/AGPL/SSPL/EUPL in Finisher's code (AGENTS.md; arch §18, reworded by owner decision 2 from "no LGPL in the app" to **"no LGPL in Finisher's code"**). FFmpeg/libav*, x264, x265, libde265 (LGPL), GStreamer (LGPL), Rubber Band (GPL) and SoundTouch (LGPL) are excluded from Finisher's code. FFmpeg is reachable only as a user-installed process plugin from `finisher-plugins-copyleft`.
- **Electron's ffmpeg: a provisional exception** (owner decision 2, made provisional by follow-up decision F4 of 2026-11-07: the desktop shell options (Electron with a documented exception plus the open-codecs build, Tauri or the OS webviews, or a custom build) are decided in sub-project 9). **Only the desktop app is affected: the server, `fin` and the media engine contain no ffmpeg.** If the shell stays Electron: Electron loads Chromium's FFmpeg as a shared library at start and does not run without it, and Finisher would ship it as a dynamically linked browser-runtime component that the user can replace and that Finisher's code never calls, with its notice in `THIRD_PARTY_NOTICES.md`, and would ship **Electron's open-codecs-only build** (`proprietary_codecs = false`) in place of the default one, so the app would carry no software H.264 or AAC decoder. The release pipeline would check the swap (file names, licence file, codec list) against the pinned Electron version *(verify those against the first pinned version)*. Counsel reviews the exception before release if it is kept (TASKS.md; sub-project 9 decides).
- **MPL-2.0** is already on the `deny.toml` allow list. It is file-level copyleft: Finisher uses MPL crates unmodified, and any patch to an MPL file is published as that file. **MPL-2.0 §3.2** applies to symphonia (and any MPL crate we ship): distributing the executable obliges Finisher to tell recipients how to get the MPL files' source and that they are under the MPL. `THIRD_PARTY_NOTICES.md` carries that statement with the crate versions and links to their source, and the notices gate (§16.3) checks it is present for every MPL package in the shipped graph.
- H.264, HEVC and AAC through OS or hardware codecs (arch §18), with the owner's decisions: AAC-LC on Linux through symphonia and vo-aacenc (decision 1); no hardware codec drivers in the Docker image (decision 3); H.264 export through OS vendors' encoders only, and no built-in HEVC encoding (decision 9). Each is on TASKS.md's "Counsel before release/upload" item.

### 16.2 Components

| Component | Licence (SPDX) | Patent terms | Use |
|---|---|---|---|
| wgpu, naga | MIT OR Apache-2.0 | — | compositor |
| dav1d 1.5.4 | BSD-2-Clause; its x86_64 assembly includes x264's `x86inc.asm` (ISC) | AOMedia Patent License 1.0 (AV1) | AV1 decode (`fin-dav1d-sys`, the submodule `third_party/dav1d`) |
| SVT-AV1 | BSD-3-Clause-Clear (since v0.9) | AOMedia Patent License 1.0 (the BSD-3-Clause-Clear text itself grants no patent rights; the grant is in `PATENTS.md`) | AV1 encode |
| libvpx | BSD-3-Clause | Google's WebM patent grant (`PATENTS`) | VP9 decode |
| libopus | BSD-3-Clause | royalty-free grants listed in its `COPYING` | Opus |
| symphonia 0.6 | MPL-2.0 | MP3 patents expired (widely reported, not re-verified); FLAC, Vorbis, ALAC royalty-free; AAC pool (Via LA) for its AAC-LC decoder, counsel checks | audio decode; AAC-LC on Linux (decision 1) |
| `mp4-atom` | MIT OR Apache-2.0 | — | ISOBMFF boxes |
| `cros-codecs` 0.0.6, `cros-libva` 0.0.13 | BSD-3-Clause | — | stateless decoding (6B-2); `cros-libva` only as a run-time-loading fork, if at all |
| libva (system library, loaded at run time) | MIT | drivers are the system's, never shipped in the app or the Docker image (decision 3); their encoders serve proxies only (decision 9) | VA-API |
| `libloading` | ISC | — | run-time loading of libva |
| vo-aacenc (decision 1) | Apache-2.0 (patent grant covers VisualOn's contributions only) | AAC pool (Via LA); counsel checks | Linux AAC-LC encode; dormant since 2017, vendored and maintained by Finisher |
| `ebur128` | MIT | — | loudness |
| thorvg (C++) | MIT | — | Lottie (native); its optional bundled loaders' licences are checked in 6D |
| dotlottie-web | MIT | — | Lottie (preview) |
| `resvg`, `usvg` ≥ 0.45 | Apache-2.0 OR MIT (MPL-2.0 up to 0.44; never use those) | — | SVG |
| `tiny-skia` | BSD-3-Clause | — | SVG raster |
| `swash` | Apache-2.0 OR MIT | — | glyph raster |
| `cosmic-text` | MIT OR Apache-2.0 | — | shaping (4C) |
| `objc2`; `objc2-av-foundation`, `objc2-video-toolbox` | MIT; Zlib OR Apache-2.0 OR MIT | OS codec, licensed by Apple (to confirm with counsel) | macOS codecs |
| `windows`, `windows-sys` | MIT OR Apache-2.0 | OS codec (to confirm with counsel) | Media Foundation (Microsoft's H.264 encoder MFT for export), D3D11VA |
| Electron's ffmpeg, open-codecs build (desktop app only) | LGPL-2.1-or-later *(verify)* | open codecs only | Chromium's media stack; the provisional exception of §16.1 (kept only if sub-project 9 keeps Electron), never called by Finisher's code |
| Mesa lavapipe (Docker image only) | MIT (Mesa) | — | software Vulkan without a GPU; installed without Mesa's VA-API or Vulkan Video codec drivers (decision 3) |
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
| `rav1d` 1.1 (BSD-2-Clause) | used in 6A-1, replaced by dav1d: its crate package leaves out the headers its aarch64 assembly needs (aarch64 ran Rust paths, 2.7–4.2× slower than dav1d), and with worker threads its debug-only `DisjointMut` checker caught two threads writing one buffer region (a data race in release builds on untrusted media), so it had to run single-threaded; a single-sponsor port whose last release is of 2025-05 |
| `dav1d` 0.11 / `dav1d-sys` 0.8 (MIT) | link a system libdav1d through `system-deps` (pkg-config); no vendored build |
| `libdav1d-sys` 0.7 (BSD-2-Clause) | builds a vendored dav1d (1.4.3, older than the current release) with meson and ninja, two more build tools on every OS |

### 16.3 Licence gates

`cargo deny` sees a crate's licence, not the C library it builds or the DLL it loads. Sub-project 6 adds three gates: the first two in `pnpm gates` from 6A's Task 1, the third in the release pipeline once the server image exists (6G at the latest):
1. **Native build gate** (`scripts/check-native-deps.mjs`). It is **keyed on `cargo metadata`**: every package in the shipped graph that has a `links` field, or a build-dependency on `cc`, `cmake`, `nasm-rs`, `bindgen` or `pkg-config`, must have an entry in `third_party/native.json` (name, version or pinned tag and commit, SPDX licence, patent-licence file, notice path, and the **BLAKE3 of each licence file**). A changed licence file hash fails the gate until a person re-reviews and updates the entry. A listed licence must be on the gate's allow list (the `deny.toml` list plus `BSD-3-Clause-Clear` and the AOMedia Patent License 1.0); `THIRD_PARTY_NOTICES.md` must name the library and version.
2. **Run-time library gate** (`third_party/runtime.json`): every library Finisher loads by name at run time is listed with who provides it (`os`, `driver`, `bundled`) and its licence: libva and its drivers (`driver`, never bundled in the desktop app or the Docker image, decision 3), `d3dcompiler_47.dll` (`os`), DXC's DLLs if ever used (`bundled`, licences to verify), Electron's `ffmpeg` library (`bundled`, the open-codecs build, the provisional exception of §16.1; listed only while the desktop shell is Electron), CoreMedia/VideoToolbox and Media Foundation (`os`). A grep over the source for `libloading::Library::new`, `dlopen` and `LoadLibrary` must find only listed names.
3. **Docker image SBOM gate:** the release pipeline generates an SPDX SBOM of the server image (a build-time tool, not shipped) and fails on any package whose licence is outside the allow list, except entries in a reviewed exceptions file that names the package, version and reason (e.g. the base image's GPL system packages, which are separate programs in the image, not linked into Finisher, if counsel agrees; arch §26 question 1). It also fails on any hardware codec driver in the image (decision 3): VA-API drivers (Intel's media driver, Mesa's VA drivers), VDPAU drivers, and any Mesa build with H.264/HEVC video codecs enabled *(verify which Mesa packages of the base image carry Vulkan Video codecs)*. NAS users with GPUs mount the host's drivers or build the documented derived image.

Vendored native sources are git submodules at pinned commits under `third_party/`.

### 16.4 Patents (for counsel; arch §26 question 1)

- AV1 (AOMedia Patent License 1.0), VP9 (Google's grant) and Opus (its contributors' grants) are royalty-free with defensive termination clauses.
- H.264 is not patent-free in 2026: community tracking lists patents in force in the US and Brazil until about 2030. HEVC has two pools (Access Advance, Via LA) and active litigation. AAC is licensed through Via LA; no primary source on its expiry was found. H.264 and HEVC reach users only through codecs their OS or GPU driver provides; AAC-LC does too, except on Linux, where Finisher ships symphonia's decoder and vo-aacenc (decision 1).
- **OS codecs vs GPU vendors' encoders:** Apple and Microsoft license the codecs they ship with their OS; whether that covers an application that drives them is for counsel. GPU vendors' encoders (NVENC, AMD's and Intel's MFTs, Linux VA-API drivers) generally come with no patent licence for the application at all. Decision 9 therefore uses only OS vendors' encoders for export (QA renders and mattes included) and lets internal preview proxies, which are never distributed, use any hardware encoder, GPU vendors' included; counsel checks both halves.
- **For counsel before release** (TASKS.md, one item across sub-projects): 6.1 (Linux AAC-LC decode and encode), 6.2 (the Electron ffmpeg exception, provisional: reviewed once sub-project 9 decides the desktop shell), 6.9 (the export and proxy encoder policy).

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

*As built (6A-2):* the `GPU_*` codes are exit 5 in `fin`, as is `SHM_REFUSED`; `EXPORT_DESTINATION` is exit 3. `PLAN_MISMATCH` carries `{at, reason: hash|noSample, want, got}` (never retried). `ORIGINALS_MISSING` is per layer, in `skipped`, with `{asset, reason: missing|notInManifest|notFound|changed|link|cloudOnly|escapes|io}`; `CODEC_UNAVAILABLE` is per layer too. `SOURCE_OUT_OF_RANGE` is not reported yet (the nearest end frame is drawn).

## 18. Crate and file map

| Crate / package | New? | Responsibility | wasm32 |
|---|---|---|---|
| `fin-media` | new | pixel formats, colour tags and matrices, `MediaInfo`, the sample table and `frame_at`, `RENDER_VERSION`, media job params and results, capability and template types; test pattern generators (`testing`) | yes, pure |
| `fin-container` | new | sample tables, edit lists, fragments, limits and codec configurations over `mp4-atom`; MP4 mux; WebM demux (6B) | yes, pure |
| `fin-shm` | new | the mapped-file channel: job-directory layout, rings, open and mapping rules (§3.4) | no |
| `fin-codecs` | new | `Decoder`/`Encoder` traits; backends `apple`, `mf`, `stateless` (`cros-codecs` with `vaapi` and `d3d11va`), `nv` (6G), `av1` (dav1d, SVT-AV1), `vpx` (decode), `opus`, `audio` (symphonia, OS AAC); the process backend client; the capability probe | no |
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

Each sub-batch gets its own plan, is green on all three OSes before the next starts (owner decision 8, also for 6B), and ends with a validation task on real hardware.

| Batch | Delivers | Depends on |
|---|---|---|
| **6A-1 Foundations** | `fin-media` (types, nearest-frame `frame_at`), frame-id patterns, the MP4 muxer, `fin-container` over `mp4-atom`, software AV1 (dav1d, SVT-AV1; `rav1d` until the dav1d switch), the CPU reference compositor, probing through `fin-container`, the native-library gates | 4C |
| **6A-2 First pixels** | the wgpu compositor core, `fin-shm`, the `media` worker family with `media.look`, job inputs in `fin-node`, `fin look`, media fields in `job.done`, `media.capabilities`, e2e and CI, validation | 6A-1, **5C-1** (the engine, warm workers, mapped files; 5C-2 only for `fin look --no-wait`) |
| **6B-1 Stateful platform codecs and proxies** | VideoToolbox, Media Foundation (stateful) decode and encode, including Microsoft's H.264 encoder MFT; backend vendor classes and the encoder policy (§6.2); the capability report (with N/LTSC guidance); the Linux Landlock worker profile (§3.5.1); HDR → SDR tone mapping; `VideoInfo` display/rotation (§4.4); proxies, thumbnails, filmstrips; the cache, its index and eviction; HTTP serving and capability URLs; WebM demux; deep probe | 6A-2 |
| **6B-2 Stateless platform codecs** | `cros-codecs` backends for VA-API (run-time libva) and D3D11VA; VA-API encode (proxies only); `media.frames` | 6B-1 |
| **6C Audio** | audio demux and decode (symphonia, including AAC-LC on Linux; OS AAC; Opus), AAC-LC encode on Linux through vendored vo-aacenc, the scalar resampler, `pcm48` and `pcm16m`, waveforms, the native mixer with pan, curves and WSOLA, the built-in audio effects and `AudioPlan.effects`, loudness and the limiter, `fin listen` | 6A-2 (6B-1 for OS AAC) |
| **6D Layers** | effects (`colorAdjust`, `lut`, `blur`), adjustment layers, nested items (with their edits), masks and `matte` assets (with their edits and `media.matteEncode`), images, SVG, Lottie and `media.templateCheck`, captions raster with the plan growth of §4.3 and caption animation, `fit` | 6A-2 |
| **6E Export** | format templates (no HEVC template), the export encoder policy in preflight, MP4 mux of encoded streams with priming edit lists, two-pass loudness, the export report and verification, `fin render`, encoder fallbacks | 6B-1, 6C, 6D |
| **6F Preview** | `fin-wasm`, `@finisher/preview` (decoder pool, scheduler, worklet mixer, dotlottie-web), server-rendered scrub frames, the harness, parity goldens and latency in Chromium and Electron | 6B-1, 6C, 6D |
| **6G Hardening** | zero-copy interop, NVDEC/NVENC on Linux (NVENC for proxies only), fuzzing at full scale, performance gates on reference and D31-minimum hardware, the full `worker` sandbox profile turned on on every OS (sub-project 7C), the Docker image SBOM gate with its codec-driver check, the cross-platform validation matrix | all, 7C |

**Order:** 6A-1 runs now. **5C-1 runs alongside 6A-1** (owner decision 5; cp §6.8: it needs only 5A Tasks 1–2); 6A-2 starts when both are in. 6C and 6D can run in parallel after 6A-2 (they touch different crates); 6B-2 can run beside 6C/6D; 6E and 6F can run in parallel after their inputs.

**Status (2026-11-08):** 6A-1 and 6A-2 are built; plan Task 16's cross-platform run is open (§25.6). 6B-1, 6C and 6D need only 6A-2 (above) and can start.

## 20. Decisions taken here

| # | Decision | Where |
|---|---|---|
| 1 | Workers evaluate plans themselves from a pinned snapshot; `fin-node` sends plan-hash samples that the worker must match | §3.3 |
| 2 | Sub-project 6 is a 5C worker family (`fin-workers media`) with its own job kinds; no supervisor or job store of its own | §3.1–3.3 |
| 3 | `fin-shm`: rings in job directories; the host never maps a file a less-trusted process can still write; `O_NOFOLLOW`, `nlink == 1`, size caps | §3.4 |
| 4 | Own sample tables, edit lists and limits on top of `mp4-atom`'s box layer | §6.1 |
| 5 | Stateless decoding (VA-API, D3D11VA) on `cros-codecs` with run-time-loaded libva | §6.1, §6.6 |
| 6 | dav1d for AV1 decode (`rav1d` measured and replaced after 6A-1); dav1d, SVT-AV1, libvpx (decode), libopus through own FFI from pinned sources | §6.4 |
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
| 17 | Every codec backend carries a vendor class (`os`, `gpu`, `software`, `plugin`); export, QA renders and mattes never use a `gpu` encoder | §6.1, §6.2, §9.2 |
| 18 | Linux media workers confine themselves with Landlock (ABI 3) from 6B-1; hosts with Landlock disabled run unconfined with a persistent status warning and a diagnostics event (ar §8.9) | §3.5.1 |
| 19 | A `media.look` frame's file name is `<timeline>-<n>_<d>-<w>x<h>-<plan hash 8>.png`, built only from the host's own values; the size is in the name because the plan hash does not cover it | §25.1 |
| 20 | Without a sandbox, grants are not enforced; with one, warm kinds run cold. A warm kind that reads untrusted input (media) relies on that rule until 7C's per-job grants; no further warm untrusted-input kind is added without it | §25.5 |

The owner's decisions are in §21; rows 17 and 18 follow from decisions 9 and 6.

## 21. Owner decisions (2026-11-07)

The owner answered the nine questions of revision 2 on 2026-11-07; every answer follows the recommendation, with question 9's Windows part decided as "drop Windows 10". Numbers are kept stable across revisions (sibling specs and TASKS.md cite them). Each entry states the decision, the facts it rests on, and where the body applies it.

1. **Linux AAC: (a).** Linux has no OS AAC codec, and AAC is the audio of nearly all phone footage and what TikTok and Instagram ask for. Finisher decodes **AAC-LC with symphonia** (MPL-2.0, unmodified) and exports **AAC-LC with a vendored vo-aacenc** that Finisher maintains (Apache-2.0, whose patent grant covers only VisualOn's own contributions; no upstream commit since 2017). symphonia refuses HE-AAC ("aac too complex") rather than decoding its core, so **HE-AAC sources need the codec plugin**. Opus-in-MP4 export was not chosen: Instagram's Reels spec lists AAC only (YouTube accepts Opus; TikTok's spec could not be read *(verify)*). **Counsel checks the patent exposure of the shipped AAC-LC decoder and encoder before release** (TASKS.md "Counsel before release/upload", 6.1). Applied in §6.1, §6.2, §6.4, §9.2, §16.
2. **Electron's bundled ffmpeg: provisional (made so by follow-up decision F4 below).** The first answer was (a), a documented exception plus the open-codecs-only build; what follows holds only if the desktop shell stays Electron, which sub-project 9 decides. Electron loads Chromium's FFmpeg as a shared library at start (`is_component_ffmpeg = true`, "for the LGPL requirement"); the default build includes H.264 and AAC decoders, and every release publishes an open-codecs-only build (`proprietary_codecs = false`). Finisher ships the open-codecs build as a replaceable browser-runtime component that its code never calls, with its notice. **Arch §18 is reworded from "no LGPL in the app" to "no LGPL in Finisher's code".** Consequence (if Electron stays): the desktop app has no software H.264 decoder, so preview uses H.264 proxies only where WebCodecs reports hardware decode, else AV1 *(verify, per pinned Electron version, the library's file names and licence file, the open build's codec list, and that WebCodecs hardware H.264 decode survives the swap)*. Counsel reviews the exception (6.2). Applied in §7.1, §12, §16.1–16.3.
3. **Hardware codec drivers in the Docker image: (a), none.** The server image ships no VA-API, VDPAU or Mesa video-codec drivers; NAS users with GPUs mount the host's drivers or build a documented derived image. The image's SBOM gate enforces it. Applied in §16.2, §16.3.
4. **Default base box: (a) contain**, as CapCut and Premiere do, with a `fit` parameter (contain/cover/none) in 6D. Reframe (sub-project 8) keyframes a larger scale to fill 9:16. The choice fixes how every existing project looks, so it is not reopened lightly. Applied in §4.3, §4.4, §5.1.
5. **5C-1 runs alongside 6A-1: yes.** 6A-1 (batches A–C of the 6A plan) needs nothing from 5C; 6A-2 needs 5C-1, which needs only 5A Tasks 1–2 (cp §6.8). The two meet at the envelope and mapped-file boundary (§3.2), checked against 5C's `test` family as soon as it exists. Applied in §19.
6. **When workers get an OS sandbox: (b).** Media workers run with 5C's cleared environment, no credentials and the job-directory write rule from 6A; on Linux a **Landlock-only** profile ships in 6B (file writes confined to `out/`, reads to the manifest's roots, GPU and driver files readable); the full `worker` profile (§3.5) is turned on in 6G when 7C lands. D31's Linux floor (kernel ≥ 6.2) makes Landlock ABI 3 available on every supported kernel, so there is no older-ABI path; hosts where Landlock is disabled run with the baseline and report `sandbox: none` (and, since follow-up decision F2 below, show a persistent status warning and record a diagnostics event). Until 7C, Linux workers have no network or exec filtering and macOS and Windows workers are unsandboxed. Applied in §3.5, §3.5.1, §15, §19.
7. **GPU identifiers in diagnostics: (a).** `media.capabilities` records the PCI vendor id, device id, GPU backend, adapter type and driver major version as numeric `meta`, never a driver's name string. Device ids narrow down a machine more than OS and core count do; GPU bugs dominate media-engine failures. Applied in §15.
8. **Platform scope of 6B: (a).** 6B-1 ships VideoToolbox and Media Foundation, 6B-2 VA-API (Intel, AMD) and D3D11VA, each merge green on all three OSes; NVDEC/NVENC on Linux (arch §12.1, unmeasured) moves to 6G. Applied in §6.6, §19.
9. **H.264/HEVC export: (a), and drop Windows 10.**
   - **(a) Encoders:** H.264 export uses **OS vendors' encoders only**: VideoToolbox on macOS, Microsoft's own H.264 encoder MFT on Windows. **Never a GPU vendor's encoder** (NVENC, AMD's or Intel's MFTs, VA-API drivers), which generally carry no patent licence for the application that drives them; those serve internal preview proxies, which are never distributed and stay inside the user's installation (scope stated by follow-up decision F3 below). Elsewhere export is **AV1** (SVT-AV1), or **H.264 through the codec plugin**. Linux therefore has no built-in H.264 export.
   - **(b) Templates:** **no built-in HEVC export template**; HEVC export only through the codec plugin.
   - **(c) Windows:** **Windows 10 is dropped; D31 becomes Windows 11 only.** Windows 10 22H2 has been out of support since 14 October 2025. The capability report states the N-edition requirement (the Media Feature Pack) and what LTSC editions lack (the Store codec extensions, HEVC and AV1) *(verify the LTSC part)*.
   - Trade-off accepted: Linux machines and Windows N machines without the Media Feature Pack export H.264 only through the plugin or fall back to AV1, which some platforms re-encode with quality loss. Counsel checks the encoder policy (6.9). Applied in §1, §6.1, §6.2, §9.2, §11.3, §13.3, §14, §16, §19.
10. **Proxies carry video only in 6B-1 (2026-11-08, 6B-1 plan Decision 4).** Spec §7.1's Opus audio inside proxies waits for 6C (libopus, AAC decode): proxy recipe v1 is video-only MP4, and 6C adds Opus audio as proxy recipe v2, which invalidates v1 entries by key. Until then preview has no proxy audio (no client exists before 6F; the mixer uses `pcm48`, also 6C). Applied in §7.1 as a note in the 6B-1 plan; §7.1's text is amended when 6C lands (done: 6C Task 20, proxy recipe v2).

**Follow-up owner decisions (2026-11-07, later the same day; numbered F1–F4 across the specs, F1 being sub-project 7's, agent §20):**
F3. **Proxy encoders (states the scope of decision 9).** The "OS vendors' encoders only" rule for H.264 and HEVC applies to **exports only**. Internal preview proxies (`media.proxy`), which are never distributed and never leave the cache of the user's installation, may use any hardware encoder, GPU-vendor ones included (NVENC, AMD's and Intel's MFTs, VA-API). QA renders (`media.render`) and mattes (`media.matteEncode`) follow the export rule. Counsel item 6.9 covers both halves, now worded this way. Applied in §6.1, §6.2, §7.1, §16.3.
F4. **Electron's bundled ffmpeg: provisional (amends decision 2).** Deferred to sub-project 9: the desktop shell options (Electron with a documented exception plus the open-codecs build, Tauri or the OS webviews, or a custom build) are decided there. Only the desktop app is affected: the server, `fin` and the media engine contain no ffmpeg. `AGENTS.md` stays unchanged; arch §18 and D13 say "provisional"; TASKS.md gains "decide the desktop shell / Electron ffmpeg" under sub-project 9, and counsel item 6.2 waits for it. Applied in §7.1, §12, §16.1–16.3.
F2. **Hosts where Landlock is disabled (amends decision 6).** On a Linux host with kernel ≥ 6.2 and Landlock turned off, everything runs, with warnings: agents, hooks, media workers and plugins, each with a persistent status warning (`server.status`, `fin status`) and a diagnostics event; nothing is refused (ar §8.9). Applied in §3.5.1, §15.

**Cross-cutting decisions of the same day that touch this spec:** D31's Linux floor is kernel ≥ 6.2 (§3.5.1); the repository targets Node 24, matching Electron's embedded Node; third-party code plugins, the codec plugin among them, may be installed before 7C with a high-risk warning and explicit approval (pl owner decision 1; §6.5 here).

## 22. Contradictions and gaps found in earlier documents

1. **Arch §18 vs Electron:** Electron's bundled `libffmpeg` is LGPL-2.1. Resolved by decision 2: arch §18 reads "no LGPL in Finisher's code", with Electron's ffmpeg as a provisional exception (§16.1) that sub-project 9 settles.
2. **Arch §12.7 vs the plan contract:** ducking in the mixer would need analysis data that `AudioPlan` does not carry (resolved by §8.5).
3. **Arch §12.3 "consumes eval output only" vs `FramePlan`/`AudioPlan`:** caption paint and placement, caption animation, matte identity and time, transition looks and audio effects are missing or undefined (resolved by §4.3); the engine also needs asset facts, which the render manifest (§4.2) provides without timeline semantics.
4. **Arch §12.8 "8-bit internally"** vs float intermediates (amended by §5.1).
5. **Arch §12.8 "frame blending or nearest-frame, chosen per item"** has no model parameter; v1 uses nearest frame (§10.2).
6. **Arch §1 "animated captions"** vs a model with no caption animation fields (§4.3).
7. **Core §5.6 and arch §27.10 say fonts are "always bundled"**; the 4C plan (Decisions 29–39) and the code fetch them on demand into a pinned, content-addressed cache and bundle none. The condition that matters (pinned bytes, never system fonts) holds either way; core §5.6 and arch §27.10 should be reworded when 4C's Task 28 records its decisions.
8. **`fin-probe` vs arch §27.3:** rotation, VFR and edit lists are not handled (§6.3).
9. **Arch §24 order:** sub-project 3 (walking skeleton with proxy preview) and sub-project 5 (jobs) are not built, yet sub-project 6 needs jobs (now from 5C, decision 5) and absorbs the skeleton's preview client (§12).
10. **Arch §12.1 lists NVDEC/NVENC for Linux** without spike evidence. Resolved by decision 8: 6G; NVENC for proxies only (decision 9).
11. **Core §5.1 transitions** name `crossfade` and `dissolve` without a visual difference (§4.3).
12. **Arch §6.2 lists mattes among regenerable analysis artifacts**; they are now `matte` assets (§11.3, pl §10.3).
13. **Arch §12.7 lists EQ and compressor as plugins;** the coordinator moved the built-in four into the mixer (§8.4).
14. **D31 lists Windows 10 22H2,** which has been out of support since 14 October 2025. Resolved by decision 9: D31 is Windows 11 only (arch D31 updated).
15. **cp §6.4** deletes a job's directory when the job ends, with no step in which the family moves its outputs; sub-project 6 asks for an `onSucceeded` hook (§3.2 item 3).
16. **cp §6.3** has no pool for hardware encoder sessions and no RAM-scaled memory limits; sub-project 6 asks for `hwenc` and the limits (§3.2 items 2 and 5).
17. **Cross-design naming:** pl revision 2 (§2, §3.5) calls the one worker binary `fin-worker` with roles `library` and `media`; cp revision 3 and the coordinator's decision call it **`fin-workers <family>`**, which this spec uses. Sub-project 8 should rename.
18. **Cross-design references:** pl cites media §8.4 for loudness; in this revision loudness is §8.6 (§8.4 is the built-in audio effects).
19. **AGENTS.md vs decision 2:** AGENTS.md says "No GPL/LGPL/AGPL/SSPL/EUPL dependencies in shipped code"; with the Electron ffmpeg exception, its wording should match arch §18's ("in Finisher's code") or name the exception. Deferred by follow-up decision F4: AGENTS.md stays unchanged until sub-project 9 decides the desktop shell.
20. **Arch §12.1 and §27.3 assume hardware H.264/HEVC encoders for export;** decision 9 limits export to OS vendors' encoders and drops built-in HEVC encoding (§6.2).

## 23. Risks

| Risk | Mitigation |
|---|---|
| GPU variance across vendors breaks T1 | the CPU oracle isolates shader bugs from tolerance issues; per-stage tolerance tests; WARP/lavapipe as CI baselines; hardware rows in each validation task |
| dav1d's meson-free build drifts from upstream on an update | `fin-dav1d-sys/build.rs` refuses any other dav1d version until its source lists and `config.h` are re-checked against `meson.build`; the native gate pins the submodule commit and licence hashes |
| `cros-codecs` is shaped for ChromeOS and its VA-API backend assumes a linked libva | 6B-2 starts with a spike; our own backends over its parsers and state machines if its backend cannot use run-time loading |
| `mp4-atom` allocates on hostile input | `fin-container`'s pre-checks and caps; fuzzing of the combination; own box layer for affected boxes as the fallback |
| 5C slips and blocks 6A-2 | 6A-1 needs nothing from 5C; the `media` family is tested against 5C's `test.*` harness as soon as it exists |
| Hardware encoders hang or pick the wrong adapter (arch §27.3) | explicit adapter binding; `ENCODER_HUNG` with a deadline and fallback to the next backend; capability probe encodes a test clip |
| HEVC on Windows without the extensions is unverified (arch §27.11) | 6B-2 validation row on a machine without them; D3D11VA path; guidance |
| Linux desktop WebGPU and real vsync are unverified (arch §27.11) | server-rendered scrub fallback; 6F validation on a real Linux desktop |
| Zero-copy interop needs wgpu hal APIs that change between versions | copy path first and always available; interop behind a feature, pinned wgpu |
| thorvg crashes on malformed templates | `media.templateCheck` in a throwaway worker; export in the render worker; 5C's retries and poison handling |
| Patent exposure (H.264/HEVC/AAC) | OS vendors' encoders only for export, GPU vendors' for proxies only, no hardware codec drivers in the Docker image (decisions 3, 9); AAC-LC on Linux (decision 1) and the Electron exception (decision 2, provisional until sub-project 9) reviewed by counsel before release (TASKS.md) |
| vo-aacenc is dormant and Finisher must maintain it | vendored fork at a pinned commit; fed only mixer PCM; a round-trip and loudness test in 6C; the codec plugin as the fallback encoder |
| Microsoft's H.264 MFT is a software encoder: slower than hardware and of unmeasured quality at the template bitrate | 6B-1 measures speed and T5 quality on the D31-minimum machine; `av1-web` and the codec plugin as alternatives |
| Landlock disabled on some Linux hosts (NAS kernels, strict container seccomp) | the baseline of §3.5.1; `sandbox: none` in the capability report with the fix, a persistent `SANDBOX_DISABLED` status warning and a diagnostics event |
| The engine grows into many crates and long files | crate map fixed here; 600-line limit; one module per stage |

## 24. Review findings (revision 2) and where they are resolved

| Finding | Resolution |
|---|---|
| **B1** own supervisor, job store, `media.jobs.*`, `fin wait` duplicate 5C | §3.1–3.3, §3.6, §18: the `media` family of 5C's `fin-workers`, media kinds and `fin-shm` only; 5C's names and codes (`fin-jobs`, `WORKER_HUNG`, `POISON_ITEM`, `HELPER_FAILED`, `jobs.*`, `JobEngine`); warm `interactive:media` workers for `fin look` (§3.2, aligned with cp revision 3); remaining asks: `hwenc`, RAM-scaled limits, `onSucceeded`; owner question 5 rewritten |
| **B2** sample-and-hold sampling | §10.2: nearest frame, ties to the earlier frame, exact rationals; §7.1 proxies and §13.2 goldens follow it |
| **H1** ownership | §3.6 table; audio effects §8.4 and `AudioItem.effects` §4.3; `pcm16m` and `media.frames` §3.3, §7.2; codec plugins as 5C sibling helpers §3.2, §6.5; `matte` assets §11.3, §4.4; `job.done`/`worker.exit` to 5C, media fields §15; `Source` to the diagnostics spec §15; sandbox requirements §3.5; `fin-shm` rules §3.4 |
| **H2** VA-API/D3D11VA are stateless | §6.1, §6.6: `cros-codecs`, libva via `libloading`; 6B re-estimated (≈1.6×) and split into 6B-1/6B-2 with a spike first |
| **H3** owner question 2 | rewritten (Electron loads ffmpeg at start; default build has H.264/AAC; recommend the exception plus the open-codecs zip; WebCodecs hardware H.264 to verify; AV1 proxies otherwise; facts marked to verify); decided as recommended in revision 3 (§21) |
| **H4** H.264/HEVC export | owner question 9 (OS vendor vs GPU vendor encoders, Windows N/LTSC, HEVC template, D31 and Windows 10's end of support); §6.2, §9.2, §16.4; decided in revision 3 (§21) |
| **H5** licence gate | §16.3: keyed on `cargo metadata` (`links`, native build-deps), licence-file hashes, a run-time library gate, the Docker SBOM gate |
| Licences: dav1d, MPL §3.2, `mp4-atom` | §16.2 (dav1d replaces `rav1d`), §16.1 (MPL §3.2 in `THIRD_PARTY_NOTICES.md`, gated), §6.1 (`mp4-atom` for boxes; tables, edit lists and limits ours; justified) |
| **M1** determinism | §13.4 (`libm` pinned exactly), §5.1 (guarded division), §8.1 (own scalar resampler), §5.1 (box mip chain, explicit level, in both compositors) |
| **M2** no clamp-to-border | §5.1 coverage edges in shader and reference |
| **M3** budgets | §14: D31-minimum column; fixed-function `normal`/`screen`/`add` with fused passes (§5.1); RAM-scaled worker limits |
| **M4** schema | §4.4: every model change and its versioning; plan additions re-bless the 4C corpus and reel goldens; `Probe.media` never in `asset.add` records |
| **M5** 6A plan | the plan: muxer before demuxer, CRF-40 check after AV1, split into 6A-1 and 6A-2, no JSON media index |
| **M6** v1 cuts | §1 "Cut from v1"; §6.2, §10.1, §12 |
| **M7** owner questions 1 and 6 | question 1 (vo-aacenc, AAC-LC only, Opus-in-MP4 acceptance); question 6 option (b) Landlock-only in 6B; both decided as recommended in revision 3 (§21, §3.5.1) |
| Small: 29.97 fps offset; VP9 on macOS | §9.1 (≤ ½ sample, no drift), §6.2 (`VTRegisterSupplementalVideoDecoderIfAvailable`) |

## 25. Sub-batch 6A as built (revision 4, 2026-11-08)

6A-1 (foundations) and 6A-2 (first pixels) are built. **As built** below means the body of this document holds as written (with the detail given); **Deviation** means the build differs from the body or from the plan, and says how. Hardware and operating systems are named by class, never by machine.

### 25.1 Decisions as built

**Containers, probing, codecs (6A-1)**

| Topic | Mark | What was built |
|---|---|---|
| Box layer (§6.1) | As built, with additions | `mp4-atom` 0.17.0 for leaf boxes (0.17.1 was under the 7-day rule at the time). Our box walker checks every size against its parent and every declared count against its box before `mp4-atom` decodes a leaf. `stz2`, `hdlr`, sample entries, `mdcv`, `clli` and `dvcC` are parsed by hand; `ctts` v0 offsets past `i32::MAX` read as negative. |
| Muxer (§6.1) | Deviation | `mp4-atom` cannot encode an unknown sample entry and has no `mdcv`, `clli`, `dvcC` or `dvvC`, so `fin-container` nests the containers and writes those boxes itself. `Mp4Writer` needs `Read + Write + Seek` (a `moov` larger than its reserve is written after moving the media data forward). |
| Limits (§6.1) | As built | `maxBoxes`, `maxBoxBytes`, `maxDepth`, `maxSamples` as in §6.1; edit lists capped at 1,024 entries (`maxEdits`); `maxPresentedFrames` is counted before any frame is stored; fragment samples are at most `min(maxSamples, file length)`; a zero-size video sample is refused; time arithmetic is checked (`MEDIA_CORRUPT`). |
| Fragmented MP4 and Opus (§6.3) | Deviation | Sub-project 2 refused fragmented MP4; `fin-probe` now reads it and reports Opus audio. |
| Probe (§6.3) | As built | `fin-probe` drops the `mp4` crate. `Probe.media: MediaInfo` is in the `asset.probe` answer only; `fin-node` refuses an `asset.add` carrying `media` (`INVALID_OP`). The model's fields keep their old meaning (sample-entry coded size, average rate, audio rate = track timescale). No media index files. |
| Sampling rule (§10.2) | As built | `frame_at` compares `2·s·timescale` with `pts(i) + pts(i+1)` in `i128` (binary search, the earlier frame wins ties). `decode_start` compares media composition times, so repeated media and open GOPs agree. |
| AV1 decode (§6.4, decision 6) | **Deviation: dav1d, not `rav1d`** | 6A-1 shipped `rav1d` 1.1.0 (aarch64 ran Rust paths 2.7–4.2× slower than dav1d because its package leaves out the headers its assembly needs; its debug checker caught two threads writing one region, so it ran on one thread). Replaced before 6A-2 by **dav1d 1.5.4** (`fin-dav1d-sys`, the `cc` crate only, a C shim of ours owns every dav1d struct, any other dav1d version fails the build until the source lists are re-checked). Multi-threaded (`DecodeThreads {threads, frame_delay}`), bit-exact with one thread, clean under ThreadSanitizer. |
| AV1 encode (§6.4) | As built | SVT-AV1 v4.3.0, built once by the featureless `fin-svt-sys` behind a C shim that owns its configuration struct. `unsafe` only in `av1/dav1d_ffi.rs` and `av1/svt_ffi.rs`. `fin-codecs`, `fin-svt-sys` and `fin-dav1d-sys` are default workspace members and built explicitly in CI (submodules, cmake, nasm). |
| Colour (§10.1) | As built | Wide-gamut SDR is converted and hard-clipped (6A; 6B-1 Task 10 replaced this with the edge-only gamut compression and HDR tone mapping, §10.1); BT.601 primaries are treated as Rec.709; identity-matrix (GBR) sources expand every channel as luma. Monochrome and 12-bit sources are `UNSUPPORTED_PIXEL_FORMAT`. |
| CPU reference (§13.2) | As built | `fin-compositor::reference`, `f32` with the pinned `libm`, an opaque accumulation (no division by alpha exists in 6A because sources are opaque), the seven blend modes by the W3C formulas, mips by `⌈w/2⌉` box averages, 33 corpus cases. The float gate covers the render modules (`libm::` required; methods and SIMD refused) and `fin-compositor` is `cargo check`ed for wasm32 in `pnpm gates`. |

**Compositor and `fin-shm` (6A-2)**

| Topic | Mark | What was built |
|---|---|---|
| wgpu (§5.1, §5.2) | As built | wgpu **30.0.1**, features `std, parking_lot, wgsl, metal, dx12, vulkan, webgpu`, no GL; `pollster` blocks on its futures; not built for WASI (the `wasm32-wasip1` T0 run keeps the reference only). `Gpu` (`AdapterChoice` from `FIN_GPU=auto|software|<backend>`; uncaptured errors and device loss are recorded, never a panic) and `Compositor`. |
| Mip levels (§5.1) | **Deviation** | Separate textures per level, sized like the reference chain; the shader mixes level `⌊L⌋` and `⌈L⌉` by the fraction (§5.1 note). |
| Blends (§5.1) | As built | `normal`, `screen`, `add` are fixed-function and fused (one pass per layer); crossfades fold their weight into alpha so they stay fused; `dissolve` and `multiply`, `overlay`, `darken`, `lighten` go through `blend.wgsl` (a `MODE` override per pipeline) and the ping-pong target. |
| T1 comparator (§13.2) | As built | PSNR over RGB and per-channel limits over RGBA. |
| `fin-shm` (§3.4) | As built, with additions | `JobDir {root}` (`input()`, `output()`, `helper_output(id)`), `checked_open(path, cap)`, `commit_output` and, from review, `commit_output_copy`, `create_output`, `create_new_output`, `write_input`; rings (`RingSpec`, `RingWriter`, `RingReader::{open_untrusted, open_sealed}`, `SlotState`); `SHM_REFUSED {file, reason, cause?}` with reason one of `link|notRegular|hardLinked|overCap|sizeMismatch|truncated|changed|header|protocol|notAbsolute|badName|missing|exists|busy|io`. A `test-hooks` feature exposes the mapping and rename hooks the tests use. Fuzzed by `ring_header`. |
| `fin-shm` use (plan) | Deviation | The plan kept `fin-shm` out of `fin-node`; `fin-node` links it (`commitJobOutput`, the input writer) and `check-crate-deps` allows exactly that (spec §18's own rule never forbade it). |
| The `media` family (§3.3) | As built | `fin-workers media [--warm]` and `fin-workers media --replay <jobdir>` (params from `in/params.json`; `--originals-anywhere` lifts the confinement of originals to the directory). Per time: `begin`, `eval_frame` with the worker's measurer, BLAKE3 of the plan's compact JSON against the host's sample (`PLAN_MISMATCH`, never retried), the GPU render, the PNG (`png`, RGBA8, fast compression), an `item`. Cancel is honoured between frames. Workers start no process (a test checks it, and on Linux that no fd or mapping under the job directory outlives a job). |
| `media.look` result and manifest (§3.3, §4.2) | Deviation | `LookResult` adds `stats {deviceReused, sourcesOpened, sourcesReused, workerJobs, media?}`; each `LookSkipped` carries its `at`. The manifest's `original` carries the stored path with size, mtime and file id (§4.2 says only the path); the opener refuses a file that changed. |
| `media.look` params (§3.3) | Deviation | Extra optional fields `projectId`, `branch`, `rev`, `outDir` (the worker ignores them); `planSamples` is optional and normally comes from `start.job.extra` (`validateJobParams` refuses non-empty `planSamples`). The dedupe key is `(project, branch, rev, timeline, at, size, outName, outDir)`: without `outName` and `outDir`, two looks of the same state to different places would coalesce. |
| Plan samples (§3.3) | Deviation | `prepare` does not pin the rev: it snapshots the head it finds and reports that rev. `LookParams.rev` is the submitter's view and part of the job's identity only. |
| Destination and names (plan Decision 7) | **Deviation (changed after Task 15)** | Frames go to `workspace/shared/look/` or `outDir` (absolute, or relative to the project folder) as `<timeline>-<n>_<d>-<w>x<h>-<plan hash, 8 hex>.png`. Until this revision the name had no size, and because the plan hash does not depend on the size, two looks of the same times at different sizes into one folder overwrote each other. The size is the named `size` (the host's own param; the worker's frame must match it) or, when none is named, the frame's own width and height, checked as a pixel count of 1 to `LOOK_MAX_SIDE`. A test renders two sizes into one folder and keeps both. |
| `media.capabilities` and `fin` (§15) | As built | A warm `media.capabilities` kind answers `MediaCapabilities {codecs, gpu, worker}`; the server caches the answer for its run (a failure is not kept); `gpu: null` when no adapter opens. `project.open` warms the `media` family. `fin look --at <t>[,<t>…] [--size WxH] [--out <dir>] [--no-wait]` and `fin media caps`. `media.look` needs `project.read` and `jobs.submit`; naming `out` also needs `project.write`; `media.capabilities` needs `server.read`. A time past the timeline is `INVALID_RANGE` (the handler checks: `eval_frame` past the end answers an empty plan). |
| Diagnostics (§15) | As built, with deviations | `job.done` is `kv` 2 with the optional `media` object `{codec, backend, hw, resolution, frames, rtFactorMilli, cacheHit, copyPath}`; `media.capabilities` v1 and `render.stats` v1 (phase `decode|composite|readback|encode`, one-minute histograms, several entries may share a window), both source `finisherd`; reserved `preview.stats` v0 and `render.sample` v0 with empty payloads. **Deviations:** `rtFactorMilli` (no floats in payloads); no per-codec `profile` (the 6A capability table has none); `render.sample` and `preview.stats` carry no fields yet. **No adapter or driver name string and no path is recorded:** the GPU is its PCI vendor id, device id and the driver's major version (`driver_major`). With `start.diagnostics` off nothing is measured into a payload. A malformed `media` object makes the journal refuse the `job.done`; the sink then records it again without `media`. |
| e2e `look` scenario and CI (§13.1) | As built, with deviations | `pnpm e2e look` (about 7 s on lavapipe): two generated clips (a CFR one and a VFR one tagged for 90° rotation), a project with a dissolve, an overlay with keyframes and `multiply`, and a caption; `fin look` at seven times including exact midpoints of the VFR clip; `skipped` holds only the caption (`LAYER_UNAVAILABLE {reason: notYet}`); plan hashes equal `plans.json`; frame ids read back from the PNGs equal the nearest-frame rule re-derived in JS; every PNG passes T1 against the CPU oracle and its T0 hashes equal `t0.json`; `--replay` writes byte-identical PNGs; a 64-frame look whose worker is killed after its first PNG is retried on a fresh worker; the journal's `job.done`, `media.capabilities` and `render.stats` entries are checked. **Deviations:** the oracle is `fin-workers`' `reference_render` example (the compositor cannot decode AV1); the job directory is copied aside by polling `<server-data>/jobs/*/in`; the worker is killed from outside (`SIGKILL` or PowerShell); `t0.json` holds one entry per fixture digest (SVT-AV1's bytes may differ per machine class), and a machine whose digest has no entry logs that it compared nothing (`E2E_LOOK_REQUIRE_T0=1` fails instead). CI installs cmake, nasm, Mesa's lavapipe and the Vulkan loader on Ubuntu, sets `FIN_GPU` (`software` on Ubuntu and Windows, `auto` on macOS) and `FIN_REQUIRE_GPU=1` on Ubuntu and Windows, and prints the encode golden's status. A nightly job fuzzes `demux` and `ring_header` for 60 s each, from **committed fuzz lockfiles** (§25.5) read with `cargo fetch --locked`. Nothing has run on GitHub yet. |
| Error codes (§17) | As built | See §17. |

### 25.2 Review fixes

**6A-1 (review on x86_64 Linux).** Edit lists capped (`maxPresentedFrames` before storing, `maxEditWork`); fragment samples capped, zero-size video samples refused; checked time arithmetic; `decode_start` on composition times; exact pixel formats (`I422`, `I422p10`, `I444p10`; monochrome and 12-bit refused); GBR range; `c_char` in the FFI (aarch64 Linux type-checks); SVT-AV1 moved to the featureless `fin-svt-sys`; full licence texts in `THIRD_PARTY_NOTICES`; the native gates catch more native builds, version drift, aliased `libloading` and loader crates; a same-machine encode golden.

**6A-2 Tasks 11–12 (the 11–12 review).**
- **B1** A warm worker is alive after its job, so `onSucceeded` commits by copy (§3.4 note). The rule this records is decision 20 (§25.5).
- **B2** A destination name is built from the host's own values: `at` from `params.at[i]` with `i` parsed from the worker's file name (which must be `<outName>-<i>.png` and equal the frame's position), the plan hash from `extra.planSamples[i]` (the frame's `plan` must be 64 lowercase hex and equal it, else `PLAN_MISMATCH`); `finalName` refuses a non-rational time or a non-digest hash; `dirname(to) === dest` is asserted.
- **B3** Workers write with `create_output` (no-follow walk, `O_EXCL`); `out/` as a link is `SHM_REFUSED link`; `--replay` confines originals to the directory.
- **M1** (owner decision) Naming `out` needs `project.write`; `lookDestination` compares the protected folders and the server's data by `(dev, ino)` against every ancestor of the resolved destination, and refuses any ancestor named `.finisher`, case-folded.
- **M2** One byte-budgeted frame cache across the open sources (1 GiB, a quarter per source, least recently used first); the decoder's pixel limit on the worker path is `LOOK_MAX_SIDE`².
- **M3** `LookParams::effective_size` applies `LOOK_MAX_SIDE` to the size actually drawn, in `prepare` and in the worker.
- **M4** The warm source key has `fileId`; a reuse reopens the path through the no-follow opener and requires the same file as the held handle.
- **L1–L5** `planSamples` refused in params; `prepare` writes `in/` through `write_input` (a missing `in/` is `SHM_REFUSED missing`); `OriginalFile` documented; `registerFamily` refuses `env` for a plugin family and any name but `FIN_GPU`; the font-cache gap of §25.5.

**Other fixes found by running 6A** FXC refused `overlay` after `MODE` substitution and the frame reported only the follow-on validation error (frames now run in an internal scope, checked first); `FIN_GPU=auto` chose Vulkan over D3D12 on Windows (D3D12 first now); a racy `fin-jobhost` domain test; `fin-shm`'s non-absolute path test built `..` with `join`, which drops it from a verbatim Windows path; a `fin-library` test that creates non-UTF-8 names now skips with a notice on APFS (`FIN_REQUIRE_RAW_NAMES=1` fails it); SVT-AV1's `LOG_QUIET` macros need `/Zc:preprocessor` under MSVC.

### 25.3 Owner decisions as applied in 6A

| Decision | Applied in 6A |
|---|---|
| 1 Linux AAC | Not in 6A (6C). |
| 2 Electron's ffmpeg, made provisional by **F4** | **As built:** the server, `fin` and the media engine contain no ffmpeg; the native and run-time library gates list what is linked. The desktop shell, and with it the exception, is sub-project 9's. |
| 3 No hardware codec drivers in the image | Not in 6A. CI installs Mesa's software Vulkan driver on the runner only. |
| 4 Default `contain` | **As built:** the base box is "contain" in `fin-core`'s plan docs and in both compositors (the anchor point lands at the frame centre plus `position`; rotation is clockwise about it). `fit` is 6D's. |
| 5 5C-1 alongside 6A-1 | **As built:** 6A-1 needed nothing from 5C; 6A-2 started on 5C-1. |
| 6 Sandbox timing | **As built:** from 6A workers run with 5C's cleared environment (plus `FIN_GPU`), no credentials and the `out/`-only write rule. Landlock is 6B-1's. |
| 7 GPU ids as numbers | **As built:** vendor id, device id, backend, adapter type and driver major version only (§25.1). |
| 8 Platform scope of 6B | Not in 6A. |
| 9 Export encoders; Windows 11 only | **F3 as built by absence:** 6A writes no H.264 or HEVC and its only encoder is SVT-AV1 (software). Windows validation is on Windows 11. |
| **F3** Proxy encoders | Not in 6A (no proxies). |
| **F4** Electron provisional | See decision 2. |

### 25.4 Measured

All compositor rows use the 33-case corpus in debug builds; "within T1" means every case passed §13.2's tolerances.

| Adapter (class) | Within T1 | Max \|Δ\| | PSNR range | Corpus, CPU reference vs GPU |
|---|---|---|---|---|
| Metal (Apple Silicon, M4) | 33 / 33 | 1 | 58.2–72.0 dB | 2.38 s vs 80 ms |
| D3D12 (AMD RX 9070 XT-class, Windows 11) | 33 / 33 | 1 | 58.2–72.0 dB | 2.35 s vs 78 ms |
| Vulkan (same GPU, amdvlk) | 33 / 33 | 1 | 58.2–72.0 dB | 2.35 s vs 66 ms |
| WARP (D3D12 software, Windows 11) | 33 / 33 | 1 | 58.2–72.0 dB | 2.35 s vs 1.63 s |
| lavapipe (Mesa 25.2, Ubuntu 24.04, 12 cores) | 33 / 33 | 1 | 58.3–71.6 dB | 2.4–2.6 s vs 216–231 ms (`mips-4k-into-1080p`: 1176 vs 37 ms) |

Off-frame pixels are exact (PSNR infinite) everywhere. The worst case is not named per adapter in these runs; the e2e frames' worst on lavapipe is 57.9 dB with max \|Δ\| 1.

- **Warm worker** (`media.look`, lavapipe, two 640×360 frames): the second job on a reused worker is faster than the first (debug 215 ms vs 265 ms; release 20 ms vs 63 ms), and its PNG bytes equal a fresh worker's.
- **dav1d decode** (pattern clips, planes copied out): Apple M4, best observed on a shared machine, one thread ~1,000 fps at 540p and ~490 at 1080p (dav1d 1.5.3's CLI on the same input: 1,101 and 463); all cores with frame delay 4 ~1,900 and ~1,280; `rav1d` on one thread 215 and 22. Windows 11 (8-core/16-thread desktop), 1080p30 8-bit: one thread 270–310 fps, all cores at delay 4 ~685, at delay 1 ~320 (ffmpeg's libdav1d: 356 and 699). A 10-bit 720p clip decodes bit-exact with ffmpeg's.
- **SVT-AV1** on Apple Silicon: 120 frames at 960×540, CRF 30, worst luma PSNR 51.9 dB; frame ids read 100 % at CRF 40 (540p and 1080p) and 60 of 60 at CRF 50 (540p).
- **Fuzzing** (Linux x64, `cargo fuzz` on stable with `RUSTC_BOOTSTRAP=1 -s none`): `ring_header` 570 s, 914 M runs; `demux` 66.6 M runs (and 8.36 M in 10 minutes during 6A-1); no finding.
- **Not measured yet** (the rows of §25.6): `fin look` latency p50/p95 over 50 runs at 1080p, warm and cold; worker spawn to `hello` p95 and warm dispatch p95; any number from a hardware Linux GPU or the D31-minimum machine; T1 on the e2e frames per adapter other than lavapipe.

### 25.5 Residuals

1. **The worker's font cache follows links.** `fin-fonts`' `FontCache` opens `<cache>/<sha256>` by path; the cache is the server's private directory and a file is kept only when its bytes hash to its name, so a swapped link can at most hide a font, not change what is drawn. Opening the cache through the no-follow opener belongs with the 6B-1 sandbox work.
2. **The warm sandbox rule (decision 20).** Warm workers serve several jobs, so a sandbox cannot grant per-job paths until 7C: with a sandbox active, warm kinds run cold; without one, grants are not enforced at all. `media.look` reads untrusted media in a warm worker and relies on that. No further warm kind that reads untrusted input may be added without it.
3. **Encode golden and T0 are Linux-only.** `fin-codecs/tests/golden/encode.json` has a macOS arm64 entry and a `linux-x86_64` entry (equal); there is none for Windows. The e2e `look` scenario's `t0.json` has the Linux entry only; a machine whose fixtures' digest has none compares nothing. The compositor's own T0 corpus (33 cases) matches on macOS arm64, x86_64 Linux and `wasm32-wasip1`; a Windows T0 result is not recorded (the Windows runs above recorded T1) and is in the §25.6 checklist.
4. **Windows UNC and 8.3 spellings are untested.** `lookDestination` is tested with UNC spellings on Linux only; 8.3 names are off on the Windows volume checked, so a short-name `outDir` has not been run. A copy commit with a live writer, and `create_output` over a link, have not run on Windows: the Windows run of `fin-shm` predates `commit_output_copy` and `create_output`.
5. **The real-GPU `driverMajor` check.** `driver_major` is the first number in wgpu's `driver_info`; it is unit-tested on sample strings and recorded on lavapipe and WARP. Its parse on a real Metal, AMD, NVIDIA or Intel `driver_info` string has to be read off hardware (a vendor that formats the string differently would record `null`, never a wrong number).
6. **No hardware Linux GPU yet.** The Linux host has llvmpipe only, so `FIN_GPU=vulkan` lands on it too; a hardware Vulkan adapter (an Intel iGPU is the planned row), and an allocation failure on a hardware GPU (a job limit does not cap VRAM; the out-of-memory test ran on WARP in a 2 GiB Windows job), are unverified.
7. **Smaller.** `SOURCE_OUT_OF_RANGE` is not reported; `--no-wait` followed by `fin wait` needs 5C-2; a rendered look has no RPC fixture (its answer names the GPU); aarch64 Windows builds dav1d's C paths only; SVT-AV1's cmake build writes its library into its submodule, so a read-only source tree cannot build it; scrubbing may want `frame_delay: 1` on seeks (the first frame after a seek waits for the frames in flight); the M4 decode numbers were taken on a shared machine and want an idle one.
8. **Nightly fuzzing resolves from committed lockfiles** (`crates/fin-container/fuzz/Cargo.lock`, `crates/fin-shm/fuzz/Cargo.lock`), so the 7-day dependency-age rule covers it: `check:crate-age` reads every `crates/*/fuzz/Cargo.lock` beside the workspace one. `cargo fuzz` has no `--locked` flag, so the job runs `cargo fetch --locked` on each fuzz crate first, which fails if a lockfile is missing or out of date.

### 25.6 Cross-platform validation (plan Task 16): checklist for the controller

Each row is run on **macOS arm64**, **Windows 11 x64** with a discrete GPU, **Linux x64** (kernel ≥ 6.2) with an Intel iGPU, the software adapters (lavapipe, WARP), and the **D31-minimum machine** (§14) where one is available. Record numbers in `archive/sp6/6a-validation.md` (by role, never by host); put durable conclusions here. 6A is done when every row passes or has an accepted, recorded exception.

- [ ] 1. `FIN_REQUIRE_SERVER=1 cargo test --workspace`, `pnpm test`, `pnpm test:gates`, `pnpm gates`, `pnpm e2e` (all scenarios, `FIN_GPU` and `FIN_REQUIRE_GPU=1` as CI sets them) green.
- [ ] 2. Compositor T0 hashes identical on macOS, Windows and Linux (no Windows T0 result is recorded) and on `wasm32-wasip1`.
- [ ] 3. T1 per adapter for the corpus **and the e2e frames**: PSNR, max \|Δ\|, the share of channels over 2/255; name the worst case (expect rotated edges or mips). Metal and the e2e frames on macOS, an Intel iGPU on Linux, the D31-minimum iGPU.
- [ ] 4. `fin look` latency, **warm** and **cold**, p50/p95 over 50 runs at 1080p from originals (budgets §14: warm ≤ 300 ms p95 on reference machines and ≤ 600 ms on the D31-minimum; cold ≤ 1.5 s and ≤ 3 s). The 6A figure is from originals, the worse case.
- [ ] 5. Worker spawn to `hello` p95 (≤ 150 ms; ≤ 300 ms D31-minimum) and warm dispatch (`start` to the first `begin`) p95.
- [ ] 6. The dav1d decision of §25.1 stays recorded (done); re-measure the decode numbers on an idle Apple Silicon machine and run the Linux x64 and aarch64 builds once more with the final sources.
- [ ] 7. Real-footage probe rows from `FIN_MEDIA_FIXTURES` with `FIN_REQUIRE_MEDIA=1`: an iPhone portrait HEVC clip reports rotation 90, HLG and Dolby Vision 8.4; a screen recording reports VFR; an edit-list export reports the trimmed start.
- [ ] 8. e2e `look` on macOS and Windows: bless or compare `t0.json` for each fixtures digest (`E2E_LOOK_REQUIRE_T0=1` once the entries exist), and add a `windows-x86_64` entry to `encode.json` from the CI log's record line.
- [ ] 9. Windows rows for `media.look`: UNC and 8.3 `outDir` spellings (an 8.3-enabled volume), a copy commit with a live writer, `create_output` over a link, through the kind and `fin look`.
- [ ] 10. `driverMajor` and the adapter ids on real hardware: Metal, the AMD GPU, an NVIDIA GPU if one is at hand, the Intel iGPU; check the recorded `media.capabilities` entry names no string.
- [ ] 11. A hardware Vulkan adapter on Linux (`FIN_GPU=vulkan` must not land on llvmpipe there); an allocation failure on a hardware GPU.
- [ ] 12. CI: a green run of `ci.yml` on all three OSes and one manual run of `nightly.yml` (nothing has gone to GitHub yet).
