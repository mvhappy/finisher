# Sub-project 7C: The OS sandbox — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the policy and its enumeration, Landlock, the seccomp filter, Seatbelt, AppContainer and the ACL broker, the agent listener, per-job grants for warm workers, session wiring), `sonnet` for plumbing (status warnings, the probe binary and CI, e2e, docs). Controller tasks (0 and 16) need real hosts.

**Goal:** every process tree Finisher starts for an agent, a hook, a media worker or a plugin runs inside `fin-sandbox`, on macOS, Linux and Windows 11, under one `SandboxPolicy` and four profiles (`agent`, `hook`, `worker`, `plugin`), and the escape suites prove each denial on each OS. After 7C:
- **nothing an agent runs can read `<server-data>/auth/`** (`local.key`, `console.key`, `credential.master`), `settings.json`, `control.db`, the journal, the agent-trace store or the audit log, so human-only operations hold against hostile code, not only against well-behaved harnesses (sp5 §4.6, owner decision 6 of sp5; agent §7.5);
- the `before7C` warning of 7A (`SANDBOX_DISABLED {profile: agent, reason: before7C}`, `apps/finisherd/src/agent/warnings.ts`) is gone wherever a sandbox applies; what remains is the F2 case (Landlock disabled: `reason: landlockDisabled`, everything runs) and the fault case (`SANDBOX_UNAVAILABLE`);
- **CLI harness processes (7D, 7E) never run as the owner unconfined**: `apps/finisherd/src/agent/sandbox.ts` is the only way the runtime starts a provider process or a shell, and it starts them in the `agent` profile with the session environment of spec §8.5, `FIN_PROJECT_DIR` included;
- 5C's `Spawner` (`crates/fin-jobhost/src/spawner.rs`) has a real implementation, the self-sandbox entry (`fin_jobs::sandbox::register`) has a registered function in `fin-workers`, and the **Linux Landlock `worker` profile ships first, before 6B-1** (sp6 owner decision 6; agent §1.4);
- **warm workers run warm under a sandbox again**, because each warm worker is granted only its own jobs' paths (owner decision, 5C-1 Task 11 review H3), not the whole `<server-data>/jobs/` root (`apps/finisherd/src/jobs/warm-worker.ts:98-101`);
- the `probe_beneath` residual of sp5 §4.17 (a mount point or hard link inside a project folder) stays one that only the user's own file access can create: no sandboxed process can make either.

**Architecture:**
- **`crates/fin-sandbox`** (new; owned by sub-project 7, spec §1.6): `policy.rs` (the pure `SandboxPolicy` of spec §8.1), `profile/{agent,hook,worker,plugin}.rs` (builders from typed inputs), `enumerate.rs` (write roots split around protected and denied roots, read roots), `status.rs` (per-OS mechanism status: `Enforced`, `Disabled {reason}`, `Unavailable {need}`), `linux/{landlock.rs, seccomp.rs, apply.rs}`, `macos/{sbpl.rs, apply.rs}`, `windows/{appcontainer.rs, job.rs, desktop.rs, apply.rs}`, `self_apply.rs` (the function `fin_jobs::sandbox::register` takes), `spawner.rs` (`SandboxSpawner: fin_jobhost::Spawner`), `bin/fin-sandbox-exec.rs` (the launcher for processes `finisherd` starts itself: agents and hooks), and `data/` (toolchain read lists and macOS `mach-lookup` allow-lists, as data).
- **`crates/fin-node`:** `acl_broker/` (Windows; write-ahead log, startup sweep, grant/revoke for sessions, jobs and plugin calls), `sandbox.rs` (napi: `sandboxStatus()`, `sandboxPolicyCheck(json)`, the broker), and the jobhost binding constructed with `SandboxSpawner` instead of `Unsandboxed`.
- **`apps/finisherd`:** `agent/sandbox.ts` (session policy, session environment, `spawnInSession`), `agent/session-dirs.ts` (spec §8.5 and §8.6 directories), `agent/listener.ts` (the Linux loopback agent listener), `sandbox/status.ts` (one source for `SANDBOX_DISABLED` per profile and the `sandbox.unconfined` event), and call-site lines in `main.ts`, `jobs/warm.ts`, `jobs/warm-worker.ts`, `jobs/files.ts`.
- **`crates/fin`:** the `tcp:` endpoint form (`endpoint.rs`).
- **`fin-core::events`:** the `sandbox.unconfined` diagnostics kind (metadata only); `sandbox.violation` is already a reserved audit kind (`packages/contracts/src/audit.ts:51`).

**Tech Stack:** Rust 2024; `landlock` 0.4.7 (MIT OR Apache-2.0), `seccompiler` 0.5.0 (Apache-2.0 OR BSD-3-Clause), `windows-sys` 0.61.2 (already in the tree); macOS `sandbox-exec` and `sandbox_init` (OS components); Node 24 and Effect 4 on the TypeScript side. No bubblewrap (LGPL), no `birdcage` (GPL), no `extrasafe` (spec §17). `pnpm gates` runs cargo-deny and the JS licence gate over these.

