# Sub-project 4: Core breadth — Design

- **Status:** draft for review (2026-11-04)
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
   - Policy checks run only when an edit is made. Examples of policy checks: minimum durations, `INSERT_INSIDE_CLIP`, safe-zone overflow.
   - A rule added later never rejects an existing project.
3. **Intent and result are both stored.**
   - Each record keeps the command as submitted (with `$placeholders`) and the resolved ops (bindings substituted), plus the inverse.
   - Replay uses the resolved ops.
   - Rebase, merge, cherry-pick and undo create *new* edits: they re-run intent through the rebase engine with full policy checks.
4. **Single writer, fenced on both sides.**
   - The lease (v3) decides who writes; epochs are per project.
   - Every file written to the project folder carries the writer's epoch.
   - Where two lines of history continue from the same point, readers keep the higher-epoch one and set the other aside. §3.6 step 5 is the exact rule.
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

### 3.1 Local database

- **Header:** `PRAGMA application_id = 0x46494E31` ("FIN1"); `PRAGMA user_version` = DB schema version.
- **Version fields:** the `meta` table adds `formatVersion`, `minReaderVersion`, `createdBy` (app version) and `lastOpenedBy`.
- **Migrations:**
  - ordered steps `crates/fin-store/src/migrations/NNNN_<name>.rs`;
  - each step is SQL and/or Rust, runs inside its own transaction, and bumps `user_version` by exactly 1;
  - before the first step, the DB is backed up with `VACUUM INTO` to `<project-data>/backups/pre-migrate-<from>-<to>-<ts>.fdb`. Raw file copies are never used, because they miss the WAL.
- **Refusals:**
  - a DB with `user_version` above what the code knows fails with `DB_TOO_NEW`;
  - a DB whose `minReaderVersion` is above the code's version fails with `DB_TOO_NEW` (`fix`: update Finisher);
  - a file without the application id fails with `NOT_A_PROJECT_DB`.
- **Migration from the SP2 layout.** Migration `0001` converts the sub-project 2 schema in place:
  - each existing tx gets `seq` = its `rev`;
  - `intent` = its stored JSON;
  - each record is serialised **once** into its segment line, stored in `record_bytes`, and hashed in rev order (§3.3).

  This migration is the first thing 4A builds (TASKS follow-up "migrations before any op or model change").
- **Tables, at the end of 4A:**
  - `meta`, `ids`;
  - `txs`: `branch`, `seq`, `tx_id`, `kind`, `record_bytes` (the exact segment line, §3.3), `tx_hash`, `parent_hash`, `epoch`, `exported`, `diff`. The typed columns (`schema_version`, `author`, `committed_at`, …) are derived from `record_bytes` for indexing only;
  - `snapshots`;
  - `segments` (files known to exist in the folder, with hash, epoch and state);
  - `lease_state`;
  - `quarantine` (records set aside, §3.6).
- **Integrity on open:**
  - every open runs `PRAGMA quick_check`;
  - `fin project verify` runs `integrity_check` plus the checks in §3.8.

### 3.2 Versioning of ops and snapshots

- **`schemaVersion`:** each committed record carries the op-schema version it was written with. Snapshots carry the model-schema version.
- **Upcasters:**
  - they live in `fin-core::upgrade` (pure, no I/O, WASM-safe) as one function per step, operating on `serde_json::Value`, so old Rust types need not be kept;
  - they cover records (intent, resolved and inverse) and snapshots;
  - they must not change ids, timestamps or bindings;
  - each step has a golden test: frozen fixtures of the old shape go in, the expected new shape comes out.
- **Replay:** `fin-core::apply_committed(project, resolved_ops) -> Result<Project, StructuralError>` performs structural checks only. `apply_tx` (the edit path) keeps full validation.
- **Open no longer fails on any diff.** The current sub-project 2 open path, which "replays the tail and fails `DB_ERROR` on any diff", is replaced: open uses the latest snapshot plus `apply_committed`. Comparing stored diffs is moved to `fin project verify`, where a mismatch is reported, not fatal.

### 3.3 Hashes and identity

