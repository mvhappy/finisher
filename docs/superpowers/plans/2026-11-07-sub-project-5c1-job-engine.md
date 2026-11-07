# Sub-project 5C-1: The job engine — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for kill and supervision semantics (the host's spawning and kill trees on each OS, host death, the engine's run loop, retries and isolation, warm workers, validation), `sonnet` for plumbing (the envelope crate, the worker binary, the napi binding, the store and kind registry, the scheduler, wiring, docs).

**Goal:** the one shared job system of spec §6.0, without its RPCs. After this plan:
- **`fin-jobs`** holds the envelope (Rust → JSON Schema → generated TS) and the worker and helper runtimes that every family links;
- **`fin-workers <family>`** is the one worker binary, with the **`test`** family (behind `test-hooks`) and the Unix host watchdog;
- **`fin-jobhost`** (linked into `fin-node` as its `jobhost` module) spawns every worker and helper through a **`Spawner`** hook with a cleared environment, frames the envelope, runs the liveness, stall and timeout timers, enforces memory limits and owns a **host-side kill tree on every OS**: a kill-on-close Job Object on Windows (each process created suspended, assigned, then resumed), process groups with `PR_SET_PDEATHSIG` and cgroup v2 where delegated on Linux, process groups on macOS, a watchdog on Unix, and stdin-EOF exit everywhere;
- **`finisherd`** has the **`JobEngine`**: jobs persisted in `control.db` (the `jobs` migration chain), a pure scheduler over resource pools (`cpu`, `gpu`, `hwenc`, `net`, `io:<root>`), warm `interactive:<family>` pools with idle expiry and recycling, helpers and mapped files, the `prepare` and `onSucceeded` kind hooks, retries with backoff, poison handling and isolation of crashing batches, restart re-queue, and the `job.done` and `worker.exit` events behind a sink interface;
- sub-project 6's 6A-2, 5D and sub-project 8's 8B/8C build against the frozen interfaces listed under **Frozen interfaces**.

**Not in this plan:** the `jobs.*` RPCs, scopes, ownership and reach checks, `jobs.watch`, `fin job …` and `fin wait` (5C-2); the `library` family (5D); the `media` family, `fin-shm` and the media `job.done` fields (sub-project 6); `fin-sandbox` (sub-project 7: the `Spawner` hook is here, its implementation is not); plugin adapters and `externalRef` (sub-project 8); journal wiring (diagnostics D4: events go to a sink until then).

**Architecture:**
- **Rust:** `crates/fin-jobs` (new; protocol types, worker and helper runtimes, the read budget, the params registry type; no process code), `crates/fin-jobhost` (new rlib; spawning, groups, kill trees, framing, timers, memory, watchdog client), `crates/fin-workers` (new binary; family dispatch, the `test` family and helpers behind `test-hooks`, `watchdog`), `crates/fin-node/src/jobhost/` (napi `JobHost`, `validateJobParams`), `crates/fin-core/src/events` (`job.done`, `worker.exit`, three name tables), `crates/fin/src/output.rs` (codes).
- **TypeScript:** `packages/contracts/src/jobs.ts` (TS-owned job records as Effect Schemas), `apps/finisherd/src/control/migrations/jobs/0001.ts`, `apps/finisherd/src/jobs/` (store, kinds, pools, scheduler, host wrapper, files, runner, isolation, warm pools, events, engine, layer), `packages/fin-node` (typings, `workersBinaryPath`, builds that place `fin-workers` beside the `.node` file).

**Tech Stack:**
- Rust 1.99.1, edition 2024. `serde`, `serde_json`, `schemars` 1, `blake3` 1 (all already in the tree); **`libc` 0.2** (MIT OR Apache-2.0; already in the lock file, now a direct dependency of `fin-jobhost` and `fin-workers` on Unix); **`windows-sys` 0.61** (MIT OR Apache-2.0; already in the tree) with `Win32_Foundation`, `Win32_Security`, `Win32_System_JobObjects`, `Win32_System_Threading`, `Win32_System_Pipes`, `Win32_System_IO`, `Win32_Storage_FileSystem` in `fin-jobhost`; `proptest` 1 (dev, already in the tree). Pipes through `std::io::pipe` (stable since 1.87).
- Node 26.12 using only APIs Node 24.22 also has; Effect 4.0.6; vitest; **`fast-check` 4** (MIT; dev-only, exact pin) for the scheduler and isolation properties.
- **No other new dependency.** Rejected: `nix` (not needed beyond `libc`), `tokio`/`async-process` (blocking threads per process are simpler and bounded), `command-group` and `process-wrap` (they cannot create processes suspended or join a pre-made Job and cgroup), `sysinfo` (heavy; RSS is two syscalls), `jsonschema` (params are validated by their Rust types).

