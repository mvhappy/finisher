# Bootstrap and Architecture Spike: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the Finisher monorepo with its quality gates, then run the throwaway spike. The spike either confirms or overturns the risky architecture choices (spec §24 #1, items 1–11) before any app code is written.

**Architecture:**
- Bootstrap lands on `main` through a `bootstrap` branch and is permanent.
- Spike code lives only on a `spike` branch. It is **never merged**. Only the spike report and any spec amendments are merged to `main`.
- Every experiment writes a machine-checked `result.json`. The final report task derives the go/no-go decisions from those files.

**Tech stack:**
- Rust (pinned stable, edition 2024), Cargo workspace.
- Node 26.12.1 and pnpm via corepack, TypeScript, Vitest, `node:test`.
- napi-rs v3, wgpu (native + WASM).
- Electron 44.10.0 (matches T3 Code).
- GitHub Actions.

**Spec:** `/Volumes/Storage/Projects/finisher/docs/specs/2026-11-01-finisher-architecture-design.md`. Read §2 (decisions), §6.7, §7, §8.2–8.4, §12, §13, §18, §23 and §24.

## Global Constraints

- **Shell:** prefix every shell command with `rtk`. If `rtk` filters output a measurement needs, use `rtk proxy <cmd>`.
- **Commits:** Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`. End messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Licence:** Finisher is MIT. **No GPL, LGPL, AGPL, SSPL or EUPL dependency in any shipped crate or package.** MPL-2.0 is allowed (file-level copyleft, unmodified use). Spike-only measurement tools (Python harnesses, the Montreal Forced Aligner) may be copyleft because they never ship. Mark them `spike-only` in the experiment README.
- **CI:** GitHub Actions. Every third-party action is pinned to a full 40-character commit SHA, with the version in a trailing comment.
- **Platform floors (D31):** macOS 14+ on Apple Silicon; Windows 11 and Windows 10 22H2 x64; Linux glibc 2.35+.
- **Naming (§3):** crates `fin-*`, packages `@finisher/*`, server `finisherd`, CLI `fin`.
- **Electron 44.10.0, Node 26.12.1, Effect 4.0.5:** pinned exactly (not caret).
- **Source size limit:** non-test source files at most 600 lines; test files at most 1500 lines. Enforced by the gate in Task 3.
- **Spike measurements:**
  - release builds only (`--release`, production JS bundles);
  - at least 5 repetitions after at least 1 warm-up run;
  - report p50, p95 and max;
  - record the environment (OS version, CPU, GPU, RAM, power state).
  - **Baseline machine:** the user's Apple M4 / 24 GB. If the user supplies an M1 / 8 GB, it is the low-end check for preview (Task 7).
- **Result file:** every spike experiment writes `spike/<id>/result.json`, validated by `spike/harness/validate-result.mjs` (Task 5). An experiment is not done until its result validates.

## Environment Prerequisites (user-provided; each task states which it needs)

| ID | Prerequisite | Needed by |
|---|---|---|
| E1 | `rustup` installed on this Mac. No Rust toolchain is currently present. | Tasks 1–15 |
| E2 | GitHub repository for `finisher` (owner and visibility decided by the user) so CI can run | Task 4 and the CI matrix runs in Tasks 6, 9, 12, 13 |
| E3 | Linux desktop machine with an Intel/AMD (VA-API) or NVIDIA GPU, reachable over SSH, with a graphical session | Tasks 8 and 9 (Linux hardware rows) |
| E4 | Windows 11 machine with a GPU (GitHub runners have no GPU) | Tasks 7 and 9 (Windows hardware rows) |
| E5 | NAS with an SMB share and an NFS export, mounted on this Mac; ideally a second client machine | Task 14 |
| E6 | Dropbox, iCloud Drive and OneDrive syncing on this Mac and on one second machine | Task 14 |
| E7 | Claude Code and Codex logged in on this Mac (both binaries are present) | Task 11 |
| E8 | Apple Developer ID identity (present) and the `video-creater-notary` Keychain item (present) | Task 13 |
| E9 | Windows code-signing: a self-signed test certificate is enough for the spike; the real certificate comes later | Task 13 |

**If a prerequisite is missing,** the affected rows record `verdict: "inconclusive"` with `blockedBy: "E<n>"`. They are never recorded as passing.

## Review Focus

1. **Measuring on the wrong build or machine.** A debug build, a single run, or a run without environment data would make a "pass" meaningless. The validator in Task 5 rejects results without `build: "release"`, at least 5 repetitions, and an `environment` object.
2. **"Fast but wrong."** A rebase benchmark that never checks correctness, or a preview that hits 60 fps with A/V drift. Every performance experiment pairs its metric with a correctness assertion (Tasks 6, 7, 14). The validator requires a `correctness` field set to `"verified"` before a pass.
3. **No GPU treated as a GPU result.** CI runners render WebGPU on SwiftShader or software. The validator downgrades any pass where `environment.gpu.kind` is `"software"` to `"inconclusive"` (Task 5 test).
4. **Sync services altering files after the run.** Dropbox and iCloud rename or duplicate files minutes later. Task 14 re-verifies state after a 15-minute settle period, not just immediately.
5. **Silent fallback.** A pipeline quietly using a software decoder, an unsandboxed spawn, or an unsigned nested binary. Tasks 9, 12 and 13 assert which path was actually taken (hardware decode flag, sandbox denial observed, `codesign`/`signtool` verify on every nested binary).

---

# Part A: Bootstrap (permanent, branch `bootstrap` → `main`)

### Task 1: Repository skeleton and agent docs

**Files:**
- Create: `LICENSE`, `README.md`, `AGENTS.md`, `CLAUDE.md`, `THIRD_PARTY_NOTICES.md`, `TASKS.md`, `.gitignore`, `.editorconfig`, `.gitattributes`

**Interfaces:**
- Produces: `AGENTS.md`, the rulebook later tasks and agents follow. `TASKS.md` is the progress tracker that Tasks 2–15 update.

- [ ] **Step 1: Create branch.** Run `rtk git switch -c bootstrap` in `/Volumes/Storage/Projects/finisher`.
- [ ] **Step 2: Write `LICENSE`.** Standard MIT text, `Copyright (c) 2026 <holder>`. Use the copyright holder the user confirmed at plan approval. Until then, use `Finisher contributors`.
- [ ] **Step 3: Write `AGENTS.md`.** Include these rules verbatim as bullets:
  - Prefix shell commands with `rtk`.
  - Conventional Commits.
  - Pin GitHub Actions to commit SHAs.
  - No GPL/LGPL/AGPL/SSPL/EUPL dependencies in shipped code.
  - Primitive ops live only in `fin-core`, one module per op.
  - TypeScript model types are generated, never hand-written.
  - Non-test source files ≤ 600 lines, tests ≤ 1500.
  - No cloud-only imports in this repository.
  - The spec is `docs/specs/2026-11-01-finisher-architecture-design.md`.
  - Spike code lives only on the `spike` branch.
- [ ] **Step 4: Write `CLAUDE.md`** containing exactly `@AGENTS.md`.
- [ ] **Step 5: Write `THIRD_PARTY_NOTICES.md`.** Headings for every entry in spec §18, each with a `status: planned` line. The file is a ledger; it is filled in as code is vendored.
- [ ] **Step 6: Write `TASKS.md`.** One checkbox per task in this plan.
- [ ] **Step 7: Write `.gitignore`** covering `target/`, `node_modules/`, `dist/`, `out/`, `*.node`, `.DS_Store`, `spike/**/fixtures/media/`, `spike/**/output/`.
- [ ] **Step 8: Commit.** `rtk git add -A && rtk git commit -m "chore: add repository skeleton and agent rules"`

### Task 2: Toolchains and workspaces

**Prerequisite:** E1.

**Files:**
- Create: `rust-toolchain.toml`, `Cargo.toml` (workspace), `crates/fin-core/Cargo.toml`, `crates/fin-core/src/lib.rs`, `package.json`, `pnpm-workspace.yaml`, `.node-version`, `tsconfig.base.json`, `vitest.config.ts`, `packages/contracts/package.json`, `packages/contracts/src/index.ts`, `packages/contracts/src/index.test.ts`

**Interfaces:**
- Produces:
  - `fin_core::version() -> &'static str`, returning `env!("CARGO_PKG_VERSION")`.
  - `@finisher/contracts` exporting `PROTOCOL_VERSION = 1` (number).
  - Root scripts `pnpm test` (Vitest) and `pnpm test:gates` (`node --test scripts/`).

