# Sub-project 5D: The library indexer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics that lose data or wake disks if wrong (per-OS enumeration, placeholder safety, safe opens, volume identity and offline rules, the catalog diff, the scheduler and its polling, the stage framework, validation), `sonnet` for plumbing (the probe refactor, the worker family, the control-store chain, search, RPCs, `fin`, the console, e2e, docs).

**Revision 1** (2026-11-07).

**Goal:** spec §7 (5D), on top of 5C-1's engine. After this plan:
- **`fin-library`** (new crate) enumerates a root **without opening a file**: Linux `getdents64` + `statx`, macOS `getattrlistbulk` with dataless materialisation off, Windows `FILE_ID_EXTD_DIR_INFO`; it classifies placeholders (macOS dataless files and directories, iCloud stubs, Windows recall attributes and cloud reparse tags, Nextcloud/ownCloud suffix files), reads volume identity and mount state, opens files for probing only through one safe-open path, and has a fake filesystem for tests;
- **`fin-workers library`** runs `library.scan`, `library.probe` and `library.hash` jobs on the shared engine (spec §6.0: 5D contributes only this family);
- **`finisherd`** keeps libraries in `control.db` (the `libraries` chain) and one **catalog** per library (`<server-data>/libraries/<id>/catalog.db`), written only by an **indexer `worker_thread`** and read by a **reader `worker_thread`**; it polls NAS roots on demand (60 s in use, 6 h otherwise, a freshness pass before answers), watches local roots as a hint, never marks files missing for an unavailable root, runs the **stage framework** that sub-projects 6 and 8 register into, and answers FTS5 name search and fingerprint lookup;
- `library.*` RPCs with 5A's scopes, `fin library …`, `fin search`, the console's `library add|rm|set`, and the `library.scan` diagnostics kind.

**Not in this plan:** proxy, PCM, analyzer and embedding stages (sub-projects 6, 8: they call `registerStage`); semantic and transcript search (8E; its catalog tables come through `registerCatalogChain`); per-library reach lists (v1 reach is the `library.read` scope, Decision 41); device principals (5B); remote ingest; full hashes stored in the project model (a TASKS follow-up for 6/11).

**Architecture:**
- **Rust:** `crates/fin-library` (new: types, names, placeholders, excludes, volumes, `Source` per OS, walker, safe open, probe and hash entry points, fake filesystem); `crates/fin-probe` (a reader-generic probe, Task 5); `crates/fin-workers/src/library/` (the family); `crates/fin-node/src/library/` (napi: root canonicalisation, identity, per-root probe threads); `crates/fin-core/src/events/` (`library.scan`, job kind names).
- **TypeScript:** `packages/contracts/src/library.ts`; `apps/finisherd/src/control/migrations/libraries/0001.ts`; `apps/finisherd/src/library/` (registry, service, poll planner, usage, kinds, stages, locate, rpc, `catalog/` with chains, schema, indexer and reader threads, diff, search); `apps/finisherd/src/console/commands/library.ts`; `crates/fin/src/commands/{library.rs, search.rs}`.

**Tech Stack:** Rust 1.99.1, edition 2024; `libc` 0.2 (MIT OR Apache-2.0, in the tree); `windows-sys` 0.61 (MIT OR Apache-2.0, in the tree) with `Win32_Foundation`, `Win32_Storage_FileSystem`, `Win32_System_IO`, `Win32_Security` and, **dev-only**, `Win32_Storage_CloudFilters` (Task 3's real placeholder test); **`unicode-normalization` 0.1** (MIT OR Apache-2.0, new: NFC keys); `blake3` 1, `serde`, `schemars` 1 (in the tree). Node 24 APIs only (spec §1.6): `node:sqlite` with FTS5 (`contentless_delete` needs SQLite ≥ 3.43; Node 24.22 ships 3.53), `worker_threads`, `fs.watch`; Effect 4.0.6; vitest; `fast-check` 4 (dev, already pinned by 5C-1). **No other dependency.** Rejected: `notify` (Node's watcher suffices for a hint), `globset` (a 100-line matcher, Decision 15), `walkdir`/`jwalk`/`ignore` (they `stat` through `std`, which opens files on Windows and follows the wrong rules for placeholders), `rusqlite` in the indexer (catalogs stay on `node:sqlite` like `control.db`).

**Spec:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` revision 4: §3.1 (chains, budgets), §3.3 (`library.*` audit kinds), §3.4 (`library.scan`), §4.5–4.7 (scopes, human-only, console), §6.0–6.9 (the engine 5D runs on), §7, §10, §11 Decisions 25–27, §12. Arch §6.7 (libraries), §20 (relinking, per-server root mapping). Sub-project 6 spec §3.2–3.3, §6.3 (shallow probe in `library.probe`), §7.1 (`proxy540` as a library stage); sub-project 8 spec §8 (analyzer stages, artifacts keyed by fingerprint), §16.2 (what 8 needs from 5D). Plans: 5A (Tasks 1, 3–6, 8), 5C-1 (frozen interfaces), 6A (Decision 5: library-root mapping comes with 5D).

**Branch:** `sp5d-library-indexer`, stacked on `sp5c1-job-engine`; rebased onto 5A's Task 6 and onto 5C-2 when batch C starts.

**What this plan uses (frozen elsewhere):**
- **5A:** `ControlStore`, `registerChain`, `Audit`, `ensurePrivateDir` (Tasks 1–2, merged); `Principal`, `RPC_SCOPES`, `withChain`, resolvers, `defaultScopes`, protocol 2 (Tasks 3–6); the console's `Prompt` and challenge flow (Task 8). The scopes `library.read`, `library.scan`, `library.admin` (human) and `jobs.submit` already exist in `SCOPES`.
- **5C-1:** `KindSpec`, `FamilySpec` (`onCall`), `BatchSpec`, `PrepareCtx`/`SucceededCtx`, `resourceHealthy`, `JobEngine` (`submit`, `get`, `cancel`, `definePool`, `onDone`, `events`), `fin_jobs::worker::{Family, JobCtx}` (`call`, `begin`/`item`, `progress`, `checkpoint`, `cancel_token`), `ReadBudget`, `JobError`. **5D needs no change to any frozen 5C-1 interface** (bandwidth travels in params, Decision 31; pools come from `definePool`).
- **5C-2:** `jobs.wait` and `fin wait` (only `fin library scan`'s e2e, Task 15). D4's `Diagnostics.journal` (on the base branch).
- **fp1** is `fin_probe::fingerprint` (it lives in `fin-probe`, not `fin-store`); from `fin-store` 5D reuses `storage_kind::detect`, `conflict_names::classify` and the placeholder knowledge of `fsio::placeholder`.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code; `pnpm gates` runs cargo-deny and the JS licence gate over what each task adds. Test corpora are generated, never downloaded.
- **Primitive ops live only in `fin-core`.** This plan adds none; `fin-core` gains only an event payload and names.
- **TypeScript model types are generated, never hand-written.** Rust → TS (`pnpm --filter @finisher/core-types generate`, drift gate `scripts/check-generated-types.mjs`): `ScanEntry`, `DirDone`, `ScanBatch`, `ProbeOutcome`, `VolumeIdentity`, `RootState`, `Placeholder`, the three kinds' params and results, `LibraryScan` (journal). TS-owned records (`LibraryView`, RPC payloads, catalog rows) are Effect Schemas in `@finisher/contracts`.
- **Read-only roots:** nothing is ever created, written, renamed, locked or deleted under a library root, by any process, in any build (Decision 26 makes this override spec §7.2's macOS listing refresh). Tests write their trees from the test process only.
- **No file under a root is opened except through `fin_library::open::open_for_read`** (Decision 22); `scripts/check-library-open.mjs` (Task 6) refuses `File::open`, `OpenOptions`, `std::fs::read*`, `std::fs::metadata` (Windows opens to stat), `CreateFileW` and `open(2)` in `crates/fin-library/src/` and `crates/fin-workers/src/library/` outside `open/` and `source/`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit: `crates/fin-core/src/events/names.rs` 504 (RPC names: if it passes 550, move `RPC_METHODS` into `names_rpc.rs` first), `kinds.rs` 370, `crates/fin/src/registry.rs` 377. Each OS's enumeration is its own module; a file passing 550 lines is split before the commit.
- Errors are `{code, message, details, fix?}`; new codes go into `crates/fin/src/output.rs` and the contracts' list. Human-only fixes address the user and contain no command (spec §3.5).
- **Privacy:** library names, roots, aliases, file names and paths are user content: never in the journal, the audit log, logs at info level, or errors' `message` (paths go in `details.path` of local errors only).
- **The repo is public:** no host names, IPs, user names or share names in tracked files; network tests read their share from the environment (`FIN_TEST_SHARE`, as 4A).

**Every commit is green** on macOS, Windows 11 and Linux. Tests that need an OS facility that may be absent (user namespaces for bind mounts, `hdiutil`) skip with a notice naming the facility; nothing else skips.

**Testing and process**
- Tests never use the network, except the opt-in share tests (`FIN_TEST_SHARE` set). Timers come from injected clocks (Rust `Timing`, Effect `TestClock`); worker timeouts are scaled through test-hooks params, never slept.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`; Tasks 13–16 also `rtk pnpm e2e`.
- Commits: Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Shell commands prefixed with `rtk`, after `source .superpowers/env.sh`.
- **Tracker:** Task 1 adds "Sub-project 5D: library indexer" to `TASKS.md`, one line per task; each task ticks its line with as-built notes in its own commit.
- **Batches and order** (against 5A, 5C-1, 5C-2):
  - **A, can start now** (they need nothing beyond the base and 5A Tasks 1–2, already on the stack): Task 1 → Tasks 2 and 3 in parallel → Task 4; Task 5 (best after 6A-1 merges, Decision 30); Task 7 after Task 1 (generated types); Task 8; Task 12's query core after Task 7.
  - **B, after 5C-1 Tasks 1–2** (merged) for Task 6, and **after 5C-1 Tasks 7–10** (kinds, engine run loop, isolation, `resourceHealthy`) for Task 9 → Task 10 → Task 11.
  - **C, after 5A Tasks 3–6** (principals, chain, scopes, protocol 2): Task 13; **after 5A Task 8** (console) and **5C-2** (`fin wait`): Task 14 → Task 15.
  - **D:** Task 16 → Task 17.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Crates, processes and threads

