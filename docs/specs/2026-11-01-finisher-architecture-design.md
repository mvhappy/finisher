# Finisher — Architecture Design

- **Status:** revision 3 (post-spike). Incorporates the Fable review (B1–B5, I1–I15), the NAS/cloud-folder requirement and the spike results (§27).
- **Date:** 2026-11-02
- **Scope:** Top-level architecture for the Finisher rewrite. Every numbered sub-project in §24 gets its own spec → plan → implementation cycle. This document fixes the boundaries between them.
- **Predecessor:** `video-creater` (working name, now the legacy reference app).
- **Amended 2026-11-07:** the owner decisions for sub-projects 5–8 (each sub-project spec's "Owner decisions (2026-11-07)" section) change D7, D8, D13, D14, D28, D30, D31 and D34 and add D35 and D36; §8, §12.1, §14, §15, §16, §18, §21, §21.1 and §26 follow them. **Follow-up decisions of the same day** (local models for the built-in agent, Landlock-disabled hosts, proxy encoders, the desktop shell's ffmpeg) change D10, D13, D30 and D36 and §8.2, §8.4 and §18.

---

## 1. Product

Finisher is a commercial, agent-first video editor for creators. It ships to external users from day one, has an open MIT core, and is designed for optional paid hosted services.

**First excellent use case:** short-form social video (TikTok / Reels / Shorts).

**Signature workflow:** point Finisher at a media library (local disk, NAS or cloud-synced folder) and let agents "cook" videos from it. The agents index the footage, find moments, build candidate edits on branches in parallel, self-review them, and hand you finished drafts.

**First-release (v1) features**
1. Media libraries on local disk, NAS and cloud-synced folders, with background indexing (§6.7, §13).
2. Transcript editing with styled, animated, word-accurate captions.
3. Long → shorts clip finder, using transcript, scenes, loudness and semantic search.
4. Auto-reframe 16:9 → 9:16 with subject tracking.
5. Lottie graphics from a **template library with slots**.
6. Reviewer: deterministic QA gate, critic loop (optional per run), review markers, shareable review page.
7. Clients: Electron desktop (macOS, Windows, Linux) and web client; headless server on all three OSes.
8. Agents: **Codex (bundled)** and **Claude Code (user-installed)**, plus the built-in agent loop on user API keys.
9. Plugins: full plugin system, with bundled plugins plus local-folder registries (a git registry is a local folder the user pulls).

**Deferred after v1:**
- public signed registry (the protocol is designed in v1);
- raw agent-authored Lottie;
- background-removal plugin (the `mask` primitive ships in v1);
- mobile companion;
- hosted cloud;
- real-time multi-user co-editing (interfaces prepared);
- a Finisher-built FFmpeg plugin (interface only).

**Reference products**
- **T3 Code:** server/client split, pairing, environments, cloud accounts, conversation rendering. Its architecture is mirrored (§16).
- **Uncut:** independent critic on rendered frames, capped rounds, review page with timecode comments, agent skills and rubrics. Borrowed under its MIT licence.
- **Palmier Pro:** review markers (`open/review/resolved`); agent operating a real timeline.
- **CapCut:** frontend UX/UI quality, magnetic timeline, caption and template ergonomics.

---

## 2. Decision log

