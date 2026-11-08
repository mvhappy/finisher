# Sub-project 2: Minimal core + server — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An agent can run `fin` against a local `finisherd` to create a project, add a media asset, and insert/trim/delete clips; every change is a validated, invertible, rebased op persisted in a local op log and streamed live to subscribers.

**Architecture:** `fin-core` (pure Rust, WASM-clean) owns the model, rational time, four ops (`asset.add`, `clip.insert`, `clip.trim`, `clip.delete`), apply/invert/rebase and the JSON Schema. `fin-store` persists committed transactions in SQLite on local disk. `fin-node` exposes store + probe to Node via napi-rs. `finisherd` (TypeScript, Effect 4) serves one RPC protocol over WebSocket and a local socket. `fin` is a Rust CLI that links `fin-core` for offline validation and talks to `finisherd` over the local socket.

**Tech Stack:** Rust 2024 (serde, schemars 1, rusqlite 0.41 bundled, napi 3 / napi-derive 3, clap 4, thiserror 2, mp4 0.14, blake3, interprocess 2), Node 24.22.2 (native TS type stripping, `node:sqlite`), pnpm 12, effect 4.0.x + @effect/platform-node 4.0.x, @napi-rs/cli 3, json-schema-to-typescript 16, vitest 5.

**Spec:** `docs/specs/2026-11-01-finisher-architecture-design.md` — §4 (topology, rules), §6.1–6.5 (layout, model, ops, concurrency), §7 (boundary), §9 (`fin`), §19 (errors), §22 (testing), §24 row 2, §27.1 and §27.7.

## Global Constraints

