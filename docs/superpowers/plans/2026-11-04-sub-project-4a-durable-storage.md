# Sub-project 4A: Durable storage — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Finisher project survives format upgrades, crashes, a damaged database, other machines and NAS or sync-folder storage. The live SQLite DB stays local. The project folder receives a verified, hash-chained, write-once export. A v3 lease admits one writer. Every other server follows read-only, and can take over.

**Architecture:**
- **`fin-core`** gains versioned commit records, upcasters and a replay mode that skips policy checks.
- **`fin-store`** gains:
  - DB migrations;
  - segment and snapshot file codecs;
  - folder scan → chain resolution → import;
  - export;
  - the v3 lease (its own heartbeat thread);
  - storage-kind detection;
  - rebuild, verify, backups and compaction.

  All folder work runs from one `Store::tick(now)` call.
- **`fin-node`** runs a per-engine maintenance thread that calls `tick` and pushes events to JS.
- **`finisherd`** exposes the new RPCs and an events stream.
- **`fin`** gets `project status|takeover|verify|rebuild|compact|recover`.

**Tech Stack:**
- Rust 2024: rusqlite 0.37 bundled; new crates `blake3`, `crc32c`, `libc` (unix) and `windows-sys` (windows), all MIT or Apache-2.0.
- Node 26, Effect 4, vitest.

**Spec:** `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §2, §3, §6 and §8. Parent: `docs/specs/2026-11-01-finisher-architecture-design.md` §6.7, §6.8 and §27.9.

## Global Constraints

- MIT. No GPL/LGPL/AGPL/SSPL/EUPL (`pnpm gates` runs cargo-deny and the JS licence gate).
- `fin-core` is pure: no I/O, clocks or randomness, and it must build for `wasm32-unknown-unknown`.
- Non-test source files ≤ 600 lines; tests ≤ 1,500.
- Errors are `{ code, message, details, fix? }`. Every new code is added to `crates/fin/src/output.rs` with the exit code given in this plan.
- SQLite and OS file locks are never used on the project folder. Everything in it is write-once (temp → fsync → rename → dir fsync, a no-op on Windows), except the in-place 512-byte `lease.json` and the lease probe file.
- Temp names: `.~fin-<ulid>.tmp`, in the destination directory.
- Hashes are `blake3:<64 lowercase hex>`; `<hash12>` is the first 12 hex characters. The CRC is CRC-32C, written as 8 lowercase hex.
- Lease: TTL 10 s, heartbeat 5 s, stale after 45 s of server time, takeover needs two stale probes ≥ 5 s apart with a probe round trip ≤ 2.5 s, self-fence at the last heartbeat + 20 s (monotonic clock). Sync folders never take over automatically.
- Segment cap: 1,000 records or 4 MiB; a transaction is never split.
- Export delay: 2 s; 30 s once more than 30 segments have been written in the last 10 minutes; always flushed on close.
- Compaction: more than 200 non-pack segments per branch; grace 10 min (local/network) or 24 h (sync).
- Backups: daily `VACUUM INTO`, keep 7; pre-migration backups, keep 3.
- Every task runs `cargo test --workspace`, `pnpm test` and `pnpm gates` before committing.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`. Node 26 via `source .superpowers/env.sh`.
- The repo is public: no host names, IPs, user names or key names in tracked files. Cross-host access details live in `archive/hosts.md` (git-ignored).

## Decisions taken here (the spec leaves them open; Task 20 records them in the spec)

1. **Naming:** the DB column is `seq`; APIs keep the name `rev` (same number). `CommittedTx.rev`, `opsSince(rev)` and `fromRev` are unchanged.
2. **Intent:** `intent` is the `Tx` as applied: after any rebase, still containing `$placeholders`.
   - `resolved` = `intent.ops` with bindings substituted (`fin_core::resolve_ops`), stored for readers and for 4B.
   - Replay applies `intent` with the recorded bindings feeding id allocation (the current `ReplayIds` idea, moved into `fin-core` as `BoundIds`). With every id fixed this is equivalent to applying `resolved`, and it reuses one apply path.
3. **Polling only (deviation from spec §3.6):** following uses polling (local 2 s, network 3 s, sync 10 s), not file-system events. Listing `.finisher/segments/<branch>` is cheap, and polling avoids a watcher dependency. Events can come later without a format change.
4. **Separate events stream (deviation from spec §3.10):** status events go on a new stream RPC `project.events {projectId}`; `ops.subscribe` is unchanged. Adding a second item type to `ops.subscribe` would break existing clients and the protocol.
5. **Server id:** a ULID stored in `<server-data>/server-id`, created on first use (written atomically, mode 0600 on unix).
6. **Project copies:** the DB `meta` stores `projectDir`. Opening a folder whose project id is already registered to a *different existing* folder fails with `PROJECT_COPY`. If the old folder no longer exists, it was moved, and `projectDir` is updated.
7. **Long native calls** (open with import, verify, rebuild, compact) run on a dedicated per-engine worker thread and resolve a napi deferred promise, so they never occupy the libuv pool. Short calls stay `AsyncTask`.

## Review Focus