- [ ] **Step 1: Write the failing Rust test** in `crates/fin-core/src/lib.rs`: `#[test] fn version_matches_manifest() { assert_eq!(version(), "0.0.0"); }`
- [ ] **Step 2: Run it** with `rtk cargo test -p fin-core`. Expected: FAIL (`version` not found).
- [ ] **Step 3: Implement `pub fn version() -> &'static str`.**
  - Set the crate version to `0.0.0`.
  - Workspace `edition = "2024"` and `resolver = "3"`.
  - `rust-toolchain.toml` pins the stable version current at execution time, as an exact `channel = "1.xx.y"`, with components `rustfmt` and `clippy`.
- [ ] **Step 4: Run it** with `rtk cargo test -p fin-core`. Expected: PASS.
- [ ] **Step 5: Write the failing TS test** in `packages/contracts/src/index.test.ts`: `expect(PROTOCOL_VERSION).toBe(1)`.
- [ ] **Step 6: Set up the JS toolchain.**
  - `.node-version` contains `26.12.1`.
  - Root `package.json` has `"packageManager": "pnpm@<exact current>"` and `"engines": { "node": "26.12.1" }`.
  - Enable pnpm with `rtk corepack enable`, then install with `rtk pnpm install`.
  - Run `rtk pnpm test`. Expected: FAIL.
- [ ] **Step 7: Implement `export const PROTOCOL_VERSION = 1 as const;`.** Run `rtk pnpm test`. Expected: PASS.
- [ ] **Step 8: Commit.** `rtk git add -A && rtk git commit -m "chore: add Rust and pnpm workspaces"`

### Task 3: Quality gates

**Files:**
- Create:
  - `deny.toml`
  - `scripts/check-js-licenses.mjs`, `scripts/check-file-size.mjs`, `scripts/check-cloud-imports.mjs`, `scripts/check-action-pins.mjs`
  - `scripts/*.test.mjs` (one per gate)
  - `scripts/fixtures/` (violating and clean samples)
- Modify: `package.json` (add a `gates` script running all four gates plus `cargo deny check`)

**Interfaces:**
- Produces: each gate is a CLI. `node scripts/<gate>.mjs [--root <dir>]` exits 0 when clean and 1 on violation, printing one `path: reason` line per violation. CI (Task 4) runs `pnpm gates`.

