# Sub-project 5D: The library indexer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics that lose data or wake disks if wrong (per-OS enumeration, placeholder safety, safe opens, volume identity and offline rules, the catalog diff, the scheduler and its polling, the stage framework, validation), `sonnet` for plumbing (the probe refactor, the worker family, the control-store chain, search, RPCs, `fin`, the console, e2e, docs).

**Revision 2** (2026-11-07), after the independent review of `8e1f84e`, with the coordinator's calls. In short: files are opened by their **raw name chain**, never by the normalised key, and keys may collide (flagged, `PATH_AMBIGUOUS`); volume identity is defined per OS and a change is confirmed by file ids; **every reader of a library file goes through `open_for_read`**, enforced by clippy `disallowed-methods` in five crates; catalog extensions are modules loaded by URL in both threads; Windows skips only name-surrogate reparse tags; automounts are not descended; polls get their own slot per volume; offline failures cost no stage attempts; aliases are human-only and never grant reach; Task 7 is split. The table at the end maps every finding to its resolution.

**Goal:** spec §7 (5D), on top of 5C-1's engine. After this plan:
- **`fin-library`** (new crate) enumerates a root **without opening a file**: Linux `getdents64` + `statx`, macOS `getattrlistbulk` with dataless materialisation off, Windows `FILE_ID_EXTD_DIR_INFO`; it classifies placeholders (macOS dataless files and directories, iCloud stubs, Windows recall attributes and cloud reparse tags, Nextcloud/ownCloud suffix files), reads volume identity and mount state, opens files **only** through one safe opener that every reader in the server uses, and has a fake filesystem for tests;
- **`fin-workers library`** runs `library.scan`, `library.probe` and `library.hash` jobs on the shared engine (spec §6.0: 5D contributes only this family);
- **`finisherd`** keeps libraries in `control.db` (the `libraries` chain) and one **catalog** per library (`<server-data>/libraries/<id>/catalog.db`), written only by an **indexer `worker_thread`** and read by a **reader `worker_thread`**; it polls NAS roots on demand (60 s in use, 6 h otherwise, a freshness pass before answers), watches local roots as a hint, never marks files missing for an unavailable root, runs the **stage framework** that sub-projects 6 and 8 register into, and answers FTS5 name search and fingerprint lookup;
- `library.*` RPCs with 5A's scopes, `fin library …`, `fin search`, the console's `library add|rm|set`, and the `library.scan` diagnostics kind.

**Not in this plan:** proxy, PCM, analyzer and embedding stages (sub-projects 6, 8: they call `registerStage`); semantic and transcript search (8E, 8C: their catalog tables come as extensions, Decision 24); per-library reach lists (v1 reach is the `library.read` scope, Decision 47); device principals (5B); remote ingest; full hashes stored in the project model (a TASKS follow-up for 6/11).

**Architecture:**
- **Rust:** `crates/fin-library` (new: types, raw paths, names and collisions, placeholders, excludes, volumes, `Source` per OS, walker, the safe opener, probe and hash entry points, the dataless policy, fake filesystem); `crates/fin-probe` (`probe_reader`, `probe_with`, `PROBE_VERSION`; Task 5); `crates/fin-workers/src/library/` (the family); `crates/fin-node/src/library/` (napi: root canonicalisation, identity, per-root probe threads, safe asset probes); `crates/fin-core/src/events/` (`library.scan`, job kind names); `clippy.toml` in `fin-library`, `fin-workers`, `fin-node`, `fin-media`, `fin-analyze`.
- **TypeScript:** `packages/contracts/src/library.ts`; `apps/finisherd/src/control/migrations/libraries/0001.ts`; `apps/finisherd/src/library/` (registry, service, poll planner, usage, kinds, stages, locate, rpc, `catalog/` with extensions, schema, indexer and reader threads, diff, search); `apps/finisherd/src/console/commands/library.ts`; `crates/fin/src/commands/{library.rs, search.rs}`.

**Tech Stack:** Rust 1.99.1, edition 2024; `libc` 0.2 (MIT OR Apache-2.0, in the tree); `windows-sys` 0.61 (MIT OR Apache-2.0, in the tree) with `Win32_Foundation`, `Win32_Storage_FileSystem`, `Win32_System_IO`, `Win32_Security` and, **dev-only**, `Win32_Storage_CloudFilters` (Task 3's real placeholder test); **`unicode-normalization` 0.1** (MIT OR Apache-2.0, new: NFC keys); `blake3` 1, `serde`, `schemars` 1, `base64` 0.22 (in the tree). Node 24 APIs only (spec §1.6): `node:sqlite` with FTS5 (`contentless_delete` needs SQLite ≥ 3.43; Node 24.22 ships 3.53), `worker_threads`, `fs.watch`; Effect 4.0.6; vitest; `fast-check` 4 (dev, already pinned by 5C-1). **No other dependency.** Rejected: `notify` (Node's watcher suffices for a hint), `globset` (a 150-line matcher, Decision 16), `walkdir`/`jwalk`/`ignore` (they `stat` through `std`, which opens files on Windows), `rusqlite` in the indexer (catalogs stay on `node:sqlite` like `control.db`).

**Spec:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` revision 4: §3.1 (chains, budgets), §3.3 (`library.*` audit kinds), §3.4 (`library.scan`), §4.5–4.7 (scopes, human-only, console), §6.0–6.9 (the engine 5D runs on), §7, §10, §11 Decisions 25–27, §12. Arch §6.7 (libraries), §20 (relinking, per-server root mapping). Sub-project 6 spec §3.2–3.3, §6.3 (shallow probe in `library.probe`), §7.1 (`proxy540` as a library stage); sub-project 8 spec §8 (analyzer stages, artifacts keyed by fingerprint), §10 (`libraries/<id>/vectors/`), §16.2 (what 8 needs from 5D). Plans: 5A (Tasks 1, 3–6, 8; Decision 19), 5C-1 (frozen interfaces), 6A (Decision 5: library-root mapping comes with 5D).

**Branch:** `sp5d-library-indexer`, stacked on `sp5c1-job-engine`; rebased onto 5A's Task 6 and onto 5C-2 when batch C starts, and onto `main` after 6A-1 merges (Task 5 needs it).

**What this plan uses (frozen elsewhere):**
- **5A:** `ControlStore`, `registerChain`, `Migration`, `ChainVersions`, `Audit`, `ensurePrivateDir` (Tasks 1–2, merged); `Principal`, `RPC_SCOPES`, `withChain`, resolvers, `defaultScopes`, protocol 2 (Tasks 3–6); the console's `Prompt` and challenge flow (Task 8). The scopes `library.read`, `library.scan`, `library.admin` (human) and `jobs.submit` already exist in `SCOPES`.
- **5C-1:** `KindSpec`, `FamilySpec` (`onCall`), `BatchSpec`, `PrepareCtx`/`SucceededCtx`, `resourceHealthy`, `JobEngine` (`submit`, `get`, `cancel`, `definePool`, `onDone`, `events`), `fin_jobs::worker::{Family, JobCtx}` (`call`, `begin`/`item`, `progress`, `checkpoint`, `cancel_token`), `ReadBudget`, `JobError`. **5D needs no change to any frozen 5C-1 interface** (bandwidth travels in params, Decision 36; pools come from `definePool`).
- **5C-2:** `jobs.wait` and `fin wait` (only `fin library scan`'s e2e, Task 15). D4's `Diagnostics.journal` (on the base branch).
- **fp1** is `fin_probe::fingerprint` (it lives in `fin-probe`, not `fin-store`); from `fin-store` 5D reuses `storage_kind::detect`, `conflict_names::classify` and the placeholder knowledge of `fsio::placeholder`.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code; `pnpm gates` runs cargo-deny and the JS licence gate over what each task adds. Test corpora are generated, never downloaded.
- **Primitive ops live only in `fin-core`.** This plan adds none; `fin-core` gains only an event payload and names.
- **TypeScript model types are generated, never hand-written.** Rust → TS (`pnpm --filter @finisher/core-types generate`, drift gate `scripts/check-generated-types.mjs`): `RawRel`, `Expect`, `ScanEntry`, `DirDone`, `ScanBatch`, `ProbeOutcome`, `VolumeIdentity`, `RootState`, `Placeholder`, the three kinds' params and results, `LibraryScan` (journal). TS-owned records (`LibraryView`, RPC payloads, catalog rows) are Effect Schemas in `@finisher/contracts`.
- **Read-only roots:** nothing is ever created, written, renamed, locked or deleted under a library root, by any process, in any build (Decision 31 makes this override spec §7.2's macOS listing refresh). Tests write their trees from the test process only.
- **Frozen rule: every reader of a library file, in every process and sub-project, opens it with `fin_library::open_for_read`** (Decisions 19–21), by its raw path and with its `expect`; no code under a root uses a path-based `std` call. Clippy `disallowed-methods` enforces it in `fin-library`, `fin-workers`, `fin-node`, `fin-media` and `fin-analyze` (Decision 21).
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit: `crates/fin-core/src/events/names.rs` 504 (RPC names: if it passes 550, move `RPC_METHODS` into `names_rpc.rs` first), `kinds.rs` 370, `crates/fin/src/registry.rs` 377. Each OS's enumeration and opener is its own module; a file passing 550 lines is split before the commit.
- Errors are `{code, message, details, fix?}`; new codes go into `crates/fin/src/output.rs` and the contracts' list. Human-only fixes address the user and contain no command (spec §3.5).
- **Privacy:** library names, roots, aliases, file names and paths are user content: never in the journal, the audit log, logs at info level, or errors' `message` (paths go in `details.path` of local errors only).
- **The repo is public:** no host names, IPs, user names or share names in tracked files; network tests read their share from the environment (`FIN_TEST_SHARE`, as 4A).

**Every commit is green** on macOS, Windows 11 and Linux. Tests that need an OS facility that may be absent (user namespaces for bind mounts, `hdiutil`, an iCloud account) skip with a notice naming the facility; nothing else skips.

**Testing and process**
- Tests never use the network, except the opt-in share tests (`FIN_TEST_SHARE` set). Timers come from injected clocks (Rust `Timing`, Effect `TestClock`); worker timeouts are scaled through test-hooks params, never slept.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk cargo clippy --workspace --all-targets` (`-D clippy::disallowed_methods` from Task 6 on), `rtk pnpm test` and `rtk pnpm gates`; Tasks 13–16 also `rtk pnpm e2e`.
- Commits: Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Shell commands prefixed with `rtk`, after `source .superpowers/env.sh`.
- **Tracker:** Task 1 adds "Sub-project 5D: library indexer" to `TASKS.md`, one line per task; each task ticks its line with as-built notes in its own commit.
- **Batches and order** (against 5A, 5C-1, 5C-2 and 6A-1):
  - **A, can start now** (nothing beyond the base and 5A Tasks 1–2, already on the stack): Task 1 → Tasks 2 and 3 in parallel → Task 4; Task 7a after Task 1 (generated types) → Task 7b; Task 8; Task 12's query core after Task 7a.
  - **Task 5 waits for 6A-1's merge** (hard dependency: both rewrite `fin-probe`'s entry points, Decision 35).
  - **B, after 5C-1 Tasks 1–2** (merged) and Task 5 for Task 6, and **after 5C-1 Tasks 7–10** (kinds, run loop, isolation, `resourceHealthy`) for Task 9 → Task 10 → Task 11.
  - **C, after 5A Tasks 3–6** (principals, chain, scopes, protocol 2): Task 13; **after 5A Task 8** (console) and **5C-2** (`fin wait`): Task 14 → Task 15.
  - **D:** Task 16 → Task 17.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Crates, processes and threads

1. **`fin-library`** holds everything that touches a root: types, raw paths, names, placeholders, excludes, volumes, the per-OS `Source`, the walker, the safe opener, probe and hash entry points, `process::deny_dataless_materialisation()`, and (feature `test-hooks`) the fake filesystem. It creates no process. `fin-workers` (the `library` family, and 6's `media` and 8's `analyze` families when they read originals) and `fin-node` (root probes, asset probes of library files) link it; `finisherd` never touches a root from JS (no `fs.*` on library paths; the one exception is `fs.watch`, Decision 30).
2. **Threads in `finisherd`:** the main thread holds the `libraries` table and the scheduler and **never runs a catalog statement or parses a batch**; **`library-indexer`** (one `worker_thread`) is the only writer of every catalog, reads and validates batch files, and hosts the watchers; **`library-reader`** (one `worker_thread`, read-only connections, WAL) serves search, listings and lookups, so answers never wait behind a write. Only structured-clone data crosses a `MessagePort` (paths, ids, rows); each indexer transaction is ≤ 2,000 rows (≤ 50 ms) so libraries interleave.
3. **Root probes** (`fin-node` `library::probeRoot`) run on **one dedicated OS thread per root**, never on libuv's pool: a stat that hangs on a dead NFS hard mount blocks only that thread. While one probe of a root is outstanding no other is issued; a probe past its deadline answers `timeout` and the thread is left to finish (counted in `status().stuckProbes`).