- MIT; no GPL/LGPL/AGPL/SSPL/EUPL dependencies (cargo-deny + JS licence gate, `pnpm gates`).
- Primitive ops live only in `fin-core`, **one module per op** (`crates/fin-core/src/ops/<op>.rs`: schema, validate, apply, invert, rebase).
- `fin-core` is pure: no I/O, no clocks, no randomness, builds for `wasm32-unknown-unknown`.
- TypeScript model types are generated from `fin-core`'s JSON Schema, never hand-written; CI fails on drift.
- Non-test source files ≤ 600 lines; tests ≤ 1500.
- Every error everywhere is `{ code, message, details, fix? }` (§19); codes are stable `SCREAMING_SNAKE` strings, never display text (§17.1).
- `fin`: agent-only, no prompts/pagers/colour; stdout = data (JSON), stderr = logs; writes return deltas only; `--dry-run` on every write; resolved values always echoed (§9).
- Time is rational. Timeline positions are seconds as `Rational`; never floats in the model.
- The live database lives at `<server-data>/projects/<project-id>/project.fdb`, never in the project folder (§6.1, D27).
- Child processes start from an explicit environment allow-list (§27.7).
- Every task runs `cargo test --workspace`, `pnpm test`, `pnpm gates` before committing; Conventional Commits; author `mvhappy <333925574+mvhappy@users.noreply.github.com>`; trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`. Node 24 via `source .superpowers/env.sh`.

## Decisions taken here (not in the spec; recorded in spec §27 by Task 0)

1. **Schema direction:** Rust `fin-core` is the source of truth for model, op, diff and error types → `schemars` JSON Schema (`packages/core-types/schema/fin-core.schema.json`, committed) → generated TS (`packages/core-types/src/generated.ts`, committed). Effect Schema defines only the RPC envelopes and treats transactions/snapshots as JSON validated by Rust.
2. **Rational JSON form:** a string `"n/d"` in lowest terms with `d > 0`, or `"n"` when `d == 1` (e.g. `"1001/30000"`, `"12"`). Parsing rejects floats.
3. **Entity IDs:** short, readable, project-global: `a<n>` assets, `t<n>` tracks, `c<n>` clips, `tl<n>` timelines. Allocated by `fin-store` from one per-project counter table at commit time (one writer per project makes this unique across branches). Transactions refer to entities they create with placeholders `$<name>`; the committed transaction and the response carry the resolved bindings.
4. **Editing semantics (sub-project 2):** all three clip ops are **ripple** edits on one track (later items on the same track shift; other tracks untouched). Inserting strictly inside a clip is rejected (`INSERT_INSIDE_CLIP`, fix: insert at the clip's start or end). Minimum clip duration is one frame at the timeline rate.
5. **Local transport:** Effect RPC with NDJSON serialization on a Unix socket `<server-data>/fin.sock` (mode 0600) or the Windows named pipe `\\.\pipe\finisher-<username>`; WebSocket on `127.0.0.1` (port written to `<server-data>/finisherd.json`, mode 0600, with a random bearer token the WS client must send). Pairing/remote auth is sub-project 5.
6. **Server data dir:** `FIN_SERVER_DATA` if set; else macOS `~/Library/Application Support/Finisher`, Linux `${XDG_DATA_HOME:-~/.local/share}/finisher`, Windows `%LOCALAPPDATA%\Finisher`.
7. **Out of scope here** (sub-project 4+): branches other than `main`, undo commands, segment export, lease, compaction, keyframes/effects, multiple timelines in the CLI.

## Review Focus

1. **Rebase across a concurrent edit:** two `fin` calls with the same `baseRev` that touch the same track — the second must be auto-rebased (positions shifted correctly) or return `CONFLICT` with both txIds; never silently produce overlapping clips. → Task 5 test `rebase_insert_over_insert_same_track`, Task 6 test `commit_stale_base_rebases`.
2. **Non-terminating or lossy time input:** `12.4s` at 29.97 fps, `00:00:12.4`, `f300`, negative and zero durations, `1e3s` — exact rationals or a typed `INVALID_TIME`, never float drift. → Task 1 tests.
3. **Crash between apply and append:** a killed `finisherd` must reopen with head = last fully committed tx and no partial state. → Task 6 test `reopen_after_uncommitted_tx`.
4. **Paths agents pass:** relative paths, `~`, spaces, non-ASCII (Ukrainian file names), Windows `C:\` and UNC paths in `asset add` — resolved to absolute, echoed, stored verbatim; missing files → `ASSET_NOT_FOUND` with the resolved path. → Task 7 and Task 12 tests.
5. **Subscriber joins mid-stream:** a subscriber starting at rev N while commits are in flight receives every rev > N exactly once, in order. → Task 10 test `subscribe_during_commits_no_gap_no_dup`.

---

### Task 0: Record decisions in the spec

**Files:** Modify `docs/specs/2026-11-01-finisher-architecture-design.md` (§4 rule 4; §27 new item "Sub-project 2 decisions").

- [ ] **Step 1:** Change §4 rule 4 to: "One protocol definition: RPC envelopes in Effect Schema; model, op, diff and error types are owned by `fin-core` and exported as JSON Schema, from which TypeScript types are generated."
- [ ] **Step 2:** Add §27 item listing decisions 1–6 above, verbatim.
- [ ] **Step 3:** `pnpm gates`; commit `docs(spec): sub-project 2 decisions`.

### Task 1: `Rational` time and time parsing (fin-core)

**Files:** Create `crates/fin-core/src/time.rs`, `crates/fin-core/tests/time.rs`; modify `crates/fin-core/src/lib.rs`, `crates/fin-core/Cargo.toml` (serde, schemars, thiserror).

**Interfaces — Produces:**
- `pub struct Rational { num: i64, den: i64 }` (private fields; invariant `den > 0`, reduced). `Rational::new(num, den) -> Result<Rational, CoreError>`, `Rational::from_int(i64)`, `zero()`, `num()`, `den()`; `Add`, `Sub`, `Neg`, `Ord`, `Display` (`"n/d"`/`"n"`); `FromStr`; serde as string; `JsonSchema` as string with pattern `^-?\d+(/[1-9]\d*)?$`. Overflow → `CoreError` code `TIME_OVERFLOW` (use `i128` intermediates).
- `pub fn parse_time(input: &str, rate: Rational) -> Result<Rational, CoreError>` accepting `12.4s`, `12s`, `-0.5s`, `00:12.4`, `01:00:12.4`, `f300` (frames at `rate`), `"1001/30000"`; decimals are exact (`12.4s` → `62/5`). Errors: `INVALID_TIME` with `details.input`.
- `pub fn frame_duration(rate: Rational) -> Rational` (= 1/rate).

- [ ] **Step 1: Failing tests** in `tests/time.rs`: `reduces("2/4") == "1/2"`; `parse_time("12.4s", 30) == 62/5`; `parse_time("f300", 30000/1001) == 10010/1000` reduced `1001/100`; `parse_time("00:12.4", 30) == 62/5`; `parse_time("01:00:00", 25) == 3600`; rejects `"1e3s"`, `"12.4"` without unit unless rational form, `"abc"`, `"1/0"` → `INVALID_TIME`; `"-0.5s"` → `-1/2`; serde round-trip `"1001/30000"`; `i64::MAX` add → `TIME_OVERFLOW`; proptest: `(a+b)-b == a` for bounded values.
- [ ] **Step 2:** `cargo test -p fin-core --test time` → fails to compile.
- [ ] **Step 3:** Implement. Add `proptest` as dev-dependency.
- [ ] **Step 4:** Tests pass; `cargo build -p fin-core --target wasm32-unknown-unknown` succeeds.
- [ ] **Step 5:** Commit `feat(fin-core): rational time and time parsing`.

### Task 2: Model, transaction envelope, errors, schema export, generated TS types

**Files:** Create `crates/fin-core/src/{model.rs,error.rs,tx.rs,ids.rs,schema.rs}`, `crates/fin-core/examples/export_schema.rs`, `crates/fin-core/tests/model.rs`; create `packages/core-types/{package.json,src/index.ts,src/generated.ts,schema/fin-core.schema.json,scripts/generate.mjs}`; modify `scripts/gates.mjs` (add drift + wasm checks), `scripts/gates` tests as needed.

**Interfaces — Produces (all `Serialize + Deserialize + JsonSchema`, `#[serde(rename_all = "camelCase")]`):**
- `pub struct CoreError { pub code: String, pub message: String, pub details: serde_json::Value, pub fix: Option<String> }` + `CoreError::new(code: &'static str, message: impl Into<String>)`, `.with_details(Value)`, `.with_fix(String)`. Codes used in this plan: `INVALID_TIME`, `TIME_OVERFLOW`, `NOT_FOUND`, `INVALID_RANGE`, `INSERT_INSIDE_CLIP`, `TRACK_KIND_MISMATCH`, `UNBOUND_PLACEHOLDER`, `DUPLICATE_PLACEHOLDER`, `CONFLICT`, `SCHEMA_VERSION`.
- `pub const SCHEMA_VERSION: u32 = 1;`
- `pub struct Project { id: String, name: String, schema_version: u32, rate: Rational, timeline: Timeline, assets: BTreeMap<String, Asset> }`
- `pub struct Timeline { id: String, tracks: Vec<Track> }`; `pub struct Track { id: String, kind: TrackKind, items: Vec<Clip> }` (sorted by `start`, non-overlapping); `pub enum TrackKind { Video, Audio }`.
- `pub struct Clip { id: String, asset: String, start: Rational, source_in: Rational, duration: Rational }`.
- `pub struct Asset { id: String, path: String, duration: Rational, video: Option<VideoInfo>, audio: Option<AudioInfo>, fingerprint: String }`; `VideoInfo { width: u32, height: u32, rate: Rational }`; `AudioInfo { sample_rate: u32, channels: u16 }`.
- `pub fn new_project(id: String, name: String, rate: Rational) -> Project` — creates `tl1` with tracks `t1` (Video) and `t2` (Audio). (Counter seeds therefore start at tl=2, t=3 — see `IdAlloc`.)
- `pub trait IdAlloc { fn next(&mut self, kind: IdKind) -> String; }`, `pub enum IdKind { Asset, Track, Clip, Timeline }`; `pub struct MemIds` (in-memory impl for tests/WASM/dry-run, seeded from a project via `MemIds::after(&Project)`).
- `pub struct Tx { tx_id: String, author: String, branch: String, base_rev: u64, schema_version: u32, ops: Vec<Op> }` (Op from Task 3; declare `pub enum Op {}` placeholder now, tagged `#[serde(tag = "op")]`).
- `pub struct Diff { changes: Vec<Change>, shifts: Vec<Shift> }`; `Change { kind: EntityKind, id: String, change: ChangeKind }` (`Added|Removed|Updated`); `Shift { track: String, from: Rational, by: Rational }`.
- `pub struct Applied { project: Project, diff: Diff, inverse: Vec<Op>, bindings: BTreeMap<String, String> }`.
- `pub fn json_schema() -> serde_json::Value` — one document with `$defs` for `Project`, `Tx`, `Op`, `Diff`, `CoreError`, `CommittedTx` (define `CommittedTx { rev: u64, tx: Tx, diff: Diff, bindings: BTreeMap<String,String> }` here).
- `packages/core-types`: `pnpm --filter @finisher/core-types generate` runs `cargo run -q -p fin-core --example export_schema > schema/fin-core.schema.json` then json-schema-to-typescript → `src/generated.ts`; `src/index.ts` re-exports.
- Gate additions in `scripts/gates.mjs`: (a) regenerate into a temp dir and fail if it differs from committed files (`GENERATED_TYPES_DRIFT`); (b) `cargo build -p fin-core --target wasm32-unknown-unknown`.