- [ ] **Step 1: Write the failing tests** with `node:test`, each running the gate against a fixture root:
  - `check-file-size.test.mjs`:
    - `big.rs` (601 lines) violates.
    - `big.test.ts` (1501 lines) violates.
    - `ok.rs` (600 lines) passes.
    - Files under `target/`, `node_modules/` and `**/generated/**` are ignored.
  - `check-cloud-imports.test.mjs`: these violate (import graph only, so names in comments or Markdown pass):
    - `import x from "@finisher-cloud/billing"`
    - `use finisher_cloud::auth;`
    - a `Cargo.toml` dependency on `finisher-cloud-*`
  - `check-action-pins.test.mjs`:
    - `uses: actions/checkout@v4` violates.
    - `uses: actions/checkout@<40 hex> # v4.2.2` passes.
    - Local `uses: ./.github/actions/x` passes.
  - `check-js-licenses.test.mjs`: a fixture `licenses.json` in `pnpm licenses list --json` shape:
    - GPL-3.0, LGPL-2.1-only, AGPL-3.0 and SSPL-1.0 each fail.
    - `SEE LICENSE IN README.md` fails unless listed in `scripts/license-allowlist.json` with a reason.
    - MIT, Apache-2.0, BSD-3-Clause, ISC and MPL-2.0 pass.
- [ ] **Step 2: Run them.** `rtk pnpm test:gates`. Expected: FAIL (scripts missing).
- [ ] **Step 3: Implement the four scripts** (Node built-ins only, no dependencies).
  - `check-js-licenses.mjs` shells out to `pnpm licenses list --json --prod` unless `--input <file>` is given.
- [ ] **Step 4: Write `deny.toml`.**
  - `[licenses] allow = ["MIT","Apache-2.0","Apache-2.0 WITH LLVM-exception","BSD-2-Clause","BSD-3-Clause","ISC","Zlib","Unicode-3.0","MPL-2.0","CC0-1.0","BSL-1.0"]`
  - `confidence-threshold = 0.9`
  - `[bans] multiple-versions = "warn"`
  - `[sources] unknown-registry = "deny"`, `unknown-git = "deny"`
- [ ] **Step 5: Run the gates.** `rtk pnpm test:gates && rtk cargo install cargo-deny --locked && rtk pnpm gates`. Expected: all tests PASS and `pnpm gates` exits 0 on the repository.
- [ ] **Step 6: Commit.** `rtk git commit -m "chore: add licence, size, cloud-import and action-pin gates"`

### Task 4: CI workflow

**Prerequisite:** E2 for running in GitHub. The workflow file and its local checks need no prerequisite.

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `pnpm test`, `pnpm test:gates`, `pnpm gates`, `cargo test --workspace` (Tasks 2–3).
- Produces: job `ci` with matrix `os: [macos-15 (arm64), <current macOS x64 label>, windows-2025, ubuntu-24.04]`. The spike branch reuses this matrix for napi, codec, sandbox and packaging rows.

- [ ] **Step 1: Write the workflow.** Steps: checkout → setup node (from `.node-version`) → corepack pnpm install `--frozen-lockfile` → Rust toolchain from `rust-toolchain.toml` → `cargo test --workspace --release` → `pnpm test` → `pnpm test:gates` → `pnpm gates` (macOS/Linux only for `cargo deny`). Pin every action to a SHA looked up at execution time with `rtk gh api repos/<owner>/<repo>/git/ref/tags/<tag>`.
- [ ] **Step 2: Check the pins.** `rtk node scripts/check-action-pins.mjs`. Expected: exit 0.
- [ ] **Step 3: Merge and push.**
  - Commit with `rtk git commit -m "ci: add cross-platform CI"`.
  - Merge with `rtk git switch main && rtk git merge --ff-only bootstrap`.
  - **With E2:** `rtk git remote add origin <url> && rtk git push -u origin main`, then confirm all four matrix legs are green with `rtk gh run list --limit 1` and `rtk gh run view <id>`.
  - **Without E2:** record `CI: not yet run (E2)` in `TASKS.md`.

---

# Part B: Spike (throwaway, branch `spike`, never merged)

All spike work happens in `spike/` on branch `spike` (`rtk git switch -c spike main`). Each experiment directory contains:
- `README.md`: question, method, thresholds, verdict, `spike-only` tools;
- the harness code;
- `result.json`.

### Task 5: Spike harness and result schema

**Files:**
- Create: `spike/README.md`, `spike/harness/result.schema.json`, `spike/harness/validate-result.mjs`, `spike/harness/validate-result.test.mjs`, `spike/harness/env-probe.mjs`

**Interfaces:**
- Produces:
  - **Result schema:**
    ```
    { id: "S1".."S11", question: string, build: "release",
      environment: { os, osVersion, cpu, ramGb, gpu: { name, kind: "hardware"|"software"|"none" }, power: "ac"|"battery", runner: "local"|"ci" },
      repetitions: integer >= 5, metrics: { [name]: { p50?, p95?, max?, value?, unit } },
      thresholds: { [metricName]: { op: "<="|">="|"==", value, unit } },
      correctness: "verified"|"not-applicable"|"failed",
      rows?: [ { label, verdict, blockedBy?, notes? } ],
      verdict: "pass"|"fail"|"inconclusive", blockedBy?: "E1".."E9", notes: string }
    ```
  - `validate-result.mjs <file>` exits 0 or 1. It recomputes the verdict from metrics against thresholds and rejects a file whose stated verdict disagrees.
  - `env-probe.mjs` prints the `environment` JSON for the current machine.

- [ ] **Step 1: Write the failing tests** in `validate-result.test.mjs`:
  - `build: "debug"` → reject.
  - `repetitions: 3` → reject.
  - A threshold missed while `verdict: "pass"` → reject.
  - All thresholds met but `correctness: "failed"` → computed verdict `fail`.
  - All met with `gpu.kind: "software"` on a GPU experiment (S3, S4, S11) → computed verdict `inconclusive`.
  - Missing prerequisite (`blockedBy: "E3"`) → verdict must be `inconclusive`.
  - A valid file → exit 0.