**Spec:** `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §1.4, §1.6, §1.7, §2 principle 9, §5.6, §7.5, **§8 (all)**, §15, §16.2 (Sandbox and `fin` rows), §17, §18, §19 decisions 13, 15, 23, 25, 30, 32, §20 (owner decisions 3, 5, 6 and F2), §22. Cross-design contracts: sp5 §4.11 (sandbox reachability), §4.6 and §4.17 (residuals), §6.2 (the `Spawner` and the self-sandbox entry), §6.3 (warm workers), §6.12 (residuals: `setsid`, WER); sp6 §3.5 and §3.5.1 (the `worker` profile and its Linux Landlock phase); sp8 §3.1, §3.2 and §16.1 (the `plugin` profile).

**Code references** with line numbers are at the 7A-2 tip `sp7a2-t13` (`772b142`), which holds everything of the integration branch plus 7A-2 Tasks 2–13; 7A-2 is not merged into the integration branch at the time of writing, so `apps/finisherd/src/agent/*` and `chain/hooks.ts` exist only there.

**Batches and branches** (each merges green on its own):

| Batch | Tasks | Needs | Unblocks |
|---|---|---|---|
| **7C-1** Policy and the Linux Landlock `worker` profile (**before 6B-1**) | 1, 2, 3 | nothing (4C integration branch); Task 0 B5 helps but does not block | sp6 6B-1 |
| **7C-2** Linux: seccomp, the launcher, the agent listener, the `Spawner` | 4, 5, 6, 7 | 7C-1; Task 0 B5 | 7D/7E on Linux, 7F, sp8 8B on Linux |
| **7C-3** macOS: Seatbelt | 8 | 7C-1; Task 0 B6 | the same on macOS |
| **7C-4** Windows: AppContainer, job objects, the ACL broker | 9, 10 | 7C-1; Task 0 B7 (capability SIDs) | the same on Windows |
| **7C-5** Wiring into sessions and jobs | 11, 12, 13, 14 | 7C-2..7C-4; **7A-2 merged** (sessions, credentials, `agent/warnings.ts`); 5A | 7D, 7E, 7F, sp6 6G, sp8 8B |
| Close | 15, 16 | all | — |

Branch names: `sp7c1-policy`, `sp7c2-linux`, `sp7c3-macos`, `sp7c4-windows`, `sp7c5-wiring`, each stacked on the integration branch of the day (`sp4c-edit-breadth` at the time of writing; 7C-5 also on the 7A-2 branch). 7C-2, 7C-3 and 7C-4 touch disjoint OS modules and can run in parallel on three machines (the Linux VM, the Mac, the Windows host).

**Out of scope:** the Claude Code and Codex adapters (7D, 7E: they call `agent/sandbox.ts`), their per-CLI toolchain lists beyond the base list (each adapter ships its own data file, produced from Task 0 and re-validated per CLI version), `finisherd provider signin` (7D/7E; 7C creates the directories and the store link it needs), the Windows brokered shell's MCP server (7D; 7C provides the container it runs commands in), hook dispatch (7F; 7C provides the `hook` profile), the plugin host (sp8 8B; 7C provides the `plugin` profile and applies it through the `Spawner`), the egress allow-list proxy (after v1, owner decision 5), any UI (9), and WER exclusion at install (9; see Decision 22).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL dependency in shipped code (`pnpm gates`). bubblewrap is not used, in shipped code or tests.
- **No cloud-only imports.**
- **Model and event types are generated, never hand-written:** `sandbox.unconfined` derives `JsonSchema` and goes through `crates/fin-store/examples/export_schema.rs` and `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). The policy crosses into TypeScript only as JSON validated by `fin-sandbox` (`sandboxPolicyCheck`), never as a second hand-written type: `agent/sandbox.ts` builds the typed *inputs* of a profile (`AgentProfileInput` etc., generated from `fin-sandbox`'s schemars types), and `fin-sandbox` builds the policy.
- **Primitive ops live only in `fin-core`**: nothing in this plan adds one.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Every OS module is its own file; the seccomp filter is data (`seccomp.rs` builds it from tables in `linux/syscalls.rs`).
- **The sandbox is the guarantee, so it fails closed:** a policy that cannot be built or applied where the mechanism is enabled refuses the start (`SANDBOX_UNAVAILABLE {profile, os, need}`); only F2's "mechanism disabled" runs unconfined, with warnings (spec §8.9).
- **The repo is public:** no host names, IPs, user names or key names in tracked files; escape-suite fixtures use temporary directories and test-created keys only.

**Testing and process**
- **Every protection has a test that fails without it** (spec principle 9): an escape case is "attempt X, observe a denial" and passes only on an observed `EACCES`/`EPERM`/`ACCESS_DENIED`/`SIGSYS`; the absence of an error never passes. Each task proves at least one of its cases with a mutant (the rule removed, the case fails) and records the mutant in the commit body.
- Escape suites run as a **non-elevated user** (on Windows a medium-integrity session; the SSH session of the Windows host is elevated, so suites run through a Task Scheduler `/IT` task, TASKS.md "Windows run follow-ups").
- Tests never use the network: TCP cases use loopback listeners the test starts; the "open egress" check of the `agent` profile connects to a loopback listener on an allowed port.
- Clocks are injected where timeouts matter (the broker's WAL replay, the listener's handshake timeout).
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`; Tasks 11–14 also run `rtk pnpm e2e`. OS-specific tasks run their suites on their OS before the commit and `cargo check --target` for the other two (`x86_64-pc-windows-gnu`, `aarch64-apple-darwin`).
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. GitHub Actions pinned to commit SHAs.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **Spikes and scratch work live in `archive/`** (git-ignored); only conclusions reach the spec.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 7C: OS sandbox", in the same commit.

## Residuals this plan closes

Every "before 7C" item in the specs and the tracker, and where it closes:

| Residual | Where it is recorded | Closed by |
|---|---|---|
| The Linux Landlock `worker` profile must exist before 6B-1 | sp6 §3.5.1, owner decision 6; TASKS.md sp7 list | Tasks 2–3 (7C-1) |
| Tools (a CLI agent's shell, a script it runs) can read `auth/local.key` and `auth/console.key` and authenticate as `local` | sp5 §4.6, §4.17; agent §7.5, §8.9 (revision 6, `before7C`) | Tasks 5, 8, 9 (deny), 11 (every agent process goes through the sandbox), 13 (escape cases) |
| An unsandboxed agent harness runs as the owner (pseudo-terminal, `settings.json`, links anywhere) | sp5 §4.6 | Task 11 (`spawnInSession` is the only spawn path; a gate forbids `child_process` in `agent/` outside it), Task 13 |
| A warm worker is granted the whole jobs root; warm kinds run cold under a sandbox | sp5 §6.3, §6.12, Decision 35; `jobs/warm.ts:15-16`; TASKS.md | Task 12 |
| `probe_beneath` follows a mount point (`RESOLVE_NO_XDEV` unset) or a hard link inside a project folder | sp5 §4.17 | Decision 20; Task 13 cases (no sandboxed process can create either) |
| CLI harnesses (7D/7E) must set `FIN_PROJECT_DIR` | agent §5.6 (7A-2 Task 8b review B1); `crates/fin/src/agent_paths.rs:134-200` | Task 11 (`sessionEnv` sets it; `assertSessionEnv` is the contract 7D/7E call) |
| The `setsid` escape and process creation by workers | sp5 §6.2, §6.12 | Task 7 (`worker` profile: process creation denied; `job_limits.active_processes = 1` on Windows, Task 9) |
| WER: `__fastfail` can start a headless `WerFault.exe`; `fastfail_dies_without_wer` is flaky | sp5 §6.12; TASKS.md "Windows run follow-ups" | Decision 22, Task 9 (in-process parts); install-time exclusion stays with sub-project 9 (Decision 22, owner decision) |
| Plugin store integrity: `store/` stays writable to the user's account | sp8 §16.1 "Store integrity (follow-up for 7C)" | Decision 21 (read-only in every profile), no re-hash (owner decision; residual recorded) |
| Other sessions' workspaces are inside the project root | agent §5.6 "Known limits" (7A review L11) | Decision 6 (owner decision: same-project reads of other sessions' workspaces allowed, server reach and `fin` unchanged) |

## Decisions taken here (the spec fixes the shape; these fix the details)

### The policy and its application

1. **One pure policy, built only in Rust.** `fin_sandbox::policy::SandboxPolicy` is the struct of spec §8.1 (`profile`, `write`, `read: ReadPolicy::Only(..)`, `protect`, `deny_read`, `unix_sockets`, `net`, `mach`, `devices`, `limits`). `ReadPolicy::Everything {except}` exists behind `#[cfg(test)]` only. Profiles are built by `profile::agent(&AgentInput)`, `profile::hook(&HookInput)`, `profile::worker(&WorkerInput)`, `profile::plugin(&PluginInput)`; the inputs are schemars types exported to `@finisher/core-types`, so TypeScript passes typed inputs and never assembles a policy itself.
2. **Enumeration is the one hard algorithm** (`enumerate.rs`, property-tested): Landlock and AppContainer only *add* access, so a write root that contains a protected root (`workspace/` never does, but a plugin's data directory or a project folder with `.finisher/` inside might) is expanded into its children minus the protected ones, recursively, down to the protected path; a denied root inside a read root is handled the same way. Seatbelt does not need it (last match wins) but uses the same output in tests, so the three OSes are checked against one model.
3. **Who applies the sandbox, per kind of process:**
   - **Agents and hooks** (third-party binaries `finisherd` starts itself): through the launcher `fin-sandbox-exec --policy-fd <n> -- <argv>` (Linux: build the Landlock ruleset and the seccomp filter, `PR_SET_NO_NEW_PRIVS`, restrict, `execve`; macOS: `execve` of `/usr/bin/sandbox-exec -p <generated profile> -D…`; Windows: `CreateProcessW` in the AppContainer with the session's capability SIDs, inside a kill-on-close job object, relaying stdio and the exit code, staying the parent). The policy arrives on an inherited pipe (never argv or the environment; it names paths, not secrets, but argv is visible to every process of the user).
   - **Workers** (our own `fin-workers` binary): the **self-sandbox entry** 5C built (`FIN_SELF_SANDBOX`, applied before `hello`; sp5 §6.2 Decision 9a), on Linux and macOS. The function is `fin_sandbox::self_apply`, registered in `crates/fin-workers/src/main.rs`. On Windows, the `Spawner` gives the AppContainer token and attributes in `prepare`.
   - **Plugins** (third-party code started by 5C): applied **before** `exec` by the `Spawner`: Linux `pre_exec` steps (the ruleset and the BPF program are built in `prepare`, on the spawn thread; the child only calls `prctl`, `landlock_restrict_self` and `seccomp`, all async-signal-safe raw syscalls); macOS the program rewritten to `sandbox-exec -p …`; Windows token and attributes.
4. **The `Spawner` reports what it enforces.** Today the napi `JobHost` gives JavaScript no way to choose a spawner (`crates/fin-node/src/jobhost/mod.rs:180-187`) and `HostOptions::new` defaults to `Unsandboxed` (`crates/fin-jobhost/src/lib.rs:107,182`); 7C constructs it with `SandboxSpawner`. `SandboxSpawner::sandboxed()` is true only when every profile it is asked for is enforced on this host; `HostCaps` (`crates/fin-jobhost/src/lib.rs:244-254`, wire form `wire.rs:518-535`) gains `profiles: {worker, plugin}: enforced|disabled|unavailable` and `per_job_grants: bool`, carried through `@finisher/contracts` `jobs.ts`, so `warm.ts`, `server.status` and sp8's install plan (`thirdPartyCode {confined}`, `crates/fin-registry/src/plan/mod.rs:201-205`) read facts, not assumptions.
4a. **Limits.** Nothing sets an rlimit today (only `RLIMIT_NOFILE` is read, for the `FD_CLOEXEC` fallback), and `JobLimits` is enforced only by leaf Jobs or delegated cgroups (`caps().leaf_limits`). `SandboxPolicy.limits` is applied as: Windows, the launcher's or the leaf Job's limits; Linux and macOS, `setrlimit(RLIMIT_NOFILE)` in the launcher or the `pre_exec` step. Process counts: the `worker` profile needs none (seccomp and Seatbelt refuse process creation); the `plugin` profile's 8 processes are the leaf cgroup's `pids.max` where cgroups are delegated, and otherwise only 5C's foreign-process check at exit (`RLIMIT_NPROC` counts every process of the user, so it cannot be used). `RLIMIT_AS` is not used: it breaks GPU drivers' address-space reservations. Memory stays 5C's per-group limit (sp5 §6.2), which already covers every profile.
5. **A profile becomes the default on an OS when its suite passes there, and is then mandatory.** `fin_sandbox::READY: &[(Profile, Os)]` is a compiled table that Task 16 updates per OS as each suite passes on the validation hosts; a profile not in it on this OS runs as today (the env allow-list and its sub-project's own rules, with the warning its spec names: sp5 §6.2, sp8 §3.1). Once in the table it applies to every process of that profile, with no opt-out (sp8 owner decision 1), and a failure to apply it is `SANDBOX_UNAVAILABLE` (the start fails). **Owner decision (2026-11-08):** there is no owner opt-out of an enforced profile; hosts without the OS feature (Landlock off, kernel below the floor) run with the persistent warnings of F2 instead. This keeps third-party plugins running before the plugin suite passes, as sp8 requires, without a setting that could silence the sandbox later.

### Profiles

6. **`agent` read policy** (spec §8.1) is `Only` of: the base toolchain list (`data/<os>/toolchain-base.txt`: the shell, coreutils, `git`, `fin`, the dynamic loader and system libraries, certificates, fonts and locale data), the harness's own list (7D/7E add `data/<os>/toolchain-<harness>.txt`), the project folder **including other sessions' `workspace/<id>/` of the same project** (readable, as the server's reach in `agent/reach.ts` and `fin`'s client check in `agent_paths.rs` allow it; **Owner decision (2026-11-08):** reading another session's workspace within the same project is allowed and neither is changed; the read policy stays consistent with that reach, and writes stay on `workspace/<sessionId>/` and `workspace/shared/` only; 7A review L11 is closed as accepted), the referenced library roots, the session directory's `credential.json`, `settings/`, `tmp/`, `provider/`, `git/`, and the harness's sign-in store file. `deny_read` lists server data, the sensitive home paths of spec §8.1, `~/.claude`, `~/.codex`, the rest of `<server-data>/providers/`, and for Claude Code the project's `CLAUDE.md`, `CLAUDE.local.md`, `.claude/`. Writes: `workspace/<sessionId>/`, `workspace/shared/` (not in `plan`), `tmp/`, `provider/`, the sign-in store file. Protect: `.finisher/`, `assets/`, `finisher.project.json`, `finisher.rules.json` (project rules, 7B), `credential.json`, `settings/`.
6a. **Devices, TCP `bind`, `EXECUTE` and special files for `agent` and `hook`** (coordinator decisions 2026-11-08, raised in the Task 1–2 review and the Task 2 notes; Task 5 builds them):
    - **Devices:** read-write on `/dev/null`, `/dev/zero` and `/dev/full`, read on `/dev/random` and `/dev/urandom`, on Linux and macOS (Windows has no device paths in the policy). **No `/dev/tty`, no `/dev/pts/*` and no other device**: agents and hooks run headless with piped stdio, which they inherit as descriptors and never open by path. `validate` refuses any other device rule for these two profiles.
    - **TCP `bind`:** the `agent` profile handles `bind` as well as `connect` (ABI ≥ 4) and grants no `bind` port, so an agent cannot bind a TCP port; `hook`, `worker` and `plugin` already refuse both. **Residual (observed in Task 5, ABI 8):** `listen()` on an unbound socket makes the kernel bind an ephemeral port without Landlock's `bind` check, so a process of any profile with TCP can still accept connections on an ephemeral port (on every interface); the escape suite asserts the residual so it flips when a kernel closes it. A seccomp refusal of `listen` for these profiles is one fix for the person writing the filter (Task 4).
    - **`EXECUTE` on `agent` write roots: kept**, as Decision 6 has it: a CLI agent runs scripts and programs it builds in its workspace (`./build.sh`, `node_modules/.bin`), and with read and write on those roots a refusal would only be bypassed by `sh script`. A `hook`'s private temp gets no `EXECUTE` (its program is its own file, a read root).
    - **`MAKE_FIFO` in write roots: kept** for `agent` and `hook` (shells and tools make FIFOs in `TMPDIR`; opening one is a file open, which Landlock mediates, so another session cannot open it). **`MAKE_SOCK`: not granted** for either: the spec refuses `AF_UNIX` for both once the seccomp filter lands (Decision 11), so a pathname socket has no use under these profiles, and while the filter is deferred (Task 4) a socket bound in one session's write root could be reached by another session's process (`connect` is not mediated below Landlock ABI 9). 7D/7E's per-CLI validation revisits this if a CLI needs one.
7. **`hook` profile:** the project read-only, the hook's own file, its toolchain (base list), a private temp; `net: None` unless the hook declares network, then the agent listener (Linux) or the pipe (Windows) or `fin.sock` (macOS) only, never IP egress; no credential unless it declares `fin` (7F mints a `plan`-scoped one; 7C only reserves the read of its file).
8. **`worker` profile:** reads = the job's manifest sources (per file where the OS allows it: Landlock and Seatbelt can grant files; else the roots), the font and media caches, the job's `in/`; writes = `out/` and a private temp; GPU rows of spec §8.10 (Linux `/dev/dri/renderD*` read/write, `card*` read-only, the Vulkan ICD and driver files, `/sys/class/drm`, `/sys/dev/char`; `/dev/nvidia*` in 6G); `net: None`; **process creation denied** (Linux: Landlock `EXECUTE` only on the `fin-workers` binary and the loader, seccomp refuses `clone`/`clone3` without `CLONE_THREAD`, `fork`, `vfork`; macOS `(deny process-fork)` and `process-exec` except none; Windows active-process limit 1). This closes the `setsid` gap of sp5 §6.2 for workers. **7C-1 ships the Landlock part only** (no seccomp, no GPU proof: 6B-1 adds its GPU paths to `WorkerInput.devices` and proves them on its hardware), exactly sp6 §3.5.1's phase.
9. **`plugin` profile:** sp8 §16.1 verbatim: no network (seccomp refuses `AF_INET`, `AF_INET6`, `AF_NETLINK`, `AF_UNIX`; `socketpair` stays), reads of the store directory, the runtime, system libraries, ONNX Runtime and the manifest's model files and `in/`; writes of `out/`, the per-project plugin data directory when granted, a private temp; GPU only with `gpu`; macOS TCC and Apple Events denied; limits from sp8 §3.3 (8 processes, 2 GiB default, 1,024 files).

### Linux

10. **Landlock** at ABI ≥ 3 (kernel ≥ 6.2 floor, owner decision): `REFER` only inside write roots (so no hard link or rename can bring a file from a read-only or unlisted tree into a write root, which keeps sp5's `probe_beneath` residual user-only), `TRUNCATE` handled, `MAKE_*` only in write roots; TCP `connect` rules at ABI ≥ 4 (the listener's port, 80, 443 for `agent`; nothing for the others, which have no IP socket anyway). **ABI 6 scoping** (abstract sockets and signals) is applied where available, in addition to the seccomp `AF_UNIX` refusal. No code path for ABI < 3: a kernel below 6.2 refuses the profile with the D31 requirement named.
11. **seccomp** (**deferred, owner decision 2026-11-08**: see Task 4; the list below is the specification a person writes the filter from) (`seccompiler`, compiled once per profile at start-up and cached as a `BpfProgram`): architecture check first (native only; x32 numbers and 32-bit compat on x86-64, compat on aarch64 → `SECCOMP_RET_KILL_PROCESS`); `socket()` by family per profile (`agent`: `AF_INET`, `AF_INET6`, and `AF_NETLINK` only with `NETLINK_ROUTE` if Task 0 B5 shows a CLI needs it; `AF_UNIX` → `EACCES`; everything else `EACCES`); `io_uring_setup|enter|register` → `EPERM`; `ptrace`, `process_vm_readv|writev`, `pidfd_getfd`, `keyctl`, `add_key`, `request_key`, `bpf`, `perf_event_open`, `userfaultfd`, `mount`, `umount2`, `pivot_root`, `move_mount`, `open_tree`, `fsopen`, `fsmount`, `unshare`, `setns`, `clone` with namespace flags, `personality`, `kexec_*`, `init_module`, `finit_module` → `EPERM`. `worker` adds the process-creation refusals of Decision 8; `plugin` refuses all IP families.
12. **Status** (`status.rs`, Linux): `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` → ABI; `ENOSYS`/`EOPNOTSUPP` → `Disabled {reason: landlockDisabled}` (F2: run, warn), ABI < 3 → `Unavailable {need: "kernel ≥ 6.2"}`; a seccomp `prctl(PR_GET_SECCOMP)` probe for a container whose profile blocks the calls → `Disabled {reason: landlockDisabled}` as well (sp6 §3.5.1 counts it as disabled). A test hook (`FIN_TEST_SANDBOX_STATUS=disabled`, test-hooks builds only) stubs it.
26. **Unix sockets hidden by a namespace** (**Owner decision (2026-11-08):** with seccomp deferred and Landlock below ABI 9, hide pathname sockets with a mount namespace). Built in Task 5b.
    - **Where:** `fin-sandbox-exec` on Linux, for `agent` and `hook`, only where Landlock is enforced (without Landlock the process could undo its own mounts; F2 already warns that everything runs unconfined). After the close-on-exec step and before Landlock, the launcher calls `unshare(CLONE_NEWUSER | CLONE_NEWNS)` (plus `CLONE_NEWNET` for a `hook` with `net: None`), has a forked helper outside the namespace write `deny` to its `setgroups` and map its uid and gid to themselves (`<id> <id> 1`: a process may not map uid 0 into a namespace it made itself, and the helper's write is the same for every uid; the user's own files keep their owner, other users' show as the overflow id), makes `/` recursively private, lays out the mounts below, returns to its working directory by path (**or to `/` where that directory is now hidden**: the old one would leave its real subtree reachable by relative paths), sets the namespace's `user.max_user_namespaces` to 0 (no nested user namespace), **drops every capability** (securebits `NOROOT`, `NO_SETUID_FIXUP`, `NO_CAP_AMBIENT_RAISE`, `KEEP_CAPS` locked; ambient cleared; bounding set dropped; `capset` to zero: a program started as root would otherwise keep the namespace's capabilities, and Landlock does not refuse `open_tree(OPEN_TREE_CLONE)`, which copies a hidden tree back), then applies Landlock and `exec`s. A Landlock-restricted process can change no mount.
    - **Mount layout.** Hidden roots, each canonicalised, skipped when missing, inside another hidden root, or at or inside a granted root (its content is granted anyway): `/run` (also `/var/run`, `$XDG_RUNTIME_DIR` = `/run/user/<uid>`, the D-Bus system bus, `docker.sock`, `systemd`, snap and `gnome-keyring` sockets), `/var/run` when it is a directory and not a link into `/run`, `/tmp` and `/var/tmp` (X11 `/tmp/.X11-unix`, `ssh-agent`'s `/tmp/ssh-*`, `tmux`, editor sockets), `/dev/shm`, the container managers' socket directories `/var/lib/incus`, `/var/lib/lxd`, `/var/snap/lxd/common/lxd` (root-equivalent for their groups) and WSLg's `/mnt/wslg`, the launcher's `$XDG_RUNTIME_DIR` when it is elsewhere, the input's `home` (`~/.ssh` ControlMaster sockets, `~/.gnupg/S.gpg-agent*`, `~/.docker/run/docker.sock`, IDE and terminal sockets), and **every directory in the policy's `deny_read`, even inside a granted root** (server data with `run/fin.sock` wherever it lives, `~/.ssh` when the home directory is granted as a library root). Each gets an empty `tmpfs` (`mode=0755,size=64k,nosuid,nodev,noexec`). **Kept paths:** every read root, write root and `unix_sockets` entry of the policy (resolved, so `/etc/resolv.conf` keeps `/run/systemd/resolve/stub-resolv.conf`) whose nearest mount above it is a hide is opened `O_PATH` inside the new mount namespace (a bind source must be one of its mounts) before the first `tmpfs`, and bind-mounted back (`MS_BIND | MS_REC`, from `/proc/self/fd/<n>`) at its own path; hides and keeps are mounted shallowest first, so a denied directory inside a kept tree is hidden after the tree is bound; its mount point is created inside the `tmpfs` only (an empty directory, or an empty file for a file; the launcher checks the parent is one of its own `tmpfs` before each `mkdir` or file creation). Each `tmpfs` is then remounted read-only. Only the kept paths keep their real content, so a socket inside a kept root (a user's own socket in a project folder, `fin.sock` when it is granted) stays reachable: Landlock still decides reads and writes there.
    - **Environment:** the launcher removes the variables that name a socket or an agent (`SOCKET_ENV` in `launch.rs`: `SSH_AUTH_SOCK`, `SSH_AGENT_PID`, `GPG_AGENT_INFO`, `DBUS_SESSION_BUS_ADDRESS`, `DBUS_SYSTEM_BUS_ADDRESS`, `DOCKER_HOST`, `CONTAINER_HOST`, `XDG_RUNTIME_DIR`, `DISPLAY`, `WAYLAND_DISPLAY`, `XAUTHORITY`, `PULSE_SERVER`, `TMUX`, `TMUX_PANE`, `SWAYSOCK`, `I3SOCK`, `KITTY_LISTEN_ON`, `WEZTERM_UNIX_SOCKET`, `NVIM`, `NVIM_LISTEN_ADDRESS`, `VSCODE_IPC_HOOK_CLI`, `VSCODE_GIT_IPC_HANDLE`, `GNOME_KEYRING_CONTROL`) in every mode, namespace or not; Task 11's `NEVER_PASSED` stays the first layer.
    - **Network namespace for `hook`** (spec §8.10: no network): a `hook` without `fin` gets an empty network namespace (only a downed `lo`), which closes UDP, `listen` and abstract sockets for it. A hook that declares `fin` keeps the host's namespace (it needs the agent listener on loopback). `agent` keeps it (open egress).
    - **Unavailable** (no unprivileged user namespaces: Ubuntu's `kernel.apparmor_restrict_unprivileged_userns` without Finisher's AppArmor profile, `kernel.unprivileged_userns_clone=0`, `user.max_user_namespaces=0`, a container's seccomp profile): the launcher first tries the namespace in a forked child (so a half-made namespace, which the AppArmor restriction produces by refusing the uid map after `unshare` succeeded, never reaches the program); a refusal there (`EPERM`, `EACCES`, `EINVAL`, `ENOSPC`, `ENOSYS`, `EUSERS` at the namespace, map or mount steps) runs today's behaviour (Landlock alone; the variables are still removed). Any other probe failure (a pipe, `fork` near `RLIMIT_NPROC`, `ENOMEM`, more than one thread) and any failure after the child succeeded is `SANDBOX_UNAVAILABLE` (fail closed), so an agent cannot make a later launch drop the namespace by exhausting a resource. `fin-sandbox-exec --probe` prints `{"namespaces": {"kind": "available" | "unavailable" | "notApplicable", "reason"?}}` (`SandboxLauncherProbe`, generated), run once by `finisherd` (`sandbox/status.ts`); where `agent` or `hook` is enforced and namespaces are unavailable, `server.status`, `finisherd status`, the start-up log and the agent runtime carry `SANDBOX_SOCKETS_EXPOSED {profile, os, reason, fix}` (a warning, never an exit). Test hook: `FIN_TEST_SANDBOX_NAMESPACES=unavailable` (test-hooks builds).
    - **AppArmor** (installer step, sub-project 9): `resources/linux/apparmor/finisher-sandbox-exec` attaches to the installed `fin-sandbox-exec` (`/opt/finisher/bin/` and `/usr/lib/finisher/`; the installer rewrites the path to where it installs) with `flags=(unconfined)` and `userns,`, the shape Ubuntu ships for its own sandboxing tools (**residual:** with `flags=(unconfined)`, a program the launcher `exec`s stays in that profile, so on Ubuntu the agent and its children may create user namespaces as far as the kernel allows; the namespace's `max_user_namespaces` of 0 refuses them inside the launcher's namespace); the installer copies it to `/etc/apparmor.d/` and loads it with `apparmor_parser -r` where `/proc/sys/kernel/apparmor_restrict_unprivileged_userns` is 1. Checked in tests by syntax only (`apparmor_parser -Q` where installed); never loaded by tests.
    - **Not covered:** `worker` (its self-sandbox runs after the job envelope starts and must stay single-threaded for `unshare(CLONE_NEWUSER)`; hiding `/run` needs GPU validation on 6B hardware, since Mesa and the Vulkan loader read parts of it; and the status would need a probe of the `fin-workers` binary, which needs its own AppArmor profile: a follow-up, with a network namespace for workers as its cheap first step) and `plugin` (applied in `pre_exec` inside the host's own binary, which the AppArmor profile does not cover). Sockets inside granted roots (a user's socket in a project folder), sockets in other places of the filesystem outside every hidden root, and abstract sockets for `agent` (ABI ≥ 6 scopes them) remain. The "available" path runs only where user namespaces work (the escape suite as root in a privileged container on hosts with the AppArmor restriction): CI has no such job yet (Task 13 adds one). The read-only remount of each `tmpfs` is defence in depth behind Landlock (which already refuses writes there), so no test can observe it alone.
    - **Residuals after it:** UDP and `listen` stay for `agent` (and for a `hook` that declares `fin`) until seccomp; on hosts without namespaces, everything of Task 4's residual list stays, with the warning.
13. **The agent listener** (spec §8.3, sp5 §4.11): `finisherd` listens on `127.0.0.1:<ephemeral>` with the local socket's framing and handshake (`server.hello`/`auth.prove`, canonical endpoint `tcp:127.0.0.1:<port>` inside both MACs), refuses any principal but `agent` and `mcp` **after** the proof (audited `auth.fail {reason: listenerKind}`), and is started only on Linux and only when a Linux session or hook needs it. `FIN_SERVER=tcp:127.0.0.1:<port>`; `crates/fin/src/endpoint.rs` gains `tcp:` (loopback addresses only; anything else is `USAGE`). The listener's port joins the `agent` profile's TCP allow-list (ABI ≥ 4).

### macOS

14. **Seatbelt** (spec §8.2): the SBPL is generated from the policy (`(deny default)`, the read roots, write roots, `(deny file-write* file-link* (subpath protected))`, `(deny file-read* (subpath denied))` then the session directory's allow, last match wins); `mach-lookup` default-deny with the allow-list from `data/macos/mach-<profile>.txt` (Task 0 B6); `process-exec` of `open`, `osascript`, `launchctl`, `security` and setuid helpers denied as defence in depth; loopback denied except Finisher's own listener; `fin` reaches `<server-data>/run/fin.sock` by `path-literal` plus `file-read-metadata` on its ancestors (sp5 §4.11). Agents and hooks go through `sandbox-exec -p`; workers through `sandbox_init` in `self_apply` (the same profile text); plugins by program rewrite. Parameters are passed with `-D` and quoted by the generator (a path with `"` or `\` is escaped; a test feeds hostile path names).
15. **No keychain by default.** A harness whose sign-in lives only in the login Keychain gains the Security server in its own allow-list file, decided per CLI by Task 0 B7 and recorded in `agent.providers` (spec §8.2).

### Windows

16. **AppContainer** (spec §8.4): one container profile per project (`finisher-<project-uuid>`) for agents and hooks, one per (plugin, project) for plugins, one per (family) for workers; **a per-session capability SID** derived from the session id (`S-1-15-3-…` from a SHA-256 of `finisher-session-<id>`, the same derivation `DeriveCapabilitySidsFromName` uses) in each agent process's `SECURITY_CAPABILITIES`; ACLs on the session's directories grant that capability SID, never the container SID (Task 0 B7 verifies; fallback in Decision 17); a private window station and desktop; a kill-on-close job object with process and memory limits and `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`; only the standard handles inherited (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` stays the host's own for workers, sp5's frozen interface; the launcher sets its own for agents). No `internetClient` capability except for `agent` (open egress, owner decision 5); none for `hook`, `worker`, `plugin`; no loopback exemption for any.
17. **If capability SIDs are not honoured for a normal AppContainer** (Task 0 B7), the fallback is the spec's (§22): one container profile per session, with the broker granting that container SID per session; Task 9 implements whichever B7 confirms and records it in the spec, so only one path ships.
18. **The ACL broker** (`crates/fin-node/src/acl_broker/`): grants and revokes `{path, sid, mask, inherit}` through `SetNamedSecurityInfoW` on allow-only ACEs, each first written to a write-ahead log in `<server-data>/run/acl-broker.wal` (append, fsync, then apply; done marker after), replayed at start (a grant whose session or job is gone is revoked; an incomplete revoke is completed), so a crash between grant and revoke never leaves a stale grant (the S8 lost-grant race). One broker per server; the napi surface is `aclGrant`, `aclRevoke`, `aclSweep`, used by `agent/sandbox.ts` (sessions), the jobs runner (per-job grants for workers and plugins) and the warm pool (Decision 19). The pipe's DACL grows per project through `PipeServer.allow(sid)` (`crates/fin-node/src/pipe_server.rs:119`); no second pipe implementation.

### Jobs

19. **Per-job grants for warm workers** (closing sp5 Decision 35): a warm worker is no longer granted `<server-data>/jobs/` but **its own directory `w.dir`**, and every job handed to it has its job directory created under `<w.dir>/jobs/<jobId>/` instead of `<root>/<jobId>/` (`jobs/files.ts` `jobDirs(root, id)` gains the parent; the hand-off creates or moves the directory before `start`, on the same volume, and deletes it after `ready`, as a cold job's is). Landlock and Seatbelt grants can only narrow, so this is the shape that lets one restriction at start cover exactly the worker's own jobs; on Windows the broker grants each job's directory to the worker's capability SID before `start` and revokes it after `ready`, so the grant is per job there too. **Under a sandbox, a warm key includes the project id**, so a warm worker reads only one project's sources and library roots (**Owner decision (2026-11-08):** one warm worker per open project and family under a sandbox, with grants per project, accepted despite the memory per worker: a hostile media file can then only reach its own project). With both, `caps().perJobGrants` is true and `warm.ts`'s `sandboxed()` gate (`apps/finisherd/src/jobs/warm.ts:83-93`, and its uses at `:185`, `:249`, `:322`, `#sandboxFound` at `:401-415`) admits warm kinds again. A cold worker already gets only its `tmp/` and each mapped file at its own mode (`jobs/runner.ts:232-252`), not its job directory; the warm hand-off keeps that rule for what it maps, and the planted-link checks of `jobs/fsafe.ts` (symlink, `nlink !== 1`, FIFO) run on the moved directory as on a cold one.

### Residuals of other sub-projects

20. **`probe_beneath` keeps `RESOLVE_NO_XDEV` unset** (**Owner decision (2026-11-08):** keep `RESOLVE_NO_XDEV` off; crossing mounts stays allowed and the sandbox stops agents creating mounts or hard links). Setting it would refuse every project whose `sources/` or a library root under it is a mount point (a NAS share mounted into a project folder, a second disk), which is a supported layout. Instead 7C makes the residual hold by construction: no sandboxed process can create a mount (seccomp refuses the mount family; Seatbelt has no mount right; an AppContainer cannot mount) or a hard link from outside its write roots into the project folder (Landlock `REFER` only inside write roots; Seatbelt `file-link*` denied outside write roots; Windows needs write access to the source file, which the container lacks), and Task 13's suite proves each, so only the user's own file access can plant either.
21. **The plugin store is read-only inside every profile** (sp8 §16.1 follow-up): `<server-data>/plugins/store/` is in `protect` for `plugin` (read only, its own package directory) and in `deny_read` for the other three. **Owner decision (2026-11-08):** sp8's host does NOT re-hash a plugin's tree before its first start in each run; it relies on the read-only store for sandboxed processes. Residual: tampering by the user's own unsandboxed processes is not detected. 7C does not change the installer.
22. **WER** (sp5 §6.12; **Owner decision (2026-11-08):** the installer, sub-project 9, excludes `fin-workers` and `fin-sandbox-exec` from Windows Error Reporting per user; this also stops crash reports for those binaries reaching Microsoft, and Finisher's own diagnostics still record their crashes). Workers already turn crash UI off in-process (`crash_ui_off`, `crates/fin-jobs/src/worker/mod.rs:190-202`: `SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX)` and `WerSetFlags(WER_FAULT_REPORTING_NO_UI)`, after the self-sandbox entry). 7C extends the same to processes that are not ours: `fin-sandbox-exec` sets the error mode before `CreateProcessW` without `CREATE_DEFAULT_ERROR_MODE`, so agents, hooks and plugins inherit it, and every AppContainer process gets the container's private desktop, so a dialog, if one were raised, is on a desktop nobody sees and cannot block the user's. `__fastfail` still reaches the WER service, which may start a headless `WerFault.exe -u`; `fastfail_dies_without_wer` (`crates/fin-workers/tests/host_windows.rs:240`, assertion in `crates/fin-jobs/tests/worker_runtime.rs:478-484`) is changed to assert what matters (no window on any desktop, no report in `ReportArchive`/`ReportQueue`), not the absence of the process. Excluding `fin-workers` and `fin-sandbox-exec` from WER at install (`WerAddExcludedApplication`, a per-user registry key) belongs to sub-project 9's installer.

### Reporting

23. **One status source** (`apps/finisherd/src/sandbox/status.ts`): `sandboxStatus()` from fin-node per profile → `server.status.warnings` entries `SANDBOX_DISABLED {profile, os, reason: landlockDisabled, fix}` (the `fix` names the `lsm=` boot parameter) for each disabled profile, `PLUGINS_UNCONFINED` for plugins (sp8's name), `SANDBOX_DISABLED {worker}` with `media.capabilities.sandbox: none` (sp6's names); the `sandbox.unconfined {profile, os, reason}` event once per profile per server start and on change. `agent/warnings.ts`'s constant `before7C` warning is replaced by a call into it. A profile not yet in `READY` on this OS keeps the warning its sub-project already shows (`reason: before7C` for agents becomes `reason: profileNotReady` with the same fix text pointing at the release notes).
24. **Violations** (spec §8.8): only what the launcher or host observes (seccomp `SIGSYS`, job-object limit messages, a broker refusal, a failure to apply) becomes an audit entry `sandbox.violation {session?, jobId?, op, profile, rootKind}` and, for a session, a `notice` node. Paths are stored as root kind plus keyed hash. Finisher never reads the macOS unified log.

### Node tools in a Windows container

25. **Node piped stdio on Windows** (S9 Windows point 8). **Owner decision (2026-11-08): Node piped stdio on Windows.**
    - **The problem.** Inside an AppContainer, libuv names its stdio pipes outside `\\.\pipe\LOCAL\` (`\\?\pipe\uv\…`). The container refuses those names and libuv retries forever, so a Node `child_process` spawn with piped stdio hangs (`spawn`, `spawnSync`, `execSync`; the `spawnSync` timeout does not fire). This hits npm scripts, Node hooks that run commands, and Node MCP servers started by a CLI. Pipes the parent makes outside the container, Rust `std::process` with pipes, and Bun (Claude Code) are unaffected.
    - **Decision: ship with the limit, but fail fast.** On Windows, a Node process inside the container ends with a clear error naming the limit instead of freezing. The detection is the Node guard of Task 9 Step 3 (a preload that probes pipe creation from a worker thread, so a stuck main thread cannot hide it), with the launcher's wall-time limit as the backstop for a Node process that did not load the guard. Task 9 Step 3 records which detection was verified.
    - **Node flags.** Node launched inside the container carries `--preserve-symlinks --preserve-symlinks-main` (S9 point 8a: Node's `realpath` `lstat`s `C:\`, which a container cannot read). Task 9 Step 3 also tests the `NODE_OPTIONS` form, so children of children inherit both the flags and the guard.
    - **Upstream.** Task 9 Step 4 prepares a small libuv patch that creates stdio pipes under `\\.\pipe\LOCAL\` when running in an AppContainer. The patch stays in `archive/` (git-ignored) and **the owner files it upstream**. Agents must not open upstream issues or pull requests.
    - **User-facing limit.** "Node tools that spawn child processes with piped stdio (npm scripts, Node hooks, Node MCP servers) fail with an error inside the Windows sandbox." Recorded in the README limits of Task 15. It lifts once Node ships the fix; the guard's probe then passes and stops blocking, so no Finisher change is needed.

## Frozen interfaces

```rust
// crates/fin-sandbox/src/policy.rs
pub enum Profile { Agent, Hook, Worker, Plugin }
pub enum ReadPolicy { Only(Vec<PathBuf>), #[cfg(test)] Everything { except: Vec<PathBuf> } }
pub enum Net { Open, Loopback { ports: Vec<u16> }, None }
pub struct Limits { pub processes: Option<u32>, pub memory: Option<u64>, pub open_files: Option<u32>, pub wall_ms: Option<u64> }
pub struct SandboxPolicy { pub profile: Profile, pub write: Vec<PathBuf>, pub read: ReadPolicy, pub protect: Vec<PathBuf>,
    pub deny_read: Vec<PathBuf>, pub unix_sockets: Vec<PathBuf>, pub net: Net, pub mach: Vec<String>,
    pub devices: Vec<DeviceRule>, pub limits: Limits }
impl SandboxPolicy { pub fn validate(&self) -> Result<(), PolicyError>; }   // absolute, normalised, no write root under a protect root, …

// crates/fin-sandbox/src/profile/*.rs (inputs are schemars types, exported to TS)
pub fn agent(i: &AgentInput) -> Result<SandboxPolicy, PolicyError>;
pub fn hook(i: &HookInput) -> Result<SandboxPolicy, PolicyError>;
pub fn worker(i: &WorkerInput) -> Result<SandboxPolicy, PolicyError>;
pub fn plugin(i: &PluginInput) -> Result<SandboxPolicy, PolicyError>;

// crates/fin-sandbox/src/status.rs
pub enum Mechanism { Enforced { detail: String }, Disabled { reason: DisabledReason }, Unavailable { need: String } }
pub fn status(profile: Profile) -> Mechanism;
pub const READY: &[(Profile, Os)];

// crates/fin-sandbox/src/self_apply.rs — registered with fin_jobs::sandbox::register
pub fn self_apply(req: &fin_jobs::sandbox::SelfSandbox) -> Result<fin_jobs::sandbox::SelfApplied, String>;   // Task 3: `Nothing` where Landlock is disabled (F2), so `hello` says `none`

// crates/fin-sandbox/src/spawner.rs
pub struct SandboxSpawner { /* status cache, compiled filters, broker handle (Windows) */ }
impl fin_jobhost::Spawner for SandboxSpawner { /* name "fin-sandbox", sandboxed(), prepare() */ }
```

```ts
// apps/finisherd/src/agent/sandbox.ts
export interface SessionSandbox {
  readonly env: Readonly<Record<string, string>>;            // spec §8.5, FIN_PROJECT_DIR included
  readonly status: SandboxMechanism;                          // enforced | disabled | unavailable
  spawn(argv: readonly string[], opts: { cwd: string; stdio: "pipe"; extraRead?: readonly string[] }): Effect<SandboxedChild, FinError>;
}
export function openSessionSandbox(s: Session, harness: Harness): Effect<SessionSandbox, FinError, Scope>;
export function assertSessionEnv(env: Readonly<Record<string, string | undefined>>, s: Session): void;  // 7D/7E call it on their launch env
```

## Review Focus

1. **Denial is observed, never assumed:** every escape case fails when its rule is removed (mutants in commit bodies). → Tasks 2, 4, 5, 8, 9, 13.
2. **Nothing of server data is reachable from any profile** except the session directory's own files and the socket or listener: `auth/`, `settings.json`, `control.db`, the journal, the trace store, the audit log, `providers/` other than the harness's store file. → Task 13 "server data" block on three OSes.
3. **No unconfined start by accident:** an enabled mechanism that fails to apply refuses the start (`SANDBOX_UNAVAILABLE`); only `Disabled` runs, and it is always visible (status, event, session notice). → Tasks 3, 11.
4. **The Linux socket story:** `AF_UNIX` refused for `agent` and `hook`, io_uring refused, x32 and compat refused, and `fin` still works over the listener. → Tasks 4, 6.
5. **Between sessions:** a session cannot reach another's directory, credential, provider directory, temp or workspace, on any OS (Windows by capability SID or the fallback). → Tasks 9, 10, 13.
6. **Warm workers see only their own jobs and project.** → Task 12.
7. **The broker never loses a grant or leaves a stale one** across crashes. → Task 10.
8. **The policy crosses into TS as typed inputs only**; no hand-written policy type. → Task 1, gate.

---

### Task 0: Spike S9 in `archive/` (controller-run; can start now) — tier: opus (controller)

**Not done yet** (TASKS.md, sub-project 7: "[ ] S9 spike in `archive/` (plan Task 0)"; the 7A plan carries the same Task 0, which this one replaces: run it once, for both). It resolves the open facts of spec §8.12 before Tasks 4–10 commit to details. Nothing from it is committed except its conclusions.

**Files:** Create (git-ignored): `archive/s9/{notes.md, linux/, macos/, windows/, h2/}`. Modify: `TASKS.md` (the S9 line), spec §8.2–8.6 and §4.5–4.6 with the conclusions.

- [~] **Step 1 (B5, Linux; kernel ≥ 6.2):** *Linux Landlock status query done (see S9 results (Linux)); seccomp moved to Task 4; the Landlock-disabled boot and the toolchain read list are still open.* on the Linux VM, a 6.2-series kernel, and a supported kernel booted with `landlock` left out of `lsm=`: a seccomp filter that checks the architecture and refuses x32/compat, io_uring, `AF_UNIX` and the other calls of Decision 11, each denial observed; libuv and the installed Codex and Claude Code still run (the io_uring fallback included); the per-CLI validation list of spec §8.3 (SSSD/LDAP user lookup, gpg-agent signing, git credential-cache, fsmonitor) and whether `AF_NETLINK`/`NETLINK_ROUTE` is needed; whether any newer Landlock ABI mediates pathname sockets; on the Landlock-disabled boot, that the status probe of Decision 12 says `Disabled` and a process runs. Record the base toolchain read list for `data/linux/toolchain-base.txt`.
- [x] **Step 2 (B6, macOS 14+):** *done, see S9 results (macOS).* under a log-only `(deny default)` profile, the minimal `mach-lookup` list per CLI and for `fin` and `fin-workers`; with default-deny, `open`, `osascript`, `launchctl` and `security` fail, nothing starts outside the sandbox, and a test keychain item created for the spike is unreadable. Record `data/macos/mach-*.txt` candidates and the base toolchain list.
- [ ] **Step 3 (B7, Windows still in progress elsewhere; the user's own sign-in, never copied):** with a test account a person signs in to through each CLI's own flow, in a sign-in home: where each CLI keeps its store per OS (file or keychain; keyed by config directory?); whether a per-session `CLAUDE_CONFIG_DIR`/`CODEX_HOME` holding a link to the store works and refreshes **in place**; parallel sessions refreshing at once; a file link on Windows without elevation; else the fallback of spec §8.6; that `system/init` and `account/read` report the sign-in kind and an account label; **per-session capability SIDs on a session directory under a normal AppContainer**, and that a second session's process cannot open it (decides Decision 16 or 17). No spike script opens a credential file.
- [ ] **Step 4 (H2):** *moved to 7D/7E or a later S9 rerun.* Claude Code under `-p` stream-JSON without `--bare` (`--setting-sources user` with a per-session config dir, `--strict-mcp-config`, the `CLAUDE.md`/`.claude/` read denial: planted settings, hooks, MCP servers and memory files observed not loaded); `--append-system-prompt`; `system/init` before the first prompt; `--max-budget-usd`; `--disallowedTools`; Codex `-c project_doc_max_bytes=0` and `developer_instructions` at the pinned version.
- [ ] **Step 5:** conclusions into the spec (each marked *verified on <host class>*), tick `TASKS.md`. **Commit** `docs: record spike S9 conclusions` (spec and `TASKS.md` only).

#### S9 results (macOS)

*Verified on Apple silicon (M4), macOS 27.0.1, non-elevated, 2026-11-08.* Versions: Node 24.22.2, Claude Code 2.1.319, Codex CLI 0.166.0, wgpu 30.0.1. Spike code and raw logs are kept outside the repo. This settles Step 2 (B6). Step 3 (B7) is only partly settled: the sign-in flows were run up to the point where a person must act, and nothing was signed in.

**`sandbox-exec` and SBPL on this macOS.** The man page marks `sandbox-exec` DEPRECATED, but it works on 27.0.1: `-f`, `-p`, `-D` with `(param …)`, last match wins, and `(deny default)`. `sandbox_init_with_parameters` (the `self_apply` path) accepts the same profile text and parameters, and after it a process cannot fork or read outside the policy. Findings that shape the generator and its tests:
- Rules match resolved paths. A rule on `/etc/hosts` is a no-op, and `/private/etc/hosts` works. The generator canonicalizes every path (`/etc`, `/var` and `/tmp` live under `/private`).
- Denials are reported by default. `(with report)` on a deny rule is a profile error ("report modifier does not apply to deny action").
- launchd's `denied lookup … Sandbox restriction` lines persist and `log show` finds them. The kernel's `Sandbox: proc(pid) deny(1) op …` lines for `sandbox-exec` processes appear only to a `log stream` started before the run, never to `log show`. Escape tests assert on errno and behaviour, not on logs.
- The denial log is not the full list. A lookup only appears once the code gets that far: VideoToolbox encode logs no `videoencoder` lookup until `trustd.agent` is allowed. Every list below was confirmed by leave-one-out runs, not read off the log.
- `(deny default)` refuses three things with no extra rule: exec of setuid binaries (`forbidden-exec-sugid`), `fork`, and `lsopen` (LaunchServices opening an app or URL).

**Method.** Discovery ran under `(allow default)(deny mach-lookup)` with an allow-list, minimized by leave-one-out. Confirmation ran under a full `(deny default)` profile shaped like the target profile (`agent`: read roots, write roots, network; `worker` and `hook`: exec of the one binary only, no fork). Each run used an empty environment and a throwaway `HOME`. Credentials were dummies only.

**Minimal `mach-lookup` per workload (full default-deny confirmed):**

| Workload | Minimal `mach-lookup` | Other non-default rules it needed | Without it |
|---|---|---|---|
| Node 24.22.2, finisherd-like (fs, crypto, Intl, loopback HTTP, Unix-socket server, `sh` + `git` children, `worker_threads`, `os.userInfo`, DNS, TLS `fetch`) | `com.apple.system.opendirectoryd.libinfo` | `network-bind`/`network-outbound` on `(local\|remote unix-socket (subpath …))` for its own socket; `network-outbound (remote unix-socket (path-literal "/private/var/run/mDNSResponder"))` for DNS | `os.userInfo()` throws `ENOENT` (`uv_os_get_passwd`); everything else works. DNS uses the mDNSResponder **Unix socket**, not mach; TLS uses Node's bundled roots. |
| Claude Code 2.1.319 (`auth status`, `-p` round trip to a 401 on a dummy key) | none (`libinfo` recommended for its shells and `git`) | `CLAUDE_CODE_TMPDIR` pointed into a write root | Nothing breaks. Denied lookups it attempts: `SecurityServer`, `securityd.xpc` (keychain), and `FSEvents` (settings hot reload will not fire). It also tries to exec `/usr/bin/security` 4 times. **Without `CLAUDE_CODE_TMPDIR` it exits with `EPERM` on `/tmp/claude-<uid>`**, which it uses whatever `TMPDIR` says. |
| Codex CLI 0.166.0 (`login status`, `exec` round trip to a 401 on a dummy key) | `com.apple.cfprefsd.daemon`, `com.apple.cfprefsd.agent` (+ `libinfo` recommended) | `ipc-posix-shm-read-data (ipc-posix-name-prefix "apple.cfprefs.")`; `CODEX_CA_CERTIFICATE` or `SSL_CERT_FILE` set to `/etc/ssl/cert.pem` | Without either cfprefsd service it exits at start with "Failed to synchronize managed preferences". Without a CA bundle env var, its platform TLS verifier needs `com.apple.trustd.agent` **and** `com.apple.SecurityServer` (`stage=Tls` errors otherwise). It does not need `user-preference-read`: cfprefsd checks that operation itself, so allowing the lookup does not open other apps' preference domains. |
| wgpu 30.0.1 on Metal (adapter, device, compute dispatch, readback) | `com.apple.MTLCompilerService` | `iokit-open-user-client` of `AGXDeviceUserClient` and `IOSurfaceRootUserClient` (`iokit-get-properties` not needed) | Without the compiler service: an adapter is found but `request_device` returns `Device(Lost)`. Without the user clients: no adapter. |
| VideoToolbox hardware decode, H.264 and HEVC 1080p (`VTDecompressionSession` with hardware required, and `AVAssetReader` to BGRA) | `com.apple.coremedia.videodecoder` | `iokit-open-user-client` of `IOSurfaceRootUserClient` | Without the service: `VTDecompressionSessionCreate` returns −12911, and `AVAssetReader` reports −11821 "Cannot Decode". **Without `IOSurfaceRootUserClient` it fails silently: the session is created and reports hardware, but it delivers 0 frames with no error.** |
| VideoToolbox hardware encode, H.264 and HEVC (`VTCompressionSession`; not in Task 0's list, but workers make proxies) | `com.apple.coremedia.videoencoder`, `com.apple.trustd.agent` | `IOSurfaceRootUserClient` | Without `trustd.agent`: −12908. Without `IOSurfaceRootUserClient`: a crash (SIGTRAP). |
| `fin`-like Unix-socket client (Rust std `UnixStream`) | none | `network-outbound (remote unix-socket (path-literal "<fin.sock>"))` plus `file-read-metadata` on its ancestors | It works with no other server-data read, and a file next to the socket stays unreadable. `getpwuid` needs `libinfo`, so `fin` takes its paths from env and args, never from the passwd database. |

Lookups attempted by every workload and denied with no observed effect: `com.apple.logd`, `com.apple.diagnosticd`, `com.apple.system.notification_center`, `com.apple.analyticsd`, `com.apple.bsd.dirhelper`, `com.apple.system.opendirectoryd.membership`, `com.apple.CoreServices.coreservicesd`, `com.apple.DiskArbitration.diskarbitrationd`, `com.apple.SystemConfiguration.{configd,DNSConfiguration}`, `com.apple.PerfPowerTelemetryClientRegistrationService`, `com.apple.powerlog.plxpclogger.xpc`, and, for `AVAssetReader`, `com.apple.tccd.system` and `com.apple.windowserver.active`.

**Default-deny escape checks (agent-shaped profile):**
- Exec of `open`, `osascript`, `launchctl` and `security` is denied (`EPERM`).
- A mutant that allows those execs while mach stays default-deny shows the next layers:
  - `open -a` and `open <url>` fail: the LaunchServices lookups (`lsd.mapdb`, `lsd.modifydb`, `coreservicesd`) are denied.
  - `osascript` fails before any Apple Event, because it cannot load its scripting components.
  - `launchctl submit` fails on the Seatbelt `job-creation` operation.
  - `security` cannot reach the keychain.
  - No marker file appeared, no launchd job was added, and no app started.
- **Allowing `mach-lookup` globally still does not let `open` launch an app**, because `(deny default)` refuses `lsopen`.
- A private named pasteboard (`NSPasteboard(name:)`, written outside the sandbox) reads as `nil` under the allow-list and returns the marker under global `mach-lookup`. It is the case that only the mach allow-list protects.
- Keychain: a spike-only item created with `-A` (any application may read it) cannot be read under the profile. It became readable only when **both** `com.apple.SecurityServer` and `file-read*` on `~/Library/Keychains` were allowed. With all files readable, `SecurityServer` alone was enough. The two layers are independent. The item was deleted afterwards.
- `ls ~/Library/Keychains` is denied.
- With `(remote ip "localhost:<port>")` only that loopback port connects, and another listening port is refused.
- With no IP rule (the `hook` shape), name resolution fails.
- In the `worker` shape, `fork` and `exec` both return `EPERM`.

**Sign-in observations (B7 partial, no account signed in):**
- **Claude Code.** `claude auth login` with `open` denied prints the authorize URL and asks for a pasted code. It opens no loopback listener and makes no network call before the paste. With the keychain lookups denied, it reads the file store `.credentials.json` in `CLAUDE_CONFIG_DIR`: a dummy token showed `loggedIn: true`, `authMethod: claude.ai`. With `CLAUDE_CONFIG_DIR` and `HOME` both redirected, it still tries to read `.claude/CLAUDE.md` under the passwd home directory. The profile's `deny_read` of `~/.claude` (Decision 6) stopped it and is required.
- **Codex.** `codex login status` with a fresh `CODEX_HOME` attempts no keychain lookup, so its store is the file by default. The browser flow (`codex login`) needs a loopback listener on `127.0.0.1:1455` plus LaunchServices, so it cannot run under the `agent` profile. `codex login --device-auth` reaches the code prompt with only the `agent` list above.

**Base toolchain reads (`data/macos/toolchain-base.txt` candidate).** These are verified for the `agent` shape:
- `literal "/"`, `/usr`, `/bin`, `/sbin`, `/System`, `/Library/Apple`, `/private/etc`, `/private/var/db/timezone`, `/private/var/db/dyld`, `/private/var/select/sh`, `/Library/Preferences/Logging`;
- `/dev/{null,zero,random,urandom,tty,dtracehelper}`;
- the active developer directory (`xcode-select -p`; only the whole subtree was verified);
- `file-read-metadata`, `sysctl-read`, `system-socket`, `ipc-posix-shm` on the `apple.shm.notification_center` prefix, and the mDNSResponder socket above.

The `worker` and `hook` shapes need `literal "/"` (dyld refuses to start without it), `/usr/lib`, `/System`, `/private/var/db/dyld`, and `/private/etc/ssl` only for TLS clients.

The `/usr/bin` developer shims (`git` and friends) are a trap. With a fresh `HOME` or `TMPDIR`, the `xcrun` cache is empty, so the shim runs `xcodebuild`, which needs all of `Xcode.app` plus LaunchServices, FSEvents and configd. Under default-deny it fails ("developer directory … isn't accessible"). The real binary in `$(xcode-select -p)/usr/bin` needs only `libinfo`.

**Recommendations for Task 8:**
1. `mach-agent.txt`: `com.apple.system.opendirectoryd.libinfo`, `com.apple.cfprefsd.daemon`, `com.apple.cfprefsd.agent`, plus `ipc-posix-shm-read-data` on the `apple.cfprefs.` prefix. Keep `user-preference-read` denied.
2. `mach-hook.txt`: empty, since `fin` needs none. Add `libinfo` only if hook shells need user lookups. They ran without it here.
3. `mach-worker.txt`: `com.apple.coremedia.videodecoder`, `com.apple.coremedia.videoencoder`, `com.apple.trustd.agent` (encode only), and `com.apple.MTLCompilerService`. Add `iokit-open-user-client` of `IOSurfaceRootUserClient` (every media job) and `AGXDeviceUserClient` (wgpu). Only Apple silicon was tested; Intel and AMD Macs use other user-client classes.
4. `mach-plugin.txt`: empty. The `gpu` grant adds the worker's GPU rows.
5. Never put these in a profile list: `SecurityServer`, `securityd.xpc`, `lsd.*`, `coreservicesd`, `launchservicesd`, `windowserver.*`, `tccd*`, `pboard`, `FSEvents`. Decision 15 stands. Neither CLI tested needs the keychain:
   - Claude Code reads its file store.
   - Codex runs on the file store and a CA bundle.
6. The adapters set these environment variables:
   - 7D sets `CLAUDE_CODE_TMPDIR` to the session's `tmp/`.
   - 7E sets `CODEX_CA_CERTIFICATE=/etc/ssl/cert.pem`.

   A user whose TLS roots live only in the keychain (a corporate proxy) needs `trustd.agent` and `SecurityServer` in the Codex list, which is a per-provider opt-in under Decision 15.
7. Resolve developer shims when the policy is built (`xcrun --find <tool>`) and put the real directory first on `PATH`. Never allow `xcodebuild`.
8. The generator canonicalizes paths (`/private/...`) and never emits `(with report)` on a deny rule.
9. The worker checks that decode returns a frame count above zero, because a missing `IOSurfaceRootUserClient` fails silently.
10. Escape tests assert on errno and behaviour. Where a test needs a log, it starts `log stream` before the run.
11. **The Step 2 mutant ("allow `mach-lookup` globally; the Terminal case must fail") does not trip on the Terminal case**: the exec denial and `lsopen` still block it. Add the private-pasteboard read as the case that must fail under that mutant. Keep the Terminal, `osascript` and `launchctl` cases for their own layers.

**Not tested:**
- B7 completion: a real sign-in and token refresh in place, parallel refreshes, and whether Claude Code writes `.credentials.json` at login when the keychain is denied (only the read path was tested).
- The real `fin` and `fin-workers` binaries. Probes with the same APIs stood in: std `UnixStream`, wgpu, Swift VideoToolbox and AVFoundation.
- macOS 14–26 and Intel or AMD GPUs.
- Split DNS or VPN with `DNSConfiguration` denied.
- ONNX Runtime and CoreML for `plugin` with `gpu`.
- TCC prompts on Documents and Desktop. These are outside every read root, so file rules deny them first.
- `appleevent-send` to Finder beyond `osascript` failing to start.
- The Metal shader-cache cost of denying `bsd.dirhelper`.
- Codex's keyring store mode.

#### S9 results (Linux)

**Tested** (Ubuntu 24.04, kernel 6.8.0 generic, glibc 2.39; active LSMs `lockdown,capability,landlock,yama,apparmor`):

- `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` returns ABI **4**. The `ERRATA` flag returns 5, so the distro has backported fixes 1 and 3.
- Kernel config: `CONFIG_IO_URING=y`, `CONFIG_X86_X32_ABI` not set, `CONFIG_SECCOMP_FILTER=y`.
- Under Docker's default seccomp profile the Landlock version query still returns 4, but `prctl(PR_GET_SECCOMP)` returns 2 and `/proc/self/status` shows `Seccomp: 2`. With `seccomp=unconfined` it returns 0.

**Recommendations for Task 3 / Decision 12:**

- `PR_GET_SECCOMP == 2` must **not** be read as "Landlock disabled": every default Docker container reports it while Landlock works. The Landlock status comes from the version query alone.
- Map `EPERM` from the version query to `Disabled`, as well as `ENOSYS` and `EOPNOTSUPP`. Some older container seccomp profiles answer unknown syscalls with `EPERM` (unverified).

**Not tested yet:**

- A real Landlock-disabled boot. A static status probe and kernels (6.8 and mainline 6.2) were prepared for a throwaway QEMU VM, but none was booted. Expectation, unverified: `EOPNOTSUPP` when Landlock is built in but left out of `lsm=`, `ENOSYS` when it is not built.
- B5 seccomp (architecture/x32 check, io_uring refusal, the socket policy, and how a refused call appears to Rust, Node and glibc) **moves to Task 4**. Build and test it there with an established, permissively licensed Rust seccomp library (for example `seccompiler`, Apache-2.0, at least 7 days old), not a hand-written C filter. Note for Task 4: glibc 2.34+ falls back from `clone3` to `clone` only on `ENOSYS`, so refusing `clone3` with `EPERM` would likely break threads; use `ENOSYS` for refused syscalls that have a fallback (untested).
- The base toolchain read list, and whether a newer Landlock ABI mediates pathname sockets.
- H2, all of it (the Claude Code isolation flags, `system/init`, `--max-budget-usd`, `--disallowedTools`, the Codex `-c project_doc_max_bytes=0` and `developer_instructions` checks). It moves to 7D/7E or a later S9 rerun. Versions seen: Codex 0.167.0, Claude Code 2.1.319.

#### S9 results (Windows)

*Verified on a Windows 11 Pro host (build 26300; AMD discrete and integrated GPUs) on 2026-11-08: a non-elevated, medium-integrity user process in the console session (started through Task Scheduler), a normal AppContainer (not LPAC), Node 24.22.2, wgpu 30.0.1, `windows` 0.62.2. Spike code and raw logs stay outside the repository, on the host and on the VM. No credential file was opened: the sign-in checks used dummy store files, and Claude Code's behaviour was read from the embedded code of the installed 2.1.319 binary.*

**Verdict: Decision 16 holds; Decision 17's fallback is not needed.** Capability SIDs derived from a name are honoured for a normal AppContainer, on files, directories and named pipes, and a process of session B cannot open session A's directory.

1. **Capability SIDs (B7, Decision 16).**
   - `DeriveCapabilitySidsFromName(n)` is `S-1-15-3-1024-` followed by the eight little-endian 32-bit words of SHA-256 over the UTF-16LE of **`n` upper-cased**. The derivation is case-insensitive, and Rust can compute it without the API (checked against an independent SHA-256 for three names).
   - **Interaction with the owner-only DACL.** The session directory sits below `private_dir`'s protected owner-only DACL (`D:P(A;OICI;FA;;;<user>)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)`) and carries an inheritable allow ACE (`0x1301bf`: modify, no `WRITE_DAC`/`WRITE_OWNER`) for session A's capability. Results:
     - Project X's container with capability A can read, create, list and `mkdir` there.
     - The same container with capability B, or with no session capability, is refused (`ERROR_ACCESS_DENIED`) for A's files and A's listing.
     - Another project's container that carries capability A is admitted. A capability is not bound to a container; the launcher alone decides which capability a process gets.
     - A grant to the container SID (a project directory) admits that project only.
   - **Objects the container creates** inherit the capability ACE and the owner-only ACEs, and get a Low label (`S:(ML;;NW;;;LW)`). Capability B cannot read them.
   - **Owner rights don't reach the container.** It can neither change the DACL (`WRITE_DAC` → 5) nor take ownership of the session directory or of its own files, although the user owns both.
   - **Revoke** applies to the next open. Handles that are already open were not tested.
   - **Ancestors.** The token keeps `SeChangeNotifyPrivilege` (with `SeIncreaseWorkingSetPrivilege` only), so open-by-path needs no grant on ancestors. `<server-data>` and `agents/` stay unlistable and un-stat-able.
   - **Broker cost** per grant or revoke (process start subtracted): about 1 ms on an empty session directory, about 105 ms with 2,000 files below it (inheritance propagation).
2. **Images and the toolchain.**
   - The executable the launcher starts is opened with the launcher's token and needs no grant.
   - Every image the container opens itself (a child process, a non-system DLL) needs read and execute. Without it, `CreateProcess` inside the container fails with 5.
   - **One static capability SID granted RX once on the toolchain roots, and added to every sandboxed process, works** (a child exe started from inside through that grant alone). Per-container grants on large trees are therefore unnecessary.
3. **The server pipe (Task 10).** The pipe used `pipe_sec::sddl`'s descriptor (`D:P(A;;GA;;;<user>)(A;;GA;;;SY)(A;;0x12008b;;;<container SID>)S:(ML;;NW;;;LW)`), and the client opened it with `GENERIC_READ | FILE_WRITE_DATA`, as `fin` does.
   - Project X's container connects, and the server reads its container SID from the client's token.
   - Project Y's container is refused with 5, whatever unrelated capabilities it holds.
   - **`GENERIC_READ | GENERIC_WRITE` is refused** (the ACE grants no append, EA or attribute writes), so keep `fin`'s mask.
   - Without the Low label the container still connected on this build. Keep the label.
   - A capability SID in the pipe DACL works as well, but `peer_allowed` checks the container SID, so `PipeServer.allow(<container SID>)` per project stays the design.
   - **A container cannot create a named pipe outside `\\.\pipe\LOCAL\`** (5). Inside `LOCAL\`, which redirects into its own namespace, it can.
4. **Job objects.**
   - **Nesting:** the launcher was already in a job (Task Scheduler's), and a launcher that puts itself in another job first can still give the container its own kill-on-close job.
   - **Assignment at creation:** `PROC_THREAD_ATTRIBUTE_JOB_LIST` works together with `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`, so no suspended-then-assign window is needed.
   - **Active-process limit:** with a limit of 1, every `CreateProcess` from inside fails with `ERROR_NOT_ENOUGH_QUOTA` (1816) and the completion port gets `ACTIVE_PROCESS_LIMIT` (3). With a limit of 3, two children start.
   - **Memory limit:** at 64 MB, allocation fails at 62 MB and the port gets `JOB_MEMORY_LIMIT` (10).
   - **Inside the container:** it can create a job and assign its own child (nested jobs are not blocked). `CREATE_BREAKAWAY_FROM_JOB` is refused (5) unless the job has `BREAKAWAY_OK`, which therefore must never be set.
   - **Launcher death:** killing the launcher killed the container.
   - **WER:** a WerFault started for the container is not counted in its job.
5. **Desktop.**
   - A **named** `CreateWindowStationW` fails with 5 from this non-elevated, medium-integrity context. An **unnamed** one (`NULL` name, as Chromium does; read the name back with `GetUserObjectInformationW`) works.
   - Inside the container, `OpenInputDesktop` is refused.
   - Without a private desktop, the container started on the user's `WinSta0\Default`.
6. **Network.**
   - With no capability, TCP to the internet fails with `WSAEACCES` (10013).
   - With `internetClient`, it connects.
   - Loopback to a listener of the same user timed out with and without `internetClient`; there is no exemption.
7. **The environment block is rewritten.**
   - **What changes:** creating an AppContainer process replaces `TEMP`, `TMP` and `LOCALAPPDATA` with `<LOCALAPPDATA of the block>\Packages\<container name>\AC\Temp` and `…\AC`, and the launcher's `TEMP` is ignored.
   - **The leak:** by default `TEMP` is per container, that is per project, and session B read a file that session A wrote there.
   - **The fix (verified):** the launcher sets `LOCALAPPDATA=<session>\localappdata` in the block and pre-creates `<session>\localappdata\Packages\finisher-<project-uuid>\AC\Temp`, which inherits the session's capability ACE. `TEMP` then lands in the session directory and session B is refused.
   - **Nested processes:** a process the container starts keeps the `TEMP` it is given; there is no rewrite.
   - Other per-container folders (`AC\INetCache`, cookies) were not tested.
8. **Node 24.22.2 in the container.**
   - **Works:** file operations in granted roots, rename, `fs.watch`, `worker_threads`, crypto, `os.userInfo`, `os.networkInterfaces`, `fs.realpathSync.native`, and a `net` client on the server pipe. DNS and HTTPS work only with `internetClient`. The server-data root stays unreadable. Child processes with `stdio: 'inherit'` or `'ignore'` work.
   - **(a) Script paths fail.** Node's JS `realpath` (used for the main script and for every `require` by path) `lstat`s each ancestor, including the drive root. On this host `C:\` and `E:\` have no `ALL APPLICATION PACKAGES` ACE, and neither do `C:\Users`, the profile or `AppData`. So `node script.js` fails with `EPERM, lstat 'C:\'`. `--preserve-symlinks --preserve-symlinks-main` on the command line avoids it (main script plus relative `require` verified), and so does `node -e`. The `NODE_OPTIONS` form was not tested.
   - **(b) Piped stdio hangs.** `spawn`, `spawnSync` and `execSync` with **piped stdio hang forever**, and the `spawnSync` timeout does not fire. libuv names its stdio pipes `\\?\pipe\uv\…`; the container may not create them (point 3), libuv reports `EADDRINUSE` and retries forever. So Node tools that spawn with pipes from inside a container hang: npm scripts, Node hooks that run commands, and Node MCP servers started by the CLI.
   - **Unaffected:** pipes the parent creates outside the container (`finisherd` starting a Node plugin host). Rust `std::process` with pipes works inside, and Claude Code 2.1.319 (Bun) starts (`--version`). S8 ran real Claude Code turns with PowerShell.
