# Cross-host storage harness

Plan 4A Task 19 (spec `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §3.12, §3.13).
It runs lease and follow scenarios between several Finisher "servers" on one project folder, then
analyses what each server logged and what the folder ended up holding. A server is a server-data
directory. Its nodes can be on one machine or on several.

Nothing about hosts, shares or users is in this repository. Access comes from `XHOST_*` variables
only. Raw results go to `archive/spike/sp4a-xhost/` (git-ignored). Only durable conclusions go
into the spec.

## Parts

- `crates/fin-store-xhost` builds the `xhost` binary for one node. It is not shipped or published.
  Each subcommand prints JSONL events (see the crate docs in `src/main.rs`).
  - Every event carries the host's monotonic clock (`mono`, which counts sleep), its wall clock
    (`wall`), the node's label, the server id, and the lease state and epoch where a store is open.
  - Holders log every commit: accepted with its seq, txHash and row epoch, or refused with its
    code and reason. They also log lease gains and losses with the reason and what the gain
    replaced, heartbeats, exports, and an `alive` sample every 500 ms.
  - Followers log every applied record (seq, txHash, epoch, origin) and every rewind.
  - At the end, each node logs the fate of every commit it got accepted (`final`) and its
    database's chain.
  - Every 5 s, each node samples its wall clock against the share's: it writes
    `xhost-clock-<label>` in the project folder and reads the file's mtime back (`clock`).
- `run.mjs` is the controller. It runs a scenario round by round on N nodes, then collects and
  analyses the events.
- `analyse.mjs` is the analyser. `analyse.test.mjs` is its self-test on synthetic rounds:
  `node --test tests/storage-xhost/`.

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
node tests/storage-xhost/run.mjs <scenario> --storage local   # the same on a local disk (control)
node tests/storage-xhost/run.mjs analyse <results dir>        # re-analyse saved results
```

- `race`: `create --free` makes the project and removes the creator's lease files. Two holders
  then start at one wall-clock instant (`--start-at-ms`), commit every 300 ms for 12 s, and a
  verifier reads the result.
- `stall*`: holder `a`, contender `b` (starts 3 s later) and passive follower `f` (another node
  if there are three). The holder stalls at 10 s. `--stall-mode`:
  - `freeze`: one last commit, then no ticks or commits, and every heartbeat fails;
  - `ticks`: the main thread stops, the heartbeat goes on;
  - `beats`: only the heartbeats fail;
  - `check`: the next fence check hangs after reading the record.
- `follow`: holder `a` commits every `--commit-every-ms` (default 1000) for 70 s, and follower `f`
  on node `b` logs every apply.

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
| `latencyMs` | follow latency: a commit on one server until it is applied on another, for commits made after the follower opened |

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