- [ ] **Step 2: Run them.** `rtk node --test spike/harness/`. Expected: FAIL.
- [ ] **Step 3: Implement `validate-result.mjs` and `env-probe.mjs`** (Node built-ins only; JSON Schema validation is hand-rolled for these fields).
- [ ] **Step 4: Run them.** `rtk node --test spike/harness/`. Expected: PASS.
- [ ] **Step 5: Commit.** `rtk git commit -m "test(spike): add result schema and validator"`

### Task 6: S1. Engine boundary (napi throughput, event loop, sidecar fallback, TS comparison)

**Answers:** spec §7 build risk; D9/D24; Fable's mind-changer #3. **Prerequisite:** E1; CI rows need E2.

**Files:**
- Create:
  - `spike/s1-engine/toy-core/` (Rust lib): `model.rs`, `ops.rs`, `rebase.rs`
  - `spike/s1-engine/toy-node/` (napi-rs v3 crate)
  - `spike/s1-engine/toy-sidecar/` (Rust bin, newline-delimited JSON over a Unix socket or Windows named pipe)
  - `spike/s1-engine/toy-ts/` (TS reimplementation of the same model)
  - `spike/s1-engine/bench.mjs`, `spike/s1-engine/electron-load/` (minimal Electron 44.10.0 app)

**Interfaces:**
- **Toy model:**
  - `Project { tracks: Vec<Track> }`, `Track { items: Vec<Item> }`, `Item { id: u64, start: i64, dur: i64 }` (time in ticks).
  - Ops: `Insert { track, id, start, dur }`, `Trim { id, start, dur }`, `Delete { id }`.
  - `apply(&mut Project, &Op) -> Result<Diff, OpError>` rejects overlaps on a track and unknown IDs.
  - `invert(&Project, &Op) -> Op` returns the op that undoes `op` when applied to the post-op state; it is computed before applying.
  - `rebase(ops: &[Op], onto: &[Op]) -> Result<Vec<Op>, Conflict>`: shift positions after inserts and deletes; a trim or delete of a deleted ID is a `Conflict`.
- **napi:**
  - `open(json) -> Handle`
  - `apply(handle, opJson) -> diffJson` (sync)
  - `applyAsync(handle, opJson) -> Promise<diffJson>`
  - `rebaseAsync(handle, opsJson, ontoJson) -> Promise<opsJson>`
  - Sidecar has the same three operations over the socket.

- [ ] **Step 1: Write the failing correctness tests** in `toy-core`:
  - Applying and then inverting 1,000 random ops returns the original state hash.
  - Rebasing a branch of 1,000 random ops onto 1,000 random `main` ops yields a project where every op is valid, or a `Conflict` naming the op.
  - TS implementation: identical final state hash to Rust for the same seeded op stream, as a Vitest test calling both.
- [ ] **Step 2: Run them.** `rtk cargo test --release -p toy-core && rtk pnpm --dir spike/s1-engine vitest run`. Expected: FAIL.
- [ ] **Step 3: Implement** `toy-core`, `toy-node`, `toy-sidecar` and `toy-ts`.
- [ ] **Step 4: Run them.** Same command. Expected: PASS. This makes `correctness: "verified"` possible.
- [ ] **Step 5: Run the benchmark.** `bench.mjs` uses a seeded 10,000-item project on 8 tracks and measures, with `perf_hooks.monitorEventLoopDelay` sampling throughout:

  | Metric | Threshold |
  |---|---|
  | `napi_apply_p99_us` (sync, including JSON) | ≤ 500 µs |
  | `napi_rebase_1k_ms` (async) | ≤ 50 ms |
  | `event_loop_delay_p99_ms` during async rebase | ≤ 10 ms |
  | `sidecar_roundtrip_p99_us` | ≤ 2000 µs |
  | `ts_rebase_1k_p99_ms` | recorded, no threshold. Below 5 ms counts as Fable mind-changer #3 in the report. |
  | `rust_rss_mb_10k` vs `ts_heap_mb_10k` | recorded |

  Run with `rtk node spike/s1-engine/bench.mjs --reps 5 > spike/s1-engine/result.json`.
- [ ] **Step 6: Electron load rows.** The `electron-load` app `require`s the napi `.node` both from the main process and from a child spawned with `ELECTRON_RUN_AS_NODE=1`, and calls `applyAsync` once.
  - Rows: `macos-arm64`, `macos-x64`, `windows-x64`, `linux-x64`. Each passes only if both load paths succeed in CI on a packaged (`electron-builder --dir`) build.
  - Add a `spike-s1` job to the CI workflow on the `spike` branch.
- [ ] **Step 7: Validate.** `rtk node spike/harness/validate-result.mjs spike/s1-engine/result.json`. Expected: exit 0.
- [ ] **Step 8: Commit.** `rtk git commit -m "test(spike): S1 engine boundary measurements"`

### Task 7: S2 + S3. Render-plan size, and WASM preview with audio master clock

**Answers:** D11, D24 mind-changer #2, spec §12.3 frame ingest, §12.7 A/V sync. **Prerequisite:** E1; the Windows GPU row needs E4.

**Files:**
- Create:
  - `spike/s2-render-plan/` (Rust bin `plan-size`)
  - `spike/s3-preview/` (Electron 44.10.0 app; Rust crate `preview-wasm` using wgpu WebGPU backend via `wasm-bindgen`; `audio-worklet.js`; `fixtures/make-fixtures.sh`)