- [ ] **Step 1: Failing tests** `tests/model.rs`: `new_project` has `tl1`, `t1` Video, `t2` Audio, rate preserved; JSON of a project round-trips byte-identically; `json_schema()["$defs"]` contains the six names; `MemIds::after(&new_project(..)).next(Clip) == "c1"` and `next(Track) == "t3"`.
- [ ] **Step 2:** Run → fail. **Step 3:** Implement. **Step 4:** `cargo test -p fin-core`, `pnpm --filter @finisher/core-types generate`, `pnpm gates` (drift gate passes on clean tree; edit `generated.ts` by hand once to confirm the gate fails, then revert).
- [ ] **Step 5:** Commit `feat(fin-core): model, tx envelope, errors and generated TS types`.

### Task 3: Ops `asset.add`, `clip.insert` and transaction apply

**Files:** Create `crates/fin-core/src/ops/{mod.rs,asset_add.rs,clip_insert.rs}`, `crates/fin-core/src/apply.rs`, `crates/fin-core/tests/ops_insert.rs`.

**Interfaces — Consumes:** Task 1–2 types. **Produces:**
- `Op::AssetAdd { id: String /* "$name" */, path, duration, video, audio, fingerprint }` serialized `{"op":"asset.add",...}`.
- `Op::ClipInsert { id: String /* "$name" */, track: String, at: Rational, asset: String /* id or "$name" */, source_in: Rational, duration: Rational }` → `"clip.insert"`.
- Internal inverse ops (serializable, never accepted from clients — `validate` rejects them unless `ApplyMode::Internal`): `Op::AssetRemove { id }`, `Op::ClipRemove { id, ripple: bool }`.
- Each op module exposes `pub(crate) fn validate(&Project, &OpRef) -> Result<(), CoreError>`, `pub(crate) fn apply(&mut Project, &Op, &mut Bindings, &mut dyn IdAlloc, &mut Diff) -> Result<Vec<Op> /* inverse */, CoreError>`.
- `pub fn apply_tx(project: &Project, tx: &Tx, ids: &mut dyn IdAlloc) -> Result<Applied, CoreError>` — atomic (works on a clone; any error returns the error and no state); checks `tx.schema_version == SCHEMA_VERSION`; resolves `$name` placeholders in op order (`UNBOUND_PLACEHOLDER`, `DUPLICATE_PLACEHOLDER`); inverse = ops' inverses in reverse order with placeholders replaced by bound ids.
- Rules: insert `at` must be ≥ 0 and equal to a clip boundary or ≥ the track end; otherwise `INSERT_INSIDE_CLIP` with `details.clip`, `fix` naming the nearest boundary; inserting before the end ripples later clips by `duration` (one `Shift`). `source_in + duration ≤ asset.duration` and `duration ≥ frame_duration(project.rate)` else `INVALID_RANGE`. Asset must have video for a Video track, audio for an Audio track (`TRACK_KIND_MISMATCH`). Unknown ids → `NOT_FOUND` with `details.id`.

