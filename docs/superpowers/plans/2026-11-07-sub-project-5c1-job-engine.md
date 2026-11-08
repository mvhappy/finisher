# Sub-project 5C-1: The job engine — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for kill and supervision semantics (the host's spawning and kill trees on each OS, host death, the napi binding's threading and backpressure, the engine's run loop, retries and isolation, warm workers, validation), `sonnet` for plumbing (the envelope crate, the worker binary, the store and kind registry, the scheduler, wiring, docs).

**Revision 2** (2026-11-07), after the independent review of `adb638e`, with the coordinator's calls. In short: plugin processes get `call`/`reply` host calls, a minor-version rule and a warm key; every process gets a leaf kill domain with `job_limits` and a self-sandbox entry; the JS thread never blocks (async spawn and kill, per-process writer threads, a bounded blocking event queue, progress coalesced in Rust); `onSucceeded` is persisted and at-least-once; exits drain stdout first; group leaders stay unreaped until `close_group`; memory limits mean the same thing everywhere (per group); isolation budgets and resource faults are tightened; Task 9 is split. The table at the end maps every finding to its resolution.

**Goal:** the one shared job system of spec §6.0, without its RPCs. After this plan:
- **`fin-jobs`** holds the envelope (Rust → JSON Schema → generated TS) and the worker and helper runtimes that every family links, with host calls (`call`/`reply`) and a self-sandbox entry;
- **`fin-workers <family>`** is the one worker binary, with the **`test`** family (behind `test-hooks`) and the Unix host watchdog;
- **`fin-jobhost`** (linked into `fin-node` as its `jobhost` module) spawns every worker and helper through a **`Spawner`** hook with a cleared environment, frames the envelope, runs the liveness, stall and timeout timers, enforces memory limits and owns a **host-side kill tree on every OS**: a kill-on-close Job Object on Windows (host → group → leaf; each process created suspended, assigned, then resumed), process groups with `PR_SET_PDEATHSIG` and cgroup v2 where delegated on Linux, process groups on macOS, a watchdog on Unix, and stdin-EOF exit everywhere;
- **`finisherd`** has the **`JobEngine`**: jobs persisted in `control.db` (the `jobs` migration chain), a pure scheduler over resource pools (`cpu`, `gpu`, `hwenc`, `net`, `io:<root>`), warm `interactive:<family>` pools with idle expiry and recycling, helpers and mapped files, the `prepare` and `onSucceeded` kind hooks, retries with backoff, poison handling and isolation of crashing batches, restart re-queue, and the `job.done` and `worker.exit` events behind a sink interface;
- sub-project 6's 6A-2, 5D and sub-project 8's 8B/8C build against the frozen interfaces listed under **Frozen interfaces**.

**Not in this plan:** the `jobs.*` RPCs, scopes, ownership and reach checks, `jobs.watch`, `fin job …` and `fin wait` (5C-2); the `library` family (5D); the `media` family, `fin-shm` and the media `job.done` fields (sub-project 6); `fin-sandbox` (sub-project 7: the `Spawner` hook, `job_limits` and the self-sandbox entry are here, their implementations are not); plugin adapters, the plugin host's `onCall` methods and `externalRef` (sub-project 8); journal wiring (diagnostics D4: events go to a sink until then).

**Architecture:**
- **Rust:** `crates/fin-jobs` (new; protocol types, worker and helper runtimes with host calls and the self-sandbox entry, the read budget, the params registry type; no process-creating code), `crates/fin-jobhost` (new rlib; spawning, kill domains, framing, timers, memory, watchdog client), `crates/fin-workers` (new binary; family dispatch, the `test` family and helpers behind `test-hooks`, `watchdog`), `crates/fin-node/src/jobhost/` (napi `JobHost`, `validateJobParams`), `crates/fin-core/src/events` (`job.done`, `worker.exit`, three name tables), `crates/fin/src/output.rs` (codes).
- **TypeScript:** `packages/contracts/src/jobs.ts` (TS-owned job records as Effect Schemas), `apps/finisherd/src/control/migrations/jobs/0001.ts`, `apps/finisherd/src/jobs/` (store, kinds, pools, scheduler, host wrapper, files, runner, calls, helpers, control, restart, isolation, warm pools, events, engine, layer), `packages/fin-node` (typings, `workersBinaryPath`, builds that place `fin-workers` beside the `.node` file).