- **`txId`:** the ULID the client sends (unchanged). It is stable across upcasts.
- **`seq`:** dense per branch (§4.1 for how branches number).
- **`txHash`:** `blake3:<64 hex>`, computed over the record's JSON line **exactly as written** (§3.4).
  - Commit serialises the record once, stores those bytes in `txs.record_bytes`, hashes them, and export writes the same bytes. Nothing is ever re-serialised for hashing or export.
  - The record contains `parentHash`: the previous record's `txHash` on that branch, the branch's fork-point hash (4B), or `null` for the first record of `main`.
  - The record does **not** contain its own `txHash` or the epoch.
- **Algorithm prefix:** every hash is written `<alg>:<hex>`, so the algorithm can change later. Readers reject unknown prefixes with `UNSUPPORTED_HASH`.
- **Canonical encoding:** stored bytes are never re-encoded, so the hash needs no canonical-JSON rules. Times stay rational strings, never floats.
- **Entity ids after import:** replay doesn't mint ids, so after every import each `ids` counter (including branch ids) is raised to max(id seen in the log) + 1. Otherwise the first edit after a takeover would reuse another machine's ids.

### 3.4 Project folder layout and file formats

```
MyProject/
  finisher.project.json
  .finisher/
    lease.json                     v3 lease record (§3.5)
    lease.claim-<epoch>            v3 claim files (server id inside; §3.5)
    lease.probe-<serverId>         server-clock probe file
    segments/<branch>/<epoch:08>-<firstSeq:012>-<lastSeq:012>-<hash12>.fseg
    snapshots/<branch>/<seq:012>-<hash12>.fsnap
    compaction/<id>.json           two-phase compaction manifests (§3.9)
```

**Names and temp files**
- `<branch>` is the branch id (`main`, or `br<n>` from 4B). `<hash12>` is the first 12 hex characters of the file hash.
- Names are strict: anything under `.finisher/` that does not match these patterns is a *foreign file* (§3.7).
- Every file is written to `.~fin-<random>.tmp` in the same directory, fsynced, renamed, then the directory is fsynced (a no-op on Windows, arch §27.9).
- The lease holder deletes `.~fin-*.tmp` files older than 2×TTL. It is the only writer, so these are crash leftovers.
- A finished `.fseg`/`.fsnap` is never modified or overwritten.

**Segment (`.fseg`).** UTF-8 JSON lines.
1. **Header line:** `{"magic":"finisher-segment","formatVersion":1,"minReaderVersion":1,"projectId","branch","writerId","epoch","firstSeq","lastSeq","prevSegmentHash","opSchemaVersions":[min,max],"createdAt"}`.
   - `prevSegmentHash` is the hash of the previous segment on this branch, or `null` for the first.
   - `createdAt` is informational only and never used for ordering.
2. **Record lines:** one transaction each: `{"txId","seq","kind","schemaVersion","parentHash","author","committedAt","intent","resolved","inverse","bindings", …kind fields}`. `kind` is `edit` in 4A; 4B adds kinds.
3. **Trailer line:** `{"crc32c":"<8 hex>","hash":"blake3:<hex>"}`, both over every byte before the trailer line. The CRC is a cheap corruption check; the BLAKE3 hash is the identity used in names and `prevSegmentHash`.

**Snapshot (`.fsnap`).**
- Same three-part shape.
- Header: `{"magic":"finisher-snapshot","formatVersion","minReaderVersion","projectId","branch","seq","txHash","modelSchemaVersion","writerId","epoch"}`.
- Body: one line holding the model state.
- Snapshots are derived and can be regenerated.
- In 4A, folder snapshots are written only by compaction (§3.9).
- A snapshot may seed an import or rebuild only if its `txHash` lies on the resolved chain.

**`finisher.project.json`.**
- Adds `formatVersion`, `minReaderVersion` and `storage` (detected kind, informational).
- It is written temp-then-rename and rewritten only on settings changes.
- Windows can report it missing for about 5 s after such a rename, so openers retry for up to 15 s before reporting `NOT_A_PROJECT`.

**Export cadence**
- The writer exports after commits, debounced to 2 s, and on close or shutdown.
- A segment holds at most 1,000 records or 4 MiB; a transaction is never split.
- A record is marked `exported` only after the rename succeeds and a read-back of the file's trailer verifies.
- **Crash between the rename and the marking:** on the next import, a folder record whose `txId` **and** `txHash` already exist in the DB marks that record exported from that file and is never re-applied. The same `txId` with a different `txHash` is a fork.

### 3.5 Lease v3

This implements arch §27.9 "v3" in `fin-store::lease`, with the changes from the research:

| Parameter | NAS / network | Local disk | Sync folder |
|---|---|---|---|
| TTL / heartbeat | 10 s / 5 s | 10 s / 5 s | 10 s / 5 s (advisory) |
| Stale after (server time) | **45 s** (was 30 s) | 45 s | never automatic |
| Takeover | automatic after stale, observed by **two probes ≥ 5 s apart**, each with probe round trip ≤ TTL/4 | same | only with explicit confirmation (`--confirm`) |

**Why 45 s.** Windows documents three client-side cache windows: `FileNotFoundCacheLifetime` 5 s, `DirectoryCacheLifetime` 10 s, `FileInfoCacheLifetime` 10 s. Our measured client skew was about 10 s, and the heartbeat interval is 5 s. Those add up to about 40 s, so 30 s was too tight.

**Claims and epochs**
- To acquire, a server creates `lease.claim-<epoch>` with `create_new` (exclusive create), with its server id as the content. The exclusivity of that name is what makes epochs unique. This is the proven spike design.
- **Every** acquisition takes a fresh epoch, including automatic re-acquisition after `LEASE_LOST`.
- After acquiring, the holder deletes claim files below its own epoch. Otherwise "missing `lease.json` plus a claim" would mean "held" forever.

**Record**
- `lease.json` is a fixed 512-byte record, rewritten in place, holding: magic, version, `serverId`, `epoch`, heartbeat counter, holder description (machine name, app version), last probe server-time, and a CRC-32C.
- **Missing** `lease.json` while any claim file exists = held.
- **Unreadable or bad checksum** = held when acquiring, **lost** when fencing.
- **Probe file:**
  - each server rewrites its own probe file in place and reads its mtime, uncached, as "server now";
  - the probe must move forward between reads; if it doesn't, staleness is not decided that round.

**Fencing (writer side)**
- The epoch is checked before every segment write and after every heartbeat.
- The holder self-fences, switching to read-only, at its last successful heartbeat + 2×TTL by its monotonic clock.

**Ownership**
- A `Lease` object is owned by the open project in `fin-store`, with its own heartbeat thread.
- `fin-node` exposes `leaseStatus()` and `takeover({confirm})`.
- `finisherd` broadcasts lease state changes on the op stream as status events.

**Storage kind detection** (`fin-store::storage_kind`) returns `local | network | sync(provider)`.
- **Sync:**
  - macOS: path under `~/Library/Mobile Documents` (iCloud) or `~/Library/CloudStorage/*`;
  - Windows: the Dropbox `info.json` roots, the `OneDrive*` environment roots, and cloud-file placeholder attributes on the folder;
  - Linux: known Dropbox/Syncthing markers.
- **Network:** filesystem type (`statfs` f_type / `GetVolumeInformation` / `GetDriveType`).
- **Override:** the user can set it per project in settings, written to `finisher.project.json`.

**On sync folders**
- The lease only warns: another holder makes the project read-only with "take over?".
- Two offline writers can legitimately hold different epochs, so the epoch rule (§3.6 step 5) **never auto-resolves** there. Diverged lines are always `CHAIN_FORK`, and the user chooses.
- A sync conflict copy *of `lease.json`* puts the project read-only as a suspected split brain.

### 3.6 Opening, importing and following

**Opening** (and every poll while following):
1. Scan `.finisher/`.
2. Classify each entry:
   - **valid:** the name parses, the file is fully present, and the trailer verifies;
   - **not arrived yet:** a placeholder (Windows `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`/`RECALL_ON_OPEN`/`OFFLINE`; macOS dataless `SF_DATALESS` or legacy `.<name>.icloud` stubs), a short file, a missing trailer, or a temp file;
   - **foreign:** anything else, including conflict copies (§3.7).
3. **Not arrived:**
   - placeholders are hydrated by reading them, with progress events;
   - everything else is retried on the next poll, never reported as corruption;
   - a gap in a chain (a missing predecessor) waits too.
4. **Build each branch's chain** from the DB's last known segment hash. Then compare:
   - folder ahead → upcast and `apply_committed` the new records in order (the import);
   - DB ahead (unexported records) and this server holds the lease → export them;
   - both diverged → steps 5–7.
5. **Resolve packs and duplicates first.**
   - A segment listed in a present pack's replaced list is equivalent to the pack.
   - A segment whose records' `txHash`es all already exist adds nothing.
   - Both are skipped before the epoch rule.
   - A chain predecessor is valid if it is the hash of a present segment **or** a hash in a present pack's replaced list.