9. **wgpu over D3D12**, with no capability (toolchain read only): enumerates the discrete, integrated and WARP adapters, picks the discrete GPU, and runs a 1M-element compute pass with a correct readback. This holds on a private desktop and on the user's desktop.
10. **Media Foundation**, with no capability:
    - **Decode:** `SourceReader` decodes a 4 s 720p H.264 MP4 both through a D3D11 DXGI device manager (120/120 frames in DXGI surfaces, so DXVA) and in software.
    - **Encode:** the vendor's hardware **encoder** MFTs (H.264, HEVC, AV1) enumerate and activate.
    - **Sharing:** a D3D11 texture shared by NT handle opens on D3D12 (`OpenSharedHandle`).
    - **Decoder MFTs:** hardware decoder MFT enumeration returns none on this GPU, unsandboxed as well; decode goes through DXVA.
    - **Grants:** the media file itself needs a grant; without one, `0x80070005`.
    - sp6's "to be proven in 6B's validation" is answered for this GPU only.
11. **WER (Decision 22).**
    - **Error mode set before `CreateProcess` and inherited:** an access violation in the container starts no WerFault and logs no Application Error event.
    - **`__fastfail` (`int 0x29`) still reaches WER.** The service starts two headless `WerFault.exe -u` (one parented to the crashing process, one to the service). This holds with `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`, and also with a per-user `WerAddExcludedApplication` entry.
    - **No visible trace:** no window appeared (no WerFault had a main window), and no report directory appeared under the per-user or machine `ReportArchive`/`ReportQueue`.
    - **Exclusion not provable here:** this host wrote no report for unsandboxed crashes either, so the exclusion's effect on reports could not be observed.
    - **Without the error mode,** both crash kinds start WerFault and log event 1000.
    - **Conclusion:** the inherited error mode is required, and the reworded `fastfail_dies_without_wer` assertion (no window, no report; not "no process") is the right one.
