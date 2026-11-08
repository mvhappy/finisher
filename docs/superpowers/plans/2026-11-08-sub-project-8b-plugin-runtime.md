# Sub-project 8B: The plugin runtime (host, protocol and SDKs) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the protocol, rights and reach, the confused-deputy rules, the egress broker, process keys, commands and `Tx.via`, the SDK runtime, confinement), `sonnet` for plumbing (gates, RPC surface, `fin`, developer mode, the TS SDK, diagnostics, e2e, docs).

**Goal:** code plugins run. After this plan:
- an installed and enabled code plugin (bundled, registry or `@dev`), `native` or `node`, runs as an out-of-process job worker of 5C's engine, keyed per (plugin, project, principal class), with the environment allow-list, no credentials, memory, process and file limits, and a crash-loop stop;
- every host call is checked against the invoker's rights intersected with the plugin's grants, and against project, branch-subtree and library reach (the `plugin` principal);
- plugin **commands** (`plugin:<id>/<command>`) run from `fin @<id> <command>`, return a transaction of fin-core ops, and are committed under the invoker with `Tx.via` set by the host only;
- **brokered HTTPS egress** (`net.fetch`) is the only network path the host offers;
- `fin-plugin-sdk` (Rust) and `@finisher/plugin-sdk` (TypeScript) implement the protocol; `fin-plugin-test` and `fin-plugin test` run a plugin against a fake host with the `command` conformance profile;
- `finisherd --plugin-dir` loads a developer plugin;
- code plugins stay **unconfined until sub-project 7C's `plugin` profile lands** on an OS (owner decision 1), and are started through it automatically, without re-approval, once it does (Task 16);
- hook handlers (`hooks:observe`, `hooks:modify`) run in sub-project 7F's hook engine (Task 17, gated on 7F).

**Spec:** `docs/specs/2026-11-07-sub-project-8-plugins-design.md` (cited as *spec §n*): §1.5 row 8B, §2, §3 (all), §4.1–4.3, §5.3 (enable, disable, crash loop), §5.5, §7.1, §7.2, §7.4, §9.3 (the broker's network rules), §11, §12, §13, §14, §15.1 rows "Host and protocol", "`fin-shm`", "SDKs", §16, §17, §19, §20, and **§23 (8A as built, which wins over unmarked paragraphs)**. Sub-project 5 (*cp §n*): §6 (5C jobs, `call`/`reply`, warm keys, `Program::Path`, the minor rule). Sub-project 7 (*agent §n*): §5.1 (branch subtrees), §7.2 (command rules), §8.9–8.11 (the `plugin` profile and its escape suite), §9 (hooks), §10 (`SecretStore`, `ModelGateway`). Parent: arch §14, §16, §27.7.

**Builds on, as built:**
- **8A** (spec §23): `fin-plugin-api` (manifest, `Runtime::{Native, Node}`, permissions and risks, `ContributionKind`, `consumable(hostApi)` with `HOST_API = 1.0.0`, `approval::requires_reapproval`), `fin-registry` (`Store` with `data_dir`, `installed_manifests`, tree hashes), `apps/finisherd/src/plugins/{lifecycle,plans,registries,bundled,content,rpc,scrub,store-sync,tables}.ts`, the `plugins`, `plugin_grants` and `plugin_plans` tables, audit and `plugin.lifecycle` events; code plugins install **disabled** and enabling one is `PLUGIN_RUNTIME_UNAVAILABLE` (`plugins/lifecycle.ts`).
- **5C-1 / 5C-2** (`apps/finisherd/src/jobs/`, `crates/fin-jobs`, `crates/fin-jobhost`): `FamilySpec {profile: "plugin", pluginId?, program, warm, warmKey, onCall}`, `ProgramSpec {path, args}` for plugins, `call`/`reply` host calls (≤ 16 in flight, ≤ 1 MiB per line), the minor-version rule for `Program::Path` processes, warm argv `… --warm`, `JobOwner`, `JobVia {plugin}`, `JobRights` (opaque until this plan), `job_limits`, `Spawner`, `JobHostCaps`. Constraints recorded there and honoured here: **`registerKind` refuses `rpc: true` for a plugin family's kinds** (5C-2 review M3: "sub-project 8 decides"), **`FamilySpec.env` is refused for plugin families** (6A-2 review L4), and **`worker.exit` leaves the plugin id out** (`journalSink`: the journal's hashing key does not reach TypeScript; 5C-1 follow-up).
- **6A** `fin-shm` (`JobDir`, `checked_open`, copies validated as copies, rings).
- **7A**: `SecretStore` and `ModelGateway` (`apps/finisherd/src/secrets/`, 7A-1); agent sessions with their own branches and principals (7A-2).

**Branch:** `sp8b-plugin-runtime` from the integration branch (`sp4c-edit-breadth` today, which carries 5A, 5C, 5D, 6A and 8A). Rebase onto 7A-2 before Task 6's agent rows, onto 7C's branch before Task 16, onto 7F's before Task 17.

**Dependencies:**
- **8A, 5C-1, 5C-2, 6A-2:** built.
- **7A-2** (runtime core): agent principals, session ids and branch subtrees, needed for the `agent:<root session>` process-key class and branch reach (Task 6). Tasks 1–5 and 7–15 do not wait for it; Task 6's agent rows do.
- **7B** (rules): command rules matching `plugin:<id>/<cmd>` (agent §7.2) are 7B's; this plan exposes the names (Task 10). Without 7B, agents reach plugin commands by scopes alone (`project.write` through the command), as they reach every other command today.
- **7C** (OS sandbox): **the `plugin` profile** (spec §16.1, X1) for confinement, Task 16. Owner decision 1: nothing else waits for it. The interface expected from 7C is in Task 16.
- **7F** (hooks): the hook engine calling plugin handlers, Task 17.
- **Not needed:** 6B and later media batches (codec, matte and frame interfaces are 8H's and 8C's), 8C's ONNX Runtime and models.

**Architecture (8B slice):**
- **`fin-plugin-api`:** `protocol/` (the typed params and results of plugin kinds and host calls; generated into `@finisher/core-types`), `manifest/contributes.rs` gains `commands` and `hooks`, `contribution.rs` makes `commands` consumable at host API **1.1.0**, `schema_subset.rs` (the command-argument schema grammar and its validator, pure).
- **`fin-core`:** `Tx.via: Option<Via>` (no upcaster).
- **`finisherd`** `src/plugins/host/`: `family.ts` (the `plugin` family, program resolution, env, limits, warm keys), `node-runtime.ts` (permission flags, `ELECTRON_RUN_AS_NODE`), `rights.ts` (invoker ∩ grants, the `JobRights` snapshot), `reach.ts`, `calls.ts` (host API through `onCall`), `egress.ts` (`net.fetch`), `commands.ts` (registry, expansion, commit), `crash-loop.ts`, `dev.ts` (`--plugin-dir`), `confinement.ts`, `hooks.ts` (Task 17). The 8A lifecycle (`plugins/lifecycle.ts`) gains enable/disable for code plugins.
- **`fin-node`:** the keyed hash of plugin ids for the journal (`journal/plugin_ref.rs`), the plugin tree re-hash before a cold start (`registry/host.rs`).
- **New crates and packages:** `crates/fin-plugin-sdk` (Rust SDK), `crates/fin-plugin-test` (the harness, a library plus the `fin-plugin test` subcommand in `crates/fin-plugin-cli`), `packages/plugin-sdk` (`@finisher/plugin-sdk`), `examples/plugins/{rust-tools,ts-tools}` (example plugins used by CI and e2e).
- **`fin`:** `fin @<publisher>/<name> <command> …`, `fin help @<publisher>/<name>`.
- **Not in 8B:** analyzers, ONNX Runtime and models (8C); secret injection and raw grants (8G, Decision 10); providers (8G); codec backends, mattes, WGSL (8H); `media.*` and `analysis.*` host methods (8C); `job.submit` from plugins (8C, Decision 6); Python and WASM runtimes (after v1); the UI (sub-project 9).