6. **Epoch rule** (network and local roots; on sync roots see §3.5). Two different segments continue from the same predecessor:
   - The higher-epoch line wins.
   - Before this server has exported anything at its epoch, its unexported DB tail counts as that higher-epoch successor. A lower-epoch segment landing after a takeover is therefore stale, not a fork.
   - The losing segment's records go to `quarantine` and are never applied. 4B offers them as a `recovered-<server>-<time>` branch.
   - Equal epochs are a `CHAIN_FORK`. That can only happen through a bug or a manual copy.
7. **Diverged local tail without the lease.** If this server has unexported records but doesn't hold the lease, and the folder has moved on, the tail goes to `quarantine`. The project stays usable read-only.
8. **Quarantine is never silent.** A non-empty quarantine is a warning in `fin project status` from 4A on, with its count and origin.

**Following.** A read-only server re-scans and imports:
- every 3 s on network roots;
- on file-system events plus a 30 s poll locally;
- on events plus a 10 s poll on sync roots.

### 3.7 Conflicts and read-only states

**Read-only reasons.** The project is read-only with one of these reasons, shown in `fin project status` and as `details.reason` on write errors (`PROJECT_READ_ONLY`, exit 4):

| Reason | Trigger | Recovery (`fix`) |
|---|---|---|
| `LEASE_HELD` | another live holder | wait, or `fin project takeover` (§3.5 rules) |
| `LEASE_LOST` | self-fenced or failed fencing check | re-acquire automatically when possible; else takeover |
| `CHAIN_FORK` | two segments on one branch claim the same predecessor | `fin project recover`: keep one side as the branch, the other becomes `recovered-*` (needs 4B; until then manual, documented) |
| `CONFLICT_COPY` | a foreign file matches a known conflict-copy pattern or parses as a Finisher file under a non-matching name | the same recovery; the copy's contents are imported to quarantine |
| `NEWER_FORMAT` | any file with `minReaderVersion` above ours | update Finisher |

**Conflict-copy patterns** (case-insensitive, matched after the strict-name check fails):
- `conflicted copy` (Dropbox);
- `.sync-conflict-` (Syncthing);
- `" <n>"` before the extension (iCloud);
- `-<hostname>` suffix (OneDrive);
- `(<n>)` and `[Conflict]` (Google Drive, unverified).

Other foreign files are listed as warnings and otherwise ignored.

### 3.8 Integrity, rebuild, backups

**Rebuild**
- **Automatic:** if `quick_check` fails, or the DB is missing while the folder has segments, the server rebuilds from the folder into a new DB and verifies it.
- **Salvage before the swap:** readable unexported records are copied from the old DB into the new DB's `quarantine`, and their count is reported.
- The new DB is then swapped in. The old file is kept as `project.fdb.corrupt-<ts>`.
- **Manual:** `fin project rebuild` forces the same.

**Verify.** `fin project verify` reports, without changing anything:
- `integrity_check`;
- every segment's CRC and hash;
- chain continuity per branch;
- epoch order;
- replaying from scratch versus the live state (§4.6 property 5);
- stored diffs versus recomputed ones.

**Backups**
- `VACUUM INTO <project-data>/backups/daily-<date>.fdb` once a day while the project is open; the last 7 are kept.
- Pre-migration backups: the last 3 are kept.
- The folder export is itself the off-machine backup when the project lives on a NAS or sync folder.

### 3.9 Compaction (history is kept)

- **Runs only on the lease holder.**
- **Trigger:** more than 200 **non-pack** segment files on a branch, or on demand (`fin project compact`).
  - With 2 s export debouncing, an active editing session reaches that in a few hours, not minutes.
  - The export debounce grows with the session: 2 s at first, then up to 30 s while edits keep coming, plus a final flush on idle or close.
- **Phase 1:**
  - write a new snapshot at the branch head;
  - write **pack segments** (each within the normal 1,000-record / 4 MiB cap) containing every record of the replaced segments, byte-for-byte as they were written, so all `txHash`/`parentHash` values still verify;
  - each pack's header lists the hashes it replaces. A pack's `prevSegmentHash` is the predecessor of the first segment it replaces. The segment after the last replaced one keeps its original `prevSegmentHash`, which resolves through the pack's replaced list (§3.6 step 5);
  - packs are never re-packed in sub-project 4;
  - write a manifest `compaction/<id>.json` listing the replaced files.