12. **The sign-in store shared in place (B7, Windows).**
    - **Links without elevation.** A file symbolic link is refused (no `SeCreateSymbolicLinkPrivilege`, Developer Mode off), so §8.6's Windows file symlink is unavailable on a default machine. A hard link works (same volume). The file has one DACL under both names: an ACE for one shared sign-in capability on the store file alone let sessions A and B read through their own links, and an in-place write through A's link was seen by B. A could not list the sign-in home or create a file in it, and without the capability it was refused. A directory junction can be created, but the target's ACL still applies.
    - **Rename breaks the link.** A temp-and-rename write through A's hard link replaced A's name with a private file and left the home stale.
    - **Claude Code 2.1.319: no link works.** The CLI's code applies on every OS.
      - Its file store is `<dir>\.credentials.json`, with `<dir>` = `CLAUDE_SECURESTORAGE_CONFIG_DIR` when set, else the config directory.
      - It writes by temp file (`<file>.tmp.<hex>`) and rename. Its in-place fallback **refuses a target with more than one link** (`EMLINK`, "it is hard-linked"), and its reader **refuses symlinks** (`O_NOFOLLOW`, "refused-symlink").
      - The first refresh therefore replaces a hard link, and a symlink is refused.
      - A server-side flag (`tengu_windows_credman`, cached in `.claude.json`) or `CLAUDE_CODE_FORCE_WINDOWS_CREDMAN=1` moves the Windows store to Credential Manager, under a target name keyed by the store directory (`Claude Code-credentials-<first 8 hex of SHA-256 of the dir>`). A container cannot reach Credential Manager (S8).
    - **What should work instead (recommendation for 7D and spec §8.6; not exercised with a real sign-in):**
      - The session gets a per-session `CLAUDE_CONFIG_DIR` plus `CLAUDE_SECURESTORAGE_CONFIG_DIR=<server-data>/providers/claude/store/`, a directory that holds only the store. The user's sign-in command sets both variables.
      - The sandbox grants the shared sign-in capability modify on that one directory, since temp and rename need create and delete there, and nothing else of `providers/`.
      - Claude Code loads nothing from that directory as configuration, so the persistence escape stays closed.
      - The variable is undocumented, so re-validate it per CLI version; §8.6's fallback (the sign-in home as config directory) remains the fallback.
    - **Codex** keeps `auth.json` as a file in its home. Its write pattern was not checked here (compiled binary); B7 on Linux/macOS decides, since the code is the same.