- [ ] **Step 1: Failing tests** `ops_insert.rs`: add asset + insert in one tx binds `$a`→`a1`, `$c`→`c1`; insert at track end appends; insert at existing boundary shifts later clip by exactly `duration` and records one `Shift`; inside-clip → `INSERT_INSIDE_CLIP` with fix; past asset end → `INVALID_RANGE`; audio-only asset on `t1` → `TRACK_KIND_MISMATCH`; failing second op leaves the input project unchanged; `apply_tx(apply_tx(p, tx).project, inverse_tx) == p` (structural equality); proptest over random insert sequences: no overlaps, items sorted.
- [ ] **Step 2–4:** fail → implement → pass (+ wasm build).
- [ ] **Step 5:** Commit `feat(fin-core): asset.add and clip.insert ops with atomic tx apply`.

### Task 4: Ops `clip.trim` and `clip.delete`

**Files:** Create `crates/fin-core/src/ops/{clip_trim.rs,clip_delete.rs}`, `crates/fin-core/tests/ops_trim_delete.rs`; modify `ops/mod.rs`.

**Interfaces — Produces:**
- `Op::ClipTrim { clip: String, edge: Edge /* Start|End */, delta: Rational }` → `"clip.trim"`. Start: `source_in += delta`, `duration -= delta`, clip `start` unchanged, later clips shift by `-delta`. End: `duration += delta`, later clips shift by `+delta`. Bounds: `source_in ≥ 0`, `source_in + duration ≤ asset.duration`, `duration ≥ one frame` → else `INVALID_RANGE` with `details.min`/`details.max` for `delta`. Inverse: `ClipTrim` with `-delta`.
- `Op::ClipDelete { clip: String }` → `"clip.delete"`; ripples later clips by `-duration`. Inverse: internal `Op::ClipRestore { clip: Clip, track: String }` (re-inserts at original `start`, ripples `+duration`).