- **Phase 2:**
  - after the pack and snapshot are read back and verified, and a grace period has passed, delete the replaced files and then the manifest;
  - grace period: 10 min local/NAS, 24 h on sync folders.
- **Readers:** they treat a pack and the segments it replaces as equivalent; duplicates are skipped by `txHash`.
- **History is never discarded in sub-project 4.** Squashing archived branches is deferred (§4.5).

### 3.10 RPC and `fin`

**New RPCs:**
- `project.status`;
- `project.takeover {confirm}`;
- `project.verify`;
- `project.rebuild`;
- `project.compact`;
- `project.recover` (stub in 4A that reports the manual procedure; real in 4B);
- `tx.get {txId}`, which completes the SP2 idempotency follow-up.

**Op stream:** `ops.subscribe` adds `status` events (lease, read-only reason, import progress).

**`fin` commands:** `fin project status|takeover|verify|rebuild|compact`. Output follows the existing one-JSON-document rule and exit codes.

**New error codes:**
- `DB_TOO_NEW`
- `NOT_A_PROJECT_DB`
- `PROJECT_READ_ONLY`
- `UNSUPPORTED_HASH`
- `SEGMENT_CORRUPT` (verify only; import treats corruption as "not arrived" until the file stops changing for 60 s, then reports it)
- `LEASE_TAKEOVER_REFUSED`

### 3.11 SP2 follow-ups that land in 4A

- `project create`: idempotent retry, reject empty `--name`, atomic create (write the DB and JSON under temp names, then rename; no orphans).
- `ops.subscribe` with `fromRev` above head → `BASE_REV_AHEAD`.
- Typecheck gate (`tsc --noEmit`) for all TS packages, tests included.
- Idempotency:
  - `tx.get`;
  - a stored hash of the arguments, so the same key with a different op or arguments → `IDEM_KEY_REUSED`.
- Rational parsing: reject non-canonical input (`2/4`, `-0`); `details.rev` stays numeric.
- `fin-node`:
  - Linux loader libc detection;
  - libuv pool use under many projects;
  - finisherd hubs removed when their last subscriber and handle close.
- Generate the `Probe` TS type from `fin-probe`; add a MOV fixture test.

### 3.12 Tests and acceptance (4A)

**Unit and property tests**
- **Classification:** valid, not-arrived, foreign and conflict-copy, for every pattern in §3.7.
- **Chain building and the epoch rule:** randomised sets of segments, including duplicates, gaps, forks, stale epochs, packs during their grace window, and a stale segment landing after a takeover but before the new holder's first export.
- **Export crash** between rename and marking: no fork, no double apply.
- **Id counters** after import: no reuse after a takeover (also in the cross-host run).
- **Migrations:**
  - SP2 database fixture → current;
  - newer-DB refusal;
  - the backup is created before migrating.
- **Upcaster golden tests.**

**Fault injection** (in-process, every OS):
- kill during export at each step (temp written, renamed, DB not yet marked);
- truncated files, bad CRC, placeholders (simulated attributes on Windows, `SF_DATALESS` via `chflags` on macOS where permitted, otherwise mocked);
- DB deleted or corrupted, then rebuilt.

**Cross-host on the NAS** (macOS + Windows + Linux VM; harness from the S10 spike, ported into `tests/storage-xhost/`, not shipped):
1. Lease races: 100 per host pair. Required: 0 double holders, 0 early takeovers, 0 applied stale writes.
2. Network loss (firewall block on the Linux VM; Wi-Fi off on the Mac) with writes in flight. Required: the stale writer's segments are quarantined, never applied.
3. Real clock skew, if any host is naturally off by ≥ 5 s at run time. Otherwise emulated, with the run noted as such.
4. Follow mode: edits from host A appear on read-only host B within 3 s plus the cache window.