1. **`fin-library`** holds everything that touches a root: types, names, placeholders, excludes, volumes, the per-OS `Source`, the walker, safe opens, probe and hash entry points, and (feature `test-hooks`) the fake filesystem. It creates no process. `fin-workers` (the `library` family) and `fin-node` (root probes at add and while offline) link it; `finisherd` never touches a root from JS (no `fs.stat` of library paths on the main thread or the libuv pool; the one exception is `fs.watch`, Decision 25).
2. **Threads in `finisherd`:** the main thread holds the `libraries` table and the scheduler and never runs a catalog statement; **`library-indexer`** (one `worker_thread`) is the only writer of every catalog and hosts the watchers; **`library-reader`** (one `worker_thread`, read-only connections, WAL) serves search, listings and lookups, so answers never wait behind a write. Messages are typed requests over `MessagePort`s with Effect wrappers; each indexer transaction is ≤ 2,000 rows (≤ 50 ms) so libraries interleave.
3. **Root probes** (`fin-node` `library::probeRoot`) run on **one dedicated OS thread per root**, never on libuv's pool: a stat that hangs on a dead NFS hard mount blocks only that thread. While one probe of a root is outstanding no other is issued; a probe past its deadline answers `timeout` and the thread is left to finish (counted in `status().stuckProbes`).

### Enumeration (never opens a file)

