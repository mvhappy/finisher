# Sub-project 6B-1: Stateful platform codecs and proxies — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (OS codec sessions and their failure modes, the encoder policy, tone mapping, the proxy frame contract, container parsing, the cache's crash rules, the sandbox), `sonnet` for plumbing (gates, kinds, RPCs, HTTP, CLI, e2e, CI, docs).

**Goal:** platform codecs on macOS and Windows, proxies and the media cache on every OS, and the first OS confinement of media workers on Linux. After this plan:
- **macOS** decodes H.264, HEVC Main/Main10, AV1 (M3 and later), VP9 and ProRes through VideoToolbox, and encodes H.264 through it for proxies and (from 6E) export; HDR sources are tone-mapped by the OS;
- **Windows 11** decodes through Media Foundation's stateful MFTs (the hardware MFT bound to the compositor's adapter), encodes H.264 for proxies with a hardware MFT and for export with Microsoft's own H.264 encoder MFT;
- **Linux** keeps 6A's software path (dav1d, SVT-AV1); VA-API is 6B-2. Its media workers confine themselves with Landlock (ABI 3), using sub-project 7C's Linux `worker` profile;
- every backend carries a **vendor class** (`os`, `gpu`, `software`, `plugin`) and one function decides which encoder a purpose may use (owner decision 9 and F3);
- `media.capabilities` reports what the machine can do, per codec and profile, with guidance for every gap (HEVC Video Extensions, the Media Feature Pack on N editions, LTSC, a GPU driver, the codec plugin, 6B-2);
- **proxies** (`540`, `1080`), thumbnails and filmstrips are made by background jobs, kept in a content-addressed cache with an index and eviction, used by `fin look`, and served to clients over HTTP through capability URLs;
- probing grows a deep probe and the model gains `VideoInfo` display size, rotation and pixel aspect (`asset.reprobe` fills old assets);
- WebM/Matroska is demuxed by `fin-container`; HDR is tone-mapped to SDR by a shader on Windows and Linux; wide-gamut SDR is soft-clipped.

**Spec:** `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` (cited as *spec §n*): §3.2 (items 2 and 5, the 5C asks), §3.5–3.5.1, §4.2, §4.4 (row 6B), §5.1–5.2, §6.1–6.3, §6.6, §7, §10.1, §13.2–13.5, §14, §15, §16, §17, §19 row 6B-1, §21 (owner decisions), §25 (6A as built, which this plan builds on). Sub-project 5 (*cp §n*): §6.2–6.4 (spawner, pools, mapped files), §7.3 (library stages). Sub-project 7 (*agent §n*): §8.3, §8.9, §8.10 (the `worker` column). Parent: arch §12.1–12.2, §12.8, §27.3.

**Builds on, as built (spec §25):** `fin-media` (types, `frame_at`, `RENDER_VERSION` 1, `jobs.rs`), `fin-container` (MP4 demux and mux over `mp4-atom`, limits), `fin-codecs` (`VideoDecoder`/`VideoEncoder`, dav1d, SVT-AV1, `FrameReader`, `capabilities()`), `fin-compositor` (wgpu 30.0.1, CPU reference, T1 comparator), `fin-shm` (`JobDir`, `checked_open`, `commit_output`, `commit_output_copy`, `create_output`, rings), the `media` family in `crates/fin-workers/src/media/` with `media.look` and `media.capabilities` (`apps/finisherd/src/jobs/kinds/media.{look,capabilities}.ts`), job inputs in `crates/fin-node/src/media/`, and `fin look`, `fin media caps`.

**Branch:** `sp6b1-platform-codecs` from the integration branch (`sp4c-edit-breadth` today, which carries 5C-1, 5C-2, 5D, 6A and 8A). Task 18 also needs 7C's branch (below); rebase onto it when its Linux `worker` profile is in.