**Recommendations for Task 9.**
- **Capabilities:**
  - Implement Decision 16 as written. Derive the session capability in Rust (SHA-256 of the upper-cased UTF-16LE `finisher-session-<id>`), and make point 1's matrix the escape test.
  - Add two **static** capabilities to every sandboxed process: one for the read-only toolchain roots (granted once, point 2) and, for agents, one per harness for the sign-in store (point 12), instead of granting the store to every session's capability.
- **Process set-up:**
  - `desktop.rs` creates the window station unnamed.
  - `launch.rs` sets `LOCALAPPDATA` in the block to a per-session directory and pre-creates the derived `AC\Temp`, and also sets `TMPDIR`.
  - Assign the job with `PROC_THREAD_ATTRIBUTE_JOB_LIST`, and never set `BREAKAWAY_OK`.
  - Map `ACTIVE_PROCESS_LIMIT`/1816 and `JOB_MEMORY_LIMIT` to `sandbox.violation`.
- **`worker` profile:** it can stay capability-free for D3D12, DXVA decode and the MF encoders.
- **Escape and compatibility suite:** add Node's root `lstat` and the hang on piped `spawn` from inside the container. **Decided (owner, 2026-11-08):** see Decision 25 and Task 9 Steps 3 and 4: ship with the limit but fail fast, carry `--preserve-symlinks --preserve-symlinks-main`, and prepare a libuv patch in `archive/` for the owner to file.