- [ ] **Step 1: Failing tests:** trim start +1s on a 5s clip at 0 followed by a clip at 5s → first clip duration 4s, source_in 1s, second clip at 4s; trim end beyond asset → `INVALID_RANGE` with `details.max`; trim to zero → `INVALID_RANGE`; delete middle clip closes the gap; delete+inverse == original; proptest over random insert/trim/delete sequences: apply∘inverse identity, no overlaps, no negative times.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(fin-core): clip.trim and clip.delete ops`.

### Task 5: Rebase

**Files:** Create `crates/fin-core/src/rebase.rs`, `crates/fin-core/tests/rebase.rs`; add `rebase` fn to each op module.

**Interfaces — Produces:**
- Each op module: `pub(crate) fn rebase(op: &Op, over: &Op, over_diff: &Diff) -> Result<Op, Conflict>`.
- `pub struct Conflict { pub reason: String, pub op_index: usize, pub against_tx: String }`; converts to `CoreError` code `CONFLICT` with those fields in `details`.
- `pub fn rebase_tx(tx: &Tx, since: &[CommittedTx]) -> Result<Tx, Conflict>` — returns `tx` with every op rebased over every committed tx after `tx.base_rev`, `base_rev` set to the last rev.
- Rules (position transform over `Shift`s): a position/`at` on the same track at or after a shift's `from` moves by `by`; edits referencing a removed clip → `Conflict` "clip removed"; trim of a clip that was also trimmed on the same edge → `Conflict`; insert whose rebased `at` lands strictly inside a clip → `Conflict`; other tracks untouched.

- [ ] **Step 1: Failing tests:** `rebase_insert_over_insert_same_track` (later insert's `at` shifts by the earlier insert's duration when `at` ≥ earlier `at`), different-track insert unchanged, edit of deleted clip → `CONFLICT`, double trim same edge → `CONFLICT`; proptest: for non-conflicting pairs, `apply(apply(p, a), rebase(b, a))` has no overlaps and contains both edits' clips.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(fin-core): rebase hooks and rebase_tx`.

### Task 6: `fin-store` — SQLite op log

**Files:** Create `crates/fin-store/{Cargo.toml,src/lib.rs,src/paths.rs,src/db.rs,src/commit.rs,tests/store.rs}`.

**Interfaces — Consumes:** `apply_tx`, `rebase_tx`, `CommittedTx`, `IdAlloc`, `new_project`. **Produces:**
- `pub fn server_data_dir() -> PathBuf` (decision 6).
- `pub struct Store` with: `Store::create(server_data: &Path, project_dir: &Path, name: &str, rate: Rational) -> Result<Store, StoreError>` (writes `<project_dir>/finisher.project.json` `{id, name, schemaVersion}` via temp+rename; refuses if it exists → `PROJECT_EXISTS`; creates `<server_data>/projects/<id>/project.fdb`); `Store::open(server_data, project_dir)` (reads the JSON; missing db → `PROJECT_DB_MISSING`, rebuild from segments is sub-project 4); `project_id()`, `head(branch: &str) -> u64`; `snapshot(branch) -> (u64, Project)`; `commit(tx: Tx) -> Result<CommittedTx, StoreError>`; `ops_since(branch, rev) -> Vec<CommittedTx>`.
- Project id: 26-char lowercase ULID from `ulid` crate (MIT) — the only randomness, outside `fin-core`.
- `StoreError` → `CoreError` (codes `PROJECT_EXISTS`, `PROJECT_DB_MISSING`, `DB_ERROR`, plus pass-through core codes).
- Schema (`PRAGMA journal_mode=WAL; synchronous=FULL; foreign_keys=ON`): `meta(key PRIMARY KEY, value)`, `ids(kind PRIMARY KEY, next INTEGER)`, `txs(rev INTEGER, branch TEXT, tx_id TEXT UNIQUE, json TEXT, diff TEXT, inverse TEXT, committed_at INTEGER, PRIMARY KEY(branch, rev))`, `snapshots(branch, rev, json, PRIMARY KEY(branch, rev))`. Snapshot every 200 txs and at rev 0. Head state cached in memory.
- `commit`: if `tx.base_rev < head` → `rebase_tx` over `ops_since`; `apply_tx` with an `IdAlloc` reading/updating `ids` inside the **same** SQLite transaction as the `txs` insert; duplicate `tx_id` returns the existing `CommittedTx` (idempotency for `--idem-key`).

