# Finisher — Architecture Design

- **Status:** revision 3 (post-spike). Incorporates the Fable review (B1–B5, I1–I15), the NAS/cloud-folder requirement and the spike results (§27).
- **Date:** 2026-11-02
- **Scope:** Top-level architecture for the Finisher rewrite. Every numbered sub-project in §24 gets its own spec → plan → implementation cycle. This document fixes the boundaries between them.
- **Predecessor:** `video-creater` (working name, now the legacy reference app).

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
9. Plugins: full plugin system, with bundled plugins plus local-folder and git registries.

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
| D3 | Codebase | New repo `/Volumes/Storage/Projects/finisher`; legacy `video-creater` stays in place as read-only salvage reference |
| D4 | Server platforms | macOS, Windows, Linux (including NAS/Docker) |
| D5 | Collaboration | Async sharing first; foundation for multi-user editing |
| D6 | Deployment | Local by default (embedded in desktop); remote/headless optional |
| D7 | Agents | Agent CLIs as providers: Codex bundled (Apache-2.0), Claude Code user-installed (not openly licensed), plus a built-in fallback loop on user API keys |
| D8 | Plugins | First-class, out-of-process, any language, JSON-RPC; registry protocol from day one |
| D9 | Server language | Hybrid: TypeScript/Effect 4 control plane + Rust edit domain and media |
| D10 | Desktop shell | Electron |
| D11 | Rendering | One Rust/wgpu compositor, native for export and WASM/WebGPU for preview |
| D12 | Timeline model | Own versioned model, OTIO-shaped, native `.otio` adapters |
| D13 | Codecs | OS/hardware codecs only plus royalty-free AV1/VP9 software; **no LGPL/GPL in the shipped app** |
| D14 | Agent edits | Live draft branches + review/merge |
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
| D28 | Providers v1 | Claude Code via the user-installed `claude` CLI in stream-JSON mode (Agent SDK not bundled); Codex app-server bundled, T3's client vendored |
| D29 | Library indexing | Semantic embeddings run locally by default (SigLIP2 via ONNX); cloud embeddings (Gemini/Qwen) are opt-in per library |
| D30 | Built-in agent APIs | Anthropic (Messages API), OpenAI (Responses API), and one OpenAI-compatible adapter (OpenRouter, Ollama, LM Studio, others) |
| D31 | Minimums | macOS 14+ on Apple Silicon; Windows 11 and Windows 10 22H2 x64 with a WebGPU-capable GPU; Linux glibc 2.35+ (Ubuntu 22.04+). Intel Macs are not supported |
| D32 | Distribution | Direct download + auto-update: notarised DMG, signed Windows installer, AppImage/deb/rpm, Docker image for the server. No app stores in v1 |

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
4. One protocol definition, in Effect Schema. Rust op types are generated from the exported JSON Schema.
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
- **Analysis artifacts** attach to assets and are referenced by ops but are not timeline state: transcripts, alignments, scenes, embeddings, face tracks, mattes, loudness, beats.

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
| `fin-workers` | Worker binaries (decode, encode, render, Lottie, SVG, analysis) |
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
  - **Risk:** permission callbacks and other control requests must be implemented against the CLI protocol. The spike validates this (§24 #1).
- **Built-in agent (v1):** Finisher's own loop on user API keys, emitting the same events. Model APIs: Anthropic (Messages), OpenAI (Responses), and one OpenAI-compatible adapter covering OpenRouter, Ollama, LM Studio and similar endpoints. More can come via plugins.
- **Codex (v1, bundled):** `codex` app-server shipped with Finisher (Apache-2.0, verified at repo level on 2026-11-02; re-check binary dependencies of the pinned release at bundling time). The user signs in with their own OpenAI account or API key. The JSON-RPC client is vendored from T3's `effect-codex-app-server` and pinned to the bundled Codex version, so protocol drift is caught at upgrade time. A user-installed newer `codex` may be selected in Settings.
- More providers come via plugins.

### 8.3 Roles, modes, permissions

- **Roles:** name, prompt, model preference, allowed commands, budget. Examples: `critic`, `clip-finder`, `caption-stylist`, `colourist`, `mixer`. Plugins can add roles.
- **Modes:**
  - `plan`: read-only.
  - `draft`: own branch only.
  - `auto`: also renders and runs the critic within budget.
- **Rules:** allow/ask/deny on command patterns, enforced in the command registry for every transport.
- **Layered settings:** `managed` > `project` > `user` > `local`.
- **Plugin installation is human-only by default.** `fin plugin install` from an agent is `deny` unless the user explicitly allows installs from already-trusted registries. Agents may `search`/`info` and *suggest* a plugin, which appears as a UI suggestion and not as an approval card worded by the agent.

### 8.4 Sandbox: OS-enforced, not just policy

- The working directory is `workspace/<session-id>/`.
- **The agent process tree runs inside an OS sandbox** applied by `finisherd`:

  | OS | Mechanism | Rules |
  |---|---|---|
  | macOS | Seatbelt profile | writable `workspace/` + temp + provider config dirs; read-only project; deny `.finisher/` and `assets/` |
  | Linux | Landlock (bwrap fallback) | same |
  | Windows | restricted token / AppContainer with ACLs on `workspace/` | same; **feasibility validated in the spike** |

- Provider permission rules (Claude Code allow/deny, Codex `workspace-write`) are a second layer, not the guarantee.
- **Integrity regardless of sandbox:** assets are referenced by content hash, and the server verifies the hash whenever an asset is used. `.finisher/` and `assets/` are server-written only, so tampering is detected and rejected.
- **Environment allow-list:** `PATH`, `HOME` (or a per-session home), `LANG`/`LC_*`, `TMPDIR`, the provider's own config/auth dir variables, `FIN_SERVER`, `FIN_SESSION`. Everything else is removed.
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
   - Resolved values are always echoed.
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
3. **Markers** (`open/review/resolved`), per-transaction change list with accept/reject, A/B compare.
4. **Share page:** exported HTML with timecode comments that come back as markers. Hosted links come later through `ShareService`.

---

## 12. Media engine

### 12.1 Codecs

- **Traits:** `Decoder`, `Encoder`, `Demuxer`, `Muxer`.
- **Backends:**
  - macOS: VideoToolbox / AVFoundation
  - Windows: Media Foundation
  - Linux: VA-API, NVDEC/NVENC
  - software: dav1d, SVT-AV1, libvpx
- Muxing is in Rust.
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

- `mask` takes alpha from a matte artifact (luma-encoded greyscale video), with feather, choke, invert and keyframes.
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

- Modalities: image, video, voice, music, SFX, dubbing (for example ElevenLabs, OpenAI images, fal.ai).
- **Catalog as data:** models, parameter schemas, cost, terms, output licence.
- **Calls:** submit, poll, cancel, fetch. The job ID is persisted before waiting, so a paid call is never re-submitted.
- Credentials are passed per call via `ModelGateway`.
- Outputs go to `workspace/` and are registered with provenance.
- Budgets and approval rules apply.

### 14.4 Data, permissions, lifecycle

- **Data:** shared-memory frames and PCM; content-addressed outputs.
- **Permissions:** `project:read`, `project:write` (ops only), `media:read`, `net`, `gpu`, `fs:<scope>`, `hooks:modify`. Approved at install; re-approved when an update adds permissions.
- **Isolation:** cleared env, sandbox, scoped credentials (not env), lazy start, crash isolation, timeouts.
- **Python plugins:** **wheels only.** The package bundles a hash-pinned lockfile, and `uv` installs with `--no-build` and `--require-hashes`, so no sdist builds run at install.
- **Versioning:** `host-api` range enforced; per-project lockfile pins versions and hashes.
- **Developer loop:** `finisherd --plugin-dir`, `fin plugin validate`.
- **SDKs (MIT):** Rust, TS, Python. `fin-plugin-test`; conformance suites for codec backends, generation providers and matte producers.

---

## 15. Registry

- **v1:** bundled plugins, plus **local-folder and git registries**. The **public signed registry comes after v1**, with the protocol designed and implemented in the client from v1.
- **Package:** signed archive with manifest, content hash, publisher, licence (copyleft flagged), permissions, `host-api` range, platforms, previews.
- **Trust model: TUF** (The Update Framework):
  - Roles: `root` (offline keys, threshold signing), `targets` (delegated per publisher scope `@publisher/*`), `snapshot`, `timestamp`.
  - This gives key rotation, revocation, freshness and protection against rollback and freeze attacks, all serveable as static files.
  - Publisher keys are bound to scopes by `targets` delegations.
  - Optional Sigstore keyless signing with a transparency log for publishers via CI OIDC (after v1).
- **Git and local registries** use the same package format and signature verification. Trust is per-registry, configured by the user or team policy.

**Rules learned from npm, PyPI and crates.io**
1. No install-time code execution (wheels-only for Python, no build scripts).
2. Versions are immutable; yank only.
3. Publisher scopes, reserved names, typosquat checks.
4. 2FA plus signing, verified through TUF metadata.
5. No plugin-to-plugin dependencies.
6. Permission changes require re-approval; code plugins don't auto-update by default.
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
  - TLS remotely with a self-signed certificate pinned at pairing;
  - public TLS when hosted.
- **Certificate rotation:** the server signs a new certificate's fingerprint with the old key before rotating. Paired clients accept it automatically. A lost key means re-pairing, with a clear UI explanation.
- **Handshake:** protocol version (HTTP 426 on mismatch) plus capabilities.
- **SSH/WSL** launch and forwarding are owned by the desktop shell.
- **Connect relay** is in `finisher-cloud`; it is not in the data path.
- **Secrets:** OS keychain (encrypted file on headless servers and NAS); read only by `finisherd` at call time; never sent to clients, env, plugins (except per-call `ModelGateway` grants), logs or projects.
- **Outbound:** HTTPS only, size caps, allow-lists, hash pins.
- **Audit log:** pairing, tokens, grants, hook blocks, hook modifications, sandbox violations.
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

---

## 18. Licensing and third-party reuse

- **Finisher:** MIT (core, SDKs, protocol).
- **Shipped app:** no LGPL/GPL. Enforced by `cargo-deny` + a JS licence gate.
- **Claude Code:** never bundled; the user installs it (D28). No licence-gate exception is needed.
- **Codex:** bundled under Apache-2.0, with its licence and NOTICE shipped. Its bundled binary is included in the licence gate and in signing and notarisation.
- **Copyleft plugin repo** (`finisher-plugins-copyleft`): plugins that are copyleft **or pull copyleft dependencies**. Candidates:
  - WhisperX (BSD itself, but depends on ffmpeg via torchcodec)
  - PySceneDetect original (BSD, but its OpenCV/PyAV wheels bundle ffmpeg)
  - sentrysearch (Apache-2.0, but uses ffmpeg)
  - RVM-based matting (GPL-3.0)
  - an FFmpeg codec backend
- **Codec patents:** H.264/HEVC via OS or hardware only.
- **`THIRD_PARTY_NOTICES`:** T3 Code (MIT, vendored paths), Uncut (MIT, skills/rubrics), PySceneDetect (BSD-3, ported), TransNetV2 (MIT), sentrysearch (design reference), thorvg, resvg, whisper.cpp, sherpa-onnx, ONNX Runtime, LanceDB, dav1d, SVT-AV1, libvpx, OTIO.
- **Model licences** are recorded and shown at download; non-commercial models are excluded.
- **Fonts:** bundled caption fonts are OFL or similarly permissive only, with licences shipped. User fonts are referenced, not redistributed. Project bundles embed only fonts whose licence allows it.
- **Not used:** T3 name, logo and relay, Clerk, Palmier code.
- **Open item:** codec patent posture review with counsel before commercial launch.

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
- **Relinking:** sources are matched by fingerprint and then full hash across registered libraries. Library roots are mapped per machine (for example `/Volumes/NAS/Media` ↔ `\\nas\media` ↔ `/mnt/media`), stored per server, not in the project.
- **Remote ingest:** chunked, resumable uploads from a client to a remote server's project `sources/`, with progress and integrity checks.

---

## 21. Product plumbing

| Concern | v1 decision |
|---|---|
| Telemetry | Opt-in only, anonymous, documented events; off by default in self-hosted servers |
| Crash reporting | Opt-in; minidumps from Electron, server and workers; scrubbed of paths and prompts |
| Model downloads | Guided first-run: shows size and purpose, resumable, verified, pausable; app usable before models finish; offline after download |
| Offline clients | Read cached projections; edits queue only while the local embedded server is reachable (no client-only offline editing in v1) |
| Localisation | All UI strings externalised from day one (ICU messages); English at launch |
| Accessibility | Keyboard-operable timeline and inspector, focus management, screen-reader labels for controls, reduced-motion setting |
| Updates | Electron auto-update; `finisherd` and worker binaries ship with the app and are version-locked to it; headless servers update via package or Docker image; protocol version and capabilities prevent mismatches |
| Onboarding | Codex sign-in (bundled); detect Claude Code (optional); set up API keys, add a library, take a sample project tour, explain the codec capability report |
| Minimum platforms | D31: macOS 14+ Apple Silicon only (no Intel Macs); Windows 11 / 10 22H2 x64 with a WebGPU-capable GPU; Linux glibc 2.35+. Machines below minimum fall back to server-rendered preview or headless use |
| Distribution | D32: notarised DMG, signed Windows installer, AppImage/deb/rpm, server Docker image; auto-update; no app stores in v1 |
| Indexing cost and privacy | D29: local embeddings by default. Cloud indexing shows an estimated cost and upload notice per library before starting |

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
| 8 | Plugins + registry | Host, manifest, SDKs, local/git registries with TUF verification, bundled analyzers (transcription + alignment, scenes, search, reframe, audio), generation providers, codec-backend and matte interfaces |
| 9 | Frontend + Electron | Editor, libraries UI, timeline, transcript view, branch bar, conversation, review mode; macOS/Windows/Linux packaging |
| 10 | Reviewer + skills | QA gate, critic, markers, share page; Uncut skills/rubrics ported |
| 11 | v1 features + hardening | Clip finder, reframe, caption templates, Lottie template packs, onboarding, signing/notarisation, release |
| 12 | After v1 | Public registry, raw Lottie, background-removal plugin, mobile, `finisher-cloud`, copyleft plugin repo |

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
1. Codec patent posture review with counsel before commercial launch.
2. Per-language aligner and model choices, decided after spike #1.6 and licence checks.
3. Lease heartbeat interval and polling cadence defaults, tuned after spike #1.10.

**Product and business questions (not blocking the architecture)**
4. Trademark and domain clearance for "Finisher" before the name goes public.
5. GitHub: owning account or org, public-from-day-one vs private until v1, and when the legacy `video-creater` repo name is replaced.
6. Contribution policy: DCO sign-off (recommended) vs CLA.
7. Windows code signing: procure Azure Trusted Signing or an EV certificate. Apple Developer ID exists.
8. Which generation-provider plugins ship first-party in v1 (candidates: ElevenLabs, OpenAI images, fal.ai).
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
2. **§12.3 preview (S3):**
   - Frame ingest is `copyExternalImageToTexture` (one GPU copy, 0.4 ms p95).
   - The preview scheduler must be **cadence-aware**: it measures the real display/rAF interval and paces against it.
   - The measured A/V offset is scheduler-internal (mean −9.6 ms = video late). End-to-end A/V latency must be measured by capture before any display-latency correction is applied.
   - Drop bursts under machine contention came from the compositor cadence falling to about 53 Hz, not from the pipeline.
3. **§12.1–12.2 codecs (S5):**
   - Rust/objc2 AVFoundation and VideoToolbox are confirmed: 2.5k fps 1080p decode, HEVC Main10 HLG, 24× realtime short-GOP proxies.
   - The hardware decoder path is verified in use via a VideoToolbox session. The software AV1 fallback (rav1e: 1.22× and then 0.75× realtime on an M4) is not reliably realtime: it is a **background job, not interactive**. Evaluate SVT-AV1 in sub-project 6.
4. **§13 transcription and alignment (S6):** the default English pipeline is:
   - **Parakeet-TDT 0.6B v2** (CC-BY-4.0, attribution) for the transcript;
   - **wav2vec2-base-960h** (Apache-2.0) CTC forced alignment;
   - a constant bias calibration.
   - Measured: WER 1.9%, start p50/p95 19/80 ms, RTF 0.046.
   - The p95 target of 50 ms stays and requires an onset-refinement stage.
   - whisper.cpp alone is not acceptable for per-word captions.
   - Multilingual: Parakeet v3 or whisper.cpp + per-language CTC.
   - MMS models are excluded (CC-BY-NC).
5. **§8.2 providers (S7):**
   - Claude Code is launched with `-p --input-format stream-json --output-format stream-json --verbose --permission-prompt-tool stdio --setting-sources project`. `--permission-prompts host` alone auto-denies.
   - Codex is driven over newline-delimited JSON-RPC (`initialize`, `initialized`, `thread/start`, `turn/start`, `*/requestApproval` → `accept|decline`, `turn/interrupt`, `thread/resume`).
   - Adapters bind events and errors to the emitting process, ignore late messages from a replaced process, and fail the pending turn on spawn failure or exit.
6. **§8.4 sandbox (S8):**
   - Protected roots (`.finisher/`, `assets/`) deny **both file-write and file-link**; this closes hardlink escapes.
   - Writable roots: `workspace/`, temp dirs, and each provider's own state dirs (`CLAUDE_CONFIG_DIR`/`~/.claude`, `~/.codex`, caches).
   - The sandbox, not the approval policy, is the guarantee: with a fooled allow-all policy only the sandbox blocked writes.
7. **§8.4 / §16 environment (S1, S7):** *every* child process (desktop shell → `finisherd` → workers, agents, plugins) starts from an explicit environment allow-list. Inherited `ELECTRON_RUN_AS_NODE`, `CLAUDECODE` and `CLAUDE_CODE_*` must never pass through.
8. **§23 packaging (S9):**
   - The signed bundle needs only `com.apple.security.cs.allow-jit`.
   - Release builds prune per-platform binaries (ONNX Runtime ships all platforms) and download optional components on demand; the spike bundle was 645 MB.
   - The shell build lists `node_modules` in `extraResources` explicitly, because electron-builder drops them silently.
9. **§6.7 storage (S10):**
   - Lease acquisition is check-then-write, so not atomic. Conflict detection (duplicate segment sequence, sync conflict copies, unknown server ids → read-only plus a recovery prompt) is **mandatory**, not optional.
   - Defaults until NAS runs: TTL 5 s, heartbeat 5 s, stale after 15 s, network-root polling every 60 s.
10. **§12.5 Lottie (S11):**
    - thorvg WASM is confirmed for template preview (≤ 2.9 ms per 1080p frame, 77.6 dB vs native).
    - The preview must size the Lottie render target from the intended pixel size, not the canvas bounding rect.
    - Template **text slots** (fonts in WASM) are unverified and must be tested before shipping text templates.
11. **Validation still outstanding** (blocked on hardware or accounts, not design):
    - Windows and Linux rows of S1, S3, S4, S5, S8 and S9;
    - NAS and sync-service rows of S10;
    - notarisation (Apple agreement).
    - These gate the cross-platform claims of D4/D31 before sub-project 6.

