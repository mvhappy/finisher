# Tasks

Spec: `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 holds the spike conclusions).
Spike code, results and report are local-only in `archive/spike/` (git-ignored).

## Spike round 1 (2026-11-02): done

S1–S11 executed on macOS. Conclusions are in spec §27.

## Spike round 2: macOS follow-ups (done)

- [x] S6b: caption word timing — learned boundary correction; p95 46/49 ms vs MFA (held out)
- [x] S3b: end-to-end A/V latency — audio 37 ms early uncompensated; compensated p95 7.4 ms
- [x] S11b: Lottie text — thorvg text rejected for slots; shared fin-text engine injects image slots
- [x] S5b: SVT-AV1 replaces rav1e (9–22× realtime, +1.2–1.5 dB); VP9 rt as reserve
- [x] Drop Intel Mac support (D31, §23, CI matrix)
- [x] Move spikes and evaluations to git-ignored `archive/`

## Spike round 3: Linux, NAS, caption ground truth

- [x] S6c: no permissive human-labelled corpus; TTS refs rejected; ElevenLabs excluded; review render of a real reel
- [x] Caption labeller tool (`archive/tools/caption-labeler/`) for owner-corrected timings
- [x] Linux VM (no GPU): S8 Landlock 90/90, S1, S5b x86 10×, S11b determinism 20/20, S9 deb/AppImage, S10 ext4
- [x] Linux iGPU (Unraid Docker): S4 WebGPU Vulkan + VA-API WebCodecs, S3 1080p60, S5 VA-API 26.7× proxies
- [x] S10 SMB single client: round-1 lease split-brains (111/200); hardened lease 200/200
- [x] S10 cross-machine race: fenced lease required (0 early takeovers, 0 stale writes); SMB defaults in §27.9
- [x] S9 notarisation — notarised and stapled under Maryna's team 9XN7WN8JN2 (API key)
- [x] Remove leftover empty NAS dirs (removed server-side 2026-11-03)

## Spike round 4: Windows (E4)

Test PC: Win 11 Pro 26300, Ryzen 7 7800X3D, RX 9070 XT + Radeon iGPU, 31 GB.
Host access details: `archive/hosts.md` (local, git-ignored).

- [x] Batch A: toolchain (`toolchain\env.ps1`), gates fixed for Windows (888bf58), S1 pass, S5b pass (17×), S11b 20/20 identical, S9 NSIS signed 17/17 PEs, S10 NTFS pass
- [ ] S10 Windows SMB: hardened lease 105/105 split brain under the PC's +9.7 s clock skew → lease must not compare local clock to server mtime (spec)
- [ ] S10(c) Windows↔Mac cross-host race (running)
- [x] Batch B: S3 pass (D3D12, 59.96 fps), S3b compensated p95 9.1 ms at 144 Hz, S5 MF hardware decode + 18.1× proxies
- [ ] Windows open: iGPU/hybrid run (per-app graphics setting), HEVC without the extensions, 240 fps phone check
- [x] Batch C: S8 AppContainer launcher; escapes 90/90 blocked; fooled policy blocked; allow-only ACLs, per-project profile, ACL broker in finisherd
- [ ] S8 Windows: Claude Code's Bash tool (Git Bash/MSYS) cannot start in an AppContainer → decide the Windows shell-tool design
- [ ] S8 Windows: rerun as a non-elevated desktop user, with real agent turns

## Captions round (S6d/S6e)

- [x] Owner labelled the reel (187 words) and the 12-clip multilingual set (1,450 words), 2026-11-03
- [x] S6d English: B0 95.7 % starts / 75.7 % ends in window vs labels; ~40 candidates; keep B0 (+ a-priori −40 ms shift); labels anchored to prefill (46/882 edges moved)
- [x] S6e multilingual: one shared stack (whisper turbo transcript forced language + base-960h CTC w/ transliteration + GBM starts); labels = prefill (reviewed by holding Enter)
- [x] Labeller blind mode (jittered prefill, play-before-review gate, Unicode scorer); owner's en/uk labels accepted as confirmed (no relabel)
- [ ] Ukrainian number verbaliser with case agreement (MIT/Rust); multilingual end-boundary corrector on permissive data
- [x] S8 Windows follow-up: non-elevated 45/45; Credential Manager denied in container; shell tool → brokered
- [x] S5 Windows HEVC via D3D11VA direct (no MF extension): 1,500/1,500 frames exact, ~1,090 fps

## Sub-project 2: minimal core (done, merged 2026-11-04)

Plan: `docs/superpowers/plans/2026-11-04-sub-project-2-minimal-core.md`.

- [x] Tasks 0–5: spec decisions, `Rational` time, model/tx/errors with generated TS types, ops `asset.add`, `clip.insert`, `clip.trim`, `clip.delete`, rebase
- [x] Tasks 6–9: `fin-store` SQLite op log, `fin-probe`, `fin-node` napi binding, `@finisher/contracts` RPC group
- [x] Tasks 10–12: `finisherd` (socket/pipe + bearer WebSocket, live op stream), `fin` CLI offline and online
- [x] Task 13: `pnpm e2e` (fin → finisherd → WebSocket subscriber), README "Run it", CI runs e2e
- [x] Cross-platform: `cargo test --workspace` (FIN_REQUIRE_SERVER=1), `pnpm test`, `pnpm gates`, `pnpm e2e` pass on macOS, Windows 11 and the Linux VM
- [x] Windows fixes from the first run: per-server-data named pipe, fin timeouts on named pipes, probe path-through-a-file code, Windows-portable tests
- [x] Final whole-branch review; fix wave (idem-key reuse → `IDEM_KEY_REUSED`, `PROJECT_DIR_UNWRITABLE`, `CANCELLED` exit 5, empty tx rejected, clock field ranges, rustfmt gate); merged to `main`

### Follow-ups for sub-project 4 (core breadth)
- [x] Lease: implement spec §27.9 "v3" (in-place checksummed record, missing+claims = held, server-time staleness with periodic re-probe) — 4A Tasks 9–11; validation with Linux clients, network loss and real skew is 4A Task 19 (pending)
- [x] Store open replays the tail through the current `fin-core` and fails `DB_ERROR` on any diff/schema change: add migrations and rebuild-from-segments **before any op or model change** — 4A Tasks 1–3, 12
- [x] `project create`: idempotent retry, reject empty `--name`, atomic create (orphan db when the JSON write fails, check-then-rename race) — 4A Task 16 (an orphan DB after a crash mid-create remains: see 4A follow-ups)
- [x] `ops.subscribe` with `fromRev` > head should return `BASE_REV_AHEAD` — 4A Task 17
- [x] Typecheck gate (`tsc`) for all TS packages, tests included (`@types/node`) — 4A Task 17 (core-types tests not yet included: see 4A follow-ups)
- [x] Idempotency: `tx.get` RPC (asset add retry after the file moved) and a stored-args hash (same key, same op kind, different args) — 4A Tasks 15–16
- [x] Rebase: per-track (not per-tx) conservatism; stale insert vs committed `asset.remove` → `CONFLICT`; stronger proptest oracle (positions, multi-track); `clip.restore` placeholders — 4B Tasks 5–6 (`stale_insert_of_removed_asset_conflicts_removed`, `stale_and_fresh_agree_multi_track`, `restore_with_placeholder_id_binds`)
- [ ] fin output: trim echoes the resulting edge; `project open` ignores `--dry-run`; `--out < --in` fix wording; negative `details.max` text when `sourceIn` ≥ asset duration (4C)
- [x] Rational parsing: reject non-canonical input ("2/4", "-0"); `details.rev` should stay numeric — 4A Task 16
- [x] `fin-node`: Linux loader libc detection; libuv pool use under many projects (long calls on a per-engine worker thread); hubs never removed in finisherd (removed with their last subscriber) — 4A Tasks 14, 17
- [x] Generate `Probe` TS type from `fin-probe`; MOV fixture test — 4A Task 16

### Follow-ups for sub-project 5 (control plane, auth)
- Windows: named-pipe DACL and pipe-squatting protection; server-data ACL check; graceful stop (no SIGTERM)
- RPC for non-`fin` clients: decode failures and unknown tags as `FinError` (not `Die`); reject relative paths server-side (`INVALID_PATH`)
- Client hardening: overall deadline and frame-size cap; test hooks (`FIN_TEST_COMMIT_GATE`, `FIN_OFFLINE_SNAPSHOT`, `FIN_NODE_BINARY_DIR`) out of release builds
- `childEnv`: pass provider auth/config dir variables when providers arrive

## Sub-project 4A: durable storage (implemented; cross-host validation pending)

Plan: `docs/superpowers/plans/2026-11-04-sub-project-4a-durable-storage.md`. Spec: `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §3 (as built). Branch `sp4a-durable-storage`, not merged: merging waits for Task 19.