4. **`Source` trait** (`fin-library::source`): `open_dir(rel) -> DirHandle` (a directory handle opened without following links), `read_dir(&DirHandle) -> Vec<RawEntry>` (name bytes, type, size, mtime ns, file id, placeholder attrs, device), `stat_root() -> RootStat`, `volume(&DirHandle) -> VolumeIdentity`. Implementations: `linux`, `macos`, `windows`, and `fake` (test-hooks).
5. **Linux:** `openat(O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)` per directory, entries by `fdopendir`/`readdir` (`getdents64`; it neither opens nor stats an entry), then `statx(dirfd, name, AT_SYMLINK_NOFOLLOW, STATX_TYPE | STATX_SIZE | STATX_MTIME | STATX_INO | STATX_MNT_ID)` per entry; symlinks are recorded as `skipped` and never followed. **Mount roots** come from `STATX_ATTR_MOUNT_ROOT` (kernel ≥ 5.8; the floor is 6.2), which also sees bind mounts that `st_dev` cannot.
6. **macOS:** `open(O_RDONLY | O_DIRECTORY | O_NOFOLLOW)` and `getattrlistbulk` with `ATTR_CMN_RETURNED_ATTRS | NAME | ERROR | OBJTYPE | MODTIME | FLAGS | FILEID | DEVID` and `ATTR_FILE_DATALENGTH`, `FSOPT_PACK_INVAL_ATTRS`, a 256 KiB buffer. At family start the worker calls `setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_PROCESS, IOPOL_MATERIALIZE_DATALESS_FILES_OFF)` (declared locally if `libc` lacks it), so any accidental access fails `EDEADLK` instead of downloading; only a probe job of a `hydrate: true` library turns it on, for its own (cold) process. Mount roots: `st_dev` differs from the parent's, or `MNT_ROOTFS`/`statfs` `f_mntonname` equals the path.
7. **Windows:** `CreateFileW(dir, FILE_LIST_DIRECTORY | SYNCHRONIZE, share all, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)` with `\\?\` paths, then `GetFileInformationByHandleEx(FileIdExtdDirectoryRestartInfo` then `FileIdExtdDirectoryInfo`, 64 KiB buffer): size (`EndOfFile`), `LastWriteTime` (100 ns → ns since the Unix epoch), attributes, `ReparsePointTag` and the 128-bit `FileId` per entry, from the directory handle alone. **Fallback chain**, recorded per root in `VolumeIdentity.enumeration`: `FileIdExtdDirectoryInfo` → `FileIdBothDirectoryInfo` (64-bit ids; older SMB servers) → `FileFullDirectoryInfo` (no ids), chosen at the first `ERROR_INVALID_PARAMETER`/`ERROR_NOT_SUPPORTED`. Reparse points that are not cloud tags (junctions, symlinks, mount points inside the root) are `skipped`.
8. **Parallelism** (spec §7.2): directory reads run on a pool of 32 threads for network and sync roots, 64 for local ones, breadth-first by depth then name, so a scan's order is deterministic and resumable (Decision 21).
9. **Names:** raw bytes kept (`name BLOB`; Windows UTF-16 stored as WTF-8); `nameNfc` (NFC, `unicode-normalization`; lossy names get `U+FFFD` and `nameLossy: true`); **`relKey`** = the NFC relative path with `/` separators, lower-cased (`str::to_lowercase`) when the root is case-insensitive. Case sensitivity per root, without writing: macOS `pathconf(_PC_CASE_SENSITIVE)`, Windows the volume's `FILE_CASE_SENSITIVE_SEARCH` flag, Linux insensitive for `vfat`/`exfat`/`ntfs3` and `cifs` mounted `nocase`, else sensitive. Paths given by callers are matched through the same key.

### Placeholders

10. **Classification** (`fin-library::placeholder`, pure over `RawEntry`), result `Placeholder = dataless | icloudStub | nextcloud | owncloud | recallOnOpen | recallOnData | offline | unpinned`:
    - macOS: `SF_DATALESS` in `ATTR_CMN_FLAGS` on files **and directories**; `.<name>.icloud` stubs name `<name>`;
    - Windows: `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `RECALL_ON_OPEN`, `OFFLINE`, an `IO_REPARSE_TAG_CLOUD*` tag, and `UNPINNED` (an online-only file even when the other bits are clear); `PINNED` with none of the recall bits is a hydrated file;
    - every OS: `<name>.nextcloud` / `<name>.owncloud` stand for `<name>`; when both the stub and `<name>` exist, the real file wins and the stub is ignored; a stub is never probed.
11. **Placeholders are catalogued from metadata only** (state `cloudOnly`, size from the entry). A placeholder **directory** (dataless, or a Windows directory with a recall bit) is recorded `cloudOnly` and **not descended**; files catalogued under it earlier keep their rows and become `cloudOnly`, never `missing`.
12. **Linux FUSE sync clients without markers** (spec §7.3): roots whose storage is `sync` on Linux get a default `bandwidthMiBs` of 4 (human-only to change), so probes read them only within that budget.

### Volumes, mounts and offline

13. **`VolumeIdentity {devId, fsId?, mountRoot, caseSensitive, enumeration, remote, volumeKey}`** at add: `st_dev`; `fsId` = `statfs.f_fsid` (macOS, Linux; `null` when 0) or the 64-bit volume serial from `FileIdInfo` (Windows); `mountRoot` per Decisions 5–7 (Windows: a drive root, a UNC share root, or a volume serial different from the parent's); `volumeKey` = BLAKE3-16 of `(fsId or devId, the share's server and share for UNC/SMB paths)`.
14. **`RootState`** (fin-node probe and the worker's first stat) = `online | unreachable | unmounted | volumeChanged | empty | timeout`. **Unmounted:** the root was a mount root and is not now (Linux: no `STATX_ATTR_MOUNT_ROOT`; elsewhere `st_dev` equals the parent's); the empty mount-point directory is never scanned. **`volumeChanged`:** `fsId` differs from the stored one (a different share at the same path): offline with `ROOT_VOLUME_CHANGED` until the owner accepts the new volume (`library.set {acceptVolume: true}`, human-only); `devId` alone changes across reboots and is not checked. **Nested mount points** below a root are tracked per directory (`dirs.mount`); one that unmounts makes its subtree `offline` (`ROOT_UNMOUNTED {dir}`), never `missing`.
15. **Excludes** (spec §7.1 defaults plus the library's `exclude`/`include` globs over `relKey`): dot-files and directories, `Thumbs.db`, `desktop.ini`, `.DS_Store`, `@eaDir`, `#recycle`, `$RECYCLE.BIN`, `System Volume Information`, `.nfs*`, `.fuse_hidden*`, sync temp files, conflict copies per `fin_store::conflict_names::classify`, and inside a directory holding `finisher.project.json` everything but `sources/`. Globs: `*`, `?`, `**`, `[…]`, case per the root; an own matcher (≤ 150 lines).
16. **Offline rules** (spec §7.2, arch §6.7): a root that is unreachable, unmounted, volume-changed or timed out, and a full scan whose root listing is empty while the catalog holds present files (`ROOT_EMPTY_SUSPECT`), put the library `offline` with catalog and stage states intact; **no file is ever marked missing in an offline library, and no file is marked missing unless its parent directory was listed completely in this scan** (Decision 19). Offline libraries are re-probed every 60 s by fin-node (Decision 3); a return to `online` triggers a full scan (`trigger: mount`). Their files stay listed and searchable with `offline: true`.
17. **Spin-up tolerance:** the first stat of a root waits up to **60 s** (fin-node probe and worker alike) before `timeout`; per-directory reads and per-entry stats wait up to **30 s**, after which that directory is `unreadable` (`code: TIMEOUT`) and the scan goes on; a stuck read thread is left behind and the cold worker exits at job end (5C-1 abandons it if it cannot). A scan that is slow but progressing is never offline; scans report `progress` per directory, so 5C-1's `stallS` (10 minutes for scans) fires only for a scan that is truly stuck.

### Catalogs and the diff

18. **Catalog schema** (`catalog` chain step 1, frozen once merged; `meta`/`migrations` as in `control.db`; `registerCatalogChain` lets 8C add its transcripts chain):
    ```sql
    CREATE TABLE dirs (id INTEGER PRIMARY KEY, parent INTEGER, name BLOB NOT NULL, nameNfc TEXT NOT NULL,
      relKey TEXT NOT NULL UNIQUE, mtimeNs INTEGER, fileId BLOB, devId TEXT, mount INTEGER NOT NULL DEFAULT 0,
      placeholder TEXT, state TEXT NOT NULL, code TEXT, gen INTEGER NOT NULL, listedGen INTEGER, lastSeen INTEGER NOT NULL);
    CREATE TABLE files (id INTEGER PRIMARY KEY, dirId INTEGER NOT NULL REFERENCES dirs (id), name BLOB NOT NULL,
      nameNfc TEXT NOT NULL, nameLossy INTEGER NOT NULL DEFAULT 0, relKey TEXT NOT NULL UNIQUE, ext TEXT,
      size INTEGER NOT NULL, mtimeNs INTEGER NOT NULL, fileId BLOB, placeholder TEXT, fp TEXT, fullHash TEXT,
      kind TEXT, probe TEXT, probeVersion INTEGER, durationMs INTEGER, width INTEGER, height INTEGER,
      state TEXT NOT NULL, code TEXT, gen INTEGER NOT NULL, firstSeen INTEGER NOT NULL, lastSeen INTEGER NOT NULL,
      lastChange INTEGER NOT NULL, missingSince INTEGER);
    CREATE INDEX files_dir ON files (dirId, nameNfc);
    CREATE INDEX files_fp ON files (fp) WHERE fp IS NOT NULL;
    CREATE INDEX files_hash ON files (fullHash) WHERE fullHash IS NOT NULL;
    CREATE INDEX files_fileid ON files (fileId) WHERE fileId IS NOT NULL;
    CREATE INDEX files_pending ON files (state, probeVersion);
    CREATE INDEX files_media ON files (kind, durationMs, width, height);
    CREATE INDEX files_size ON files (size);
    CREATE TABLE stages (fp TEXT NOT NULL, stage TEXT NOT NULL, version TEXT NOT NULL, state TEXT NOT NULL,
      code TEXT, jobId TEXT, attempts INTEGER NOT NULL DEFAULT 0, at INTEGER NOT NULL, PRIMARY KEY (fp, stage)) WITHOUT ROWID;
    CREATE INDEX stages_state ON stages (stage, state, at);
    CREATE VIRTUAL TABLE names USING fts5 (name, dir, content = '', contentless_delete = 1,
      tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');          -- rowid = files.id
    CREATE TABLE scans (id INTEGER PRIMARY KEY, jobId TEXT, mode TEXT NOT NULL, trigger TEXT NOT NULL, gen INTEGER NOT NULL,
      startedAt INTEGER NOT NULL, finishedAt INTEGER, outcome TEXT, code TEXT, counts TEXT);
    ```
    File `state`: `present | missing | cloudOnly | unreadable | probeFailed | skipped | offline`; dir `state`: `present | missing | cloudOnly | unreadable | excluded | offline`. `meta.libraryId` must equal the directory's id (a catalog copied from another library is moved aside). `PRAGMA quick_check` at open; a damaged catalog is moved to `catalog.db.corrupt-<ulid>` and rebuilt by a full scan (stage results survive in the caches, keyed by fingerprint). A chain whose `minReaderVersion` is above ours puts the library offline with `CATALOG_TOO_NEW`.
19. **The diff** (indexer, per `ScanBatch`, one transaction):
    - an unknown `relKey` whose `fileId` matches a row not seen in this generation, with equal size and mtime, is a **rename** (the row's path changes, its id, fingerprint and history stay); otherwise a new `present` row, probe pending;
    - a known `relKey` with a different size, mtime or file id is **changed** (`fp`, `probe`, `probeVersion` cleared; `lastChange` set);
    - a `DirDone {rel, complete: true}` marks **missing** every row of that directory not seen in this generation, and every subdirectory absent from the listing with its whole subtree; `complete: false` (an error, a timeout, a placeholder directory) marks nothing;
    - after a probe, a new row whose fingerprint equals a **missing** row's in the same library is a **move**: the missing row takes the new path and the new row is dropped;
    - a missing file that reappears with its old size and mtime is `present` again with its fingerprint;
    - rows missing for **90 days** are deleted in batches of ≤ 500 (their stage rows when no other row has the fingerprint).
20. **Batches over the envelope** (5C-1 Decisions 4, 5a): the scan worker writes up to 4,096 entries (≤ 8 MiB) per NDJSON file in its job's `out/`, then `call library.batch {file, seq}`; the `library` family's `onCall` reads the file with a size cap, hands it to the indexer and replies after the commit, so the worker never runs ahead of the catalog. Bulk never travels in the envelope; the file is the worker's (less trusted) output and is parsed defensively.
21. **Generations and resume:** the service allocates a generation per scan (`params.gen`); a scan is `resumable`. After a restart its `prepare` writes `in/dirs.ndjson` with each known directory's `relKey`, `mtimeNs` and whether it was listed in this generation; the worker skips listings already done but still descends through them.

### Safe opens

22. **Safe opens** (`open_for_read(root, rel, expect)`, the only file open under a root): (1) re-check the entry without opening it (Linux `statx`; macOS `getattrlist` with `FSOPT_NOFOLLOW`; Windows `FindFirstFileExW(FindExInfoBasic)` on the exact path, a directory read) and refuse a placeholder with `FILE_CLOUD_ONLY` unless `hydrate`; (2) open without following any link (Linux `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS)` from the root fd; macOS `O_NOFOLLOW_ANY`; Windows `FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN`, then `GetFinalPathNameByHandleW` must lie under the root and `FileAttributeTagInfo` must show no reparse tag); (3) compare the handle's identity, size and mtime with `expect`; a mismatch is `changed` (its directory is rescanned), not a probe. The residual window between (1) and (2), where a provider could evict a file, is documented.

### Polling and watching

23. **Network roots, `poll: demand`** (spec §7.2, spec §11 Decision 27): a `library.scan {mode: poll}` job re-stats every known directory and re-lists those whose mtime changed, **every 60 s while the library is in use** (a project that references it is open, a `library.watch` stream is open, or it was searched, listed or scanned in the last 10 minutes) and **every 6 hours otherwise**. Before a search or listing of a network or sync library whose last poll is older than 5 minutes, the service submits an `interactive` poll of the directories the answer touches (a listing: that directory; a search: all) and waits ≤ 5 s; past that the answer says `stale: true` and the poll goes on. The owner may set a fixed interval. A daily full scan (spec default for network roots) runs at the first poll tick after it is due, so an unused NAS wakes at most at the 6-hour ticks.
24. **Sync roots** poll every 10 minutes and scan fully daily; **local roots** scan fully weekly.
25. **Local roots: `fs.watch` (recursive) in the indexer thread** is a hint: events (or a `null` file name) queue a targeted rescan of their directories after 2 s of quiet; an `error`/`ENOSPC`/overflow switches the library to 5-minute polling with `WATCH_DEGRADED` until the next restart. On Linux Node's recursive watch walks the tree synchronously, which is why it lives off the main thread.
26. **macOS SMB listings are not refreshed by writing** (spec §7.2 says "as fin-store does", which creates and deletes a file in the directory): libraries are read-only, so on macOS shares a change can take up to 60 s (smbfs `dir_cache_max`) longer to appear. Recorded as a deviation in Task 16. NFS attribute caches (`acdirmax`, default 60 s) behave the same on Linux.
27. **Pools** (spec §6.3) per **volume**, so two libraries on one NAS share its disks: `io:<volumeKey>.scan` (1) and `io:<volumeKey>.read` (2 for network and sync, 4 for local), defined with `definePool` when a library is added or loaded. Scans take `.scan`; probes, hashes and every stage that reads originals take `.read` (`libraryPools(id)` is frozen for 6 and 8).
28. **Offline from a job:** a scan or probe worker that finds the root unreachable answers `error {code: LIBRARY_OFFLINE, retry: never}`; the service puts the library offline and re-probes it (Decision 16), instead of the engine's backoff piling up retries. `resourceHealthy` of every library kind answers the library's cached root state, re-probed within 5 s when older than 60 s; `RESOURCE_STUCK` on its pool also makes it offline.

### Probe, fingerprint and hash

29. **`library.probe`** (batch ≤ 256 files; items = catalog file ids; `utility` QoS; `io:<v>.read` + `cpu`) opens each file once through Decision 22 and computes **`fp1` first, then the shallow probe**, from the same handle: a file whose probe fails keeps its fingerprint (`probeFailed`, `code`). Results go to the catalog through `call library.probed {results ≤ 32}` (then `item` acks), not item values, so `job_items` in `control.db` stays small. A crash isolates the in-flight file (5C-1 Decision 32); `POISON_ITEM` is recorded per file and not retried until its size or mtime changes.
30. **`fin-probe` gains `probe_reader(r: impl Read + Seek, meta: FileMeta, path) -> ProbeOutcome {fingerprint, probe}`** with `probe(path)` kept as a wrapper. It is a mechanical change to the readers' signatures; it should land after 6A-1 merges (6A-1 rewrites the MP4 path to `fin-container`), else 6A-1 rebases over it. **`PROBE_VERSION`** (a `fin-probe` constant, bumped by 6A-1 and by any probe change) is stored per file, and a new version re-queues only the probe stage.
31. **Bandwidth** (spec §6.2): `bandwidthMiBs` reaches the worker in params (`readBps`, split evenly over the pool's slots) and wraps the file in a `BudgetedReader` over `fin_jobs::ReadBudget`; enumeration is not budgeted (metadata only).
32. **Probe candidates:** the catalog lists every non-excluded file; only files whose extension is in the library's `kinds` table (default `media`, `image`, `svg`, `lottieTemplate`, `font`, `lut` extensions, case-insensitive; `kinds: all` probes everything) are probed, the others are `skipped`. The extension only filters: the recorded kind comes from the content (fin-probe).
33. **`library.hash`** (batch ≤ 64; items = file ids; whole-file BLAKE3 through the budget; item values `blake3:<hex>`, written by the kind's `onSucceeded`) runs only on demand (`library.lookup {hashes, hashCandidates: true}` and `Libraries.hash`), never in a sweep (spec §7.3).

### Stages

34. **The probe stage is built in** and keyed by file; every other stage is **keyed by fingerprint** and implicitly depends on `probe`. `registerStage(spec)` (frozen) declares `{name, version, appliesTo, dependsOn, jobKind, batch, params, priority?, maxInFlight?, isDone?}`; the stage's kind must be a batch kind whose items are fingerprints and whose result keeps 5C-1's default `combine` shape (`{items, itemErrors}`).
35. **The pump** (per library, per stage): pending = files `present`, probed, `appliesTo`, dependencies `done`, no `stages` row at this version or one `failed` with attempts left; it keeps ≤ `maxInFlight` (default 2) jobs queued or running, owner `system`, priority `background` by default, `library` set; it stops while the library is offline or paused and never selects `cloudOnly` files unless `hydrate`. `isDone(fp)` (a cache hit in 6's or 8's index) marks a row `done` without a job. Completion comes from `JobEngine.onDone`: items → `done`; item errors → `failed` with the code (`POISON_ITEM` permanently for this version); a whole failed job → `failed` for its items, retried after 1 h, 3 attempts. At start, rows `queued`/`running` are reconciled with `engine.get(jobId)`.
36. **Versions:** a stage's `version` string changes (6: encoder settings; 8: analyzer id, version and model hashes) → its rows at other versions become `stale` and are re-queued at `background` priority; other stages are untouched (spec §7.3). Outputs are the stage owner's, keyed by fingerprint under `<server-data>/cache/` (5D never reads or deletes them).

### Search, lookup and locate

37. **`library.search`**: the query is tokenised like `unicode61` and each term becomes a prefix term (`"interv"*`), over `name` and `dir`; filters on `kind`, `durationMs`, `width`, `height`, `lastChange ≥ since`, `state`; ordered by `bm25(names, 4.0, 1.0)`, then `relKey`, then id; across libraries merged by score with a keyset cursor `(score, libraryId, fileId)`. A query with no terms lists by `lastChange` descending. Sub-project 8E replaces the scorer with hybrid fusion behind the same RPC.
38. **`library.lookup {fingerprints ≤ 1000 | hashes ≤ 1000, hashCandidates?}`**: exact matches across this server's libraries; with `hashCandidates`, rows of equal size without a full hash are hashed by a `library.hash` job (`{matches, pending: {job}}`; needs `library.scan` and `jobs.submit`).
39. **`locate({path?, fingerprint?, fullHash?})`** (in-process, frozen for sub-project 6's manifest): a path under a root, or under one of the library's **aliases** (other machines' paths of the same root, arch §20: `/Volumes/<share>/Media` ↔ `\\<server>\media` ↔ `/mnt/media`, stored per server), maps to the root and is looked up by `relKey`; then by fingerprint; then by full hash. It answers `{library, fileId, absolute, state, match, offline}` or nothing; `cloudOnly` is answered, and 6 refuses it at export with `FILE_CLOUD_ONLY`.
40. **Answers carry freshness:** `stale`, `offline` and `lastPollAt` on every search and listing.

### RPCs, scopes, CLI

41. **Scopes** (spec §4.5, §7.6): `library.add`, `library.remove`, and `library.set` of `hydrate`, `bandwidthMiBs` or `acceptVolume` need `library.admin` (human-only, an escalation from `library.set`); other `library.set` fields (`name`, `include`, `exclude`, `kinds`, `scan`, `aliases`, `paused`, `stages`) need `library.scan`; listings, search, lookup and `library.watch` need `library.read`; `library.scan` needs `library.scan` and `jobs.submit`. **Reach** in v1: every principal holding `library.read` reaches every library of this server (credentials carry `projects`, not libraries); `libraryReach(principal, id)` is the one function 8's host calls use, so per-library lists can come later in one place. `asset.probe` and `asset.add` paths inside a library root are in reach for principals with `library.read` (5A Decision 19's "(5D)" clause), and a `cloudOnly` path answers `FILE_CLOUD_ONLY` before fin-node reads it.
42. **Library names** are unique per server, case-insensitively (`LIBRARY_NAME_TAKEN`); commands take an id or a name. `library.add` starts the first full scan and answers `{library, job}`.
43. **Audit** (spec §3.3): `library.add`, `library.remove`, `library.set` (human-only fields only), with `{libraryId, storage, fields}`; never names, roots or paths.
44. **`library.scan` journal events** (spec §3.4) for full and incremental scans, watch rescans and polls that found changes; no-change polls are counted in `library.status` only, so a 60 s poll does not write 1,440 events a day. Payload as the spec, `files`/`dirs` as log₂ buckets, `trigger: add|schedule|demand|watch|mount`.
45. **Codes:** 3 `LIBRARY_ROOT_INVALID`, `LIBRARY_OVERLAP`, `LIBRARY_NOT_FOUND`, `LIBRARY_NAME_TAKEN`, `FILE_NOT_FOUND`, `INVALID_PATH`; 4 `FILE_CLOUD_ONLY`; 5 `LIBRARY_OFFLINE`, `LIBRARY_ROOT_UNREADABLE`, `CATALOG_TOO_NEW`; 7 `HUMAN_ONLY` (5A); warnings in `library.status`: `ROOT_EMPTY_SUSPECT`, `ROOT_UNMOUNTED`, `ROOT_VOLUME_CHANGED`, `WATCH_DEGRADED`, `RESOURCE_STUCK`, `ENUMERATION_FALLBACK`.

## Frozen interfaces

Frozen once Tasks 1, 6, 7, 10 and 11 merge, for sub-projects 6, 8 and 9; a change needs a coordinator decision and a note in each consumer's plan.

```rust
// crates/fin-library (Task 1; Tasks 2–5 implement)
pub enum Placeholder { Dataless, IcloudStub, Nextcloud, Owncloud, RecallOnOpen, RecallOnData, Offline, Unpinned }
pub enum EntryType { File, Dir, Symlink, Other }
pub struct ScanEntry { pub dir: String /* relKey */, pub name: Vec<u8>, pub name_nfc: String, pub name_lossy: bool,
    pub rel_key: String, pub entry_type: EntryType, pub size: u64, pub mtime_ns: i64, pub file_id: Option<Vec<u8>>,
    pub dev: Option<String>, pub mount_root: bool, pub placeholder: Option<Placeholder>, pub excluded: Option<String> }
pub struct DirDone { pub rel_key: String, pub mtime_ns: Option<i64>, pub complete: bool, pub code: Option<String>, pub entries: u32 }
pub struct ScanBatch { pub seq: u64, pub entries: Vec<ScanEntry>, pub dirs_done: Vec<DirDone> }
pub struct VolumeIdentity { pub dev_id: String, pub fs_id: Option<String>, pub mount_root: bool, pub case_sensitive: bool,
    pub enumeration: String, pub remote: bool, pub volume_key: String }
pub enum RootState { Online(VolumeIdentity), Unreachable { code: String }, Unmounted, VolumeChanged { fs_id: String }, Empty, Timeout }
pub fn identity(root: &Path) -> io::Result<VolumeIdentity>;
pub fn root_state(root: &Path, stored: &VolumeIdentity, timeout: Duration) -> RootState;
pub fn open_for_read(root: &Path, rel_key: &str, expect: &Expect, hydrate: bool) -> Result<File, OpenError>;   // Decision 22
pub struct ProbeOutcome { pub file: i64, pub fingerprint: Option<String>, pub probe: Option<fin_probe::Probe>,
    pub probe_version: u32, pub state: String, pub code: Option<String> }
```

```ts
// apps/finisherd/src/library/api.ts (Tasks 10–12): the in-process API for sub-projects 6 and 8
export type LibraryStorage = "local" | "network" | "sync";
export interface LibraryPools { readonly scan: PoolName; readonly read: PoolName }
export interface StageFile { readonly library: string; readonly fileId: number; readonly relKey: string; readonly absolute: string;
  readonly fp: string; readonly size: number; readonly kind?: string; readonly probe?: unknown /* fin_probe::Probe */;
  readonly storage: LibraryStorage }
export interface StageSpec {
  readonly name: string;                                   // /^[a-z][a-zA-Z0-9.:-]{0,63}$/; "probe" is reserved
  readonly version: string;                                // opaque; a change re-queues this stage only
  readonly appliesTo: (f: StageFile) => boolean;
  readonly dependsOn?: ReadonlyArray<string>;
  readonly jobKind: string;                                // a registered batch KindSpec; items = fingerprints
  readonly batch: number;                                  // ≤ 256
  readonly params: (files: ReadonlyArray<StageFile>, ctx: { readonly library: string; readonly pools: LibraryPools }) => unknown;
  readonly priority?: Priority;                            // default background
  readonly maxInFlight?: number;                           // per library, default 2
  readonly isDone?: (fp: string) => Effect.Effect<boolean>;
}
export interface Located { readonly library: string; readonly fileId: number; readonly absolute: string;
  readonly state: FileState; readonly match: "path" | "alias" | "fingerprint" | "hash"; readonly offline: boolean }
export type FileEvent = { readonly type: "added" | "changed" | "missing" | "renamed" | "moved" | "probed" | "stage";
  readonly library: string; readonly fileId: number; readonly fp?: string; readonly stage?: string };
export interface Libraries {
  list(): ReadonlyArray<LibraryView>;
  get(id: string): Effect.Effect<LibraryView, FinError>;                         // LIBRARY_NOT_FOUND
  locate(ref: { readonly path?: string; readonly fingerprint?: string; readonly fullHash?: string }): Effect.Effect<Located | undefined, FinError>;
  pools(id: string): LibraryPools;
  online(id: string): Effect.Effect<boolean>;
  use(ids: ReadonlyArray<string>): { release(): void };                         // in use (Decision 23)
  registerStage(spec: StageSpec): void;
  stageStates(fp: string): Effect.Effect<ReadonlyArray<{ library: string; stage: string; version: string; state: string; code?: string }>>;
  hash(library: string, fileIds: ReadonlyArray<number>): Effect.Effect<JobView, FinError>;
  onFile(listener: (e: FileEvent) => void): () => void;
  reach(principal: Principal, id: string): boolean;                             // Decision 41
}
export class LibrariesTag extends Context.Tag("Libraries")<LibrariesTag, Libraries>() {}

// apps/finisherd/src/library/catalog/chains.ts (Task 7): for 8C's transcripts tables
export function registerCatalogChain(name: string, migrations: ReadonlyArray<Migration>, versions?: ChainVersions): void;

// @finisher/contracts library.ts (Task 13): Effect Schemas; TS types are Schema.Type of them
LibraryView = { id; name; root; aliases: string[]; storage: LibraryStorage; provider?: string;
  state: "online" | "offline" | "scanning" | "paused"; offlineReason?: string; include?: string[]; exclude: string[];
  kinds: string[] | "all"; scan: { poll: "demand" | number; full: "daily" | "weekly" | "manual" }; hydrate: boolean;
  bandwidthMiBs?: number; stages: string[]; counts: Record<FileState, number>; lastScan?: { at; mode; outcome; code? };
  lastPollAt?: number; warnings: { code: string; dir?: string }[]; createdAt: number }
FileState = "present" | "missing" | "cloudOnly" | "unreadable" | "probeFailed" | "skipped" | "offline"
SearchHit = { library; fileId; relKey; name; kind?; size; durationMs?; width?; height?; fp?; state: FileState; score: number; offline: boolean }
```

**RPCs** (Task 13; scopes per Decision 41):

| RPC | Payload → result |
|---|---|
| `library.add` | `{root, name?, storage?, include?, exclude?, kinds?, scan?, hydrate?, bandwidthMiBs?, challenge?}` → `{library, job}` |
| `library.remove` | `{id, challenge?}` → `{}` (cancels its jobs, deletes its catalog directory) |
| `library.set` | `{id, name?, include?, exclude?, kinds?, scan?, aliases?, paused?, stages?, hydrate?, bandwidthMiBs?, acceptVolume?, challenge?}` → `{library}` |
| `library.list`, `library.status` | `{}` → `{libraries}`; `{id}` → `{library, jobs, stages: [{stage, version, done, queued, failed, stale}], watch?}` |
| `library.files` | `{id, dir?, state?, kind?, cursor?, limit ≤ 500}` → `{dir, dirs, files, next?, stale, offline}` |
| `library.file` | `{id, path \| fileId}` → `{file, stages}` (`INVALID_PATH` for `..`, absolute or escaping paths) |
| `library.search` | `{query?, libraries?, kind?, duration?: {min?, max?}, width?, height?, since?, state?, cursor?, limit ≤ 200}` → `{hits, next?, stale}` |
| `library.lookup` | `{fingerprints? \| hashes?, hashCandidates?}` → `{matches: [{key, library, fileId, relKey, absolute, state}], pending?: {job}}` |
| `library.scan` | `{id, full?, dirs?}` → `{job}` |
| `library.watch` (stream) | `{ids?}` → a snapshot of `LibraryView`s, then `{type: library \| scan \| progress, …}` |

**`fin`:** `fin library ls|status <lib>|scan <lib> [--full] [--dir <rel>]…|files <lib> [<dir>] [--state] [--kind] [--limit] [--cursor]|show <lib> <path>|search [<query>] [--lib <lib>]… [--kind] [--min-duration] [--max-duration] [--min-width] [--min-height] [--since] [--limit]|set <lib> [--name] [--include]… [--exclude]… [--kinds] [--poll demand|<s>] [--full daily|weekly|manual] [--alias <path>]… [--pause|--resume]`; `fin search <query> …` = `fin library search`. **Console** (TTY and typed challenge, 5A Decision 30): `finisherd library add <dir> [--name] [--storage local|network|sync] [--include]… [--exclude]… [--poll] [--full] [--hydrate] [--bandwidth <MiB/s>]`, `finisherd library rm <lib>`, `finisherd library set <lib> [--hydrate on|off] [--bandwidth <MiB/s>|off] [--accept-volume]`.

## Questions for the owner

None. The plan follows spec revision 4 and its owner decisions; the details below are engineering choices recorded as Decisions and written back to the spec in Task 16:
- read-only roots win over the macOS listing refresh of spec §7.2 (Decision 26): up to 60 s more latency on macOS SMB shares;
- `ROOT_VOLUME_CHANGED` with a human-only `acceptVolume` (Decision 14), nested mount points tracked per directory, `ENUMERATION_FALLBACK`;
- pools per volume rather than per root (Decision 27); bandwidth in params (Decision 31); the default 4 MiB/s on Linux FUSE sync roots (Decision 12);
- `fin library set` for the non-human fields, unique library names, aliases at `library.scan` (Decisions 41–42);
- v1 library reach = the `library.read` scope (Decision 41); no-change polls not journaled (Decision 44).

## Review Focus

1. **No placeholder is ever opened, read or descended into, on any OS, and nothing is written under a root.** → Task 1 `classify_matrix`, `stub_and_real_file_real_wins`; Task 2 `iopol_off_makes_dataless_access_fail`, `dataless_dir_not_descended`; Task 3 `cfapi_placeholders_never_fetched` (a real sync root: zero `FETCH_DATA`/`FETCH_PLACEHOLDERS` callbacks), `recall_on_open_never_opened`; Task 4 `fake_placeholders_open_counter_is_zero`; Task 6 `check-library-open` gate; Task 15 `root_is_byte_identical_after_scan`.
2. **An unavailable root never marks a file missing;** unmounted, volume-changed, empty-suspect and timed-out roots go offline with the catalog intact; a partial listing marks nothing. → Task 1 `root_state_classify`; Task 2 `bind_mount_unmounted_is_offline`, `disk_image_detached_is_offline`; Task 4 `first_stat_45s_of_60_is_online`, `entry_timeout_marks_dir_unreadable_not_missing`; Task 7 `incomplete_dir_marks_nothing_missing`, `root_empty_suspect_aborts_before_marks`; Task 10 `offline_library_rechecked_every_60s`.
3. **The diff is right:** renames by file id keep history, moves by fingerprint merge, changes clear probes, resumed scans skip done listings. → Task 7 property `diff_matches_reference_model` (fast-check over random trees and edits) and the 10k-tree test.
4. **NAS disks are left alone when nobody uses them;** in use, changes appear within a minute; answers are fresh or say `stale`. → Task 10 `demand_cadence_with_injected_clock`, `freshness_pass_bounded_5s_then_stale`, `unused_library_polls_every_6h`.
5. **Safe opens:** no link followed, no escape from the root, identity checked before reading. → Tasks 2, 3 `open_refuses_symlink_component`, `open_refuses_junction`, `identity_mismatch_is_changed`.
6. **The event loop:** no catalog statement on the main thread; lag p99 ≤ 20 ms during a 100k-file scan. → Task 15 budget.
7. **Stages:** versions re-queue only their stage; dependencies hold; offline and paused libraries stop the pump; restart reconciles. → Task 11.
8. **Scopes and human-only:** `library.add/remove` and the human-only `set` fields refuse every non-human principal; fixes carry no command; paths never leave the root. → Task 13 matrix.
9. **Privacy:** no name or path in the journal, audit or info logs. → Task 13 `journal_and_audit_carry_no_names`.
10. **Files ≤ 600 lines; no GPL dependency; every commit green on three OSes.** → gates at every commit; Task 17.

---

### Task 1: `fin-library` types, names, placeholders, excludes, volume rules and the fake source — tier: opus — can start now

**Files:** Create `crates/fin-library/{Cargo.toml (features test-hooks), src/{lib.rs, types.rs, names.rs, placeholder.rs, exclude.rs, glob.rs, volume.rs, source/{mod.rs, fake.rs}}}`; modify `Cargo.toml` (member), `crates/fin-store/examples/export_schema.rs` (+ dev-dependency), `packages/core-types` (regenerated), `TASKS.md`. Test: `crates/fin-library/tests/{names.rs, placeholder.rs, exclude.rs, volume.rs, fake.rs}`.

**Interfaces:** the `fin-library` block above; `Source`, `RawEntry`, `RootStat` (Decision 4); `classify(entry, siblings) -> Option<Placeholder>`; `Excludes::new(defaults, include, exclude, case)`, `excluded(rel_key, is_dir) -> Option<&'static str>`; `classify_root(stored, now: RootStat) -> RootState`; `FakeSource::from_json(spec)` (tree, attributes, placeholders, per-path delays and errors, mount flags, an open counter). Decisions 4, 9–11, 13–15.

- [ ] **Step 1: Write failing tests:** `nfc_keys_and_lossy_names` (NFD input from macOS, invalid UTF-8, unpaired UTF-16 surrogates as WTF-8), `case_folded_keys_only_on_insensitive_roots`; `classify_matrix` (every attribute bit and tag per OS, `PINNED` hydrated, `UNPINNED` online-only), `stub_and_real_file_real_wins`, `nextcloud_and_owncloud_stub_names`, `icloud_stub_names`; `default_excludes`, `conflict_copies_excluded`, `project_folders_keep_only_sources`, `glob_matrix` (`**`, classes, case); `root_state_classify` (unmounted by mount-root loss and by `st_dev` = parent, volume change by `fsId`, `devId`-only change is online); `fake_source_reports_and_counts`.
- [ ] **Step 2: Implement** (pure; no OS call outside `source/`). **Step 3: Run** the gates → PASS. **Commit** `feat(fin-library): names, placeholders, excludes, volume rules and a fake source`.

### Task 2: Linux and macOS sources, volume identity and safe opens — tier: opus — after Task 1, beside Task 3

**Files:** Create `crates/fin-library/src/source/{unix.rs, linux.rs, macos.rs}`, `crates/fin-library/src/open/{mod.rs, unix.rs}`, `crates/fin-library/src/volume_unix.rs`. Test: `crates/fin-library/tests/{source_unix.rs, open_unix.rs, mounts_unix.rs}`.

Decisions 5, 6, 9, 13, 14, 22.

- [ ] **Step 0:** confirm in `libc` 0.2.191 the bindings for `statx` (with `STATX_ATTR_MOUNT_ROOT`, `STATX_MNT_ID`), `openat2`/`open_how`, `getattrlistbulk`/`attrlist`, `O_NOFOLLOW_ANY`, `setiopolicy_np`; declare what is missing locally (`syscall(SYS_openat2)`), and note it in `source/mod.rs`.
- [ ] **Step 1: Write failing tests:** `enumerates_without_opening_files` (Linux: a tree whose files are mode 0000 still enumerates with sizes and mtimes; macOS the same), `symlinks_skipped_never_followed`, `file_ids_and_mount_roots`, `bind_mount_unmounted_is_offline` (Linux, re-executed under `unshare -Urm`; skips with a notice without user namespaces), `disk_image_detached_is_offline` (macOS, `hdiutil attach -mountpoint <tmp>` then `detach`), `case_sensitivity_detected_without_writing`; `iopol_off_makes_dataless_access_fail` (macOS: the policy reads back as off in the worker setup function; a real dataless file is checked in Task 17), `dataless_dir_not_descended` (fake attrs through the macOS code path's classifier); `open_refuses_symlink_component`, `open_stays_beneath_root`, `identity_mismatch_is_changed`, `placeholder_refused_before_open`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(fin-library): Linux and macOS enumeration, volume identity and safe opens`.

### Task 3: Windows source, volume identity and safe opens — tier: opus — after Task 1, beside Task 2

**Files:** Create `crates/fin-library/src/source/windows.rs`, `crates/fin-library/src/open/windows.rs`, `crates/fin-library/src/volume_windows.rs`; modify `crates/fin-library/Cargo.toml` (windows-sys features; `Win32_Storage_CloudFilters` dev-only). Test: `crates/fin-library/tests/{source_windows.rs, cfapi_windows.rs}`.

Decisions 7, 9, 10, 13, 14, 22.

- [ ] **Step 0: prove the placeholder path on a real Cloud Files sync root** (no admin): `CfRegisterSyncRoot` on a temp directory, `CfConnectSyncRoot` with callbacks that count `FETCH_DATA` and `FETCH_PLACEHOLDERS`, `CfCreatePlaceholders` for files (recall on data access) and a directory (recall on open/enumeration). Record what `FILE_ID_EXTD_DIR_INFO` reports for each, and whether `FindFirstFileExW` and the open of Decision 22 step 2 trigger a callback. If an open with `FILE_FLAG_OPEN_REPARSE_POINT` hydrates, step 2 never runs for a flagged entry (already the rule) and the finding is recorded in Task 16.
- [ ] **Step 1: Write failing tests:** `cfapi_placeholders_never_fetched` (a full scan and a probe pass over the sync root: zero callbacks; entries `cloudOnly`; the placeholder directory not descended), `recall_on_open_never_opened`, `extd_dir_info_fields` (128-bit ids, 100 ns times → ns, sizes), `enumeration_fallback_chain` (forced through a test hook), `junctions_and_symlinks_skipped`, `long_paths_with_verbatim_prefix`, `open_refuses_junction`, `final_path_must_be_under_root`, `volume_serial_and_case_flag`, `subst_drive_removed_is_unreachable`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates on Windows → PASS. **Commit** `feat(fin-library): Windows enumeration from directory handles, volume identity and safe opens`.

### Task 4: The walker: parallel reads, timeouts, batches, polls and the test overlay — tier: opus — after Task 1 (develops on `FakeSource`; real sources after Tasks 2–3)

**Files:** Create `crates/fin-library/src/{walk/{mod.rs, pool.rs, batch.rs, poll.rs}, timing.rs, overlay.rs (test-hooks)}`. Test: `crates/fin-library/tests/{walk.rs, walk_timeouts.rs, overlay.rs}`.

**Interfaces:**
```rust
pub enum WalkMode { Full { gen: u64, resume: Option<KnownDirs> }, Dirs { rel: Vec<String> }, Poll { known: KnownDirs } }
pub struct WalkOpts { pub threads: usize, pub excludes: Excludes, pub timing: Timing, pub batch_entries: usize }
pub struct Timing { pub first_stat: Duration /* 60 s */, pub entry: Duration /* 30 s */ }
pub fn walk(src: &dyn Source, mode: WalkMode, opts: &WalkOpts, sink: &mut dyn FnMut(ScanBatch) -> Result<(), WalkStop>,
    cancel: &CancelToken) -> Result<WalkSummary, WalkError>;   // WalkError::Offline(RootState)
```
The overlay (test-hooks only): a `.fin-fake.json` at a root marks real paths as placeholders, delays first stats and entries, injects errors and unmounts, and appends every open of a listed path to a counter file outside the root; release builds never read it (`scripts/check-test-hooks.mjs` covers the crate). Decisions 8, 11, 14, 16, 17, 21.

- [ ] **Step 1: Write failing tests:** `walk_order_is_deterministic`, `parallel_reads_bounded_by_threads`, `batches_capped_at_4096_entries`, `dir_done_complete_only_after_full_listing`; `first_stat_45s_of_60_is_online` and `first_stat_past_60s_is_offline` (scaled timing), `entry_timeout_marks_dir_unreadable_not_missing`, `eio_and_eacces_dirs_incomplete`; `poll_relists_only_changed_mtimes`, `dirs_mode_targets_only_given_dirs`, `resume_skips_listed_dirs_but_descends`; `nested_mount_unmounted_is_offline_subtree`, `root_empty_with_catalog_is_suspect`; `cancel_stops_within_one_batch`; `fake_placeholders_open_counter_is_zero` (overlay, through every walk mode).
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(fin-library): the parallel walker with spin-up-tolerant timeouts, polls and resumable scans`.

### Task 5: The probe and hash entry points — tier: sonnet — after Task 2 or 3 (either); best after 6A-1 merges

**Files:** Modify `crates/fin-probe/src/lib.rs` and its readers (`probe_reader`, `FileMeta`, `PROBE_VERSION`; `probe(path)` stays); create `crates/fin-library/src/{probe.rs, hash.rs, budget.rs}`. Test: `crates/fin-probe/tests/reader.rs`, `crates/fin-library/tests/probe.rs`.

Decisions 22, 29–31, 33.

- [ ] **Step 1: Write failing tests:** `probe_reader_equals_probe` (every fixture kind: same `Probe`, same `fp1`), `fp_survives_probe_failure`, `probe_entry_uses_one_handle`, `budget_limits_probe_reads` (injected clock), `hash_is_whole_file_blake3`, `cloud_only_refused_unless_hydrate`, `changed_identity_is_changed_not_probed`, `probe_version_is_exported`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(fin-probe,fin-library): reader-generic probe; budgeted probe and hash of library files`.

### Task 6: The `library` worker family and the open gate — tier: sonnet — after Tasks 4–5 and 5C-1 Tasks 1–2

**Files:** Create `crates/fin-workers/src/library/{mod.rs, scan.rs, probe.rs, hash.rs, params.rs}`, `scripts/check-library-open.mjs` (+ `.test.mjs`); modify `crates/fin-workers/src/main.rs` (dispatch), `crates/fin-core/src/events/names_jobs.rs` (`library.scan`, `library.probe`, `library.hash`), the schema export, `scripts/gates.mjs`. Test: `crates/fin-workers/tests/library_*.rs` (over 5C-1's in-memory runtime harness and the overlay).

**Interfaces:** params and results (exported): `LibraryScanParams {library, root, mode: full|dirs|poll, gen, dirs?, excludes, threads, timing? (test-hooks)}`, `LibraryProbeParams {library, root, hydrate, readBps?, files: in/files.ndjson}` (items = file ids), `LibraryHashParams {library, root, readBps?}`; host calls `library.batch {file, seq}`, `library.probed {results}`; `error LIBRARY_OFFLINE {state}` with `retry: never`. The family sets the macOS I/O policy before `hello` (after the self-sandbox entry). Decisions 6, 20, 28–33.

- [ ] **Step 1: Write failing tests:** `scan_streams_batches_through_calls_and_waits_for_replies`, `scan_reports_progress_per_directory`, `offline_root_is_library_offline_never_retried`, `probe_acks_items_after_probed_reply`, `probe_crash_on_one_file_is_isolatable` (a test-hooks crash file: the in-flight item is the file), `hash_items_carry_hashes`, `family_starts_no_process` (5C-1's foreign-process check), `check_library_open_flags_forbidden_apis`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(fin-workers): the library family: scans, probes and hashes`.

### Task 7: Catalogs: chains, schema, the indexer and reader threads, the diff — tier: opus — after Task 1 (generated types); can start now

**Files:** Create `apps/finisherd/src/library/catalog/{chains.ts, migrations/catalog/0001.ts, open.ts, indexer-thread.ts, reader-thread.ts, client.ts, diff.ts, apply.ts, fts.ts}`. Test: `apps/finisherd/test/library/{catalog.test.ts, diff.test.ts, diff.property.test.ts, threads.test.ts}`.

**Interfaces:** `registerCatalogChain` (frozen); `IndexerClient {open(lib), close(lib), applyBatch(lib, batch, gen), finishScan(lib, scan), applyProbed(lib, results), applyHashes, stagesSet, knownDirs(lib, gen) → file, pending(lib, stage, limit), purgeMissing(lib)}` and `ReaderClient {files, file, search, lookup, counts, stageCounts}`, all `Effect`s over `MessagePort`s. Decisions 2, 18–21.

- [ ] **Step 1: Write failing tests:** `schema_and_chains_apply_once`, `catalog_of_another_library_moved_aside`, `corrupt_catalog_rebuilt`, `too_new_catalog_is_offline`; `diff_matches_reference_model` (fast-check: random trees, renames with and without file ids, moves, rewrites, deletions, placeholder flips, incomplete directories, resumed generations, against a brute-force model), `incomplete_dir_marks_nothing_missing`, `root_empty_suspect_aborts_before_marks`, `rename_keeps_id_and_fp`, `move_by_fingerprint_merges`, `missing_reappears_present`, `missing_purged_after_90_days_in_batches`; `fts_rows_follow_inserts_renames_deletes`; `transactions_capped_at_2000_rows`, `reader_never_waits_for_writer` (a long apply does not delay a search past 20 ms), `main_thread_runs_no_catalog_statement` (a spy on `DatabaseSync` in the main thread).
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): library catalogs with an indexer and a reader thread`.

### Task 8: The `libraries` chain, the registry and root probes — tier: sonnet — can start now (5A Task 1 is merged)

**Files:** Create `apps/finisherd/src/control/migrations/libraries/0001.ts`, `apps/finisherd/src/library/registry.ts`, `crates/fin-node/src/library/{mod.rs, rootprobe.rs}`; modify `crates/fin-node/src/lib.rs`, `packages/fin-node` (typings). Test: `apps/finisherd/test/library/registry.test.ts`, `crates/fin-node/tests/rootprobe.rs`.

**Interfaces:** `libraries (id TEXT PRIMARY KEY, name TEXT NOT NULL, nameKey TEXT NOT NULL UNIQUE, root TEXT NOT NULL, rootKey TEXT NOT NULL UNIQUE, aliases TEXT NOT NULL, volume TEXT NOT NULL, storage TEXT NOT NULL, storageOverride TEXT, state TEXT NOT NULL, offlineReason TEXT, include TEXT, exclude TEXT NOT NULL, kinds TEXT NOT NULL, scan TEXT NOT NULL, hydrate INTEGER NOT NULL, bandwidthMiBs REAL, stages TEXT NOT NULL, gen INTEGER NOT NULL, createdBy TEXT NOT NULL, createdAt INTEGER NOT NULL, lastPollAt INTEGER, lastFullAt INTEGER)`; `Registry {add, remove, set, get, list, byName}` (validation: absolute existing directory, not inside server data, no overlap by `rootKey` prefix in either direction, `storage_kind::detect` with override); napi `libraryCanonicalRoot(path) → {root, rootKey, identity}` and `libraryProbeRoot(root, identity, timeoutMs) → Promise<RootState>` (Decision 3). Audit kinds `library.add|remove|set`. Decisions 3, 12–14, 42, 43.

- [ ] **Step 1: Write failing tests:** `add_validates_root` (relative, missing, a file, inside server data, `LIBRARY_OVERLAP` both ways, symlinked roots canonicalised), `names_unique_case_insensitive`, `storage_detected_with_override`, `linux_sync_default_bandwidth`; `probe_runs_off_the_libuv_pool` (four hung probes on four roots do not delay `fs.readFile`), `one_probe_per_root_outstanding`, `timeout_leaves_thread_and_counts_it`; `audit_rows_carry_no_names_or_paths`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd,fin-node): libraries in control.db and per-root probe threads`.

### Task 9: Library kinds and family wiring on the engine — tier: opus — after Tasks 6–8 and 5C-1 Tasks 7–10

**Files:** Create `apps/finisherd/src/library/kinds.ts` (`library.scan`, `library.probe`, `library.hash`, the `library` `FamilySpec` with `onCall`), `apps/finisherd/src/library/pools.ts`. Test: `apps/finisherd/test/library/kinds.test.ts` (a real engine, test-hooks `fin-workers`, overlay roots).

**Interfaces:** `scan`: `resources: [pools.scan]`, `qos: utility`, `stallS: 600`, `timeoutS: 86400`, `resumable`, `prepare` writes `in/dirs.ndjson`, `files: [{name: "out/", mode: "write"}]`, `dedupe: <library>:<mode>` for full scans and polls (targeted `dirs` scans are not deduplicated); `probe`: `batch` over file ids, `resources: [pools.read, "cpu"]`, `prepare` writes `in/files.ndjson`, `dedupe` by the sorted ids' hash; `hash`: `batch`, `onSucceeded` writes hashes; all `resourceHealthy: online(library)`; scopes `["library.scan"]`. Decisions 20, 21, 27–29, 33.

- [ ] **Step 1: Write failing tests:** `full_scan_of_10k_tree_lands_in_catalog`, `scan_resumes_after_finisherd_restart_without_relisting`, `poll_job_touches_only_changed_dirs`, `probe_poison_file_isolated_and_recorded`, `offline_mid_scan_marks_nothing_missing` (overlay unmounts the root halfway), `resource_unhealthy_never_poisons` (root offline during a probe crash), `pools_shared_per_volume`, `batch_files_parsed_defensively` (a garbage batch file fails the job, writes nothing), `bandwidth_splits_over_slots`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): library job kinds on the shared engine`.

### Task 10: The library service: states, triggers, demand polling, watches, journal — tier: opus — after Task 9

**Files:** Create `apps/finisherd/src/library/{service.ts, plan.ts (pure poll planner), usage.ts, watch.ts (indexer-thread side), events.ts, api.ts}`, `crates/fin-core/src/events/payloads/library.rs`; modify `crates/fin-core/src/events/{kinds.rs (one line), payloads/mod.rs}`, `apps/finisherd/src/main.ts` (layer after the engine; close before it), `apps/finisherd/src/projects.ts` (call-site lines: `use()` on open, release on close). Test: `apps/finisherd/test/library/{plan.property.test.ts, service.test.ts, watch.test.ts}`.

**Interfaces:** `plan(libs, now, usage) → Action[]` (pure: `poll | full | recheck | none` per library, with the next wake-up); `LibrariesTag` and the `Libraries` API (frozen; `registerStage` from Task 11). States `online → scanning → online`, `offline` (with reason) ↔ recheck, `paused`. Decisions 16, 17, 23–28, 44.

- [ ] **Step 1: Write failing tests:** `demand_cadence_with_injected_clock` (60 s while a project referencing it is open, a watch stream is open or within 10 min of a search; 6 h otherwise), `unused_library_polls_every_6h`, `daily_full_runs_at_first_tick_after_due`, `fixed_interval_override`, `freshness_pass_bounded_5s_then_stale`, `offline_library_rechecked_every_60s`, `return_online_triggers_mount_scan`, `volume_changed_waits_for_accept`, `local_watch_hint_rescans_after_2s_quiet`, `watch_overflow_degrades_to_5min_polls`, `library_scan_events_payload_closed` (no names, buckets log₂), `no_change_polls_not_journaled`, `plan_never_two_jobs_per_library` (property).
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): the library service with demand-driven polling, watch hints and offline handling`.

### Task 11: The stage framework, `locate` and file events — tier: opus — after Task 10

**Files:** Create `apps/finisherd/src/library/{stages.ts, pump.ts, locate.ts}`. Test: `apps/finisherd/test/library/{stages.test.ts, locate.test.ts}` (a `test.stage` batch kind behind test-hooks, items = fingerprints).

**Interfaces:** `registerStage`, `stageStates`, `onFile`, `locate`, `pools`, `hash` (the frozen `Libraries` members). Decisions 34–36, 39.

- [ ] **Step 1: Write failing tests:** `stage_runs_after_probe_for_applicable_files`, `depends_on_orders_stages`, `version_bump_requeues_only_that_stage`, `max_in_flight_per_library`, `offline_and_paused_stop_the_pump`, `cloud_only_skipped_unless_hydrate`, `is_done_skips_the_job`, `poison_item_fails_permanently_for_version`, `failed_job_retried_after_1h_three_times`, `restart_reconciles_queued_and_running`, `same_fp_in_two_libraries_runs_once` (the kind's dedupe), `locate_by_path_alias_fp_hash`, `locate_reports_cloud_only_and_offline`, `file_events_for_every_change`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): the library stage framework and source location`.

### Task 12: Search and lookup — tier: sonnet — query core after Task 7; freshness after Task 10

**Files:** Create `apps/finisherd/src/library/{search.ts, lookup.ts}` (reader-thread side: `catalog/query.ts`). Test: `apps/finisherd/test/library/search.test.ts`.

Decisions 37, 38, 40.

- [ ] **Step 1: Write failing tests:** `terms_are_prefix_and_diacritic_insensitive`, `separators_split_names` (`interview_take3.final.mov`), `filters_kind_duration_size_since_state`, `cursor_pages_without_gaps_or_repeats_across_libraries` (property), `empty_query_lists_recent`, `fts_syntax_in_queries_is_escaped`, `lookup_by_fingerprint_and_hash`, `hash_candidates_submit_one_job`, `search_waits_for_freshness_then_stale`, `offline_hits_marked`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): library name search and fingerprint lookup`.

### Task 13: Contracts, RPCs, scopes, reach and `library.watch` — tier: sonnet — after Tasks 10–12 and 5A Tasks 3–6

**Files:** Create `packages/contracts/src/library.ts`, `apps/finisherd/src/library/rpc.ts`; modify `packages/contracts/src/{rpc.ts, scopes.ts (rules, escalation of human-only set fields), errors.ts, index.ts}`, `apps/finisherd/src/chain/resolve.ts` (`libraryReach`; library roots in `asset.probe`/`asset.add` reach), `apps/finisherd/src/handlers.ts`, `crates/fin-core/src/events/names.rs` (`RPC_METHODS`), `crates/fin/src/output.rs` (codes). Test: `apps/finisherd/test/library/rpc.test.ts`.

Decisions 40–45.

- [ ] **Step 1: Write failing tests:** `rpc_scopes_cover_library_rpcs` (5A's coverage test passes), `human_only_matrix_for_add_remove_and_admin_fields` (every principal kind; only `console` passes, with a challenge), `fix_texts_contain_no_command`, `set_non_human_fields_with_library_scan`, `scan_needs_jobs_submit_too`, `files_refuses_escaping_paths` (`..`, absolute, symlinked-out), `asset_probe_inside_library_root_in_reach`, `asset_probe_of_cloud_only_is_file_cloud_only_without_reading`, `watch_starts_with_snapshot_and_marks_in_use`, `journal_and_audit_carry_no_names`, `exit_codes_mapped`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd,contracts): library RPCs with scopes, reach and a watch stream`.

### Task 14: `fin library`, `fin search` and the console — tier: sonnet — after Task 13, 5A Task 8 and 5C-2

**Files:** Create `crates/fin/src/commands/{library.rs, search.rs}`, `apps/finisherd/src/console/commands/library.ts`; modify `crates/fin/src/{registry.rs, map.rs}` (call-site lines), `apps/finisherd/src/console/main.ts`. Test: `crates/fin/tests/cli_library.rs`, `apps/finisherd/test/console.test.ts` (library cases).

- [ ] **Step 1: Write failing tests:** `ls_status_files_show_search_output_and_json`, `scan_prints_job_and_returns`, `fin_wait_on_scan_job` (5C-2), `set_human_fields_from_fin_is_human_only_exit_7`, `libraries_by_id_or_name`, `console_add_needs_tty_and_code`, `console_add_starts_first_scan`, `console_rm_and_set_hydrate`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(fin,finisherd): fin library and fin search; console library commands`.

### Task 15: e2e `library` scenario and budgets — tier: sonnet — after Task 14

**Files:** Create `scripts/e2e/library.mjs`, `apps/finisherd/scripts/library-budget.ts` (with a generator for 100k files in 5k directories, names in several scripts and normalisations); modify `scripts/e2e/lib.mjs` (console helper for `library add`).

- [ ] **Step 1: e2e:** add a library from the console helper; `fin wait` its scan; `fin library files/search/show`; rename, move, rewrite and delete files, then `fin library scan` and check states; overlay placeholders and a Nextcloud stub stay `cloudOnly` with an open counter of zero; unmount through the overlay → offline, nothing missing; `root_is_byte_identical_after_scan` (a hash of the tree and its metadata before and after).
- [ ] **Step 2: Budgets** (release builds; Task 17 records them on each OS): 100k-file local full scan end to end **≤ 20 s** on the Linux VM (recorded on macOS and Windows); unchanged poll of 5k directories **≤ 1.5 s** local; indexer apply **≥ 25k entries/s**; search p95 **≤ 50 ms** on one 100k library and **≤ 200 ms** over five; event-loop lag p99 **≤ 20 ms** and zero main-thread catalog statements during the scan; catalog **≤ 1 KiB per file** with FTS; worker RSS **≤ 256 MiB**; probe throughput recorded. With `FIN_TEST_SHARE`: a 10k-file scan and an unchanged 1k-directory poll on the share, recorded.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `test(e2e): library scenario; library budgets`.

### Task 16: Spec as built, architecture pointer, tracker — tier: sonnet — after Task 15

**Files:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` §7 "as built (5D)" (the crates and threads, the enumeration fallbacks, Task 3's Cloud Files findings, the deviations: no listing refresh on macOS shares, pools per volume, `ROOT_VOLUME_CHANGED`, nested mounts, bandwidth in params, no-change polls unjournaled, measured numbers), §10 (`unicode-normalization`), §11 references; arch §27 pointer "Sub-project 5D as built" and §20's root mapping (aliases); `TASKS.md`: tick, and follow-ups — sub-project 6 registers `proxy540` and the PCM feed as stages and resolves manifests through `locate`; 8C registers analyzer stages and its catalog chain; 8E replaces the scorer; full hashes of project assets (6/11); per-library reach lists with accounts; sub-project 9's libraries UI on `library.watch`.

- [ ] **Step 1:** Write the notes from the code. **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 5D as built`.

### Task 17: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

On macOS (Apple silicon), Windows 11 (non-elevated) and the Linux VM; host details in `archive/hosts.md`.

- [ ] **Step 1:** `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` on each OS.
- [ ] **Step 2: Real placeholders,** where the host has them: an iCloud Drive folder with "Optimise Mac Storage" evicted files and a dataless directory (macOS); a OneDrive folder with online-only files and folders (Windows); a Nextcloud client in virtual-file suffix mode if available. Add each as a library from the console: the scan and probe passes leave every placeholder online-only (the provider's own status, and no download in its activity view), and the catalog shows them `cloudOnly`.
- [ ] **Step 3: NAS behaviour** with `FIN_TEST_SHARE` (SMB on macOS and Windows, SMB and NFS on Linux): unplug or unmount the share mid-scan → offline, nothing missing, back online → mount scan; a NAS left idle with no library in use sees one poll per 6 h in its access log (or the server's request counters); a sleeping disk's first stat passes within 60 s.
- [ ] **Step 4: Measure** `library-budget.ts` on each OS and record in TASKS and the spec.
- [ ] **Step 5:** Fix what fails (one commit each, with a test); re-run Step 1.