**Tech stack:**
- Rust 2024 (`fin-plugin-sdk`, `fin-plugin-test`); no new third-party crate is needed: the SDK uses `fin-jobs`' worker runtime, `serde_json`, `fin-shm`. The schema subset validator is our own (Decision 13), so the `jsonschema` crate of spec §18.1 is not added.
- Node 24.22 (finisherd's runtime and the `node` plugin runtime; its permission model, `--permission`), Effect 4, vitest; `node:https` and `node:dns` for the broker (no HTTP client dependency); esbuild only as a documented bundling recipe for plugin authors (not a dependency of the SDK package).
- Every crate and package added is MIT, `publish = false` / `"private": true` in this batch (crates.io and npm publication is sub-project 11's release work), and passes `cargo deny` and the JS licence gate.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code.
- **One wire** (Decision 2): plugin processes speak 5C's `fin-jobs` envelope and nothing else; no second protocol stream, no socket.
- **A plugin process gets no ambient authority from the host:** `childEnv()` (arch §27.7) plus only `FIN_PLUGIN_PROTOCOL` and a private `TMPDIR`/`TEMP`/`TMP` (spec §3.3); no `FIN_*` credential, no provider variable, no `FIN_TRAINING`, no `FamilySpec.env` (refused for plugin families: kept); no socket to `finisherd`; no inherited handle but stdio. A test spawns the example plugin and dumps its environment and open descriptors/handles.
- **Plugins propose; fin-core decides** (spec §2.5): a plugin writes canonical state only by returning ops committed through `tx.commit` under the invoker. Plugins cannot define primitive ops; **primitive ops live only in `fin-core`, one module per op** (8B adds none).
- **Rights are invoker ∩ grants on every call, job and commit; jobs are owned by the invoker** (spec §3.7). No plugin ever holds a human-only scope, `branch.protected`, `review.decide`, `project.admin`, `auth.admin` or `diagnostics.*`.
- **Only the host emits diagnostics and audit records** (spec §11); third-party ids are keyed hashes, bundled ids table names; plugin stderr is never journaled.
- **Third-party code stays high risk with an explicit acceptance** on every install and update (8A, unchanged); nothing auto-updates; nothing is refused for lack of a sandbox (owner decision 1; no `SANDBOX_UNAVAILABLE` gate except where 7C's profile passed its suite and then fails to apply, agent §8.10).
- **TypeScript model types are generated, never hand-written:** every protocol type, contribution type, `JobRights`, `Via` and RPC payload derives `JsonSchema`, is added to `crates/fin-store/examples/export_schema.rs`, and is regenerated with `pnpm --filter @finisher/core-types generate`. The TS SDK imports the generated types.
- **No cloud-only imports** (`check-cloud-imports`); the broker's only network code is `node:https`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500; `plugins/host/` is split per concern from the start (above).
- Errors are `{code, message, details, fix?}` with the exits of spec §13, added to `crates/fin/src/output.rs` and `apps/finisherd/src/console/exit-codes.ts` in the task that raises them (their parity test pins both); errors to non-human callers lose paths (`plugins/scrub.ts`).

**Testing and process**
- Plugin processes in tests are the example plugins (`examples/plugins/*`) built by the test setup and the `testplugin` family of 5C; nothing downloads a plugin.
- The broker's tests use a loopback HTTPS server with a throwaway certificate and a **test-only resolver hook** (`test-hooks` builds) that lets a named loopback address through the private-address check; release builds have no such hook (`check-test-hooks` covers it).
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`; tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Shell commands are prefixed with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key material in tracked files; test keys and certificates are generated per run.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 8B" in the same commit; Task 1 adds the section.
- **Batches** (each green on all three OSes before the next):
  - **A, types and the wire:** Tasks 1–3.
  - **B, processes and lifecycle:** Tasks 4–5.
  - **C, rights, host API and egress:** Tasks 6–8 (Task 6's agent rows after 7A-2).
  - **D, commands:** Tasks 9–11.
  - **E, SDKs and the harness:** Tasks 12–14 (Task 12 may start after Task 2).
  - **F, diagnostics:** Task 15.
  - **G, confinement and hooks:** Task 16 (after 7C's `plugin` profile), Task 17 (after 7F).
  - **H, e2e, validation, docs:** Tasks 18–20.

## Decisions taken here

1. **Owner decisions applied (spec §20).** Decision 1 (third-party code plugins allowed now, with warnings): registry code plugins become enableable here, before 7C, with 8A's high-risk warning and acceptance unchanged, unconfined until 7C's `plugin` profile lands on an OS, then confined automatically (Task 16). Decision 2 (brokered secrets): the broker never hands a plugin a key; secret injection itself is 8G's (Decision 10), and there is no raw path in 8B. Decision 4 (the weight licence governs): model files are 8C's; a code plugin whose manifest declares `models` cannot be enabled before 8C (Decision 18). Decision 5 (generation through OpenRouter and fal.ai) and decision 7 (2-of-3 offline root keys, a ceremony before the first first-party registry) do not touch 8B: providers are 8G's, the registry and its keys sub-project 11's; 8B's tests use generated keys only.
2. **One wire: the `fin-jobs` envelope** (5C-1 Decision 44, a coordinator decision; it supersedes spec §3.4's separate JSON-RPC 2.0 stream). A plugin process is a 5C worker of the `plugin` family (`Program::Path`, profile `plugin`). Mapping of spec §3.4:
   - `initialize {hostApi, protocol, plugin, grants, dirs, locale, limits}` → the worker's `hello` (`v` 1, any `minor`, `kinds`, `warm`) and, per job, a **`PluginContext`** in `start.extra` (host API version, plugin id and version, the granted permissions, dirs, locale, limits); `initialized` → `ready`; `shutdown`/`exit` → 5C's `shutdown` and stdin EOF;
   - **host → plugin** methods (`command.run`, `hook.<event>`, later `analyze.run`, `provider.*`, `codec.*`, `matte.run`) → **jobs of plugin kinds** (`plugin.command` v1 here; `plugin.hook` v1 in Task 17);
   - **plugin → host** methods (`project.snapshot`, `net.fetch`, `log`, …) → **host calls** (`call {id, method, params}` → `reply`), routed to the `plugin` family's `onCall` (Task 7);
   - the JSON-RPC types of spec §3.4 live on as the typed params and results of those kinds and calls in `fin-plugin-api::protocol`. Task 20 rewrites spec §3.4 to this.
3. **One `plugin` family** (the name `WORKER_FAMILIES` already holds), not a family per plugin: `program: (job) => ProgramSpec` resolves the job's plugin (`job.via.plugin`) to its store directory and runtime; `warmKey: (job) => "<plugin>@<version>|<project or none>|<class>"` (spec §3.3's process key; the version is in the key so an update never reuses an old process); `warm {idleS: 300}`. 5C changes made here, each with its test: (a) a family of profile `plugin` may declare up to 16 warm slots (5C limits families to 2, which would let two keys thrash every other plugin's processes); (b) `worker.exit`'s plugin id comes from `family.pluginId ?? job.via?.plugin` (attempt and helper paths), so one family serves many plugins.
4. **Principal classes** (spec §3.3): `person` (the `console`, `local` and `device` principals), `agent:<root session id>` (7A-2's session tree), `system` (work the host starts). A call is routed only to its own key's process. Calls without a project use `none`.
5. **Long-lived work (commands, hooks) runs warm and exchanges data only through calls and results** (each ≤ 1 MiB, 5C's line cap); a snapshot larger than that is paged (`project.snapshot {page}`). Reason: a warm process outlives its job, and Node's permission flags are fixed at spawn, so it cannot be granted each job's `in/` and `out/` (the warm sandbox rule of spec §25.5 decision 20 in sub-project 6 is the same problem). `in/`/`out/` files and rings (spec §3.5) are used by one-process-per-job kinds (8C's analyzers onward); those kinds run cold, under 5C's job directory `<server-data>/jobs/<jobId>/{in,out}/` with `fin-shm`'s rules. **Deviation from spec §3.5 and §5.1:** no `plugins/run/<key>/<callId>/` tree; 5C owns job directories. **Deviation from spec §3.3:** the per-message cap is 5C's 1 MiB, not 16 MiB. The per-project plugin data directory (`<server-data>/plugins/data/@pub/name/<projectId|_>/`, spec §5.1) is created on first use when `fs pluginData` is granted, and named in `PluginContext.dirs.data`.
6. **Plugin kinds are never submitted by RPC** *(needs owner confirmation)*. 5C-2 left "how plugins' kinds are submitted" to 8B. Recommendation: keep `registerKind`'s refusal of `rpc: true` for plugin families permanently; clients reach plugin work only through typed RPCs that check rights and build the job (`plugin.command.run` here; 8C's analysis RPCs; 8G's generation RPCs), and plugins reach their own kinds only through a `job.submit` host call, which arrives with 8C, the first batch that has plugin job kinds. Alternative: let a plugin family mark kinds `rpc: true` with a `paramsReach` written by the host from the plugin's declared schema, so `fin job submit` could start plugin jobs directly.
7. **Command calls are jobs.** A command run is a `plugin.command` v1 job: `defaultPriority: interactive`, `qos: normal`, `warm: true`, `timeoutS: 60` (the manifest maximum), `maxAttempts: 1` (a command is never retried: it may have reached an external API), `resources: []`, no files, `memory` from Decision 9. The host cancels the job at the command's own timeout (default 10 s, at most 60 s by manifest) and answers `PLUGIN_TIMEOUT`. Its owner is the invoker, its `via` the plugin, its `rights` Decision 8's snapshot. The ≤ 20 ms p95 round-trip budget (spec §15.4) is measured in Task 19 against 5C's warm hand-off; a miss is recorded, not worked around with a second path.
8. **`JobRights`** (opaque in 5C, defined here, generated): `{principal: {kind, id}, class, scopes: [scope], projects: "all" | [projectId], branch?: {root, subtree: true}, libraries: "all" | [libraryId], grants: [Permission]}`, computed once at submit as **invoker ∩ grants** (spec §3.7's mapping: `project:read` → `project.read`; `project:write` → `project.write` limited to `tx.commit` of the returned transaction; `media:read` → `library.read` and asset reads; `jobs:submit` → `jobs.submit` for the plugin's own kinds) and checked again on every call against the live principal (a revoked or narrowed invoker narrows its running plugin calls at the next call).
9. **Process limits before 7C** (spec §3.3): memory `fixedMiB: 2048` per process (5C `MemoryRule`; analyzers' declared need is 8C's), 8 processes and 1,024 open files through 5C's `job_limits` (rlimits on unix, the job object on Windows), CPU priority from the job's QoS, 5C's hard kill. After 7C the same limits come from the `plugin` profile.
10. **Egress in 8B is brokered HTTPS without credentials** *(needs owner confirmation)*. Spec §1.5 puts "brokered egress bound to `ModelGateway` grants" in 8G, while spec §3.1 lists brokered egress among the host-side rules every code plugin runs under from 8B. Recommendation: Task 8 builds `net.fetch` with every network rule of spec §9.3 (HTTPS only, declared and approved origins, the host's own resolution with private, loopback, link-local, CGNAT, Tailscale, multicast, unspecified and IPv4-mapped forms refused, connection to the checked address with SNI and `Host` from the origin, same-origin redirects ≤ 3, cross-origin redirects returned unfollowed, 64 MiB and 120 s caps, a per-plugin rate limit) and **no secret injection**: a request naming a grant answers `SECRET_MISSING {name}`, and `secret.grant` (raw) does not exist; 8G binds `ModelGateway` grant handles to (plugin, name, origins) (spec §22 item 21 asks 7 for the origin field) and adds injection. Alternative: build the grant binding in 8B as well, which pulls 7A's `ModelGateway` change and its audit forward.
11. **Response bodies** are held by the host (≤ 64 MiB) and read by the plugin in pages of ≤ 768 KiB (`net.read {handle, offset, length}`, base64 in the reply), released when the call's job ends; streaming a body into `out/` is for cold kinds (8G's providers).
12. **`Tx.via`** (spec §3.8): `Via {plugin: "<id>@<version>", command: "<name>"}` on `fin_core::Tx`, `skip_serializing_if = "Option::is_none"`, ignored by apply and replay, no upcaster (absent = none; existing records hash unchanged). **The server strips `via` from every client-sent `tx.commit`** before validation; only `plugins/host/commands.ts` sets it. Rebase and cherry-pick carry it.
13. **Command arguments use a restricted JSON Schema subset, validated by our own pure code** (`fin-plugin-api::schema_subset`): `type` (`object` at the top, `string`, `number`, `integer`, `boolean`, `array`), `properties`, `required`, `additionalProperties: false` (required at every object), `enum`, `const`, `minimum`/`maximum`, `minLength`/`maxLength`, `pattern` (a linear-time regex: the `regex` crate already in the tree), `items`, `minItems`/`maxItems`, `description` (a message key); nothing else (no `$ref`, no remote references, no formats). The manifest validator checks the schema at install (8A's `validate`), and the host checks arguments with the same code before the job is created. **Deviation from spec §18.1:** no `jsonschema` crate (smaller surface, wasm-safe, no remote `$ref`).
14. **Host API 1.1.0.** `commands` becomes consumable (spec §3.4: a newly consumable kind bumps the minor); `hooks` becomes consumable only in Task 17 (1.2.0). A manifest's `hostApi` range must include the running host's (`PLUGIN_HOST_API`), as 8A checks.
15. **Crash loop** (spec §3.3): three `PLUGIN_CRASHED` exits of one plugin (any key) within 10 minutes set `state: crashLoop` (`plugins` row), stop its processes, fail its queued jobs with `PLUGIN_CRASH_LOOP`, audit `plugin.crashLoop`; only the owner's `plugin.enable` clears it.
16. **Store tree re-hash before a cold start** *(needs owner confirmation)*. 8A checks a package's tree hash at install only, and the store stays writable to the user's account (spec §16.1 "Store integrity", left to be decided with 7C). Recommendation: until 7C makes the store read-only to plugin processes, the host re-hashes the plugin's store directory (`fin_registry::package::tree_hash`, in `fin-node`) before every cold start of a plugin process and refuses a mismatch: the plugin is disabled (`state: disabled`, problem `treeChanged`, audited `plugin.disable {auto: true}`), the call fails `PLUGIN_TREE_CHANGED {plugin}` (exit 7), and re-enabling needs the owner after a re-install. Cost: one BLAKE3 pass over the package per cold start (measured in Task 19 against the cold-start budget). Alternative: trust the store until 7C.
17. **Warnings while unconfined** *(needs owner confirmation)*. Spec §3.1 and §13 define `PLUGINS_UNCONFINED` for hosts where Landlock is disabled. Before 7C every host runs plugins unconfined, and `server.status` already shows the generic `SANDBOX_DISABLED` warning for workers and plugin processes (`apps/finisherd/src/server-admin.ts`). Recommendation: `server.status` and `fin status` carry `PLUGINS_UNCONFINED {enabled: <count of enabled code plugins>, reason: noProfile | landlockDisabled}` whenever at least one enabled code plugin would run unconfined on this host, and `plugin.list` marks each code plugin `confined: false`; the warning disappears when the `plugin` profile applies (Task 16) or no code plugin is enabled. Alternative: rely on the generic `SANDBOX_DISABLED` until 7C and keep `PLUGINS_UNCONFINED` for Landlock-disabled hosts only, as the spec words it.
18. **Manifests that declare `models` cannot be enabled before 8C** (`PLUGIN_CONTRIBUTION_UNSUPPORTED {kind: "models", hostApi}` at `plugin.enable`): the model manager, its licence gate (owner decision 4: the weight licence governs) and ONNX Runtime are 8C's, so 8B never fetches or opens a model.
19. **Developer mode** (spec §5.5): `finisherd --plugin-dir <dir>` (console start only) validates the folder as `@dev/<name>` (source class `dev`), prints its permissions, asks on the console to approve them for this run, and serves it for that run only; it is never reachable by `agent` or `mcp` principals (its commands are absent from their `plugin.commands` and refused `PLUGIN_NOT_FOUND`), never re-hashed (it is meant to change), and shows `PLUGINS_UNCONFINED` until 7C.
20. **Hooks wait for 7F** *(needs owner confirmation)*. Recommendation: Task 17 is gated on 7F's hook engine, and 8B may merge without it: until then `hooks` stays unconsumable (`PLUGIN_CONTRIBUTION_UNSUPPORTED`), host API stays 1.1. Alternative: hold 8B's merge until 7F.
21. **The SDKs implement the envelope's plugin rules** (5C-1 Decisions 3, 35; `TASKS.md` 8B plan note): `hello` with `v` 1 and the SDK's `minor`, no `envelope` hash required; a trailing `--warm` argument means "send `ready` after each job"; an unknown message type from a newer host is answered or dropped by the host's minor rule, never fatal in the SDK.
22. **Plugin ids in `worker.exit`** (the 5C-1 follow-up): `worker.exit` goes to `kv` 2 with `plugin?: PluginRef` (8A's `{bundled: Name} | {hashed: Hashed}`), hashed in `fin-node`: `journalSink` passes the raw id to a `fin-node` journal call, which computes the keyed hash in Rust, so the key never reaches TypeScript. `plugin.exit` (spec §11) is emitted from the same exit event.

## Review focus

1. **No confused deputy:** every host call, commit and job checks invoker ∩ grants and reach; a process never serves another key; rights narrow when the invoker's do. → Task 6 `permission_reach_matrix`, `revoked_invoker_narrows_running_call`; Task 4 `warm_key_never_crosses`; Task 9 `command_commits_as_invoker`.
2. **Nothing ambient:** environment, descriptors and handles of a plugin process; no `via` from clients; no secret ever in a plugin's view. → Task 4 `plugin_env_is_the_allow_list`, `only_stdio_inherited`; Task 3 `client_via_is_stripped`; Task 8 `no_secret_reaches_plugin`.
3. **The broker cannot be pointed inside:** literal, DNS and rebinding private addresses, IPv4-mapped forms, redirects, non-HTTPS. → Task 8.
4. **Bytes the plugin can still change are never trusted:** paged replies only through the host's buffers; cold kinds' outputs validated as copies (exercised with the `testplugin` family's file kind until 8C). → Tasks 7, 14.
5. **Lifecycle:** enable, disable, crash loop, update while running (the old version keeps serving until a person approves a widening), tree re-hash. → Tasks 4, 5.
6. **Confinement switches on automatically and fails closed** once 7C's profile passed its suite on an OS. → Task 16.
7. **The SDKs are the protocol's second implementation:** the conformance profile runs both example plugins. → Tasks 12–14.

---

## Batch A: types and the wire

### Task 1: Tracker, crates, packages and gates — tier: sonnet

**Depends on:** nothing.

**Files:**
- Modify: `TASKS.md` (section "Sub-project 8B" with Tasks 1–20, the batch order and the 7A-2, 7C and 7F gates).
- Create: `crates/fin-plugin-sdk/` and `crates/fin-plugin-test/` (`Cargo.toml` with the workspace fields, `license = "MIT"`, `publish = false`, `lib.rs` stubs with their module docs), `packages/plugin-sdk/` (`package.json` `@finisher/plugin-sdk`, `"private": true`, MIT, `src/index.ts` stub, vitest config), `examples/plugins/{rust-tools,ts-tools}/` (manifests only).
- Modify: root `Cargo.toml` (members gain `examples/plugins/rust-tools`, a `publish = false` binary package; it, `fin-plugin-sdk` and `fin-plugin-test` outside `default-members` like `fin-plugin-cli`; `packages/*` is already in `pnpm-workspace.yaml`), `scripts/check-crate-deps.mjs` (+ test): `fin-plugin-sdk` links `fin-jobs`, `fin-plugin-api`, `fin-shm`, `fin-core` and nothing of `fin-node`, `fin-registry`, `fin-codecs` or wgpu; `fin` links none of the new crates; `fin-node` links neither SDK nor harness.

**Tests:** the crate-deps gate on a fake graph where `fin-plugin-sdk` depends on `fin-node` → one failure.

- [ ] **Steps:** tracker, skeletons, gate and test, three gates → PASS. **Commit** `build(plugins): 8B tracker, SDK and harness skeletons, dependency gate`.

### Task 2: Protocol types, `commands` and `hooks` contributions, the argument schema subset — tier: opus

**Depends on:** Task 1.

**Files:**
- Create: `crates/fin-plugin-api/src/protocol/{mod.rs, context.rs, command.rs, calls.rs, errors.rs}`, `crates/fin-plugin-api/src/schema_subset.rs`, `crates/fin-plugin-api/tests/{protocol.rs, schema_subset.rs}`.
- Modify: `crates/fin-plugin-api/src/{manifest/contributes.rs, contribution.rs (HOST_API 1.1.0; `commands` consumable), validate/content.rs}`, `crates/fin-store/examples/export_schema.rs`; regenerate `@finisher/core-types`.

**Interfaces (pure, wasm-safe):**
```rust
pub struct CommandContribution { pub name: String /* [a-z0-9][a-z0-9-]{0,63} */, pub title: String /* message key */,
                                 pub args: SchemaSubset, pub writes: bool, pub timeout_s: Option<u16> /* ≤ 60 */ }
pub struct HookContribution { pub event: String, pub handler: String }          // typed now; consumable from Task 17
pub struct PluginContext { pub host_api: Version, pub plugin: PluginRefFull /* {id, version} */, pub grants: Vec<Permission>,
                           pub dirs: Dirs /* {plugin, data?, tmp} */, pub locale: String, pub limits: CallLimits }
pub struct CommandRun { pub command: String, pub args: Value, pub project: String, pub branch: String, pub rev: u64 }
pub enum CommandOut { Tx { ops: Vec<fin_core::Op> }, Data { value: Value } }    // ≤ 1 MiB as a line
pub mod calls { /* ProjectSnapshot {page?}, ProjectSnapshotPage {bytes_b64, page, pages, rev}, ProjectView {budget?, range?},
                   NetFetch {request: HttpRequest, grant?}, NetFetchReply {status, headers, handle, length, redirect_not_followed?}, NetRead {handle, offset, length},
                   Log {level, msg} */ }
pub fn validate_args(schema: &SchemaSubset, args: &Value) -> Result<(), Vec<ArgProblem>>;
```
- `validate` refuses a command schema outside the subset (`PLUGIN_MANIFEST_INVALID {path: contributes.commands[i].args, reason}`), a duplicate command name, a timeout over 60 s, and a `hooks` entry while `hooks` is unconsumable (`PLUGIN_CONTRIBUTION_UNSUPPORTED`).
- `consumable(1.1)` = `fontCatalogs`, `captionStyles`, `commands`.

**Tests:** round trips of every type; `validate_args` against a table (each keyword, `additionalProperties` refusal, a catastrophic regex is still linear); `schema_outside_subset_refused` (`$ref`, `format`, `oneOf`); `host_api_1_1_consumes_commands`; generated TypeScript drift gate.

- [ ] **Steps:** tests, implementation, schema export, three gates → PASS. **Commit** `feat(plugin-api): protocol types, commands contribution, host API 1.1 and the argument schema subset`.

### Task 3: `Tx.via` in fin-core; the server strips it from clients — tier: opus

**Depends on:** Task 1.

**Files:**
- Modify: `crates/fin-core/src/tx.rs` (`pub via: Option<Via>`), the rebase and cherry-pick paths in `fin-core` (and `fin-store` where it re-signs records) to carry it, `crates/fin-store/examples/export_schema.rs`, `apps/finisherd/src/handlers.ts` (`tx.commit`: drop `via` from the request before anything reads it), `packages/contracts/src/rpc.ts` (the request schema does not list `via`; the stored record's does).
- Tests: `crates/fin-core/tests/tx_via.rs`, `apps/finisherd/test/tx-via.test.ts`.

**Tests:** `via_absent_is_unchanged_bytes` (a record without `via` serialises and hashes as before: the 4A/4B chain goldens are untouched); `via_survives_commit_rebase_pick_export_import` (spec §3.8's round trip); `apply_and_replay_ignore_via`; `client_via_is_stripped` (a client `tx.commit` with `via` stores none); review change lists show it (the field reaches `history` views).

- [ ] **Steps:** tests, implementation, generated types, three gates → PASS. **Commit** `feat(core): Tx.via, carried by rebase and pick, stripped from client commits`.

## Batch B: processes and lifecycle

### Task 4: The `plugin` family: programs, environment, limits, warm keys — tier: opus

**Depends on:** Tasks 2, 3.

**Files:**
- Create: `apps/finisherd/src/plugins/host/{family.ts, node-runtime.ts, program.ts, keys.ts}`, `apps/finisherd/test/plugins-host-family.test.ts`.
- Modify: `apps/finisherd/src/jobs/kinds.ts` (Decision 3a: `warm.slots` ≤ 16 for `profile: "plugin"`, still 1–2 for the others), `apps/finisherd/src/jobs/{attempt.ts, helpers.ts}` (Decision 3b), `crates/fin-node/src/registry/host.rs` (napi `treeHash(dir)` for Decision 16), `apps/finisherd/src/main.ts` (register the family when the plugin layer starts).

**Behaviour:**
- `program(job)`: the job's `via.plugin` (`<id>@<version>`) → its installed row (enabled, not revoked, not `crashLoop`) → its store directory → `native`: the manifest's executable for this platform, `args: []`; `node`: `process.execPath` with `--permission --allow-fs-read=<plugin dir> [--allow-fs-read=<data dir>] [--allow-fs-write=<data dir>] --allow-fs-read=<tmp> --allow-fs-write=<tmp> <main>` (one path per flag; no `--allow-child-process`, `--allow-addons`, `--allow-worker`, `--allow-wasi`), with `ELECTRON_RUN_AS_NODE=1` set for that child only when `process.versions.electron` is present. The engine appends `--warm`.
- Env: `childEnv()` + `FIN_PLUGIN_PROTOCOL=1` + private `TMPDIR`/`TEMP`/`TMP` under `<server-data>/plugins/tmp/<keyHash>/` (created 0700, swept at start).
- `warmKey` (Decision 3), `warm {idleS: 300, slots: 16}`, `memory` Decision 9, `job_limits` `{processes: 8, openFiles: 1024}`.
- Before a cold start: Decision 16's re-hash (bundled plugins compare with the compiled-in hash; `@dev` plugins skip).
- `hello` from a plugin process: `v` 1 and the minor rule (5C), its kinds must include the job's kind, else `WORKER_PROTOCOL_MISMATCH` → `PLUGIN_PROTOCOL`.
- Errors: 5C's `WORKER_CRASHED` → `PLUGIN_CRASHED`, `WORKER_HUNG` → `PLUGIN_TIMEOUT`, `WORKER_PROTOCOL_MISMATCH`/protocol errors → `PLUGIN_PROTOCOL`, `WORKER_OOM` kept.

**Tests:** `plugin_env_is_the_allow_list` and `only_stdio_inherited` (the example plugins print their environment and descriptors/handles); `node_permission_flags_on_node_24_22` (the TS example cannot read a sibling of its plugin dir, cannot write its plugin dir, cannot spawn: each an observed `ERR_ACCESS_DENIED`); `electron_run_as_node_only_under_electron`; `warm_key_never_crosses` (two projects, two classes: four processes, never shared); `update_uses_a_new_process` (the version in the key); `slots_up_to_16_for_plugin_family`; `worker_exit_names_the_jobs_plugin`; `tree_changed_disables_plugin`; `env_refused_for_plugin_family` and `rpc_refused_for_plugin_kind` still hold.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(plugins): the plugin family with process keys, runtimes, limits and the tree re-hash`.

### Task 5: Enable, disable, crash loop and update for code plugins — tier: opus

**Depends on:** Task 4.

**Files:** Modify `apps/finisherd/src/plugins/{lifecycle.ts, tables.ts, rpc.ts}`; create `apps/finisherd/src/plugins/host/crash-loop.ts`; migration `apps/finisherd/src/control/migrations/plugins/0004.ts` (`state` gains `crashLoop`; `plugins.problem` column); tests `apps/finisherd/test/plugins-runtime-lifecycle.test.ts`.

**Behaviour:**
- `plugin.enable` on a code plugin (owner, human-only as in 8A): the platform must match, `hostApi` must include 1.1, `models` must be absent (Decision 18); then `enabled: true, state: active`, its commands registered (Task 9), audit `plugin.enable`, `plugin.lifecycle`. `PLUGIN_RUNTIME_UNAVAILABLE` is no longer raised for code plugins (it stays for a runtime this build lacks, e.g. a future `python` manifest).
- `plugin.disable`: stop the plugin's processes (5C `shutdown`, then kill after 2 s), unregister its commands, fail its queued jobs `PLUGIN_DISABLED`; running commands are cancelled.
- Crash loop (Decision 15); re-enable clears it.
- **Update while running:** an update that `requires_reapproval` (8A's `approval.rs`) keeps the old version enabled and serving until a person approves the new plan; an approved update switches the row, new jobs use the new key (the version is in it), old processes drain and stop.
- `plugin.list` gains `confined` and `problem`.

**Tests:** `enable_code_plugin_runs_a_command` (with Task 9's stub kind); `disable_stops_processes_and_fails_queued`; `three_crashes_in_ten_minutes_crash_loop` (and two crashes do not); `crash_loop_cleared_by_owner_enable`; `models_manifest_cannot_enable`; `widening_update_keeps_old_version_serving`; `third_party_update_still_needs_acceptance` (8A behaviour unchanged).

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(plugins): enable, disable, crash loop and updates for code plugins`.

## Batch C: rights, host API and egress

### Task 6: The `plugin` principal, rights and reach — tier: opus

**Depends on:** Task 4; the agent rows on **7A-2**.

**Files:** Create `apps/finisherd/src/plugins/host/{rights.ts, reach.ts}`, `crates/fin-plugin-api/src/protocol/rights.rs` (the `JobRights` type, generated); modify `packages/contracts/src/jobs.ts` (`JobRightsSchema` becomes the generated type), `apps/finisherd/src/jobs/submit.ts` (a plugin family's job must carry `rights`). Tests: `apps/finisherd/test/plugins-rights.test.ts`.

**Behaviour:** Decision 8. Reach (spec §3.6): a project, asset or artifact outside the invoker's `projects` is `PROJECT_NOT_FOUND` (ids never leak); `project.snapshot`/`project.view` read only the call's branch, which for an `agent` must lie in its session's subtree (`BRANCH_NOT_ALLOWED`); library-scoped reads resolve only through projects in reach or libraries the invoker may read. Permission absent → `PLUGIN_PERMISSION_DENIED {perm}`. The principal recorded in audit and journal is `plugin:<id>@<version>` acting for the invoker.

**Tests:** `permission_reach_matrix` (each host method of Task 7 × {granted, not granted} × {own project, other project, other branch subtree, other library}); `rights_are_invoker_intersect_grants` (an agent without `project.write` cannot commit through a plugin that has `project:write`); `revoked_invoker_narrows_running_call` (the next call of a running command fails after the invoker's credential is revoked); `no_human_only_scope_ever` (property over all permission sets); agent rows (`agent_branch_subtree_enforced`) after 7A-2.

- [ ] **Steps:** tests, implementation, generated types, three gates → PASS. **Commit** `feat(plugins): plugin rights as invoker ∩ grants, with project, branch and library reach`.

### Task 7: The host API through `onCall` — tier: opus

**Depends on:** Task 6.

**Files:** Create `apps/finisherd/src/plugins/host/calls.ts`, `apps/finisherd/src/plugins/host/log.ts`; tests `apps/finisherd/test/plugins-calls.test.ts`.

**Methods (host API 1.1):**
- `project.snapshot {page?}` (`project:read`): the call's project and branch at the job's pinned `rev`, as canonical JSON paged in ≤ 768 KiB base64 pages; the same `rev` for every page of one job.
- `project.view {budget?, range?}` (`project:read`): `fin view` text (sub-project 4's view), capped at 768 KiB.
- `net.fetch`, `net.read`: Task 8.
- `log {level, msg}`: rate-limited (20 per second per process, then counted), scrubbed (paths and control characters removed), capped (4 KiB), written to the server log with the plugin's keyed id, never journaled.
- **Reserved, answered `INVALID_REQUEST {method, availableFrom}`:** `analysis.get`, `analysis.find`, `media.pcm`, `media.frames` (8C), `media.file` (8H), `job.submit` (8C, Decision 6), `secret.grant` (8G, Decision 10).
- Every call decodes its params with the generated schema first (a malformed call is `PLUGIN_PROTOCOL`, the process is not killed), then checks rights and reach (Task 6).
- stderr of a plugin process: scrubbed and capped (64 KiB per minute) into the server log; never journaled.

**Tests:** each method's happy path and its denials; `snapshot_pages_share_rev` (a commit between pages does not change them); `malformed_params_are_protocol_errors`; `reserved_methods_name_their_batch`; `log_is_rate_limited_and_scrubbed`; `stderr_never_journaled` (the journal's free-text grep).

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(plugins): host API 1.1 calls with rights and reach checks`.

### Task 8: The egress broker (`net.fetch`) — tier: opus

**Depends on:** Task 7.

**Files:** Create `apps/finisherd/src/plugins/host/{egress.ts, egress-address.ts, egress-bodies.ts}`; test hook `apps/finisherd/src/plugins/host/egress-test-hooks.ts` (built only with `testKinds`/test hooks); tests `apps/finisherd/test/plugins-egress.test.ts`.

**Behaviour:** Decisions 10 and 11. Origins: exactly the manifest's approved `net` origins (scheme, host, port; no wildcards). Resolution: `dns.lookup(host, {all: true})`, every address checked (spec §9.3's list, IPv4-mapped IPv6 decoded first); the connection uses an `https.Agent` whose `lookup` returns the checked address, with `servername` and `Host` from the origin, so DNS cannot change between check and connect. Same-origin redirects (≤ 3) followed; a cross-origin redirect returns the 3xx with `redirectNotFollowed: true` and its `Location`. Request bodies ≤ 768 KiB, inline in the call (larger uploads are 8G's, through a cold kind's `out/`); response bodies ≤ 64 MiB held host-side; 120 s per request; 10 requests per second per plugin; `EGRESS_DENIED {origin}`, `EGRESS_NOT_HTTPS`, `EGRESS_PRIVATE_ADDRESS`, `SECRET_MISSING {name}`. The broker sends no cookie jar, no proxy credentials and no `Authorization` of its own. Counters go to `plugin.stats` (Task 15).

**Tests** (spec §15.1 "Egress", built against a loopback HTTPS server through the test resolver hook): `undeclared_origin_denied`; `http_and_ws_refused`; `private_addresses_refused` (literals: 127/8, 10/8, 172.16/12, 192.168/16, 169.254/16, 100.64/10, fc00::/7, fe80::/10, ::1, ::ffff:127.0.0.1, 0.0.0.0, multicast); `dns_answer_with_one_private_address_refused`; `rebinding_between_resolve_and_connect_cannot_redirect` (a resolver that answers differently on the second query is never asked twice); `same_origin_redirects_followed_cross_origin_returned`; `body_and_time_caps`; `rate_limit`; `no_secret_reaches_plugin` (a request naming a grant answers `SECRET_MISSING`; no header is added).

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(plugins): brokered HTTPS egress with address checks and redirect rules`.

## Batch D: commands

### Task 9: Plugin commands: registry, expansion, commit with `Tx.via` — tier: opus

**Depends on:** Tasks 3, 5, 7.

**Files:** Create `apps/finisherd/src/plugins/host/commands.ts`, `apps/finisherd/src/jobs/kinds/plugin.command.ts`; modify `packages/contracts/src/rpc.ts` (`plugin.commands {}` → `{digest, commands: [{name: "plugin:<id>/<cmd>", plugin, command, title, args, writes}]}`; `plugin.command.run {projectId, branch?, name, args, dryRun?}` → `{txId?, rev?, outcome: accepted|rebased|conflict|data, data?, ops?}`), `packages/contracts/src/scopes.ts` (`plugin.command.run` needs `project.read`, and `project.write` when the command `writes`), `apps/finisherd/src/handlers.ts`. Tests: `apps/finisherd/test/plugins-commands.test.ts`.

**Behaviour** (spec §3.8, Decision 7):
1. The name `plugin:<id>/<command>` resolves to an enabled plugin's command (not reachable: `PLUGIN_NOT_FOUND`); `args` checked by `validate_args` (`PLUGIN_COMMAND_ARGS {problems}`).
2. `rev` = the head of the call's branch; the job is submitted owned by the invoker, with `via` and `rights`, on the key's warm process.
3. `CommandOut::Tx {ops}` → `Tx {baseRev: rev, via: {plugin, command}, author: the invoker}` → `tx.commit` under the invoker (fin-core validates, rebases or refuses `CONFLICT`); an op the plugin returns that fin-core refuses is `PLUGIN_TX_INVALID {problems}`; `dryRun` validates without committing (as every write). `CommandOut::Data` is returned as is (≤ 1 MiB).
4. `plugin.commands`' `digest` = BLAKE3 of the canonical list, so `fin` can cache per server.
5. Journal `plugin.command` (Task 15) for every outcome.

**Tests:** `command_commits_as_invoker` (author and principal are the invoker's; `via` names the plugin and command); `conflicting_tx_rebases_or_conflicts`; `invalid_ops_are_plugin_tx_invalid`; `args_validated_before_job`; `dry_run_commits_nothing`; `timeout_at_declared_limit`; `writes_false_command_returning_tx_refused`; `disabled_plugin_command_not_found`.

- [ ] **Steps:** tests, implementation, generated types, three gates → PASS. **Commit** `feat(plugins): plugin commands committed under the invoker with Tx.via`.

### Task 10: `fin @<id> <command>`, help and the exported names — tier: sonnet

**Depends on:** Task 9.

**Files:** Create `crates/fin/src/commands/plugin_cmd.rs`; modify `crates/fin/src/{main.rs, registry_plugins.rs, output.rs}` (argument parsing for `@publisher/name command --key value…`, mapped onto the command's schema; `fin help @<id>` from `plugin.commands`, cached per server by digest under the client's cache dir), `fin schema --registry` (lists plugin command names as `plugin:<id>/<cmd>`). Tests: `crates/fin/tests/plugin_command.rs`.

**Behaviour:** argument forms follow `fin`'s conventions (`--name value`, `--flag`, JSON for objects with `--args-json`); `--dry-run`; exit codes of spec §13. The name `plugin:<id>/<cmd>` is the one 7B's command rules match (agent §7.2) and the one `commandOf` returns for `plugin.command.run`; Task 10 adds a contract test that pins it for 7B.

**Tests:** `fin_runs_plugin_command`; `help_lists_commands_from_cache`; `unknown_command_lists_available`; `schema_registry_contains_plugin_names`; `args_mapping_table`.

- [ ] **Steps:** tests, implementation, three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(fin): fin @<id> <command> and plugin command help`.

### Task 11: Developer mode (`finisherd --plugin-dir`) — tier: sonnet

**Depends on:** Task 9.

**Files:** Create `apps/finisherd/src/plugins/host/dev.ts`; modify `apps/finisherd/src/main.ts` (the flag, console only), `apps/finisherd/src/console/commands/…` (the start-up approval prompt, `safe()` for every manifest string). Tests: `apps/finisherd/test/plugins-dev.test.ts`.

**Behaviour:** Decision 19.

**Tests:** `dev_plugin_runs_for_this_run_only`; `dev_plugin_invisible_to_agents`; `dev_permissions_need_console_approval`; `dev_plugin_not_rehashed`; the console warning is printed.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(plugins): developer mode with --plugin-dir`.

## Batch E: SDKs and the harness

### Task 12: Rust SDK `fin-plugin-sdk` — tier: opus

**Depends on:** Task 2 (types); end-to-end tests after Task 9.

**Files:** `crates/fin-plugin-sdk/src/{lib.rs, runtime.rs, host.rs, command.rs, files.rs, prelude.rs, macros.rs}`, `examples/plugins/rust-tools/{finisher-plugin.json, src/main.rs}` (a `trim-silence`-shaped command that returns ops built from the snapshot, and a `fetch-title` command over `net.fetch`), `crates/fin-plugin-sdk/tests/sdk.rs`.

**Interface:**
```rust
pub trait Plugin: Default {
    fn command(&mut self, host: &Host, call: CommandCall) -> Result<CommandOut, PluginError>;
}
pub struct Host { /* typed host calls over fin_jobs::worker's JobCtx::call */ }
impl Host { pub fn snapshot(&self) -> Result<fin_core::model::Project, PluginError>;     // pages joined, rev checked
            pub fn view(&self, budget: Option<u32>) -> Result<String, PluginError>;
            pub fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, PluginError>;  // net.fetch + net.read pages
            pub fn log(&self, level: Level, msg: &str); pub fn context(&self) -> &PluginContext; pub fn cancelled(&self) -> bool; }
fin_plugin_sdk::main!(MyPlugin);   // hello (v 1, the SDK's minor), --warm, the job loop, cancel, alive, ready
```
`files.rs` (for cold kinds from 8C): `fin-shm` readers and declared-size writers for `in/`/`out/`, slot messages, behind a feature `files` so command-only plugins do not link `fin-shm`.

**Tests:** `sdk_speaks_the_envelope` (against 5C's host in-process: `hello`, a job, `ready`, `--warm`); `unknown_host_message_type_is_not_fatal`; `cancel_reaches_command`; `snapshot_pages_join`; the example builds for the host platform and runs through Task 9's path.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(plugin-sdk): Rust SDK over the fin-jobs envelope, with an example plugin`.

### Task 13: TypeScript SDK `@finisher/plugin-sdk` — tier: sonnet

**Depends on:** Task 2 (generated types); end-to-end tests after Task 9.

**Files:** `packages/plugin-sdk/src/{index.ts, runtime.ts, host.ts, envelope.ts, files.ts}`, `packages/plugin-sdk/README.md` (the esbuild recipe: one ES module, no `node_modules`, no install scripts; `engines.node` → the manifest's `runtime.node`), `examples/plugins/ts-tools/{finisher-plugin.json, src/plugin.ts, build.mjs}`, `packages/plugin-sdk/test/sdk.test.ts`.

**Interface:** `definePlugin({ commands: { "trim-silence": async (args, host) => ({ tx: [...] }) } })`; `host.snapshot()`, `host.view()`, `host.fetch(req)`, `host.log()`, `host.context`, `host.signal` (an `AbortSignal` for cancel); positional reads only (`fs.readSync`) and declared-size writes for cold kinds' files (`files.ts`, 8C onward); generated types from `@finisher/core-types`; reads a trailing `--warm` (Decision 21).

**Tests:** the same envelope tests as Task 12 against 5C's host; `node_permission_denials_surface_as_errors` (the example's attempt to read outside is an `ERR_ACCESS_DENIED` reported as `PLUGIN_PERMISSION_DENIED`-shaped data, not a crash); the bundled example has no `import` outside `node:` and itself.

- [ ] **Steps:** tests, implementation, three gates → PASS. **Commit** `feat(plugin-sdk): TypeScript SDK and an example Node plugin`.

### Task 14: `fin-plugin-test` and `fin-plugin test` with the `command` profile — tier: opus

**Depends on:** Tasks 12, 13.

**Files:** `crates/fin-plugin-test/src/{lib.rs, fake_host.rs, fixtures.rs, profiles/command.rs, fake_https.rs}`, `crates/fin-plugin-cli/src/commands/test.rs` (`fin-plugin test <dir> [--profile command] [--fixture <snapshot.json>]`), `.github/workflows/ci.yml` (run both example plugins through the profile on all three OSes; pinned actions). Tests: `crates/fin-plugin-test/tests/profiles.rs`.

**Behaviour** (spec §7.4): a fake host that speaks the envelope's host side, serves `project.snapshot`/`view` from fixture snapshots, `net.fetch` from a fake HTTPS API (a loopback server with a throwaway certificate), and records every call. **`command` profile:** each declared command answers within its timeout; args outside the schema are never delivered (the harness checks the plugin's own validation is not relied on); returned ops validate under fin-core; cancel is honoured within 2 s; a crash is reported as `PLUGIN_CRASHED`; no host call outside the declared permissions is attempted (every such call is a recorded failure); no secret or path appears in logs. The `analyzer`, `provider`, `codec` and `matte` profiles are their batches' (8C, 8G, 8H).

**Tests:** both examples pass; a deliberately broken example (undeclared permission use, a slow command, a crash) fails with the expected findings.

- [ ] **Steps:** tests, implementation, CI change, three gates → PASS. **Commit** `feat(plugin-test): fake host, the command conformance profile and fin-plugin test`.

## Batch F: diagnostics

### Task 15: Diagnostics, audit and error codes — tier: sonnet

**Depends on:** Tasks 4–9.

**Files:** Modify `crates/fin-core/src/events/{kinds.rs, payloads/plugin.rs, payloads/jobs.rs}` (`plugin.stats` v1, `plugin.exit` v1, `plugin.command` v1 with training field `args: Content<toolArgs>`; `worker.exit` `kv` 2 with `plugin?: PluginRef`, Decision 22), `crates/fin-node/src/journal/plugin_ref.rs` (napi: record a `worker.exit`/`plugin.*` event with a raw plugin id, hashed in Rust), `apps/finisherd/src/jobs/journal-sink.ts` (pass the id through), `apps/finisherd/src/plugins/host/stats.ts` (per-minute counters), the audit kinds `plugin.crashLoop` and `plugin.disable {auto: true}` (Decisions 15, 16), `crates/fin/src/output.rs` and `apps/finisherd/src/console/exit-codes.ts` (`PLUGIN_CRASHED`, `PLUGIN_TIMEOUT`, `PLUGIN_PROTOCOL`, `PLUGIN_OUTPUT_INVALID` (4); `PLUGIN_DISABLED`, `PLUGIN_CRASH_LOOP`, `PLUGINS_UNCONFINED` (5); `PLUGIN_COMMAND_ARGS`, `PLUGIN_TX_INVALID` (3); `PLUGIN_PERMISSION_DENIED`, `EGRESS_DENIED`, `EGRESS_NOT_HTTPS`, `EGRESS_PRIVATE_ADDRESS`, `SECRET_MISSING`, `PLUGIN_TREE_CHANGED` (7)). Tests: the registry's schema tests, `exit_codes_parity`.

**Tests:** `plugin_ref_is_name_or_keyed_hash` (bundled → `Name`, other → `Hashed`, the raw id never in the journal); `worker_exit_kv2_carries_plugin`; `plugin_command_training_args_are_content`; `stats_hold_no_free_text`; `events_off_means_nothing_built`.

- [ ] **Steps:** tests, implementation, generated types, three gates → PASS. **Commit** `feat(core): plugin.stats, plugin.exit, plugin.command; worker.exit names plugins by PluginRef`.

## Batch G: confinement and hooks

### Task 16: Confinement through 7C's `plugin` profile — tier: opus

**Depends on:** **7C's `plugin` profile** (spec §16.1, X1; the 7C plan, being written in parallel, owns the task) and Tasks 4, 5, 15.

**Interface expected from 7C** (reconcile with the 7C plan when it is published):
- `fin-sandbox`'s `Profile::Plugin` and 7C's `Spawner` applying it to every `SpawnRequest` with profile `plugin` on an OS where the plugin escape suite passed, from the request's grants (read roots: the plugin's store directory, the runtime, system libraries, the call's `in/` for cold kinds; write roots: `out/` for cold kinds, the per-project data directory when granted, the private temp; no network; GPU only with `gpu`; macOS TCC and Apple Events denied; Windows: one container profile per (plugin, project) and per-job ACL grants through the broker);
- `JobHostCaps` reporting, per profile and OS, whether the profile is enforced (`plugin: enforced | notProven | landlockDisabled`);
- `SANDBOX_UNAVAILABLE {profile: plugin}` when an enforced profile fails to apply, and `sandbox.violation {profile: plugin}` audit records for the denials the launcher observes.

**Files:** Create `apps/finisherd/src/plugins/host/confinement.ts`; modify `apps/finisherd/src/plugins/host/family.ts` (the grants above in the `SpawnRequest`; Node flags kept as the seat belt inside), `apps/finisherd/src/plugins/{rpc.ts, lifecycle.ts}` (`confined` and the `thirdPartyCode {confined}` warning read `JobHostCaps`), `apps/finisherd/src/server-admin.ts` (Decision 17's `PLUGINS_UNCONFINED`). Tests: `apps/finisherd/test/plugins-confinement.test.ts`; the plugin escape suite's plugin side (`examples/plugins/escape/`: a native and a Node plugin that attempt each case of spec §16.1 and report what happened).

**Behaviour:** where `plugin` is `enforced`, every plugin process (bundled, registry and `@dev` alike) starts through it, automatically and without re-approval; access a plugin took without declaring it now fails and is audited. Where it is `notProven` or `landlockDisabled`, plugins keep running unconfined with the warning; nothing is refused. Whether the tree re-hash of Decision 16 stays once the store is read-only to plugin processes is recorded with the owner's answer.

**Tests:** `profile_applied_automatically_where_enforced`; `apply_failure_fails_the_call` (`SANDBOX_UNAVAILABLE`); `landlock_disabled_runs_with_warning`; `confined_flag_follows_caps`; the escape suite (7C runs it in CI; this task makes the plugin side and checks every case is an observed denial on each OS where the profile is enforced).

- [ ] **Steps:** rebase onto 7C, tests, implementation, three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(plugins): start plugin processes through 7C's plugin profile where it is enforced`.

### Task 17: Hook handlers (`hooks:observe`, `hooks:modify`) — tier: opus

**Depends on:** **7F's hook engine** (agent §9) and Task 9.

**Files:** Create `apps/finisherd/src/plugins/host/hooks.ts`, `apps/finisherd/src/jobs/kinds/plugin.hook.ts`; modify `crates/fin-plugin-api/src/contribution.rs` (`hooks` consumable at host API 1.2.0), `crates/fin-plugin-api/src/protocol/hook.rs`, the SDKs (`hook` handlers), audit `hook.modify`. Tests: `apps/finisherd/test/plugins-hooks.test.ts`.

**Behaviour** (spec §3.6, agent §9.1): 7F's engine calls plugin handlers in the `plugin` layer through `plugin.hook` jobs on the subscriber's key; receiving an event needs `hooks:observe` for it; answering `modify` needs `hooks:modify` granted for that hook, else the answer is treated as `allow` and audited; a handler sees only events its subscriber may see; both payloads of a `modify` are audited; timeouts follow agent §9.1's order.

**Tests:** `observe_without_grant_receives_nothing`; `modify_without_grant_is_allow_and_audited`; `modify_with_grant_rewrites_payload_and_audits_both`; `handler_sees_only_subscribers_events`; `hook_timeout_ordering`.

- [ ] **Steps:** rebase onto 7F, tests, implementation, three gates → PASS. **Commit** `feat(plugins): hook handlers with hooks:observe and hooks:modify`.

## Batch H: e2e, validation, docs

### Task 18: e2e `plugin-code` scenario — tier: sonnet

**Depends on:** Tasks 1–15 (Tasks 16 and 17 add their steps when they land).

**Files:** Create `scripts/e2e/plugin-code.mjs`; modify `scripts/e2e.mjs`.

**Scenario:** build both example plugins; publish them into a local registry made with `fin-plugin registry init|publish` and generated keys; add the registry (root pinned); plan and install the TS plugin (`thirdPartyCode {confined: false}` first, id typed back) and the Rust one; enable both (owner); `fin @example/ts-tools trim-silence --dry-run` then for real: the commit's author is the invoker and `via` names the plugin; a second branch's agent-like principal cannot reach the first project (`PROJECT_NOT_FOUND`); `fetch-title` against the scenario's loopback HTTPS server through the test resolver hook succeeds, and against a private address is `EGRESS_PRIVATE_ADDRESS`; kill the plugin process three times → `crashLoop` and the audit row; disable → `PLUGIN_DISABLED` for queued work; edit a file in the store → `PLUGIN_TREE_CHANGED` (Decision 16, if confirmed); `server.status` shows `PLUGINS_UNCONFINED` (Decision 17, if confirmed); the journal holds `plugin.command`, `plugin.exit`, `worker.exit` `kv` 2 with a keyed `plugin`, and no raw plugin id, path or stderr text (free-text grep).

- [ ] **Steps:** scenario, a green CI run on all three OSes → **Commit** `test(e2e): plugin-code scenario`.

### Task 19: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

**Depends on:** Tasks 1–18 (16 and 17 where they landed).

Each row on **macOS arm64**, **Windows 11 x64** and **Linux x64** (kernel ≥ 6.2, and a Landlock-disabled host for the warning row), as a non-elevated user; under the packaged Electron app on macOS and Windows where one exists (sub-project 9), else with finisherd's Node. Numbers to `archive/sp8/8b-validation.md` by role, never by host; durable conclusions to the spec (Task 20).

- [ ] 1. `FIN_REQUIRE_SERVER=1 cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` (all scenarios, `plugin-code` included) green.
- [ ] 2. Budgets (spec §15.4): cold start to the first `ready`/answer: native ≤ 150 ms, Node ≤ 400 ms (plus the re-hash of Decision 16, reported separately); command round trip on a warm process ≤ 20 ms p95 over 200 runs (Decision 7: record the 5C hand-off and the commit separately).
- [ ] 3. Node's permission flags on Node 24.22 on each OS: the example's denied reads, writes, spawns and addon loads; `ELECTRON_RUN_AS_NODE` only for plugin children under Electron.
- [ ] 4. Environment, descriptors and handles of plugin processes on each OS (nothing beyond the allow-list and stdio); on Windows the kill-on-close job object and the process limit; on unix the rlimits.
- [ ] 5. The broker on each OS: the private-address matrix with the system resolver (not the test hook) against local addresses of the host; an IPv6-only origin; a proxy in the environment is not used.
- [ ] 6. macOS: a TCC-protected read from an unconfined plugin before 7C (expected: the prompt is the app's; record it for the spec's residual); after Task 16, denied without a prompt.
- [ ] 7. Crash loop, disable and update-while-running on each OS; a store tree edited behind the server.
- [ ] 8. Where 7C's profile is enforced: the plugin escape suite passes with observed denials only.
- [ ] 9. CI: a green `ci.yml` run on all three OSes with both example plugins through `fin-plugin test`.

**8B is done** when every row passes or has an accepted, recorded exception. **Commit** (records only) `docs: 8B validation results`.

### Task 20: Spec as built, architecture pointer, notices and tracker — tier: sonnet

**Depends on:** Task 19.

**Files:** Modify `docs/specs/2026-11-07-sub-project-8-plugins-design.md` (status line; a new **§24 "Sub-batch 8B as built"** mirroring §23: this plan's decisions As built / Deviation, review fixes, owner decisions and the answers to the questions below, measured results, residuals; §3.4 rewritten to the one wire (Decision 2); "As built" notes in §1.5, §3.1–3.8, §4.2, §5.1, §5.3, §5.5, §7, §9.3, §11, §13, §15.1, §16, §17, §18.1 (no `jsonschema`); §22 gains the 5C/8 wire contradiction and its resolution), `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§6.12's residual "`journalSink` leaves the `plugin` field out" marked resolved; the plugin family's warm-slot limit), `docs/specs/2026-11-01-finisher-architecture-design.md` §27 (a pointer item for 8B; arch §16's sandbox wording per spec §22 item 23), `THIRD_PARTY_NOTICES.md` (nothing new is linked; the SDK packages' own MIT notice), `TASKS.md` (8B done; residuals and follow-ups for 7C, 7F, 8C, 8G, sub-project 11's publication of the SDKs).

- [ ] **Steps:** write the as-built section from the tasks' commits and Task 19's results, update the tracker → three gates → PASS. **Commit** `docs: 8B as built`.

---

## Owner decisions (2026-11-07) as they bear on 8B

| Decision | In 8B |
|---|---|
| 1 Third-party code plugins allowed now, with warnings | Registry code plugins become enableable (Task 5) with 8A's warning and acceptance; unconfined until 7C's `plugin` profile lands on an OS, then confined automatically (Task 16); nothing refused for lack of a sandbox. |
| 2 Brokered secrets | No key ever reaches a plugin; 8B's broker injects nothing, and has no raw path (Decision 10); grant binding is 8G's. |
| 4 The weight licence governs | No model is fetched or opened in 8B; manifests with `models` wait for 8C's licence gate (Decision 18). |
| 5 Generation via OpenRouter and fal.ai | 8G's; the broker is the path they will use. |
| 7 2-of-3 offline root keys | Sub-project 11's ceremony; 8B's tests and e2e use generated keys only. |

## Questions for the owner

1. **Plugin kinds and `jobs.submit` (Decision 6):** keep `registerKind`'s refusal of `rpc: true` for plugin families permanently, so clients reach plugin work only through typed RPCs and plugins submit their own kinds through a `job.submit` host call from 8C (recommended), or let plugin kinds be submitted directly with `jobs.submit` under a host-written `paramsReach`?
2. **Egress without credentials in 8B (Decision 10):** may 8B ship `net.fetch` with all of spec §9.3's network rules but no secret injection (a request naming a grant answers `SECRET_MISSING`), leaving `ModelGateway` grant binding and injection to 8G (recommended), or should the binding be built in 8B too?
3. **Store re-hash before 7C (Decision 16):** re-hash a plugin's store directory before every cold start and disable it on a mismatch until 7C makes the store read-only to plugins (recommended), or trust the store until 7C?
4. **`PLUGINS_UNCONFINED` before 7C (Decision 17):** show `PLUGINS_UNCONFINED` in `server.status` and `fin status` on every host while an enabled code plugin runs unconfined (recommended), or only on Landlock-disabled hosts, as the spec words it, relying on the generic `SANDBOX_DISABLED` warning elsewhere?
5. **Hooks and 7F (Decision 20):** may 8B merge without hook handlers when 7F's hook engine is not ready, keeping `hooks` unconsumable until a follow-up task (recommended), or should 8B wait for 7F?
