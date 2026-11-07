# Sub-project 4: Core breadth — Design

- **Status:** draft for review (2026-11-04). Stage 4A implemented (2026-11-05, branch `sp4a-durable-storage`); §3 describes it as built. Its cross-host validation (§3.13) is pending. Stage 4B implemented (2026-11-06, branch `sp4b-history`, stacked on 4A); §4 describes it as built, and the places in §3 that 4B changed say so. Its cross-host validation (§4.8) is pending.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §6, §7, §9, §12.3–12.7 and §27.9 for sub-project 4. Where the two disagree, this document wins for sub-project 4, and arch §27 gets a pointer when the stage merges.
- **Builds on:** sub-project 2 (merged `382b344`): `fin-core` with `asset.add`, `clip.insert`, `clip.trim`, `clip.delete`; `fin-store` SQLite op log; `fin-node`; `@finisher/contracts`; `finisherd`; `fin`.

## 1. Goal and stages

Sub-project 4 makes the core complete enough that later sub-projects only consume it:
- projects survive format changes, machines, NAS and sync folders (**4A**);
- edits have history: branches, merge, undo (**4B**);
- the model covers everything v1 edits, and evaluation tells the media engine exactly what to draw and play (**4C**).

It is built in three stages, in this order. Each stage gets its own implementation plan. Each is merged only after it passes on macOS, Windows 11 and the Linux VM, including the NAS rows that apply. 4A comes first for two reasons: every later model change needs migrations, and the lease is the riskiest piece.

**Success, for all stages:**
- every capability can be driven and tested through `fin` → `finisherd` (or `fin` offline where it applies);
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on all three OSes;
- there is no UI work.

**Out of scope:**
- pixels and sound: compositor, mixer, rasterising text (sub-project 6);
- transcription (sub-project 8);
- `.otio` import/export;
- `fin apply`, `fin batch` and `fin tx begin|commit` (sub-project 7);
- review items built from conflicts (sub-project 5);
- browser-engine (`fin-wasm`) replay tests (sub-project 3).

## 2. Principles fixed by the design discussion

These follow the research summarised in §7.

1. **History is a record of facts.**
   - A committed transaction is stored as written and never rewritten.
   - Old records are upgraded **in memory, on read**, by pure, total, deterministic one-step converters ("upcasters").
2. **Replay applies; it never re-judges.**
   - Replaying committed history runs only structural checks: the referenced entity exists, and no clips overlap on a track.
   - Policy checks run only when an edit is made. Examples of policy-only checks: the one-frame minimum duration, `sourceIn + duration` within the asset (assets can be relinked), trim-delta limits, safe-zone overflow. (`INSERT_INSIDE_CLIP` is a policy check but not a good example: with ripple an insert strictly inside a clip always overlaps, so replay rejects it structurally anyway. §3.2 lists both sets.)
   - A rule added later never rejects an existing project.
3. **Intent and result are both stored.**
   - Each record keeps the command as submitted (with `$placeholders`) and the resolved ops (bindings substituted), plus the inverse.
   - Replay applies the stored intent with the recorded bindings feeding id allocation, which is equivalent to applying the resolved ops (plan Decision 2, §3.2).
   - Rebase, merge, cherry-pick and undo create *new* edits: they re-run intent through the rebase engine with full policy checks.
4. **Single writer, fenced on both sides.**
   - The lease (v3) decides who writes; epochs are per project.
   - Every segment carries an epoch: the highest lease epoch its records were committed at (§3.4 "Export").
   - Where two lines of history continue from the same point, readers keep the higher-epoch one and set the other aside. §3.6.2 step 6 is the exact rule.
   - The storage can't enforce a token, so writer-side fencing alone can always race.
5. **The local DB is a cache of the folder plus the not-yet-exported tail.**
   - It can always be rebuilt from the project folder's files.
   - It is never wiped to "recover": unexported records are salvaged to quarantine first (§3.8).