- [ ] **Step 1: Failing tests:** create→open round-trip; `commit` returns rev 1 and bindings; `commit_stale_base_rebases` (two txs with `base_rev 0` on the same track both commit, positions correct); conflicting stale tx → `CONFLICT`, head unchanged; duplicate `tx_id` → same `CommittedTx`, head unchanged; `reopen_after_uncommitted_tx` (inject a failure after apply before SQLite commit via a test hook; reopen head == previous); `ops_since(0)` replays to the same snapshot as `snapshot()`; ids never reused after reopen; Ukrainian project name and dir round-trip.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(fin-store): sqlite op log with rebase-on-commit`.

### Task 7: `fin-probe` — container probe and fingerprint

**Files:** Create `crates/fin-probe/{Cargo.toml,src/lib.rs,tests/probe.rs,tests/support/mod.rs}`. Fixtures are written at test time by `tests/support/mod.rs` with the `mp4` crate's writer (a 2 s, 25 fps H.264-tagged video track and a 48 kHz AAC-tagged audio track); no media is committed. Task 12 reuses this helper via a `fin-probe` dev feature `fixtures`.

**Interfaces — Produces:**
- `pub struct Probe { duration: Rational, video: Option<VideoInfo>, audio: Option<AudioInfo>, fingerprint: String, size: u64 }` (reuse fin-core `VideoInfo`/`AudioInfo`).
- `pub fn probe(path: &Path) -> Result<Probe, CoreError>` — MP4/MOV via `mp4` crate; duration = movie timescale rational; video rate from sample count/duration (rational); errors `ASSET_NOT_FOUND` (`details.path` = absolute path), `UNSUPPORTED_CONTAINER`.
- `pub fn fingerprint(path: &Path) -> io::Result<String>` = `"fp1:" + hex(blake3(size_le || mtime_ns_le || head 64 KiB || middle 64 KiB || tail 64 KiB))` (spec §6.7).

- [ ] **Step 1: Failing tests:** generated fixture probes to duration `2`, rate `25`, 48000 Hz; fingerprint stable across two calls and changes when one byte in the middle changes; missing file → `ASSET_NOT_FOUND` with absolute path; non-MP4 bytes → `UNSUPPORTED_CONTAINER`; path with spaces and Cyrillic works.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(fin-probe): mp4/mov probe and fast fingerprint`.

### Task 8: `fin-node` napi binding + `@finisher/fin-node`

**Files:** Create `crates/fin-node/{Cargo.toml,build.rs,src/lib.rs}`, `packages/fin-node/{package.json,index.js,index.d.ts,test/engine.test.ts}`.

**Interfaces — Consumes:** `Store`, `probe`. **Produces (JS):**
- `class Engine { static create(serverData: string, projectDir: string, name: string, rate: string): Promise<Engine>; static open(serverData: string, projectDir: string): Promise<Engine>; readonly projectId: string; head(branch: string): Promise<number>; commit(txJson: string): Promise<string /* CommittedTx JSON */>; snapshot(branch: string): Promise<string /* {rev, project} JSON */>; opsSince(branch: string, rev: number): Promise<string /* CommittedTx[] JSON */>; close(): Promise<void> }`; `probe(path: string): Promise<string /* Probe JSON */>`.
- All methods run on the libuv thread pool (`AsyncTask`), never blocking the event loop; a `Store` per `Engine` behind a `Mutex`.
- Errors: rejected `Error` whose `message` is the core message and which carries `code`, `details` (parsed object) and `fix`.
- Build: `pnpm --filter @finisher/fin-node build` → `napi build --platform --release` into `packages/fin-node/`; `index.js` loads `fin-node.<platform>-<arch>.node`.