**Interfaces:**
- **S2:** `plan-size --seconds 60 --fps 30 --items 50 --keyframed-params 20 --caption-blocks 36` prints `{ bytes_per_frame_json, bytes_per_frame_bincode, bake_ms_per_frame }`.
- **S3:**
  - Fixtures, generated by `make-fixtures.sh` using the macOS `avconvert`/`AVFoundation` CLI or a Swift script. **No ffmpeg in the repository**; a locally installed ffmpeg may be used for fixtures only and marked `spike-only`.
    - `sync-1080p60.mp4`: H.264, keyframe every 0.5 s, a white flash plus a 1 kHz beep every 2.000 s, 60 s long.
    - `scrub-540p30.mp4`: H.264, keyframe every 0.5 s.
  - Preview loop: WebCodecs `VideoDecoder` → `VideoFrame` → wgpu `Queue::copy_external_image_to_texture`. Also try a raw WebGPU `importExternalTexture` variant in JS, to answer whether zero-copy is possible.
  - Presentation is scheduled against an AudioWorklet clock (audio is master).

- [ ] **Step 1: Write the failing S2 test.** `plan-size` with `--seconds 1 --fps 30 --items 1 --keyframed-params 0 --caption-blocks 0` outputs a `bytes_per_frame_bincode` > 0, and frame 0 and frame 29 differ when one opacity keyframe spans the second. Run with `rtk cargo test --release -p plan-size`. Expected: FAIL.
- [ ] **Step 2: Implement `plan-size`** with a per-frame instruction list: layer id, source frame index, 3×3 transform, opacity, effect params, caption glyph runs. Run the test. Expected: PASS.
- [ ] **Step 3: Measure S2.** Run at the full parameters. Mind-changer threshold, recorded in the result: `bytes_per_frame_bincode ≤ 4096` **and** `bake_ms_per_frame ≤ 0.5`. Write `spike/s2-render-plan/result.json` with `correctness: "verified"` from Step 2.
- [ ] **Step 4: Write the failing S3 sync test.** An automated in-app test detects the flash luminance step in presented frames and the beep onset in the worklet's output buffer, and computes the offset for each of the 30 events. A Vitest/Playwright-Electron test asserts that 30 offsets were detected. It fails before the app exists.
- [ ] **Step 5: Implement the preview app** and make the detection test pass. This makes `correctness: "verified"` possible.
- [ ] **Step 6: Measure S3** on the M4 (AC power), 5 runs:

  | Metric | Threshold |
  |---|---|
  | `fps_sustained_1080p60` | ≥ 58 |
  | `dropped_frames_pct` | ≤ 1 |
  | `av_offset_p95_ms` | ≤ 16.7 |
  | `av_offset_max_ms` | ≤ 33 |
  | `ingest_ms_p95` | ≤ 2 |
  | `zero_copy_available` | recorded (`wgpu` vs raw `importExternalTexture`) |
  | `scrub_time_to_frame_p95_ms` (100 random seeks on `scrub-540p30.mp4`) | ≤ 100 |

  Windows GPU row with E4. M1/8 GB row if the user supplies that machine.
- [ ] **Step 7: Validate both results and commit.** `rtk git commit -m "test(spike): S2 render-plan size and S3 WASM preview"`

### Task 8: S4. WebGPU and WebCodecs in Electron on Linux

**Answers:** spec §12.3 fallback, D19 Linux desktop. **Prerequisite:** E3.

**Files:**
- Create: `spike/s4-linux-gpu/probe.html`, `spike/s4-linux-gpu/run.sh`, `spike/s4-linux-gpu/result.json`
- Reuses the `spike/s3-preview` app unchanged.

**Interfaces:**
- Consumes: the S3 app and thresholds (Task 7).
- Produces rows:
  - `webgpu-default` (no flags)
  - `webgpu-flags` (record the exact switches needed, e.g. `--enable-unsafe-webgpu`, `--enable-features=Vulkan`)
  - `webcodecs-hw-h264` (`VideoDecoder.isConfigSupported({ codec: "avc1.640028", hardwareAcceleration: "prefer-hardware" })` plus observed decoder implementation in `chrome://gpu`)
  - `s3-thresholds`
  - `fallback-server-preview` (server-rendered 540p JPEG/H.264 frames over LAN; threshold ≥ 30 fps, p95 latency ≤ 150 ms)

- [ ] **Step 1: Write the probe test.** `run.sh` launches Electron headful with `probe.html`, which writes a JSON row set. It fails if any row is missing.
- [ ] **Step 2: Run** on the E3 machine for each GPU available (Intel/AMD Mesa, and NVIDIA if present). Record the driver versions in `environment`.
- [ ] **Step 3: Validate and commit.** `rtk git commit -m "test(spike): S4 Linux Electron GPU paths"`

### Task 9: S5. OS codec decode/encode and proxy speed

**Answers:** D13, spec §12.1–12.2, the Windows HEVC mitigation. **Prerequisites:** E1; E2 for CI rows; E3/E4 for hardware rows.

**Files:**
- Create:
  - `spike/s5-codecs/` (Rust bin `codec-probe`), with modules `mac.rs` (`objc2-video-toolbox` / `objc2-av-foundation`), `win.rs` (`windows` crate, Media Foundation), `linux.rs` (VA-API via the `libva`/`cros-libva` crate, NVDEC/NVENC via `nvidia-video-codec-sdk` when present), `sw.rs` (dav1d + SVT-AV1 + libvpx via their Rust bindings, BSD-licensed)
  - `spike/s5-codecs/fixtures/README.md` (sources: iPhone HEVC 10-bit HDR clip, H.264 1080p30, VFR screen recording; all user-supplied or self-recorded)