**Recommendations for Task 10.**
- **Session directories:**
  - The broker creates them with the capability ACE already in the creating security descriptor (nothing to propagate).
  - Their revoke is their deletion at session end, still recorded in the WAL for the sweep.
  - Grants on existing trees cost about 50 ms per 1,000 files.
- **Pipe:** `PipeServer.allow(<container SID>)` as planned. `fin` keeps `GENERIC_READ | FILE_WRITE_DATA`, and the Low label stays.

**Not tested.**
- Real sign-in, refresh in place and parallel refresh. `system/init` and `account/read` labels (they need a person signing in).
- Codex's store writer.
- `CLAUDE_SECURESTORAGE_CONFIG_DIR` with a real sign-in.
- The `NODE_OPTIONS` form of the Node flags.
- Bun's own piped spawn inside a container (relied on S8).
- Other GPU vendors and hybrid laptops. HEVC without the extensions. Hardware decoder MFTs.
- WER report creation on a host that keeps reports.
- Handles that stay open across a revoke.
- A second local account.
- The other per-container folders.

### Task 1: `fin-sandbox`: the policy, the profiles and enumeration — tier: opus — 7C-1

**Files:**
- Create: `crates/fin-sandbox/Cargo.toml` (features `test-hooks`), `crates/fin-sandbox/src/{lib.rs, policy.rs, enumerate.rs, error.rs, inputs.rs}`, `crates/fin-sandbox/src/profile/{mod.rs, agent.rs, hook.rs, worker.rs, plugin.rs}`, `crates/fin-sandbox/data/{linux,macos,windows}/toolchain-base.txt` (placeholders until Task 0, each with a header naming its source).
- Modify: `Cargo.toml` (workspace member), `crates/fin-store/examples/export_schema.rs` (the `*Input` types), `packages/core-types` (generated), `deny.toml` if needed, `TASKS.md`.
- Test: `crates/fin-sandbox/tests/{policy.rs, enumerate_props.rs, profiles.rs}`.

**Interfaces:** the frozen `policy.rs` and `profile/*.rs` above; `enumerate::write_roots(write, protect, deny) -> Vec<PathBuf>`, `enumerate::read_roots(read, deny) -> Vec<PathBuf>`; `AgentInput {sessionId, mode, harness, projectDir, workspaceDir, sharedDir?, libraryRoots, sessionDir, signInStore?, toolchains, serverData, home, listener?}`, `HookInput`, `WorkerInput {jobDir, tmp, sources, caches, devices, warm?: {workerDir, libraryRoots}}`, `PluginInput` (sp8 §16.1 fields).