**Tech Stack:**
- Rust 1.99.1, edition 2024. `serde`, `serde_json`, `schemars` 1, `blake3` 1, `napi` 3 (all already in the tree); **`libc` 0.2** (MIT OR Apache-2.0; already in the lock file, now a direct dependency of `fin-jobhost`, `fin-workers` and `fin-jobs` on Unix); **`windows-sys` 0.61** (MIT OR Apache-2.0; already in the tree) with `Win32_Foundation`, `Win32_Security`, `Win32_System_JobObjects`, `Win32_System_Threading`, `Win32_System_Pipes`, `Win32_System_IO`, `Win32_Storage_FileSystem` in `fin-jobhost`, and `Win32_System_Diagnostics_Debug`, `Win32_System_ErrorReporting` in `fin-jobs` (the runtime's crash-UI suppression, Decision 6); `proptest` 1 (dev, already in the tree). Pipes through `std::io::pipe` (stable since 1.87).
- Node 26.12 using only APIs Node 24.22 also has; Effect 4.0.6; vitest; **`fast-check` 4** (MIT; dev-only, exact pin) for the scheduler and isolation properties.
- **No other new dependency.** Rejected: `nix` (not needed beyond `libc`), `tokio`/`async-process` (blocking threads per process are simpler and bounded), `command-group` and `process-wrap` (they cannot create processes suspended or join a pre-made Job and cgroup), `sysinfo` (heavy; the memory figures are a few syscalls), `jsonschema` (params are validated by their Rust types).

**Spec:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` revision 3, §3.1 (migration chains), §3.4 (`job.done`, `worker.exit`), §6.0–6.6, §6.8, §6.9, §6.10 (the interface additions of this plan, added with revision 2), §10, §11 Decisions 23–24d. Sub-project 6: `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` §3.1–3.6, §14 (memory defaults), §17 (`WORKER_OOM`). Sub-project 8: `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §3.3–3.7, §16.2. 6A plan: `docs/superpowers/plans/2026-11-07-sub-project-6a-media-foundations.md` (6A-2 entry check and Task 12's kind declaration). 5A plan: `docs/superpowers/plans/2026-11-07-sub-project-5a-principals-and-scopes.md` (Tasks 1 and 2 are frozen).

**Branch:** `sp5c1-job-engine`, stacked on `sp5a-principals` (5A Tasks 1, 2 and 10 in; this plan uses only Tasks 1 and 2). It runs beside 5A Tasks 3–14 and beside 6A-1. 5C-2 stacks on this branch and on 5A Task 6. Rebase onto `main` as 4C and 5A merge.

**What this plan uses from 5A (frozen):** `openControlStore`/`ControlStore {db, tx, deleteInBatches, close}` and `registerChain(name, migrations)` (`apps/finisherd/src/control/{db,migrations}.ts`), the `ControlStoreTag` layer, `ensurePrivateDir(dir)` (`config.ts`) for `<server-data>/jobs/`, `childEnv()` (`env.ts`). Nothing else: no principals (owners are recorded, not checked), no audit kinds (none are job kinds).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate over what each task adds.
- **Primitive ops live only in `fin-core`.** This plan adds none. `fin-core` stays pure (it gains only event payloads and name tables).
- **TypeScript model types are generated, never hand-written.** The envelope, the test kinds' params and results, `JobDone` and `WorkerExit` are Rust types exported through `fin-store`'s `export_schema` example and regenerated with `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). TS-owned control-plane records (the job row and its views, kind declarations) are Effect Schemas in `@finisher/contracts`, their TS types `Schema.Type` of them. **Params are validated by Rust** (`validateJobParams`, Decision 27), as the contracts say for every RPC payload.
- **Dependency rules:** `fin-jobs` depends only on `std`, `serde`, `serde_json`, `schemars`, `blake3`, and `libc`/`windows-sys` for the self-sandbox entry and crash-UI suppression; it creates no process. `fin-workers`' shipped families never call a process-creating API (Decision 24); `finisherd` uses no `child_process` for jobs.
- **The JS thread never blocks on a process** (Decision 11a): no synchronous napi call waits for a spawn, a kill, a pipe or a reap.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit: `crates/fin-core/src/events/names.rs` 504 (new tables go into `names_jobs.rs`, referenced from `TABLES`), `crates/fin-core/src/events/kinds.rs` 370 (two registry lines). Planned splits are named in each task; a file that grows past 550 lines is split before the commit.
- Errors are `{ code, message, details, fix? }`. New codes go into `crates/fin/src/output.rs` and the contracts' code list (Decision 41).
- **The repo is public:** no host names, IPs, user names, cgroup paths of a real machine or key names in tracked files.

**Every commit is green** on macOS, Windows 11 and Linux. Until Task 5 lands, `fin-jobhost`'s Windows kill domain answers `JOBHOST_UNSUPPORTED` and its process tests are `#[cfg(unix)]`; the TS tests that spawn real processes skip on `win32` with a notice naming Task 5. From Task 9a on, nothing skips on any OS except the cgroup tests (Decision 16) where cgroups are not delegated.

**Testing and process**
- Tests never use the network. Every engine test uses a temp server data and a test-hooks `fin-workers` (`packages/fin-node/test/native/`).
- **Process tests** (`crates/fin-workers/tests/host_*.rs`) find the binary through `env!("CARGO_BIN_EXE_fin-workers")`; host-death tests re-execute the test binary as the host (the `fin-fonts` crash-child pattern) and kill it with `SIGKILL` / `TerminateProcess`. A pid is "gone" when `kill(pid, 0)` answers `ESRCH` (Unix) or `OpenProcess` fails / `GetExitCodeProcess` is not `STILL_ACTIVE` (Windows), polled for ≤ 5 s.
- Timing-dependent tests take their timers from `Timing` (Rust) or Effect's `TestClock` (TS) and never sleep for a real 30 s.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks 6 and 9a–12 also run `rtk pnpm e2e` once at the end (it must stay green; 5C-1 adds no scenario, Task 12 says why).
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **Tracker:** Task 1 adds a "Sub-project 5C-1: job engine" section to `TASKS.md` with one line per task; each task ticks its line, with as-built notes, in its own commit.
- **Batches and order:**
  - A: Task 1 → Task 2. Task 7 starts after Task 1; Task 8 after Task 7. Tasks 7 and 8 run beside Tasks 2–6 (they touch only TS files and need only Task 1's generated types).
  - B: Task 3 → Tasks 4 and 5 in parallel (each implements Task 3's `KillDomain` trait in its own files: `unix/`, `linux/` vs `windows/`); Task 6 after Task 3, beside 4 and 5.
  - C: Tasks 9a → 9b → 10 → 11 (strictly in order; 9a needs 4, 5, 6, 7, 8).
  - D: Tasks 12 → 13 → 14.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Crates and binaries

1. **Three crates.** `fin-jobs` (protocol and runtimes, linked by every worker and helper and by the host), `fin-jobhost` (the host side: an rlib so its logic is tested natively; `fin-node` is a `cdylib` with `test = false`), `fin-workers` (the binary). `fin-node`'s `jobhost` module is only the napi binding over `fin-jobhost`, so spawning still lives in `finisherd`'s process (spec §6.2), and `fin-sandbox` (sub-project 7) can implement `Spawner` without napi.
2. **Where binaries are found.** `fin-workers[.exe]` sits beside the `fin-node` `.node` file: `packages/fin-node/` (release `build`) and `packages/fin-node/test/native/` (`build:test`, with `test-hooks`); `FIN_NODE_BINARY_DIR` already selects the directory. A family's program is **`Program::Bundled(name)`**, a bare name resolved in that directory and limited to `fin-workers` and, for 8C, `fin-analyze`, or **`Program::Path(abs)`** for plugin processes (8B), set only by a registered family or helper resolver, never from job params.
3. **Versioning.** `hello` carries `v` (the major, = 1), `minor` (this plan: 0), `envelope` (the first 16 hex digits of BLAKE3 over the envelope's canonical JSON Schema, computed once per process) and the `{kind, version}` pairs it serves.
   - **`Program::Bundled` processes** (built with this tree): the host requires `v == 1`, an **equal `envelope`** and the dispatched kind at the declared version. Anything else is **`WORKER_PROTOCOL_MISMATCH {expected, got}`**, never retried.
   - **`Program::Path` processes** (plugins, built elsewhere against an SDK): the host requires `v == 1` and the kind at its version, and accepts any `minor` and any or no `envelope`. A message with an unknown `t` from a process whose `minor` is above the host's gets `reply {id, error: {code: "UNKNOWN_MESSAGE"}}` when it has an `id` and is otherwise dropped and counted; it is not killed. At the host's minor or below, an unknown `t` is a protocol error, as for bundled processes. Additions within a major are optional messages and optional fields only.

### The envelope (`fin-jobs`)

4. **Framing:** UTF-8 NDJSON on the process's stdin/stdout, one message per line: `{"v":1,"t":"<type>","seq":n, …}`. Lines ≤ 1 MiB; a `checkpoint`'s `data` ≤ 256 KiB; `seq` starts at 1 in each direction and grows by exactly 1. Bad JSON, an unknown `t` (but see Decision 3), a `seq` gap, an oversized line or a message naming another job than the one started → the host kills the process with reason **`protocolError`**; the job sees `WORKER_CRASHED {protocol: <what>}` and is isolated like a crash (Decision 31). **stderr** is a log stream: lines ≤ 4 KiB go to `finisherd`'s log at debug level as `[job <id> <family>] …`; the last 8 KiB are kept and stored as `error.details.stderrTail` of a crashed attempt (control.db only, never the journal).
5. **Messages.**
   - Host → worker: `start {job: {id, kind, kindVersion, params, checkpoint?, attempt, extra?}, limits: {aliveMs, memoryBytes?, readBytesPerSec?}, files: [{name, path, mode, bytes?}], helpers: [{id, kind, files}], dirs: {job, tmp}, diagnostics}`, `cancel {job, reason: cancel|pause}`, `shutdown {}`, `reply {id, value?, error?}`.
   - Worker → host: `hello {v, minor, role: worker, family, kinds, warm, envelope?, selfSandbox?, pid}`, `begin {job, item}`, `item {job, item, value?, error?}` (with `error`: a failed item, recorded in `itemErrors`; the job goes on), `progress {job, done, total?, unit, phase?}`, `checkpoint {job, data}`, `alive {}`, `result {job, value}`, `error {job, error: {code, message, details, retry: never|backoff|fresh}, itemErrors?}`, `ready {}`, `diag {job?, kind, payload}`, `call {id, method, params}`.
   - Any message counts as alive; the runtime sends `alive` every `limits.aliveMs` (5 s) from its own thread.
5a. **Host calls** (`call`/`reply`): a process asks its host for something by `call {id, method, params}` (`id` unique per process, `method` a dotted name). The host routes it to the family's `FamilySpec.onCall` (or the helper's `HelperProgram.onCall`) with a `CallCtx {job, owner, via, rights, role, helper?}`; the `Effect`'s value or `FinError` comes back as `reply {id, value}` / `reply {id, error}`. A family without `onCall`, or a method it does not know, gets `error {code: "INVALID_REQUEST", details: {method}}`. At most 16 calls are in flight per process (the 17th gets `error {code: "CALL_LIMIT"}`); replies may come in any order; calls still open when the process exits are interrupted. A call and its reply are ≤ 1 MiB like every line: bulk data goes through mapped files, and ring handoff (`slot`/`release`, sub-project 6 §3.4) is a pair of calls. Sub-project 8 serves its host API this way (`project.snapshot`, `media.pcm`, `net.fetch`, `job.submit`, …); 5C-1 ships only the mechanism and test methods.
6. **Worker runtime** (`fin_jobs::worker::run`), in order: the self-sandbox entry (Decision 9a); on Windows `SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX)` and `WerSetFlags(WER_FAULT_REPORTING_NO_UI)`; `hello`. Then it reads `start`, runs the family's `run` with a `JobCtx`, answers `result` or `error`, and **exits 0 (cold) or sends `ready` (warm)**.
   - **stdin EOF at any time → `std::process::exit(EXIT_HOST_GONE = 70)` at once** (the host is gone).
   - `cancel` sets the job's `CancelToken`; the family checks it and returns `JobError::cancelled()`. With `reason: pause` a resumable family writes a checkpoint first.
   - `shutdown` (sent only to an idle worker) exits 0.
   - A panic in `run` is caught, answered as `error {code: WORKER_CRASHED, retry: backoff}`, and the process exits 101 (a warm worker is not reused after a panic).
   - **Before `ready`, a warm family has closed every file and unmapped every mapping under the job directory** (a family contract; Decision 36 tests it). Windows can then delete the directory, and the next job sees nothing of the last one.
7. **`diag`** is forwarded to the sink (Decision 42) only when `start.diagnostics` is true and the kind is in the family's `diagKinds`; otherwise it is dropped and counted (`workers().diagDropped`). With `diagnostics: false`, `JobCtx::diag` is a no-op, so a family builds no payload (6A-2's "zero emissions" test).
8. **Helpers** have their own channel: helper → host `hello {v, minor, role: helper, kind, id, envelope?, selfSandbox?, pid}`, `alive`, `error`, `call`; host → helper `attach {job, files, dirs}`, `shutdown {}`, `reply`. The helper runtime (`fin_jobs::helper::run`) mirrors Decision 6 (self-sandbox entry first; stdin EOF → exit 70). A helper's stdio goes to the host only; workers and helpers share nothing but the job's mapped files.

### Spawning and the kill tree (`fin-jobhost`)

9. **`Spawner`** (spec §6.2) is called for every process with a `SpawnRequest` (role, profile `worker|plugin`, family, helper, program, args, env, cwd, path grants, QoS) and may rewrite a `PlatformCommand`:
   - program and arguments, for a wrapper; extra environment;
   - on Linux, extra child-side `pre_exec` steps, run after ours (they must not allocate, Decision 12);
   - on Windows, extra process attributes and a token;
   - on every OS, **`job_limits: JobLimits {active_processes?, ui?}`**, applied to the process's leaf kill domain (Decision 17), and **`self_sandbox: Option<SelfSandbox {profile, args}>`** (Decision 9a).

   macOS takes no `pre_exec` (Decision 13); a sandbox there rewrites the program or uses the self-sandbox entry. The default is **`Unsandboxed`**: `caps().sandboxed == false`, the engine logs one warning at start and `workers()` reports it (5C-2 puts it in `server.status`).
9a. **Self-sandbox entry** (for sandboxes that a process must apply to itself after `exec`: seccomp on Linux, Seatbelt `sandbox_init` on macOS).
   - When `self_sandbox` is set, the host passes it to the process as `FIN_SELF_SANDBOX` (JSON, no secret).
   - `fin_jobs::worker::run` and `fin_jobs::helper::run` first call the function registered with `fin_jobs::sandbox::register(f)`; sub-project 7's `fin-sandbox` supplies it, and `fin-workers`' `main` registers it. Only then do they send `hello`, which reports `selfSandbox: <profile>`.
   - **Fail closed:** a variable set with no function registered, or a function that fails, makes the process exit 78 before `hello` (the host reports `SpawnFailed("selfSandbox")`). A `hello` whose `selfSandbox` differs from the request is a protocol error.
   - In 5C-1 nothing registers a function, so a test `Spawner` exercises both paths.
10. **Environment and handles.** Every process gets `childEnv()` with no extras (never `FIN_SERVER`, `FIN_SESSION`, `FIN_CREDENTIAL`), plus `TMPDIR`/`TEMP`/`TMP` = its private temp directory (Decision 38), and `FIN_SELF_SANDBOX` when requested; cwd = the job directory (cold) or the worker directory (warm). It inherits exactly stdin, stdout and stderr:
    - Linux: `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` in `pre_exec` (fallback: `fcntl(FD_CLOEXEC)` up to `RLIMIT_NOFILE`);
    - macOS: `POSIX_SPAWN_CLOEXEC_DEFAULT`;
    - Windows: the pipes are created non-inheritable. **Only on the spawn thread, immediately around `CreateProcessW`**, the three child ends are made inheritable (`SetHandleInformation`), passed in `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, and made non-inheritable again, so no concurrent `CreateProcess` elsewhere in the process (libuv's) can inherit them.
      **As built (deviation, Task 5):** the flip still leaked the ends into concurrent inheriting spawns (every batch of `concurrent_libuv_spawn_inherits_no_pipe`), so the host never makes a handle inheritable. It keeps a **spawn broker**, a suspended, never-resumed process in the host Job; the three ends are duplicated into it as inheritable for the call only, and the child is created with `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS` = the broker plus `HANDLE_LIST`. The broker is the bundled `fin-workers.exe --spawn-broker` (exits 0 at once if ever resumed), else `%SystemRoot%\System32\cmd.exe /d /c exit 0`, never the host's own executable; it is created at `Host::start` with a DACL granting `SYSTEM` only and `OWNER RIGHTS` nothing. Without a broker a spawn is refused (`WORKER_UNAVAILABLE {reason: noBroker}`): there is no inheritable fallback.
11. **One spawner thread; per-process threads.**
    - Every spawn and every kill runs on one long-lived `jobhost-spawn` thread: on Linux `PR_SET_PDEATHSIG` fires when the *thread* that forked exits, so spawning from a short-lived napi or pool thread would kill healthy workers.
    - Per process the host runs a stdout reader, a stderr reader, a **writer thread fed by a bounded queue** (256 messages or 4 MiB) and a waiter.
    - `send` enqueues and never blocks. A full queue means the process is not reading its stdin: the process is killed as `hung` (`WORKER_HUNG`).
    - Every blocking syscall retries on `EINTR`.
11a. **The JS thread never blocks** (napi, Task 6).
    - `spawn`, `killGroup` and `shutdown` return promises settled from the spawner thread; `openGroup` only allocates an id (the OS objects are made on the spawner thread at the group's first spawn); `send` only enqueues.
    - Events reach JS through one **bounded `ThreadsafeFunction`** (1,024 events **and** 16 MiB of their JSON; one larger event still goes alone): when JS falls behind, the reader threads block, the pipes fill, and the worker blocks on its own writes. Backpressure reaches the worker; memory does not grow.
    - Lifecycle events (`spawned`, `exit`, `groupEmpty`, a spawn's answer) go through the same queue but are not counted and never wait: a process has one of each (an `exit` carries ≤ 8 KiB of stderr tail), and most come from the spawn thread, which therefore does not stall kills, spawns and kill waits behind a lagging JS thread. What remains: an `exit` the spawn thread reports itself (an abandoned or unwaited process) waits for that process's own events lock while its reader waits on the queue, and a pending `progress` flushed before an `exit` is counted. Acceptable: both last only as long as the JS thread is behind, and it never waits on the host.
    - A `spawn` resolves through the event queue, before the process's `spawned` and before its readers start: no event of a process reaches JS before its `spawn` resolves (the code after `await spawn(…)` has run, since Node drains microtasks between threadsafe-function calls).
    - Every promise is settled: a dropped answer (a callback never run, a panic on a host thread, a thread that did not start) rejects `INTERNAL {reason: "unanswered"}` rather than leaving the promise pending and the event loop held.
    - The liveness timers count silence **only while a reader waits on its pipe**; time spent blocked handing events to JS does not count, so a slow event loop never makes a worker look hung.
    - **Progress is coalesced in Rust:** per process the newest `progress` is kept and forwarded at most 4 times a second. Any other message from that process first flushes the pending one, so order is kept. `alive` never reaches JS.
12. **Linux:** `std::process::Command` with `CommandExt::pre_exec`. Every path, `CString` and fd the child needs is prepared before the fork: **no allocation and no lock in `pre_exec`**, only async-signal-safe syscalls, each retried on `EINTR`. In order:
    - `setpgid(0, group_pgid_or_0)`;
    - `prctl(PR_SET_PDEATHSIG, SIGKILL)`;
    - `getppid() != host_pid` → `_exit(70)`;
    - where cgroups are delegated, write `"0"` to the leaf's `cgroup.procs` through an fd opened before the fork;
    - `setpriority` for QoS;
    - `close_range`.

    Nothing of the worker runs before it is in its process group and cgroup.
13. **macOS:** `posix_spawn` through `libc` with `POSIX_SPAWN_SETPGROUP`, `POSIX_SPAWN_CLOEXEC_DEFAULT`, **`POSIX_SPAWN_START_SUSPENDED`** and `posix_spawnattr_set_qos_class_np`. The host registers the pgid with the watchdog (Decision 15), then resumes the process with `kill(pid, SIGCONT)`; a failure between spawn and resume kills the suspended process. Task 3 proves first that `SIGCONT` resumes a `START_SUSPENDED` process. If it does not, the host spawns without suspension and registers the pgid right after `posix_spawn` returns; the worker's first act is then to block on its stdin until `start` or `attach`, so it runs no job code unregistered, and Task 13 records the difference.
14. **Windows: host → group → leaf Jobs.**
    - At `Host::start`: one **host Job** (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`, no `BREAKAWAY_OK`, a non-inheritable handle held only by `finisherd`) and one advisory I/O completion port.
    - Per group: a **nested group Job** with the same flags, plus `JOB_OBJECT_LIMIT_JOB_MEMORY` when the group has a limit (Decision 20).
    - Per process: a **leaf Job** nested in its group Job, carrying the process's `job_limits` (`JOB_OBJECT_LIMIT_ACTIVE_PROCESS`, `JOB_OBJECT_UILIMIT_*`).
    - Each process: `CreateProcessW` with `CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT` and the QoS priority class, an explicit environment block and the handle list → `AssignProcessToJobObject` to the host, group and leaf Jobs in that order (the nesting holds even when `finisherd` already runs inside a terminal's or Electron's Job) → `ResumeThread`. A failure at any step `TerminateProcess`es the suspended process (`SpawnFailed`).
    - Cancel is `TerminateJobObject(group)`, which ends its leaves.
    - **Exits come from the process handle** (the waiter waits on it). The completion port's messages are not guaranteed to arrive, so they only speed things up. At each exit the host confirms the facts from the Jobs: `JobObjectLimitViolationInformation` (a memory violation → `oom`) and `JobObjectBasicAccountingInformation.TotalProcesses` (foreign processes, Decision 22).
    - If `finisherd` dies, the host Job's last handle closes and every process dies.
15. **Unix host death: the watchdog.** Process groups alone do not cover a dead host: on Linux `PR_SET_PDEATHSIG` reaches only direct children, and macOS has no equivalent, so a grandchild (spec §6.9's `test.spawn`) or a hung worker that never reads stdin would survive.
    - `Host::start` therefore starts **`fin-workers watchdog`** in its own process group, ignoring `SIGINT`, `SIGTERM`, `SIGHUP` and `SIGPIPE`.
    - The host writes `+g <pgid>` / `-g <pgid>` and `+c <cgroup dir>` / `-c <cgroup dir>` lines to its stdin as groups open and close. A `-g` line is always written **before** the group's leader is reaped (Decision 17).
    - When the parent exits (Linux `pidfd_open` + `poll`; macOS `kqueue` `EVFILT_PROC`/`NOTE_EXIT`) or stdin reaches EOF, the watchdog **first reads stdin to EOF**, so a late `-g` line still counts. It then sends `SIGKILL` to every listed process group, writes `1` to every listed `cgroup.kill`, and exits.
    - If the watchdog dies, the host logs it, restarts it at most once a minute and re-sends the list.
    - The watchdog is host machinery, not a worker: it serves no job and starts no process.
    - **Remaining window, documented:** a process that leaves its group (`setsid`/`setpgid`) escapes, both here and after a cancel (Decision 18); and members forked in the instant between the host's death and the watchdog's `kill(-pgid)` can be missed. Delegated cgroups (Decision 16) and sub-project 7's process-creation denial close both.
16. **cgroup v2 (Linux), where delegated.**
    - Used only when `/proc/self/cgroup` names a v2 path `<cg>` that is writable by us, whose `cgroup.procs` holds only `finisherd`'s own pid, and that has `cgroup.kill` (kernel ≥ 5.14). That is true for a systemd user service with `Delegate=yes` or a container with its own cgroup namespace. It is false for a terminal's scope (the shell lives there too), so `finisherd` never rearranges a cgroup it does not own.
    - Then the host moves itself into `<cg>/host` and enables the available `+memory +pids` controllers on `<cg>`, `<cg>/jobs` and each group.
    - It makes `<cg>/jobs/g<group>` per group (`memory.max` from the group's limit, `memory.oom.group = 1`) and `<cg>/jobs/g<group>/p<proc>` per process (`pids.max` from `job_limits.active_processes`).
    - Kill is `cgroup.kill` on the group. An OOM is the group's `memory.events` `oom_kill > 0`.
    - **At start, leftover `<cg>/jobs/g*` directories** from a previous run are killed (`cgroup.kill`) and removed.
    - `FIN_CGROUPS=off` in `finisherd`'s environment disables it; anything else is `auto`. Otherwise process groups.
17. **Kill domains.** Task 3 defines the per-OS trait `KillDomain` (`open_group`, `open_leaf`, `place` (called in the child or on the suspended process), `kill`, `memory`, `foreign`, `exit_facts`, `close_group`). Task 3 implements it for Unix process groups, Task 4 for cgroups, Task 5 for Windows Jobs.
    - A **group** is one kill domain: a nested Job (Windows), a cgroup (Linux, delegated) or a process group (otherwise; the group's first process creates the pgid, later ones join it). Every process also has a **leaf** inside its group: a leaf Job or leaf cgroup carrying `job_limits`, or nothing extra with process groups (where `job_limits` are not enforced: `caps().leafLimits == false`).
    - A **cold job** has one group for its worker and helpers. A **warm worker** has a group of its own, and each job it runs gets a second group for that job's helpers (spec §6.3).
    - **Reaping (Unix):** the waiter observes a group leader's exit with `waitid(P_PID, pid, WEXITED | WNOWAIT)` and reports it, but **reaps the leader only at `close_group`** (after the watchdog's `-g`). The pgid therefore cannot be reused while the group is open. Other members are reaped at once.
    - **`kill_group` on a closed group is a no-op.**
18. **Kill and abandonment.**
    - `kill_group` = `TerminateJobObject` / `cgroup.kill` / `kill(-pgid, SIGKILL)`, then waits `killWaitMs` (5 s) for every process of the group to exit.
    - One still alive (uninterruptible I/O) is **abandoned**: reported once (`worker.exit` reason `abandoned`), its pool slot released, reaped in the background and logged when it finally exits.
    - Two abandoned processes whose jobs share an `io:` pool mark that pool **stuck**: nothing is dispatched there, and new submits that need it fail **`RESOURCE_STUCK {pool}`** (exit 5) until one of them exits.
    - With process groups (macOS; Linux without delegated cgroups), a process that `setsid`s out of its group survives a cancel as it survives host death (Decision 15). This is a **deviation from spec §6.9**, which expects `test.spawn`'s grandchild gone after cancel and host death on all three OSes. It holds on Windows and with delegated cgroups; elsewhere only for a grandchild that stays in the group. Task 13 records it.
19. **Timers** (in the host, monotonic `Instant`, values from `Timing`; tests shorten them):
    - **hello** 10 s after spawn → `noHello` (`WORKER_UNAVAILABLE`, retried with backoff);
    - **hung** 30 s without any message after `hello` → `WORKER_HUNG`;
    - **stall** the kind's `stallS` (default 600 s) without `begin`, `item` or `progress` while a job runs → `JOB_STALLED`;
    - **timeout** the kind's `timeoutS` from `start` → `JOB_TIMEOUT`.

    Each kills the job's groups; a hung or stalled warm worker is lost with its group.
19a. **Exit after drain.** When the waiter sees a process exit, its stdout reader keeps reading until EOF or 200 ms, whichever comes first (a grandchild may hold the pipe open). Only then is `Exit` emitted, **from the same per-process queue as its messages**. A `result` written just before `exit(0)` therefore always arrives before the `Exit`, and the job succeeds.
20. **Memory limits: one meaning everywhere, per group** (sub-project 6 §3.2 item 5).
    - A `MemoryRule {fixedMiB?, ramFraction?, capMiB?}` resolves at dispatch to bytes: `fixedMiB`, else `min(capMiB, ramFraction × os.totalmem())`. The family gives a rule per class (`interactive`, `batch`); a kind may override it, as a function of its params (`media.render` at 2160p).
    - **The limit applies to the sum of the group's processes**: the worker and its helpers for a cold job. For a warm job, the worker's group takes the family's interactive rule and the job's helper group takes the kind's rule. Per group rather than per process because a job's helpers are part of the job, and a per-process limit would let a job with two helpers use three times its budget.
    - Enforcement: Windows `JOB_OBJECT_LIMIT_JOB_MEMORY` on the group Job (confirmed by the limit-violation information at exit, Decision 14); Linux delegated `memory.max` on the group cgroup; elsewhere the host samples each process every second and kills the group when the sum passes the limit. Sampled figures: Linux **`RssAnon + RssShmem`** from `/proc/<pid>/status` (file-backed pages are reclaimable and not the job's), macOS **`ri_phys_footprint`** from `proc_pid_rusage(RUSAGE_INFO_V4)`.
    - All of them answer **`WORKER_OOM {limit, used?}`**.
    - The host also attaches the worker group's current figure to every `ready` it forwards (Decision 36's recycling).
21. **QoS** (spec §6.2): `normal`; `utility` = macOS `QOS_CLASS_UTILITY`, Linux nice 5, Windows `BELOW_NORMAL_PRIORITY_CLASS`; `background` = `QOS_CLASS_BACKGROUND`, nice 10, `BELOW_NORMAL_PRIORITY_CLASS`. Interactive jobs run at `normal` whatever their kind says.
22. **Foreign processes.** The host counts processes in a group beyond the ones it spawned:
    - Windows: from `TotalProcesses` of the group Job (the port's `NEW_PROCESS` messages are only a hint);
    - Linux: from `cgroup.procs`, or a `/proc` scan for the pgid at each `result` and at exit;
    - macOS: `proc_listpgrppids`.

    `ExitInfo.foreign` carries the count. The engine logs any non-zero count. With the `test-hooks` build of `fin-node`, a job of any family but `test` with `foreign > 0` fails `WORKER_CRASHED {foreign}`, so 6A-2's and 5D's test suites catch a family that starts processes.
23. **Cancel** (spec §6.2): `cancel {reason: cancel}`, a grace of 2 s (500 ms for interactive jobs), then `kill_group` on the job's groups. On a warm worker, a job that answers within the grace (`error JOB_CANCELLED` then `ready`) keeps the worker; otherwise the worker's group is killed too.
24. **Workers never start processes** (spec §6.2, Decision 24b). Two checks hold the rule:
    - the static gate `scripts/check-worker-spawn.mjs` (in `pnpm gates`), which refuses `std::process::Command`, `fork`, `vfork`, `posix_spawn*`, `exec*`, `CreateProcess*`, `ShellExecute*` and `clone` in `crates/fin-workers/src/` and `crates/fin-jobs/src/`, outside `test_family/` and `watchdog.rs`;
    - Decision 22, at run time.

### Engine and persistence (`finisherd`)

25. **The `jobs` chain, step 1** (`control/migrations/jobs/0001.ts`, frozen once merged):
    ```sql
    CREATE TABLE jobs (
      id TEXT PRIMARY KEY, kind TEXT NOT NULL, kindVersion INTEGER NOT NULL, family TEXT NOT NULL,
      params TEXT NOT NULL, ownerKind TEXT NOT NULL, ownerId TEXT NOT NULL, via TEXT, rights TEXT,
      project TEXT, library TEXT, parent TEXT, priority TEXT NOT NULL, state TEXT NOT NULL,
      phase TEXT NOT NULL DEFAULT 'run',            -- 'run' | 'hooks' (Decision 40)
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
26. **Writes:**
    - a state or phase transition is one `UPDATE`, at once;
    - progress stays in memory (events at most 4 per second per job);
    - a checkpoint is written at most every 10 s per job (the newest kept, flushed at the next transition);
    - item outcomes are written in one transaction per second or per 256 rows.

    Every statement is prepared once (p99 ≤ 1 ms with 10k jobs, Task 12's budget). **Items are therefore at-least-once:** an item begun but not reported re-runs after a crash, and an item reported in the last second before `finisherd` itself dies may re-run after the restart. A family's per-item effects must tolerate a re-run; items acknowledged in `job_items` never re-run.
27. **Submit:**
    - an unknown kind → `JOB_KIND_UNKNOWN` (3);
    - params go through fin-node's `validateJobParams(kind, version, params)` (each kind's Rust params type, registered by its family's crate) → `JOB_PARAMS_INVALID {errors}` (3);
    - the same `idemKey` from the same owner → that job;
    - the same `dedupeKey` while `queued`, `running` or `paused` → that job;
    - a kind with `rememberPoison`, and a finished job of the same `dedupeKey` that failed `POISON_ITEM` → that job (sub-project 6 §3.3);
    - 50,000 unfinished jobs → `JOB_QUEUE_FULL` (5).

    Then insert and dispatch in the same turn (a ready warm worker directly, Decision 35; otherwise `schedule()`).
28. **Restart:** at start, every `running` row is examined:
    - **`phase: hooks`** (the worker had succeeded, its result is persisted, the hook may not have finished): its directory is kept and `onSucceeded` runs again, with no process, before the job ends `succeeded` (Decision 40);
    - otherwise it becomes `interrupted`. One that is resumable (a `resumable` kind, or a batch kind with items left) and has attempts left goes back to `queued` from its checkpoint and recorded items (the interrupted run counts as an attempt: a row found `running` at start means a crash). The others stay `interrupted` with `JOB_INTERRUPTED` and emit `job.done` with outcome `interrupted`.

    `paused` rows stay paused. Every directory under `<server-data>/jobs/` is deleted except those of `phase: hooks` jobs.
29. **Graceful close** (`engine.close`):
    - dispatch stops;
    - idle warm workers get `shutdown`;
    - jobs in `phase: hooks` finish their hook (bounded by its timeout);
    - running jobs get `cancel {reason: pause}` and a 2 s grace, then their groups are killed and they are handled as Decision 28 handles them, at once, so a clean stop leaves no `running` row in `phase: run`; **except that the stopped run spends no attempt** (owner decision, 9b review L6): a resumable job goes back to `queued` with its attempts as before the run, as a pause leaves them; a job whose pause was asked before the close stays `paused` (9b review M2); a run that ended with its own definitive outcome before the stop reached it follows the retry policy (9b review M1);
    - then `Host::shutdown`.
30. **Retention:** at start and hourly, finished jobs older than 7 days or beyond the newest 10,000 finished are deleted: their `job_items` first, then the rows, ≤ 500 rows per statement through `deleteInBatches`.

### Retries and isolation

31. **Failure classes** (spec §6.5), decided in this order:

    | Cause | Retry | Isolate (bisect) |
    |---|---|---|
    | a **resource fault**: `SIGBUS` (Unix) or `EXCEPTION_IN_PAGE_ERROR` (Windows), i.e. a mapped file or disk failed under the process; or any crash when the kind's `resourceHealthy(job)` answers false | backoff | never |
    | crash: exit without `result`/`error`, another signal, `protocolError`, a caught panic | backoff | yes (cold batch jobs only) |
    | `WORKER_OOM` on a cold job | with an item in flight: as a crash; otherwise none (it fails) | only the in-flight rule |
    | `WORKER_OOM` on a **reused warm worker** | once, at once, on a fresh worker | never |
    | `WORKER_HUNG`, `JOB_STALLED`, `JOB_TIMEOUT`, `HELPER_FAILED`, `noHello`/`SpawnFailed` (`WORKER_UNAVAILABLE`) | backoff | never |
    | worker `error` with `retry: backoff` (I/O, an offline root) / `fresh` (`GPU_DEVICE_LOST`) / `never` (default) | backoff / once at once on a fresh process / none | never |
    | `WORKER_PROTOCOL_MISMATCH`, a failed `prepare` or `onSucceeded` | none | never |
    | cancel | none (`JOB_CANCELLED`) | — |

    - **`resourceHealthy?(job)`** (an `Effect<boolean>` on the kind, e.g. 5D: "is the library root still online?") is asked **before any `POISON_ITEM` verdict**. False turns the crash into a resource fault: no item is poisoned.
    - **Backoff:** `5 s × 2ⁿ`, where n is the number of attempts made so far (10 s, 20 s, 40 s …), at most 5 minutes (`nextRunAt`, persisted), up to `maxAttempts` (default 3).
    - **Interactive jobs retry once, at once, on a fresh worker** (their `maxAttempts` is 2) and are **never isolated**.
    - The last failure's code is the job's error.
32. **Isolation of a crashing batch** (pure `isolation.ts`, state persisted in `jobs.isolation`). A batch kind declares `batch {items(params), restrict(params, items), combine?}`. Items acknowledged with `item` are never re-run. After a crash:
    - if an item was **in flight** (`begin` without `item`) and this run was not already that item alone, the next run is that item **alone**, then the rest;
    - if it was already alone, it fails as **`POISON_ITEM`** (an item error), and the rest go on without it;
    - if **no item was in flight**, the remainder is split in halves, each run in turn (a remainder of one is treated as "alone").

    **Isolation runs start at once, without backoff**, and do not count against `maxAttempts`. They have their own budget: at most `2⌈log₂ n⌉ + 2p` crashing runs, where n is the job's item count and p the poison items found so far, **capped at 2n**. Past it, the job fails `WORKER_CRASHED {isolation: "budget"}`, the case of crashes that no single item explains. Non-crash failures during isolation follow the table and count attempts. The final result is `combine(results, items)` (default `{items: {<item>: value}, itemErrors}`).

### Warm workers and scheduling

33. **Pools** are named slot counters. Defaults: `cpu` = max(1, cores − 1), `gpu` 1, `hwenc` 2 (sub-project 6 resizes it from the capability report), `net` 4. `io:<key>` pools are defined by their owner (5D) with `definePool`. A kind's `resources` lists pool names (one slot each, or a function of params). A job that needs an undefined pool fails `WORKER_UNAVAILABLE {pool}`.
34. **Order** (spec §6.3): effective priority = base level minus one level per 10 minutes waited (never above `interactive`), then `createdAt`, then id. `schedule(state, now) → Dispatch[]` is pure. The queue is indexed by (resource set, priority) as FIFOs, so one blocked head never hides a runnable job behind it. It is property-tested against a brute-force reference.
35. **Warm pools** (spec §6.3).
    - `interactive:<family>` holds 1 worker by default (at most 2; the family's `warm.slots`).
    - Only kinds marked `warm` submitted at `interactive` priority use it; a `warm` kind submitted at another priority runs cold.
    - A warm job takes a `cpu` slot while it runs but **never waits for one** (`cpu` may run over by at most the interactive slots), so a long cold job never delays it.
    - **Warm keys:** a family may declare `warmKey?(job) → string` (sub-project 8: `(plugin, project, invoker class)`). A warm worker is bound to the key of its first job and serves only jobs with that key. The family's slots are shared across keys; when a key has no worker and the pool is full, the longest-idle worker of another key is shut down to make room.
    - **A job whose key has a `ready` worker is handed to it directly in the submit's own turn, without `schedule()`:** one enqueue on that worker's stdin. Budget: ≤ 5 ms from submit to `start` written, p99, measured with 10k queued cold jobs and cold spawns in flight (Task 11).
    - With no ready worker and room in the pool, one is spawned and the job waits for its `hello`; otherwise the job waits in the pool's FIFO.
    - **Warm argv** (batch A review, M3): the engine makes a process warm by appending `--warm` to its argv (`fin-workers <family> --warm`; a plugin's `<path> <args…> --warm`), through `KindRegistry.program(family, job, "warm")`. A family declares one cold argv; `Family::warm()` stays its declaration that it can serve warm jobs.
    - **Order within one `schedule()`** (batch A review, M2): every dispatch first (a warm job only to a ready worker of its own key), then spawns and retirements for the warm jobs left waiting; a retirement takes only a ready worker whose key has no due queued job, so a worker is never retired from under a job it could run now, and alternating keys do not ping-pong.
    - **The `cpu` overrun** of warm dispatches is bounded by the sum of `slots` over every warm family (`warmOverrun`), checked by `schedule` exactly as `Pools.take` applies it.
36. **Warm lifecycle:**
    - **idle expiry:** a worker idle for **60 s** (the family's `warm.idleS`) gets `shutdown` (`idleExpired`);
    - **recycling:** it is retired after **500 jobs or 30 minutes**, or at a `ready` whose reported memory (Decision 20) passes **80 % of its limit**. **The replacement is spawned first**; the old worker keeps serving until the replacement's `hello`, then gets `shutdown` (`recycled`);
    - **a crash or kill during a job** loses the worker. The job is retried once at once on a fresh worker, and the pool spawns a replacement at the next interactive submit;
    - **pre-warming:** `warm(family)` starts one now if the pool has none (sub-project 6 calls it when a project opens);
    - times come from Effect's `Clock`;
    - the "closed before `ready`" contract of Decision 6 is checked in test-hooks builds: after `ready` the host lists the worker's open files and mappings (`/proc/<pid>/fd` and `maps`; `proc_pidinfo` on macOS; on Windows the job directory's delete, which a held handle makes fail). Any path under the job directory fails the job `WORKER_CRASHED {heldFiles}`.
37. **Pause** (spec §6.3):
    - `{job}` on a queued job → `paused` (it keeps its place);
    - on a running resumable job → `cancel {reason: pause}` with a 10 s grace for the checkpoint, then `paused` (killed after the grace; the last checkpoint stands);
    - a running non-resumable job finishes and is not paused;
    - `{all}`: dispatch stops, running resumable jobs pause as above, others finish, warm workers are kept until they expire;
    - `resume` puts a paused job back as `queued` with its original `createdAt`.

### Files, helpers and hooks

38. **Job directories** (spec §6.4). `<server-data>/jobs/` is made with `ensurePrivateDir`. Each job gets `<jobId>/` (0700, the parent's DACL on Windows) with `tmp/`, and each helper `helpers/<id>/tmp/`. A kind's `MappedFile {name, bytes?, mode}`:
    - `name` is relative, segments `[A-Za-z0-9._-]+`, no `..`, at most 4 deep;
    - a name ending in `/` is a directory (created 0700). A `write` or `readWrite` directory (`out/`) is **emptied before each attempt**;
    - a file **with** `bytes` is created 0600 at that size (zero-filled, `ftruncate` / `SetEndOfFile`) and recreated before each attempt when its mode is `write` or `readWrite`;
    - a file **without** `bytes` is written by the kind's `prepare` hook, and 5C checks it exists as a regular file with one link (no symlink, no reparse point) before `start`;
    - caps: 1 GiB per file, 4 GiB per job (`JOB_PARAMS_INVALID` above them). A creation error fails the attempt with the storage code (`IO_ERROR`), retried with backoff.

    5C never reads the files. The directory is deleted when the job ends (after `onSucceeded`), with Windows sharing-violation retries (20 × 25 ms). Leftovers are swept at start and hourly, keeping `phase: hooks` jobs' directories (Decision 28).
39. **Helpers** (spec §6.4): `HelperSpec {kind: codec|plugin, id, profile: worker|plugin, files}` comes from the kind (a list or a function of params); each `(kind, id)` has a registered `HelperProgram` that resolves its program and may serve its calls (`onCall`).
    - **Order:** create the directory and files → `prepare` → spawn each helper in the job's group → await its `hello` (10 s) → `attach` → spawn or reuse the worker → `start`.
    - A helper that exits or sends `error` before the worker's `result` fails the job with **`HELPER_FAILED {helper, code}`** and kills the job's groups.
    - After the `result`, helpers get `shutdown`, 2 s, then their group is killed.
40. **Kind hooks** (sub-project 6 §3.2 item 3; 6A plan Task 12):
    - **`prepare(ctx)`** runs once per job directory (the first attempt, and the first attempt after a restart), after 5C created the directory and files and before any process starts. It writes the files without `bytes` (`in/snapshot.json`, `in/manifest.json`) and may return `{extra}`, passed to the worker as `start.job.extra` and kept in `jobs.extra`.
    - **`onSucceeded(ctx)` is at-least-once and must be idempotent.** After the worker's `result`, 5C **first persists the result and `phase: 'hooks'`** in one `UPDATE`, then runs the hook:
      - it runs **after every process that could still write the directory has exited** (a cold job's whole group). For a warm job, the job's helper group has exited, while the warm worker, which sent `ready` and closed its files (Decision 6), stays alive; the hook still copies before it validates, as `fin-shm` requires;
      - it runs **before 5C deletes the directory**;
      - it may return a replacement result;
      - its budget is the kind's `hookTimeoutS` (60 s; past it, `JOB_TIMEOUT`); a failure fails the job with the hook's error, never retried;
      - a cancel during the hook is too late: the job ends `succeeded`;
      - if `finisherd` dies during the hook, the restart keeps the directory and runs the hook again (Decision 28) with `rerun: true`, so a hook that moved some outputs must tolerate finding them moved.
    - Both hooks are `Effect`s. Long work in them (copies, hashing, renames of large outputs) runs off the JS thread through fin-node, never as a blocking call on the event loop.
41. **Codes** (spec §6.9; exit classes in `crates/fin/src/output.rs`):
    - 3: `JOB_KIND_UNKNOWN`, `JOB_PARAMS_INVALID`, `JOB_NOT_FOUND`;
    - 4: `JOB_FAILED`, `JOB_CANCELLED`, `JOB_INTERRUPTED`, and the result codes `WORKER_CRASHED`, `WORKER_HUNG`, `JOB_STALLED`, `JOB_TIMEOUT`, `POISON_ITEM`, `HELPER_FAILED`;
    - 5: `JOB_QUEUE_FULL`, `WORKER_UNAVAILABLE`, `WORKER_PROTOCOL_MISMATCH`, `WAIT_TIMEOUT`, `RESOURCE_STUCK`, **`WORKER_OOM`** (5C-1 enforces the limit, so it owns the code; 6A's plan maps it to 5 too);
    - internal (only in `reply` errors): `UNKNOWN_MESSAGE`, `CALL_LIMIT`;
    - 3 also `JOBHOST_STATE`: a job host call on a group or host the caller should know is gone (`details.reason`: `unknownGroup`, `groupClosed`, `shutDown`), a caller's bug, never retried. (`JOBHOST_UNSUPPORTED` was removed with Task 5: every OS has a kill domain.)

### Events and plugins

42. **The sink** (spec §6.8): `JobEventSink {jobDone, workerExit, forward}`. **`job.done` is at least once** (9b review M4): a terminal row is marked announced only after the sink took its `job.done`; a sink that throws, or a crash in between, has it sent again at the next start, with the run statistics stored with the terminal row (`jobs.stats`, chain step 2). Consumers dedupe by the job id, which `jobDone(event, job)` passes beside the payload (the journal payload has no id). Until diagnostics D4 wires the journal into `finisherd`, the engine is given `memorySink()` (tests read it; `finisherd` keeps the last 1,000 and logs them at debug). D4 replaces it with a journal sink (a TASKS follow-up).
    - **`job.done`** (`kv` 1), for every job that reaches a terminal state: `kind`, `family`, `outcome: succeeded|failed|cancelled|interrupted`, `code?`, `attempts`, `priority`, `warm`, `queuedMs`, `runMicros`, `items?`, `itemsFailed?`, `isolated`, `helpers`, plus the kind's `journal(job)` extra fields (sub-project 6's `kv` 2).
    - **`worker.exit`**, for every process 5C started, the watchdog excepted: `role`, `family`, `helperKind?`, `pluginId?` (the sink hashes it into `plugin: Hashed`), `reason`, `exitCode?`, `signal?`, `jobsServed`, `lifetimeS`, `sandboxed`. `reason` is one of `done|idleExpired|recycled|cancelled|killed|crashed|hung|stalled|abandoned|spawnFailed|oom|protocolError`; **`oom` and `protocolError` are added** here (the kind is new, so no bump).
    - **Name tables**, in `fin-core/src/events/names_jobs.rs`, with a drift test that keeps the registered kinds and families inside them:
      - `JOB_KINDS`: `test.*` now; 5D, 6 and 8C add theirs;
      - `WORKER_FAMILIES`: `test`, `library`, `media`, `analyze`, `plugin`;
      - `SIGNALS`: lower case without `SIG` (`kill`, `term`, `int`, `hup`, `segv`, `bus`, `abrt`, `ill`, `fpe`, `pipe`, `xcpu`, `sys`, `trap`).
43. **The in-process bus:** `onDone(listener)` receives the `JobView` after the terminal row is written (sub-project 7's hooks attach here; 8's budgets read `owner` and `runMicros`). `events()` is a `Stream` of `{type: state|progress|item, job}` that 5C-2's `jobs.watch` will serve. A throwing listener is logged and never affects the job.
44. **Plugins** (sub-project 8 §3.3–3.7):
    - A job records `owner` (the invoker; `system` for in-process callers), `via {plugin}` and `rights` (the invoker ∩ grants snapshot, opaque here). They are passed to hooks, to `onCall` (`CallCtx`) and to the bus.
    - 5C-1 checks none of them: 5C-2 checks ownership, 8B checks rights in its `onCall` methods.
    - A family or helper of a plugin resolves to `Program::Path` with profile `plugin`, names its plugin id for `worker.exit`, and is versioned by Decision 3's minor rule.
    - Its warm processes are keyed by `warmKey` (Decision 35). It talks to the plugin host through `call`/`reply` (Decision 5a), so the plugin process speaks only this envelope; 8B's SDK implements it.

## Frozen interfaces

Once Tasks 1, 6, 7 and 9a–11 merge, these are frozen for 6A-2, 5D, 5C-2 and 8B/8C; a change needs a coordinator decision and a note in each consumer's plan.

```rust
// crates/fin-jobs (Task 1)
pub const PROTOCOL: u32 = 1;  pub const MINOR: u32 = 0;
pub const EXIT_HOST_GONE: i32 = 70;  pub const EXIT_SELF_SANDBOX: i32 = 78;
pub const EXIT_HOST_PROTOCOL: i32 = 65;   // the host sent a line the runtime cannot decode (batch A review, m3)
// Runtime exit codes: 0 done (cold) or shutdown; 65 bad host line; 70 host gone (stdin EOF); 78 self-sandbox failed;
// 101 caught panic (worker::EXIT_PANIC); a helper whose `run` fails exits 1.
pub fn envelope_id() -> &'static str;                               // Decision 3
pub enum HostMsg { Start(Start), Cancel { job: String, reason: CancelReason }, Shutdown {}, Attach(Attach),
    Reply { id: u64, value: Option<Value>, error: Option<JobError> } }
pub enum WorkerMsg { Hello(Hello), Begin { job: String, item: String },
    Item { job: String, item: String, value: Option<Value>, error: Option<JobError> },
    Progress(Progress), Checkpoint { job: String, data: Value }, Alive {}, Result { job: String, value: Value },
    Error { job: String, error: JobError, item_errors: Option<Vec<ItemError>> }, Ready {},
    Diag { job: Option<String>, kind: String, payload: Value },
    Call { id: u64, method: String, params: Value } }
pub struct Hello { pub v: u32, pub minor: u32, pub role: Role, pub family: String, pub kinds: Vec<KindVersion>, pub warm: bool,
    pub envelope: Option<String>, pub self_sandbox: Option<String>, pub pid: u32 }
pub struct Start { pub job: StartJob, pub limits: Limits, pub files: Vec<FileGrant>, pub helpers: Vec<HelperGrant>, pub dirs: Dirs, pub diagnostics: bool }
pub struct StartJob { pub id: String, pub kind: String, pub kind_version: u32, pub params: Value, pub checkpoint: Option<Value>, pub attempt: u32, pub extra: Option<Value> }
pub struct FileGrant { pub name: String, pub path: PathBuf, pub mode: FileMode, pub bytes: Option<u64> }   // FileMode: Read | Write | ReadWrite
pub struct JobError { pub code: String, pub message: String, pub details: Value, pub retry: Retry }      // Retry: Never | Backoff | Fresh
pub struct KindParams { pub kind: &'static str, pub version: u32,
    pub validate: fn(&Value) -> Result<Value, Vec<String>>, pub schema: fn() -> schemars::Schema }

pub mod sandbox {                                                   // Decision 9a (sub-project 7 registers)
    pub struct SelfSandbox { pub profile: String, pub args: Value }
    pub type ApplyFn = fn(&SelfSandbox) -> Result<(), String>;
    pub fn register(f: ApplyFn);                                    // once, in main, before run()
}
pub mod worker {
    pub trait Family: Send {
        fn name(&self) -> &'static str;
        fn kinds(&self) -> &'static [KindVersion];
        fn warm(&self) -> bool { false }
        fn run(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError>;
    }
    pub fn run(family: impl Family) -> !;                           // self-sandbox entry → crash-UI off → hello → loop
    impl JobCtx<'_> {
        pub fn file(&self, name: &str) -> Option<&FileGrant>;  pub fn files(&self) -> &[FileGrant];
        pub fn helpers(&self) -> &[HelperGrant];  pub fn dirs(&self) -> &Dirs;  pub fn limits(&self) -> &Limits;
        pub fn begin(&mut self, item: &str);  pub fn item(&mut self, item: &str, value: Option<Value>);
        pub fn item_failed(&mut self, item: &str, error: JobError);
        pub fn progress(&mut self, p: Progress);  pub fn checkpoint(&mut self, data: Value) -> Result<(), JobError>;
        pub fn cancelled(&self) -> Option<CancelReason>;  pub fn cancel_token(&self) -> CancelToken;   // Clone + Send
        pub fn reporter(&self) -> Reporter;                     // Clone + Send: progress/begin/item/call from other threads
        pub fn call(&self, method: &str, params: Value) -> Result<Value, JobError>;   // blocks this thread until the reply
        pub fn diagnostics(&self) -> bool;  pub fn diag(&mut self, kind: &'static str, payload: Value);
        pub fn read_budget(&self) -> Option<ReadBudget>;        // token bucket over limits.readBytesPerSec
    }
}
pub mod helper {
    pub trait Helper: Send { fn kind(&self) -> HelperKind; fn id(&self) -> &str;
        fn run(&mut self, attach: &Attach, ctx: &mut HelperCtx<'_>) -> Result<(), JobError>; }
    pub fn run(helper: impl Helper) -> !;
    impl HelperCtx<'_> { pub fn call(&self, method: &str, params: Value) -> Result<Value, JobError>; pub fn cancel_token(&self) -> CancelToken; }
}
```

```rust
// crates/fin-jobhost/src/spawner.rs (Task 3) — frozen for sub-project 7
pub enum Program { Bundled(String), Path(PathBuf) }
pub enum Profile { Worker, Plugin }
pub enum Role { Worker, Helper { kind: HelperKind, id: String } }
pub struct PathGrant { pub path: PathBuf, pub mode: FileMode }
pub struct SpawnRequest { pub role: Role, pub profile: Profile, pub family: String, pub program: Program, pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>, pub cwd: PathBuf, pub grants: Vec<PathGrant>, pub qos: Qos, pub label: String }
pub struct JobLimits { pub active_processes: Option<u32>, pub ui: Option<UiLimits> }   // UiLimits: Windows JOB_OBJECT_UILIMIT_* flags
pub struct PlatformCommand { pub program: PathBuf, pub args: Vec<OsString>, pub env: Vec<(OsString, OsString)>, pub cwd: PathBuf,
    pub job_limits: JobLimits, pub self_sandbox: Option<fin_jobs::sandbox::SelfSandbox>,
    #[cfg(target_os = "linux")] pub pre_exec: Vec<Box<dyn Fn() -> std::io::Result<()> + Send + Sync>>,   // no allocation inside
    #[cfg(windows)] pub attributes: Vec<WinAttribute>, #[cfg(windows)] pub token: Option<OwnedHandle> }
pub trait Spawner: Send + Sync {
    fn name(&self) -> &'static str;
    fn sandboxed(&self) -> bool;
    fn prepare(&self, req: &SpawnRequest, cmd: &mut PlatformCommand) -> Result<(), SpawnError>;
}
pub struct Unsandboxed;   // name "none", sandboxed false, prepare does nothing
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
  readonly resourceHealthy?: (job: JobView & { params: P }) => Effect.Effect<boolean>;   // asked before any POISON_ITEM
  readonly files?: OrFn<P, ReadonlyArray<MappedFile>>; readonly helpers?: OrFn<P, ReadonlyArray<HelperSpec>>;
  readonly memory?: OrFn<P, MemoryRule>;                         // per group (Decision 20)
  readonly prepare?: (ctx: PrepareCtx<P>) => Effect.Effect<{ readonly extra?: unknown } | void, FinError>;
  /** At least once; must be idempotent (Decision 40). */
  readonly onSucceeded?: (ctx: SucceededCtx<P, R>) => Effect.Effect<R | void, FinError>;
  readonly hookTimeoutS?: number;
  readonly journal?: (job: JobView) => Readonly<Record<string, unknown>>;   // extra job.done fields
}
export interface BatchSpec<P> { items(params: P): ReadonlyArray<string>; restrict(params: P, items: ReadonlyArray<string>): P;
  combine?(results: ReadonlyArray<unknown>, items: ReadonlyMap<string, ItemOutcome>): unknown }
export interface PrepareCtx<P> { readonly job: JobView & { params: P }; readonly dir: string; path(name: string): string; readonly attempt: number }
export interface SucceededCtx<P, R> extends PrepareCtx<P> { readonly result: R; readonly items: ReadonlyMap<string, ItemOutcome>; readonly rerun: boolean }
export type ItemOutcome = { readonly value?: unknown } | { readonly error: FinError };
export type ProgramSpec = { readonly binary: "fin-workers" | "fin-analyze"; readonly args: ReadonlyArray<string> }
                        | { readonly path: string; readonly args: ReadonlyArray<string> };   // path: plugins only
export interface CallCtx { readonly job: JobView; readonly role: "worker" | "helper"; readonly helper?: { kind: string; id: string };
  readonly owner: JobOwner; readonly via?: JobVia; readonly rights?: unknown }
export type OnCall = (ctx: CallCtx, method: string, params: unknown) => Effect.Effect<unknown, FinError>;
export interface FamilySpec {
  readonly family: string; readonly profile: "worker" | "plugin";
  readonly program: ProgramSpec | ((job: JobView) => ProgramSpec);
  readonly memory?: { readonly interactive?: MemoryRule; readonly batch?: MemoryRule };
  readonly warm?: { readonly slots?: number; readonly idleS?: number; readonly maxJobs?: number; readonly maxAgeS?: number };
  readonly warmKey?: (job: JobView) => string;                  // Decision 35
  readonly onCall?: OnCall;                                       // Decision 5a
  readonly diagKinds?: ReadonlyArray<string>; readonly pluginId?: string;
}
export interface HelperProgram { readonly kind: "codec" | "plugin"; readonly id: string; readonly profile: "worker" | "plugin";
  resolve(job: JobView): ProgramSpec; readonly onCall?: OnCall; readonly pluginId?: string }
export interface KindRegistry { registerFamily(f: FamilySpec): void; registerKind<P, R>(k: KindSpec<P, R>): void;
  registerHelper(h: HelperProgram): void; kind(name: string): KindSpec | undefined; family(name: string): FamilySpec | undefined;
  helper(kind: string, id: string): HelperProgram | undefined; kinds(): ReadonlyArray<KindSpec>;
  program(family: string, job: JobView, mode: SpawnMode): ProgramSpec }   // the family's argv, `--warm` appended for a warm spawn
export type SpawnMode = "cold" | "warm";  export const WARM_ARG = "--warm";
// Warmness comes from the engine (batch A review, M3): `FamilySpec.program` is the cold argv and never holds `--warm`;
// the engine spawns every process through `program(family, job, mode)`, which appends `--warm` for a warm spawn,
// `Program::Path` plugin processes included. `Family::warm()` (Rust) stays the family's declaration that it can serve warm.

// apps/finisherd/src/jobs/engine.ts (Tasks 9a–11)
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
  workers(): WorkersView;   // pools, warm workers, live processes, stuck pools, host caps (killTree, watchdog, leafLimits, sandboxed)
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

The napi `JobHost` (Task 6), `fin-jobhost`'s `Host` and the `KillDomain` trait (Task 3) are internal to 5C; only the spawner block above is frozen, for sub-project 7.

## Questions for the owner

None. The plan follows spec revision 3. The details it adds are engineering choices, recorded as Decisions and, in spec §6.10 and Task 13, as spec additions or deviations:
- the Unix watchdog and its remaining window (Decision 15);
- the `setsid` escape without cgroups, on macOS and Linux, after cancel and after host death (Decision 18, against spec §6.9);
- cgroups only for a cgroup `finisherd` owns alone (Decision 16);
- `fin-jobhost` as its own crate (Decision 1);
- `prepare` beside an at-least-once `onSucceeded` (Decision 40);
- `rememberPoison` and `resourceHealthy` (Decisions 27, 31);
- host calls, the minor-version rule and warm keys for plugins (Decisions 3, 5a, 35);
- leaf kill domains with `job_limits`, and the self-sandbox entry (Decisions 9, 9a, 17);
- `WORKER_OOM` as 5C's code, with per-group limits (Decision 20), and the `oom`/`protocolError` exit reasons.

## Review Focus

1. **Nothing runs outside its kill tree, and the tree dies with the host on every OS:** suspended → assigned → resumed on Windows and macOS; `pre_exec` before exec on Linux; the host Job's handle, PDEATHSIG and the watchdog; no stray handle or fd inherited; spawns only from the long-lived thread; leaders unreaped until `close_group`.
   → Task 3 `sigcont_resumes_start_suspended` (first), `pdeathsig_survives_spawning_thread_exit`, `only_stdio_is_inherited`, `leader_unreaped_until_close_group`; Task 4 `grandchild_gone_after_host_sigkill`, `hung_worker_gone_after_host_sigkill`, `late_minus_g_is_honoured`; Task 5 `process_is_in_its_leaf_job_before_it_runs`, `grandchild_gone_after_host_terminate`, `concurrent_libuv_spawn_inherits_no_pipe`; Task 12 `finisherd_kill_leaves_no_process`.
2. **Cancel kills the whole tree,** with the right grace, and a stuck process is abandoned rather than awaited forever. → Task 3 `cancel_kills_double_forked_grandchild`, `kill_on_closed_group_is_a_no_op`; Task 4 `abandon_after_kill_wait`; Task 5 `cancel_terminates_job_tree`; Task 9b `two_abandoned_mark_io_pool_stuck`.
3. **The JS thread never blocks; backpressure reaches the worker;** liveness is not fooled by a slow event loop. → Task 6 `spawn_and_kill_are_async`, `slow_js_backpressures_the_worker`, `slow_js_is_not_hung`, `progress_is_coalesced`.
4. **Exits never overtake messages.** → Task 3 `result_then_immediate_exit_is_success`.
5. **Liveness, stall and timeout are separate,** and resource failures are never bisected. → Task 3 timer tests; Task 10 `hang_is_never_bisected`, `sigbus_is_a_resource_fault`, `resource_unhealthy_never_poisons`.
6. **Isolation targets the in-flight item; acknowledged items never re-run; the budget holds and is reachable only by unexplained crashes.** → Task 10 `poison_item_isolated_in_two_runs`, `isolation_properties`, `isolation_budget_exhausted_by_unexplained_crashes`.
7. **Restart and close:** no `running` row in `phase: run` survives; resumable jobs resume from their checkpoint; `phase: hooks` jobs re-run their hook with their directory intact. → Task 9b `restart_requeues_resumable_from_checkpoint`, `restart_reruns_on_succeeded`, `close_leaves_no_running_rows`; Task 9a `job_dirs_swept_at_start`.
8. **Warm workers:** separate slots, direct hand-off within 5 ms under load, expiry and recycling (memory-driven too, replacement first), crash retry on a fresh worker, warm keys, no state and no open files between jobs. → Task 11.
9. **Plugins can live on the envelope:** host calls, the minor rule, warm keys. → Task 1 `call_reply_round_trip`; Task 3 `plugin_minor_unknown_type_gets_error_reply`; Task 11 `warm_key_separates_workers`.
10. **Memory means the same thing everywhere** (per group, the documented figures). → Task 4 `group_sum_over_limit_is_oom`; Task 5 `job_memory_limit_is_oom_confirmed_by_violation_info`.
11. **Workers get no credential, no inherited environment and no other process's handles; shipped families start no process; a requested self-sandbox fails closed.** → Task 3 `env_is_the_allow_list`, `self_sandbox_fails_closed`; Task 12 `check-worker-spawn` gate; Decision 22.
12. **The event loop:** statements p99 ≤ 1 ms with 10k jobs; batched deletes; no per-item write storms. → Task 12 budget.
13. **Files ≤ 600 lines; no GPL dependency; every commit green on three OSes.** → `pnpm gates` at every commit; Task 14.

---

### Task 1: `fin-jobs` and the job event kinds — tier: sonnet — batch A

**Files:**
- Create:
  - `crates/fin-jobs/Cargo.toml` (feature `test-hooks`: the test kinds' params);
  - `crates/fin-jobs/src/{lib.rs, msg_host.rs, msg_worker.rs, error.rs, codec.rs, budget.rs, kinds.rs, sandbox.rs, test_kinds.rs}`;
  - `crates/fin-jobs/src/worker/{mod.rs, ctx.rs, io.rs, calls.rs}` and `crates/fin-jobs/src/helper.rs`;
  - `crates/fin-core/src/events/payloads/jobs.rs` (`JobDone`, `WorkerExit`, Decision 42) and `crates/fin-core/src/events/names_jobs.rs`.
- Modify:
  - `Cargo.toml` (workspace member; default member);
  - `crates/fin-core/src/events/{kinds.rs (two registry lines, kv 1, sources [Finisherd]), names.rs (TABLES), payloads/mod.rs}`;
  - `crates/fin-store/examples/export_schema.rs` and its `Cargo.toml` dev-dependency (export the envelope, the test kinds' params and results, `JobDone` and `WorkerExit`);
  - `packages/core-types` (regenerated);
  - `TASKS.md` (the new 5C-1 section).
- Test: `crates/fin-jobs/tests/{envelope.rs, worker_runtime.rs, calls.rs}`; the fin-core kinds and name-table tests extended.

**Interfaces:** the `fin-jobs` block under **Frozen interfaces**, plus:
```rust
pub mod codec {
    pub const MAX_LINE: usize = 1 << 20;  pub const MAX_CHECKPOINT: usize = 256 << 10;
    pub fn encode<M: Serialize>(seq: u64, msg: &M) -> Result<Vec<u8>, CodecError>;          // adds v and seq, appends \n
    pub fn decode_worker(line: &[u8], expect_seq: u64, lenient_minor: bool) -> Result<Decoded<WorkerMsg>, CodecError>;
        // Decoded::Msg | Decoded::Unknown { t, id }   (only when lenient_minor; Decision 3)
    pub fn decode_host(line: &[u8], expect_seq: u64) -> Result<HostMsg, CodecError>;         // TooLong | Json | UnknownType | Seq{expected, got}
}
pub struct ReadBudget { /* token bucket, refills per ms */ }
impl ReadBudget { pub fn new(bytes_per_sec: u64) -> Self; pub fn take(&self, n: u64); /* blocks while empty */ }
#[cfg(feature = "test-hooks")] pub mod test_kinds { pub static PARAMS: &[KindParams]; /* SleepParams, EchoParams, CrashParams, … */ }
```
Decisions 3–9a, 42.

- [ ] **Step 1: Write failing tests:**
  - `envelope_round_trips_every_message` (serde both ways; field names camelCase as in spec §6.2), `envelope_id_is_stable_and_changes_with_the_schema` (a golden of the current id; a test-only extra field changes it);
  - `decode_refuses_long_lines_unknown_types_and_seq_gaps`, `lenient_decode_returns_unknown_with_id`, `checkpoint_over_cap_is_refused`;
  - `generated_ts_matches` (the drift gate covers it; the test asserts the schema names exist), `job_kinds_registered_at_v1`, `names_jobs_tables_valid`, `worker_exit_reasons_closed`;
  - the runtime, driven over in-memory pipes with a toy family:
    - `hello_first_then_result_then_exit_cold`, `warm_sends_ready_and_takes_the_next_start`;
    - `alive_every_interval_from_its_own_thread` (while `run` blocks);
    - `stdin_eof_exits_70` (in a child process: the test re-executes itself);
    - `cancel_sets_the_token`, `pause_reason_reaches_the_family`;
    - `panic_is_answered_as_worker_crashed_and_exits_101`;
    - `diag_is_a_no_op_when_off`;
    - `reporter_works_from_another_thread`;
    - `call_reply_round_trip`, `calls_from_two_threads_get_their_own_replies`, `call_error_reply_is_a_job_error`;
    - `helper_waits_for_attach_and_exits_on_shutdown`, `helper_can_call`;
    - `self_sandbox_entry_runs_before_hello` (a registered test function records the order), `self_sandbox_requested_but_unregistered_exits_78`;
    - `windows_crash_ui_is_off` (`GetErrorMode` reports the flags);
  - `read_budget_limits_rate` (10 MiB at 5 MiB/s takes ≈ 2 s ± 10 %, with an injected clock).
- [ ] **Step 2: Implement.** Each `msg_*.rs` ≤ 350 lines. The stdout writer is one `Mutex<BufWriter<Stdout>>` with the `seq` counter, flushed per message. The stdin reader thread decodes `HostMsg`, routes `reply` to the waiting caller by `id` and everything else to the run loop, and exits the process on EOF.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(fin-jobs,fin-core): job envelope, worker and helper runtimes, and the job event kinds`.

### Task 2: `fin-workers` with the `test` family and test helpers — tier: sonnet — after Task 1

**Files:**
- Create:
  - `crates/fin-workers/Cargo.toml` (bin `fin-workers`; feature `test-hooks` enables `fin-jobs/test-hooks` and the `test` family; a self dev-dependency enables it for its own tests);
  - `crates/fin-workers/src/{main.rs, families.rs}`;
  - `crates/fin-workers/src/test_family/{mod.rs, kinds.rs, batch.rs, helper.rs}` (`#[cfg(feature = "test-hooks")]`).
- Modify:
  - `Cargo.toml` (member and default member: like `fin`, only its self dev-dependency turns `test-hooks` on, so `scripts/check-test-hooks.mjs` keeps it out of the normal graph);
  - `packages/fin-node/package.json` (`build` and `build:test` also build `fin-workers` and copy it beside the `.node` file: `scripts/copy-workers.mjs`);
  - `.gitignore` (the copied binaries).
- Test: `crates/fin-workers/tests/family_test.rs`.

**Interfaces:**
```text
fin-workers <family>                  run a family's worker (stdin/stdout envelope)
fin-workers --helper <kind>:<id>      run a helper (test builds: test:ok|exitEarly|hang|spawn|call)
fin-workers watchdog                  Task 4 (Unix)
fin-workers --version                 prints the envelope id
```
Test kinds (params types in `fin_jobs::test_kinds`, Rust → generated TS):
- `test.sleep {ms, progressEveryMs?}`; `test.echo {value}`; `test.progressFlood {n}` (n `progress` messages as fast as it can);
- `test.warm {value, delayMs?, holdFile?}` (warm; result `{value, pid, served}` so tests see reuse; `holdFile` keeps a job-directory file open past `ready`, to break Decision 6 on purpose);
- `test.crash {items: string[], poison?: string[], how: abort|segv|bus|fastfail|exit, between?: bool, crashOnSecond?: bool}` (batch: `begin`/`item` per item). It crashes inside a poison item, between items when `between`, or on the second item of any run with two or more items when `crashOnSecond` (a crash no single item explains). `bus` raises `SIGBUS` / `EXCEPTION_IN_PAGE_ERROR`; `fastfail` is Windows `__fastfail`;
- `test.hang {}` (stops `alive` and blocks); `test.stall {}` (keeps `alive`, never reports progress);
- `test.spawn {detach: none|setsid, out}` (**breaks the no-process rule on purpose**: double-forks a sleeping grandchild, writes both pids to the mapped file `out`, then sleeps);
- `test.files {}` (writes a pattern into every `write` file, reads every `read` file, returns their sizes and hashes; reports whether `out/` was empty at start);
- `test.checkpoint {steps, crashAt?}` (resumable: a checkpoint per step; crashes once at `crashAt`);
- `test.oom {mib}` (allocates and touches `mib` MiB);
- `test.call {method, params}` (one host call; returns the reply);
- `test.diag {}` (sends one `diag` of a test kind);
- `test.garbage {what: json|seq|long|otherJob|unknownType}` (protocol violations);
- `test.exitAfterResult {}` (writes `result` and calls `exit(0)` in the same instant);
- `test.badHello {}`, registered under the separate family `testbad`, whose `hello` claims another envelope id; `test.pluginMinor {}`, under family `testplugin`, run as `Program::Path` with profile `plugin`: `hello` with `minor: 1` and no `envelope`, then an unknown `t` with an `id`.

- [ ] **Step 1: Write failing tests** (`family_test.rs`, through the binary with a minimal in-test host over pipes, Unix and Windows):
  - every kind's happy path; `hello_lists_the_test_kinds`;
  - `crash_kinds_die_as_asked` (exit status / signal / NTSTATUS);
  - `garbage_kinds_emit_what_they_say`;
  - `helper_ok_attaches_and_exits_on_shutdown`, `helper_exit_early_exits_nonzero`;
  - `version_prints_the_envelope_id`, `unknown_family_exits_2`.

  The release build's lack of the `test` family is held by `scripts/check-test-hooks.mjs` (no `test-hooks` in the normal graph) and checked by hand in Task 14.
- [ ] **Step 2: Implement.** The family table in `families.rs` is `#[cfg]`-gated; `main.rs` only parses arguments and dispatches.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(fin-workers): the worker binary with the test family`.

### Task 3: `fin-jobhost`: the kill-domain trait, Unix spawning, framing, timers and cancel — tier: opus — after Task 2

**Files:**
- Create:
  - `crates/fin-jobhost/Cargo.toml`;
  - `crates/fin-jobhost/src/{lib.rs, host.rs, spawner.rs, proc.rs, writer.rs, framing.rs, timers.rs, domain.rs, exit.rs, memory.rs}` (`domain.rs` holds the `KillDomain` trait and its test double);
  - `crates/fin-jobhost/src/unix/{mod.rs, linux_spawn.rs, macos_spawn.rs, pgroup.rs}` (the process-group `KillDomain`);
  - `crates/fin-jobhost/src/windows/mod.rs` (a `KillDomain` stub answering `JOBHOST_UNSUPPORTED` until Task 5).
- Modify: `Cargo.toml` (member, default member), `crates/fin-workers/Cargo.toml` (dev-dependency on `fin-jobhost`).
- Test: `crates/fin-workers/tests/{host_support.rs, host_unix.rs, host_common.rs}`.

**Interfaces:**
```rust
pub struct Timing { pub hello_ms: u32, pub hung_ms: u32, pub kill_wait_ms: u32, pub rss_sample_ms: u32, pub exit_drain_ms: u32 }   // 10 000, 30 000, 5 000, 1 000, 200
pub struct HostOptions { pub binary_dir: PathBuf, pub timing: Timing, pub spawner: Arc<dyn Spawner>, pub cgroups: CgroupMode, pub watchdog: bool,
                         pub host_minor: u32 }
pub struct HostCaps { pub kill_tree: KillTree /* JobObject | Cgroup | ProcessGroup */, pub watchdog: bool, pub cgroup_memory: bool,
                      pub leaf_limits: bool, pub sandbox: &'static str, pub sandboxed: bool }
pub enum HostEvent { Hello { proc: ProcId, hello: Hello },
                     Message { proc: ProcId, msg: WorkerMsg, memory: Option<u64> /* on `ready` only */ },  // `alive` is consumed here
                     Stderr { proc: ProcId, line: String }, Exit { proc: ProcId, info: ExitInfo },        // Exit after the drain (Decision 19a)
                     GroupEmpty { group: GroupId } }
pub struct ExitInfo { pub reason: ExitReason, pub code: Option<i64>, pub signal: Option<&'static str>, pub foreign: Option<u32>,
                      pub memory_peak: Option<u64>, pub stderr_tail: String, pub lifetime_ms: u64 }
pub enum ExitReason { Exited, Killed(KillReason), Hung, Stalled, TimedOut, Oom, ResourceFault, ProtocolError(String),
                      ProtocolMismatch { expected: String, got: String }, NoHello, SpawnFailed(String), Abandoned }
pub enum KillReason { Cancel, Shutdown, Recycle, IdleExpired, HelperFailed, JobEnded, Hung, Test }
pub struct GroupSpec { pub label: String, pub memory_bytes: Option<u64>, pub qos: Qos }
pub struct Watch { pub job: String, pub stall_ms: Option<u32>, pub timeout_ms: Option<u32> }

impl Host {
    pub fn start(opts: HostOptions, events: EventSink) -> Result<Host, HostError>;   // EventSink: bounded, blocking (Decision 11a)
    pub fn caps(&self) -> HostCaps;
    pub fn open_group(&self, spec: GroupSpec) -> GroupId;                         // allocates only
    pub fn spawn(&self, group: GroupId, req: SpawnRequest, done: Box<dyn FnOnce(Result<ProcId, HostError>) + Send>); // on the spawner thread
    pub fn send(&self, proc: ProcId, msg: &HostMsg) -> Result<(), SendError>;     // enqueue; QueueFull kills the process as hung
    pub fn watch(&self, proc: ProcId, w: Option<Watch>);
    pub fn close_stdin(&self, proc: ProcId);
    pub fn kill_group(&self, group: GroupId, reason: KillReason, done: Box<dyn FnOnce() + Send>);  // no-op once closed
    pub fn close_group(&self, group: GroupId);                                    // reaps the leader (Decision 17)
    pub fn shutdown(&self, grace_ms: u32, done: Box<dyn FnOnce() + Send>);
}

// domain.rs — per-OS, internal; Tasks 4 and 5 implement it independently
pub(crate) trait KillDomain: Send + Sync {
    fn open_group(&self, id: GroupId, spec: &GroupSpec) -> io::Result<GroupHandle>;
    fn open_leaf(&self, group: &GroupHandle, limits: &JobLimits) -> io::Result<LeafHandle>;
    fn place(&self, group: &GroupHandle, leaf: &LeafHandle, p: &mut Placement) -> io::Result<()>;   // pre-exec fds / assign the suspended process
    fn kill(&self, group: &GroupHandle) -> io::Result<()>;
    fn memory(&self, group: &GroupHandle) -> Option<u64>;
    fn foreign(&self, group: &GroupHandle, spawned: u32) -> Option<u32>;
    fn exit_facts(&self, group: &GroupHandle) -> ExitFacts;                      // oom?, totalProcesses?
    fn close_group(&self, group: GroupHandle);
}
```
The `Spawner` block under **Frozen interfaces**. Decisions 3, 4, 9–13, 17–19a, 21, 23. Files: `host.rs` ≤ 450 lines (group and process tables, event dispatch), `proc.rs` the reader and waiter threads, `writer.rs` the bounded per-process writer, `timers.rs` one timer thread with a binary heap.

- [ ] **Step 0 (macOS, before anything else):** `sigcont_resumes_start_suspended`: a process made with `POSIX_SPAWN_START_SUSPENDED` writes nothing until `kill(pid, SIGCONT)`, then runs. If it fails, implement Decision 13's fallback and note it in TASKS before going on.
- [ ] **Step 1: Write failing tests** (Unix now; Task 5 makes `host_common.rs` run on Windows):
  - `host_common.rs`:
    - `spawn_hello_start_result_exit`, `bundled_program_resolves_only_known_names`;
    - `env_is_the_allow_list` (a `test.echo` of its own environment: only `childEnv` keys and `TMPDIR`);
    - `protocol_errors_kill_the_process` (each `test.garbage` variant → `ProtocolError`), `protocol_mismatch_on_bad_hello`, `plugin_minor_unknown_type_gets_error_reply` (`testplugin`: the process lives and receives `reply {error: UNKNOWN_MESSAGE}`), `bundled_unknown_type_is_a_protocol_error`;
    - `no_hello_times_out`, `hung_is_detected` (`test.hang`, `hung_ms` 300), `stall_is_detected` (`test.stall`, `stall_ms` 300), `timeout_is_detected`;
    - `result_then_immediate_exit_is_success` (`test.exitAfterResult`, 1,000 runs: the `result` message always precedes `Exit`, and `Exit` is `Exited` code 0);
    - `cancel_grace_then_kill` (a family that ignores `cancel` dies after the grace; one that answers keeps its exit `Exited`);
    - `writer_queue_full_kills_as_hung` (a test family that stops reading stdin while the host sends 300 messages);
    - `stderr_tail_is_kept_and_capped`, `exit_reports_code_and_signal`;
    - `spawner_prepare_can_rewrite_args` (a test `Spawner` adds an argument the test family echoes), `unsandboxed_caps`;
    - `self_sandbox_fails_closed` (a test `Spawner` requests `self_sandbox` on a binary with no registered function: `SpawnFailed("selfSandbox")`), `self_sandbox_mismatch_is_protocol_error`;
  - `host_unix.rs`:
    - `processes_share_the_group_pgid`;
    - `cancel_kills_double_forked_grandchild` (`test.spawn` with `detach: none`: both pids gone after `kill_group`);
    - `leader_unreaped_until_close_group` (after the leader exits, `kill(-pgid, 0)` still succeeds on the zombie group until `close_group`), `kill_on_closed_group_is_a_no_op`;
    - `pdeathsig_set_on_linux` (a test helper reports `prctl(PR_GET_PDEATHSIG)`), `pdeathsig_survives_spawning_thread_exit` (spawn from a thread that then exits: the worker lives);
    - `pre_exec_does_not_allocate` (Linux: the spawn runs with a test allocator that aborts on allocation in the child between fork and exec);
    - `eintr_is_retried` (a host thread taking a signal storm with `SA_RESTART` off still spawns and reaps correctly);
    - `only_stdio_is_inherited` (the worker lists `/proc/self/fd` or `/dev/fd`: 0, 1, 2 only, while the host holds 50 extra open fds);
    - `qos_nice_applied` (Linux nice 5 / 10; macOS QoS class read back with `proc_pidinfo`);
    - `macos_spawn_is_suspended_until_registered` (a test hook delays the resume: the process has not sent `hello`).
- [ ] **Step 2: Implement** per Decisions 11–13, 17, 19a. Non-leader exits are reaped with `waitpid(pid)` (never `-1`), and leaders with `waitid(… WNOWAIT)` then `waitpid` at `close_group`, so `finisherd`'s own `child_process` children are untouched.
- [ ] **Step 3: Run** the gates (Linux and macOS; Windows compiles the stub) → PASS. **Commit** `feat(fin-jobhost): kill domains, spawning in process groups, envelope framing, liveness and cancel`.

### Task 4: Unix host death, cgroups, memory limits, foreign processes, abandonment — tier: opus — after Task 3, beside Task 5

**Files:**
- Create: `crates/fin-workers/src/watchdog.rs` (Unix), `crates/fin-jobhost/src/unix/{watchdog.rs (the client), memory.rs, foreign.rs}`, `crates/fin-jobhost/src/linux/cgroup.rs` (the cgroup `KillDomain`).
- Modify: `crates/fin-jobhost/src/{host.rs (domain selection only), memory.rs}`, `crates/fin-jobhost/src/unix/pgroup.rs`, `crates/fin-workers/src/main.rs`.
- Test: `crates/fin-workers/tests/{host_death_unix.rs, host_cgroup.rs, host_memory.rs}`.

**Interfaces:**
```text
watchdog stdin:  "+g <pgid>\n" "-g <pgid>\n" "+c <cgroup dir>\n" "-c <cgroup dir>\n"
                 parent exit or EOF → read stdin to EOF → SIGKILL every listed group, write "1" to every cgroup.kill, exit 0
```
```rust
pub enum CgroupMode { Auto, Off }
pub(crate) fn detect_cgroup() -> Option<CgroupRoot>;   // Decision 16's three conditions; None with a logged reason
pub(crate) fn sweep_leftover_groups(root: &CgroupRoot) -> io::Result<usize>;
```
Decisions 15, 16, 18, 20, 22. Only Task 3's `KillDomain` is touched from outside these files.

- [ ] **Step 1: Write failing tests:**
  - `host_death_unix.rs` (the host is a re-executed test binary, killed with `SIGKILL`):
    - `grandchild_gone_after_host_sigkill` (`test.spawn`, `detach: none`), `setsid_grandchild_escape_is_documented` (`detach: setsid`: gone in cgroup mode; otherwise the test asserts it survives and prints the documented gap, so a later fix shows up as a test change);
    - `hung_worker_gone_after_host_sigkill` (`test.hang` ignores stdin: the watchdog kills it);
    - `late_minus_g_is_honoured` (the host writes `-g` and dies before the watchdog reads it; a stranger process given that pgid number by a test hook is not signalled);
    - `worker_exits_on_stdin_eof` (watchdog disabled: a `test.sleep` worker exits 70);
    - `watchdog_ignores_sigint_and_sigterm`, `watchdog_restarts_and_resends` (kill the watchdog: the host restarts it, and a later host kill still cleans up), `watchdog_is_not_reported_as_worker_exit`;
  - `host_cgroup.rs` (skipped with a notice unless Decision 16's conditions hold; the Linux VM runs it under `systemd-run --user -p Delegate=yes --wait`):
    - `delegation_detected_only_when_alone`, `host_moves_into_host_leaf`;
    - `process_gets_its_leaf_before_exec` (the worker reads `/proc/self/cgroup` in its first instruction of `main`);
    - `cgroup_kill_takes_setsid_grandchild`;
    - `group_memory_max_and_oom_reported`, `leaf_pids_max_from_job_limits`;
    - `leftover_groups_swept_at_start`;
    - `terminal_scope_is_not_used` (a cgroup with a second process → process groups);
  - `host_memory.rs`:
    - `group_sum_over_limit_is_oom` (a worker and a helper of 48 MiB each under a 64 MiB group limit, sampled: `Oom` within 2 s), `under_limit_runs`;
    - `sampled_figure_is_anon_plus_shmem` (Linux: a worker mapping a 256 MiB file read-only is not counted), `macos_uses_phys_footprint`;
    - `memory_rule_resolution` (unit: fixed, fraction, cap);
  - `foreign_processes_are_counted` (`test.spawn` → `foreign == 1`; `test.echo` → 0);
  - `abandon_after_kill_wait` (a test hook makes the waiter report a process still alive after the kill: `Abandoned` once, the later exit logged). Marking a pool stuck is the engine's (Task 9b).
- [ ] **Step 2: Implement.** The watchdog is ≤ 200 lines; `cgroup.rs` ≤ 400.
- [ ] **Step 3: Run** the gates on Linux and macOS → PASS. **Commit** `feat(fin-jobhost): host watchdog, delegated cgroups and per-group memory limits on Unix`.

### Task 5: Windows: host, group and leaf Job Objects, suspended spawns — tier: opus — after Task 3, beside Task 4

**Files:**
- Create: `crates/fin-jobhost/src/windows/{job.rs (the Job `KillDomain`), spawn.rs, iocp.rs, env_block.rs, inherit.rs}`.
- Modify: `crates/fin-jobhost/src/windows/mod.rs` (replace the stub), `crates/fin-jobhost/Cargo.toml` (`windows-sys` features of the Tech Stack), `crates/fin-workers/tests/host_common.rs` (runs on Windows now).
- Test: `crates/fin-workers/tests/host_windows.rs` (`#[cfg(windows)]`), `crates/fin-workers/tests/host_death_windows.rs`.

**Interfaces:** `Host` as in Task 3; `KillTree::JobObject`; internal:
```rust
pub(crate) struct HostJob { job: OwnedHandle, port: OwnedHandle, thread: JoinHandle<()> }      // the port is advisory
pub(crate) struct GroupJob { job: OwnedHandle, key: usize }   pub(crate) struct LeafJob { job: OwnedHandle }
pub(crate) fn spawn_suspended(cmd: &PlatformCommand, stdio: ChildStdio, flags: u32) -> io::Result<SuspendedProcess>;  // inherit.rs flips the flags around this call only
impl SuspendedProcess { fn assign(&self, host: &HostJob, group: &GroupJob, leaf: &LeafJob) -> io::Result<()>; fn resume(self) -> io::Result<RunningProcess>; }
```
The peak memory and limit violations come from `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` and `JobObjectLimitViolationInformation` of the group Job; no sampling on Windows. Decisions 10, 14, 17, 18, 20–23.

- [ ] **Step 1: Write failing tests** (a non-elevated Windows 11 account):
  - `process_is_in_its_leaf_job_before_it_runs` (`IsProcessInJob` true for host, group and leaf while suspended; a test hook pauses between assign and resume);
  - `nesting_works_inside_an_outer_job` (the test host first puts itself into a Job, as a terminal or Electron may);
  - `leaf_active_process_limit` (`job_limits.active_processes = 1`: `test.spawn` cannot create its child), `leaf_ui_limits_applied`;
  - `cancel_terminates_job_tree` (`test.spawn`: worker and grandchild gone after `kill_group`);
  - `grandchild_gone_after_host_terminate` (the re-executed host killed with `TerminateProcess`: everything in its jobs gone);
  - `breakaway_is_refused` (a test child asking `CREATE_BREAKAWAY_FROM_JOB` fails to create it);
  - `unhandled_exception_dies_without_wer` (`test.crash` `how: segv` exits within 2 s, no dialog), `fastfail_dies_without_wer` (`how: fastfail`: exit `0xC0000409` within 2 s and no `WerFault.exe` naming the worker's pid within 5 s);
  - `only_stdio_is_inherited` (a host holding 50 inheritable handles: the worker can open none by value), `concurrent_libuv_spawn_inherits_no_pipe` (a thread spawning `cmd /c` children in a loop with `bInheritHandles` while the spawn thread runs 200 spawns: no child holds a job pipe, and every worker sees stdin EOF when its host closes it);
  - `exits_come_from_process_handles` (with the completion port's thread stopped by a test hook, every exit is still reported);
  - `env_block_is_explicit` (no parent variable leaks; `Path` vs `PATH` casing kept);
  - `job_memory_limit_is_oom_confirmed_by_violation_info` (`test.oom` under the group limit → `Oom`, also with the port thread stopped);
  - `foreign_processes_counted_from_total_processes`;
  - `priority_class_applied` (`GetPriorityClass` = `BELOW_NORMAL` for utility and background);
  - `spawn_failure_leaves_no_suspended_process` (an assign forced to fail by a test hook: the process is terminated, `SpawnFailed`);
  - `host_common.rs` passes on Windows.
- [ ] **Step 2: Implement.** UTF-16 command-line quoting follows the documented `CommandLineToArgvW` rules (a unit test round-trips 50 awkward arguments); `job.rs` and `spawn.rs` ≤ 400 lines each.
- [ ] **Step 3: Run** the gates on Windows and Linux → PASS. **Commit** `feat(fin-jobhost): kill-on-close Job Objects with leaf jobs and suspended spawns on Windows`.

**As built (Task 5, with the review of the Task 4/5 merge):**
- **Spawn broker** (Decision 10's deviation, above): `windows/broker.rs`; `inherit.rs` is gone. Measured on Windows 11 (release build, suspended): `fin-workers.exe` 0.4 MB commit, 2.0 MB working set, 1.2–1.4 ms to create (31 ms cold); `cmd.exe` 1.5 MB, 2.3 MB, ≈ 1 ms (5 ms cold); a copy of an Electron app's own executable would cost 7.7–8.8 MB commit and 24–77 ms cold. Tests (`host_broker_windows.rs`): `no_broker_refuses_spawns`, `broker_is_the_bundled_binary_and_closed_to_other_processes` (as the same user without `SeDebugPrivilege`, every process right and the thread's resume are denied), `broker_falls_back_to_cmd`, `broker_entry_exits_at_once`.
- **The Job the host runs in:** with `SILENT_BREAKAWAY_OK` the broker breaks away, so processes are born outside it and still die with the host (`silent_breakaway_outer_job_is_harmless`). An active-process limit counts the host, the broker (made at start, so its slot is taken first) and each process (console hosts are not counted); a full Job refuses as `WORKER_UNAVAILABLE {reason: processLimit}` (`ERROR_NOT_ENOUGH_QUOTA`), and a Job with room for the host alone gives `noBroker` (`outer_active_process_limit_refuses_as_process_limit`). Its memory limit is read through a null Job handle at start; an abnormal exit after a new outer peak above that limit is `Oom` (`WORKER_OOM`), not a crash, without ending the group (`outer_memory_limit_is_oom_not_a_crash`; best effort: only the innermost outer Job is visible).
- **`close_group` ends what members left on every OS:** `TerminateJobObject` (Windows), cgroup kill (Linux), and now `kill(-pgid)` for process groups before `-g` (the leader is still unreaped) (`host_common` `close_group_ends_what_a_member_left`).
- **Test hooks:** `HostOptions::{broker_images_for_test, after_send_for_test, fail_waiter_thread_for_test}`, `Host::broker_for_test`; `an_answer_before_send_returns_is_on_its_job` (deterministic: `send` is held until the reader has handled the answer; the old order fails it every run) and `a_missing_waiter_thread_still_reports_the_exit` (both OSes).

### Task 6: The napi `JobHost`: async calls, backpressure, coalescing; params validation; codes — tier: opus — after Task 3 (beside 4 and 5)

**Files:**
- Create: `crates/fin-node/src/jobhost/{mod.rs, napi_types.rs, events.rs, params.rs}`.
- Modify:
  - `crates/fin-node/{Cargo.toml (fin-jobhost; fin-jobs with test-hooks under fin-node's test-hooks), src/lib.rs}`;
  - `packages/fin-node/{index.d.ts, loader.js, loader.d.ts}` (`workersBinaryPath(dir?)`);
  - `crates/fin/src/output.rs` and the `packages/contracts` code list (Decision 41).
- Test: `packages/fin-node/test/jobhost.test.ts`.

**Interfaces:**
```ts
// packages/fin-node/index.d.ts
export interface JobHostOptions { binaryDir: string; timing?: Partial<{ helloMs: number; hungMs: number; killWaitMs: number; rssSampleMs: number; exitDrainMs: number }>;
  cgroups?: "auto" | "off"; watchdog?: boolean; eventQueue?: number /* 1024 */ }
export type JobHostEvent =   // generated from fin-jobhost's `wire` (as are JobHostOptions, JobSpawnRequest, JobHostExitInfo, JobHostCaps)
  | { type: "spawned"; proc: number; group: number }
  | { type: "hello"; proc: number; hello: Hello } | { type: "message"; proc: number; msg: WorkerMsg; memory?: number }
  | { type: "stderr"; proc: number; line: string } | { type: "exit"; proc: number; info: ExitInfo }
  | { type: "groupEmpty"; group: number };
export class JobHost {
  static start(opts: JobHostOptions, onEvent: (e: JobHostEvent) => void): JobHost;    // JOBHOST_UNSUPPORTED before Task 5 on win32
  caps(): { killTree: "jobObject" | "cgroup" | "processGroup"; watchdog: boolean; cgroupMemory: boolean; leafLimits: boolean; sandbox: string; sandboxed: boolean };
  openGroup(spec: { label: string; memoryBytes?: number; qos: "normal" | "utility" | "background" }): number;   // allocates only
  spawn(group: number, req: JobSpawnRequest): Promise<number>;                  // req.program: {bundled: name} | {path}
  send(proc: number, msg: HostMsg): "ok" | "full" | "gone";                    // enqueue; full: being killed as hung; the cause comes with `exit`
  watch(proc: number, w: { job: string; stallMs?: number; timeoutMs?: number } | null): void;
  closeStdin(proc: number): void;
  killGroup(group: number, reason: "cancel" | "shutdown" | "recycle" | "idleExpired" | "helperFailed" | "jobEnded" | "hung" | "test"): Promise<void>;
  closeGroup(group: number): void;                                              // queues the reap on the spawner thread
  shutdown(graceMs: number): Promise<void>;
  /** test-hooks builds only */ killProcForTest?(proc: number): void; abandonForTest?(proc: number): void; pauseEventsForTest?(ms: number): void;
}
export function validateJobParams(kind: string, version: number, params: unknown): { ok: true; params: unknown } | { ok: false; errors: string[] };
export function jobKindVersions(): Array<{ kind: string; version: number }>;     // what validateJobParams knows
```
```rust
// crates/fin-node/src/jobhost/params.rs: one static list, families append theirs:
static PARAMS: &[&[KindParams]] = &[#[cfg(feature = "test-hooks")] fin_jobs::test_kinds::PARAMS /* , fin_media::jobs::PARAMS (6A-2) */];
```
Events go through one bounded `ThreadsafeFunction` called with `ThreadsafeFunctionCallMode::Blocking` from the reader threads. Messages are converted with `serde_json::Value` → JS on the JS thread. Promises are settled through their own non-blocking `ThreadsafeFunction`. Decisions 2, 11, 11a, 27, 41.

- [ ] **Step 1: Write failing tests** (`jobhost.test.ts`, test-hooks build):
  - `starts_and_reports_caps`, `spawns_test_echo_through_napi`, `events_arrive_in_order_per_process`;
  - `spawn_and_kill_are_async` (an event-loop lag monitor shows no turn over 5 ms while 20 spawns and kills run);
  - `slow_js_backpressures_the_worker` (`pauseEventsForTest(2000)` during `test.progressFlood`: the host's memory stays bounded and the worker's writes block, measured by its own progress counter);
  - `slow_js_is_not_hung` (`hungMs` 300 and events paused for 2 s: no `WORKER_HUNG`);
  - `progress_is_coalesced` (`test.progressFlood` of 100,000: ≤ 4 progress events per second reach JS, and the last value arrives before `result`);
  - `kill_group_from_js`, `validate_params_accepts_and_rejects` (errors name the field), `workers_binary_path_finds_the_test_binary`;
  - `node_child_process_exits_are_not_stolen` (100 `child_process.spawn` children run beside 20 jobhost processes: every exit seen by its owner);
  - `shutdown_resolves_after_all_exits`.
- [ ] **Step 2: Implement.** `mod.rs` ≤ 450 lines (the napi class); `events.rs` the TSFN and coalescing; `napi_types.rs` the conversions.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(fin-node): job host binding with async spawns and kills, backpressure and params validation`.

### Task 7: Contracts, the `jobs` chain, the store, kinds and families, the test kinds, the event sink — tier: sonnet — after Task 1, beside Tasks 2–6

**Files:**
- Create:
  - `packages/contracts/src/jobs.ts`;
  - `apps/finisherd/src/control/migrations/jobs/0001.ts`;
  - `apps/finisherd/src/jobs/{store.ts, kinds.ts, events.ts}`;
  - `apps/finisherd/src/jobs/kinds/test.ts` (the `test`, `testbad` and `testplugin` families and kinds; registered only when the engine is built with `testKinds: true`).
- Modify: `packages/contracts/src/index.ts`, `TASKS.md`.
- Test: `apps/finisherd/test/jobs/{store.test.ts, kinds.test.ts, events.test.ts}`.

Needs only Task 1 (the generated `JobDone`, `WorkerExit` and test-kind types). `test_kinds_match_rust_params` runs against fin-node's `validateJobParams` and so is skipped with a notice until Task 6 is in; Task 9a removes the skip.

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
  enterHooks(id: string, result: unknown): JobView;                         // result + phase 'hooks', state stays running (Decision 40)
  checkpoint(id: string, data: unknown): void;                              // the caller throttles (Decision 26)
  items(id: string): Map<string, ItemOutcome>;  writeItems(id: string, items: ReadonlyArray<[string, ItemOutcome]>): void;
  list(q: JobQuery): { jobs: JobView[]; next?: string };
  unfinishedCount(): number;
  running(): JobView[];  inHooks(): JobView[];  queued(): JobView[];  paused(): JobView[];
  retention(now: number): Effect.Effect<number>;                            // Decision 30
}
```
The test kinds have one `KindSpec` per Task 2 kind:
- `test.crash` is a batch kind; `test.checkpoint` is resumable;
- `test.warm` is `{warm: true, defaultPriority: "interactive"}`;
- `test.files` declares a `read` file without `bytes` (written by its `prepare`), a `write` file with `bytes` and an `out/` directory;
- `test.helper` declares `HelperSpec {kind: "codec", id: "test", profile: "worker"}`;
- the `test` family has an `onCall` serving `test.echo`, `test.fail` and `test.slow`, and `testplugin` a `warmKey` from its params.

Decisions 25–27, 38, 42.

- [ ] **Step 1: Write failing tests:**
  - `jobs_chain_registers_and_migrates` (with 5A's `core`; `formatVersion.jobs` = 1; a fresh and an existing database; the `phase` column defaults to `run`);
  - `transitions_are_legal_only`, `enter_hooks_keeps_running_and_persists_the_result`;
  - `idem_per_owner`, `dedupe_active_only`, `poison_memory`;
  - `items_batch_write_and_read`, `list_cursor_and_filters`;
  - `retention_deletes_items_first_in_batches` (10k jobs × 4 items: ≥ 80 statements, yields between them);
  - `registry_refuses_duplicate_kinds_and_unknown_families`, `mapped_file_names_validated` (`..`, absolute, too deep, bad characters);
  - `test_kinds_match_rust_params` (every test `KindSpec` has a `validateJobParams` entry at its version);
  - `memory_sink_caps_and_records`.
- [ ] **Step 2: Implement.** `store.ts` ≤ 500 lines (split `store-queries.ts` for `list` if needed).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): job store in the jobs chain, kind registry and event sink`.

### Task 8: Pools and the scheduler — tier: sonnet — after Task 7, beside Tasks 2–6

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
  warm?: { family: string; key: string } }
export interface SchedState { queue: QueueIndex; pools: Pools;
  warm: ReadonlyMap<string /* family */, { slots: number; workers: ReadonlyArray<{ key: string; state: "spawning" | "ready" | "busy"; idleSince?: number }> }> }
export type Dispatch = { job: string; mode: "cold" } | { job: string; mode: "warm"; family: string; key: string }
  | { mode: "spawnWarm"; family: string; key: string; retire?: number /* worker to shut down first */ };
export function schedule(state: SchedState, now: number): Dispatch[];    // pure; the caller applies the takes
export class QueueIndex { add(j: QueuedJob): void; remove(id: string): void; size(): number }
```
`schedule()` serves queued warm jobs (no ready worker at submit) and cold jobs; the direct hand-off at submit (Decision 35) does not go through it. Decisions 33–35.

- [ ] **Step 1: Write failing tests** (fast-check, 1,000 runs each, seeds printed on failure):
  - `pools_never_over_committed` (cold dispatches never exceed any pool; warm jobs over-run `cpu` by at most the interactive slots);
  - `priority_then_fifo`, `aging_raises_one_level_per_10_minutes`;
  - `blocked_head_does_not_hide_runnable_jobs`;
  - `warm_pool_is_separate` (a full `cpu` never blocks a warm dispatch), `warm_keys_never_share_a_worker`, `full_pool_retires_the_longest_idle_other_key`;
  - `next_run_at_is_respected`, `stuck_pool_never_dispatched`;
  - `matches_brute_force_reference` (random states; the reference sorts everything);
  - `schedule_is_fast` (50,000 queued, 20 resource sets: one call ≤ 1 ms median on Linux; asserts ≤ 5 ms).
- [ ] **Step 2: Implement.** **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): resource pools and a pure job scheduler`.

### Task 9a: The engine: submit, the store, cold runs, files, host calls and hooks — tier: opus — after Tasks 4, 5, 6, 7, 8

**Files:**
- Create: `apps/finisherd/src/jobs/{engine.ts, runner.ts, host.ts, files.ts, layer.ts, view.ts, calls.ts}`.
- Modify: `apps/finisherd/src/jobs/store.ts` (only if a query is missing), `apps/finisherd/test/jobs/kinds.test.ts` (drop the Task 7 skip), `TASKS.md`.
- Test: `apps/finisherd/test/jobs/{engine.test.ts, runner.test.ts, files.test.ts, hooks.test.ts, calls.test.ts}`, `apps/finisherd/test/support/jobs.ts` (an engine over a temp server data, the test-hooks `fin-workers`, short `Timing`, `TestClock` where needed).

**Interfaces:** `JobEngine`, `JobEngineTag`, `layerJobEngine` under **Frozen interfaces**. Until later tasks: helpers, pause and abandonment answer `WORKER_UNAVAILABLE {notYet}` (Task 9b); a `warm` kind runs cold and `warm(family)` succeeds doing nothing (Task 11); failures use a minimal "fail" policy (Task 10).
```ts
export interface JobEngineOptions { readonly binaryDir?: string; readonly timing?: JobHostOptions["timing"]; readonly testKinds?: boolean;
  readonly cores?: number; readonly totalMem?: number; readonly cgroups?: "auto" | "off"; readonly clock?: Clock.Clock }
// host.ts:   wraps JobHost; routes events by proc id to their runner; converts ExitInfo to (reason, FinError)
// runner.ts: one attempt of one cold job: dir and files → prepare → worker (hello, start) → messages
//            → result/error/exit → enterHooks → onSucceeded → outcome
// files.ts:  jobDir, createFiles(kind, params, attempt), emptyOutDirs, checkPrepared, removeJobDir (Windows retries), sweep(keep)
// calls.ts:  routes `call` to FamilySpec.onCall / HelperProgram.onCall with a CallCtx; ≤ 16 in flight; replies via host.send
```
Decisions 5a, 10, 19, 20, 25–28 (the `phase: hooks` part), 33, 38, 40–43. `engine.ts` ≤ 550 lines (submit, dispatch loop, transitions, public API); `runner.ts` ≤ 550.

- [ ] **Step 1: Write failing tests:**
  - submit: `unknown_kind`, `invalid_params_name_the_field`, `idem_returns_existing`, `dedupe_returns_active`, `queue_full_at_limit` (with a test limit), `submit_dispatches_in_the_same_turn`;
  - runs:
    - `echo_succeeds_with_result`, `progress_events_are_throttled`, `items_are_recorded`;
    - `job_done_and_worker_exit_emitted` (fields per Decision 42; `sandboxed: false`);
    - `owner_via_rights_are_kept` (8's fields round-trip to the view, the bus and `CallCtx`);
    - `diag_forwarded_only_when_on_and_allowed`;
    - liveness through the engine: `hung_job_fails_worker_hung`, `stalled_job_fails_job_stalled`, `timeout_fails_job_timeout`, `oom_fails_worker_oom`;
  - calls: `call_reaches_on_call_with_ctx`, `call_error_is_replied`, `unknown_method_is_invalid_request`, `seventeenth_call_is_call_limit`, `open_calls_interrupted_at_exit`;
  - files: `files_created_sized_and_private` (0600/0700; Windows DACL inherited), `out_dir_emptied_between_attempts` (`test.files` reports `out/` empty on attempt 2), `missing_prepared_file_fails_the_attempt`, `symlinked_prepared_file_is_refused`, `job_dir_removed_after_failure`, `job_dirs_swept_at_start`;
  - hooks:
    - `prepare_writes_before_any_process` (a `prepare` that records `workers().processes` = 0);
    - `on_succeeded_runs_after_group_exit_before_delete` (the hook sees the worker's pid gone and the directory present; after it, the directory is gone);
    - `result_and_phase_persisted_before_the_hook` (the hook reads the row: `running`, `phase: hooks`, result stored);
    - `on_succeeded_can_replace_the_result`, `on_succeeded_failure_fails_the_job`, `hook_timeout_is_job_timeout`, `cancel_during_hook_ends_succeeded`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates on all three OSes (or Linux plus CI for the others) and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): job engine with submit, cold runs, mapped files, host calls and kind hooks`.

### Task 9b: Helpers, cancel and pause, abandonment, restart and close — tier: opus — after Task 9a

**Files:**
- Create: `apps/finisherd/src/jobs/{helpers.ts, control.ts (cancel, pause, resume), restart.ts (start-up recovery, close)}`.
- Modify: `apps/finisherd/src/jobs/{engine.ts, runner.ts}`.
- Test: `apps/finisherd/test/jobs/{helpers.test.ts, control.test.ts, restart.test.ts}`.

Decisions 18, 23, 28, 29, 37, 39, 40 (re-run).

- [ ] **Step 1: Write failing tests:**
  - helpers: `helpers_start_before_the_worker` (hello order), `helpers_only_get_their_files` (the attach lists only the helper's names), `helper_exit_early_is_helper_failed`, `helpers_die_with_the_job` (a `test:spawn` helper's grandchild gone after the job), `helpers_shut_down_after_result`, `helper_calls_reach_its_on_call`;
  - cancel and pause: `cancel_queued`, `cancel_running_kills_tree` (`test.spawn`), `pause_queued_keeps_place`, `pause_running_resumable_checkpoints`, `pause_all_stops_dispatch`, `resume_requeues`;
  - abandonment: `two_abandoned_mark_io_pool_stuck` (with `abandonForTest`), `stuck_pool_refuses_submit_resource_stuck`, `stuck_clears_when_one_exits`;
  - persistence:
    - `transitions_written_at_once`, `checkpoint_throttled_to_10s` (TestClock);
    - `restart_requeues_resumable_from_checkpoint` (`test.checkpoint`: a second engine over the same server data resumes at the checkpointed step);
    - `restart_interrupts_non_resumable`, `restart_keeps_paused`;
    - `restart_reruns_on_succeeded` (a hook that kills the engine half-way through on its first call: after the restart the directory is intact, the hook runs again with `rerun: true`, and the job ends `succeeded` with exactly one `job.done`);
    - `items_reported_in_the_last_second_may_rerun` (documents Decision 26's at-least-once);
    - `close_leaves_no_running_rows`, `retention_runs_hourly` (TestClock).
- [ ] **Step 2: Implement.** **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): job helpers, cancel and pause, abandonment, restart and close`.

### Task 10: Retries, backoff, poison handling and isolation — tier: opus — after Task 9b

**Files:**
- Create: `apps/finisherd/src/jobs/isolation.ts` (pure: `classify(exit | error, healthy) → FailureClass`; `nextStep(state, failure, attempt) → Retry | Isolate | Fail`; the isolation state machine of Decision 32).
- Modify: `apps/finisherd/src/jobs/{runner.ts, engine.ts}`.
- Test: `apps/finisherd/test/jobs/{isolation.test.ts (pure, fast-check), retries.test.ts (real processes)}`.

**Interfaces:**
```ts
export type FailureClass = "resource" | "crash" | "oom" | "oomWarm" | "transient" | "deterministic" | "fresh" | "mismatch" | "hook" | "cancel";
export function classify(f: { exit?: ExitInfo; error?: JobErrorMsg; hook?: FinError; warmReused?: boolean; healthy?: boolean }): { cls: FailureClass; error: FinError };
export interface IsolationState { n: number; pending: string[]; inFlightAlone?: string; queue: string[][]; poisoned: string[]; crashRuns: number }
export function budget(s: IsolationState): number;                                 // min(2n, 2⌈log₂ n⌉ + 2·poisoned)
export function startIsolation(items: ReadonlyArray<string>): IsolationState;
export function afterRun(s: IsolationState, run: { items: ReadonlyArray<string>; done: ReadonlyArray<string>; inFlight?: string; crashed: boolean }): IsolationState | "exhausted";
export function nextRun(s: IsolationState): ReadonlyArray<string> | undefined;     // undefined: nothing left
export function backoffMs(attemptsSoFar: number): number;                           // 5 s × 2^n, ≤ 300 s
```
Decisions 31, 32.

- [ ] **Step 1: Write failing tests:**
  - pure:
    - `classification_table` (every row of Decision 31, in its order), `backoff_is_5s_times_2_to_the_n` (n = 1, 2, 3 → 10 s, 20 s, 40 s; capped at 300 s);
    - `isolation_properties` (fast-check, over n ≤ 256 items with any set of p deterministic poison items and all other items healthy: acknowledged items never re-run; isolation ends with exactly those p poisoned and the rest done; crash runs ≤ `min(2n, 2⌈log₂ n⌉ + 2p)`, so the budget is never exhausted; with p = 0 nothing is poisoned);
    - `unexplained_crashes_exhaust_the_budget` (a pure model where every run of ≥ 2 items crashes on its second item: exhausted after `2⌈log₂ n⌉ + 1` crash runs, nothing poisoned);
  - processes:
    - `poison_item_isolated_in_two_runs` (`test.crash` with one poison item in flight: run 2 is the item alone and fails `POISON_ITEM`, run 3 completes the rest; the result lists 1 item error and the rest);
    - `isolation_runs_without_backoff` (TestClock not advanced: all isolation runs happen);
    - `crash_between_items_bisects` (`between: true`), `reported_items_are_not_rerun` (the worker's `begin` log);
    - `isolation_budget_exhausted_by_unexplained_crashes` (`crashOnSecond: true`, 8 items: `WORKER_CRASHED {isolation: "budget"}`, no item poisoned);
    - `sigbus_is_a_resource_fault` (`how: bus`: backoff retry, never isolated, no poison), `resource_unhealthy_never_poisons` (`resourceHealthy` false: the in-flight item is not poisoned);
    - `hang_is_never_bisected`, `stall_is_job_stalled_never_bisected`, `helper_failure_is_never_bisected`;
    - `retry_backoff_persists_next_run_at` (TestClock: 10 s, 20 s; a restart keeps `nextRunAt`), `max_attempts_then_failed`;
    - `fresh_retry_once_at_once` (`error retry: fresh`), `deterministic_error_not_retried`, `mismatch_not_retried` (`testbad` family);
    - `oom_with_item_in_flight_isolates`, `oom_without_item_fails`;
    - `interactive_retries_once_at_once_and_is_never_isolated`.
- [ ] **Step 2: Implement.** **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): job retries with backoff and isolation of crashing batches`.

### Task 11: Warm workers — tier: opus — after Task 10

**Files:**
- Create: `apps/finisherd/src/jobs/warm.ts` (per family: workers by key, ready lists, FIFO, expiry and recycling timers, per-job helper groups, the direct hand-off).
- Modify: `apps/finisherd/src/jobs/{engine.ts, runner.ts, isolation.ts (only the `oomWarm` wiring)}`.
- Test: `apps/finisherd/test/jobs/warm.test.ts`.

**Interfaces:** `JobEngine.warm(family)` live; `workers()` lists warm workers `{family, key, pid, state: spawning|ready|busy|retiring, served, ageS, idleS, memory}`. Decisions 6, 23, 31 (`oomWarm`), 35, 36; spec §6.3.

- [ ] **Step 1: Write failing tests:**
  - `back_to_back_on_one_process` (100 `test.warm` jobs: one pid, `served` 1…100);
  - **`dispatch_p99_under_5ms_under_load`** (submit → `start` enqueued for 1,000 `test.warm` jobs while 10,000 cold `test.sleep` jobs are queued and 4 cold spawns are in flight; asserts ≤ 5 ms p99 on Linux, records the others), `ready_worker_bypasses_schedule` (a spy on `schedule()` sees no call for a direct hand-off);
  - `first_submit_prewarms`, `warm_family_starts_one_ahead`, `slots_default_one_max_two`;
  - `cold_jobs_never_delay_interactive` (all `cpu` slots busy with `test.sleep`; a `test.warm` still dispatches at once);
  - `idle_expiry_60s` (TestClock: `shutdown` at 60 s, `worker.exit` reason `idleExpired`);
  - `recycled_after_500_jobs`, `recycled_after_30_minutes`, `recycled_at_80_percent_memory` (a `ready` reporting 85 % of the limit), `replacement_starts_before_retirement` (the old worker serves until the new `hello`; no job waits for a cold start);
  - `crash_mid_job_retries_on_fresh_worker` (`killProcForTest` during a job: the retry runs on a new pid and succeeds; `worker.exit` `crashed`/`killed`), `oom_on_reused_warm_worker_retries_fresh` (never isolated), `replacement_on_next_submit`;
  - `cancel_keeps_a_cooperative_worker`, `cancel_kills_an_uncooperative_worker` (500 ms grace);
  - `helpers_get_a_per_job_group_on_warm_workers` (a warm kind with a test helper: the helper's group dies with the job, the worker lives);
  - `files_closed_before_ready` (`holdFile`: `WORKER_CRASHED {heldFiles}` in the test-hooks build; on Windows the directory delete after `ready` succeeds without `holdFile`);
  - `warm_key_separates_workers` (`testplugin` with keys A and B: two workers, never crossed), `full_pool_retires_longest_idle_key`;
  - `fresh_and_reused_results_equal` (the same `test.warm` params on a fresh and on a reused worker give equal results apart from `pid`/`served`, the family contract of spec §6.3);
  - `warm_kind_at_normal_priority_runs_cold`, `memory_limit_is_the_interactive_rule`, `close_shuts_down_warm_workers`.
- [ ] **Step 2: Implement.** `warm.ts` ≤ 550 lines (split `warm-keys.ts` if needed).
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): warm interactive worker pools with keys, expiry and recycling`.

### Task 12: Wiring, `finisherd`-death tests, the spawn gate and budgets — tier: sonnet — after Task 11

**Files:**
- Create:
  - `scripts/check-worker-spawn.mjs` (+ `.test.mjs`; Decision 24);
  - `apps/finisherd/test/jobs/kill.test.ts`;
  - `apps/finisherd/test/support/engine-child.ts` (a Node child that builds the engine layer over a server data, submits what its argv says, prints the pids it learns, and waits);
  - `apps/finisherd/scripts/jobs-budget.ts`.
- Modify:
  - `apps/finisherd/src/main.ts`: import `jobs/store.ts` before the store opens; `ensurePrivateDir(<server-data>/jobs)`; `layerJobEngine` after the control store, with `memorySink`; `engine.close()` in the shutdown path before the store closes; one start log line with `caps()`;
  - `scripts/gates.mjs`, `TASKS.md`.

**Why no e2e scenario here:** 5C-1 has no RPC and no `fin` command; the first `fin`-driven job scenario comes with 5C-2 (`fin wait`) and 6A-2 (`fin look`). `kill.test.ts` drives a real engine process instead.

- [ ] **Step 1: Write failing tests:**
  - `kill.test.ts` (all three OSes):
    - `finisherd_kill_leaves_no_process`: the engine child runs `test.spawn` (`detach: none`) and a `test.hang`; the test kills the child with `SIGKILL` / `process.kill` (= `TerminateProcess` on Windows). Worker, grandchild and the hung worker are gone within 5 s, and the watchdog is gone on Unix;
    - `restart_after_kill_interrupts_and_sweeps` (a second engine child over the same server data: the rows are `interrupted`/requeued per Decision 28 and `jobs/` is empty);
    - `graceful_close_leaves_no_running_rows`;
  - `main.ts`: `server_starts_with_the_job_engine` (the existing `server.test.ts` start path; the start log names the kill tree and `sandboxed: false`), `jobs_dir_is_private`;
  - `check-worker-spawn.test.mjs`: flags each forbidden API in a fixture tree, ignores `test_family/` and `watchdog.rs`, passes on the real tree.
- [ ] **Step 2: Budget script** (`jobs-budget.ts`, release builds). It prints a table; Task 14 records it.
  - store statements p99 with 10k jobs and 40k items (≤ 1 ms);
  - warm hand-off p99 under Task 11's load (≤ 5 ms);
  - spawn → `hello` p50/p95 for `test.echo` (recorded; 6A-2's budget is ≤ 150 ms p95), **also with `finisherd`'s heap grown to 1 GiB resident** (Linux forks with `pre_exec`, so page-table copying scales with the parent; recorded, and if p95 passes 150 ms, a TASKS follow-up for a small spawn helper process);
  - cancel → tree gone p95.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): start the job engine with the server; host-death tests and the worker spawn gate`.

### Task 13: Spec as built, architecture pointer, tracker — tier: sonnet — after Task 12

**Files:**
- `docs/specs/2026-11-07-sub-project-5-control-plane-design.md`:
  - §6.0–6.6 and §6.8–6.9 "as built (5C-1)": the three crates, the watchdog and its window, the cgroup conditions, `prepare`, the at-least-once `onSucceeded`, `rememberPoison`, `resourceHealthy`, `WORKER_OOM` per group, the new exit reasons, the isolation budget, measured numbers;
  - **§6.9's deviation:** the `setsid` grandchild survives cancel and host death with process groups (macOS; Linux without delegated cgroups);
  - fold §6.10's interface additions into the sections they amend;
  - §10 rows for `fast-check` and `libc`; §11 decisions referenced.
- `docs/specs/2026-11-01-finisher-architecture-design.md`: the §27 pointer "Sub-project 5C-1 as built", and §7's `fin-workers` line.
- `TASKS.md`: tick the line, and add the follow-ups:
  - **D4 replaces `memorySink` with the journal sink**;
  - 5C-2: the RPCs and ownership checks; `server.status` reports `caps()`;
  - sub-project 7: implement `Spawner`, the self-sandbox function and `job_limits` (active process 1 for the `worker` profile), and deny process creation in the `worker` profile;
  - sub-project 8: serve its host API through `onCall`, key plugin warm processes by `warmKey`, and implement the envelope (minor rule) in its SDK;
  - the `setsid` gap; a spawn helper process if Task 12's large-heap spawn passes 150 ms;
  - sub-project 9: package and sign `fin-workers` beside `fin-node`.

- [ ] **Step 1:** Write the as-built notes from the code, listing every deviation from spec revision 3. **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 5C-1 as built`.

### Task 14: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

On macOS (Apple silicon), Windows 11 (non-elevated) and the Linux VM; host details in `archive/hosts.md`.

- [ ] **Step 1:** `cargo test --workspace`, `cargo test -p fin-workers` (test-hooks), `pnpm test`, `pnpm gates`, `pnpm e2e` on each OS. On each OS, also check that the release `fin-workers` refuses `test` (exit 2).
- [ ] **Step 2: Kill trees by hand,** each OS. Start a real `finisherd` (test-hooks `fin-node`) with the engine child of Task 12 running `test.spawn` (`none` and `setsid`) and `test.hang`. Kill `finisherd` with `kill -9` / Task Manager's "End task" / `taskkill /F`, then list leftover processes (`ps -eo pid,pgid,comm`, `Get-Process`).
  - Expected: none, except the documented `setsid` gap with process groups.
  - Repeat on Linux under `systemd-run --user -p Delegate=yes --wait` (cgroup mode: none at all), and inside the Docker image if one exists by then.
- [ ] **Step 3: Windows specifics:**
  - `finisherd` started from Windows Terminal and from PowerShell (both may put it in a Job): the host → group → leaf nesting works;
  - `test.crash` `segv` and `fastfail` show no WER dialog;
  - Defender does not slow spawn → `hello` beyond the recorded p95 (note it if it does).
- [ ] **Step 4: macOS specifics:** the watchdog survives `Ctrl-C` in the terminal that runs `finisherd` until `finisherd` exits, then cleans up; QoS read back with `taskinfo` or `ps -o pri`; `ri_phys_footprint` against Activity Monitor's memory column for `test.oom`.
- [ ] **Step 5: Measure** with `jobs-budget.ts` on each OS and record in TASKS and the spec: statement p99, warm hand-off p99 under load, spawn → `hello` p50/p95 (normal and 1 GiB heap), cancel → tree gone p95, and the watchdog's cleanup time after host death.
- [ ] **Step 6:** Fix what fails (one commit each, with a test); re-run Step 1.

## Review findings (revision 2) and where they are resolved

| Finding | Resolution |
|---|---|
| **B1** plugin processes cannot use the envelope | `call`/`reply` routed to `FamilySpec.onCall`/`HelperProgram.onCall` with `CallCtx` (Decision 5a; frozen `WorkerMsg::Call`, `HostMsg::Reply`, `JobCtx::call`, `HelperCtx::call`); exact envelope hash only for `Program::Bundled`, `v` + `minor` for plugins, unknown `t` above the host's minor gets an error reply (Decision 3; Task 3 `plugin_minor_unknown_type_gets_error_reply`); `FamilySpec.warmKey` (Decision 35; Task 11 `warm_key_separates_workers`) |
| **B2** spawner and sandbox expressiveness | leaf kill domain per process: Windows host → group → leaf Job, Linux leaf cgroup (Decisions 14, 16, 17); `PlatformCommand.job_limits {active_processes, ui}` (Decision 9; Task 5 `leaf_active_process_limit`, Task 4 `leaf_pids_max_from_job_limits`); self-sandbox entry in `worker::run`/`helper::run` before `hello`, failing closed (Decision 9a; `fin_jobs::sandbox`; Task 1 and Task 3 tests) |
| **B3** the JS thread must never block | async `spawn`, `killGroup`, `shutdown`; `openGroup` allocates only; per-process writer threads with bounded queues; a bounded blocking TSFN from the readers; liveness ignores time blocked on JS; progress coalesced in Rust (Decisions 11, 11a; Task 6 tests); Task 6 raised to opus |
| **B4** `onSucceeded` lost on a crash | result + `phase: 'hooks'` persisted before the hook; restart keeps the directory and re-runs it; contract "idempotent, at least once"; `phase` column in migration 0001; `SucceededCtx.rerun` (Decisions 25, 28, 40; Task 9a `result_and_phase_persisted_before_the_hook`, Task 9b `restart_reruns_on_succeeded`) |
| **H1** exit can overtake the last messages | the reader drains stdout to EOF or 200 ms, then `Exit` goes out on the same per-process queue (Decision 19a; Task 3 `result_then_immediate_exit_is_success`) |
| **H2** pgid reuse and watchdog races | leaders observed with `waitid(WNOWAIT)` and reaped only at `close_group`; `kill_group` on a closed group is a no-op; `-g` written before the reap, and the watchdog drains stdin before killing; remaining window documented; leftover cgroup `jobs/g*` swept at start (Decisions 15–17; Task 3 `leader_unreaped_until_close_group`, Task 4 `late_minus_g_is_honoured`, `leftover_groups_swept_at_start`) |
| **H3** resource faults poisoned items; warm OOM; leaks | `SIGBUS`/`EXCEPTION_IN_PAGE_ERROR` are resource faults; `KindSpec.resourceHealthy` asked before any poison verdict; OOM on a reused warm worker → one fresh retry, never isolation; recycle at `ready` above 80 % of the limit (Decisions 20, 31, 36; Task 10 `sigbus_is_a_resource_fault`, `resource_unhealthy_never_poisons`, Task 11 `oom_on_reused_warm_worker_retries_fresh`, `recycled_at_80_percent_memory`) |
| **H4** memory limits meant different things per OS | per group everywhere, with the reason (Decision 20); Windows `JOB_OBJECT_LIMIT_JOB_MEMORY`, confirmed by limit-violation info and `TotalProcesses` because port messages are not guaranteed (Decisions 14, 22); Linux samples `RssAnon + RssShmem`, macOS `ri_phys_footprint` (Task 4 `sampled_figure_is_anon_plus_shmem`, Task 5 `job_memory_limit_is_oom_confirmed_by_violation_info`) |
| **M1** isolation budget and contradictions | budget `min(2n, 2⌈log₂ n⌉ + 2p)`; isolation runs at once without backoff; interactive jobs never isolated; the property now covers deterministic poison sets and never exhausts, while exhaustion is tested only with unexplained crashes (`crashOnSecond`) (Decisions 31, 32; Task 10) |
| **M2** warm hand-off and recycling details | a ready worker is handed its job directly, without `schedule()` (Decision 35; Task 11 `ready_worker_bypasses_schedule`); p99 measured with 10k queued and cold spawns in flight; the replacement starts before retirement; families close every file and mapping before `ready`, checked in test builds (Decisions 6, 36; Task 11 `files_closed_before_ready`) |
| **M3** task dependencies | fin-core payloads moved to Task 1, so Task 7 needs only Task 1; Tasks 7/8 wording made consistent ("after", running beside 2–6); Task 3 defines the per-OS `KillDomain` trait, so Tasks 4 and 5 are independent; Task 9 split into 9a (submit, store, cold runs, files, hooks, calls) and 9b (helpers, cancel/pause, abandonment, restart/close) |
| **M4** `setsid` escape and Windows inheritance | the escape applies on macOS and after cancel too; recorded as a deviation from spec §6.9 (Decisions 15, 18; Task 4 `setsid_grandchild_escape_is_documented`; Task 13); Windows pipes inheritable only around `CreateProcessW` on the spawn thread, exits detected from process handles (Decisions 10, 14; Task 5 `concurrent_libuv_spawn_inherits_no_pipe`, `exits_come_from_process_handles`) |
| Minor: backoff formula | `5 s × 2ⁿ`, n = attempts so far, ≤ 5 min (Decision 31; Task 10 `backoff_is_5s_times_2_to_the_n`) |
| Minor: `out/` between attempts | write directories emptied before each attempt (Decision 38; Task 9a `out_dir_emptied_between_attempts`) |
| Minor: item delivery | items are at-least-once after a crash, stated (Decision 26; Task 9b `items_reported_in_the_last_second_may_rerun`) |
| Minor: EINTR, allocation in `pre_exec` | every blocking syscall retries `EINTR`; nothing allocated or locked in `pre_exec`, Spawner steps included (Decisions 9, 11, 12; Task 3 `pre_exec_does_not_allocate`, `eintr_is_retried`) |
| Minor: `__fastfail` | the runtime turns crash UI off (`SetErrorMode`, `WerSetFlags`), and `fastfail_dies_without_wer` is tested (Decision 6; Task 5) |
| Minor: `SIGCONT` and `START_SUSPENDED` | Task 3 Step 0 proves it first, with a fallback (Decision 13) |
| Minor: fork cost with a large heap | spawn → `hello` also measured with a 1 GiB heap (Task 12 budget; follow-up if over 150 ms) |
| Interface additions visible to sp7/sp8 | spec §6.10 added with this revision (Call/Reply, minor rule, `warmKey`, leaf domains and `job_limits`, the self-sandbox entry, `phase: hooks`), folded into §6 by Task 13 |