1. **Unicode, spaces and UNC paths** (e.g. `\\nas\share\Проект 1`, `/Volumes/NAS/Мой проект`): every folder operation works, and segment names stay ASCII. → Task 13, `open_export_follow_in_unicode_dir`.
2. **The project folder disappears mid-session** (NAS unmounted, folder renamed): the server keeps running, the project goes read-only with `LEASE_LOST`, writes fail cleanly, and it recovers when the folder returns. → Task 13, `folder_vanishes_then_returns`.
3. **Two servers on one machine** (two server-data dirs) open the same folder: one writes, one follows. → Task 13, `two_server_datas_one_folder`.
4. **A copied project folder** opened alongside the original: refused with `PROJECT_COPY`, never sharing one DB. → Task 13, `copied_folder_is_refused`.
5. **Folder not writable or disk full during export:** records stay unexported, status shows the export error, a retry succeeds later, nothing is lost. → Task 7, `export_retries_after_unwritable_dir` (unix chmod; Windows uses a read-only attribute on the dir's files).

---

### Task 1: SP2 fixture, record types, upcasters, replay mode (fin-core)

**Files:**
- Create: `crates/fin-store/examples/make_sp2_fixture.rs`, `crates/fin-store/tests/fixtures/sp2/` (generated, committed)
- Create: `crates/fin-core/src/record.rs`, `crates/fin-core/src/upgrade.rs`, `crates/fin-core/src/replay.rs`
- Modify: `crates/fin-core/src/lib.rs`, `crates/fin-core/src/apply.rs`, op validators under `crates/fin-core/src/ops/`
- Test: `crates/fin-core/tests/record.rs`, `crates/fin-core/tests/replay.rs`

**Interfaces:**
- Produces:
  - `pub const RECORD_FORMAT: u32 = 1; pub const MIN_READER: u32 = 1;`
  - `pub struct Record { tx_id: String, seq: u64, kind: RecordKind, schema_version: u32, parent_hash: Option<String>, author: String, committed_at: i64, intent: Tx, resolved: Vec<Op>, inverse: Vec<Op>, bindings: BTreeMap<String,String> }`, with camelCase serde, fields serialised in this order, and `kind` as `"edit"`.
  - `pub enum RecordKind { Edit }`, with serde lowercase and `#[non_exhaustive]`.
  - `pub fn encode_record(r: &Record) -> Vec<u8>`: compact JSON, no trailing newline.
  - `pub fn resolve_ops(ops: &[Op], bindings: &BTreeMap<String,String>) -> Result<Vec<Op>, CoreError>`.
  - `pub fn upcast_record(v: serde_json::Value) -> Result<Record, CoreError>` and `pub fn upcast_snapshot(v: serde_json::Value) -> Result<Project, CoreError>`. Both apply one-step converters in order, from the value's `schemaVersion` up to `SCHEMA_VERSION`. A version above it → `NEWER_FORMAT`.
  - `pub struct BoundIds<'a>`, which implements `IdAlloc` by handing out bindings in placeholder order. It is moved from `fin-store/src/db.rs` `ReplayIds`.
  - `pub fn apply_committed(project: &Project, intent: &Tx, bindings: &BTreeMap<String,String>) -> Result<Applied, CoreError>`. It uses `ApplyMode::Replay` and fails with `REPLAY_FAILED` (`details {txId, cause}`) only on structural errors.
  - `ApplyMode::Replay`.

- [ ] **Step 1: Generate the SP2 fixture before changing any code.**
  - Write `make_sp2_fixture.rs` against the *current* `Store` API. It creates a project in `tests/fixtures/sp2/{folder,server-data}` and commits:
    - `asset.add`;
    - two `clip.insert` (one with a stale base, so it rebases);
    - `clip.trim`;
    - `clip.delete`;
    - then 200 more inserts and deletes, so a DB snapshot exists at rev 200.
  - It also writes `expected.json` = `snapshot("main")`.
  - Run it with `cargo run -p fin-store --example make_sp2_fixture` and commit the outputs. Check that the WAL is checkpointed (`PRAGMA wal_checkpoint(TRUNCATE)` before close) so a single `.fdb` file is committed.
- [ ] **Step 2: Write failing tests.**
  - `record_roundtrip_is_byte_stable`: `encode_record` then parse then `encode_record` gives identical bytes; the field order matches the struct.
  - `resolve_ops_substitutes_every_placeholder` / `resolve_ops_unbound_placeholder_errors` (`UNBOUND_PLACEHOLDER`).
  - `upcast_chain_runs_steps_in_order`: with a `#[cfg(test)]` synthetic step v0→v1, golden in and golden out.
  - `upcast_rejects_newer` → `NEWER_FORMAT`.
  - `replay_ignores_policy_rules`: build a tx that `apply_tx` rejects with `INSERT_INSIDE_CLIP` because of a policy-only check; `apply_committed` with its bindings succeeds.
  - `replay_rejects_structural`: a clip id that doesn't exist → `REPLAY_FAILED` with cause `NOT_FOUND`.
- [ ] **Step 3: Run `rtk cargo test -p fin-core`.** Expected: FAIL to compile (missing items).
- [ ] **Step 4: Implement.**
  - Classify every `CoreError` raised in `ops/*/validate`:
    - **structural** (checked in Replay): `NOT_FOUND`, `DUPLICATE_ID`, `TRACK_KIND_MISMATCH`, item overlap, `UNBOUND_PLACEHOLDER`, `DUPLICATE_PLACEHOLDER`, `TIME_OVERFLOW`;
    - **policy** (skipped in Replay): everything else, including `INSERT_INSIDE_CLIP` and minimum duration.
  - Put the policy checks behind `if mode != ApplyMode::Replay`.
- [ ] **Step 5: Run `rtk cargo test --workspace` and `pnpm gates`** (the wasm build must still pass). Expected: PASS.
- [ ] **Step 6: Commit** `feat(core): versioned commit records, upcasters and policy-free replay`.

### Task 2: DB migrations framework and migration 0001 (fin-store)

**Files:**
- Create: `crates/fin-store/src/migrations/mod.rs`, `crates/fin-store/src/migrations/m0001_records.rs`, `crates/fin-store/src/backup.rs`
- Modify: `crates/fin-store/src/db.rs` (schema → v1 tables), `crates/fin-store/src/lib.rs`, `crates/fin-store/Cargo.toml` (`blake3`, `crc32c`)
- Test: `crates/fin-store/tests/migrations.rs`

**Interfaces:**
- Consumes: `Record`, `encode_record`, `resolve_ops` (Task 1).
- Produces:
  - `pub const APPLICATION_ID: i32 = 0x4649_4E31;`
  - `pub const DB_VERSION: u32` (= 1 after this task).
  - `pub(crate) fn migrate(conn: &mut Connection, project_data: &Path) -> Result<(), StoreError>`.
  - `pub(crate) fn hash_bytes(b: &[u8]) -> String` (blake3 prefixed).
  - `pub(crate) fn backup_to(conn: &Connection, dest: &Path) -> Result<(), StoreError>` (`VACUUM INTO`).
  - New `txs` columns: `seq, branch, tx_id, kind, record_bytes BLOB, tx_hash, parent_hash, epoch INTEGER, exported INTEGER, diff, schema_version, author, committed_at`.
  - New tables: `segments(branch, hash PRIMARY KEY, name, epoch, first_seq, last_seq, pack INTEGER, replaces TEXT, state)`, `lease_state(key PRIMARY KEY, value)`, `quarantine(id INTEGER PRIMARY KEY, origin TEXT, reason TEXT, record_bytes BLOB, added_at)`.
  - `meta` keys: `formatVersion`, `minReaderVersion`, `createdBy`, `lastOpenedBy`, `projectDir`.

- [ ] **Step 1: Write failing tests.**
  - `sp2_fixture_migrates_to_identical_state`: copy the fixture to a tmp dir, open it, and `snapshot("main")` equals `expected.json`. Every tx has `record_bytes`; `tx_hash == hash_bytes(record_bytes)`; `parent_hash` chains by seq; the first is `null`; `exported = 0`.
  - `migration_backs_up_first`: `backups/pre-migrate-0-1-*.fdb` exists and opens.
  - `newer_db_is_refused`: set `user_version = 99` → `DB_TOO_NEW`, and the file is unchanged (byte-compare).
  - `min_reader_too_new_is_refused`: `meta.minReaderVersion = 99` → `DB_TOO_NEW`.
  - `foreign_sqlite_is_refused`: an SQLite file with `application_id` 0 and no `meta` → `NOT_A_PROJECT_DB`.
  - `failed_step_rolls_back`: a test-hook step that errors leaves `user_version` and the tables as they were.
  - `pre_migration_backups_keep_three`.
- [ ] **Step 2: Run `rtk cargo test -p fin-store --test migrations`.** Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Detect the SP2 DB (`user_version = 0` with a `txs.json` column) and treat it as version 0.
  - A brand-new DB is created at `DB_VERSION` directly.
  - Each step runs in its own `IMMEDIATE` transaction and sets `user_version` inside it.
  - 0001 builds each `Record` from the old `json`/`inverse`/`bindings`/`committed_at` columns in rev order (`seq = rev`, `epoch = 0`), then drops the old columns by table rebuild.
  - Existing DB snapshots stay as they are; they now load through `upcast_snapshot`.
- [ ] **Step 4: Run tests.** Expected: PASS. Run the full workspace and `pnpm gates`.
- [ ] **Step 5: Commit** `feat(store): versioned DB with migrations; SP2 projects migrate to hashed records`.

### Task 3: Commit writes records; open uses snapshot + replay

**Files:**
- Modify: `crates/fin-store/src/commit.rs`, `crates/fin-store/src/db.rs` (remove `replay`'s diff comparison; `insert_tx` takes a `Record`), `crates/fin-store/src/lib.rs` (`refresh`)
- Test: `crates/fin-store/tests/store.rs`

**Interfaces:**
- Consumes: `apply_committed`, `upcast_record`, `upcast_snapshot` (Task 1); the schema (Task 2).
- Produces:
  - `Store::commit` stores `record_bytes = encode_record(&Record{…})`, `tx_hash`, `parent_hash = previous tx_hash on branch or None`, `epoch = current lease epoch or 0`.
  - `pub fn replay_record(project: &Project, bytes: &[u8]) -> Result<Applied, StoreError>` replaces `pub fn replay`.
  - `ops_since` builds `CommittedTx { rev: seq, tx: record.intent, diff, bindings }`.

- [ ] **Step 1: Write failing tests.**
  - `commit_chains_hashes`: after three commits, each `parent_hash` is the previous `tx_hash`, and the hashes verify.
  - `idempotent_commit_returns_same_record`: same `txId` → same rev and hash.
  - `reopen_survives_policy_tightening`: insert a raw record that policy would reject but structure allows → reopen succeeds and the snapshot contains it.
  - `reopen_reports_structural_damage`: a hand-edited record referencing a missing clip → open fails with `REPLAY_FAILED`, `details.txId`.
  - Keep `reopen_replays_past_a_periodic_snapshot` green.
- [ ] **Step 2: Run** `rtk cargo test -p fin-store`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement.** The commit path is unchanged apart from building the `Record` once, hashing those bytes, and storing them. `refresh` loads the latest DB snapshot via `upcast_snapshot`, then `replay_record` for the tail.
- [ ] **Step 4: Run** the workspace, `pnpm test`, `pnpm e2e` and `pnpm gates`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(store): commit stores exact record bytes; replay no longer re-judges history`.

### Task 4: File primitives (fsio) and segment/snapshot codecs

**Files:**
- Create: `crates/fin-store/src/fsio.rs`, `crates/fin-store/src/segment.rs`, `crates/fin-store/src/snapfile.rs`, `crates/fin-store/src/names.rs`
- Modify: `crates/fin-store/Cargo.toml` (`libc` for unix targets; `windows-sys` with features `Win32_Storage_FileSystem`, `Win32_Foundation`)
- Test: `crates/fin-store/tests/codec.rs`

**Interfaces:**
- Produces:
  - `fsio::write_once(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()>`: temp `.~fin-<ulid>.tmp`, fsync, rename (fails if the target exists, using `link`+`unlink` on unix and `MoveFileExW` without `REPLACE_EXISTING` on Windows), dir fsync. Returns `AlreadyExists` when the target exists.
  - `fsio::read_uncached(path) -> io::Result<Vec<u8>>`, ported from spike `fresh_read`: `F_NOCACHE` on macOS, `O_DIRECT` with fallback on Linux, `FILE_FLAG_NO_BUFFERING` on Windows.
  - `fsio::write_in_place(path, bytes) -> io::Result<()>`: open without truncate, write at 0, `sync_all`, retrying transient errors 60× every 25 ms (spike `retry`/`transient` code lists).
  - `fsio::is_placeholder(path, meta) -> bool`:
    - Windows attributes `0x400000 | 0x40000 | 0x1000`;
    - macOS `st_flags & SF_DATALESS (0x40000000)`;
    - otherwise false.
  - `names::SegmentName { epoch: u64, first_seq: u64, last_seq: u64, hash12: String }` with `parse(&str) -> Option<Self>` and `Display` as `{epoch:08}-{first:012}-{last:012}-{hash12}.fseg`.
  - `names::SnapName { seq, hash12 }` → `{seq:012}-{hash12}.fsnap`.
  - `segment::Header { magic: "finisher-segment", format_version, min_reader_version, project_id, branch, writer_id, epoch, first_seq, last_seq, prev_segment_hash: Option<String>, op_schema_versions: [u32;2], created_at: i64, pack: bool, replaces: Vec<String> }`.
  - `segment::encode(header, records: &[&[u8]]) -> (Vec<u8>, String /*hash*/)`.
  - `segment::decode(bytes) -> Result<Segment, SegError>`, where `Segment { header, records: Vec<Vec<u8>>, hash }` and `SegError { Incomplete, Corrupt(String), Newer { min_reader: u32 }, UnsupportedHash }`.
  - `snapfile::encode/decode` with `SnapHeader { magic: "finisher-snapshot", format_version, min_reader_version, project_id, branch, seq, tx_hash, model_schema_version, writer_id, epoch }` and the body as one line of `Project` JSON.

- [ ] **Step 1: Write failing tests.**
  - `segment_roundtrip_preserves_record_bytes`.
  - `trailer_covers_all_prior_bytes`: flip any single byte → `Corrupt`.
  - `truncated_is_incomplete_not_corrupt`: cut the file at any offset inside the trailer, or with no trailer → `Incomplete`.
  - `newer_min_reader_is_reported` and `unknown_hash_prefix_is_unsupported`.
  - `names_parse_strict`: rejects `" 2"`, `(1)`, `conflicted copy` and wrong widths; accepts its own `Display` output.
  - `write_once_never_overwrites`.
  - `in_place_write_keeps_inode` (unix: same inode; windows: same file id).
- [ ] **Step 2: Run.** Expected: FAIL. **Step 3: Implement.** **Step 4: Run all and gates.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(store): write-once file primitives and segment/snapshot codecs`.

### Task 5: Folder scan and classification

**Files:**
- Create: `crates/fin-store/src/scan.rs`, `crates/fin-store/src/conflict_names.rs`
- Test: `crates/fin-store/tests/scan.rs`

**Interfaces:**
- Consumes: `names`, `segment::decode`, `fsio::is_placeholder` (Task 4).
- Produces:
  - `pub enum Entry { Segment { branch, name: SegmentName, seg: Segment }, Snapshot { branch, name: SnapName, header: SnapHeader }, NotArrived { path, why: NotArrivedWhy }, Foreign { path, conflict_copy: Option<Provider> }, Newer { path, min_reader: u32 } }`.
  - `pub enum NotArrivedWhy { Placeholder, Incomplete, Temp }`.
  - `pub enum Provider { Dropbox, Syncthing, ICloud, OneDrive, GoogleDrive, Unknown }`.
  - `pub fn scan(project_dir: &Path) -> io::Result<Vec<Entry>>`.
  - `conflict_names::classify(name: &str, hostnames: &[String]) -> Option<Provider>`. The patterns are case-insensitive:
    - `conflicted copy` → Dropbox;
    - `.sync-conflict-` → Syncthing;
    - `^(.*) \d+(\.[^.]+)?$` → ICloud;
    - `-<hostname>` before the extension → OneDrive;
    - `\(\d+\)` or `\[conflict\]` → GoogleDrive.

    A file that parses as a Finisher file under a non-matching name → `Unknown`.
  - Placeholders are hydrated by reading the whole file (the caller decides when); `scan` itself never reads a placeholder.

- [ ] **Step 1: Write failing tests:**
  - `classifies_each_provider_pattern` (table-driven);
  - `lease_json_conflict_copy_is_flagged`;
  - `temp_files_are_not_arrived`;
  - `placeholder_is_not_arrived` (unix: mocked via a test hook that marks paths; macOS: `chflags dataless` is not permitted for users, so use the hook);
  - `own_names_are_never_foreign`;
  - `newer_file_reported`.
- [ ] **Step 2–4: Run → FAIL; implement; run all and gates → PASS.**
- [ ] **Step 5: Commit** `feat(store): classify project-folder entries (segments, not-arrived, conflict copies)`.

### Task 6: Chain resolution (pure)

**Files:**
- Create: `crates/fin-store/src/chain.rs`
- Test: `crates/fin-store/tests/chain.rs` (unit cases plus proptest; add `proptest` to dev-deps)

**Interfaces:**
- Produces:
  - `pub struct Known { last_hash: Option<String>, known_tx: HashMap<String /*txId*/, String /*txHash*/>, own_epoch: Option<u64>, exported_at_own_epoch: bool, unexported_first_parent: Option<Option<String>>, kind: StorageKind }`.
  - `pub struct Resolution { apply: Vec<SegRef>, mark_exported: Vec<(String /*txId*/, String /*segHash*/)>, quarantine: Vec<(SegRef, QuarantineReason)>, fork: Option<ForkInfo>, waiting: Vec<String>, local_tail_diverged: bool }`.
  - `pub fn resolve(branch_segments: &[SegRef], known: &Known) -> Resolution`, where `SegRef { hash, header, records: Vec<(txId, txHash, bytes)> }`.
- Algorithm, mirroring spec §3.6 steps 4–8:
  1. Drop segments listed in any present pack's `replaces`.
  2. Map each replaced hash → its pack, so predecessors resolve.
  3. Drop segments whose records all have a `(txId, txHash)` in `known_tx`, adding them to `mark_exported`. The same txId with a different hash → fork.
  4. Walk from `last_hash` following `prev_segment_hash`. With one successor, append it. With none and other segments present, wait (a gap).
  5. With two or more successors, network/local storage keeps the highest epoch and quarantines the rest; equal epochs or sync storage → `fork`.
  6. When `unexported_first_parent` is `Some(p)`, `own_epoch` is `Some(e)` and `!exported_at_own_epoch`, the local tail is a virtual successor of `p` with epoch `e` and joins step 5. If it loses, set `local_tail_diverged`.

- [ ] **Step 1: Write failing tests (unit):**
  - `linear_chain_applies_in_order`;
  - `gap_waits`;
  - `fork_equal_epoch`;
  - `stale_epoch_quarantined`;
  - `stale_segment_after_takeover_before_first_export_is_quarantined` (the reviewer's C2 hole);
  - `pack_and_originals_in_grace_window_are_one` (C2 false positive);
  - `sync_kind_never_auto_resolves`;
  - `crash_after_rename_marks_exported_no_fork` (I1);
  - `same_txid_different_hash_is_fork`.
- [ ] **Step 2: Write failing property tests:**
  - `prop_never_applies_lower_epoch_branch_after_higher`;
  - `prop_resolution_is_order_independent` (shuffled input gives the same result);
  - `prop_packing_preserves_resolution` (replacing any run of segments with a pack doesn't change `apply`'s record sequence).
- [ ] **Step 3–4: Run → FAIL; implement; run → PASS (proptest default 256 cases, < 5 s).**
- [ ] **Step 5: Commit** `feat(store): pure chain resolution with epoch rule, packs and fork detection`.

### Task 7: Export

**Files:**
- Create: `crates/fin-store/src/export.rs`
- Modify: `crates/fin-store/src/lib.rs` (add the `project_dir` field and the `tick` skeleton)
- Test: `crates/fin-store/tests/export.rs`

**Interfaces:**
- Consumes: `fsio::write_once`, `segment::encode`, the `segments` table.
- Produces:
  - `pub(crate) struct ExportPolicy { recent: VecDeque<Instant> }` with `fn delay(&self, now: Instant) -> Duration` (2 s, or 30 s if more than 30 entries are within 10 min).
  - `pub(crate) fn export_pending(store: &Store, now: Instant, force: bool) -> Result<ExportReport, StoreError>`, where `ExportReport { segments_written: u32, records: u32, error: Option<CoreError> }`.
  - An export only runs while this server holds a valid lease; `Lease::check()` (Task 9) is called right before each `write_once`. Until Task 9 lands, `Store` uses a `NoLease` stub that always allows.
  - Records are marked `exported = 1` only after `read_uncached` of the new file decodes, with a hash that matches.
  - Test hook `FailPoint { AfterTempWrite, AfterRename, BeforeMarkExported }` under `test-hooks`.
  - Also deletes `.~fin-*.tmp` older than 20 s (2×TTL) in `.finisher/segments/*`.

- [ ] **Step 1: Write failing tests:**
  - `export_writes_capped_segments`: 2,500 tiny records → 3 files of at most 1,000, chained.
  - `export_never_splits_a_tx`.
  - `export_delay_grows_under_churn`.
  - `crash_at_each_failpoint_recovers_without_fork`: for each FailPoint, reopen, `tick`, then `chain::resolve` shows no fork and every record is exported exactly once.
  - `export_retries_after_unwritable_dir` (Review Focus 5).
  - `old_temp_files_are_removed`.
- [ ] **Step 2–4: Run → FAIL; implement; run all and gates → PASS.**
- [ ] **Step 5: Commit** `feat(store): export committed records as verified write-once segments`.

### Task 8: Import, quarantine, id counters

**Files:**
- Create: `crates/fin-store/src/import.rs`
- Modify: `crates/fin-store/src/db.rs` (`Counters::raise_to_log`)
- Test: `crates/fin-store/tests/import.rs`

**Interfaces:**
- Consumes: `scan` (Task 5), `chain::resolve` (Task 6), `replay_record` (Task 3).
- Produces:
  - `pub(crate) fn import(store: &mut Store) -> Result<ImportReport, StoreError>`, where `ImportReport { applied: Vec<CommittedTx>, quarantined: u32, waiting: Vec<String>, fork: Option<ForkInfo>, foreign: Vec<ForeignFile>, newer: bool }`.
  - Applied records keep their bytes and hash, are marked `exported = 1`, and fill `segments`.
  - A placeholder is hydrated by reading it once per `tick`, at most 16 MiB per tick.
  - After an import, `Counters::raise_to_log(&txn)` sets each id kind to (max id seen in `txs.bindings` and the snapshot) + 1.
  - The ids now include `branch` (prefix `br`) for 4B. Add `IdKind::Branch` in fin-core, which needs no migration because `ids` is keyed by text.

- [ ] **Step 1: Write failing tests:**
  - `second_store_imports_first_stores_segments` (two server-data dirs, one folder);
  - `import_upcasts_and_applies_without_policy`;
  - `quarantine_counts_and_keeps_bytes`;
  - `ids_never_reused_after_takeover`: A commits c1..c5 and exports; B imports and commits an insert; B's new clip is c6;
  - `newer_segment_makes_read_only_newer_format`;
  - `placeholder_hydrates_then_applies`.
- [ ] **Step 2–4: Run → FAIL; implement; run → PASS.**
- [ ] **Step 5: Commit** `feat(store): import folder segments with quarantine and id counter repair`.

### Task 9: Lease v3

**Files:**
- Create: `crates/fin-store/src/lease/mod.rs`, `lease/record.rs`, `lease/clock.rs`, `lease/claim.rs`, `crates/fin-store/src/server_id.rs`
- Test: `crates/fin-store/tests/lease.rs`

**Interfaces:**
- Produces:
  - `pub fn server_id(server_data: &Path) -> Result<String, StoreError>` (Decision 5).
  - `lease::record`: `encode(&LeaseRecord) -> [u8; 512]` and `decode(&[u8]) -> Option<LeaseRecord>`.
    - Layout, ported from the spike: compact JSON body, then `\n`, then the CRC-32C of the body as 8 hex, then `\n`, then ASCII spaces to 512 bytes.
    - `LeaseRecord { magic: "finisher-lease", version: 1, server_id, epoch, heartbeat: u64, holder: String, probe_server_ms: i64 }`.
  - `lease::clock::ServerClock`:
    - `probe(dir) -> Result<ProbeSample, io::Error>`: rewrite `lease.probe-<serverId>` in place, `sync_all`, then read the mtime from a fresh uncached `stat` → `ProbeSample { server_ms, rtt: Duration, at: Instant }`.
    - `now_server_ms(&self, at: Instant) -> Option<i64>`: last sample + monotonic elapsed. Returns `None` if the last sample is older than 60 s, or the mtime didn't advance.
    - Re-probe every 30 s.
  - `lease::Params::for_kind(StorageKind) -> Params { ttl: 10s, heartbeat: 5s, stale_after: 45s, confirm_gap: 5s, max_rtt: 2.5s, self_fence: 20s, auto_takeover: bool }`. `auto_takeover` is false for `Sync`.
  - `pub struct Lease` (`Send + Sync`; owns a heartbeat thread; stopped on drop):
    - `acquire(dir, server_id, holder, params) -> Result<Lease, LeaseState>`;
    - `state(&self) -> LeaseState`;
    - `check(&self) -> Result<u64 /*epoch*/, LeaseLost>` (spike `check()` order, with "unreadable = lost");
    - `epoch()`;
    - `takeover(dir, …, confirm: bool) -> Result<Lease, CoreError>`.
  - `pub enum LeaseState { Held { epoch }, HeldElsewhere { holder, epoch, age_server_ms: Option<i64> }, Stale { holder, epoch }, Lost { reason } }`.
  - **Claims:**
    - `lease.claim-<epoch>`, created with `create_new`, content = server id;
    - each acquisition tries `base+1..base+4`;
    - after acquiring, delete claims below its own epoch;
    - "missing `lease.json` while any claim exists" = held;
    - unreadable or bad CRC = held when acquiring.
  - **Staleness:** `now_server_ms - probe_server_ms_in_record > stale_after`, observed in two probes ≥ `confirm_gap` apart, each with rtt ≤ `max_rtt`, with the record unchanged between them.
  - **Clock injection for tests:** `Params` carries `Arc<dyn MonoClock>`. Tests use a fake monotonic clock and a temp dir whose mtimes are real.

- [ ] **Step 1: Write failing tests:**
  - `record_is_512_bytes_and_crc_checked`;
  - `first_acquire_takes_epoch_1`;
  - `second_server_sees_held`;
  - `claim_name_exclusive_gives_unique_epochs` (two threads race 200× → never the same epoch);
  - `missing_record_with_claim_is_held`;
  - `bad_crc_is_held_on_acquire_and_lost_on_fence`;
  - `stale_needs_two_probes_5s_apart`;
  - `self_fences_after_20s_without_heartbeat`;
  - `sync_kind_requires_confirm` → `LEASE_TAKEOVER_REFUSED` without confirm;
  - `reacquire_after_lost_takes_fresh_epoch`;
  - `old_claims_deleted_after_acquire`;
  - `server_id_is_stable`.
- [ ] **Step 2–4: Run → FAIL; implement; run → PASS.**
- [ ] **Step 5: Commit** `feat(store): lease v3 (in-place checksummed record, exclusive claims, server-time staleness)`.

### Task 10: Storage-kind detection

**Files:**
- Create: `crates/fin-store/src/storage_kind.rs`
- Test: inline unit tests, plus `crates/fin-store/tests/storage_kind.rs`

**Interfaces:**
- Produces:
  - `pub enum StorageKind { Local, Network, Sync(Provider) }`, serialised as `"local" | "network" | "sync:<provider>"`.
  - `pub fn detect(dir: &Path, override_: Option<StorageKind>) -> StorageKind`.
- Rules (spec §3.5):
  - **macOS:**
    - a path under `~/Library/Mobile Documents` → `Sync(ICloud)`;
    - under `~/Library/CloudStorage/<Provider>-*` → that provider (`Dropbox`, `OneDrive`, `GoogleDrive`, else `Unknown`);
    - `statfs` `f_fstypename` in `smbfs | nfs | afpfs | webdav` → `Network`.
  - **Linux:** `statfs` `f_type` in `{SMB2 0xFE534D42, CIFS 0xFF534D42, NFS 0x6969, FUSE 0x65735546 (only when path is under a known Dropbox/Syncthing root)}`. A `.stfolder` ancestor → `Sync(Syncthing)`; a `.dropbox` ancestor → `Sync(Dropbox)`.
  - **Windows:**
    - a UNC path or `GetDriveTypeW == DRIVE_REMOTE` → `Network`;
    - a path under `%OneDrive%`/`%OneDriveCommercial%`/`%OneDriveConsumer%` → `Sync(OneDrive)`;
    - under a Dropbox `info.json` root (`%APPDATA%\Dropbox\info.json`, `%LOCALAPPDATA%\Dropbox\info.json`) → `Sync(Dropbox)`;
    - a folder with `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` capability (has a cloud-files sync root, via the attribute on the root) → `Sync(Unknown)`.
  - `finisher.project.json` field `storageOverride` wins.

- [ ] **Step 1: Write failing tests:** table tests over pure helpers (`classify_macos_path`, `classify_linux_fs`, `classify_windows`), plus `override_wins`.
- [ ] **Step 2–4: Run → FAIL; implement; run → PASS.**
- [ ] **Step 5: Commit** `feat(store): detect local, network and sync-folder storage`.

### Task 11: Store integration — open, tick, read-only, follow

**Files:**
- Create: `crates/fin-store/src/status.rs`, `crates/fin-store/src/tick.rs`
- Modify: `crates/fin-store/src/lib.rs` (keep it under 600 lines; move the open/create logic to `open.rs` if needed), `crates/fin-store/src/paths.rs` (`finisher.project.json` with `formatVersion`, `minReaderVersion`, `storage`, `storageOverride`; read retry for 15 s on `NotFound` once the folder exists)
- Test: `crates/fin-store/tests/integration.rs`

**Interfaces:**
- Consumes: Tasks 2–10.
- Produces:
  - `Store::open(server_data, project_dir) -> Result<Store, StoreError>`. Order:
    1. Read the project file.
    2. Check for a copy (Decision 6).
    3. If the DB is missing, or `quick_check` fails, rebuild (Task 12). Otherwise migrate.
    4. Detect the storage kind.
    5. Try to acquire the lease.
    6. `import`.
    7. Export if holding.
  - `Store::create` is unchanged in its signature (Task 16 makes it atomic).
  - `Store::tick(&mut self, now: Instant) -> Result<Vec<StoreEvent>, StoreError>`. Each call:
    - polls at the kind's cadence (Decision 3);
    - imports;
    - exports if holding;
    - retries the lease (automatic takeover when stale and `auto_takeover`; on success, import before allowing commits);
    - re-checks fencing;
    - removes old temp files;
    - runs daily backups (Task 12) and compaction when triggered (Task 13).
  - `pub enum StoreEvent { Applied(CommittedTx /*branch inside tx*/), Status(ProjectStatus) }`. `Status` is emitted only when the status changes.
  - `Store::status(&self) -> ProjectStatus`, where `ProjectStatus { project_id, storage: StorageKind, lease: LeaseState, read_only: Option<ReadOnly>, branches: Vec<BranchStatus { branch, head, exported_through }>, last_export_ms: Option<i64>, export_error: Option<CoreError>, quarantine: QuarantineSummary { count, origins: Vec<String> }, foreign: Vec<String>, waiting: Vec<String> }`, serialised camelCase.
  - `pub struct ReadOnly { reason: ReadOnlyReason, fix: String }` with `ReadOnlyReason { LeaseHeld, LeaseLost, ChainFork, ConflictCopy, NewerFormat }`, serialised `SCREAMING_SNAKE`.
  - `Store::commit` returns `PROJECT_READ_ONLY` (`details.reason`, `fix`) whenever `read_only` is `Some`.
  - `Store::takeover(&mut self, confirm: bool) -> Result<ProjectStatus, StoreError>`.
  - `Store::close(self)` flushes the export (`force`) and releases nothing (the lease simply stops heartbeating, so others wait out staleness). Recorded so tests don't expect instant handover.
  - Tests in `integration.rs` use two `Store`s on one folder with separate server-data dirs and a fake monotonic clock via `Params`.

- [ ] **Step 1: Write failing tests:**
  - `writer_and_follower_converge` (A commits; B ticks; B's snapshot equals A's);
  - `follower_commit_is_read_only_lease_held`;
  - `takeover_after_writer_stops`: A stops ticking; with the fake clock 45 s + 5 s later, B auto-takes over and commits;
  - `conflict_copy_makes_read_only` (drop `… (conflicted copy 2026-11-04).fseg` in);
  - `lease_json_conflict_copy_makes_read_only_on_sync`;
  - `newer_format_read_only`;
  - Review Focus tests:
    - `open_export_follow_in_unicode_dir` (dir `Мой проект 1`);
    - `folder_vanishes_then_returns` (rename the folder away → `LEASE_LOST` and writes refused; rename back → writable again with a new epoch);
    - `two_server_datas_one_folder`;
    - `copied_folder_is_refused` (`PROJECT_COPY`);
  - `project_file_missing_briefly_is_retried`.
- [ ] **Step 2–4: Run → FAIL; implement; run all, `pnpm test`, `pnpm e2e` and gates → PASS.**
- [ ] **Step 5: Commit** `feat(store): open/tick integration — lease, import, export, read-only states, follow`.

### Task 12: Rebuild, verify, backups, salvage

**Files:**
- Create: `crates/fin-store/src/rebuild.rs`, `crates/fin-store/src/verify.rs`
- Modify: `crates/fin-store/src/backup.rs`
- Test: `crates/fin-store/tests/rebuild.rs`

**Interfaces:**
- Produces:
  - `pub(crate) fn rebuild(server_data, project_dir, old_db: Option<&Path>) -> Result<RebuildReport, StoreError>`:
    1. Build `project.fdb.rebuild-<ts>` from the folder: snapshot seed only if its `txHash` is on the resolved chain; then segments.
    2. Verify it.
    3. Salvage readable unexported records from the old DB into `quarantine` (origin `salvage`).
    4. Rename the old DB to `project.fdb.corrupt-<ts>`.
    5. Rename the new one into place.

    `RebuildReport { records: u64, salvaged: u32, seeded_from_snapshot: Option<u64> }`.
  - `Store::rebuild(&mut self)` (manual).
  - `Store::verify(&self) -> VerifyReport { ok: bool, checks: Vec<Check { name, ok, details: Value }> }`. Check names, in order:
    - `integrity_check`
    - `segment_checksums`
    - `chain_continuity`
    - `epoch_order`
    - `replay_matches_live`
    - `diffs_match`

    A mismatch is reported, never fatal. A segment that is corrupt but has stopped changing for 60 s is reported as `SEGMENT_CORRUPT` inside `segment_checksums`.
  - `backup::daily(store, now_ms)` writes `<project-data>/backups/daily-YYYY-MM-DD.fdb` at most once per local calendar day and keeps 7.
  - `quick_check` runs on every open; failure → automatic rebuild.

- [ ] **Step 1: Write failing tests:**
  - `deleted_db_rebuilds_identically`;
  - `corrupt_db_rebuilds_and_salvages_unexported` (corrupt page bytes in a copy; 3 unexported commits end up in quarantine);
  - `rebuild_ignores_offchain_snapshot`;
  - `verify_reports_each_check` (inject a diff mismatch → `diffs_match` false, overall `ok` false, the store still opens);
  - `daily_backup_once_per_day_keeps_seven`.
- [ ] **Step 2–4: Run → FAIL; implement; run → PASS.**
- [ ] **Step 5: Commit** `feat(store): rebuild from folder with salvage, verify report, daily backups`.

### Task 13: Compaction

**Files:**
- Create: `crates/fin-store/src/compact.rs`
- Test: `crates/fin-store/tests/compact.rs`

**Interfaces:**
- Produces:
  - `pub(crate) fn compact(store: &mut Store, now_ms: i64, force: bool) -> Result<CompactReport, StoreError>`.
  - It runs only while holding the lease, when a branch has more than 200 non-pack segments (or `force`).
  - **Phase 1:**
    1. Write a snapshot at the branch head (`.fsnap`).
    2. Write packs: each holds at most 1,000 records / 4 MiB of the replaced segments' records, byte-for-byte, with `pack: true` and `replaces: [hashes]`. Each pack's `prev_segment_hash` is the predecessor of the first segment it replaces.
    3. Write `compaction/<ulid>.json` `{ branch, replaced: [names], packs: [names], snapshot: name, created_ms, kind }`.
  - **Phase 2** (a later `tick`, after the grace period): read back and verify the packs and snapshot, delete the replaced files, then delete the manifest.
  - Packs are never re-packed.
  - Interrupted compaction is safe at every step:
    - before the manifest, the extra packs are just duplicates;
    - a manifest is re-checked and finished on the next tick.

- [ ] **Step 1: Write failing tests:**
  - `compaction_preserves_every_record_and_hash`;
  - `fresh_reader_during_grace_window_sees_no_fork`;
  - `fresh_reader_after_deletion_resolves_through_pack`;
  - `interrupted_at_each_phase_is_safe` (FailPoints);
  - `follower_never_compacts`.
- [ ] **Step 2–4: Run → FAIL; implement; run → PASS.**
- [ ] **Step 5: Commit** `feat(store): two-phase compaction into packs that keep full history`.

### Task 14: fin-node bindings, maintenance thread, events

**Files:**
- Modify: `crates/fin-node/src/lib.rs` (split into `engine.rs`, `tasks.rs` and `maintenance.rs` to stay under 600 lines), `packages/fin-node/index.js`, `packages/fin-node/index.d.ts`
- Test: `packages/fin-node/test/storage.test.ts`

**Interfaces:**
- Consumes: `Store::{tick, status, takeover, verify, rebuild, compact}` and `StoreEvent`.
- Produces (JS):
  - `engine.status(): Promise<string>`.
  - `engine.takeover(confirm: boolean): Promise<string>`.
  - `engine.verify(): Promise<string>`, `engine.rebuild(): Promise<string>`, `engine.compact(): Promise<string>` (long calls; Decision 7).
  - `engine.txGet(txId: string): Promise<string | null>` (returns the `CommittedTx` JSON).
  - `engine.onEvent(cb: (json: string) => void): void`, through a napi `ThreadsafeFunction`. Events are `{"type":"applied","tx":CommittedTx}` or `{"type":"status","status":ProjectStatus}`.
  - **Maintenance thread:**
    - one per Engine, ticking every 500 ms under the store mutex;
    - stops on `close()`;
    - a panic inside `tick` marks the engine `Panicked`, like other calls.

- [ ] **Step 1: Write failing vitest tests:**
  - `status reports lease held and storage kind`;
  - `second engine on same folder follows and emits applied events`;
  - `txGet returns committed tx or null`;
  - `verify returns report`;
  - `close stops maintenance`.
- [ ] **Step 2–4: Run `pnpm test` → FAIL; implement; run → PASS.**
- [ ] **Step 5: Commit** `feat(fin-node): storage status, takeover, verify, rebuild, compact, tx lookup and event callback`.

### Task 15: Contracts, finisherd, fin commands

**Files:**
- Modify: `packages/contracts/src/rpc.ts`, `apps/finisherd/src/{projects.ts,handlers.ts}`, `crates/fin/src/{backend.rs,output.rs,registry.rs,main.rs}`, `crates/fin/src/commands/project.rs`
- Create: fixtures in `crates/fin/tests/fixtures/rpc/` (via `capture-rpc-fixtures`)
- Test: `apps/finisherd/test/storage.test.ts`, `crates/fin/tests/cli_online.rs`, `crates/fin/tests/rpc_contract.rs`

**Interfaces:**
- **New RPC tags** (all with `error: FinError`):

  | Tag | Payload | Success |
  |---|---|---|
  | `project.status` | `{projectId}` | `Unknown` (ProjectStatus) |
  | `project.takeover` | `{projectId, confirm: Boolean}` | `Unknown` (ProjectStatus) |
  | `project.verify` | `{projectId}` | `Unknown` |
  | `project.rebuild` | `{projectId}` | `Unknown` |
  | `project.compact` | `{projectId}` | `Unknown` |
  | `project.recover` | `{projectId}` | `{manual: String}` (the documented manual steps from spec §3.7; real in 4B) |
  | `project.events` | `{projectId}` | stream of `{type: "status", status}` |
  | `tx.get` | `{projectId, txId}` | `CommittedTxSchema` or `null` |

- **finisherd:**
  - registers `engine.onEvent`;
  - `applied` events publish to the per-branch hub, so followers' `ops.subscribe` clients see imported edits;
  - `status` events publish to a per-project status PubSub.
- **`fin` commands:**
  - `fin project status|takeover [--confirm]|verify|rebuild|compact|recover`, each with a registry entry, a `main.rs` arm and a sample in `every_registry_command_parses`;
  - `--idem-key` retry uses `tx.get` when the commit outcome is unknown.
- **Exit codes:**
  - `PROJECT_READ_ONLY`, `LEASE_TAKEOVER_REFUSED` → 4;
  - `DB_TOO_NEW`, `NOT_A_PROJECT_DB`, `UNSUPPORTED_HASH`, `NEWER_FORMAT`, `PROJECT_COPY`, `REPLAY_FAILED` → 3.

- [ ] **Step 1: Write failing tests:**
  - finisherd `status/takeover/verify RPCs`;
  - `events stream emits status on lease change`;
  - `follower ops.subscribe receives imported edits`;
  - fin `project_status_prints_json`;
  - `read_only_commit_exits_4`;
  - rpc_contract fixtures for every new tag.
- [ ] **Step 2–4: Run → FAIL; implement; regenerate fixtures; run all, `pnpm e2e` and gates → PASS.**
- [ ] **Step 5: Commit** `feat: project storage RPCs, events stream and fin project commands`.

### Task 16: SP2 follow-ups (Rust side)

**Files:** `crates/fin-store/src/lib.rs` (`create`), `crates/fin-core/src/time.rs`, `crates/fin/src/commands/{mod.rs,project.rs}`, `crates/fin-probe/` (JsonSchema derive and MOV test), `crates/fin-core/examples/export_schema.rs`, `packages/core-types` (regenerated)

**Interfaces:**
- **Atomic `Store::create`:**
  1. Write the DB as `project.fdb.creating-<ulid>` and the marker temp.
  2. Rename the DB, then rename the marker with `write_once` semantics (`AlreadyExists` → `PROJECT_EXISTS`).
  3. Retrying with the same `--idem-key` (the new `create` param `idem: Option<String>`, stored in `meta.createKey`) returns the existing project.
  4. An empty or whitespace-only name → `INVALID_NAME` (exit 3).
- **Canonical rationals:** `parse_time`/`Rational::from_str` reject non-canonical input (`2/4`, `-0`, `+1`, `01`, `0/5`) with `INVALID_TIME` and `fix` showing the canonical form.
- **Numeric `details.rev`:** every error with `details.rev` sets it numeric (grep `"rev"` in `crates/fin-store` and `crates/fin`).
- **Idempotency key hash:** `--idem-key` commits store an args hash (`blake3` over the op kinds plus canonical args JSON) in the record's intent. The same key with a different hash → `IDEM_KEY_REUSED`.
- **`Probe` type:** `fin-probe::Probe` derives `JsonSchema` and is exported into `fin-core.schema.json` definitions, so `packages/core-types` exports `Probe`. Add a MOV fixture test (`fixtures` feature writes a `.mov` brand).

- [ ] **Step 1: Write failing tests:**
  - `create_is_atomic_under_marker_failure`;
  - `create_retry_same_key_returns_project`;
  - `empty_name_rejected`;
  - `noncanonical_rationals_rejected` (table);
  - `idem_key_same_key_different_args_rejected`;
  - `probe_mov_fixture`;
  - the generated-types gate passes with `Probe`.
- [ ] **Step 2–4: Run → FAIL; implement; run all and gates → PASS.**
- [ ] **Step 5: Commit** `fix: SP2 follow-ups — atomic idempotent create, canonical rationals, idem args hash, Probe type`.

### Task 17: SP2 follow-ups (TS side) and typecheck gate

**Files:** `apps/finisherd/src/projects.ts`, `packages/contracts/src/rpc.ts`, `packages/fin-node/index.js`, `packages/fin-node/tsconfig.json` (new), `scripts/gates.mjs`, `scripts/check-typecheck.mjs` (new) with its test, root `package.json`

**Interfaces:**
- `ops.subscribe` with `fromRev` > head fails with `BASE_REV_AHEAD` before streaming.
- Hubs and status PubSubs are removed when their subscriber count drops to 0 and no engine handle refers to them.
- Linux loader: choose `-gnu` or `-musl` by `process.report.getReport().header.glibcVersionRuntime` (present = gnu), and fall back to trying the other.
- `contracts` uses the generated `Probe` type for `asset.probe`.
- Gate `check-typecheck.mjs` runs `tsc --noEmit -p` for `apps/finisherd`, `packages/contracts` (tests included), `packages/core-types` and `packages/fin-node`. Add `@types/node` where missing.

- [ ] **Step 1: Write failing tests:**
  - `subscribe beyond head fails BASE_REV_AHEAD`;
  - `hub removed after last unsubscribe`;
  - loader unit test with a mocked report;
  - `check-typecheck.test.mjs` (fails on a seeded type error in a temp copy).
- [ ] **Step 2–4: Run → FAIL; implement; fix any type errors the new gate finds; run all and gates → PASS.**
- [ ] **Step 5: Commit** `fix: SP2 follow-ups — subscribe bounds, hub cleanup, loader libc detection, typecheck gate`.

### Task 18: e2e scenario

**Files:** `scripts/e2e.mjs` (split scenarios into `scripts/e2e/` modules if it nears 600 lines)

- [ ] **Step 1:** Add the scenario `storage`:
  1. Create a project; make 5 writes; `fin project status` shows `exportedThrough == head`.
  2. Stop finisherd and delete `<server-data>/projects/<id>/project.fdb*`.
  3. Start it, open the project, and check `snapshot.get` equals the pre-delete snapshot.
  4. `fin project verify` returns `ok: true`.
  5. A second finisherd with its own server-data opens the same folder: `status.readOnly.reason == "LEASE_HELD"`. A commit through it exits 4. A WS `ops.subscribe` on it receives a new edit made through the first server within 5 s.
- [ ] **Step 2:** Run `rtk pnpm e2e` on macOS. Expected: both scenarios pass.
- [ ] **Step 3: Commit** `test(e2e): storage scenario — rebuild from folder and follower`.

### Task 19: Cross-platform and cross-host validation (controller-run; needs hosts)

**Files:** `crates/fin-store-xhost/` (new workspace crate, `publish = false`, not shipped; bin `xhost`), `tests/storage-xhost/run.mjs`, `tests/storage-xhost/README.md` (no host details). Results are written to `archive/spike/sp4a-xhost/` (git-ignored).

**Interfaces:**
- `xhost` subcommands, each emitting JSONL with host monotonic and wall ns:
  - `holder --dir D --secs N --commit-every-ms M`: acquire, commit, export, tick;
  - `contender --dir D --secs N`: tick until it holds, then commit;
  - `follower --dir D --secs N`: tick, recording applied seqs and times;
  - `verify --dir D`.
- `run.mjs` reads host and share access from `archive/hosts.md`-derived env (`XHOST_*` variables, never committed), copies binaries, runs the matrix and summarises.

- [ ] **Step 1:** Run the full suites on Windows 11 and the Linux VM: `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e`. Record the results in TASKS.md.
- [ ] **Step 2:** Run the NAS matrix (spec §3.12). Rows:
  1. lease races, 100 per pair (Mac↔Win, Mac↔Linux, Win↔Linux);
  2. network loss with writes in flight;
  3. real skew (opportunistic, else emulated, noted);
  4. follow latency.

  Required: 0 double holders, 0 early takeovers, 0 applied stale writes, 0 id reuse, and follow ≤ 3 s plus the cache window.
- [ ] **Step 3:** Run the sync-folder rows on the Mac (iCloud Drive, plus Dropbox/OneDrive if installed): temp-then-rename upload, conflict-copy naming, placeholder hydration. Rows that can't run are recorded as blocked on the owner.
- [ ] **Step 4:** Fix the defects found, each with a regression test in the owning crate. Then re-run the affected rows.
- [ ] **Step 5: Commit** `test(storage): cross-host lease and sync-folder harness`. Results stay in the archive.

### Task 20: Docs and tracker

**Files:** `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 pointer), `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` (record Decisions 1–7 and the measured results), `TASKS.md`, `README.md` (`fin project …` in "Run it")

- [ ] **Step 1:** Record the decisions and results; tick the 4A follow-ups; add any new follow-ups found.
- [ ] **Step 2:** Run `rtk pnpm gates`. Expected: ok.
- [ ] **Step 3: Commit** `docs: record 4A decisions and results`.