### Enumeration (never opens a file)

4. **`Source` trait** (`fin-library::source`): `open_dir(&RawRel) -> DirHandle` (opened without following links), `read_dir(&DirHandle) -> Vec<RawEntry>` (raw name bytes, type, size, mtime ns, file id, device, attributes, reparse tag, automount flag), `stat_root() -> RootStat`, `volume(&DirHandle) -> VolumeIdentity`. Implementations: `linux`, `macos`, `windows`, and `fake` (test-hooks). Directories are always reached by their raw component chain from the root handle.
5. **Linux:** `openat(dirfd, raw_name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC | O_NOATIME)` per directory (`O_NOATIME` dropped after one `EPERM`, where we do not own the files), entries by `fdopendir`/`readdir` (`getdents64`; it neither opens nor stats an entry), then `statx(dirfd, name, AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT, STATX_TYPE | STATX_SIZE | STATX_MTIME | STATX_INO | STATX_MNT_ID)` per entry. Symlinks are `skipped` and never followed. **`STATX_ATTR_AUTOMOUNT` entries are nested mounts and are not descended** (recorded `excluded` with code `AUTOMOUNT`), so a scan never triggers an autofs or systemd automount. **Mount roots** come from `STATX_ATTR_MOUNT_ROOT` (kernel ≥ 5.8; the floor is 6.2), which also sees bind mounts that `st_dev` cannot.
6. **macOS:** `openat(O_RDONLY | O_DIRECTORY | O_NOFOLLOW)` and `getattrlistbulk` with `ATTR_CMN_RETURNED_ATTRS | NAME | ERROR | OBJTYPE | MODTIME | FLAGS | FILEID | DEVID` and `ATTR_FILE_DATALENGTH`, `FSOPT_PACK_INVAL_ATTRS`, a 256 KiB buffer. Every process that may open a library file calls `deny_dataless_materialisation()` first: `setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_PROCESS, IOPOL_MATERIALIZE_DATALESS_FILES_OFF)` (declared locally if `libc` lacks it), so any accidental access fails `EDEADLK` instead of downloading; only a cold job of a `hydrate: true` library turns it back on for its own process. Mount roots: `st_dev` differs from the parent's, or `statfs.f_mntonname` equals the path.
7. **Windows:** `CreateFileW(dir, FILE_LIST_DIRECTORY | SYNCHRONIZE, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)` with `\\?\` paths built from raw names, then `GetFileInformationByHandleEx(FileIdExtdDirectoryRestartInfo` then `FileIdExtdDirectoryInfo`, 64 KiB buffer): size (`EndOfFile`), `LastWriteTime` (100 ns → ns since the Unix epoch), attributes, the reparse tag and the 128-bit `FileId`, from the directory handle alone. **Fallback chain**, recorded per root in `VolumeIdentity.enumeration` and warned as `ENUMERATION_FALLBACK`: `FileIdExtdDirectoryInfo` → `FileIdBothDirectoryInfo` (64-bit ids; older SMB servers) → `FileFullDirectoryInfo` (no ids), chosen at the first `ERROR_INVALID_PARAMETER`/`ERROR_NOT_SUPPORTED`. **Where the reparse tag lives:** `ReparsePointTag` in `FILE_ID_EXTD_DIR_INFO`; in `FILE_ID_BOTH_DIR_INFO` and `FILE_FULL_DIR_INFO` the `EaSize` field holds the tag when `FILE_ATTRIBUTE_REPARSE_POINT` is set (as `WIN32_FIND_DATA.dwReserved0` does).
8. **Reparse tags (Windows):** only **name-surrogate tags** (`IsReparseTagNameSurrogate`: symlinks, junctions and mount points, and any other tag with the surrogate bit) are `skipped` and never followed. **Data-bearing tags** (`IO_REPARSE_TAG_CLOUD*` of a hydrated file, `DEDUP`, `WOF`, other non-surrogate filter tags) are ordinary files when no recall or offline bit is set; with one, Decision 12 applies.
9. **Parallelism** (spec §7.2): directory reads run on a pool of 32 threads for network and sync roots, 64 for local ones, breadth-first by depth then raw name, so a scan's order is deterministic and resumable (Decision 27).

### Names, keys and collisions

10. **Raw names are authoritative.** Each catalog row keeps its component's raw bytes (`name BLOB`: Unix bytes; Windows UTF-16 as WTF-8); a file's path is its directory chain of raw names plus its own, and **every open uses that chain** (`RawRel`), never `relKey` or a display string. `nameNfc` (NFC via `unicode-normalization`; lossy names get `U+FFFD` and `nameLossy: true`) is for display; **`relKey`** (NFC path with `/` separators, lower-cased with `str::to_lowercase` when the root is case-insensitive) is a lookup and search key only, with a plain index. Case sensitivity per root, without writing: macOS `pathconf(_PC_CASE_SENSITIVE)`, Windows `FILE_CASE_SENSITIVE_SEARCH`, Linux insensitive for `vfat`/`exfat`/`ntfs3` and `cifs` mounted `nocase`, else sensitive.
11. **Collisions** are expected, flagged and never fatal: within a directory, two raw names whose `relKey`s are equal (NFC/NFD twins, case twins that a Samba share on a case-sensitive filesystem exposes, two lossy names, names `to_lowercase` folds together while NTFS's upcase table or APFS's folding keeps them apart, or the reverse) both keep their rows, each gets `collision` bits (`nfc | case | lossy`), and the library warns `NAME_COLLISION {count}`. A **path lookup** (`library.file {path}`, `locate`, asset reach) whose key matches several rows answers **`PATH_AMBIGUOUS`** (3) with `details.fileIds`; callers then address by `fileId`. A collision never fails a batch: uniqueness is `(dirId, name)` on raw bytes (Decision 23).

### Placeholders

12. **Classification** (`fin-library::placeholder`, pure over `RawEntry`), result `Placeholder = dataless | icloudStub | nextcloud | owncloud | recallOnOpen | recallOnData | offline | unpinned`:
    - macOS: `SF_DATALESS` in `ATTR_CMN_FLAGS` on files **and directories**; `.<name>.icloud` stubs stand for `<name>`;
    - Windows: `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `RECALL_ON_OPEN`, `OFFLINE`, and `UNPINNED` (an online-only file even when the other bits are clear), on any tag (Decision 8); `PINNED` with none of those bits is hydrated;
    - every OS: `<name>.nextcloud` / `<name>.owncloud` stand for `<name>`; when both the stub and `<name>` exist, the real file wins and the stub is ignored; a stub is never probed.
13. **Placeholders are catalogued from metadata only** (state `cloudOnly`, size from the entry). A placeholder **directory** (dataless, or a Windows directory with a recall bit) is recorded `cloudOnly` and **not descended**; files catalogued under it earlier keep their rows and become `cloudOnly`, never `missing`.
14. **Linux FUSE sync roots without markers default to metadata only** (state `skipped`, code `METADATA_ONLY`): nothing under them is opened unless the owner sets `hydrate: true` (then probes run within `bandwidthMiBs`). This deviates from spec §7.3's "probed only within the bandwidth budget" (a FUSE client downloads on any read, and a default budget still downloads); Task 16 records it.

### Volumes, mounts and offline

15. **`VolumeIdentity {key, kind, devId, mountRoot, caseSensitive, enumeration, remote, rootFileId, sample}`**, measured at add and refreshed at each full scan:
    - **macOS:** local volumes `ATTR_VOL_UUID` (`getattrlist` on the root); network volumes (`smbfs`, `nfs`, `afpfs`, `webdav`) `statfs.f_mntfromname` with any `user[:pass]@` stripped, plus `f_fstypename`;
    - **Linux:** the `/proc/self/mountinfo` line of the root's `stx_mnt_id`; for `nfs`, `nfs4`, `cifs`, `smb3` and `fuse.*` the mount source (user part stripped) plus the type; for local filesystems `statfs.f_fsid` plus the type;
    - **Windows:** the 64-bit volume serial (`FileIdInfo`), plus `\\server\share` for UNC paths and mapped drives (from `GetFinalPathNameByHandleW`'s `\\?\UNC\` form);
    - `key` = BLAKE3-16 of the OS's fields (never stored raw in the journal); `rootFileId` and `sample` = the root's file id and up to 16 top-level entries' file ids, sizes and mtimes.
16. **Excludes** (spec §7.1 defaults plus the library's `exclude`/`include` globs over `relKey`): dot-files and directories, `Thumbs.db`, `desktop.ini`, `.DS_Store`, `@eaDir`, `#recycle`, `$RECYCLE.BIN`, `System Volume Information`, `.nfs*`, `.fuse_hidden*`, sync temp files, conflict copies per `fin_store::conflict_names::classify`, automounts (Decision 5), and inside a directory holding `finisher.project.json` everything but `sources/`. Globs: `*`, `?`, `**`, `[…]`, case per the root; an own matcher (≤ 150 lines). **A catalogued file or directory that becomes excluded** (an `exclude` glob added, an automount appearing) goes `missing` with code **`EXCLUDED`**, a directory with its whole subtree, but only once the listing that reported it excluded closes completely (never while offline, never from an incomplete listing); `EXCLUDED` rows are never the old row of a move by fingerprint or of a `dev:fileId` rename (their files still exist), and are purged like other missing rows (Task 7b review).
17. **`RootState`** (fin-node probe and the worker's first stat) = `online | unreachable | unmounted | volumeChanged | empty | timeout`. **Unmounted:** the root was a mount root and is not now (Linux: no `STATX_ATTR_MOUNT_ROOT`; elsewhere `st_dev` equals the parent's); the empty mount-point directory is never scanned. **`volumeChanged`** needs both a different `key` **and** a failed confirmation: the root's file id differs, or fewer than half of the sampled top-level entries still match by file id, size and mtime. A key change that passes confirmation (a server renamed, a share remounted under another source string) updates the key silently and is logged at debug; `devId` alone is never compared. A confirmed change puts the library offline with `ROOT_VOLUME_CHANGED` until the owner accepts the new volume (`library.set {acceptVolume: true}`, human-only). **Nested mount points** below a root are walked and catalogued, and tracked per directory (`dirs.mount`, with the mount's volume identity: the walker measures it after opening a mount root and reports it as `DirDone.volume`, the indexer hands it back as `KnownDir.volume`); one that unmounts makes its subtree `offline` (`ROOT_UNMOUNTED {dir}`), one that now holds another volume (a different key that its root file id and sample do not confirm, the rule above) is `ROOT_VOLUME_CHANGED {dir}`; neither is ever `missing`.
18. **Offline rules** (spec §7.2, arch §6.7): a root that is unreachable, unmounted, volume-changed or timed out, and a full scan whose root listing is empty while the catalog holds present files (`ROOT_EMPTY_SUSPECT`), put the library `offline` with catalog and stage states intact; **no file is ever marked missing in an offline library, and none unless its parent directory was listed completely** (Decision 25). Offline libraries are re-probed every 60 s by fin-node (Decision 3); a return to `online` triggers a full scan (`trigger: mount`). Their files stay listed and searchable with `offline: true`. **Spin-up tolerance:** the first stat of a root waits up to **60 s** before `timeout`; per-directory reads and per-entry stats up to **30 s**, after which that directory is `unreadable` (`code: TIMEOUT`) and the scan goes on; a stuck read thread is left behind and the cold worker exits at job end (5C-1 abandons it if it cannot). Scans report `progress` per directory, so the scan kind's `stallS` (10 minutes) fires only for a scan that is truly stuck.

### Opening library files

19. **`open_for_read(root, rel: &RawRel, expect: &Expect, hydrate) -> Result<File, OpenError>`** (frozen), the only open of a file under a root:
    1. **re-check without opening** (Linux `statx(AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT)` from the parent fd; macOS `getattrlist(FSOPT_NOFOLLOW)`; Windows `FindFirstFileExW(FindExInfoBasic)` on the exact path, a directory read): a placeholder is `FILE_CLOUD_ONLY` unless `hydrate`; a name-surrogate tag is `INVALID_PATH`;
    2. **open without following links**: Linux `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS`, `O_NOATIME` when we own the file) from the root fd, falling back on `ENOSYS` or `EPERM` (seccomp in containers) to **per-component `openat(O_NOFOLLOW | O_DIRECTORY)`** with an `fstat` check that each component is a directory; **nested mounts are crossed** (they are catalogued, Decision 17), so the opened handle must be on the row's device (`Expect.dev`, `Expect::check_volume`: another device is `Changed`), and a row without a device on the root's mount (`Escapes`); macOS `O_NOFOLLOW_ANY`; Windows `CreateFileW(GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, OPEN_EXISTING, FILE_FLAG_SEQUENTIAL_SCAN)` (no `FILE_FLAG_OPEN_REPARSE_POINT`, so data-bearing filters such as dedup and WOF serve the data), then `GetFinalPathNameByHandleW` must lie under the root and `FileAttributeTagInfo` must show no name-surrogate tag and no recall bit;
    3. **identity:** the handle's file id (if `expect` has one), size and mtime must equal `expect`; a mismatch is `OpenError::Changed` (its directory is rescanned), never a read.

    The residual window between (1) and (2), in which a provider could evict a file, is documented.
20. **Every reader goes through it** (frozen rule): `StageFile` and `Located` carry `root`, `rel` (raw), `display`, `expect {size, mtimeNs, fileId}` and `hydrate`, so sub-projects 6 and 8 open originals exactly as 5D does; their `media` and `analyze` families call `deny_dataless_materialisation()` at start (a family contract, tested in Task 6 for `library` and noted in TASKS for 6 and 8). `fin-probe` gains **`probe_with(path, open)`** so `asset.probe`/`asset.add` of a library file run in fin-node through `open_for_read` with the catalogued row (Decision 47); `probe(path)` stays `probe_with(path, File::open)` for project files.
21. **The gate: clippy `disallowed-methods`** in `clippy.toml` of `fin-library`, `fin-workers`, `fin-node`, `fin-media` and `fin-analyze` (each added when the crate exists; `scripts/check-clippy-config.mjs` fails when a listed crate exists without it): `std::fs::{File::open, OpenOptions::open, metadata, symlink_metadata, canonicalize, read, read_to_string, read_dir, read_link}`, `std::path::Path::{exists, try_exists, is_file, is_dir, is_symlink, metadata, symlink_metadata, canonicalize, read_dir, read_link}`, `dunce::canonicalize`. **Allow-lists** are explicit: every `#[allow(clippy::disallowed_methods)]` (item or module) is listed with a reason in `scripts/clippy-allow.json`, and the script fails on an unlisted one; the per-OS `source/` and `open/` modules of `fin-library` and fin-node's project-store modules (which never receive library paths) are the expected entries.
22. **Hard links and shared file ids:** two rows may share a file id (hard links, or a server that reuses ids); they are flagged `hardlink`, never merged, and file ids are hints keyed with the device (`dev:id`) for rename detection only (Decision 25). SMB servers that answer 0 or unstable ids degrade to size-and-mtime matching.

### Catalogs and the diff

23. **Catalog schema** (`core` extension, step 1, frozen once merged; `meta`/`migrations(ext, n)` as in `control.db`):
    ```sql
    CREATE TABLE dirs (id INTEGER PRIMARY KEY, parent INTEGER, name BLOB NOT NULL, nameNfc TEXT NOT NULL,
      relKey TEXT NOT NULL, mtimeNs INTEGER, fileId BLOB, devId TEXT, mount TEXT, placeholder TEXT,
      collision INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL, code TEXT, emptySince INTEGER,
      listedGen INTEGER, lastSeen INTEGER NOT NULL, UNIQUE (parent, name));
    CREATE INDEX dirs_key ON dirs (relKey);
    CREATE TABLE files (id INTEGER PRIMARY KEY, dirId INTEGER NOT NULL REFERENCES dirs (id), name BLOB NOT NULL,
      nameNfc TEXT NOT NULL, nameLossy INTEGER NOT NULL DEFAULT 0, relKey TEXT NOT NULL, ext TEXT,
      collision INTEGER NOT NULL DEFAULT 0, hardlink INTEGER NOT NULL DEFAULT 0, size INTEGER NOT NULL,
      mtimeNs INTEGER NOT NULL, fileId BLOB, placeholder TEXT, fp TEXT, fullHash TEXT, kind TEXT, probe TEXT,
      probeVersion INTEGER, durationMs INTEGER, width INTEGER, height INTEGER, state TEXT NOT NULL, code TEXT,
      renameOf INTEGER, firstSeen INTEGER NOT NULL, lastSeen INTEGER NOT NULL, lastChange INTEGER NOT NULL,
      missingSince INTEGER, UNIQUE (dirId, name));
    CREATE INDEX files_key ON files (relKey);
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
    File `state`: `present | missing | cloudOnly | unreadable | probeFailed | skipped | offline`; dir `state`: `present | missing | cloudOnly | unreadable | excluded | offline`. `meta.libraryId` must equal the directory's id (a catalog copied from another library is moved aside). `PRAGMA quick_check` at open; a damaged catalog is moved to `catalog.db.corrupt-<ulid>` and rebuilt by a full scan (stage results survive in the caches, keyed by fingerprint). An extension whose `minReaderVersion` is above ours puts the library offline with `CATALOG_TOO_NEW`. **Size:** Task 7a measures bytes per file with the FTS `dir` column on the 100k tree; if it passes the 1 KiB budget, directory tokens move to a `dirnames` FTS table (rowid = `dirs.id`) that search joins, and Task 7a records which.
24. **Catalog extensions are modules registered by URL** (frozen; 8C's transcripts, 8E's chunk tables): `registerCatalogExtension({name, module: URL})` from the main thread; **both threads `import()` the module** at start, and only the name and URL cross a port. The module exports `{migrations, versions?, ops, queries}`: `migrations` run by the indexer when a catalog opens; `ops[name](db, args)` run in the indexer inside a transaction (writes); `queries[name](db, args)` run in the reader on its read-only connection. Callers use `IndexerClient.exec(lib, ext, op, args)` and `ReaderClient.query(lib, ext, op, args)`; `args` and results are structured-clone data; **no function crosses a `MessagePort`**. 5D's own schema is the `core` extension, loaded the same way.
25. **The diff** (indexer, per `ScanBatch`, one transaction). A row is identified by `(dirId, raw name)`:
    - each `DirDone {complete: true}` closes **one listing** of that directory (all its entries precede it in the job's batches): rows of the directory absent from it are **missing**, and so are subdirectories absent from it with their subtrees; `complete: false` (an error, a timeout, a placeholder or automount directory, a nested mount unmounted or replaced, a partial listing `ENTRY_UNREADABLE` whose readable entries precede it) marks nothing; a nested mount closed `ROOT_UNMOUNTED`/`ROOT_VOLUME_CHANGED` makes its subtree `offline` and no listing under it marks anything until the mount point lists completely again (Decision 17); excluded rows as Decision 16;
    - **listing time** (Task 7b review): a listing is read before it is applied, so its `start` is the earlier of its first entry's transaction time and the job attempt's **`startedAt`** (`IndexerClient.attemptStart`, taken by the scan kind before the attempt reads anything and passed to every `applyBatchFile` of the attempt); only rows with `lastSeen < start` are marked (a row a later poll saw stays), and a listing records `mtimeNs`/`listedGen` only when it started no earlier than the one recorded (`dirs.listedAt`). The batch format is unchanged (no read time per `DirDone`);
    - **a listing that drops from N > 0 entries to 0 is incomplete** (`DIR_EMPTY_SUSPECT`; `emptySince` = its apply time): nothing is marked; an empty listing from another job whose `start` is at least 60 s later confirms it, and any entry applied under the directory in between clears the suspicion. Catalog time never runs backwards (`max(clock, lastAt)`) and the default clock holds forward jumps to the monotonic pace for 10 minutes;
    - a new entry whose `dev:fileId` matches a row elsewhere with equal size and mtime records `renameOf`; when that old row becomes missing (its directory is listed without it), the old row takes the new path and keeps its id, fingerprint and history; if the old directory lists it too, both rows stay as **hard links** (Decision 22);
    - a known row with a different size, mtime or file id is **changed** (`fp`, `probe`, `probeVersion` cleared; `lastChange` set);
    - after a probe, a new row whose fingerprint equals a **missing** row's in the same library is a **move**: the missing row takes the new path and the new row is dropped;
    - a missing file that reappears with its old size and mtime is `present` again with its fingerprint; collisions are recomputed per closed listing (Decision 11);
    - rows missing for **90 days** are deleted in batches of ≤ 500 (their stage rows when no other row has the fingerprint), and only once a later full-scan generation has run (`files.missingGen < beforeGen`), so a clock jump alone purges nothing;
    - a subtree change (a vanished, excluded or placeholder directory) is one transaction written in steps of ≤ 2,000 rows with event-loop turns between them: readers see it whole, other libraries interleave.
26. **Batches over the envelope** (5C-1 Decisions 4, 5a): the scan worker writes up to 4,096 entries (≤ 8 MiB) per NDJSON file in its job's `out/`, then `call library.batch {file, seq}`; the family's `onCall` checks only that `file` is a plain name inside the job's `out/` and passes **the path** to the indexer, which opens it (no-follow, regular file, size cap), parses and validates it against the generated `ScanBatch` schema, applies it, and only then lets `onCall` reply. The main thread never reads or parses a batch; a bad file fails the job and writes nothing.
27. **Generations and resume:** the service allocates a generation per full scan (`params.gen`); full scans are `resumable`. After a restart the kind's `prepare` asks the indexer to write `in/dirs.ndjson` (each known directory's raw chain, `mtimeNs`, and whether it was listed in this generation); the worker skips listings already done but still descends through them.

### Polling and watching

28. **Network roots, `poll: demand`** (spec §7.2, spec §11 Decision 27): a `library.scan {mode: poll}` job re-stats every known directory and re-lists those whose mtime changed, **every 60 s while the library is in use** (a project that references it is open, a `library.watch` stream is open, or it was searched, listed or scanned in the last 10 minutes) and **every 6 hours otherwise**. Before a search or listing of a network or sync library whose last poll is older than 5 minutes, the service submits an `interactive` poll of the directories the answer touches (a listing: that directory; a search: all) and waits ≤ 5 s; past that the answer says `stale: true` and the poll goes on. The owner may set a fixed interval. A daily full scan (spec default for network roots) runs at the first poll tick after it is due, so an unused NAS wakes at most at the 6-hour ticks.
29. **Sync roots** poll every 10 minutes and scan fully daily; **local roots** scan fully weekly.
30. **Local roots: `fs.watch` (recursive) in the indexer thread** is a hint: events (or a `null` file name) queue a targeted rescan of their directories after 2 s of quiet; an `error`/`ENOSPC`/overflow switches the library to 5-minute polling with `WATCH_DEGRADED` until the next restart. On Linux Node's recursive watch walks the tree synchronously, which is why it lives off the main thread.
31. **macOS SMB listings are not refreshed by writing** (spec §7.2 says "as fin-store does", which creates and deletes a file in the directory): libraries are read-only, so on macOS shares a change can take up to 60 s (smbfs `dir_cache_max`) longer to appear. NFS attribute caches (`acdirmax`, default 60 s) behave the same on Linux. Task 16 records the deviation.
32. **Pools** (spec §6.3) per **volume key**, so two libraries on one NAS share its disks: `io:<key>.scan` (1: full scans), **`io:<key>.poll` (1: polls, freshness passes and watch rescans)** and `io:<key>.read` (2 for network and sync, 4 for local: probes, hashes and every stage that reads originals), defined with `definePool` when a library is added or loaded (`libraryPools(id)` is frozen for 6 and 8). *Why a separate poll slot rather than injecting directories into a running full scan:* a full scan of a large NAS runs for hours, and the 60 s cadence and the 5 s freshness bound cannot wait for it; polls are metadata-only and short, so one extra concurrent walker per volume is a bounded cost, while feeding live priority directories into a walker would couple its deterministic, resumable order (Decision 27) to outside input. A poll and a full scan may list the same directory; each listing is closed by its own `DirDone` (Decision 25), so the diff stays correct.
33. **Offline from a job:** a scan or probe worker that finds the root unreachable answers `error {code: LIBRARY_OFFLINE, retry: never}`; the service puts the library offline and re-probes it (Decision 18), instead of the engine's backoff piling up retries. `resourceHealthy` of every library kind answers the library's cached root state, re-probed within 5 s when older than 60 s; `RESOURCE_STUCK` on its pools also makes it offline.

### Probe, fingerprint and hash

34. **`library.probe`** (batch ≤ 256 files; items = catalog file ids; `utility` QoS; `io:<key>.read` + `cpu`) opens each file once through `open_for_read` and computes **`fp1` first, then the shallow probe**, from the same handle: a file whose probe fails keeps its fingerprint (`probeFailed`, `code`). Results go to the catalog through `call library.probed {results ≤ 32}` (then `item` acks), not item values, so `job_items` in `control.db` stays small. A crash isolates the in-flight file (5C-1 Decision 32); `POISON_ITEM` is recorded per file and not retried until its size or mtime changes.
35. **`fin-probe` changes (Task 5, after 6A-1 merges):** `probe_reader(r: impl Read + Seek, meta: FileMeta, path) -> ProbeOutcome {fingerprint, probe}`, `probe_with(path, open)` (Decision 20), and **`PROBE_VERSION`, created here** (`= 1`), stored per file; a new version re-queues only the probe stage. **Note for sub-project 6's plan:** 6A-2 and later bump `PROBE_VERSION` whenever probe output changes (TASKS item, Task 16).
36. **Bandwidth** (spec §6.2): `bandwidthMiBs` reaches the worker in params (`readBps`, split evenly over the pool's slots) and wraps the file in a `BudgetedReader` over `fin_jobs::ReadBudget`; enumeration is not budgeted (metadata only).
37. **Probe candidates:** the catalog lists every non-excluded file; only files whose extension is in the library's `kinds` table (default `media`, `image`, `svg`, `lottieTemplate`, `font`, `lut` extensions, case-insensitive; `kinds: all` probes everything) are probed, the others are `skipped`. The extension only filters: the recorded kind comes from the content.
38. **`library.hash`** (batch ≤ 64; items = file ids; whole-file BLAKE3 through `open_for_read` and the budget; item values `blake3:<hex>`, written by the kind's `onSucceeded`) runs only on demand (`library.lookup {hashes, hashCandidates: true}` and `Libraries.hash`), never in a sweep (spec §7.3).

### Stages

39. **The probe stage is built in** and keyed by file; every other stage is **keyed by fingerprint** and implicitly depends on `probe`. `registerStage(spec)` (frozen) declares `{name, version, appliesTo, dependsOn, jobKind, batch, params, priority?, maxInFlight?, isDone?}`; the stage's kind must be a batch kind whose items are fingerprints, whose result keeps 5C-1's default `combine` shape (`{items, itemErrors}`), and whose worker opens originals only through `open_for_read` with the `StageFile` it is given.
40. **The pump** (per library, per stage): pending = files `present`, probed, `appliesTo`, dependencies `done`, no `stages` row at this version or one `failed` with attempts left; it keeps ≤ `maxInFlight` (default 2) jobs queued or running, owner `system`, priority `background` by default, `library` set; it stops while the library is offline or paused and never selects `cloudOnly` files unless `hydrate`. **A server-wide in-flight set keyed by `(fingerprint, stage)`** (and `(library, fileId)` for probes, rebuilt from `stages.jobId` at start) keeps the same fingerprint out of two jobs, across libraries; no batch-hash dedupe. `isDone(fp)` (a cache hit in 6's or 8's index) marks a row `done` without a job. Completion comes from `JobEngine.onDone`: items → `done`; item errors → `failed` with the code (`POISON_ITEM` permanently for this version); a whole failed job → `failed` for its items, retried after 1 h, 3 attempts. **Failures with `LIBRARY_OFFLINE` or `RESOURCE_STUCK` (and jobs cancelled because the library went offline or was paused) consume no attempt:** their rows return to pending. At start, rows `queued`/`running` are reconciled with `engine.get(jobId)`.
41. **`prioritize(fp, stage)`** (frozen; sub-project 6 calls it when an asset is first used in a project): the row jumps the pump's queue and is submitted at once alone at `normal` priority, unless already in flight; dependencies run first the same way.
42. **Versions and outputs:** a stage's `version` string changes (6: encoder settings; 8: analyzer id, version and model hashes) → its rows at other versions become `stale` and are re-queued at `background` priority; other stages are untouched (spec §7.3). Outputs are the stage owner's, keyed by fingerprint under `<server-data>/cache/` (5D never reads or deletes them). **`library.remove`** cancels the library's jobs, closes and deletes its catalog files (`catalog.db*` and 5D's other files in `libraries/<id>/`) and **keeps `<server-data>/libraries/<id>/vectors/`** (8E's index), with a TASKS note that 8E owns its cleanup; Task 16 writes this into spec §7.

### Search, lookup and locate

43. **`library.search`**: the query is tokenised like `unicode61` and each term becomes a prefix term (`"interv"*`), over `name` and `dir`; filters on `kind`, `durationMs`, `width`, `height`, `lastChange ≥ since`, `state`; scored by `bm25(names, 4.0, 1.0)`. **Pagination uses a snapshot:** the first page ranks up to 2,000 hits across the requested libraries (merged by score, ties by library id and row id) and keeps them in the reader thread under an opaque cursor (5-minute TTL, ≤ 64 live snapshots, LRU); later pages slice it, so pages never repeat or skip while catalogs change; past 2,000 the answer says `truncated: true`. *Why:* bm25 depends on corpus statistics that change with every write, so a keyset on scores is not stable. A query with no terms lists by `lastChange` descending with a score-free `(library, rowid)` keyset. Sub-project 8E replaces the scorer behind the same RPC.
44. **`library.lookup {fingerprints ≤ 1000 | hashes ≤ 1000, hashCandidates?}`**: exact matches across this server's libraries; with `hashCandidates`, rows of equal size without a full hash are hashed by a `library.hash` job (`{matches, pending: {job}}`; needs `library.scan` and `jobs.submit`).
45. **`locate({path?, fingerprint?, fullHash?})`** (in-process, frozen for sub-project 6's manifest): a path under a root, or under one of the library's **aliases** (other machines' paths of the same root, arch §20: `/Volumes/<share>/Media` ↔ `\\<server>\media` ↔ `/mnt/media`, stored per server), is mapped to the root and looked up by `relKey` (several rows → `PATH_AMBIGUOUS`); then by fingerprint; then by full hash. It answers a `Located` (raw path, display, `expect`, state, match, offline) or nothing; `cloudOnly` is answered, and 6 refuses it at export with `FILE_CLOUD_ONLY`. Aliases serve `locate` for system-internal resolution only, never reach (Decision 47).
46. **Answers carry freshness:** `stale`, `offline` and `lastPollAt` on every search and listing.

### RPCs, scopes, CLI

47. **Scopes and reach** (spec §4.5, §7.6):
    - `library.add`, `library.remove`, and `library.set` of `hydrate`, `bandwidthMiBs`, `acceptVolume` or **`aliases`** need `library.admin` (human-only, an escalation from `library.set`); other `library.set` fields (`name`, `include`, `exclude`, `kinds`, `scan`, `paused`, `stages`) need `library.scan`; listings, search, lookup and `library.watch` need `library.read`; `library.scan` needs `library.scan` and `jobs.submit`;
    - **library reach** in v1: every principal holding `library.read` reaches every library of this server (credentials carry `projects`, not libraries); `libraryReach(principal, id)` is the one function 8's host calls use;
    - **asset reach to a library file** (`asset.probe`, `asset.add`) is granted only when the path's key **finds exactly one catalogued row** (present, in a library the principal reaches; never through an alias), and the read then goes through `open_for_read` with that row's raw path and `expect`; there is **no path-prefix check**. This replaces 5A Decision 19's "(5D) a library root for principals with roots": a root grants nothing by itself, a catalogued file does. `cloudOnly` rows answer `FILE_CLOUD_ONLY`, several rows `PATH_AMBIGUOUS`, none `PATH_NOT_ALLOWED` (as 5A).
48. **Library names** are unique per server, case-insensitively (`LIBRARY_NAME_TAKEN`); commands take an id or a name. `library.add` starts the first full scan and answers `{library, job}`.
49. **Audit** (spec §3.3): `library.add`, `library.remove`, `library.set` (human-only fields only), with `{libraryId, storage, fields}`; never names, roots, aliases or paths.
50. **`library.scan` journal events** (spec §3.4) for full and incremental scans, watch rescans and polls that found changes; no-change polls are counted in `library.status` only, so a 60 s poll does not write 1,440 events a day. Payload as the spec, `files`/`dirs` as log₂ buckets, `trigger: add|schedule|demand|watch|mount`.
51. **Codes:** 3 `LIBRARY_ROOT_INVALID`, `LIBRARY_OVERLAP`, `LIBRARY_NOT_FOUND`, `LIBRARY_NAME_TAKEN`, `FILE_NOT_FOUND`, `INVALID_PATH`, **`PATH_AMBIGUOUS`**; 4 `FILE_CLOUD_ONLY`; 5 `LIBRARY_OFFLINE`, `LIBRARY_ROOT_UNREADABLE`, `CATALOG_TOO_NEW`; 7 `HUMAN_ONLY`, `PATH_NOT_ALLOWED` (5A); warnings in `library.status`: `ROOT_EMPTY_SUSPECT`, `ROOT_UNMOUNTED`, `ROOT_VOLUME_CHANGED`, `DIR_EMPTY_SUSPECT`, `NAME_COLLISION`, `WATCH_DEGRADED`, `RESOURCE_STUCK`, `ENUMERATION_FALLBACK`.

## Frozen interfaces

Frozen once Tasks 1, 2, 3, 6, 7a, 10 and 11 merge, for sub-projects 6, 8 and 9; a change needs a coordinator decision and a note in each consumer's plan. **The rule of Decision 20 (every reader of a library file uses `open_for_read`) is frozen with them.**

```rust
// crates/fin-library (Task 1 types; Tasks 2–3 implement the opener)
pub struct RawRel(pub Vec<Vec<u8>>);          // raw components below the root; serde: base64 per component
pub struct Expect { pub size: u64, pub mtime_ns: i64, pub file_id: Option<Vec<u8>>, pub dev: Option<String> }
pub enum Placeholder { Dataless, IcloudStub, Nextcloud, Owncloud, RecallOnOpen, RecallOnData, Offline, Unpinned }
pub enum EntryType { File, Dir, Symlink, Other }
pub struct ScanEntry { pub dir: RawRel, pub name: Vec<u8>, pub name_nfc: String, pub name_lossy: bool, pub rel_key: String,
    pub entry_type: EntryType, pub size: u64, pub mtime_ns: i64, pub file_id: Option<Vec<u8>>, pub dev: Option<String>,
    pub mount_root: bool, pub automount: bool, pub reparse_tag: Option<u32>, pub placeholder: Option<Placeholder>,
    pub excluded: Option<String> }
pub struct DirDone { pub dir: RawRel, pub mtime_ns: Option<i64>, pub complete: bool, pub code: Option<String>, pub entries: u32,
    pub volume: Option<VolumeIdentity> }
pub struct ScanBatch { pub seq: u64, pub entries: Vec<ScanEntry>, pub dirs_done: Vec<DirDone> }
pub struct VolumeIdentity { pub key: String, pub kind: String, pub dev_id: String, pub mount_root: bool, pub case_sensitive: bool,
    pub enumeration: String, pub remote: bool, pub root_file_id: Option<Vec<u8>>, pub sample: Vec<SampleEntry> }
pub enum RootState { Online(VolumeIdentity), Unreachable { code: String }, Unmounted, VolumeChanged { key: String }, Empty, Timeout }
pub fn identity(root: &Path) -> io::Result<VolumeIdentity>;
pub fn root_state(root: &Path, stored: &VolumeIdentity, timeout: Duration) -> RootState;
pub fn open_for_read(root: &Path, rel: &RawRel, expect: &Expect, hydrate: bool) -> Result<File, OpenError>;
    // OpenError: CloudOnly | Changed | NotFound | Escapes | NameSurrogate | Io(io::Error)
pub mod process { pub fn deny_dataless_materialisation() -> io::Result<()>; pub fn allow_dataless_materialisation() -> io::Result<()>; }
pub struct ProbeOutcome { pub file: i64, pub fingerprint: Option<String>, pub probe: Option<fin_probe::Probe>,
    pub probe_version: u32, pub state: String, pub code: Option<String> }
// crates/fin-probe (Task 5)
pub const PROBE_VERSION: u32;
pub fn probe_with(path: &Path, open: impl FnOnce(&Path) -> io::Result<File>) -> Result<Probe, CoreError>;
pub fn probe_reader(r: impl Read + Seek, meta: FileMeta, path: &Path) -> ProbeOutcome;
```

```ts
// apps/finisherd/src/library/api.ts (Tasks 10–12): the in-process API for sub-projects 6 and 8
export type LibraryStorage = "local" | "network" | "sync";
export interface LibraryPools { readonly scan: PoolName; readonly poll: PoolName; readonly read: PoolName }
export interface RawRelWire { readonly components: ReadonlyArray<string> }       // base64 of each raw component (generated RawRel)
export interface ExpectWire { readonly size: number; readonly mtimeNs: string; readonly fileId?: string }
export interface StageFile { readonly library: string; readonly fileId: number; readonly root: string; readonly rel: RawRelWire;
  readonly display: string; readonly expect: ExpectWire; readonly hydrate: boolean; readonly fp: string; readonly size: number;
  readonly kind?: string; readonly probe?: unknown /* fin_probe::Probe */; readonly storage: LibraryStorage }
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
export interface Located { readonly library: string; readonly fileId: number; readonly root: string; readonly rel: RawRelWire;
  readonly display: string; readonly expect: ExpectWire; readonly hydrate: boolean; readonly state: FileState;
  readonly match: "path" | "alias" | "fingerprint" | "hash"; readonly offline: boolean }
export type FileEvent = { readonly type: "added" | "changed" | "missing" | "renamed" | "moved" | "probed" | "stage";
  readonly library: string; readonly fileId: number; readonly fp?: string; readonly stage?: string };
export interface Libraries {
  list(): ReadonlyArray<LibraryView>;
  get(id: string): Effect.Effect<LibraryView, FinError>;                         // LIBRARY_NOT_FOUND
  locate(ref: { readonly path?: string; readonly fingerprint?: string; readonly fullHash?: string }): Effect.Effect<Located | undefined, FinError>; // PATH_AMBIGUOUS
  pools(id: string): LibraryPools;
  online(id: string): Effect.Effect<boolean>;
  use(ids: ReadonlyArray<string>): { release(): void };                         // in use (Decision 28)
  registerStage(spec: StageSpec): void;
  prioritize(fp: string, stage: string): Effect.Effect<void, FinError>;         // Decision 41
  stageStates(fp: string): Effect.Effect<ReadonlyArray<{ library: string; stage: string; version: string; state: string; code?: string }>>;
  hash(library: string, fileIds: ReadonlyArray<number>): Effect.Effect<JobView, FinError>;
  onFile(listener: (e: FileEvent) => void): () => void;
  reach(principal: Principal, id: string): boolean;                             // Decision 47
}
export class LibrariesTag extends Context.Tag("Libraries")<LibrariesTag, Libraries>() {}

// apps/finisherd/src/library/catalog/extensions.ts (Task 7a): for 8C's transcripts and 8E's chunks
export function registerCatalogExtension(ext: { readonly name: string; readonly module: URL }): void;
export interface CatalogExtensionModule {                                       // the module's exports, loaded in both threads
  readonly migrations: ReadonlyArray<Migration>; readonly versions?: ChainVersions;
  readonly ops: Readonly<Record<string, (db: DatabaseSync, args: unknown) => unknown>>;       // indexer, in a transaction
  readonly queries: Readonly<Record<string, (db: DatabaseSync, args: unknown) => unknown>>;   // reader, read-only
}
export interface IndexerClient { exec(lib: string, ext: string, op: string, args: unknown): Effect.Effect<unknown, FinError>; /* + 5D's own ops */ }
export interface ReaderClient { query(lib: string, ext: string, op: string, args: unknown): Effect.Effect<unknown, FinError>; /* + 5D's own queries */ }

// @finisher/contracts library.ts (Task 13): Effect Schemas; TS types are Schema.Type of them
LibraryView = { id; name; root; aliases: string[]; storage: LibraryStorage; provider?: string;
  state: "online" | "offline" | "scanning" | "paused"; offlineReason?: string; include?: string[]; exclude: string[];
  kinds: string[] | "all"; scan: { poll: "demand" | number; full: "daily" | "weekly" | "manual" }; hydrate: boolean;
  bandwidthMiBs?: number; stages: string[]; counts: Record<FileState, number>; lastScan?: { at; mode; outcome; code? };
  lastPollAt?: number; warnings: { code: string; dir?: string; count?: number }[]; createdAt: number }
FileState = "present" | "missing" | "cloudOnly" | "unreadable" | "probeFailed" | "skipped" | "offline"
SearchHit = { library; fileId; relKey; display; kind?; size; durationMs?; width?; height?; fp?; state: FileState;
  collision: boolean; score: number; offline: boolean }
```

**RPCs** (Task 13; scopes per Decision 47):

| RPC | Payload → result |
|---|---|
| `library.add` | `{root, name?, storage?, include?, exclude?, kinds?, scan?, hydrate?, bandwidthMiBs?, aliases?, challenge?}` → `{library, job}` |
| `library.remove` | `{id, challenge?}` → `{}` (cancels its jobs, deletes its catalog files, keeps `vectors/`, Decision 42) |
| `library.set` | `{id, name?, include?, exclude?, kinds?, scan?, paused?, stages?, aliases?, hydrate?, bandwidthMiBs?, acceptVolume?, challenge?}` → `{library}` |
| `library.list`, `library.status` | `{}` → `{libraries}`; `{id}` → `{library, jobs, stages: [{stage, version, done, queued, failed, stale}], watch?}` |
| `library.files` | `{id, dir?: path \| dirId, state?, kind?, cursor?, limit ≤ 500}` → `{dir, dirs, files, next?, stale, offline}` (keyset on `(nameNfc, id)`) |
| `library.file` | `{id, path \| fileId}` → `{file, stages}` (`INVALID_PATH` for `..`, absolute or escaping paths; `PATH_AMBIGUOUS`) |
| `library.search` | `{query?, libraries?, kind?, duration?: {min?, max?}, width?, height?, since?, state?, cursor?, limit ≤ 200}` → `{hits, next?, truncated, stale}` |
| `library.lookup` | `{fingerprints? \| hashes?, hashCandidates?}` → `{matches: [{key, library, fileId, relKey, display, state}], pending?: {job}}` |
| `library.scan` | `{id, full?, dirs?}` → `{job}` |
| `library.watch` (stream) | `{ids?}` → a snapshot of `LibraryView`s, then `{type: library \| scan \| progress, …}` |

**`fin`:** `fin library ls|status <lib>|scan <lib> [--full] [--dir <path>]…|files <lib> [<dir>] [--state] [--kind] [--limit] [--cursor]|show <lib> (<path> | --id <fileId>)|search [<query>] [--lib <lib>]… [--kind] [--min-duration] [--max-duration] [--min-width] [--min-height] [--since] [--limit]|set <lib> [--name] [--include]… [--exclude]… [--kinds] [--poll demand|<s>] [--full daily|weekly|manual] [--pause|--resume]`; `fin search <query> …` = `fin library search`. **Console** (TTY and typed challenge, 5A Decision 30): `finisherd library add <dir> [--name] [--storage local|network|sync] [--include]… [--exclude]… [--poll] [--full] [--hydrate] [--bandwidth <MiB/s>] [--alias <path>]…`, `finisherd library rm <lib>`, `finisherd library set <lib> [--hydrate on|off] [--bandwidth <MiB/s>|off] [--alias <path>]… [--no-aliases] [--accept-volume]`.

## Questions for the owner

None. The plan follows spec revision 4, its owner decisions and the coordinator's calls on the review; Task 16 writes these engineering choices back into the spec:
- read-only roots win over the macOS listing refresh of spec §7.2 (Decision 31); Linux FUSE sync roots are metadata-only unless `hydrate` (Decision 14, against §7.3);
- `ROOT_VOLUME_CHANGED` confirmed by file ids, with a human-only `acceptVolume` (Decision 17); nested mounts and automounts; `ENUMERATION_FALLBACK`; `NAME_COLLISION`, `PATH_AMBIGUOUS`, `DIR_EMPTY_SUSPECT`;
- pools per volume with a separate poll slot (Decision 32); bandwidth in params (Decision 36);
- aliases human-only and never reach; asset reach by catalogued row (Decision 47, replacing 5A Decision 19's root clause);
- `library.remove` keeps `vectors/` (Decision 42); no-change polls not journaled (Decision 50).

## Review Focus

1. **No placeholder is ever opened, read or descended into, on any OS, and nothing is written under a root.** → Task 1 `classify_matrix`, `stub_and_real_file_real_wins`; Task 2 Step 0 (a real dataless file and folder) and `dataless_dir_not_descended`; Task 3 `cfapi_placeholders_never_fetched`, `recall_on_open_never_opened`; Task 4 `fake_placeholders_open_counter_is_zero`; Task 15 `root_is_byte_identical_after_scan`.
2. **Files are opened by raw names; key collisions never lose or fail anything.** → Task 1 `raw_rel_round_trips_non_utf8_and_wtf8`, `collision_bits`; Task 7b `twins_keep_both_rows`, `ambiguous_lookup_is_path_ambiguous`; Task 2 `opens_non_utf8_name_by_raw_chain`.
3. **Every reader goes through `open_for_read`.** → Task 6 the clippy gate and `check-clippy-config`; Task 13 `asset_probe_of_library_file_uses_catalogued_row`; Task 11 `stage_file_carries_raw_path_and_expect`.
4. **An unavailable root never marks a file missing;** unmounted, volume-changed (confirmed), empty-suspect and timed-out roots go offline with the catalog intact; partial and suddenly empty listings mark nothing; automounts are not triggered. → Task 1 `root_state_classify`, `volume_change_needs_confirmation`; Task 2 Step 0 and `bind_mount_unmounted_is_offline`, `automount_not_triggered`; Task 4 `entry_timeout_marks_dir_unreadable_not_missing`; Task 7b `incomplete_dir_marks_nothing_missing`, `dir_drop_to_zero_needs_confirmation`.
5. **The diff is right:** renames by `dev:fileId` keep history, hard links stay two rows, moves by fingerprint merge, resumed scans skip done listings, concurrent poll and scan listings agree. → Task 7b property `diff_matches_reference_model` and the 10k-tree test.
6. **NAS disks are left alone when nobody uses them;** in use, changes appear within a minute, even during a full scan. → Task 10 `demand_cadence_with_injected_clock`, `freshness_pass_bounded_5s_then_stale`; Task 9 `poll_runs_beside_a_full_scan`.
7. **Safe opens:** no link followed, no escape, identity checked, data-bearing reparse tags readable, fallbacks without `openat2`. → Tasks 2, 3 opener tests.
8. **Stages:** versions re-queue only their stage; offline costs no attempts; one fingerprint is never in two jobs; `prioritize` jumps the queue. → Task 11.
9. **The event loop:** no catalog statement or batch parse on the main thread; lag p99 ≤ 20 ms during a 100k-file scan. → Tasks 7a, 15.
10. **Scopes, human-only and privacy:** add/remove/aliases/hydrate refuse every non-human principal; no name or path in the journal, audit or info logs. → Task 13.
11. **Files ≤ 600 lines; no GPL dependency; every commit green on three OSes.** → gates at every commit; Task 17.

---

### Task 1: `fin-library` types, raw paths, names, placeholders, excludes, volume rules and the fake source — tier: opus — can start now

**Files:** Create `crates/fin-library/{Cargo.toml (features test-hooks), src/{lib.rs, types.rs, raw.rs, names.rs, collide.rs, placeholder.rs, reparse.rs, exclude.rs, glob.rs, volume.rs, source/{mod.rs, fake.rs}}}`; modify `Cargo.toml` (member), `crates/fin-store/examples/export_schema.rs` (+ dev-dependency), `packages/core-types` (regenerated), `TASKS.md`. Test: `crates/fin-library/tests/{raw.rs, names.rs, placeholder.rs, exclude.rs, volume.rs, fake.rs}`.

**Interfaces:** the types of the `fin-library` block; `Source`, `RawEntry`, `RootStat` (Decision 4); `classify(entry, siblings) -> Option<Placeholder>`; `reparse::is_name_surrogate(tag)`; `collisions(listing) -> Vec<(index, bits)>`; `Excludes`; `classify_root(stored, now: RootStat) -> RootState`; `FakeSource::from_json(spec)` (tree with raw names, attributes, tags, placeholders, per-path delays and errors, mount and automount flags, an open counter). Decisions 4, 8, 10–13, 15–17.

- [x] **Step 1: Write failing tests:** `raw_rel_round_trips_non_utf8_and_wtf8`, `nfc_keys_and_lossy_names`, `case_folded_keys_only_on_insensitive_roots`, `collision_bits` (NFC/NFD twins, case twins, two lossy names, `ß`/`ẞ` and Turkish dotted I against `to_lowercase`); `classify_matrix` (every attribute bit per OS, `PINNED` hydrated, `UNPINNED` online-only, recall bits on `CLOUD*`, `DEDUP` and `WOF` tags), `name_surrogates_only_are_skipped`, `stub_and_real_file_real_wins`, `nextcloud_owncloud_icloud_stub_names`; `default_excludes`, `conflict_copies_excluded`, `automounts_excluded`, `project_folders_keep_only_sources`, `glob_matrix`; `root_state_classify` (unmounted by mount-root loss and by `st_dev` = parent), `volume_change_needs_confirmation` (key change with matching root id and sample → online; key change with a different root id or < half the sample → `volumeChanged`), `fake_source_reports_and_counts`.
- [x] **Step 2: Implement** (pure; no OS call outside `source/`). **Step 3: Run** the gates → PASS. **Commit** `feat(fin-library): raw paths, names and collisions, placeholders, excludes, volume rules and a fake source`.

### Task 2: Linux and macOS sources, volume identity and the opener — tier: opus — after Task 1, beside Task 3

**Files:** Create `crates/fin-library/src/source/{unix.rs, linux.rs, macos.rs}`, `crates/fin-library/src/open/{mod.rs, linux.rs, macos.rs}`, `crates/fin-library/src/volume/{linux.rs, macos.rs}`, `crates/fin-library/src/process.rs`. Test: `crates/fin-library/tests/{source_unix.rs, open_unix.rs, mounts_unix.rs}`.

Decisions 5, 6, 10, 15, 17, 19, 22.

- [x] **Step 0: measure before building** (findings at the top of `volume/mod.rs`, then in Task 16):
  - confirm in `libc` 0.2.191 `statx` (with `AT_NO_AUTOMOUNT`, `STATX_ATTR_MOUNT_ROOT`, `STATX_ATTR_AUTOMOUNT`, `STATX_MNT_ID`), `openat2`/`open_how`, `getattrlistbulk`/`attrlist`, `ATTR_VOL_UUID`, `O_NOFOLLOW_ANY`, `setiopolicy_np`; declare what is missing locally;
  - **identity across a remount** on each OS: Linux a loop-mounted image and a bind mount (in `unshare -Urm`) and, with `FIN_TEST_SHARE`, an NFS and a CIFS share unmounted and remounted; macOS a disk image detached and re-attached and an SMB share reconnected. Record which of `ATTR_VOL_UUID`, `f_mntfromname`, mountinfo source, `f_fsid`, `st_dev`, the root's file id and the sample stay equal;
  - **a real dataless file and folder** (macOS, on a host with iCloud Drive: `brctl evict` a test file and a test folder): `getattrlistbulk` reports `SF_DATALESS` for both, enumerating the parent does not materialise them (`ls -lO` still shows `dataless`), and with the policy off an open fails `EDEADLK`. Without an iCloud account the controller runs it before this task merges.
- [x] **Step 1: Write failing tests:** `enumerates_without_opening_files` (a tree of mode-0000 files still enumerates with sizes and mtimes), `symlinks_skipped_never_followed`, `automount_not_triggered` (a `STATX_ATTR_AUTOMOUNT` entry from the fake attrs is not descended; on a Linux host with autofs or a systemd automount unit available, the real one stays unmounted), `file_ids_and_mount_roots`, `bind_mount_unmounted_is_offline` (Linux, under `unshare -Urm`), `disk_image_detached_is_offline` (macOS, `hdiutil attach -mountpoint <tmp>` then `detach`), `case_sensitivity_detected_without_writing`, `identity_stable_across_remount` (the Step 0 cases this host supports); `policy_off_in_process` (`deny_dataless_materialisation` reads back off), `dataless_dir_not_descended`; opener: `opens_non_utf8_name_by_raw_chain`, `open_refuses_symlink_component`, `open_stays_beneath_root`, `openat2_enosys_and_eperm_fall_back_per_component` (forced through a test hook), `noatime_dropped_after_eperm`, `identity_mismatch_is_changed`, `placeholder_refused_before_open`.
- [x] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(fin-library): Linux and macOS enumeration, volume identity and the safe opener`.

### Task 3: Windows source, volume identity and the opener — tier: opus — after Task 1, beside Task 2

**Files:** Create `crates/fin-library/src/source/windows.rs`, `crates/fin-library/src/open/windows.rs`, `crates/fin-library/src/volume/windows.rs`; modify `crates/fin-library/Cargo.toml` (windows-sys features; `Win32_Storage_CloudFilters` dev-only). Test: `crates/fin-library/tests/{source_windows.rs, cfapi_windows.rs, open_windows.rs}`.

Decisions 7, 8, 10, 12, 15, 17, 19, 22.

- [x] **Step 0: measure before building** (findings in `volume/windows.rs`, then Task 16):
  - **a real Cloud Files sync root** (no admin): `CfRegisterSyncRoot` on a temp directory, `CfConnectSyncRoot` with callbacks counting `FETCH_DATA` and `FETCH_PLACEHOLDERS`, `CfCreatePlaceholders` for files (recall on data access) and a directory (recall on open). Record what each fallback class reports (tag in `ReparsePointTag` or `EaSize`, attributes), and that `FindFirstFileExW` triggers no callback;
  - **data-bearing tags:** a file compressed with `compact /c /exe:xpress4k` (WOF) and, where the host has Data Deduplication, a deduplicated file, each read correctly through Decision 19's open;
  - **identity across a remount:** a `subst` drive removed and re-added, a VHD attached and re-attached if the host allows it non-elevated, and with `FIN_TEST_SHARE` a mapped drive and a UNC path reconnected. Record which of the volume serial, `\\server\share`, the root's file id and the sample stay equal.
- [x] **Step 1: Write failing tests:** `cfapi_placeholders_never_fetched` (a full scan and a probe pass over the sync root: zero callbacks; entries `cloudOnly`; the placeholder directory not descended), `recall_on_open_never_opened`, `hydrated_cloud_file_is_readable`, `wof_file_is_readable`, `extd_dir_info_fields` (128-bit ids, 100 ns times → ns, sizes), `tag_read_from_easize_in_fallbacks`, `enumeration_fallback_chain` (forced through a test hook), `junctions_and_symlinks_skipped`, `long_paths_with_verbatim_prefix`, `opens_with_all_share_flags` (a file held open for write and delete by another handle is still read), `open_refuses_junction_component`, `final_path_must_be_under_root`, `volume_serial_unc_and_case_flag`, `subst_drive_removed_is_unreachable`, `identity_stable_across_remount`.
- [x] **Step 2: Implement.** **Step 3: Run** the gates on Windows → PASS. **Commit** `feat(fin-library): Windows enumeration from directory handles, volume identity and the safe opener`.

### Task 4: The walker: parallel reads, timeouts, batches, polls and the test overlay — tier: opus — after Task 1 (develops on `FakeSource`; real sources after Tasks 2–3)

**Files:** Create `crates/fin-library/src/{walk/{mod.rs, pool.rs, batch.rs, poll.rs}, timing.rs, overlay.rs (test-hooks)}`. Test: `crates/fin-library/tests/{walk.rs, walk_timeouts.rs, overlay.rs}`.

**Interfaces:**
```rust
pub enum WalkMode { Full { gen: u64, resume: Option<KnownDirs> }, Dirs { dirs: Vec<RawRel> }, Poll { known: KnownDirs } }
pub struct WalkOpts { pub threads: usize, pub excludes: Excludes, pub timing: Timing, pub batch_entries: usize }
pub struct Timing { pub first_stat: Duration /* 60 s */, pub entry: Duration /* 30 s */ }
pub fn walk(src: &dyn Source, mode: WalkMode, opts: &WalkOpts, sink: &mut dyn FnMut(ScanBatch) -> Result<(), WalkStop>,
    cancel: &CancelToken) -> Result<WalkSummary, WalkError>;   // WalkError::Offline(RootState)
```
The overlay (test-hooks only): a `.fin-fake.json` at a root marks real paths as placeholders, delays first stats and entries, injects errors, unmounts and automounts, and appends every `open_for_read` of a listed path to a counter file outside the root; release builds never read it (`scripts/check-test-hooks.mjs` covers the crate). Decisions 9, 13, 17, 18, 25, 27.

- [x] **Step 1: Write failing tests:** `walk_order_is_deterministic`, `parallel_reads_bounded_by_threads`, `batches_capped_at_4096_entries`, `listing_entries_precede_their_dir_done`; `first_stat_45s_of_60_is_online` and `first_stat_past_60s_is_offline` (scaled timing), `entry_timeout_marks_dir_unreadable_not_missing`, `eio_and_eacces_dirs_incomplete`; `poll_relists_only_changed_mtimes`, `dirs_mode_targets_only_given_dirs`, `resume_skips_listed_dirs_but_descends`; `nested_mount_unmounted_is_offline_subtree`, `root_empty_with_catalog_is_suspect`; `cancel_stops_within_one_batch`; `fake_placeholders_open_counter_is_zero` (overlay, through every walk mode).
- [x] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(fin-library): the parallel walker with spin-up-tolerant timeouts, polls and resumable scans`.

### Task 5: `fin-probe` entry points, `PROBE_VERSION`, and library probe and hash — tier: sonnet — **after 6A-1 merges** and Task 2 or 3

**Files:** Modify `crates/fin-probe/src/lib.rs` and its readers (`probe_reader`, `probe_with`, `FileMeta`, **`PROBE_VERSION` created**; `probe(path)` stays); create `crates/fin-library/src/{probe.rs, hash.rs, budget.rs}`. Test: `crates/fin-probe/tests/reader.rs`, `crates/fin-library/tests/probe.rs`.

Decisions 19, 20, 34–38. Hard dependency on 6A-1's merge (it rewrites the MP4 path to `fin-container`); starting earlier means rewriting the same entry points twice.

- [x] **Step 1: Write failing tests:** `probe_reader_equals_probe` (every fixture kind: same `Probe`, same `fp1`), `probe_with_uses_the_given_opener`, `fp_survives_probe_failure`, `probe_entry_uses_one_handle`, `budget_limits_probe_reads` (injected clock), `hash_is_whole_file_blake3`, `cloud_only_refused_unless_hydrate`, `changed_identity_is_changed_not_probed`, `probe_version_is_exported`.
- [x] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(fin-probe,fin-library): probe_with, probe_reader and PROBE_VERSION; budgeted probe and hash of library files`.

### Task 6: The `library` worker family and the clippy gate — tier: sonnet — after Tasks 4–5 and 5C-1 Tasks 1–2

**Files:** Create `crates/fin-workers/src/library/{mod.rs, scan.rs, probe.rs, hash.rs, params.rs}`, `clippy.toml` in `crates/{fin-library, fin-workers, fin-node}` (and in `fin-media` when this branch has it), `scripts/{check-clippy-config.mjs, clippy-allow.json}` (+ `.test.mjs`); modify `crates/fin-workers/src/main.rs` (dispatch), `crates/fin-core/src/events/names_jobs.rs` (`library.scan`, `library.probe`, `library.hash`), the schema export, `scripts/gates.mjs` (clippy with `-D clippy::disallowed_methods`), existing `allow` sites in `fin-node` (listed with reasons). Test: `crates/fin-workers/tests/library_*.rs` (over 5C-1's runtime harness and the overlay).

**Interfaces:** params and results (exported): `LibraryScanParams {library, root, mode: full|dirs|poll, gen, dirs?, excludes, threads, timing? (test-hooks)}`, `LibraryProbeParams {library, root, hydrate, readBps?, files: in/files.ndjson}` (items = file ids; each line `{id, rel, expect}`), `LibraryHashParams {library, root, readBps?}`; host calls `library.batch {file, seq}`, `library.probed {results}`; `error LIBRARY_OFFLINE {state}` with `retry: never`. The family calls `deny_dataless_materialisation()` before `hello` (after the self-sandbox entry). Decisions 6, 21, 26, 33–38.

- [x] **Step 1: Write failing tests:** `scan_streams_batches_through_calls_and_waits_for_replies`, `scan_reports_progress_per_directory`, `offline_root_is_library_offline_never_retried`, `probe_acks_items_after_probed_reply`, `probe_crash_on_one_file_is_isolatable`, `hash_items_carry_hashes`, `family_denies_dataless_before_hello` (macOS), `family_starts_no_process`; gate: `clippy_flags_each_disallowed_method` (a fixture crate), `unlisted_allow_fails`, `listed_crate_without_config_fails`.
- [x] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(fin-workers): the library family; clippy disallowed-methods over library readers`.

### Task 7a: Catalogs: extensions, schema and the indexer and reader threads — tier: opus — after Task 1 (generated types); can start now

**Files:** Create `apps/finisherd/src/library/catalog/{extensions.ts, core/{index.ts, migrations/0001.ts, ops.ts, queries.ts}, open.ts, indexer-thread.ts, reader-thread.ts, client.ts, batch-file.ts, fts.ts}`. Test: `apps/finisherd/test/library/{catalog.test.ts, extensions.test.ts, threads.test.ts}`, a test extension module under `apps/finisherd/test/library/fixtures/`.

**Interfaces:** `registerCatalogExtension`, `CatalogExtensionModule`, `IndexerClient` (`exec`, and 5D's `open`, `close`, `applyBatchFile(lib, path, gen)`, `finishScan`, `applyProbed`, `applyHashes`, `stagesSet`, `writeKnownDirs(lib, gen, path)`, `pending(lib, stage, limit)`, `purgeMissing`) and `ReaderClient` (`query`, and `files`, `file`, `search`, `lookup`, `counts`, `stageCounts`), all `Effect`s over `MessagePort`s. Decisions 2, 23, 24, 26.

- [x] **Step 1: Write failing tests:** `core_and_extension_migrations_apply_once_per_catalog`, `extension_module_loaded_in_both_threads`, `exec_runs_in_a_transaction_and_query_is_read_only` (a write from a query fails), `functions_never_cross_ports` (a non-cloneable arg is refused before posting), `catalog_of_another_library_moved_aside`, `corrupt_catalog_rebuilt`, `too_new_extension_is_offline`; `batch_file_read_and_validated_in_indexer` (a path outside `out/`, a symlink, an oversized file and a schema-invalid line each fail with nothing written), `main_thread_parses_no_batch` and `main_thread_runs_no_catalog_statement` (spies in the main thread); `fts_rows_follow_inserts_renames_deletes`; `transactions_capped_at_2000_rows`, `reader_never_waits_for_writer` (a long apply does not delay a search past 20 ms); **`bytes_per_file_with_fts_dir`** on the generated 100k tree, recording the figure and the `dir`/`dirnames` choice of Decision 23.
- [x] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): library catalogs with extensions and indexer and reader threads`.

### Task 7b: The diff and its property test — tier: opus — after Task 7a

**Files:** Create `apps/finisherd/src/library/catalog/core/{diff.ts, apply.ts, collisions.ts}`. Test: `apps/finisherd/test/library/{diff.test.ts, diff.property.test.ts}` (a brute-force reference model in `test/library/support/model.ts`).

Decisions 11, 22, 25, 27.

- [x] **Step 1: Write failing tests:** `diff_matches_reference_model` (fast-check: random trees with raw and colliding names, renames with and without file ids, hard links, moves, rewrites, deletions, placeholder flips, incomplete and suddenly empty directories, resumed generations, a poll interleaved with a full scan, against the model), `incomplete_dir_marks_nothing_missing`, `dir_drop_to_zero_needs_confirmation`, `root_empty_suspect_aborts_before_marks`, `rename_keeps_id_and_fp`, `hard_links_stay_two_rows`, `move_by_fingerprint_merges`, `missing_reappears_present`, `twins_keep_both_rows`, `ambiguous_lookup_is_path_ambiguous`, `collision_never_fails_a_batch`, `missing_purged_after_90_days_in_batches`.
- [x] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): the library catalog diff`.

### Task 8: The `libraries` chain, the registry and root probes — tier: sonnet — can start now (5A Task 1 is merged)

**Files:** Create `apps/finisherd/src/control/migrations/libraries/0001.ts`, `apps/finisherd/src/library/registry.ts`, `crates/fin-node/src/library/{mod.rs, rootprobe.rs}`; modify `crates/fin-node/src/lib.rs`, `packages/fin-node` (typings). Test: `apps/finisherd/test/library/registry.test.ts`, `crates/fin-node/tests/rootprobe.rs`.

**Interfaces:** `libraries (id TEXT PRIMARY KEY, name TEXT NOT NULL, nameKey TEXT NOT NULL UNIQUE, root TEXT NOT NULL, rootKey TEXT NOT NULL UNIQUE, aliases TEXT NOT NULL, volume TEXT NOT NULL, storage TEXT NOT NULL, storageOverride TEXT, state TEXT NOT NULL, offlineReason TEXT, include TEXT, exclude TEXT NOT NULL, kinds TEXT NOT NULL, scan TEXT NOT NULL, hydrate INTEGER NOT NULL, bandwidthMiBs REAL, stages TEXT NOT NULL, gen INTEGER NOT NULL, createdBy TEXT NOT NULL, createdAt INTEGER NOT NULL, lastPollAt INTEGER, lastFullAt INTEGER)`; `Registry {add, remove, set, get, list, byName}` (validation: absolute existing directory representable as a string, not inside server data, no overlap by `rootKey` prefix in either direction, `storage_kind::detect` with override); napi `libraryCanonicalRoot(path) → {root, rootKey, identity}` and `libraryProbeRoot(root, identity, timeoutMs) → Promise<RootState>` (Decision 3). Audit kinds `library.add|remove|set`. Decisions 3, 14, 15, 17, 48, 49.

- [x] **Step 1: Write failing tests:** `add_validates_root` (relative, missing, a file, inside server data, `LIBRARY_OVERLAP` both ways, symlinked roots canonicalised), `names_unique_case_insensitive`, `storage_detected_with_override`, `linux_fuse_sync_is_metadata_only`; `probe_runs_off_the_libuv_pool` (four hung probes on four roots do not delay `fs.readFile`), `one_probe_per_root_outstanding`, `timeout_leaves_thread_and_counts_it`; `audit_rows_carry_no_names_or_paths`.
- [x] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd,fin-node): libraries in control.db and per-root probe threads`.

### Task 9: Library kinds and family wiring on the engine — tier: opus — after Tasks 6–8 and 5C-1 Tasks 7–10

**Files:** Create `apps/finisherd/src/library/kinds.ts` (`library.scan`, `library.probe`, `library.hash`, the `library` `FamilySpec` with `onCall`), `apps/finisherd/src/library/pools.ts`. Test: `apps/finisherd/test/library/kinds.test.ts` (a real engine, test-hooks `fin-workers`, overlay roots).

**Interfaces:** `scan`: `resources: [mode === "full" ? pools.scan : pools.poll]`, `qos: utility`, `stallS: 600`, `timeoutS: 86400`, `resumable` (full), `prepare` asks the indexer to write `in/dirs.ndjson`, `files: [{name: "out/", mode: "write"}]`, `dedupe: <library>:<mode>` for full scans and polls (targeted `dirs` scans are not deduplicated); `probe`: `batch` over file ids, `resources: [pools.read, "cpu"]`, `prepare` writes `in/files.ndjson` (raw paths and `expect`), no dedupe (the in-flight set, Decision 40); `hash`: `batch`, `onSucceeded` writes hashes; all `resourceHealthy: online(library)`; scopes `["library.scan"]`. Decisions 26, 27, 32–34, 38.

- [ ] **Step 1: Write failing tests:** `full_scan_of_10k_tree_lands_in_catalog`, `scan_resumes_after_finisherd_restart_without_relisting`, `poll_job_touches_only_changed_dirs`, `poll_runs_beside_a_full_scan` (separate pools), `probe_poison_file_isolated_and_recorded`, `offline_mid_scan_marks_nothing_missing` (the overlay unmounts the root halfway), `resource_unhealthy_never_poisons`, `pools_shared_per_volume`, `on_call_passes_paths_only` (the main thread never opens a batch file), `bandwidth_splits_over_slots`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): library job kinds on the shared engine`.

### Task 10: The library service: states, triggers, demand polling, watches, journal — tier: opus — after Task 9

**Files:** Create `apps/finisherd/src/library/{service.ts, plan.ts (pure poll planner), usage.ts, watch.ts (indexer-thread side), events.ts, api.ts}`, `crates/fin-core/src/events/payloads/library.rs`; modify `crates/fin-core/src/events/{kinds.rs (one line), payloads/mod.rs}`, `apps/finisherd/src/main.ts` (layer after the engine; close before it), `apps/finisherd/src/projects.ts` (call-site lines: `use()` on open, release on close). Test: `apps/finisherd/test/library/{plan.property.test.ts, service.test.ts, watch.test.ts}`.

**Interfaces:** `plan(libs, now, usage) → Action[]` (pure: `poll | full | recheck | none` per library, with the next wake-up); `LibrariesTag` and the `Libraries` API (frozen; stages from Task 11). States `online → scanning → online`, `offline` (with reason) ↔ recheck, `paused`. Decisions 17, 18, 28–33, 50.

- [ ] **Step 1: Write failing tests:** `demand_cadence_with_injected_clock` (60 s while a project referencing it is open, a watch stream is open or within 10 min of a search; 6 h otherwise), `unused_library_polls_every_6h`, `daily_full_runs_at_first_tick_after_due`, `fixed_interval_override`, `freshness_pass_bounded_5s_then_stale`, `freshness_pass_not_blocked_by_full_scan`, `offline_library_rechecked_every_60s`, `return_online_triggers_mount_scan`, `volume_changed_waits_for_accept`, `local_watch_hint_rescans_after_2s_quiet`, `watch_overflow_degrades_to_5min_polls`, `library_scan_events_payload_closed` (no names, log₂ buckets), `no_change_polls_not_journaled`, `plan_at_most_one_job_per_pool_kind_per_library` (property).
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): the library service with demand-driven polling, watch hints and offline handling`.