- [ ] **Step 1: Write failing tests:**
  - `validate_refuses_relative_unnormalised_and_overlapping` (a write root under a protect root; `..`; a relative path);
  - `enumerate_props` (proptest over random trees: no enumerated write root contains a protected or denied path; the union of enumerated roots equals write minus protected; read roots never contain a `deny_read` path; deterministic order);
  - `agent_plan_has_no_shared_write`, `agent_reads_only_allow_list` (no `$HOME`, no `/`, no server data but the session directory's listed files), `agent_reads_same_project_workspaces_only` (Decision 6: another session's `workspace/<id>/` of the same project is readable and not writable, another project's is neither), `agent_claude_denies_claude_md` (harness `claudeCode`), `agent_protects_project_state` (`.finisher/`, `assets/`, `finisher.project.json`, `finisher.rules.json`, `credential.json`, `settings/`);
  - `worker_cold_vs_warm` (cold: the job's `in/`, `out/`, tmp; warm: the worker directory, never the jobs root), `worker_net_none_processes_one`;
  - `plugin_matches_sp8_16_1` (a table test: every row of sp8 §16.1);
  - `hook_net_none_unless_declared`;
  - `inputs_schema_is_generated` (drift gate covers the new types).
- [ ] **Step 2: Implement.** Paths are canonicalised by the caller (`finisherd` resolves links before building inputs; `validate` refuses a non-canonical path), so the policy never follows a link itself.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(sandbox): fin-sandbox policy, profiles and root enumeration`.

### Task 2: Linux Landlock and the `worker` profile's Landlock phase — tier: opus — 7C-1

**Files:**
- Create: `crates/fin-sandbox/src/linux/{mod.rs, landlock.rs, apply.rs}`, `crates/fin-sandbox/src/self_apply.rs`, `crates/fin-sandbox/src/bin/fin-sandbox-probe.rs` (test-hooks only: performs one named attempt and prints `{op, errno}`).
- Modify: `crates/fin-workers/Cargo.toml` and `crates/fin-workers/src/main.rs` (`fin_jobs::sandbox::register(fin_sandbox::self_apply)` before `run()`), `TASKS.md`.
- Test: `crates/fin-sandbox/tests/linux_landlock.rs`, `crates/fin-sandbox/tests/escape_worker_linux.rs`.

**Interfaces:** `linux::landlock::ruleset(policy: &SandboxPolicy, abi: Abi) -> Result<RulesetCreated, ApplyError>`; `linux::apply::restrict_self(policy)`; `self_apply` accepts `SelfSandbox {profile: "worker", args: WorkerInput}` and builds and applies the Landlock part (seccomp joins in Task 4).

- [ ] **Step 1: Write failing tests** (Linux, Landlock enabled; skipped with a printed reason where `status()` says `Disabled`, and that skip is itself asserted in Task 3's stubbed run):
  - the S8 file cases as data (`tests/cases/files.toml`: protected roots, links, renames, truncation, read denials), each run by `fin-sandbox-probe` under `restrict_self` and passing only on `EACCES`/`EPERM`/`EXDEV`;
  - **`hard_link_into_write_root_refused`** and **`rename_across_roots_refused`** (`REFER` only inside write roots; Decision 10);
  - `worker_cold_reads_only_manifest` (a source not in the manifest is `EACCES`), `worker_writes_only_out_and_tmp` (`in/` is read-only, `out/` writable, the job directory itself not), `worker_warm_sees_only_its_dir` (a sibling job directory under the jobs root is `EACCES`);
  - `self_apply_fails_closed` (an unreadable `FIN_SELF_SANDBOX` and a policy that cannot apply exit 78 before `hello`, through `fin_jobs`' test worker);
  - `abi_below_3_refused` (stubbed ABI 2 → `Unavailable {need: "kernel ≥ 6.2"}`).
- [ ] **Step 2: Implement.** One mutant per block (drop the `REFER` restriction; widen `in/` to write), recorded in the commit.
- [ ] **Step 3: Run** the gates on the Linux VM → PASS; `cargo check` for the two other targets. **Commit** `feat(sandbox): Linux Landlock and the worker profile's Landlock phase`.

### Task 3: Landlock disabled runs everything with warnings (F2) — tier: sonnet — 7C-1

**Files:**
- Create: `crates/fin-sandbox/src/status.rs`, `crates/fin-core/src/events/payloads/sandbox.rs` (`sandbox.unconfined {profile, os, reason}`, metadata only), `crates/fin-node/src/sandbox.rs` (napi `sandboxStatus()`), `apps/finisherd/src/sandbox/status.ts`.
- Modify: `crates/fin-core/src/events/{mod.rs, names.rs}` (the kind; append-only snapshot), `crates/fin-node/src/lib.rs`, `apps/finisherd/src/main.ts` (one call), `apps/finisherd/src/server-admin.ts:36-48` (today `warnings: agentWarnings()` only: it gains the per-profile entries), `apps/finisherd/src/jobs/layer.ts:97-98` (the start-up line reads the status), `crates/fin-core/src/events/payloads/media.rs` (`Sandbox::Landlock` reported when applied, sp6 §15), `crates/fin/src/output.rs` (`SANDBOX_DISABLED` is a warning, never an exit), `TASKS.md`.
- Test: `crates/fin-sandbox/tests/status.rs`, `apps/finisherd/test/sandbox-status.test.ts`.

- [ ] **Step 1: Write failing tests:** with `FIN_TEST_SANDBOX_STATUS=disabled` (test-hooks): **every profile starts** (a worker job runs; `self_apply` with a `worker` request applies nothing and the worker reports `selfSandbox: "none"`), `server.status.warnings` holds `SANDBOX_DISABLED {profile: worker, reason: landlockDisabled, fix}` with the `lsm=` fix and `fin status` prints it, `sandbox.unconfined` is recorded once per profile per server start and again on change, **no `SANDBOX_UNAVAILABLE`, no `agents.unsandboxed` setting exists** (spec §16.2); with `unavailable`, a worker job fails `SANDBOX_UNAVAILABLE {profile: worker, os, need}`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(sandbox): mechanism status, SANDBOX_DISABLED warnings and sandbox.unconfined`.

**After Task 3, 7C-1 merges** and 6B-1 can apply the worker profile through `FIN_SELF_SANDBOX` (its media kinds' `WorkerInput`, plus its GPU device rules).

### Task 4: The seccomp filter — tier: opus — 7C-2 — **DEFERRED (owner decision 2026-11-08)**

> **Deferred.** **Owner decision (2026-11-08):** Linux ships with Landlock only for now; the seccomp filter is an open hardening item. **Reason:** an automated safety filter twice stopped agents from writing the filter, so a person writes it later from the syscall list of Decision 11 and Step 1 below; agents then review it, wire it into `linux/apply.rs`, `self_apply.rs`, the launcher and the `Spawner`, and add the tests of Step 1. **Agents do not write seccomp filter code.** Later tasks skip their seccomp steps and say so in place (Tasks 5, 6, 7). **Residual risk until it lands:** sandboxed processes keep more of the kernel's interface: process creation in the `worker` profile at the syscall level (Landlock `EXECUTE` still limits `exec` to the worker binary, but `fork`/`clone` are not refused), `ptrace` within the sandbox's own Landlock domain (Landlock already refuses tracing anything outside it), `process_vm_*`, `pidfd_getfd` within that domain, `bpf`, `perf_event_open`, `userfaultfd`, `keyctl`, the mount and namespace calls (an unprivileged mount needs a user namespace, which then is not refused either), `io_uring` and the x32 and 32-bit compat entry points; and **`AF_UNIX` is not refused, so `agent` and `hook` processes can connect to any pathname socket their user may open** (a D-Bus session bus, `ssh-agent`, `gpg-agent`, `docker.sock`, X11, `fin.sock`), because Landlock below ABI 9 (`RESOLVE_UNIX`, applied where the kernel has it) does not mediate `connect()` on pathname sockets (**narrowed by Decision 26**: where user namespaces are available the launcher hides the usual socket locations and gives a hook without network an empty network namespace; elsewhere `SANDBOX_SOCKETS_EXPOSED`); `plugin` processes likewise keep every socket family Landlock does not handle (UDP, raw, netlink, `AF_UNIX`; TCP `connect` and `bind` stay refused at ABI ≥ 4). **UDP for every profile:** Landlock handles TCP only, so a `hook` (network "none", Decision 7) and a `worker` can still send UDP anywhere, and an `agent` can reach loopback UDP services that spec §8.7 puts out of reach (Task 5 asserts it: `udp_is_a_residual_until_seccomp`). **`listen` on an unbound TCP socket** accepts connections on an ephemeral port in every profile (Decision 6a). Reads, writes, `exec`, links, renames, truncation, TCP ports (ABI ≥ 4), abstract sockets and signals (ABI ≥ 6) are still confined by Landlock. Recorded in TASKS.md and the README's limits.

**Files:**
- Create: `crates/fin-sandbox/src/linux/{seccomp.rs, syscalls.rs}`.
- Modify: `crates/fin-sandbox/src/linux/apply.rs` and `self_apply.rs` (Landlock then seccomp, both before `hello`), `TASKS.md`.
- Test: `crates/fin-sandbox/tests/linux_seccomp.rs`, `tests/cases/linux.toml`.

**Interfaces:** `linux::seccomp::program(profile: Profile, arch: Arch) -> BpfProgram` (cached per profile); `syscalls.rs` holds the refusal tables per profile and arch as data.

- [ ] **Step 1: Write failing tests:**
  - unit tests of the compiled filter (spec §16.2): the first instructions check `AUDIT_ARCH_*`; an x32-tagged number and a 32-bit compat call evaluate to `KILL_PROCESS`; `io_uring_setup` to `EPERM`; `socket(AF_UNIX, …)` to `EACCES` for `agent` and `hook`; `socketpair` allowed;
  - escape cases under `fin-sandbox-probe` (spec §8.11 Linux): connecting to a D-Bus session bus, an `ssh-agent`, a `gpg-agent`, a fake `docker.sock` and an X11 socket (each a pathname socket the test creates) is refused; a socket created through io_uring is refused (its entry points are); an x32 call kills the process (`SIGSYS` seen by the parent); `pidfd_getfd` of the test's own parent, `ptrace`, `bpf`, `mount`, `unshare(CLONE_NEWUSER)` refused; `worker`: `fork`, `clone` without `CLONE_THREAD` and `execve` of `/bin/sh` refused; `plugin`: `AF_INET`, `AF_INET6`, `AF_NETLINK`, `AF_UNIX` refused, `socketpair` works;
  - `libuv_falls_back_without_io_uring` (a Node child under the `agent` filter reads and writes files).
- [ ] **Step 2: Implement.** Mutants: drop the arch check; allow `AF_UNIX`.
- [ ] **Step 3: Run** the gates on the Linux VM → PASS. **Commit** `feat(sandbox): Linux seccomp filter with architecture check, socket policy and io_uring refusal`.

### Task 5: The launcher and the Linux `agent` and `hook` profiles — tier: opus — 7C-2

**Files:**
- Create: `crates/fin-sandbox/src/bin/fin-sandbox-exec.rs`, `crates/fin-sandbox/src/launch.rs` (policy from an inherited fd, apply, exec), `crates/fin-sandbox/data/linux/{toolchain-base.txt, git-config}` (from Task 0 B5).
- Modify: `crates/fin-jobhost/src/spawner.rs` `BUNDLED` (add `fin-sandbox-exec`), the bundled-binaries list in `apps/finisherd` (where `fin` and `fin-workers` are found), `TASKS.md`.
- Test: `crates/fin-sandbox/tests/escape_agent_linux.rs`, `crates/fin-sandbox/tests/launcher.rs`.

- [ ] **Step 1: Write failing tests:** the launcher refuses a policy on argv or in the environment (only `--policy-fd`), refuses an invalid policy (`SANDBOX_UNAVAILABLE`), exits with the child's code and forwards signals; under `agent`: **reading `<server-data>/auth/local.key`, `auth/console.key`, `auth/credential.master`, `settings.json`, `control.db`, the journal, the trace store and the audit log is `EACCES`** (a test server-data tree); writing `~/.claude/settings.json` and `~/.codex/config.toml` (test HOME) refused; anything under `<server-data>/providers/` other than the harness's store file refused; another session's directory refused; the project's `CLAUDE.md` unreadable for `claudeCode`; an IP connect to the allowed loopback port works and (ABI ≥ 4) to another port is refused; `git status` and `git commit` work under the session's git configuration (`GIT_CONFIG_GLOBAL`, `GIT_CONFIG_NOSYSTEM=1`, `core.fsmonitor=false`, empty `credential.helper`, `commit.gpgsign=false`) with `HOME`, `USER`, `LOGNAME`, `SHELL` exported (spec §8.3 validation list); `hook`: no IP socket, project read-only.
- **Seccomp skipped (Task 4 deferred):** the launcher applies Landlock only; no seccomp filter is built or installed, and no case here depends on one.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates on the Linux VM → PASS. **Commit** `feat(sandbox): fin-sandbox-exec and the Linux agent and hook profiles`.

### Task 6: The agent listener and `fin`'s `tcp:` endpoint — tier: opus — 7C-2

**Files:**
- Create: `apps/finisherd/src/agent/listener.ts`.
- Modify: `apps/finisherd/src/main.ts` (a third `serveRpc` beside the local and WebSocket ones, started on Linux on first need; its endpoint in `listeners`), `apps/finisherd/src/transport.ts` (beside `makeLocalServer`, `:84-93`: a loopback `net.Server` with the same framing), `apps/finisherd/src/connections.ts` (transport `agentTcp`; principal filter after the proof), `apps/finisherd/src/agent/builtin/environment.ts:18` and `apps/finisherd/src/agent/wiring.ts:84` (`FIN_SERVER` "the local socket until 7C's agent listener": the listener on Linux), `crates/fin/src/endpoint.rs` (`Endpoint` at `:22-41` gains `Tcp(SocketAddr)`, loopback only), `crates/fin/src/output.rs` if a code is new, `TASKS.md`.
- Test: `apps/finisherd/test/agent-listener.test.ts`, `crates/fin/tests/cli_endpoint_tcp.rs`.

- [ ] **Step 1: Write failing tests:** the listener binds `127.0.0.1` only, an ephemeral port; an `agent` and an `mcp` credential complete the handshake with the canonical endpoint `tcp:127.0.0.1:<port>` in both MACs (a proof made for another endpoint fails); a `local`, `console` or device principal is refused after the proof and audited (`auth.fail {reason: listenerKind}`); a client that sends nothing is cut after the handshake timeout; the connection cap holds; `fin` with `FIN_SERVER=tcp:127.0.0.1:<port>` and `FIN_CREDENTIAL` works and `tcp:10.0.0.1:1` is `USAGE`; **from inside the `agent` profile on Linux, `fin view` works over the listener and connecting to `fin.sock` fails** (seccomp).
- [ ] **Step 2: Implement.**
- **Seccomp skipped (Task 4 deferred):** "connecting to `fin.sock` fails" needs the `AF_UNIX` refusal; until the filter lands that case is not run, and the suite asserts the residual instead (from inside the `agent` profile `fin.sock` still accepts a connection, below Landlock ABI 9), so the case flips when the filter is wired in.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): Linux agent listener over loopback TCP; fin tcp: endpoint`.

### Task 5b: Unix sockets hidden by a namespace (Decision 26) — tier: opus — 7C-2

**Files:**
- Create: `crates/fin-sandbox/src/linux/namespace/{mod.rs, layout.rs, caps.rs}` (probe and entering; the layout plan, pure; dropping capabilities), `resources/linux/apparmor/finisher-sandbox-exec`, `crates/fin-sandbox/tests/namespace_linux.rs`.
- Modify: `crates/fin-sandbox/src/{launch.rs, status.rs, lib.rs}` (`register_schemas` gains `SandboxLauncherProbe`), `src/bin/fin-sandbox-exec.rs` (`--probe`; the namespace before Landlock; `SOCKET_ENV` removed), `src/bin/fin-sandbox-probe.rs` (`open-tree`, `unshare-user` ops), `tests/{escape_agent_linux.rs, launcher.rs}` (the socket and UDP gap tests; the rest with the namespace off), `crates/fin/tests/cli_endpoint_tcp.rs` (the `fin.sock` gap), `packages/core-types` (generated), `apps/finisherd/src/sandbox/status.ts` (`SANDBOX_SOCKETS_EXPOSED`), `crates/fin/src/output.rs` and `apps/finisherd/src/console/exit-codes.ts` (warning code), `README.md` (limits), `TASKS.md`.

- [x] **Step 1: Write failing tests:** the layout plan (pure: hidden roots dropped when missing, nested or inside a granted root; kept paths only under hidden roots, shallowest first, resolved); with namespaces available: connecting to a socket under `/run` (where the test can create one) and under `$XDG_RUNTIME_DIR` fails with `ENOENT` under `agent` and `hook`, after an unsandboxed control connects; a socket in a granted root stays reachable; `/proc/<parent>/root/…` does not reach a hidden socket; granted files under a hidden root (the resolver file) stay readable; the hook without `fin` is in another network namespace and its UDP send fails; `SOCKET_ENV` is removed in every mode; with `FIN_TEST_SANDBOX_NAMESPACES=unavailable` the program runs under Landlock alone and `--probe` says `unavailable`, and `server.status` carries `SANDBOX_SOCKETS_EXPOSED` for an enforced `agent`/`hook`. The AppArmor profile parses (`apparmor_parser -Q`, skipped with a reason where not installed). Hosts that refuse user namespaces skip the "available" cases with a printed reason (`FIN_REQUIRE_NAMESPACES=1` turns the skip into a failure; the suite runs them as root in a privileged container on a host with the AppArmor restriction).
- [x] **Step 2: Implement.** Mutants: drop `/run` from the hidden roots; skip binding kept paths back (the resolver case fails).
- [x] **Step 3: Run** the gates → PASS. **Commit** `feat(sandbox): hide Unix sockets from agents and hooks with a mount namespace`.

### Task 7: `SandboxSpawner` for 5C (Linux), workers and plugins — tier: opus — 7C-2

**Files:**
- Create: `crates/fin-sandbox/src/spawner.rs`.
- Modify: `crates/fin-node/src/jobhost/mod.rs:180-187` (construct the host with `SandboxSpawner`), `crates/fin-jobhost/src/{lib.rs, host.rs, wire.rs}` (`HostCaps.profiles`, `per_job_grants`, Decision 4), `packages/contracts/src/jobs.ts` (the caps schema), `apps/finisherd/src/jobs/{host.ts, layer.ts}` (`JobHostLike.caps()`; the start-up line reads `profiles`), `crates/fin-registry/src/plan/mod.rs:201-205` (`thirdPartyCode.confined` from the plugin profile's status, as the 8A plan expects), `TASKS.md`.
- Test: `crates/fin-sandbox/tests/spawner_linux.rs`, `crates/fin-workers/tests/host_sandbox.rs`.

**Design (recorded 2026-11-08, before coding): from the job host's grants to a profile input.** The host's `SpawnRequest` stays the frozen shape (`cwd`, `env`, `grants: [{path, mode}]`); `SandboxSpawner::new(server_data)` knows `<serverData>` (fin-node's `JobHostOptions.serverData`, which `finisherd` sets) and maps each request, refusing anything it cannot place (`SpawnError`, which the runner reports as `SANDBOX_UNAVAILABLE`), so a grant is never silently widened or dropped:
- **Decision per profile** (`spawner::decide(profile, mechanism, ready)`): a profile not in `READY` for this OS passes through as `Unsandboxed` did (Decision 5) and reports `disabled`; a ready one is applied where the mechanism is enforced (`enforced`), passes through where Landlock is disabled (F2, `disabled`), and refuses the spawn where the mechanism is unavailable (`unavailable`). `HostCaps.profiles {worker, plugin}` is that answer; `sandboxed()` is true only when both are `enforced`; `per_job_grants` stays false until Task 12. Test hook (test-hooks builds): `FIN_TEST_SANDBOX_READY=worker,plugin` adds rows to `READY` for this OS, read by `status::ready()`. `status()` answers the same for every profile (the Task 3 special case that reported `worker` unavailable before it was ready is dropped: the spawner is the profile's only requester), and a test checks the two agree for every mechanism and table.
- **`worker`** (self-sandbox, Decision 3): `tmp` = the `readWrite` grant that `TMPDIR` in the request's environment names; **cold** (`cwd` strictly inside `<serverData>/jobs/`, never the jobs root): `jobDir = cwd`; a grant inside `cwd/in/` with mode `read`, or inside `cwd/out/` with mode `write`/`readWrite`, is covered by the profile's `in/` and `out/` rules; a `read` grant inside `<serverData>/fonts/` or `<serverData>/cache/` is a cache (any other inside server data is refused); any other `read` grant is a source (canonical, outside server data, per file); anything else (a write grant elsewhere, a grant on the job directory itself) is refused. **Warm** (a request whose `cwd` is a warm worker's directory: `<serverData>/jobs/<name>` with a `readWrite` grant on the jobs root, as `warm-worker.ts` builds it today) is refused while the profile is applied: per-job grants are Task 12, and `warm.ts` runs warm kinds cold until `caps().perJobGrants`. GPU rows: `fin_sandbox::linux::gpu::devices()` for the `media` family. A **helper** (`role: helper`, `cwd` `<job>/helpers/<id>/`) is refused under an applied profile: mapping its own files is a follow-up before the profile becomes ready (Task 16). The policy is built and validated in `prepare` (so a bad input fails the spawn, not the worker), `cmd.self_sandbox = {profile: "worker", args: WorkerInput}` (refused above 120 KiB, below Linux's 128 KiB per environment string; a file or descriptor for large inputs is a follow-up), `job_limits.active_processes = 1`.
- **`plugin`** (`pre_exec`, Decision 3; sp8b Task 16's grants): `callDir = cwd` (one directory inside server data, never server data or the jobs root), `tmp` as for workers; the one `read` grant inside `<serverData>/plugins/store/` is the package directory (`storeDir`; none or several: refused); `read` grants inside `cwd/in/` and `write`/`readWrite` grants inside `cwd/out/` are covered; `read` grants inside `<serverData>/plugins/models/` are `models`, any other inside server data is refused, and those outside are `system` (the runtime included: the profile treats both as read roots); a `readWrite` grant on `/dev/dri/renderD*` is a GPU render node (`gpu`; any other device refused); one other `readWrite` grant outside server data and `/dev` is the per-project data directory (`pluginData {write: true}`); `home` is the host process's `HOME`. `job_limits.active_processes` is the profile's 8 (enforced where leaf limits are); memory stays the group's limit (Decision 4a). In `prepare` (the spawn thread) the spawner builds the Landlock ruleset and keeps its descriptor; the `pre_exec` steps are `prctl(PR_SET_NO_NEW_PRIVS)`, `landlock_restrict_self(fd, 0)` and `setrlimit(RLIMIT_NOFILE)`, raw calls on prepared data (the host marks descriptors close-on-exec, never closes them, so the ruleset's survives to `exec` and closes there).
- **Consumers:** `@finisher/contracts` `JobHostCaps` gains `profiles` and `perJobGrants`; `warm.ts` gates warm kinds per family profile (`profiles[family.profile] === "enforced" && !perJobGrants`, or a worker that sandboxed itself); `server.status`'s worker warning reads `profiles.worker` instead of `sandboxed`; `layer.ts`'s start-up line names both profiles; `fin-registry`'s `PlanRequest` gains `plugin_confined`, set by fin-node from `spawner::plugin_confined()` (the same decision the job host's spawner makes; never by the caller), which `thirdPartyCode {confined}` reports; `KindSpec.reads(extra, params)` gives a cold worker's `read` grants: `media.look` persists the manifest's sources from `prepareMediaJob` (canonical) and the font cache in `extra.reads`, the library kinds read their canonical `root`. A spawn refused with `SANDBOX_UNAVAILABLE` is a deterministic failure (no retry). `finisherd` resolves `<serverData>` once (`realpath`) for the jobs root and the spawner.

- [x] **Step 1: Write failing tests:** `prepare` for `Profile::Worker` sets `self_sandbox {profile: "worker", args}` from `SpawnRequest.grants` and `job_limits.active_processes = Some(1)`; for `Profile::Plugin` adds `pre_exec` steps and no self-sandbox; a worker under the real host applies Landlock + seccomp and reports `hello.selfSandbox = "worker"`; **a worker that tries `setsid`+`fork` (the `test` family's spawn kind) is refused** (closing sp5 §6.2's gap for workers); a plugin test binary cannot open a TCP socket or read server data; a profile not in `READY` for Linux runs as `Unsandboxed` did (Decision 5) and says so in `caps().profiles`; `sandboxed()` is true only when every requested profile is enforced.
- **Seccomp skipped (Task 4 deferred):** workers and plugins get Landlock only ("applies Landlock + seccomp" is Landlock alone; the `setsid`+`fork` refusal and the plugin's socket-family refusals other than TCP wait for the filter, and their cases are not run); the `pre_exec` steps are `prctl` and `landlock_restrict_self` only.
- [x] **Step 2: Implement.** `pre_exec` steps allocate nothing (a test runs them under a `malloc` guard in the child).
- [x] **Step 3: Run** the gates on the Linux VM → PASS. **Commit** `feat(sandbox): SandboxSpawner for the job host on Linux`.

### Task 8: macOS Seatbelt — tier: opus — 7C-3

**Files:**
- Create: `crates/fin-sandbox/src/macos/{mod.rs, sbpl.rs, apply.rs}`, `crates/fin-sandbox/data/macos/{toolchain-base.txt, mach-agent.txt, mach-hook.txt, mach-worker.txt, mach-plugin.txt}` (Task 0 B6), `crates/fin-sandbox/tests/escape_macos.rs`.
- Modify: `launch.rs`, `self_apply.rs`, `spawner.rs` (macOS arms), `status.rs` (`sandbox-exec` present → `Enforced`; missing → `Unavailable`), `TASKS.md`.

- [ ] **Step 1: Write failing tests:** the generator's output for each profile against committed goldens (`tests/golden/macos/*.sb`) and with hostile path names (quotes, backslashes, newlines refused); escape cases (spec §8.11 macOS): server data unreadable as in Task 5; **`open -a Terminal`, `osascript -e`, `launchctl submit` each fail and the process tree shows nothing new outside the sandbox**; a test keychain item whose access list excludes the probe is unreadable and `~/Library/Keychains` is unreadable; loopback ports other than the listener refused; TCC-protected folders (Documents, Desktop) fail closed with no prompt; `appleevent-send` to Finder refused (`plugin`); `worker` cannot `fork` or `exec`; `fin` reaches `fin.sock` and nothing else of server data.
- [ ] **Step 2: Implement.** Mutant: allow `mach-lookup` globally; the Terminal case must fail.
- [ ] **Step 3: Run** the gates on the Mac (non-elevated) → PASS. **Commit** `feat(sandbox): macOS Seatbelt profiles with default-deny mach-lookup`.

### Task 9: Windows AppContainer, job objects and WER — tier: opus — 7C-4

**Files:**
- Create: `crates/fin-sandbox/src/windows/{mod.rs, appcontainer.rs, capability.rs, job.rs, desktop.rs, apply.rs, wer.rs}`, `crates/fin-sandbox/tests/escape_windows.rs`.
- Modify: `launch.rs` (error mode before `CreateProcessW`, Decision 22), `spawner.rs` (Windows arm: the AppContainer token for `CreateProcessAsUserW` and the `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` attribute through `PlatformCommand.{token, attributes}`; the host's spawn broker keeps `HANDLE_LIST`, `crates/fin-jobhost/src/windows/{spawn.rs, broker.rs}`; `job_limits {active_processes: 1}` for `worker`), `crates/fin-workers/tests/host_windows.rs:240` and `crates/fin-jobs/tests/worker_runtime.rs:478-484` (`fastfail_dies_without_wer` asserts no window and no report, Decision 22), `TASKS.md`.

- [ ] **Step 1: Write failing tests** (non-elevated session): container profile create/reuse/delete per project; capability SID derivation is stable and distinct per session; **a process of session B cannot open session A's directory** (Decision 16, or 17's fallback per Task 0 B7); the S8 Windows paths (junctions, ADS, 8.3 names, `\\?\`, admin shares, WMI, `schtasks`) refused; Credential Manager denied; server data unreadable; no loopback for any profile; `worker`: a second process is refused by the job's active-process limit; `plugin`: no network capability; the job object kills the tree when the launcher dies; **no window appears on any desktop for an access violation or a `__fastfail`** and no report lands in `ReportArchive`/`ReportQueue` (Decision 22); **a Node child with piped stdio inside the container ends with the `WINDOWS_NODE_PIPES` error within the guard's timeout instead of hanging, and `node script.js` runs with the carried flags** (Decision 25).
- [ ] **Step 2: Implement.**
- [ ] **Step 3 (Decision 25: fail fast on Node piped stdio):** build the Node guard and make the Windows launcher carry it. In `archive/` first, check these on the Windows host and keep the one that works; each is cheap to test:
  - **Guard (the intended design).** A small CommonJS preload shipped with `fin-sandbox-exec` (`crates/fin-sandbox/data/windows/node-guard.cjs`, no dependencies). The launcher sets a marker variable in the container's environment. On the first `child_process` call with piped stdio the guard runs a probe in a `worker_threads` worker: create a named pipe with a libuv-style name (`\\?\pipe\uv\…`) and give up after about 2 s; if it cannot, the call throws an error with code `WINDOWS_NODE_PIPES` and a message naming the limit ("Node child processes with piped stdio are not supported inside the Windows sandbox until Node ships the AppContainer pipe fix; see the Finisher README"). The probe runs off the main thread because the hang is in `uv_spawn` on the calling thread and `spawnSync`'s timeout does not fire. A probe that succeeds caches the result and changes nothing, so the guard goes inert once Node is fixed.
  - **Delivery.** Node started by the launcher gets `--preserve-symlinks --preserve-symlinks-main --require <guard>` on its command line. Also test the `NODE_OPTIONS` form (`--preserve-symlinks --preserve-symlinks-main --require <guard>`), which S9 did not: if it works, set it in the container environment so Node started by npm, hooks and MCP servers inherits both the flags and the guard. Bun and Rust children ignore it.
  - **Backstop.** A Node process that did not load the guard still hangs, so the launcher's wall-time limit (`Limits.wall_ms`, Decision 4a) ends the job tree and reports `sandbox.violation {op: "nodePipes"}` if the tree was alive with no I/O progress at the end. Do not add a CPU-sampling heuristic: it would kill legitimate compute.
  - **Tests:** `node_piped_spawn_fails_fast` (`spawn`, `spawnSync` and `execSync` with piped stdio from inside the container end within 5 s with `WINDOWS_NODE_PIPES`, the process exits non-zero), `node_inherit_and_ignore_stdio_still_work`, `node_script_runs_with_preserve_symlinks`, `guard_inert_when_pipes_work` (the probe against a pipe name the container may create passes and the call goes through).
- [ ] **Step 4 (libuv patch, for the owner to file):** prepare the smallest patch to libuv that creates stdio pipes under `\\.\pipe\LOCAL\` when the process runs in an AppContainer (the pipe-name builder in `src/win/pipe.c`, selected by the process-token's AppContainer flag), with a reproduction (the S9 hang from inside a container) and a test, against a current libuv checkout and Node 24.22.2's bundled libuv. Put the patch, the repro and a short note in `archive/s9/windows/libuv-appcontainer-pipes/` (git-ignored). **The owner files it upstream. Agents must not open upstream issues or pull requests**, and must not push the patch anywhere. Check libuv's licence (MIT) before copying any code.
- [ ] **Step 5: Run** the gates on the Windows host through a non-elevated session → PASS; `cargo check` for the other targets. **Commit** `feat(sandbox): Windows AppContainer profiles, job objects and no-UI fault reporting`.

### Task 10: The ACL broker and per-project pipe SIDs — tier: opus — 7C-4

**Files:**
- Create: `crates/fin-node/src/acl_broker/{mod.rs, wal.rs, apply.rs, sweep.rs}`, `apps/finisherd/src/sandbox/broker.ts` (the napi wrapper; a no-op off Windows).
- Modify: `crates/fin-node/src/lib.rs`, `crates/fin-node/src/sandbox.rs` (`aclGrant`, `aclRevoke`, `aclSweep`), `apps/finisherd/src/main.ts` (sweep before any listener), `apps/finisherd/src/pipe.ts` call sites for `PipeServer.allow(sid)` per project, `TASKS.md`.
- Test: `crates/fin-node/tests/acl_broker.rs`, `apps/finisherd/test/acl-broker.test.ts`.

- [ ] **Step 1: Write failing tests:** grant then revoke leaves the DACL as before (read back); **a crash injected between WAL append and apply, and between apply and the done marker, is completed at the next start** (test hook kills the process at each point); the S8 lost-grant race (two concurrent grants on one directory both survive); a grant for a session or job that no longer exists is revoked by the sweep; a protected root is never granted (refused before the WAL); `PipeServer.allow(container SID)` lets that project's container open the pipe and another project's container still cannot (spec §8.11 Windows).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates on Windows → PASS. **Commit** `feat(fin-node): ACL broker with a write-ahead log and per-project pipe SIDs`.

### Task 11: Session wiring: directories, environment, `spawnInSession` — tier: opus — 7C-5 (after 7A-2 merges)

**Files:**
- Create: `apps/finisherd/src/agent/{sandbox.ts, session-dirs.ts}`, `scripts/gates/check-agent-spawn.mjs` (no `child_process` import under `apps/finisherd/src/agent/` except `sandbox.ts` and the built-in `fin` tool's existing spawn, `agent/builtin/fin-tool.ts:339-347`, which runs our own `fin` with the agent credential and stays outside the OS sandbox as spec §8.9 says).
- Modify: `apps/finisherd/src/agent/warnings.ts` (`AGENT_TOOLS_UNSANDBOXED`, the constant `before7C` warning, replaced by `sandbox/status.ts`, Decision 23), `apps/finisherd/src/main.ts:242-243` (the F2 comment), `apps/finisherd/src/env.ts` (`childEnv`'s extras already carry `FIN_PROJECT_DIR` and `FIN_CREDENTIAL`; `NEVER_PASSED` gains the variables of spec §8.5 it lacks: `SSH_AUTH_SOCK`, `GPG_AGENT_INFO`, `DBUS_SESSION_BUS_ADDRESS`, `DOCKER_HOST`, `DISPLAY`, `WAYLAND_DISPLAY`, `*_API_KEY`; the allow-list already drops them, the list makes the intent tested), `apps/finisherd/src/agent/runtime-engine.ts` (open the session sandbox at start and resume; close it at end), `apps/finisherd/src/agent/credentials.ts` (the credential file under the session directory of §8.5, unchanged path), `packages/contracts/src/agent.ts` (warnings doc), `scripts/gates/*` registration, `TASKS.md`.
- Test: `apps/finisherd/test/agent-sandbox.test.ts`.

**Interfaces:** `SessionSandbox`, `openSessionSandbox`, `assertSessionEnv` (frozen above). `session-dirs.ts`: `ensureSessionDirs(s) → {dir, tmp, provider, settings, git, blobs}` (0700; `credential.json` 0600; Windows: private DACL plus the broker grant to the session's capability SID); `linkSignInStore(harness, s)` creates the link from `provider/` to the store file in `<server-data>/providers/<harness>/home/` and **never opens the target** (spec §8.6).

- [x] **Step 1: Write failing tests:**
  - `session_env_is_exact` (spec §8.5: `childEnv()` plus `FIN_SERVER` (socket, listener or pipe per OS), `FIN_SESSION`, `FIN_CREDENTIAL`, **`FIN_PROJECT_DIR`** (the session's project folder, links resolved), `TMPDIR`, the provider directory variable; never `FIN_SERVER_DATA`, `FIN_AUTHOR`, `FIN_TRAINING`, `FIN_DIAGNOSTICS`, `SSH_AUTH_SOCK`, `GPG_AGENT_INFO`, `DBUS_SESSION_BUS_ADDRESS`, `DOCKER_HOST`, `DISPLAY`, `WAYLAND_DISPLAY`, `CLAUDE_CODE_OAUTH_TOKEN`, `*_API_KEY`), and `assertSessionEnv` throws on each violation (the contract 7D/7E call);
  - **`harness_with_wrong_project_dir_reads_nothing`**: a child of `spawnInSession` that resets `FIN_PROJECT_DIR` to another project's folder gets `PATH_NOT_ALLOWED {reason: projectMismatch}` from `fin caption import --words` (the check of `agent_paths.rs:186-200`), and cannot read that folder at all under the sandbox;
  - **`agent_child_cannot_read_local_key`**: a `spawnInSession` child runs `cat <server-data>/auth/local.key` and `fin --server-data … whoami` without `FIN_CREDENTIAL`: both fail (OS denial), on each OS the suite runs on;
  - `sign_in_store_never_opened` (`finisherd_never_opens_sign_in_store`, spec §16.2: a file-access spy over session start, a scripted child's run and end: no `open` under `<server-data>/providers/` by `finisherd`);
  - `disabled_host_runs_with_notice` (stub `disabled`: the session starts, a `notice` node and `agent.session.get` carry `SANDBOX_DISABLED {reason: landlockDisabled}`); `unavailable_refuses_start` (`SANDBOX_UNAVAILABLE`); `profile_not_ready_keeps_warning` (Decision 23);
  - `violation_is_audited` (a seccomp kill in a session child → `sandbox.violation` with root kind and keyed hash, and a `notice` node).
- **From the Task 5 review:** the session git configuration of `data/linux/git-config` sits at `GIT_CONFIG_GLOBAL`, which a repository's own `.git/config` overrides; pass `core.fsmonitor=false` and the empty `credential.helper` through `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>` as well (command-line precedence), and set a private `GNUPGHOME` (spec §8.3). Start the launcher only where `sandboxStatus()` says the agent profile is enforced or Landlock-disabled (it refuses other `Disabled` reasons with exit 78), and close the request pipe's write end in the child.
- [x] **Step 2: Implement.**
- [x] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(agent): session sandbox, directories and environment; every agent process through fin-sandbox`.

### Task 12: Warm workers under a sandbox: per-job grants — tier: opus — 7C-5

**Files:**
- Modify: `apps/finisherd/src/jobs/{files.ts, warm.ts, warm-worker.ts, warm-run.ts, kinds.ts}` (Decision 19: job directories under the worker's directory for warm hand-offs; the warm key gains the project id under a sandbox; `sandboxed()` admits warm kinds when `caps().perJobGrants`), `apps/finisherd/src/sandbox/broker.ts` (per-job grants on Windows), sp5 spec §6.3 note, `TASKS.md`.
- Test: `apps/finisherd/test/jobs/warm-sandbox.test.ts`, `crates/fin-workers/tests/warm_landlock.rs`.

- [ ] **Step 1: Write failing tests:** with the sandbox enforced and `perJobGrants`, warm kinds run warm again (the hand-off budget of sp5 §6.3 holds within 5 ms on Linux); **a warm worker cannot read a job directory that is not one of its own** (another warm worker's job, a cold job, a plugin job: `EACCES` through a test kind that tries); a warm worker for project A cannot read project B's sources; after `ready`, the previous job's directory is gone; a crash mid-job leaves no directory the next worker can see; on Windows, the broker's per-job grant is revoked after `ready` (DACL read back); without `perJobGrants` (a stubbed host), warm kinds still run cold (the owner decision holds as a fallback).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(jobs): per-job grants for warm workers under a sandbox`.

### Task 13: Escape suites as one runner, in CI on three OSes — tier: sonnet — 7C-5

**Files:**
- Create: `crates/fin-sandbox/tests/cases/{server_data.toml, sessions.toml, links_mounts.toml}`, `crates/fin-sandbox/tests/escape_common.rs` (runs each case file per profile and OS through `fin-sandbox-probe`), `scripts/sandbox-suite.mjs` (`pnpm sandbox:suite`, prints a per-profile, per-OS table).
- Modify: `.github/workflows/ci.yml` (a `sandbox` job per OS, actions pinned to SHAs, as a non-root user on Linux), `package.json` (script), `TASKS.md`.

- [ ] **Step 1: Write the cases** (spec §8.11, every block): S8 core; server data; between sessions (directory, credential, provider directory, temp, workspace; Windows by capability SID); **links and mounts (Decision 20): a hard link from a read root or an unlisted path into `workspace/` is refused; `mount`, `open_tree`, `fsmount` refused (Linux); a junction created in `workspace/` is not followed by `probe_beneath` (Windows, sp5's check) and creating one to a path outside the roots is still only a name (the target stays unreadable)**; `worker` and `plugin` suites (sp8 §16.1 list); the ACL broker's crash cases are in Task 10.
- [ ] **Step 2: Run** `pnpm sandbox:suite` on all three OSes → every case observed denied; record the table in the commit body. **Commit** `test(sandbox): escape suites for every profile on three OSes, in CI`.

### Task 14: e2e `sandbox` scenario — tier: sonnet — 7C-5

**Files:** Create `scripts/e2e/sandbox.mjs`. Modify `scripts/e2e.mjs` (register), `TASKS.md`.

- [ ] **Step 1: Write the scenario** (with the scripted `fake` harness and a session started from the console): the runtime starts a scripted child through `spawnInSession` (a small script standing in for a CLI agent: 7D/7E's fake CLIs replace it later); from inside it: `fin item trim …` lands on the session's branch over the OS's transport (Linux: the listener); `cat` of `auth/local.key` fails; a write outside the workspace fails; a write in `workspace/<sessionId>/` succeeds; `fin caption import --words /etc/hosts` is `PATH_NOT_ALLOWED`; `server.status` shows no `before7C` warning; a media `look` job runs warm under the worker profile (Linux) and its worker cannot see another job's directory.
- [ ] **Step 2: Run** `rtk pnpm e2e` on Linux → PASS. **Commit** `test(e2e): agent processes and warm workers under fin-sandbox`.

### Task 15: Spec as built, cross-references, tracker — tier: sonnet — after Task 14

**Files:** Modify `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` (§8 rewritten "as built": names, data files, codes, the `READY` table, Decisions 16/17 outcome; §1.4 7C row; §18 file map; §19 decisions; §8.9's `before7C` paragraph replaced), `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§4.6, §4.17, §6.2, §6.3, §6.12: each "until 7C" marked closed with a pointer, or left open with the reason), `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` §3.5.1 (the Landlock phase delivered), `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §16.1 (the profile delivered; the store follow-up's outcome), `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 pointer; §8.4 "bwrap fallback" removed), `README.md` (security model: what the sandbox now guarantees per OS, and what F2 hosts lack; **the limits list: on Windows, Node tools that spawn child processes with piped stdio fail fast with `WINDOWS_NODE_PIPES` (Decision 25), until Node ships the fix**), `TASKS.md` (follow-ups).

- [ ] **Step 1: Write** the as-built sections from the code (names, defaults, codes), and every residual that stays open, with its reason.
- [ ] **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 7C as built`.

### Task 16: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

- [ ] **Step 1:** On macOS (14 and the newest), Windows 11 (non-elevated, Defender on) and the Linux VM (kernel ≥ 6.7 for the TCP rules, plus a 6.2-series kernel and a Landlock-disabled boot): `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e`, `pnpm sandbox:suite`.
- [ ] **Step 2:** The reference NAS Docker image (spec §8.3): Docker's default seccomp profile allows the Landlock calls; the image's worker and agent profiles apply; with Landlock disabled on the host, everything runs with the warnings.
- [ ] **Step 3:** The per-CLI validation list (spec §8.3) against the installed Codex and Claude Code under the `agent` profile on each OS (start, a tool call, `git`, `fin` through the OS's transport), as far as 7C's base list allows; per-CLI lists are 7D/7E's.
- [ ] **Step 4:** Update `fin_sandbox::READY` for every (profile, OS) whose suite passed, in its own commit (`feat(sandbox): enable <profile> on <os>`), so the switch to "mandatory" is reviewable on its own (Decision 5).
- [ ] **Step 5:** Record results in TASKS.md and the spec; fix waves as needed; final whole-branch review. **Commit** `docs: 7C validation results`.

## Questions for the owner

**Answered (2026-11-08)**, each recorded in the Decision it concerns as "Owner decision (2026-11-08)": mounts and `RESOLVE_NO_XDEV` (Decision 20); one warm worker per open project (Decision 19); other sessions' workspaces (Decision 6); WER exclusion at install (Decision 22); no plugin-tree re-hash (Decision 21); no opt-out of enforced profiles (Decision 5); Node piped stdio on Windows (Decision 25: ship with the limit but fail fast, carry the symlink flags, libuv patch prepared in `archive/` for the owner to file). No open questions.