**Spec:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` revision 3, §3.1 (migration chains), §3.4 (`job.done`, `worker.exit`), §6.0–6.6, §6.8, §6.9, §10, §11 Decisions 23–24d. Sub-project 6: `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` §3.1–3.6, §14 (memory defaults), §17 (`WORKER_OOM`). Sub-project 8: `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §3.3, §3.7, §16.2. 6A plan: `docs/superpowers/plans/2026-11-07-sub-project-6a-media-foundations.md` (6A-2 entry check and Task 12's kind declaration). 5A plan: `docs/superpowers/plans/2026-11-07-sub-project-5a-principals-and-scopes.md` (Tasks 1 and 2 are frozen).

**Branch:** `sp5c1-job-engine`, stacked on `sp5a-principals` (5A Tasks 1, 2 and 10 in; this plan uses only Tasks 1 and 2). It runs beside 5A Tasks 3–14 and beside 6A-1. 5C-2 stacks on this branch and on 5A Task 6. Rebase onto `main` as 4C and 5A merge.

**What this plan uses from 5A (frozen):** `openControlStore`/`ControlStore {db, tx, deleteInBatches, close}` and `registerChain(name, migrations)` (`apps/finisherd/src/control/{db,migrations}.ts`), the `ControlStoreTag` layer, `ensurePrivateDir(dir)` (`config.ts`) for `<server-data>/jobs/`, `childEnv()` (`env.ts`). Nothing else: no principals (owners are recorded, not checked), no audit kinds (none are job kinds).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate over what each task adds.
- **Primitive ops live only in `fin-core`.** This plan adds none. `fin-core` stays pure (it gains only event payloads and name tables).
- **TypeScript model types are generated, never hand-written.** The envelope, the test kinds' params and results, `JobDone` and `WorkerExit` are Rust types exported through `fin-store`'s `export_schema` example and regenerated with `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). TS-owned control-plane records (the job row and its views, kind declarations) are Effect Schemas in `@finisher/contracts`, their TS types `Schema.Type` of them. **Params are validated by Rust** (`validateJobParams`, Decision 26), as the contracts say for every RPC payload.
- **Dependency rules:** `fin-jobs` has no process, thread-pool or I/O dependency beyond `std`; `fin-workers`' shipped families never call a process-creating API (Decision 24); `finisherd` uses no `child_process` for jobs.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit: `crates/fin-core/src/events/names.rs` 504 (new tables go into `names_jobs.rs`, referenced from `TABLES`), `crates/fin-core/src/events/kinds.rs` 370 (two registry lines). Planned splits are named in each task; a file that grows past 550 lines is split before the commit.
- Errors are `{ code, message, details, fix? }`. New codes go into `crates/fin/src/output.rs` and the contracts' code list (Decision 41).
- **The repo is public:** no host names, IPs, user names, cgroup paths of a real machine or key names in tracked files.

**Every commit is green** on macOS, Windows 11 and Linux. Until Task 5 lands, `fin-jobhost` answers `JOBHOST_UNSUPPORTED` on Windows and its process tests are `#[cfg(unix)]`; the TS tests that spawn real processes skip on `win32` with a notice naming Task 5. From Task 9 on, nothing skips on any OS except the cgroup tests (Decision 15) where cgroups are not delegated.

**Testing and process**
- Tests never use the network. Every engine test uses a temp server data and a test-hooks `fin-workers` (`packages/fin-node/test/native/`).
- **Process tests** (`crates/fin-workers/tests/host_*.rs`) find the binary through `env!("CARGO_BIN_EXE_fin-workers")`; host-death tests re-execute the test binary as the host (the `fin-fonts` crash-child pattern) and kill it with `SIGKILL` / `TerminateProcess`. A pid is "gone" when `kill(pid, 0)` answers `ESRCH` (Unix) or `OpenProcess` fails / `GetExitCodeProcess` is not `STILL_ACTIVE` (Windows), polled for ≤ 5 s.
- Timing-dependent tests take their timers from `Timing` (Rust) or Effect's `TestClock` (TS) and never sleep for a real 30 s.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks 6 and 9–12 also run `rtk pnpm e2e` once at the end (it must stay green; 5C-1 adds no scenario, Task 12 says why).
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **Tracker:** Task 1 adds a "Sub-project 5C-1: job engine" section to `TASKS.md` with one line per task; each task ticks its line, with as-built notes, in its own commit.
- **Batches:**
  - A: Task 1 → Task 2; Tasks 7 and 8 beside them once Task 1 is in;
  - B: Task 3 → Tasks 4 and 5 in parallel (disjoint files: `unix/`, `linux/`, `macos/` vs `windows/`); Task 6 after Task 3, beside 4 and 5;
  - C: Tasks 9 → 10 → 11 (strictly in order; 9 needs 4, 5, 6, 7, 8);
  - D: Tasks 12 → 13 → 14.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Crates and binaries

1. **Three crates.** `fin-jobs` (protocol and runtimes, linked by every worker and helper and by the host), `fin-jobhost` (the host side: an rlib so its logic is tested natively; `fin-node` is a `cdylib` with `test = false`), `fin-workers` (the binary). `fin-node`'s `jobhost` module is only the napi binding over `fin-jobhost`, so spawning still lives in `finisherd`'s process (spec §6.2), and `fin-sandbox` (sub-project 7) can implement `Spawner` without napi.
2. **Where binaries are found.** `fin-workers[.exe]` sits beside the `fin-node` `.node` file: `packages/fin-node/` (release `build`) and `packages/fin-node/test/native/` (`build:test`, with `test-hooks`); `FIN_NODE_BINARY_DIR` already selects the directory. A family's program is **`Program::Bundled(name)`**, a bare name resolved in that directory and limited to `fin-workers` and, for 8C, `fin-analyze`, or **`Program::Path(abs)`** for plugin processes (8B), set only by a registered family or helper resolver, never from job params.
3. **Versioning.** `hello` carries `v` (= 1), `envelope` (the first 16 hex digits of BLAKE3 over the envelope's canonical JSON Schema, computed once per process) and the `{kind, version}` pairs it serves. The host requires `v == 1`, an equal `envelope` and the dispatched kind at the declared version; anything else is **`WORKER_PROTOCOL_MISMATCH {expected, got}`** (never retried; the job fails).

### The envelope (`fin-jobs`)

4. **Framing:** UTF-8 NDJSON on the process's stdin/stdout, one message per line: `{"v":1,"t":"<type>","seq":n, …}`. Lines ≤ 1 MiB; a `checkpoint`'s `data` ≤ 256 KiB; `seq` starts at 1 in each direction and grows by exactly 1. Bad JSON, an unknown `t`, a `seq` gap, an oversized line or a message naming another job than the one started → the host kills the process with reason **`protocolError`**; the job sees `WORKER_CRASHED {protocol: <what>}` and is isolated like a crash (Decision 31). **stderr** is a log stream: lines ≤ 4 KiB go to `finisherd`'s log at debug level as `[job <id> <family>] …`; the last 8 KiB are kept in memory and stored as `error.details.stderrTail` of a crashed attempt (control.db only, never the journal).
5. **Messages.** Host → worker: `start {job: {id, kind, kindVersion, params, checkpoint?, attempt, extra?}, limits: {aliveMs, memoryBytes?, readBytesPerSec?}, files: [{name, path, mode, bytes?}], helpers: [{id, kind, files}], dirs: {job, tmp}, diagnostics}`, `cancel {job, reason: cancel|pause}`, `shutdown {}`. Worker → host: `hello {v, role: worker, family, kinds, warm, envelope, pid}`, `begin {job, item}`, `item {job, item, value?, error?}` (with `error`: a failed item, recorded in `itemErrors`; the job goes on), `progress {job, done, total?, unit, phase?}`, `checkpoint {job, data}`, `alive {}`, `result {job, value}`, `error {job, error: {code, message, details, retry: never|backoff|fresh}, itemErrors?}`, `ready {}`, `diag {job?, kind, payload}`. Any message counts as alive; the runtime sends `alive` every `limits.aliveMs` (5 s) from its own thread.
6. **Worker runtime** (`fin_jobs::worker::run`): sends `hello`, reads `start`, runs the family's `run` with a `JobCtx`, answers `result` or `error`, then **exits 0 (cold) or sends `ready` (warm)**. **stdin EOF at any time → `std::process::exit(EXIT_HOST_GONE = 70)` at once** (the host is gone). `cancel` sets the job's `CancelToken`; the family checks it and returns `JobError::cancelled()`; with `reason: pause` a resumable family writes a checkpoint first. `shutdown` (sent only to an idle worker) exits 0. A panic in `run` is caught, answered as `error {code: WORKER_CRASHED, retry: backoff}` and the process exits 101 (a warm worker is not reused after a panic).
7. **`diag`** is forwarded to the sink (Decision 39) only when `start.diagnostics` is true and the kind is in the family's `diagKinds`; otherwise it is dropped and counted (`workers().diagDropped`). With `diagnostics: false`, `JobCtx::diag` is a no-op, so a family builds no payload (6A-2's "zero emissions" test).
8. **Helpers** have their own channel: helper → host `hello {v, role: helper, kind, id, envelope, pid}`, `alive`, `error`; host → helper `attach {job, files, dirs}`, `shutdown {}`. The helper runtime (`fin_jobs::helper::run`) mirrors Decision 6 (stdin EOF → exit 70). A helper's stdio goes to the host only; workers and helpers share nothing but the job's mapped files.

### Spawning and the kill tree (`fin-jobhost`)

9. **`Spawner`** (spec §6.2) is called for every process with a `SpawnRequest` (role, profile `worker|plugin`, family, helper, program, args, env, cwd, path grants, QoS) and may rewrite a `PlatformCommand` (program and arguments for a wrapper; extra environment; on Linux extra child-side `pre_exec` steps run after ours; on Windows extra process attributes and a token). macOS takes no `pre_exec` (Decision 13): a sandbox there rewrites the program. The default is **`Unsandboxed`**: `caps().sandboxed == false`, the engine logs one warning at start and `workers()` reports it (5C-2 puts it in `server.status`).
10. **Environment and handles.** Every process gets `childEnv()` with no extras (never `FIN_SERVER`, `FIN_SESSION`, `FIN_CREDENTIAL`), plus `TMPDIR`/`TEMP`/`TMP` = its private temp directory (Decision 33); cwd = the job directory (cold) or the worker directory (warm). It inherits exactly stdin, stdout and stderr: Linux `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` in `pre_exec` (fallback: `fcntl(FD_CLOEXEC)` up to `RLIMIT_NOFILE`), macOS `POSIX_SPAWN_CLOEXEC_DEFAULT`, Windows `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` with the three pipe ends.
11. **One spawner thread.** Every spawn runs on one long-lived `jobhost-spawn` thread: on Linux `PR_SET_PDEATHSIG` fires when the *thread* that forked exits, so spawning from a short-lived napi or pool thread would kill healthy workers. Per process the host runs a stdout reader, a stderr reader and a waiter thread (blocking I/O; ≤ ~40 processes in practice); stdin is written under a per-process mutex.
12. **Linux:** `std::process::Command` with `CommandExt::pre_exec` (async-signal-safe calls only), in order: `setpgid(0, group_pgid_or_0)`; `prctl(PR_SET_PDEATHSIG, SIGKILL)`; `getppid() != host_pid` → `_exit(70)`; where cgroups are delegated, write `"0"` to the group's `cgroup.procs` through an fd opened before the fork; `setpriority` for QoS; `close_range`. Nothing of the worker runs before it is in its process group and cgroup.
13. **macOS:** `posix_spawn` through `libc` with `POSIX_SPAWN_SETPGROUP`, `POSIX_SPAWN_CLOEXEC_DEFAULT`, **`POSIX_SPAWN_START_SUSPENDED`** and `posix_spawnattr_set_qos_class_np`; the host registers the pgid with the watchdog (Decision 15) and then resumes the process with `kill(pid, SIGCONT)`. A failure between spawn and resume kills the suspended process.
14. **Windows:** at `Host::start` one **host Job** (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`, no `BREAKAWAY_OK`, a non-inheritable handle held only by `finisherd`) and one I/O completion port; per group a **nested Job** with the same flags plus `JOB_OBJECT_LIMIT_PROCESS_MEMORY` when the group has a limit, associated with the port (key = group id). Each process: `CreateProcessW` with `CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT` and the QoS priority class, an explicit environment block and the handle list → `AssignProcessToJobObject(host)` → `AssignProcessToJobObject(group)` (that order builds the nesting even when `finisherd` itself already runs inside a terminal's or Electron's Job) → `ResumeThread`. A failure at any step `TerminateProcess`es the suspended process (`SpawnFailed`). Cancel is `TerminateJobObject(group)`. The port's `NEW_PROCESS`, `EXIT_PROCESS`, `ABNORMAL_EXIT_PROCESS`, `PROCESS_MEMORY_LIMIT` and `ACTIVE_PROCESS_ZERO` messages drive Decisions 18, 20 and 22. If `finisherd` dies, the host Job's last handle closes and every process dies.
15. **Unix host death: the watchdog.** Process groups alone do not cover a dead host: on Linux `PR_SET_PDEATHSIG` reaches only direct children, and macOS has no equivalent, so a grandchild (spec §6.9's `test.spawn`) or a hung worker that never reads stdin would survive. `Host::start` therefore starts **`fin-workers watchdog`** in its own process group, ignoring `SIGINT`, `SIGTERM`, `SIGHUP` and `SIGPIPE`. The host writes `+g <pgid>` / `-g <pgid>` and `+c <cgroup dir>` / `-c <cgroup dir>` lines to its stdin as groups open and close. On stdin EOF or the parent's exit (Linux `pidfd_open` + `poll`; macOS `kqueue` `EVFILT_PROC`/`NOTE_EXIT`), it sends `SIGKILL` to every listed process group, writes `1` to every listed `cgroup.kill`, and exits. A process group id cannot be reused while any member lives, so the kill cannot hit a stranger. If the watchdog dies, the host logs it, restarts it at most once a minute and re-sends the list. The watchdog is host machinery, not a worker: it serves no job and starts no process.
16. **cgroup v2 (Linux), where delegated.** Used only when `/proc/self/cgroup` names a v2 path `<cg>` that is writable by us, whose `cgroup.procs` holds only `finisherd`'s own pid, and that has `cgroup.kill` (kernel ≥ 5.14) — true for a systemd user service with `Delegate=yes` or a container with its own cgroup namespace, false for a terminal's scope (the shell lives there too), so `finisherd` never rearranges a cgroup it does not own. Then the host moves itself into `<cg>/host`, enables the available `+memory +pids` controllers on `<cg>` and `<cg>/jobs`, and makes `<cg>/jobs/g<group>` per group (`memory.max` from the group's limit, `memory.oom.group = 1`). Kill is `cgroup.kill`; an OOM is `memory.events` `oom_kill > 0`. `FIN_CGROUPS=off` in `finisherd`'s environment disables it; anything else is `auto`. Otherwise process groups.
17. **Groups.** A group is one kill domain: a nested Job (Windows), a cgroup (Linux, delegated) or a process group (otherwise; the group's first process creates the pgid, later ones join it). A **cold job** has one group for its worker and helpers. A **warm worker** has a group of its own, and each job it runs gets a second group for that job's helpers (spec §6.3).
18. **Kill and abandonment.** `kill_group` = `TerminateJobObject` / `cgroup.kill` / `kill(-pgid, SIGKILL)`, then waits `killWaitMs` (5 s) for every process of the group to be reaped. One still alive (uninterruptible I/O) is **abandoned**: reported once (`worker.exit` reason `abandoned`), its pool slot released, reaped in the background and logged when it finally exits. Two abandoned processes whose jobs share an `io:` pool mark that pool **stuck**: nothing is dispatched there and new submits that need it fail **`RESOURCE_STUCK {pool}`** (exit 5) until one of them exits.
19. **Timers** (in the host, monotonic `Instant`, values from `Timing`; tests shorten them): **hello** 10 s after spawn → `noHello` (`WORKER_UNAVAILABLE`, retried with backoff); **hung** 30 s without any message after `hello` → `WORKER_HUNG`; **stall** the kind's `stallS` (default 600 s) without `begin`, `item` or `progress` while a job runs → `JOB_STALLED`; **timeout** the kind's `timeoutS` from `start` → `JOB_TIMEOUT`. Each kills the job's groups; a hung or stalled warm worker is lost with its group.
20. **Memory limits** (sub-project 6 §3.2 item 5). A `MemoryRule {fixedMiB?, ramFraction?, capMiB?}` resolves at dispatch to bytes: `fixedMiB`, else `min(capMiB, ramFraction × os.totalmem())`. The family gives a rule per class (`interactive`, `batch`), a kind may override it, as a function of its params (`media.render` at 2160p). Enforcement: Windows `JOB_OBJECT_LIMIT_PROCESS_MEMORY` on the group's Job (the port's `PROCESS_MEMORY_LIMIT` marks the exit `oom`, and the process is killed if it did not crash by itself); Linux delegated `memory.max`; elsewhere the host samples the RSS of every process of the group every second (Linux `/proc/<pid>/statm`, macOS `proc_pidinfo(PROC_PIDTASKINFO)`) and kills the group over the limit. All three answer **`WORKER_OOM {limit, rss?}`**. A warm worker's group takes the interactive limit.
21. **QoS** (spec §6.2): `normal`; `utility` = macOS `QOS_CLASS_UTILITY`, Linux nice 5, Windows `BELOW_NORMAL_PRIORITY_CLASS`; `background` = `QOS_CLASS_BACKGROUND`, nice 10, `BELOW_NORMAL_PRIORITY_CLASS`. Interactive jobs run at `normal` whatever their kind says.
22. **Foreign processes.** The host counts processes in a group beyond the ones it spawned: Windows from the port's `NEW_PROCESS` messages; Linux from `cgroup.procs`, or a `/proc` scan for the pgid at each `result` and at exit; macOS `proc_listpgrppids`. `ExitInfo.foreign` carries the count. The engine logs any non-zero count; with the `test-hooks` build of `fin-node`, a job of any family but `test` with `foreign > 0` fails `WORKER_CRASHED {foreign}`, so 6A-2's and 5D's test suites catch a family that starts processes.
23. **Cancel** (spec §6.2): `cancel {reason: cancel}`, a grace of 2 s (500 ms for interactive jobs), then `kill_group` on the job's groups. On a warm worker, a job that answers within the grace (`error JOB_CANCELLED` then `ready`) keeps the worker; otherwise the worker's group is killed too.
24. **Workers never start processes** (spec §6.2, Decision 24b): held by the static gate `scripts/check-worker-spawn.mjs` (in `pnpm gates`), which refuses `std::process::Command`, `fork`, `vfork`, `posix_spawn*`, `exec*`, `CreateProcess*`, `ShellExecute*` and `clone` in `crates/fin-workers/src/` and `crates/fin-jobs/src/` outside `test_family/` and `watchdog.rs`, and by Decision 22 at run time.

### Engine and persistence (`finisherd`)

25. **The `jobs` chain, step 1** (`control/migrations/jobs/0001.ts`, frozen once merged):
    ```sql
    CREATE TABLE jobs (
      id TEXT PRIMARY KEY, kind TEXT NOT NULL, kindVersion INTEGER NOT NULL, family TEXT NOT NULL,
      params TEXT NOT NULL, ownerKind TEXT NOT NULL, ownerId TEXT NOT NULL, via TEXT, rights TEXT,
      project TEXT, library TEXT, parent TEXT, priority TEXT NOT NULL, state TEXT NOT NULL,
      pausedFrom TEXT, warm INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0,
      maxAttempts INTEGER NOT NULL, idemKey TEXT, dedupeKey TEXT, checkpoint TEXT, extra TEXT,
      result TEXT, error TEXT, isolation TEXT, createdAt INTEGER NOT NULL, startedAt INTEGER,
      finishedAt INTEGER, nextRunAt INTEGER);
    CREATE UNIQUE INDEX jobs_idem ON jobs (ownerKind, ownerId, idemKey) WHERE idemKey IS NOT NULL;
    CREATE INDEX jobs_dedupe ON jobs (kind, dedupeKey) WHERE dedupeKey IS NOT NULL;
    CREATE INDEX jobs_active ON jobs (state, priority, createdAt) WHERE finishedAt IS NULL;
    CREATE INDEX jobs_finished ON jobs (finishedAt) WHERE finishedAt IS NOT NULL;
    CREATE INDEX jobs_owner ON jobs (ownerKind, ownerId, createdAt);
    CREATE TABLE job_items (jobId TEXT NOT NULL REFERENCES jobs (id), item TEXT NOT NULL,
      value TEXT, error TEXT, PRIMARY KEY (jobId, item)) WITHOUT ROWID;
    ```
    `isolation` holds the isolation state of Decision 32 (JSON). No foreign key leaves the chain. `formatVersion` 1, `minReaderVersion` 1. The chain registers itself when `jobs/store.ts` is imported, which `main.ts` does before it opens the store.
26. **Writes:** a state transition is one `UPDATE` at once; progress stays in memory (events at most 4 per second per job); a checkpoint is written at most every 10 s per job (the newest kept, flushed at the next transition); item outcomes are written in one transaction per second or per 256 rows. Every statement is prepared once (p99 ≤ 1 ms with 10k jobs, Task 12's budget).
27. **Submit:** unknown kind → `JOB_KIND_UNKNOWN` (3); params through fin-node's `validateJobParams(kind, version, params)` (each kind's Rust params type, registered by its family's crate) → `JOB_PARAMS_INVALID {errors}` (3); the same `idemKey` from the same owner → that job; the same `dedupeKey` while `queued`, `running` or `paused` → that job; a kind with `rememberPoison` and a finished job of the same `dedupeKey` that failed `POISON_ITEM` → that job (sub-project 6 §3.3); 50,000 unfinished jobs → `JOB_QUEUE_FULL` (5). Then insert and schedule in the same turn.
28. **Restart:** at start every `running` row becomes `interrupted`; one that is resumable (a `resumable` kind, or a batch kind with items left) and has attempts left goes back to `queued` from its checkpoint and recorded items (the interrupted run counts as an attempt); the others stay `interrupted` with `JOB_INTERRUPTED` and emit `job.done` with outcome `interrupted`. `paused` rows stay paused. Every directory under `<server-data>/jobs/` is deleted (nothing runs yet).
29. **Graceful close** (`engine.close`): dispatch stops; idle warm workers get `shutdown`; running jobs get `cancel {reason: pause}` and a 2 s grace, then their groups are killed and they are handled as Decision 28 handles them, at once, so a clean stop leaves no `running` row; then `Host::shutdown`.
30. **Retention:** at start and hourly, finished jobs older than 7 days or beyond the newest 10,000 finished are deleted: their `job_items` first, then the rows, ≤ 500 rows per statement through `deleteInBatches`.

### Retries and isolation

31. **Failure classes** (spec §6.5):

    | Cause | Retry | Isolate (bisect) |
    |---|---|---|
    | crash: exit without `result`/`error`, a signal, `protocolError`, a caught panic | backoff | yes |
    | `WORKER_OOM` | with an item in flight: as a crash; otherwise none (it fails) | only the in-flight rule |
    | `WORKER_HUNG`, `JOB_STALLED`, `JOB_TIMEOUT`, `HELPER_FAILED`, `noHello`/`SpawnFailed` (`WORKER_UNAVAILABLE`) | backoff | never |
    | worker `error` with `retry: backoff` (I/O, an offline root) / `fresh` (`GPU_DEVICE_LOST`) / `never` (default) | backoff / once at once on a fresh process / none | never |
    | `WORKER_PROTOCOL_MISMATCH`, a failed `prepare` or `onSucceeded` | none | never |
    | cancel | none (`JOB_CANCELLED`) | — |

    Backoff: 5 s × 2ⁿ⁻¹ for the n-th retry, at most 5 minutes (`nextRunAt`, persisted), up to `maxAttempts` (default 3). **Interactive jobs retry once, at once, on a fresh worker** (their `maxAttempts` is 2). The last failure's code is the job's error.
32. **Isolation of a crashing batch** (pure `isolation.ts`, state persisted in `jobs.isolation`): a batch kind declares `batch {items(params), restrict(params, items), combine?}`. Items reported with `item` are never re-run. After a crash: if an item was **in flight** (`begin` without `item`) and this run was not already that item alone, the next run is that item **alone**, then the rest; if it was already alone, it fails as **`POISON_ITEM`** (an item error) and the rest go on without it; if **no item was in flight**, the remainder is split in halves, each run in turn (a remainder of one is treated as "alone"). Isolation runs count against a separate budget of `2 + 2·⌈log₂ n⌉` crashing runs (n = the job's items), past which the job fails `WORKER_CRASHED`; non-crash failures during isolation follow the table and count attempts. The final result is `combine(results, items)` (default `{items: {<item>: value}, itemErrors}`).

### Warm workers and scheduling

33. **Pools** are named slot counters. Defaults: `cpu` = max(1, cores − 1), `gpu` 1, `hwenc` 2 (sub-project 6 resizes it from the capability report), `net` 4; `io:<key>` pools are defined by their owner (5D) with `definePool`. A kind's `resources` lists pool names (one slot each, or a function of params). A job that needs an undefined pool fails `WORKER_UNAVAILABLE {pool}`.
34. **Order** (spec §6.3): effective priority = base level minus one level per 10 minutes waited (never above `interactive`), then `createdAt`, then id. `schedule(state, now) → Dispatch[]` is pure; the queue is indexed by (resource set, priority) as FIFOs, so one blocked head never hides a runnable job behind it; property-tested against a brute-force reference.
35. **Warm pools** (spec §6.3). `interactive:<family>` holds 1 worker by default (at most 2; the family's `warm.slots`). Only kinds marked `warm` submitted at `interactive` priority use it; a `warm` kind submitted at another priority runs cold. A warm job takes a `cpu` slot while it runs but **never waits for one** (`cpu` may run over by at most the interactive slots), so a long cold job never delays it. Dispatch to a `ready` worker is one stdin write in the submit's own turn (budget ≤ 5 ms from submit to `start`, p99). With no ready worker and room in the pool, one is spawned and the job waits for its `hello`; otherwise the job waits in the pool's FIFO.
36. **Warm lifecycle:** a worker idle for **60 s** (the family's `warm.idleS`) gets `shutdown` (`idleExpired`); after **500 jobs or 30 minutes** it is replaced at its next `ready` (`recycled`); a crash or kill during a job loses it, the job is retried once at once on a fresh worker, and the pool spawns a replacement at the next interactive submit. `warm(family)` starts one now if the pool has none (sub-project 6 calls it when a project opens). Times come from Effect's `Clock`.
37. **Pause** (spec §6.3): `{job}` on a queued job → `paused` (it keeps its place); on a running resumable job → `cancel {reason: pause}` with a 10 s grace for the checkpoint, then `paused` (killed after the grace; the last checkpoint stands); a running non-resumable job finishes and is not paused. `{all}`: dispatch stops, running resumable jobs pause as above, others finish, warm workers are kept until they expire. `resume` puts a paused job back as `queued` with its original `createdAt`.

### Files, helpers and hooks

38. **Job directories** (spec §6.4): `<server-data>/jobs/` is made with `ensurePrivateDir`; each job gets `<jobId>/` (0700, the parent's DACL on Windows) with `tmp/`, and each helper `helpers/<id>/tmp/`. A kind's `MappedFile {name, bytes?, mode}`: `name` is relative, segments `[A-Za-z0-9._-]+`, no `..`, at most 4 deep; a name ending in `/` is a directory (created 0700); a file **with** `bytes` is created 0600 at that size (zero-filled, `ftruncate` / `SetEndOfFile`) and recreated before each attempt when its mode is `write` or `readWrite`; a file **without** `bytes` is written by the kind's `prepare` hook, and 5C checks it exists as a regular file with one link (no symlink, no reparse point) before `start`. Caps: 1 GiB per file, 4 GiB per job (`JOB_PARAMS_INVALID` above them); a creation error fails the attempt with the storage code (`IO_ERROR`), retried with backoff. 5C never reads the files. The directory is deleted when the job ends (after `onSucceeded`), with Windows sharing-violation retries (20 × 25 ms); leftovers are swept at start and hourly.
39. **Helpers** (spec §6.4): `HelperSpec {kind: codec|plugin, id, profile: worker|plugin, files}` from the kind (a list or a function of params); each `(kind, id)` has a registered `HelperProgram` that resolves its program. Order: create the directory and files → `prepare` → spawn each helper in the job's group → await its `hello` (10 s) → `attach` → spawn or reuse the worker → `start`. A helper that exits or sends `error` before the worker's `result` fails the job with **`HELPER_FAILED {helper, code}`** and kills the job's groups; after the `result`, helpers get `shutdown`, 2 s, then their group is killed.
40. **Kind hooks** (sub-project 6 §3.2 item 3; 6A plan Task 12):
    - **`prepare(ctx)`** runs once per job directory (the first attempt, and the first attempt after a restart), after 5C created the directory and files, before any process starts; it writes the files without `bytes` (`in/snapshot.json`, `in/manifest.json`) and may return `{extra}`, passed to the worker as `start.job.extra` and kept in `jobs.extra`.
    - **`onSucceeded(ctx)`** runs after the worker's `result`, **after every process that could still write the directory has exited** (a cold job's whole group; for a warm job, the job's helper group, while the warm worker, which sent `ready`, stays alive, so the hook copies before it validates, as `fin-shm` requires), and **before 5C deletes the directory**. It may return a replacement result. Its budget is the kind's `hookTimeoutS` (60 s; past it `JOB_TIMEOUT`); a failure fails the job with the hook's error, never retried. A cancel during the hook is too late: the job ends `succeeded`.
    - Both hooks are `Effect`s; long work in them (copies, hashing, renames of large outputs) runs off the JS thread through fin-node, never as a blocking call on the event loop.
41. **Codes** (spec §6.9; exit classes in `crates/fin/src/output.rs`): 3 `JOB_KIND_UNKNOWN`, `JOB_PARAMS_INVALID`, `JOB_NOT_FOUND`; 4 `JOB_FAILED`, `JOB_CANCELLED`, `JOB_INTERRUPTED`, and the result codes `WORKER_CRASHED`, `WORKER_HUNG`, `JOB_STALLED`, `JOB_TIMEOUT`, `POISON_ITEM`, `HELPER_FAILED`; 5 `JOB_QUEUE_FULL`, `WORKER_UNAVAILABLE`, `WORKER_PROTOCOL_MISMATCH`, `WAIT_TIMEOUT`, `RESOURCE_STUCK`, **`WORKER_OOM`** (5C-1 enforces the limit, so it owns the code; 6A's plan maps it to 5 too). `JOBHOST_UNSUPPORTED` (5) exists only until Task 5.

### Events and plugins

42. **The sink** (spec §6.8): `JobEventSink {jobDone, workerExit, forward}`. Until diagnostics D4 wires the journal into `finisherd`, the engine is given `memorySink()` (tests read it; `finisherd` keeps the last 1,000 and logs them at debug); D4 replaces it with a journal sink (a TASKS follow-up). **`job.done`** (`kv` 1) for every job that reaches a terminal state: `kind`, `family`, `outcome: succeeded|failed|cancelled|interrupted`, `code?`, `attempts`, `priority`, `warm`, `queuedMs`, `runMicros`, `items?`, `itemsFailed?`, `isolated`, `helpers`, plus the kind's `journal(job)` extra fields (sub-project 6's `kv` 2). **`worker.exit`** for every process 5C started, the watchdog excepted: `role`, `family`, `helperKind?`, `pluginId?` (the sink hashes it into `plugin: Hashed`), `reason: done|idleExpired|recycled|cancelled|killed|crashed|hung|stalled|abandoned|spawnFailed|oom|protocolError` (**`oom` and `protocolError` added** here; the kind is new, so no bump), `exitCode?`, `signal?`, `jobsServed`, `lifetimeS`, `sandboxed`. Names: `JOB_KINDS` (`test.*` now; 5D, 6 and 8C add theirs), `WORKER_FAMILIES` (`test`, `library`, `media`, `analyze`, `plugin`), `SIGNALS` (lower case without `SIG`: `kill`, `term`, `int`, `hup`, `segv`, `bus`, `abrt`, `ill`, `fpe`, `pipe`, `xcpu`, `sys`, `trap`), in `fin-core/src/events/names_jobs.rs`; a drift test keeps the registered kinds and families inside them.
43. **The in-process bus:** `onDone(listener)` receives the `JobView` after the terminal row is written (sub-project 7's hooks attach here; 8's budgets read `owner` and `runMicros`); `events()` is a `Stream` of `{type: state|progress|item, job}` that 5C-2's `jobs.watch` will serve. A throwing listener is logged and never affects the job.
44. **Plugins** (sub-project 8 §3.7): a job records `owner` (the invoker; `system` for in-process callers), `via {plugin}` and `rights` (the invoker ∩ grants snapshot, opaque here) and passes them to hooks and the bus; 5C-1 checks none of them (5C-2 checks ownership, 8B checks rights at each host call). A family or helper of a plugin resolves to `Program::Path` with profile `plugin`, and names its plugin id for `worker.exit`. The process must speak this envelope on its stdio (8B's SDK implements it for job processes).

## Frozen interfaces

Once Tasks 1, 6, 7 and 9–11 merge, these are frozen for 6A-2, 5D, 5C-2 and 8B/8C; a change needs a coordinator decision and a note in each consumer's plan.

```rust
// crates/fin-jobs (Task 1)
pub const PROTOCOL: u32 = 1;
pub const EXIT_HOST_GONE: i32 = 70;
pub fn envelope_id() -> &'static str;                               // Decision 3
pub enum HostMsg { Start(Start), Cancel { job: String, reason: CancelReason }, Shutdown {}, Attach(Attach) }
pub enum WorkerMsg { Hello(Hello), Begin { job: String, item: String },
    Item { job: String, item: String, value: Option<Value>, error: Option<JobError> },
    Progress(Progress), Checkpoint { job: String, data: Value }, Alive {}, Result { job: String, value: Value },
    Error { job: String, error: JobError, item_errors: Option<Vec<ItemError>> }, Ready {},
    Diag { job: Option<String>, kind: String, payload: Value } }
pub struct Start { pub job: StartJob, pub limits: Limits, pub files: Vec<FileGrant>, pub helpers: Vec<HelperGrant>, pub dirs: Dirs, pub diagnostics: bool }
pub struct StartJob { pub id: String, pub kind: String, pub kind_version: u32, pub params: Value, pub checkpoint: Option<Value>, pub attempt: u32, pub extra: Option<Value> }
pub struct FileGrant { pub name: String, pub path: PathBuf, pub mode: FileMode, pub bytes: Option<u64> }   // FileMode: Read | Write | ReadWrite
pub struct JobError { pub code: String, pub message: String, pub details: Value, pub retry: Retry }      // Retry: Never | Backoff | Fresh
pub struct KindParams { pub kind: &'static str, pub version: u32,
    pub validate: fn(&Value) -> Result<Value, Vec<String>>, pub schema: fn() -> schemars::Schema }

pub mod worker {
    pub trait Family: Send {
        fn name(&self) -> &'static str;
        fn kinds(&self) -> &'static [KindVersion];
        fn warm(&self) -> bool { false }
        fn run(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError>;
    }
    pub fn run(family: impl Family) -> !;
    impl JobCtx<'_> {
        pub fn file(&self, name: &str) -> Option<&FileGrant>;  pub fn files(&self) -> &[FileGrant];
        pub fn helpers(&self) -> &[HelperGrant];  pub fn dirs(&self) -> &Dirs;  pub fn limits(&self) -> &Limits;
        pub fn begin(&mut self, item: &str);  pub fn item(&mut self, item: &str, value: Option<Value>);
        pub fn item_failed(&mut self, item: &str, error: JobError);
        pub fn progress(&mut self, p: Progress);  pub fn checkpoint(&mut self, data: Value) -> Result<(), JobError>;
        pub fn cancelled(&self) -> Option<CancelReason>;  pub fn cancel_token(&self) -> CancelToken;   // Clone + Send
        pub fn reporter(&self) -> Reporter;                     // Clone + Send: progress/begin/item from other threads
        pub fn diagnostics(&self) -> bool;  pub fn diag(&mut self, kind: &'static str, payload: Value);
        pub fn read_budget(&self) -> Option<ReadBudget>;        // token bucket over limits.readBytesPerSec
    }
}
pub mod helper {
    pub trait Helper: Send { fn kind(&self) -> HelperKind; fn id(&self) -> &str;
        fn run(&mut self, attach: &Attach, ctx: &mut HelperCtx<'_>) -> Result<(), JobError>; }
    pub fn run(helper: impl Helper) -> !;
}
```

```ts
// @finisher/contracts jobs.ts (Task 7): Effect Schemas; TS types are Schema.Type of them
Priority = "interactive" | "normal" | "background"
JobState = "queued" | "running" | "paused" | "succeeded" | "failed" | "cancelled" | "interrupted"
JobOwner = { kind: string /* PrincipalKind | "system"; narrowed in 5C-2 */; id: string }
JobVia = { plugin: string }      JobRights = unknown /* opaque until 8B */
Progress = { done: number; total?: number; unit: "bytes" | "items" | "frames" | "seconds"; phase?: string; at: number }
MappedFile = { name: string; bytes?: number; mode: "read" | "write" | "readWrite" }
HelperSpec = { kind: "codec" | "plugin"; id: string; profile: "worker" | "plugin"; files: string[] }
JobView = { id; kind; kindVersion; family; params: unknown; owner: JobOwner; via?; project?; library?; parent?;
  priority; state; attempts; maxAttempts; idemKey?; dedupeKey?; progress?; result?: unknown; error?: FinError;
  itemErrors?: { item: string; error: FinError }[]; warm: boolean; createdAt; startedAt?; finishedAt?; nextRunAt? }

// apps/finisherd/src/jobs/kinds.ts (Task 7)
export type PoolName = "cpu" | "gpu" | "hwenc" | "net" | `io:${string}`;
export interface MemoryRule { readonly fixedMiB?: number; readonly ramFraction?: number; readonly capMiB?: number }
type OrFn<P, T> = T | ((params: P) => T);
export interface KindSpec<P = unknown, R = unknown> {
  readonly kind: string; readonly version: number; readonly family: string;
  readonly scopes: ReadonlyArray<string>;                       // checked by 5C-2
  readonly resources: OrFn<P, ReadonlyArray<PoolName>>;
  readonly warm?: boolean; readonly defaultPriority: Priority; readonly qos: "normal" | "utility" | "background";
  readonly timeoutS: number; readonly stallS?: number; readonly maxAttempts?: number; readonly resumable?: boolean;
  readonly batch?: BatchSpec<P>; readonly dedupe?: (params: P) => string | undefined; readonly rememberPoison?: boolean;
  readonly files?: OrFn<P, ReadonlyArray<MappedFile>>; readonly helpers?: OrFn<P, ReadonlyArray<HelperSpec>>;
  readonly memory?: OrFn<P, MemoryRule>;
  readonly prepare?: (ctx: PrepareCtx<P>) => Effect.Effect<{ readonly extra?: unknown } | void, FinError>;
  readonly onSucceeded?: (ctx: SucceededCtx<P, R>) => Effect.Effect<R | void, FinError>;
  readonly hookTimeoutS?: number;
  readonly journal?: (job: JobView) => Readonly<Record<string, unknown>>;   // extra job.done fields
}
export interface BatchSpec<P> { items(params: P): ReadonlyArray<string>; restrict(params: P, items: ReadonlyArray<string>): P;
  combine?(results: ReadonlyArray<unknown>, items: ReadonlyMap<string, ItemOutcome>): unknown }
export interface PrepareCtx<P> { readonly job: JobView & { params: P }; readonly dir: string; path(name: string): string; readonly attempt: number }
export interface SucceededCtx<P, R> extends PrepareCtx<P> { readonly result: R; readonly items: ReadonlyMap<string, ItemOutcome> }
export type ItemOutcome = { readonly value?: unknown } | { readonly error: FinError };
export type ProgramSpec = { readonly binary: "fin-workers" | "fin-analyze"; readonly args: ReadonlyArray<string> }
                        | { readonly path: string; readonly args: ReadonlyArray<string> };   // path: plugins only
export interface FamilySpec {
  readonly family: string; readonly profile: "worker" | "plugin";
  readonly program: ProgramSpec | ((job: JobView) => ProgramSpec);
  readonly memory?: { readonly interactive?: MemoryRule; readonly batch?: MemoryRule };
  readonly warm?: { readonly slots?: number; readonly idleS?: number; readonly maxJobs?: number; readonly maxAgeS?: number };
  readonly diagKinds?: ReadonlyArray<string>; readonly pluginId?: string;
}
export interface HelperProgram { readonly kind: "codec" | "plugin"; readonly id: string; readonly profile: "worker" | "plugin";
  resolve(job: JobView): ProgramSpec; readonly pluginId?: string }
export interface KindRegistry { registerFamily(f: FamilySpec): void; registerKind<P, R>(k: KindSpec<P, R>): void;
  registerHelper(h: HelperProgram): void; kind(name: string): KindSpec | undefined; family(name: string): FamilySpec | undefined }

// apps/finisherd/src/jobs/engine.ts (Tasks 9–11)
export interface SubmitSpec { readonly kind: string; readonly params: unknown; readonly priority?: Priority;
  readonly idemKey?: string; readonly project?: string; readonly library?: string; readonly parent?: string;
  readonly via?: JobVia; readonly rights?: unknown; readonly maxAttempts?: number }
export interface JobEngine {
  readonly kinds: KindRegistry;
  submit(owner: JobOwner, spec: SubmitSpec): Effect.Effect<JobView, FinError>;
  get(id: string): Effect.Effect<JobView, FinError>;                                  // JOB_NOT_FOUND
  list(q: { owner?: JobOwner; state?: JobState[]; kind?: string; project?: string; library?: string;
    cursor?: string; limit?: number }): { readonly jobs: JobView[]; readonly next?: string };   // limit ≤ 500
  wait(ids: ReadonlyArray<string>, opts: { readonly any?: boolean; readonly timeoutMs: number }): Effect.Effect<ReadonlyArray<JobView>, FinError>; // WAIT_TIMEOUT
  cancel(id: string): Effect.Effect<JobView, FinError>;
  pause(target: { readonly job: string } | { readonly all: true }): Effect.Effect<void, FinError>;
  resume(target: { readonly job: string } | { readonly all: true }): Effect.Effect<void, FinError>;
  warm(family: string): Effect.Effect<void, FinError>;
  definePool(name: PoolName, size: number): void;                                       // also resizes
  workers(): WorkersView;   // pools, warm workers, live processes, stuck pools, host caps (killTree, watchdog, sandboxed)
  events(): Stream.Stream<JobEvent>;
  onDone(listener: (job: JobView) => void): () => void;
  close(): Effect.Effect<void>;
}
export class JobEngineTag extends Context.Tag("JobEngine")<JobEngineTag, JobEngine>() {}
export const layerJobEngine: (serverData: string, opts?: JobEngineOptions) => Layer.Layer<JobEngineTag, ServerStartError, ControlStoreTag | JobEventSinkTag>;

// apps/finisherd/src/jobs/events.ts (Task 7)
export interface JobEventSink {
  jobDone(e: JobDone & { readonly extra?: Readonly<Record<string, unknown>> }): void;   // JobDone generated from fin-core
  workerExit(e: Omit<WorkerExit, "plugin"> & { readonly pluginId?: string }): void;      // WorkerExit generated from fin-core
  forward(origin: { readonly family: string; readonly job?: string }, kind: string, payload: unknown): void;
}
export class JobEventSinkTag extends Context.Tag("JobEventSink")<JobEventSinkTag, JobEventSink>() {}
export function memorySink(cap?: number): JobEventSink & { readonly done: ReadonlyArray<…>; readonly exits: ReadonlyArray<…>; readonly forwarded: ReadonlyArray<…> };
```

The napi `JobHost` (Task 6) and `fin-jobhost`'s `Host` (Task 3) are internal to 5C; only `Spawner`/`PlatformCommand`/`SpawnRequest` (Task 3) are frozen, for sub-project 7.

## Questions for the owner

None. The plan follows spec revision 3; the details it adds are engineering choices recorded as Decisions and, in Task 13, as spec deviations: the Unix watchdog (Decision 15), cgroups only for a cgroup `finisherd` owns alone (Decision 16), `fin-jobhost` as its own crate (Decision 1), `prepare` beside `onSucceeded` (Decision 40; 6A-2 needs it), `rememberPoison` (Decision 27; sub-project 6 §3.3 needs it), `WORKER_OOM` as 5C's code, and the `oom`/`protocolError` exit reasons.

## Review Focus

1. **Nothing runs outside its kill tree, and the tree dies with the host on every OS:** suspended → assigned → resumed on Windows and macOS; `pre_exec` before exec on Linux; the host Job's handle, PDEATHSIG and the watchdog; no stray handle or fd inherited; spawns only from the long-lived thread.
   → Task 3 `pdeathsig_survives_spawning_thread_exit`, `only_stdio_is_inherited`; Task 4 `grandchild_gone_after_host_sigkill`, `hung_worker_gone_after_host_sigkill`; Task 5 `process_is_in_its_job_before_it_runs`, `grandchild_gone_after_host_terminate`; Task 12 `finisherd_kill_leaves_no_process`.
2. **Cancel kills the whole tree,** with the right grace, and a stuck process is abandoned rather than awaited forever. → Task 3 `cancel_kills_double_forked_grandchild`; Task 4 `abandon_after_kill_wait`; Task 5 `cancel_terminates_job_tree`; Task 9 `two_abandoned_mark_io_pool_stuck`.
3. **Liveness, stall and timeout are separate,** and resource failures are never bisected. → Task 3 timer tests; Task 10 `hang_is_never_bisected`, `helper_failure_is_never_bisected`, `stall_is_job_stalled`.
4. **Isolation targets the in-flight item; done items never re-run; the bound holds.** → Task 10 `poison_item_isolated_in_two_runs`, `crash_between_items_bisects`, `isolation_properties`.
5. **Restart and close:** no `running` row survives; resumable jobs resume from their checkpoint; job directories are gone. → Task 9 `restart_requeues_resumable_from_checkpoint`, `close_leaves_no_running_rows`, `job_dirs_swept_at_start`.
6. **Warm workers:** separate slots, ≤ 5 ms dispatch, expiry and recycling, crash retry on a fresh worker, no state between jobs. → Task 11.
7. **Hooks run at the right moment:** `prepare` before any process, `onSucceeded` after every writer of a cold job has exited and before deletion. → Task 9 `on_succeeded_runs_after_group_exit_before_delete`.
8. **The envelope is strict** (versions, `seq`, sizes, job ids) and bulk data never travels in it. → Task 1, Task 3 `protocol_errors_kill_the_process`.
9. **Workers get no credential, no inherited environment and no other process's handles; shipped families start no process.** → Task 3 `env_is_the_allow_list`; Task 12 `check-worker-spawn` gate; Decision 22.
10. **The event loop:** statements p99 ≤ 1 ms with 10k jobs; batched deletes; no per-item write storms. → Task 12 budget.
11. **Files ≤ 600 lines; no GPL dependency; every commit green on three OSes.** → `pnpm gates` at every commit; Task 14.

---

### Task 1: `fin-jobs`: the envelope, the worker and helper runtimes — tier: sonnet — batch A

**Files:**
- Create: `crates/fin-jobs/Cargo.toml` (feature `test-hooks`: the test kinds' params), `crates/fin-jobs/src/{lib.rs, msg.rs, error.rs, codec.rs, budget.rs, kinds.rs, test_kinds.rs}`, `crates/fin-jobs/src/worker/{mod.rs, ctx.rs, io.rs}`, `crates/fin-jobs/src/helper.rs`.
- Modify: `Cargo.toml` (workspace member; default member), `crates/fin-store/examples/export_schema.rs` and its `Cargo.toml` dev-dependency (export the envelope and the test kinds' params and results), `packages/core-types` (regenerated), `TASKS.md` (new 5C-1 section).
- Test: `crates/fin-jobs/tests/{envelope.rs, worker_runtime.rs}`.

**Interfaces:** the `fin-jobs` block under **Frozen interfaces**, plus:
```rust
pub mod codec {
    pub const MAX_LINE: usize = 1 << 20;  pub const MAX_CHECKPOINT: usize = 256 << 10;
    pub fn encode<M: Serialize>(seq: u64, msg: &M) -> Result<Vec<u8>, CodecError>;          // adds v and seq, appends \n
    pub fn decode_worker(line: &[u8], expect_seq: u64) -> Result<WorkerMsg, CodecError>;     // TooLong | Json | UnknownType | Seq{expected, got}
    pub fn decode_host(line: &[u8], expect_seq: u64) -> Result<HostMsg, CodecError>;
}
pub struct ReadBudget { /* token bucket, refills per ms */ }
impl ReadBudget { pub fn new(bytes_per_sec: u64) -> Self; pub fn take(&self, n: u64); /* blocks while empty */ }
#[cfg(feature = "test-hooks")] pub mod test_kinds { pub static PARAMS: &[KindParams]; /* SleepParams, EchoParams, CrashParams, … */ }
```
Decisions 3–8.

- [ ] **Step 1: Write failing tests:**
  - `envelope_round_trips_every_message` (serde both ways; field names camelCase as in spec §6.2);
  - `envelope_id_is_stable_and_changes_with_the_schema` (a golden of the current id; a test-only extra field changes it);
  - `decode_refuses_long_lines_unknown_types_and_seq_gaps`; `checkpoint_over_cap_is_refused`;
  - `generated_ts_matches` (the drift gate covers it; the test asserts the schema names exist);
  - runtime, driven over in-memory pipes with a toy family: `hello_first_then_result_then_exit_cold`, `warm_sends_ready_and_takes_the_next_start`, `alive_every_interval_from_its_own_thread` (while `run` blocks), `stdin_eof_exits_70` (in a child process: the test re-executes itself), `cancel_sets_the_token`, `pause_reason_reaches_the_family`, `panic_is_answered_as_worker_crashed_and_exits_101`, `diag_is_a_no_op_when_off`, `reporter_works_from_another_thread`, `helper_waits_for_attach_and_exits_on_shutdown`;
  - `read_budget_limits_rate` (10 MiB at 5 MiB/s takes ≈ 2 s ± 10 %, with an injected clock).
- [ ] **Step 2: Implement.** `msg.rs` ≤ 400 lines (split `msg_host.rs`/`msg_worker.rs` if it grows); the stdout writer is one `Mutex<BufWriter<Stdout>>` with the `seq` counter, flushed per message; the stdin reader thread decodes `HostMsg` and hands them to the run loop over a channel, exiting the process on EOF.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(fin-jobs): job envelope and worker and helper runtimes`.

### Task 2: `fin-workers` with the `test` family and test helpers — tier: sonnet — after Task 1

**Files:**
- Create: `crates/fin-workers/Cargo.toml` (bin `fin-workers`; feature `test-hooks` enables `fin-jobs/test-hooks` and the `test` family; a self dev-dependency enables it for its own tests), `crates/fin-workers/src/{main.rs, families.rs}`, `crates/fin-workers/src/test_family/{mod.rs, kinds.rs, batch.rs, helper.rs}` (`#[cfg(feature = "test-hooks")]`).
- Modify: `Cargo.toml` (member and default member: like `fin`, only its self dev-dependency turns `test-hooks` on, so `scripts/check-test-hooks.mjs` keeps it out of the normal graph), `packages/fin-node/package.json` (`build` and `build:test` also build `fin-workers` and copy it beside the `.node` file: `scripts/copy-workers.mjs`), `.gitignore` (the copied binaries).
- Test: `crates/fin-workers/tests/family_test.rs`.

**Interfaces:**
```text
fin-workers <family>                  run a family's worker (stdin/stdout envelope)
fin-workers --helper <kind>:<id>      run a helper (test builds: test:ok|exitEarly|hang|spawn)
fin-workers watchdog                  Task 4 (Unix)
fin-workers --version                 prints the envelope id
```
Test kinds (params types in `fin_jobs::test_kinds`, Rust → generated TS):
- `test.sleep {ms, progressEveryMs?}`; `test.echo {value}`; `test.warm {value, delayMs?}` (warm; result `{value, pid, served}` so tests see reuse);
- `test.crash {items: string[], poison?: string[], how: abort|segv|exit, between?: bool}` (batch: `begin`/`item` per item; crashes inside a poison item, or between items when `between`);
- `test.hang {}` (stops `alive` and blocks); `test.stall {}` (keeps `alive`, never reports progress);
- `test.spawn {detach: none|setsid, out}` (**breaks the no-process rule on purpose**: double-forks a sleeping grandchild, writes both pids to the mapped file `out`, then sleeps);
- `test.files {}` (writes a pattern into every `write` file, reads every `read` file, returns their sizes and hashes);
- `test.checkpoint {steps, crashAt?}` (resumable: a checkpoint per step; crashes once at `crashAt`);
- `test.oom {mib}` (allocates and touches `mib` MiB);
- `test.diag {}` (sends one `diag` of a test kind);
- `test.garbage {what: json|seq|long|otherJob}` (protocol violations);
- `test.badHello {}` registered under the separate family `testbad`, whose `hello` claims another envelope id.

- [ ] **Step 1: Write failing tests** (`family_test.rs`, through the binary with a minimal in-test host over pipes, Unix and Windows): every kind's happy path; `hello_lists_the_test_kinds`; `crash_kinds_die_as_asked` (exit status / signal); `garbage_kinds_emit_what_they_say`; `helper_ok_attaches_and_exits_on_shutdown`; `helper_exit_early_exits_nonzero`; `version_prints_the_envelope_id`; `unknown_family_exits_2`. The release build's lack of the `test` family is held by `scripts/check-test-hooks.mjs` (no `test-hooks` in the normal graph) and checked by hand in Task 14.
- [ ] **Step 2: Implement.** The family table in `families.rs` is `#[cfg]`-gated; `main.rs` only parses arguments and dispatches.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(fin-workers): the worker binary with the test family`.

### Task 3: `fin-jobhost`: groups, Unix spawning, framing, timers and cancel — tier: opus — after Task 2

**Files:**
- Create: `crates/fin-jobhost/Cargo.toml`, `crates/fin-jobhost/src/{lib.rs, host.rs, spawner.rs, proc.rs, framing.rs, timers.rs, group.rs, exit.rs, memory.rs}`, `crates/fin-jobhost/src/unix/{mod.rs, linux_spawn.rs, macos_spawn.rs, pgroup.rs}`, `crates/fin-jobhost/src/windows/mod.rs` (a stub answering `JOBHOST_UNSUPPORTED` until Task 5).
- Modify: `Cargo.toml` (member, default member), `crates/fin-workers/Cargo.toml` (dev-dependency on `fin-jobhost`).
- Test: `crates/fin-workers/tests/{host_support.rs, host_unix.rs, host_common.rs}`.

**Interfaces:**
```rust
pub struct Timing { pub hello_ms: u32, pub hung_ms: u32, pub kill_wait_ms: u32, pub rss_sample_ms: u32 }   // 10 000, 30 000, 5 000, 1 000
pub struct HostOptions { pub binary_dir: PathBuf, pub timing: Timing, pub spawner: Arc<dyn Spawner>, pub cgroups: CgroupMode, pub watchdog: bool }
pub struct HostCaps { pub kill_tree: KillTree /* JobObject | Cgroup | ProcessGroup */, pub watchdog: bool, pub cgroup_memory: bool,
                      pub sandbox: &'static str, pub sandboxed: bool }
pub enum HostEvent { Hello { proc: ProcId, hello: Hello }, Message { proc: ProcId, msg: WorkerMsg },   // `alive` is consumed by the host
                     Stderr { proc: ProcId, line: String }, Exit { proc: ProcId, info: ExitInfo }, GroupEmpty { group: GroupId } }
pub struct ExitInfo { pub reason: ExitReason, pub code: Option<i64>, pub signal: Option<&'static str>, pub foreign: Option<u32>,
                      pub rss_peak: Option<u64>, pub stderr_tail: String, pub lifetime_ms: u64 }
pub enum ExitReason { Exited, Killed(KillReason), Hung, Stalled, TimedOut, Oom, ProtocolError(String),
                      ProtocolMismatch { expected: String, got: String }, NoHello, SpawnFailed(String), Abandoned }
pub enum KillReason { Cancel, Shutdown, Recycle, IdleExpired, HelperFailed, JobEnded, Test }
pub struct GroupSpec { pub label: String, pub memory_bytes: Option<u64>, pub qos: Qos }
pub struct Watch { pub job: String, pub stall_ms: Option<u32>, pub timeout_ms: Option<u32> }

impl Host {
    pub fn start(opts: HostOptions, on_event: Box<dyn Fn(HostEvent) + Send + Sync>) -> Result<Host, HostError>;
    pub fn caps(&self) -> HostCaps;
    pub fn open_group(&self, spec: GroupSpec) -> Result<GroupId, HostError>;
    pub fn spawn(&self, group: GroupId, req: SpawnRequest) -> Result<ProcId, HostError>;   // on the spawner thread; returns after the process runs
    pub fn send(&self, proc: ProcId, msg: &HostMsg) -> Result<(), HostError>;
    pub fn watch(&self, proc: ProcId, w: Option<Watch>);                                   // arm/clear per-job stall and timeout timers
    pub fn close_stdin(&self, proc: ProcId);
    pub fn kill_group(&self, group: GroupId, reason: KillReason);
    pub fn close_group(&self, group: GroupId);                                             // once empty
    pub fn shutdown(&self, grace_ms: u32);
}

// spawner.rs — frozen for sub-project 7
pub enum Program { Bundled(String), Path(PathBuf) }
pub enum Profile { Worker, Plugin }
pub enum Role { Worker, Helper { kind: HelperKind, id: String } }
pub struct PathGrant { pub path: PathBuf, pub mode: FileMode }
pub struct SpawnRequest { pub role: Role, pub profile: Profile, pub family: String, pub program: Program, pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>, pub cwd: PathBuf, pub grants: Vec<PathGrant>, pub qos: Qos, pub label: String }
pub struct PlatformCommand { pub program: PathBuf, pub args: Vec<OsString>, pub env: Vec<(OsString, OsString)>, pub cwd: PathBuf,
    #[cfg(target_os = "linux")] pub pre_exec: Vec<Box<dyn Fn() -> std::io::Result<()> + Send + Sync>>,
    #[cfg(windows)] pub attributes: Vec<WinAttribute>, #[cfg(windows)] pub token: Option<OwnedHandle> }
pub trait Spawner: Send + Sync {
    fn name(&self) -> &'static str;
    fn sandboxed(&self) -> bool;
    fn prepare(&self, req: &SpawnRequest, cmd: &mut PlatformCommand) -> Result<(), SpawnError>;
}
pub struct Unsandboxed;   // name "none", sandboxed false, prepare does nothing
```
Decisions 4, 9–13, 17–19, 21, 23. Files: `host.rs` ≤ 450 lines (group and process tables, event dispatch), `proc.rs` the per-process threads, `timers.rs` one timer thread with a binary heap.

- [ ] **Step 1: Write failing tests** (Unix now; Task 5 makes `host_common.rs` run on Windows):
  - `host_common.rs`: `spawn_hello_start_result_exit`, `bundled_program_resolves_only_known_names`, `env_is_the_allow_list` (a `test.echo` of its own environment: only `childEnv` keys and `TMPDIR`), `protocol_errors_kill_the_process` (each `test.garbage` variant → `ProtocolError`), `protocol_mismatch_on_bad_hello`, `no_hello_times_out`, `hung_is_detected` (`test.hang`, `hung_ms` 300), `stall_is_detected` (`test.stall`, `stall_ms` 300), `timeout_is_detected`, `cancel_grace_then_kill` (a family that ignores `cancel` dies after the grace; one that answers keeps its exit `Exited`), `stderr_tail_is_kept_and_capped`, `exit_reports_code_and_signal`, `spawner_prepare_can_rewrite_args` (a test `Spawner` adds an argument the test family echoes), `unsandboxed_caps`;
  - `host_unix.rs`: `processes_share_the_group_pgid`, `cancel_kills_double_forked_grandchild` (`test.spawn` with `detach: none`: both pids gone after `kill_group`), `pdeathsig_set_on_linux` (read `/proc/<pid>/status`'s parent-death signal through a test helper that reports `prctl(PR_GET_PDEATHSIG)`), `pdeathsig_survives_spawning_thread_exit` (spawn from a thread that then exits: the worker lives), `only_stdio_is_inherited` (the worker lists `/proc/self/fd` or `/dev/fd`: 0, 1, 2 only, while the host holds 50 extra open fds), `qos_nice_applied` (Linux nice 5 / 10; macOS QoS class read back with `proc_pidinfo`), `macos_spawn_is_suspended_until_registered` (a test hook delays the resume: the process has not sent `hello`).
- [ ] **Step 2: Implement** per Decisions 11–13. Exits are reaped by a waiter thread per process with `waitpid(pid)` (never `-1`), so `finisherd`'s own `child_process` children are untouched.
- [ ] **Step 3: Run** the gates (Linux and macOS; Windows compiles the stub) → PASS. **Commit** `feat(fin-jobhost): spawning in process groups, envelope framing, liveness and cancel`.

### Task 4: Unix host death, cgroups, memory limits, foreign processes, abandonment — tier: opus — after Task 3, beside Task 5

**Files:**
- Create: `crates/fin-workers/src/watchdog.rs` (Unix), `crates/fin-jobhost/src/unix/{watchdog.rs (the client), rss.rs, foreign.rs}`, `crates/fin-jobhost/src/linux/cgroup.rs`.
- Modify: `crates/fin-jobhost/src/{host.rs, group.rs, memory.rs}`, `crates/fin-jobhost/src/unix/{linux_spawn.rs, pgroup.rs}`, `crates/fin-workers/src/main.rs`.
- Test: `crates/fin-workers/tests/{host_death_unix.rs, host_cgroup.rs, host_memory.rs}`.

**Interfaces:**
```text
watchdog stdin:  "+g <pgid>\n" "-g <pgid>\n" "+c <cgroup dir>\n" "-c <cgroup dir>\n"
                 EOF or parent exit → SIGKILL every listed group, write "1" to every cgroup.kill, exit 0
```
```rust
pub enum CgroupMode { Auto, Off }
pub(crate) fn detect_cgroup() -> Option<CgroupRoot>;   // Decision 16's three conditions; None with a logged reason
```
Decisions 15, 16, 18, 20, 22.

- [ ] **Step 1: Write failing tests:**
  - `host_death_unix.rs` (the host is a re-executed test binary, killed with `SIGKILL`): `grandchild_gone_after_host_sigkill` (`test.spawn`, `detach: none` and `setsid`: with `setsid` the pgid kill misses it, so the test asserts it gone only in cgroup mode and otherwise records the documented gap), `hung_worker_gone_after_host_sigkill` (`test.hang` ignores stdin: the watchdog kills it), `worker_exits_on_stdin_eof` (watchdog disabled: a `test.sleep` worker exits 70), `watchdog_ignores_sigint_and_sigterm`, `watchdog_restarts_and_resends` (kill the watchdog: the host restarts it and a later host kill still cleans up), `watchdog_is_not_reported_as_worker_exit`;
  - `host_cgroup.rs` (skipped with a notice unless Decision 16's conditions hold; the Linux VM runs it under `systemd-run --user -p Delegate=yes --wait`): `delegation_detected_only_when_alone`, `host_moves_into_host_leaf`, `group_gets_its_cgroup_before_exec` (the worker reads `/proc/self/cgroup` in its first instruction of `main`), `cgroup_kill_takes_setsid_grandchild`, `memory_max_set_and_oom_reported`, `terminal_scope_is_not_used` (a cgroup with a second process → process groups);
  - `host_memory.rs`: `rss_over_limit_is_oom` (`test.oom` 256 MiB under a 64 MiB limit, sampled: `Oom` within 2 s), `under_limit_runs`, `memory_rule_resolution` (unit: fixed, fraction, cap);
  - `foreign_processes_are_counted` (`test.spawn` → `foreign == 1`; `test.echo` → 0), `abandon_after_kill_wait` (a test hook makes the waiter report a process still alive after the kill: `Abandoned` once, later exit logged). Marking a pool stuck is the engine's (Task 9).
- [ ] **Step 2: Implement.** The watchdog is ≤ 200 lines; `cgroup.rs` ≤ 350.
- [ ] **Step 3: Run** the gates on Linux and macOS → PASS. **Commit** `feat(fin-jobhost): host watchdog, delegated cgroups and memory limits on Unix`.

### Task 5: Windows: the host Job Object, suspended spawns, nested jobs and the completion port — tier: opus — after Task 3, beside Task 4

**Files:**
- Create: `crates/fin-jobhost/src/windows/{job.rs, spawn.rs, iocp.rs, env_block.rs}` (the peak memory comes from the group Job's `JOBOBJECT_EXTENDED_LIMIT_INFORMATION.PeakProcessMemoryUsed`; no sampling on Windows).
- Modify: `crates/fin-jobhost/src/windows/mod.rs` (replace the stub), `crates/fin-jobhost/Cargo.toml` (`windows-sys` features of the Tech Stack), `crates/fin-workers/tests/host_common.rs` (runs on Windows now).
- Test: `crates/fin-workers/tests/host_windows.rs` (`#[cfg(windows)]`), `crates/fin-workers/tests/host_death_windows.rs`.

**Interfaces:** `Host` as in Task 3; `KillTree::JobObject`; internal:
```rust
pub(crate) struct HostJob { job: OwnedHandle, port: OwnedHandle, thread: JoinHandle<()> }      // port thread → group events
pub(crate) struct GroupJob { job: OwnedHandle, key: usize }
pub(crate) fn spawn_suspended(cmd: &PlatformCommand, stdio: StdioHandles, flags: u32) -> io::Result<SuspendedProcess>;
impl SuspendedProcess { fn assign(&self, host: &HostJob, group: &GroupJob) -> io::Result<()>; fn resume(self) -> io::Result<RunningProcess>; }
```
Decisions 10, 14, 18, 20–23.

- [ ] **Step 1: Write failing tests** (a non-elevated Windows 11 account):
  - `process_is_in_its_job_before_it_runs` (`IsProcessInJob` true for both jobs while suspended; a test hook pauses between assign and resume);
  - `nesting_works_inside_an_outer_job` (the test host first puts itself into a Job, as a terminal or Electron may);
  - `cancel_terminates_job_tree` (`test.spawn`: worker and grandchild gone after `kill_group`);
  - `grandchild_gone_after_host_terminate` (the re-executed host killed with `TerminateProcess`: everything in its jobs gone);
  - `breakaway_is_refused` (a test child asking `CREATE_BREAKAWAY_FROM_JOB` fails to create it);
  - `unhandled_exception_dies_without_wer` (`test.crash` `how: segv` exits within 2 s, no dialog);
  - `only_stdio_is_inherited` (the handle list: a host holding 50 inheritable handles; the worker can open none by value);
  - `env_block_is_explicit` (no parent variable leaks; `Path` vs `PATH` casing kept);
  - `memory_limit_is_oom` (`test.oom` under the job's limit → `Oom` from `PROCESS_MEMORY_LIMIT`);
  - `foreign_processes_counted_from_port`;
  - `priority_class_applied` (`GetPriorityClass` = `BELOW_NORMAL` for utility and background);
  - `spawn_failure_leaves_no_suspended_process` (an assign forced to fail by a test hook: the process is terminated, `SpawnFailed`);
  - `host_common.rs` passes on Windows.
- [ ] **Step 2: Implement.** UTF-16 command line quoting by the documented `CommandLineToArgvW` rules (a unit test round-trips 50 awkward arguments); `job.rs` and `spawn.rs` ≤ 400 lines each.
- [ ] **Step 3: Run** the gates on Windows and Linux → PASS. **Commit** `feat(fin-jobhost): kill-on-close Job Objects with suspended spawns on Windows`.

### Task 6: The napi `JobHost`, params validation, events kinds, names and codes — tier: sonnet — after Task 3 (beside 4 and 5)

**Files:**
- Create: `crates/fin-node/src/jobhost/{mod.rs, napi_types.rs, params.rs}`, `crates/fin-core/src/events/payloads/jobs.rs`, `crates/fin-core/src/events/names_jobs.rs`.
- Modify: `crates/fin-node/{Cargo.toml (fin-jobhost; fin-jobs with test-hooks under fin-node's test-hooks), src/lib.rs}`, `packages/fin-node/{index.d.ts, loader.js, loader.d.ts}` (`workersBinaryPath(dir?)`), `crates/fin-core/src/events/{kinds.rs (two registry lines), names.rs (TABLES), payloads/mod.rs}`, `crates/fin/src/output.rs` and `packages/contracts` code list (Decision 41), `packages/core-types` (regenerated).
- Test: `packages/fin-node/test/jobhost.test.ts`, `crates/fin-core/tests/` (the kinds' schema and name-table tests extended).

**Interfaces:**
```ts
// packages/fin-node/index.d.ts
export interface JobHostOptions { binaryDir: string; timing?: Partial<{ helloMs: number; hungMs: number; killWaitMs: number; rssSampleMs: number }>;
  cgroups?: "auto" | "off"; watchdog?: boolean }
export type JobHostEvent =
  | { type: "hello"; proc: number; hello: Hello } | { type: "message"; proc: number; msg: WorkerMsg }
  | { type: "stderr"; proc: number; line: string } | { type: "exit"; proc: number; info: ExitInfo }
  | { type: "groupEmpty"; group: number };
export class JobHost {
  static start(opts: JobHostOptions, onEvent: (e: JobHostEvent) => void): JobHost;    // JOBHOST_UNSUPPORTED before Task 5 on win32
  caps(): { killTree: "jobObject" | "cgroup" | "processGroup"; watchdog: boolean; cgroupMemory: boolean; sandbox: string; sandboxed: boolean };
  openGroup(spec: { label: string; memoryBytes?: number; qos: "normal" | "utility" | "background" }): number;
  spawn(group: number, req: JobSpawnRequest): number;          // req.program: {bundled: name} | {path}
  send(proc: number, msg: HostMsg): void;
  watch(proc: number, w: { job: string; stallMs?: number; timeoutMs?: number } | null): void;
  closeStdin(proc: number): void;
  killGroup(group: number, reason: "cancel" | "shutdown" | "recycle" | "idleExpired" | "helperFailed" | "jobEnded" | "test"): void;
  closeGroup(group: number): void;
  shutdown(graceMs: number): Promise<void>;
  /** test-hooks builds only */ killProcForTest?(proc: number): void; abandonForTest?(proc: number): void;
}
export function validateJobParams(kind: string, version: number, params: unknown): { ok: true; params: unknown } | { ok: false; errors: string[] };
export function jobKindVersions(): Array<{ kind: string; version: number }>;     // what validateJobParams knows
```
```rust
// fin-core events (kv 1; sources [Finisherd]): JobDone, WorkerExit per Decision 42
// crates/fin-node/src/jobhost/params.rs: one static list, families append theirs:
static PARAMS: &[&[KindParams]] = &[#[cfg(feature = "test-hooks")] fin_jobs::test_kinds::PARAMS /* , fin_media::jobs::PARAMS (6A-2) */];
```
Events are delivered through one `ThreadsafeFunction` (non-blocking queue; messages converted with `serde_json::Value` → JS once, on the JS thread). Decisions 2, 26, 41, 42.

- [ ] **Step 1: Write failing tests** (`jobhost.test.ts`, test-hooks build): `starts_and_reports_caps`, `spawns_test_echo_through_napi`, `events_arrive_in_order_per_process`, `kill_group_from_js`, `validate_params_accepts_and_rejects` (errors name the field), `workers_binary_path_finds_the_test_binary`, `node_child_process_exits_are_not_stolen` (100 `child_process.spawn` children run beside 20 jobhost processes: every exit seen by its owner), `shutdown_resolves_after_all_exits`; Rust: `job_kinds_schema_exported`, `names_jobs_tables_valid`, `worker_exit_reasons_closed`.
- [ ] **Step 2: Implement.** `mod.rs` ≤ 450 lines (the napi class), `napi_types.rs` the conversions.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(fin-node,fin-core): job host binding, params validation and the job event kinds`.

### Task 7: Contracts, the `jobs` chain, the store, kinds and families, the test kinds, the event sink — tier: sonnet — after Task 1 (beside Tasks 2–6)

**Files:**
- Create: `packages/contracts/src/jobs.ts`, `apps/finisherd/src/control/migrations/jobs/0001.ts`, `apps/finisherd/src/jobs/{store.ts, kinds.ts, events.ts}`, `apps/finisherd/src/jobs/kinds/test.ts` (the `test` family and kinds; registered only when the engine is built with `testKinds: true`).
- Modify: `packages/contracts/src/index.ts`, `TASKS.md`.
- Test: `apps/finisherd/test/jobs/{store.test.ts, kinds.test.ts, events.test.ts}`.

**Interfaces:** the contracts, `kinds.ts` and `events.ts` blocks under **Frozen interfaces**, plus:
```ts
// jobs/store.ts — the only module that touches the jobs tables
export class JobStore {
  constructor(store: ControlStore, clock?: () => number);
  insert(row: NewJobRow): JobView;
  get(id: string): JobView | undefined;
  byIdem(owner: JobOwner, idemKey: string): JobView | undefined;
  activeByDedupe(kind: string, key: string): JobView | undefined;
  poisonedByDedupe(kind: string, key: string): JobView | undefined;
  transition(id: string, to: JobState, patch?: TransitionPatch): JobView;   // one UPDATE; refuses illegal transitions (a bug: throws)
  checkpoint(id: string, data: unknown): void;                              // the caller throttles (Decision 26)
  items(id: string): Map<string, ItemOutcome>;  writeItems(id: string, items: ReadonlyArray<[string, ItemOutcome]>): void;
  list(q: JobQuery): { jobs: JobView[]; next?: string };
  unfinishedCount(): number;
  running(): JobView[];  queued(): JobView[];  paused(): JobView[];
  retention(now: number): Effect.Effect<number>;                            // Decision 30
}
```
The test kinds: one `KindSpec` per Task 2 kind, with `test.crash` as a batch kind, `test.checkpoint` resumable, `test.warm` `{warm: true, defaultPriority: "interactive"}`, `test.files` declaring a `read` file without `bytes` (written by its `prepare`), a `write` file with `bytes` and an `out/` directory, and `test.helper` declaring `HelperSpec {kind: "codec", id: "test", profile: "worker"}`. Decisions 25–27, 38, 42.

- [ ] **Step 1: Write failing tests:** `jobs_chain_registers_and_migrates` (with 5A's `core`; `formatVersion.jobs` = 1; a fresh and an existing database), `transitions_are_legal_only`, `idem_per_owner`, `dedupe_active_only`, `poison_memory`, `items_batch_write_and_read`, `list_cursor_and_filters`, `retention_deletes_items_first_in_batches` (10k jobs × 4 items: ≥ 80 statements, yields between them), `registry_refuses_duplicate_kinds_and_unknown_families`, `mapped_file_names_validated` (`..`, absolute, too deep, bad characters), `test_kinds_match_rust_params` (every test `KindSpec` has a `validateJobParams` entry at its version), `memory_sink_caps_and_records`.
- [ ] **Step 2: Implement.** `store.ts` ≤ 500 lines (split `store-queries.ts` for `list` if needed).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): job store in the jobs chain, kind registry and event sink`.

### Task 8: Pools and the scheduler — tier: sonnet — after Task 7 (beside Tasks 2–6)

**Files:**
- Create: `apps/finisherd/src/jobs/{pools.ts, schedule.ts}`.
- Modify: `package.json` / `apps/finisherd/package.json` (`fast-check`, dev, exact pin), `scripts/license-allowlist.json` if the JS licence gate needs the entry.
- Test: `apps/finisherd/test/jobs/schedule.test.ts`.

**Interfaces:**
```ts
export interface Pools { size(name: string): number | undefined; used(name: string): number; define(name: string, size: number): void;
  take(names: ReadonlyArray<string>): boolean; give(names: ReadonlyArray<string>): void; stuck(name: string): boolean; snapshot(): PoolView[] }
export function defaultPools(cores: number): Pools;                       // cpu = max(1, cores − 1), gpu 1, hwenc 2, net 4
export interface QueuedJob { id: string; priority: Priority; createdAt: number; nextRunAt?: number; resources: ReadonlyArray<string>;
  warmFamily?: string }
export interface SchedState { queue: QueueIndex; pools: Pools; warm: ReadonlyMap<string, { ready: number; spawning: number; slots: number }> }
export type Dispatch = { job: string; mode: "cold" } | { job: string; mode: "warm"; family: string } | { mode: "spawnWarm"; family: string };
export function schedule(state: SchedState, now: number): Dispatch[];    // pure; the caller applies the takes
export class QueueIndex { add(j: QueuedJob): void; remove(id: string): void; size(): number }
```
Decisions 33–35.

- [ ] **Step 1: Write failing tests** (fast-check, 1,000 runs each, seeds printed on failure): `pools_never_over_committed` (cold dispatches never exceed any pool; warm jobs over-run `cpu` by at most the interactive slots), `priority_then_fifo`, `aging_raises_one_level_per_10_minutes`, `blocked_head_does_not_hide_runnable_jobs`, `warm_pool_is_separate` (a full `cpu` never blocks a warm dispatch), `next_run_at_is_respected`, `stuck_pool_never_dispatched`, `matches_brute_force_reference` (random states; the reference sorts everything), and `schedule_is_fast` (50,000 queued, 20 resource sets: one call ≤ 1 ms median on Linux; asserts ≤ 5 ms).
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): resource pools and a pure job scheduler`.

### Task 9: The engine: submit, cold runs, files, helpers, hooks, cancel, pause, persistence and restart — tier: opus — after Tasks 4, 5, 6, 7, 8

**Files:**
- Create: `apps/finisherd/src/jobs/{engine.ts, runner.ts, host.ts, files.ts, layer.ts, view.ts}`.
- Modify: `apps/finisherd/src/jobs/store.ts` (only if a query is missing), `TASKS.md`.
- Test: `apps/finisherd/test/jobs/{engine.test.ts, runner.test.ts, files.test.ts, restart.test.ts}`, `apps/finisherd/test/support/jobs.ts` (an engine over a temp server data, the test-hooks `fin-workers`, short `Timing`, `TestClock` where needed).

**Interfaces:** `JobEngine`, `JobEngineTag`, `layerJobEngine` under **Frozen interfaces** (warm parts stubbed: a `warm` kind runs cold until Task 11, `warm(family)` succeeds doing nothing), plus:
```ts
export interface JobEngineOptions { readonly binaryDir?: string; readonly timing?: JobHostOptions["timing"]; readonly testKinds?: boolean;
  readonly cores?: number; readonly totalMem?: number; readonly cgroups?: "auto" | "off"; readonly clock?: Clock.Clock }
// host.ts: wraps JobHost; routes events by proc id to their runner; converts ExitInfo to (reason, FinError)
// runner.ts: one attempt of one job: dir and files → prepare → helpers (hello, attach) → worker (hello, start) → messages
//            → result/error/exit → hooks → outcome { kind: "succeeded" | "failed" | "cancelled" | "paused" | "retry", … }
// files.ts:  jobDir, createFiles(kind, params), checkPrepared, removeJobDir (Windows retries), sweep(keep: Set<string>)
```
Decisions 10, 17–19, 23, 25–30, 33, 37–43. `engine.ts` ≤ 550 lines (submit, dispatch loop, transitions, public API); `runner.ts` ≤ 550; failure classification lives in Task 10's `isolation.ts`, which this task calls with a minimal "fail" policy until Task 10.

- [ ] **Step 1: Write failing tests:**
  - submit: `unknown_kind`, `invalid_params_name_the_field`, `idem_returns_existing`, `dedupe_returns_active`, `queue_full_at_limit` (with a test limit), `submit_dispatches_in_the_same_turn`;
  - runs: `echo_succeeds_with_result`, `progress_events_are_throttled`, `items_are_recorded`, `job_done_and_worker_exit_emitted` (fields per Decision 42; `sandboxed: false`), `owner_via_rights_are_kept` (8's fields round-trip to the view and the bus), `diag_forwarded_only_when_on_and_allowed`;
  - files and hooks: `files_created_sized_and_private` (0600/0700; Windows DACL inherited), `prepare_writes_before_any_process` (a `prepare` that records `workers().processes` = 0), `missing_prepared_file_fails_the_attempt`, `symlinked_prepared_file_is_refused`, `on_succeeded_runs_after_group_exit_before_delete` (the hook sees the worker's pid gone and the directory present; after it, the directory is gone), `on_succeeded_can_replace_the_result`, `on_succeeded_failure_fails_the_job`, `hook_timeout_is_job_timeout`, `job_dir_removed_after_failure_and_cancel`;
  - helpers: `helpers_start_before_the_worker` (hello order), `helpers_only_get_their_files` (the attach lists only the helper's names), `helper_exit_early_is_helper_failed`, `helpers_die_with_the_job` (`test:spawn` helper's grandchild gone after the job), `helpers_shut_down_after_result`;
  - cancel and pause: `cancel_queued`, `cancel_running_kills_tree` (`test.spawn`), `pause_queued_keeps_place`, `pause_running_resumable_checkpoints`, `pause_all_stops_dispatch`, `resume_requeues`;
  - liveness through the engine: `hung_job_fails_worker_hung`, `stalled_job_fails_job_stalled`, `timeout_fails_job_timeout`, `oom_fails_worker_oom` (macOS/Linux sampling, Windows job limit, cgroup where delegated);
  - abandonment: `two_abandoned_mark_io_pool_stuck` (with `abandonForTest`), `stuck_pool_refuses_submit_resource_stuck`, `stuck_clears_when_one_exits`;
  - persistence: `transitions_written_at_once`, `checkpoint_throttled_to_10s` (TestClock), `restart_requeues_resumable_from_checkpoint` (`test.checkpoint`: a second engine over the same server data resumes at the checkpointed step), `restart_interrupts_non_resumable`, `restart_keeps_paused`, `job_dirs_swept_at_start`, `close_leaves_no_running_rows`, `retention_runs_hourly` (TestClock).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates on all three OSes (or Linux plus CI for the others) and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): job engine with persistence, helpers, mapped files and kind hooks`.

### Task 10: Retries, backoff, poison handling and isolation — tier: opus — after Task 9

**Files:**
- Create: `apps/finisherd/src/jobs/isolation.ts` (pure: `classify(exit | error) → FailureClass`; `nextStep(state, failure, attempt) → Retry | Isolate | Fail`; the isolation state machine of Decision 32).
- Modify: `apps/finisherd/src/jobs/{runner.ts, engine.ts}`.
- Test: `apps/finisherd/test/jobs/{isolation.test.ts (pure, fast-check), retries.test.ts (real processes)}`.

**Interfaces:**
```ts
export type FailureClass = "crash" | "oom" | "resource" | "deterministic" | "fresh" | "mismatch" | "hook" | "cancel";
export function classify(f: { exit?: ExitInfo; error?: JobErrorMsg; hook?: FinError }): { cls: FailureClass; error: FinError };
export interface IsolationState { pending: string[]; inFlightAlone?: string; queue: string[][]; poisoned: string[]; crashRuns: number; budget: number }
export function startIsolation(items: ReadonlyArray<string>): IsolationState;
export function afterRun(s: IsolationState, run: { items: ReadonlyArray<string>; done: ReadonlyArray<string>; inFlight?: string; crashed: boolean }): IsolationState;
export function nextRun(s: IsolationState): ReadonlyArray<string> | undefined;     // undefined: nothing left
export function backoffMs(retry: number): number;                                   // 5 s × 2^(retry−1), ≤ 300 s
```
Decisions 31, 32.

- [ ] **Step 1: Write failing tests:**
  - pure: `classification_table` (every row of Decision 31), `backoff_doubles_to_five_minutes`, `isolation_properties` (fast-check: done items never re-run; every item ends done or poisoned; a single poison item among n is found; crash runs ≤ `2 + 2⌈log₂ n⌉`; no poison → nothing poisoned);
  - processes: `poison_item_isolated_in_two_runs` (`test.crash` with one poison item in flight: run 2 is the item alone and fails `POISON_ITEM`, run 3 completes the rest; the result lists 1 item error and the rest), `crash_between_items_bisects` (`between: true`), `reported_items_are_not_rerun` (the worker's `begin` log), `hang_is_never_bisected`, `stall_is_job_stalled_never_bisected`, `helper_failure_is_never_bisected`, `retry_backoff_persists_next_run_at` (TestClock: 5 s, 10 s; a restart keeps `nextRunAt`), `max_attempts_then_failed`, `fresh_retry_once_at_once` (`error retry: fresh`), `deterministic_error_not_retried`, `mismatch_not_retried` (`testbad` family), `oom_with_item_in_flight_isolates`, `oom_without_item_fails`, `interactive_retries_once_at_once`, `isolation_budget_exhausted_fails_worker_crashed` (every item poison).
- [ ] **Step 2: Implement.** **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): job retries with backoff and isolation of crashing batches`.

### Task 11: Warm workers — tier: opus — after Task 10

**Files:**
- Create: `apps/finisherd/src/jobs/warm.ts` (per family: workers, ready list, FIFO, expiry and recycling timers, per-job helper groups).
- Modify: `apps/finisherd/src/jobs/{engine.ts, runner.ts, schedule.ts (only to wire `spawnWarm`)}`.
- Test: `apps/finisherd/test/jobs/warm.test.ts`.

**Interfaces:** `JobEngine.warm(family)` live; `workers()` lists warm workers `{family, pid, state: spawning|ready|busy, served, ageS, idleS}`. Decisions 23, 35, 36; spec §6.3.

- [ ] **Step 1: Write failing tests:**
  - `back_to_back_on_one_process` (100 `test.warm` jobs: one pid, `served` 1…100) with **`dispatch_p99_under_5ms`** (submit → `start` written, measured in the engine; asserts ≤ 5 ms p99 on Linux, records the others);
  - `first_submit_prewarms`, `warm_family_starts_one_ahead`, `slots_default_one_max_two`;
  - `cold_jobs_never_delay_interactive` (all `cpu` slots busy with `test.sleep`; a `test.warm` still dispatches at once);
  - `idle_expiry_60s` (TestClock: `shutdown` at 60 s, `worker.exit` reason `idleExpired`), `recycled_after_500_jobs`, `recycled_after_30_minutes`;
  - `crash_mid_job_retries_on_fresh_worker` (`killProcForTest` during a job: the retry runs on a new pid and succeeds; `worker.exit` `crashed`/`killed`), `replacement_on_next_submit`;
  - `cancel_keeps_a_cooperative_worker`, `cancel_kills_an_uncooperative_worker` (500 ms grace);
  - `helpers_get_a_per_job_group_on_warm_workers` (a warm kind with a test helper: the helper's group dies with the job, the worker lives);
  - `fresh_and_reused_results_equal` (the same `test.warm` params on a fresh and on a reused worker give equal results apart from `pid`/`served`, the family contract of spec §6.3);
  - `warm_kind_at_normal_priority_runs_cold`;
  - `memory_limit_is_the_interactive_rule`;
  - `close_shuts_down_warm_workers`.
- [ ] **Step 2: Implement.** `warm.ts` ≤ 500 lines.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): warm interactive worker pools with expiry and recycling`.

### Task 12: Wiring, `finisherd`-death tests, the spawn gate and budgets — tier: sonnet — after Task 11

**Files:**
- Create: `scripts/check-worker-spawn.mjs` (+ `.test.mjs`; Decision 24), `apps/finisherd/test/jobs/kill.test.ts`, `apps/finisherd/test/support/engine-child.ts` (a Node child that builds the engine layer over a server data, submits what its argv says, prints the pids it learns, and waits), `apps/finisherd/scripts/jobs-budget.ts`.
- Modify: `apps/finisherd/src/main.ts` (import `jobs/store.ts` before the store opens; `ensurePrivateDir(<server-data>/jobs)`; `layerJobEngine` after the control store, with `memorySink`; `engine.close()` in the shutdown path before the store closes; one start log line with `caps()`), `scripts/gates.mjs`, `TASKS.md`.

**Why no e2e scenario here:** 5C-1 has no RPC and no `fin` command; the first `fin`-driven job scenario comes with 5C-2 (`fin wait`) and 6A-2 (`fin look`). `kill.test.ts` drives a real engine process instead.

- [ ] **Step 1: Write failing tests:**
  - `kill.test.ts` (all three OSes): `finisherd_kill_leaves_no_process` (the engine child runs `test.spawn` and a `test.hang`; the test kills the child with `SIGKILL` / `process.kill` (= `TerminateProcess` on Windows); worker, grandchild and the hung worker are gone within 5 s; the watchdog is gone on Unix), `restart_after_kill_interrupts_and_sweeps` (a second engine child over the same server data: the rows are `interrupted`/requeued per Decision 28 and `jobs/` is empty), `graceful_close_leaves_no_running_rows`;
  - `main.ts`: `server_starts_with_the_job_engine` (the existing `server.test.ts` start path; the start log names the kill tree and `sandboxed: false`), `jobs_dir_is_private`;
  - `check-worker-spawn.test.mjs`: flags each forbidden API in a fixture tree, ignores `test_family/` and `watchdog.rs`, passes on the real tree.
- [ ] **Step 2: Budget script** (`jobs-budget.ts`, release builds): store statements p99 with 10k jobs and 40k items (≤ 1 ms), warm dispatch p99 (≤ 5 ms), spawn → `hello` p50/p95 for `test.echo` (recorded; 6A-2's budget is ≤ 150 ms p95), cancel → tree gone p95. Prints a table; Task 14 records it.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): start the job engine with the server; host-death tests and the worker spawn gate`.

### Task 13: Spec as built, architecture pointer, tracker — tier: sonnet — after Task 12

**Files:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§6.0–6.6 and §6.8–6.9 "as built (5C-1)": the three crates, the watchdog, the cgroup conditions, `prepare`, `rememberPoison`, `WORKER_OOM`, the new exit reasons, the isolation budget, measured numbers; §10 rows for `fast-check` and `libc`; §11 decisions referenced), `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 pointer "Sub-project 5C-1 as built"; §7's `fin-workers` line), `TASKS.md` (tick; follow-ups: **D4 replaces `memorySink` with the journal sink**; 5C-2 RPCs and ownership checks; `server.status` reports `caps()`; sub-project 7 implements `Spawner` and denies process creation in the `worker` profile; Linux without delegated cgroups cannot kill a `setsid` grandchild after host death (the watchdog kills process groups only): documented gap, closed by 7's sandbox or a delegated cgroup; sub-project 9 packages and signs `fin-workers` beside `fin-node`).

- [ ] **Step 1:** Write the as-built notes from the code, listing every deviation from spec revision 3. **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 5C-1 as built`.

### Task 14: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

On macOS (Apple silicon), Windows 11 (non-elevated) and the Linux VM; host details in `archive/hosts.md`.

- [ ] **Step 1:** `cargo test --workspace`, `cargo test -p fin-workers` (test-hooks), `pnpm test`, `pnpm gates`, `pnpm e2e` on each OS.
- [ ] **Step 2: Kill trees by hand,** each OS: start a real `finisherd` (test-hooks `fin-node`) with the engine child of Task 12 running `test.spawn` (`none` and `setsid`) and `test.hang`; kill `finisherd` with `kill -9` / Task Manager's "End task" / `taskkill /F`; list leftover processes (`ps -eo pid,pgid,comm`, `Get-Process`). Expected: none, except the documented Linux `setsid` gap without cgroups. Repeat on Linux under `systemd-run --user -p Delegate=yes --wait` (cgroup mode: none at all) and inside the Docker image if one exists by then.
- [ ] **Step 3: Windows specifics:** `finisherd` started from Windows Terminal and from PowerShell (both may put it in a Job): nesting works; a `test.crash` `segv` shows no WER dialog; Defender does not slow spawn → `hello` beyond the recorded p95 (note it if it does).
- [ ] **Step 4: macOS specifics:** the watchdog survives `Ctrl-C` in the terminal that runs `finisherd` until `finisherd` exits, then cleans up; QoS read back with `taskinfo` or `ps -o pri`.
- [ ] **Step 5: Measure** with `jobs-budget.ts` on each OS and record in TASKS and the spec: statement p99, warm dispatch p99, spawn → `hello` p50/p95, cancel → tree gone p95, the watchdog's cleanup time after host death.
- [ ] **Step 6:** Fix what fails (one commit each, with a test); re-run Step 1.