### Task 11: The stage framework, `prioritize`, `locate` and file events — tier: opus — after Task 10

**Files:** Create `apps/finisherd/src/library/{stages.ts, pump.ts, inflight.ts, locate.ts}`. Test: `apps/finisherd/test/library/{stages.test.ts, locate.test.ts}` (a `test.stage` batch kind behind test-hooks whose worker opens with `open_for_read`; items = fingerprints).

**Interfaces:** `registerStage`, `prioritize`, `stageStates`, `onFile`, `locate`, `pools`, `hash` (the frozen `Libraries` members). Decisions 20, 39–42, 45.

- [ ] **Step 1: Write failing tests:** `stage_runs_after_probe_for_applicable_files`, `stage_file_carries_raw_path_and_expect`, `depends_on_orders_stages`, `version_bump_requeues_only_that_stage`, `max_in_flight_per_library`, `same_fp_in_two_libraries_never_in_two_jobs`, `offline_and_stuck_failures_cost_no_attempt`, `offline_and_paused_stop_the_pump`, `cloud_only_skipped_unless_hydrate`, `is_done_skips_the_job`, `poison_item_fails_permanently_for_version`, `failed_job_retried_after_1h_three_times`, `prioritize_jumps_the_queue_with_dependencies`, `restart_reconciles_queued_and_running`, `locate_by_path_alias_fp_hash`, `locate_ambiguous_and_cloud_only`, `file_events_for_every_change`, `remove_keeps_vectors_dir`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): the library stage framework and source location`.

### Task 12: Search and lookup — tier: sonnet — query core after Task 7a; freshness after Task 10

**Files:** Create `apps/finisherd/src/library/{search.ts, lookup.ts}` and `catalog/core/{search.ts, snapshot.ts}` (reader-thread side). Test: `apps/finisherd/test/library/search.test.ts`.

Decisions 43, 44, 46.

- [ ] **Step 1: Write failing tests:** `terms_are_prefix_and_diacritic_insensitive`, `separators_split_names` (`interview_take3.final.mov`), `filters_kind_duration_size_since_state`, `snapshot_cursor_stable_while_catalogs_change` (property: pages over a snapshot never repeat or skip while writes and new documents shift bm25), `snapshot_ttl_and_lru`, `truncated_past_2000`, `empty_query_keyset_is_score_free`, `fts_syntax_in_queries_is_escaped`, `lookup_by_fingerprint_and_hash`, `hash_candidates_submit_one_job`, `search_waits_for_freshness_then_stale`, `offline_hits_marked`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): library name search and fingerprint lookup`.