| # | Area | Decision |
|---|---|---|
| D1 | Audience | Product for creators from day one; paid hosted features later |
| D2 | Content focus | Short-form social first |
| D3 | Codebase | New repo (this one); legacy `video-creater` stays in place as read-only salvage reference |
| D4 | Server platforms | macOS, Windows, Linux (including NAS/Docker) |
| D5 | Collaboration | Async sharing first; foundation for multi-user editing |
| D6 | Deployment | Local by default (embedded in desktop); remote/headless optional |
| D7 | Agents | Agent CLIs as providers: Codex bundled (Apache-2.0), Claude Code user-installed (not openly licensed), each signed in by the user through the CLI's own flow; plus a built-in loop on the user's OpenRouter key (D30) |
| D8 | Plugins | First-class, out-of-process, any language, JSON-RPC; registry protocol from day one. Third-party code plugins are allowed from the start, before sub-project 7C's OS sandbox: installing one shows a high-risk warning, its permissions need a person's explicit approval, and nothing auto-updates; the env allow-list, no credentials, brokered egress and the in/out rules apply throughout, and the plugin sandbox profile is enforced automatically once 7C lands (§14.4) |
| D9 | Server language | Hybrid: TypeScript/Effect 4 control plane + Rust edit domain and media |
| D10 | Desktop shell | Electron, **provisionally**: whether it stays (with a documented ffmpeg exception and the open-codecs build), or the desktop app moves to Tauri or the OS webviews or a custom build, is decided in sub-project 9 (§18) |
| D11 | Rendering | One Rust/wgpu compositor, native for export and WASM/WebGPU for preview |
| D12 | Timeline model | Own versioned model, OTIO-shaped, native `.otio` adapters |
| D13 | Codecs | Decode through OS/hardware codecs plus royalty-free software (AV1/VP9; AAC on Linux through symphonia). Export of H.264 only through **OS-vendor encoders** (VideoToolbox; Microsoft's own H.264 encoder MFT), never GPU-vendor encoders. That rule covers **exports** (QA renders and mattes follow it); internal preview proxies, which are never distributed, may use any hardware encoder, GPU-vendor ones included. Elsewhere AV1, or H.264 through the codec plugin. No built-in HEVC export template: HEVC export only through a plugin. AAC export on Linux through vendored vo-aacenc. **No LGPL/GPL in Finisher's code**; Electron's bundled ffmpeg is a **provisional** exception that affects the desktop app only; the desktop shell is decided in sub-project 9 (§18). Patents: counsel (§26) |
| D14 | Agent edits | Live draft branches + review/merge. An `auto` session may merge into `main` only under a per-project owner policy: off by default, enabled by a human-only setting, merging only when the QA gate and the critic pass, every merge audited (§8.3) |
| D15 | State model | Server-authoritative append-only op log with branches |
| D16 | CLI | `fin`: agent-only, a thin client, the primary agent interface; MCP is a thin adapter |
| D17 | Graphics | Lottie templates with slots (v1); raw Lottie later; native caption engine |
| D18 | Parallelism | Multiple agents in parallel on separate branches |
| D19 | Clients | Electron + web (v1); mobile later |
| D20 | Licence | MIT (core, SDKs, protocol) |
| D21 | Cloud code | Private `finisher-cloud` repo; additive only |
| D22 | Agent artifacts | Agents write to `workspace/`; canonical state changes only via ops |
| D23 | T3 Code reuse | Reference + frozen vendored slices, not a fork |
| D24 | Rust/TS boundary | Rust owns the edit domain; TS owns orchestration and non-edit state; plugins cannot add primitive ops |
| D25 | Timeline UI | `dnd-timeline` + `@dnd-kit/core` (from legacy) |
| D26 | Skills / critic | Borrowed from Uncut (MIT, attributed) |
| D27 | Storage | NAS and cloud-synced folders are first-class for media and projects; SQLite never lives on network or synced storage (§6.7) |
| D28 | Providers v1 | Claude Code via the user-installed `claude` CLI in stream-JSON mode (Agent SDK not bundled); Codex app-server bundled, T3's client vendored. The user signs in to each unmodified CLI through the CLI's own flow (subscription or API key); Finisher never copies, stores or intermediates CLI credentials (§8.2) |
| D29 | Library indexing | Semantic embeddings run locally by default (SigLIP2 via ONNX); cloud embeddings (Gemini/Qwen) are opt-in per library |
| D30 | Built-in agent APIs | **One OpenAI-compatible adapter** in v1, with two presets (owner decision, follow-up of 2026-11-07): **OpenRouter**, on a key the user pastes (the only agent-provider credential Finisher stores; the fal.ai generation key is the other pasted key, §14.3), and **local** (Ollama, LM Studio or a user-supplied OpenAI-compatible base URL), which needs no API key. Loopback URLs are allowed for the built-in agent only, because it runs in `finisherd` and needs no sandbox egress change. The direct Anthropic (Messages) and OpenAI (Responses) adapters are not in v1 |
| D31 | Minimums | macOS 14+ on Apple Silicon; **Windows 11** x64 with a WebGPU-capable GPU (Windows 10 is not supported); Linux glibc 2.35+ and **kernel 6.2+** (Landlock ABI 3 or later), for desktops and for hosts of the server's Docker image alike: Ubuntu 22.04's stock 5.15 kernel and Debian 12's 6.1 are below it. Intel Macs are not supported (D36) |
| D32 | Distribution | Direct download + auto-update: notarised DMG, signed Windows installer, AppImage/deb/rpm, Docker image for the server. No app stores in v1 |
| D33 | Internationalisation | i18n-ready from the first commit, English-only at launch: every user-facing string is an ICU message in a catalogue, never a literal; translations are added later without code changes (§17.1). UI language and content language (transcripts, captions, generated text) are separate settings |
| D34 | Diagnostics and training data | One versioned event envelope (types in `fin-core`, one closed payload enum, TS generated) written to a local, append-only, size- and age-capped journal in `<server-data>/events/` by `finisherd` only, with content-addressed blobs whose lifecycle follows the entries that reference them. Two purposes with separate consent: **diagnostics** (metadata only, edit outcomes included; on by default everywhere with an opt-out) and **training** (adds user and model content, edit intents and LLM traffic; only after an explicit opt-in by a person, never from `fin`; closed-model outputs may be training targets, every example carries its model provenance, and counsel must clear the first training run and the first upload); a per-project `collect: never` flag in `finisher.project.json` binds every collaborator; env overrides; off means nothing is recorded. Every payload field is classed `meta`, `scrubbed` or `content`; paths, host details and free-form names are never stored raw. Events link to the op log by `txId` and actor. No upload until a later sub-project; the upload view (`export --as-upload`) fixes what it may ship, and the dataset builder runs locally (§21.1) |
| D35 | Node runtime | `finisherd`, `fin` and the repository's tooling target **Node 24**, the major Electron embeds (24.x), so the server the desktop starts as an `ELECTRON_RUN_AS_NODE` child is the server CI tests. Only APIs present in Node 24 are used, `node:sqlite` included. The switch from Node 26 is a TASKS item, with a check that `node:sqlite` and every API in use work on Node 24 |
| D36 | Platform changes (2026-11-07) | **Windows 11 only** (Windows 10 22H2 dropped: sub-project 6 decision 9) and **Linux kernel ≥ 6.2** (sub-project 7 decision 3). Every supported Linux host therefore has Landlock ABI 3; sandbox fallbacks and partial-ABI code paths that existed only for older kernels are gone. A supported kernel with Landlock disabled (not built in, or left out of the boot `lsm=` list) is still supported (owner decision, follow-up of 2026-11-07): **everything runs** (agents, hooks, media workers and plugins), unconfined, each profile with a persistent status warning (`server.status` and `fin status`) and a diagnostics event. Nothing is refused and there is no override setting (sub-project 6, 7 and 8 specs). NAS and cloud folders used as storage (D27) are unaffected; a NAS that runs the server's Docker image must meet the kernel minimum. D31 carries the result |

---

## 3. Naming

| Thing | Name |
|---|---|
| Product | Finisher |
| Server | `finisherd` |
| Agent CLI | `fin` |
| Rust crates | `fin-*` |
| TS packages | `@finisher/*` |
| Project-internal dir | `.finisher/` |
| Cloud repo (private) | `finisher-cloud` |
| Copyleft plugins repo | `finisher-plugins-copyleft` |

---

## 4. System topology

```
 Clients ───────────────────────────────────────────────────────────────────────
  Electron desktop      Web browser        Mobile (later)      fin CLI      MCP adapter
        │                    │                  │                  │            │
        └──── one versioned protocol: Effect RPC over WebSocket + HTTP ────────┘
              (snapshot + sequenced op streams; local socket for fin)
                                     │
 finisherd (Node, TypeScript, Effect 4) ─────────────────────────────────────────
  gateway · auth/pairing · environments · agent runtime · jobs · plugin host ·
  hooks · review · broadcast · library indexer · app store (SQLite, non-edit state)
                                     │ napi-rs (async, off the event loop)
 Rust ───────────────────────────────────────────────────────────────────────────
  fin-node  → fin-core (model, ops, eval, layout, branch, schema) + fin-store
  workers   → fin-codecs, fin-compositor, fin-audio, fin-captions, thorvg, resvg, ONNX
  fin-wasm  → fin-core + fin-compositor + fin-audio for client preview
  fin       → static CLI binary, links fin-core, talks to finisherd over local socket
```

**Rules**
1. Every edit enters through one gateway as an operation. Nothing else writes canonical state.
2. The server is identical everywhere: embedded in desktop, headless on any OS or a NAS, and composed into the private cloud.
3. Crash-prone native work runs out of process, with a hard kill on timeout or cancel.
4. One protocol definition: RPC envelopes in Effect Schema; model, op, diff and error types are owned by `fin-core` and exported as JSON Schema, from which TypeScript types are generated.
5. **The stable contract is the versioned JSON op format plus its JSON Schema**, not a crate or package.
6. Command-pattern permissions (§8.3) and `PreCommand` hooks (§10) apply to the **command registry**, whatever the transport: `fin`, MCP, plugin, UI composer.

---

## 5. Open core and private cloud

| Extension point (open) | Local default | Cloud (private) |
|---|---|---|
| `AuthProvider` / `Identity` | Device pairing, single owner | Accounts, OAuth/SSO, orgs, roles |
| `ProjectStoreBackend` | SQLite in local server data + project segment export (§6.7) | Postgres + object storage, multi-tenant |
| `MediaStorage` | Local disk, NAS, synced folders | S3-compatible, signed URLs, proxy streaming |
| `WorkerPool` | Local workers | Remote GPU fleet |
| `ModelGateway` | User keys, agent CLIs (bundled Codex, user-installed Claude Code) | Managed credits, metering |
| `Metering` / `Entitlements` | No-op | Billing, plan limits |
| `ShareService` | Exported HTML review folder | Hosted review links, team workspaces |
| `ConnectRelay` | None (LAN/Tailscale/SSH routes) | Account-linked relay + managed tunnels |
| Frontend extension slots | Empty | Billing, team, account, share UI |

- Clients discover cloud features as handshake capabilities.
- **The open-repo CI guard fails on imports of cloud-only packages** (an import-graph check, not name matching).
- The cloud repo runs the open repo's test suites against its implementations.

---

## 6. Data model, op log, branches, sync, storage

### 6.1 Project layout

```
MyProject/                      (may live on local disk, NAS, or a cloud-synced folder)
  finisher.project.json         project id, name, schema version, library refs
  .finisher/
    segments/                   immutable op-log segments + snapshots (sync-safe, §6.7)
    lease.json                  writer lease (server id, heartbeat)
  sources/                      optional managed copies of media
  workspace/shared/, workspace/<session-id>/    agent/user scratch (agents read+write)
  assets/                       registered assets (server-written, write-once files)
```

- The **live working database** is on the server's local disk at `<server-data>/projects/<project-id>/project.fdb`, never in the project folder (§6.7).
- Caches, proxies and analysis artifacts are content-addressed in `<server-data>/cache/`. They are regenerable and never synced.

### 6.2 Model

- `Project` → `Timeline`s, `Bin`s, `Asset`s.
- `Timeline` → `Track`s → `Item`s: clip, gap, caption block, Lottie graphic, SVG/image, adjustment layer.
- Items carry typed `Effect`s, a `mask` (§12.6), audio params (§12.7) and `Keyframe` lanes.
- Time is rational.
- **Analysis artifacts** attach to assets and are referenced by ops but are not timeline state: transcripts, alignments, scenes, embeddings, face tracks, loudness, beats. **Mattes are project assets** in `assets/` (owner decision of 2026-11-07, sub-project 8 decision 3; project schema version 3), not regenerable cache, because a matte producer's output may not be reproducible.

### 6.3 Operations

- Each op is typed and versioned.
- Envelope: `opId`, `author`, `branch`, `baseRev`, `txId`, `schemaVersion`.
- Pipeline: validate → apply → append → broadcast. Transactions are atomic.
- Every op defines `invert` and rebase hooks.
- **Primitive ops are defined only in `fin-core`.**
- **Plugin commands are expanded on the server, against a pinned snapshot:**
  1. The host gives the plugin a read snapshot at `rev`.
  2. The plugin returns one transaction tagged with that `baseRev`.
  3. `fin-core` validates it and rebases it, or rejects it with `Conflict`.

### 6.4 Branches and undo

- **Branches:**
  - `main` is the default view.
  - Agent drafts, critic fixes and variants are branches.
  - **Merge** rebases the branch's transactions onto `main` with re-validation. Conflicts become review items.
  - Cherry-pick works per transaction.
  - `checkpoint` names a revision; `try` makes a throwaway sub-branch.
  - *As built (5E):* conflicts become `mergeConflict` review items, a rejection is a durable `drop` record, and a merge can be limited to what was reviewed: control-plane spec §8 (item 25 of §27).
- **Undo appends; the log is never truncated.**
  - Undoing transaction *T* = append the inverse of *T*, rebased onto the current head. If it can't be rebased, the result is a typed `Conflict`.
  - Undo/redo stacks are kept per (author, branch).
  - Stacks are **dropped when the branch is merged**. After a merge, reverting means a new revert transaction on the target branch.
  - `fin undo --to <checkpoint>` = append the inverses of all of that author's later transactions on that branch, newest first.
  - Property tests cover undo with interleaved authors, after rebase and after cherry-pick (§22).

### 6.5 Concurrency

- Every write carries `baseRev`. The server auto-rebases when that is safe, otherwise returns `Conflict`.
- **One writer per project:** exactly one `finisherd` holds a project's writer lease (§6.7). All writers go through it.
- Other servers can open the project read-only.

### 6.6 Live sync

- Clients subscribe to `(project, branch)` and receive a snapshot at sequence *N*, then **committed ops** with sequence > *N*.
- Clients replay those ops through `fin-wasm`. Diffs from `fin-node.apply` serve only TS-side consumers (review, hooks, notifications).
- Binary data moves on separate channels, referenced by hash.
- Snapshots and ops are versioned; migrations are tested functions.

### 6.7 Storage locations: local, NAS, cloud-synced (first-class)

**Rule:** SQLite and OS file locks are never used on network or synced storage. Everything written to such storage is written once and immutable, by temp-file-then-rename, so SMB/NFS and Dropbox/iCloud/Google Drive/OneDrive sync handle it correctly.

**Media libraries**
- A **library** is a registered root on local disk, NAS (SMB/NFS mount or UNC path) or a synced folder. Libraries are read-only to Finisher by default.
- **Change detection:** native file-system events on local disks; on network and synced roots, periodic scans with configurable cadence (FS events are unreliable on SMB/NFS).
- **Identity without reading terabytes:** a fast fingerprint (size + mtime + sampled head/middle/tail hash), upgraded to a full hash lazily when an asset is used in a project.
- **Background indexing jobs** (throttled, resumable, pausable, bandwidth-aware): probe, then proxy, then transcription and alignment, then scenes, then embeddings.
  - Results live in the server's local cache, keyed by fingerprint.
  - The whole library becomes searchable by agents (`fin search`, `fin library …`).
- **Proxies are always cached locally**, so editing and preview never stream originals over the network. Final export reads originals.
- **Offline libraries:** when a NAS is unreachable, the cached proxies and analysis keep working for editing and preview. Export waits and reports exactly which originals are missing.
- **Remote media:** a remote `finisherd` (for example one on the NAS) reads libraries it can mount. Client-local files reach a remote server through **remote ingest**: chunked, resumable upload into `sources/` of a server-side project.

**Server data is local (as built, review #4):** the server-data root and every project's data folder (`<server-data>/projects/<id>/`, the `project.fdb` with its WAL and OS locks) must be on a known local filesystem, judged where the path really lands (symlinks and ancestors resolved; the nearest existing ancestor for a path not made yet). Known local: Linux ext2/3/4, XFS, Btrfs, ZFS, F2FS, bcachefs, tmpfs, ramfs; macOS APFS, HFS+; Windows NTFS, ReFS on a fixed drive; and in each case not a sync client's folder. Anything else is `SERVER_DATA_NOT_LOCAL` (details `path`, `resolved`, `filesystem`, `override`), including a container's overlayfs, which hides the disk under it: a Docker `finisherd` keeps its server data on a mounted volume. `FIN_ALLOW_NONLOCAL_SERVER_DATA=1` accepts any filesystem at the person's risk. `finisherd` checks the root at every start before it creates anything (fin-node `checkServerDataLocal`); `Store::create` and `Store::open` check the project's data folder before writing (`fin_store::locality`; `OpenOptions::allow_nonlocal` for tests). The restart shortcut's own local-disk test (which accepts overlayfs: OS locks work there between processes of one host) is unchanged.

**Projects in synced or network folders**
- The working `project.fdb` stays on the server's local disk.
- The project folder receives a **sync-safe export**:
  - immutable op-log **segment files** `.finisher/segments/<serverId>-<seq>.seg`, appended every few seconds and at transaction boundaries;
  - periodic **snapshot files**;
  - `finisher.project.json`.
- **Opening a project:** the server compares segments in the folder with its local database. It imports any segments it lacks (for example ones written by another machine) and rebuilds or catches up the database. A project folder can therefore move between machines or servers by syncing alone.
- **Single writer without OS locks:** `.finisher/lease.json` holds the server ID and a heartbeat, refreshed every 15 s.
  - Another server that finds a fresh lease opens **read-only** and offers "take over", which requires the lease to be stale or the user to confirm.
  - Sync conflict copies of the lease or segments are detected (unknown server ID, duplicate seq). The project goes read-only with a clear recovery prompt; it is never silently merged.
- `assets/` and `workspace/` work normally on network and synced storage: write-once files, temp-then-rename. `workspace/` scanning uses polling on network roots.

**Server on the NAS**
- `finisherd` runs headless in Docker on a Linux NAS (Synology, TrueNAS, Unraid) or any server, with the NAS libraries mounted locally.
- Agents and jobs run next to the media.
- Hardware acceleration comes from the NAS GPU if one exists; otherwise jobs can be routed to a paired workstation's worker pool (later; `WorkerPool`).

### 6.8 Op-log maintenance, backup, recovery

- **Snapshots** every N ops or minutes per branch.
- **Branch archiving:** rejected or abandoned branches are squashed to a summary and archived after a configurable period. History on `main` is kept.
- **Compaction** folds old segments into a snapshot plus a tail. Exported segments are compacted the same way, with old segment files removed only after the new snapshot is synced (two-phase).
- **Backups:** `VACUUM INTO` local backups on a schedule, plus the segment export, which is itself an off-machine backup when the folder is on a NAS or synced.
- **Integrity:** checksums on segments; database integrity check on open; automatic rebuild from segments if the local database is damaged.
- The atomic-save, journal and recovery ideas from the legacy app are ported explicitly (§25).

---

## 7. Rust / TypeScript boundary

| Crate | Responsibility |
|---|---|
| `fin-core` | Pure, no I/O, WASM-compatible. `model/`, `ops/` (one module per op: schema, validate, apply, invert, rebase), `eval/` (rational time, keyframes, easing, params, nesting, audio params), `layout/` (caption measurement; fonts injected), `branch/` (rebase, undo, conflicts), `schema/` (JSON Schema export) |
| `fin-store` | rusqlite op log on local disk, snapshots, segment export/import, lease, compaction |
| `fin-node` | napi-rs: `open`, `apply(baseRev, tx) → diff \| error`, `rebase`, `undo`, `view(budget)`, `snapshot`, `importSegments`. Async on a thread pool. |
| `fin-wasm` | `fin-core` + `fin-compositor` + `fin-audio` for clients (no store) |
| `fin-compositor` | wgpu video render graph (native + WASM) |
| `fin-audio` | Audio mixer (native + WASM), §12.7 |
| `fin-codecs` | OS/hardware codecs + process-based backend trait |
| `fin-captions` | Caption rasterisation |
| `fin-jobs` | The job envelope (NDJSON protocol types, Rust → JSON Schema → generated TS) and the worker and helper runtimes every family links, with host calls and the self-sandbox entry; creates no process (sub-project 5C-1) |
| `fin-jobhost` | The host side of the job system (an rlib linked into `fin-node` as its `jobhost` module): spawning behind the `Spawner` hook, host-owned kill domains on every OS, framing, timers, memory limits, the Unix watchdog client (sub-project 5C-1) |
| `fin-workers` | The one worker binary, `fin-workers <family> [--warm]`, found beside the `fin-node` `.node` file; 5C-1 ships the `test` family (behind `test-hooks`), the Unix `watchdog` and the Windows `--spawn-broker`; sub-projects 5D, 6 and 8C add the `library`, `media` and `analyze` families (decode, encode, render, Lottie, SVG, analysis); a worker never starts a process |
| `fin` | Agent CLI; links `fin-core` |

- **TypeScript (Effect 4)** owns gateway, contracts, auth, environments, agent runtime, jobs, library indexer, plugin host, hooks, review workflow, broadcast and settings. It has its own SQLite for non-edit state, on local disk.
- **Model types are generated** into `@finisher/core-types`, and CI fails on drift.
- **Boundary rules:**
  - TS touches project data only through `fin-node`. Small payloads go in, compact diffs come out.
  - Long calls are async; a lint and a test catch synchronous calls on the event loop.
  - Rust detects conflicts; TS turns them into review items.
- **Build risk:** the napi + Electron prebuild matrix is **not** proven in T3's code (T3's Rust crates are plain sidecar binaries, not napi). It is validated in the spike (§24 #1).
  - **Fallback if napi is problematic:** run `fin-node` as a sidecar process over a local socket with the same API. This costs some latency but changes no domain code.

---

## 8. Agent runtime

### 8.1 Model

- T3's vocabulary: `Thread → Run → Node` (message, reasoning, plan, tool call/result, approval, subagent, usage).
- **Session** = one agent on one branch: provider, model, branch, scope, mode, budget. Sessions are persisted, become `interrupted` on crash, and can be resumed from any device.
- **Subagents** run on child branches. The UI shows the tree.

### 8.2 Providers

- **Adapter contract:** forked from T3's `provider-core` `ProviderAdapterV2` (openSession, ensureThread, startTurn, steer, interrupt, rollback→branch checkpoint, fork, runtime-request responses → `Stream<Event>`).
- **Claude Code (v1):** drives the **user-installed `claude` CLI** in stream-JSON mode (`--input-format stream-json --output-format stream-json`).
  - The Agent SDK is not bundled. Its npm licence is "SEE LICENSE IN README.md", not an open licence.
  - T3's Claude adapter is a reference for event mapping only.
  - Detection, version check and first-run guidance are in onboarding (§21).
  - **Sign-in (owner decision of 2026-11-07):** the user signs in to the unmodified CLI themselves, with the CLI's own flow. Finisher never runs that flow on the user's behalf (it only prints the command), and never opens, copies, stores or proxies the credentials. The user signs in once per CLI in a Finisher sign-in home under server data; every session shares that one sign-in file in place, and everything else (settings, transcripts, hooks, `projects/`) is per session. Trade-off: any session of that CLI can read or swap the shared sign-in file, so an account change stops new sessions until a person confirms (sub-project 7 spec §8.6).
  - **Risk:** permission callbacks and other control requests must be implemented against the CLI protocol. The spike validates this (§24 #1).
- **Built-in agent (v1):** Finisher's own loop, emitting the same events, on **one OpenAI-compatible adapter** (D30) with two presets: **OpenRouter**, on the key the user pastes (the only agent-provider credential Finisher stores), or a **local** OpenAI-compatible server (Ollama, LM Studio, a user-supplied base URL) that needs no key and may be on loopback, because the loop runs in `finisherd`, not in a sandbox; CLI sign-ins stay with the CLIs. Direct Anthropic and OpenAI adapters are not in v1. More can come via plugins.
- **Codex (v1, bundled):** `codex` app-server shipped with Finisher (Apache-2.0, verified at repo level on 2026-11-02; re-check binary dependencies of the pinned release at bundling time). The user signs in to it with their own OpenAI account or API key, through Codex's own flow; Finisher does not hold or relay that credential. The JSON-RPC client is vendored from T3's `effect-codex-app-server` and pinned to the bundled Codex version, so protocol drift is caught at upgrade time. A user-installed newer `codex` may be selected in Settings.
- More providers come via plugins.

### 8.3 Roles, modes, permissions

- **Roles:** name, prompt, model preference, allowed commands, budget. Examples: `critic`, `clip-finder`, `caption-stylist`, `colourist`, `mixer`. Plugins can add roles.
- **Modes:**
  - `plan`: read-only.
  - `draft`: own branch only.
  - `auto`: also renders and runs the critic within budget. It may merge into `main` only where the project's owner policy allows it (D14): off by default, switched on by a human-only setting, and only when the QA gate and the critic both pass; each such merge is audited.
- **Rules:** allow/ask/deny on command patterns, enforced in the command registry for every transport.
- **Layered settings:** `managed` > `project` > `user` > `local`.
- **Plugin installation is human-only by default.** `fin plugin install` from an agent is `deny` unless the user explicitly allows installs from already-trusted registries. Agents may `search`/`info` and *suggest* a plugin, which appears as a UI suggestion and not as an approval card worded by the agent.

### 8.4 Sandbox: OS-enforced, not just policy

- The working directory is `workspace/<session-id>/`.
- **The agent process tree runs inside an OS sandbox** applied by `finisherd`:

  | OS | Mechanism | Rules |
  |---|---|---|
  | macOS | Seatbelt profile | writable `workspace/` + temp + the session's own provider config dir + the CLI's shared sign-in file (never its folder); read-only project; deny `.finisher/` and `assets/` |
  | Linux | Landlock + seccomp (kernel ≥ 6.2, D31; where Landlock is disabled everything runs unconfined with a status warning, D36) | same |
  | Windows | AppContainer with ACLs on `workspace/` | same |

  *As built (7C): sub-project 7 spec §25 and item 22 of §27.* No bubblewrap (LGPL; sub-project 7 spec §8.3). The seccomp half is deferred; agents and hooks get a user and mount namespace that hides the usual socket folders instead.

- Provider permission rules (Claude Code allow/deny, Codex `workspace-write`) are a second layer, not the guarantee.
- **Integrity regardless of sandbox:** assets are referenced by content hash, and the server verifies the hash whenever an asset is used. `.finisher/` and `assets/` are server-written only, so tampering is detected and rejected.
- **Environment allow-list:** `PATH`, `HOME` (or a per-session home), `LANG`/`LC_*`, `TMPDIR`, the provider's own config/auth dir variables, `FIN_SERVER`, `FIN_SESSION`, `FIN_CREDENTIAL` (the path of the session's credential file, never the secret itself; sub-project 5 spec §4.1). Everything else is removed.
- **Token delivery:** `FIN_TOKEN` is **not** in the environment. `fin` obtains a scoped credential from a per-session 0600 credential file inside the session sandbox, or from peer-credential checks on the local socket. Subprocesses cannot inherit it from the environment.

### 8.5 Workspace artifacts

- Agents write SVG, PNG, Lottie JSON, downloads, scripts and notes under `workspace/`.
- `fin asset add` hashes the file, copies it into `assets/` (write-once) and records `asset.add`. `fin asset update` creates a new version.
- **Validation:**
  - SVG: sanitised, rendered by resvg.
  - Lottie: validated against the template-slot schema in v1. Raw Lottie comes after v1.
  - Media: probed, then proxied.
  - Limits on size and type.
- **Provenance:** author, prompt, tool; for downloads, source URL and licence. QA flags assets with unknown licence.
- `finisherd` shows unregistered files live in a drafts bin. Network roots are polled.
- `fin workspace gc` deletes only after user confirmation.

### 8.6 Approvals, cost, saved prompts

- Approval cards can be answered from any device.
- Budgets cover tokens, money and render minutes.
- Cancel keeps the branch.
- **Saved prompts** (`/make-shorts 5`) are the human shortcuts; the CLI stays agent-only.

---

## 9. `fin` — the agent-only CLI

1. **Discovery:**
   - `fin` alone prints a map of under 1.5k tokens.
   - `fin help <group>`, `fin schema <cmd>`.
   - `fin guide <task>`: skills versioned with the server.
2. **Addressing:**
   - words: `w:182..w:240`, `--quote`
   - entities: `c12`, `t1`, `m3`, `s7`, `face:2`
   - relative and time: `after:c12`, `+2s`, `12.4s`, `00:12.4`, `f300`
   - handles: `r:3`
   - Resolved values are always echoed. A keyed retry, a lost answer found again or a rebased commit echoes what the commit made; a value the commit cannot tell (an edge once later edits moved it) is `null`, never a fresh plan's.
3. **Views:**
   - `fin view` (screenplay-style, `--budget`, `--range`)
   - `fin status`
   - `fin context`
   - `fin library ls|search|status` for indexed libraries
4. **Writes return deltas only.**
5. **Declarative edits:** `fin apply <spec>`; paper edits from quotes; `--dry-run` everywhere.
6. **Perception:** `fin look --at` (frame with safe zones and caption boxes), `fin look --range --grid` (contact sheet), `fin listen` (text audio summary).
7. **Errors:** `code`, `message`, alternatives, `fix`. No prompts, pagers or colour. stdout = data, stderr = logs. Stable exit codes.
8. **Experiments:** `fin checkpoint`, `fin undo [--to]` (append-only semantics, §6.4), `fin try`, `--idem-key`, `fin note`.
9. **Jobs:** `render`, `analyze`, `transcribe`, `gen` return job IDs; `fin wait`; `fin check` runs QA.
10. **Speed:** static binary, local socket, `fin batch` (JSONL), `fin tx begin|commit`, offline validation via the linked `fin-core`.
11. **Domain commands:** `fin asset`, `fin preset`, `fin gen`, `fin search`, `fin plugin search|info` (install is human-only by default).
12. **One registry:** MCP tools and plugin command groups are generated into help, schema and MCP.
13. **Eval harness** scores CLI changes (§22).

---

## 10. Hooks

| Event | Default capabilities |
|---|---|
| `SessionStart` / `SessionEnd` | add context, log |
| `UserPrompt` | add context, block |
| `PreCommand` / `PostCommand` | block with reason, add context |
| `PreOp` / `PostOp` | veto, annotate, trigger follow-ups |
| `BranchReady` | extra QA, gate the critic |
| `PreMerge` / `PostMerge` | enforce policy, notify |
| `PreRender` / `PostRender`, `JobDone` | validate, publish, notify |
| `Approval`, `Notification` | route to phone, Slack, etc. |

- **Replies:** `allow | block | context` by default.
- **`modify`** (rewriting a prompt, command arguments or an op) requires an explicit `hooks:modify` permission granted per hook. The original and modified payloads are both logged and shown in the session timeline.
- **Handlers:** commands (JSON on stdin) or plugin RPC methods. Timeouts and fixed ordering apply.
- **Every hook command runs with the same cleared environment, sandbox and timeouts**, whatever its source: user config, project config or plugin.
- **Project-config hooks stay disabled until the user trusts the project.**

---

## 11. Reviewer

1. **QA gate:**
   - black or frozen frames
   - audio gaps, clipping, loudness against target (§12.7)
   - caption safe zones, overlaps, timing and **alignment drift** (§13)
   - cuts in the middle of a word
   - export spec
   - unknown-licence assets
   - QA profiles are presets.
2. **Critic** (optional per run; on by default in `auto` mode):
   - a read-only session with sampled frames, contact sheet, transcript, QA report and brief;
   - rubrics borrowed from Uncut (intent, stills, motion/jitter, legibility, colour, mix, beat sync) plus hook strength and pacing;
   - fixes come as a child branch; rounds are capped (default 3); a summary lists what is still open.
3. **Markers** (`open/review/resolved`), per-transaction change list with accept/reject, A/B compare. *As built (5E):* the change list, per-transaction decisions, durable rejections and reviewed-only merges are control-plane spec §8; markers and A/B compare are not built.
4. **Share page:** exported HTML with timecode comments that come back as markers. Hosted links come later through `ShareService`.

---

## 12. Media engine

### 12.1 Codecs

- **Traits:** `Decoder`, `Encoder`, `Demuxer`, `Muxer`.
- **Backends:**
  - macOS: VideoToolbox / AVFoundation
  - Windows: Media Foundation
  - Linux: VA-API (decode; encode for proxies only); NVIDIA on Linux arrives in 6G
  - software: dav1d, SVT-AV1, libvpx; on Linux, AAC decode through symphonia and AAC export through vendored vo-aacenc
- Muxing is in Rust.
- **Export encoders (owner decision of 2026-11-07, sub-project 6 decision 9):** H.264 export uses OS-vendor encoders only (VideoToolbox on macOS, Microsoft's own H.264 encoder MFT on Windows), never a GPU vendor's encoder (NVENC, AMD's or Intel's MFTs, VA-API drivers) even where present. GPU vendors' encoders serve proxies only, which never leave the cache; QA renders and mattes follow the export rule. Elsewhere, export is AV1, or H.264 through the codec plugin, so Linux has no built-in H.264 export. There is no built-in HEVC encoding or HEVC export template; HEVC export comes only through the codec plugin.
- No software H.264/HEVC encoders are shipped.
- **Process-based backend trait** for plugin codecs (FFmpeg plugin interface plus conformance suite).
- **Capability report** per machine drives presets and import decisions.
- **Windows HEVC (iPhone footage) mitigation:**
  - Import detects missing HEVC decode and shows first-run guidance: Microsoft's HEVC Video Extensions, or hardware decode if the GPU offers it.
  - The user-installed copyleft FFmpeg codec plugin is offered as a prominent alternative.
  - Affected files remain usable for analysis only if decodable. Otherwise they are listed with a fix.
- **The same mitigation pattern applies on Linux without VA-API/NVDEC.**

### 12.2 Proxies

- **Format:** short-GOP proxies (keyframe every ≤ 0.5 s; all-intra for the scrub tier when the encoder allows) for random access.
- **Resolution ladder:** 540p (scrub and remote clients), 1080p (default preview), originals for export.
- **Encoder fallback order:** OS/hardware H.264 → hardware AV1 → SVT-AV1 (software, slowest; reported in the capability report).
- **VFR sources** are conformed to CFR proxies, and a PTS map is kept so export samples originals correctly.
- Proxies, PCM, waveforms, thumbnails and filmstrips are cached on **local disk**, content-addressed.

### 12.3 Compositor

- **Render graph:** frames → transform/crop/effects/LUT → mask → blend → captions/Lottie/SVG.
- **Native:** export, `fin look`, QA renders. **WASM:** client preview.
- **Frame ingest in preview:** WebCodecs `VideoFrame` → `copyExternalImageToTexture`, one GPU copy. Zero-copy is attempted only if the spike shows wgpu supports it; it is not a requirement.
- It consumes `fin-core::eval` output only.
- Golden-frame parity tests compare native and WASM output.
- **Fallback when WebGPU is unavailable:** server-rendered preview frames.

### 12.4 Captions

- Word timings from the **aligned** transcript (§13) → blocks → line breaking → style preset → per-word animation.
- Platform safe zones drive layout and QA.

### 12.5 Lottie and SVG

- thorvg (worker for export, WASM for preview).
- **v1: dotLottie templates with typed slots.** Raw agent-authored Lottie and its validator come after v1.
- SVG via resvg after sanitisation.

### 12.6 Masks (core primitive, v1)

- `mask` takes alpha from a matte asset (luma-encoded greyscale video), with feather, choke, invert and keyframes.
- Producers are plugins. The background-removal plugin comes after v1. Its model licences are to be verified: BiRefNet (MIT), MODNet (Apache-2.0), SAM2 (Apache-2.0), MediaPipe (Apache-2.0), Apple Vision. **Excluded:** RVM (GPL-3.0; copyleft repo only) and RMBG-2.0 (non-commercial).

### 12.7 Audio (`fin-audio`)

- **Format:** 48 kHz, 32-bit float, internally stereo (mono and stereo sources supported). Resampling at decode.
- **Model** (evaluated by `fin-core::eval`): per-item gain and gain keyframes, fades and crossfades, per-track gain/mute/solo, pan, and an audio effect chain (EQ, compressor, limiter, de-noise as plugins).
- **Ducking rules:** music ducks under speech, using voice-activity artifacts. Rules are configurable presets.
- **Loudness:** integrated LUFS and true-peak measured on export. Normalisation targets come from the format template (for example −14 LUFS / −1 dBTP for social).
- **Native mixer** for export and QA renders. **WASM mixer** in an AudioWorklet for preview.
- **Master clock:** in preview, the audio clock drives the video. Video frames are presented against the audio playback position, and late frames are dropped rather than slowing audio.
- **A/V sync tolerance** is a CI budget: ±1 frame at 30 fps in preview and exact in export.
- Beats, loudness and music/speech classification are analysis artifacts (§13).

### 12.8 Colour and frame rate (v1 policy)

- **Working space:** SDR Rec.709, 8-bit internally for v1 (pipeline types are prepared for 10-bit and HDR later).
- **HDR sources** (HLG, PQ, Dolby Vision from phones) are tone-mapped to SDR at decode:
  - macOS: OS tone mapping.
  - Windows and Linux: Finisher's own GPU tone-mapping shader (BT.2390-style).
  - The source's HDR metadata is kept for future HDR export.
- **10-bit sources** are decoded at 10-bit where the codec allows, then converted.
- **Frame rate:** the timeline has a fixed rate. VFR sources are conformed through their PTS map (§12.2). Rate conversion uses frame blending or nearest-frame, chosen per item.

---

## 13. Analysis (built-ins as bundled plugins)

| Analysis | Implementation | Artifact |
|---|---|---|
| Transcription | whisper.cpp (MIT); sherpa-onnx (Apache-2.0) voice activity detection + diarization | words, speakers, silences, fillers |
| **Forced alignment** | CTC aligner via ONNX (wav2vec2-style, licence-checked per language), or Parakeet-TDT word timestamps via sherpa-onnx (CC-BY-4.0, to verify). Chosen per language by measured quality. | word-accurate timings |
| Scenes | Rust port of PySceneDetect detectors (BSD-3, attributed); optional TransNetV2 (MIT) via ONNX | shot boundaries |
| Semantic search | sentrysearch-style pipeline in Rust: chunk → embed (**local SigLIP2 via ONNX by default**; Gemini / Qwen3-VL API opt-in per library, with a cost estimate shown before indexing) → LanceDB | searchable chunks |
| Reframe | face/subject detection (ONNX) → smoothed tracks → keyframed 9:16 crop | tracks, crop paths |
| Audio | LUFS, music/speech classification, beat detection | loudness, beats |

- **Alignment quality** is a tracked metric: mean and p95 word-boundary error against a labelled fixture set. The target is p95 under 50 ms for supported languages (to be confirmed in the spike). Caption QA flags drift.
- All analysis is fed PCM or frames from `fin-codecs`.
- ONNX Runtime runs on CoreML, DirectML or CUDA.
- Models download on demand, pinned by SHA-256, with licences recorded (UX in §21).
- **Library indexing** runs this pipeline over registered libraries in the background (§6.7). Agents can then search and assemble across the whole library.
- **Clip finder:** candidate moments come from transcript, scenes, loudness and semantic hits. The agent ranks them, then spawns a child branch per short, each running paper edit → reframe → captions → graphics → reviewer.

---

## 14. Plugins

### 14.1 Principle and kinds

- **Small kernel:** op log, edit engine, compositor, audio, jobs, plugin host, gateway.
- First-party plugins are bundled and use the same API, manifest and registry protocol as third-party ones.
- **Code plugin:** separate process, JSON-RPC over stdio, versioned host API. Written in Rust, Node, or Python.
- **Content pack:** data only. Templates, presets, skills, WGSL effects, LUTs.

### 14.2 Contributions

| Area | Contributions |
|---|---|
| Agent | `fin` command groups (server-side expansion, §6.3), skills (progressive disclosure), agent roles, saved prompts, agent providers, critic rubrics, external tools |
| Media | analyzers, WGSL effects (run in the compositor), transitions, LUTs, audio effects, codec backends, importers/exporters, matte producers |
| Generation | providers (§14.3) |
| Templates & presets | caption styles, Lottie templates, format templates, edit-style presets, export presets, QA profiles, ducking rules |
| Extension slots | typed data validated in Rust against plugin JSON Schema |
| Hooks | §10 handlers |
| UI | declarative native surfaces (status items, toasts, badges, state-driven side panes) and sandboxed iframe panels |

**Plugins cannot define primitive ops.**

### 14.3 Generation providers

- Modalities: image, video, voice, music, SFX, dubbing.
- **First-party providers in v1: OpenRouter and fal.ai** (owner decision of 2026-11-07). The user pastes a fal.ai key; OpenRouter shares the built-in agent's key. Modalities neither catalogue covers (dubbing and voice cloning, unless confirmed at 8G's start) are a known v1 gap, left to third-party provider plugins.
- **Catalog as data:** models, parameter schemas, cost, terms, output licence.
- **Calls:** submit, poll, cancel, fetch. The job ID is persisted before waiting, so a paid call is never re-submitted.
- Credentials are passed per call via `ModelGateway`.
- Outputs go to `workspace/` and are registered with provenance.
- Budgets and approval rules apply.

### 14.4 Data, permissions, lifecycle

- **Data:** shared-memory frames and PCM; content-addressed outputs.
- **Permissions:** `project:read`, `project:write` (ops only), `media:read`, `net`, `gpu`, `fs:<scope>`, `hooks:modify`. Approved at install by a person; re-approved when an update adds permissions.
- **Isolation:** cleared env, sandbox, scoped credentials (not env), lazy start, crash isolation, timeouts.
- **Third-party code plugins before sub-project 7C** (D8): allowed. Installing one shows a high-risk warning, its permissions need a person's explicit approval, and it never auto-updates. Until 7C's `plugin` sandbox profile exists, the env allow-list, the absence of credentials, brokered egress and the in/out rules are the boundary; the profile is then enforced on every code plugin automatically, with no reinstall.
- **Python plugins:** **wheels only.** The package bundles a hash-pinned lockfile, and `uv` installs with `--no-build` and `--require-hashes`, so no sdist builds run at install.
- **Versioning:** `host-api` range enforced; per-project lockfile pins versions and hashes.
- **Developer loop:** `finisherd --plugin-dir` (8B), `fin-plugin validate <dir>` (the publisher tool; `fin` does not link the font and catalog readers an offline check needs). The manifest, permissions, plans and approvals of this section are recorded as built in `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §4–5 and §23 (item 17 of §27).
- **SDKs (MIT):** Rust, TS, Python. `fin-plugin-test`; conformance suites for codec backends, generation providers and matte producers.

---

## 15. Registry

- **v1:** bundled plugins, plus **local-folder registries**. **A git registry is a local folder** the user keeps current with `git pull` (or a sync tool): Finisher never runs git and trusts nothing about its history, and TUF refuses a pull that rolls the repository back. The **public signed registry comes after v1**, with the protocol designed and implemented in the client from v1.
- **Package:** a zip archive (`.finpkg`) with the manifest (id, version, licence, permissions, `host-api` range, platforms); copyleft is flagged in the install plan. A package carries no signature of its own: its identity is the hash and length in signed TUF targets metadata, whose custom fields repeat the manifest's id, version, kind, licence and permissions digest.
- **Trust model: TUF** (The Update Framework):
  - Roles: `root` (offline keys, threshold signing), `targets` (delegated per publisher scope `@publisher/*`), `snapshot`, `timestamp`.
  - First-party root: the owner holds 3 offline keys with a 2-of-3 threshold, created in a key ceremony before the first registry is published.
  - This gives key rotation, revocation, freshness and protection against rollback and freeze attacks, all serveable as static files.
  - Publisher keys are bound to scopes by `targets` delegations.
  - Optional Sigstore keyless signing with a transparency log for publishers via CI OIDC (after v1).
- **Local-folder registries** (git ones included) use the same package format and TUF verification as the public registry will. Trust is per root: the owner pins a root (a file or its SHA-256) when adding a registry, with no trust on first use, and the trusted metadata survives removing the registry.

**Rules learned from npm, PyPI and crates.io**
1. No install-time code execution (wheels-only for Python, no build scripts).
2. Versions are immutable; yank only.
3. Publisher scopes, reserved names, typosquat checks.
4. 2FA plus signing, verified through TUF metadata.
5. No plugin-to-plugin dependencies.
6. Permission changes require re-approval; code plugins never auto-update.
7. Signed advisories and revocation.
8. Rollback- and freeze-resistant metadata.

---

## 16. Pairing, auth, security (mirrors T3 Code)

- **Environment:** stable ID plus ordered routes (LAN, Tailscale, SSH, public, Connect).
- **Pairing:** `finisherd pair` or Settings produces a code, QR code or `finisher://pair` link.
  - Links are **one-time** with a **TTL of 10 minutes or less** (secret in the URL fragment).
  - Pairing needs server-side confirmation (the owner approves the device name and role on an existing paired device or the server console).
  - Links are bearer credentials; the UI warns against pasting them into chats.
- **Sessions:**
  - DPoP-bound tokens; WebSocket tickets.
  - Every RPC declares a required scope.
  - Narrow agent and plugin scopes, delivered as credentials and never via env.
  - Devices can be listed and revoked.
- **Transport:**
  - local socket and `127.0.0.1` locally;
  - TLS remotely with a self-signed certificate pinned at pairing (desktop shell and native clients);
  - browsers reach a remote server only over a route whose certificate they trust (owner-supplied files or a reverse proxy) or over loopback through an SSH tunnel; no ACME client and no click-through (owner decision 3 of 2026-11-07, as the 5B plan `docs/superpowers/plans/2026-11-08-sub-project-5b-pairing-and-remote-access.md` records it; built as control-plane spec §5.5);
  - public TLS when hosted.
- **Certificate rotation:** the server signs a new certificate's fingerprint with the old key before rotating. Paired clients accept it automatically. A lost key means re-pairing, with a clear UI explanation.
- **Handshake:** protocol version (HTTP 426 on mismatch) plus capabilities.
- **SSH/WSL** launch and forwarding are owned by the desktop shell.
- **Connect relay** is in `finisher-cloud`; it is not in the data path.
- **Secrets:** OS keychain (encrypted file on headless servers and NAS); read only by `finisherd` at call time; never sent to clients, env, plugins (except per-call `ModelGateway` grants), logs or projects.
- **Outbound:** HTTPS only, size caps, allow-lists, hash pins.
- **Audit log:** pairing, tokens, grants, hook blocks, hook modifications, sandbox violations, `auto` merges into `main`. It keeps device names and remote addresses only for pairing, revocation and failed authentication; it is local and owner-only, kept 180 days, and never uploaded.
- **Human-only operations** are human-only from sub-project 5A, through the TTY-gated `finisherd` console until the UI exists. Until sub-project 7C's sandbox, this is advisory against an agent run in a raw shell (it can read the key files); the README's security model says so.
- **Threat priority:** injected agent > malicious plugin > stolen device > network attacker.

---

## 17. Frontend and UX

- **Stack:** React 19, TanStack, Tailwind v4, Base UI primitives (forked from T3 and rethemed), `dnd-timeline` + `@dnd-kit/core`, the legacy timeline helpers adapted, virtualised items.
- **State:** a store per subscription, fed by committed ops replayed through `fin-wasm`; selector reads.
- **Layout (CapCut-inspired):**
  - **Left rail:** Media/Libraries, Templates, Captions, Text/Lottie, Effects, Audio, Generate, Plugins.
  - **Centre:** preview with direct manipulation and safe zones.
  - **Right:** inspector and agent conversation.
  - **Bottom:** magnetic timeline with a transcript-view toggle.
  - **Branch bar:** chips with status and change preview; compare mode.
- **Libraries UI:** indexing progress per library, offline/online state, search across libraries.
- **Agent experience:** T3-grade conversation; live op highlighting; transaction cards; review mode.
- **Performance budgets (CI):**
  - 60 fps scrubbing on 540p/1080p short-GOP proxies
  - op → visible in LAN clients in < 100 ms
  - 5k items
  - first paint < 1.5 s
  - A/V sync ±1 frame
- **Electron:** pruned fork of T3's desktop (backend manager, protocol, updates, window/menu, settings, SSH/WSL), shipped for macOS, Windows and Linux. Web client in v1.

### 17.1 Internationalisation (D33)

- **Catalogue:**
  - All user-facing text lives in ICU MessageFormat catalogues (`packages/i18n/messages/<locale>.json`), with plurals, select and arguments.
  - The library is MIT/BSD with typed, tree-shakable message functions (Paraglide JS or Lingui; chosen in sub-project 4). `en` is the source locale and the only one shipped at launch.
- **No literals:** a lint gate (`no-literal-string` for JSX, attributes and `aria-*`) fails CI on hard-coded UI text.
- **Message keys are semantic** (`timeline.clip.split`), never English sentences. A new key needs a description for translators.
- **Server and agents never send display text to the UI:**
  - Errors, reviewer findings, op names and notifications travel as stable codes plus parameters; the client renders them through the catalogue.
  - Free text written by agents is content, not UI, and is shown as-is.
- **Formatting:**
  - Numbers, dates, durations, file sizes and lists go through `Intl` with the active locale. Timecode stays locale-independent.
- **Layout readiness:**
  - CSS uses logical properties (`margin-inline-start`, etc.) so RTL works later.
  - No text baked into images or icons. Layouts tolerate +40 % text expansion.
- **Pseudo-locale in CI:** `en-XA` (accented and expanded) renders every screen in Playwright visual QA, catching unexternalised or truncated strings.
- **Separate settings:** UI locale and content language are independent.
  - Content language drives the ASR/aligner choice, caption fonts and shaping (`fin-text`), and hyphenation.
  - The CLI (`fin`) is agent-only and English, but returns the same error codes.
- **Plugins** ship their own catalogues under their namespace; the registry manifest declares supported locales.

---

## 18. Licensing and third-party reuse

- **Finisher:** MIT (core, SDKs, protocol).
- **No LGPL/GPL in Finisher's code.** Enforced by `cargo-deny` + a JS licence gate.
- **Provisional exception: Electron's bundled ffmpeg** (LGPL; first decided 2026-11-07 in sub-project 6 decision 2, then made provisional by the owner's follow-up decision of the same day). Electron does not run without it. If the desktop shell stays Electron, Finisher would ship **Electron's open-codecs-only build** in place of the default one, as a dynamically linked browser-runtime component that the user can replace and that Finisher's code never calls, with its notice in `THIRD_PARTY_NOTICES`, and the run-time library gate would list exactly that library (sub-project 6 spec §16). **The desktop-shell options (Electron with a documented exception plus the open-codecs build, Tauri or the OS webviews, or a custom build) are decided in sub-project 9.** Only the desktop app is affected: the server, `fin` and the media engine contain no ffmpeg. `AGENTS.md` stays unchanged until that decision.
- **Claude Code:** never bundled; the user installs it (D28). No licence-gate exception is needed.
- **Codex:** bundled under Apache-2.0, with its licence and NOTICE shipped. Its bundled binary is included in the licence gate and in signing and notarisation.
- **Copyleft plugin repo** (`finisher-plugins-copyleft`): plugins that are copyleft **or pull copyleft dependencies**. Candidates:
  - WhisperX (BSD itself, but depends on ffmpeg via torchcodec)
  - PySceneDetect original (BSD, but its OpenCV/PyAV wheels bundle ffmpeg)
  - sentrysearch (Apache-2.0, but uses ffmpeg)
  - RVM-based matting (GPL-3.0)
  - an FFmpeg codec backend
- **Codec patents:** H.264/HEVC decode via OS or hardware only; H.264 export via OS-vendor encoders only, never GPU-vendor encoders; no built-in HEVC export (D13). AAC on Linux (symphonia, vendored vo-aacenc) waits on counsel's patent check.
- **`THIRD_PARTY_NOTICES`:** T3 Code (MIT, vendored paths), Uncut (MIT, skills/rubrics), PySceneDetect (BSD-3, ported), TransNetV2 (MIT), sentrysearch (design reference), thorvg, resvg, whisper.cpp, sherpa-onnx, ONNX Runtime, LanceDB, dav1d, SVT-AV1, libvpx, OTIO.
- **Model licences** are recorded and shown at download; non-commercial models are excluded.
- **Fonts:** bundled caption fonts are OFL or similarly permissive only, with licences shipped. User fonts are referenced, not redistributed. Project bundles embed only fonts whose licence allows it.
- **Not used:** T3 name, logo and relay, Clerk, Palmier code.
- **Open item:** codec patent posture review with counsel before commercial launch, as part of TASKS.md's "Counsel before release/upload" item.

---

## 19. Error handling

- One typed error shape across Rust, napi, RPC, `fin` and plugins: `{ code, message, details, fix? }`.
- Worker crashes become retryable job failures.
- Hooks and plugins time out visibly.
- Provider failures mark sessions `interrupted` and resumable.
- Storage errors (lease conflict, sync-conflict copies, offline library) are typed, with recovery actions.

---

## 20. Portability: project bundles and remote ingest

- **Project folders are portable by sync** (§6.7). Opening on another machine imports segments.
- **Bundle export** (`.finisher-bundle`): op-log snapshot + segments, assets, optionally sources, and embeddable fonts. Proxies and caches are excluded because they can be regenerated. **Import** rebuilds the database and re-indexes in the background.
- **Relinking:** sources are matched by fingerprint and then full hash across registered libraries. Library roots are mapped per machine (for example `/Volumes/NAS/Media` ↔ `\\nas\media` ↔ `/mnt/media`), stored per server, not in the project. A library's root belongs to one server; its aliases (the other machines' paths of the same root) resolve paths for the system only (`locate`) and never grant reach: reading a library file from a project needs a catalogued row (control-plane spec §7.8, 5D plan Decision 47).
- **Remote ingest:** chunked, resumable uploads from a client to a remote server's project `sources/`, with progress and integrity checks.

---

## 21. Product plumbing

| Concern | v1 decision |
|---|---|
| Telemetry | D34 (§21.1): a local event journal with consent per purpose (diagnostics, training), a per-project never-collect flag and documented, versioned kinds. Basic diagnostics is on by default with an opt-out; training content needs an explicit opt-in by a person, which an agent's `fin` cannot give (§21.1). No network upload in v1 until the uploader sub-project ships |
| Crash reporting | Panics and uncaught errors are `error.panic` journal events (§21.1). Under diagnostics they hold metadata only: the repo-relative `file:line` (or none, for a third-party location), a keyed hash of the scrubbed message, the process and the thread; no backtrace frames. The scrubbed message itself is training content. Minidumps from Electron, server and workers come with the uploader, under the same consent |
| Model downloads | Guided first-run: shows size and purpose, resumable, verified, pausable; app usable before models finish; offline after download |
| Offline clients | Read cached projections; edits queue only while the local embedded server is reachable (no client-only offline editing in v1) |
| Localisation | All UI strings externalised from day one (ICU messages); English at launch |
| Accessibility | Keyboard-operable timeline and inspector, focus management, screen-reader labels for controls, reduced-motion setting |
| Updates | Electron auto-update; `finisherd` and worker binaries ship with the app and are version-locked to it; headless servers update via package or Docker image; protocol version and capabilities prevent mismatches |
| Onboarding | Codex sign-in by the user through Codex's own flow (bundled); detect Claude Code (optional; the user signs in to it themselves); paste an OpenRouter key (optional), add a library, take a sample project tour, explain the codec capability report |
| Minimum platforms | D31: macOS 14+ Apple Silicon only (no Intel Macs); Windows 11 x64 with a WebGPU-capable GPU; Linux glibc 2.35+ and kernel 6.2+. Machines below minimum fall back to server-rendered preview or headless use |
| Distribution | D32: notarised DMG, signed Windows installer, AppImage/deb/rpm, server Docker image; auto-update; no app stores in v1 |
| Indexing cost and privacy | D29: local embeddings by default. Cloud indexing shows an estimated cost and upload notice per library before starting |

### 21.1 Diagnostics and training data (D34)

**Why.** Finisher collects data for two purposes: to improve the app (**diagnostics**), and to fine-tune an open-weight model on video editing, i.e. what belongs in an edit and what does not (**training**). Training includes everything that goes through an LLM. This section fixes the plumbing so that later sub-projects only add sources, the uploader and the dataset builder, never a rewrite. Implementation plan: `docs/superpowers/plans/2026-11-07-diagnostics-and-training-data-hooks.md`.

**Phasing**

| Phase | Delivers |
|---|---|
| Now (sub-project 4C branch) | Envelope, kinds and the payload enum, the journal and its blob lifecycle, consent and the project flag, the first sources (edits and history with their outcomes, intents and actors, `fin`, RPC, eval, fonts, errors, sessions), `fin diagnostics`, and the upload view (`export --as-upload`). No uploader, no network |
| Sub-project 5A | Sources and actors from principals; export of content, purge, `project default` and consent changes human-only (the console confirms them: y/N, or the target's name typed back for a purge, with the server's challenge code sent by the console itself); exports redacted unless they ask for content, which only a person may (built, §27 item 18) |
| Sub-project 7 | Fills the reserved agent kinds (turns, tool calls and results, edit verdicts) and sets `FIN_CALL` |
| Sub-project 9 | `ui.*` events, the UI's actor on commits, the onboarding consent question (content, no pre-selected answer) and the diagnostics notice, a "what we collect" page |
| Sub-project 10 | `review.*` verdicts (marker resolve, accept/reject per transaction) |
| Sub-project 11 or the cloud phase | The **local** dataset builder (below); the uploader (its trait, batching, backoff, cursor), which ships the builder's examples and the upload view's metadata entries; the receiving service; the first-upload notice |

**Events are data.** One envelope, versioned, with its types in `fin-core::events` (pure: no clock, no randomness; the emitter supplies `id` and `ts`) and the TypeScript types generated like every model type. In memory a payload is one variant of a closed enum, serialised on the journal's writer thread:

```json
{"v":1,"id":"01JC…","ts":1794571200123,"session":"01JC…","source":"finNode",
 "kind":"log.commit","kv":2,
 "consent":{"rev":3,"purposes":["diagnostics"],"basis":"default"},
 "ctx":{"project":"01jb…","branch":"br1","txId":"01JC…","author":"k1:3f…",
        "actor":{"kind":"agent","session":"01JC…","call":"k1:9a…"}},
 "payload":{"record":"edit","outcome":"rebased","baseRev":41,"rebasedOver":2,"ops":{"item.trim":1},"micros":812}}
```

- `v` is the envelope version; `kv` the version of the kind's payload. Kinds are `domain.verb` (lower camel case within a segment) from one closed registry in `fin-core`; version 0 marks a **reserved** kind whose payload may still change.
- `session` is the emitting process run (ULID), bracketed by `session.start` and `session.end`. `source` ∈ `fin`, `finisherd`, `finNode`, `ui`, `agent` (additive later). `console` and `mcp` were added by 5A. The server sets `source` from the connection's principal (`local` and `agent` → `fin`, `mcp` → `mcp`, `console` → `console`, `device` → `ui`; sub-project 5 spec §3.4), never from the payload or the transport.
- `ctx` links an event to the op log: project id, branch id, `txId`, the author as a keyed hash, and the **actor** (`user|agent|plugin|system|unknown`, with the agent or UI session and the tool call). `fin` sends its actor in the headers of every RPC request (`FIN_SESSION`, and `FIN_CALL`, the tool-call id an agent harness sets), so commits carry it too; a commit without one has actor `unknown`. Only a `local` principal's headers set the kind and session: an `agent` or `mcp` credential is always `{agent, session: <its id>}` and only its `call` comes from the header (5A).
- Ids are stored raw only when they are ULIDs; any other id (an idempotency key, a session name) is keyed-hashed.

**Field classes.** Every payload field has exactly one class, in its type and in the JSON Schema (`x-fin-class`):
- **`meta`:** numbers, booleans, closed enums, error codes, ids, keyed hashes, repo-relative source locations (`crates/…`, `apps/…`), coarse platform facts (OS family and major version, arch, core count, memory bucket), and **names** (commands, flags, RPC methods, media extensions), which come only from code or static tables, never from user input. Payloads cannot hold a free-form string as `meta`; a schema test enforces it.
- **`scrubbed`:** free text authored by Finisher code (error and panic messages) with absolute paths, the home and server-data directories, the user name and the host name replaced by placeholders *before* it is written.
- **`content`:** user text, captions, transcripts, file and library names, prompts, model outputs, tool arguments and results, media hashes, edit intents (the ops of a transaction), error messages, and frames and audio. Each content field names its kind (`userText`, `caption`, `fileName`, `prompt`, `modelInput`, `modelOutput`, `toolArgs`, `toolResult`, `mediaHash`, `txIntent`, `errorMessage`, `frame`, `audio`). Content is scrubbed too where it is Finisher's text.
- **Large content is a blob:** `BlobRef {hash, bytes, mediaType}`, only inside a content field, with the bytes in `<server-data>/events/blobs/<hash>` (keyed, content-addressed). A blob lives exactly as long as an entry references it: purge, retention, the size cap and `never` remove it with its last entry, by reference counting at rotation and purge. Frames and audio are always blobs; nothing writes one in the first phase, and a later sub-project adds their sources.
- **Never stored, in any class:** raw paths (a path becomes `{root: project|library|workspace|serverData|other, ext, hash}`), host names, IPs, MAC addresses, user names, serial numbers, credentials, raw `fin` arguments.
- Keyed hashes are BLAKE3 keyed with a per-install key (`events/key`, 0600), so they join records on this machine without being reversible by dictionary elsewhere. `purge --all` deletes the key.

**Consent** (owner decisions of 2026-11-07)
- **Two tiers.**
  - **Basic diagnostics** is metadata only. It includes edit outcomes: which ops an edit used, and whether it was kept, undone, reverted, rebased, deduped or conflicted. It serves both purposes: improving the app, and training on metadata alone. It is on by default everywhere (desktop, headless, Docker) with an opt-out, and is disclosed in the terms, the privacy notice and a first-run notice. It holds no content, but the absence of content alone does not establish legitimate interest (GDPR Art. 6(1)(f)): that basis holds only with a documented purpose, necessity and balancing assessment, which counsel reviews with the rest of the consent model (TASKS.md, "Counsel before release/upload", D34). Storing it on the device (ePrivacy Art. 5(3)) is already a counsel item; whether *uploading* diagnostics needs consent under ePrivacy is a further counsel question.
  - **Training content** adds captions, transcripts, prompts, LLM conversations, edit intents, error messages, file names, media hashes and, later, sampled frames and audio. It needs an explicit opt-in **by a person**: the onboarding question (sub-project 9, no pre-selected answer), a hand edit of `privacy.training` in `settings.json`, or `FIN_TRAINING=on` in `finisherd`'s own environment. Until then training is `undecided` and records no content. The build default can never turn it on; a test enforces it.
  - **Agents cannot opt in.** `fin` runs in the agent sandbox, so `fin diagnostics consent training` accepts only `off` and `unset`, and `on` is a usage error that names the ways in. The `diagnostics.consent` RPC refuses `training: on` from every principal but a `device` (the app's onboarding question), the server's console included (`CONSENT_UI_ONLY`). The sandbox's env allow-list does not pass `FIN_TRAINING`.
  - **Prior art.** Cursor's and Copilot's individual plans train on code by default and are criticised for it; their business plans default off. Zed keeps telemetry on and takes training data only by opt-in, and VS Code sends telemetry by default with a notice. Finisher follows Zed for content and VS Code for metadata, because footage, faces and voices are riskier than code (Art. 9).
- `<server-data>/settings.json` `privacy: {diagnostics: on|off, training: on|off, retentionDays, maxMiB}`, written by `diagnostics.consent` and `fin diagnostics consent` through a settings writer that changes only those keys and keeps everything else in a hand-edited file. An absent key takes the build default and records `basis: default`; a present one `basis: user`.
- Env overrides (`finisherd`'s): `FIN_DIAGNOSTICS`, `FIN_TRAINING` (`on|off`, `basis: env`); `DO_NOT_TRACK=1` forces both off. Off always wins over on. `fin` also honours its own `DO_NOT_TRACK` and `FIN_DIAGNOSTICS=off` and then sends nothing.
- The journal opens with the consent already resolved from the settings and the env, so nothing is written before they apply. An unreadable `privacy` section keeps the last good settings; only before the first good read does it mean both off (fail closed, as with fonts).
- **Training implies content; diagnostics never does.** With only diagnostics on, content fields are removed at write time and training-only kinds are written as their metadata part. The writer applies the consent in force when the entry was recorded **and** when it is written, so content queued before a withdrawal never reaches disk. Content entries always carry `basis: user` or `env`, never `default`.
- **Per project:** `finisher.project.json` `collect: "never"` (absent means "follow each user's settings"). It travels with the folder, so every collaborator's server honours it when it re-reads the file. Setting it needs the writer lease; clearing it is human-only once permissions exist (sub-project 7). While a project's flag is unknown (the project is not open on this server), new entries for it are dropped, but nothing is purged. An explicit `never` purges that project's entries and blobs from this server's journal, and the built examples derived from them (see **Dataset builder**); each collaborator's server does the same when it sees the flag.
- **Off means off.** When no purpose is on, nothing is recorded anywhere: no file, no directory, no in-memory ring. The gate is one atomic load before a payload is even built. Normal stderr logs are unaffected.
- Each entry carries the consent it was written under (`rev`, `purposes`, `basis`). Changes are journaled as `consent.changed` on a path that never drops, under the old consent, just before they take effect. **The upload view re-checks current consent per entry**: withdrawing a purpose stops the upload of everything already recorded for it, and deletes the built examples derived from those entries (see **Dataset builder**).

**The journal** (`fin-journal`, linked by `fin-node`)
- **One writer per server data:** `finisherd` (the `fin-node` journal). Other processes send events to it: `fin` over the local socket, the UI over the WebSocket later. Offline `fin` records nothing.
- `<server-data>/events/j-<yyyymmdd>-<ulid>.jsonl`: one JSON object per `\n`-terminated line, append-only. Files rotate at 8 MiB or a day; an idle file is closed by the hourly caps pass once its day has passed, so retention reaches its entries too. Retention defaults are 30 days and 128 MiB in total, blobs included, oldest first: files are in write order (their ULID, which stays monotonic when the clock goes back), the age cap removes every closed file past it and the size cap the first written. An entry of another envelope version is unknown to readers, like an unknown kind.
- **Crash-safe:** a torn last line is truncated on open and skipped by readers; closed files are fsynced; a blob is on disk before the line that references it; a panic or shutdown flushes synchronously, with a timeout.
- **Never blocks editing:** emitters enqueue onto a data channel bounded by entries and by bytes. A full channel drops the event and counts it (`journal.dropped`). Consent, project policy, purge and export use a separate control path that never drops. A failing disk suspends the journal and editing goes on. A writer thread serialises, batches and flushes every second or 64 KiB, and owns every file operation, purge and export included. Data never wakes it (a wake would cost every commit a cross-CPU interrupt): it parks until its flush timer, unless 64 KiB are in flight or a control message arrives, so an entry reaches disk within ~2 s (two flush intervals) at worst.
- **High-rate signals are aggregated:** RPC and eval timings become per-minute histograms (`rpc.stats`, `eval.stats`), not one event per call. Streaming RPCs (`ops.subscribe`, `project.events`) are not timed.
- **Cost is measured** (`journal_cost`): disabled emit, enqueue latency, writer throughput, and the commit-path overhead in `fin-node` with diagnostics and with training on. The budgets live in the plan.

**Kinds now:**
- `session.start|end|unclean`, `consent.changed`;
- `log.commit`, one per attempt to append to the op log (edits, undo, redo, merge, pick, revert, checkpoint, branch create and set, recovery), classified in `fin-node` from typed results and the store's dedupe signal: `record` with the txIds it links to, outcome `accepted|rebased|deduped|noOp|conflict|refused|dryRun`, error code, op-kind counts, `baseRev` and the number of revs it was rebased over, conflict codes and duration; with training, also the intent as committed (or as requested, on failure). No state snapshot is stored: a training example's context is only what was captured under training consent (see **Dataset builder**);
- `cli.command` (registry name, flag names, keyed `argsHash`, duration, exit code, error code, online, dry run, txIds);
- `rpc.stats`, `eval.stats`, `fonts.fetch`, `storage.state`, `error.internal`, `error.panic` (location and message hash; the message as content), `journal.dropped`.

**Agent kinds (sub-project 7):** the v0 reservations are replaced by version 1 of `agent.session`, `agent.turn`, `agent.toolCall` and `agent.toolResult`, the new `agent.approval`, and the retirement of `agent.editVerdict` (verdicts are `review.verdict`). The kinds, their provenance fields, the agent-trace store that holds their content and the join to commits are in `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §12 (as built by 7A, §24), which wins over this document.

Also reserved: `ui.action` (9) and `review.verdict` (10).

**"Show me what's collected."**
- `fin diagnostics show` gives the consent and its basis, the project flags, the files, blobs and sizes, counts per kind, the oldest and newest entries and the drops.
- `fin diagnostics export [--since] [--project] [--kind] [--limit] [--as-upload <purpose>]` writes JSONL to stdout. `--as-upload` shows exactly what an uploader would send now.
- `fin diagnostics purge (--all | --project <id> | --before <date>)` (it also deletes the built examples derived from the purged entries); `fin diagnostics project never|default`.
- The same calls are RPCs for the UI. Until sub-project 7, exporting content, purging and `project default` are open to `fin` too, so an agent can read the user's recorded content; content is served only to the `fin` and `ui` sources.

**The upload view (no network now).** `export --as-upload <purposes>` (`Journal::export` with an upload filter) is the contract a future uploader ships: closed files only, entries whose stamped purposes and the **current** consent share a purpose, projects not `never` or unknown now, content and blob references removed unless training is both current and stamped. The uploader sub-project adds the `Uploader` trait, batching, a cursor per uploader (at least once, deduplicated by event `id`) and pseudonymisation of project, branch and tx ids with its own key before sending. It reads through this view, so nothing in the journal changes.

**Dataset builder (planned, out of scope now).** It runs **locally**, on the user's machine, over the journal and the agent-trace store only. **It never reads the op log**, so it sees only what was captured under training consent (`crates/fin-dataset-inputs` reads the journal's `events/` and `agent-traces/` and links nothing else; `scripts/check-crate-deps.mjs` enforces it). The uploader ships the examples it built, plus the upload view's metadata entries; raw journals, trace stores and op logs never leave the machine.
- **Decision (2026-11-09, owner): consented capture only.** A training example's context is whatever the journal or the agent-trace store recorded at event time under training consent, and nothing rebuilt afterwards. Rebuilding the state at `log.commit.baseRev` from the op log would re-read project content (captions, file names, footage identity) that no consent covered and that purge, withdrawal and `collect: never` do not reach.
- **Example:** the instruction (the agent turn joined by `actor.call` and `txId`, or the `fin` command), the intent, and the **context captured with the edit**. For an **agent edit** the context is the agent's own `fin view` output and other tool results for that call, read from the agent-trace store (sp7 §12). For a **human edit** it is the ops, the intent and the label only: no state is reconstructed.
- **Possible later extension (sub-project 11):** capture a redacted context at event time, under the same consent, so human edits can carry a view too. **Rejected:** per-entity provenance filtering over the op log (tracking which entities each consent covered): it would put the op log back in the builder's inputs and make every purge a graph problem.
- **Examples are a regenerable cache** keyed by their source entry ids (journal entry ids and trace-store ids). Purge, consent withdrawal and `collect: never` delete every example derived from an affected entry and drop it from each uploader's cursor, so a withdrawn entry cannot be uploaded later in a built example; regenerating from what remains is always possible.
- **Label:** kept, undone or reverted (a `log.commit` with `record: undo|revert` naming that txId, and how soon), merged into `main`, branch abandoned, or a conflict and its resolution (the next commit by the same actor on that branch), plus agent edit verdicts.
- Pairs of kept and undone edits from the same context give preference data. Exports are versioned and licence-tagged, and only made from entries recorded under the training purpose.
- **Training targets (owner decision of 2026-11-07, sub-project 7 decision 1):** human edits and model outputs, **closed-model outputs included**, may be training targets. This goes against the recommendation (human edits and open-weight outputs only), and closed-model providers' terms forbid using their outputs to train other models.
  - **Counsel before the first training run or upload** is mandatory and recorded as the gate `training.closedModelTargets: pendingCounsel|cleared|excluded`, default `pendingCounsel`, set only by a person after counsel's review and audited (sub-project 7 spec §12.7). While it is `pendingCounsel` or `excluded`, the dataset builder and every upload path drop each example whose target comes from a closed- or unknown-weights turn, and refuse a run that needs them.
  - **Reversible by design:** every example is tagged with its model provenance, `{provider, model, harness}` plus the weights class (`open|closed|unknown`), taken from `agent.session` and `agent.turn`; a person's edit has no model. Closed-model targets can be filtered out later without re-collection.

**As built (plan D, D1–D7, 2026-11-07).** The plumbing above is implemented on the sub-project 4C branch; `TASKS.md` "Diagnostics hooks" has the per-task detail.
- **Crates and seams:** the model in `fin-core::events` (20 kinds: 13 v1, 7 reserved v0); the journal in `fin-journal` (writer thread, consent, key, blobs, project policies, purge, export, sessions); the napi `Journal` and the edit-path sources in `fin-node` (`commit_log.rs` classifies outcomes from typed results and the store's `WriteOutcome` note); `finisherd`'s `diagnostics.ts` (six `diagnostics.*` RPCs, the source per transport, a token bucket per source of 50/s with a burst of 200, the settings watch); `fin`'s `diag.rs` and `fin diagnostics`.
- **Measured cost** (release, Linux VM): the disabled gate 0.4–0.8 ns (budget ≤ 5 ns); `record` p99 1–2.6 µs (≤ 5 µs); writer 468–616k diagnostics entries/s (≥ 100k) and 350–397 MiB/s of content (≥ 20 MiB/s); `record_sync` against a stalled writer returns at its 250 ms / 1 s timeout; 482 B per diagnostics entry, 792 B with a 2-op intent. On the commit path (`commit_overhead`, 1k-item project, median paired difference against `off` in the same round): diagnostics +3.2–7.3 µs (+0.14–0.33 %; budget ≤ 2 % and ≤ 20 µs), training +5.1–10.6 µs (+0.23–0.47 %; ≤ 5 % and ≤ 50 µs).
- **Deviations from the design text:**
  - The writer is not woken by data (Decision 18). It parks until its flush timer, so an entry reaches disk within ~2 s; `export`, `status` and a consent change flush first. A wake on every `try_send` cost a commit 13 µs.
  - The age and size caps also delete while recording is off (deletion only).
  - Merge and recovery entries carry the first record's txId, `<requestTxId>-1` (keyed, since it is not a ULID), on every path; `fin`'s `cli.command` for a merge names the same id.
  - `consent.changed` is journaled only for a change while a server runs (`diagnostics.consent`, a hand edit the settings watch applies). A restart under a different consent (an env override added or removed) writes none: the new `rev` and the stamps of the entries show it.
  - `--as-upload training` keeps only entries stamped `training` and allowed by the consent in force now (Decision 31; D7 corrected `fin-node`, which had mapped it to every purpose). With training on, metadata kinds are stamped with both purposes, so the training view also carries the metadata recorded while training was on.
  - Rate-limited `diagnostics.record` calls answer `recorded: false` and are counted in the service, not the journal. An unknown kind is `EVENT_KIND_UNKNOWN`. A consent write that fails is `SETTINGS_WRITE_FAILED` (exit 5); a merged `privacy` section that breaks the journal's rules is `SETTINGS_INVALID`.
  - `fin diagnostics show` has no `--limit` (`export --limit n` lists the newest entries). `fin` records no `cli.command` for `diagnostics purge`, so `purge --all` leaves no key behind. `help`, `schema` and usage errors are recorded with `online: false` when a server is reachable.
- **End to end** (`pnpm e2e diagnostics`, `scripts/e2e/diagnostics.mjs`): off by env leaves no `events/`. By default diagnostics are on and training is `undecided`, with no content. One `cli.command` per `fin` run. Outcomes `accepted`, `rebased` (`rebasedOver`), `deduped` and `conflict`, the undo's links, and matching actors on `cli.command` and `log.commit`. Every committed `log.commit` txId resolves through `tx.get`; keyed ids are computed with the install key through the test-hooks `Journal.idRefForTest`. The leak grep covers the temp dir, `$HOME`, `$USER`, the host name, argument markers and unkeyed `blake3:`. A `never` project is purged and dropped, also by a second server following the folder. `fin` cannot opt in; `FIN_TRAINING=on` records an intent; a hand edit opts in and `consent training off` withdraws (two `consent.changed`), which empties the training upload view; `purge --all` leaves no key and no blobs.

---

## 22. Testing

| Layer | Approach |
|---|---|
| `fin-core` | property tests: apply∘invert, rebase consistency, undo with interleaved authors, after rebase and after cherry-pick, validation soundness; replayed through `fin-wasm` in Vitest |
| Storage | segment export/import round-trip; simulated sync conflicts (duplicate seq, conflict copies, stale leases); network-root polling; offline libraries; database rebuild from segments |
| Compositor / audio | golden frames native vs WASM; audio mixer null tests; A/V sync measurement |
| Alignment | labelled fixture set; boundary-error metrics as CI trend |
| Protocol | contract tests on generated types against a real `finisherd` with fixtures |
| Agents | `fin` eval harness (Codex, Claude Code, built-in): success, calls, tokens, retries |
| Sandbox | escape tests per OS (writes to `.finisher/` and `assets/` must fail; asset hash tampering detected) |
| Plugins | `fin-plugin-test`; conformance suites; registry TUF verification tests |
| UI | Vitest; Playwright visual QA against a fixture server |
| Performance | §17 budgets as CI gates |

---

## 23. CI (GitHub Actions)

- Actions pinned to commit SHAs.
- Matrix: macOS arm64 (no Intel Macs), Windows x64, Linux x64 (arm64 where available).
- **Gates:** build, tests, licence gate, cloud import-graph guard, file-size lint, generated-types drift, event-loop lint, performance and A/V budgets, sandbox escape tests.
- **Artifacts:** napi prebuilds (or the sidecar fallback), WASM build, signed and notarised release artifacts.

---

## 24. Sub-projects and build order

| # | Sub-project | Outcome |
|---|---|---|
| 0 | Bootstrap | Monorepo (Cargo + pnpm), MIT, `THIRD_PARTY_NOTICES`, CI skeleton, `AGENTS.md`, initial skills |
| 1 | **Spike (throwaway)** | Measure and decide (see below) |
| 2 | Minimal core + server | `fin-core` with **three ops** (insert clip, trim, delete) + `fin-store` + minimal gateway and snapshot/op stream + `fin` with those commands |
| 3 | **Walking skeleton** | `fin` edit → live in a bare web client → proxy preview with audio, end to end, on a project stored in a synced folder |
| 4 | Core breadth | Full op set, eval, layout, branches, undo, schema/type generation, property tests, segment export, lease, maintenance |
| 5 | Control plane breadth | Pairing/auth/environments, scopes, review workflow, jobs, library indexer |
| 6 | Media engine | Codecs, proxies, compositor, `fin-audio`, captions, Lottie templates, SVG, masks, colour/VFR, export |
| 7 | Agent runtime | Codex adapter (bundled), Claude Code CLI adapter, built-in agent, sessions/branches, subagents, roles, modes, OS sandbox, hooks, workspace artifacts, full `fin`, eval harness |
| 8 | Plugins + registry | Host, manifest, SDKs, local-folder registries (git ones are folders) with TUF verification, bundled analyzers (transcription + alignment, scenes, search, reframe, audio), generation providers, codec-backend and matte interfaces |
| 9 | Frontend + Electron | Editor, libraries UI, timeline, transcript view, branch bar, conversation, review mode; macOS/Windows/Linux packaging |
| 10 | Reviewer + skills | QA gate, critic, markers, share page; Uncut skills/rubrics ported |
| 11 | v1 features + hardening | Clip finder, reframe, caption templates, Lottie template packs, onboarding, signing/notarisation, release |
| 12 | After v1 | Public registry, raw Lottie, background-removal plugin, mobile, `finisher-cloud`, copyleft plugin repo |

**Mobile (owner decisions 2026-11-08):** native iOS/Android with limited offline editing becomes sub-project 12, "mobile core and offline editing" (not designed yet); see `docs/specs/2026-11-08-mobile-offline-editing-decisions.md`.

**Spike (#1) measures**
1. napi apply/rebase on a 10k-item project with 1k-op branches. **Electron ABI prebuild** on all three OSes; if it's problematic, evaluate the sidecar fallback.
2. Render-plan size, against D24's "would change my mind" conditions.
3. WebCodecs → WebGPU → WASM preview at 1080p60 with **audio as master clock**. Measure A/V drift and whether `VideoFrame` ingest into wgpu is zero-copy or one copy.
4. **WebGPU and WebCodecs hardware decode in Electron on Linux** (flags, GPU blocklists). Map the fallbacks.
5. OS decode/encode on three OSes, including **HEVC on Windows** and **VA-API on Linux**, and short-GOP proxy encode speed.
6. **Word-alignment accuracy:** whisper.cpp alone vs a CTC forced aligner vs Parakeet-TDT, on a labelled set.
7. **Agent CLI control:** Claude Code stream-JSON permission requests, interrupt and resume; bundled Codex app-server with the vendored T3 client, run inside the OS sandbox.
8. **OS sandbox feasibility:** Seatbelt, Landlock and Windows AppContainer around a provider CLI.
9. **Signed and notarised throwaway build** (macOS + Windows) bundling Electron + Node server + napi `.node` + Rust workers + ONNX Runtime + the Codex binary.
10. **SMB/NFS and synced-folder behaviour:** segment export/import, lease heartbeat, conflict-copy detection on Dropbox/iCloud/OneDrive, polling cost on a large NAS library.
11. **thorvg in WASM** for Lottie template preview: frame time for typical templates at 1080p, plus parity with the native worker. Lottie templates ship in v1, so this applies even though raw Lottie is deferred.

---

## 25. Salvage from legacy `video-creater`

Ported deliberately:
- Atomic save, journal and recovery ideas (`project/storage.rs`, `project/split.rs`) → `fin-store` segments and maintenance.
- Process runner (`render_pipeline/process.rs`).
- Keychain credentials (`provider_credentials.rs`).
- Licence-policy gate (`render_pipeline/plugin_policy.rs`, policy scripts).
- Pinned model downloads (`speech_models.rs`).
- Signed generation catalog and paid-call recovery (`generation/`).
- `dnd-timeline` usage and timeline helpers (`src/lib/timeline-*`).
- Compositor starting points (`frame_compositor/`, `gpu_graphics/`, `graphics/`).
- thorvg vendoring and provenance (`vendor/dotlottie-rs/SOURCE.json`).
- Matte reference (`project/matte.rs`).
- Visual QA approach.
- Legacy skills where Uncut's don't cover a need.

**Not carried over:** Temporal, GStreamer/GES, Tauri, Swift sidecars as core dependencies (an optional macOS acceleration plugin is allowed), the split JSON project format, hand-maintained IPC types, the 182-arm tool dispatcher.

---

## 26. Risks and open questions

| Risk | Mitigation |
|---|---|
| napi + Electron prebuild matrix unproven | Spike #1.1; sidecar fallback with the same API |
| WebGPU/WebCodecs gaps (Linux Electron, older GPUs, iOS later) | Spike #1.3–4; server-rendered preview fallback |
| Word-timestamp quality | Forced-alignment stage; metric + fixtures; spike #1.6 |
| Agent CLI protocol drift | Bundled Codex pinned to the vendored client; Claude Code version detection; adapter boundary; eval harness |
| Windows sandbox feasibility | Spike #1.8; hash-verified assets as an integrity backstop |
| Windows HEVC / Linux codec gaps | Capability report, guidance, codec-backend plugins |
| Sync services mangling project folders | Immutable segments, lease, conflict-copy detection, rebuild from segments; spike #1.10 |
| `fin-core` god-crate | Module per op; file-size lint |
| Upstream T3/Uncut churn | Frozen vendored slices |
| Scope | v1 cuts (§1); skeleton at #3 |

**Open questions**
1. Codec patent posture review with counsel before commercial launch (TASKS.md "Counsel before release/upload").
2. Per-language aligner and model choices, decided after spike #1.6 and licence checks.
3. Lease heartbeat interval and polling cadence defaults, tuned after spike #1.10.

**Product and business questions (not blocking the architecture)**
4. Trademark and domain clearance for "Finisher" before the name goes public.
5. GitHub: owning account or org, public-from-day-one vs private until v1, and when the legacy `video-creater` repo name is replaced.
6. Contribution policy: DCO sign-off (recommended) vs CLA.
7. Windows code signing: procure Azure Trusted Signing or an EV certificate. Apple Developer ID exists.
8. ~~Which generation-provider plugins ship first-party in v1.~~ Decided 2026-11-07: OpenRouter and fal.ai (§14.3).
9. Authoring and licensing of the starter Lottie template packs and caption presets.
10. Rights-cleared test footage for the eval harness, alignment fixtures and golden frames.
11. Direct publishing to TikTok, YouTube, Instagram (proposed: after v1).
12. Pricing and paid hosted feature set (cloud phase).

---

## 27. Post-spike amendments (revision 3)

Spike code, evidence and the report are throwaway and live in the git-ignored `archive/spike/` folder (`report/2026-11-spike-report.md`, `report/results/`); this section is the durable record. Where an entry conflicts with an earlier section, the entry wins.

1. **§7 Rust/TS boundary (S1, S2): confirmed.**
   - napi measured: apply p99 1.9 µs, same-track 1k-op rebase 4.0 ms (every op shifted), event-loop p99 1.35 ms.
   - A TS model rebases about 5.8× slower at p50 (22.9 ms, JSON parsing excluded) and uses 2.4× the memory.
   - Baked render plans are small (676 B/frame), but keyframe evaluation and caption layout still have to be shared Rust code.
   - The sidecar fallback is measured at 22 µs p99 and is the documented alternative if napi prebuilds fail on any platform.
   - **Linux x64** (Ubuntu 24.04 VM, Alder Lake):
     - apply p99 3.4 µs, rebase 5.3 ms, event loop 1.1 ms, sidecar 32 µs;
     - Electron main and `ELECTRON_RUN_AS_NODE` child load 5/5, with state hashes identical to macOS.
   - **Windows 11 x64** (Ryzen 7 7800X3D, MSVC):
     - apply p99 2.5 µs, rebase 5.2 ms, sidecar 31 µs over a named pipe;
     - Electron main and child load 5/5, with identical state hashes.
     - Windows timers tick at 15.6 ms, so event-loop latency needs a longer measurement window there.
2. **§12.3 preview (S3):**
   - Frame ingest is `copyExternalImageToTexture` (one GPU copy, 0.4 ms p95).
   - The preview scheduler must be **cadence-aware**: it measures the real display/rAF interval and paces against it.
   - **End-to-end A/V latency (S3b, measured by ScreenCaptureKit capture; offset = sound − light, + = audio late):**
     - The round-1 scheduler plays audio **37 ms early** on average (−48 ms under load), because frames reach the glass 23–56 ms after rAF, with a pipeline depth that varies per run. `getOutputTimestamp` already includes CoreAudio output latency.
     - The player therefore samples the audio clock at rAF + the running median (last 9 probes) of Element Timing `presentationTime − rAF` + scan-out + panel response, and shows the **nearest** frame. The probe is a 1-px `elementtiming` element inserted every 30 rAFs. Panel response is 5 ms until physically verified.
     - It never adds `outputLatency` (double-counting). Result: compensated |offset| p95 7.4 ms (held out).
     - **The display-correction constant is per platform and measured, never assumed.**
       - Element Timing `presentationTime` is 1.75 ms after the real display time on macOS but 4.8 ms before it on Windows.
       - The "nearest frame" window is half the **display** refresh interval, not half the content frame.
     - **Windows (RX 9070 XT, 144 Hz, measured with Windows.Graphics.Capture + WASAPI loopback):**
       - Uncompensated |offset| p95 13.9 ms: frames reach the glass a steady 2 refreshes after rAF.
       - Compensated p95 9.1 ms (held out).
       - `getOutputTimestamp` already includes the WASAPI output latency there too (−0.97 ms vs the OS play time).
     - The player interpolates the audio clock between render callbacks (Windows steps it every 10 ms).
     - Open: physical panel check (240 fps phone test), Bluetooth/AirPlay outputs, refresh rates other than 60 and 144 Hz, Linux. About 2 % of macOS events have a one-refresh window-server slip.
   - Drop bursts under machine contention came from the compositor cadence falling to about 53 Hz, not from the pipeline.
   - **Linux, Intel Gen12 iGPU (S3/S4, Docker on Xvfb, so indicative):**
     - WebGPU runs on Vulkan/ANV, not the fallback adapter.
     - WebCodecs decodes H.264, HEVC (incl. Main10), VP9 and AV1 on VA-API: 889–1,875 fps at 1080p, with the video engine verified busy.
     - The wgpu preview ran 1080p60 with 59.99 fps, 0 drops, ingest 0.3 ms, scrub p95 7.3 ms.
   - **Chromium switches on Linux:**
     - Wayland: none, because Vulkan switches crash the GPU process there.
     - X11 without DRI3: `--use-angle=vulkan --enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan`.
     - `--ozone-platform=headless` is unusable.
   - **HEVC on Linux:** Chromium has no software HEVC decoder there, so HEVC preview needs VA-API or a proxy.
   - **Without a GPU:** only the SwiftShader/llvmpipe fallback is available, behind unsafe flags. Such machines use proxies and server-rendered preview (D31).
   - A real-desktop Linux run with real vsync is still pending.
   - **Windows (RX 9070 XT, 4K 144 Hz HDR desktop): confirmed.**
     - WebGPU runs on D3D12, not the fallback adapter; ANGLE and the compositor run on D3D11; `importExternalTexture` works.
     - The 1080p60 preview ran at 59.96 fps with 0.06 % drops, ingest 0.2 ms and scrub p95 27 ms.
     - WebCodecs hardware decode at 1080p: H.264, HEVC Main/Main10, VP9 and AV1 at 969–1,206 fps. No software HEVC decoder.
     - WebCodecs hardware H.264/HEVC **encode hangs** (supported, but no chunks). Encode stays in `fin-codecs`, not WebCodecs.
     - Chromium on Windows ignores `powerPreference`: GPU choice on hybrid machines follows the Windows per-app graphics setting.
3. **§12.1–12.2 codecs (S5):**
   - Rust/objc2 AVFoundation and VideoToolbox are confirmed: 2.5k fps 1080p decode, HEVC Main10 HLG, 24× realtime short-GOP proxies.
   - The hardware decoder path is verified in use via a VideoToolbox session. 
   - **Software fallback encoder (S5b): SVT-AV1** (preset 10, CRF mode, closed GOP every 15 frames). On an M4 it ran 9–22× realtime at 540p, both with all cores and with 4 threads. It was +1.2 to +1.5 dB PSNR over rav1e at matched bitrate, with a 69 ms p95 software scrub. It adds 3.8 MB.
   - **rav1e is dropped** (0.4–1.3× realtime; doesn't scale with threads at 540p).
   - **libvpx VP9 `--rt`** (cpu-used 6–7) is the reserve for clients without AV1 decode.
   - **Licence:** SVT-AV1 is BSD-3-Clause-Clear + AOMedia Patent License 1.0, the same patent licence as dav1d and libaom. It is added to the cargo-deny allow list and `THIRD_PARTY_NOTICES` when adopted.
   - **Build:** from source at a pinned, vendored tag; never the binding's prebuilt download. Keep a minimal FFI as the fallback to the single-vendor binding.
   - **Linux x64 (Alder Lake, AVX2):** builds from source; 10.4× realtime all cores and 10.7× on 4 threads at 540p; PSNR matches arm64 within 0.02 dB; static lib 10.5 MB.
   - **Windows x64 (MSVC + NASM, Ninja):** builds from source; 17.2× realtime all cores and 16.0× on 4 threads; quality identical to Linux; static lib 12 MB.
   - **Windows Media Foundation (Rust, RX 9070 XT):**
     - hardware decode verified (D3D11 textures, video engine busy): H.264 1080p 1,119 fps, HEVC Main10 HLG 1,114 fps;
     - 540p short-GOP proxies via the AMD H.264 MFT at 18.1× realtime, keyframes exactly every 0.5 s.
     - `fin-codecs` binds the hardware encoder MFT explicitly by adapter: the default sink writer picked the iGPU's MFT and hung.
     - Media Foundation ignores MP4 edit lists, so our demuxer applies them (VFR → CFR is then exact).
     - **HEVC needs the HEVC Video Extensions**, the only HEVC decoder Media Foundation offers. Without them Finisher decodes HEVC through D3D11VA directly, as Chromium does. Not yet verified on a machine without them.
   - **Linux VA-API (Intel Gen12):**
     - decodes H.264, HEVC Main/Main10, VP9 and AV1;
     - encodes H.264, HEVC and VP9 (low-power VDEnc), with 540p proxies at 26.7× realtime; no AV1 encode.
     - `fin-codecs` on Linux uses libva FFI (MIT; `cros-libva` BSD-3 is an alternative) and keeps frames on the GPU: 10-bit download to the CPU is slow (174 fps).
4. **§13 transcription and alignment (S6):** the default English pipeline is:
   - **Parakeet-TDT 0.6B v2** (CC-BY-4.0, attribution) for the transcript;
   - **wav2vec2-base-960h** (Apache-2.0, fp32 ONNX; int8 costs 7–10 ms of p95) CTC Viterbi word spans;
   - a **learned start/end boundary correction** (S6b): gradient-boosted trees, 0.5 MB, ported to Rust. Features are CTC blank probabilities, band energies, energy edges and pause/letter context. It is trained on MFA-aligned LibriSpeech dev-clean, speaker-disjoint from test.
   - **Measured, held out (WER 1.9 / 4.1%; RTF ≈ 0.1 on CPU):**
     - start/end p95 vs MFA references: 46/49 ms on test-clean and 52/60 ms on test-other (round 1: 80/119 ms).
   - **Quality gate** (replaces "p95 ≤ 50 ms"):
     - ≥ 95 % of word starts and ends must fall within [−125, +45] ms of human-labelled boundaries (ITU-R BT.1359 detectability: a caption late by more than 45 ms, or early by more than 125 ms).
     - p95 against MFA ≤ 50 ms is tracked, not gated.
     - p95 ≤ 50 ms against humans can't be a gate: published evaluations put only 89 % of MFA's word boundaries within 50 ms of hand labels (Rousso et al., Interspeech 2024).
     - On MFA references (held out), 95.9 % of starts and 96.9 % of ends are in the window.
     - **Reference set (S6c–S6e):** the owner's reel (187 words) plus 12 ElevenLabs clips (en, uk, de, es, fr, pt-BR; 1,450 words). The owner checked English and Ukrainian word by word and corrected what was wrong; unchanged edges there are confirmed correct. The other four languages are unverified references (mostly the generator's own timing). Labels came from an independent prefill, never from the pipeline under test. ElevenLabs audio is evaluation-only: never used to train or tune. **Retired 2026-11-07** (sub-project 8 decision 6): the generated clips leave the evaluation set; permissive human speech is relabelled in their place, the owner's footage stays evaluation-only under a written licence, and MUSAN is filtered per file (PD/CC0/CC-BY).
     - **English, measured vs those labels:** the pipeline above puts 95.7 % of starts but only 75.7 % of ends in the window (reel 89.8 / 70.6). Every miss is late: the boundary correction, trained on MFA conventions, extends ends about 60 ms past where people place them.
     - **Decision:** starts use the learned correction; **ends use the raw CTC span end** (95.7 / 91.5 % pooled, 89.8 / 83.4 % on the reel). This was chosen on a small set from about 40 candidates (listed in the archive), so expect some selection optimism.
     - **Next step (sub-project 8):** retrain the boundary correction on permissive public data with the gate window as its objective, with music mixed in (the reel's misses are under background music). Fitted end offsets reached 96.8 % in leave-one-clip-out, which shows the headroom.
     - Rejected on these labels: Parakeet's native timestamps, whisper cross-attention timing, NeMo Forced Aligner (80 ms frames), larger CTC models (no gain beyond noise at 3–5× the cost), VAD/energy edge snapping and speech enhancement before alignment. CrisperWhisper (NC weights) and whisper-timestamped (AGPL) are excluded on licence.
     - Digit words ("6", "$20", "7:30") get no timing from CTC; numbers are spelled out per language before alignment. Ukrainian needs grammatical case agreement (a permissive verbaliser is still to build).
   - whisper.cpp alone is not acceptable for per-word captions. Qwen3-ForcedAligner was evaluated and rejected: 80 ms timestamp steps.
   - **Multilingual (uk, de, es, fr, pt-BR): one shared stack, no per-language models.**
     - Transcript: whisper large-v3-turbo (MIT) with the language forced: 2.3–3.5 % WER on every language. Parakeet v3 is rejected for non-English: it can't force the language and switched to English mid-clip in French (30 % WER).
     - Alignment: the same wav2vec2-base-960h CTC, with accents stripped and Cyrillic transliterated. It beat every permissive per-language XLS-R model and a phoneme CTC model in every language.
     - Learned start correction, raw CTC ends (as in English): starts 98–100 % and ends 94–98 % in the window against the references; Ukrainian and French ends miss the 95 % line by about 1 point, all at pauses.
     - Runs on CPU via ONNX Runtime at about 0.14 real-time factor with whisper turbo. GPU is not needed (DirectML was 3–4× slower than CPU on an RX 9070 XT).
   - MMS models are excluded (CC-BY-NC).
5. **§8.2 providers (S7):**
   - Claude Code is launched with `-p --input-format stream-json --output-format stream-json --verbose --permission-prompt-tool stdio --setting-sources project`. `--permission-prompts host` alone auto-denies.
   - Codex is driven over newline-delimited JSON-RPC (`initialize`, `initialized`, `thread/start`, `turn/start`, `*/requestApproval` → `accept|decline`, `turn/interrupt`, `thread/resume`).
   - Adapters bind events and errors to the emitting process, ignore late messages from a replaced process, and fail the pending turn on spawn failure or exit.
6. **§8.4 sandbox (S8):**
   - Protected roots (`.finisher/`, `assets/`) deny **both file-write and file-link**; this closes hardlink escapes.
   - Writable roots: `workspace/`, temp dirs, and each provider's own state dirs (`CLAUDE_CONFIG_DIR`/`~/.claude`, `~/.codex`, caches).
   - The sandbox, not the approval policy, is the guarantee: with a fooled allow-all policy only the sandbox blocked writes.
   - **Linux Landlock (ABI 4, kernel 6.8): confirmed.** 90/90 escapes blocked; unsandboxed, 7/7 succeeded.
     - Landlock only adds access, so a protected root can't sit inside a writable root. Write roots are enumerated around protected roots.
     - Each agent gets a private `TMPDIR` inside a write root instead of `/tmp`.
     - A hardlink denial surfaces as EXDEV. TCP can be filtered; UDP can't.
     - Real agent turns under Landlock still need logged-in CLIs on a Linux host.
   - **Windows AppContainer (normal, not LPAC): confirmed.** 90/90 core escapes blocked, plus Windows-specific paths. Those are junctions, ADS, 8.3 names, `\\?\` paths, admin shares, WMI and `schtasks`; all succeed unsandboxed.
     - LPAC is rejected: it breaks node, curl, git, PowerShell, ConPTY and Codex.
     - **Allow-only ACLs.** Deny ACEs and integrity labels don't stop an AppContainer (17/25 and 9/25 escapes). Protected roots sit outside write roots. If nesting is unavoidable, the protected root stops inheriting, and the parent loses delete-child.
     - **One container profile per project** (`finisher-<project-uuid>`, never reused), granted once. Launch takes 35 ms against 3.1 s when granting and revoking per launch on 10k files.
     - **ACL changes go through one broker in `finisherd`**, with a write-ahead log and a startup sweep. Concurrent read-modify-write lost a grant, and a crash leaves ACEs behind (5/5).
     - **Per session:** a private TEMP/home, a private window station and desktop (CLIs don't start without one), a kill-on-close job, and only the standard handles. `fin` talks to `finisherd` over a named pipe, because loopback TCP is blocked.
     - Imported assets are copied, not moved: a moved file keeps its old ACL.
     - **Open: Claude Code's Bash tool.** Git Bash/MSYS can't start in any AppContainer; PowerShell and cmd work. Decide before Windows ships: PowerShell-only, a non-MSYS shell, or a brokered shell tool.
     - **Open:** a non-elevated desktop user, and real agent turns.
     - A restricted token is not better: no network isolation, and it breaks DNS, ConPTY and PowerShell.
7. **§8.4 / §16 environment (S1, S7):** *every* child process (desktop shell → `finisherd` → workers, agents, plugins) starts from an explicit environment allow-list. Inherited `ELECTRON_RUN_AS_NODE`, `CLAUDECODE` and `CLAUDE_CODE_*` must never pass through.
8. **§23 packaging (S9):**
   - The signed bundle needs only `com.apple.security.cs.allow-jit`.
   - Release builds prune per-platform binaries (ONNX Runtime ships all platforms) and download optional components on demand; the spike bundle was 645 MB.
   - The shell build lists `node_modules` in `extraResources` explicitly, because electron-builder drops them silently.
   - **macOS: notarised.** Publisher is the owner's Apple Developer team.
     - The app is signed with that team's Developer ID Application certificate and notarised with an App Store Connect API key (`notarytool --key/--key-id/--issuer`, the same path CI will use).
     - Stapled; Gatekeeper reports "Notarized Developer ID"; all native parts launch.
     - Apple's queue took about 30 minutes, so release CI must not use a short notarisation timeout.
   - **Linux (Ubuntu 24.04):** AppImage 270 MB, deb 207 MB; napi, ONNX and Codex load.
     - Ubuntu's AppArmor user-namespace restriction disables the Chromium sandbox unless an AppArmor profile is installed. An AppImage can't install one and so runs with the sandbox silently off.
     - **Linux ships as deb/rpm with the AppArmor profile; the AppImage is secondary and warns.** AppImage also needs libfuse2.
   - **Windows:** a per-user NSIS installer, 220 MB (789 MB installed, of which Codex is 355 MB); napi, ONNX and Codex load.
     - electron-builder doesn't sign single-file `extraResources`. Native files ship inside directory entries so that every PE gets signed; the release check runs `signtool verify` on every PE (17/17).
9. **§6.7 storage (S10):**
   - **The round-1 check-then-write lease is rejected.** On SMB it split-brained:
     - 111/200 races from one Mac;
     - 6/50 from two machines on a fresh folder;
     - 50/50 with per-writer temp names.
   - **SMB 3.1.1, macOS + Linux clients: the fenced lease is required.**
     - **Claim:** epoch by exclusive create (`O_EXCL`), per-writer temp files, retried rename.
     - **Reads:** lease reads are uncached (`F_NOCACHE` on macOS, `O_DIRECT` on Linux). Default client caching held stale content for 14–36 s; uncached reads cut the worst lag to 60 ms.
     - **Takeover:** only after stale-after, plus a confirming re-read ≥ 1.5 s later.
     - **Fencing:** a check (no newer epoch claim) before every write and after every heartbeat; the holder self-fences at its stamp + 2×TTL. An unreadable lease (e.g. EACCES) counts as held.
     - **Results:** the hardened-but-unfenced design survived 200/200 cross-machine races but took over early in 12/24 kill -9 tests and let a stale holder clobber the new one in 10/10. The fenced design: 0/24 early takeovers and 0/20 stale writes.
   - **Defaults:**
     - SMB roots: TTL 10 s, heartbeat 5 s, stale after 30 s, lease polled every 2–5 s.
     - Local roots: TTL 5 s, heartbeat 5 s, stale after 15 s.
     - Durability uses plain `fsync`: `F_FULLFSYNC` is ENOTSUP on macOS smbfs.
     - Server ids are case-normalised (shares can be case-insensitive). `link()` is unavailable.
     - Warn on `cache=loose`, `actimeo` > 1 or SMB1.
   - **Clock skew: a lease never compares the local clock with server mtimes.** A Windows client 9.7 s ahead of the NAS split-brained the hardened lease in 105/105 races; with the clock corrected it passed 200/200.
     - Staleness is judged in server time: the server's clock offset is measured from the mtime of a freshly written probe file, or by heartbeat-counter progress.
     - Fencing still prevented every stale write under skew.
   - **Windows SMB client:**
     - `create_new`, hard links and no-replace rename each had exactly one winner in 100/100 races.
     - Rename-over fails with ACCESS_DENIED while a reader holds the file open, so lease readers open with delete sharing and writers retry.
     - Directory fsync isn't possible on Windows (ACCESS_DENIED), so it is a no-op there.
   - **Windows ↔ Mac on one share: the fenced lease above is not enough. The lease is revised as follows (spike "v3", the required design):**
     - **Why:** after a Mac rename-over of `lease.json`, Windows reports the file *missing* for up to 5 s (its file-not-found cache; `FILE_FLAG_NO_BUFFERING` doesn't help), so the fenced lease treated a live lease as absent: 5/12 early takeovers even with correct clocks, 3 of them of a live holder. A newly created claim file can stay invisible to Windows `stat`/readdir for up to 9.9 s. Under real clock skew (+9.9 s) the fenced lease took over early 12/12, and the 1.5 s confirm re-read missed a live holder at TTL 10 s.
     - **Record:** `lease.json` is created once, then always rewritten **in place** as a fixed 512-byte record with a checksum (never rename-over). In-place writes were visible to the other host before the writer's call returned. Reads stay uncached.
     - **Missing or unreadable:** a missing `lease.json` while any `lease.claim-*` exists counts as held, never absent. An unreadable or bad-checksum record counts as held when acquiring and as **lost** when fencing.
     - **Epoch:** the record's epoch is authoritative; claim-file existence is only a secondary signal (it can lag 10 s on Windows).
     - **Time:** staleness is judged in server time. The offset is measured from a freshly written probe file's mtime and re-measured periodically (a synced host still drifted about 29 ms/h). A takeover is refused when the probe's round trip exceeds TTL/4.
     - **Result (v3):** 300 cross-host races with 0 double holders, 0/48 early takeovers and 0/50 stale writes, under emulated +9.9 s skew and after the clock was corrected. The skew emulation reproduced the real-skew behaviour (4/4).
     - The defaults above hold (TTL 10 s, heartbeat 5 s, stale after 30 s, 1.5 s confirm, self-fence at stamp + 2×TTL). Overshoot was 1.45–3.3 s.
     - **Still to validate:** v3 with Linux clients, under network loss, with a real (not emulated) skew, and with segment writes in flight.
     - **Clocks:** Windows did eventually sync on its own, but only after running 9.9 s off for at least a day with "Set time automatically" on. The design assumes any client clock may be wrong by seconds and never asks users to fix it.
   - **Conflict detection stays mandatory.** A write already in flight during network loss landed 10 s after takeover. Segments carry their epoch; duplicate sequence numbers, lower epochs, sync conflict copies and unknown server ids put the project read-only and prompt recovery.
   - **Media on a NAS:** a 1080p H.264 reel decoded at 650–686 fps over SMB (about 66 MB/s; first frame 0.11–0.23 s) against about 2,300 fps locally, and proxy speed was similar. Proxies stay local.
   - **Library scans on a NAS:** about 60k files take 161 s with 32 parallel stats against 450 s serially. Full scans run on open or on demand, never continuously; network roots are polled by directory mtime every 60 s.
10. **§12.5 Lottie (S11):**
    - thorvg WASM is confirmed for template preview (≤ 2.9 ms per 1080p frame, 77.6 dB vs native).
    - The preview must size the Lottie render target from the intended pixel size, not the canvas bounding rect.
    - **Text slots (S11b): thorvg text is not used for slots.**
      - Slot text is laid out and rasterised by the shared Rust text engine (`fin-text`: cosmic-text + swash, the same path as captions). It is oversampled for animated scale and injected as an image slot.
      - The spike's `fin-text` rendered native and wasm32 bit-identically in 10/10 cases, with Arabic shaping, bidi, kerning and font fallback.
      - Its output is also byte-identical across x86-64 Linux and arm64 macOS (20/20), on condition that fonts are always bundled, never taken from the system. *Since 4C (item 15): always pinned bytes, fetched on demand, never taken from the system.*
      - Why not thorvg: it lacks shaping, bidi, kerning, colour emoji, per-glyph font fallback and OTF/CFF fonts. Its text layout also differs across versions, and a wrapped 7-line box took 12 ms per frame.
      - Per-character animators work only on baked-glyph templates.
      - **Colour emoji is open:** swash has no COLRv1, so this needs a bitmap or COLRv0 font, or a COLRv1 painter.
    - Native thorvg is pinned to the thorvg version inside the shipped dotlottie-web (v1.1.x). A golden-frame test checks it: v1.1.1 is pixel-identical to dotlottie-web 0.81.0, and the legacy 1.0.6-dev pin is not.
    - Hosts validate that every font a template names is registered; unregistered names fall back differently per runtime.
    - Template JSON is validated, and export runs in the sandboxed worker, because thorvg segfaults on malformed `chars`.
11. **Validation still outstanding** (blocked on hardware or accounts, not design):
    - Windows: the cross-host SMB race against the Mac, S8 as a non-elevated user with real agent turns, an iGPU/hybrid run, and HEVC without the extensions;
    - a Linux real-desktop run (vsync, Wayland/X11 session) and Linux S3b latency;
    - sync-service rows of S10;
    - These gate the cross-platform claims of D4/D31 before sub-project 6.

12. **Sub-project 2 decisions:**
    1. **Schema direction:** Rust `fin-core` is the source of truth for model, op, diff and error types → `schemars` JSON Schema (`packages/core-types/schema/fin-core.schema.json`, committed) → generated TS (`packages/core-types/src/generated.ts`, committed). Effect Schema defines only the RPC envelopes and treats transactions/snapshots as JSON validated by Rust.
    2. **Rational JSON form:** a string `"n/d"` in lowest terms with `d > 0`, or `"n"` when `d == 1` (e.g. `"1001/30000"`, `"12"`). Parsing rejects floats.
    3. **Entity IDs:** short, readable, project-global: `a<n>` assets, `t<n>` tracks, `c<n>` clips, `tl<n>` timelines. Allocated by `fin-store` from one per-project counter table at commit time (one writer per project makes this unique across branches). Transactions refer to entities they create with placeholders `$<name>`; the committed transaction and the response carry the resolved bindings.
    4. **Editing semantics (sub-project 2):** all three clip ops are **ripple** edits on one track (later items on the same track shift; other tracks untouched). Inserting strictly inside a clip is rejected (`INSERT_INSIDE_CLIP`, fix: insert at the clip's start or end). Minimum clip duration is one frame at the timeline rate.
    5. **Local transport:** Effect RPC with NDJSON serialization on a Unix socket `<server-data>/fin.sock` (mode 0600) or the Windows named pipe `\\.\pipe\finisher-<username>` (for the default server-data; any other server-data uses `finisher-<username>-<hash>`, 12 hex chars of SHA-256 of its lower-cased path, because pipe names are machine-global — clients read the name from `finisherd.json`); WebSocket on `127.0.0.1` (port written to `<server-data>/finisherd.json`, mode 0600, with a random bearer token the WS client must send; **superseded by item 18**: the token is gone, a WebSocket needs a one-time ticket minted over a proved connection, and the socket moved to `<server-data>/run/fin.sock`). Pairing/remote auth is sub-project 5.
    6. **Server data dir:** `FIN_SERVER_DATA` if set; else macOS `~/Library/Application Support/Finisher`, Linux `${XDG_DATA_HOME:-~/.local/share}/finisher`, Windows `%LOCALAPPDATA%\Finisher`.

13. **Sub-project 4A, durable storage (as built, 2026-11-05):** the design of §6.7 and §6.8 as implemented is recorded in `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §3, which wins over this document where they differ.
    - **The §27.9 "v3" lease is implemented** (`fin-store::lease`), with these amendments (sub-project 4 spec §3.5):
      - stale after 45 s of server time on every storage kind, not 30 s;
      - takeover needs two stale probes ≥ 5 s apart, each with a round trip ≤ TTL/4, *and* the identical record seen for ≥ 25 s on the observer's monotonic clock. A live NAS holder is never taken over, even with `--confirm`;
      - probe samples landing more than 1 s below the extrapolated server time are rejected;
      - the monotonic clock counts time asleep;
      - a claim above the holder's epoch that is younger than 45 s loses the lease (claims are the authority on epochs), and each acquisition creates a single claim at `base + 1`;
      - a commit trusts a full check that passed within the last 1 s;
      - a restart re-takes its own lease at once through an OS lock in server data, never in the project folder.
    - **The folder:** hash-chained, write-once segments labelled by commit epoch; reader-side epoch rule with rewind of a lost applied line; quarantine instead of deletion; rebuild with salvage; verify; daily and pre-migration backups; two-phase compaction into packs that keep full history.
    - **§6.7 polling:** project folders are polled (local 2 s, network 1 s, sync 10 s), not watched, and the heartbeat is 5 s, not 15 s.
    - **Still outstanding:** cross-host and sync-folder validation (sub-project 4 spec §3.13). This replaces the "Still to validate" line of item 9 for the lease.

14. **Sub-project 4B, history (as built, 2026-11-06):** the design of §6.3–6.6 for branches, undo, merge and live sync as implemented is recorded in `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §4 (with the §3 places it changed), which wins over this document where they differ.
    - **All branch state is derived from the log** (`branch` and `branchSet` records on the parent's log; `merged` from merge copies), so branches sync through the folder like edits; branch ids are `br<n>`, names are display only. A branch made under one id on two servers across a takeover is told apart by its `branch` record (epoch, then hash).
    - **§6.4 amendments:** undo and redo append records and never truncate; a redo is a unit of its own; stacks are dropped only when one of the branch's records is copied by a merge *from that branch into a branch that is not its descendant*, never by a cherry-pick; merges are transitive and judge "already there" by origin root; merged copies keep the source's entity ids and are one unit of the merging author on the target. Merge conflicts are returned as a list (`CONFLICT` `details.conflicts`); turning them into review items is sub-project 5.
    - **Rebase (§6.5):** positions map through the item edge they were written against (sub-project 4 §4.3 *Anchor mapping*, 2026-11-09), falling back to exact per-track shift maps with ProseMirror-style mirrored mappings; per-entity conflicts (`REMOVED`, `SAME_EDGE`, …); every op re-run at the head with full policy.
    - **§6.8 branch archiving:** archived branches are kept and hidden; squashing them is deferred beyond sub-project 4.
    - **Rewinds cascade** to descendant branches whose `branch` record was lost; quarantined records come back as `recovered-*` branches (`fin project recover`).
    - **Still outstanding:** cross-host validation (sub-project 4 spec §4.8).

15. **Sub-project 4C, edit breadth (as built, 2026-11-07):** the model, edits, evaluation, captions and text measurement of §6.2–6.4, §7, §9 and §12.3–12.7 as implemented are recorded in `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §5 (with the §3 and §4 places it changed), which wins over this document where they differ.
    - **Model and edits:** several timelines, bins and markers; video, audio and caption tracks; items of kind clip, caption, graphic, image, adjustment and nested with common fields, effects, keyframes and link groups; transitions at cuts; a settings registry behind generic `param.set`, `keyframe.*` and `effect.*`; structural edits with explicit modes, each with an exact inverse, a footprint, a shift map and a content map. Schema version 2, with frozen upcasters, so every earlier project opens with the same state.
    - **Evaluation:** `fin-core::eval` produces deterministic frame and audio plans (`f64` with `+ − × ÷ sqrt` only, a float gate, a plan-hash corpus byte-identical on Linux, macOS, Windows and wasm32). The compositor and mixer of sub-project 6 consume only these plans.
    - **§27.10 amended: fonts are fetched and pinned, not bundled.** `fin-text` (cosmic-text, no swash) measures from fonts named by SHA-256: projects pin exact files, the server fetches them on demand from a pinned, licence-gated catalog into a content-addressed cache, and a missing font leaves a caption unmeasured without blocking an edit. Item 10's condition for identical output, "fonts always bundled, never taken from the system", now reads **"always pinned bytes, never taken from the system"**.
    - **Still outstanding:** cross-platform validation (sub-project 4 spec §5.10).

16. **Sub-project 5C-1, the job engine (as built, 2026-11-08):** the shared job system of §9.9 as implemented is recorded in `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` §6 (§6.0–6.6 and §6.8–6.12; §6.10 maps every decision of its plan to As built or Deviation), which wins over this document where they differ.
    - **Shape:** `fin-jobs`, `fin-jobhost` and `fin-workers` (§7); spawning in `finisherd`'s process through `fin-node`, behind a `Spawner` hook that sub-project 7's `fin-sandbox` implements; jobs persisted in `control.db`; a pure, property-tested scheduler over resource pools; warm workers, helpers and mapped files; retries, poison handling and isolation; `job.done` and `worker.exit` into the diagnostics journal.
    - **Kill trees are host-owned on every OS:** a host, group and leaf Job Object hierarchy on Windows (processes created suspended, spawned through a broker so no pipe handle is ever inheritable), delegated cgroup v2 on Linux where available, process groups elsewhere, and on Unix a watchdog process that reaps the groups if the host dies.
    - **Deviations from this document and from the sub-project 5 design:** with process groups (macOS; Linux without a delegated cgroup) a `setsid` grandchild survives cancel and host death until sub-project 7 denies process creation to workers; a graceful stop spends no attempt of a resumable job; warm kinds run cold under a sandbox until 7C grants per-job paths.
    - **Still outstanding:** the cross-platform run of the whole engine on macOS and Windows 11 (5C-1 Task 14), the held-files check there, an endpoint-security check of the Windows spawn broker, and the job RPCs (5C-2).

17. **Sub-project 8A, packages, registries and lifecycle (as built, 2026-11-08):** the plugin manifest, `.finpkg` packages, local-folder TUF registries, install plans and approvals, bundled plugins and the first content consumers of §14 and §15 as implemented are recorded in `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §4–6 and §23 (§23.1 maps every decision of its plan to As built or Deviation), which wins over this document where they differ.
    - **Shape:** `fin-plugin-api` (pure), `fin-registry` (the TUF client and editor over `tough` 0.24 on `aws-lc-rs`, the strict package reader, the store, plans), `fin-plugin` (publisher tooling, not shipped); `fin-node` runs the registry on its worker thread; `finisherd` keeps registries, plugins, grants and plans in `control.db` and drives **plan, then approve, then install**. Nothing a plugin ships runs in 8A: font catalogs and caption styles are active at install, and code plugins install disabled until 8B.
    - **Amendments to §14 and §15:** git registries are local folders (§15); packages carry no signature of their own; third-party code plugins install behind a high-risk `thirdPartyCode` warning and an explicit acceptance on every install and update (§14.4, D8); only a person's plan can be approved, and only a person approves it; registry folder paths are shown to people only; `fin-plugin validate`, not `fin plugin validate`, is the offline check.
    - **Still outstanding:** the cross-platform run of 8A on macOS and Windows 11 (Task 15), the code-signature check of bundled plugins (`bundledIntegrity` is `hashesOnly` until sub-project 11), the first-party signing-key ceremony, and `finisherd --plugin-dir` and the code runtimes (8B).

18. **Sub-project 5A, principals, local credentials and scopes (as built, 2026-11-08):** §16 (auth, security), §21.1 (sources and human-only operations) and item 12.5's local transport as implemented are recorded in `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` §3 and §4 (§4.15 maps every decision of its plan to As built or Deviation), which wins over this document where they differ.
    - **Trust comes from credentials, never from transports:** every connection proves a key in a mutual HMAC handshake bound to the environment id and the canonical endpoint (`local.key` for `fin` and the user's tools, `console.key` for `finisherd <command>`, a derived key for each `agent` and `mcp` credential); keys rotate at every start; `fin` sends nothing without a credential and checks the server's proof before anything else. The Unix socket lives in `<server-data>/run/` apart from `<server-data>/auth/`; on Windows `fin-node` serves the named pipe itself with its own security descriptor and peer checks. The WebSocket has no bearer token: it needs a one-time ticket in `Sec-WebSocket-Protocol`, behind Host and Origin checks. Protocol 2.
    - **One middleware chain** (authenticate, resolve, authorise, policy, confirmation, handler, after) gives every RPC its scopes, all required; references are resolved by the store before any rule; branch limits and a revoked credential are re-checked inside the store's write transaction. `fin` gains exit code 7 (denied).
    - **§21.1 amended:** sources and actors come from the principal, not the transport (`console` and `mcp` are new sources); consent changes, purges, `project default`, content exports, confirmed takeovers and relocations are human-only, done through a TTY-gated console that asks for a typed, server-issued code; training is turned on only by a device (the app's onboarding), by hand in `settings.json` or through `FIN_TRAINING`; exports carry content only on request.
    - **Not a boundary yet:** until sub-project 7's sandbox (7C), an unsandboxed agent harness runs as the owner and can read `auth/console.key` and `auth/local.key`, so human-only operations and credential scopes bind only harnesses that use `fin` or MCP (owner decision 6, documented in the README).
    - **Still outstanding:** the cross-platform run of 5A on macOS and Windows 11 and its measurements (5A Task 14; Windows has run only the pipe listener's own tests), pairing and devices (5B).
19. **Sub-project 6A, media foundations (as built, 2026-11-08):** the first sub-batch of the media engine as implemented is recorded in `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` §25 (§25.1 marks every decision As built or Deviation), which wins over this document where they differ.
    - **Shape:** `fin-media`, `fin-container` (over `mp4-atom`), `fin-codecs` (dav1d decode and SVT-AV1 encode, each behind a C shim), `fin-compositor` (a wgpu 30 compositor beside the CPU reference that is its oracle), `fin-shm` (the mapped-file channel); the `media` family of `fin-workers` with `media.look` and `media.capabilities`; `fin look` and `fin media caps`; media fields in `job.done` and the `media.capabilities` and `render.stats` diagnostics kinds.
    - **Amendments to §12.1 and §27.3:** AV1 decodes with dav1d (built with the `cc` crate; `rav1d` was measured and dropped); mip levels are separate textures; a frame's file name carries its size; workers write and the host moves outputs through `fin-shm`, by copy from a warm worker.
    - **Still outstanding:** the cross-platform run of 6A on macOS and Windows 11 and a hardware Linux GPU (spec §25.6), a green CI run on GitHub, the worker sandbox (6B-1 Landlock, 6G the full profile).
20. **Sub-project 7A, the agent runtime core and the built-in agent (as built, 2026-11-08):** §8 (sessions, providers, the built-in agent, budgets), §9's `FIN_CALL` and agent credentials, §16's `ModelGateway` and secrets, and §21.1's agent kinds as implemented are recorded in `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §1–13 (each 7A part marked *As built (7A)*) and §24 (§24.1 maps every decision of its plan to As built or Deviation), which wins over this document where they differ.
    - **Shape:** the runtime in `finisherd` (`apps/finisherd/src/agent/`): sessions, runs and nodes in `control.db` (the `agent` migration chain), interrupted on a crash and resumed without re-running a tool; one adapter contract with the scripted `fake` harness and the built-in loop over one OpenAI-compatible Chat Completions client with an OpenRouter preset (the pasted key, the only secret Finisher stores) and a local preset (no key); the tools `fin` (argv, no shell) and three workspace file tools.
    - **Sessions are principals:** each has its own root branch (`agent.<role>-<n>`) and an `agent` credential whose id is the session id, scopes from its mode (`plan`, `draft`, `auto`), writes confined to its branch subtree and paths to its project, its workspace and its project's libraries; `fin` under that credential never falls back to `local.key`, keeps its default branch and tries per session on the server, and takes its project folder from `FIN_PROJECT_DIR`; the server sets every credential principal's author and records every agent commit in `session_txs`, kept for the project's life.
    - **Recording:** metadata to the diagnostics journal, content only under training to the separate agent-trace store, with provenance on every session and turn; outcomes (merged, undone, reverted, abandoned) are tracked for 5E's `review.verdict` (registered at version 1 by 5E, so the tracker now emits it; outcomes stored before that are not back-filled, §27 item 25); the counsel gate `training.closedModelTargets` exists with the default `pendingCounsel`.
    - **Not a boundary yet:** before 7C every host runs agent tools unsandboxed, with the persistent warning `SANDBOX_DISABLED {reason: before7C}`.
    - **Still outstanding:** real-provider sessions and the cross-platform run on macOS and Windows 11 (7A Task 15); 7B–7H.

21. **Sub-project 5D, the library indexer (as built, 2026-11-08):** the library design of §6.7 and §20 (scanning, volume identity, placeholders, the catalog, stages, search and lookup) as implemented is recorded in `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` §7 (§7.8 maps every decision of its plan to As built or Deviation), which wins over this document where they differ.
    - **Shape:** `fin-library` (types, raw paths, enumeration per OS, the walker, the safe opener, probe and hash entry points; no process of its own), the `library` family of `fin-workers`, root probes in `fin-node`; in `finisherd` one catalog writer thread and one reader thread, with `control.db` holding only the `libraries` table.
    - **Every reader of a library file uses `fin_library::open_for_read`** (raw name chain, no links followed, placeholders refused, identity checked), enforced by clippy `disallowed-methods` in the crates that read originals; sub-projects 6 and 8 follow the same rule.
    - **Deviations:** no listing refresh on macOS shares (libraries are read-only); Linux FUSE sync roots are metadata-only unless `hydrate`; pools per volume with a separate poll slot; library aliases never grant reach.
    - **Still outstanding:** cold scans, the macOS and Windows end-to-end runs and the share figures (5D Task 17).
22. **Sub-project 7C, the OS sandbox (as built, 2026-11-09):** §8.4 as implemented is recorded in `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §25 (what is built, the deviations, the `READY` table and the residual risks), which wins over this document where they differ.
    - **Shape:** `fin-sandbox` builds one `SandboxPolicy` per process from typed inputs and applies the `agent`, `hook`, `worker` and `plugin` profiles: Linux Landlock (ABI ≥ 3) through the launcher `fin-sandbox-exec` (which stays the parent as a subreaper), a worker's self-sandbox or a plugin's `pre_exec` steps; macOS Seatbelt; Windows AppContainer with per-session capability SIDs, job objects and the ACL broker in `fin-node`. 5C's job host spawns through `SandboxSpawner`; warm workers get per-job grants (Linux); `fin` reaches the server from a Linux sandbox over the loopback agent listener (`tcp:`).
    - **Deviations:** the seccomp filter is deferred to a person (owner decision 2026-11-08), so Linux pathname sockets are hidden from agents and hooks by a namespace (where user namespaces work) instead of refused; the Windows suites prove the configuration and compute denials with `AccessCheck` (no escape attempts, owner decision); Node tools with piped stdio fail fast in a Windows container (`WINDOWS_NODE_PIPES`).
    - **Not a boundary yet by default:** `fin_sandbox::READY` is empty until the validation hosts pass each profile (7C Task 16), so a release build still runs every profile unconfined with `SANDBOX_DISABLED {reason: profileNotReady}` (item 20's `before7C` is gone). Supersedes item 18's and item 20's "Not a boundary yet" on each OS once its profiles are in `READY`.
    - **Still outstanding:** Task 16 (validation on the three OSes), the seccomp filter, the Windows escape tests, the macOS launcher's `setsid` handling, per-job grants on Windows.

23. **Sub-project 5B, pairing, devices, remote access and environments (as built, 2026-11-09):** §16 (pairing, sessions, transport, secrets) as implemented is recorded in `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` §5 (§5.12 maps every decision of its plan to As built or Deviation, §5.13 lists the owner decisions and review fixes, §5.14 the residuals), which wins over this document where they differ.
    - **Shape:** one-time pairing offers (a `finisher://pair` link and QR code, a 256-bit secret in the fragment, at most 10 minutes) approved by the owner on the console or a device after comparing a six-digit safety code; devices are human principals holding every scope, proving with a P-256 key through DPoP (RFC 9449, server nonces) for 15-minute tokens and 30-second WebSocket tickets; revocation ends tokens, tickets and live connections at once.
    - **Transport:** remote access is off until `finisherd remote enable` or `FIN_LISTEN`; off loopback it is TLS only, with a self-signed certificate made from the environment's identity key (pinned by SPKI) or the owner's certificate files (verified normally, then the `envId`); per-listener Host and Origin checks and connection pools.
    - **Amendments to §16:** certificate rotation is re-pairing (`finisherd tls rotate`), not a statement signed by the old key; device confirmations: only a device's `server.stop` needs a challenge code, sent by the app after its own dialog; devices name server paths only inside allowed roots; copied server data is refused (`ENV_MOVED`) until `env adopt` or `env reset`; secrets live in the OS keychain when it answers without a prompt, else in private files, optionally wrapped with `FIN_SECRETS_KEY_FILE`.
    - **Still outstanding:** the cross-platform and cross-host run (5B Task 15); sub-project 9's device key storage, pairing pages, `TLS_PIN_MISMATCH` and the `server.stop` dialog.
24. **Sub-project 8B, the plugin runtime (as built, 2026-11-12):** the plugin host as implemented is recorded in `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §3 and §24 (§24.1 maps every decision of its plan to As built or Deviation), which wins over this document where they differ.
    - **Shape:** a code plugin is a worker of one `plugin` family on the `fin-jobs` envelope (not a separate JSON-RPC stream, amending D8 for plugins), keyed by plugin, version, project and effective authority; commands are `plugin.command` jobs committed through the chain's `tx.commit` under the invoker with `Tx.via`; host calls are `project.snapshot`, `project.view`, `log`, `net.fetch`, `net.read`; the Rust and TypeScript SDKs and developer mode (`finisherd --plugin-dir`) are built.
    - **Amendments to §16:** no plugin is OS-confined before 7C's `plugin` profile is enforced on an OS (the sandbox wording of §16 should say so, plugins spec §22 item 23); egress is brokered HTTPS with the address checks of §16 and no secret injection until 8G; `PLUGINS_UNCONFINED` shows while any enabled code plugin runs unconfined.
    - **Still outstanding:** confinement (Task 16, on 7C), hook handlers (Task 17, on 7F), the `fin-plugin-test` harness (Task 14), the cross-platform run and budgets (Task 19), and the open owner question on an empty plugin transaction.
25. **Sub-project 5E, the review workflow (as built, 2026-11-12):** §6.4 ("conflicts become review items") and §11.3 (per-transaction accept/reject) as implemented are recorded in `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` §8 (§8.8 maps every decision of its plan to As built or Deviation, §8.9 lists the owner decisions and review fixes, §8.10 the residuals), which wins over this document where they differ.
    - **Shape:** a `branchReview` item holds a change list planned at a snapshot of both heads; a person accepts or rejects each origin transaction, and a merge copies only what was reviewed (`include` and `sourceUpTo`, checked inside the merge's write transaction), so a commit that lands on the source afterwards is never merged by it. A rejection is a **`drop` record in the op log** of the target branch (record format 3 for that kind only; derived index `m0004_drops`), so it syncs, survives moves and rebuilds, and keeps a later plain `fin merge` from bringing the transaction back; `DROPPED` is reported in `excluded`, not `skipped`. Merge conflicts and plugin suggestions are review items too.
    - **Protected branches** (default `main`) refuse writes from principals without `branch.protected`, at the chain's policy stage and again inside the store's write transaction; `local` keeps the scope, agents and MCP credentials do not. Before 7C this binds credentials, not a raw shell that can read `local.key` (README "Security model").
    - **Auto merge** exists as a server-internal path with all its checks and tests, and enabling it answers `AUTO_MERGE_UNAVAILABLE` until sub-project 10 records QA-gate and critic verdicts. `review.verdict` v1 is registered, so 7A's outcome tracker emits.
    - **Still outstanding:** the cross-platform and mixed-version run (5E Task 13); sub-project 10's verdicts and the flag; sub-project 7's delivery of `changesRequested` as a turn.
26. **Sub-project 7B, subagents, roles, rules and approvals (as built, 2026-11-12):** §8.3 (roles, modes, command rules, approvals), §8.6 (saved prompts) and §9's `fin agent` and `fin approval` as implemented are recorded in `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §3–7, §9.1, §9.4, §13.1 and §15 (each 7B part marked *As built (7B)*) and §26 (§26.1 maps every decision of its plan to As built or Deviation), which wins over this document where they differ.
    - **Shape:** command rules (`allow|ask|deny` on command globs) from the `managed`, `local`, `project` (trusted only), `user` and `role` layers, combined deny > ask > allow and enforced in `finisherd`'s chain for every `agent` call against the exported registry (`packages/contracts/registry.json`); an `ask` for a running session is a durable approval card (`APPROVAL_PENDING`, exit 8) with one-time exact grants and `remember` writing a visible rule; roles are complete data (prompts, rules, tools, spawn); a session spawns children on child branches (`fin agent spawn|wait|ls|send|stop|rollback`) with budgets carved from its own, up to depth 3 and 8 live per root; per-server live limits queue runs; rollback to a run; compaction in the built-in loop; saved prompts for people.
    - **Deviations:** child and critic branches are named with `.` (`<parent root>.<role>-<n>`, `<reviewed>.critic-<n>`); a grant is consumed in `control.db` before the write (a crash loses it, never doubles a write; owner decision); a `review.request` leaves a notice instead of ending the run; only tokens and money are carved.
    - **Not a boundary yet:** rules and approvals bind calls through the server; before 7C's profiles are in `READY` an agent's tools run unconfined (item 22).
    - **Still outstanding:** cross-platform validation (7B Task 14); 7D/7E's permission mapping and provider rollback; sub-project 10's verdicts, `AUTO_MERGE_AVAILABLE` and the `autoMerge.roles|into` console flag, so nothing merges automatically yet; 7F's `PreMerge` and `BranchReady` hooks.