**Sync folders (macOS, owner's accounts; blocked on the owner if not installed):**
- temp-then-rename upload on iCloud Drive and OneDrive/Dropbox;
- conflict-copy naming per provider;
- placeholder hydration.

**Acceptance:**
- all of the above pass, or are recorded as blocked with a reason;
- an SP2-era project opens after migration with identical state;
- `pnpm e2e` gains a scenario: edit, export, delete the DB, rebuild, identical snapshot.

---

## 4. Stage 4B — history

### 4.1 Branches

- **Identity:** a branch has an id (`main`, then `br<n>` from the shared id counter, §3.3), a display name (unique among active branches), a parent branch, `forkSeq` and `forkHash`, `createdBy`, and a state (`active | archived | merged`).
- **Numbering:** a branch's `seq` continues from `forkSeq + 1`, so seqs up to the fork mean the same on both. The first record's `parentHash` is `forkHash`.
- **Storage: all branch state is derived from the log, so it syncs.** The local branch table is only an index.
  - **Creation:** a `branch` record in the **parent** branch's log (no ops).
  - **Rename and archive:** `branchSet` records on the parent's log.
  - **`merged`:** derived from `merge` records (§4.4).
- **Live sync:** `ops.subscribe` takes `branch` (it already does).
- **CLI:** `fin branch new <name> [--from <branch>@<seq|checkpoint>]`, `fin branch ls`, `fin branch archive <name>`. A global `--branch <name>` (default `main`) applies to every command.

### 4.2 Record kinds

Added to the segment format; the format version is bumped via the 4A migration path:

| kind | Fields | Ops |
|---|---|---|
| `edit` | — | yes |
| `undo` | `undoes: txId` | inverse of target, rebased |
| `redo` | `redoes: txId` (an undo) | inverse of that undo, rebased |
| `merge` | `mergeId`, `from: branch`, `origin: txId` | the origin's intent re-run on target |
| `cherryPick` | `origin: txId`, `from: branch` | same |
| `revert` | `reverts: mergeId` | inverses of the merge group, newest first |
| `checkpoint` | `name` | none |
| `branch` | `branchId`, `name` | none |
| `branchSet` | `branchId`, `name?`, `archived?` | none |

**Unknown kinds.** 4B bumps `formatVersion` and `minReaderVersion` of segment files to 2. A reader that meets an unknown `kind` in any file treats it as `NEWER_FORMAT`.

### 4.3 Rebase engine (stronger)

- **Rebase rule:** an edit is rebased by transforming the positions its intent refers to through the **per-track shift maps** of the edits committed since its base, then re-running it with full policy checks.
- **Shift maps** are exact: every structural op records how it moved later items (ripple by Δ from t; overwrite/lift: none). The sub-project 2 case "a later position on a track our trim/delete rippled → conflict" is removed.
- **Conflicts are judged per track and per entity:**
  - an edit on track B never conflicts with edits on track A, **unless** they touch the same link group or one of them moves an item between the two tracks. The unit is the set of tracks the edit's link groups span;
  - an edit of entity X conflicts only with intervening edits that removed X, or changed the same edge or parameter of X.
- **References to removed entities:** an intent referring to an entity removed in between → `CONFLICT` with reason `REMOVED`. This includes a stale insert of an asset that was removed.
- **Placeholders:** `clip.restore` and the internal inverse ops support `$placeholders`.
- **Conflict detail:** each conflict is `{txId, opIndex, reason, againstTx, fix}`. Merge returns the full list, not just the first.

### 4.4 Merge, cherry-pick, revert

- **`fin merge <branch> [--into main] [--skip-conflicting] [--dry-run]`:**
  - **candidates:** the source branch's content records (`edit`, `undo`, `redo`, `cherryPick`, `revert`) whose `txId` is not already the `origin` of a `merge` record on the target. Records skipped earlier are therefore offered again;
  - candidates are re-run in order on the target head;
  - **all-or-nothing:** one DB transaction, or nothing plus the full conflict list. Export happens as usual; followers apply any prefix safely, so no folder-level atomicity is needed;
  - `--skip-conflicting` merges the rest and leaves the skipped records on the branch;
  - the source branch becomes `merged` when nothing is left.
- **`fin cherry-pick <txId> [--into <branch>]`:** the same machinery for one record.
- **`fin revert <mergeId>`:** one `revert` record. It may conflict, like undo.

### 4.5 Undo, redo, checkpoints, `try`, archiving

**Undo and redo**
- **The stacks are derived from the log, not stored.**
  - For (author, branch), simulate the branch's records **oldest-first**, keeping an undo stack and a redo stack of *units*.
  - A unit is one `edit`, `cherryPick` or `revert` record, one whole `merge` group (by `mergeId`), or one `undo` record (including an `undo --to`).
  - The author's content record, or the first record of a merge group: push the unit on the undo stack and clear the redo stack.
  - `undo` (with `undoes: txId | mergeId`): pop that unit from the undo stack and push the undo record on the redo stack.
  - `redo` (with `redoes: <undo txId>`): pop it from the redo stack and push the original unit back on the undo stack.
  - `checkpoint`, `branch` and `branchSet` records are skipped.
  - Records copied **to another branch** by a merge are units of *that* branch's merging author on the target. On the source, the stack is dropped (arch §6.4). Merged edits are undone on the target as one merge unit or with `revert`, never individually.
- **One redo reverses one undo record,** so it also reverses a whole `undo --to`.
- **`fin undo`:** takes the unit's stored **inverse** (not its intent), rebases it across the records since the unit with full policy checks, then commits. It may `CONFLICT`.
- **`fin redo`:** the same for the undo it cancels.
- **`fin undo --to <checkpoint>`:** one `undo` record holding the inverses of all of the author's later records on the branch, newest first, all-or-nothing.

**Checkpoints:** `fin checkpoint <name>` appends a `checkpoint` record. Names are unique per branch.

**`try`**
- `fin try [name]` creates `try-<n>` from the current head and switches the CLI's default branch for this server-data to it (stored in `fin` config).
- `fin try keep` merges it into its parent.
- `fin try drop` archives it.

**Recovered branches.** Quarantined records (§3.6) are offered via `fin project recover`. They become `recovered-<server>-<time>`, a branch forked at their last common hash.

**Archiving.**
- Archived branches are hidden from `fin branch ls` (unless `--all`) and are kept.
- Squashing them is **deferred out of sub-project 4**: nothing before sub-project 6 needs it, and it needs its own two-phase delete.

### 4.6 Property tests (proptest, in `cargo test`, < 30 s total)

1. `apply` then `inverse` returns the exact prior state, for every op kind.
2. Rebasing onto an empty interval is the identity.
3. **Rebase coherence:** if both stale-rebased and fresh-at-head succeed, they produce the same state. This is checked against a reference model of positions on several tracks.
4. **Undo/redo:** with 2–4 interleaved authors, after rebase and after cherry-pick, the result matches a reference model.
5. **Replay:** replaying any generated log from scratch (via `apply_committed`) equals the live state, and survives an export → import round trip.
6. **Validation soundness:** for any random op, `apply_tx` never panics, and success implies the model invariants hold (no overlap, minimum duration, references resolve).

### 4.7 Acceptance (4B)

- All property tests pass.
- Every command above has `fin` contract tests.
- `pnpm e2e` gains a scenario: branch, edit on both sides, merge with one conflict (`--skip-conflicting`), undo the merge, verify state.
- Cross-host: a branch made on host A merges on host B after takeover.

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
| `fin-core` | `upgrade/`, `apply_committed` | `branch/`, rebase rewrite, undo derivation | `model` growth, `ops/*`, `params/`, `eval/`, `layout/` (via `TextMeasurer`), `captions/` |
| `fin-store` | `migrations/`, `segment/`, `snapshot_file/`, `lease/`, `storage_kind/`, `import/`, `compact/`, `backup/` | branch table, record kinds, quarantine → recovered | — |
| `fin-text` (new) | — | — | shaping and measurement |
| `fin-probe` | `Probe` TS type, MOV fixture | — | image/SVG/Lottie/font/LUT/audio probes |
| `fin-node` | lease/status/verify/rebuild bindings | branch/merge/undo bindings | eval bindings |
| `@finisher/contracts`, `finisherd` | new RPCs, status events | branch RPCs | edit RPCs (generated from the registry where possible) |
| `fin` | `project *` commands | `branch`, `merge`, `cherry-pick`, `revert`, `undo`, `redo`, `checkpoint`, `try` | edit commands, `view`, `status`, `schema param` |

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
| Sync providers break temp-then-rename | Sync-folder test rows; fall back to unique final names written directly (always valid names, verified by trailer) |
| The rebase rewrite introduces subtle position bugs | Property 3 with a multi-track reference model; keep SP2 rebase tests as regression |
| The settings registry is too generic for good errors | Every `ParamSpec` carries unit, range and a `fix` template; contract tests on error text |
| `fin-text` adds binary size to `fin` and the WASM build | Fonts are loaded from the bundled set at runtime, not embedded in `fin`; size measured in CI |
| 4C is large | 4C's plan is split into batches (model + structural edits; params + eval; captions + text; view/status), each green before the next |