- [ ] **Step 1: Failing vitest** `engine.test.ts`: create/commit/snapshot round-trip in a temp server-data; `CONFLICT` rejection has `code === "CONFLICT"` and `details.againstTx`; event loop stays responsive (a 1 ms `setInterval` keeps ticking during 500 commits; max gap < 20 ms); `probe` on a missing path rejects with `ASSET_NOT_FOUND`.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(fin-node): napi engine and probe bindings`.

### Task 9: `@finisher/contracts` — RPC definitions

**Files:** Modify `packages/contracts/{package.json,src/index.ts,src/index.test.ts}`; create `packages/contracts/src/{rpc.ts,errors.ts}`.

**Interfaces — Produces (Effect 4 `effect/unstable/rpc` `Rpc`/`RpcGroup`, schemas from `effect/Schema`):**
- `PROTOCOL_VERSION = 1`.
- `FinError = Schema.TaggedError("FinError")({ code: String, message: String, details: Unknown, fix: optional(String) })`.
- `FinRpcs = RpcGroup.make(...)` with: `server.hello {}` → `{ protocolVersion, serverVersion, capabilities: string[] }`; `project.create { dir, name, rate }` → `{ projectId, rev }`; `project.open { dir }` → `{ projectId, rev }`; `asset.probe { path }` → `Probe` (Unknown typed as generated `Probe`); `tx.commit { projectId, tx: Unknown }` → `CommittedTx`; `snapshot.get { projectId, branch }` → `{ rev, project }`; `ops.subscribe { projectId, branch, fromRev }` → **stream** of `CommittedTx`. All fail with `FinError`.
- Types for `Unknown` payloads come from `@finisher/core-types` via `Schema.declare`-free type assertions (runtime validation is Rust's job).

- [ ] **Step 1: Failing test:** the group exposes exactly these 7 tags; `FinError` encodes/decodes round-trip; protocol version constant is 1.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(contracts): fin rpc group v1`.

### Task 10: `finisherd` minimal server

**Files:** Create `apps/finisherd/{package.json,src/main.ts,src/config.ts,src/projects.ts,src/handlers.ts,src/transport.ts,src/env.ts,test/server.test.ts}`.

**Interfaces — Consumes:** `FinRpcs`, `Engine`, `probe`. **Produces:**
- `node apps/finisherd/src/main.ts [--server-data <dir>]` starts the server; writes `<server-data>/finisherd.json` `{ pid, wsPort, socket, token, protocolVersion }` (0600) and removes it on shutdown; SIGINT/SIGTERM graceful.
- `ProjectRegistry` (Effect service): `node:sqlite` `<server-data>/app.db` table `projects(id, dir, last_opened)`; holds open `Engine`s; `project.create`/`open` register them.
- Commit flow: `tx.commit` → `engine.commit` → publish to a per-`(projectId, branch)` `PubSub`; `ops.subscribe` subscribes to the PubSub **first**, then replays `opsSince(fromRev)`, then live, dropping revs ≤ last emitted (no gap, no dup).
- Transports: RPC server over local socket/pipe with NDJSON serialization (decision 5) and over WebSocket on `127.0.0.1:0` requiring `Authorization: Bearer <token>`.
- `env.ts`: `childEnv(extra)` implementing the §27.7 allow-list (unused by spawn yet; unit-tested now so later spawns use it).

- [ ] **Step 1: Failing tests** (vitest, real server in a temp dir): hello returns protocol 1; create→commit→snapshot over the socket; WS without token rejected; `subscribe_during_commits_no_gap_no_dup` (subscriber at rev 3 while 50 commits run concurrently receives revs 4..53 exactly once in order); `CONFLICT` arrives as `FinError` with code; restart keeps projects (open by dir works); `childEnv` drops `ELECTRON_RUN_AS_NODE`, `CLAUDECODE`, `CLAUDE_CODE_*`.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(finisherd): minimal rpc server with live op stream`.

### Task 11: `fin` CLI — registry, output contract, offline validation

**Files:** Create `crates/fin/{Cargo.toml,src/main.rs,src/registry.rs,src/output.rs,src/map.rs,src/addr.rs,src/commands/{mod.rs,clip.rs,asset.rs,project.rs,view.rs,schema.rs},tests/cli_offline.rs}`.

**Interfaces — Consumes:** fin-core. **Produces:**
- Command registry: each command declares `name`, `group`, `summary`, `args` (typed), `json_schema()` of its input, `write: bool`. `fin` alone prints the map (< 1.5k tokens, asserted by a test counting `len/4`); `fin help <group>`; `fin schema <cmd>` prints the input JSON Schema.
- Output: success → one JSON object on stdout; writes print `{ "rev", "txId", "bindings", "diff", "resolved": {...} }`; `--dry-run` prints the same shape with `"dryRun": true` and no rev, computed by `apply_tx` on a snapshot with `MemIds`. Error → `{ "error": { code, message, details, fix } }` on stdout. Exit codes: `0` ok, `2` usage (`USAGE`), `3` validation (any core validation code), `4` `CONFLICT`, `5` server unavailable (`SERVER_UNAVAILABLE`), `6` internal.
- Addressing (`addr.rs`): times via `fin_core::parse_time` with the project rate; `+2s`/`-2s` relative for trim deltas; `after:c12` → that clip's end; `--at end` → track end. Resolved values echoed in `resolved`.
- Commands (args): `project create <dir> --name <s> --rate <fps rational, default 30>`; `project open <dir>`; `status`; `view [--track t1] [--range a..b]` (JSON: tracks → clips with `start`, `end`, `asset`, `sourceIn`); `asset add <path>`; `clip insert --track <t> --at <time|after:cN|end> --asset <aN> [--in <time>] (--dur <time> | --out <time>)`; `clip trim <cN> (--start <±time> | --end <±time>)`; `clip delete <cN>`; `batch` (JSONL of `{cmd, args}` → one tx); global `--project <dir>` (default: walk up from cwd to `finisher.project.json`), `--idem-key <k>` (becomes `txId`), `--dry-run`.

- [ ] **Step 1: Failing tests** `cli_offline.rs` (no server; uses a snapshot fixture via `FIN_OFFLINE_SNAPSHOT=<file>` test hook): map under 1.5k tokens and lists every group; `schema clip.insert` is valid JSON Schema; `clip insert --at 12.4s` echoes `resolved.at == "62/5"`; inside-clip dry-run exits 3 with `INSERT_INSIDE_CLIP` and a `fix`; unknown flag exits 2 with `USAGE`; nothing written to stderr on success; no ANSI escapes anywhere.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(fin): command registry, output contract and offline dry-run`.