### Task 13: Contracts, RPCs, scopes, reach and `library.watch` — tier: sonnet — after Tasks 10–12 and 5A Tasks 3–6

**Files:** Create `packages/contracts/src/library.ts`, `apps/finisherd/src/library/rpc.ts`, `crates/fin-node/src/library/asset_probe.rs`; modify `packages/contracts/src/{rpc.ts, scopes.ts (rules; escalation of human-only set fields), errors.ts, index.ts}`, `apps/finisherd/src/chain/resolve.ts` (`libraryReach`; asset reach by catalogued row, Decision 47), `apps/finisherd/src/{handlers.ts, projects.ts}` (call-site lines for library-file asset probes), `crates/fin-core/src/events/names.rs` (`RPC_METHODS`), `crates/fin/src/output.rs` (codes). Test: `apps/finisherd/test/library/rpc.test.ts`.

Decisions 11, 20, 46–51.

- [ ] **Step 1: Write failing tests:** `rpc_scopes_cover_library_rpcs`, `human_only_matrix_for_add_remove_aliases_and_admin_fields` (every principal kind; only `console` passes, with a challenge), `fix_texts_contain_no_command`, `set_non_human_fields_with_library_scan`, `scan_needs_jobs_submit_too`, `files_refuses_escaping_paths`, `asset_probe_of_library_file_uses_catalogued_row` (opened with the row's raw path and `expect`; an uncatalogued file under a root is `PATH_NOT_ALLOWED`; a path reached only through an alias is `PATH_NOT_ALLOWED`; twins are `PATH_AMBIGUOUS`), `asset_probe_of_cloud_only_is_file_cloud_only_without_reading`, `watch_starts_with_snapshot_and_marks_in_use`, `journal_and_audit_carry_no_names`, `exit_codes_mapped`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd,contracts): library RPCs with scopes, reach and a watch stream`.

### Task 14: `fin library`, `fin search` and the console — tier: sonnet — after Task 13, 5A Task 8 and 5C-2

**Files:** Create `crates/fin/src/commands/{library.rs, search.rs}`, `apps/finisherd/src/console/commands/library.ts`; modify `crates/fin/src/{registry.rs, map.rs}` (call-site lines), `apps/finisherd/src/console/main.ts`. Test: `crates/fin/tests/cli_library.rs`, `apps/finisherd/test/console.test.ts` (library cases).

- [ ] **Step 1: Write failing tests:** `ls_status_files_show_search_output_and_json`, `show_by_id_resolves_ambiguity`, `scan_prints_job_and_returns`, `fin_wait_on_scan_job` (5C-2), `set_human_fields_from_fin_is_human_only_exit_7`, `libraries_by_id_or_name`, `console_add_needs_tty_and_code`, `console_add_starts_first_scan`, `console_rm_set_hydrate_and_aliases`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(fin,finisherd): fin library and fin search; console library commands`.

### Task 15: e2e `library` scenario and budgets — tier: sonnet — after Task 14

**Files:** Create `scripts/e2e/library.mjs`, `apps/finisherd/scripts/library-budget.ts` (with a generator for 100k files in 5k directories, names in several scripts and normalisations, NFC/NFD and case twins); modify `scripts/e2e/lib.mjs` (console helper for `library add`).

- [ ] **Step 1: e2e:** add a library from the console helper; `fin wait` its scan; `fin library files/search/show`; rename, move, rewrite and delete files, then `fin library scan` and check states; twins are both listed and `show <path>` answers `PATH_AMBIGUOUS`; overlay placeholders and a Nextcloud stub stay `cloudOnly` with an open counter of zero; unmount through the overlay → offline, nothing missing; `root_is_byte_identical_after_scan` (a hash of the tree and its metadata before and after).
- [ ] **Step 2: Budgets** (release builds; Task 17 records them on each OS). **Warm** means the tree's metadata is in the OS cache (a second scan); **cold** means after the controller drops caches (Linux `echo 3 > /proc/sys/vm/drop_caches`, macOS `purge`, Windows a reboot or a VHD detach and re-attach) and is recorded separately, without a budget:
  - 100k-file local full scan end to end **≤ 20 s warm** on the Linux VM; cold recorded on every OS;
  - unchanged poll of 5k directories **≤ 1.5 s** warm, local;
  - indexer apply **≥ 25k entries/s**;
  - search p95 **≤ 50 ms** on one 100k library and **≤ 200 ms** over five, first page and later pages;
  - event-loop lag p99 **≤ 20 ms**, zero main-thread catalog statements and batch parses during the scan;
  - catalog **≤ 1 KiB per file** with FTS (Task 7a's figure re-checked);
  - worker RSS **≤ 256 MiB**; probe throughput recorded;
  - with `FIN_TEST_SHARE`: a 10k-file scan (cold and warm) and an unchanged 1k-directory poll on the share, recorded.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `test(e2e): library scenario; library budgets`.

### Task 16: Spec as built, architecture pointer, tracker — tier: sonnet — after Task 15

**Files:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` §7 "as built (5D)": the crates and threads; raw names, collisions and `PATH_AMBIGUOUS`; per-OS volume identity and Tasks 2–3's remount findings; the Cloud Files and dataless findings; enumeration fallbacks and reparse-tag rules; the opener and the frozen reader rule; catalog extensions; deviations (no listing refresh on macOS shares, Linux FUSE sync metadata-only, pools per volume with a poll slot, `ROOT_VOLUME_CHANGED`, nested mounts and automounts, `DIR_EMPTY_SUSPECT`, bandwidth in params, aliases human-only and never reach, `library.remove` keeps `vectors/`, no-change polls unjournaled); measured numbers. §10 (`unicode-normalization`), §11 references. Arch §27 pointer "Sub-project 5D as built" and §20's root mapping (aliases). The 5A plan's Decision 19 gets a pointer to Decision 47 here. `TASKS.md`: tick, and follow-ups:
  - sub-project 6: register `proxy540` and the PCM feed as stages, resolve manifests through `locate`, open originals only with `open_for_read`, call `deny_dataless_materialisation()` in the `media` family, add `clippy.toml` to `fin-media`, **bump `PROBE_VERSION` whenever probe output changes**, call `prioritize` when an asset is first used;
  - sub-project 8: register analyzer stages and catalog extensions (8C transcripts, 8E chunks), the same reader rules for `analyze` and `clippy.toml` in `fin-analyze`; **8E owns cleanup of `libraries/<id>/vectors/` after `library.remove`**; 8E replaces the scorer;
  - full hashes of project assets (6/11); per-library reach lists with accounts; sub-project 9's libraries UI on `library.watch`.

- [ ] **Step 1:** Write the notes from the code. **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 5D as built`.

### Task 17: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

On macOS (Apple silicon), Windows 11 (non-elevated) and the Linux VM; host details in `archive/hosts.md`.

- [ ] **Step 1:** `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `pnpm test`, `pnpm gates`, `pnpm e2e` on each OS.
- [ ] **Step 2: Real placeholders,** where the host has them: an iCloud Drive folder with evicted files and a dataless directory (macOS); a OneDrive folder with online-only files and folders (Windows); a Nextcloud client in virtual-file suffix mode if available. Add each as a library from the console: the scan and probe passes leave every placeholder online-only (the provider's own status, and no download in its activity view), and the catalog shows them `cloudOnly`.
- [ ] **Step 3: NAS behaviour** with `FIN_TEST_SHARE` (SMB on macOS and Windows, SMB and NFS on Linux): unplug or unmount the share mid-scan → offline, nothing missing, back online → mount scan; remount the share under another source string → still the same volume (confirmation passes); a NAS left idle with no library in use sees one poll per 6 h in its access log (or the server's request counters); a sleeping disk's first stat passes within 60 s; a poll during a full scan still lands within a minute.
- [ ] **Step 4: Measure** `library-budget.ts` on each OS, warm and cold, and record in TASKS and the spec.
- [ ] **Step 5:** Fix what fails (one commit each, with a test); re-run Step 1.

## Review findings (revision 1) and where they are resolved

| Finding | Resolution |
|---|---|
| **B1** opens by normalised key | raw component chain authoritative; every open by `RawRel`; `relKey` lookup-only (Decision 10); `StageFile`/`Located` carry `root`, `rel`, `display`, `expect`, `hydrate`; `open_for_read(root, &RawRel, &Expect, hydrate)` (frozen); Task 2 `opens_non_utf8_name_by_raw_chain`, Task 1 `raw_rel_round_trips_non_utf8_and_wtf8` |
| **B2** `relKey` uniqueness | `UNIQUE (dirId, name)` / `(parent, name)` on raw bytes, `relKey` a plain index (Decision 23); collision bits and `NAME_COLLISION`; `PATH_AMBIGUOUS` for multi-row lookups; a collision never fails a batch (Decision 11); Task 1 `collision_bits`, Task 7b `twins_keep_both_rows`, `collision_never_fails_a_batch` |
| **B3** volume identity per OS | macOS `ATTR_VOL_UUID` / stripped `f_mntfromname` + type; Linux mountinfo source + type for nfs/cifs/fuse, `f_fsid` + type locally; Windows serial + UNC server/share (Decision 15); change confirmed by root file id and a top-level sample (Decision 17); Step 0 remount measurements in Tasks 2 and 3; Task 1 `volume_change_needs_confirmation` |
| **B4** readers bypass the opener | frozen rule (Global Constraints, Decision 20); `expect` and `hydrate` in `StageFile`; `deny_dataless_materialisation()` in `media` and `analyze` too (TASKS for 6, 8); `fin_probe::probe_with` for `asset.probe`/`add` (Decisions 20, 35; Task 13); clippy `disallowed-methods` in five crates with an allow-list file and a config check, replacing the grep gate (Decision 21; Task 6) |
| **B5** extension functions across ports | extensions registered by module URL, imported in both threads, exporting `migrations`, `ops`, `queries`; `IndexerClient.exec` and `ReaderClient.query`; only cloneable data crosses (Decision 24; Task 7a `extension_module_loaded_in_both_threads`, `functions_never_cross_ports`) |
| **B6** Windows reparse tags | only name surrogates skipped; data-bearing tags (hydrated `CLOUD*`, `DEDUP`, `WOF`) read when no recall bit; tag in `EaSize` for the fallback classes (Decisions 7, 8, 12, 19); Task 3 Step 0, `wof_file_is_readable`, `tag_read_from_easize_in_fallbacks` |
| **M1** automounts | `AT_NO_AUTOMOUNT` on `statx`; `STATX_ATTR_AUTOMOUNT` entries are nested mounts, not descended (Decisions 5, 16, 19); Task 2 `automount_not_triggered` |
| **M2** 8 MiB parse on the main thread | `onCall` passes the path; the indexer opens, validates and applies (Decisions 2, 26); Task 7a `batch_file_read_and_validated_in_indexer`, `main_thread_parses_no_batch`; Task 9 `on_call_passes_paths_only` |
| **M3** polls behind full scans | chosen: an `io:<key>.poll` slot per volume, with the reasons (Decision 32); Task 9 `poll_runs_beside_a_full_scan`, Task 10 `freshness_pass_not_blocked_by_full_scan` |
| **M4** offline costs attempts; batch dedupe | `LIBRARY_OFFLINE`/`RESOURCE_STUCK` and offline/pause cancels consume no attempt; a server-wide in-flight set per `(fingerprint, stage)` replaces batch-hash dedupe (Decision 40; Task 11) |
| **M5** `prioritize`; `vectors/` on remove | `prioritize(fp, stage)` frozen (Decision 41); `library.remove` keeps `vectors/`, TASKS note for 8E, written into the spec by Task 16 (Decision 42) |
| **M6** N → 0 listing | incomplete (`DIR_EMPTY_SUSPECT`), confirmed by a second empty listing ≥ 60 s later from another job (Decision 25; Task 7b `dir_drop_to_zero_needs_confirmation`) |
| **M7** Linux FUSE sync reads | metadata only unless `hydrate`; spec deviation recorded (Decision 14; Task 8; Task 16) |
| **S1** aliases and reach | aliases human-only (`library.admin`) and never reach; asset reach = exactly one catalogued row, then `open_for_read`, no prefix check; 5A Decision 19 reconciled (Decision 47; Task 13 cases; Task 16 pointer) |
| **S2** share flags, `O_NOATIME`, `openat2` fallback | all three share flags on Windows opens; `O_NOATIME` where we own the files; per-component `openat` on `ENOSYS`/`EPERM` (Decisions 5, 7, 19; Tasks 2, 3) |
| **S3** hard links, shared ids | flagged `hardlink`, never merged; ids keyed `dev:id`, hints only (Decisions 22, 25; Task 7b `hard_links_stay_two_rows`) |
| Sizing: Task 7 | split into 7a (extensions, schema, threads) and 7b (diff, property test) |
| Order: `fin-probe` | Task 5 creates `PROBE_VERSION` and waits for 6A-1's merge; TASKS note for 6 to bump it (Decision 35; Task 16) |
| Order: real dataless check | Task 2 Step 0 (`brctl evict` on an iCloud Drive host) |
| Budgets | warm budget and cold recorded separately (Task 15); FTS `dir` bytes measured in Task 7a with the `dirnames` fallback (Decision 23); cross-library cursor via a reader-thread snapshot, score-free keyset for empty queries (Decision 43; Task 12) |