**Interfaces:**
- `codec-probe decode <file> --frames 600` → `{ backend, hardware: bool, fps }`.
- `codec-probe encode-proxy <file> --height 540 --gop-seconds 0.5` → `{ backend, hardware: bool, realtime_factor }`.
- `codec-probe caps` → codec capability report JSON (the D13 capability report prototype).

- [ ] **Step 1: Write the failing test.** `codec-probe decode` on the H.264 fixture returns 600 frames whose first-frame checksum matches a decoded reference frame taken from the same backend twice. This is determinism, not cross-backend equality. Run with `rtk cargo test --release -p codec-probe`. Expected: FAIL.
- [ ] **Step 2: Implement the backends.** macOS first. Windows and Linux are compiled and run in the CI matrix and on E3/E4.
- [ ] **Step 3: Measure.** Per-OS rows:

  | Row | Threshold |
  |---|---|
  | `decode-h264-1080p` | ≥ 240 fps hardware |
  | `decode-hevc10-hdr` | records `available` and fps; Windows also records whether the HEVC extension is present |
  | `encode-proxy-540p` | ≥ 4× realtime on Apple Silicon; ≥ 2× on Windows/Linux with hardware |
  | `sw-av1-proxy-540p` | ≥ 1× realtime |
  | `vfr-pts-map` | conformed frame count equals expected, and the PTS map round-trips (correctness) |

  Rows that silently used a software path when hardware was expected must report `hardware: false`. That makes them `fail`, not `pass`.
- [ ] **Step 4: Validate and commit.** `rtk git commit -m "test(spike): S5 OS codecs and proxy encode"`

### Task 10: S6. Word-alignment accuracy

**Answers:** spec §13 forced alignment, open question #2. **Prerequisite:** E1.

**Files:**
- Create: `spike/s6-alignment/` containing `dataset.md`, `run_whispercpp.sh`, `align_ctc.py` (spike-only), `run_parakeet.sh` (sherpa-onnx), `score.py`, `result.json`

**Interfaces:**
- **Ground truth:**
  - 200 utterances from LibriSpeech `test-clean` (CC BY 4.0) with Montreal Forced Aligner word alignments (MFA is a spike-only tool).
  - 10 minutes of short-form-style speech supplied by the user, MFA-aligned and spot-checked by hand at 20 words.
  - Record each source's licence in `dataset.md`.
- **Candidates:**
  - (a) whisper.cpp with word-level output (`--max-len 1 --split-on-word`, DTW timestamps enabled);
  - (b) whisper.cpp text + CTC forced alignment with `facebook/wav2vec2-base-960h` (Apache-2.0) exported to ONNX;
  - (c) Parakeet-TDT word timestamps via sherpa-onnx. Record the model licence.
- **Scorer:** `score.py --ref <dir> --hyp <dir>` → `{ wer, start_err_p50_ms, start_err_p95_ms, end_err_p95_ms, rtf }`.

- [ ] **Step 1: Write the failing scorer test.** `score.py` on a 3-word synthetic pair offset by exactly 40 ms reports `start_err_p50_ms == 40` and `wer == 0`. Run with `rtk python3 -I -m pytest spike/s6-alignment -q`. Expected: FAIL.
- [ ] **Step 2: Implement `score.py`.** Run the test. Expected: PASS.
- [ ] **Step 3: Run all three candidates** on both sets on the M4. Rows per candidate per set. Thresholds: `start_err_p95_ms ≤ 50`, `end_err_p95_ms ≤ 50`, `rtf ≤ 0.2` (transcription + alignment combined).
- [ ] **Step 4: Record the recommended candidate** in `notes` (best p95 that meets RTF, licence-clean), validate, and commit with `rtk git commit -m "test(spike): S6 word alignment accuracy"`.

### Task 11: S7. Agent CLI control (Claude Code stream-JSON, bundled Codex app-server)

**Answers:** D28, spec §8.2. **Prerequisite:** E7.

**Files:**
- Create:
  - `spike/s7-agents/claude-driver.ts` (spawns the user-installed `claude`)
  - `spike/s7-agents/codex-driver.ts` (uses the Codex JSON-RPC client copied from `/tmp/t3research/repo/packages/effect-codex-app-server` at its current commit; record the commit and copied paths in `spike/s7-agents/VENDORED.md`)
  - `spike/s7-agents/scenario.md`, `spike/s7-agents/drivers.test.ts`

**Interfaces:**
- Produces: a normalised event log `events.jsonl` per run, using the T3 vocabulary (`message`, `reasoning`, `tool_call`, `tool_result`, `approval_request`, `turn_end`, `usage`). This is reference input for sub-project 7.

- [ ] **Step 1: Write the failing scenario tests.** For each driver, a temp project dir with `workspace/` and a prompt that asks the agent to create `workspace/hello.svg` and then edit `.finisher/x`. Assert, in order:
  1. a streamed assistant message arrives;
  2. an approval/permission request for the file write is intercepted by the driver;
  3. answering *deny* produces a tool-denied result;
  4. answering *allow* on the workspace write creates the file;
  5. `interrupt` during a running tool ends the turn within 5 s;
  6. resuming by session/thread ID continues with prior context (the agent recalls a token given in turn 1).
