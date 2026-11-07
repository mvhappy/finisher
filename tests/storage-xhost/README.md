# Cross-host storage harness

Plan 4A Task 19 (spec `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §3.12, §3.13),
with the history rows of plan 4B Task 23 (§4.8) and the 4C rows of plan 4C Task 29 (§5.10).
It runs lease and follow scenarios between several Finisher "servers" on one project folder, then
analyses what each server logged and what the folder ended up holding. A server is a server-data
directory. Its nodes can be on one machine or on several.

Nothing about hosts, shares or users is in this repository. Access comes from `XHOST_*` variables
only. Raw results go to `archive/spike/sp4a-xhost/` (git-ignored; `XHOST_RESULTS` names another
directory under the repository, e.g. `archive/spike/sp4b-xhost`). Only durable conclusions go
into the spec.

## Parts

- `crates/fin-store-xhost` builds the `xhost` binary for one node. It is not shipped or published.
  Each subcommand prints JSONL events (see the crate docs in `src/main.rs`).
  - Every event carries the host's monotonic clock (`mono`, which counts sleep), its wall clock
    (`wall`), the node's label, the server id, and the lease state and epoch where a store is open.
  - Holders log every commit: accepted with its seq, txHash and row epoch, or refused with its
    code and reason. They also log lease gains and losses with the reason and what the gain
    replaced, heartbeats, exports, and an `alive` sample every 500 ms.
  - Followers log every applied record (seq, branch, kind, txHash, epoch, origin) and every
    rewind, and after each import that applied something the heads and hashes (`hashes`).
  - At the end, each node logs the fate of every commit it got accepted (`final`, with every
    branch's head and the hashes) and its database's chain (`main`'s rows, and every branch's
    in `all`).
  - `hashes`: every branch's head (seq and txHash), `stateHash` (every branch's state) and
    `indexHash` (the derived `branches` and `record_links` tables). Two servers that have
    followed the same folder must have the same ones.
  - Every 5 s, each node samples its wall clock against the share's: it writes
    `xhost-clock-<label>` in the project folder and reads the file's mtime back (`clock`).
- `run.mjs` is the controller. It runs a scenario round by round on N nodes, then collects and
  analyses the events.
- `analyse.mjs` is the analyser, `analyse-history.mjs` its history checks and `analyse-edit4c.mjs`
  its 4C checks. `analyse.test.mjs`, `analyse-history.test.mjs` and `analyse-edit4c.test.mjs` are
  their self-tests on synthetic rounds: `node --test tests/storage-xhost/`.

## Building

Build only this crate. `test-hooks` is on for it, and `-p` keeps the feature out of shipped
builds (a `cargo build --workspace` would unify it):

```sh
cargo build --release -p fin-store-xhost      # target/release/xhost
```

Build a native binary on each OS: Linux, macOS, and Windows (MSVC). Copy it to the remote host and
put its path in `XHOST_<NODE>_BIN`.

## Nodes

`--nodes a,b[,c]` (or `XHOST_NODES`) names the nodes. For node `N`, the variables are uppercased:

| Variable | Meaning | Default |
|---|---|---|
| `XHOST_N_CMD` | command prefix that runs a command on that host, e.g. an `ssh` alias or a wrapper script; the command line is passed as one argument | unset = local |
| `XHOST_N_BIN` | path of `xhost` on that host | local: `target/release/xhost`; remote: `xhost` |
| `XHOST_N_SHARE` | the share's harness directory as that host mounts it | `XHOST_SHARE` |
| `XHOST_N_LOCAL` | local-disk directory for the control runs | `XHOST_LOCAL` |
| `XHOST_N_DATA` | server-data base on that host's local disk | `XHOST_DATA`, else `archive/spike/sp4a-xhost/data/<N>` (local) or `xhost-data/<N>` (remote home) |
| `XHOST_N_HOST` | logical host: nodes with the same value share a monotonic clock | `local` for local nodes, else `N` |
| `XHOST_N_OS` | `posix` or `windows`: argument quoting and the path separator | `posix` |
| `XHOST_TOL_MS` | tolerance of cross-host time comparisons | `1000` |

Every round creates its own project folder, `<share>/<run id>/<scenario>-r<k>`. Server data is
reused across rounds (one identity per server), except the verifier's, which is fresh every round
so it resolves the folder from scratch. The controller can run on any machine that can reach the
nodes. Each remote host needs only the `xhost` binary built for its OS, the share mounted, and a
writable local directory for server data.

Two servers on one machine are two nodes with the same `XHOST_N_HOST` and different names, so
they get different server-data directories.

## Scenarios

```sh
node tests/storage-xhost/run.mjs race --rounds 100            # (a) lease races on a fresh project
node tests/storage-xhost/run.mjs race-stale --rounds 5        # racing the takeover of a stopped holder
node tests/storage-xhost/run.mjs stall                        # (b) holder stalls 90 s, writes in flight
node tests/storage-xhost/run.mjs stall-short                  # stall of 30 s: self-fence, no takeover
node tests/storage-xhost/run.mjs stall-check                  # stall inside a fence check (in flight)
node tests/storage-xhost/run.mjs stall-sigstop                # whole process stopped (SIGSTOP/SIGCONT)
node tests/storage-xhost/run.mjs skew --skew-ms -60000        # emulated server-time skew on the holder
node tests/storage-xhost/run.mjs follow                       # (c) follow latency
node tests/storage-xhost/run.mjs history                      # a branch made on one host merges on another (5 rounds)
node tests/storage-xhost/run.mjs mixed --nodes a,o            # a 4B holder with a 4A follower (XHOST_O_BIN: a 4A xhost)
node tests/storage-xhost/run.mjs upgrade4c                    # a 4B project opens in 4C (4C rows, below)
node tests/storage-xhost/run.mjs mixed4c --nodes a,b,c,d      # 4B and 4C servers on one project
node tests/storage-xhost/run.mjs newer4c                      # a 4C project on 4B
node tests/storage-xhost/run.mjs edit4c [--nodes a,b,c]       # fonts and captions across hosts (5 rounds)
node tests/storage-xhost/run.mjs <scenario> --storage local   # the same on a local disk (control)
node tests/storage-xhost/run.mjs analyse <results dir>        # re-analyse saved results
```

- `race`: `create --free` makes the project and removes the creator's lease files. Two holders
  then start at one wall-clock instant (`--start-at-ms`), commit every 300 ms for 12 s, and a
  verifier reads the result. `--b-delay-ms D` starts racer `b` D ms later (one OS may reach its
  claim first every time); `--skew-ms K` gives racer `a` an emulated server-time skew, as with a
  client whose mtimes come from its own skewed clock.
- `stall*`: holder `a`, contender `b` (starts 3 s later) and passive follower `f` (another node
  if there are three). The holder stalls at 10 s. `--stall-mode`:
  - `freeze`: one last commit, then no ticks or commits, and every heartbeat fails;
  - `ticks`: the main thread stops, the heartbeat goes on;
  - `beats`: only the heartbeats fail;
  - `check`: the next fence check hangs after reading the record.
- `follow`: holder `a` commits every `--commit-every-ms` (default 1000) for 70 s, and follower `f`
  on node `b` logs every apply.
- `history`: `xhost brancher` on node a holds the lease, commits a clip on `main`, makes branch
  `x`, commits `--edits` (12) edits alternating between `x` and `main` (edits 3 and 4 trim the
  same edge of the clip on each side), waits for the export, and stops without closing. When
  it has ended, `xhost merger` on node b starts with the heads it reached (`--expect`), waits
  for the takeover and those heads, merges `x` into `main` with `--skip-conflicting`, exports,
  and reports `merged` (`{merged, skipped, stateHash, indexHash, heads}`). Follower `f` on node
  c (else a, the host the merger is not on) stops one follow cycle after applying the merge
  (`followed`). Fresh verifiers run on nodes a and b.
- `mixed`: a 4B `brancher` on node a edits `main` only for `--plain-ms` (20 s), then makes a
  branch and edits both; node b runs the `follower` of a 4A build (plan 4B Decision 4).

## 4C rows

Plan 4C Task 29 Steps 4–6. The 4B nodes run a 4B `xhost` (built from the 4B branch): put its
path in that node's `XHOST_<N>_BIN`. Every other node runs the 4C `xhost` of this branch. 4B
and 4C nodes never share server data (4B nodes use `*4b` subdirectories), except in
`upgrade4c`, which upgrades 4B server data on purpose (a fresh directory each round).

| Row | Nodes | 4B nodes | Same host |
|---|---|---|---|
| `upgrade4c` | a, b | a | a and b (`XHOST_A_HOST` = `XHOST_B_HOST`: b runs on a's server data) |
| `mixed4c` | a, b, c, d (X = a, d; Y = b, c) | a, c | a with d, b with c |
| `newer4c` | a, b | b | none (two hosts) |
| `edit4c` | a, b[, c] | none | none (the offline follower c defaults to b) |

- `upgrade4c` (Step 4): node a's 4B `brancher` makes branch `x`, edits `main` and `x`, waits
  for the export and stops. Then node b's `xhost upgrade` opens the **same server data** (its
  path as node a sees it), so the 4B database is migrated in place: it logs the heads and hashes
  and each project database's `user_version` before and after the open (`upgraded`), runs
  `Store::verify` (`upgradeVerify`), re-takes its own stopped lease, undoes the 4B brancher's
  top unit on `main` (a `commit` of kind `undo`), logs the hashes (`undone`), exports and
  closes. A fresh 4C `verify` on node b rebuilds from the folder alone.
- `mixed4c` (Step 5): 4B holder a on host X writes every second for `--secs` (30) and closes.
  When it has ended, 4C holder b on host Y (`holder --hold-secs`) waits for the takeover (45 s
  staleness, then the ordering rule) and writes for `--hold-secs` (20), then writes its done
  marker. 4B follower c on Y follows throughout (`--follow-secs`, 120; raise it when b's host
  is a Mac, whose ordering-rule wait is 65 s) and must turn read-only `NEWER_FORMAT` at b's
  first record. 4C follower d on X follows until b's done marker (`follower --until-done b`).
  Fresh 4C verifiers on Y and X.
- `newer4c` (Step 5): 4C holder a creates the project, writes for `--hold-secs` (10) and
  closes; then 4B follower b on the other host follows for `--follow-secs` (15), and a 4B
  `verify` runs there (role `verify-4b`). Each must open the project read-only
  `NEWER_FORMAT` (the 4B branch has the project-file fix) or refuse it; the analyser records
  which, and that 4B wrote nothing (no commit, segment, lease record or claim). A fresh 4C
  verify on node a is the reference.
- `edit4c` (Step 6), 5 rounds: `xhost editor` on node a holds the lease and runs the edit
  script: a caption track; `caption import` (`compose::caption_import`) of Latin, Arabic, CJK
  and emoji words in `preset:bold` (Inter 700), with the `font.pin` ops
  `fin_fonts::plan::plan_pins` plans against the built-in catalog prepended (as `fin` does
  through `fonts.plan`), committed with a `fin-text` measurer over its own font cache
  (`<server-data>/fonts`), which the real fetcher fills first; a project style from
  `preset:bold` on the second caption; a `caption.setText`; an `item.move`. Once exported it
  logs `plans4c` (the hashes and the frame plans at a time inside each caption) and its done
  marker, and closes. Follower `f` on node b (`--fonts fetch`) empties its own font cache at
  start, follows until the editor's done marker plus one follow cycle (`followed`), fetches
  every pinned file itself (`fonts`, with the files downloaded), and logs `plans4c`. Follower
  `o` (node c, else b; `--fonts offline`) fetches nothing: its captions are unmeasured
  (`fontMissing`). Fresh verifiers on nodes a and b.
  - A frame in `plans4c` is `{t, sha256, shapeSha256, textEngine, captions, plan}`: `sha256`
    digests the compact plan JSON without `textEngine` (as the reel e2e does); `shapeSha256`
    also leaves out each caption's `layout` and `unmeasured`.
  - `FIN_FONT_CACHE` (a warmed cache, e.g. from `cargo run -p fin-fonts --example warm_cache`)
    on the editor's host seeds the editor's cache to save bandwidth; the fetch follower never
    uses it. Proxies come from the usual variables (`HTTPS_PROXY`, …).

The done marker is `xhost-done-<label>.json` in the project folder (`{label, heads}`), written
once everything is exported; `follower --until-done L` stops `--settle-ms` after reaching those
heads, after one more follow cycle.

## What the analyser reports

Evidence that needs no synchronised clocks comes first:

- the verifier's resolved chain;
- segment epochs and writers, claims;
- each server's own list of accepted commits, and their final status (`tx_status`).

Time comparisons use the monotonic clock between processes of one host. Across hosts they use
share time (`wall` plus the node's median `clock` offset against the share's mtimes), within
`XHOST_TOL_MS`.

| Count | Meaning |
|---|---|
| `doubleHolders` | two servers' hold windows overlap between their last `held` samples (more than the tolerance across hosts), **or** one epoch is used by two servers (lease gains, accepted commits, segment writers, claims) |
| `doubleHolderSuspects` | windows that overlap only within sampling granularity (up to the first non-held sample) |
| `earlyTakeovers` / `takeovers` | a gain replacing another server's record less than 45 s (stale-after) after that server's last stamp: its acquisition or last successful heartbeat, as sampled. Expected when the old holder's view of server time is skewed back (`skew`) |
| `unsafeTakeovers` | the same, under 25 s: sooner than the record must be seen unchanged, which outlasts the old holder's 20 s self-fence (**critical**) |
| `staleAcceptedAfterNewerEpoch` | a commit accepted at epoch E after another server's commit at a higher epoch |
| `staleExported` | records in the folder that are not on the resolved chain |
| `staleApplied`, `rewinds` | records a node applied that are not on the resolved chain, and rewind events |
| `staleLeftApplied` | rows of a node's final database that are not on the resolved chain |
| `idReuse` | a txId accepted by two servers, an entity id bound by two commits on the resolved chain, or a seq twice on the chain |
| `seqCollisions` | a seq accepted by two servers (information: expected only when a stale write was accepted) |
| `lostAcked` | accepted commits missing from the resolved chain (**critical**), split into `lostAckedSetAside` (in the writer's quarantine, e.g. `DIVERGED_TAIL`) and `lostAckedVanished` |
| `openRetries` | opens refused with `PROJECT_NOT_FOUND` and retried (`--open-retry-ms`; the `history` merger retries for 90 s): a folder another host has just made, not yet in this host's directory cache (information) |
| `latencyMs` | follow latency: a commit on one server until it is applied on another, for commits made after the follower opened |

History counts (`history` and `mixed`, `analyse-history.mjs`):

| Count | Meaning |
|---|---|
| `historyMerged`, `historySkipped` | records the merge copied, and those it skipped |
| `historySkippedMissing` | the branch's conflicting trim was not reported skipped with `SAME_EDGE` (**critical**) |
| `historyMergedWrong` | the merge did not copy every other branch edit (**critical**) |
| `historyHashMismatches` / `historyViews` | views (the follower after its follow cycle, each fresh verifier) whose `stateHash` or `indexHash` differs from the merger's (**critical**) |
| `mixedFollowed` / `mixedEditOnly` | edit-only records in v1 segments the 4A follower applied (`mixedMissed`: **critical**) |
| `mixedEditsInBranchSegment` | edit-only records exported in the same segment as the branch record: that segment is v2, so a 4A reader never sees them (information) |
| `mixedAppliedPastBranch` | records at or after the branch record the 4A follower applied (**critical**) |
| `mixedNewerFormat` | the 4A follower turned read-only `NEWER_FORMAT` (0 is **critical**) |

4C counts (`analyse-edit4c.mjs`; every row's 0 or missing reference is **critical**):

| Count | Meaning |
|---|---|
| `upgradeMigrated` | the project's database was in the server data, at an older `user_version`, before the upgrade opened it |
| `upgradeHeadsSame` | every branch head (seq and txHash) after the upgrade equals the 4B brancher's final heads |
| `upgradeVerifyOk`, `upgradeUndone` | `Store::verify` after the upgrade is ok; an undo of a 4B record was accepted |
| `upgradeRebuildSame` | the fresh verify's `stateHash` and `indexHash` equal the upgrade's after the undo |
| `mixed4cFollowed4b` / `mixed4cAccepted4b` | the 4B holder's records the 4B follower applied (`mixed4cMissed`: **critical**) |
| `mixed4cApplied4c` | 4C records the 4B follower applied (**critical**) |
| `mixed4cNewerFormat` | the 4B follower turned read-only `NEWER_FORMAT` |
| `mixed4cHashMismatches` / `mixed4cViews` | fresh verifiers whose hashes differ from the 4C follower's after b's done marker (**critical**) |
| `newer4cReadOnly`, `newer4cRefused` | 4B processes (the follower and the verify) that opened the 4C project read-only `NEWER_FORMAT`, or refused it (both are accepted outcomes; a refused open is noted, not a problem) |
| `newer4cWritable`, `newer4cWrote4b` | 4B processes that opened it writable, and 4B commits, segments, lease records or claims (**critical**) |
| `edit4cHashMismatches` / `edit4cViews` | both followers after their follow cycle and the fresh verifiers whose hashes differ from the editor's (**critical**) |
| `edit4cPlanMismatches` / `edit4cFrames` | frame plans of the fetch follower whose `sha256` differs from the editor's (**critical**) |
| `edit4cOfflineUnmeasured`, `edit4cOfflineMeasured` | the offline follower's caption layers unmeasured with `fontMissing`, and others (**critical**) |
| `edit4cOfflineOtherDiffs` | the offline follower's frames that differ from the editor's beyond caption `layout`/`unmeasured` and `textEngine` (**critical**) |
| `edit4cEditorUnmeasured` | caption layers the editor did not measure (**critical**) |
| `edit4cDownloaded` / `edit4cPinnedFiles` | files the fetch follower downloaded itself, of the distinct files the project pins (fewer is **critical**) |

Lost records are the common `lostAcked` count: with `all`, every branch's accepted records
(branch and merge records included) must be on the verifier's resolved rows.

A round is critical when it has a double holder, an unsafe takeover, a lost acknowledged write that is not in
quarantine, or a stale record left applied. `summary.json` holds the counts per scenario and round,
and `detail.json` every finding with its evidence and the measured clock offsets.

Limits:
- A hold window is sampled from the node's main loop. While a tick is blocked on the share, no
  samples are taken, so an overlap in that time is only a suspect. The commit and folder evidence
  covers actual double writes.
- The `skew` scenario emulates skew through the lease's `MonoClock::server_shift_ms`: the holder
  sees the share's clock shifted, as with a client-set mtime on a skewed client. It is not a real
  clock offset.
