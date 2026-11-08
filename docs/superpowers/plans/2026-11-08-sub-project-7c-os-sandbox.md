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
| WER: `__fastfail` can start a headless `WerFault.exe`; `fastfail_dies_without_wer` is flaky | sp5 §6.12; TASKS.md "Windows run follow-ups" | Decision 22, Task 9 (in-process parts); install-time exclusion stays with sub-project 9 (Question 4) |
| Plugin store integrity: `store/` stays writable to the user's account | sp8 §16.1 "Store integrity (follow-up for 7C)" | Decision 21 (read-only in every profile), Question 5 (re-hash) |
| Other sessions' workspaces are inside the project root | agent §5.6 "Known limits" (7A review L11) | Decision 6 (excluded from the `agent` read policy; Question 3 for the server-side reach) |

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
5. **A profile becomes the default on an OS when its suite passes there, and is then mandatory.** `fin_sandbox::READY: &[(Profile, Os)]` is a compiled table that Task 16 updates per OS as each suite passes on the validation hosts; a profile not in it on this OS runs as today (the env allow-list and its sub-project's own rules, with the warning its spec names: sp5 §6.2, sp8 §3.1). Once in the table it applies to every process of that profile, with no opt-out (sp8 owner decision 1), and a failure to apply it is `SANDBOX_UNAVAILABLE` (the start fails). This keeps third-party plugins running before the plugin suite passes, as sp8 requires, without a setting that could silence the sandbox later.

### Profiles

6. **`agent` read policy** (spec §8.1) is `Only` of: the base toolchain list (`data/<os>/toolchain-base.txt`: the shell, coreutils, `git`, `fin`, the dynamic loader and system libraries, certificates, fonts and locale data), the harness's own list (7D/7E add `data/<os>/toolchain-<harness>.txt`), the project folder **minus `workspace/` other than `workspace/<sessionId>/` and `workspace/shared/`** (closing 7A review L11 on the OS side; Question 3 covers the server-side reach), the referenced library roots, the session directory's `credential.json`, `settings/`, `tmp/`, `provider/`, `git/`, and the harness's sign-in store file. `deny_read` lists server data, the sensitive home paths of spec §8.1, `~/.claude`, `~/.codex`, the rest of `<server-data>/providers/`, and for Claude Code the project's `CLAUDE.md`, `CLAUDE.local.md`, `.claude/`. Writes: `workspace/<sessionId>/`, `workspace/shared/` (not in `plan`), `tmp/`, `provider/`, the sign-in store file. Protect: `.finisher/`, `assets/`, `finisher.project.json`, `credential.json`, `settings/`.
7. **`hook` profile:** the project read-only, the hook's own file, its toolchain (base list), a private temp; `net: None` unless the hook declares network, then the agent listener (Linux) or the pipe (Windows) or `fin.sock` (macOS) only, never IP egress; no credential unless it declares `fin` (7F mints a `plan`-scoped one; 7C only reserves the read of its file).
8. **`worker` profile:** reads = the job's manifest sources (per file where the OS allows it: Landlock and Seatbelt can grant files; else the roots), the font and media caches, the job's `in/`; writes = `out/` and a private temp; GPU rows of spec §8.10 (Linux `/dev/dri/renderD*` read/write, `card*` read-only, the Vulkan ICD and driver files, `/sys/class/drm`, `/sys/dev/char`; `/dev/nvidia*` in 6G); `net: None`; **process creation denied** (Linux: Landlock `EXECUTE` only on the `fin-workers` binary and the loader, seccomp refuses `clone`/`clone3` without `CLONE_THREAD`, `fork`, `vfork`; macOS `(deny process-fork)` and `process-exec` except none; Windows active-process limit 1). This closes the `setsid` gap of sp5 §6.2 for workers. **7C-1 ships the Landlock part only** (no seccomp, no GPU proof: 6B-1 adds its GPU paths to `WorkerInput.devices` and proves them on its hardware), exactly sp6 §3.5.1's phase.
9. **`plugin` profile:** sp8 §16.1 verbatim: no network (seccomp refuses `AF_INET`, `AF_INET6`, `AF_NETLINK`, `AF_UNIX`; `socketpair` stays), reads of the store directory, the runtime, system libraries, ONNX Runtime and the manifest's model files and `in/`; writes of `out/`, the per-project plugin data directory when granted, a private temp; GPU only with `gpu`; macOS TCC and Apple Events denied; limits from sp8 §3.3 (8 processes, 2 GiB default, 1,024 files).

### Linux

10. **Landlock** at ABI ≥ 3 (kernel ≥ 6.2 floor, owner decision): `REFER` only inside write roots (so no hard link or rename can bring a file from a read-only or unlisted tree into a write root, which keeps sp5's `probe_beneath` residual user-only), `TRUNCATE` handled, `MAKE_*` only in write roots; TCP `connect` rules at ABI ≥ 4 (the listener's port, 80, 443 for `agent`; nothing for the others, which have no IP socket anyway). **ABI 6 scoping** (abstract sockets and signals) is applied where available, in addition to the seccomp `AF_UNIX` refusal. No code path for ABI < 3: a kernel below 6.2 refuses the profile with the D31 requirement named.
11. **seccomp** (`seccompiler`, compiled once per profile at start-up and cached as a `BpfProgram`): architecture check first (native only; x32 numbers and 32-bit compat on x86-64, compat on aarch64 → `SECCOMP_RET_KILL_PROCESS`); `socket()` by family per profile (`agent`: `AF_INET`, `AF_INET6`, and `AF_NETLINK` only with `NETLINK_ROUTE` if Task 0 B5 shows a CLI needs it; `AF_UNIX` → `EACCES`; everything else `EACCES`); `io_uring_setup|enter|register` → `EPERM`; `ptrace`, `process_vm_readv|writev`, `pidfd_getfd`, `keyctl`, `add_key`, `request_key`, `bpf`, `perf_event_open`, `userfaultfd`, `mount`, `umount2`, `pivot_root`, `move_mount`, `open_tree`, `fsopen`, `fsmount`, `unshare`, `setns`, `clone` with namespace flags, `personality`, `kexec_*`, `init_module`, `finit_module` → `EPERM`. `worker` adds the process-creation refusals of Decision 8; `plugin` refuses all IP families.
12. **Status** (`status.rs`, Linux): `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` → ABI; `ENOSYS`/`EOPNOTSUPP` → `Disabled {reason: landlockDisabled}` (F2: run, warn), ABI < 3 → `Unavailable {need: "kernel ≥ 6.2"}`; a seccomp `prctl(PR_GET_SECCOMP)` probe for a container whose profile blocks the calls → `Disabled {reason: landlockDisabled}` as well (sp6 §3.5.1 counts it as disabled). A test hook (`FIN_TEST_SANDBOX_STATUS=disabled`, test-hooks builds only) stubs it.
13. **The agent listener** (spec §8.3, sp5 §4.11): `finisherd` listens on `127.0.0.1:<ephemeral>` with the local socket's framing and handshake (`server.hello`/`auth.prove`, canonical endpoint `tcp:127.0.0.1:<port>` inside both MACs), refuses any principal but `agent` and `mcp` **after** the proof (audited `auth.fail {reason: listenerKind}`), and is started only on Linux and only when a Linux session or hook needs it. `FIN_SERVER=tcp:127.0.0.1:<port>`; `crates/fin/src/endpoint.rs` gains `tcp:` (loopback addresses only; anything else is `USAGE`). The listener's port joins the `agent` profile's TCP allow-list (ABI ≥ 4).

### macOS

14. **Seatbelt** (spec §8.2): the SBPL is generated from the policy (`(deny default)`, the read roots, write roots, `(deny file-write* file-link* (subpath protected))`, `(deny file-read* (subpath denied))` then the session directory's allow, last match wins); `mach-lookup` default-deny with the allow-list from `data/macos/mach-<profile>.txt` (Task 0 B6); `process-exec` of `open`, `osascript`, `launchctl`, `security` and setuid helpers denied as defence in depth; loopback denied except Finisher's own listener; `fin` reaches `<server-data>/run/fin.sock` by `path-literal` plus `file-read-metadata` on its ancestors (sp5 §4.11). Agents and hooks go through `sandbox-exec -p`; workers through `sandbox_init` in `self_apply` (the same profile text); plugins by program rewrite. Parameters are passed with `-D` and quoted by the generator (a path with `"` or `\` is escaped; a test feeds hostile path names).
15. **No keychain by default.** A harness whose sign-in lives only in the login Keychain gains the Security server in its own allow-list file, decided per CLI by Task 0 B7 and recorded in `agent.providers` (spec §8.2).

### Windows

16. **AppContainer** (spec §8.4): one container profile per project (`finisher-<project-uuid>`) for agents and hooks, one per (plugin, project) for plugins, one per (family) for workers; **a per-session capability SID** derived from the session id (`S-1-15-3-…` from a SHA-256 of `finisher-session-<id>`, the same derivation `DeriveCapabilitySidsFromName` uses) in each agent process's `SECURITY_CAPABILITIES`; ACLs on the session's directories grant that capability SID, never the container SID (Task 0 B7 verifies; fallback in Decision 17); a private window station and desktop; a kill-on-close job object with process and memory limits and `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`; only the standard handles inherited (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` stays the host's own for workers, sp5's frozen interface; the launcher sets its own for agents). No `internetClient` capability except for `agent` (open egress, owner decision 5); none for `hook`, `worker`, `plugin`; no loopback exemption for any.
17. **If capability SIDs are not honoured for a normal AppContainer** (Task 0 B7), the fallback is the spec's (§22): one container profile per session, with the broker granting that container SID per session; Task 9 implements whichever B7 confirms and records it in the spec, so only one path ships.
18. **The ACL broker** (`crates/fin-node/src/acl_broker/`): grants and revokes `{path, sid, mask, inherit}` through `SetNamedSecurityInfoW` on allow-only ACEs, each first written to a write-ahead log in `<server-data>/run/acl-broker.wal` (append, fsync, then apply; done marker after), replayed at start (a grant whose session or job is gone is revoked; an incomplete revoke is completed), so a crash between grant and revoke never leaves a stale grant (the S8 lost-grant race). One broker per server; the napi surface is `aclGrant`, `aclRevoke`, `aclSweep`, used by `agent/sandbox.ts` (sessions), the jobs runner (per-job grants for workers and plugins) and the warm pool (Decision 19). The pipe's DACL grows per project through `PipeServer.allow(sid)` (`crates/fin-node/src/pipe_server.rs:119`); no second pipe implementation.

### Jobs

19. **Per-job grants for warm workers** (closing sp5 Decision 35): a warm worker is no longer granted `<server-data>/jobs/` but **its own directory `w.dir`**, and every job handed to it has its job directory created under `<w.dir>/jobs/<jobId>/` instead of `<root>/<jobId>/` (`jobs/files.ts` `jobDirs(root, id)` gains the parent; the hand-off creates or moves the directory before `start`, on the same volume, and deletes it after `ready`, as a cold job's is). Landlock and Seatbelt grants can only narrow, so this is the shape that lets one restriction at start cover exactly the worker's own jobs; on Windows the broker grants each job's directory to the worker's capability SID before `start` and revokes it after `ready`, so the grant is per job there too. **Under a sandbox, a warm key includes the project id**, so a warm worker reads only one project's sources and library roots (Question 2: one warm worker per open project and family costs memory). With both, `caps().perJobGrants` is true and `warm.ts`'s `sandboxed()` gate (`apps/finisherd/src/jobs/warm.ts:83-93`, and its uses at `:185`, `:249`, `:322`, `#sandboxFound` at `:401-415`) admits warm kinds again. A cold worker already gets only its `tmp/` and each mapped file at its own mode (`jobs/runner.ts:232-252`), not its job directory; the warm hand-off keeps that rule for what it maps, and the planted-link checks of `jobs/fsafe.ts` (symlink, `nlink !== 1`, FIFO) run on the moved directory as on a cold one.

### Residuals of other sub-projects

20. **`probe_beneath` keeps `RESOLVE_NO_XDEV` unset** (needs owner confirmation; Question 1). Setting it would refuse every project whose `sources/` or a library root under it is a mount point (a NAS share mounted into a project folder, a second disk), which is a supported layout. Instead 7C makes the residual hold by construction: no sandboxed process can create a mount (seccomp refuses the mount family; Seatbelt has no mount right; an AppContainer cannot mount) or a hard link from outside its write roots into the project folder (Landlock `REFER` only inside write roots; Seatbelt `file-link*` denied outside write roots; Windows needs write access to the source file, which the container lacks), and Task 13's suite proves each, so only the user's own file access can plant either.
21. **The plugin store is read-only inside every profile** (sp8 §16.1 follow-up): `<server-data>/plugins/store/` is in `protect` for `plugin` (read only, its own package directory) and in `deny_read` for the other three. Whether sp8's host re-hashes a tree before it starts it is sp8's code and an owner choice (Question 5); 7C does not change the installer.
22. **WER** (sp5 §6.12; needs owner confirmation for the install part, Question 4). Workers already turn crash UI off in-process (`crash_ui_off`, `crates/fin-jobs/src/worker/mod.rs:190-202`: `SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX)` and `WerSetFlags(WER_FAULT_REPORTING_NO_UI)`, after the self-sandbox entry). 7C extends the same to processes that are not ours: `fin-sandbox-exec` sets the error mode before `CreateProcessW` without `CREATE_DEFAULT_ERROR_MODE`, so agents, hooks and plugins inherit it, and every AppContainer process gets the container's private desktop, so a dialog, if one were raised, is on a desktop nobody sees and cannot block the user's. `__fastfail` still reaches the WER service, which may start a headless `WerFault.exe -u`; `fastfail_dies_without_wer` (`crates/fin-workers/tests/host_windows.rs:240`, assertion in `crates/fin-jobs/tests/worker_runtime.rs:478-484`) is changed to assert what matters (no window on any desktop, no report in `ReportArchive`/`ReportQueue`), not the absence of the process. Excluding `fin-workers` and `fin-sandbox-exec` from WER at install (`WerAddExcludedApplication`, a per-user registry key) belongs to sub-project 9's installer.

### Reporting

23. **One status source** (`apps/finisherd/src/sandbox/status.ts`): `sandboxStatus()` from fin-node per profile → `server.status.warnings` entries `SANDBOX_DISABLED {profile, os, reason: landlockDisabled, fix}` (the `fix` names the `lsm=` boot parameter) for each disabled profile, `PLUGINS_UNCONFINED` for plugins (sp8's name), `SANDBOX_DISABLED {worker}` with `media.capabilities.sandbox: none` (sp6's names); the `sandbox.unconfined {profile, os, reason}` event once per profile per server start and on change. `agent/warnings.ts`'s constant `before7C` warning is replaced by a call into it. A profile not yet in `READY` on this OS keeps the warning its sub-project already shows (`reason: before7C` for agents becomes `reason: profileNotReady` with the same fix text pointing at the release notes).
24. **Violations** (spec §8.8): only what the launcher or host observes (seccomp `SIGSYS`, job-object limit messages, a broker refusal, a failure to apply) becomes an audit entry `sandbox.violation {session?, jobId?, op, profile, rootKind}` and, for a session, a `notice` node. Paths are stored as root kind plus keyed hash. Finisher never reads the macOS unified log.

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
pub fn self_apply(req: &fin_jobs::sandbox::SelfSandbox) -> Result<(), String>;

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

- [ ] **Step 1 (B5, Linux; kernel ≥ 6.2):** on the Linux VM, a 6.2-series kernel, and a supported kernel booted with `landlock` left out of `lsm=`: a seccomp filter that checks the architecture and refuses x32/compat, io_uring, `AF_UNIX` and the other calls of Decision 11, each denial observed; libuv and the installed Codex and Claude Code still run (the io_uring fallback included); the per-CLI validation list of spec §8.3 (SSSD/LDAP user lookup, gpg-agent signing, git credential-cache, fsmonitor) and whether `AF_NETLINK`/`NETLINK_ROUTE` is needed; whether any newer Landlock ABI mediates pathname sockets; on the Landlock-disabled boot, that the status probe of Decision 12 says `Disabled` and a process runs. Record the base toolchain read list for `data/linux/toolchain-base.txt`.
- [ ] **Step 2 (B6, macOS 14+):** under a log-only `(deny default)` profile, the minimal `mach-lookup` list per CLI and for `fin` and `fin-workers`; with default-deny, `open`, `osascript`, `launchctl` and `security` fail, nothing starts outside the sandbox, and a test keychain item created for the spike is unreadable. Record `data/macos/mach-*.txt` candidates and the base toolchain list.
- [ ] **Step 3 (B7, the user's own sign-in, never copied):** with a test account a person signs in to through each CLI's own flow, in a sign-in home: where each CLI keeps its store per OS (file or keychain; keyed by config directory?); whether a per-session `CLAUDE_CONFIG_DIR`/`CODEX_HOME` holding a link to the store works and refreshes **in place**; parallel sessions refreshing at once; a file link on Windows without elevation; else the fallback of spec §8.6; that `system/init` and `account/read` report the sign-in kind and an account label; **per-session capability SIDs on a session directory under a normal AppContainer**, and that a second session's process cannot open it (decides Decision 16 or 17). No spike script opens a credential file.
- [ ] **Step 4 (H2):** Claude Code under `-p` stream-JSON without `--bare` (`--setting-sources user` with a per-session config dir, `--strict-mcp-config`, the `CLAUDE.md`/`.claude/` read denial: planted settings, hooks, MCP servers and memory files observed not loaded); `--append-system-prompt`; `system/init` before the first prompt; `--max-budget-usd`; `--disallowedTools`; Codex `-c project_doc_max_bytes=0` and `developer_instructions` at the pinned version.
- [ ] **Step 5:** conclusions into the spec (each marked *verified on <host class>*), tick `TASKS.md`. **Commit** `docs: record spike S9 conclusions` (spec and `TASKS.md` only).

### Task 1: `fin-sandbox`: the policy, the profiles and enumeration — tier: opus — 7C-1

**Files:**
- Create: `crates/fin-sandbox/Cargo.toml` (features `test-hooks`), `crates/fin-sandbox/src/{lib.rs, policy.rs, enumerate.rs, error.rs, inputs.rs}`, `crates/fin-sandbox/src/profile/{mod.rs, agent.rs, hook.rs, worker.rs, plugin.rs}`, `crates/fin-sandbox/data/{linux,macos,windows}/toolchain-base.txt` (placeholders until Task 0, each with a header naming its source).
- Modify: `Cargo.toml` (workspace member), `crates/fin-store/examples/export_schema.rs` (the `*Input` types), `packages/core-types` (generated), `deny.toml` if needed, `TASKS.md`.
- Test: `crates/fin-sandbox/tests/{policy.rs, enumerate_props.rs, profiles.rs}`.

**Interfaces:** the frozen `policy.rs` and `profile/*.rs` above; `enumerate::write_roots(write, protect, deny) -> Vec<PathBuf>`, `enumerate::read_roots(read, deny) -> Vec<PathBuf>`; `AgentInput {sessionId, mode, harness, projectDir, workspaceDir, sharedDir?, libraryRoots, sessionDir, signInStore?, toolchains, serverData, home, listener?}`, `HookInput`, `WorkerInput {jobDir, tmp, sources, caches, devices, warm?: {workerDir, libraryRoots}}`, `PluginInput` (sp8 §16.1 fields).

- [ ] **Step 1: Write failing tests:**
  - `validate_refuses_relative_unnormalised_and_overlapping` (a write root under a protect root; `..`; a relative path);
  - `enumerate_props` (proptest over random trees: no enumerated write root contains a protected or denied path; the union of enumerated roots equals write minus protected; read roots never contain a `deny_read` path; deterministic order);
  - `agent_plan_has_no_shared_write`, `agent_reads_only_allow_list` (no `$HOME`, no `/`, no server data but the session directory's listed files), `agent_excludes_other_workspaces` (Decision 6), `agent_claude_denies_claude_md` (harness `claudeCode`), `agent_protects_project_state` (`.finisher/`, `assets/`, `finisher.project.json`, `credential.json`, `settings/`);
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

### Task 4: The seccomp filter — tier: opus — 7C-2

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
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates on the Linux VM → PASS. **Commit** `feat(sandbox): fin-sandbox-exec and the Linux agent and hook profiles`.

### Task 6: The agent listener and `fin`'s `tcp:` endpoint — tier: opus — 7C-2

**Files:**
- Create: `apps/finisherd/src/agent/listener.ts`.
- Modify: `apps/finisherd/src/main.ts` (a third `serveRpc` beside the local and WebSocket ones, started on Linux on first need; its endpoint in `listeners`), `apps/finisherd/src/transport.ts` (beside `makeLocalServer`, `:84-93`: a loopback `net.Server` with the same framing), `apps/finisherd/src/connections.ts` (transport `agentTcp`; principal filter after the proof), `apps/finisherd/src/agent/builtin/environment.ts:18` and `apps/finisherd/src/agent/wiring.ts:84` (`FIN_SERVER` "the local socket until 7C's agent listener": the listener on Linux), `crates/fin/src/endpoint.rs` (`Endpoint` at `:22-41` gains `Tcp(SocketAddr)`, loopback only), `crates/fin/src/output.rs` if a code is new, `TASKS.md`.
- Test: `apps/finisherd/test/agent-listener.test.ts`, `crates/fin/tests/cli_endpoint_tcp.rs`.

- [ ] **Step 1: Write failing tests:** the listener binds `127.0.0.1` only, an ephemeral port; an `agent` and an `mcp` credential complete the handshake with the canonical endpoint `tcp:127.0.0.1:<port>` in both MACs (a proof made for another endpoint fails); a `local`, `console` or device principal is refused after the proof and audited (`auth.fail {reason: listenerKind}`); a client that sends nothing is cut after the handshake timeout; the connection cap holds; `fin` with `FIN_SERVER=tcp:127.0.0.1:<port>` and `FIN_CREDENTIAL` works and `tcp:10.0.0.1:1` is `USAGE`; **from inside the `agent` profile on Linux, `fin view` works over the listener and connecting to `fin.sock` fails** (seccomp).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): Linux agent listener over loopback TCP; fin tcp: endpoint`.

### Task 7: `SandboxSpawner` for 5C (Linux), workers and plugins — tier: opus — 7C-2

**Files:**
- Create: `crates/fin-sandbox/src/spawner.rs`.
- Modify: `crates/fin-node/src/jobhost/mod.rs:180-187` (construct the host with `SandboxSpawner`), `crates/fin-jobhost/src/{lib.rs, host.rs, wire.rs}` (`HostCaps.profiles`, `per_job_grants`, Decision 4), `packages/contracts/src/jobs.ts` (the caps schema), `apps/finisherd/src/jobs/{host.ts, layer.ts}` (`JobHostLike.caps()`; the start-up line reads `profiles`), `crates/fin-registry/src/plan/mod.rs:201-205` (`thirdPartyCode.confined` from the plugin profile's status, as the 8A plan expects), `TASKS.md`.
- Test: `crates/fin-sandbox/tests/spawner_linux.rs`, `crates/fin-workers/tests/host_sandbox.rs`.

- [ ] **Step 1: Write failing tests:** `prepare` for `Profile::Worker` sets `self_sandbox {profile: "worker", args}` from `SpawnRequest.grants` and `job_limits.active_processes = Some(1)`; for `Profile::Plugin` adds `pre_exec` steps and no self-sandbox; a worker under the real host applies Landlock + seccomp and reports `hello.selfSandbox = "worker"`; **a worker that tries `setsid`+`fork` (the `test` family's spawn kind) is refused** (closing sp5 §6.2's gap for workers); a plugin test binary cannot open a TCP socket or read server data; a profile not in `READY` for Linux runs as `Unsandboxed` did (Decision 5) and says so in `caps().profiles`; `sandboxed()` is true only when every requested profile is enforced.
- [ ] **Step 2: Implement.** `pre_exec` steps allocate nothing (a test runs them under a `malloc` guard in the child).
- [ ] **Step 3: Run** the gates on the Linux VM → PASS. **Commit** `feat(sandbox): SandboxSpawner for the job host on Linux`.

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

- [ ] **Step 1: Write failing tests** (non-elevated session): container profile create/reuse/delete per project; capability SID derivation is stable and distinct per session; **a process of session B cannot open session A's directory** (Decision 16, or 17's fallback per Task 0 B7); the S8 Windows paths (junctions, ADS, 8.3 names, `\\?\`, admin shares, WMI, `schtasks`) refused; Credential Manager denied; server data unreadable; no loopback for any profile; `worker`: a second process is refused by the job's active-process limit; `plugin`: no network capability; the job object kills the tree when the launcher dies; **no window appears on any desktop for an access violation or a `__fastfail`** and no report lands in `ReportArchive`/`ReportQueue` (Decision 22).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates on the Windows host through a non-elevated session → PASS; `cargo check` for the other targets. **Commit** `feat(sandbox): Windows AppContainer profiles, job objects and no-UI fault reporting`.

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

- [ ] **Step 1: Write failing tests:**
  - `session_env_is_exact` (spec §8.5: `childEnv()` plus `FIN_SERVER` (socket, listener or pipe per OS), `FIN_SESSION`, `FIN_CREDENTIAL`, **`FIN_PROJECT_DIR`** (the session's project folder, links resolved), `TMPDIR`, the provider directory variable; never `FIN_SERVER_DATA`, `FIN_AUTHOR`, `FIN_TRAINING`, `FIN_DIAGNOSTICS`, `SSH_AUTH_SOCK`, `GPG_AGENT_INFO`, `DBUS_SESSION_BUS_ADDRESS`, `DOCKER_HOST`, `DISPLAY`, `WAYLAND_DISPLAY`, `CLAUDE_CODE_OAUTH_TOKEN`, `*_API_KEY`), and `assertSessionEnv` throws on each violation (the contract 7D/7E call);
  - **`harness_with_wrong_project_dir_reads_nothing`**: a child of `spawnInSession` that resets `FIN_PROJECT_DIR` to another project's folder gets `PATH_NOT_ALLOWED {reason: projectMismatch}` from `fin caption import --words` (the check of `agent_paths.rs:186-200`), and cannot read that folder at all under the sandbox;
  - **`agent_child_cannot_read_local_key`**: a `spawnInSession` child runs `cat <server-data>/auth/local.key` and `fin --server-data … whoami` without `FIN_CREDENTIAL`: both fail (OS denial), on each OS the suite runs on;
  - `sign_in_store_never_opened` (`finisherd_never_opens_sign_in_store`, spec §16.2: a file-access spy over session start, a scripted child's run and end: no `open` under `<server-data>/providers/` by `finisherd`);
  - `disabled_host_runs_with_notice` (stub `disabled`: the session starts, a `notice` node and `agent.session.get` carry `SANDBOX_DISABLED {reason: landlockDisabled}`); `unavailable_refuses_start` (`SANDBOX_UNAVAILABLE`); `profile_not_ready_keeps_warning` (Decision 23);
  - `violation_is_audited` (a seccomp kill in a session child → `sandbox.violation` with root kind and keyed hash, and a `notice` node).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(agent): session sandbox, directories and environment; every agent process through fin-sandbox`.

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

**Files:** Modify `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` (§8 rewritten "as built": names, data files, codes, the `READY` table, Decisions 16/17 outcome; §1.4 7C row; §18 file map; §19 decisions; §8.9's `before7C` paragraph replaced), `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§4.6, §4.17, §6.2, §6.3, §6.12: each "until 7C" marked closed with a pointer, or left open with the reason), `docs/specs/2026-11-07-sub-project-6-media-engine-design.md` §3.5.1 (the Landlock phase delivered), `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §16.1 (the profile delivered; the store follow-up's outcome), `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 pointer; §8.4 "bwrap fallback" removed), `README.md` (security model: what the sandbox now guarantees per OS, and what F2 hosts lack), `TASKS.md` (follow-ups).

- [ ] **Step 1: Write** the as-built sections from the code (names, defaults, codes), and every residual that stays open, with its reason.
- [ ] **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 7C as built`.

### Task 16: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

- [ ] **Step 1:** On macOS (14 and the newest), Windows 11 (non-elevated, Defender on) and the Linux VM (kernel ≥ 6.7 for the TCP rules, plus a 6.2-series kernel and a Landlock-disabled boot): `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e`, `pnpm sandbox:suite`.
- [ ] **Step 2:** The reference NAS Docker image (spec §8.3): Docker's default seccomp profile allows the Landlock calls; the image's worker and agent profiles apply; with Landlock disabled on the host, everything runs with the warnings.
- [ ] **Step 3:** The per-CLI validation list (spec §8.3) against the installed Codex and Claude Code under the `agent` profile on each OS (start, a tool call, `git`, `fin` through the OS's transport), as far as 7C's base list allows; per-CLI lists are 7D/7E's.
- [ ] **Step 4:** Update `fin_sandbox::READY` for every (profile, OS) whose suite passed, in its own commit (`feat(sandbox): enable <profile> on <os>`), so the switch to "mandatory" is reviewable on its own (Decision 5).
- [ ] **Step 5:** Record results in TASKS.md and the spec; fix waves as needed; final whole-branch review. **Commit** `docs: 7C validation results`.

## Questions for the owner

1. **`probe_beneath` and mount points (Decision 20).** Keep `RESOLVE_NO_XDEV` off, so projects whose `sources/` (or a library root inside the folder) is a mount point keep working, and rely on the sandbox to stop agents creating mounts or hard links (recommended); or set it and refuse such layouts? *Needs owner confirmation.*
2. **One warm worker per open project under a sandbox (Decision 19).** Per-job grants need the warm key to include the project, so a person with three projects open has up to three warm media workers (memory per worker as sp6 measures it) instead of one. Accept (recommended), or keep warm workers cross-project and grant every open project's library roots to each (fewer processes, wider reads)? *Needs owner confirmation.*
3. **Other sessions' workspaces in the server-side reach (Decision 6; 7A review L11).** 7C removes other sessions' `workspace/<id>/` from the `agent` read policy. Should the server's path reach (`agent/reach.ts`) and `fin`'s client check (`agent_paths.rs`) exclude them too, so a session cannot name another session's scratch file in an RPC (recommended), or is that within "same project" and acceptable? *Needs owner confirmation.*
4. **WER at install (Decision 22).** Exclude `fin-workers` (and `fin-sandbox-exec`) from Windows Error Reporting per user at install, which also stops crash reports reaching Microsoft for those binaries (sub-project 9's installer; recommended), or leave WER as is and accept a headless `WerFault.exe` after a `__fastfail`? *Needs owner confirmation.*
5. **Plugin store integrity (Decision 21, sp8 §16.1).** 7C makes the store read-only to every sandboxed process. Should sp8's host also re-hash a plugin's tree before its first start in each server run (catches tampering by the user's own unsandboxed processes; costs a hash of the tree, recommended), or move the store to a directory only the server's own account can write (a second account: not a v1 shape)? *Needs owner confirmation.*
6. **Profiles become mandatory per OS by a compiled table (Decision 5).** A profile switches from "warn and run" to "enforced, no opt-out" in a release once its suite passes on that OS, with no runtime setting. Confirm there is to be no owner setting to turn an enforced profile off (recommended: none, as the spec's removal of `agents.unsandboxed` implies). *Needs owner confirmation.*