- [ ] **Step 2: Run them.** `rtk pnpm --dir spike/s7-agents vitest run`. Expected: FAIL.
- [ ] **Step 3: Implement both drivers.**
  - **Claude:** `claude -p --input-format stream-json --output-format stream-json --verbose`, with permission handling through the CLI's permission-prompt mechanism (`--permission-prompt-tool`). Discover the exact control-message shapes from `claude --help` and the CLI's stream output, and record them in `scenario.md`.
  - **Codex:** `codex app-server` (the installed binary stands in for the bundled one).
- [ ] **Step 4: Run them.** Same command. Expected: all six checks per driver PASS. Rows: `claude-*`, `codex-*`. Any check that needed an undocumented workaround is noted.
- [ ] **Step 5: Validate and commit.** `rtk git commit -m "test(spike): S7 agent CLI control"`

### Task 12: S8. OS sandbox feasibility

**Answers:** spec §8.4. **Prerequisites:** E1, E7 (macOS); E2 for Linux and Windows CI rows.

**Files:**
- Create: `spike/s8-sandbox/` with `macos/finisher-agent.sb` (Seatbelt profile), `linux/` (Rust bin `landlock-run`, `landlock` crate), `windows/` (Rust bin `appcontainer-run`, `windows` crate), `escape-tests.sh` / `escape-tests.ps1`, `result.json`
- Reference only: OpenAI Codex's own Seatbelt and Windows sandbox implementations in `openai/codex` (Apache-2.0). Record any copied snippets in `VENDORED.md`.

**Interfaces:**
- Launcher contract on all OSes: `<launcher> --write <workspace> --read <project> --deny <project>/.finisher --deny <project>/assets --allow-net -- <cmd...>`.

- [ ] **Step 1: Write the failing escape tests.** Under the launcher:
  - writing `workspace/ok.txt` succeeds;
  - writing `.finisher/x` fails;
  - `cp` into `assets/` fails;
  - a symlink in `workspace/` pointing into `.finisher/`: writing through it fails;
  - a hardlink attempt into `.finisher/` fails;
  - reading `project/finisher.project.json` succeeds;
  - an HTTPS GET to `api.anthropic.com` succeeds.
- [ ] **Step 2: Run them.** `rtk bash spike/s8-sandbox/escape-tests.sh`. Expected: FAIL (no launcher).
- [ ] **Step 3: Implement the three launchers.**
- [ ] **Step 4: Measure.** Rows per OS:
  - `escapes-blocked` (all 7 checks);
  - `claude-runs-sandboxed` (macOS: the S7 Claude scenario passes under the launcher);
  - `codex-runs-sandboxed`;
  - `node-runs-sandboxed` (Windows: a Node child runs inside the AppContainer).

  Pass requires a denial actually observed in each blocked check (the error code is recorded).
- [ ] **Step 5: Validate and commit.** `rtk git commit -m "test(spike): S8 OS sandbox launchers"`

### Task 13: S9. Signed and notarised bundle

**Answers:** spec §23 artifacts, D32. **Prerequisites:** E8 (macOS); E9 + E2 (Windows); E2 (Linux AppImage).

**Files:**
- Create: `spike/s9-packaging/` containing:
  - an Electron 44.10.0 app with electron-builder config;
  - `server/index.mjs`, a stub `finisherd` that loads the S1 `.node`, spawns the S5 `codec-probe caps` worker, loads ONNX Runtime (`onnxruntime-node`, MIT) and runs `codex --version` from a bundled Codex binary;
  - `entitlements.mac.plist`, `verify-mac.sh`, `verify-win.ps1`, `result.json`

**Interfaces:**
- The stub server is launched by Electron main as a child with `ELECTRON_RUN_AS_NODE=1`, using T3's backend-manager pattern (JSON bootstrap over stdin).

- [ ] **Step 1: Write the failing verify scripts.**
  - `verify-mac.sh`:
    - `codesign --verify --deep --strict` on the `.app`;
    - `codesign -dv` on **every** Mach-O inside (list them with `find … -type f -perm +111` plus `.node`/`.dylib`) shows a Developer ID and the hardened runtime;
    - `spctl -a -vv` is accepted;
    - `stapler validate` passes;
    - launching the app produces a `server-ready` line listing `napi:ok worker:ok onnx:ok codex:ok`.
  - `verify-win.ps1`: `signtool verify /pa` on every `.exe`/`.dll`/`.node`, then the same launch check.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Build and sign.** Notarise with credentials from the `video-creater-notary` Keychain item. Windows uses the self-signed test certificate (record that SmartScreen reputation is untested).
- [ ] **Step 4: Measure.** Rows: `macos-notarised`, `windows-signed-test-cert`, `linux-appimage-launch`. Record the minimum entitlements that were actually required, and the installer sizes (no threshold).
- [ ] **Step 5: Validate and commit.** `rtk git commit -m "test(spike): S9 signed bundle with server, napi, worker, ONNX, Codex"`

### Task 14: S10. NAS and synced-folder storage

**Answers:** D27, spec §6.7. **Prerequisites:** E5, E6.

**Files:**
- Create: `spike/s10-storage/` containing:
  - Rust lib `segstore`: `segment.rs`, `lease.rs`, `import.rs`, `conflicts.rs`;
  - Rust bin `segtorture`;
  - `scan-bench/` (Rust bin `libscan`);
  - `result.json`.

**Interfaces:**
- `segstore::export(db_ops: &[Op], dir: &Path, server_id: &str)`: writes `.finisher/segments/<serverId>-<seq>.seg` via temp file + rename, with a CRC32 trailer.
- `segstore::import(dir) -> Result<Vec<Op>, ImportError>`.
- `segstore::Lease::{acquire(dir, server_id, ttl), heartbeat(), state() -> Fresh | Stale | Conflict}`.
- `conflicts::detect(dir) -> Vec<ConflictFile>`. Recognises at least: Dropbox `"(… conflicted copy …)"`, iCloud `" 2"` suffix duplicates, OneDrive `"-<MachineName>"` suffix, unknown server IDs, duplicate `seq`.
- `libscan <root> --fingerprint sampled` → `{ files, full_scan_s, incremental_scan_s, fingerprint_files_per_s, cpu_pct }`.