- [x] Task 1: versioned commit records, upcasters, policy-free replay (`apply_committed`); SP2 fixture
- [x] Task 2: DB schema v1, migrations (frozen m0001), pre-migration backups
- [x] Task 3: commit stores exact record bytes; open no longer re-checks diffs
- [x] Task 4: write-once file primitives, segment and snapshot codecs
- [x] Task 5: folder scan and classification (not arrived, foreign, conflict copies, caps, cache)
- [x] Task 6: pure chain resolution (record tree, line-max epoch rule, packs, fork kinds)
- [x] Task 7: export (commit-epoch labels, read-back, `TX_TOO_LARGE`, `SEGMENT_COLLISION`, `RECORD_TOO_LARGE`)
- [x] Task 8: import with quarantine, diverged tail, id-counter repair
- [x] Task 9: lease v3 (in-place record, single claim, server-clock probe, suspend-aware fencing)
- [x] Task 10: storage-kind detection (leans to sync), override warnings
- [x] Task 11: open/tick integration, ordering rule (`CATCHING_UP`), read-only precedence, project copies, restart shortcut, server-id clones, auto-rewind with generations and `HISTORY_REWOUND`
- [x] Task 12: rebuild from folder with salvage, verify report, daily backups, incarnation, `db.lock`, genesis hash
- [x] Task 13: two-phase compaction into packs (exact stand-in rule, equivalence re-resolve before deleting)
- [x] Task 14: fin-node worker thread, maintenance tick, events, `ENGINE_BUSY`
- [x] Task 13 review follow-up: transitive merges with presence by origin root (Decision 34: `origin::Roots`, same-root mirrors, `UNDOES_TARGET_RECORDS` through roots, dependents of excluded records `DEPENDS_ON_SKIPPED`, `ALREADY_PRESENT` per first binder's root), mergeId/txId guard on every write, `merged` state rechecked on a merge's target
- [x] Task 15: storage RPCs, `project.events`, `fin project status|takeover|verify|rebuild|compact|recover|acknowledge`, `tx.get` set-aside, `--idem-key` via `tx.get`
- [x] Tasks 16–17: SP2 follow-ups (atomic idempotent create, canonical rationals, `Tx.argsHash`, `Probe` type, generated envelope types, `BASE_REV_AHEAD`, hub cleanup, loader libc detection, typecheck gate)
- [x] Task 18: `pnpm e2e` storage scenario (rebuild from folder, follower)
- [x] Task 20: spec, tracker and README record the decisions (spec §3.13 holds the Task 19 checklist)
- [ ] **Task 19: cross-host and sync-folder validation** (spec §3.13). Must measure or confirm:
  - [x] full suites on Windows 11 and the Linux VM — pass on both (2026-11-05; Windows 542 Rust, 85 JS, gate tests, e2e); fixed the Windows probe ignoring sync markers. macOS 550 Rust, 85 JS, gates, e2e pass (one APFS test-only fix)
  - NAS matrix per host pair: lease races (0 double holders, 0 early takeovers, 0 applied stale writes, 0 id reuse), network loss with writes in flight, real or emulated skew, follow latency ≤ 3 s + cache window
    - [x] Windows↔Linux: all rows 0 critical on the final code (spec §3.13 item 2)
    - [x] Mac↔Linux before the fix: found the macOS smbfs 30–60 s directory cache (unsafe 10 s ordering-rule wait); fixed (65 s wait + listing refresh, 3e284dc); network poll 1 s, churn export delay 5 s
    - [ ] Mac↔Linux and Mac↔Windows on the final code (needs the rebuilt Mac binary)
  - [x] items 4, 5, 7, 8, 13, 14, 15 and most of 10 on Linux↔Windows SMB (spec §3.13); fixed the cifs cached-mtime probe read and verify flagging locked segments as corrupt
  - mtime source on SMB (server- vs client-set; Samba 2 s write-time delay; cifs deferred close); claims judged by client-set mtime under skew > 45 s
  - clock skew; suspend/resume of a holder (wakes fenced); Windows claim invisibility race (up to 9.9 s)
  - `File::try_lock`/`lock_shared` behaviour on Linux, Windows, macOS (`lease.lock`, `db.lock`, `server-id.lock`, restart shortcut)
  - machine-id sources (`/etc/machine-id`, `gethostuuid`, `MachineGuid`); `FileIdInfo` on ReFS; mount-root rules (macOS `/Volumes`, Windows mapped drives)
  - storage-kind detection on real iCloud (incl. Desktop & Documents), OneDrive (KFM, SharePoint), Google Drive (`G:`), Dropbox (File Provider), rclone/FUSE, WSL2
  - Dropbox conflict names (`conflicted copy`, `Case Conflict`, `Selective Sync Conflict`), Google Drive patterns
  - transient error lists (`fsio::is_transient`); SQLite `immutable=1` URI forms and backup fsync on Windows
  - compaction deletes on SMB (open handles), sync folders delivering deletions before packs
  - sync rows: temp-then-rename upload, placeholder hydration, sync gate after a confirmed takeover
  - daily backups: macOS local date (`localtime_r`/`tzset`), Windows `FileTimeToLocalFileTime` around DST
  - status after `lease.json` is deleted under a holder (`LEASE_HELD` vs `LEASE_LOST`)

### Follow-ups from 4A
- [ ] fin-store holds the store mutex across folder I/O in `tick` (import reads, export writes): take it only around DB transactions (read/verify segments outside, apply under; write packs outside; `VACUUM INTO` on a second connection). A big follower import delays short calls today
- [ ] `ProjectStatus` (and the verify, rebuild and compact reports) still `Schema.Unknown` in contracts: generate them
- [ ] `cargo clippy --workspace --all-targets -D warnings` is clean (Linux, 2026-11-05) but CI does not run it: add it to CI
- [ ] Sweep orphan databases left in server data by a crash between create's DB rename and its project-file write
- [ ] `meta.createKey` is lost when a rebuild has no old DB (the key is not in the folder)
- [ ] Rational schema pattern `^(0|-?[1-9]\d*)(/[1-9]\d*)?$` accepts non-canonical `0/5` and `3/1` (the parser rejects them)
- [ ] `Probe` TS type: optional fields vs `null` mismatch
- [ ] RPC envelope check vs generated schema is a vitest test (`envelopes.test.ts`), not a `pnpm gates` gate
- [ ] Typecheck gate: project list hardcoded in `scripts/check-typecheck.mjs`; core-types tests not typechecked
- [ ] fin-node `loadNative` untested
- [ ] No e2e or RPC fixture for `tx.get` set-aside / `TX_SET_ASIDE` (unit tests only; a live server cannot produce `SALVAGED`/`SUPERSEDED` without fake clocks)
- [ ] `BASE_REV_AHEAD` vs `HISTORY_REWOUND` precision for a subscriber whose `fromRev` is beyond a rewound head
- [ ] `tx.commit.generation` and `ops.subscribe.generation` optional in protocol 1 → required in protocol 2
- [ ] Linux NFS: directory attribute cache (`acdirmax`, 60 s by default) exceeds the 10 s ordering-rule wait; NFS is classified `network` but untested. Measure, and use a longer wait or a listing refresh there
- [ ] Windows: `PROJECT_NOT_FOUND` for ~2.5 s right after another host creates the project (Windows directory cache hides `.finisher/` too; the 15 s project-file retry doesn't cover it)
- [ ] macOS smbfs: whether the lease probe can read a client-set mtime (no `AT_STATX_FORCE_SYNC`); untested
- [ ] A segment held open by another SMB client slows compaction to one 60 s retry per file
- [ ] Clean close could write a "released" lease record so other servers need not wait out 45 s of staleness
- [ ] `fin project verify` / `rebuild` / `compact` have no progress or cancellation (only `ENGINE_BUSY {operation, sinceMs}`)
- [ ] `diffs::fill` loads a whole branch's rows; restore tooling for daily backups should mint a new incarnation (backups already do; a hand-copied DB does not)
- [ ] Abandoned compaction manifests are never deleted automatically; orphan packs from a crash before the manifest stay as harmless duplicates; one manifest per branch at a time
- [ ] Conflict copies are re-read on every pass (scanner never caches foreign files); a conflicted branch directory is listed, not entered (its segments are not quarantined)
- [ ] A sync folder with losing segments re-reports the fork on every poll until 4B `recover`
- [ ] A copy opened while the original's share is unmounted counts as a move; the original is then refused as the copy
- [x] Rewind rule (d) needs 4B to record branch bases (`meta` `branchBase.<b>`) — 4B Task 10 cascades instead; only hand-made roots keep the `branchBase` rule
- [x] Pack headers record replaced segments' last records (`replaces: [{hash, lastTxHash}]`, exact stand-in rule) — done in Task 13
- [x] finisherd hubs removed with their last subscriber — done in Task 17
- [ ] fin-node untested paths: `lagged` (needs > 1,024 events before registration), close on GC drop
- [ ] Warn (status) on Linux mount options that break the lease's assumptions: cifs `cache=loose`, `actimeo`/`closetimeo` > 1, SMB1 (`vers=1.0`)
- [ ] Lease: a failed in-place write of our own record could leave it torn; the holder then self-fences and the folder needs `takeover --confirm`. Consider rewriting a torn record in the heartbeat when no claim above ours exists, as `take` does after its own write
- [ ] Lease: `create_new`-only fallback for the first `lease.json` (Unix without hard links: FAT/exFAT, macOS smbfs as far as known, NAS shares refusing `link` with `EACCES`) still leaves a short record after a crash. Task 19: macOS smbfs takes that path (no hard links); Linux cifs and Windows publish atomically
- [ ] Lease: a claim `create_new` that landed but reported a transient error leaves our own orphan claim (status names it as this server's); it holds acquisition off for 45 s

## Sub-project 4B: history

Plan: `docs/superpowers/plans/2026-11-05-sub-project-4b-history.md` (branch `sp4b-history`).

- [x] Task 1: make room (split db, import, export, apply and status modules; pure refactor)
- [x] Task 2: scrubbed 4A fixture (`crates/fin-store/tests/fixtures/4a`), generated from 4A code
- [x] Task 3: record kinds with typed links, content-versioned segments (edit-only v1, any 4B kind v2), per-kind reader versions, DB migration 0002 (`branches`, `record_links`)
- [x] Task 4: exact per-track shift maps (`rebase::map`: `StepMap`, `Mapping` with mirrors; ProseMirror recover rule, Decision 16)
- [x] Task 5: per-track, per-entity conflict footprints (`rebase::footprint`, one `footprint` per op module; `Conflict` with reason codes, `Conflicts` → `CONFLICT` with `details.conflicts`)
- [x] Task 6: rebase engine rewrite (`rebase_apply`: map op by op through mirrored inverses, judge unmirrored steps, re-run with full policy; `rebase_tx` in the frame at the base, `state::state_at` in commit), `clip.restore` placeholders. Proptest budget, first measure: `rebase_props` + `shift_map` 0.15 s wall in a debug build on the Linux VM (tests 0.05 s + 0.02 s; budget < 8 s). Review fixes: undo/redo mirrors from `CommittedTx.link` (`steps_from`), `Conflict.details` and re-run blame, `details.baseRev` on a base failure, commit frame by rewind or replay (`state_near_head`); unchanged timing
- [x] Task 7: one write path (`Store::write`: dedupe, read-only, fence, IMMEDIATE txn, in-memory lease re-check, commit; dry runs in a rolled-back DEFERRED txn with no read-only or lease check, so on followers too) used by commit; lineage reads from the `branches` index (`branch::lineage`: `state_at`, `state_near_head`, `records_since` filling pending diffs per ancestor, `hash_at`/`tip_hash` with the fork-hash fallback in `refresh`); `state.rs` removed
- [x] Task 8: branch index derived from the log (`fin_core::branch`: `BranchIndex`, name rules, references `id | name | name#id`, duplicate names held by the lowest numeric id); `Store::create_branch` (`branch` record on the parent, index row, fork snapshot), `Store::set_branch` (`branchSet`: rename, archive, restore, `--restore --as`), `Store::branches`, `Store::resolve_branch`; references resolved in commit and reads; `NAME_TAKEN`, `BRANCH_ARCHIVED` (exit 3); `derived::recompute`
- [x] Task 9: branch-aware import (worklist parents first; children learnt from applied `branch` records in the same pass, with their fork snapshot; segments must start at the fork point, `Known::fork`; unfit `branch`/`branchSet` records set aside `MALFORMED`), Decision 8 (a set-aside `branch` record takes its directories' segments along, recursively; the same id made on two servers across a takeover keeps the set-aside branch's segments off the live one; directories with an unknown `branch` record wait: `unknownBranches`), export parents first, counters past branch ids and quarantined ids (`Counters::raise_to_quarantine`), status `branches[]` gains `name, parent, forkSeq, state`; Task 8 review fixes (writes refuse index-less branches, absorb validates names and fork hashes, `try-<n>-2` defaults, no-op `branchSet` refused)
- [x] Task 9b: branch identity across servers (Decision 33): `branch` records carry their fence `epoch`; an id made twice goes to (epoch desc, txHash asc), the loser is an inert row (its branch's content `ID_COLLISION`, applied subtrees voided through the cascade), unfit branch records are inert rows too (`record_links.ignored`, warning `BRANCH_RECORDS_IGNORED`); every branch segment names its `branch` record (`branchRecord`, always format 2) and is classified by it; seeds stop below the first branch record
- [x] Task 10: rewinds and diverged tails cascade to descendant branches whose `branch` record was lost, recursively (`import::cascade`: rows to quarantine by the 4A origin rule, snapshots, `segments` rows and per-branch `meta` deleted, generation bumped from rev 0, a `Rewound` event per branch; interim rule (d) gone, rule (b) counts the cascaded rows); `derived::recompute` after every rewind, diverged tail and cascade (stale `record_links` rows dropped, index rebuilt); cascade entry reusable for a voided `branch` record (`cascade::subtree` + `Cause::voided`)
- [x] Task 11: branch-aware verify, rebuild and compaction: verify resolves each 4B branch from the segments naming its index record at its fork point (`Known::for_branch`), `chain_continuity` starts at `forkSeq + 1` with `forkHash` as first parent, new check 9 `branch_index` (index and `record_links` against `derived::compute`, index rows without state, branches without index row, other segments of a branch directory named by a set-aside or unknown record only); rebuild gives the follower's DB back (branches, renames, archived, branch of branch, collisions); compaction resolves at the fork point, never packs across branch records and abandons a manifest naming another record's segment. Folded in the 9b re-review fix: a `branch` record that holds its id again after a recompute gets its fork state back in the same pass, `ID_COLLISION` follows the pass's verdicts, a reborn id forgets the old branch's rewinds
- [x] Task 12: undo and redo derived from the log: `fin_core::history` (`LogView`, per-author stacks per branch from its own records, `merged_away` drops for merges into non-descendants only, a redo record is its own unit, `plan_undo`/`plan_redo` rebase stored inverses over the records since with mirrors, full policy); `Store::undo`, `Store::redo` (through `Store::write`, links in `CommittedTx.link`, dry runs on followers, retries by txId), `Store::undo_stacks`; `HistoryResult`, `Stacks` in the generated types; `NOTHING_TO_UNDO`, `NOTHING_TO_REDO` (exit 3)
- [x] Task 13: merge through the rebase engine: `fin_core::history::{candidates, merge}` (common point of two lineages, candidates per Decision 19 with `UNDOES_TARGET_RECORDS` exclusions, steps = inverted source path + target path + planned copies with copy/original mirrors, copies keep the source's ids through `BoundIds`, `ALREADY_PRESENT` vs `ID_COLLISION` by origin-chain root, `DEPENDS_ON_SKIPPED`, full conflict list); `steps_from` mirrors an undo of a merge group; `Store::merge` (one transaction, `<mergeId>-<n>`, dedupe on `<mergeId>-1`, `argsHash`, mergeId/txId collisions refused, dry runs on followers), `MergeReport` in the generated types; `segment::set_max_records_for_test` (thread-local). Task 12 review folded in: redo stack dropped with the undo stack on merge, `HistoryReq.args_hash`. Proptest budget, second measure: `rebase_props` + `shift_map` + `undo` + `merge` 0.15 s wall (debug, Linux VM; budget < 15 s)
- [x] Task 14: cherry-pick, revert and derived `merged` state. `fin_core::history::{pick, merged, origin}`: a cherry-pick re-runs one record through the merge's steps and names its origin root (with `originHash`), and is a no-op when the target already holds that root. A revert inverts a merge group newest first, rebased over later records, and is a no-op once the group's effect is gone. `steps_from` mirrors a revert with its group. `merged` (Decision 21, for merges into a non-descendant) is computed in `derived::compute` and kept incrementally on writes and import passes; a new edit reactivates the branch. `Store::cherry_pick`/`revert` (`argsHash`, retries from the log, dry runs on followers, a txId that is a mergeId refused). Followers derive the same states in every arrival order, and so does a rebuild
  - Review fix: mirror chains pair part by part and merges/picks pair with the copy in effect (merge after revert-then-undo, or undo-then-redo, keeps the source's order; property `pick_props`); the merged state follows every branch on a copy's origin chain; a copy of a revert counts as the revert; a revert lists every conflicting record; no-op picks/reverts answer `noOp` (`ALREADY_PRESENT`, `ALREADY_REVERTED`); on import, every branch given records or made by a `branch` record is rechecked (copies may arrive first; all 120 orders converge)
- [x] Task 15: checkpoints and `undo --to`: `checkpoint` records (no ops; names per branch, case-insensitive, `NAME_TAKEN`), `Store::find_checkpoint` over the lineage newest first, `branch new --from <b>@<checkpoint>` (`At::Checkpoint`, also below the parent's fork); `fin_core::history::plan_undo_to`: one `undo` record for the author's units made after the checkpoint (a redo unit counts when the units it recreated do), equal to the undos in a row, all or nothing with every failing unit listed; `HistoryReq.to`, `CheckpointReq.args_hash`, dry runs on followers, retries, follower and rebuild equality
  - Review fix: a stale edit based between two units of an `undo --to` rebases as over the undos in a row (the undo is cut per present unit plus an unpaired tail; property `undo_to_props`); a redo whose undo is at or before the checkpoint is taken; a redo standing for units on both sides is refused (`CONFLICT` `STRADDLES_CHECKPOINT`)
- [x] Task 16: recovered branches: `Store::recover_groups` lists the current quarantine rows not recovered as `parentHash` groups (child groups for set-aside child branches, forks at the last record in `txs` or at the copy of a recovered row; problems `ID_COLLISION`, `NO_COMMON_HISTORY`, policy failures, `NOTHING_TO_RECOVER`, `DEPENDS_ON_SKIPPED`), `Store::recover` makes `recovered-<server8>-<YYYYMMDD-HHMMSS>` branches in one write (records re-run under fresh ids with literal ids remapped, as `cherryPick {origin, from, originHash}`; checkpoints again as checkpoints; `-2`, `-3` names; retries by txId; dry runs on followers) and reads `--import <dir>` segments into quarantine (`IMPORTED`, also while read-only); a row is recovered while a pick names its hash (rows without ops follow the rows around them), so status stops counting it on every server and after a rebuild

## Blocked on hardware or accounts

- Linux real desktop (vsync, Wayland/X11) and Linux S3b latency
- E6 sync services, E9 Windows signing cert
- E2 GitHub repo: no longer blocked (the repo exists), but CI has not yet completed a run on GitHub

## Open

- D33 i18n-ready from day one (spec §17.1): catalogue library choice in sub-project 4

- S6: end-boundary accuracy; spell out digit words before CTC
- S3b: 240 fps phone check of panel response; Bluetooth/AirPlay output
- S11b: colour emoji in fin-text (COLRv1)
- Cleanup when done: `~/finisher-spike` on the Linux VM (11 GB) and its `finisher-spike/gfx:1` image