### Task 12: `fin` transport and online commands

**Files:** Create `crates/fin/src/{client.rs,endpoint.rs}`, `crates/fin/tests/cli_online.rs`.

**Interfaces — Consumes:** Task 10 server, Task 11 registry. **Produces:**
- `endpoint.rs`: resolves `FIN_SERVER` (socket path or `pipe:<name>`) else `<server-data>/finisherd.json`; missing/unreachable → `SERVER_UNAVAILABLE` with `fix: "start finisherd"`.
- `client.rs`: Effect RPC NDJSON client over `interprocess` local sockets — request `{"_tag":"Request","id","tag","payload","headers":[]}`, reads `Exit`/`Chunk` messages; implemented against frames captured from the real server in a contract test (fixtures in `crates/fin/tests/fixtures/rpc/*.ndjson`, refreshed by `pnpm --filter finisherd capture-rpc-fixtures`).
- `asset add` → `asset.probe` then `tx.commit` with `asset.add` (`$a`), printing the bound id and resolved absolute path.

- [ ] **Step 1: Failing tests** (spawn `finisherd` with temp `FIN_SERVER_DATA`): full flow create → asset add (fixture from Task 7 helper) → insert ×2 → trim → delete → view matches expected; relative path, `~`, spaces and Cyrillic file names resolve and echo absolute paths; missing file → exit 3 `ASSET_NOT_FOUND`; server down → exit 5; same `--idem-key` twice → identical output, one rev; two concurrent `clip insert` with the same snapshot both succeed with rebased positions.
- [ ] **Step 2–4**, **Step 5:** Commit `feat(fin): local-socket client and online commands`.

### Task 13: End-to-end check, gates and README

**Files:** Create `scripts/e2e.mjs`, `scripts/e2e.test.mjs`; modify `scripts/gates.mjs`, `README.md` ("Run it" section), `.github/workflows/ci.yml` (add `cargo test`, napi build, e2e on the matrix; actions SHA-pinned), `TASKS.md`.

- [ ] **Step 1:** `scripts/e2e.mjs`: builds fin + fin-node, starts finisherd in a temp dir, opens a WebSocket subscriber, runs the Task 12 flow via `fin`, asserts the subscriber saw every rev in order and that the last `CommittedTx.rev` equals `snapshot.get`'s rev. (Client-side replay through `fin-wasm` is sub-project 3.)
- [ ] **Step 2:** Run on macOS and on the Windows PC (see `archive/hosts.md`) — both pass. Linux VM (see `archive/hosts.md`) — pass.
- [ ] **Step 3:** README "Run it": start `finisherd`, then `fin project create …` examples.
- [ ] **Step 4:** Commit `test: end-to-end fin → finisherd → subscriber`.

---

## Execution waves (for parallel subagents)

1. Task 0 (controller) → Task 1 → Task 2 (one agent, sequential: they define the shared types).
2. In parallel: **A** Tasks 3 → 4 → 5 (fin-core ops), **B** Task 7 (fin-probe), **C** Task 9 (contracts).
3. In parallel: **D** Task 6 (fin-store), **E** Task 11 (fin CLI offline).
4. Task 8 (fin-node) → Task 10 (finisherd) → Task 12 (fin online) → Task 13.

Each parallel agent works in its own git worktree on a branch `sp2/<task>`; the controller merges in wave order after review.