- [ ] **Step 1: Write the failing unit tests:**
  - An export/import round trip of 10,000 ops gives an identical state hash.
  - A corrupt segment (flipped byte) gives an `ImportError::Checksum`.
  - A fresh lease held by A means B sees `Fresh` and must open read-only.
  - A lease not heartbeated for 3 × TTL is `Stale`.
  - Each conflict-name fixture is detected.
  - Run with `rtk cargo test --release -p segstore`. Expected: FAIL.
- [ ] **Step 2: Implement `segstore`.** Run the tests. Expected: PASS.
- [ ] **Step 3: Torture runs.** `segtorture` runs on each real location: SMB, NFS, Dropbox, iCloud Drive, OneDrive.
  - Machine A writes 10,000 ops in bursts.
  - Machine B imports, both immediately and **again after a 15-minute settle**, and asserts an identical state hash.
  - Then A and B both try to write at once: assert that exactly one holds the lease and the other is read-only.
  - Then simulate a lost heartbeat and assert takeover only after the stale threshold.
  - Rows per location. Correctness comes from the hash checks.
- [ ] **Step 4: Scan benchmark.** Run on the NAS with a library of at least 100,000 files.

  | Metric | Threshold |
  |---|---|
  | `incremental_scan_s` | ≤ 30 |
  | `cpu_pct` | ≤ 100 (one core) |
  | `fingerprint_files_per_s` over SMB | ≥ 200 |
  | `full_scan_s` | recorded |

  Record the recommended lease TTL, heartbeat (spec default 15 s) and polling cadence in `notes`.
- [ ] **Step 5: Validate and commit.** `rtk git commit -m "test(spike): S10 NAS and synced-folder storage"`

### Task 15: S11. thorvg in WASM for Lottie template preview

**Answers:** spec §12.5, v1 Lottie templates. **Prerequisite:** E1.

**Files:**
- Create: `spike/s11-lottie/` containing:
  - `templates/`: three test Lotties authored for the spike (lower-third with a text slot, animated title, logo sting), or taken from thorvg's MIT test resources; record the source;
  - `wasm-bench.html` using `@lottiefiles/dotlottie-web` (MIT, thorvg-in-WASM);
  - `native-render/`: Rust bin using the legacy vendored `dotlottie-rs` from `/Volumes/Storage/Projects/video-creater/src-tauri/vendor/dotlottie-rs`, referenced by path and not copied;
  - `compare.mjs`, `result.json`.

**Interfaces:**
- Both renderers write 1080p RGBA PNGs for frames 0, 15, 30, 45 and 60.
- `compare.mjs` reports PSNR per frame.

- [ ] **Step 1: Write the failing compare test.** `compare.mjs` on two identical PNGs reports PSNR `Infinity` and on a 1-pixel change reports a finite value. Run with `rtk node --test spike/s11-lottie`. Expected: FAIL. Then implement and PASS.
- [ ] **Step 2: Measure** in the S3 Electron shell on the M4:

  | Metric | Threshold |
  |---|---|
  | `wasm_render_ms_p95_1080p` per template | ≤ 8 |
  | `psnr_min_db` WASM vs native | ≥ 35 (`correctness: "verified"` when met on all frames) |

- [ ] **Step 3: Validate and commit.** `rtk git commit -m "test(spike): S11 thorvg WASM vs native"`

---

# Part C: Decision (permanent, merged to `main`)

### Task 16: Spike report and spec amendments

**Files:**
- Create: `docs/spike/2026-11-spike-report.md` on a `docs/spike-report` branch from `main`
- Modify: `docs/specs/2026-11-01-finisher-architecture-design.md` (only the sections a result overturns or refines)
- Modify: `TASKS.md`

**Interfaces:**
- Consumes: every `spike/s*/result.json` (copied into `docs/spike/results/` so the evidence lives on `main` while the spike code stays on its branch).

- [ ] **Step 1: Validate every result.** `rtk sh -c 'for f in spike/s*/result.json; do node spike/harness/validate-result.mjs "$f" || exit 1; done'` on the `spike` branch. Expected: exit 0.
- [ ] **Step 2: Write the report.**
  - A table of every experiment and row, with verdict, key metrics and `blockedBy`.
  - Then one decision paragraph each:

    | Decision | Driven by |
    |---|---|
    | D9/D24 (napi vs sidecar vs TS model) | S1, S2 |
    | D11 compositor path and zero-copy | S3, S4 |
    | D13 codec plan per OS, and the capability-report contents | S5 |
    | Aligner per language | S6 |
    | Agent drivers | S7 |
    | Sandbox per OS | S8 |
    | Packaging and entitlements | S9 |
    | Storage defaults (TTL, heartbeat, polling) | S10 |
    | Lottie preview | S11 |

  - Any `inconclusive` row lists the prerequisite needed and whether it blocks sub-project 2.
- [ ] **Step 3: Amend the spec** where results differ from it. Each amendment cites the result file. Bump the spec status line to "revision 3 (post-spike)".
- [ ] **Step 4: Commit and merge.** `rtk git commit -m "docs: add spike report and post-spike spec amendments"`, then fast-forward `main` after the user reviews the report. The `spike` branch is kept, unmerged, as evidence.