6. **Newer data is never written by older code.**
   - Every persisted format carries `formatVersion` and `minReaderVersion` (Chromium's `last_compatible_version` pattern).
   - Code below `minReaderVersion` refuses.
   - Code at or above `minReaderVersion` but below `formatVersion` may open read-only.

---

## 3. Stage 4A — durable storage

**State (2026-11-05):** implemented on branch `sp4a-durable-storage` (plan `docs/superpowers/plans/2026-11-04-sub-project-4a-durable-storage.md`, Tasks 1–18). The cross-host and sync-folder validation (Task 19) has not run yet: see §3.13. This section describes 4A **as built**. The crate module docs in `crates/fin-store/src/` hold the exact rules, and this section names them where they matter.

**Decisions taken by the 4A plan** (the earlier draft left these open):

| # | Decision | Where |
|---|---|---|
| 1 | The DB column is `seq`. APIs keep the name `rev` for the same number (`CommittedTx.rev`, `opsSince(rev)`, `fromRev`). | §3.1 |
| 2 | `intent` is the `Tx` as applied (after any rebase, still with `$placeholders`). `resolved` is stored for readers and 4B. Replay applies `intent` with the recorded bindings feeding id allocation. | §3.2 |
| 3 | Following **polls** (local 2 s, network 1 s, sync 10 s) and does not use file-system events. *Deviation from the earlier §3.6.* | §3.6 |
| 4 | Status and history events go on a separate stream RPC, `project.events`. `ops.subscribe` does not change shape. *Deviation from the earlier §3.10.* | §3.10 |
| 5 | The server id is a lowercase ULID in `<server-data>/server-id`, created on first use (written atomically, mode 0600 on unix). | §3.5 |
| 6 | Project copies: the DB `meta` records `projectDir`. A folder whose project id is registered to another folder that still exists is refused (`PROJECT_COPY`). | §3.7 |
| 7 | Long native calls (open with import, takeover, verify, rebuild, compact, close) run on a per-engine worker thread, never on the libuv pool. | §3.10 |

### 3.1 Local database

- **Location:** `<server-data>/projects/<id>/project.fdb` (WAL, `synchronous=FULL`). The folder beside it holds `backups/`, `db.lock`, `lease.lock` and the kept copies from rebuilds (§3.8).
- **Header:** `PRAGMA application_id = 0x46494E31` ("FIN1"); `PRAGMA user_version` = the DB schema version (**1** at the end of 4A, **2** since 4B: migration 0002 below).
- **`meta`:** `formatVersion`, `minReaderVersion`, `createdBy` (app version; DBs migrated from sub-project 2 have none), `lastOpenedBy`, `projectId`, `projectDir` (§3.7), `incarnation` (§3.6.3), `genesisHash` (§3.4), `serverIds` (every server id that opened this DB, so "own records" survive a `SERVER_ID_RENEWED`), plus per-branch `generation.<b>`, `rewinds.<b>`, `diffsPending.<b>`, and `salvagedFrom`, `salvageIncomplete`, `compactionAbandoned`, `createKey` (§3.11). 4B adds `quarantineIdsScannedThrough` (§3.3), `quarantineMarksThrough` and `recoverDismissed` (§4.5 "Recovered branches"). `branchBase.<b>` is read only for a branch without an index row (§3.6.2 step 6a (d)) and never written.
- **Schema v1 tables:**
  - `meta (key, value)`, `ids (kind, next)`. `ids` is keyed by kind name, so a kind added later (`branch`) needs no migration;
  - `txs`: `branch`, `seq` (PK `(branch, seq)`), `tx_id` (unique), `kind`, `record_bytes` (the exact segment line, §3.3), `tx_hash`, `parent_hash`, `epoch` (the lease epoch it was committed at, or for an imported row its segment's epoch, an upper bound), `exported`, `diff`, plus the derived `schema_version`, `author` and `committed_at`. A partial index `txs_unexported` covers `exported = 0`;
  - `snapshots (branch, seq, json)`: rev 0 and every 200 revs;
  - `segments (branch, hash, name, epoch, first_seq, last_seq, pack, replaces, state)`. `state` is `exported` (written by this server and verified by read-back) or `imported` (taken from the folder; packs are always `imported`, because they carry other writers' records);
  - `lease_state (key, value)`;
  - `quarantine (tx_hash UNIQUE, branch, origin, reason, detail, record_bytes, added_at, seen_at, current)` (§3.6.2).
- **Migrations** (`crates/fin-store/src/migrations/`):
  - ordered steps. Each runs in its own `IMMEDIATE` transaction, which also sets `user_version` to the step's target, so a failed step rolls back whole and earlier steps stay committed;
  - `migrations::check` reads the header and `meta` through a read-only connection first, so a DB this code refuses is never written to;
  - before the first step, one backup is taken for the whole run (§3.8 "Backups").
- **Migration 0001 (sub-project 2 → v1)** is **frozen at schema v1**. It uses no current `fin-core` types (`RawValue`, `REFS_V1`) and pins its output with a test, so later model changes cannot alter what it writes. Its steps:
  - each old tx becomes a v1 record line: `seq` = its `rev`, and `intent`/`inverse`/`bindings` are its stored JSON verbatim;
  - `resolved` = `intent.ops` with the v1 reference fields substituted;
  - the records are hashed and chained in rev order at epoch 0, not exported;
  - `txs` and `snapshots` are rebuilt (`snapshots.rev` → `seq`);
  - it adds the 4A tables, `application_id`, `formatVersion` and `minReaderVersion`.

  A sub-project 2 project opens after migration with identical state (fixture `crates/fin-store/tests/fixtures/sp2/`, `expected.json` = `{rev, project}`).
- **Migration 0002 (v1 → v2, 4B)** is frozen at schema v2 the same way: literal values only, and it only creates tables and indexes (never `ALTER TABLE`), so a fresh schema (`db::SCHEMA`) and a migrated one stay identical. It creates the derived tables `branches`, `record_links` and `quarantine_marks` (§4.1, §4.5) and the index `quarantine_counted`, inserts the `main` row, and sets `formatVersion`/`minReaderVersion` to `"2"`. A 4A project opens after migration with identical state (fixture `crates/fin-store/tests/fixtures/4a/`, generated by 4A code and scrubbed of host details).
- **Refusals** (exit 3):
  - `user_version` above what the code knows, or `meta.minReaderVersion` above the code's version → `DB_TOO_NEW` (`fix`: update Finisher);
  - a foreign non-zero `application_id` → `NOT_A_PROJECT_DB`. Neither is ever rebuilt over (§3.8).
- **Integrity on open:** `rebuild::damaged` checks the header, schema and `meta`, then runs `PRAGMA quick_check(20)` read-only. Damage triggers an automatic rebuild (§3.8). `fin project verify` runs `integrity_check` and the other checks in §3.8.
- **`db.lock`:** every open `Store` holds a shared OS lock on `<project-data>/db.lock`; a rebuild holds it exclusively (§3.8). OS locks are reliable here because server data is local. They are never used in the project folder.

### 3.2 Versioning of ops and snapshots

- **`schemaVersion`:** each committed record carries the op-schema version it was written with. Snapshots carry the model-schema version.
- **Record format** (`fin-core::record`): `RECORD_FORMAT = 1`, `MIN_READER = 1` in 4A; **2/2 since 4B**. A record is `{"txId","seq","kind","schemaVersion","parentHash","author","committedAt","intent","resolved","inverse","bindings"}` in that field order, compact JSON, encoded once at commit (`encode_record`).
  - **4B kind fields** sit right after `kind`, in this order, each omitted when absent: `undoes, redoes, mergeId, from, origin, originHash, reverts, name, branchId, forkSeq, forkHash, epoch, archived, recovers` (§4.2). An `edit` record has none of them, so 4B writes `edit` records byte-identical in shape to 4A's, and stored bytes are never re-encoded.
  - `intent` is the `Tx` as applied (Decision 2): the rebased one, still with its `$placeholders`.
  - `resolved` = `intent.ops` with bindings substituted (`resolve_ops`).
  - `Tx` gains an optional `argsHash` (§3.11), omitted when absent so that older records stay byte-identical.
  - An unknown `kind` is `NEWER_FORMAT`. 4B's kinds are `edit, undo, redo, merge, cherryPick, revert, checkpoint, branch, branchSet` (camelCase); a field set that does not match its kind is `INVALID_FORMAT` (§4.2).
- **Upcasters** (`fin-core::upgrade`):
  - one pure step per version on `serde_json::Value`, covering records (intent, resolved, inverse) and snapshots;
  - steps must not change ids, timestamps or bindings;
  - upcasting a record also stamps `intent.schemaVersion` with the current version;
  - an older version with no step is `SCHEMA_VERSION`; a newer one is `NEWER_FORMAT`;
  - each step needs a golden test (frozen old shape in, expected new shape out). Version 1 is the first, so 4A has no steps yet.
- **Replay** (`fin-core::apply_committed(project, intent, bindings)`):
  - applies `intent` with the recorded bindings feeding id allocation (`BoundIds`). With every id fixed this is the same as applying `resolved`, and it reuses one apply path;
  - the bindings must equal what the intent allocates, else `BINDINGS_MISMATCH`;
  - `apply_tx` (the edit path) keeps full validation;
  - a structural failure is `REPLAY_FAILED {txId, cause}`.
- **Structural checks (run by replay):**
  - `NOT_FOUND`, `DUPLICATE_ID`, `TRACK_KIND_MISMATCH`, `ASSET_IN_USE`, `UNBOUND_PLACEHOLDER`, `DUPLICATE_PLACEHOLDER`, `TIME_OVERFLOW`;
  - asset duration > 0 (`INVALID_RANGE`, in every mode);
  - literal ids must be a kind prefix plus digits (`INVALID_ID`, replay only). In 4A `clip.restore` took no `$` ids. **Since 4B** its clip id may be a `$placeholder` (§4.3): the op allocates a fresh id (`Op::allocates` covers it), and replay hands the recorded one out again through `BoundIds`, like any other placeholder. A client tx still cannot send `clip.restore` (`INVALID_OP`); recovery and the history planners use it in `Internal` mode;
  - per shifted track after each op: `CLIP_OVERLAP`, `CLIP_EMPTY`, `CLIP_NEGATIVE` (start and `sourceIn` ≥ 0);
  - the record's `schemaVersion` must be current after upcasting.
- **Policy checks (skipped by replay):**
  - the one-frame minimum;
  - `sourceIn + duration` within the asset (assets can be relinked);
  - trim-delta limits;
  - a negative insert/restore start (replay catches it anyway as `CLIP_NEGATIVE`);
  - `INSERT_INSIDE_CLIP` (replay catches it anyway: with ripple, an insert strictly inside a clip always overlaps, so it fails `CLIP_OVERLAP`);
  - a no-ripple restore overlap (caught as `CLIP_OVERLAP`);
  - a bare `$`, and client-only rules.

  The examples in principle 2 (§2) therefore use the first three. `INSERT_INSIDE_CLIP` is a policy check, but not a *policy-only* one.
- **New codes (exit 3):** `NEWER_FORMAT`, `REPLAY_FAILED`, `INVALID_FORMAT`, `BINDINGS_MISMATCH`, `CLIP_OVERLAP`, `CLIP_EMPTY`, `CLIP_NEGATIVE`, `INVALID_ID`.
- **Open no longer fails on any diff.** Open uses the latest snapshot plus `apply_committed`. Stored diffs are not re-checked on open or import; they are served by `ops_since` and used by rebase, so **the stored diff wins until `fin project verify`** reports a mismatch (`diffs_match`, §3.8).

### 3.3 Hashes and identity

- **`txId`:** the ULID the client sends (unchanged). It is stable across upcasts and unique across branches.
- **`seq`:** dense per branch (§4.1 for how branches number). APIs call it `rev` (Decision 1).
- **`txHash`:** `blake3:<64 lowercase hex>` over the record's bytes **exactly as written**. Commit serialises the record once, stores the bytes in `txs.record_bytes`, hashes them, and export writes the same bytes. Nothing is re-serialised for hashing or export.
  - The record contains `parentHash`: the previous record's `txHash` on that branch, the branch's fork-point hash (4B), or `null` for the first record of `main`.
  - The record contains neither its own `txHash` nor the epoch.
- **Other hashes:** file hashes are BLAKE3 too; `<hash12>` is the first 12 hex characters. The CRC is CRC-32C, written as 8 lowercase hex.
- **Algorithm prefix:** every hash is written `<alg>:<hex>`. A reader that meets an unknown prefix reports `UNSUPPORTED_HASH` (exit 3) only when the file's `formatVersion` is newer than its own; otherwise the file is corrupt.
- **Canonical encoding:** stored bytes are never re-encoded, so the hash needs no canonical-JSON rules. Times stay rational strings, never floats.
- **Genesis:** the rev-0 project is built from `genesis = {name, rate}` (in `finisher.project.json`, §3.4). Its hash, `genesisHash` (BLAKE3 of that JSON), is in `meta`, in every segment header and in every snapshot header. A segment written under another genesis is never imported (`GENESIS_MISMATCH`, §3.6) and never used by a rebuild (§3.8): an edited `genesis.rate` would otherwise silently change a rebuilt history.
- **Entity ids after import:** replay doesn't mint ids. After every import that adds or removes rows, and on each store's first pass, every `ids` counter (including branch ids) is raised past the highest id in the log. Counters never go down: ids used by records that were later set aside (a diverged tail, a rewind) are never handed out again. **4B** also raises them from `branches.id` and from the targets of `branch`/`branchSet` records, and past every id a **quarantine** row binds or names (`Counters::raise_to_quarantine`, plan Decision 28): it scans the rows added since `meta` `quarantineIdsScannedThrough` (by `quarantine.id`, which only grows) on every pass, reading bindings, resolved-op ids and `branchId` (from the raw JSON when the record does not decode). So this server never hands out an id a set-aside record used; ids another server handed out first are remapped by recovery (§4.5).

### 3.4 Project folder layout and file formats

```
MyProject/
  finisher.project.json
  .finisher/
    lease.json                     v3 lease record (§3.5)
    lease.claim-<epoch:08>         v3 claim files (server id inside; §3.5)
    lease.probe-<serverId>         server-clock probe file
    segments/<branch>/<epoch:08>-<firstSeq:012>-<lastSeq:012>-<hash12>.fseg
    snapshots/<branch>/<seq:012>-<hash12>.fsnap
    compaction/<ulid>.json         two-phase compaction manifests (§3.9)
```

**Names and temp files**
- `<branch>` is `main` or a branch id `br<digits>` (4B; display names never reach the folder). Names are strict (`fin-store::names`), and only the canonical spelling parses (`lease.claim-12` does not). Anything else under `.finisher/` is a *foreign file* (§3.7). Segment names stay ASCII whatever the folder path.
- Every file is written to `.~fin-<ulid>.tmp` in its destination directory, fsynced (`F_FULLFSYNC` on Apple with a plain `fsync` fallback, which smbfs needs), renamed **without replacing**, then the directory is fsynced (a no-op on Windows, where it is ACCESS_DENIED). This is `fsio::write_once`. A finished `.fseg`, `.fsnap` or manifest is never modified or overwritten.
- The exceptions are `lease.json` and the probe files: they are rewritten in place (`write_in_place`, which never creates a file), never renamed over. `lease.json` is created once like any other file, but published only by a no-replace step the file server decides (§3.5 "Files").
- Lease and verification reads bypass the client cache (`read_uncached`).
- Errors treated as transient and retried (`fsio::is_transient`): unix `EINTR`, `EAGAIN`, `EBUSY`, `ESTALE`; Windows 5 (access denied), 32 (sharing violation), 33 (lock violation), 64 (network name deleted). The S10 spike also retried `ENOENT`, `EIO` and `EACCES` and Windows 2 and 3; they stay out of this generic list on purpose: "missing" is an answer the lease relies on, a real `EIO` or permission error would cost every caller 1.5 s, and the lease holder rides out a cifs `soft` mount's `EIO` by its own retry up to the self-fence deadline (§3.5). The reasons are in `fsio::transient`. §3.13 validates this list.
- **Temp sweep:** the export removes leftover `.~fin-*.tmp` files under `segments/`, `segments/*`, `snapshots/*` and `compaction/` (the holder's, and a crashed listing refresh's, §3.6). A file is removed once this server has seen it unchanged (same length and mtime) for 2×TTL (20 s) on its own monotonic clock, checked at most every 10 s. The sweep never compares the file's mtime with the local clock.

**Segment (`.fseg`).** UTF-8 JSON lines, each ending in `\n`:
1. **Header:** `{"magic":"finisher-segment","formatVersion":1,"minReaderVersion":1,"projectId","branch","writerId","epoch","firstSeq","lastSeq","prevSegmentHash","opSchemaVersions":[min,max],"createdAt","genesisHash"}`, plus on packs only `"pack":true,"replaces":[{"hash","lastTxHash"},…]`.
   - `epoch` is the **highest commit epoch** (`txs.epoch`) of the segment's records, not the lease epoch at export time (§3.4 "Export").
   - `prevSegmentHash` is the hash of the segment whose last record precedes this one's first, or `null` for the first.
   - `createdAt` (unix ms) is the latest `committedAt` of the records, so a re-export after a crash is byte-identical. It is informational only, never used for ordering.
   - **Versions follow content (4B, plan Decision 4).** A segment or pack whose records are all `edit` on `main` is written `formatVersion: 1, minReaderVersion: 1`, exactly as 4A wrote it; one holding any 4B kind, and every segment of a non-`main` branch, is `2/2` (`segment::version_for_branch`). The version is derived from the bytes, so a re-export stays byte-identical. Readers (reader version 2) accept both. A format-1 segment holding a 4B kind is corrupt (a v1 label means edit-only). *Why:* a 4A follower keeps following a 4B folder until a 4B kind is used; then it meets a format-2 file and goes read-only `NEWER_FORMAT`, as principle 6 wants.
   - **`branchRecord` (4B, Decision 33):** every segment of a non-`main` branch names the `txHash` of the `branch` record that made its branch, after `genesisHash` (packs keep it). Decode refuses a branch segment without it, a `main` segment with it, and a format-1 segment with it. Import, verify and compaction classify a branch directory's segments by the record they name (§4.1 "Import"), because an id `br<n>` alone can name two branches made on two servers across a takeover.
2. **Records:** one per line, the exact `record_bytes`.
3. **Trailer:** `{"crc32c":"<8 hex>","hash":"blake3:<64 hex>"}`, both over every byte before the trailer line. The CRC is the cheap corruption check; the BLAKE3 hash is the identity used in names and `prevSegmentHash`.

A truncated file always decodes as *incomplete* (not arrived); any single damaged byte of a complete file always decodes as *corrupt*.

**Snapshot (`.fsnap`).**
- The same frame. Snapshots, compaction manifests and `finisher.project.json` stay at format 1 in 4B; each file kind is checked against its own reader version (`snapfile::READER_VERSION = 1`), so a snapshot with `minReaderVersion` 2 is `Newer` although segments read version 2. Header: `{"magic":"finisher-snapshot","formatVersion","minReaderVersion","projectId","branch","seq","txHash","modelSchemaVersion","writerId","epoch","genesisHash"}`. The body is one line holding the model state.
- Snapshots are derived. In 4A only compaction writes them (§3.9), and a rebuild may seed from one (§3.8) only if its `(seq, txHash)` lies on the resolved chain and its `genesisHash` matches.

**`finisher.project.json`.**
- Fields: `id`, `name`, `schemaVersion` (sub-project 2), plus:
  - `formatVersion`, `minReaderVersion`. A file whose `minReaderVersion` is above ours opens read-only (`NEWER_FORMAT`); this is re-read on every poll;
  - `storage`: the kind detected when the file was written (informational; every open detects again);
  - `storageOverride`: the user's choice of kind (§3.5);
  - `genesis`: `{name, rate}`, what rev 0 is built from. Without it a server with no DB could not rebuild the project, because the rate is in no segment.
- Fields this version does not know are kept when the file is rewritten.
- It is the one folder file that is replaced, not written once. It is written temp-then-rename at create and on settings changes. One more rewrite exists: a holder opening a sub-project 2 project backfills `genesis` from the DB's rev-0 snapshot, together with the version fields and `storage`.
- Reading retries a missing file for up to 15 s, but only when `.finisher/` exists: Windows can report a just-renamed file missing for about 5 s. Transient errors are retried for the same 15 s.
- A folder that shows neither the file nor `.finisher/` fails at once on local disks and sync folders (a typo), but on **network storage** (judged by the folder, or by its parent when the folder itself is hidden; with neither present, a typo higher up in the path, it fails at once) it is retried for up to **12 s**: a client that looked the path up before another host created the project keeps answering "not found" from its cache (Windows ~4.9 s, its not-found cache; macOS smbfs 0.4–2.4 s; §3.13 item 14). On macOS network shares each retry first refreshes the listings of the folder's parent and the folder (create and remove a temp file, at most once a second, §3.6), which shows the new folder at once; on Windows that does not help, so only the wait does. A probe that could not be removed is removed on the next attempt and when the read ends; one still left is reported on stderr (nothing else sweeps the user's folder).
- A read of a cloud placeholder that times out (`ETIMEDOUT`; Windows `ERROR_CLOUD_FILE_REQUEST_TIMEOUT`, documented, not seen) is retried up to 3 more times 1 s apart, only while the file is a placeholder (`fsio::hydrating`: every uncached read, the project file at open and rebuild's snapshot seeds). Any other timeout is a real outage and is returned at once. Reads that must stay fast never wait for a download: the lease's record, claim and read-back reads (`fsio::read_uncached_once`; a failed one is a failed beat or an unreadable record) and each poll's reread of the project file (under the store's mutex; a failed reread changes nothing).
- A sync conflict copy of this file is a warning (`PROJECT_FILE_CONFLICT`); the copy is never read.

**Export** (`fin-store::export`)
- **Cadence:** the holder exports 2 s after the first unexported commit is seen. Once more than 30 segments have been written in the last 10 minutes (churn) it waits 5 s, or 30 s on a sync folder, where every file costs the sync client an upload and a download. On local and network storage the short delay keeps following fast, and compaction (§3.9) bounds the segment count. `close` flushes; dropping a `Store` does not, because folder I/O can block for seconds and a drop must not.
- **Acknowledged, not yet exported.** A commit is acknowledged once it is durable in the local database. Until its export (the delay above) it exists only on this server: if the holder stalls or dies before exporting and another server takes over and moves the folder on, the record is set aside as `DIVERGED_TAIL` when this server next imports (§3.6.2 steps 6–7): recoverable from quarantine, never silently dropped.
- **Gate:** export runs only while this server holds the lease and the project is writable (§3.7). The full lease check runs again right before every write (the fence).
- **Epoch label = highest commit epoch** of the segment's records, which must not exceed the current lease epoch. Labelling with the epoch at export time would launder records committed under a lost lease into a newer epoch. Example: B commits t1 at epoch 2 and loses the lease; C exports u at 3; B re-acquires at 4. Labelled 4, t1 would beat u, which other servers have already applied. Labelled 2, it loses, as the epoch rule intends (§3.6.2 step 6).
- **Sizes:** at most 1,000 records or 4 MiB per segment, and a transaction is never split. Commit refuses a record above 4 MiB − 1 KiB with `TX_TOO_LARGE` (exit 2), so every segment fits. A stored record that is already too large (only a migrated sub-project 2 record can be) stops the export at it with `RECORD_TOO_LARGE` (exit 3); verify reports it (`record_size`).
- **Write and read back.** A record is marked `exported` only after the segment's rename *and* an uncached read-back that decodes to our hash.
  - The read-back covers write-once outcomes that lie: `AlreadyExists` after an NFS link retransmit, `NotFound` after a Windows rename that succeeded on the server, and a failed directory fsync after the rename.
  - A file missing on read-back is retried briefly in place, then on a later pass with the same bytes.
  - Another valid file under our name → `SEGMENT_COLLISION` (exit 4). It is never overwritten; the next attempt adds 1 ms to `createdAt` for a different name. A file that is present but cannot be verified after repeated reads is treated as a collision.
- **Retries** reuse the exact bytes (and so the name) of the failed attempt while the lease is held. A file already in the folder that starts at the same seq, continues the same predecessor and holds exactly our next records is adopted rather than written twice.
- **Crash between the rename and the marking:** the next import finds folder records whose `txId` **and** `txHash` the DB holds unexported. It marks them exported from that file and never applies them again. The same `txId` with a different `txHash` is a fork (`TxIdReused`, §3.6).
- **Failures:** folder failures (unwritable, disk full, collision) don't fail the pass. They show as the status's `exportError`, the records stay unexported, and the export retries once per delay.

### 3.5 Lease v3

This implements arch §27.9 "v3" in `fin-store::lease`. The module docs there hold a table mapping every rule to its source. The spike code was not available, so the lease was written from §27.9 and this section; §3.13 lists what the cross-host runs must still confirm.

| Parameter | NAS / network | Local disk | Sync folder |
|---|---|---|---|
| TTL / heartbeat | 10 s / 5 s | 10 s / 5 s | 10 s / 5 s (advisory) |
| Stale after (server time) | **45 s** (arch §27.9 said 30 s) | 45 s | never automatic |
| Takeover | automatic: two stale probes ≥ 5 s apart, each with round trip ≤ TTL/4 (2.5 s), **and** the record unchanged for ≥ 25 s by the observer's monotonic clock | same | only with `--confirm` |
| Self-fence | last successful heartbeat + 2×TTL (20 s), suspend-aware monotonic clock | same | same |

**Why 45 s.** Windows documents three client-side cache windows: `FileNotFoundCacheLifetime` 5 s, `DirectoryCacheLifetime` 10 s and `FileInfoCacheLifetime` 10 s. Add the measured client skew of about 10 s and the 5 s heartbeat, and the total is about 40 s, so 30 s was too tight.

**Server identity (Decision 5)**
- The server id is a lowercase ULID in `<server-data>/server-id`, created on first use: a temp file (0600) moved into place without replacing, so racing processes both read the winner's id.
- `server-id.origin` records the server-data directory's identity and the OS machine id beside the id: `/etc/machine-id` (or `/var/lib/dbus/machine-id`), macOS `gethostuuid`, Windows `MachineGuid`.
- When either differs, the server data is a clone (`cp -a`, a restored backup, a cloned VM). It mints a **new** id (status `SERVER_ID_RENEWED`) and follows like any other server. Without a readable identity the restart shortcut below is off.

**Files**
- `lease.json`: a fixed 512-byte record. Compact JSON `{magic, version, serverId, epoch, heartbeat, holder, probeServerMs}`, then `\n`, the body's CRC-32C as 8 hex and `\n`, then spaces up to 512 bytes. `holder` is the machine name and app version, for people; `probeServerMs` is the holder's server time when it wrote the record. The record is created once, then only rewritten in place, so an in-place rewrite always covers the whole previous record. Two acquirers can both create it (the claims alone don't prevent that: acquirers with different views claim different epochs), so the step that names it is always one the file system or file server decides, never replacing: then exactly one wins. Creating it stages the whole record in a temp file in `.finisher/`, fsyncs it and publishes it (`lease::acquire::publish`):
  - Unix: a hard link (NFS `LINK`; SMB `FileLinkInformation` with `ReplaceIfExists` 0). No-replace renames are not used: Linux cifs enforces `RENAME_NOREPLACE` only by a local lookup and sends `ReplaceIfExists` 1, and `renamex_np(RENAME_EXCL)` on macOS smbfs is no better known;
  - Windows: `MoveFileExW` without replacing (`FileRenameInformation` with `ReplaceIfExists` 0);
  - no hard links (unsupported, or refused with `EACCES` as some NAS shares do): `create_new`, then writing. Still exclusive; misreading a refusal only costs crash-safety.

  Where it is published atomically (local disks with hard links, NFS, Linux cifs to servers with hard links, every Windows client), a crash leaves no record (a claim without one, which ages and is taken over automatically) or a whole one, never a short one, which reads as damaged and needs `takeover --confirm`. File systems and shares without hard links on Unix (FAT/exFAT, macOS smbfs as far as known) keep that risk; Task 19 records which real mounts take which path. A publish reported as failed whose record is ours on reading back (a lost reply) counts as done. A crash's temp file is swept by the next acquisition.
- `lease.claim-<epoch:08>`: one per acquisition, created with `create_new`, holding the claimant's server id. The exclusivity of the name is what makes epochs unique. Claims are a secondary signal: a new one can stay invisible to Windows for up to 9.9 s. The record's epoch is authoritative.
- `lease.probe-<serverId>`: each server's own probe file.
- The lease never creates `.finisher/`, so a project folder that vanished cannot come back empty.

**Time** (arch §27.9 "Clock skew", "Time")
- The lease never compares the local wall clock with a server mtime.
- **Server time** is the mtime of our own probe file, read uncached right after rewriting it, then carried forward by monotonic elapsed time. Every server time in the lease (the holder's stamp, an observer's "now", a claim's age) comes from the same file server.
- A probe sample is used only if it moved forward and lands no more than 1 s (plus the previous round trip) below the extrapolated server time. A sample older than 60 s is not used; the heartbeat re-probes every 30 s.
- **The monotonic clock counts time asleep:** `CLOCK_BOOTTIME` on Linux, `CLOCK_MONOTONIC` on macOS, `QueryInterruptTimePrecise` on Windows. The source is fixed at start-up. A sleeping machine therefore wakes already fenced. If no such clock is available, status warns `CLOCK_IGNORES_SLEEP`; the record compare and the claims still fence that machine.

**Acquiring** (`Contender::observe`, `take`)
- Reading `lease.json` uncached gives one of these states:
  - missing with no claims → free;
  - missing beside a claim → **held**, aged by the newest claim's server-time mtime, so an acquirer that crashed between claim and record cannot hold the folder forever. When the newest claim is this server's own and no lease of this server holds it (this process held that epoch and lost it, `lease.json` having been deleted under it; or the server-data lock is free, so no process of this server data runs a lease: an earlier run's claim), it is judged the same way but reported as **lost** (`LEASE_LOST`): this server knows it holds nothing and cannot prove a lease, while every other server sees it as held. Either re-acquires once the claim is stale; `takeover` refuses with the usual codes but says this server's lease was lost, and on a sync folder `takeover --confirm` re-takes it at once (it does not back off for our own lost claim: no acquisition is in progress, and the next claim name still decides a race) (§3.13 item 18);
  - unreadable or bad checksum → held, and taken over only by a confirmed takeover;
  - ours and still heartbeating in this process → held;
  - ours at an epoch this process held whose lease is lost or dropped → ours to re-take at once (in-process registry);
  - otherwise held, aged by this round's server time − the record's stamp.
- **Staleness needs all of:**
  - age > 45 s in two counting rounds ≥ 5 s apart, with no non-stale round between them;
  - the identical record (or claim set) seen for ≥ 25 s (self-fence + heartbeat) by our own monotonic clock. A running holder rewrites it within 20 s or fences itself, so it is never taken over, however wrong either clock is;
  - observations no more than 90 s apart (older evidence is discarded).

  A round whose probe doesn't move forward, or whose round trip exceeds 2.5 s, counts for nothing.
- **Take:**
  1. Back off if a claim above the record's epoch is younger than 45 s of server time (or its age is unknown): another acquisition is in progress.
  2. Claim exactly `base + 1`. `base` is the highest of the record's epoch, every claim, every segment name's epoch and the last epoch this process held here, so epochs never repeat even with every lease file gone or the record damaged. If that claim is taken, back off without creating anything: stepping on to `base + 2` could never win and would fence the winner. A name still pending deletion (Windows answers `create_new` with ACCESS_DENIED then) is retried like any transient error for 1.5 s, each attempt a `create_new`, so the name still has one owner; still failing, this attempt fails and the next one tries again.
  3. Re-read `lease.json`; it must be exactly what was judged.
  4. Write our record: created as under "Files" when it was missing (the file server decides, so a record hidden by Windows' not-found cache still wins), else in place.
  5. Read it back.
  6. Delete the claims below our epoch, then start the heartbeat.

  Any failure puts back what was there, deletes our claim and backs off.
- Every acquisition takes a fresh epoch, including automatic re-acquisition after `LEASE_LOST`, a restart, and a manual takeover.

**Heartbeat and fencing**
- One beat every 5 s:
  1. re-probe if due;
  2. **fence**: `lease.json` must still name our id and epoch, and no claim above our epoch younger than 45 s may exist (claims are the authority on epochs);
  3. right before writing, in memory: not dropped, not lost, deadline not passed (a beat can stall for any time on a hung share or a sleeping machine);
  4. rewrite in place with `heartbeat + 1`;
  5. read back and list claims again.

  A beat that finishes past the deadline revives nothing.
- A failed beat is retried after 1 s. Nothing is lost until the self-fence deadline, but evidence of another writer loses the lease at once.
  - **Evidence:** a valid record naming another server or epoch, a deleted `lease.json`, a young claim above our epoch.
  - **Not evidence**, only a failed beat: a read of our record that fails (a Linux cifs `soft` mount returns `EIO` while it reconnects), a record still not valid after 10 re-reads 20 ms apart (a torn or short SMB read; the spike's values), the same in the read-back, and a listing error (status `LEASE_CHECK`). `Lease::check` refuses its write then (`transient`) but keeps the lease. Nothing is ever written over a record the holder could not read. A damaged record therefore fences its holder at the deadline rather than at once (arch §27.9 says "lost when fencing"; the spike comparison, §3.13 item 3, showed that loses the lease on one bad read).
  - A failed write of our record, and a read-back showing an older heartbeat of ours (the write did not land, or not visibly), are failed beats too.
  - A failed beat is no success: the deadline still runs from the last beat that passed every step, and the next commit runs a full check instead of trusting one from the last second, including a check that was still running when the beat failed.
- **A record naming a lower epoch, or another server, over ours loses the lease**: the holder never rewrites its own record over it. This is deliberate. It cannot create a second holder, and only availability suffers (the lease is re-acquired at a fresh epoch). The S10 spike rewrote its record in that case.
- A leftover claim from a crashed acquirer makes the holder lose its lease. It re-acquires once the claim is 45 s old: old claims are ignored, never deleted by others.
- `Lease::check` (the fence) is run before every segment write, every compaction write or delete, and every commit. A commit runs the full check before taking the DB lock and only the in-memory part under it. It trusts a full check that passed within the last **1 s**: a commit writes only the local DB, and every folder write takes its own full check. A commit in that last second of a lost lease becomes a diverged tail (§3.6.2 step 7).
- Once lost, a lease stays lost; re-acquiring makes a new one. Dropping or closing does not release it: others wait out staleness. (A "released" record on clean close is a follow-up.)

**Restart shortcut** (`<server-data>/projects/<id>/lease.lock`)
- A lease holds an OS lock (`File::try_lock`) on this server-data file for its whole life, even while its heartbeat thread is stuck in I/O.
- On open, the folder's record may name our own server id. Then:
  - the lock is free, the record's holder host is this host, the server data is on a local disk (an allow-list: ext2/3/4, XFS, Btrfs, ZFS, F2FS, bcachefs, tmpfs, ramfs and overlayfs on Linux; APFS and HFS+ on macOS; NTFS and ReFS on a fixed drive on Windows), and the server id is not a clone → the previous process is gone. Re-take at once, at a fresh epoch through a claim;
  - the lock is held → another process of this server data holds the lease: follow.
- Without the shortcut, a restarted server would wait about a minute.

**Takeover** (`fin project takeover [--confirm]`)
- Like an automatic acquisition, plus:
  - with `--confirm` on a sync folder: takes the lease whatever its state (the lease is advisory there, and the user has said the other machine stopped);
  - with `--confirm` anywhere: replaces a damaged `lease.json`.
- A live holder on local or network storage is **never** taken over, even with `--confirm`.
- Refusals are `LEASE_TAKEOVER_REFUSED` (exit 4) with `details.reason`: `CONFIRM_REQUIRED`, `HELD`, `STALE_UNCONFIRMED` (it looks stopped; the confirming probe comes ≥ 5 s later), `CONTENDED` or `UNAVAILABLE`.

**Ownership**
- The `Lease` is owned by the open `Store`, with its own heartbeat thread (stopped and joined on drop).
- `fin-node` exposes it through `status()` and `takeover(confirm)`. `finisherd` publishes lease changes as `status` events on `project.events` (§3.10).

**Storage kind** (`fin-store::storage_kind`: `local | network | sync(provider)`)
- The kind picks the lease parameters, the poll cadence, the export's churn delay, the ordering rule's directory-cache wait (with the OS), the compaction grace and whether the epoch rule may decide.
- **Principle:** taking a sync folder for local or network storage is the dangerous mistake, because it enables automatic takeover on what is only an advisory lease. The reverse is merely annoying. Ambiguous evidence therefore **leans to sync**, and a failed probe yields `network` unless a sync signal was already seen.
- **macOS:**
  - sync: `<x>/Library/Mobile Documents/**` (iCloud), `<x>/Library/CloudStorage/<Provider>-*/**`, iCloud "Desktop & Documents" (reached through links into `com~apple~CloudDocs`), well-known sync folder names in the home folder, a dataless (`SF_DATALESS`) folder or ancestor, and any FUSE file system;
  - network: `smbfs`, `nfs`, `afpfs`, `webdav`.
- **Windows** (sync evidence first):
  - sync: a registered Cloud Files sync root (`CfGetSyncRootInfoByPath`; the binary imports `cldapi.dll`, Windows 10 1709+), the `OneDrive*` environment roots, Dropbox `info.json` roots, Google Drive (volume label, `DriveFS`, `%LOCALAPPDATA%\Google\DriveFS`), well-known sync folders in the profile, and recall or pin attributes on the folder or an ancestor;
  - network: UNC paths, and remote or undeterminable drives.
- **Linux:**
  - sync: markers on the path or an ancestor. These are Syncthing (`.stfolder`, `.stignore`, `.stversions`), Dropbox (a regular `.dropbox` file; the `~/.dropbox` config directory doesn't count), Nextcloud/ownCloud (`.sync_<hex>.db`, `._sync_<hex>.db`, `.csync_journal.db`), Resilio (`.sync/`) and MEGA (`.megaignore`, `.debris`). Any FUSE mount except those below is sync too, including one whose subtype can't be read;
  - network: SMB/CIFS, NFS, 9p, Ceph, AFS and `fuse.sshfs`;
  - local: `fuseblk` (ntfs-3g, exfat);
  - WSL: `/mnt/<drive>/Users/<user>/…` gets the Windows home-folder rules.
- **Override:** `storageOverride` in `finisher.project.json` is honoured. An override that weakens a detected sync folder to local or network warns (`STORAGE_OVERRIDE`), and one that doesn't parse is ignored with that warning.

**On sync folders**
- The lease is advisory: another holder makes the project read-only with "take over?" (`--confirm`).
- Two offline writers can legitimately hold different epochs, so the epoch rule **never decides** there. Any two lines are a `SyncFolder` fork (`CHAIN_FORK`), the local unexported tail included. Nothing is set aside, and the user chooses. A rewind (§3.6.2 step 6a) can therefore never happen on a sync folder.
- A conflict copy of any Finisher file, `lease.json` included, makes the project read-only (`CONFLICT_COPY`) as a suspected split brain.

### 3.6 Opening, importing and following

**Opening** (`Store::open`, in order):
1. Read `finisher.project.json` (15 s retry; 12 s on network storage when nothing shows yet, §3.4).
2. No DB: rebuild it from the folder (§3.8, "Missing"), using the file's `genesis`. Without a genesis → `PROJECT_DB_MISSING`.
3. Take the shared `db.lock`. Check the DB read-only: refuse `NOT_A_PROJECT_DB` or `DB_TOO_NEW`, and rebuild a damaged DB (§3.8). Then check `PROJECT_DB_MISMATCH` and project copies (`PROJECT_COPY`, `PROJECT_ORIGINAL_UNREACHABLE`, §3.7).
4. Migrate. Record `lastOpenedBy`, `projectDir` and the server id in `serverIds`.
5. Detect the storage kind (`storageOverride` wins, §3.5).
6. Try once to acquire the lease. A follower keeps its contender for `tick`.
7. Run an import pass. The read-only state is therefore in place before `open` returns, so no commit can slip in after a restart.
8. If this server holds the lease and the file has no genesis (a sub-project 2 project), backfill it (§3.4).

Open writes no segments; the first `tick` exports, and `close` flushes. Open's folder I/O is a lease probe, a record read, perhaps a claim, one scan and one lease check: tens of milliseconds locally. A rebuild inside open takes longer, so `fin-node` runs open on the worker thread (Decision 7).

**One import pass** (`Store::import`; module docs of `import`, `scan`, `chain`):
1. **Scan** `.finisher/` (§3.6.1).
2. **Hydrate** cloud placeholders under `segments/` by reading each whole file, at most 16 MiB per pass, round-robin from where the last pass stopped, so one slow placeholder cannot starve the others. The *next* scan sees the real file. Legacy `.<name>.icloud` stubs are skipped, because reading doesn't hydrate them.
3. **Classify:** verified segments go per branch. Newer and unsupported-hash files set `NEWER_FORMAT`. Foreign files are listed; any that decode as segments are imported to quarantine (`CONFLICT_COPY`, deduplicated by `txHash`).
4. **Genesis check:** segments whose header `genesisHash` differs from the DB's are not imported. The branch reports `GENESIS_MISMATCH {expected, files}` as its import error (read-only `IMPORT_ERROR`). They are not our history, so they are not quarantined.
5. **Resolve** each branch the DB knows (§3.6.2), all in one `BEGIN IMMEDIATE` transaction for the whole pass, so a pass lands completely or not at all. **4B:** branches are resolved parents first, and a `branch` record applied in the pass makes its child known in the same pass (a worklist, §4.1 "Import"); a branch directory whose `branch` record the DB does not know yet waits (`unknownBranches`). When a branch's segment set, `head`, tail, lease and kind are what the last resolution saw, its verdict is reused without running again (`resolve` is pure). The lease files' highest epoch, needed for rewind rule (c), is read **before** `BEGIN`, never under the DB lock.
6. **Act** (`import::apply`), in order:
   1. mark exported;
   2. set a diverged tail aside;
   3. apply new records through `apply_committed` (upcast first). The rows are stored with their exact bytes, already exported, at the segment's epoch. A record that can't be read stops the branch there (`NEWER_FORMAT`, or the branch's import error);
   4. add `segments` rows (`imported`) for every segment whose last record is now in the DB, and for every present pack ending on the DB line unless the resolution sets any of it aside;
   5. quarantine.
7. **Raise id counters** (§3.3), quarantined ids included (4B).

**Following (Decision 3).** `Store::tick` runs the folder work once per poll, at the storage kind's cadence: **local 2 s, network 1 s, sync 10 s**. No file-system events are used. This deviates from the earlier text (events plus a 30 s poll locally, events plus 10 s on sync). Listing `.finisher/segments/<branch>` is cheap, polling needs no watcher dependency, and events can be added later without a format change. A poll:
1. import (queuing `Rewound` then `Applied` events);
2. export, if holding and writable;
3. the fencing re-check while holding (a lost lease is dropped → `LEASE_LOST`);
4. one acquisition attempt when not holding (right after a loss too), always at a fresh epoch. After an acquisition the ordering rule applies, and an import runs at once when it needs no wait;
5. the daily backup (§3.8);
6. compaction (§3.9).

**Listing refresh (macOS network shares).** macOS smbfs serves a directory listing from a cache for 30–60 s (`dir_cache_min`/`dir_cache_max`, `man nsmb.conf`), to every process on the machine; a change made through this machine drops that directory's cache at once, a `stat` does not. So on a macOS network share each import scan first creates and removes a temp-named file (`.~fin-<ulid>.tmp`) in `segments/` and in each `segments/<branch>/`. It is best effort: a failure (a read-only user) is ignored and the listing may be a minute old. It only speeds following; the ordering rule never relies on it.

The plan put the retry before the re-check; re-checking first lets the same poll retry a loss it found. Calls between polls only export and report status changes. Every call ends with a `Status` event if the status changed (lease ages ignored).

**The ordering rule** (after **every** acquisition: at open, automatic re-acquisition, takeover)
- Commits and exports are blocked, read-only reason **`CATCHING_UP`**, until an import pass that started at the new epoch has resolved every branch with nothing `waiting` and no import error.
- Without it, a stale local tail, or a new commit on top of it, would be written at the new epoch. It would beat records another server exported meanwhile, laundering them: export labels by commit epoch, but a new commit raises the tail's epoch.
- **Directory-cache wait.** The pass that lifts the block must start at least **10 s** (monotonic; the Windows SMB directory cache), or **65 s** on a network share mounted by macOS (smbfs `dir_cache_max` 60 s plus a margin; measured: another host's new segments stayed out of a Mac's listings for 30 s), after the acquisition when any of these holds:
  - the replaced `lease.json` named another server, or was damaged;
  - a claim above the highest epoch in our DB belongs to another server;
  - `lease.json` was missing while the folder holds a segment by another writer or above our epochs.

  Passes before then still import. Limit: a user's `/etc/nsmb.conf` can raise `dir_cache_max` above 60 s; that is not detected, and the 65 s no longer covers it.
- **Sync gate.** On a sync folder the block also stays while any file under `segments/` has not arrived (`SYNC_ARRIVING`). The wait bounds only a directory cache, not propagation: after a confirmed takeover on a sync folder, the other machine's last edits may still be on their way, and nothing bounds what hasn't started arriving.

#### 3.6.1 Scan classification

`fin-store::scan` lists `.finisher/` and classifies each entry (`Entry`):

| Entry | Meaning |
|---|---|
| `Segment`, `Snapshot` | verified (CRC and BLAKE3 trailer), with name, directory and header agreeing |
| `Manifest` | `compaction/<ulid>.json`, listed by name (compaction reads it) |
| `NotArrived {Placeholder, Incomplete, Temp, Locked, …}` | a file another client holds locked or pending deletion (Windows error 5/32, Linux `EBUSY`; seen on SMB during compaction), a cloud placeholder (Windows `RECALL_ON_DATA_ACCESS`/`RECALL_ON_OPEN`/`OFFLINE`; macOS `SF_DATALESS` or a `.<name>.icloud` stub), a short file or missing trailer, our temp files, sync temp files (`.syncthing.*.tmp`, `~syncthing~*.tmp`). Retried on the next poll, never reported as corruption. A placeholder is never read by the scan |
| `Damaged {reason, len, modified}` | a canonically named file that decodes as corrupt, or exceeds its size cap |
| `Newer` | `minReaderVersion` above ours (`NEWER_FORMAT`) |
| `UnsupportedHash` | a newer format with an unknown hash prefix |
| `Unreadable` | a non-transient read error (permissions, I/O, a cloud file that cannot be fetched); the rest of the scan is unaffected |
| `Foreign {conflict_copy, content}` | anything else. `conflict_copy` is the provider when a known pattern matches, or `Unknown` for a Finisher file under a name not its own; `content` is the decoded file, if it is one, for quarantine |

- **Layout read:** `.finisher/*` regular files (our own lease files are skipped, so a copy of `lease.json` is foreign); `segments/<branch>/*`, `snapshots/<branch>/*` with `<branch>` = `main` or `br<digits>`; `compaction/*`.
  - A directory with any other name is **one** `Foreign` entry and is not entered. A sync client's copy of a branch directory gets `conflict_copy`, so it is `CONFLICT_COPY`.
  - Files directly in `segments/` or `snapshots/` are foreign.
  - Other directories are not scanned.
- **Never followed:** a path that is not a regular file (symlink, FIFO, socket, device) is `Foreign` and never opened.
- **Litter skipped:** `.DS_Store`, AppleDouble `._*`, `desktop.ini`, `Thumbs.db`, `.dropbox`, `.dropbox.attr`, `.nfs*`, `.fuse_hidden*`, `.smbdelete*`.
- **Size caps** (from metadata, before reading): a segment above 2×4 MiB + 64 KiB or a snapshot above 256 MiB is `Damaged("too large")`; a foreign file above the segment cap is listed without decoding.
- **Cache:** only verified segments and snapshots are cached, keyed by path and `(len, mtime)`. Names are content-addressed and files write-once, so a hit needs no read. `NotArrived`, `Damaged`, `Foreign` and `Unreadable` are never cached, because sync roots hydrate out of order.
- **Conflict-copy names** (`conflict_names`): a name is a conflict copy of provider P only if removing P's marker leaves one of our canonical names. Litter that merely looks like a marker (`notes 2`) therefore doesn't count. Matching is case-insensitive.
  - Dropbox: a parenthesised group containing `conflicted copy`, `Case Conflict` or `Selective Sync Conflict`;
  - Syncthing: `.sync-conflict-<date>-<time>-<id>`;
  - iCloud: ` <n>` before the extension;
  - OneDrive: `-<hostname>` (every `-` split of the stem is tried);
  - Google Drive (unverified): `(<n>)` or `[Conflict]`.

  A conflict copy of a `lease.probe-*` file is listed only, because each server writes only its own.
- **Stuck files:** on local and network roots every write is atomic, so a file seen incomplete for more than 5 polls can't be in transit: status warns `INCOMPLETE_FILES` (a crashed foreign writer, or damage). A `Locked` file that has looked the same (length, mtime) for 60 s, on any kind of folder, is `LOCKED_FILES` in status and unreadable in verify (`SEGMENT_LOCKED` in `unsettled` before that); a segment deleted by compaction while another client briefly holds it stays quiet.

#### 3.6.2 Chain resolution

`fin-store::chain` is pure (no I/O, no clock). Its result depends only on the *set* of segments and on what the DB holds (`Known`). Code comments cite this section by the step numbers of the earlier text, which are kept as labels: steps 4–5 (chain building, packs and duplicates), 6 (epoch rule), 6a (rewind), 7 (diverged local tail), 8 (quarantine is never silent).

**Steps 4–5 — model.** History is a tree of **records** keyed by `txHash` and linked by `parentHash`; segments are only containers. Steps 4–5 of the earlier text fall out of content addressing:
- a pack and the segments it replaces hold the same records, so they are one line, not two successors;
- a re-exported duplicate adds nothing;
- a follower whose DB stops inside a pack resumes inside it;
- links can't form cycles.

**Segment-level checks** (`chain::graph`):
- A segment is *malformed* when its records don't parse, its seqs don't run `firstSeq..=lastSeq`, its `parentHash`es don't chain, or a `txId` repeats.
- A segment's first record must continue its predecessor, else the segment is malformed:
  - `prevSegmentHash` `null`: it starts the branch (on `main` the first record has no parent);
  - `prevSegmentHash` a present segment: its parent is that segment's last record, seq + 1;
  - `prevSegmentHash` only in an honoured pack's `replaces`: its parent is exactly that entry's `lastTxHash` (**the exact stand-in rule**: a pack stands in for a deleted segment only as that segment's last record);
  - anything else: a gap. The segment waits, and its predecessor's hash is reported in `waiting`.
- A pack is honoured only if it holds the `lastTxHash` of every `replaces` entry and every record of each present segment it lists, whose last record must be that `lastTxHash`.
- A record's epoch is the highest epoch of the segments holding it.

**`Known`** (what the DB holds per branch; `import::known`): `{head, last_hash, known_tx, tail, tail_epoch, holds_lease, kind}`.
- `head`: the `txHash` of the highest exported row;
- `last_hash`: its segment;
- `known_tx`: every `txId → txHash` in the DB, on any branch, the tail included;
- `tail`: the unexported rows in seq order, which must follow every exported row;
- `tail_epoch`: the highest commit epoch of the tail.

This replaces "build each branch's chain from the DB's last known segment hash". Keying the DB line on records instead of a segment hash is what makes three cases work: a holder's old tail after it re-acquires, a live holder on a stale line, and re-exports whose segment boundaries differ from the first export.

**Step 6 — epoch rule** (local and network roots)
- Where two or more lines continue from one record (or from the branch start), **each line's epoch is the highest epoch anywhere on it**, and the unique highest line wins ("the latest holder's line wins"). The losers' records are quarantined (`STALE_EPOCH`) and never applied. A tie is a fork (`EqualEpoch`).
- The DB's unexported tail is a line too, **at its commit epoch** (`tail_epoch`). It competes **only while this server holds the lease** (`holds_lease`).
- This corrects the earlier step 6 wording ("before this server has exported anything at its epoch, its tail counts as that higher-epoch successor"). The tail counts at the epoch its records were committed at, not at the current lease epoch. That is what makes a lower-epoch segment landing after a takeover stale, and an old tail of a lost lease lose.

**The walk** (`chain::walk`):
1. Follow `head` to the branch start. If the folder doesn't hold `head`, wait and decide nothing.
2. Extend with tail records the folder already holds in order (a crash after rename): these are `mark_exported`.
3. Down the DB line, each record with siblings is a competition:
   - the DB line wins → the siblings are quarantined;
   - a sibling wins at or above `head` → **`AppliedLineLost`** fork (step 6a may rewind);
   - a sibling wins inside the exported part of the tail → the tail diverged;
   - a tie, or any two lines on sync storage → fork.
4. At the end, the unexported tail competes the same way. Without the lease it doesn't compete: once the folder has moved on from where the tail starts, the tail is set aside (step 7), or on sync storage the branch forks with the tail as one side.
5. New records follow the unique child or the winner until a leaf, a fork, or a record whose `txId` the DB holds with other bytes (**`TxIdReused`** fork).

**Fork kinds:** `EqualEpoch`, `SyncFolder`, `AppliedLineLost` and `TxIdReused`, each with its sides. Every fork is read-only `CHAIN_FORK` unless step 6a rewinds an `AppliedLineLost`.

**Nothing is dropped.** Every record of a usable segment ends up in exactly one of `apply`, `mark_exported`, `quarantine`, `pending` (beyond a fork, behind a gap; shown in status), or the DB's own line. Records descending from a malformed file are quarantined as `MALFORMED`.

**Verdicts are per poll.** Every verdict (quarantine, diverged tail, fork, waiting, newer) is recomputed on every pass and never persisted as final. A lying pack, for instance, is honoured until the segment it falsely lists arrives.

**Step 6a — rewinding a lost applied line** (`import::rewind`)
- **Why:** a holder exports with fence → `write_once`, and nothing makes the rename conditional on still holding the lease. A holder paused between the two can land a segment arbitrarily late, after another server took over and wrote the same seqs at a higher epoch. A follower that applied the late segment before the winner's arrived would be `CHAIN_FORK` forever, while a follower that saw both at once follows the winner: the two would never agree. Fencing makes this rare, but it is a legitimate path, not only a bug. Rewinding makes every follower converge whatever the arrival order. The ordering rule is still required (laundering), but it is not enough on its own.
- **When** (all must hold, else `CHAIN_FORK`):
  - (a) the storage isn't a sync folder (there `AppliedLineLost` can't occur; checked anyway);
  - (b) this server doesn't hold the lease, **or** holds it at epoch `E` and every lost row is below `E`. The relaxation for a holder: a server that applied a late line and then acquired the lease, before the winner's files reached it, rewinds rather than freezing the project while heartbeating;
  - (c) the winner's epoch is backed by the lease state: `lease.json`'s epoch or a claim is at or above it. A higher epoch nobody claimed looks like a hand copy or a bug;
  - (d) no branch **without an index row** (a root made by hand) is based inside the lost range: it is based where `meta` `branchBase.<b>` says, else inside. In 4A this refused any branch based inside; **4B replaces the refusal with a cascade** (plan Decision 7): a branch with an index row whose `branch` record is among the lost rows is rewound whole with them, recursively (below), and one whose `branch` record survives forks below the lost range and is untouched. *Why:* followers must converge whatever the arrival order; refusing would bring back the permanent `CHAIN_FORK` that this step removes. For (b) the cascaded children's rows count as lost rows, so a holder whose own edits at its epoch sit on a child that would cascade stays `CHAIN_FORK`.
- **What:**
  1. Lost exported rows go to quarantine as **`SUPERSEDED`**: origin `local` when this server wrote them (judged by its `segments` rows or by `writerId` ∈ `serverIds`; packs never count), else `folder`. Lost unexported rows go as `DIVERGED_TAIL`.
  2. The rows, the DB snapshots at or above the fork seq, and the `segments` rows whose last record is lost are deleted. This includes a pack spanning the fork; the pack *file* stays, and the winner resolves through its `replaces`.
  3. The head is rebuilt from the latest snapshot below the fork. Id counters are not lowered.
  4. The branch's generation is bumped and the rewind recorded.
  5. The branch is resolved once more in the same transaction.
  6. **Cascade (4B, `import::cascade`):** each descendant whose `branch` record was lost (found in the index before the rows go: children with `parent` = the branch and `created_seq` ≥ the fork seq, then theirs, parents first) is rewound whole, from rev 0, fork snapshot included: exported rows → `SUPERSEDED`, unexported → `DIVERGED_TAIL`, each with the origin rule of step 1 (detail `seq N (epoch e): its branch record <hash12> was set aside`); its snapshots, `segments` rows and per-branch `meta` are deleted; its generation is bumped with the rewind recorded from rev 0 (the id may come back as another branch forking elsewhere, so a caller holding the old generation is `HISTORY_REWOUND` whatever its base); a `Rewound` event is queued per branch, so finisherd ends its subscribers. The child's folder directory then classifies as set aside with its `branch` record (§4.1 "Import"); a follower that never applied the line gets `STALE_EPOCH` for the same records, so the quarantine set by hash is the same.
  7. **Derived tables:** every rewind, diverged tail and cascade runs `derived::recompute` in the same transaction (stale `record_links` rows dropped, the index and `merged` rebuilt from the log; §4.1).

  At most one rewind per branch per pass.
**Step 7 — diverged local tail without the lease.** If this server has unexported records, doesn't hold the lease, and the folder has moved on from where they start, they go to quarantine (`DIVERGED_TAIL`, origin `local`) and the head is rebuilt; the project stays usable read-only. This happens before applying, so a retried tx that the folder holds under the same `txId` can take it. It uses the same history machinery as a rewind: it bumps the generation and emits `Rewound`. In 4B it cascades like a rewind: a tail holding a `branch` record takes that child along (`DIVERGED_TAIL`).

**Step 8 — quarantine** (`import::quarantine`)
- One row per `txHash`, so a stale line re-reported every poll, or a diverged tail overlapping a conflict copy, is one row.
- Reasons: `STALE_EPOCH`, `MALFORMED`, `DIVERGED_TAIL`, `SUPERSEDED`, `CONFLICT_COPY`, and from rebuilds `SALVAGED` and `MISSING_FROM_FOLDER` (§3.8). **4B adds** `ID_COLLISION` (the records of a branch whose `branch` record lost its id to another server's, §4.1) and `IMPORTED` (records of `.fseg` files moved out of the folder and read back with `fin project recover --import`, origin `foreign`, §4.5). Origins: `folder`, `local`, `foreign`, `salvage`.
- `current` says whether this pass's verdicts still set the row aside; status counts only current rows. A `DIVERGED_TAIL` (or salvaged) record exists nowhere else, so it stays current until its record is in `txs` again. A `SUPERSEDED` row's `current` follows the folder's verdicts, so two followers have the same current set.
- `DIVERGED_TAIL`, `SUPERSEDED`, `SALVAGED`, `ID_COLLISION` and `IMPORTED` keep their reason, origin and detail once set. `ID_COLLISION` is current while the pass's verdicts set its record aside, like `SUPERSEDED` (the id's holder can itself be set aside later, and then the loser's branch comes back); `IMPORTED` is always current, like `DIVERGED_TAIL`. Rows are never deleted except by a rebuild, so `quarantine.id` only grows (counters and the recovered marks use it as a watermark). Bytes are kept forever, and import never deletes rows. A record the DB holds is never quarantined.
- **Never silent:** status warns `QUARANTINE` with the count per reason and origin, naming `fin project recover`. 4B offers quarantined records as `recovered-<server8>-<time>` branches (§4.5); a row stood for by a recovered record, or dismissed on this server, is no longer counted.

#### 3.6.3 Generations and rewound history

- Each branch has a **generation**, bumped by every rewind or diverged tail (`meta` `generation.<b>`; the last 32 rewinds in `rewinds.<b>`).
- A DB rebuilt from the folder starts again at 0, so every DB also has a random **incarnation** (`meta` `incarnation`). It is minted at creation, by a rebuild, in a backup copy, and at the first open of a DB made before it existed.
- Callers hold a `Generation {incarnation, n}` (`ProjectStatus.incarnation`, `snapshot.get`, `opsSinceAt`).
- `Store::commit_at_generation(tx, Some(gen))` refuses with **`HISTORY_REWOUND`** (exit 4):
  - another incarnation → `details.reason` `REBUILT`, `rev` 0, whatever the base;
  - a stale `n`, when a rewind since then reaches `baseRev` → `{branch, rev, generation, expected, baseRev}`, reason `REWOUND`. A `baseRev` below every such rewind rebases as usual;
  - an `n` above the current one, or rewinds no longer in the history → refused.
- A superseded tx is in quarantine, not `txs`, so its `txId` is not deduplicated: a retry without a generation is committed again on top of the winner. Callers that must not re-apply a lost edit pass the generation or ask `tx.get` first.
- **`tx.get`** (`Store::tx_status`) answers with the `CommittedTx`, `{setAside: {txId, branch, reason, origin}}` (from quarantine), or `null`.
- **Warnings:** `REWOUND` (count, writers, epochs, winner epoch; louder when own records were rewound) lasts 24 h or until acknowledged. When the lost records span more than one segment or more than 30 s of commits, it is **`SPLIT_BRAIN_SUSPECTED`** instead (two writers at once, not one late rename), shown until acknowledged. `Store::acknowledge_rewind(branch, generation)` / `fin project acknowledge --rewind <n>` clears them.

### 3.7 Conflicts and read-only states

**Read-only reasons.** The project is read-only with one reason, shown in `fin project status` (`readOnly`) and as `details.reason` on write errors (`PROJECT_READ_ONLY`, exit 4). In **precedence order**, highest first:

| Reason | Trigger | Recovery (`fix`) |
|---|---|---|
| `CHAIN_FORK` | a fork the import cannot settle (§3.6.2) | `fin project recover`: the manual procedure (stop every server, move one history's segment files out of the folder), then, once writable, `fin project recover --import <dir> --create all` makes that side a `recovered-*` branch (4B, Decision 29) |
| `CONFLICT_COPY` | a foreign file matching a conflict-copy pattern, a Finisher file under a name not its own, or a conflicted branch directory (any storage kind) | the same procedure (move the copy out, `--import` it); the copy's records go to quarantine |
| `NEWER_FORMAT` | a segment, snapshot or project file with `minReaderVersion` above ours, or an unknown record kind | update Finisher |
| `IMPORT_ERROR` | the last import could not apply a record (`REPLAY_FAILED`, `GENESIS_MISMATCH`, …) | per the error; `fin project rebuild` when the DB is at fault |
| `LEASE_LOST` | self-fenced, or a failed fencing check | re-acquired automatically when possible; else takeover |
| `LEASE_HELD` | another live holder | wait, or `fin project takeover` (§3.5 rules) |
| `CATCHING_UP` | the ordering rule after an acquisition (§3.6) | wait (seconds; on a sync folder until files arrive) |

The first four come from the folder's content and need a person or an update, whoever holds the lease. The last three are the lease's and resolve by waiting or a takeover. Commits answer a retry of an already committed `txId` before the read-only check, so a follower answers retries.

**Status** (`Store::status` → `fin project status`, `project.status`):
- **Fields:** `projectId`, `storage`, `storageWarning`, `lease` (tagged by `state`: `held {epoch}` / `heldElsewhere {holder, epoch, ageServerMs}` / `stale {holder, epoch}` / `lost {reason}`), `leaseCheck`, `readOnly {reason, fix}`, `branches [{branch, name, parent, forkSeq, state, head, exportedThrough, generation}]` (4B added `name`, `parent`, `forkSeq`, `state`; `main` first, then by numeric id), `lastExportMs`, `exportError`, `importError`, `quarantine {count, origins, reasons}`, `foreign`, `waiting`, `pending`, `unknownBranches` (4B: branch directories whose `branch` record is not known yet; never read-only), `warnings [{code, message}]`, `incarnation`, and `compaction` (§3.9). `fin-node` adds `busy {operation, sinceMs}` (§3.10).
- **Warnings:** `STORAGE_OVERRIDE`, `CLOCK_IGNORES_SLEEP`, `LEASE_CHECK`, `QUARANTINE`, `REWOUND`, `SPLIT_BRAIN_SUSPECTED`, `FOREIGN_FILES`, `INCOMPLETE_FILES`, `LOCKED_FILES`, `SERVER_ID_RENEWED`, `SYNC_ARRIVING`, `COMPACTION_ABANDONED`, `PROJECT_FILE_CONFLICT`, `DB_REBUILT`, `SALVAGE_INCOMPLETE`, `MISSING_FROM_FOLDER`, and in 4B `BRANCH_RECORDS_IGNORED` (inert `branch`/`branchSet` records, §4.1). Branch changes reach clients as `status` events; 4B adds no event type.

Other foreign files are listed (`FOREIGN_FILES`) and otherwise ignored. The conflict-copy patterns are in §3.6.1.

**Project copies (Decision 6)** (`fin-store::copies`)
- A copy of a project folder must never share the original's DB. Opening a folder whose project id this server data knows from a *different, existing* folder that still holds the project → **`PROJECT_COPY`** (exit 3).
- Two spellings of one folder (a symlink, a second mount) are the same folder when they canonicalise alike or are one directory by identity: device + inode, or on Windows the volume serial + `FileIdInfo` (ReFS) or file index. Files are not compared, so a hard-linked copy (`cp -al`, rsnapshot) is still a copy.
- The recorded folder **moved** (and `projectDir` is updated) only if both:
  - its absence can be trusted: the nearest existing ancestor isn't a mount root, a mount base (`/Volumes`, `/mnt`, `/media`, `/run/media`, `/net`, `/Network`) or an empty mount point right below one, and can be listed. A renamed parent folder is a move;
  - the folder being opened holds, per branch, the newest segment the DB knows from the folder (original or pack), so it isn't an older copy.
- Otherwise **`PROJECT_ORIGINAL_UNREACHABLE`** (exit 3), unless the caller says it moved: `fin project open --relocate`, `project.open {relocate: true}`. That is a person's decision, never automatic.
- `projectDir` is stored losslessly for any OS path (`{"utf8"}`, `{"unix": hex}`, `{"wide": hex}`).
- Known limit: a folder on a share that is unmounted while its copy is opened counts as moved. Once the share is back, opening the original is refused as the copy.

### 3.8 Integrity, rebuild, backups

**Rebuild** (`fin-store::rebuild`, under the **exclusive** `db.lock`)
- **When:**
  - **Missing:** no DB, and the project file carries a genesis;
  - **Corrupt** (`rebuild::damaged`): a bad header, an unreadable schema or `meta` under our `application_id`, a foreign `application_id` whose `meta.projectId` is ours (only the header is damaged), or a failing `quick_check`. A foreign or newer DB is never rebuilt over;
  - **Manual:** `fin project rebuild` (`Store::rebuild`, or `rebuild_project` with no server running).
- **Locking:** a manual rebuild while another store holds the DB is `DB_IN_USE` (exit 5). Open's automatic rebuild waits ≤ 30 s for a racing open, then uses that open's DB.
- **Steps:**
  1. Re-check under the lock (another open may have rebuilt meanwhile).
  2. **Rev 0:** the old DB's rev-0 snapshot when it reads, else `new_project(genesis)`; neither → `DB_CORRUPT` (exit 3). Any folder segment whose `genesisHash` differs from that rev 0 → `GENESIS_MISMATCH` (exit 3), and nothing is built. A file genesis that doesn't match a readable old rev 0 is reported.
  3. Build `project.fdb.rebuild-<ulid>`: fresh incarnation, rev 0, then one import pass of the whole folder through a store that holds no lease.
     - **Seeding:** on a branch at rev 0, the highest folder snapshot whose `(seq, txHash)` is on the resolved chain, whose `genesisHash` matches, and whose records up to it all read as this branch's is the state after that seq. Those records are stored unreplayed with diff `null`, and replay continues from the snapshot.
     - Their diffs are filled lazily (`diffs::fill`, `meta` `diffsPending.<b>`) by one replay the first time a read needs them. The DB then equals a full replay (tested).
     - Seeding saves time only by deferring diffs, because in 4A the history is always in the folder. A lying on-chain seed is trusted, and verify's `replay_matches_live` catches it.
     - **4B:** a branch's start is its fork point (`seq == forkSeq`, records from `forkSeq + 1`), so a child forked at 0 seeds like `main` and never from a genesis; a seed never spans a `branch` or `branchSet` record (those must be absorbed and replayed), so the highest seed below the first one on the chain is used. A rebuild restores every branch, the index, `merged`, checkpoints and the recovered marks from the folder alone (they are all derived from the log); salvage keeps every branch's rows (`MISSING_FROM_FOLDER` on any branch). `REBUILD_WOULD_LOSE.exportedThrough` still reports `main`'s.
  4. **Salvage** (`rebuild::salvage`) reads every old `txs` row the new DB lacks:
     - unexported → `SALVAGED`; exported but no longer yielded by the folder → `MISSING_FROM_FOLDER`. Both have origin `salvage` and stay current;
     - old quarantine rows come along (origin `salvage`, reason kept); id counters are MAX-merged; `projectDir`, `serverIds`, `createKey` and `salvageIncomplete` carry over;
     - the sources are the old DB, every kept `corrupt-*`/`replaced-*` copy not listed in its `meta` `salvagedFrom`, and stale `rebuild-*` builds;
     - tables are read by rowid ranges, skipping past bad pages, so one bad page loses only its rows. Anything unread is reported (`salvageComplete: false`, warning **`SALVAGE_INCOMPLETE`** naming the kept copy, persisted in `meta`).
  5. A **manual** rebuild that would drop exported records the folder no longer holds is refused: **`REBUILD_WOULD_LOSE`** `{missing, exportedThrough, rebuiltThrough}` (exit 3), nothing swapped. An automatic (corrupt) rebuild proceeds and warns `MISSING_FROM_FOLDER`.
  6. Verify the new DB with the checks that need no replay.
  7. **Swap.** The old DB is renamed with its `-wal`/`-shm`, so SQLite never replays that WAL onto the new file and the kept copy still opens:
     - a damaged DB → `project.fdb.corrupt-<UTC ts>`;
     - a healthy DB replaced by a manual rebuild → `project.fdb.replaced-<UTC ts>`;
     - with no old DB, an orphan `-wal` → `project.fdb.orphan-<ts>-wal`.

     The new file is linked in without replacing anything; if that fails, the old one is moved back. The newest 3 of each kept kind are kept. A copy with incomplete salvage is never pruned until `fin project acknowledge --salvage <copy>`.
- **Results:** status warns `DB_REBUILT`, and `StoreEvent::Rebuilt {incarnation}` is emitted. A crash at any step leaves the old DB, the new one, or neither with the old one kept as `corrupt-*`; the next open rebuilds again and salvages that copy. Other stores on the same DB keep the old file (documented; `db.lock` prevents this for stores of one server data).

**Verify** (`fin project verify`, `fin-store::verify`). It changes nothing, and runs inside one read transaction. The report is `{ok, checks[{name, ok, details}]}`; a failed report is `VERIFY_FAILED {report, failed}` (exit 3), and a check whose own reads fail reports `ok: false` with `details.error`. The checks, in order:
1. `integrity_check`: `PRAGMA integrity_check` is `ok`.
2. `segment_checksums`: a fresh scan decodes every segment and snapshot (CRC, BLAKE3, name and header agreeing).
   - A damaged canonical file is `SEGMENT_CORRUPT` (`details.corrupt`). Off sync folders this is immediate (writes are atomic). On sync folders it needs the same length and mtime for 60 s of this store's monotonic clock (every import and verify observes), or the mtime already ≥ 60 s old at first sighting; until then the file is `unsettled`.
   - Placeholders are not hydrated (`unchecked`, not a failure).
   - A `segments` row whose file isn't listed and that no present pack replaces is `missing`.
3. `chain_continuity`: the DB rows are dense from seq 1, each `parent_hash` and `tx_hash` checks out, each record reads as its row, and exported rows are a prefix. A fresh resolution of the folder must hold every exported row at its seq with the same hash, with no fork and nothing waiting. A folder ahead of the DB is reported, not a failure; rows beyond the chain while files are arriving are `unchecked`.
4. `epoch_order`: along the fresh chain, each record's epoch (the lowest of the present segments holding it, so originals beat packs) never decreases. A pack's label must equal the highest epoch of the present segments it replaces. Unexported commit epochs are non-decreasing and at least the highest exported epoch.
5. `replay_matches_live`: replaying each branch from its first snapshot gives every stored snapshot and the live head.
6. `diffs_match`: stored diffs equal recomputed ones; diffs not yet filled are counted (`notStored`).
7. `genesis`: `new_project(file genesis)` equals the DB's rev 0, and every segment's `genesisHash` matches (`segmentsDisagree`).
8. `record_size`: no stored record exceeds the segment record limit (a migrated record can; §3.4).
9. `branch_index` (4B, `verify/branches.rs`): `index` (the `branches` table row by row, `merged` included, against `derived::compute` over the log), `links` (`record_links` against the rows the log's non-edit records give, `ignored` marks included, and no orphans), `marks` (`quarantine_marks` against the quarantine rows' bytes), `stateless` (an index row with no state), `unindexed` (a branch with rows and no index row; it refuses writes), and `segments` (a branch directory's other segments must name a record that is inert or set aside; one unknown here is `waiting`, a failure unless files are arriving, or `ahead` when a folder chain this verify resolved makes it). `compute` only reads, so verify still writes nothing.

In 4B `chain_continuity` resolves each branch from the segments of its directory that name its index record, at the index's fork point (`Known::for_branch`), starts it at `forkSeq + 1` and requires `forkHash` as its first parent; a hand-made root (rows, no index row) reads as in 4A.

`SEGMENT_CORRUPT` is a verify finding, not an error code. Import treats a damaged file as not usable (and keeps observing it); it never fails on one.

**Backups** (`fin-store::backup`; `VACUUM INTO` a temp file in `<project-data>/backups/`, fsync, rename without replacing; never a raw copy, which would miss the WAL)
- **Pre-migration:** `pre-migrate-<from>-<to>-<UTC ts>Z.fdb`, one per migration run (covering the whole range), the last 3 kept.
- **Daily:** `daily-YYYY-MM-DD.fdb`, by the **local** calendar date (`localtime_r` after one `tzset` on unix, `SystemTimeToTzSpecificLocalTimeEx` with the current dynamic zone on Windows, which applies the zone's rules for the date converted; UTC fallback), at most one per day while open, the last 7 kept.
  - Taken by **every** server, holder or follower: it is a copy of local state, and no folder I/O is involved.
  - Checked once per poll; a failure is retried after 10 min.
  - Names dated after tomorrow sort oldest when pruning.
- Both kinds: only strictly named files are pruned, the new backup is never pruned, and prune errors are ignored. A backup copy gets a fresh incarnation, so a restored backup is a new incarnation. Stale `.~fin-*.tmp` files older than 10 min are removed.
- The folder export is itself the off-machine backup when the project lives on a NAS or sync folder.

### 3.9 Compaction (history is kept)

`fin-store::compact` is the only code that deletes from the project folder, and what it deletes may be another machine's only copy of history, so every step re-checks from a fresh read.

- **Who and when:** only the lease holder; only while `read_only()` is `None` (so not while `CATCHING_UP`, forked, conflicted, newer or failing to import); a full lease check before every write and every delete. Followers never compact. Losing the lease forgets every grace period observed. `tick` checks on every poll (cheap when idle); `fin project compact` forces a run.
- **Trigger:** more than **200** non-pack segments on a branch that no present pack replaces, or `--force`/`fin project compact`. With the 2 s export debounce, an active session reaches that in hours; continuous commits, capped by the churn delay (§3.4) at 12 segments a minute (2 on a sync folder), in about 17 minutes.
- **Phase 1** (per branch; one pending manifest per branch at a time):
  1. **Fresh view:** an uncached scan; a fresh resolution (`Known::empty`) with no fork, nothing waiting or pending, and every file under `segments/<b>` settled; the DB's exported rows exactly the chain's prefix. Otherwise wait 60 s.
  2. **Eligible segments:** non-pack segments wholly on the resolved chain, exported in the DB, not set aside, not held by an abandoned manifest, not named as predecessor by a malformed file (once gone, that file could link through the pack), and linked to a present predecessor. **Never a segment off the resolved chain:** losing segments are what let a late follower reach the same verdict, and 4B's `recovered-*` branches need them.
  3. **Packs:** runs of consecutive eligible segments with **non-decreasing epochs**, cut at 1,000 records / 4 MiB with whole segments, at least 2 segments each. A pack holds the records byte for byte and carries:
     - `"pack": true` and `"replaces": [{"hash", "lastTxHash"}]`;
     - `prevSegmentHash` = its first segment's;
     - **epoch label = the highest epoch it replaces** (its last segment's). Duplicates inside its range at a higher epoch are left alone, so no verdict of the epoch rule can change;
     - `createdAt` = the latest it replaces, so a re-planned pack is the same file and is adopted;
     - our `writerId` and `genesisHash`.

     Packs are never re-packed in sub-project 4.
  4. **Write**, each fenced, write-once and read back:
     - the snapshot at the branch's exported head (or a verified one already there);
     - each pack, then its `segments` row (state `imported`);
     - the manifest `compaction/<ulid>.json`: `{"magic":"finisher-compaction","formatVersion":1,"minReaderVersion":1,"projectId","writerId","epoch","branch","replaced":[names],"packs":[names],"snapshot":name|null,"createdMs","kind"}`. `kind` is the writer's storage kind; `createdMs` is informational.

     Interrupted before the manifest, the packs are duplicates that readers skip by `txHash`.
- **Phase 2** (per manifest, ours or another server's):
  1. **Grace:** 10 min (local/network) or 24 h (sync), the longer of this server's kind and the manifest's (an unparsable kind counts as sync). It is measured on this server's suspend-aware monotonic lease clock from the first poll that, holding the lease, found **all the manifest's packs and its snapshot present** (status `arriving` until then). Never from `createdMs` or a file time.
  2. After the grace: fence at the manifest's epoch, re-read the manifest (it must be unchanged), and take a fresh view. Then check:
     - each pack is present (else wait), not set aside, and all its records are on the chain and exported (ahead of the DB → wait; off the chain → abandon);
     - the snapshot is present, on the chain, and its body equals the DB state;
     - each present replaced segment is not a pack, is in some listed pack's `replaces`, is not set aside, has every record byte-equal in the packs, and has an epoch no higher than its packs' label. Absent replaced files are skipped (an interrupted phase 2).
  3. **Equivalence re-resolve:** resolve the branch again **without** the files to be deleted, both for a fresh reader (`Known::empty`) and for this DB. The verdicts (chain, set-aside records and reasons, fork, waiting, pending, diverged) must equal those with the files; any difference abandons the manifest ("would change what … resolves").
  4. Delete the replaced files (a full lease check before each), `sync_dir`, fence, then delete the manifest.
- **Wait vs abandon:** a file or check that isn't settled waits and retries in 60 s. Anything that contradicts the manifest **abandons** it:
  - nothing more is deleted for it, and it stays in the folder;
  - status warns `COMPACTION_ABANDONED`;
  - the DB remembers it (`meta` `compactionAbandoned`, across restarts) until a forced run re-checks it.

  Abandoning is per server: another holder checks the manifest afresh after its own grace. An invalid manifest (other project, newer format, bad names, overlap) is abandoned at first sight. Abandoned manifests are never deleted automatically (a person removes them; status says so).
- **Readers:** a pack and the segments it replaces are one line (§3.6.2). Every server records a `segments` row for each present pack ending on its DB line, so project-copy checks and export chaining still work after the originals are deleted. Export and import prefer an original over a pack ending at the same seq.
- **Folder scan:** `compaction/` is listed; a conflict copy of a manifest is `CONFLICT_COPY`. The temp sweep covers `compaction/` and `snapshots/*` too.
- **Status:** `compaction {pending [{manifest, branch, writerId, state: arriving|grace|waiting|abandoned|unreadable, reason, graceEndsMs}], lastRunMs, lastError}`. An unreadable manifest blocks phase 1 on every branch.
- **Branches (4B):** a branch's view and the fresh reader of phase 2's equivalence check resolve at its fork point (`Known::for_branch`) from the segments naming the index's `branch` record; packs keep `branchRecord`, runs are cut where it changes, so a pack never spans two branch records, and phase 2 abandons a manifest whose pack or replaced segment names another record than the branch's (the holder changed since phase 1). A pack's version follows its content like any segment's.
- **History is never discarded in sub-project 4.** Squashing archived branches is deferred (§4.5).

### 3.10 Engine, RPC and `fin`

**`fin-node` threading (Decision 7)**
- **Short calls** (`head`, `commit`, `snapshot`, `opsSince`, `opsSinceAt`, `status`, `txGet`, `acknowledgeRewind`, `acknowledgeSalvage`) are napi `AsyncTask`s.
- **4B short calls** (`crates/fin-node/src/history.rs`): `branchCreate`, `branches(all)`, `branchSet`, `resolveBranch(name)`, `undo`, `redo`, `checkpoint`, `merge`, `cherryPick`, `revert`. Each takes the camelCase JSON of the `fin-store` request struct (`deny_unknown_fields`; a request that does not decode is `INVALID_OP` naming the field) and resolves to JSON. `recover` (listing included) is a long call.
- **Long calls** (`create`/`open`, `takeover`, `verify`, `rebuild`, `compact`, `close`) run one at a time, FIFO, on a **per-engine worker thread** and resolve a napi deferred promise. They never occupy the libuv pool.
- A long call moves the store out of the engine's mutex (`Busy {operation, sinceMs}`). Short calls wait up to **250 ms** for it (so the daily backup of a small DB doesn't fail commits), then reject with **`ENGINE_BUSY`** `{operation, sinceMs}` (exit 5, transient).
- `status()` never waits: while busy it returns the cached status plus `busy`.
- There is no cancellation mid-operation. Long calls after close are `ENGINE_CLOSED`.
- **Maintenance thread:** a light `tick` every 500 ms under the mutex. Due daily backups and compactions are queued as long calls on the worker (`backup (auto)`, `compaction (auto)`), so they don't hold the mutex. Still under the mutex today: import passes, so a large follower import delays short calls (follow-up in TASKS).
- A panic in a tick is a fatal `error` event and stops the thread.
- `close` stops and joins maintenance, then flushes the export (`Store::close`). A dropped engine queues a fire-and-forget close on the worker.
- **Events** (`engine.onEvent`; the newest 1,024 are buffered until a callback registers): `applied {tx}`, `status {status}`, `rewound {branch, fromRev, quarantined, own, generation, incarnation}`, `rebuilt {incarnation}`, `lagged`, `error {error, fatal}`. Every import pass (open, takeover and rebuild included) queues `rewound` then `applied`, and the tick appends them before `status`. The JSON types are generated from `fin-store::wire` (`EngineEvent`, `TxSetAside`, `Generation`).

**`finisherd` routing** (`apps/finisherd/src/hubs.ts`)
- Each (project, branch) has an op channel, and each project an events channel. A channel is a sliding PubSub (a slow subscriber loses the oldest items and never blocks a commit) plus a log of **endings** (the last 64).
- **Endings:** a rewind (from its `fromRev`), a rebuild or lag (`fromRev` 1), an eviction (`fromRev` 0). Each bumps the channel's `seq`, and every item carries the `seq` it was published at.
- A subscriber records the `seq` before subscribing. An item with a higher `seq` replays the endings in between, so a marker lost in an overflow is never missed. If the log has been pruned, the subscriber ends with `HISTORY_REWOUND {rev 0, reason TOO_FAR_BEHIND}`.
- How subscribers end:
  - a rewind or rebuild ends a subscriber whose last rev ≥ `fromRev` with `HISTORY_REWOUND`: `{branch, rev: fromRev, generation}` for a rewind; `{rev 0, reason REBUILT, incarnation}` for a rebuild; reason `EVENTS_LAGGED` for lag;
  - an eviction (fatal engine error) ends every stream with `ENGINE_CLOSED {projectId, reason: evicted}`.
- Each ending carries the generation it **produces**. A subscriber whose backlog was already read at that generation or later isn't ended (found by the e2e storage scenario: an open that rebuilt the DB queues `rebuilt` for its first tick, after a subscriber could already attach).
- The hub does not deduplicate. Channels are removed with their last subscriber.
- **Branches (4B):** hubs are keyed by branch **id**. `ops.subscribe`, `snapshot.get` and `project.acknowledgeRewind` resolve a branch reference (id, name or `name#id`) through the engine first (`main` skips the call); an unknown one is `BRANCH_NOT_FOUND` (4A answered an empty stream). Every branch and history write runs under the project's commit lock; afterwards each `CommittedTx` of its result (`record`, `committed`, or the bare record) is published to its own `tx.branch` hub in rev order, so a merge reaches the target's subscribers record by record and a recovery that writes on several branches reaches each. A dry run publishes nothing. Cascaded branches end their subscribers through the existing `rewound` endings, one per branch.

**RPCs** (`@finisher/contracts`; protocol 1, additive except where noted):

| RPC | Payload | Result |
|---|---|---|
| `project.create` | `{dir, name, rate, idemKey?}` | `{projectId, rev}` |
| `project.open` | `{dir, relocate?}` | `{projectId, rev}` |
| `tx.commit` | `{projectId, tx, generation?}` | `CommittedTx` |
| `snapshot.get` | `{projectId, branch}` | `{rev, project, generation?}` (read under one lock) |
| `ops.subscribe` (stream) | `{projectId, branch, fromRev, generation?}` | `CommittedTx` items. `fromRev` beyond the head → `BASE_REV_AHEAD`; a `generation` that no longer matches → `HISTORY_REWOUND {rev 0, reason GENERATION}` at once |
| `project.status` | `{projectId}` | `ProjectStatus` (+ `busy`) |
| `project.takeover` | `{projectId, confirm}` | `ProjectStatus` |
| `project.verify` | `{projectId}` | verify report |
| `project.rebuild` | `{projectId}` | `RebuildReport` |
| `project.compact` | `{projectId}` | `CompactReport {skipped, manifests, packsWritten, replaced, finished, deleted, waiting, abandoned}` |
| `project.recover` | `{projectId, txId?, author?, create?: "all" \| <group>, importDir?, dismiss?, dryRun?}` | `RecoverReport {groups, created, committed, imported, skippedFiles, dismissed, manual, dryRun}` (4B; was `{manual}` in 4A, §4.5) |
| `project.acknowledgeRewind` | `{projectId, branch, generation}` | `{acknowledged}` |
| `project.acknowledgeSalvage` | `{projectId, copy}` | `{acknowledged}` |
| `project.events` (stream) | `{projectId}` | first `{type: status}`, then `status`, `rewound`, `rebuilt` and `error` events |
| `tx.get` | `{projectId, txId}` | `CommittedTx`, `{setAside: {txId, branch, reason, origin}}`, or `null` |
| `branch.create` (4B) | `{projectId, txId, author, name?, from, at?: {seq} \| {checkpoint}, dryRun?, argsHash?}` | `BranchCreated {branch: BranchView, record, dryRun}` |
| `branch.list` (4B) | `{projectId, all?}` | `{branches: BranchView[]}`; `BranchView {id, name, display, parent, forkSeq, head, state, createdBy}` (`main` has `createdBy: ""`) |
| `branch.set` (4B) | `{projectId, txId, author, branch, name?, archived?, dryRun?, argsHash?}` | `CommittedTx` (the `branchSet` record) |
| `history.undo` (4B) | `{projectId, txId, author, branch, to?, dryRun?, argsHash?}` | `HistoryResult {committed, undone, dryRun, noOp?}` |
| `history.redo` (4B) | `{projectId, txId, author, branch, dryRun?, argsHash?}` | `HistoryResult` (`undone`: the undo it reverses) |
| `history.checkpoint` (4B) | `{projectId, txId, author, branch, name, dryRun?, argsHash?}` | `CommittedTx` |
| `history.merge` (4B) | `{projectId, txId, author, from, into, skipConflicting?, dryRun?, argsHash?}` | `MergeReport {mergeId, committed, merged, skipped, excluded, sourceState, dryRun}` |
| `history.cherryPick` (4B) | `{projectId, txId, author, origin, into, dryRun?, argsHash?}` | `HistoryResult` (`noOp: ALREADY_PRESENT` when nothing to do) |
| `history.revert` (4B) | `{projectId, txId, author, mergeId, branch, dryRun?, argsHash?}` | `HistoryResult` (`undone: [mergeId]`; `noOp: ALREADY_REVERTED`) |

- **Decision 4 (deviation from the earlier text, which put `status` events on `ops.subscribe`):** status and history events go on `project.events`, and `ops.subscribe` keeps its item type. A second item type would break existing clients and the protocol.
- **Generation in protocol 1:** `tx.commit.generation` and `ops.subscribe.generation` are **optional**, so protocol-1 clients keep working; `fin` always sends the snapshot's generation. A future protocol 2 should make both required.
- **Still hand-declared:** the verify, rebuild and compact results are `Schema.Unknown`, and so is `ProjectStatus` (generating it is a follow-up). Of the 4B results, `BranchViewSchema` and `ConflictSchema` are Effect schemas checked against the generated JSON Schema (`envelopes.test.ts`, plus a `Same<>` type check); the larger results are `Schema.Unknown` narrowed to the generated TS types.
- **`project.recover` is not additive (4B):** its result changed from `{manual}` to the `RecoverReport` (whose `manual` holds the same procedure text for `CHAIN_FORK`/`CONFLICT_COPY`, else `null`). A protocol-1 client that reads only `manual` still finds it; one that validates the result shape strictly breaks. There are no such clients outside this repository, so protocol 1 was kept; protocol 2 should record it.
- **4B adds no event type:** branch changes reach clients through `status` events (`branches[]` carries `name`, `parent`, `forkSeq`, `state`).

**`fin` commands** (run in the project folder; one JSON document out):
- `fin project status` → `{status}`;
- `fin project takeover [--confirm]` → `{status}`;
- `fin project verify` → `{report}`; a failed report is `VERIFY_FAILED` (exit 3);
- `fin project rebuild` → `{report, offline}`. With no server running (`SERVER_UNAVAILABLE` at connect), it rebuilds in process: `Store::open` + `Store::rebuild` (exports first) + close, or `rebuild_project` when the DB can't open (`DB_ERROR`, `DB_CORRUPT`, `REPLAY_FAILED`). `DB_IN_USE` (exit 5) if a server holds the DB anyway;
- `fin project compact` → `{report}`;
- `fin project recover [--import <dir>] [--create <group|all>] [--dismiss <group>] [--dry-run]` → the `RecoverReport`, with `committed` replaced by `written` (a count) (4B; 4A printed `{manual}`);
- `fin project acknowledge --rewind <n> | --salvage <copy>` → `{acknowledged}`; the branch is the global `--branch` since 4B (the command's own `--branch` is gone), resolved by name in finisherd;
- `fin project open <dir> [--relocate]`.
- **4B branch and history commands** (each writes through the server, `--dry-run` and `--idem-key` on every write; with no server, also for `--dry-run`, they are `SERVER_UNAVAILABLE`, since there is nothing to plan against):
  - `fin branch new <name> [--from <b>[@<seq|checkpoint>]]` → `{branch, rev, txId}`; all digits after `@` is a seq, anything else a checkpoint name;
  - `fin branch ls [--all]` → `{branches}` (archived only with `--all`);
  - `fin branch rename <old> <new>`, `fin branch archive <b> [--restore [--as <new>]]` → `{rev, txId}`;
  - `fin branch default [<b>|--clear]`: shows, sets (stores the id) or clears the default branch; no project write;
  - `fin checkpoint <name>` → `{rev, txId}`;
  - `fin undo [--to <checkpoint>]` → `{undone, rev, txId}` (`--to <digits>` is `USAGE`: seqs are not accepted); `fin redo` → `{redone, rev, txId}`;
  - `fin merge <branch> [--into <b>] [--skip-conflicting]` → `{merge}` (the report without `committed`, plus `rev` and `records`); a conflict exits 4 with `details.conflicts`;
  - `fin cherry-pick <txId> [--into <b>]` → `{rev, txId, noOp?}`; `fin revert <mergeId>` → `{reverted, rev, txId, noOp?}`;
  - `fin try [name]` → `{branch}`; `fin try keep` → `{merge}`; `fin try drop` → `{branch}` (§4.5);
  - `rev`/`txId` are `null` when nothing was written (`noOp`), and `dryRun: true` marks every planned-only answer. `undone` and a merge's `merged`/`skipped`/`excluded` are cut to 20 entries with `<key>Total` (`fin` has no verbose mode; the full lists are in the log).
- **Global `--branch <b>` (4B):** every command's target branch is the flag, else the project's default in `<server-data>/fin-config.json`, else `main`. Writes (`Tx.branch`), `status` and `view` use it. A default that names no branch is `BRANCH_NOT_FOUND` (`details.source` `fin-config.json`, `fix` `fin branch default --clear`), never a silent `main`; a project the server cannot be asked about (`ENGINE_BUSY`, a lost answer) is an error too. A positional named `branch` would take the global flag's clap id, so `merge` and `branch archive` name theirs `source`/`target` internally (pinned by a test).
- **`fin-config.json` (4B)** in server data: `{defaultBranch: {<projectId>: <branchId>}, tries: {<projectId>: {<branchId>: <parentId>}}}`, written temp-then-rename (0600 on unix) under `fin-config.json.lock` (exclusive create, 10 s wait, a lock older than 30 s is taken over); a broken file fails loudly rather than turning into writes on `main`. It is per server data, not in the project folder: another machine has its own defaults.
- **Author (4B, Decision 25):** `fin` sends `$FIN_AUTHOR` when set and not empty, else `"fin"`, as `Tx.author` and as every history request's `author`. Undo stacks are per author (§4.5).

**`--idem-key`** asks `tx.get` before sending:
- committed → that commit (with the `argsHash` check, §3.11);
- set aside → **`TX_SET_ASIDE`** (exit 4), never resent;
- unknown → commit.

A commit whose answer was lost in flight is looked up once with `tx.get` on a fresh connection. **4B:** branch and history writes do the same (`tx.get(K)`, or `tx.get(K-1)` for a merge, whose records are `<mergeId>-<n>`) and send `argsHash` (BLAKE3 of `[{cmd, args}]`) with a key; the store answers a retried txId from the log, so one resend covers a lost answer. `--dry-run --idem-key K` for an already committed `K` answers that commit (without `dryRun: true`). `project.recover` takes no `argsHash`.

**Error codes added in 4A and 4B, and their exit codes** (`crates/fin/src/output.rs` is authoritative):

| Exit | Codes |
|---|---|
| 2 (usage: can never succeed as sent) | `TX_TOO_LARGE` |
| 3 (validation, refused data) | `DB_TOO_NEW`, `NOT_A_PROJECT_DB`, `UNSUPPORTED_HASH`, `NEWER_FORMAT`, `INVALID_FORMAT`, `REPLAY_FAILED`, `BINDINGS_MISMATCH`, `CLIP_OVERLAP`, `CLIP_EMPTY`, `CLIP_NEGATIVE`, `INVALID_ID`, `RECORD_TOO_LARGE`, `DB_CORRUPT`, `REBUILD_WOULD_LOSE`, `GENESIS_MISMATCH`, `VERIFY_FAILED`, `PROJECT_COPY`, `PROJECT_ORIGINAL_UNREACHABLE`, `INVALID_NAME`; 4B: `NAME_TAKEN`, `BRANCH_ARCHIVED`, `NOTHING_TO_UNDO`, `NOTHING_TO_REDO` |
| 4 (the project moved under you) | `PROJECT_READ_ONLY` (`details.reason` per §3.7), `LEASE_LOST`, `LEASE_TAKEOVER_REFUSED`, `SEGMENT_COLLISION`, `HISTORY_REWOUND`, `TX_SET_ASIDE` |
| 5 (unavailable, transient) | `ENGINE_BUSY`, `DB_IN_USE` |

`SEGMENT_CORRUPT` is a verify finding (§3.8), not an error code. 4B reuses `INVALID_NAME`, `INVALID_OP`, `BRANCH_NOT_FOUND`, `NOT_FOUND` (`details.kind` `tx | merge | checkpoint | group`), `CONFLICT` (exit 4, with `details.conflicts`, §4.3) and `IDEM_KEY_REUSED`. Conflict reasons (`REMOVED`, `SAME_EDGE`, …) and no-op reasons are not error codes.

### 3.11 SP2 follow-ups that land in 4A

All landed (plan Tasks 16–17):
- **`project create`** (`fin-store::create`): atomic and idempotent.
  - An empty or whitespace name → `INVALID_NAME` (exit 3; `fin` checks on `--dry-run` too).
  - The DB is built as `project.fdb.creating-<ulid>`, closed and renamed into place. The project file is then written **write-once**; that rename is the commit point, and `.finisher/` is made only after it.
  - Any failure removes the DB, and the folder if create made it. A crash between the DB rename and the file write leaves an unreferenced DB in server data (a sweep is a follow-up).
  - A retry with the same `idemKey` (`meta.createKey`) and the same name and rate opens the project; the same key with another name or rate → `IDEM_KEY_REUSED`; anything else → `PROJECT_EXISTS`.
  - A retry waits ≤ 10 s for the creator's `lease.json`, so it follows rather than taking the lease. `finisherd` serialises creates per folder.
  - `createKey` survives a rebuild that can read the old DB, and is lost when the DB is missing (it isn't in the folder).
- **`ops.subscribe`** with `fromRev` above the head → `BASE_REV_AHEAD`, before streaming.
- **Typecheck gate:** `tsc --noEmit` for finisherd, contracts, core-types and fin-node (`scripts/check-typecheck.mjs`, part of `pnpm gates`).
- **Idempotency:** `tx.get`, and **`Tx.argsHash`**: `fin` hashes the command input (BLAKE3 of `[{cmd, args}]`, with `asset.add` paths canonicalised), only with `--idem-key`.
  - The store refuses a stored `txId` whose `argsHash` differs → `IDEM_KEY_REUSED {txId, rev, committedOps}`; a malformed hash → `INVALID_OP`.
  - The hash is not derivable from the stored intent, because a retry plans ops against a newer snapshot and the stored tx is rebased.
  - It survives export, import and rebuild.
- **Rationals:**
  - `FromStr` accepts only the canonical form (`2/4`, `-0`, `+1`, `01` → `INVALID_TIME` with `details.canonical`);
  - stored data deserialises leniently (normalising), and `check_canonical_times` reports `INVALID_TIME {input, canonical, path}` over RPC;
  - `details.rev` is always numeric.
- **`fin-node`:**
  - the Linux loader picks `-gnu` or `-musl` from the diagnostic report, falling back to the other;
  - long calls are off the libuv pool (Decision 7);
  - finisherd hubs are removed with their last subscriber.
- **`Probe`** has a JSON Schema and a generated TS type. There is a MOV fixture test (`qt` brand).
- **Schema export** moved to `crates/fin-store/examples/export_schema.rs`, so it can include `fin-store`'s wire types and `Probe`. The contracts' Effect envelope schemas are compared with the generated `$defs` by a vitest test (`packages/contracts/src/envelopes.test.ts`).

### 3.12 Tests and acceptance (4A)

**Unit and property tests** (all in `cargo test --workspace`; 550+ Rust tests at Task 16):
- **Classification:** valid, not-arrived, foreign and conflict-copy, for every pattern in §3.6.1.
- **Chain building and the epoch rule:**
  - reference-model property tests over random segment sets (duplicates, gaps, forks, stale epochs, packs in grace, a stale segment landing after a takeover but before the new holder's first export);
  - scenario tests for malformed files, `txId` reuse, partial-overlap re-exports and lying packs;
  - an exhaustive arrival-order test (every order of three segments, one poll per arrival) for rewind convergence.

  The reference models restate the decision procedure, so they can't catch a rule that both share wrongly; the scenario tests cover that.
- **Export crash** between rename and marking: no fork, no double apply.
- **Id counters** after import: no reuse after a takeover.
- **Migrations:** the SP2 fixture → current with identical state; newer-DB refusal; the backup is taken before migrating; m0001's output is pinned.
- **Upcaster golden tests** (none yet: there is no step).
- **Rewind:**
  - followers F and G converge (same state and quarantine);
  - own records are rewound too;
  - an unbacked winner stays read-only, and sync never rewinds;
  - rewinds across a snapshot boundary, and inside and right after a pack;
  - a subscriber gets `HISTORY_REWOUND`, and a stale-generation commit is refused;
  - compaction keeps the losing segment.

**Fault injection** (in process, every OS):
- kill during export at each step;
- truncated files, bad CRC, simulated placeholders;
- the DB deleted, corrupted (header, pages) or replaced, then rebuilt with salvage;
- crashes between the rebuild swap's renames;
- compaction interrupted after the snapshot, pack, manifest and deletes (`CompactFail` hooks);
- the folder vanishing and returning, unwritable folders, Unicode and space paths.

**Integration** (`finisherd` + `fin`):
- a follower is a second `finisherd` with its own server data on one folder;
- lease changes, stale generations, set-aside records, and offline rebuild;
- the RPC fixtures cover every new scenario.

`pnpm e2e` has a storage scenario: edit, export, delete the DB, rebuild from the folder with an identical snapshot, and a follower.

**Cross-host on the NAS and sync folders:** plan Task 19 (§3.13), harness `tests/storage-xhost/` (not shipped):
1. lease races, 100 per host pair: 0 double holders, 0 early takeovers, 0 applied stale writes, 0 id reuse;
2. network loss with writes in flight: the stale writer's segments are quarantined or superseded, never left applied;
3. real clock skew if any host is naturally off by ≥ 5 s, else emulated and noted;
4. follow latency: edits from host A appear on read-only host B within the export delay (2 s; 5 s under churn) plus one poll (1 s on a share) plus the cache window;
5. sync folders (macOS, owner's accounts; blocked on the owner if not installed): temp-then-rename upload, conflict-copy naming per provider, placeholder hydration.

**Acceptance:** all of the above pass, or are recorded as blocked with a reason; an SP2-era project opens after migration with identical state (done); the e2e storage scenario passes (done).

### 3.13 Validation results (Task 19) — pending

Task 19 has not run. 4A is not mergeable until it has. Each item below was written from the spec and §27.9 without being exercised on the real systems, and Task 19 must measure or confirm it. Results go here as durable conclusions; raw runs stay in `archive/`.

1. **Full suites** on Windows 11 and the Linux VM: `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e`. macOS code paths have so far only been type-checked from Linux.
   - *Linux VM, 2026-11-05:* all pass (555 Rust, 85 JS, 32 gate tests, `pnpm gates`, both e2e scenarios).
   - *Windows 11 (MSVC toolchain, NTFS), 2026-11-05:* `cargo test --workspace --release` with `FIN_REQUIRE_SERVER=1` 542 pass (the rest are Unix-only), `pnpm test` 85 pass, `pnpm test:gates` and `pnpm e2e` pass. `pnpm gates` was not run there; its checks don't depend on the OS. The first run found 5 failures: 4 were test-only (`\` separators in expected paths; `Main` and `main` are one folder on a case-insensitive volume). The fifth was a real bug: the Windows storage probe never looked for sync-folder marker files, so a Syncthing (or Resilio, `.dropbox`) folder was classified `local`. It now checks markers first, as on Unix.
   - *macOS (APFS, case-insensitive), 2026-11-05:* `cargo test --workspace --release` with `FIN_REQUIRE_SERVER=1` 550 pass, `pnpm test` 85, `pnpm test:gates` 32, `pnpm e2e` and `pnpm gates` pass. One test-only failure: APFS refuses non-UTF-8 names (EILSEQ), so the non-UTF-8 scan test skips there. After the lease fixes (item 3) and the directory-cache fix (item 2) the suites were re-run on all three: Linux 576, macOS 569 (`3e284dc`, the first macOS compile of the macOS-only paths; clippy clean), Windows (the affected crates).
2. **NAS matrix** (§3.12 rows 1–4) for Mac↔Windows, Mac↔Linux and Windows↔Linux.
   - *Setup, 2026-11-05:* one SMB 3.1.1 share on a NAS; Linux cifs (`cache=strict,actimeo=1`), Windows 11, macOS 27 smbfs. Per pair: 100 lease races on a fresh project, 5 races for a stopped holder's takeover, a 90 s holder stall with writes in flight (also 30 s, and a stall inside a fence check), −60 s emulated server-time skew on the holder, and follow latency; the asymmetric rows also with the roles swapped. The harness is `tests/storage-xhost/`.
   - *Windows↔Linux, on the final code:* every row 0 critical: no double holder, no takeover before staleness, no stale write applied or left applied, no id reuse, no acknowledged write vanished. The only set-aside records were the 1–2 a stalled holder had acknowledged but not exported (`DIVERGED_TAIL`, §3.4). Follow latency with commits every 500 ms: p50 2.0 s, p95 3.1 s (races); a Windows follower p50 5.8 s (the Windows directory cache); stall and skew rows have p95 25–48 s because they include the takeover wait.
   - *Mac↔Linux, before the directory-cache fix:* races, stopped-holder takeovers, stall, skew and follow all 0 critical with the Linux node as verifier. With a Mac holder stalled and the Mac as verifier, 64–71 acknowledged, exported records looked lost: they were on the share, but the Mac's listing did not show them. **macOS smbfs caches a directory listing for 30–60 s** (`nsmb.conf` `dir_cache_min`/`dir_cache_max`; measured 30–31 s), so the 10 s ordering-rule wait was unsafe on a Mac: a Mac that took over could have committed over another server's exported records. Fixed: the wait is 65 s on macOS network shares, and listings are refreshed first by creating and removing a temp file in the directory, which drops the client's cache (measured: immediate; a `stat` does not) (§3.6).
   - *Mac↔Linux and Mac↔Windows, on the final code (65 s wait and listing refresh):* every row 0 critical, including the Mac-holder stall and skew rows that failed before the fix: no double holder, no unsafe takeover, no stale write applied, no id reuse, no acknowledged write vanished (14,000+ accepted commits across the two matrices). Set-aside records were only the 1–9 a stalled or skewed holder had acknowledged but not exported (`DIVERGED_TAIL`). Follow latency in races p50 2.2 s, p95 3.5 s; a Windows follower p50 ~7 s. A Mac opening a project another host created moments earlier could get `PROJECT_NOT_FOUND` (its lookup cache), as Windows did; fixed by the network retry (item 14, 2026-11-05).
   - *Found on the way:* macOS smbfs has no hard links (`ENOTSUP`), so the first `lease.json` takes the `create_new` fallback there (§3.5 "Files"); Linux cifs and Windows publish it atomically. `F_FULLFSYNC` is `ENOTSUP` on smbfs; `fsio::sync_file` falls back to `fsync`.
3. **Lease against the spike:** diff the lease's behaviour against the S10 spike's v3 when its code is available.
   - *Comparison with an archive copy, 2026-11-05:* four differences. Fixed: (a) a failed or torn read of the holder's own record lost the lease at once; it is now a failed beat, bounded by the self-fence deadline, with the spike's 10 × 20 ms re-read (§3.5 "Heartbeat and fencing"); (b) creating `lease.json` with `create_new` and then writing left a short record after a crash, which only `takeover --confirm` cleared; it is now staged and published with a server-decided hard link (`MoveFileExW` on Windows), falling back to `create_new` only where there are no hard links (§3.5 "Files"); (c) a Windows claim name pending deletion (ACCESS_DENIED) failed the acquisition; it is now retried (§3.5 "Take" step 2). Kept: (d) a lower-epoch record over ours gives the lease up where the spike rewrote (§3.5). The spike's transient error lists were reviewed and not adopted wholesale (§3.4 "Names and temp files"). Still to run on real systems with the rest of Task 19.
4. **mtime source on SMB:** whether the probe file's mtime is set by the server or by the client. Covers Samba's 2 s write-time delay and the Linux cifs deferred close. Claims are judged old by server time. Under a client-set mtime with skew above 45 s, two racers should both lose (no double write); confirm this.
   - *Linux↔Windows SMB, 2026-11-05:* the NAS clock, read independently from the SMB2 NEGOTIATE response, was 0.1 ms off Linux and 30.9 ms off Windows. Windows probes read server time, set when the server handles the FLUSH (within ~2 ms of the NAS clock; close never changes it). Linux cifs can serve a client-set mtime from its attribute cache after a deferred close (`closetimeo`), 3 s off in a test, so the probe reads the mtime with `statx(AT_STATX_FORCE_SYNC)` (`fsio::modified_on_server`). Samba shows an unflushed write's mtime only 2 s later; an fsync sets it at once. With ±60 s emulated skew on one racer (56 races, some with the Windows racer delayed 120–200 ms), 0 double holders: claim and record exclusivity are decided by the server. The "both lose" outcome was never provoked. macOS smbfs has no `AT_STATX_FORCE_SYNC` equivalent and was not tested for client-set mtimes.
5. **Clock skew:** real skew if available, else emulated, under the takeover rules (25 s unchanged-record rule, probe rejection > 1 s below extrapolation).
   - *Emulated −60 s server-time skew, Linux↔Windows, 2026-11-05:* a contender saw the live holder's record 62.7 s old from the start and never took it over while the holder ran; after a stall, takeover came 26–29 s after the holder's last stamp (50.5–51 s without skew). So a skew of 45 s or more defeats the staleness window, and the 25 s unchanged-record rule (monotonic, longer than the 20 s self-fence) is what prevents an unsafe takeover: 0 unsafe takeovers, 0 double holders. A constant skew never triggers the probe rejection (> 1 s below extrapolation); none of 96 real probes was rejected.
6. **Suspend/resume of a holder:** a sleeping Mac or Windows holder wakes fenced (suspend-aware clocks: `CLOCK_MONOTONIC` on macOS, `QueryInterruptTimePrecise` on Windows), and a takeover it slept through is not overwritten.
   - *macOS, 2026-11-05:* `counts_sleep()` is true and the code uses `CLOCK_MONOTONIC`, which Apple documents as counting sleep. A real sleep/wake was not tested (the Mac stays awake for other work).
7. **Windows claim invisibility:** a fresh `lease.claim-*` can stay invisible to Windows for up to 9.9 s. Check the race with the single `base + 1` claim and the 45 s claim authority.
   - *Linux↔Windows SMB, 2026-11-05:* a Linux-created claim stayed invisible to Windows for 0.95–9.7 s (median ~9.6 s) to listing, stat and open-by-name alike, but a Windows `create_new` of that name failed at once with `AlreadyExists` (4/4). Linux sees Windows claims at once. The rewritten-in-place `lease.json` was visible to Windows 0.06–0.4 s after a takeover. The single base+1 claim is therefore safe under invisibility: the name's uniqueness is decided by the server. 200 fresh-project races and 44 races with Windows delayed: 0 double holders.
8. **OS lock behaviour** of `File::try_lock` / `lock_shared` (`lease.lock`, `db.lock`, `server-id.lock`) on Linux, Windows and macOS, including the restart shortcut. The shortcut compares the holder host by `describe_holder`'s `"<host> (finisher …)"` prefix.
   - *macOS APFS, 2026-11-05:* exclusive/shared `try_lock` behave as `dblock.rs` and `server_lock.rs` assume, within a process and across processes. Not yet run on SMB. The restart shortcut compares `gethostname()`, which macOS can change with the network when no `HostName` is set: the shortcut then misses (slower, not unsafe).
   - *SMB, 2026-11-05:* every lock file (`lease.lock`, `db.lock`, `server-id.lock`, finisherd's) lives in server data on a local disk; no `*.lock` file was found on the share after all runs.
9. **Machine-id sources:** `/etc/machine-id`, `gethostuuid`, `MachineGuid`. Also directory identity: `FileIdInfo` on ReFS, the file index on NTFS, and a zero serial on FAT/exFAT.
   - *macOS, 2026-11-05:* `gethostuuid` matches `IOPlatformUUID` (re-checked with this item's test: the id is its 32 hex digits, lower case). APFS directory identity is `st_dev`/`st_ino`; on the smbfs share too (a server-provided inode).
   - *Linux VM, 2026-11-05:* `/etc/machine-id` exists (`/var/lib/dbus/machine-id` is a link to it) and is what the code reads; a test compares them.
   - *Windows 11, 2026-11-05:* `machine_id` equals `MachineGuid` from `reg query` (the 64-bit view). On NTFS the code takes `FileIdInfo` first, not the file index: its 128-bit id is what `fsutil file queryFileID` prints and the volume serial is the 64-bit NTFS serial (`fsutil fsinfo ntfsinfo`), on `C:` and `E:`. On the SMB share `FileIdInfo` gives nothing usable, so the 64-bit file index is used, again equal to `fsutil`'s id (the server's inode). Tests on all three OSes: one directory has one identity across spellings and a rename, a sibling another. ReFS and FAT/exFAT were not available (the only FAT volume is the unmounted EFI partition), so `FileIdInfo` on ReFS and the zero-serial rule on FAT stay untested.
10. **Mount-root rules for copies:** macOS `/Volumes`, Linux `/mnt`/`/media`, Windows mapped drives and UNC paths, and unmounted shares.
   - *Windows, 2026-11-05:* a mapped-drive path canonicalises to `\\?\UNC\host\share\…`, so both spellings are one folder, and a project created through a drive letter records the UNC path (unmapping the drive does not make it unreachable). A copy on the share or on local disk is refused with `PROJECT_COPY`; renaming the original within a reachable share counts as a move. A genuinely unreachable share was not reproduced (Windows kept access after `net use /delete`).
11. **Storage-kind detection on real installs:**
    - macOS: iCloud Drive including Desktop & Documents, Dropbox File Provider, Google Drive, OneDrive;
    - Windows: OneDrive Known Folder Move and SharePoint libraries, Google Drive `G:`;
    - Linux: rclone/FUSE mounts, WSL2 `/mnt/c`.
    - *macOS, 2026-11-05:* iCloud Drive → `sync:icloud`; a File Provider sync client under `~/Library/CloudStorage` → `sync:unknown`; APFS → `local`; smbfs and NFS → `network`. Dropbox, Google Drive, OneDrive and Desktop & Documents were not installed or enabled.

    Also check the Linux `fuseblk` = local rule and the `~/.dropbox` false positive.
12. **Conflict-copy names:** Dropbox `(… conflicted copy …)`, `(Case Conflict)` and `(Selective Sync Conflict)` (from memory), Google Drive `(<n>)` / `[Conflict]` (unverified), iCloud and OneDrive.
13. **Transient error lists** (`fsio::is_transient`): unix `EINTR`/`EAGAIN`/`EBUSY`/`ESTALE`, Windows 5/32/33/64. Also SQLite `immutable=1` URI forms (drive, UNC, `\\?\`) and the backup fsync through a write handle on Windows.
   - *SMB, 2026-11-05:* codes seen in cross-host tests: opening a file pending deletion — Windows 5, Linux `ENOENT`; opening a file held without sharing — Windows 32, Linux `EBUSY`; `create_new` on a name pending deletion — Windows 80 (`AlreadyExists`); deleting a file another client holds open — Windows succeeds, Linux `EIO` (left off the transient list: callers wait and re-list). No `ESTALE`, `EAGAIN` or Windows 64 appeared (no reconnects). The SMB codes seen are covered by `fsio::is_transient`. `immutable=1` URIs open the plain, canonical, `\\?\` and UNC spellings on Windows and Linux (test).
14. **Export on SMB/NFS:** read-back after `AlreadyExists`/`NotFound`; the 15 s project-file retry on Windows.
   - *Linux↔Windows SMB, 2026-11-05:* `write_once` with an uncached read-back, 150/150 per OS: the read-back always found the file the first time and a second write always got `AlreadyExists`. Temp-then-rename replacement while the other host reads, all four host combinations: ~1,600 reads, 0 `NotFound`, new content visible in 50–90 ms. Gap: Windows reports `PROJECT_NOT_FOUND` for ~2.5 s when another host has just created the project and Windows listed the parent before (the directory cache); the 15 s retry does not cover it because `.finisher/` is invisible too. Usability only. NFS was not tested.
   - *`PROJECT_NOT_FOUND` after another host creates the project, 2026-11-05 (Linux creates on the NAS share, the other host opens):* with no earlier lookup of the path, Windows and the Mac see the new project at once (the parent's listing alone hides nothing on Windows; on the Mac it hid it 0.4–1.5 s). After a lookup of the project path, its project file and `.finisher/` (an earlier failed open), Windows answers "not found" for ~4.9 s (its 5 s not-found cache, 15/15 raw trials) and the Mac for 1.5–2.4 s. Creating and removing a temp file in the parent makes the Mac see it at once (0.1–0.2 s, 7/7) but does not help on Windows (4.9 s, 6/6). The old code failed every such open at once (Windows 3/3, Mac 3/3); with the 12 s network retry (§3.4) every one opened (Windows 6/6, open 6.2–6.3 s vs 1.7 s with no earlier lookup; Mac 5/5, 1.4–1.7 s vs 1.2–1.4 s). A path that does not exist fails after 12.1 s on the share (both) and at once (9–27 ms) on a local disk; after the review fix one whose parent is missing too fails at once on the share (12 ms Windows, 42 ms Mac), and the new-project opens were re-run (Windows 3/3 in 6.2–6.3 s, Mac 3/3 in 1.1–1.5 s). Linux cifs and NFS take the same 12 s retry (not measured).
15. **Compaction on SMB:** deleting a file another client has open (`ACCESS_DENIED` or a pending delete on Windows should be reported as wait and retried); sync folders delivering deletions before the pack (readers wait; 24 h grace).
   - *Linux↔Windows SMB, 2026-11-05, real compaction with grace 0:* Linux compacting while Windows holds segments open (normal sharing): each delete returns `EIO` but the file goes pending deletion; the next pass finds it gone and the compaction finishes (one 60 s retry per held file). Windows holding without delete sharing: the delete fails until release, then succeeds. Windows compacting while Linux holds them: deletes succeed; the pending names reappear in Windows listings as unreadable (error 5) for a while and compaction waits until the holder closes. Verify used to report such files as `SEGMENT_CORRUPT`; it now lists them as unsettled, reason `locked` (§3.4). Sync folders delivering deletions before the pack were not tested.
16. **Sync folders:** temp-then-rename upload, placeholder hydration (Windows attributes, `SF_DATALESS`, `.icloud` stubs), and the sync gate after a confirmed takeover.
   - *iCloud Drive, macOS 27, one device, 2026-11-05:* files ending in `.tmp` (any case) are never uploaded, so `.~fin-<ulid>.tmp` stays local; after the rename the final name uploads once, at full size, within 1–5 s. Writing straight to a final name is **not** safe: iCloud uploaded a growing file several times, truncated (400 KB, 2.4 MB, 4 MB of 6 MB), so the §8 fallback "unique final names written directly" must not be used on iCloud. An evicted file keeps its size, carries `dataless` (no `.icloud` stub on this version); `stat` does not download it, `open` returns at once and the first read blocks ~1 s and downloads the whole file. This matches `fsio::is_placeholder` and the hydrate pass: with every segment evicted, a fresh server's verify opened in ~3 s and applied all records. One first read of a freshly uploaded 8.4 MB file failed with `ETIMEDOUT` (1 in ~25); the hydrate pass retries on its next pass, and since 2026-11-05 every other direct read of a placeholder retries a timeout up to 3 times 1 s apart (§3.4; unit-tested through a test hook, not re-run on iCloud). The folder classifies `sync:icloud`. Conflict-copy names and what a second device sees mid-upload need a second device; the sync gate after a confirmed takeover was not exercised across devices.
17. **Daily backups:** the macOS local date (`localtime_r` after `tzset`), and Windows `FileTimeToLocalFileTime` around DST changes.
    - *macOS, 2026-11-05:* `local_ymd` is right under five zones (including UTC+14) and both 2026 European DST days.
    - *Windows 11 (system zone W. Europe), 2026-11-05:* `FileTimeToLocalFileTime` applies the bias in force *now* to any time: in November it turned 2026-03-29T21:30Z into 22:30 (CEST is 23:30) and dated 2026-03-29T22:30Z and 2026-10-24T22:30Z a day early (2 of 6 instants 30 min either side of local midnight on the DST days). Right for "now", which daily backups use, but not for the backup's "tomorrow" bound across a DST change. Replaced by `SystemTimeToTzSpecificLocalTimeEx` with the current dynamic zone (the zone's rules for that date). A Windows test injects zones by key, so the system zone is not changed: W. Europe, GMT, FLE, Pacific (the nights after the 8 March and 1 November 2026 US changes, each date different under the other bias), Line Islands (UTC+14) and Hawaii, 16 instants around the 2026 DST changes, plus 2006-03-20T07:30Z in Pacific under the 2006 rules (DST from 2 April: 19 March, where 2026 rules would give the 20th), and the W. Europe instants through the system zone; all pass. Not tested: with automatic DST adjustment turned off, the current zone has `DynamicDaylightTimeDisabled` set and Windows applies the standard bias all year (documented), which is the clock the user sees.
18. **Lease after `lease.json` is deleted under a holder:** status reported the lease as held elsewhere and `LEASE_HELD` rather than `LEASE_LOST` (seen in Task 15 tests). Confirm the intended reading.
   - *2026-11-05:* the holder lost the lease correctly at its next beat (a deleted record is evidence, §3.5), but its next acquisition attempt then found only its own claim beside the missing record, which §3.5 reads as held, and reported itself as another holder (`heldElsewhere` with its own server id). Intended reading: the holder cannot prove its lease any more, so it reports `LEASE_LOST`; other servers see missing record + claim = held (`LEASE_HELD`). Fixed: a newest claim that is this server's, at the epoch this process held and lost (in-process registry), is reported as lost and still judged like any claim, so both wait for it to go stale (no faster re-acquisition). After review (2026-11-05): the same holds after a restart or for a second process of the same server data when the server-data lock is free (no process of ours runs a lease; taken only to ask); while another process holds the lock it stays held. `fin project takeover` keeps its refusal codes there but says that this server's lease at epoch N was lost and is re-acquired once that claim is stale (~45 s). On a sync folder the claim shows as `Stale` once stale, and `takeover --confirm` re-takes it at once (before, it backed off for our own young claim as for an acquisition in progress). Tests on Linux, Windows and macOS: contender (`lease.rs`, also the sync case) and store (`integration_lease.rs`, holder, follower, restart with the lock free and held). Tests: the contender (`lease.rs`), the store's read-only reason for the holder and a follower (`integration_lease.rs`), and the finisherd event test, which now expects `LEASE_LOST` only.

---

## 4. Stage 4B — history

**State (2026-11-06):** implemented on branch `sp4b-history`, stacked on `sp4a-durable-storage` (plan `docs/superpowers/plans/2026-11-05-sub-project-4b-history.md`, Tasks 1–21). The cross-platform and cross-host validation (Task 23) has not run yet: see §4.8. This section describes 4B **as built**. The module docs of `fin-core::{record, rebase, branch, history}` and `fin-store::{branch, derived, history_ops, import, recover}` hold the exact rules; code comments cite the plan's decision numbers, which the table below keeps. The interfaces (RPCs, `fin` commands, error codes) are in §3.10, rows marked 4B.

**Decisions taken by the 4B plan** (the earlier draft left these open):

| # | Decision | Where |
|---|---|---|
| 1 | Kind fields are flat, optional record fields right after `kind`, in a fixed order; a typed `Link` reads them. *Why:* `edit` records stay byte-identical to 4A. | §3.2, §4.2 |
| 2 | `undoes` is a list of unit ids, newest first (`undo --to` names many). | §4.2 |
| 3 | A `branch` record carries `forkSeq` and `forkHash` (absent at 0): the index derives from the log alone, and a fork may be below the head. | §4.1 |
| 4 | Segment version follows content: edit-only `main` segments stay v1, anything else is v2. | §3.4 |
| 5 | DB schema v2 (m0002) adds derived tables; "recovered" is derived, not stored. | §3.1, §4.1, §4.5 |
| 6 | Derived tables follow the log: updated on commit and import, recomputed after any row deletion, compared by verify. | §4.1, §3.8 |
| 7 | Rewinds and diverged tails cascade to descendants whose `branch` record was lost, replacing 4A's refusal. | §3.6.2 step 6a |
| 8 | A set-aside `branch` record takes the segments that name it along; a directory whose record is unknown waits. | §4.1 |
| 9 | Reads below a fork read the ancestors (lineage reads). | §4.1 |
| 10 | Export runs parents before children. | §4.1 |
| 11 | Name rules, uniqueness among non-archived branches, duplicates from import. | §4.1 |
| 12 | Branch references are an id, an active name or `<name>#<id>`; stored data holds the id. | §4.1 |
| 13 | `main` cannot be renamed or archived; an archived branch refuses writes. | §4.1, §4.5 |
| 14 | Creation stores the parent's state at the fork as the child's first snapshot. | §4.1 |
| 15 | Import learns branches from the `branch` records it applies, in the same pass. | §4.1 |
| 16 | The rebase engine is a mapping with mirrors (ProseMirror `Mapping` semantics). | §4.3 |
| 17 | Conflict reasons are codes; `CONFLICT` lists every conflict. | §4.3 |
| 18 | Merged and picked copies keep the source's entity ids; `DUPLICATE_ID` is `ALREADY_PRESENT` or `ID_COLLISION` by origin root. `clip.restore` takes `$` ids. | §4.3, §4.4 |
| 19 | Merge candidates (superseded in part by 34). | §4.4 |
| 20 | One `merge` record per copy, `<mergeId>-<n>`, written by the merging author. | §4.4 |
| 21 | `merged` state: nothing of the branch's own is left to merge into a non-descendant that holds a merge from it. | §4.4 |
| 22 | Undo stacks per (author, branch) over the branch's own records; dropped only by a merge into a non-descendant. | §4.5 |
| 23 | A redo record is a unit of its own. *Deviation:* the earlier text pushed the original unit back; the effect is the same and the inverse is exact. | §4.5 |
| 24 | Mirrors pair undo↔units, redo↔undo, copy↔inverted original, and same-root records across two paths. | §4.3 |
| 25 | Stacks are keyed by `Tx.author`; `fin` sends `$FIN_AUTHOR`, else `fin`. | §3.10, §4.5 |
| 26 | History writes take `txId` (the idempotency key) and `dryRun`; dry runs work on a follower. | §4.1 |
| 27 | Recovered branches: groups, fresh ids, names. | §4.5 |
| 28 | Id counters are raised past quarantined ids. | §3.3 |
| 29 | `CHAIN_FORK` keeps the manual procedure, plus `fin project recover --import <dir>`. In-band fork resolution (a record kind) is a follow-up. | §3.7, §4.5 |
| 30 | New codes `NAME_TAKEN`, `BRANCH_ARCHIVED`, `NOTHING_TO_UNDO`, `NOTHING_TO_REDO` (exit 3); `NOT_FOUND` gains `details.kind`. | §3.10 |
| 31 | RPC tags `branch.*`, `history.*`; `project.recover` answers the `RecoverReport`; no new event type. | §3.10 |
| 32 | Global `--branch`, `fin branch rename`, `try` records in `fin-config.json`. | §3.10, §4.5 |
| 33 | Branch identity across servers: `branch` records carry their fence `epoch`, segments name their `branch` record, an id made twice is held by (epoch desc, txHash asc). *Added during implementation.* | §3.4, §4.1 |
| 34 | Merges are transitive, and "already there" is judged by origin root. *Added during implementation (review of the merge task).* | §4.4 |

### 4.1 Branches

**Identity and numbering**
- A branch has an id (`main`, then `br<n>` from the shared id counter, §3.3), a display name, a parent, `forkSeq`/`forkHash`, `createdBy`, and a state `active | archived | merged` (archived wins over merged).
- A branch's `seq` continues from `forkSeq + 1`, so seqs up to the fork mean the same on both; its first record's `parentHash` is `forkHash`. Creation stores the parent's state at the fork as `snapshots(child, forkSeq)` (Decision 14), so the child's head is `forkSeq` before its first record, and `refresh` takes `forkHash` as the tip there.
- **Lineage reads (Decision 9):** `ops_since`, `state_at`, `hash_at` and the stale-commit frame read a branch's own rows, then each ancestor up to the lowest fork below it (a fork below the parent's own fork reads the grandparent), filling pending diffs per ancestor. Below the fork, `ops_since` returns the ancestors' records with their own `tx.branch`. A parent loop, a gap in a branch's rows, or an ancestor short of the fork is `DB_ERROR`.

**Names and references** (Decisions 11–13)
- Display names match `[A-Za-z0-9._-]{1,64}`, do not start with `-` or `.`, and are neither `main` nor like an id (`br\d+`), case-insensitively. A bad name is `INVALID_NAME` (`details {kind, name, reason}`).
- Names are unique, case-insensitively, among **non-archived** branches; `merged` branches keep theirs. A taken name is `NAME_TAKEN` (`details {name, branch: <holder>}`). Duplicates can still arrive through import: the lower numeric id holds the name (`br9` before `br10`), the others list as `<name>#<id>`.
- A **reference** in every API is an id, the name of the branch that holds it, or `<name>#<id>` (which also reaches an archived branch). The store resolves it under the write lock; intents, records and hubs hold the id. An unknown reference is `BRANCH_NOT_FOUND`.
- `main` (any root) cannot be renamed or archived (`INVALID_OP`).

**Storage: all branch state is derived from the log, so it syncs.**
- **Creation:** a `branch` record on the **parent's** log at head + 1, `{branchId, name, forkSeq, forkHash, epoch}`, no ops (`intent.ops == []`). The fork is the requested seq (default: the parent's head) or a checkpoint (§4.5), also below the parent's own fork (`main@v1` from `br1`); past the head is `INVALID_OP`. The id is allocated inside the write, after the parent is checked, so a refused create consumes none. Without a name the branch is `try-<digits of its id>` (`-2`, `-3`, … when taken). An archived parent is `BRANCH_ARCHIVED`.
- **Rename, archive, restore:** a `branchSet` record `{branchId, name?, archived?}` on the branch's parent, also when the parent is archived (Decision 13 as built: an archived parent still takes `branchSet` records, so its child can be renamed, archived or restored, but refuses `branch` records and content). An archived branch can be renamed without restoring it. `NAME_TAKEN` is checked for the state after the set; restoring onto a taken name needs `--restore --as <new>`, which the `fix` says. A `branchSet` that changes nothing is `INVALID_OP`.
- **The index** (m0002 tables, Decision 5):
  - `branches (id, name, parent, fork_seq, fork_hash, created_seq, created_by, created_at, record, epoch, archived, merged)`: `record` and `epoch` are the `branch` record that holds the id (Decision 33);
  - `record_links (tx_id, branch, seq, kind, author, origin, origin_hash, from_branch, merge_id, targets, name, ignored, recovers)`: one row per non-`edit` record, `UNIQUE (branch, seq)`, indexed by origin, origin hash, merge id and `recovers`. `ignored` says why an inert record took no effect; `recovers` is the quarantined record a recovery's record stands for.
  - `quarantine_marks (tx_hash, kind, target)`: per quarantine row, its record's kind and the branch it names (§4.5).
  - Both are updated incrementally by commit and import and rebuilt from `txs` by `derived::recompute` after any row deletion (rewind, diverged tail, cascade). `derived::compute` is the read-only fold verify compares against (`branch_index`, §3.8). Debug builds assert after every pass that folded a branch record that the incremental index equals `compute`.
- **`merged`** is derived from the copies on other branches (§4.4).
- **Writes** go through one path, `Store::write` (dedupe on the first txId, read-only check, lease check, IMMEDIATE transaction, fence, commit), shared by commit and every history operation. **Dry runs** (Decision 26) run the same planning in an IMMEDIATE transaction that is rolled back, with no read-only, lease or fence check, so they work on a follower. *Why IMMEDIATE:* planning writes (ids, filled diffs, index rows), and a DEFERRED transaction that read first would fail at once with `SQLITE_BUSY` when a leader beside the follower commits. A dry run writes `epoch` 0 in a `branch` record it keeps nothing of.
- **Idempotency:** the txId is the key. A retry answers from the log (with `dryRun: false`); a key holding another kind of record, another merge group, or another `argsHash` is `IDEM_KEY_REUSED`; a malformed `argsHash` is `INVALID_OP`. A txId equal to an existing mergeId is refused by commits, undo, redo, branch create and set, checkpoint, cherry-pick and revert, and a merge refuses a mergeId that is a txId or whose `<mergeId>-<n>` another record holds (`IDEM_KEY_REUSED` both ways).
- `branch.list` lists `main` too, with `createdBy: ""` (migration 0002's row).

**Import** (Decisions 8, 10, 15, 33)
- **Worklist:** branches resolve parents first (index order: depth, then numeric id), then branches with rows but no index row. An applied `branch` record makes its child known in the same pass (index row, fork snapshot) and pushes it on the worklist. Each record is absorbed into the index **before** its row is stored, so an unfit record writes nothing.
- **Start check:** a non-`main` segment with `prevSegmentHash` `null` must start at `forkSeq + 1` with `parentHash == forkHash`, else it is malformed. The chain cache key includes the fork point.
- **Classification by `branchRecord`:** a known branch resolves from the segments of its directory that name the index's record. Every other segment is classified by the record it names: the record is a current quarantine row (`STALE_EPOCH`, `SUPERSEDED`, `DIVERGED_TAIL`, `MALFORMED`, `ID_COLLISION`) → every record of the segment is quarantined with that reason (origin `folder`, detail `branch record <hash12> set aside`), recursively for the grandchildren they make; the record is inert in `txs` → `ID_COLLISION`, or `MALFORMED` when the index refused it; unknown → the directory **waits** (`ImportReport.unknownBranches`, status `unknownBranches`, never read-only). *Why:* nothing is dropped or silent, and a set-aside child can never apply onto a live branch of the same id. Each named record is one primary-key lookup; nothing is scanned.
- **Id collisions across a takeover** (Decision 33): two servers can make the same `br<n>`, on the same parent or different ones. The id is held by `(epoch desc, txHash asc)` of the `branch` records: arrival-order independent, and it survives packing because it reads record bytes, not segment labels. The losing record is an **inert row**: applied on its parent's chain (no ops; the index ignores it), so the parent keeps following. When the loser's branch was already applied, its subtree is rewound whole as `ID_COLLISION` (`cascade::subtree`, `DIVERGED_TAIL` for its unexported rows), and the winner's branch is resolved in the same pass. If the holder is itself set aside later, the loser's branch comes back with its fork state restored. A branch born under an id forgets the rewinds recorded under it (marked `retired`; status still reports them).
- **Unfit records are inert too:** a name the rules refuse, an unknown branch, a `forkHash` that is not the parent's record at `forkSeq`, a `branchSet` of a branch that lost its id. They are marked in `record_links.ignored` (rewritten from `compute` on every pass that folded a branch record, so every follower in any arrival order and a rebuild hold the same marks), and status warns **`BRANCH_RECORDS_IGNORED`**. A record on a resolved chain therefore never stalls a branch for index reasons.
- **Export** runs parents before children (index order, then index-less branches by name; Decision 10). *Why:* a follower then never sees a child segment before its parent's `branch` record from the same writer. A branch's first segment has `prevSegmentHash` `null` and its first record's parent is `forkHash`.
- **A 4A reader** (reader version 1) lists every `segments/br<n>/` file as `Newer` and goes read-only `NEWER_FORMAT`; it already does once the parent's segment holding the `branch` record (format 2) arrives, so nothing changes for it.

**Verify, rebuild, compaction** are branch-aware: §3.8 (check 9 `branch_index`, rebuild seeding) and §3.9.

**Live sync:** `ops.subscribe {projectId, branch}` takes a reference; hubs are keyed by id, and each committed record is published to its own branch's hub (§3.10).

### 4.2 Record kinds

| kind | Fields (after `kind`, in Decision 1's order) | Ops |
|---|---|---|
| `edit` | — | yes |
| `undo` | `undoes: [unit id]` (a txId or a mergeId, newest first) | the units' stored inverses, rebased |
| `redo` | `redoes: <undo txId>` | that undo's stored inverse, rebased |
| `merge` | `mergeId`, `from: <branch id>`, `origin: <source txId>` | the source record re-run on the target |
| `cherryPick` | `from`, `origin: <root txId>`, `originHash?`, `recovers?` | the record re-run on the target |
| `revert` | `reverts: <mergeId>` | the group's inverses, newest first, rebased |
| `checkpoint` | `name`, `recovers?` | none |
| `branch` | `name`, `branchId`, `forkSeq`, `forkHash?`, `epoch`, `recovers?` | none |
| `branchSet` | `name?`, `branchId`, `archived?`, `recovers?` | none |

- **Field order** on the record: `undoes, redoes, mergeId, from, origin, originHash, reverts, name, branchId, forkSeq, forkHash, epoch, archived, recovers`, each omitted when absent. `RecordKind` serialises camelCase. `Record::link()` reads the fields as a typed `Link` and refuses a field set that does not match the kind (`INVALID_FORMAT`); upcasting validates it. `CommittedTx.link` carries the link, omitted for edits.
- **`origin`:** a merge copy names the source record it copies; a cherry-pick names the **origin root** (a pick of a pick, or of a merge copy, names the record first written), and `originHash` is the root's `txHash`, also on a recovered copy (§4.5).
- **`epoch`** on a `branch` record is the fence epoch at commit (part of the bytes and the hash; required; 4A never wrote a `branch` record), Decision 33.
- **`recovers`** (on `cherryPick`, `checkpoint`, `branch`, `branchSet`) names the quarantined record a recovery's record stands for, and only a recovery writes it (§4.5), so no other record's bytes change and only it marks a quarantine row recovered. *Why not `originHash`:* an ordinary pick's root may itself be set aside later (its line loses), and must still be counted and offered (final review of 4B).
- Kinds with ops (`edit, undo, redo, merge, cherryPick, revert`) are content; `checkpoint`, `branch` and `branchSet` have `intent.ops == []` (client `tx.commit` still refuses empty ops).
- **Format:** `RECORD_FORMAT = 2`, `MIN_READER = 2`; segments are versioned by content (§3.4). A reader that meets an unknown `kind` in any file treats it as `NEWER_FORMAT`.

### 4.3 Rebase engine (stronger)

- **Rule:** an edit is rebased op by op: each op's positions are mapped through the shift maps of the records committed since its base, then the op is re-run at the evolving head with full policy. A record's `Diff.shifts` is its map; the inverse of `Shift {from, by}` is `{from + by, -by}`. The sub-project 2 "blind after our own trim/delete" conflict is gone.
- **Mapping order** (Decision 16): `[inverse(our earlier ops), intervening steps, our rebased earlier ops]`, with mirrors. `Assoc::After` everywhere.
- **Recover rule (ProseMirror):** a position in a removed range `[s, e]` collapses to `s` and is recorded as deleted with `offset = pos − s`, except at the edge on the assoc side (`e` for `After`). It resumes through a **mirror** only when the two shifts are on the same track with equal `|by|`. Without a mirror, only a position strictly inside `(s, e)` is unmapped (`Deleted`); one at the deleted edge is a boundary and carries on from `s`, as in 4A, so an insert at the end of a clip whose neighbour was deleted still maps. With `After`, a position at the start of our own earlier insert resumes at offset 0, which keeps "insert before our own earlier insert" in order.
- **Mirrors** (Decision 24, `rebase::steps_with_units`): an undo is cut into one part per unit it names, each mirroring its unit (an `undo --to` whose units in the interval are a prefix of its list gets one unpaired tail part for the older ones); a redo mirrors its undo; a revert mirrors its group; a merge or pick copy mirrors its inverted original; a target record mirrors the inverted source record of the same origin root. A step is in one pair at most: a part whose step is already paired stands unpaired and re-applies it, so chains (revert, then undo of the revert; undo, then redo) pair with the copy in effect. Mirrored pairs are left out of conflict checks.
- **Conflicts are judged per entity** (footprints: one `footprint` function per op module, from the resolved op and its own diff, never from a project):
  - touching or using X when an unmirrored intervening record removed X (the last removal or creation decides) → `REMOVED`. This covers a stale insert of an asset that `asset.remove` removed;
  - the same edge of X → `SAME_EDGE`; the same parameter → `SAME_PARAM` (4C);
  - removing an entity someone else edited is **not** a conflict (deleting a clip another author trimmed, on undo or redo too). There are two exceptions. A font unpin after a repin conflicts (below). So does a removal whose content the same transaction puts back into another entity: in an undo of `item.split` or `caption.split`, the left part's trim-back covers the right part's range, so the lift `item.remove` of the right part is judged as a put-back of it. The planner marks those removals from the records' links (`history::absorbs`, `Rebase::absorbs`): an undo, redo or revert of a split, or a copy of one, followed through undo, redo and origin links. The undo of an overwrite insert has the same two ops, but it removes an entity of its own, so it is no conflict. Another author's later edit of the right part (a parameter, an effect, its text or style, its end) is then `SAME_PARAM`, or `SAME_EDGE` for an edge, blamed on that author;
  - **an op that puts an entity back whole** from a snapshot (`Footprint::overwrites`) meets any later change of that entity, of an entity nested in it (`<id>/…`: an item's effects) or of what lies on it as a conflict. What lies on it is any put-back entity among a record's `tracks` or `uses`: an insert or move onto a put-back track, a `marker.add` or `track.add` on a put-back timeline, a placement in a put-back ordered list (below). It counts a touch of any kind, a creation or a removal by an unmirrored intervening record: `SAME_EDGE` when that record moved an edge of a put-back entity, else `SAME_PARAM`, blamed on the first such record. These ops are `item.restore`, `effect.restore`, `track.restore` (the track, its items and transitions, and anything put on it since), `timeline.restore` (the timeline, its markers and tracks, all on them, and anything added to it since), and the `marker.add`, `asset.add`, `bin.add` and `style.add` that invert a removal (a literal id; a `$placeholder` makes a new entity and puts nothing back). They arrive on every path that rebases ops: undo, redo, `undo --to`, merge, cherry-pick and revert, including the internal restores inside the inverses of `item.move`, `caption.merge` and the overwrite trims and moves, all through `rebase_prepared`. It is judged after the op re-ran, so an op that cannot run keeps the re-run's reason (`ALREADY_PRESENT`, `ID_COLLISION`, `CLIP_OVERLAP`, …). It is also judged only when the re-run wrote something: an op that writes nothing at the head (a pin of a file and fallback place already held) loses nothing, so the same pin on both sides merges. *As built (2026-11-07):* the rule above it had no counterpart for re-creation, so a snapshot went over later edits without a word. The 4C undo property and review found the cases (fixed-case tables: `props_history_4c`, `restore_conflicts`):
    - an undo of a move dropped another author's later trim, effect edit or keyframe on the item;
    - after a split of the moved item, the undo played the right part's media twice;
    - a merge, cherry-pick or revert of a source removal and its undo dropped the target's edit of that entity: an item's trim, an effect's `enabled`, a marker's label, a trim on a removed track, an item inserted or moved onto it (a moved item left the project), a timeline's name, a marker or track added to it, an asset's or bin's move, a style's field, a font's repin.
    - **ordered lists** (as built, 2026-11-07, `ops::util::lists`): a put-back at its old absolute index reordered the target's list. Unpinning Lora and undoing it, merged over main's pin of Roboto at fallback 0, gave `[Lora, Roboto]`. Removing t2 and undoing it, merged over main's `track.add` of t4 at index 1, gave `[t1, t2, t4, t3]` and changed the composite. Each ordered list is now an entity: the track order of each timeline (`<timeline>#tracks`: `track.add` and `track.restore` name their timeline, and `track.move` and `track.remove` record it on the track's change, `Change::timeline`, so edits on disjoint timelines never meet), the timeline order (`#timelines`), each item's effect order (`<item>#effects`) and the font fallback order (`#fonts.fallback`). Every op that adds, moves or removes a member lists it in `uses`. An absolute index never maps through later list changes, so an op that names one is carried to the head by its neighbours (`rebase::places`; as built 2026-11-07, review M2). In the op's own frame, the planner reads the members right before and after the place the index names, leaving out a member the op moves. At the head the engine puts the op between them if they are still neighbours, or next to the one that is left if the other was removed. Another author's placement between them, both neighbours gone, or a list that was empty and is not now is `SAME_PARAM`, blamed on the first record that used the list. This covers the index put-backs (`track.restore`, `timeline.restore`, `effect.restore`, a `font.pin` that places its key, a move back to an old index) and the client ops that name an index (`track.add` or `effect.add` with one, `track.move`, `effect.move`), on every path: undo, redo, `undo --to`, revert, merge, cherry-pick and stale commits. Appends and removals elsewhere never stop an index op, and an undo of a removal puts the member back between the neighbours it had. Two cases the old absolute index got wrong are now right: after another author removed an earlier member, the member goes back before its successor; S1's clamp is gone. Only the index field is rewritten at the head, so the stored ops and inverses keep their format. Relative put-backs stored in the inverse were rejected because they change that format. Markers (sorted by position), bins, assets and styles (maps) have no index put-back;
    Inverses stay absolute, as §4.5 stores them. A relative move back was considered and rejected: it does not cover the restore of a removal, and after a split it would move only the left part back. Fonts: the pin itself is no put-back. `font.pin` is also the client op of a literal key, and two pins of one file must merge. Instead a full `font.unpin` touches the pin's `sha256` and `fallback` as well as removing it, so an unpin after another author's repin or reorder of the key is `SAME_PARAM`, because its inverse puts the old file back. With the split's right part above, these are the only removals of an edited entity that conflict. A `font.pin` that places its key at an index does put the fallback *order* back, as any index does, and two identical pins still merge, since the second writes nothing;
  - edits on disjoint track sets never conflict, but entity rules ignore tracks, so a 4C move between tracks still meets the other track's edits. Link groups (arch §6.4) come with 4C;
  - entities our own earlier ops created are never judged.
- **A re-run that fails** is a conflict with the policy code it raised, blamed on the one unmirrored step that changed an entity the op changes, would create or the error names (a `font.pin` meeting another pin of its key, `FONT_PINNED`, is blamed on that pin). A stale tx that does not apply at its own base carries `details.baseRev` and a fix to resend against the head.
- **Conflict detail** (Decision 17): `{txId, opIndex, reason, againstTx, details?, fix}`. Reasons: `REMOVED`, `SAME_EDGE`, `SAME_PARAM`, `DEPENDS_ON_SKIPPED`, `ALREADY_PRESENT`, `ID_COLLISION`, `UNDOES_TARGET_RECORDS`, `STRADDLES_CHECKPOINT` (§4.5), or a re-run's policy code. `CONFLICT` (exit 4) keeps the 4A detail fields from the first conflict and adds `details.conflicts`, the full list.
- **Stale commits** take their frame from `state_near_head` (a rewind from the head through stored inverses, or a replay from the nearest snapshot, whichever touches fewer records), across the lineage.
- **Placeholders:** `clip.restore` may name its clip with a `$` id (internal ops only); replay hands the recorded id out through `BoundIds` (§3.2).

### 4.4 Merge, cherry-pick, revert

**Merge** (`fin merge <branch> [--into <b>] [--skip-conflicting] [--dry-run]`; `--into` defaults to the target branch, §3.10)
- **Common point C:** the two lineages share a branch, nearest first; C is the lower of the seqs up to which each lineage reads it. Each path is a lineage's records after C. Branches with no shared history (or a branch into itself) are `INVALID_OP`.
- **Candidates (Decision 34; supersedes the txId tests of Decision 19):** the source path's records that carry edits (`edit`, `undo`, `redo`, `cherryPick`, `revert`, and `merge` copies the source brought in from other branches), in order, unless the target already holds their **origin root** (the root is on the target's lineage, or a target record after C has the same root). *Why:* merges are transitive (br2 → br1 → main brings br2's records; an undo of a merge on the source goes along with its group; `try keep` after updating from the parent works), and "already there" holds however a record travelled. A candidate skipped earlier is offered again, since nothing copied it.
- **Excluded, `UNDOES_TARGET_RECORDS`:** a source `undo`, `redo` or `revert` (or a copy of one) whose units hold a record whose root is on the target, directly or through another such record: re-running it would undo the target's own edits. Reported in `MergeReport.excluded`.
- **`DEPENDS_ON_SKIPPED`:** a conflict blamed on a skipped or excluded candidate (its inverse stands unmirrored), and an `undo`, `redo` or `revert` whose unit holds a skipped or excluded candidate: its inverse would undo what the target never got. So a multi-unit `undo --to` record with one skipped unit is skipped whole.
- **Re-run:** each candidate is re-run at the target head over its steps (the source path before it inverted, the target path with its mirrors, the copies planned so far), with the source's bindings (`BoundIds`) and full policy (`Client` for client ops and copies of them, `Internal` for inverses). Copies keep the source's entity ids (Decision 18). A `DUPLICATE_ID` is judged per id by the origin root of the record that first bound it on each path: the same root on both, or no binder on either (a literal id older than C, which a restore brings back as the same entity) → `ALREADY_PRESENT`; otherwise `ID_COLLISION` (two servers across a takeover can hand out the same `c<n>`).
- **Records** (Decision 20): one `merge` record per copy, txIds `<mergeId>-<n>`, sharing the `mergeId` (the caller's txId). Their intent and record author are the **merging author**, so the copies are one unit on the target's stacks; the source author survives through `origin`.
- **All-or-nothing:** one DB transaction, or nothing plus the full conflict list. Without `--skip-conflicting`, planning goes on past a conflict as if that record were skipped, so the full list comes back (`CONFLICT`, exit 4). With it, the rest is merged and the skipped records stay on the source. Followers apply any prefix of the group safely, so no folder-level atomicity is needed.
- **A merge with nothing to copy writes nothing:** an empty report. The same mergeId used later therefore merges what arrived since.
- The source may be archived; the target must be writable. `MergeReport {mergeId, committed, merged, skipped, excluded, sourceState, dryRun}`; a retry answers from the log with `skipped`/`excluded` empty.

**`merged` state** (Decision 21 as built)
- B is `merged` when a branch T that is **not a descendant of B** holds a `merge` record from B and nothing of B's own (seq > `forkSeq`) is left to merge into T (`left_to_merge`, the candidate rules above). Records that are no candidates (excluded, or already present) do not keep B active. A branch merged on through another (br2 → br1 → main) stays merged through br1. A merge into a descendant never marks the parent merged, so `merge main --into <child>` never makes `main` merged.
- A new edit on B makes it `active` again, and so does a merge into B that brings records B would still have to merge.
- It is settled on every write and import: each new copy rechecks every branch holding a record of its origin root (up the chain to the root, then down every copy, transitively), the copy's `from`, and on import every branch the pass gave records or made (copies can arrive before what they copy). `derived::compute` derives it from the log alone, so followers in any arrival order and a rebuild agree.
- **Trap (as in git):** after undoing or reverting a merge on T, B stays `merged`: its records are still copied there (the copies' origins are B's), so a new merge of B into T copies nothing. Getting them back takes an undo of the undo or revert, or a cherry-pick of B's records.

**Cherry-pick** (`fin cherry-pick <txId> [--into <b>]`)
- The merge machinery for one record: a `cherryPick` record naming the origin root (`originHash` its txHash), re-run over the same steps, keeping its ids. Unknown record → `NOT_FOUND` (`details.kind` `tx`); a record with no ops (`checkpoint`, `branch`, `branchSet`) → `INVALID_OP`; `merge` copies may be picked.
- Nothing to do (the target already holds the root, or the record is at or below C) answers `noOp: ALREADY_PRESENT` and writes nothing, so a repeat is harmless.
- A source undo, redo or revert tied to the target's records is `CONFLICT` `UNDOES_TARGET_RECORDS`; a conflict blamed on an earlier source record the target does not hold, or an undo, redo or revert whose unit's record the target does not hold (a redo whose undo is not there included), is `DEPENDS_ON_SKIPPED`, named by the picked record.
- A pick never drops the source's stacks (arch §6.4: only "when the branch is merged").

**Revert** (`fin revert <mergeId>`)
- One `revert` record: the group's stored inverses, newest record first, in the frame after the group's last record, rebased over the branch's records since (mirrors included). The group must lie on the branch's lineage, else `NOT_FOUND` (`details.kind` `merge`). `HistoryResult.undone` is `[mergeId]`.
- Nothing to do once the group's effect is gone (reverted, or undone, and not brought back by an undo of that or a redo; a copy of a revert counts as the revert, by origin root) answers `noOp: ALREADY_REVERTED`. So reverting twice is a no-op, and an undo of the revert lets it be reverted again.
- A revert that conflicts lists every group record whose inverse cannot be carried to the head, as a merge lists its candidates.

**Known reason imprecisions** (the refusals are right; their reasons could be clearer; TASKS follow-ups):
- an undo whose clip is back already (re-created by a copy) is `ID_COLLISION`, where `ALREADY_PRESENT` would say it better: the undo planner does not judge origins; only a direct pick of a restore can still get there;
- a copy whose clip is gone may answer the re-run's own `NOT_FOUND` with no blamed record, instead of `REMOVED` or `DEPENDS_ON_SKIPPED`, when an excluded inverted source undo lies between the two steps of a mirrored pair (the entity rule leaves mirrored pairs out).

### 4.5 Undo, redo, checkpoints, `try`, archiving, recovery

**Undo and redo** (Decisions 22–25)
- **The stacks are derived from the log, not stored.** For (author, branch), the branch's **own** records (seq > `forkSeq`) are simulated oldest first; `author` is `Tx.author`. Units:
  - one `edit`, `cherryPick` or `revert` record of the author;
  - a merge group, while its records follow one another with the same `mergeId`, keyed by its first record's author (the merging author);
  - a `redo` record: a unit of its own that stands for the unit it recreated (Decision 23). The next undo takes the redo record's stored inverse at its frame and writes `undoes: [<redo txId>]`. *Why:* exact without mapping through the undo/redo pair.
- A new unit clears the redo stack; an `undo` pops each unit it names and pushes itself on the redo stack; a `redo` pops its undo and pushes itself as a unit. `checkpoint`, `branch` and `branchSet` records are skipped.
- **Dropping:** the top-most unit with a record that is the origin of a **`merge` record with `from` = this branch on a branch that is not a descendant of it** drops with everything below it, from both stacks. Cherry-picks never drop; a merge into a child never drops; a merge via a child (br2, a child of br1, merged into main) does not drop br1's stacks, because `from` must be the branch itself.
- **`fin undo`:** the unit's stored inverse (its records newest first, one op per op), in the frame at the unit's last seq, rebased over the branch's own records since with mirrors, re-run with full policy (`Internal`), then appended as `undo {undoes: [unit id]}`. It may `CONFLICT` (one conflict; `details.author`, `details.branch`, a `fix` naming `FIN_AUTHOR`). Nothing to undo is `NOTHING_TO_UNDO` (`details {branch, author}`).
- **`fin redo`:** the same with the undo record's stored inverse; `NOTHING_TO_REDO` after a new edit. One redo reverses one undo record, so it reverses a whole `undo --to`.
- Merged records are undone on the target as one merge unit or with `revert`, never one by one.
- Stacks are read from `record_links` (kind, author, links) and only the unit's records and the interval are parsed: 1,500 own records, about 13 ms per undo in a debug build.

**Checkpoints**
- `fin checkpoint <name>` appends a `checkpoint` record (no ops). Names follow the branch name rules and are not all digits (a seq); they are unique per branch among the branch's **own** checkpoints, case-insensitively (`NAME_TAKEN`, `details {name, branch, seq, txId}`). An archived branch refuses it (`BRANCH_ARCHIVED`).
- A name is found over the lineage, own branch first, each ancestor only through the seqs it gives the lineage, newest first: a branch's checkpoint hides an ancestor's of the same name, and one on the parent after the fork is not found (`NOT_FOUND`, `details.kind` `checkpoint`). `fin branch new --from <b>@<name>` forks at the checkpoint record's seq.
- Checkpoints change no state: stacks and merged state skip them, a merge has nothing to copy from one, and a pick of one is `INVALID_OP`.

**`fin undo --to <checkpoint>`**
- One `undo` record for the author's units made after the checkpoint, newest first, all or nothing; it equals the plain undos in a row (each unit's inverse rebased over the later records and the undos planned before it). A checkpoint at or before the fork undoes all of the author's own records on the branch. `redo` with `to` is `INVALID_OP`; an unknown checkpoint is `NOT_FOUND` before `NOTHING_TO_UNDO` (which then adds `details.checkpointSeq`).
- **Which units:** the walk takes units from the top of the stack while the unit was **made** after the checkpoint. A unit's "made" is a range through its redo chain: its own last seq for a unit of records, the units it recreated for a redo unit. A unit whose range lies after the checkpoint is taken; the walk stops at one at or before it; one on both sides (a redo of an earlier `undo --to`, then `undo --to` a later checkpoint) is refused: `CONFLICT` with reason **`STRADDLES_CHECKPOINT`** (`details {unit, checkpointSeq}`, `fix`: plain undos, or `undo --to` an earlier checkpoint), nothing written. So "e1, checkpoint, undo, redo" keeps e1 (it is part of the checkpoint's state), while a redo whose undo is at or before the checkpoint counts at its own seq ("e1, undo, checkpoint, redo" takes it: the checkpoint's state lacks it).
- **Conflicts:** planning goes on past a failing unit as if it were left alone, so `CONFLICT` lists every failing unit (`txId` the record's, `opIndex` from the record's first op, `details.unit`); one blamed on a skipped unit's record is `DEPENDS_ON_SKIPPED`.

**`try`**
- `fin try [name]` forks from the target branch's head (default name `try-<n>`), records the try in `fin-config.json` (`tries: {<projectId>: {<branchId>: <parentId>}}`, per project because two projects in one server data both make `br1`) and makes it the default branch.
- `fin try keep` merges it into its recorded parent; `fin try drop` archives it. Both switch back (the default is cleared when the parent is `main`) and forget the try. Off a recorded try branch both are `USAGE` (`fix: fin try`). `try drop` on a default whose branch is gone clears the default and the try (`{cleared, switchedTo}`).

**Archiving**
- Archived branches are hidden from `fin branch ls` (unless `--all`), kept, and addressed by id or `<name>#<id>`. They refuse writes (`BRANCH_ARCHIVED`, exit 3, `fix: fin branch archive <id> --restore`); they can still be a merge's source.
- Squashing them is **deferred out of sub-project 4**: nothing before sub-project 6 needs it, and it needs its own two-phase delete.

**Recovered branches** (Decisions 5, 8, 27–29; `fin-store::recover`)
- **Listing:** `fin project recover` lists the current quarantine rows that nothing stands for yet as **groups**: runs of `parentHash` links on one branch. A row with two children on its branch, or a child on another branch, ends its group, and each child starts a child group (`children`). A group's id is its first record's hash12. Each group forks where `create` will fork it (`branch`, `forkSeq`, `forkHash`, `forkGroup`; `recordsBranch` is the records' own branch): at the first record's parent in `txs`, or, through rows already recovered, at the copy of the nearest recovered row (with the id remap learnt from the copies, so a later group continues an earlier recovery). Every group is planned against the database (fresh ids from a copy of the counters, nothing kept), so `problems` are exact: `ID_COLLISION`, `NO_COMMON_HISTORY`, `BRANCH_ARCHIVED` fork, policy failures, `NOTHING_TO_RECOVER`, `DEPENDS_ON_SKIPPED` for the children of a group that cannot be made; `notes` list rows it would leave (a rename whose name is now held, a branch that is gone). `recoverable` says whether it can be made.
- **`--create <group|all>`** makes branches in one write: per group a `branch` record on its fork branch, then a record standing for each row, then its child groups (forked from the recovered parent at the copy of their fork record, or from the original parent when the fork is on the surviving line). Records are `<txId>-<n>` over the whole request; a retry of the txId answers from the log; any `<txId>-<n>` another record holds, or a merge id, is `IDEM_KEY_REUSED`. `all` makes every recoverable group and skips the rest; one group that cannot be made is `INVALID_OP` (`details.problems`), and a child whose parent group would make a branch is refused with a fix naming the parent. Unknown id: `NOT_FOUND` (`details.kind` `group`).
- **What stands for a row:**
  - a record with ops → a `cherryPick` of its **origin root** (for a `merge`/`cherryPick` row, its `origin` followed through `txs` and quarantine), `from` the root's branch, `originHash` the root's hash (when known) and `recovers` the quarantined record's hash. So later merges judge the copy by the same root as its source (Decisions 18, 34) and never bring it twice;
  - a `checkpoint` → a fresh `checkpoint` of the same name with `recovers`;
  - a `branch` row whose branch has records → the group of those records makes its branch with that `recovers`; an empty branch's `branch` row → that branch made again (its own name, `-2`… when taken) at its fork, with `recovers`;
  - a `branchSet` of a **live** branch → the same change again as a fresh `branchSet` on its parent with `recovers` (also when already in effect, so the row is stood for), unless the name is now held (`NAME_TAKEN`, a note with a `fin branch rename` fix), the branch is gone or is `main`; a `branchSet` of a branch a quarantined `branch` row makes comes back with that branch.
- **Ids** (Decisions 27, 28): when the root is in `txs` the copy keeps its ids (the source's). Otherwise each id the record bound gets a fresh one from the counters, shared by every copy of the same root (in this write and with earlier recoveries), and literal ids of later records in the group are remapped, inverse-derived ops included. A literal id neither remapped nor at the fork state is `ID_COLLISION` only when a record **off the fork's lineage at or below the fork** binds it, not counting a copy whose origin binds the same id. *Deviation from the plan's "any record in the DB":* that would refuse the common case of an undo restoring a clip deleted before the fork, which is the same entity (the same reasoning as Decision 18).
- **Names:** `recovered-<server8>-<YYYYMMDD-HHMMSS>`: `<server8>` the first 8 characters of the writer's server id when all 8 are letters or digits (ours for `local`/`salvage` rows; for `folder` and `foreign` rows the `writerId` of the segment holding the record, found by one folder scan before the write lock), else `unknown`; the time is the group's first `committedAt` in UTC; `-2`, `-3`, … against the index and the groups listed before it, so the listed name is what `--create all` makes.
- **`--import <dir>`:** every `.fseg` under the directory that decodes and names this project; records `txs` lacks go to quarantine as `IMPORTED` (origin `foreign`, detail the absolute path), an existing row becomes current again. Files that cannot be used are listed (`skippedFiles {path, reason}`: unreadable, not a segment, another project); `imported` counts rows added or made current. It is allowed while read-only (that is its use after `CHAIN_FORK`), and with `--dry-run` it runs inside the dry write, so the listing sees the rows and nothing is kept.
- **`--dismiss <group>`:** a group that cannot be recovered stops being listed and counted on this server (`meta` `recoverDismissed`: local, not exported, lost on a rebuild; allowed while read-only). A recoverable group is `INVALID_OP`. `tx.get` still answers for its rows.
- **Marks:** a row counts as recovered while a record in `txs` names it as `recovers` (any kind; never an ordinary pick's `originHash`), so every server and a rebuild agree, and verify compares it through `record_links`. Status leaves recovered and dismissed rows out of `QUARANTINE`. To keep that cheap, `quarantine_marks (tx_hash, kind, target)` holds each row's kind and named branch, filled once per row past `meta` `quarantineMarksThrough`, and the summary is kept while the database is unchanged: 10,000 rows, 7.6 ms per changed poll, 0.35 ms unchanged (debug).
- **`CHAIN_FORK` and `CONFLICT_COPY`** keep the manual procedure (Decision 29): the report's `manual` names it (stop every server, move one history's files or the copy out of the folder, reopen, then `--import <dir> --create all`). In-band fork resolution, a record kind chain resolution would honour, is a follow-up.

### 4.6 Property tests (proptest, in `cargo test`, < 30 s total)

Each `proptest!` sets its cases explicitly and honours `PROPTEST_CASES`.
1. `apply` then `inverse` returns the exact prior state, for every op kind, internal ops and `$` restore ids included (`props_apply`, 256).
2. Rebasing onto an empty interval is the identity (`rebase_props`, 256).
3. **Rebase coherence:** if both stale-rebased and fresh-at-head succeed, they produce the same state, on an anchor model over 3 tracks with 0–6 committed edits (some followed by their undo) and asset ops; a stale refusal where the fresh edit applies must be `SAME_EDGE` or `REMOVED` (`rebase_props`, 128).
4. **Undo/redo:** 2–4 interleaved authors on `main` and a side branch (edit, undo, redo, cherry-pick, merges both ways) match a list-op reference model per track, with predicted conflict reasons, plus an independent oracle from clip lists and per-edge trim histories (`props_history`, 64; 15,000 cases on three seeds pass).
5. **Replay:** a generated log with branches (forked at a checkpoint or an earlier seq), merges and picks in any direction, archive and restore, revert and `undo --to`: each branch replays from its first snapshot to its head, and a follower importing the folder (partially along the way, then whole) has equal heads, index rows (`merged` included) and stacks, and both verify (`props_replay`, 16).
6. **Validation soundness:** `apply_tx` never panics for any op, ids and times (overflow included) in every mode, and success keeps the invariants (`props_apply`, 512).

Also: `shift_map`, `pick_props` (merge or pick after any revert/undo/redo chain keeps the source's order), `undo_to_props` (`undo --to` equals the undos in a row, also for stale edits), `chain_props` (256), and fin-core `time` (256). A seeded bug (the recover edge in `map.rs` flipped) fails props 2–4 and `pick_props`. **Budget** (debug, Linux VM): fin-core's 8 property binaries 2.6 s, fin-store's `props_replay` + `chain_props` 3.2 s; about 5.8 s in all.

### 4.7 Acceptance (4B)

- All property tests pass. *(Done.)*
- Every command has `fin` contract tests (`crates/fin/tests/cli_history.rs`, live server; RPC fixtures replayed by `rpc_contract.rs`). *(Done.)*
- `pnpm e2e` gains the `history` scenario: branch, edits on both sides, a `SAME_EDGE` merge conflict (exit 4), `--skip-conflicting`, undo of the merge, state checked, and a subscriber on `main` receiving the merge then the undo; beyond that redo, `undo --to`, rename/archive/restore, `try keep|drop`, cherry-pick, revert, dry runs, keyed merge retries, per-branch subscribers, and a real `recover --import` + `--create` from a second server's diverged copy. *(Done.)*
- Cross-host: a branch made on host A merges on host B after takeover. *(Pending: §4.8.)*

### 4.8 Validation results (Task 23)

Run 2026-11-06 on `sp4b-history` (c4ef8db plus the harness rows) and the 4A setup of §3.13: one SMB 3.1.1 share on a NAS, Linux cifs, Windows 11, macOS smbfs. The harness is `tests/storage-xhost/` (rows `history` and `mixed`; `xhost brancher`, `xhost merger`; `verify` and followers report every branch's head, `stateHash` and `indexHash`). Raw runs stay in `archive/`.

1. **Full suites**, `FIN_REQUIRE_SERVER=1`, all pass:
   - *Linux VM* (debug): `cargo test --workspace` 939, `pnpm test` 107, `pnpm gates`, `pnpm e2e` (smoke, storage, history 19 s).
   - *macOS* (`--release`): 934, 107, `pnpm gates` (cargo-deny included), `pnpm e2e` (history 20 s).
   - *Windows 11* (`--release`, MSVC): 925, 107, `pnpm gates` (it skips cargo-deny on Windows), `pnpm e2e` (history 20 s).
   - The counts differ only by platform-gated tests (Unix symlink, FIFO, permission and non-UTF-8 name tests; the Linux storage probes).
2. **`history` row**, 5 runs per pair. Each run: the brancher makes `x`, commits 6 edits on each line (one same-edge trim on each side) and stops without closing; the merger on the other host takes over, merges `x` into `main` with `--skip-conflicting` and exports.
   - *Windows↔Linux* (Linux branches, Windows merges) and *Mac↔Linux* (the Mac branches, Linux merges): 10/10 runs clean. 0 lost records (100 accepted per pair: edits on both lines, branch and merge records). Every run merged 5 records and reported the branch's trim skipped as `SAME_EDGE`. The follower on the brancher's host, after one follow cycle, and fresh verifiers on both hosts had the merger's `stateHash` and `indexHash` (15/15 views per pair, the derived `branches` and `record_links` tables included). The merge reached the other host's follower 2.2–3.7 s after it was committed. The merger took over 48–49 s after the brancher stopped and wrote from 56–60 s (the 10 s ordering-rule wait, §3.6).
   - *Mac↔Windows* (Windows branches, the Mac merges): on `sp4b-history` alone, 1 of 5 runs merged. In the other 4 the Mac merger's open failed with `PROJECT_NOT_FOUND`: a project made by Windows 15 s earlier was hidden behind the Mac's cached listing. 4B lacks the network open retry that 4A has since added (`7716b8a`, `145d137`; §3.13 item 14). With binaries built from `sp4b-history` with the current 4A merged in (the code merges cleanly; only this spec conflicts), 5/5 runs are clean, with the same counts as the other pairs and 0 harness-level open retries. A Mac merger writes about 114 s after the holder stopped: it takes over at 48 s, then waits 65 s (the macOS ordering-rule wait, §3.6). A Windows follower sees the merge 7.8–9.5 s after its commit (the Windows directory cache).
   - **Merging 4B therefore needs the current 4A merged into it first.**
3. **Mixed-version row** (Decision 4): a 4B brancher edits `main` only for 20 s, then makes a branch and edits both lines; a 4A follower (current 4A) follows on another host. Runs: Mac holder with a Linux follower, Windows holder with a Linux follower, Linux holder with a Mac follower; 2 runs each. Results:
   - The 4A follower applied every edit-only record in a v1 segment (117/117).
   - It turned read-only `NEWER_FORMAT` in every run, and applied nothing at or past the branch record.
   - The 4B side lost nothing.
   - **Edits exported in the same segment as the first 4B record are invisible to 4A** (0–1 per run, 5 in all): that segment is v2 because its version follows its content. So a 4A follower stops at the last export before the first 4B record, not at the record itself. This is by design. The records are on the share, and a 4B reader applies them. A follow-up could cut the segment before the first non-edit record.
4. **4A rows with 4B on top**, all 0 critical: 0 double holders, 0 unsafe or early takeovers, 0 stale writes applied or left applied, 0 id reuse, 0 acknowledged writes vanished.
   - *Races*, 20 per pair (Windows↔Linux 762 accepted, Mac↔Linux 684, Mac↔Windows 796): follow latency p50 2.1–2.2 s, p95 3.4–3.7 s.
   - *Stalls*, a 90 s holder freeze with writes in flight: the only set-aside records were the 2–8 the stalled holder had acknowledged but not exported (`DIVERGED_TAIL`, §3.4).
   - The Mac↔Windows rows were run on both builds. In 2 of each build's 20 races, the Mac racer failed to open (`PROJECT_NOT_FOUND`), so those rounds had a single racer. The project's parent, the run's new directory, had been made by Windows 1.5 s earlier and was hidden behind the Mac's cached listing of its own parent.
   - **4A gap:** even with 4A's retry this fails at once, because 4A skips the retry when the parent looks missing (to fail fast on typos, §3.13 item 14). A Mac opening a project in a directory another host has just made needs up to ~60 s, or an explicit refresh of the grandparent. Recorded for 4A.
5. **Defects:** none in 4B. The two findings are a stacking gap (4B needs the current 4A: item 2) and a 4A follow-up (item 4). The 4A-to-4B segment boundary is by design (item 3). No 4B code changed. The harness gained the history rows, an `--open-retry-ms` option, and analyser self-tests (`node --test tests/storage-xhost/`).

---

## 5. Stage 4C — edit breadth

### 5.1 Model

- **Project:** `timelines` (several), `bins` (tree; assets belong to one bin), `assets`, `fonts` (registry; §5.6), `styles` (caption style presets).
- **Timeline:** `id`, `name`, `rate`, `resolution`, `markers` (`{id, at, duration?, label, color}`), `tracks`.
- **Track:**
  - `kind: video | audio | caption`;
  - `name`, `mute`, `solo`, `lock`, `gain` (audio);
  - `items` sorted by `start`, with no overlap and implicit gaps (the current model).
- **Item** has the common fields `id`, `start`, `duration` and `kind`. Kinds:
  - `clip` (asset, `sourceIn`, `speed`);
  - `caption` (block, §5.5);
  - `graphic` (template asset + slot values);
  - `image` (image or SVG asset);
  - `adjustment` (applies its effects to the tracks below);
  - `nested` (timeline id; cycles rejected).
  - **Last-batch kinds:** `adjustment`, `nested`, `mask` and `blend` are built in 4C's final batch. Nothing consumes them before sub-project 6, so they may move there if 4C runs long. They stay in the model from the start so no migration is needed.
- **Common item fields:**
  - `transform` (position, scale, rotation, anchor);
  - `crop`, `opacity`, `blend`;
  - `effects[]`;
  - `mask?` (`{matte: artifact ref, feather, choke, invert}`);
  - `audio?` (`gain`, `pan`, `fadeIn`, `fadeOut`);
  - `keyframes` (per parameter path);
  - `link?` (link group id; linked items move, trim and delete together).
- **Speed:** a constant rational, greater than 0. Ramps and reverse come later.
- **Transitions:**
  - Overlapping clips on one track are still forbidden.
  - A crossfade or dissolve is a `transition` stored on the **track**: `{id, between: [itemA, itemB], kind: crossfade|dissolve, duration, curve}`.
  - It is valid only while A's end equals B's start. It is anchored to the items, so ripples move it with them.
  - An edit that separates or removes either item removes the transition, and its inverse restores it.

### 5.2 Settings registry

- **`fin-core::params`** declares every parameter of every item kind and built-in effect as a `ParamSpec`:
  - `path`;
  - `type`: number with min/max/unit, bool, enum, colour, vec2, rational time, text, or asset ref;
  - `animatable`;
  - `default`.
- **Generic edits:**
  - `param.set {item, path, value}`;
  - `keyframe.set {item, path, at, value, interp}`, `keyframe.remove`;
  - `effect.add {item, kind, at?}`, `effect.remove`, `effect.move`.

  Validation is driven by the registry.
- **Built-in video effects in v1:**
  - `crop` (part of the item);
  - `colorAdjust` (exposure, contrast, saturation, temperature, tint);
  - `lut` (asset ref);
  - `blur`.

  Audio effects beyond gain/pan/fades are plugins (sub-project 8), so the registry is designed to accept plugin-declared specs later.
- **Schema:** `fin schema param` lists the registry, generated from the same source as the JSON Schema.

### 5.3 Structural edits

- **Edit modes:** structural edits that can move other items carry a **required** `mode` in the op, so a stored record is never ambiguous.
- **CLI default:** `fin` defaults `--mode` to `ripple` for clips and `overwrite` for every other item kind, and always echoes the mode it used.

| Edit | Notes |
|---|---|
| `item.insert` | generalises `clip.insert`; any kind; `ripple` or `overwrite` |
| `item.move` | to a time and/or track; `ripple` or `overwrite` |
| `item.split` | at a time strictly inside the item; keyframes and effects are copied, times rebased. On a `caption` item it is refused with `fix: use caption.split` |
| `item.delete` | `ripple` (close gap) or `lift` (leave gap) |
| `item.trim` | edge `in`/`out`; `ripple`, `roll` (with the neighbour), `overwrite` |
| `item.slip` | moves `sourceIn` only |
| `item.slide` | moves the item, trimming its neighbours |
| `item.link` / `item.unlink` | link groups |
| `transition.set` / `transition.remove` | at a cut point |
| `track.add` / `track.remove` / `track.move` / `track.set` | `remove` only when empty, or with `--with-items` |
| `timeline.add` / `timeline.set` / `timeline.remove` | `remove` refused while nested anywhere |
| `marker.add` / `marker.set` / `marker.remove` | |
| `bin.add` / `bin.move` / `bin.remove`; `asset.move` | `bin.remove` only when empty |
| `asset.remove` | refused while referenced (`ASSET_IN_USE`, `details.items`) |

- **Compatibility:** sub-project 2's `clip.insert/trim/delete` stay as accepted aliases for one format version. Upcasters convert old records to `item.*`.
- **Inverse and shift map:** every edit defines both (§4.3).

### 5.4 Evaluation (`fin-core::eval`)

- **`eval_frame(project, timeline, t) -> FramePlan`.** The plan contains the ordered layers. For each layer:
  - the item id and kind;
  - the source and source time (through speed and nested timelines);
  - every parameter resolved at `t`;
  - effects in order;
  - the mask with its resolved values;
  - the transition state.
- **`eval_audio(project, timeline, range) -> AudioPlan`:** per-item source ranges and piecewise gain curves (gain × fades × crossfades × track gain, mute/solo applied), plus pan.
- **Keyframes:**
  - `{at (item-relative rational), value, interp}`;
  - `interp` is `hold`, `linear` or `bezier{x1,y1,x2,y2}` (CSS cubic-bézier semantics); named easing presets map to bézier values.
- **Determinism across platforms:**
  - `f64` with `+ − × ÷ sqrt` only;
  - bézier inversion uses a fixed iteration count (Newton, then bisection, fixed bounds);
  - no `sin`/`cos`/`pow` from the platform: rotation is output as an angle, and the compositor builds the matrix.
  - A test runs a fixed corpus and compares hashes of the serialised plans across macOS, Windows, Linux and wasm32 (wasm via `cargo test --target wasm32-wasip1` under wasmtime, if available in CI; otherwise native-only, noted).
- **Rendering:** the compositor and mixer in sub-project 6 consume only these plans (arch §12.3).

### 5.5 Captions

- **Block:**
  - `caption` items on caption tracks;
  - `{words: [{text, start, end}], styleRef, overrides, source?: {artifact, wordRange}}`;
  - word times are relative to the item.
- **Edits:**
  - `caption.import {track, words | srt, style}` creates blocks. The block-splitting rule (max chars/lines, gap threshold) is part of the style preset;
  - `caption.setText` (word-level, keeping times where words match, distributing the rest);
  - `caption.split`, `caption.merge`, `caption.setStyle`.
- **Transcripts:** a block keeps its own words. Later transcript changes don't touch it; re-syncing is an explicit edit (sub-project 8).
- **Safe zones** are named per platform preset (TikTok, Reels, Shorts, generic 9:16/16:9) as data in `fin-core`.

### 5.6 Text measurement (`fin-text`)

- **New crate `fin-text`:** cosmic-text + swash (MIT/Apache), ported from the S11b spike. It is pure, WASM-safe, and has fonts injected as bytes.
- **4C scope:** shaping, bidi, kerning, fallback, line breaking, and measurement → `TextLayout {lines, boxes, overflow}`. Rasterisation is sub-project 6.
- **Fonts:**
  - always bundled, never from the system (the condition for identical output, arch §27.10);
  - starting set: Inter, Noto Sans (Latin, Cyrillic, Greek), Noto Sans Arabic, Noto Sans Hebrew, all OFL. The set is listed in `THIRD_PARTY_NOTICES`;
  - users can add fonts as font assets.
  - **`fin-core` does not depend on `fin-text`.** `fin-core` defines a `TextMeasurer` trait. `fin-text` implements it, and `fin-node`/`fin` inject that implementation (bundled fonts plus project font assets) into the edit path and eval.
  - **Replay (`apply_committed`) never needs a measurer.**
  - **Known consequence:** a font or `fin-text` update can make an accepted caption overflow. Old records still replay, but a rebase, merge or undo that re-checks it may return `CAPTION_OVERFLOW` as a conflict.
- **Uses:**
  - caption validation rejects blocks that overflow the style's safe zone (`CAPTION_OVERFLOW`, `fix`: shorten, split, or reduce size);
  - eval attaches each caption's layout to the frame plan.

### 5.7 Assets

- `asset.add` gains kinds: `image` (PNG/JPEG/WebP probe), `svg` (size probe; sanitising is sub-project 6), `lottieTemplate` (dotLottie manifest and slot list, validated per arch §27.10), `font`, `lut` (.cube), and `audio`-only files (WAV, M4A/AAC).
- Probing stays in `fin-probe`.

### 5.8 `fin view`, `fin status`, commands

- **`fin view [--timeline] [--range a..b] [--budget <tokens>]`:**
  - screenplay-style text: one line per item per track, in time order, with captions' text inline and markers;
  - it degrades (merging, eliding) to fit the budget, and states what it elided.
- **`fin status`:** project, branch, head seq, lease and read-only reason, last export, unmerged branches, warnings.
- **Commands:** every edit in §5.2–5.5 gets a `fin` command with `--dry-run`. The output echoes the resolved values, which fixes the SP2 follow-ups: trim echoes the resulting edge, `--out < --in` wording, negative `details.max`, and `project open` ignoring `--dry-run`.

### 5.9 Acceptance (4C)

- Golden tests for each edit (apply, inverse, shift map), and every edit included in the 4B property generators.
- A plan-hash determinism corpus that matches across the three OSes.
- `fin-text` measurement identical across OSes for a fixed corpus (Arabic, Hebrew, Cyrillic, CJK fallback, emoji fallback box).
- `pnpm e2e` gains a scenario: build a 30 s reel by `fin` only (clips on two tracks, a graphic, captions imported from a word list, keyframed opacity, a crossfade), then check `fin view` and a set of `eval_frame` samples against goldens.
- A migration test: an SP2 project and a 4A/4B project upgrade to the 4C model.

---

## 6. Crate and file map

| Crate / package | 4A | 4B | 4C |
|---|---|---|---|
| `fin-core` | `record`, `upgrade`, `replay` (`apply_committed`) | as built: `record/link`, `rebase/` (`map`, `footprint`, `conflict`, engine), `branch`, `history/` (`undo`, `checkpoint`, `candidates`, `merge`, `pick`, `merged`, `origin`; pure planners over `LogView`) | `model` growth, `ops/*`, `params/`, `eval/`, `layout/` (via `TextMeasurer`), `captions/` |
| `fin-store` | as built: `migrations/`, `segment`, `snapfile`, `names`, `fsio`, `scan/`, `chain/`, `import/`, `export/`, `lease/`, `storage_kind/`, `open`, `tick`, `status`, `copies`, `history`, `rebuild/`, `verify/`, `compact/`, `backup`, `create`, `wire` | as built: `migrations/m0002_history`, `write` (one write path), `branch/` (`lineage`, `index`, `create`), `derived`, `history_ops/` (`SqlLog`, undo, merge, pick, checkpoint), `recover/`, `import/` (`pass`, `branches`, `aside`, `cascade`), `verify/branches` | — |
| `fin-text` (new) | — | — | shaping and measurement |
| `fin-probe` | `Probe` TS type, MOV fixture | — | image/SVG/Lottie/font/LUT/audio probes |
| `fin-node` | worker thread, maintenance tick, events; status/takeover/verify/rebuild/compact/txGet bindings | as built: `history.rs` (branch and history calls; `recover` long) | eval bindings |
| `@finisher/contracts`, `finisherd` | new RPCs, `project.events`, hub endings | as built: `branch.*`, `history.*`, `project.recover` report; hubs keyed by branch id | edit RPCs (generated from the registry where possible) |
| `fin` | `project *` commands | as built: `branch new\|ls\|rename\|archive\|default`, `merge`, `cherry-pick`, `revert`, `undo [--to]`, `redo`, `checkpoint`, `try`, `project recover` flags, global `--branch`, `fin-config.json` | edit commands, `view`, `status`, `schema param` |

## 7. Research basis (summary)

- **Upcasting on read, never changing stored events, and replay without re-validation:** Axon and Marten versioning docs; Dudycz's event-versioning patterns.
- **Explicit format version in headers:** LiteFS LTX lacks one and is worse for it. **CRC for corruption and a cryptographic hash for identity:** Git packs, Kafka, Automerge. **Epoch kept out of the hashed identity:** Kafka's partition leader epoch.
- **Fencing tokens must be checked by the storage or the reader:** Kleppmann (2016) and Chubby's sequencers and lock-delay. That is why readers enforce epochs (principle 4).
- **Server time from a probe file's mtime:** Solaris make `lock.c`; liblockfile. **Windows SMB client cache lifetimes:** Microsoft's file-server tuning docs (5/10/10 s).
- **Local live DB only:** SQLite "use over network" guidance; Lightroom refuses network catalogs. **Lock UX:** Premiere Productions `.prlock`. **Never auto-merging conflict copies:** KeePass. **Per-provider conflict naming:** Dropbox, iCloud TN2336, OneDrive, Syncthing docs.
- **DB migration conventions:** SQLite `application_id`/`user_version`; Chromium `meta`/`last_compatible_version`; Firefox downgrade refusal; backup before migrating.
- **Unverified, so they become test tasks:** atomic temp-then-rename upload on iCloud and OneDrive; Google Drive conflict naming; whether a sync folder's mtimes reflect any shared clock (assumed not).

## 8. Risks

| Risk | Mitigation |
|---|---|
| Lease v3 still split-brains on a host pair not yet tested (Linux SMB client, real skew) | 4A cross-host matrix gates the merge; reader-side epoch fencing caps the damage to quarantine |
| The lease was written from the spec, not the spike code, and its Windows/macOS paths are only type-checked from Linux | Task 19 (§3.13) runs every OS and host pair before merge; a late write that slips past fencing is rewound by followers (§3.6.2 step 6a) |
| Storage-kind detection misses a sync setup, enabling automatic takeover on an advisory lease | Detection leans to sync on ambiguous evidence; real installs checked in Task 19; `storageOverride` for the rest |
| Compaction deletes another machine's only copy of a record | Holder-only, fenced deletes; grace from the packs' arrival; equivalence re-resolve before deleting; never deletes off-chain segments; abandons on any contradiction |
| Sync providers break temp-then-rename | Sync-folder test rows (iCloud: temp-then-rename is safe, §3.13 item 16). The fallback of unique final names written directly is unsafe on iCloud, which uploads partial files |
| The rebase rewrite introduces subtle position bugs | Property 3 with a multi-track reference model; keep SP2 rebase tests as regression. As built (§4.6): props 3–5, `pick_props` and `undo_to_props`, with an independent oracle in prop 4; a seeded mapping bug fails them |
| Branches made on two servers across a takeover diverge between followers | Branch identity by `branch` record (`branchRecord` in every branch segment, `(epoch desc, txHash asc)` for a reused id), inert rows instead of stalls, all-arrival-order tests with verify and rebuild (§4.1); the cross-host `history` row (§4.8) |
| The settings registry is too generic for good errors | Every `ParamSpec` carries unit, range and a `fix` template; contract tests on error text |
| `fin-text` adds binary size to `fin` and the WASM build | Fonts are loaded from the bundled set at runtime, not embedded in `fin`; size measured in CI |
| 4C is large | 4C's plan is split into batches (model + structural edits; params + eval; captions + text; view/status), each green before the next |