**Dependencies:**
- **6A-2** (built): everything listed under "Builds on".
- **5C-1 and 5C-2** (built): `JobEngine`, the `hwenc` pool (defined at size 2, `apps/finisherd/src/jobs/pools.ts`, "sub-project 6 resizes it" with `Pools.define`), `MemoryRule {fixedMiB | ramFraction, capMiB}` per family priority and per kind (5C-1 Decision 20, `apps/finisherd/src/jobs/kinds.ts`), `onSucceeded`, `Spawner`, the self-sandbox entry (`fin_jobs::sandbox::register`, `FIN_SELF_SANDBOX`), `JobHostCaps.sandboxed`, `WORKER_OOM {limit}`. So the two **5C asks of spec §3.2** (a `hwenc` pool, RAM-scaled memory limits) are met by 5C's existing interfaces; Task 9 uses them and adds no 5C code.
- **5D** (built): the library stage framework (`apps/finisherd/src/library/stages.ts`, `StageRegistry.register`) for the `proxy540` stage.
- **7C (planned on `home/plans-7b-7c`, `docs/superpowers/plans/2026-11-08-sub-project-7c-os-sandbox.md`): Task 18 depends on 7C Task 2** (Linux Landlock and the `worker` profile's Landlock phase) **and 7C Task 7** (`SandboxSpawner` for the job host on Linux) (spec owner decision 6; `TASKS.md` "7C must deliver the Linux Landlock `worker` profile before 6B-1"). Tasks 1–17 and 19–20 do not wait for them; Task 18, and with it the batch's merge, does. **7C Task 12** (per-job grants for warm workers) does not gate 6B-1; it only lifts Decision 15's cold-budget rule afterwards.
- **Not needed:** 6C (audio), 6D (layers), 6E (export), 7A, 8.

**Architecture (6B-1 slice):**
- **`fin-media`:** `backend.rs` (vendor classes, the backend table, encode purposes, `encoder_allowed`), `cache.rs` (recipes, cache keys, `EntryMeta`), `colour/{tonemap,softclip}.rs`, capability types v2 and `ProxyParams`/`ThumbsParams`/`ProbeDeepParams` in `jobs.rs` (split into `jobs/{look,caps,proxy,thumbs,probe,manifest}.rs`: 427 lines today).
- **`fin-container`:** `demux/webm/` (EBML, clusters, cues, limits), `codec_config/sei.rs` (in-band HDR SEI for the deep probe).
- **`fin-codecs`:** `registry.rs` (backend choice by capability and policy), `hung.rs` (`ENCODER_HUNG` deadline and fallback), `probe/` (the capability probe per OS), `apple/` (VideoToolbox decode, encode and pixel transfer; macOS only), `mf/` (Media Foundation decode and encode; Windows only).
- **`fin-compositor`:** `shaders/tonemap.wgsl` and the ingest stage's HDR and soft-clip path; `reference/ingest.rs` uses the same `fin-media` functions; a `scale` pass for proxies and thumbnails (the box mip chain of spec §5.1, then bilinear) with NV12 conversion on the GPU.
- **`fin-workers`** (`media` family): `media/{proxy.rs, thumbs.rs, probe_deep.rs, encode_pipeline.rs}`; `media.look` opens proxies when the manifest names them; Linux self-sandbox registration in `main.rs`.
- **`fin-node`:** `media/cache/{mod,index,evict}.rs` (the index `cache/media/index.db`, written by `fin-node` only, spec §7.2), manifest `proxies` (spec §4.2), napi `MediaCache`.
- **`fin-core`:** `VideoInfo.display`, `rotation`, `pixelAspect` (optional; format bump); the op `asset.reprobe` (`ops/asset_reprobe.rs`, one module).
- **`finisherd`:** `jobs/kinds/media.{proxy,thumbs,probeDeep}.ts`, `media/{stages,cache,http,cap-tokens,proxy}.ts`, RPCs `media.proxy`, `media.proxy.status`, `media.thumbs`, `media.url`, `media.cache.status`, `media.cache.gc`, `asset.reprobe`.
- **`fin`:** `media proxy`, `media thumbs`, `cache status|gc`, `asset reprobe`; `look --originals`.
- **Not in 6B-1:** VA-API, D3D11VA and `media.frames` (6B-2); audio of any kind, AAC included, and therefore audio in proxies (6C; Decision 4); export, templates and `media.render` (6E); NVDEC/NVENC and zero-copy (6G); the full `worker` profile on macOS and Windows (6G with 7C); the Docker image (6G).

**Tech stack:**
- Rust 2024; macOS: `objc2` 0.6 and `objc2-core-foundation`, `objc2-io-surface` 0.3 (already in the tree through wgpu), plus `objc2-video-toolbox`, `objc2-core-media`, `objc2-core-video` 0.3.x (Zlib OR Apache-2.0 OR MIT; target-gated). Windows: `windows` 0.62 (in the tree; MIT OR Apache-2.0) with the `Win32_Media_MediaFoundation`, `Win32_Graphics_Direct3D11`, `Win32_Graphics_Dxgi`, `Win32_System_Com` features. Linux: 7C's `fin-sandbox` (which owns `landlock`). `rusqlite` 0.37 (bundled; already in the tree through `fin-store`) for the cache index.
- Every new crate is at least 7 days old (`check:crate-age`) and passes `cargo deny` in the task that adds it; its licence goes in the commit body.
- Node 24.22, Effect 4, vitest, as the repository.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny, the JS licence gate, the native build gate and the run-time library gate (spec §16.3). **No FFmpeg, libav\*, x264, x265, GStreamer or libde265 anywhere**, and no Electron ffmpeg in anything this plan builds (F4: the desktop app's ffmpeg is provisional and sub-project 9's).
- **Finisher ships no H.264 or HEVC implementation of its own** (D13). H.264 and HEVC are decoded and encoded only by OS or hardware codecs; on Linux they stay `CODEC_UNAVAILABLE` until 6B-2 (VA-API), with guidance.
- **Encoder policy (owner decision 9, follow-up F3; spec §6.2):** exports, QA renders and mattes use only `os` and `software` encoders and the codec plugin, never a `gpu` encoder; **internal preview proxies may use any hardware encoder**, `gpu` included, and a `gpu`-encoded file never leaves the cache. **No built-in HEVC encoding** outside test fixtures on macOS. One function (`fin_media::backend::encoder_allowed`) decides; no call site re-implements it, and a gate test lists every `open_encoder` call site.
- **No hardware codec driver is installed, bundled or required** by anything here (owner decision 3). 6B-1 changes nothing in a Docker image (none exists before 6G).
- **Windows 11 only** (owner decision 9c): no Windows 10 code path, test or guidance.
- **Dependency rules** (spec §18, `scripts/check-crate-deps.mjs`): `fin` links no media crate; `fin-node` links `fin-media`, `fin-container` and `fin-shm` (as built), never `fin-codecs`, `fin-compositor` or wgpu; only `fin-workers` links `fin-codecs` and `fin-compositor`; `fin-codecs`' platform crates are target-gated (`[target.'cfg(target_os = "macos")'.dependencies]`, `cfg(windows)`).
- **`unsafe`** only in the FFI modules (`apple/sys.rs`, `mf/sys.rs`, the existing `av1/*_ffi.rs`); each `unsafe` block carries a `// SAFETY:` comment, and the clippy config denies `undocumented_unsafe_blocks` in `fin-codecs`.
- **Workers never start processes** (spec §2.9); every new kind runs in `fin-workers media`.
- **Primitive ops live only in `fin-core`,** one module per op: `asset.reprobe` is `crates/fin-core/src/ops/asset_reprobe.rs` with its inverse and footprint.
- **TypeScript model types are generated, never hand-written:** every wire type (capability types v2, proxy, thumbs and probe params and results, `EntryMeta`, `media.url`, cache status) derives `JsonSchema`, is added to `crates/fin-store/examples/export_schema.rs`, and is regenerated with `pnpm --filter @finisher/core-types generate`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Split per stage from the start (`apple/{session,decode,encode,transfer,format}.rs`, `mf/{com,enumerate,decode,encode,d3d,sample,codecapi}.rs`, `webm/{ebml,cluster,cues,codec_private}.rs`).
- Errors are `{code, message, details, fix?}`; every new code goes into `crates/fin/src/output.rs` and `apps/finisherd/src/console/exit-codes.ts` with its exit (a test pins their parity): `ENCODER_HUNG`, `PROXY_UNAVAILABLE`, `CACHE_IO` → 5; `ASSET_CHANGED` → 3; `SANDBOX_UNAVAILABLE` takes 7C's exit.
- **No media files are committed.** H.264 and HEVC fixtures are encoded at test time by the OS encoder of the machine (spec §13.3); rows that need clips no OS here can encode (VP9, ProRes, HEVC on Windows and Linux, phone HDR) read `FIN_MEDIA_FIXTURES` and skip with a notice; `FIN_REQUIRE_MEDIA=1` fails them instead.
- **Diagnostics never record** a path, a file name, an adapter or driver name string, an MFT's friendly name or a decoder's error string; adapters are their PCI vendor id, device id and driver major version (owner decision 7).

**Determinism**
- The tone-mapping and soft-clip functions are pure `fin-media` code with the pinned `libm`, scalar, in a fixed order, used by the CPU reference; the float gate (`check-eval-floats.mjs`) covers `colour/tonemap.rs` and `colour/softclip.rs`.
- **`RENDER_VERSION` 1 → 2** in Task 10 (the soft clip changes the T0 hashes of wide-gamut cases); the T0 goldens are re-blessed in that commit with the reason. Every cache key includes `RENDER_VERSION`, so 6A's caches (none persisted) and any later change invalidate correctly.
- T3 (decoders bit-exact on the same bitstream), T4 (HDR within one OS) and T5 (encoded output decoded against the reference) are checked as spec §13.2 states; a backend that fails T3 is excluded from the backend table, not tolerated.

**Testing and process**
- **Platform tests** are compiled only on their OS (`#[cfg(target_os = "macos")]`, `#[cfg(windows)]`). An OS feature a machine lacks (AV1 decode on an M1, the HEVC Video Extensions, a hardware MFT on a CI runner) skips with a notice; `FIN_REQUIRE_PLATFORM_CODECS=1` on validation hosts makes the skip a failure. `FIN_REQUIRE_GPU` keeps its 6A meaning.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`; tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end. Platform tasks (4–7) also run their crate's tests on their OS (a controller host) before the commit; Linux CI still has to stay green (the code is target-gated).
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Shell commands are prefixed with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files; host notes stay in `archive/hosts.md`, measurements in `archive/sp6/6b1-*.md`.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 6B-1" in the same commit; Task 1 adds the section.
- **Batches** (each green on all three OSes before the next, owner decision 8):
  - **A, foundations:** Tasks 1–3.
  - **B, macOS:** Tasks 4–5 (on a macOS host; can run beside C).
  - **C, Windows:** Tasks 6–7 (on a Windows 11 host; can run beside B).
  - **D, capabilities, colour, probe, containers:** Tasks 8–12 (8 after B and C; 9–12 after A).
  - **E, cache, proxies and serving:** Tasks 13–17.
  - **F, the Linux worker profile:** Task 18 (after 7C's Linux `worker` profile).
  - **G, diagnostics, e2e, validation, docs:** Tasks 19–22.

## Decisions taken here

1. **Owner decisions applied (spec §21, 2026-11-07).** Decision 1 (Linux AAC-LC by symphonia and a vendored vo-aacenc, counsel checks the patents) is **6C's**: 6B-1 adds no audio codec, and the capability report lists Linux AAC as `none` with the guidance "arrives with the audio batch". Decision 3 (no codec drivers in Docker): nothing here bundles or requires a driver. Decision 4 (contain): proxies keep the coded orientation and size ratio, so the base box is unchanged whether a layer reads a proxy or an original. Decision 6 (Landlock in 6B): Task 18, on 7C's profile. Decision 7 (GPU ids): the capability report and the encoder adapters record numeric ids only. Decision 8 (all three OSes): each batch merges green on macOS, Windows 11 and Linux; Linux's platform codecs are 6B-2's. Decision 9 and F3 (exports through OS-vendor encoders only, proxies through any hardware encoder; no HEVC template; Windows 11 only): Decision 2 below. F4 (Electron's ffmpeg provisional, desktop only, decided in sub-project 9): 6B-1 neither ships nor assumes it; the proxy codec rule of spec §7.1 is implemented without reference to the desktop shell (Decision 5).
2. **One policy function.** `fin_media::backend::encoder_allowed(class, codec, purpose) -> Result<(), PolicyRefusal>` with `EncodePurpose {Export, QaRender, Matte, Proxy}` (and `TestFixture` behind `fin-codecs`' `testing` feature only): `Export | QaRender | Matte` allow `Os | Software | Plugin`; `Proxy` allows every class; `Hevc` is allowed only for `Plugin` and `TestFixture`. `fin-codecs::registry::open_encoder(cfg)` filters its candidates through it, and no other code opens an encoder. 6B-1 opens encoders for `Proxy` and `TestFixture` only; `Export`, `QaRender` and `Matte` are exercised by unit tests here and used by 6D/6E.
3. **Vendor classes of the platform backends.** VideoToolbox, AudioToolbox and Microsoft's own MFTs (`msmpeg2vdec`, the HEVC, AV1 and VP9 extension MFTs Microsoft publishes, `CLSID_MSH264EncoderMFT`) are `os`; an MFT whose `MFT_ENUM_HARDWARE_VENDOR_ID_Attribute` names a GPU vendor, VA-API (6B-2) and NVDEC/NVENC (6G) are `gpu`; dav1d, SVT-AV1 are `software`. A hardware MFT that does not report a vendor id is `gpu` (fail closed for export). The class is recorded per backend in `media.capabilities` and `job.done.media.backend`.
4. **Proxies carry video only in 6B-1.** **Decision (2026-11-08):** owner. Spec §7.1 puts Opus audio inside proxies, but decoding AAC and encoding Opus are 6C's (libopus, OS AAC, Linux AAC per decision 1). Proxy recipe v1 is video-only MP4; **6C adds Opus audio as proxy recipe v2** (Opus 128 kb/s stereo), which invalidates v1 entries by key. Until then preview has no proxy audio (no client exists before 6F, and the mixer uses `pcm48`, also 6C). Neither libopus encode nor OS AAC decode is pulled into 6B-1.
5. **Proxy codec choice.** Per spec §7.1: a hardware H.264 encoder (any class) where the capability probe found one that passed its test clip, else SVT-AV1 (preset 10, CRF, closed GOP every 15 frames). macOS: VideoToolbox; Windows: a hardware MFT on the compositor's adapter, else Microsoft's MFT; Linux: SVT-AV1 (VA-API encode is 6B-2). The entry's `meta.json` names the codec; `fin media proxy --codec h264|av1` makes the other one on request (the "generated on demand" path of spec §7.1). Which codec a desktop preview prefers (F4, Electron's open-codecs build) is 6F's and sub-project 9's, and needs nothing from this plan beyond `--codec`.
6. **Tier sizes.** `540` fits the coded size into 960 × 540 (or 540 × 960 for portrait coded sizes), `1080` into 1920 × 1080, keeping the coded aspect ratio, even dimensions, never upscaling (a source smaller than the tier is encoded at its own size). Rotation and pixel aspect are not baked (spec §7.1): the compositor applies them from the original's `MediaInfo`, which the entry keeps.
7. **The proxy frame contract** (spec §7.1, §10.2): a proxy holds exactly one frame per original presented frame, in presentation order, stamped at the original's nominal rate (CFR); `meta.json` holds the original's `MediaInfo` and `table.bin` its sample table (the `fin-media` binary encoding of `SampleTable`, versioned). Every reader maps a source time to an original frame index with `frame_at` on the original's table and reads proxy frame `i`. A test reads the frame-id pattern back from every proxy frame of CFR and VFR sources.
8. **GOP rules.** `540`: all-intra when the selected hardware encoder supports intra-only at ≥ 8× real time (measured by the capability probe's test clip), else a keyframe every 0.25 s; `1080`: closed GOP, keyframe at least every 0.5 s; no B-frames in either tier (frame-accurate seeks without reorder delay); SVT-AV1 proxies use spec §7.1's fixed setting.
9. **HDR in proxies and looks.** HDR sources are tone-mapped when the proxy is made (spec §7.1), on macOS by `VTPixelTransferSession` (spec §10.1), elsewhere by the shader of Task 10; the proxy is tagged BT.709 SDR and its entry records `toneMapped: os|shader`. Dolby Vision profile 8 uses its base layer; profile 5 is `CODEC_UNAVAILABLE` everywhere (spec §10.1).
10. **The cache index is `fin-node`'s** (spec §7.2): `cache/media/index.db` (SQLite through `rusqlite`, WAL, one writer: `fin-node`'s cache thread). Entries are directories `cache/media/<key[0..2]>/<key>/` with the payload and `meta.json`. A commit is rename-then-insert; at start, directories with no row are adopted when their `meta.json` validates and their key recomputes, else deleted; rows with no directory are dropped. Workers never touch the cache: a cold kind's `onSucceeded` commits from `out/` by `fin-shm::commit_output` (rename), after 5C's `checkTree`.
11. **Cache keys** (spec §7.2): BLAKE3 over the canonical JSON of `{fingerprint, recipe, recipeVersion, params, renderVersion, decoderClass}`, where `decoderClass` is the vendor class of the backend that decoded the original (T3 makes classes interchangeable for 8-bit content; the class is still in the key so a backend found non-conformant later invalidates only its entries). The key is computed in `fin-media::cache` (pure) by both `fin-node` and the worker, and the worker's must equal the host's (`PLAN_MISMATCH {reason: cacheKey}`).
12. **Eviction** (spec §7.2): least recently used above the cap, default `min(50 GiB, 15 % of the volume)` (`media.cacheMaxGiB` in `settings.json`); pinned entries are never evicted: those a running job reads (pinned for the job's life) and those of assets used by an open project's timelines. `fin cache gc` evicts down to the cap; `--to <GiB>` lower. A full volume is `CACHE_IO {freeBytes}` and fails the job without retry.
13. **Media HTTP authentication: capability URLs only.** **Decision (2026-11-08):** coordinator. Spec §7.3 describes a header-authenticated `GET /media/<key>/<file>` and capability URLs for elements that cannot send headers. 5A removed bearer tokens from the HTTP side (the WebSocket takes a one-time ticket, `apps/finisherd/src/upgrade.ts`), so a header form would need a new credential on every range request. Capability URLs minted by `media.url {key, file}` over the authenticated RPC connection are therefore **the only way to fetch media bytes**: `GET /media/cap/<token>/<file>` (token: 32 random bytes, base64url, in memory, bound to `{key, file, principal, expiresAt ≤ 10 min}`, revoked with the principal), with `Host` and `Origin` checked as on the upgrade. There is no `/media/<key>/<file>` route, and spec §7.3 is updated to say so (Task 22).
14. **Reach for media bytes** (spec §7.3): `media.url` needs `project.read` and reach over one of the projects the entry serves (the index's `entry_projects`); a key the caller cannot reach is `NOT_FOUND`, the same answer as a missing key. HTTP answers 404 for every unknown, expired or revoked token and every file not in the entry's file list.
15. **Warm kinds under Landlock run cold until 7C Task 12.** **Decision (2026-11-08):** coordinator. Spec §3.5.1 lets a warm `media` worker restrict itself once to the library roots, the caches and `<server-data>/jobs/`; the later owner decision of the 5C-1 Task 11 review (H3; `TASKS.md`, spec §25.5 decision 20) says that while a sandbox is active, warm kinds run cold until 7C grants a warm worker each job's paths. This plan follows it: with Landlock active, `media.look` and `media.capabilities` are dispatched cold on Linux, and each cold worker restricts itself to its one job (spec §3.5.1's one-job rule). Consequence: until **7C Task 12** (warm workers under a sandbox: per-job grants) lands, `fin look` on a Linux host with Landlock is held to the cold budget (≤ 1.5 s p95), not the warm one (≤ 300 ms). The owner has already decided on one warm worker per open project under a sandbox (7C Decision 19, branch `home/plans-7b-7c`, `docs/superpowers/plans/2026-11-08-sub-project-7c-os-sandbox.md`), so **once 7C Task 12 lands, warm media workers run warm under Landlock and the warm budget (≤ 300 ms p95) applies again**; 6B-1 adds no wider warm ruleset (the alternative spec §3.5.1 describes stays rejected). 7C Task 2 supplies the Landlock phase of the `worker` profile that Task 18 uses.
16. **libvpx VP9 decode moves to 6B-2.** **Decision (2026-11-08):** coordinator. Spec §6.4 lists libvpx for VP9 "where the OS has none", but the 6B-1 row does not. macOS (VideoToolbox, supplemental registration) and Windows (MF) decode VP9 in 6B-1; **on Linux VP9 is `CODEC_UNAVAILABLE`** (guidance: 6B-2) until 6B-2 adds VA-API and libvpx as Linux's software fallback in the same batch. AV1-in-WebM decodes everywhere through dav1d from Task 12.
17. **Thumbnails and filmstrips are PNG** (the one image encoder in the tree): `thumbs` at the requested times (≤ 64, longest side ≤ 512); `strip` one sprite PNG per 10 s of source at a fixed tile height (90 px default) with a `strip.json` index of tile times. Both are `item`-reporting kinds (spec §3.2 item 6).
18. **Memory rules** (spec §14, through 5C's `MemoryRule`): `media.proxy`, `media.thumbs`, `media.probeDeep` on the `batch` priority take `min(4 GiB, 25 % of RAM)`, and `min(6 GiB, 35 %)` when the source's display size passes 1920 × 1080; `media.look` keeps 6A's `{ramFraction 0.5, capMiB 6144}` (as built, spec §3.3), recorded in §26 as a deviation from §14's interactive row with 6A's reason (its warm frame cache). `hwenc` is resized to the capability report's encoder session limit (2 where unknown) once the first `media.capabilities` answer arrives.
19. **`media.capabilities` is kind v2** (params unchanged, result v2) and journal kind `media.capabilities` v2; a v1 consumer does not exist outside this repository. The probe encodes a 16-frame test clip with each hardware encoder (with `ENCODER_HUNG`'s deadline) and keeps only encoders that produced it, and counts how many concurrent sessions open (up to 4) for the session limit.
20. **`ENCODER_HUNG`** (spec §17, arch §27.3): an encoder that has accepted frames and produced no packet for `max(2 s, 4 frame durations × 30)` is abandoned (its session is released on a separate thread, never awaited), the job's `item` or encode restarts from its first frame on the next allowed candidate, and the error is recorded as a warning in the result. Two hangs in one job fail it with `ENCODER_HUNG {backend, adapter}`.

## Review focus

1. **The policy holds:** no `gpu` encoder can be opened for `Export`, `QaRender` or `Matte`; no HEVC encoder outside `TestFixture`; every `open_encoder` caller passes a purpose. → Task 2 `policy_matrix`, Task 3 `gpu_candidate_skipped_for_export`, Task 7 `hardware_mft_never_export`, the call-site gate.
2. **Platform sessions fail typed, never panic or hang:** corrupt and truncated input, device loss, an encoder that stops, cancel within 2 s, session limits. → Tasks 4–7 `corrupt_input_is_typed`, `cancel_within_2s`; Task 3 `hung_encoder_falls_back`.
3. **T3:** the same 8-bit bitstream decodes bit-exact on VideoToolbox, MF and dav1d (AV1), and on VideoToolbox and MF (H.264) through the fixture cache's recorded YUV hashes. → Tasks 4, 6, 21.
4. **The proxy frame contract:** one frame per original frame, nearest-frame lookups through the original's table, VFR and edit lists included. → Task 14 `proxy_frames_match_original_ids`, Task 16 `look_from_proxy_same_frame_ids`.
5. **The cache never serves a half-written or foreign entry:** rename-then-insert, adoption rules at start, pins, keys recomputed. → Task 13 `crash_between_rename_and_insert`, `foreign_dir_not_adopted`, `pinned_not_evicted`.
6. **HTTP serves only what the caller may reach:** token scope, expiry, revocation, file list, range bounds, no existence leak. → Task 17.
7. **Tone mapping is defined once** and the reference and the shader agree within T1. → Task 10.
8. **The worker profile confines** writes to `out/` and a temp directory and reads to the job's files, the caches and GPU files; a Landlock-disabled host warns and runs; an enabled host that fails to apply it fails the job. → Task 18.
9. **Licences and patents:** no new native library outside the gates; no H.264/HEVC code of our own; counsel items untouched. → Task 1 gates, Task 22.

---

## Batch A: foundations

### Task 1: Tracker, dependencies and gate entries — tier: sonnet

**Depends on:** nothing.

**Files:**
- Modify: `TASKS.md` (section "Sub-project 6B-1" with Tasks 1–22 and the batch order).
- Modify: root `Cargo.toml` (`[workspace.dependencies]`: `objc2-video-toolbox`, `objc2-core-media`, `objc2-core-video` at the 0.3 version the tree's `objc2-*` use; the `windows` features above), `crates/fin-codecs/Cargo.toml` (target-gated sections; feature `testing` already exists).
- Modify: `third_party/runtime.json`: VideoToolbox, CoreMedia, CoreVideo (`os`), Media Foundation (`mfplat.dll`, `os`), Microsoft's MFTs (`os`), hardware MFTs (`driver`, note "proxies only, owner decision 9/F3"); none is loaded by name by our code, so the gate's grep finds no new call.
- Modify: `scripts/check-crate-deps.mjs` (+ test): `fin-codecs`' `objc2-video-toolbox`, `objc2-core-media`, `objc2-core-video` only under `cfg(target_os = "macos")`, `windows` media features only under `cfg(windows)`; `fin-node` still never links `fin-codecs`.
- Create: `scripts/check-encoder-call-sites.mjs` (+ test), added to `scripts/gates.mjs`: `open_encoder(` and any `VTCompressionSessionCreate`/`ProcessInput` on an encoder MFT appear only in the listed files (`fin-codecs/src/{registry.rs, apple/encode.rs, mf/encode.rs, av1/svt.rs}`), and every `open_encoder` call outside `fin-codecs` passes an `EncodePurpose`.

**Tests:** the crate-deps gate on a fake graph where `fin-codecs` takes `objc2-video-toolbox` for every target → one failure; the call-site gate on a fake source with a bare `VTCompressionSessionCreate` in `fin-workers` → one failure.

- [ ] **Step 1:** Tracker section, dependencies (each ≥ 7 days old, `cargo deny` clean, licences in the commit body), gate changes and tests.
- [ ] **Step 2:** three gates → PASS. **Commit** `build(media): 6B-1 tracker, platform codec dependencies and encoder call-site gate`.

### Task 2: Vendor classes, the encoder policy and capability types v2 — tier: opus

**Depends on:** Task 1.

**Files:**
- Create: `crates/fin-media/src/backend.rs`, `crates/fin-media/tests/policy.rs`.
- Split: `crates/fin-media/src/jobs.rs` → `jobs/{mod,look,caps,manifest}.rs` (no behaviour change; the generated types keep their names), then add `jobs/caps.rs` v2 types.
- Modify: `crates/fin-store/examples/export_schema.rs`; regenerate `@finisher/core-types`.

**Interfaces:**
```rust
pub enum VendorClass { Os, Gpu, Software, Plugin }
pub enum EncodePurpose { Export, QaRender, Matte, Proxy, #[cfg(feature = "testing")] TestFixture }
pub struct BackendEntry { pub name: &'static str, pub class: VendorClass, pub decode: &'static [VideoCodec], pub encode: &'static [VideoCodec] }
pub static BACKENDS: &[BackendEntry];          // videotoolbox, mf-ms (Microsoft's MFTs), mf-hw (a GPU vendor's MFT), dav1d, svt-av1; vaapi (6B-2), nvidia (6G) reserved
pub fn backend(name: &str) -> Option<&'static BackendEntry>;
pub fn encoder_allowed(class: VendorClass, codec: VideoCodec, purpose: EncodePurpose) -> Result<(), PolicyRefusal>;
pub struct PolicyRefusal { pub reason: PolicyReason /* gpuForDistributed | hevcBuiltIn */ }
```
Capability types v2 (`jobs/caps.rs`):
```text
CodecCaps { codec, profile?, bitDepth?, decode: hw|sw|none, encode: hw|sw|none, backends: [{name, class, direction, hw}], guidance?: [Guidance] }
Guidance { code: hevcExtensions | av1Extension | mediaFeaturePack | ltscStoreCodecs | gpuDriver | codecPlugin | laterBatch, batch?: "6B-2" | "6C" | …, fix }
EncoderCaps { backend, class, codec, intraOnly, intraFps?, sessionLimit?, adapter?: {vendorId, deviceId, driverMajor?} }
MediaCapabilities v2 { codecs: [CodecCaps], encoders: [EncoderCaps], gpu, worker: {version, renderVersion, sandbox: none|landlock} }
```

**Tests:** `policy_matrix` (every class × codec × purpose against a table written out in the test); `gpu_never_for_export_qa_matte`; `hevc_encode_only_plugin_or_test_fixture`; every `BACKENDS` name is unique and in the `fin-media` name table the journal uses (`Name`, no free string); schema round trips; the `jobs.rs` split leaves the generated TypeScript unchanged except for the new types (drift gate).

- [ ] **Steps:** tests, implementation, schema export, three gates → PASS. **Commit** `feat(media): backend vendor classes, the encoder policy and capability types v2`.

### Task 3: Backend choice, encoder configuration and `ENCODER_HUNG` — tier: opus

**Depends on:** Task 2.

**Files:**
- Create: `crates/fin-codecs/src/{registry.rs, hung.rs, config.rs}`, `crates/fin-codecs/src/testing/fake.rs` (fake decoders and encoders: correct, hanging, failing, corrupt), `crates/fin-codecs/tests/registry.rs`.
- Modify: `crates/fin-codecs/src/{traits.rs, caps.rs, frame.rs, lib.rs, reader.rs}`; `crates/fin-container/src/demux/` (expose `CodecConfig::{Avc, Hevc, Av1, Vp9, ProRes}` records from `avcC`, `hvcC`, `av1C`, `vpcC` and the ProRes sample entry, as raw bytes plus parsed profile/level/bit depth).

**Interfaces:**
```rust
pub struct DecoderConfig<'a> { pub track: &'a VideoTrack, pub config: &'a CodecConfig, pub threads: DecodeThreads, pub max_pixels: u32, pub adapter: Option<AdapterId> }
pub struct EncoderConfig { pub codec: VideoCodec, pub profile: Option<Profile>, pub size: Size, pub rate: Rational,
                           pub gop: Gop /* {key_every_frames, closed, intra_only} */, pub rc: RateControl /* Crf(u8) | Vbr{bps} | Cqp(u8) */,
                           pub purpose: EncodePurpose, pub adapter: Option<AdapterId> }
pub fn open_decoder(cfg: &DecoderConfig, caps: &ProbeResult) -> Result<Box<dyn VideoDecoder>, CoreError>;   // first backend that claims the codec/profile/depth, OS before software
pub fn open_encoder(cfg: &EncoderConfig, caps: &ProbeResult) -> Result<EncoderSession, CoreError>;            // candidates filtered by encoder_allowed
pub struct EncoderSession { /* the open encoder, its BackendEntry, and the remaining candidates for fallback */ }
impl EncoderSession { pub fn send(..); pub fn receive(..); pub fn finish(..); pub fn fell_back(&self) -> Option<FallbackNote>; }
```
- `DecodedFrame` gains `PixelFormat::{Nv12, P010}` planes (already named in `fin-media::format`) and `hw: bool`; the compositor's ingest takes them (6A ingests I420/I444 from dav1d only).
- `hung.rs` implements Decision 20 on a dedicated thread per session; an abandoned session is dropped on that thread.
- `capabilities()` (6A's static table) becomes `probe::capabilities()` in Task 8; until then `ProbeResult::software_only()`.

**Tests:** `open_decoder_prefers_os_then_software` (fakes); `gpu_candidate_skipped_for_export`; `hevc_encoder_refused_outside_test_fixture`; `hung_encoder_falls_back` (a fake that stops after 3 frames: the session restarts on the next candidate, the output has every frame, `fell_back` names both); `two_hangs_fail_with_encoder_hung`; `nv12_and_p010_ingest_match_reference` (compositor T1 for NV12 and P010 inputs built from the 6A pattern).

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(codecs): backend choice under the encoder policy, encoder config and ENCODER_HUNG fallback`.

## Batch B: macOS (VideoToolbox)

### Task 4: VideoToolbox decode and pixel transfer — tier: opus (macOS host)

**Depends on:** Task 3.

**Files:**
- Create: `crates/fin-codecs/src/apple/{mod.rs, sys.rs, session.rs, decode.rs, format.rs, transfer.rs}`, `crates/fin-codecs/tests/apple_decode.rs` (`#![cfg(target_os = "macos")]`).

**Behaviour:**
- `VTDecompressionSession` from the container's configuration (`CMVideoFormatDescriptionCreateFromH264ParameterSets` / `…HEVCParameterSets`, an `av1C`/`vpcC` extension atom for AV1 and VP9, the ProRes sample description); VP9 needs `VTRegisterSupplementalVideoDecoderIfAvailable(kCMVideoCodecType_VP9)` once per process.
- Output: `NV12` for 8-bit, `P010` for 10-bit SDR, planes copied out of the `CVPixelBuffer` (the copy path; IOSurface import is 6G). Frames are delivered in presentation order through a reorder queue keyed by pts, so `receive` behaves like dav1d's.
- **HDR** (PQ, HLG, Dolby Vision 8.x): `VTPixelTransferSession` converts to 8-bit BT.709 with the OS's tone mapping (Decision 9); the frame is tagged `toneMapped: os`. Profile 5 → `CODEC_UNAVAILABLE`.
- `hw` from `kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder`; hardware is preferred, never required.
- Errors: `kVTVideoDecoderBadDataErr` and friends → `DECODE_FAILED {backend: videotoolbox, frame}` (the OS status as a number in `details`, never a string); malformed parameter sets → `MEDIA_CORRUPT`; session invalidation (GPU change, sleep) → the session is rebuilt once, then `DECODE_FAILED`.
- `flush` (`VTDecompressionSessionFinishDelayedFrames` + reset) for seeks; cancel is honoured between packets.

**Tests** (H.264 and HEVC fixtures come from Task 5's encoder; until it lands they use `FIN_MEDIA_FIXTURES` and skip):
- `av1_bit_exact_with_dav1d` (M3 or later; skips with a notice on M1/M2): SVT-AV1 fixtures, every frame's planes equal dav1d's (T3);
- `frame_ids_read_back` and `seek_to_any_frame` for H.264, HEVC Main and Main10, AV1;
- `hdr_is_tone_mapped_and_tagged` (an HLG- and a PQ-tagged fixture written by box tweaks, spec §13.3; the frame is SDR-tagged with `toneMapped: os`);
- `vp9_and_prores_from_fixture_cache` (skip without `FIN_MEDIA_FIXTURES`);
- `corrupt_input_is_typed` (truncated and bit-flipped samples), `cancel_within_2s`, `dv_profile_5_unavailable`.

- [ ] **Steps:** tests, implementation (on a macOS host), `cargo test -p fin-codecs` there and the three gates on Linux → PASS. **Commit** `feat(codecs): VideoToolbox decode with OS tone mapping`.

### Task 5: VideoToolbox encode (H.264 proxies; HEVC test fixtures) — tier: opus (macOS host)

**Depends on:** Task 4.

**Files:**
- Create: `crates/fin-codecs/src/apple/encode.rs`, `crates/fin-codecs/tests/apple_encode.rs`.
- Modify: `crates/fin-codecs/src/fixture.rs` and `examples/make_fixture.rs` (`--codec h264|hevc` on macOS, through `EncodePurpose::TestFixture`).

**Behaviour:** `VTCompressionSession` for H.264 (High, 8-bit) with `kVTCompressionPropertyKey_AllowFrameReordering = false`, `MaxKeyFrameInterval`/`…Duration` from `Gop`, real-time off, `AverageBitRate` or `Quality` from `RateControl`; intra-only by a key frame interval of 1 (Decision 8); `avcC` from the format description; class `os`. HEVC (Main, Main10) only with `TestFixture`. `ENCODER_HUNG` through Task 3's session.

**Tests:** `h264_round_trip_t5` (encode the pattern, decode with Task 4, PSNR ≥ 38 dB and SSIM ≥ 0.97 at 1080p and 8 Mb/s, frame count and ids exact); `gop_is_closed_and_bounded` (keyframe spacing read from `stss`); `intra_only_when_asked`; `hevc_fixture_only_with_testing_feature` (the policy refuses `Proxy` for HEVC); `encoder_records_class_os`.

- [ ] **Steps:** tests, implementation, macOS tests and Linux gates → PASS. **Commit** `feat(codecs): VideoToolbox H.264 encode for proxies; HEVC for test fixtures only`.

## Batch C: Windows 11 (Media Foundation)

### Task 6: Media Foundation decode, bound to the compositor's adapter — tier: opus (Windows 11 host)

**Depends on:** Task 3.

**Files:**
- Create: `crates/fin-codecs/src/mf/{mod.rs, sys.rs, com.rs, enumerate.rs, decode.rs, d3d.rs, sample.rs}`, `crates/fin-codecs/tests/mf_decode.rs` (`#![cfg(windows)]`).
- Modify: `crates/fin-compositor/src/device.rs` (expose the adapter's LUID and PCI ids as `AdapterId`, no strings).

**Behaviour:**
- COM initialised per decoding thread (MTA), `MFStartup` once per process.
- `MFTEnumEx(MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER, …)`, filtered to the compositor's adapter with `MFT_ENUM_ADAPTER_LUID` (arch §27.3), falling back to Microsoft's software decoder; hardware MFTs use an `IMFDXGIDeviceManager` on a D3D11 device created on the same adapter; async MFTs through their event generator, sync ones through `ProcessInput`/`ProcessOutput`.
- Output: NV12/P010, copied from the D3D11 texture through a staging texture (the copy path; shared handles are 6G).
- Codecs: H.264 (built in), HEVC (only with the HEVC Video Extensions: absent → `CODEC_UNAVAILABLE` with `hevcExtensions` guidance; D3D11VA is 6B-2), AV1 (the AV1 extension), VP9 (built in).
- Windows N without the Media Feature Pack: `MFStartup` or the decoder enumeration fails → `CODEC_UNAVAILABLE` with `mediaFeaturePack` guidance, never a crash; AV1 still decodes through dav1d.
- Errors and device loss typed as in Task 4 (`MF_E_*` HRESULTs as numbers in `details`).

**Tests:** `av1_bit_exact_with_dav1d` (AV1 extension present; else skip with notice); `frame_ids_read_back`, `seek_to_any_frame` for H.264 (fixtures from Task 7's Microsoft encoder) and AV1; `decoder_device_on_compositor_adapter` (the LUID of the decoder's device equals the compositor's); `hevc_without_extensions_has_guidance` (on a host without them; skip otherwise); `corrupt_input_is_typed`, `cancel_within_2s`; `h264_yuv_hash_recorded` (writes the per-fixture YUV hash the cross-OS T3 row compares, Task 21).

- [ ] **Steps:** tests, implementation, Windows tests and Linux gates → PASS. **Commit** `feat(codecs): Media Foundation decode bound to the compositor's adapter`.

### Task 7: Media Foundation encode (hardware MFT for proxies, Microsoft's MFT for export) — tier: opus (Windows 11 host)

**Depends on:** Task 6.

**Files:**
- Create: `crates/fin-codecs/src/mf/{encode.rs, codecapi.rs}`, `crates/fin-codecs/tests/mf_encode.rs`.
- Modify: `crates/fin-codecs/src/fixture.rs` and `make_fixture` (`--codec h264` on Windows, through Microsoft's MFT).

**Behaviour:**
- Candidates for H.264: hardware encoder MFTs on the compositor's adapter (class `gpu` per Decision 3, or `os` if Microsoft's own hardware MFT), then `CLSID_MSH264EncoderMFT` (class `os`, software; absent on N editions without the Media Feature Pack). `Proxy` may take either; `Export`, `QaRender` and `Matte` take only Microsoft's (Task 3's filter).
- `ICodecAPI`: `CODECAPI_AVEncMPVGOPSize`, `CODECAPI_AVEncMPVDefaultBPictureCount = 0`, `CODECAPI_AVEncCommonRateControlMode` (`Quality`/`UnconstrainedVBR`/`CBR`), `CODECAPI_AVEncCommonMeanBitRate`, `CODECAPI_AVEncVideoForceKeyFrame` for closed GOPs; `avcC` built from the output media type's sequence header.
- Hardware MFTs that hang (arch §27.3) are caught by Task 3's deadline and fall back to Microsoft's MFT for proxies.

**Tests:** `ms_mft_round_trip_t5` (decode with Task 6; PSNR/SSIM as Task 5); `hardware_mft_never_export` (a hardware MFT listed first is skipped for `Export` and chosen for `Proxy`); `gop_is_closed_and_bounded`; `n_edition_guidance` (simulated by a test hook that hides the MFT); `hardware_mft_hang_falls_back` (test hook injecting a stalled `ProcessOutput`). Speed of Microsoft's MFT at 1080p30 (spec §14's provisional rows) is measured in Task 21, not gated here.

- [ ] **Steps:** tests, implementation, Windows tests and Linux gates → PASS. **Commit** `feat(codecs): Media Foundation H.264 encode: hardware MFTs for proxies, Microsoft's MFT for export`.

## Batch D: capabilities, colour, probe, containers

### Task 8: The capability probe and report — tier: opus

**Depends on:** Tasks 2–7.

**Files:**
- Create: `crates/fin-codecs/src/probe/{mod.rs, apple.rs, mf.rs, linux.rs, guidance.rs}`, `crates/fin-codecs/tests/probe.rs`.
- Modify: `crates/fin-workers/src/media/caps.rs` (kind `media.capabilities` v2), `apps/finisherd/src/jobs/kinds/media.capabilities.ts` (version 2), `apps/finisherd/src/media/caps.ts` (cache per server run as today), `crates/fin/src/commands/media_caps.rs` (prints the guidance lines).

**Behaviour:**
- Decode support per codec, profile and bit depth by opening a session on a one-frame fixture made in memory (SVT-AV1 for AV1; the OS encoder's for H.264/HEVC where one exists; else by querying the OS: `VTIsHardwareDecodeSupported`, `MFTEnumEx` results).
- Encoders: each candidate encodes a 16-frame 540p test clip under Decision 20's deadline; one that fails or hangs is left out and noted; intra-only speed measured (Decision 8); concurrent sessions counted up to 4 (`sessionLimit`).
- Guidance (`probe/guidance.rs`, one table): HEVC Video Extensions (Windows), the AV1 extension (Windows), the Media Feature Pack (Windows N), the Store codec extensions LTSC lacks *(verify the LTSC list on an LTSC host in Task 21; until verified the text says "may lack")*, a GPU driver (no hardware decoder where the adapter has one), the codec plugin (HEVC export everywhere, H.264 export on Linux; spec §6.5), `laterBatch` (Linux H.264/HEVC/VP9 decode: 6B-2; AAC everywhere: 6C).
- Adapters: vendor id, device id, driver major (decision 7) of the GPU each hardware backend is bound to.
- `worker.sandbox`: `landlock` when the worker applied Task 18's profile, else `none`.

**Tests:** `report_lists_every_codec_row`; `guidance_is_a_closed_table` (every `Guidance.code` has a `fix`); `failed_encoder_left_out` (fake); `session_limit_counted` (fake with a limit of 2); on each OS host: the report matches the machine (validated by hand in Task 21, recorded in `archive/`).

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(media): capability probe with encoder test clips, session limits and guidance`.

### Task 9: The 5C asks: `hwenc` sizing and RAM-scaled limits — tier: sonnet

**Depends on:** Task 8.

**Files:** Modify `apps/finisherd/src/media/caps.ts` (after the first answer: `pools.define("hwenc", sessionLimit ?? 2)`), `apps/finisherd/src/jobs/kinds/media.look.ts` (keep `MEDIA_MEMORY`; document Decision 18), new kinds' `memory` (Tasks 11, 14, 15 use `mediaBatchMemory(params)` from `apps/finisherd/src/media/memory.ts`). Test: `apps/finisherd/test/media-pools.test.ts`.

**Tests:** `hwenc_resized_from_caps` (a fake report with limit 3 → `pools.size("hwenc") === 3`; a failed report leaves 2); `batch_memory_rule_scales` (1080p source → `{ramFraction 0.25, capMiB 4096}`, 4K source → `{0.35, 6144}`); a job over its limit fails `WORKER_OOM {limit}` (5C's existing path, exercised with the `test` family's allocation kind).

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(media): size hwenc from the capability report; RAM-scaled limits for batch media kinds`.

### Task 10: HDR tone mapping and the wide-gamut soft clip — tier: opus

**Depends on:** Task 3.

**Files:**
- Create: `crates/fin-media/src/colour/{tonemap.rs, softclip.rs}`, `crates/fin-compositor/src/shaders/tonemap.wgsl`, `crates/fin-media/tests/tonemap.rs`.
- Modify: `crates/fin-compositor/src/{ingest.rs, reference/ingest.rs}`, `crates/fin-media/src/version.rs` (`RENDER_VERSION = 2`), `scripts/check-eval-floats.mjs` (cover the two new modules), the T0 goldens (`crates/fin-compositor/tests/…/t0.json`), `scripts/e2e/look-golden/t0.json` (re-blessed for the fixtures digest; Linux entry).

**Behaviour** (spec §10.1, Windows and Linux; macOS frames arrive tone-mapped by the OS, Task 4):
- PQ: ST 2084 EOTF to absolute luminance; HLG: inverse OETF and the OOTF at a 1,000-nit nominal peak (BT.2100) to display light; then the **BT.2390 EETF** on PQ-encoded luminance (source peak from `mdcv`/`clli` or 1,000 nits, target 100 nits SDR, knee per BT.2390), applied to `max(R,G,B)` with the ratio kept (no hue shift); then BT.2020 → BT.709 by the 3×3 matrix and the **soft clip** (`softclip.rs`: a smooth roll-off above 0.8 per channel toward 1.0, defined as a closed-form function, the same one wide-gamut SDR uses in place of 6A's hard clip); then the BT.1886 encode.
- Dolby Vision 8.x: the base layer (HLG or HDR10) as above; the RPU is ignored.
- One pure function `fin_media::colour::tonemap::to_sdr(rgb_linear, params) -> rgb` used by the CPU reference; the WGSL is a line-for-line port, and T1 holds between them.

**Tests:** `eetf_matches_bt2390_table` (hand-computed points); `hlg_ootf_at_1000_nits`; `softclip_continuous_and_monotonic`; `wide_gamut_softclip_t0` (new goldens, `RENDER_VERSION` 2); `gpu_tonemap_within_t1` (lavapipe, WARP, Metal); `t0_change_without_bump_fails` still holds (it failed before the bump in this commit).

- [ ] **Steps:** tests, implementation, re-bless with the reason in the commit body, three gates and `rtk pnpm e2e look` → PASS. **Commit** `feat(compositor): BT.2390 HDR tone mapping and wide-gamut soft clip (RENDER_VERSION 2)`.

### Task 11: `VideoInfo` display, rotation and pixel aspect; `asset.reprobe`; the deep probe — tier: opus

**Depends on:** Task 3 (the deep probe opens decoders).

**Files:**
- Modify: `crates/fin-core/src/model/asset.rs` (`VideoInfo.display: Option<Size>`, `rotation: Option<Rotation>`, `pixelAspect: Option<Rational>`), the format version per core §3.2 (`formatVersion` bump; `minReaderVersion` unchanged; absent = unknown, no upcaster), `crates/fin-core/src/ops/{mod.rs, asset_reprobe.rs}` (new op: sets the three fields and the existing `video`/`audio` from a fresh probe; inverse restores the previous values; footprint = the asset), `crates/fin-node/src/…` (`asset.add` fills the new fields from `fin-probe`), `crates/fin-container/src/codec_config/sei.rs` (H.264/HEVC SEI: mastering display colour volume, content light level), `crates/fin-media/src/jobs/probe.rs` (`ProbeDeepParams {asset, fingerprint}`, `ProbeDeepResult {decodable, backend, class, hw, hdr?: HdrInfo, firstFrame: {width, height, format}}`), `crates/fin-workers/src/media/probe_deep.rs`, `apps/finisherd/src/jobs/kinds/media.probeDeep.ts` (normal, `cpu`, cold, `dedupe` = fingerprint + `RENDER_VERSION` + caps digest), `apps/finisherd/src/handlers.ts` (`asset.reprobe {projectId, asset, deep?}`: probes, optionally runs `media.probeDeep`, commits the op), `crates/fin/src/commands/asset.rs` (`fin asset reprobe <asset> [--deep]`), the 4B property generators (the new op).
- Tests: `crates/fin-core/tests/ops_asset_reprobe.rs`, upgrade goldens for the format bump, `apps/finisherd/test/media-probe-deep.test.ts`.

**Tests:** `reprobe_fills_rotation_of_portrait_clip` (a generated clip with a 90° `tkhd` matrix: before `display` absent, after `display` 1080×1920, `rotation` 90); `reprobe_inverse_restores`; `older_reader_refuses_reprobe_op`; `probe_media_never_in_asset_add` still passes (spec §4.4); `sei_hdr_read_when_no_boxes` (a fixture with SEI and no `mdcv`); `probe_deep_reports_backend_and_class`; the eval corpus hashes are unchanged (no plan field changes).

- [ ] **Steps:** tests, implementation, generated types, three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(core): VideoInfo display, rotation and pixel aspect; asset.reprobe; media.probeDeep`.

### Task 12: WebM/Matroska demux — tier: opus

**Depends on:** Task 3.

**Files:**
- Create: `crates/fin-container/src/demux/webm/{mod.rs, ebml.rs, segment.rs, cluster.rs, cues.rs, codec_private.rs}`, `crates/fin-container/src/testing/webm_writer.rs` (test-only writer), `crates/fin-container/tests/webm.rs`, `crates/fin-container/fuzz/fuzz_targets/webm.rs` (and its committed `Cargo.lock` entry, spec §25.5 item 8).
- Modify: `crates/fin-container/src/{lib.rs, limits.rs}` (WebM limits: element depth 16, element size against the bytes left, `maxSamples` and `maxPresentedFrames` as MP4), `crates/fin-probe/src/…` (dispatch by EBML magic), `.github/workflows/nightly.yml` (fuzz `webm` 60 s).

**Behaviour:** EBML variable-length ids and sizes (unknown sizes only for `Segment` and `Cluster`), `TimecodeScale`, `SimpleBlock` and `BlockGroup` (no lacing for video; Xiph, EBML and fixed lacing for audio), keyframe flags, `Cues` when present else a cluster scan, `CodecPrivate` for AV1 (`av1C`), VP9 (feature records) and Opus (`OpusHead`); `ContentEncoding` (compression, encryption) → `UNSUPPORTED_CONTAINER {reason: contentEncoding}`; `Colour` elements → `ColourTags` and HDR metadata; `ProjectionType` other than none → `UNSUPPORTED_TRANSFORM`. The result is the same `SampleTable` and `MediaInfo` as MP4, so `frame_at`, `FrameReader` and probing need nothing new.

**Tests:** `av1_in_webm_decodes_like_mp4` (the same frames written into both containers decode to identical planes); `vfr_block_timestamps`; `cues_and_scan_agree`; `unknown_size_cluster`; `content_encoding_refused`; hostile inputs (size past the parent, depth, a 2⁵⁶ size, a lacing count over the block) → typed errors; the fuzz target runs 60 s clean locally.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(container): WebM/Matroska demux under limits, with a fuzz target`.

## Batch E: cache, proxies and serving

### Task 13: The media cache, its index and eviction — tier: opus

**Depends on:** Task 2.

**Files:**
- Create: `crates/fin-media/src/cache.rs` (pure: `Recipe {name, version}` constants `PROXY_540`, `PROXY_1080`, `THUMBS`, `STRIP`; `CacheKey::compute(&KeyInput) -> CacheKey` (Decision 11); `EntryMeta` = `meta.json`: `{recipe, recipeVersion, key, renderVersion, decoderClass, encoder?: {backend, class}, files: [{name, bytes, blake3}], media?: MediaInfo, createdAt}`), `crates/fin-node/src/media/cache/{mod.rs, index.rs, evict.rs, adopt.rs}` (napi `MediaCache`), `apps/finisherd/src/media/cache.ts` (pins, settings, RPCs), `crates/fin/src/commands/cache.rs`.
- Modify: `crates/fin-node/Cargo.toml` (`rusqlite` from the workspace), `packages/contracts/src/rpc.ts` (`media.cache.status`, `media.cache.gc`), `crates/fin/src/registry.rs`, `apps/finisherd/src/config.ts` (`media.cacheMaxGiB`).
- Tests: `crates/fin-node/tests/media_cache.rs`, `apps/finisherd/test/media-cache.test.ts`.

**Interfaces (napi, async on `fin-node`'s cache thread):**
```ts
class MediaCache {
  static open(serverData: string, capBytes: number): MediaCache;
  lookup(key: string): EntryMeta | null;                  // touches last-use
  commit(jobOutDir: string, key: string, projects: string[]): EntryMeta;   // fin-shm checks, verify meta and file hashes, rename into place, then insert
  pin(key: string, holder: string): void; unpin(key: string, holder: string): void;
  entriesFor(asset: { fingerprint: string }): EntryMeta[];
  status(): { bytes: number; entries: number; capBytes: number; pinned: number; byRecipe: Record<string, { bytes: number; entries: number }> };
  gc(targetBytes?: number): { evicted: number; freedBytes: number };
}
```
- Index tables: `entries {key PK, recipe, recipe_version, fingerprint, bytes, created_at, last_used_at}`, `entry_projects {key, project_id}`, `pins {key, holder}` (holders `job:<id>` and `project:<id>`, rebuilt at start for open projects).
- Decisions 10–12. Eviction runs after each commit and every 10 minutes; a pinned entry is skipped; a running job's pin is released by the kind's end hook.
- RPCs: `media.cache.status {}` (`server.read`), `media.cache.gc {toGiB?}` (`settings.admin`, the existing owner scope for server-wide settings; no new scope); `fin cache status`, `fin cache gc [--to <GiB>]`.

**Tests:** `commit_is_rename_then_insert` and `crash_between_rename_and_insert` (a test hook stops between the two; the next open adopts the directory); `foreign_dir_not_adopted` (a directory whose key does not recompute, or whose file hash differs, is deleted); `row_without_dir_dropped`; `pinned_not_evicted`; `lru_order`; `concurrent_commit_same_key_first_wins`; `cache_io_on_full_volume` (a quota'd temp directory on Linux; skip elsewhere); `projects_mapping_for_reach`; the cap default is `min(50 GiB, 15 % of the volume)`.

- [ ] **Steps:** tests, implementation, generated types, three gates → PASS. **Commit** `feat(node): the media cache, its SQLite index, pins and eviction`.

### Task 14: `media.proxy` and the `proxy540` library stage — tier: opus

**Depends on:** Tasks 3, 8, 9, 10, 13 (and 4–7 for the OS encoders on their hosts).

**Files:**
- Create: `crates/fin-media/src/jobs/proxy.rs` (`ProxyParams {asset, fingerprint, tier: t540|t1080, codec?: h264|av1, key}`, `ProxyResult {key, codec, backend, class, frames, bytes, toneMapped?, fellBack?}`), `crates/fin-workers/src/media/{proxy.rs, encode_pipeline.rs, scale.rs}`, `crates/fin-compositor/src/{scale.rs, nv12.rs}` (scale through the box mip chain then bilinear, and RGB → NV12 BT.709 limited range on the GPU, with a CPU reference for T1), `apps/finisherd/src/jobs/kinds/media.proxy.ts`, `apps/finisherd/src/media/{proxy.ts, stages.ts}`, `crates/fin/src/commands/media.rs` (`fin media proxy <asset> [--tier 540|1080] [--codec h264|av1] [--no-wait]`, `fin media status <asset>`).
- Modify: `packages/contracts/src/rpc.ts` (`media.proxy {projectId, asset, tier?, codec?, wait?}` → `{job}` or `ProxyResult`; `media.proxy.status {projectId, asset}` → `{tiers: [{tier, key?, codec?, job?}]}`), `apps/finisherd/src/projects.ts` (on first use of an asset in a project, submit a background `1080` proxy; spec §7.1 "When").
- Tests: `crates/fin-workers/tests/media_proxy.rs`, `apps/finisherd/test/media-proxy.test.ts`.

**Behaviour:**
- Worker: open the original through 6A's no-follow opener (size, mtime, file id checked as the manifest records); decode every presented frame in presentation order with `open_decoder`; tone-map (Decision 9); scale to the tier (Decision 6); convert to NV12; encode with `open_encoder(EncodePurpose::Proxy)` and Decisions 5, 8; mux MP4 with `fin-container` (fast start, `colr` BT.709 limited), CFR timestamps at the original's nominal rate (Decision 7); write `table.bin` (the original's sample table) and `meta.json` into `out/<key>/` with `create_output`; progress per 1 % of frames; cancel between frames. The worker recomputes the cache key and compares it (Decision 11).
- Kind: priority `background`, `qos: background`, cold, resources `["gpu", "hwenc"]` when the capability report names a hardware encoder for the codec, else `["gpu", "cpu"]` (a function of params over the cached report); `memory` Decision 18; `dedupe` = the cache key; `rememberPoison: true` (a poison item stays failed until its fingerprint or `RENDER_VERSION` changes, spec §3.3); `onSucceeded` → `MediaCache.commit` (projects: the project that asked, and every project whose assets share the fingerprint); `prepare` pins nothing (proxies read originals, not the cache).
- Stage: `proxy540` registered with 5D (`StageRegistry.register({name: "proxy540", version: "<recipe v>.<RENDER_VERSION>", jobKind: "media.proxy", batch: 1, appliesTo: video assets, params})`); `1080` on first use; both on request.
- `PROXY_UNAVAILABLE {asset, tier, job}` answers a consumer that needs a proxy that is being made.

**Tests:** `proxy_frames_match_original_ids` (CFR 30 fps, VFR, an edit-listed clip, a 90°-rotated clip: proxy frame `i` reads the original's frame id `i`; frame count equals the original's presented frames); `tier_sizes_and_no_upscale`; `gop_rules_540_and_1080` (read back from `stss`); `hdr_source_gives_sdr_proxy`; `rotation_not_baked`; `dedupe_coalesces_identical_requests`; `poison_remembered_until_fingerprint_changes`; `proxy_may_use_gpu_encoder` (fake `gpu` encoder chosen for `Proxy`); `cache_key_mismatch_is_plan_mismatch`; the stage runs for a library asset and skips audio-only assets.

- [ ] **Steps:** tests, implementation, generated types, three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(media): media.proxy with the one-frame-per-frame contract and the proxy540 stage`.

### Task 15: `media.thumbs`: thumbnails and filmstrips — tier: sonnet

**Depends on:** Task 14 (scale pass, cache commit).

**Files:** Create `crates/fin-media/src/jobs/thumbs.rs` (`ThumbsParams {asset, fingerprint, mode: {thumbs: {times ≤ 64, maxSide ≤ 512}} | {strip: {tileHeight}}, key}`), `crates/fin-workers/src/media/thumbs.rs`, `apps/finisherd/src/jobs/kinds/media.thumbs.ts` (background, `gpu`, batch with one `item` per thumbnail or per strip sprite), RPC `media.thumbs` in `packages/contracts/src/rpc.ts`, `fin media thumbs <asset> --at … | --strip`. Tests: `crates/fin-workers/tests/media_thumbs.rs`.

**Behaviour:** Decision 17; frames chosen with `frame_at`; reads the `1080` proxy when its entry exists, else the original; tone-mapped like proxies; each output an `item`, so a crash keeps what was done (spec §3.2 item 6).

**Tests:** `thumb_frame_ids_follow_frame_at` (midpoint ties included); `strip_index_times`; `crash_keeps_done_items` (5C's `test-hooks` kill after the second item); `reads_proxy_when_present`.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(media): media.thumbs for thumbnails and filmstrips`.

### Task 16: `media.look` from proxies — tier: sonnet

**Depends on:** Task 14.

**Files:** Modify `crates/fin-media/src/jobs/manifest.rs` (`ManifestAsset.proxies: [{tier, key, dir, codec}]`, spec §4.2 "from 6B"), `crates/fin-node/src/media/inputs.rs` (fills `proxies` from the index and pins them for the job), `crates/fin-workers/src/media/{sources.rs, open_cache.rs}` (a layer reads the `1080` proxy when present: `frame_at` on the entry's `table.bin`, then proxy frame `i`; originals with `--originals`), `apps/finisherd/src/media/look.ts` and `crates/fin/src/commands/look.rs` (`--originals`), `LookResult.frames[].source: proxy|original`. Tests: `crates/fin-workers/tests/media_look.rs` additions.

**Tests:** `look_from_proxy_same_frame_ids` (every e2e `look` time picks the same original frame id through the proxy); `look_from_proxy_within_t5_of_original`; `originals_flag_bypasses_proxies`; `missing_proxy_dir_falls_back_to_original` (the entry vanished between manifest and open: the layer reads the original and the result says so).

- [ ] **Steps:** tests, implementation, three gates and `rtk pnpm e2e look` → PASS. **Commit** `feat(media): media.look reads proxies through the original's sample table`.

### Task 17: Serving cache entries over HTTP with capability URLs — tier: sonnet

**Depends on:** Task 13.

**Files:** Create `apps/finisherd/src/media/{http.ts, cap-tokens.ts}`; modify `apps/finisherd/src/upgrade.ts` (route `GET /media/cap/…` to `http.ts` before the WebSocket path; same `Host`/`Origin` checks), `packages/contracts/src/rpc.ts` (`media.url {key, file}` → `{url, expiresAt}`), `apps/finisherd/src/handlers.ts`, the principal revocation hook (5A) to drop tokens. Tests: `apps/finisherd/test/media-http.test.ts`.

**Behaviour:** Decisions 13–14. Single `bytes=` ranges (`206`, `Content-Range`), `416` past the end, `HEAD`, `Content-Type` from the file's extension in the entry (`video/mp4`, `image/png`, `application/json`), `Cache-Control: private, max-age=600, immutable`, `Cross-Origin-Resource-Policy: same-origin`; files are opened with `fin-shm`'s no-follow opener through `fin-node` and streamed; at most 64 tokens per principal (oldest dropped).

**Tests:** `range_requests_exact_bytes`; `unknown_expired_revoked_all_404`; `file_outside_entry_404` (`../`, `%2e%2e`, a name not in `meta.json`); `reach_required_for_media_url` (another project's key is `NOT_FOUND`); `host_and_origin_checked`; `token_bound_to_file`; `principal_revocation_drops_tokens`.

- [ ] **Steps:** tests, implementation, three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(server): media cache entries over HTTP with capability URLs`.

## Batch F: the Linux worker profile (after 7C)

### Task 18: Media workers under 7C's Linux Landlock `worker` profile — tier: opus

**Depends on:** **7C Task 2** (Linux Landlock and the `worker` profile's Landlock phase) and **7C Task 7** (`SandboxSpawner` for the job host on Linux) (sp6 decision 6), Tasks 13–16 (the grants name the cache and the new kinds).

**Interface from 7C** (reconciled with the 7C plan, Tasks 2 and 7; a later mismatch is fixed in this task, not worked around):
- **7C Task 2:** `fin-sandbox` (crate) with `policy::SandboxPolicy` and `Profile::Worker` (agent §8.1, §8.10), and `fin_sandbox::self_apply` (`crates/fin-sandbox/src/self_apply.rs`; the plan's earlier name `fin_sandbox::linux::self_apply` is dropped), registered by 7C itself in `crates/fin-workers/src/main.rs` with the signature of `fin_jobs::sandbox::ApplyFn` (`fn(&SelfSandbox) -> Result<(), String>`); it accepts `SelfSandbox {profile: "worker", args}` (7C's `WorkerInput`, the read, write and device grants of the job) and applies a Landlock ABI 3 ruleset with `no_new_privs`; seccomp joins in 7C Task 4 and is not needed here;
- **7C Task 7:** `SandboxSpawner` turning a `SpawnRequest`'s path grants (profile `worker`) into the `FIN_SELF_SANDBOX` request, and reporting `HostCaps.profiles` and `sandboxed()` (so `JobHostCaps.sandboxed` and `sandbox: landlock|none` with `reason: landlockDisabled` follow from it);
- 7C's `SANDBOX_DISABLED {profile: worker}` status warning (already shaped in `apps/finisherd/src/server-admin.ts`) and `sandbox.unconfined` diagnostics event, and `SANDBOX_UNAVAILABLE` when Landlock is enabled but the ruleset fails to apply (agent §8.9).
- **No local fallback.** Spec §3.5.1 allowed "a small module in `fin-workers` that 7C later replaces"; the dependency is explicit instead (`TASKS.md`), so this plan builds none. If 7C's profile is late, Batch F waits and Batches A–E may merge first with Linux workers unconfined under the existing warning, as in 6A.

**Files:**
- Modify: `crates/fin-workers/src/main.rs` and `crates/fin-workers/Cargo.toml` (**7C Task 2 already adds** `fin_jobs::sandbox::register(fin_sandbox::self_apply)` and the `fin-sandbox` dependency; this task adds nothing there beyond a rebase), `apps/finisherd/src/jobs/kinds/media.*.ts` (each kind's path grants per spec §3.5.1: reads = the manifest's originals and proxies for this job, the font cache, the media cache entries named, `in/`; writes = `out/` and a private temp; devices = `/dev/dri/renderD*` read/write, `/dev/dri/card*` read-only, the Vulkan ICD and driver files, `/sys/class/drm` and `/sys/dev/char` reads), `crates/fin-workers/src/media/fonts.rs` (open the font cache through `fin-library`'s no-follow opener: spec §25.5 residual 1), `crates/fin-codecs/src/probe/mod.rs` (`worker.sandbox`).
- Tests: `crates/fin-workers/tests/media_sandbox.rs` (`#![cfg(target_os = "linux")]`), `apps/finisherd/test/media-sandbox.test.ts`.

**Behaviour:** Decision 15: with Landlock active, warm kinds dispatch cold (5C's rule, already in `warm.ts` behind `caps().sandboxed`/`hello.selfSandbox`) until 7C Task 12 lands, so every media worker confines itself to one job; once 7C Task 12 is in (`caps().perJobGrants`), `media.look` and `media.capabilities` run warm again and this task's cold-only test is replaced by 7C's warm-sandbox tests. No seccomp (network and process creation stay open until 7C's full profile in 6G; spec §3.5.1). A Landlock-disabled host runs the baseline and reports `sandbox: none`; nothing is refused.

**Tests (worker escape suite, Linux):** each is "attempt, observe a denial" (agent §8.11): write outside `out/` (the job dir's `in/`, the cache, the project folder, `/tmp` outside the private temp); read a file the manifest does not name (another source in the same library root, server data, `~/.ssh` of a test home); create a link out of `out/`; and the positive rows: `media.look` and `media.proxy` succeed under the profile on lavapipe and (validation) an Intel iGPU, with identical bytes to an unconfined run. `landlock_disabled_runs_with_warning` (7C's test hook that reports Landlock disabled: the job succeeds, `server.status` carries `SANDBOX_DISABLED {profile: worker}`, `media.capabilities` says `sandbox: none`); `apply_failure_fails_the_job` (`SANDBOX_UNAVAILABLE`).

- [ ] **Steps:** rebase onto 7C's branch, tests, implementation, three gates and `rtk pnpm e2e` on Linux with Landlock (CI's Ubuntu 24.04 kernel) → PASS. **Commit** `feat(workers): media workers confine themselves with 7C's Landlock worker profile on Linux`.

## Batch G: diagnostics, e2e, validation, docs

### Task 19: Diagnostics and error codes — tier: sonnet

**Depends on:** Tasks 8, 14, 15.

**Files:** Modify `crates/fin-core/src/events/{kinds.rs, payloads/…}` (`media.capabilities` v2: per codec and profile, classes, guidance codes, encoder session limits and adapter ids, `sandbox`; `render.stats` gains the `encode` phase per backend), `crates/fin-workers/src/media/diag.rs` (`job.done.media` for `media.proxy`, `media.thumbs`, `media.probeDeep`: `codec`, `backend`, `hw`, `resolution`, `frames`, `rtFactorMilli`, `cacheHit`, `copyPath`), `crates/fin/src/output.rs`, `apps/finisherd/src/console/exit-codes.ts` (codes of the Global Constraints). Tests: the registry's schema tests, `exit_codes_parity`.

**Tests:** no free-form string in the new payloads (adapter and MFT names absent; a test builds a report on a host whose adapter name contains a path-like string and greps the journal); `strip_content` on the new kinds; a `media.proxy` job's `job.done` carries the media object.

- [ ] **Steps:** tests, implementation, generated types, three gates → PASS. **Commit** `feat(core): media.capabilities v2, encode stats and media fields for the new kinds`.

### Task 20: e2e `proxy` scenario and CI — tier: sonnet

**Depends on:** Tasks 13–17, 19 (Task 18 adds its Linux rows when it lands).

**Files:** Create `scripts/e2e/proxy.mjs`; modify `scripts/e2e.mjs`, `.github/workflows/ci.yml` (macOS and Windows jobs run the platform codec tests with `FIN_REQUIRE_PLATFORM_CODECS=0`, since runners lack the HEVC extensions and hardware encoders; every action pinned to a commit SHA).

**Scenario:** generate an AV1 CFR clip, an AV1 VFR clip rotated 90° (everywhere), an H.264 clip (macOS and Windows: the OS encoder); `fin asset add` into a library and a project; wait for the `proxy540` stage and `fin media proxy --tier 1080` for each; `fin look` at seven times (midpoints included) reads proxies (`source: proxy`) with the same frame ids as `--originals`; `fin media thumbs --strip`; `media.url` + an HTTP range request returns the exact bytes; `fin cache status` lists the entries; `fin cache gc --to 0` evicts nothing pinned by the open project and everything else after it closes; the journal has `media.capabilities` v2 and `job.done.media` for the proxy jobs; on Linux, H.264 assets are `CODEC_UNAVAILABLE` with `laterBatch` guidance.

- [ ] **Steps:** scenario, CI changes, a green CI run on all three OSes → **Commit** `test(e2e): proxy scenario; ci: platform codec tests on macOS and Windows`.

### Task 21: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

**Depends on:** Tasks 1–20.

Each row on **macOS arm64** (an M1/M2 and an M3 or later for AV1 decode), **Windows 11 x64** with a discrete GPU (and an Intel iGPU machine for a second hardware MFT vendor), **Linux x64** (kernel ≥ 6.2, Landlock enabled, an Intel iGPU), a **Linux host with Landlock disabled**, the software adapters, and the **D31-minimum machine** (spec §14). Numbers go to `archive/sp6/6b1-validation.md` by role, never by host; durable conclusions to the spec (Task 22).

- [ ] 1. `FIN_REQUIRE_SERVER=1 cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` (all scenarios) green, with `FIN_REQUIRE_PLATFORM_CODECS=1` on the macOS and Windows hosts.
- [ ] 2. T3 across OSes: the per-fixture YUV hashes of H.264 and AV1 fixtures agree between VideoToolbox, MF (hardware and Microsoft's MFT) and dav1d; a backend that differs is excluded and recorded.
- [ ] 3. T5 and speed of each encoder for proxies: VideoToolbox, each hardware MFT, Microsoft's MFT, SVT-AV1 (spec §14: `540` hardware ≥ 10× real time, ≥ 4× on the D31-minimum; SVT-AV1 4 threads ≥ 5×, ≥ 2×).
- [ ] 4. Microsoft's H.264 MFT at 1080p30 on the 60 s reel's frames (spec §14's provisional export rows: ≥ 2× real time, ≥ 1× on the D31-minimum) and its T5 quality at the template bitrate: replace "provisional" with the measurement or record the miss as a 6E risk.
- [ ] 5. `ENCODER_HUNG` on real hardware: the probe's test clip on each hardware encoder; record any encoder left out.
- [ ] 6. Capability reports per host recorded (codes only), including a Windows N edition without the Media Feature Pack, a host without the HEVC Video Extensions, and an LTSC host if one is at hand (verify the `ltscStoreCodecs` text; else keep "may lack").
- [ ] 7. HDR rows from `FIN_MEDIA_FIXTURES` (`FIN_REQUIRE_MEDIA=1`): an iPhone HLG/Dolby Vision 8.4 clip and a PQ clip tone-mapped on each OS; T4 within each OS; mean ΔE2000 between OSes reported (not gated).
- [ ] 8. Proxy contract on real footage: a phone VFR screen recording and an edit-list export: proxy frame `i` is original frame `i` (frame ids where the clip carries them; otherwise per-frame PSNR against the original decode ≥ 35 dB with no index offset).
- [ ] 9. `fin look` from proxies, warm and cold, p50/p95 over 50 runs at 1080p (spec §14: warm ≤ 300 ms, cold ≤ 1.5 s; D31-minimum ≤ 600 ms / ≤ 3 s); on Linux with Landlock, the cold figure is the one that applies until 7C Task 12 lands, and the warm figure applies after it (Decision 15).
- [ ] 10. Linux worker profile: the escape suite as a non-elevated user on the Landlock host; the warning path on the Landlock-disabled host; GPU rendering under the profile on the Intel iGPU.
- [ ] 11. Cache under load: 10,000 entries, `gc` time, adoption after a killed `finisherd` mid-commit.
- [ ] 12. CI: a green `ci.yml` run on all three OSes and one manual `nightly.yml` run with the `webm` fuzz target.

**6B-1 is done** when every row passes or has an accepted, recorded exception. **Commit** (records only) `docs: 6B-1 validation results`.

### Task 22: Spec as built, architecture pointer, notices and tracker — tier: sonnet

**Depends on:** Task 21.

**Files:** Modify `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` (status line; a new **§26 "Sub-batch 6B-1 as built"** mirroring §25: decisions As built / Deviation with this plan's Decision numbers, review fixes, owner decisions as applied, measured numbers, residuals, the 6B-1 validation checklist; "As built" notes in §3.2, §3.5.1, §4.2, §4.4, §6.1–6.4, §7, §10.1, §14, §15, §17, §19; Decisions 4, 13, 15 and 16 recorded as decided on 2026-11-08, and §7.3 and §6.4 amended per Decisions 13 and 16), `docs/specs/2026-11-01-finisher-architecture-design.md` §27 (a pointer item for 6B-1), `THIRD_PARTY_NOTICES.md` (the `objc2-*` crates are covered by the cargo notice list; the OS frameworks named as `os` components), `TASKS.md` (6B-1 done, residuals and follow-ups for 6B-2, 6C, 6E, 6F, 6G; the counsel items below unchanged).

**Counsel items stay counsel items** (`TASKS.md` "Counsel before release/upload"; nothing in this plan decides them): 6.1 (Linux AAC-LC decode and encode, decision 1: not built here); 6.9 (the export and proxy encoder policy: built here as decided, with counsel's review still owed before release); 6.2 (the Electron ffmpeg exception: waits for sub-project 9).

- [ ] **Steps:** write the as-built section from the tasks' commits and Task 21's results, update the tracker → three gates → PASS. **Commit** `docs: 6B-1 as built`.

---

## Owner decisions (2026-11-07) as they bear on 6B-1

| Decision | In 6B-1 |
|---|---|
| 1 Linux AAC via symphonia decode and vendored vo-aacenc export; counsel checks patents | Not built (6C). The capability report says so. Counsel item 6.1 unchanged. |
| 2 / F4 Electron's ffmpeg provisional, desktop only, decided in sub-project 9 | Nothing here ships or assumes it; `--codec` serves either proxy codec. |
| 3 No codec drivers in Docker | Nothing bundled or required; no image before 6G. |
| 4 Default fit contain | Proxies keep coded orientation and aspect; base box unchanged. |
| 6 Landlock in 6B | Task 18 on 7C Tasks 2 and 7 (the Linux `worker` profile); warm kinds run cold under it until 7C Task 12 (Decision 15). |
| 10 Proxies are video-only in 6B-1 (2026-11-08) | Proxy recipe v1 has no audio track; 6C adds Opus audio as recipe v2 (Decision 4). |
| 7 GPU ids recorded | Numeric adapter ids for decoders and encoders; no strings. |
| 8 6B covers all three OSes | Every batch green on macOS, Windows 11 and Linux; Linux platform codecs are 6B-2. |
| 9 Exports through OS-vendor encoders only; no HEVC template; Windows 11 only | `encoder_allowed` (Decision 2); HEVC encode only for macOS test fixtures; no Windows 10 path. |
| F3 Proxies may use any hardware encoder | `EncodePurpose::Proxy` allows `gpu`; a `gpu` file never leaves the cache. |

## Answered

All four questions are answered (2026-11-08). The owner decided Decision 4 (video-only proxies; 6C adds Opus as recipe v2). The coordinator decided Decisions 13 (capability URLs only), 15 (warm `fin look` held to the cold budget under Landlock until 7C Task 12, then warm again under 7C Decision 19's one warm worker per open project) and 16 (libvpx VP9 decode in 6B-2). Nothing is open.
