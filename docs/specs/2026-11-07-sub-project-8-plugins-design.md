# Sub-project 8: Plugins and registry — Design

- **Status:** revision 4 (2026-11-08), amended 2026-11-09 after an external review (findings 2 and 3, validated): warm plugin processes have no per-call directories (§3.2, §3.5, §5.1, §16.1) and process memory, temp state and `pluginData` are keyed by effective authority (§3.3, §5.1, §12, Decision 28). sub-batch 8A is built (see below and §23); sub-batch 8B is built except confinement, hook handlers and the `fin-plugin-test` harness, and its cross-platform validation has not run (§24, as built 2026-11-12); 8C–8H are design, at revision 3 (2026-11-07). Revision 2 answers the independent review of revision 1 (`848f01f`): the confused-deputy, shared-memory, sandbox-profile and TUF blockers, the high and medium findings, the v1 cuts and the coordinator's ownership decisions (§1.3, §1.6, §22). Revision 3 records the owner's decisions of 2026-11-07 (§20) and revises the body where they differ from revision 2's recommendations: third-party code plugins run before 7C (§3.1, §5.3, §16.1), and the v1 generation providers are OpenRouter and fal.ai (§9.4). Revision 4 (2026-11-08) records **sub-batch 8A, packages, registries and lifecycle, as built** (§3.7, §4–6, §11, §13–15, §17–18 and §20 carry "As built" and "Deviation" marks; §23 maps every decision of the 8A plan, lists the owner decisions and review fixes taken while building it, the measured results and the residuals). Where §23 and an unmarked paragraph differ, §23 wins. §24 does the same for 8B, and wins over an unmarked paragraph where they differ. Sub-batches 8C–8H are not built; their sections are the design. The plan for 8A is `docs/superpowers/plans/2026-11-07-sub-project-8a-packages-and-registries.md`.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited as *arch §n*). This document refines arch §4 rule 6, §6.2–6.3, §8.3, §9.11–9.12, §12.1, §12.6–12.7, §13, §14, §15, §16, §18, §21.1, §22 and §27.4 for sub-project 8. Where the two disagree, this document wins for sub-project 8, and arch §27 gets a pointer when each sub-batch merges.
- **Builds on:** sub-project 4 (*core §n*: `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`), the diagnostics hooks D1–D7, and the drafts of sub-project 5 (*cp §n*, branch `sp5-design`: principals, scopes, the console, `fin-jobs`, the library stage framework, review items), sub-project 6 (*media §n*, branch `sp6-design`: `fin-worker`, PCM and frame feeds, the codec traits and their conformance suite, masks) and sub-project 7 (*agent §n*, branch `sp7-design`, under revision: branch subtrees per session §5.1, command rules §7.2, approvals §7.3, `fin-sandbox` and the ACL broker §8, hooks §9, `SecretStore` and `ModelGateway` grants §10, budgets §13). §16 lists exactly what sub-project 8 needs from each; the `plugin` sandbox profile is a **cross-design requirement** on sub-project 7 (§16.1, X1).
- **House style:** as the sub-project 4 spec. Each sub-batch gets its own plan; each section is rewritten "as built" when its sub-batch lands.
- **Licence facts** in §6.6, §8 and §18 were checked against primary sources (crates.io and npm metadata, each project's licence file or model card) on 2026-11-07. They are not legal advice.

---

## 1. Goal, scope and sub-batches

### 1.1 Goal

Sub-project 8 makes Finisher extensible without trusting what extends it, and ships the analysis that the v1 features stand on:
- a **package format, manifest and lifecycle** (install, update, enable, permissions, removal) shared by first-party and third-party plugins (arch §14.1);
- **local-folder registries** verified with **TUF**, with the public registry's protocol implemented in the client (arch §15); a git registry is a local folder the user keeps up to date with `git pull`;
- a **plugin host**: out-of-process plugins over JSON-RPC, a capability-scoped host API whose every call is checked against the caller's reach, and the `plugin` principal that sub-project 5 reserved (cp §3.2);
- **SDKs** for Rust and TypeScript, a test harness and conformance suites;
- the **bundled analyzers**: transcription and alignment, scenes, semantic search, reframe and audio (arch §13), whose results become fin-core data, then annotations or ops;
- the **generation-provider contract**, the **codec-backend** and **matte-producer** interfaces (with sub-project 6), and media content (WGSL effects, transitions, LUTs).

### 1.2 Non-goals

- **The public signed registry service** (arch §1 "deferred"), Sigstore and the publisher website. The client speaks the protocol; nothing here hosts it.
- **The UI**: plugin browser, permission dialogs, iframe panels and declarative surfaces (sub-project 9). §5.4 defines the plan objects those dialogs render; the console renders them until then.
- **The OS sandbox, the ACL broker, the hook engine, budgets and approval cards** (sub-project 7). §16 states what sub-project 8 requires of them.
- **Background removal** (after v1, arch §1). The matte interface ships with a key-based reference producer (§10.3).
- **The FFmpeg codec plugin** (`finisher-plugins-copyleft`, arch §18). The interface and conformance suite are here.
- **Agent-provider plugins** (arch §8.2): the contribution point is reserved and built against sub-project 7's adapter contract after v1.

### 1.3 Cut from v1 (accepted after review)

| Cut | Consequence | Section |
|---|---|---|
| Python runtime, `uv`, the Python SDK | v1 code plugins are `native` or `node`; the Python design is kept as an after-v1 note | §3.2, §7.3 |
| WebAssembly runtime | no in-process sandbox; arch §12.7's third-party real-time audio effects wait (decision 2) | §3.2 |
| WebSocket brokering | brokered egress is HTTPS request/response and streamed responses only | §9.3 |
| Agent installs (`agentInstall`) | every install, update and enable is a person's | §5.3 |
| Validating contributions with no consumer | a manifest naming a contribution kind the running host cannot consume is refused (`PLUGIN_CONTRIBUTION_UNSUPPORTED`); each kind is unlocked by the batch that consumes it | §4.2 |
| `slots`, `importers`, `embedders` contributions | no extension slots, no import plugins, no cloud embedders (D29's cloud indexing waits, §22 item 19) | §4.2, §8.6 |
| Opt-in diarization, TransNetV2, Beat This! | v1 has no speaker labels, no learned dissolve detection, DSP beats only | §8.3–8.5 |
| The LanceDB evaluation | 8E starts with an exact scan and adds `usearch` only after its licence and dependency check | §8.6 |
| TUF for the bundled plugins | bundled plugins are covered by the app's signature and a compiled-in hash list | §5.2 |
| A git transport (`gix`) | v1 git registries are local folders the user `git pull`s | §5.2 |

### 1.4 Success, for every sub-batch

- Every capability is driven and tested through `fin` → `finisherd`, the `finisherd` console (cp §4.6) for human-only operations, or `fin-plugin` (the publisher CLI, §5.6).
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS arm64, Windows 11 x64 and Linux x64; from 8B on, the plugin escape suite (§16.1) passes where 7C has landed.
- The licence gate covers plugin packages, model weights (by their recorded licence) and evaluation data (by its data card).
- No UI work. No host names, IPs, user names or key material in tracked files; test keys are generated in tests or are obviously fake fixtures.

### 1.5 Sub-batches and order

| Batch | Delivers | Depends on | Unblocks |
|---|---|---|---|
| **8A** Packages, registries and lifecycle | `fin-plugin-api` (manifest, permissions, licences; generated types), the `.finpkg` format and its strict reader, `fin-registry` (TUF client and editor over local folders, trusted store, advisories, a test-only HTTP transport), the plugin store, install plans and approvals, install/update/remove/enable, bundled plugins (hash-verified), content packs with consumers (font catalogs, caption styles), `fin-plugin` publisher tooling, `fin plugin …`, `finisherd plugin …` | 5A (Tasks 1–8 and 11 need nothing of 5A) | 8B; sub-project 11 (template and caption packs) |
| **8B** Plugin host and SDKs | the plugin protocol, process runtimes (`native`, `node`), process keys (§3.3), the `plugin` principal and reach checks, the `fin-jobs` adapter, server-side command expansion with `Tx.via`, plugin command names in `fin` and in sub-project 7's rules, hook handlers with `hooks:observe` and `hooks:modify`, `fin-plugin-sdk` (Rust), `@finisher/plugin-sdk` (TS), `fin-plugin-test`; registry-installed code plugins run from 8B on, unconfined until 7C's `plugin` profile lands, behind a high-risk warning and an explicit acceptance at install (owner decision 1, §5.3) | 8A, 5C, 6A (`fin-shm`), 7A; 7C for OS confinement (applied automatically when it lands); 7F for hooks | 8C–8H |
| **8C** Analysis foundation and speech | fin-core `analysis` types and `ArtifactRef`, the artifact cache and recipes, analysis jobs and library stages, ONNX Runtime and the model manager, `fin-analyze` (the bundled analyzers binary), VAD, transcription, number verbalisation, alignment and boundary correction (retrained and re-selected on permissive labels, §8.3), transcript views, word addressing, captions from transcripts, re-sync and paper edits | 8B, 5C, 5D, 6C | 8D, 8E |
| **8D** Scenes and audio analysis | scene detectors (PySceneDetect port), loudness curves, speech/music classes, DSP beats, `compose::duck`, markers and splits from analysis | 8C, 6A–6B (frames), 6C | 8E, 8F |
| **8E** Semantic search | chunking, SigLIP2 embeddings, an exact-scan vector index (then `usearch`), transcript full-text, hybrid `library.search`, `fin search` | 8C, 8D, 5D | sub-project 11 (clip finder) |
| **8F** Reframe | face and person detection, tracking, shot-aware smoothing, crop paths, `compose::reframe` | 8D, 6B, 6D | sub-project 11 |
| **8G** Generation providers | the provider contract, catalogs, brokered egress bound to `ModelGateway` grants, paid-call safety, provenance, the first-party providers OpenRouter and fal.ai (owner decision 5, §9.4) | 8B, 7A (`SecretStore`, grants); 7B and agent §13 for agent budgets and approvals | sub-project 11 |
| **8H** Media interfaces | codec backends as plugins, matte producers and the key matte, WGSL effects, transitions and LUT packs with `plugin.pin`/`unpin`/`repin` and pinned ParamSpecs, offline audio processors, schema version 5 (with matte assets, owner decision 3, and qualified effect kinds, Decision 30; numbered 5 after the keyframe-id (3) and 6B-1 video-field (4) bumps) | 8B, 6B, 6D, 6E | copyleft repo, after-v1 background removal |

**8A status (2026-11-08): built and merged into the integration branch. Its cross-platform validation (8A Task 15) is partly done and still owed on macOS and Windows 11 (§23.5).**

**Order:** 8A → 8B → 8C → {8D, 8G} → {8E, 8F} → 8H. 8G needs only 8B and 7A and can run beside 8C–8F; 8H needs 8B and sub-project 6's 6D/6E and can run beside 8D–8F. Arch §24 puts sub-project 7 before 8; no part of 8B waits for 7C (owner decision 1): 7C adds OS confinement to plugins that already run.

**8B status (2026-11-12): built on branch `sp8b` except confinement (Task 16, waits for 7C's `plugin` profile) and hook handlers (Task 17, waits for 7F); the cross-platform validation (Task 19) has not run. §24 records the decisions as built, the review fixes and the residuals; where §24 and an unmarked paragraph differ, §24 wins.**

### 1.6 Ownership across sub-projects (coordinator decision)

| Owner | Owns | Sub-project 8 uses it as |
|---|---|---|
| **Sub-project 5** | `fin-jobs`: job messages, the scheduler, resource pools, `externalRef`, idempotency, retries and bisection | analyzer, codec, matte and generation jobs are ordinary `fin-jobs` jobs |
| **Sub-project 6** | the **one** `fin-worker` binary (roles `library`, `media`), and the hardened shared-memory crate **`fin-shm`** (§3.5) | decode service for PCM and frames; `fin-shm` for every plugin data exchange |
| **Sub-project 7** | `fin-sandbox` and the ACL broker, with profiles `agent`, `hook`, `worker` and **`plugin`** | every plugin process starts through `fin-sandbox` with the `plugin` profile (§16.1) |
| **Sub-project 8** | the plugin host (`apps/finisherd/src/plugins/`), its protocol, and its **adapter to `fin-jobs`** (a plugin process that runs a job speaks `fin-jobs`' worker messages through the plugin protocol) | — |

Revision 1 named sub-project 5's binary `fin-workers` and sub-project 6's `fin-worker`; there is one, `fin-worker`.

### 1.7 What exists today (the baseline)

- **fin-core** owns 50 primitive op modules (`crates/fin-core/src/ops/`), multi-op builders in `compose/`, the param registry (`params/`; built-in effects `colorAdjust`, `lut`, `blur` with `&'static str` paths), eval, captions (`CaptionBlock.source: {artifact: String, wordRange}`), `Mask {matte: String, …}`, `SCHEMA_VERSION = 2` with frozen upgrade steps (`upgrade/`), and the events registry (`Source` = `fin|finisherd|finNode|ui|agent`, `ActorKind` includes `plugin`, closed name tables, `ContentKind`).
- **No artifact type exists.** Analysis artifacts are referenced by bare strings in two places and produced nowhere.
- **fin-fonts** fetches pinned files over HTTPS (`ureq`, `rustls` with the `ring` provider) into a content-addressed cache, unpacks zip archives (`zip` 9) and merges catalogs by entry id; the 4C plan leaves registry-supplied font catalogs to sub-project 8.
- **`fin`'s command registry** is static (`crates/fin/src/registry.rs`); sub-project 7 exports it as `packages/contracts/registry.json` and enforces command rules on the server by command name (agent §7.2).
- **finisherd** has no plugin host.

---

## 2. Principles

1. **A plugin is untrusted code with declared needs.** It gets exactly what its granted permissions name, through the host, checked per call against the call's context. It never gets ambient authority: no inherited environment, no credentials, no project folder, no socket to `finisherd`, no network. The host gives none of these; the OS denies them once 7C's `plugin` profile has landed on that OS, and until then a code plugin could take them with the user's rights (§3.1, owner decision 1).
2. **No confused deputy.** Everything a plugin does on someone's behalf runs with that someone's rights intersected with the plugin's grants, and nothing a plugin learned for one principal is reachable by another (§3.3, §3.7).
3. **The host never trusts bytes the plugin can still change.** Inputs live where the plugin can only read; outputs are copied out of plugin-writable files and validated as copies (§3.5).
4. **Trust comes from signed metadata, not from where a file was found.** Registry packages are verified through TUF metadata against a root the owner pinned; bundled plugins through the app's own signature and hashes compiled into it.
5. **Plugins propose; fin-core decides.** A plugin writes canonical state only by returning a transaction of fin-core's primitive ops, validated and committed under the invoker's rights (D24, arch §6.3). Plugins cannot define primitive ops.
6. **Analysis results are data first.** Every analyzer writes a typed, versioned fin-core artifact. Annotations are views of artifacts; ops come only from pure fin-core builders over artifacts.
7. **First-party plugins use the third-party path.** Bundled analyzers have manifests and permissions and run through the same host and sandbox profile.
8. **No install-time code, no auto-escalation.** Installing unpacks and verifies; nothing runs. A new permission, and any update to content that steers agents, needs a person.
9. **Licences are data and are gated** (§18), for code, models and evaluation data alike.
10. **Every error is `{code, message, details, fix?}`** with a stable exit code; every diagnostic field has a class (arch §21.1).

---

## 3. The plugin model

### 3.1 Kinds

| Kind | What runs | Examples | Isolation |
|---|---|---|---|
| **Content pack** | nothing: data validated in Rust by the batch that consumes it | font catalogs, caption styles (8A); skills, saved prompts, agent roles, critic rubrics (when sub-projects 7 and 10 consume them); LUTs, WGSL effects and transitions (8H); templates and presets (sub-projects 6, 10, 11) | none needed; WGSL runs in sub-project 6's render worker after validation (§10.4) |
| **Code plugin** | OS processes keyed per §3.3, or one per job | commands, hook handlers, analyzers, generation providers, codec backends, matte producers | `fin-sandbox` `plugin` profile (§16.1) once 7C lands + capability-checked host API |

**Code plugins before 7C (owner decision 1).** Code plugins, third-party ones from registries included, run from 8B on, before 7C's `plugin` profile exists. Until the profile passes the plugin escape suite on an OS, a plugin process there is **unconfined**: it runs with the user's rights, and what holds is what the host itself does:
- the environment allow-list (`childEnv()`, §3.3) and no credentials: no `FIN_*` credential, no provider variable, no key handed over except a raw grant (§9.3);
- brokered egress (§9.3) as the only network path the host offers, and secrets only through grant handles;
- the `in/`/`out/` rules and copy-then-validate (§3.5), reach checks and invoker ∩ grants (§3.6–3.7);
- for `node` plugins, Node's permission flags (§3.2: a seat belt, not a sandbox);
- process limits where the OS gives them without `fin-sandbox` (rlimits on unix, a kill-on-close job object on Windows) and `fin-jobs`' hard kill.

Nothing at the OS level stops an unconfined plugin from opening sockets, reading the user's files or (on macOS) triggering TCC prompts attributed to the app. Hence: installing a **third-party code plugin** (any code plugin not bundled and not a developer plugin) is **high risk whatever its permissions**, its install plan carries the warning `thirdPartyCode` (with `confined: false` while the profile is absent on that OS), and approval needs an explicit acceptance (`accept.thirdPartyCode`, §5.3) on top of the permission grant. Code plugins never update automatically (§5.3). **When 7C lands**, the host starts every plugin process, bundled and third-party alike, through the `plugin` profile on each OS where it passes the escape suite, automatically and without re-approval; access a plugin took without declaring it then fails as a denial (`sandbox.violation`, audited). On a host where Landlock is disabled (Linux kernel ≥ 6.2 with it turned off), plugins keep running unconfined: `server.status` and `fin status` carry the persistent warning `PLUGINS_UNCONFINED` and a diagnostics event is recorded (agent §8.9, follow-up decision F2); nothing is refused for it.

**As built (8B).** Registry code plugins are enableable (`plugin.enable` re-validates the manifest on the store directory: platform, host API, Node range; `models` is refused `PLUGIN_CONTRIBUTION_UNSUPPORTED {kind: models, hostApi}` until 8C's licence gate). They run unconfined: `server.status` and `fin status` carry `PLUGINS_UNCONFINED {enabled, reason: profileNotReady | landlockDisabled | spawnerUnconfined}` on every host while at least one enabled, active code plugin would run unconfined (the count excludes disabled and crash-looped plugins), and `plugin.list` marks each code plugin `confined: false`. `spawnerUnconfined` means the profile could be enforced but the job host's spawner does not apply it. Nothing is refused for lack of a sandbox. Brokered egress (`net.fetch`) is built with every §9.3 network rule and no secret injection (§9.3).

### 3.2 Runtimes of code plugins

| Runtime | Manifest | Start | When to use |
|---|---|---|---|
| `native` | an executable per platform in the package | through `fin-sandbox` once 7C lands (unconfined before, §3.1) | analyzers, codecs, mattes: anything with ONNX Runtime, GPU or SIMD. The bundled analyzers are one native binary, `fin-analyze` (§8.1) |
| `node` | `main` (one bundled `.mjs` file: no `node_modules`, no install scripts) and **`node`: a minimum Node version** (a semver range; the host refuses with `PLUGIN_NODE_VERSION` when its own Node is older) | finisherd's own Node (`process.execPath`), through `fin-sandbox` once 7C lands, with `--permission --allow-fs-read=<plugin dir>` plus, when `fs pluginData` is granted, `--allow-fs-read=<data dir>` and `--allow-fs-write=<data dir>`, and `--allow-fs-read=<tmp> --allow-fs-write=<tmp>` for the key's private temp directory (one path per flag; **no per-call path**: the flags are fixed at spawn and Node has no way to grant a path afterwards, so a warm process has no `in/` or `out/`, §3.5; a cold kind with file inputs or outputs gets its job directory's flags at its own spawn) and no `--allow-child-process`, `--allow-addons`, `--allow-worker` or `--allow-wasi`. Under Electron, `ELECTRON_RUN_AS_NODE=1` is set for that child only | commands, providers, hooks: glue around HTTP APIs |
| `python` | — | — | **after v1** (design note: per-plugin virtual environment by `uv` with `--no-build --require-hashes --only-binary :all:`, run as `python -I -m <module>`; `uv` and CPython downloaded by hash on first use, never bundled) |
| `wasm` | — | — | **after v1** (decision 2) |

- **Node's permission model is a seat belt, not the sandbox** (Node documents that it does not stop malicious code). The `plugin` profile is the guarantee once 7C lands; before it, the seat belt is all a `node` plugin has (§3.1).
- **Node plugins under the signed app (macOS).** A child the app spawns runs with the app's code signature and entitlements when it is the app's own Electron binary, and macOS attributes TCC decisions (camera, microphone, Documents, Desktop, Downloads, Photos, contacts, screen recording) to the *responsible* process, which is the app for any child, whatever binary it runs. A separate unentitled helper would therefore drop the entitlements but **not** the inherited TCC grants, unless it disclaims responsibility through a private launch attribute. **Decision:** once 7C's `plugin` profile lands on macOS, every plugin process (`node` and `native`) runs under a Seatbelt profile that denies `mach-lookup` of `com.apple.tccd` and `com.apple.tccd.system`, denies `appleevent-send` and `mach-lookup` of `com.apple.coreservices.appleevents`, and grants no read root inside TCC-protected folders. Requests then fail closed with no prompt, for every runtime alike, and the plugin escape suite tests it (§16.1). Before then, a plugin's TCC requests are the app's and may prompt the user in its name (§3.1). The app's only entitlement is `com.apple.security.cs.allow-jit` (arch §27.8), which a Node child needs anyway.
- **ONNX Runtime is host-provided.** The Rust SDK's `onnx` feature loads the app's pinned ORT shared library from a read-only path given at `initialize`.

**As built (8B).** A plugin is a worker of the `plugin` family (`Program::Path`), spawned as `<program> <args…> --warm`. `native` runs the manifest's executable for the platform with no arguments of its own. `node` runs `process.execPath --permission` with one `--allow-fs-read` per readable path (the plugin directory, its data directory, its private temp) and `--allow-fs-write` for the data and temp directories only; never `--allow-child-process`, `--allow-addons`, `--allow-worker` or `--allow-wasi`; `ELECTRON_RUN_AS_NODE=1` is set for that child only when finisherd itself runs under Electron. The environment is an allow-list plus `FIN_PLUGIN_PROTOCOL=1` and private `TMPDIR`/`TEMP`/`TMP`; only stdio is inherited. Limits before 7C (from 5C's `job_limits`): 2,048 MiB memory, 8 processes, 1,024 open files. **Residuals:** Node 24.22's permission model has no network control, so a `node` plugin can open sockets until 7C's profile applies; the process limit is enforced only where the job host has leaf limits (cgroups on Linux, Job Objects on Windows); `openFiles` is not applied on macOS (`posix_spawn`) or Windows.

### 3.3 Processes and process keys

- **Long-lived processes** (commands, hook handlers, providers) are keyed by **(plugin, project, authority key)**. The **authority key** is the caller's principal class plus a hash of its **effective rights** plus a **policy generation**. The class is `person` (the `console`, `local` and `device` principals), `agent:<root session id>` (one agent session's tree, agent §5.1), `system` (work the host starts itself, such as library stages), or `<kind>:<id>` for any other principal (an MCP client, say). The rights hash covers what a call can do through the host: scopes, branch root or session, and libraries. The policy generation increments when a credential is narrowed or revoked or when the agent session tree changes (a session starts, ends or is re-parented); **a generation change retires that key's processes and discards its temp state** (§5.1). A call is routed only to the process of its own key, so state a plugin keeps in memory, in its temp directory or in `pluginData` for one authority is unreachable from a caller with another. A key's process starts lazily at its first activation and stops after 5 minutes idle; the number of live keys is bounded by live sessions (agent §5.5) and open projects. Calls without a project (registry-wide provider estimates) use the key `(plugin, none, authority key)`.
  - **Agent trees.** A caller whose effective rights are narrower than its root session's (a child session, for instance, holds its mode's scopes within the parent's, agent §5.1) never runs in the root's warm process: it runs the plugin **cold** (one process per call under `fin-jobs`) or under **its own authority key**, so nothing the root's process learned with wider rights is reachable by a narrower caller.
  - **Why `person` and `system` can share a key.** Every `person` principal holds identical plugin-relevant rights today (all four mappable scopes, every project and branch), and so does `system` work, so one key per class is an authority key with a constant rights hash. **The sharing holds only while that is true:** if members of a class stop holding identical plugin-relevant rights (a narrower device credential, say), the class moves to rights-hash keys like the agent classes, and the shared per-project `pluginData` directory (§5.1) closes with it.
  - **Decision (2026-11-09, owner):** process memory, the per-key temp directory and persistent plugin state are keyed by effective authority (the hybrid: a shared per-project directory for the two uniform classes, authority-keyed directories for the rest), not by principal class alone. §5.1, §12, Decision 28.
- **Jobs** (analyzers, codec backends, matte producers, generation calls) run **one process per job** under `fin-jobs` (cp §6.2): the plugin process is the job's worker through the `fin-jobs` adapter, with the job's resources, liveness, hard kill of the process tree, retries and crash bisection.
- **Environment:** `childEnv()` (arch §27.7) plus only `FIN_PLUGIN_PROTOCOL` and a private `TMPDIR`/`TEMP` in the process's own directory. No `FIN_*` credential, no provider variable, no `FIN_TRAINING`.
- **Limits** (applied by `fin-sandbox`): memory (default 2 GiB; analyzers up to the manifest's declared need ≤ 16 GiB), process count (8), open files (1,024), CPU priority from the job's priority class, a 16 MiB cap per protocol message.
- **Crashes:** in-flight calls fail with `PLUGIN_CRASHED` (retryable for jobs). Three crashes of one plugin within 10 minutes disable it (`PLUGIN_CRASH_LOOP`, audited) until the owner re-enables it.
- **Timeouts:** `initialize` 10 s; a command 10 s by default, at most 60 s by manifest; hooks as agent §9.1 orders them; jobs by `fin-jobs` liveness (`alive` every 5 s, killed after 30 s of silence).

**As built (8B).** One `plugin` family serves every plugin (`program(job)` resolves `job.via.plugin` to its store directory and runtime). The process key is `<plugin>@<version>|<project|none>|<authorityKey>`: the version is in the key, so an update never reuses an old process; the authority key is `<class>.<rightsHash>.<generation>` (class `person`, `agent:<root session id>` or `system`, written with `-` for `:`; `rightsHash` 32 hex over the caller's scopes, projects, branch, libraries and grants; `generation` the class's policy generation, persisted in `control.db` `plugin_authority`). Generations bump on every credential mint and end (an agent's bump is its tree root's) and on a device revoked; a bump retires the class's older warm processes, waits up to 65 s for them and the class's running jobs, then removes their temp and authority-data directories. Up to 16 warm slots for a `plugin`-profile family (other families keep 1-2) and `idleS` 300. Three `PLUGIN_CRASHED` job ends of one plugin (any key) within 10 minutes set `state: crashLoop`, `problem: PLUGIN_CRASH_LOOP`, stop its processes and audit `plugin.crashLoop`; only the owner's `plugin.enable` clears it. Crashes are counted from job ends, not process exits: a warm process that dies while idle fails no job and is not counted. **Deviation:** the per-message cap is 5C's 1 MiB, not 16 MiB. **Deviation:** a job queued before a generation bump runs under the new key. No tree re-hash happens before a cold start (Decision 16, §24.1).

### 3.4 The protocol

**As built (8B, plan Decision 2, which supersedes this section's earlier JSON-RPC 2.0 design after the sub-project 5 job system fixed one wire).** A plugin process speaks the `fin-jobs` envelope (5C-1 Decision 44) and is a worker of the `plugin` family. There is no separate protocol stream.

- **Framing:** one JSON object per `\n`-terminated line on stdin/stdout, at most 1 MiB; stderr is the plugin's log, scrubbed and size-capped by the host before it reaches the server log.
- **Types** live in `fin-plugin-api::protocol` (Rust, pure), exported to JSON Schema and generated into `@finisher/core-types`. Nothing on either side is hand-written.
- **Mapping of the earlier JSON-RPC design:** `initialize {hostApi, protocol, plugin, grants, dirs, locale, limits}` is the worker's `hello` (`v` 1, any `minor`, `kinds`, `warm`) and, per job, a **`PluginContext`** in `start.extra` (host API version, plugin id and version, granted permissions, `dirs {plugin, data?, tmp}`, locale, `limits`); `initialized` is `ready`; `shutdown`/`exit` are 5C's `shutdown` and stdin EOF. **Host to plugin** methods (`command.run`, later `hook.<event>`, `analyze.run`, `provider.*`, `codec.*`, `matte.run`) are **jobs of plugin kinds**: `plugin.command` v1 here; `plugin.hook` v1 with Task 17. **Plugin to host** methods are **host calls** (`call {id, method, params}`, answered by `reply`), routed to the `plugin` family's `onCall` (§3.6). The JSON-RPC types live on as the typed params and results of those kinds and calls.
- **Plugin kinds are never submitted by RPC** (Decision 6): `registerKind` refuses `rpc: true` for plugin families permanently; clients reach plugin work through typed RPCs that check rights and build the job (`plugin.command.run`; 8C's analysis RPCs; 8G's generation RPCs), and plugins reach their own kinds through a `job.submit` host call in 8C.
- **Versioning:** `hostApi` is a semver (**1.1.0** as built); a manifest's `hostApi` range must include the running host's (`PLUGIN_HOST_API`); additive host methods and newly consumable contribution kinds bump the minor (`commands` made 1.1; `hooks` will make 1.2). The envelope's own minor rule governs unknown message types: the SDKs drop and log an unknown type from a newer host, never fatal.
- **Errors** are `{code, message, details, fix?}` with the exits of §13.

### 3.5 Data exchange: `in/`, `out/` and rings (`fin-shm`, owned by sub-project 6)

**Two routes, by process lifetime.** *Warm* (long-lived) processes, which serve commands, hook handlers and provider calls, get **no per-call directories**: they exchange data only through calls and results (each ≤ 1 MiB, the `fin-jobs` line cap; a larger snapshot is paged, `project.snapshot {page}`), because a warm process outlives its call and a sandbox's read and write roots are fixed at its spawn (Node's `--allow-fs-*` flags cannot be extended afterwards; Landlock, Seatbelt and AppContainer grants can only narrow). Any kind with **file inputs or outputs** (`media:file`, 8C's analyzers and matte producers, 8G's generation providers streaming into `out/`) runs **cold**, one process per job under `fin-jobs`, in 5C's job directory `<server-data>/jobs/<jobId>/{in,out}/`; there is no `plugins/run/` tree. A warm kind that needs per-job files stays cold under a sandbox until 7C's per-job grants (command-style warm kinds that use calls only stay warm under the `plugin` profile, plan 8B Task 16).

- **Decision (2026-11-09, owner):** no per-call directories for warm plugins. **The documented route for a future warm kind that needs files** is plan 7C Decision 19's per-job exchange directory inside the worker's *own* granted folder (`<w.dir>/jobs/<jobId>/`, created before the hand-off, removed after), keyed per authority key (§3.3) so one authority's warm worker never has another's files in reach. **Descriptor passing is rejected**: 5C passes only stdio to a worker, and neither Node nor Windows offers a portable equivalent.

Each cold job has a directory `<server-data>/jobs/<jobId>/` with two halves:
- **`in/`**: written by the host before the call, **read-only to the plugin** (a sandbox read root, never a write root). Snapshots over 1 MiB, PCM, frames, artifacts the call may read.
- **`out/`**: the plugin's only write root for the call. Outputs, output rings, temp files.

Rules, all implemented once in **`fin-shm`**, which `fin-worker` and the plugin host both use:
1. **The host never maps a file the plugin can write.** Output files are opened with `O_NOFOLLOW` (`FILE_FLAG_OPEN_REPARSE_POINT` on Windows, then a reparse-tag check); `fstat`/`GetFileInformationByHandle` on the handle must show a **regular file** with **`nlink == 1`** whose **size equals the size the plugin declared** and is **≤ the call's cap**. The host then **copies** it with positional reads into host-private memory or `<server-data>/plugins/staging/` and validates **the copy**. A short read (a file truncated during the copy) is `PLUGIN_OUTPUT_INVALID`, never a `SIGBUS`.
2. **Job outputs are validated after the plugin process has exited**, so nothing can change while the host checks them. A warm process returns data only inline in its results (≤ 1 MiB, validated like any message), which makes its liveness irrelevant; if a warm kind ever writes files through 7C Decision 19's route, they are copied first and validated as copies (rule 1).
3. **Rings are fixed-size and host-created.** An input ring (`in/<name>.ring`: header, N slots of a fixed size) is created and sized by the host, written only by the host, and mapped read-only by the plugin. An output ring (`out/<name>.ring`) is also created and sized by the host before the call and then falls under the plugin's write grant; the host reads its slots with positional reads only (rule 1), never through a mapping.
4. **Slot ownership moves by control messages:** `slot {ring, index, seq, len}` from the writer, `release {ring, index, seq}` from the reader. A slot is read only after its `slot` message and reused only after its `release`; a message naming a wrong `seq`, an out-of-range index or a length over the slot size is `PLUGIN_PROTOCOL`.
5. The host deletes a job's directory after its result is stored (5C's lifecycle); a startup sweep removes leftovers.
6. **The TS SDK reads files** with positional reads (`fs.readSync`) and never maps them; Node plugins get whole input files, not streaming rings, in v1.

**As built (8B).** There is no `plugins/run/<key>/<callId>/` tree and no per-call directory for a warm process (Decisions 5 and 29): commands and hooks exchange data only through host calls and results (each at most 1 MiB as a line; larger data is paged). `in/`, `out/` and rings belong to one-process-per-job kinds (8C onward), which run cold under 5C's `<server-data>/jobs/<jobId>/{in,out}/`; the SDKs already carry their readers and exact-size writers behind the `files` feature, and the host's copy of an output is the validated one. `PluginContext.dirs` is `{plugin, data?, tmp}`: `data` is the caller's own directory, present only with the `fs pluginData` permission.

### 3.6 The capability-scoped host API (host API 1.0) and reach

**Every host call is checked against the call's context**, not only against the plugin's grants:
- **Project reach:** a call carries its project; any project, asset or artifact outside the invoking principal's `projects` reach is `PROJECT_NOT_FOUND` (cp §4.4: ids never leak).
- **Branch reach:** `project.snapshot` and `project.view` read only the call's branch; for an `agent` principal that branch must be inside the session's branch subtree (agent §5.1), else `BRANCH_NOT_ALLOWED`. A returned transaction targets the call's branch only.
- **Library reach:** `media.*` and `analysis.find` by fingerprint resolve only through assets of projects in reach or libraries the invoker may read (`library.read`, cp §7.6); a fingerprint that is not in reach is `ARTIFACT_NOT_FOUND`.

| Method (plugin → host) | Permission | Notes |
|---|---|---|
| `project.snapshot {rev?}` | `project:read` | the call's project and branch at its pinned `rev` (arch §6.3), inline or as an `in/` file |
| `project.view {budget?, range?}` | `project:read` | `fin view` text |
| `analysis.get {ref}`, `analysis.find {fingerprint, kind}` | `project:read` or `media:read` | reach-checked |
| `media.pcm {asset \| fingerprint, range?, rate, channels}` | `media:read` | an `in/` file or ring from `fin-worker`'s audio cache (6C); never a source path |
| `media.frames {asset \| fingerprint, times \| stride, maxSide, format: rgb8\|gray8}` | `media:read` | an `in/` ring from a `fin-worker` decode role |
| `media.file {asset}` | `media:file` | a per-job read grant on that one source file through the ACL broker (codec plugins only) |
| `net.fetch {request, grant?}` | `net` (origins) | brokered HTTPS (§9.3) |
| `job.submit {kind, params}` | `jobs:submit` | only the plugin's own job kinds; the job is **owned by the invoker** with **invoker ∩ grants** (§3.7) |
| `job.progress`, `job.checkpoint`, `job.item`, `job.alive` | (in a job) | `fin-jobs` worker messages through the adapter |
| `log {level, msg}` | — | rate-limited, scrubbed |

| Method (host → plugin) | Contribution |
|---|---|
| `command.run {command, args, project, branch, rev}` → `{tx?: {ops, baseRev}, data?}` | `commands` |
| `hook.<event> {payload}` → `allow \| block {reason} \| context {text} \| modify {payload}` | `hooks`: receiving any event needs `hooks:observe`; `modify` needs `hooks:modify` granted per hook (agent §9.1) |
| `analyze.run {analyzer, input, params, models}` (in a job) | `analyzers` |
| `provider.estimate \| submit \| poll \| cancel \| fetch \| find` | `providers` (§9) |
| `codec.*`, `demux.*` | `codecBackends` (§10.1) |
| `matte.run` (in a job) | `matteProducers` (§10.3) |

**As built (8B).** Host API **1.1.0** (`commands` became consumable; `hooks` is typed but refused `PLUGIN_CONTRIBUTION_UNSUPPORTED` by presence, so the API stays 1.1 until a follow-up after 7F). Built calls: `project.snapshot`, `project.view`, `log`, `net.fetch`, `net.read`. A call goes through, in order: method name (a later batch's method is `INVALID_REQUEST {method, availableFrom}`: `analysis.*`, `media.pcm|frames`, `job.submit` 8C, `media.file` 8H, `secret.grant` 8G; any other name `{reason: unknownMethod}`), decoding by the `fin-plugin-api` types (unknown fields refused, `PLUGIN_PROTOCOL`), the authority key of its own job (`PLUGIN_AUTHORITY_RETIRED`, exit 5, `retryable`), `narrowRights` against the live invoker (`CREDENTIAL_REVOKED`), then permission and reach (`PLUGIN_PERMISSION_DENIED {perm, missing: grant | invoker}`, `PROJECT_NOT_FOUND`, `BRANCH_NOT_ALLOWED`). **Page size (normative): 768,000 bytes before base64**, for snapshot pages, `project.view` (cut at whole lines) and `net.read` (`CallLimits.pageBytes`); a page's base64 and the envelope fit the 1 MiB line. The Rust SDK's `PAGE_BYTES` (756 KiB, 774,144 bytes) is only an upper clamp on the host's value (`min(pageBytes, PAGE_BYTES)`); the doc comments in `fin-plugin-api::protocol::calls` that say "756 KiB" are stale and the host's number wins. **Snapshot:** a command's snapshot is read at submit and held by the host (`CommandRun.rev` = every page's `rev` = `Tx.baseRev`), under a per-run handle in `CommandRun.snapshot` (opaque to plugins); other jobs pin at their first read. `log`: 20 a second per process, 4 KiB, scrubbed (paths become `<path>`, control and bidi characters become spaces), written to the server log under a keyed plugin id; stderr is scrubbed, capped at 64 KiB a minute and never journaled. A call from a process whose key the host does not know fails closed with `PLUGIN_AUTHORITY_RETIRED`.

### 3.7 The `plugin` principal

- **Kind `plugin`, id `<@publisher/name>@<version>`** (cp §3.2). A plugin process holds no credential and cannot call `finisherd`'s RPC; the principal authorises what the host does on the plugin's behalf and labels it in the audit log and the journal.
- **Effective rights = invoker ∩ grants**, for everything: a command run by an agent can write only what that agent can write, on that agent's branch subtree; a hook handler sees only events the hook's subscriber may see.
- **Jobs a plugin starts** (`job.submit`, or a command that queues analysis) carry the invoker's principal intersected with the plugin's grants, are **owned by the invoker** (cp §6.1 `owner`; agent §13.1 counts them against the invoker's render-minute budget), and run under the key `(plugin, project, the invoker's class)`. Jobs the host starts itself (library stages) are owned by `system` with the plugin's grants and library reach only.
- **Mapping** of permissions to sub-project 5 scopes: `project:read` → `project.read`; `project:write` → `project.write` restricted to `tx.commit` of the returned transaction; `media:read` → `library.read` and asset reads; `jobs:submit` → `jobs.submit` for the plugin's own kinds. No plugin ever holds a human-only scope, `branch.protected`, `review.decide`, `project.admin`, `auth.admin` or `diagnostics.*`.
- **New scopes** (added to cp §4.4 in 8A): `plugin.read` (`plugin.list|info|search|plan|discard`, `plugin.content.get`, `registry.list|status|refresh`), `plugin.install` (install, update, remove; **owner**), `plugin.admin` (enable, disable, registries; **owner**), `plugin.suggest` (any principal with `project.read`).
  - **As built.** `plugin.install` and `plugin.admin` are human-only scopes: only a person holds them, never `local`, `agent` or `mcp`, and each of their RPCs carries the console's confirmation `challenge`. `plugin.plan {allowDowngrade}` escalates to `plugin.install`, so only a person plans a downgrade. A refresh needs only `plugin.read`, and a refresh by a non-human principal installs nothing (§5.3). **Registry folder paths go to `human` principals only** (§23.3): `registry.list|status|add|refresh` omit `path` for every other caller, `local` included, and errors and problems for other callers lose paths too.

**As built (8B).** `JobRights {principal, class, scopes, projects, branch?, libraries, grants}` (generated) is computed once at submit as invoker ∩ grants (`project:read` needs `project.read`; `project:write` needs `project.write` and covers only `tx.commit` of the returned transaction; `media:read` gives `library.read`; `jobs:submit` is for the plugin's own kinds) and narrowed against the live invoker on every call. An `agent` principal reaches only its session's subtree (7A-2's `BranchReach.inSubtree`) and fails closed without a session root. No human-only scope is ever granted to a plugin (property test). A plugin family's job must carry `via` and valid rights whose principal is its owner.

### 3.8 Commands, names and `Tx.via`

1. **Names:** a plugin command's registry name is **`plugin:<id>/<command>`** (for example `plugin:@acme/tools/trim-silence`). That is the name sub-project 7's command rules match (agent §7.2: `deny plugin:@acme/*`, `ask plugin:*/publish`), the name `commandOf` returns for the RPC, and the name in `fin schema --registry`. `fin` spells it `fin @acme/tools trim-silence …`; `fin help @acme/tools` lists a plugin's commands from `plugin.commands` (cached per server, keyed by a registry digest), and the MCP adapter exposes them under the same names.
2. The host validates `args` against the command's JSON Schema, pins `rev` = the head of the call's branch and calls `command.run` on the process of the call's key.
3. A returned `tx` is tagged `baseRev = rev`, gets `via: {plugin: "<id>@<version>", command}` and is committed through `tx.commit` under the invoker's principal and author. fin-core validates and rebases or rejects (`CONFLICT`). `--dry-run` works as for every write.
4. **`Tx.via`** (new optional field, 8B): stored with the record; ignored by apply and replay. It needs no upcaster (absent = none). **The server strips any client-sent `via`**: only the plugin host sets it, so no client can attribute its edits to a plugin. Rebase and cherry-pick carry it into the new record (a round-trip test covers commit → rebase → pick → export → import). Review change lists (cp §8) and the dataset builder read it.

**As built (8B).** Item 1: `plugin.commands {}` returns `{digest, commands: [{name, plugin, command, title, args, writes, timeoutS}]}`, filtered to what the caller may run; `source: dev` plugins are never listed to or run by `agent` or `mcp` principals. **Deviation: `plugin.commands` needs `project.read`, not `plugin.read`** (agent sessions hold no `plugin.read`). `plugin.command.run {projectId, branch?, name, args?, dryRun?}` needs `project.read` plus `project.write` when the command writes (`PLUGIN_PERMISSION_DENIED` if the plugin lacks the `project:write` grant). Item 2: the order is name (`PLUGIN_NOT_FOUND`), arguments (`PLUGIN_COMMAND_ARGS {problems}`), grant, all before any job; the snapshot is pinned at submit; the job is `plugin.command` v1 (warm, interactive, `maxAttempts` 1: a command is never retried), cancelled at the command's timeout (default 10 s, at most 60 s, counted from submit; `PLUGIN_TIMEOUT`) or when the caller goes. Item 3: the result (`{tx}` or `{data}`) is decoded and checked by fin-core against the held project (`PLUGIN_PROTOCOL` for an undecodable one, `PLUGIN_TX_INVALID {problems}` for refused ops, `reason: readOnlyCommand` for a `writes: false` command returning a transaction); the commit goes through the chain's own `tx.commit` on the invoker's connection, so the live credential, scopes, branch limits, write guard and author stamping apply and 7B's rules see the host commit (`PolicyInput.host`). A conflict is `outcome: conflict` (`CONFLICT`, exit 4, with `rev` and `ops`); `accepted` means the commit is `baseRev + 1`, else `rebased`; `--dry-run` returns `{outcome: data, dryRun: true, rev, ops}` and commits nothing. Item 4: `Tx.via` is `Option<Box<Via>>` with `skip_serializing_if`, no upcaster; rebase, pick, merge and the store's pick carry it with no code change; the server drops any client `via` (even `null`) in `tx.commit`. The journal's `IntentTx` leaves it out (a third-party id would need a keyed hash).
5. **`fin @<id> <command>`** (`crates/fin/src/commands/plugin_cmd.rs`): flags map onto the argument schema (`--name value`, `--flag`, repeated flags for arrays, `--args-json` for objects); command-line problems are `USAGE` before any RPC. `fin help @<id>` shows usage lines from `plugin.commands`, cached per server under the client's cache directory by digest and used when the server is unreachable. **§3.8.1: `fin schema --registry` lists the dynamic pattern `plugin:<id>/<command>` only**, not each installed command; the installed names come from `plugin.commands` and `fin plugin commands [<id>]`. The entry's identity (`projectId, branch, name, args`) also stands for its `tx.commit`. A run is never retried (`--idem-key` and `--timeline` are refused); a writing run whose answer was lost says it may have run (`mayHaveRun`).

### 3.9 fin-core changes, all together

| Change | Batch | Shape and compatibility |
|---|---|---|
| `Tx.via: Option<Via>` | 8B | optional, absent by default; no upcaster; server-stripped from clients; carried by rebase and pick |
| `fin-core::analysis` artifact types, `ArtifactRef` | 8C | artifacts are not project state; `CaptionSource.artifact` keeps its `String` type with a documented grammar `ar:<kind>:<hex16>`, so the serialised model does not change |
| `compose` builders (§8.8) | 8C–8F | pure functions; no model change |
| `plugin.pin`, `plugin.unpin`, `plugin.repin` | 8H | **three ops, one module each**, modelled on `font.pin`/`font.unpin`/`font.repin`, with inverses and footprints |
| `Project.plugins: BTreeMap<PluginId, PluginPin {version, sha256, effects: [{kind, params: [ParamSpec]}]}>` (`kind` is the **qualified** kind, §10.4) | 8H | the pack's ParamSpecs are **stored in project state at pin time**; lookup is built-ins ∪ pinned, so the same op log validates identically on every machine whether or not the pack is installed |
| `ParamSpec.path`, `Scope::Effect.kind` as `Cow<'static, str>` | 8H | built-ins stay `Borrowed`; pinned specs are `Owned`; serialisation unchanged |
| `AssetKind::Matte`, `Mask.matte` naming an asset | 8H / 6D (owner decision 3) | a new enum variant changes the serialised shape |
| **`SCHEMA_VERSION` 4 → 5** (written "2 → 3" before the keyframe-id bump to 3 and 6B-1's video fields at 4) | 8H (or 6D if it lands first) | one bump carries `Project.plugins` and `AssetKind::Matte` together: an upgrade step (`snapshot_v4`, `record_v4`, frozen, with goldens) adds the empty `plugins` map; newer projects are refused by older code per core §2.6 |

---

## 4. The manifest

### 4.1 Schema (`finisher-plugin.json`, `fin-plugin-api::manifest`, generated types)

```jsonc
{
  "manifestVersion": 1,
  "id": "@publisher/name",              // [a-z0-9][a-z0-9-]{1,38} / [a-z0-9][a-z0-9-]{1,63}
  "version": "1.4.2",                    // semver 2.0 without build metadata, immutable once published
  "kind": "content" | "code",
  "hostApi": "^1.0",
  "license": "MIT",                      // SPDX expression
  "publisher": { "name": "…", "url": "…" },
  "messages": "i18n", "locales": ["en"],  // ICU catalogues (arch §17.1)
  "platforms": ["darwin-arm64", "win32-x64", "linux-x64", "linux-arm64"],   // code only
  "runtime": { "type": "native", "exec": { "darwin-arm64": "bin/darwin-arm64/plugin", … } }
           | { "type": "node", "main": "dist/plugin.mjs", "node": ">=24.22" },
  "permissions": [
    { "perm": "project:read" }, { "perm": "project:write" }, { "perm": "media:read" }, { "perm": "media:file" },
    { "perm": "jobs:submit" }, { "perm": "gpu" },
    { "perm": "net", "origins": ["https://api.example.org"] },
    { "perm": "secrets", "names": ["example"], "mode": "brokered" | "raw" },
    { "perm": "fs", "scope": "pluginData", "access": "read" | "readWrite" },
    { "perm": "hooks:observe", "hooks": ["PostCommand"] },
    { "perm": "hooks:modify", "hooks": ["PreCommand"] }
  ],
  "contributes": { … §4.2 … },
  "models": [{ "id": "…", "url": "…", "sha256": "…", "bytes": 0, "license": "Apache-2.0",
               "attribution": "…", "provenance": "…", "purpose": "msg.key" }]
}
```

- **Validation is Rust** (`fin-plugin-api::validate`, through `fin-node`): grammar, required fields per kind, every referenced file present, every contribution kind consumable by this host (§4.2), every ParamSpec well formed, every message key in the `en` catalogue, permissions closed and minimal (a content pack with permissions is invalid), `platforms` matching `runtime.exec`, `deny_unknown_fields` everywhere (a typo never drops a permission).
- **Reserved publishers:** `@finisher`, `@fin`, `@official`, `@system`, `@local`; `@dev/*` is the developer-mode scope (§5.5). `fs` with `scope: workspace` is not in v1.

**As built (8A, `fin-plugin-api`, pure, no crate of ours, built for `wasm32-unknown-unknown` in `pnpm gates`):**
- The manifest is the typed `Manifest` with `deny_unknown_fields` everywhere; `Permission` is parsed by hand because serde's tagged unit variants accept extra fields. The file is `finisher-plugin.json` at the package root, UTF-8 JSON (the `jsonc` above is illustration), at most 256 KiB (`MANIFEST_TOO_LARGE`, exit 2). `Platform` is the six shipped OS/architecture pairs, `Runtime` is `native` (an executable per platform) or `node` (a `.mjs` main and a range with a minimum). Ids are `@publisher/name` with the grammar above, no `--`, no leading or trailing `-`, and Windows device names (`con`, `nul`, `com1`…) refused; semver has no build metadata.
- `validate(bytes, &dyn PackageFiles, &ValidateCtx)` returns the `Manifest` or **every** problem at once (`Invalid {code, message, details}`: `MANIFEST_TOO_LARGE`, `PLUGIN_MANIFEST_INVALID {path, reason}`, `PLUGIN_CONTRIBUTION_UNSUPPORTED {kind, hostApi}`, `PLUGIN_HOST_API`, `PLUGIN_PLATFORM`, `PLUGIN_NODE_VERSION`) in the families `kind`, `permissions`, `files`, `messages`, `content` and licences. The source class (`bundled`, `registry`, `dev`) is a parameter: reserved publishers are refused outside bundled sources and `@dev` outside developer mode. Content checks are injected (`ContentChecks`): the host passes `fin_core::captions::parse_style_file` for caption styles and `FontCatalog::from_json` plus the catalog merge for font catalogs, so a pack the host cannot consume is refused at validation, not at use. `fin-plugin validate` runs the same checks offline.
- **Rules beyond the list above:** a content pack must contribute something; `hooks:modify` hooks must also be observed; every referenced file and every message key must exist.
- The manifest, plan, registry status and event payload types derive `JsonSchema`; their TypeScript types are generated (`export_schema`, then `pnpm --filter @finisher/core-types generate`) and the drift gate covers them.

### 4.2 Contributions, by consumer

A contribution kind is accepted only once a batch consumes it; until then a manifest naming it is `PLUGIN_CONTRIBUTION_UNSUPPORTED {kind, hostApi}`.

| Key | Consumer | Risk | Batch |
|---|---|---|---|
| `fontCatalogs` | fin-fonts catalog merge (4C Decision 30) | low | 8A |
| `captionStyles` | `style.add --from @pub/pack:<name>` (copied into the project) | low | 8A |
| `commands`, `hooks` | §3.8, agent §9 | per permissions | 8B (hooks with 7F) |
| `skills`, `prompts`, `roles`, `rubrics` | `fin guide`, saved prompts, roles as data (agent §6.2, §6.4), critic rubrics (10) | **medium**: they steer agents; never auto-updated (§5.3) | when 7B, 7G or 10 consume them |
| `analyzers` | §8.1 | per permissions | 8C |
| `providers` | §9 | per permissions | 8G |
| `codecBackends`, `matteProducers`, `audioProcessors` | §10 | per permissions | 8H |
| `luts`, `effects`, `transitions` | §10.4 | low | 8H |
| `lottieTemplates`, `formatTemplates`, `exportPresets`, `qaProfiles`, `duckingRules`, `editPresets` | sub-projects 6, 10, 11 | low | when consumed |
| `ui`, `agentProviders` | sub-project 9; after v1 | — | — |
| `slots`, `importers`, `embedders` | — | — | cut from v1 |

**As built (8A).** `fin_plugin_api::consumable(hostApi)` is the table: for host API 1.x it lists `fontCatalogs` and `captionStyles` and nothing else; any other key in `contributes` is kept by name and refused as `PLUGIN_CONTRIBUTION_UNSUPPORTED`, so no unconsumed kind is half-validated. Code plugins with an empty `contributes` install (8A cannot run them; the lifecycle is tested with them).
- **`fontCatalogs`** merge into the server's catalog (`FontCatalog::merged`): entries are appended in order, an entry id the catalog or an earlier pack has is `catalogIdTaken`, a pack's `fallback` ids go after the built-in order (**Deviation:** §4.2 said only "catalog merge"), `catalogVersion` stays the base's so pins keep naming the version they were planned at, and the merged catalog is validated again. A pack **cannot shadow what exists**: a file or face already listed, or a face family equal to one the catalog or an earlier pack has (case-folded), is refused (§23.2 H1). The plan makes the same merge against the built-in catalog and every installed content pack, disabled and revoked ones included (stricter than the sync at start, which leaves a pack that does not merge out and reports `fontCatalogRejected`), and the commit re-checks it (`PLAN_MISMATCH {reason: installed, field: catalogIds}`).
- **`captionStyles`** are served by `plugin.content.get {id, kind: captionStyle, name}` and copied into a project by `fin style add --from @pub/pack:<name>` through the existing `style.add` op, with every style path the parameter registry knows. A style file is a `CaptionStyle` without `id` and passes `parse_style_file` (every key survives the round trip, every style path validates as `style.add` would). `plugin.content.get` serves nothing for a plugin that is not installed, disabled, revoked or bundled-and-mismatched, and re-validates on every read.
- Agent-facing kinds (`skills`, `prompts`, `roles`, `rubrics`) have no consumer yet. The rule that they never auto-update is implemented (`InstallPlan.agentFacing`) and tested with a test-only kind flag.

**As built (8B).** `commands` is consumable from host API 1.1 (`consumable(1.1)` = `fontCatalogs`, `captionStyles`, `commands`). A command is `{name [a-z0-9][a-z0-9-]{0,63}, title (an `en` message key), args (the schema subset, §3.4 and §18.1), writes, timeoutS (1-60)}`; `validate` refuses a schema outside the subset (`argsSchema {at, problem}`), a duplicate name, a timeout outside 1-60, and a content pack that contributes commands. `hooks` is typed and refused by presence (`PLUGIN_CONTRIBUTION_UNSUPPORTED`) until Task 17.

### 4.3 Permission risks shown to people

| Risk | Permissions |
|---|---|
| low | `project:read`, `media:read`, `fs pluginData read` |
| medium | `project:write`, `jobs:submit`, `fs pluginData readWrite`, `gpu` (kernel GPU drivers become reachable), `hooks:observe` alone, agent-facing content (§4.2) |
| high | `net`, `secrets`, `media:file`, `hooks:modify`, **`hooks:observe` together with `net`** (hook payloads carry prompts, commands and ops, and `net` can send them away), and **any third-party code plugin**, whatever its permissions (§3.1) |

**Widening** is any added permission, an added origin, secret or hook, `brokered` → `raw`, `read` → `readWrite`, a risk increase from a combination (adding `net` to a plugin that has `hooks:observe`), or any change to agent-facing content.

**As built (8A).** `risk::{risk_of, risks, overall}` compute risk over the whole permission set, so combinations count; agent-facing content has a floor of medium. The third-party-code rule needs the source, so it is applied by the plan (`fin-registry::plan`), not by `fin-plugin-api`: a `kind: code` package from a registry (not bundled, not `@dev`) is high risk whatever its permissions and carries the warning `thirdPartyCode {confined: false}` (no `plugin` profile exists yet). `PermissionDiff {added, widened: [{from, to}], removed, riskUp}` (`widened` is objects, not tuples, which generate `unknown` in TypeScript) with `widens()`, and `permissions_widen()`, which also counts a change of an `fs` scope; `requires_reapproval` and `reapproval_reasons` (permissions, agent-facing content by hash, kind, runtime type) are ready for 8B. Licence classes are `permissive` (every required branch allowed by `deny.toml`; `OFL-1.1` counts, for font packs), `weakCopyleft`, `copyleft`, `nonCommercial` (refused) and `unknown` (refused for bundled plugins, a warning elsewhere), ordered permissive < weakCopyleft < unknown < copyleft < nonCommercial; `OR` takes its best branch and `AND` its worst. Both copyleft classes need `accept.copyleft`.

---

## 5. Lifecycle

### 5.1 Where things live

```
<server-data>/plugins/
  registries/<registryId>/registry.json      {id, kind: local, path, trust: {rootSha256, freshness}}
  trust/<rootSha256>/                        trusted TUF metadata per root (kept when a registry is removed, §6.3)
  store/@<publisher>/<name>/<version>-<sha12>/   unpacked package, read-only after install
  staging/<planId>/                          verified, unpacked, not yet approved (10-minute life)
  data/@<publisher>/<name>/<projectId|_>/    the plugin's writable data (fs pluginData), per project: ONLY for the `person` and `system` classes, and only while every member of a class holds identical plugin-relevant rights (§3.3)
  authdata/@<publisher>/<name>/<projectId|_>/<authorityKey>/   pluginData for every other caller (agent sessions, MCP clients): one directory per authority key, deleted when the session ends or the key's generation changes (§3.3)
  tmp/<keyHash>/                             the process key's private temp, per authority key; discarded on a generation change
  (no per-call tree: cold jobs use `<server-data>/jobs/<jobId>/{in,out}/`, 5C, §3.5)
<server-data>/models/<sha256>/               model files with model.json (§8.2)
control.db: registries, plugins, plugin_grants, plugin_plans (cp §3.1)
```

- `plugins {id, version, source: bundled|registry|dev, registryId?, targetSha256, kind, enabled, installedBy, installedAt, autoUpdate, state: active|disabled|revoked|crashLoop}`; `plugin_grants {pluginId, version, permission JSON, grantedBy, grantedAt}`.
- The store is local disk only (arch §6.7). Packages are never written after install; updates install beside and switch.
- **Plugin data is per project** (and `_` for calls without one), so state a plugin persists for one project is not readable from another project's process key.

**As built (8A).**
- `control.db` holds `registries`, `plugins`, `plugin_grants`, `plugin_plans` (migration chain `plugins`, 0001) and `plugin_suggestions` (0002, a stand-in until 5E's review items land). **Deviation:** every installed row carries the trust root it was verified under (`plugins.rootSha256`, 0003), and a plan row carries `base` (the canonical installed `{version, grants}` it was made against), the maker (`byKind`, `byId`, `human`) and `rootSha256`.
- `store/@pub/name/<version>-<treeHash12>/` is unpacked, files read-only (0444/0555; the read-only attribute on Windows). The store keeps no index: installed state is `control.db`'s. An installed directory is always whole: a commit re-hashes a directory already in place and replaces it when it differs, a remove renames before deleting, and `Store::sweep_store` deletes `.tmp-*` leftovers. At start `plugins/store-sync.ts` removes store folders no row names and disables rows whose folder is missing (reported in `plugin.list` `problems`). The `run/`, `data/` and `models/` trees are 8B's.
- `staging/<planId>/{plan.json, pkg/}` is built in `staging/.tmp-<planId>/` and renamed; plans expire after 10 minutes, are swept at start and every minute, and the sweep also removes fetched target copies. A principal may hold **4 open plans** (`TOO_MANY_PLANS`, exit 5); `plugin.discard {planId}` frees a slot (§23.2).
- Trusted TUF metadata is `trust/<rootSha256>/`, with its store layout in §6.3.

**As built (8B).** Under `<server-data>/plugins/`: `tmp/<keyHash>/` (private temp per process key, 0700, swept at start, removed when the key is retired), `data/@pub/name/<projectId|_>/` (shared by the `person` and `system` classes only while their rights are identical; checked on every use) and `authdata/@pub/name/<projectId|_>/<authorityKey>/` (everyone else, created on first use and only with `fs pluginData`, deleted at session end and at any generation change). There is no `plugins/run/` tree (§3.5). `control.db` gains `plugins.problem` (plugins migration 0004) and `plugin_authority` (0005). Models are placed under `<server-data>/plugins/models/` by 8C, not `<server-data>/models/`.

### 5.2 Sources of plugins

- **Bundled** (no TUF in v1): the app ships first-party plugins unpacked in its resources. `fin-node` carries a compiled-in list `BUNDLED_PLUGINS: [{id, version, treeHash}]` generated at build time; at start each bundled tree is re-hashed and installed or refreshed only if it matches. On signed releases the operating system's code signature covers the resources (macOS notarisation, Windows Authenticode), so the hash list detects corruption and the signature detects tampering. Bundled plugins can be disabled but not removed, and update only with the app.
- **Local-folder registries:** a directory holding a TUF repository (§6.1) on local disk, a NAS or a synced folder, read-only to Finisher. **A git registry is such a folder** that the user keeps current with `git pull` (or a sync tool); Finisher never runs git and trusts nothing about its history or commits. A pull that rolls the repository back is refused by TUF (§6.2).
- **Adding a registry** (`registry.add`, owner) needs its root: `--root <file>` or `--root-sha256 <hex>` checked against `1.root.json` (or the newest root it chains to); otherwise the console prints the root's hash and key ids and the owner must type the hash back. There is no silent trust on first use.
- **HTTPS registries** (the public registry after v1) use the same client with an HTTPS transport; in v1 an HTTP transport exists **only in test builds** (feature `test-http`), so the `tuf-conformance` suite can run against the client.

**As built (8A).**
- **Bundled plugins:** sources are `resources/plugins/bundled/<name>/` (unpacked; `resources/**` is `-text` so hashes match on every checkout). `crates/fin-node/build.rs` hashes each folder with `fin_registry::package::tree_hash` into `BUNDLED_PLUGINS: &[(id, version, treeHash)]` (the folder name is the id without `@finisher/`). `finisherd` runs `syncBundled()` at start before anything is served: a match with nothing installed installs without a plan; a match that differs from the installed row (newer, older or other bytes) is replaced through the same lock as an approval by `system:bundled` (a widening is applied: the app update was the approval; it is audited with `auto: true`); a mismatch is never installed, and an installed row is revoked with `PLUGIN_BUNDLED_MISMATCH`. A revoked bundled pack changes `state` only, so `enabled` stays the owner's switch and a restore puts back what the owner had. The root is `FIN_BUNDLED_PLUGINS_DIR` or `<repo>/resources/plugins/bundled`; packaged apps set it. 8A ships one pack, `@finisher/captions-basic` 1.0.0 (`bold`, `clean`, `punch`, each a built-in preset). **Deviation from the signature sentence above:** no signed release exists before sub-project 11, so on every build the compiled-in hashes are the only check; `plugin.list` reports `bundled.integrity: hashesOnly` and will report `signed` once sub-project 11 adds the code-signature check (§23.4).
- **Local-folder registries** are `registries/<id>/registry.json {id, kind: local, path, trust: {rootSha256, freshness: relaxed}}`. The pinned hash (or file) must be a root of the folder (`1.root.json` or a newer root of its chain), else `REGISTRY_UNTRUSTED` and nothing is written. Registry ids are `[a-z0-9][a-z0-9-]{0,63}` (`REGISTRY_INVALID`, `REGISTRY_EXISTS`, `REGISTRY_NOT_FOUND`). Registries are reconciled with fin-registry's folders at start. Removing the registry and re-adding it with the same root resumes from the trusted versions (§6.3).
- **Adding a registry without `--root`** (console): the console asks the owner to paste the **publisher's hash first**, describes the folder's root (its key ids and threshold) only after the pasted hash matches, and adds nothing on an empty or different answer. A `--root` file inside the registry folder draws a warning. `registry.trust {registryId, rootSha256}` is audited before `registry.add` in every case (file, `--root-sha256` or typed back).
- A **git registry** is a folder: help text and docs say "point `--dir` at your clone and `git pull` it"; nothing in 8A runs git. The e2e scenario uses a local clone it commits to and resets without a network, and a reset to an older commit is refused as `TUF_ROLLBACK` while installed plugins keep working.

### 5.3 Install, update, enable, remove

1. **Plan** (`plugin.plan {id, version?, registry?, update?}`, `plugin.read`): refresh the registry (§6.2), copy the target to `staging/`, verify length and hashes, unpack strictly (§6.4), validate the manifest and match it against the target's signed custom metadata, check `hostApi`, platform and Node version, compute the permission diff, collect warnings (yanked, advisories, look-alike ids, copyleft, staleness) → an **`InstallPlan`** with a `planId` and a `digest`. Nothing runs.
2. **Approve and install** (`plugin.install {planId, digest, accept: {copyleft?, thirdPartyCode?}}`, `plugin.install`, owner, from a plan a **human** made): re-hash the staging tree, rename it into `store/`, record the grants exactly as planned, audit, journal. A third-party code plugin's plan needs `accept.thirdPartyCode: true` (else `THIRD_PARTY_CODE_NOT_ACCEPTED`): the person has seen the `thirdPartyCode` high-risk warning, and whether the plugin will run confined on this OS, and approves each listed permission (the console prints them one per line and asks for the plugin's id typed back, not `yes`). Content packs become active; code plugins become enableable from 8B on, before 7C too (owner decision 1, §3.1).
3. **Update:** a plan with `update: true`. **Automatic updates** apply only to content packs with `autoUpdate` on, only when nothing widens, **never to agent-facing content** (skills, prompts, roles, rubrics: an update can change what agents are told) and **never as the result of a refresh a non-human principal triggered**: an agent's or `system`'s `registry.refresh` updates trusted metadata and applies `disable` advisories, but installs nothing. **Code plugins never update automatically** (arch §15 rule 6), bundled ones aside (they update with the app), and a third-party code plugin's update needs a person's plan and `accept.thirdPartyCode` again even when nothing widens. A widening update keeps the old version running until a person approves the new grants.
4. **Downgrade** only by a person naming the exact version (`--allow-downgrade`); TUF still refuses rolled-back metadata.
5. **Enable/disable** (owner): per server. Disabling stops the plugin's processes, unregisters its contributions and fails its queued jobs with `PLUGIN_DISABLED`.
6. **Remove** (owner): disable, delete the store directory, and with `--purge-data` its data. Bundled plugins cannot be removed (`PLUGIN_BUNDLED`).
7. **Suggestions:** an agent may call `plugin.suggest {id, reason}` (arch §8.3); it opens a `pluginSuggestion` review item (cp §8.1), shown as a suggestion, never as an approval card worded by the agent. Agents never install: `fin plugin install` is `HUMAN_ONLY` (agent §7.2's default `deny plugin.install` stays as a second layer).

**As built (8A).**
- **Plan** (`plugin.plan {id, version?, registry?, update?, allowDowngrade?}`, `plugin.read`): the registry is the named one, the one the plugin is installed from, or the only one that lists it, else `REGISTRY_AMBIGUOUS {id, registries}` (exit 2; `reason: rootChanged` when the row's registry id now has another root). The plan refreshes its registry first, applying advisories and never updating, fetches the newest version that is not yanked (or the named one) with its signed length as the cap, re-checks SHA-256, unpacks strictly, validates, requires the manifest id to equal the request's and the signed custom metadata to equal the package (`PLUGIN_MANIFEST_INVALID {reason: metadataMismatch, field}`), checks `hostApi`, platform and Node version, and compares the action with the installed row (`install|update|downgrade`; `PLUGIN_ALREADY_INSTALLED`, `PLUGIN_NOT_INSTALLED`, `PLUGIN_DOWNGRADE_REFUSED`). A downgrade needs a person and the exact `version`. Bundled plugins answer `PLUGIN_BUNDLED` (they update with the app). The `digest` is BLAKE3 of the canonical plan JSON with `digest` empty.
- **Warnings** (`thirdPartyCode {confined: false}` is always first): `yanked`, `advisory {id, severity, action, signedBy}`, `registryStale {days}`, `copyleft {spdx, class}`, `licenceUnknown`, `lookalikeId {similar}` (folded `0/o 1/l/i 5/s rn/m vv/w`, hyphens dropped, or one edit away) and `registryChanged` (a registry other than the row's, or one whose root changed; part of the digest).
- **Approve and install** (`plugin.install {planId, digest, accept, autoUpdate?}`) takes one lock over every change to installed plugins and refuses, in this order: a missing or expired plan (`PLAN_EXPIRED {reason: missing|expired}`), a plan made by a non-human (`HUMAN_ONLY {reason: planMaker}`, whoever submits it), a non-human approver (`HUMAN_ONLY {reason: approver}`), a digest or installed-state mismatch (`PLAN_MISMATCH {reason: digest|installed|registry|tree|plan}`), `COPYLEFT_NOT_ACCEPTED`, `THIRD_PARTY_CODE_NOT_ACCEPTED` (also for an update of third-party code that widens nothing). It re-hashes the staged tree, moves it into `store/`, then writes the row and grants as planned in one `control.db` transaction after re-checking the base; a failed transaction removes the placed version. It re-reads the trusted advisories under the lock, so a `disable` advisory that landed after the plan installs the version revoked. Content packs are active at install (a pack the owner disabled stays disabled); code plugins are `enabled: false, state: disabled`. Refusals are audited `denied`.
- **Deviation (M2 "one transaction"):** the Rust commit runs on `fin-node`'s worker thread, and a SQLite transaction held open across that await would pull unrelated writers on the connection (audit rows, credentials) into it and roll them back with it. The lifecycle lock isolates the steps instead, and the base is re-checked before the commit and again inside the row transaction.
- **Automatic updates** run in `afterRefresh` after a **person's** successful refresh, and only for a content pack with `autoUpdate` on whose plan is an update of content that is not agent-facing, widens nothing (`added`, `widened`, `riskUp`), carries no `thirdPartyCode`, `yanked` or `disable`-advisory warning and is permissive; otherwise the plan is discarded. Anyone's successful refresh revokes the versions a signed `disable` advisory names (`PLUGIN_REVOKED`, audited). A revoked version stays revoked if the advisory is later withdrawn: remove it and install again. `autoUpdate` is set by `plugin.install {autoUpdate}` (the console's `--auto-update`) and kept on updates when absent. An update removes the replaced version's directory after the switch.
- **Enable** on a code plugin is `PLUGIN_RUNTIME_UNAVAILABLE`, on a revoked version `PLUGIN_REVOKED`; **remove** refuses bundled plugins, tolerates a store directory already gone and, with `--purge-data`, purges data in that case too. Removing a pack whose fonts a project pinned keeps the font files (fonts are content-addressed and pinned by hash); the pack's catalog entries disappear for new pins.
- **Suggestions:** `plugin.suggest {id, reason, projectId}` (`plugin.suggest` and `project.read`) writes a `plugin_suggestions` row (**Deviation:** 5E's review items have not landed; a follow-up moves the rows into `pluginSuggestion` review items). One suggestion per (principal, project, plugin), 20 per principal and project (`TOO_MANY_SUGGESTIONS`).

**As built (8B).** `plugin.enable` (owner only, as in 8A) validates the code plugin on its store directory, refuses `models` manifests (Decision 18), clears `problem` (and a crash loop), and `PLUGIN_RUNTIME_UNAVAILABLE` remains only where no plugin host is running. Disable, revocation and removal cancel the plugin's running jobs, stop its warm processes (key prefix `<id>@`) and remove its keys' temp directories once the processes are gone. An approved update of an enabled code plugin keeps it enabled (a crash-looped one stays so), drains and stops the old version's key; an unapproved plan changes nothing, so the old version keeps serving. `requires_reapproval` (8A) decides whether an update needs a new approval. `plugin.list` rows gain `problem` and, for code plugins, `confined`.

### 5.4 The install plan (what the permission prompt shows)

`InstallPlan {planId, digest, plugin: {id, version, kind, displayName, publisher}, source: {kind, registryId?}, action: install|update|downgrade, from?, permissions: [{perm, risk, details}], diff: {added, widened, removed}, licence: {spdx, class}, bytes, models: [{id, bytes, licence, attribution}], warnings: [{code, details}], registryStaleDays?, expiresAt}`. A third-party code plugin's plan always carries the warning `{code: thirdPartyCode, details: {confined}}`, shown first and as high risk (§3.1). Sub-project 9 renders it; the console prints it and asks for `yes`. A plan made by a non-human principal is marked and refused by `plugin.install`, even when a person submits its id.

**As built (8A).** `InstallPlan {planId, digest, plugin {id, version, kind, displayName, publisher}, source {kind, registryId?}, action, from?, permissions [{permission, risk}], diff, risk, licence {spdx, class}, bytes, models, warnings, registryStaleDays?, agentFacing, treeHash, targetSha256?, expiresAt}`. **Deviations:** `permissions` entries are `{permission, risk}` (no `details`); `displayName` is the id's name until manifests carry one; `treeHash` and `targetSha256` bind the staged tree and the signed target. **A non-human principal's plan is returned to it, stored `unusable` and its staging deleted at once**, so an agent's plan costs no disk and can never be approved (§23.2 M2). The console prints, in this order: third-party-code and `registryChanged` warnings first and in capitals, the header, risk and licence, each permission on its own line with its risk, the diff of an update (`+ adds`, `~ widens`, `- drops`, risk up) and the other warnings, then asks for `yes`, or for the plugin's id typed back when a `thirdPartyCode` warning is present (which sets `accept.thirdPartyCode`). A `copyleft` warning needs `--accept-copyleft` and otherwise stops before asking. Every exit after a plan that is not installed (cancel, a refusal, a wrong typed id) calls `plugin.discard`. Everything the console prints that came from a registry, publisher or package goes through `safe()`, which strips C0/C1 controls and bidi overrides (§23.2 M1).

### 5.5 Developer mode

`finisherd --plugin-dir <dir>` (console only) loads one unpacked plugin as `@dev/<name>` without signatures, for that server run only, with grants approved at start on the console. Before 7C's `plugin` profile exists on the OS, a developer plugin runs unconfined like every code plugin (§3.1), with a console warning; it is never reachable from an agent principal. `fin plugin validate <dir>` validates offline.

**As built (8A) — Deviation:** the loader is **not built**: it runs code, so it comes with 8B's host. 8A only accepts the `@dev` scope in validation when the source class is `dev` (`ValidateCtx`). **Deviation:** there is no `fin plugin validate`. `fin` would need `fin-fonts` (ureq, rustls, zip, fin-text) to read a font catalog the host's way, so the offline validator is the publisher tool: **`fin-plugin validate <dir>`** (and so `pack` and `registry publish`), which checks caption styles with `parse_style_file` and font catalogs with `FontCatalog::from_json` plus the merge into the built-in catalog.

**As built (8B).** `finisherd --plugin-dir <dir>` (serve only): the console asks, on a terminal (`CONSOLE_NEEDS_TTY` otherwise, exit 7), after printing `DEVELOPER PLUGIN` and `PLUGINS_UNCONFINED` first, the publisher, licence, commands and every permission with its risk, and any existing `@dev` plugin data it would see; `Proceed? [y/N]`, or the id typed back when any permission is high-risk. The folder must be a code plugin whose id is `@dev/<name>` (a non-`@dev` id is refused, not renamed), without `models`, outside server data. It is installed as a `source: dev` row for that run only (every manifest permission granted by the console, audited), never re-hashed, deleted when the server's scope closes, and hidden from and refused to `agent` and `mcp` principals. **Residuals:** the `local` principal (the agent's shell before 7C) can run dev plugins; a program driving a pseudo-terminal passes the terminal check; a dev plugin's data directory persists across runs.

### 5.6 Publisher tooling (`fin-plugin`, a separate binary, not shipped in the app)

`fin-plugin new <kind>`, `validate`, `pack` (reproducible), `test` (§7.4), and `registry init|delegate|publish|yank|advise|rotate-key|resign-timestamp`, built on `fin-registry`'s editor (§6.6). Keys are ed25519 PEM files; `fin-plugin` never uploads anything. Private keys and the key-ring folder are owner-only from creation: 0600 and 0700 on Unix; on Windows a protected DACL (nothing inherited) granting only the current user and `SYSTEM` (the OS, which can read any file already; services that run as it without backup semantics would otherwise fail). A key folder or private key that anyone else can reach is refused, ssh-style, with `KEY_UNPROTECTED {path, problems}` and the `chmod` or `icacls` command that fixes it. On Unix that means any group or other mode bit; on Windows it means an owner other than the user, `SYSTEM` or Administrators, a NULL DACL (FAT, exFAT), or an allow ACE for any other SID.

**As built (8A, `crates/fin-plugin-cli`, binary `fin-plugin`, not in `default-members`, not shipped; hand-parsed arguments; nothing new in the dependency tree).**
- Commands: `new content|code <dir>` (`@example/<dir>` 0.1.0; code scaffolds use a `node` runtime so they validate on any OS), `validate <dir>` (every problem in `details.problems`), `pack <dir> -o <file>` (`{id, version, file, bytes, sha256, entries, treeHash}`), `keygen <private.pem> [--public <pub.pem>]` (prints `{keyId}` only, never overwrites: `KEY_EXISTS`), and `registry init|delegate|publish|yank|advise|rotate-key|resign-timestamp <dir> --keys <key-ring> …`, plus `registry trust reset-clock --plugins <dir> --id <id>` (human-only; clears only `lastRefreshAt`). **Deviation:** `test` (§7.4) is 8B's. JSON goes to stdout and `{code, message, details, fix?}` errors to stderr, with the exits of §13 (2 `USAGE`/`MANIFEST_TOO_LARGE`; 3 validation, `KEY_UNPROTECTED`; 4 `*_EXISTS`, `DIR_NOT_EMPTY`; 5 `IO_ERROR`, `REGISTRY_UNREACHABLE`, `CLOCK_SUSPECT`; 7 `TUF_*`, `REGISTRY_UNTRUSTED`).
- **Keys never sit in the published folder**: `--keys` is refused inside `<dir>` (`..`, symlinks, missing folders and case folding resolved; the editor refuses it too, `REGISTRY_INVALID {reason: keysInside}`). Key files are created 0600 and key-ring folders 0700 on Unix. On Windows both are created in the creating call with a protected DACL (the current user and `SYSTEM`; files without inheritance flags), so a data drive root's Authenticated Users or Users entries never reach a key, not even before a rename. **Loading is ssh-style on both OSes** (§23.3): `RepoBuilder::open`, every ring write and read, and `SigningKey::read` (ring keys, `--root-keys`, `--key`, `--new`) check the folder or file and refuse with `KEY_UNPROTECTED {path, problems}` (exit 3), whose `fix` is the `chmod 700`/`chmod 600` or `icacls` commands (`/setowner`, `/grant:r` user and `SYSTEM`, `/inheritance:r`, `/remove:g` for explicit grants) and a message to rotate keys others may have read. Windows accepts an owner of the user, `SYSTEM` or Administrators and allow ACEs for those plus `CREATOR OWNER`/`OWNER RIGHTS` (as OpenSSH for Windows does, so a ring under the profile passes); Administrators are not granted on creation. A first publish needs the targets key (help says so); `yank` takes the publisher's key (only that role can change its targets); `advise --severity` defaults to high and `--id` to `ADV-<12 hex>`.

---

## 6. Registries and TUF

### 6.1 Repository layout

```
metadata/
  <n>.root.json                  every root version (chain for rotation)
  timestamp.json
  <n>.snapshot.json              consistent snapshots
  <n>.targets.json               top-level targets: delegates per publisher
  <n>.<publisher>.json           delegated targets, one role per publisher
targets/
  <publisher>/<name>/<sha256>.<version>.finpkg     consistent target names
  <publisher>/advisories.json                       signed advisories (§6.5)
```

- **Roles** (arch §15): `root` (threshold ≥ 1 for team registries; for first-party ones 2 of 3 offline keys held by the owner, owner decision 7), `targets` (delegates only: one terminating delegation per publisher with paths `<publisher>/*`), `snapshot`, `timestamp`; publishers sign their own delegated role.
- **Target custom metadata** (signed): `{id, version, kind, hostApi, platforms, license, permissionsDigest, manifestSha256, yanked?, bytesUnpacked}`.
- **Signatures:** ed25519; canonical JSON as TUF 1.0 specifies (the crate's implementation, never ours).

**As built (8A, `fin-registry::tuf`, `editor`, `search`, `advisory`).**
- A signed target is named `<publisher>/<name>/<version>.finpkg` and fetched as `<dir>/<sha256>.<base>`, the TUF 1.0 consistent name (`tough`'s own `<sha>.<dir/base>` is not used: targets are fetched by us, with the signed length as the cap, then length and SHA-256 checked). `search` lists a package only when its id's publisher is the signing role and the target name has that shape, from signed custom metadata alone; yanked versions are hidden from search and `resolve(None)` and stay resolvable by exact version.
- **Custom metadata** is `TargetCustom {id, version, kind, hostApi, platforms, license, permissionsDigest, manifestSha256, yanked?, bytesUnpacked}` (an empty `platforms` is not signed as `[]`). **`permissionsDigest` is BLAKE3 over the sorted canonical permission JSON** (not SHA-256 as the first draft had), the same function the plan compares, pinned by `permissions_digest_is_pinned`.
- **Extra structural checks** (`checks.rs`): consistent snapshots, exactly one terminating `<publisher>/*` delegation per publisher (no top-level role names, no nested delegation, targets inside the prefix), at most 500 delegations, no role the snapshot lists left unloaded, and top-level target names that are plain package paths. Registries without consistent snapshots are refused, and nested delegations are not stored or searched (limits that matter for the conformance run, §23.4).
- **Editor** (`editor::RepoBuilder`, a non-test module used by `fin-plugin` and tests): `init`, `open`, `delegate`, `publish` (strict reader and `validate`; `PLUGIN_VERSION_EXISTS`), `yank`, `advise`, `rotate(Role, keys, threshold)`, `resign_timestamp`. Keys are PKCS#8 / SPKI PEM (OpenSSL v1 documents parse). `delegate` signs the delegation into `targets` together with the publisher's first role, because `tough` 0.24 loads no delegated role that has no file; re-delegation is `rotate(Role::Publisher)`, which re-signs the role with the new key in the same commit. Fixed vectors (`tests/fixtures/tuf/vectors/{basic,rotated}`, regenerated with `FIN_REGEN_TUF_VECTORS=1`) use a `test-only` publisher and discarded keys.
- **Advisories** are `SignedAdvisory {signedBy, advisory}`: registry-wide ones (top-level `targets`) apply to any plugin, a publisher's only to its own; `versions` is a semver `VersionReq` (one that does not parse matches nothing). `summary` is at most 500 characters and may hold no control characters, checked when parsed from signed metadata (`REGISTRY_INVALID {reason: advisories, field}`) and when the editor signs.

### 6.2 Client update workflow (TUF 1.0 §5, per registry)

1. Load the trusted root; fetch `<n+1>.root.json` until none is found (≤ 32 per refresh), each verified by the old and the new threshold; **save each newly trusted root atomically** (temp then rename) before fetching the next. On a rotation of the timestamp or snapshot keys, delete the trusted timestamp and snapshot (**fast-forward recovery**).
2. Fetch `timestamp.json` (≤ 16 KiB), verify, refuse a lower version (rollback); expiry per §6.3.
3. Fetch the snapshot named by the timestamp (hash and length checked); refuse lower role versions than trusted.
4. Fetch top-level and delegated targets by snapshot version; a publisher's role signs only `<publisher>/*` (terminating), so `@a` can never vouch for `@b/x`.
5. Persist every trusted file atomically and only after the whole chain verifies; a failure keeps the previous trusted set and reports `TUF_*`.
6. Targets are fetched with the signed length as a cap and accepted only if every listed hash matches.

**As built (8A).** `TrustedRepo::{init, open, refresh(Arc<dyn Transport>, &Policy, &dyn Clock) -> RefreshReport, targets, target, fetch_target, fetch_target_bytes, versions, max_stale_days, last_refresh_at}` wraps `tough` 0.24 (`default-features = false`; `refresh` takes an `Arc` because `tough` needs a `'static` transport). `tough` fell short in six places, each covered in `fin-registry::tuf` and pinned by a test:
1. Its datastore writes are not atomic and it writes `timestamp.json` before the snapshot verifies. **A refresh gives `tough` a scratch datastore** seeded with copies of the trusted timestamp and snapshot (never `latest_known_time.json`) and promotes a whole generation only after `load()` succeeds.
2. It takes the trusted root from the caller and stores it re-serialised. We keep our own `roots/<n>.root.json` chain from the raw fetched bytes: the `Transport` adapter saves the `<n>.root.json` it served (verified by then) before `tough` asks for `<n+1>`, temp then rename.
3. Any transport error on `<n+1>.root.json` ends its root walk as "no newer root". The adapter fails the refresh after any root error but not-found (`FsTransport` returns not-found only for a missing regular file).
4. When a saved root changes the timestamp or snapshot keys, the adapter deletes our trusted timestamp and snapshot in the same step (**fast-forward recovery**).
5. `tough` reads the wall clock. It always loads with `ExpirationEnforcement::Unsafe`, and `strict` and `relaxed` expiry are applied with the injected `Clock` (§6.3).
6. Its size caps cover the root, timestamp and snapshot only. The adapter enforces the per-file caps by name: root and targets 4 MiB, timestamp 16 KiB, snapshot 1 MiB, a delegated role 4 MiB. `MAX_ROOT_UPDATES` is 33 because `tough` stops when the root reaches anchor + limit, so one refresh trusts at most 32 new roots (`REGISTRY_INVALID {reason: tooManyRoots}`; the verified roots stay and the next refresh continues).

Errors are `{code, message, details, fix?}` (`REGISTRY_UNTRUSTED|INVALID|UNREACHABLE`, `TUF_SIGNATURE|THRESHOLD|ROLLBACK|EXPIRED|TARGET_MISMATCH|DELEGATION`, `PLUGIN_NOT_FOUND`, `IO_ERROR`) and name the delegated role. `transport::{Transport, TransportError {NotFound, TooLarge, BadPath, Unreachable}, FsTransport}`: regular files only, no symlinks, path components that refuse `\`, `:` and control characters.

### 6.3 Freshness, staleness, clocks, removal

- **`strict`** (future public registries): an expired timestamp is `TUF_EXPIRED`; nothing new installs; installed plugins keep running.
- **`relaxed`** (local folders, the only v1 kind): metadata is loaded with expiry enforcement off (`tough`'s `ExpirationEnforcement::Unsafe`, which 8A Task 1 verifies skips *only* expiry checks) and the host applies its own rule: expired by more than **`maxStaleDays` (default 7)**, read from the signed root's `custom.finisher.maxStaleDays` (≤ 90), is `TUF_EXPIRED`; within it, `REGISTRY_STALE` is a warning. Rollback checks stay strict.
- **Staleness is shown per plugin:** `plugin.list` and `plugin.info` carry `registryStaleDays` for every installed plugin from a registry, and status warns when any is stale.
- **Clocks:** each root's trust directory records the wall-clock time of its last successful refresh, never later than the newest trusted timestamp's expiry (so a clock that ran fast cannot lock refreshes out for longer than that); a clock more than a day earlier than it is `CLOCK_SUSPECT`, and refreshes change nothing until it is fixed. After correcting a clock that ran fast, the owner can clear the recorded time on the server with `fin-plugin registry trust reset-clock` (human-only; it changes nothing else).
- **Trust store:** each root's directory changes under an exclusive lock file (`trust/<rootSha256>.lock`) held for a whole refresh, root rotations included. A refresh writes a whole generation (the metadata as fetched, and the verified bytes of the advisories files it lists) and then names it current; a generation a crash left unnamed is never reused and is deleted on the next open. Advisories are read from that kept copy, never re-read from the folder: a kept copy that is missing, does not match its signed hash or does not parse is `REGISTRY_INVALID`, never an empty list.
- **Removing a registry keeps its trusted metadata** (`trust/<rootSha256>/` stays); re-adding a registry with the same root resumes from the trusted versions, so remove-and-re-add cannot roll it back. `registry.forget --root <sha>` (owner, console) deletes it explicitly.

**As built (8A).**
- **Freshness** (`tuf/freshness.rs`) counts every trusted role, publisher roles included: `strict` → `TUF_EXPIRED {role, days}`; `relaxed` → `TUF_EXPIRED {role, days, maxStaleDays}` past `maxStaleDays`, else the refresh succeeds with `REGISTRY_STALE {role, days, maxStaleDays}` and `staleDays`. `plugin.list` carries `registryStaleDays` and `fin plugin ls` prints `REGISTRY_STALE` warnings.
- **Clock check** (`clock::check`) runs before any fetch: `CLOCK_SUSPECT {now, lastRefreshAt}` when the injected clock is more than 24 h before `state.json`'s `lastRefreshAt`; `lastRefreshAt` never moves back and is capped at the newest timestamp's expiry.
- **Trust store layout:** `trust/<rootSha256>/roots/<n>.root.json` as fetched; `gen-<k>/{timestamp,snapshot,targets}.json`, `roles/<publisher>.json` and `kept/<target>` (the verified advisories) behind a `current` pointer replaced atomically; `state.json`; `tmp/`. A generation is numbered past every `gen-*` on disk, and opening a store deletes generations `current` does not name, so a crash between the rename and `current` wedges nothing. An exclusive lock file (`trust/<sha>.lock`: std's `File::lock`, `flock` on Unix, `LockFileEx` on Windows) is held for a whole refresh and rotation and shared while loading. `forget` refuses a root a registry still trusts (`REGISTRY_INVALID {reason: inUse}`); a root file in a registry folder must be `1.root.json` or a newer root of its chain.

### 6.4 The package (`.finpkg`)

- A zip archive (`zip` 9, already in the tree) with `finisher-plugin.json` at the root and the files the manifest names. Packages carry no signature of their own: their identity is the hash and length in signed targets metadata.
- **Strict reader** (`fin-registry::package`): regular files and directories only (no symlinks, hard links or devices); relative NFC UTF-8 names without `..`, leading `/`, drive letters, `\`, NUL, Windows reserved names or trailing dots and spaces, ≤ 255 bytes per component and ≤ 1,024 per path, unique after case folding; ≤ 10,000 entries, ≤ 512 MiB unpacked, ≤ 200 : 1 ratio per entry, declared sizes checked against written bytes; extracted into `staging/` with modes 0644/0755 (executables only where the manifest names them), never through a path that resolves outside it.
- Models are not inside packages (§8.2).

**As built (8A, `fin-registry::package`).** `unpack(archive, dest, executables, &Limits) -> Unpacked {root, tree_hash, manifest, files}` works in two passes. Pass 1 reads only the central directory and refuses duplicate names (`zip` keeps one entry per name, so a second would be invisible), entry-count and size limits, non-UTF-8 names (non-ASCII needs the UTF-8 flag), compression other than stored or deflate, encryption, anything but regular files and directories, every name rule above and a root manifest over 256 KiB. Pass 2 creates `dest` (it must not exist), streams each file to a temp file in a private directory inside `dest`, refuses anything but exactly the declared size, sets 0644/0755 (archive modes ignored), renames into place, creates directories one component at a time (each checked to be a real directory) and removes `dest` on any failure. Beyond the list above the reader refuses `:` and Windows' other forbidden characters, device names in any case and with any extension (`CON`…`CLOCK$`, `COM0–9` and the superscript digits, `LPT` likewise), NTFS 8.3 short names (`shortName`) and a file used as a directory. `tree_hash(root)` is BLAKE3 over sorted `path \0 class \0 sha256`, class `x` for the owner-execute bit (always `f` on Windows). `pack` is reproducible: files only, sorted, deflate level 6, DOS-epoch timestamps, "made by Unix" (the `zip` writer otherwise writes DOS on Windows), modes from the executables list, no extra fields; it refuses what the reader would. Errors: `PACKAGE_UNSAFE {entry, reason}`, `PACKAGE_TOO_LARGE {reason, limit, got}`, `PLUGIN_MANIFEST_INVALID {path, reason: missing|schema}`, `MANIFEST_TOO_LARGE`, `IO_ERROR`. **Deviations:** a lying declared size is caught by `zip` itself (`corrupt`) before the reader's own count (`sizeMismatch`); a "hard link" is a second entry for one path (zip has no hard links; tools that keep them write duplicates).

### 6.5 Yanks, advisories, revocation, key rotation

- **Versions are immutable** (arch §15 rule 2). A yanked version is not offered to new installs and stays installable by exact version with a warning.
- **Advisories** are signed targets (per publisher, and registry-wide under top-level `targets`): `{id, plugin, versions, severity, action: warn|disable, summary, url?}`. A refresh by any principal applies `disable` advisories (`PLUGIN_REVOKED`, audited, journaled): disabling is a safety action, not an update.
- **Key rotation:** root by a new root version signed by both thresholds; targets, snapshot and timestamp keys by root; a publisher's key by re-delegation. A compromised publisher key is answered by re-delegation plus a `disable` advisory.

**As built (8A).** A refresh by any principal applies `disable` advisories through `Registry::revocations(installed)`; a row whose registry id now has another root is not revoked by it (§23.2 M1). A kept advisories file that is missing, mismatched or unparseable is `REGISTRY_INVALID`, never an empty list, and an unparseable signed file fails the refresh before anything is trusted. Rotations are tested for the root (1→2→3 with threshold changes, each root saved atomically), for the timestamp key with fast-forward recovery (also when the process dies between the rotating root and the new timestamp) and for a publisher key.

### 6.6 TUF implementation

| Candidate | Version (2026-11-07) | Licence | Notes | Choice |
|---|---|---|---|---|
| `tough` (awslabs) | 0.24.0 (2026-07) | MIT OR Apache-2.0 | TUF 1.0 client and repository editor; delegations, consistent snapshots; used for Bottlerocket updates. Crypto through `aws-lc-rs` (`aws-lc-sys`: ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0), all allowed by `deny.toml` through an allowed branch); async on `tokio`; it pulls `rustls` with the `aws_lc_rs` provider | **chosen**, behind the 8A Task 1 gate |
| fork of `tough`'s narrow crypto onto `ring` | — | MIT OR Apache-2.0 | `tough` uses crypto only to parse public keys and verify signatures (and to sign, in the editor); those paths are small enough to carry as a patched fork on `ring`, which the tree already uses | **fallback** if Task 1's gate fails |
| `tuf` (rust-tuf) | crates.io lists 0.3.0-beta15, but the latest tagged release on the project's own tracker is 0.3.0-beta9 (2022-07), and its README calls it not production-ready | MIT OR Apache-2.0 | unmaintained in practice | **rejected** (it was revision 1's fallback) |
| `tuf-js` + `@tufjs/models` | 6.0.2 | MIT | the npm CLI's client; TypeScript, no repository editor | rejected: verification belongs beside the Rust package reader, and the editor must be Rust |
| `python-tuf` | — | Apache-2.0 OR MIT | the reference implementation | test-time oracle only (`tuf-conformance`, §15.1) |

- **Two `rustls` providers in one `.node`.** `fin-fonts` uses `rustls` with `ring`; `tough` brings the `aws_lc_rs` provider. With both compiled in, `rustls` panics when code builds a `ClientConfig` without naming a provider and no process default is installed. **Rule:** every `ClientConfig` in our crates is built with `ClientConfig::builder_with_provider(...)` naming its provider, nothing installs a process default, and a test builds `fin-node` with both and exercises every HTTPS path (fonts, the TUF test transport) without a default. 8A Task 1 proves it with a real HTTPS fetch on all three OSes.
- **Build needs:** `aws-lc-sys` builds C with `cc`; whether CMake or NASM is needed depends on its version and target (Windows x64 needs NASM unless the `prebuilt-nasm` feature uses its bundled objects). Task 1 records what each OS needs; a C toolchain without CMake or NASM in CI is the gate.
- `fin-registry` wraps the chosen crate behind `TrustedRepo` (§6.2), with a filesystem transport and, in test builds, an HTTP transport. `fin-node` runs it on its per-engine worker thread (core §3.10 Decision 7) inside a current-thread `tokio` runtime owned by `fin-registry`; nothing async leaks into fin-node's API.

**As built (8A).**
- **Choice confirmed by the Task 1 gate: `tough` 0.24.0 (`default-features = false`, no `http`) on `aws-lc-rs` 1.19 / `aws-lc-sys` 0.47; no `ring` fork.** The gate ran on macOS arm64, Linux x64 and Windows 11 x64 with no CMake, NASM or Go on `PATH` (`AWS_LC_SYS_CMAKE_BUILDER=0`: the `cc` builder only). `aws-lc-sys` builds with "Building with: CC"; on Windows x64 it stops without NASM unless `aws-lc-rs/prebuilt-nasm` is enabled, so `fin-registry` enables it under `cfg(all(windows, target_arch = "x86_64"))` (26 bundled objects). `cargo deny check licenses sources bans` passes on the current `deny.toml`; AWS-LC is listed in `third_party/native.json` and `THIRD_PARTY_NOTICES.md`.
- **Release `.node` growth:** macOS +5.02 MiB; Linux +6.62 MiB unstripped, **+5.48 MiB stripped**; Windows +4.32 MiB. **Deviation:** the plan's 6 MiB limit is met on the stripped artifact only, so `fin-node`'s size budget is stated on the stripped `.node` (or `strip` is set in the release profile) once it links `fin-registry`.
- **Both `rustls` providers coexist:** `tough` brings `rustls` with default features, so every build that links it has both `ring` and `aws_lc_rs`, and `ClientConfig::builder()` without a provider panics (checked). `fin-fonts`' only config (`Http::new`) already names `ring`; the test transport names `aws_lc_rs`. `crates/fin-fonts/tests/provider.rs` `no_process_default_provider` keeps it honest: it asserts an unnamed builder panics and drives a fetch through a loopback `CONNECT` proxy to a ClientHello with no default installed. A real HTTPS fetch through both clients in one `.node` succeeded on all three OSes.
- **The behaviour checks** (17 of 17 on `tough`'s own editor output) are pinned in `fin-registry` by `unsafe_expiry_only`, `each_root_saved_atomically` and `timestamp_key_rotation_with_fast_forward_recovery`.
- **`test-http`:** `transport::HttpTransport::{new(metadataUrl, targetsUrl), at(rootUrl)}` (`ureq` 3.5 with `rustls-no-provider` and `platform-verifier`, already in the tree through `fin-fonts`; no proxy, no redirects; 404 and 410 are not-found; bodies are read one byte past the cap) is enabled only by `fin-registry`'s own dev-dependency on itself and the `tuf_conformance_client` example (`required-features`). `scripts/check-test-hooks.mjs` refuses `test-http` anywhere in the default members' normal graph.
- **`fin-node`'s `Registry`** (napi, on the per-engine worker thread; every call is a promise, none runs on the JS thread): `plan`, `commit(planId, digest, accept?, checks?)`, `removePlugin`, `sweep`, `validateManifest`, the registry calls (`registryList|Status|Add|Remove|Forget|Refresh`, `search`, `info`), `stagedPlan`, `discardPlan`, `revocations`, `installedDirs`, `packContent`, `packStyle`, `bundledList|Verify`.

---

## 7. SDKs and developer experience

### 7.1 Rust: `fin-plugin-sdk` (MIT, crates.io)

```rust
use fin_plugin_sdk::{prelude::*, analysis::Transcript};

#[derive(Default)]
struct Speech;

impl Plugin for Speech {
    fn command(&mut self, host: &Host, call: CommandCall) -> Result<CommandOut, PluginError> {
        let snap = host.snapshot()?;                     // fin_core::model::Project
        let ops = fin_core::compose::caption_import(&snap, /* … */)?;
        Ok(CommandOut::tx(ops))
    }
    fn analyze(&mut self, host: &Host, job: AnalyzeJob, out: &mut ArtifactOut) -> Result<(), PluginError> {
        let pcm = host.pcm(job.input(), 16_000, 1)?;    // fin-shm: an in/ file, mapped read-only
        out.write(Transcript { /* … */ })                // written to out/ with its size declared; the host validates a copy
    }
}

fin_plugin_sdk::main!(Speech);
```

The protocol loop, typed host calls, `fin-shm` readers and writers (slot messages included), progress and cancellation, artifact writers over `fin-core::analysis` types, and an `onnx` feature (`ort` 2 with `load-dynamic`, loading the host's ORT).

### 7.2 TypeScript: `@finisher/plugin-sdk` (MIT, npm)

`definePlugin({ commands: { "trim-silence": async (args, host) => ({ tx: [...] }) }, providers: { … } })`, generated types, `host.fetch` over brokered egress, positional-read access to `in/` files (no mapping, no rings), declared-size writes to `out/`, and a bundling recipe (esbuild to one ES module). The package's `engines.node` becomes the manifest's `runtime.node`.

### 7.3 Python (after v1)

The design note of §3.2 holds; nothing is built in v1.

### 7.4 `fin-plugin-test`

A harness that runs a plugin against a fake host: fixture snapshots, generated PCM and frames (sub-project 6's test patterns), a fake HTTPS API for providers, assertions on calls, permissions, reach and outputs. **Conformance profiles:** `command`, `analyzer` (artifact validity, determinism declaration, cancellation within 2 s, crash recovery), `provider` (§9.5), `codec` (media §13.5), `matte` (§10.3). CI runs every bundled plugin through its profile.

**As built (8B).** `fin-plugin-sdk` (Rust, MIT) and `@finisher/plugin-sdk` (TypeScript, MIT) are built and unpublished (`publish = false`, `"private": true`; sub-project 11 publishes them). Both are the envelope's second implementation: `hello` with `v` 1, the SDK's minor and no `envelope` hash; a trailing `--warm` means "send `ready` after each job"; unknown host message types are dropped and logged once per type (framing, `seq`, `v` and the shape of known types stay fatal, exit 65); stdin EOF exits 70; a cancel fails an in-flight call `JOB_CANCELLED` and the job always ends `JOB_CANCELLED`. Cross-SDK error contract (pinned by tests on both): unknown command `PLUGIN_PROTOCOL {reason: unknownCommand}`; undecodable arguments `{reason: args}`; bad job `{reason: kind | params | context}`; snapshot `{reason: snapshotPage | snapshotPages | snapshotRev | snapshotJson}`; body `{reason: bodyLength | bodyPage | bodyTooLarge}`; unrepresentable result `{reason: badResult}`; result or error over 1 MiB `{reason: tooLarge, bytes}`; an uncaught failure `PLUGIN_COMMAND_FAILED`; a plugin error's `fix` travels as `details.fix` and the host lifts it into `FinError.fix`. Examples `examples/plugins/rust-tools` and `ts-tools` (`trim-silence`, `fetch-title`) pass 8A `validate`, run under the real plugin command path in `plugins-sdk-e2e.test.ts` and in the `plugin-code` e2e scenario. **Deviation (§7.2):** the TypeScript SDK ships no esbuild; `tools/bundle.mjs` (Node's `stripTypeScriptTypes`) bundles a stated subset and refuses everything else loudly; the README gives the esbuild recipe. **`fin-plugin-test` (Task 14):** a fake host that starts a plugin as the host's `plugin` family does (runtime program, Node's permission flags, trailing `--warm`, the environment allow-list, a private temp), speaks the envelope's host side, answers calls in the host's order (method, decode, the manifest's permissions, then the call), serves a fixture snapshot (built in, or `--fixture`) and `net.fetch` through a loopback HTTPS API with a certificate made per run (the fake host is the broker), and records every call. The **`command` profile**: out-of-schema arguments are refused by the host and never sent; every command answers within its timeout in a warm process (and, once it passed, in a cold one) without crashing (`PLUGIN_CRASHED`) or breaking the envelope; a returned transaction applies under fin-core (`PLUGIN_TX_INVALID`); a cancel ends the job within 2 s; every call outside the declared permissions or origins is a finding; no `log` message holds a path or the run's canary secret. `fin-plugin test <dir> [--profile command] [--fixture f] [--exec p] [--node p]` prints the report, or fails `PLUGIN_TEST_FAILED` (exit 3) with the findings; CI runs both examples through it on all three OSes. Not built: the media fixtures (PCM, frames) and the other profiles (with 8C, 8G, 8H); the Python SDK stays after v1.

---

## 8. Bundled analyzers

### 8.1 Shape

- **`fin-analyze`** is one native Rust binary built with the Rust SDK, shipped in the app and installed as several bundled plugins sharing it, each with its own manifest: `@finisher/speech`, `@finisher/scenes`, `@finisher/audio`, `@finisher/search`, `@finisher/reframe`, `@finisher/key-matte` (8H). Permissions: `media:read`, plus `gpu` for the vision analyzers; no `net`, no `project:write`.
- **Analyzer contribution:** `{id, version, artifact, kv, inputs: {audio: {rate, channels}} | {frames: {maxSide, fps, format}} | {artifacts: [kinds]}, params (JSON Schema), appliesTo, resources, models, stage: {library, default, dependsOn}}`. The host registers one `fin-jobs` kind `analyze` (`params.analyzer` selects) and, when `stage.library`, a library stage in cp §7.3's framework keyed by `(analyzer, version, model hashes)`.
- **Recipes and caching:** an artifact id is `ar:<kind>:<hex16>`, the hex being BLAKE3 of the canonical recipe `{analyzer id and version, model SHA-256s, canonical params, input fingerprint and stream, input feed version and decoder class, runtime class (ORT version, provider class cpu|gpu)}`. Identical recipes hit the cache (`<server-data>/cache/analysis/`, indexed by `fin-node`, LRU-evicted, regenerable); the job's `dedupeKey` is the recipe (cp §6.3). The artifact records its content hash.
- **Determinism:** pure Rust stages are bit-exact across OSes and wasm32. ONNX stages run on the CPU provider with fixed thread counts and deterministic compute and are compared with tolerances (§15.1). GPU providers are part of the recipe.
- **Results:** artifacts in the cache; views (`analysis.get`, `fin transcript`, `fin scenes`, `fin beats`, `fin search`); ops from fin-core builders (§8.8).

### 8.2 ONNX Runtime and the model manager

- **ONNX Runtime** (MIT) is one pinned shared library per platform, loaded through `ort` 2 (MIT OR Apache-2.0; still `2.0.0-rc.15`, pinned exactly and wrapped in `fin-analyze::onnx`). The CPU provider is the default everywhere (the spike measured DirectML 3–4× slower than CPU for speech, arch §27.4). CoreML (an OS framework) is used for vision on macOS when it passes the tolerance tests; GPU providers on Windows and Linux are user-installed (CUDA), and DirectML is not bundled until its redistributable terms are checked.
- **Models** are downloaded on demand into `<server-data>/models/<sha256>/` with `model.json {id, sha256, bytes, licence, attribution, provenance, source, plugin}`: resumable, verified, pausable, through `model.fetch` jobs on the `net` pool (run by the host, not the plugin), from commit-pinned upstream URLs in the signed or bundled manifest. Gated upstream weights are mirrored by Finisher, pinned by SHA-256, when their licence permits redistribution. Attribution is shown in the model manager and the notices (§18.2).

### 8.3 Speech: VAD, transcription, alignment (`@finisher/speech`, 8C)

| Stage | Model / method | Licence | Output |
|---|---|---|---|
| VAD and chunking | Silero VAD (ONNX) | MIT | speech regions; chunks ≤ 30 s cut at pauses |
| Transcript, English | Parakeet-TDT 0.6B v2, TDT greedy decode in Rust over ONNX | CC-BY-4.0 (attribution; the ONNX conversion and any quantisation are stated) | words with ASR timing |
| Transcript, other languages (uk, de, es, fr, pt-BR first) | whisper large-v3-turbo, language forced from the content language (arch §17.1), detected from the first 30 s when unset | MIT | words |
| Number verbalisation | own Rust rules per language (cardinals, ordinals, currency, times, years; Ukrainian case agreement), NeMo text-processing grammars (Apache-2.0) as reference only | ours (MIT) | spoken forms mapped back to display words |
| Alignment | wav2vec2-base-960h CTC (fp32 ONNX), Viterbi word spans in Rust; accents stripped, Cyrillic transliterated | Apache-2.0 | word spans |
| Boundaries | **starts:** learned correction (gradient-boosted trees, evaluated in Rust from a data file); **ends:** raw CTC span end, until 8C re-selects the rule (below) | ours | aligned word times |

- **Excluded:** MMS (CC-BY-NC), CrisperWhisper (NC weights) and whisper-timestamped (AGPL) on licence; Qwen3-ForcedAligner and NeMo Forced Aligner (80 ms steps) and Parakeet v3 for non-English (cannot force the language) on quality (arch §27.4). **WhisperX** is BSD-2-Clause; it is not used because it needs Python, PyTorch, pyannote's gated models and ffmpeg (through torchcodec), not because of its own licence.
- **Diarization** is cut from v1; `speaker?` stays reserved in the artifact.
- **Quality gate** (arch §27.4): ≥ 95 % of word starts and of word ends within [−125, +45] ms of human labels per supported language; p95 against MFA references ≤ 50 ms tracked, not gated; WER tracked.
- **Re-selection on permissive labels.** Arch §27.4 chose "raw CTC ends" from about 40 candidates measured against labels on the owner's reel and generated TTS clips. Those clips are not permissively licensed, and choosing a rule on them is tuning; they are retired from selection and gating (owner decision 6). 8C rebuilds the human-labelled reference set from permissive speech (Common Voice CC0, LibriSpeech and MLS CC BY 4.0), labelled for Finisher, re-runs the candidate selection and the gate on it, and records both results.
- **Retraining the start correction** (8C):
  - data: LibriSpeech train-clean-100/360 and MLS (de, es, fr, pt; both CC BY 4.0) and Common Voice (CC0; the release, its archive SHA-256 and the download terms accepted for it are pinned in the data card); references from our own Montreal Forced Aligner runs (MFA: MIT; each acoustic model's licence checked, else trained by us on the same data);
  - augmentation: music under speech at 0–20 dB SNR from **MUSAN, filtered per file**. MUSAN's corpus page states CC BY 4.0, but its music comes from several sources and each file's own licence is listed in the corpus's per-directory LICENSE files; only files whose own licence is public domain, CC0 or CC BY are used and the file list is in the data card; the owner reads the AGENTS.md rule as applying to the files used (owner decision 6);
  - objective: the gate window's hit rate; speaker-disjoint splits; held-out evaluation on the permissive human-labelled set (the owner's footage is evaluation-only and never a training input, §15.3);
  - outputs: the model file (≈ 0.5 MB, shipped) and its data card in `THIRD_PARTY_NOTICES` and the model manager. Training runs live in `archive/` (AGENTS.md).
- **Transcript artifact** (`fin-core::analysis::Transcript`, kv 1): `{language, models: {asr, aligner, corrector}, words: [{text, spoken?, startMs, endMs, conf, timing: aligned|interpolated|asr, speaker?, filler}], segments: [{startMs, endMs, words: [first, end)}], silences: [{startMs, endMs}]}`, in integer milliseconds of source time (the caption grid, core §5.5).

### 8.4 Scenes (`@finisher/scenes`, 8D)

- **Detectors** (pure Rust, bit-exact): ports of PySceneDetect's `ContentDetector`, `AdaptiveDetector` and `ThresholdDetector` (BSD-3-Clause, attributed), over a frame feed at ≤ 256 px, every frame up to 30 fps and every second frame above.
- TransNetV2 is cut from v1.
- **Artifact** `Scenes {rate, cuts: [{frame, kind: cut|fadeIn|fadeOut|dissolve, score, detector}]}` in source frame numbers; `dissolve` comes from the adaptive detector's slow-change pattern only.

### 8.5 Audio (`@finisher/audio`, 8D)

- **Loudness:** momentary and short-term LUFS curves and integrated loudness from `fin-audio`'s EBU R128 meter (media §8.4).
- **Classes:** speech, music, silence, other per 0.5 s, from YAMNet (Apache-2.0, ONNX conversion) mapped to four classes, merged with Silero VAD regions.
- **Beats:** own DSP in Rust (spectral-flux onsets, tempo by autocorrelation, dynamic-programming beat tracking after Ellis 2007, downbeats from a meter estimate). Learned beat models are cut from v1. **Excluded:** madmom's models (CC BY-NC-SA), aubio (GPL), Essentia (AGPL).
- **Artifacts:** `Loudness`, `AudioClasses`, `Beats {tempo, beatsMs, downbeatsMs, confidence}`.

### 8.6 Semantic search (`@finisher/search`, 8E)

- **Chunks:** per shot (from `Scenes`), split into ≤ 2 s windows; one to three keyframes per chunk.
- **Embeddings:** SigLIP2 base (Apache-2.0; ONNX image and text towers; multilingual text) → 768-d, L2-normalised, stored as `f16`.
- **Index:** per library, `<server-data>/libraries/<id>/vectors/`, keyed by embedder id and version. **v1 starts with an exact scan** over `f16` vectors (1M chunks × 768 × 2 B ≈ 1.5 GB, scanned in blocks with SIMD); an HNSW index from `usearch` is added when a library passes the exact scan's latency budget (§15.4), and only after its licence and dependency check (Apache-2.0 declared; its C++ core and build need review). LanceDB (arch §13) is not used in v1.
- **Hybrid `library.search {mode: semantic|text|hybrid}`** (cp §7.4): reciprocal-rank fusion of vector hits, transcript FTS5 hits and name hits, with probe filters → `{file, fingerprint, startMs, endMs, score, signals, thumb}`, reach-checked like every library call.
- Cloud embedders (D29) are cut from v1.

### 8.7 Reframe (`@finisher/reframe`, 8F)

| Stage | Model / method | Licence |
|---|---|---|
| Faces | MediaPipe BlazeFace short- and full-range, converted to ONNX | Apache-2.0 (model card; the copy found on 2026-11-07 is a mirror of Google's asset store, to be re-checked against Google's own page before shipping) |
| People and subjects | YOLOX-Nano/Tiny | code Apache-2.0; the released weights carry no separate licence and are taken under the repository's, recorded as such |
| Tracking | ByteTrack-style association (Kalman + IoU), own Rust implementation | ours |
| Smoothing | per shot: dead zone, critically damped spring, maximum pan speed, hold on lost tracks; resets at scene cuts | ours |

- **Excluded:** Ultralytics YOLO models (AGPL-3.0), InsightFace SCRFD/RetinaFace weights (non-commercial).
- **Subject choice:** the largest stable face, else the most central person.
- **Artifacts:** `Tracks {rate, tracks: [{id, kind: face|person, samples: [{frame, box, score}]}]}`, `CropPath {aspect, rate, keys: [{frame, cx, cy, scale}]}`.

### 8.8 From results to ops (fin-core builders)

| Builder (`fin-core::compose`) | From | Ops | `fin` |
|---|---|---|---|
| `captions_from_transcript` (wraps `caption_import`) | a `Transcript` word range of a clip, mapped through `sourceIn` and `speed` | `caption.*`, `item.insert` | `fin captions add --from-transcript c12 [--words w:a..w:b]` |
| `caption_resync` | a caption block's `source` and the current transcript | `caption.setWords` | `fin captions resync <item>` |
| `paper_edit` | kept or removed word ranges (`w:182..w:240`, `--quote`) | `item.split`, `item.delete` (ripple) | `fin transcript cut|keep …`, `--no-fillers` |
| `split_at_scenes` | `Scenes` within an item | `item.split` | `fin scenes split <item>` |
| `markers_from` | `Scenes`, `Beats` | `marker.add` | `fin scenes markers`, `fin beats markers` |
| `duck` (media §8.5) | `AudioClasses`/VAD + a ducking preset | `keyframe.set` on gain | `fin audio duck <music track>` |
| `reframe` | `CropPath` + item | `param.set`/`keyframe.set` on transform and crop, thinned to ≤ 4 keys per second | `fin reframe <item> --aspect 9:16` |

Word addressing (`w:<n>`, arch §9.2) resolves against the transcript of the clip's asset and echoes the resolved text and times. Every builder is pure and dry-runnable.

---

## 9. Generation providers (8G)

### 9.1 Contribution and catalog

`providers: [{id, modalities: [image|video|voice|music|sfx|dubbing], catalog: "catalog.json"}]`. The **catalog is data**, signed with the package: `models: [{id, modality, params (JSON Schema), cost: {unit: call|second|character|image|token, amountMicros, currency}, terms (URL), outputLicence: {spdx | "provider-terms", commercialUse, attributionRequired}, limits, outputs: [mediaTypes]}]`.

### 9.2 Calls and paid-call safety

1. `provider.estimate {model, params}` → `{costMicros, currency, basis}`; sub-project 7's budgets (agent §13.1, charged to the invoker) and approval rules (agent §7.3, kind `plugin`) decide for agents; a person's call from the console or the UI needs no approval card.
2. The host creates the `fin-jobs` job, **owned by the invoker**, in state `submitting`, with an idempotency key **persisted before** `provider.submit {model, params, idemKey}` is sent; the plugin passes the key to the provider where the API supports one.
3. `submit` → `{externalRef}`, persisted into the job's `externalRef` before polling (arch §14.3, cp §6.4).
4. `provider.poll` with backoff; `provider.cancel`; `provider.fetch` → files in the job's `out/`, copied and validated after the process exits (§3.5).
5. **A crash between 2 and 3** leaves `submitting` without a ref: the host never re-submits. It calls `provider.find {idemKey}` when implemented, else fails with `GEN_SUBMIT_UNKNOWN` naming the key, for a person to check the provider's dashboard.
6. Outputs are probed, moved into `workspace/<session>/gen/` (or `workspace/shared/gen/` for a person) and registered by `asset.add` with **provenance** `{provider, model, paramsHash, prompt (content), costMicros, currency, outputLicence, externalRef (keyed hash), createdAt}`.

### 9.3 Brokered egress and credentials (`ModelGateway`, arch §5, §16, agent §10)

- **Plugins have no network.** All provider traffic goes through `net.fetch`, executed by the host.
- **HTTPS only**, to an origin the plugin declared and the person approved. Plain HTTP, other schemes and WebSockets are refused (`EGRESS_NOT_HTTPS`; WebSocket brokering is cut from v1).
- **Addresses:** the host resolves the name itself and refuses loopback, private (RFC 1918, RFC 4193), link-local, CGNAT and Tailscale (100.64.0.0/10), multicast, unspecified and IPv4-mapped forms of these (`EGRESS_PRIVATE_ADDRESS`); it **connects to the address it checked** (no second resolution), with SNI and `Host` from the origin, so DNS rebinding cannot redirect a checked name.
- **Redirects:** same-origin redirects are followed (≤ 3) with credentials; a **cross-origin redirect is not followed**: the 3xx response goes back to the plugin with its `Location`, and a request to that origin is a separate call, without credentials unless a secret is bound to that origin.
- **Secrets are bound to (plugin, name, origins).** The manifest declares `{secret, origins, header, scheme}`; at call time the host asks `ModelGateway` for a **grant handle** (agent §10: `ModelGateway.grant({…})` returns a short-lived in-memory handle) with `{plugin, secret, origins, principal, purpose: "gen:<provider>"}`, injects the header only into requests to a bound origin, and drops the handle after the call. The plugin never sees the key. Grants are counted per plugin and principal; the first per session and every denial are audited (`grant`).
- **Raw grants** (`secrets` with `mode: raw`, high risk): `secret.grant {name}` returns the value for one call, bound the same way and audited per call, for SDK shapes the broker cannot carry. Brokered is the default and raw the audited exception (owner decision 2). Neither first-party provider (§9.4) uses raw grants.
- **Limits:** response bodies capped per call (default 64 MiB, streamed to `out/`), 120 s per request, a per-plugin rate limit.

**As built (8B, `plugins/host/egress*.ts`).** `net.fetch` and `net.read` implement every network rule above and no secret injection: a request naming a grant answers `SECRET_MISSING {name}` and nothing is sent; `ModelGateway` grant binding is 8G's. Order: grant, URL, scheme (`EGRESS_NOT_HTTPS`), userinfo refused, origin exactly one of the plugin's approved origins (`EGRESS_DENIED`), refused headers (`host`, `connection`, `content-length`, `transfer-encoding`, `te`, `trailer`, `upgrade`, `keep-alive`, `expect`, `proxy-*`), body at most 768,000 bytes, rate limits (10 requests a second per plugin, 4 in flight per plugin and 64 overall, 16 name lookups; `EGRESS_RATE_LIMITED {reason, retryAfterMs, retryable}`), one name resolution per call with every answer checked (`EGRESS_PRIVATE_ADDRESS`: every IANA special-purpose block that is not globally reachable, IPv4-mapped, compatible, NAT64, 6to4 and SIIT forms decoded, anything unparsable refused), connection to the checked address with SNI and `Host` from the origin, no keep-alive, proxy, cookies or credentials. Same-origin redirects (at most 3) are followed; a cross-origin one, a fourth, or one to `http:` returns the 3xx with `redirectNotFollowed: true`. 120 s for the whole call (`EGRESS_TIMEOUT`), response bodies at most 64 MiB (`EGRESS_TOO_LARGE`), held by the host under random per-job handles and read in pages of 768,000 bytes; at most 256 MiB held per job and 512 MiB server-wide; released when the job ends. DNS, connect and TLS failures are `EGRESS_UNREACHABLE`. The private address is logged, scrubbed, and never given to the plugin. Test hooks exist only in `test-hooks` builds (`check-test-hooks` refuses any import from `apps/*/src`). **Residuals:** bodies are held in memory; `content-encoding` is passed through undecoded; a plugin's own `Authorization` or `Cookie` header is sent as given.

### 9.4 First-party providers (owner decision 5)

v1 ships two first-party providers, each a Node plugin installed as a bundled plugin, with brokered egress only (no raw grants):

| Plugin | Provider | Origin | Modalities in v1 | Secret |
|---|---|---|---|---|
| `@finisher/openrouter` | OpenRouter | `https://openrouter.ai` | image (models whose output modality is image) | the OpenRouter key the user pasted: the `SecretStore` entry the built-in agent uses (agent §10; the project's own key where it has one), bound to this plugin and origin |
| `@finisher/fal` | fal.ai | its queue API origin and the result-file hosts it returns, each declared (result fetches carry no credentials) | image, video, voice (TTS), music, SFX | a fal.ai key the user pastes |

The modalities and origins are the design's target; each is confirmed against the providers' catalogues and API documentation at 8G start, and a modality neither covers becomes a v1 gap like those below.

- **Catalogs are curated subsets.** Both services list far more models than v1 needs, and fal.ai's catalog changes weekly. Each bundled catalog (§9.1) lists only models whose terms and output licence were checked, with `outputLicence.commercialUse` true; a model whose output terms forbid commercial use or are unclear is left out. Catalogs change with app releases only (bundled plugins update with the app, §5.2), so a new upstream model waits for a release.
- **Paid-call safety** (§9.2) maps onto fal.ai's queue API: the request id it returns is the `externalRef`, polled and fetched as usual. Whether either API takes an idempotency key is checked at 8G start; without one, a crash between submit and `externalRef` ends in `GEN_SUBMIT_UNKNOWN` (§9.2 step 5).
- **Spend:** generation through OpenRouter draws on the same key, and therefore the same provider-side spend limit, as the built-in agent in that project; Finisher's own budgets (agent §13) count each call against its invoker.
- **Known v1 gaps.** **Dubbing** (translation re-voiced to the original timing) and **voice cloning** have no first-party provider in v1 unless the curated catalogs cover them; their presence and terms on OpenRouter and fal.ai are to be confirmed against the providers' catalogues at 8G start. Voice cloning, if offered, also needs a consent design (whose voice) that v1 does not have, so it stays out of the v1 catalog either way. The `dubbing` modality stays in the contract (§9.1) for third-party providers.

### 9.5 Conformance (`provider` profile)

A fake HTTPS provider API: estimates match the catalog; a crash after submit never re-submits; `find` recovers; cancel stops billing calls; poll respects `Retry-After`; fetched files match declared types and sizes; no secret appears in the plugin's logs or outputs; cross-origin redirects arrive unfollowed; private addresses are refused.

---

## 10. Media interfaces (with sub-project 6)

### 10.1 Codec backends

- **Contribution:** `codecBackends: [{id, decode: [{codec, profiles, bitDepths, chroma}], encode, demux: [containers], mux, priority, bitExact, licence}]`. The capability report (media §6.2) lists plugin backends beside the built-in ones; templates ask for capabilities, never backends (media §9.2).
- **Protocol:** sub-project 6's process backend (media §6.1) is the host side; packets in and frames out through `fin-shm` rings (`codec.open`, `codec.packets`, `codec.frames`, `codec.flush`, `codec.close`); `demux.*` work on a per-job read grant of the one source file (`media:file`).
- **Licence:** copyleft backends (the FFmpeg plugin) need the copyleft acknowledgement and come only from a registry the owner added.
- **Conformance:** media §13.5 as the `codec` profile; a failing backend is installable but not selectable (`CODEC_BACKEND_NONCONFORMANT`).

### 10.2 Offline audio processors

`audioProcessors` (de-noise, voice isolation) render a processed copy of an item's audio range as a new audio asset through a job (for example RNNoise, BSD-3-Clause, or DeepFilterNet, MIT OR Apache-2.0); a builder swaps the item's audio to it. Real-time effects are built into `fin-audio` (decision 2).

### 10.3 Matte producers

- **Contribution:** `matteProducers: [{id, subjects: [person|foreground|object|key], prompted, temporal, maxSide, resources}]`.
- **Protocol:** `matte.run {asset, range, size, prompts?}` in a job; rgb8 frames in through an `in/` ring, gray8 alpha out through an `out/` ring, **one per source frame, in order**; the host encodes the luma video with sub-project 6's encoder at the source rate (media §11.3).
- **Where mattes live** (owner decision 3): a matte a mask uses is a project asset of kind `matte` in `assets/` (write-once, synced), and `Mask.matte` names that asset, so export on another machine never depends on a cache or on a model reproducing its pixels. `AssetKind::Matte` is part of the `SCHEMA_VERSION` 3 bump (§3.9); project folders grow by the encoded matte.
- **Reference producer:** `@finisher/key-matte` (Rust, no model): chroma and luma keying with spill and edge controls; the conformance fixture and a v1 green-screen feature.
- **Conformance (`matte` profile):** frame count and order, value range, size, declared determinism, cancel within 2 s, crash recovery.
- **After v1:** background removal from BiRefNet (MIT), MODNet (Apache-2.0), SAM2 (Apache-2.0) or MediaPipe (Apache-2.0); excluded: RVM (GPL-3.0) and RMBG-2.0 (non-commercial).

### 10.4 WGSL effects, transitions and LUT packs

- A content pack's `effects` entry is `{kind, wgsl, params: [ParamSpec]}` with media §5.1's ABI. Validation at install: `naga` (MIT OR Apache-2.0) parses and validates, refuses bindings, storage buffers, atomics and textures beyond the ABI's, and limits the uniform block; render workers keep their GPU watchdog.
- **Pinning:** a project that uses a pack's effect pins it with `plugin.pin {id, version, sha256}`, which stores the pack's effect ParamSpecs in `Project.plugins` (§3.9); `plugin.repin` moves to another version (re-validating every item that uses its kinds, with conflicts as usual), and `plugin.unpin` is refused while an item uses a pinned kind (`PLUGIN_PIN_IN_USE`). Validation looks up built-ins ∪ pinned specs, so a project validates the same everywhere; rendering needs the pack's WGSL by `sha256` and reports a missing pack by item (`PLUGIN_MISSING`), never rendering differently. This is the per-project lockfile of arch §14.4, kept in the op log.

- **Decision (2026-11-09, owner): effect kinds are plugin-qualified (external review finding 29).** A pack's `effects` entry declares a bare `kind`; the item stores the **qualified kind** `@publisher/name:kind` (the pack's `PluginId`, a colon, the bare kind). *Why:* `Effect.kind` is the only thing an item stores (`fin-core::model::Effect`), the registry is built-ins ∪ pinned specs, and nothing says who owns a name, so two packs both declaring `glow`, or a pack declaring `blur`, would collide in pins, ParamSpec lookup and rendering.
  - **Grammar and reservation.** A bare kind is `[A-Za-z][A-Za-z0-9]{0,63}`. Built-in kinds (`colorAdjust`, `lut`, `blur`, and every kind `fin-core` adds later) stay bare, are **reserved**, and can never contain `@` or `:`; so a qualified kind cannot collide with a current or future built-in. Pack validation at install refuses a pack that declares a built-in name (`PLUGIN_MANIFEST_INVALID {path, reason: "reserved effect kind"}`) or the same bare kind twice. Two packs may both ship `glow`: they are `@a/looks:glow` and `@b/looks:glow`.
  - **Where the qualified kind is the identity.** `plugin.pin` stores each effect's qualified kind with its ParamSpecs in `Project.plugins` (the pin's `effects[].kind` is qualified, the manifest's is bare); `effect.add` and `Effect.kind` carry the qualified string; `Scope::Effect.kind` and the registry lookup (built-ins ∪ pinned, §3.9) key on it; the policy check "unknown kind" is a lookup of the whole string; footprints and conflict keys are the effect-id entities (`<item>/<fx>`, `<item>/<fx>#<key>`) and so need no change, while every kind-keyed check (`plugin.repin` re-validation of each item using the pack's kinds, the `PLUGIN_PIN_IN_USE` test of `plugin.unpin`, the pins' derived "kinds provided" set) matches on the `@publisher/name:` prefix and the full string. Rendering resolves the pack by the pin (`PluginId` → `sha256`) and the bare kind inside it, so the WGSL a kind names is the pinned pack's, never another's; a missing pack is `PLUGIN_MISSING` by item as before.
  - **Short names are a client convenience.** `fin` (and `compose` builders on the client side) may accept a bare name when it is unambiguous: a built-in name resolves to the built-in; otherwise a name that matches exactly one pinned pack's kind resolves to its qualified kind; several matches fail with `EFFECT_KIND_AMBIGUOUS {candidates}` and the exact qualified spelling in `fix`. The op that reaches `fin-core` always holds the qualified kind, so the op log never depends on what was pinned when the short name was typed.
  - **No alias, no rename.** A pack that changes its `PluginId` is another pack; items using the old kinds stay on the old pin until repinned by hand (`plugin.repin` stays within one `PluginId`). A new pack version that drops a kind an item uses makes `plugin.repin` a conflict, as before.
  - **Format.** The qualified kinds ship with the plugins format, project schema version 5 (v3 is stable keyframe ids, v4 the 6B-1 video fields, v5 plugins and mattes together): the §3.9 row now reads 4 → 5 (it said "2 → 3" before those bumps). Nothing is built before then: `Effect.kind` stays a `String` and the built-in registry unchanged. The effect-pack work (8H) implements this decision; its plan inherits it.

---

## 11. Diagnostics and training capture (arch §21.1)

- **Who emits:** the host (`source: finisherd` or `finNode`) only; plugin processes cannot write the journal and never learn the consent state; no `Source::Plugin` is added. A commit a plugin command produced carries the invoker's actor and `Tx.via`; a job a plugin started is owned by the invoker and carries the invoker's `ctx.actor`, with `via` in its payload.
- **Plugin references:** bundled plugins, analyzers, models and providers are `Name`s from new closed tables (`BUNDLED_PLUGINS`, `ANALYZERS`, `MODELS`, `PROVIDERS`); any other plugin, command or model id is a keyed `Hashed`.
- **Kinds** (all `meta` unless noted):

| Kind | Batch | Payload |
|---|---|---|
| `plugin.lifecycle` | 8A | `action: plan\|install\|update\|downgrade\|remove\|enable\|disable\|revoke`, `outcome`, `code?`, `plugin: PluginRef`, `source: bundled\|registry\|dev`, `kind: code\|content`, `permissionsAdded`, `risk`, `bytes` (log₂ bucket), `micros` |
| `registry.refresh` | 8A | `outcome`, `code?` (`TUF_*`), roles updated, targets (log₂ bucket), `staleDays?`, `byHuman`, `micros` |
| `plugin.stats` | 8B | per minute, per plugin and method family: calls, errors by code, reach denials, timeouts, crashes, p50/p95 micros, bytes in and out (buckets), egress requests and refusals by reason |
| `plugin.exit` | 8B | `plugin`, `reason: ok\|crash\|timeout\|killed\|oom\|protocol\|sandbox\|idle`, exit code or signal, uptime |
| `plugin.command` | 8B | `plugin`, `command: PluginRef`, `outcome: accepted\|rebased\|conflict\|refused\|invalid\|timeout\|data`, op-kind counts, `txId`; training: `args: Content<toolArgs>` |
| `analysis.run` | 8C | `analyzer`, `models`, `language`, `outcome`, `code?`, input seconds or frames, real-time factor, provider class, cache hit, attempts, `artifact: IdRef` |
| `model.fetch` | 8C | `model`, `bytes`, `outcome`, `code?`, `resumed`, `micros` |
| `gen.call` | 8G | `provider`, `model`, `modality`, `phase`, `outcome`, `code?`, cost bucket, latency; training: `prompt: Content<prompt>`, `params: Content<toolArgs>`, `outputs: Content<mediaHash>[]` |

- **As built (8A, `fin-core::events::payloads::plugin`, emitted by `fin-node` with `source: finNode`, never by a plugin).** `PluginLifecycle {action: Name(PLUGIN_ACTIONS), outcome: ok|refused|failed, code?, plugin?: PluginRef, source, kind: content|code|unknown, permissionsAdded, risk: low|medium|high|unknown, bytesLog2, micros}` and `RegistryRefresh {outcome, code?, rolesUpdated, targetsLog2, staleDays?, byHuman, micros}`, both v1; `PluginRef = {bundled: Name(BUNDLED_PLUGINS)} | {hashed: Hashed}` (bundled by table, else the journal's keyed hash, absent when the call never learnt which plugin). **Deviations:** `kind` and `risk` gain `unknown` (a plan refused before its manifest parses); `plugin` is optional; `BUNDLED_PLUGINS` holds ids without the `@` (`finisher/captions-basic`) because `NAME_PATTERN` does not allow it. `log2_bucket` is 0 for 0, else ⌊log₂ n⌋ + 1. With the journal off nothing is built (`events_off_means_nothing_built`). Tests: `lifecycle_events_hold_no_free_text`, `plugin_ref_never_free_text`, `bundled_ref_is_a_name`.
- **Training capture of plugin traffic** is typed, never raw HTTP: command arguments, provider prompts and parameters, output media hashes. Brokered request and response bodies are never journaled.
- **Never recorded:** plugin stderr, registry paths, publisher names, package file names, model URLs, prompts outside `Content`, artifact payloads.
- **Audit log** (cp §3.3): `plugin.install|update|remove|enable|disable|revoke|crashLoop` (an install or update of third-party code records the `thirdPartyCode` acceptance and `confined`), `plugin.grant` (with the diff), `registry.add|remove|trust|forget`, `registry.verifyFail`, `grant` (secret grant handles and raw grants), `hook.modify`, `sandbox.violation` (from 7C, with `profile: plugin`).
  - **As built (8A):** everything above except `crashLoop`, `grant`, `hook.modify` and `sandbox.violation` (8B and 7C). `details` is a closed schema: `pluginId`, `version`, `registryId`, `rootSha256`, `permissionsAdded`, `risk`, `thirdPartyCode {accepted, confined}`, `auto: true` on automatic and bundled changes, `planId`/`pluginId` on failed installs. Refusals are `denied` rows (`HUMAN_ONLY`, `*_NOT_ACCEPTED`); `registry.verifyFail` is written for `TUF_*`, `CLOCK_SUSPECT`, `REGISTRY_INVALID` and `REGISTRY_UNTRUSTED`; `plugin.revoke` carries `PLUGIN_REVOKED`, also from a plan's refresh.

**As built (8B).** `plugin.stats` v1 (per plugin, 60 s windows: per call family `command | project | net | log | other` calls, errors by code, reach denials, p50/p95 latency, bytes log₂; plus timeouts, crashes, dropped log and stderr lines, and the egress counters), `plugin.exit` v1 (`reason: ok | crash | timeout | killed | oom | protocol | sandbox | idle`; `sandbox` is unused until 7C; only for worker-role exits) and `plugin.command` v1 (command as a keyed hash, outcome `accepted | rebased | conflict | refused | invalid | timeout | data | failed`, `code?`, op-kind counts, `txId` as an `IdRef`, `dryRun`, `micros`; the validated arguments only under training, inline up to 16 KiB, else `argsOmitted: tooLarge`). All have source `finisherd`. `worker.exit` goes to `kv` 2 with `plugin?: PluginRef`, hashed in `fin-node` (`Journal.recordPlugin`) so the keyed-hash key never reaches TypeScript. Journal tests check that no id, publisher, stderr text, log text, origin or path reaches the journal. Audit: `plugin.crashLoop` (`auto: true`). No `plugin.disable` is audited as automatic (no tree re-hash exists, Decision 16). A plugin's own error code never reaches a record: `mapPluginError` keeps host-known codes and turns the rest into `PLUGIN_COMMAND_FAILED` with no details.

---

## 12. Security and threat model

Arch §16 priority: injected agent > malicious plugin > stolen device > network attacker. Sub-project 8 adds the supply chain.

| Threat | Mitigation | Residual |
|---|---|---|
| Malicious plugin reads or writes the user's files | `plugin` sandbox profile (§16.1): read roots limited to the plugin, the runtime, system libraries, ORT, its models and (cold kinds) the job's `in/`; write only (cold kinds) `out/`, its pluginData directory for the caller's authority (§5.1) and temp | **before 7C lands on an OS, every code plugin there, third-party ones included, is unconfined and runs with the user's rights** (owner decision 1, §3.1): the host's rules hold, the OS enforces nothing; third-party code is high risk at install and explicitly accepted; confinement applies automatically when 7C lands |
| **Confused deputy**: a plugin uses one principal's call to act for another, or a job outlives its invoker's rights | invoker ∩ grants on every call, job and commit; jobs owned by the invoker; reach checks (project, branch subtree, library) on every host call; **process memory, temp directory and `pluginData` keyed by effective authority** (principal class + rights hash + policy generation, §3.3, §5.1): a narrower caller never reaches a process, temp file or data directory that a wider one used, a child session never runs in its root's warm process, and a narrowed or revoked credential retires its key; the shared per-project data directory is offered only to the `person` and `system` classes while their rights are identical | a plugin may still misuse what one invoker could do anyway; callers with identical authority share a process and its data by design; the uniform-class sharing ends (those classes move to authority keys) if members of a class ever hold different plugin-relevant rights; before 7C the directories are a host rule, not an OS boundary (§3.1) |
| Plugin writes the project behind the user's back | ops only, through `tx.commit` under the invoker; `Tx.via` set by the host only; protected branches (cp §8.5) | granted plugins can make bad edits; reviewable and undoable |
| **Shared-memory TOCTOU and SIGBUS** | the host never maps plugin-writable files; outputs opened without following links, checked (regular, `nlink == 1`, declared size ≤ cap), copied, validated as copies; job outputs after exit; host-created fixed-size rings with slot messages (§3.5) | — |
| Plugin exfiltrates media or prompts | no network in the sandbox; brokered HTTPS to declared origins only; origins shown at install; `hooks:observe` + `net` is high risk | a granted origin receives what the plugin sends it; before 7C an unconfined plugin can open its own sockets |
| **SSRF through the broker** (the NAS admin page, a router, cloud metadata, `finisherd` itself) | private, loopback, link-local and CGNAT addresses refused after the host's own resolution; connects to the checked address; HTTPS only; cross-origin redirects not followed | — |
| Credential theft or replay to another origin | secrets bound to (plugin, name, origins); host-side injection through grant handles; no credentials after a cross-origin redirect | raw grants (opt-in) expose a key for one call |
| **TCC and Apple Events inheritance** (macOS) | Seatbelt denies `tccd` and Apple Events lookups; no read roots in TCC-protected folders (§3.2) | before 7C, TCC prompts from a plugin appear in the app's name |
| Compromised publisher key | per-publisher terminating delegations; rotation; `disable` advisories applied on any refresh | versions installed before the advisory run until the next refresh |
| Compromised registry folder or a hostile `git pull` | TUF thresholds, consistent snapshots, rollback checks, length caps; trust kept across registry removal | freeze up to `maxStaleDays` (7 by default) |
| Malicious archive | strict reader into staging (§6.4) | — |
| Look-alike ids | ASCII ids, reserved publishers, edit-distance warnings in the plan | a person can still approve |
| Injected agent installs, enables, widens or updates a plugin | owner scopes; plans from non-humans refused; agent-triggered refreshes install nothing; agent-facing content never auto-updates; suggestions only | — |
| Hostile content steering agents (skills, prompts, roles) | medium risk, shown at install; never auto-updated; any change is a widening | an approved pack's text is trusted by agents |
| Hostile model file | models only from signed or bundled manifests, pinned by SHA-256; inference inside the sandboxed process | an ORT exploit runs with the plugin's rights (the user's, before 7C) |
| GPU driver attack surface | `gpu` is medium risk; only render nodes (Linux), Metal user clients (macOS) and D3D (Windows) are reachable, and only with the grant | a kernel driver bug |
| Plugin starves the server | `fin-jobs` pools, memory and process limits, idle stop, crash-loop disable | — |
| Hook handlers see or rewrite agent commands | `hooks:observe` and per-hook `hooks:modify` grants; both payloads audited (agent §9.1) | — |

---

## 13. Error codes

| Exit | Codes |
|---|---|
| 2 | `MANIFEST_TOO_LARGE`, `PLUGIN_MESSAGE_TOO_LARGE`, `REGISTRY_AMBIGUOUS {id, registries}` (several registries list the plugin: name one) |
| 3 | `PLUGIN_NOT_FOUND`, `PLUGIN_NOT_INSTALLED`, `PLUGIN_BUNDLED`, `PLUGIN_MANIFEST_INVALID {path, reason}`, `EFFECT_KIND_AMBIGUOUS {candidates}` (a short effect name matches several pinned packs: spell the qualified kind), `PLUGIN_CONTRIBUTION_UNSUPPORTED {kind, hostApi}`, `PACKAGE_UNSAFE {entry, reason}`, `PACKAGE_TOO_LARGE`, `PLUGIN_HOST_API {range, host}`, `PLUGIN_PLATFORM {platforms}`, `PLUGIN_NODE_VERSION {range, node}`, `PLAN_EXPIRED`, `PLAN_MISMATCH`, `PLUGIN_ALREADY_INSTALLED {installed, version}`, `PLUGIN_DOWNGRADE_REFUSED {installed, version}`, `REGISTRY_NOT_FOUND`, `REGISTRY_INVALID`, `PLUGIN_COMMAND_ARGS`, `PLUGIN_TX_INVALID`, `ARTIFACT_NOT_FOUND`, `ANALYZER_NOT_FOUND`, `ANALYSIS_LANGUAGE_UNSUPPORTED`, `WORD_RANGE_INVALID`, `PLUGIN_UNPINNED`, `PLUGIN_PIN_IN_USE`, `GEN_PARAMS_INVALID` |
| 4 | `REGISTRY_EXISTS {id}` (a registry of that id is added already), `PLUGIN_CRASHED`, `PLUGIN_TIMEOUT`, `PLUGIN_PROTOCOL`, `PLUGIN_OUTPUT_INVALID {reason: link\|notRegular\|linked\|size\|truncated\|schema}`, `ANALYSIS_FAILED`, `GEN_PROVIDER_ERROR {status, providerCode?}`, `GEN_SUBMIT_UNKNOWN {idemKey}`, `MODEL_HASH_MISMATCH`, `ARTIFACT_STALE` |
| 5 | `TOO_MANY_PLANS {limit}` (a principal has 4 install plans open: install or `plugin.discard` one, or wait 10 minutes), `TOO_MANY_SUGGESTIONS {limit}`, `PLUGIN_DISABLED`, `PLUGIN_CRASH_LOOP`, `PLUGIN_RUNTIME_UNAVAILABLE`, `PLUGIN_MISSING`, `REGISTRY_UNREACHABLE`, `REGISTRY_STALE` (warning), `PLUGINS_UNCONFINED` (status warning, §3.1), `MODEL_DOWNLOAD_FAILED`, `MODEL_MISSING`, `GEN_RATE_LIMITED`, `CLOCK_SUSPECT` |
| 7 | `PLUGIN_PERMISSION_DENIED {perm}`, `PROJECT_NOT_FOUND` and `BRANCH_NOT_ALLOWED` for reach (cp §4.4, agent §5.1), `PLUGIN_REVOKED {advisory}`, `PLUGIN_BUNDLED_MISMATCH {reason: treeHash}` (a bundled tree that does not match the hash built into the app), `REGISTRY_UNTRUSTED`, `TUF_SIGNATURE`, `TUF_THRESHOLD`, `TUF_ROLLBACK {role, trusted, got}`, `TUF_EXPIRED {role}`, `TUF_TARGET_MISMATCH {hash \| length}`, `TUF_DELEGATION`, `PERMISSIONS_NEED_APPROVAL {diff}`, `COPYLEFT_NOT_ACCEPTED`, `THIRD_PARTY_CODE_NOT_ACCEPTED`, `EGRESS_DENIED {origin}`, `EGRESS_NOT_HTTPS`, `EGRESS_PRIVATE_ADDRESS`, `SECRET_MISSING {name}`, `GEN_BUDGET_EXCEEDED`, `GEN_APPROVAL_REQUIRED`; and cp's `HUMAN_ONLY` for installs from agents |

**As built (8A).** The table above is implemented in `crates/fin/src/output.rs` and the console's `exit-codes.ts` (a test pins their parity). Additions to it: `KEY_EXISTS` and `PLUGIN_VERSION_EXISTS`, `DIR_NOT_EMPTY` (4) and `PUBLISHER_NOT_DELEGATED` belong to `fin-plugin` only; `KEY_UNPROTECTED {path, problems}` (3, with a `fix`); `TOO_MANY_PLANS` and `TOO_MANY_SUGGESTIONS` (5); `REGISTRY_AMBIGUOUS` (2) also carries `details.reason: rootChanged`; `HUMAN_ONLY` carries `details.where: console` and `reason: planMaker|approver`; `PLAN_MISMATCH` carries `reason`. `plugin.content.get` also answers `NOT_FOUND` (3, with `available`) for a missing style name or a code plugin. For callers that are not `human`, errors lose `details.path|dir|file` and any value that looks like a path, and a message that names one becomes fixed text with its code kept (`plugins/scrub.ts`; also `registry.list` problems and `registry.refresh` results). The `fix` of a refused `fin plugin install` names the app and the console in words, not a command, so an agent never reads a runnable instruction.

`PLUGIN_MESSAGE_TOO_LARGE` is listed for the message catalogues 8B will read for plugin-facing text; 8A raises `MANIFEST_TOO_LARGE` only, and nothing raises `PLUGIN_MESSAGE_TOO_LARGE` yet (the only catalogue read, the `en` title of a caption style, is bounded by the package's own size caps).

No plugin is refused for lack of a sandbox: a host with Landlock disabled runs plugins unconfined under `PLUGINS_UNCONFINED`, and sub-project 7's `SANDBOX_UNAVAILABLE` is not used for it (owner decision 1 and follow-up decision F2; it exists only for a failure to apply a profile where the mechanism is enabled, agent §8.9). A cross-origin redirect is not an error: `net.fetch` returns the 3xx response with `redirectNotFollowed: true`. Codes go into `crates/fin/src/output.rs` and `@finisher/contracts` in the batch that introduces them. TUF failures are exit 7 (a person must look), not 5.

**As built (8B).** Exit codes are those of `crates/fin/src/output_codes.rs`; `apps/finisherd/src/console/exit-codes.ts` pins their parity, and the plugin-visible ones are also in `fin-plugin-api::protocol::errors::codes`. The codes 8B added, beyond the table above:

| Exit | Codes |
|---|---|
| 3 | `PLUGIN_COMMAND_ARGS {problems}` (arguments outside the command's schema, every problem sorted by path), `PLUGIN_TX_INVALID {problems}` (a transaction fin-core refuses, or one from a command with `writes: false`, `reason: readOnlyCommand`), `EGRESS_TOO_LARGE {what: request \| response \| held, limit}` |
| 4 | `PLUGIN_CRASHED`, `PLUGIN_TIMEOUT {timeoutS}`, `PLUGIN_PROTOCOL {reason \| method}` (an undecodable message, a bad job, a `hello` without the job's kind), `PLUGIN_COMMAND_FAILED {reason?}` (a handler's uncaught failure, and any code of the plugin's own: the code and its details are dropped so no third-party text reaches a record), `PLUGIN_OUTPUT_INVALID {reason}` (**reserved for 8C**: nothing raises it yet) |
| 5 | `PLUGIN_CRASH_LOOP`, `PLUGIN_AUTHORITY_RETIRED {retryable: true}` (a call from a process whose authority key a policy generation retired: run the command again), `EGRESS_TIMEOUT {limitS}`, `EGRESS_RATE_LIMITED {reason, retryAfterMs, retryable}`, `EGRESS_UNREACHABLE {origin, reason}` (DNS, connect or TLS failure) |
| 7 | `PLUGIN_PERMISSION_DENIED {perm, missing: grant \| invoker}`, `EGRESS_DENIED {origin}`, `EGRESS_NOT_HTTPS {scheme}`, `EGRESS_PRIVATE_ADDRESS {origin, reason}`, `SECRET_MISSING {name}` (no secret injection before 8G) |

`PLUGIN_TREE_CHANGED` does not exist (Decision 16). `JOB_TIMEOUT` and `WORKER_HUNG` surface as `PLUGIN_TIMEOUT`, `WORKER_CRASHED` as `PLUGIN_CRASHED`, protocol mismatches as `PLUGIN_PROTOCOL`; `WORKER_OOM` is kept. A plugin error's `fix` is lifted into `FinError.fix` (scrubbed); a plugin's own code never answers with a fix. `PLUGIN_MESSAGE_TOO_LARGE` is still not raised. `PLUGIN_NOT_FOUND` carries `details.available` for an unknown command or plugin. A plugin command that returns an empty transaction is refused `INVALID_OP {field: ops}` by the commit rather than treated as a no-op (open owner question, §24.4).

---

## 14. Cross-platform notes

- **Paths:** Windows reserved names, case-folded duplicates and NFD names are refused in packages on every OS (§6.4); the store is read-only (unix modes; on Windows the read-only attribute and ACLs from the broker).
- **As built (8A):** the trust lock is std's `File::lock` (`flock`; `LockFileEx` over the whole file on Windows, per handle, released when the handle closes). `atomic::write` replaces a file a reader holds open the std way on Windows (share-delete; the reader keeps the old bytes) and is refused, old bytes kept and no temp file left, when a program holds it without share-delete. `keys_inside` resolves `\\?\`, case, `/`, `..`, 8.3 short names, junctions, `subst` drives and UNC aliases of a local folder (also compared by folder identity, `same-file`). Key files and key-ring folders are owner-only from creation on both OSes and refused on load otherwise (§5.6); on Windows a folder or file on **FAT or exFAT** (a NULL DACL) is refused with the NTFS hint. Unpacked store files are read-only (the read-only attribute on Windows). AWS-LC builds with the `cc` builder under MSVC and copies the 26 `prebuilt-nasm` objects, so a Windows build needs neither NASM nor CMake. `resources/**` is `-text` so bundled tree hashes are identical on every checkout. **`fin-plugin registry init` writes `<registry>/.gitattributes` with `* -text`** (unless the folder has one): Git for Windows' default `core.autocrlf=true` otherwise checks the signed JSON out with CRLF and every length and hash fails (`REGISTRY_INVALID {reason: tooLarge}`; found by the e2e `plugins` scenario on Windows 11). A registry made before this needs the file added and committed by hand.
- **`fin-shm`:** `O_NOFOLLOW` + `fstat` on unix; `FILE_FLAG_OPEN_REPARSE_POINT` + `GetFileInformationByHandle` (`nNumberOfLinks`, reparse tag) on Windows; positional reads (`pread`; `ReadFile` with an `OVERLAPPED` offset).
- **Node:** finisherd's runtime is Node 24.22, the Node Electron embeds (cp §1.6, the repository's Node 24 target); 8B checks the permission flags of §3.2 on it, one path per `--allow-fs-*` flag; `ELECTRON_RUN_AS_NODE` is set only for Node plugin children (arch §27.7 bans inheriting it).
- **GPU:** §16.1.

---

## 15. Test strategy

### 15.1 Layers

| Layer | What | Where |
|---|---|---|
| Manifest and packages | grammar and schema property tests; every consumed contribution's validator; refusal of unconsumed kinds; the strict reader against hostile archives built in code | `fin-plugin-api`, `fin-registry` |
| TUF | repositories built with the editor and throwaway keys: valid chain; tampered target; wrong length; rollback of each role; expiry with `relaxed` inside and beyond `maxStaleDays`; mix-and-match; root rotation 1→2→3 with threshold changes, each root saved atomically; timestamp-key rotation with fast-forward recovery; delegation escape; revoked publisher; endless data; advisories; remove-and-re-add keeps trust | `fin-registry` |
| TUF conformance | `tuf-conformance` (Python, CI-only) against a client binary over `fin-registry` with the test-only HTTP transport | CI, Linux |
| Lifecycle | plan → approve → install; non-human plans refused; widening and agent-facing updates need a person; agent-triggered refreshes install nothing; bundled hash verification; disable/remove; advisories | `finisherd` vitest, e2e `plugins` |
| Host and protocol | protocol round-trips; permission × reach matrix per host method (other project, other branch subtree, other library); process keys never share a process across classes or projects; jobs owned by the invoker; `Tx.via` stripped from clients and carried by rebase and pick | 8B |
| `fin-shm` | outputs replaced by symlinks, junctions or hard links; size mismatch; truncation during the copy (no SIGBUS, `PLUGIN_OUTPUT_INVALID`); ring slot messages with a wrong `seq` or index; writes to `in/` refused by the sandbox | 6A (crate), 8B (host use) |
| Plugin escape suite | §16.1 | 8B with 7C |
| Egress | private and loopback addresses (literal, by DNS, by rebinding between resolve and connect), IPv4-mapped IPv6, HTTP, cross-origin redirects with and without credentials, a secret bound to another origin | 8G |
| SDKs | example plugins of each SDK pass their conformance profiles | 8B |
| Analyzers | **A0** pure Rust stages bit-exact by hash on three OSes and wasm32; **A1** ONNX stages on fixtures: identical transcript text across OSes, word times within ±10 ms, embeddings cosine ≥ 0.999, boxes within 2 px; **A2** quality metrics on evaluation sets as trends, the speech gate as a merge gate | 8C–8F |
| Builders | golden ops, inverses, inclusion in the 4B property generators | fin-core |
| Pins | the same op log with pinned ParamSpecs validates identically with and without the pack installed; schema 2 → 3 upgrade goldens | 8H |

**As built (8A).**
- *Manifest and packages:* `fin-plugin-api` unit tests per rule family; `hostile_archive_corpus` (refused archives built byte by byte in code, plus entry, byte, manifest and corrupt-archive cases), `pack_is_reproducible`, `tree_hash_changes_on_any_byte_or_mode`.
- *TUF:* the attack matrix of Task 4b (tampered target, length caps on every fetch, rollback of timestamp, snapshot, targets and delegated roles, mix-and-match, revoked timestamp key, a root without the old threshold, `@acme` listing `other/*`, an oversized timestamp, 32 and 33 new roots, root-walk transport errors), rotations, freshness, and the fixed vectors.
- *TUF conformance* (`scripts/tuf-conformance/`, Linux job `tuf-conformance.yml` with pinned actions): the suite is pinned to tuf-conformance `8535468`; `tuf_conformance_client` is its entrypoint over the `test-http` transport. **Result: 80 passed, 39 expected failures, 0 unexpected** (the xfails are listed with reasons and strict). The **attack matrix** (`attacks.mjs`, the real client against a hostile loopback server) is **23 of 23 as required**: rollback 2, freeze 3, expired 2, mix-and-match 2, key rotation 3, threshold 3, endless data 3, tampered target 2, baseline 3.
- *Lifecycle:* `plugins-lifecycle.test.ts`, `plugins-review.test.ts`, `plugins-bundled.test.ts`, `plugins-content.test.ts`, `plugins-scopes.test.ts`, `console-plugin.test.ts`, `console-safe.test.ts`, `crates/fin/tests/plugin_cmd.rs`, `fin-node` `registry.test.ts`, `crates/fin-node/tests/bundled.rs`, and the e2e `plugins` scenario (13 steps, each with its audit row, and the journal's free-text grep).

**As built (8B).** Plugin processes in tests are the example plugins and a test-only Node probe (`apps/finisherd/test/fixtures/plugins/probe`, speaking the envelope by hand, with ops for echo, marker, command and failure) run through the real `plugin` family. Suites: `plugins-host-family`, `plugins-runtime-lifecycle`, `plugins-authority`, `plugins-rights` (an 80-cell permission and reach matrix over agent and MCP callers; a fast-check property that no human-only scope is ever granted), `plugins-egress` (against a loopback HTTPS server with a per-run certificate and a test-only resolver hook), `plugins-commands`, `plugins-commands-server`, `plugins-dev`, `plugins-sdk-e2e` and `crates/fin/tests/plugin_command.rs` (a real finisherd). The `plugin-code` e2e scenario (`scripts/e2e/plugin-code.mjs`) installs both SDK examples and the probe from a generated-key registry, runs commands with `via`, `fetch-title` through the broker (and `EGRESS_PRIVATE_ADDRESS`), an agent session's reach, authority isolation of memory, temp and data between a person, an agent session and its child and their removal at session end, the crash loop, disable with running and queued work, `PLUGINS_UNCONFINED {enabled}`, and a free-text grep of the journal. Several tests were mutation-checked (the snapshot hold, `storeDirOf`, the stop matcher, the crash window). **Not run yet (Task 19): macOS, Windows, a Landlock-disabled Linux host, and the measured budgets of §15.4 (command round trip p95 ≤ 20 ms over 5C's warm hand-off).**

### 15.2 Fixtures

No media in the repository (media §2.8). Speech fixtures come from the permissive corpora of §15.3, through a script that verifies hashes and licences, into a local cache. Test TUF keys are generated per run; a few fixed vectors use keys whose names say `test-only`.

### 15.3 Evaluation and training data (all permissive; AGENTS.md)

| Use | Sources | Licence | Note |
|---|---|---|---|
| ASR, alignment, boundary training | LibriSpeech; Multilingual LibriSpeech; Common Voice (release, archive hash and accepted download terms pinned in the data card) | CC BY 4.0; CC BY 4.0; CC0 | — |
| Human-labelled boundaries (the gate and rule selection) | clips from the sources above, labelled by people for Finisher | as the source | replaces the generated TTS clips (owner decision 6) |
| The owner's footage | evaluation only, never committed, never trained on | a written licence from the owner to the project (owner decision 6) | — |
| Music under speech | MUSAN music, **filtered per file** to public domain, CC0 and CC BY | per file | owner decision 6 |
| Scenes, reframe, search | Blender Foundation open films (CC BY), the owner's footage as above, with our own labels | CC BY / as above | — |
| Beats | synthetic click and drum tracks; CC BY music from the filtered MUSAN list, labelled by us | ours / CC BY | — |

**Excluded:** generated clips from commercial TTS services (not permissively licensed), GTZAN (copyright unclear), MUSDB18 (NC), BBC Planet Earth shot sets (NC), WIDER FACE (NC), TIMIT and Buckeye (restricted), **VoxCeleb**.

### 15.4 Budgets (validation gates on the reference machines of media §14)

| Area | Budget |
|---|---|
| Registry refresh, local folder with 500 targets | ≤ 300 ms |
| Install of a 50 MiB package after the plan | ≤ 2 s |
| Plugin cold start to `initialize` answered | native ≤ 150 ms + sandbox launch; Node ≤ 400 ms + sandbox launch |
| Command round trip (warm, small snapshot) | ≤ 20 ms p95 |
| Output copy and validation | ≥ 1 GB/s on local SSD |
| Transcription + alignment, CPU | RTF ≤ 0.15 |
| Scenes at 256 px | ≥ 20× real time |
| SigLIP2 embedding, CPU | ≥ 3× real time at one keyframe per 2 s |
| Vector search, exact scan | ≤ 100 ms p95 up to 200k chunks; above that `usearch` (≤ 50 ms p95, recall@10 ≥ 0.95) |
| Reframe (detect every 2nd frame at 480 px + track) | ≥ 2× real time on CPU |

---

## 16. What sub-project 8 needs from sub-projects 5, 6 and 7

### 16.1 X1: the `plugin` sandbox profile (cross-design requirement on sub-project 7)

Agent §8 defines `SandboxPolicy` and its use for agents and hooks, with a worker hook for sub-project 6. **Sub-project 8 requires a `plugin` profile in `fin-sandbox` (7C)**. Registry-installed code plugins do not wait for it (owner decision 1): until it passes the plugin escape suite on an OS they run unconfined there (§3.1), and once it does the host starts every plugin process through it automatically, without re-approval. The profile:
- **Network: none.** Linux: seccomp refuses `socket()` for `AF_INET`, `AF_INET6`, `AF_NETLINK` and `AF_UNIX` (agent §8.3; `socketpair` stays for stdio). macOS: Seatbelt `(deny network*)`. Windows: an AppContainer with **no** `internetClient`, `internetClientServer` or `privateNetworkClientServer` capability, and no loopback exemption.
- **Files:** read roots = the plugin's store directory, the runtime (finisherd's Node, or the Electron framework, for `node` plugins), system libraries and frameworks (`/usr/lib`, `/lib`, `/System/Library`, `%SystemRoot%\System32` and the like), ONNX Runtime's library, the model files the manifest names, and (cold kinds) the job's `in/`; write roots = (cold kinds) the job's `out/`, the pluginData directory of the caller's authority key (§5.1, with `fs pluginData readWrite`) and the key's private temp directory; everything else denied, server data included, and a warm process is granted nothing per call (§3.5). Another authority key's data and temp directories are never in reach.
- **Per-job grants on Windows** through the ACL broker (agent §8.4): one container profile per (plugin, project) (to be verified per authority key: the container's SID must not let one class read another's data or temp directory, plan 8B Task 16); ACL grants on the cold job's directory added before the job and removed after it, write-ahead logged like the broker's session grants; `media:file` grants on a single source file per job. Warm processes get no per-call grant (§3.5).
- **GPU** only with the `gpu` grant: Linux `/dev/dri/renderD*` (never `card*`) and, when CUDA is used, `/dev/nvidia*`; macOS the IOKit user clients Metal needs (the `IOGPU`/`AGXDevice` classes) and nothing else under `iokit-open`; Windows D3D11/D3D12 through the container's graphics access.
- **macOS TCC and Apple Events:** deny `mach-lookup` of `com.apple.tccd`, `com.apple.tccd.system` and `com.apple.coreservices.appleevents`, and `appleevent-send` (§3.2).
- **Limits:** memory, processes and wall clock per §3.3, from a kill-on-close job object or rlimits.
- **Store integrity (follow-up for 7C):** `store/` directories stay writable to the user's account after install (only their files are marked read-only, which the same account can undo), so a package's tree hash is checked only when it is installed (8A Task 8): a re-install of a changed tree replaces it, nothing checks it at start or before a run. The profile makes the store read-only to plugin processes; whether the host also re-hashes a tree before starting it, or keeps the store in a directory only the server can write, is decided with 7C. *Decided (plan 7C Decision 21, owner decision 2026-11-08):* `<server-data>/plugins/store/` is read-only to a plugin (its own package directory only) and denied to the other three profiles; **the host does not re-hash a tree before its first start**. Residual: tampering by the user's own unsandboxed processes is not detected.
- *As built (7C, agent §25.1):* `fin_sandbox::profile::plugin`, applied by `SandboxSpawner` (Linux: a Landlock ruleset built in `prepare`, `pre_exec` steps `RLIMIT_NOFILE`, `PR_SET_NO_NEW_PRIVS`, `landlock_restrict_self`; macOS: the program rewritten to `sandbox-exec -p`, with `tccd`, `tccd.system` and Apple Events lookups denied and no read root in TCC-protected folders; Windows: a container per (plugin, project) with no network capability). Grants are mapped from 5C's and refused when they widen (a server-data read is a model only under `plugins/models/`, a read-write device only `/dev/dri/renderD*`); 8 processes through the leaf's `job_limits`. `fin-registry`'s `thirdPartyCode {confined}` reads `plugin_confined()` from fin-node, never the caller. **Deviations:** no seccomp yet (agent §25.2), so on Linux the network rule is Landlock's TCP rule only (no TCP connect or bind, at ABI ≥ 4: kernel 6.7; nothing on 6.2–6.6) and **UDP, raw, netlink and `AF_UNIX` sockets stay open**; macOS applies no open-files limit and has no form for the `gpu` grant yet; Windows per-job grants through the broker are not wired; warm plugins run cold under a sandbox. The profile applies on an OS only once `plugin` is in `READY` (agent §25.3); until then plugins run as §3.1 says, with `PLUGINS_UNCONFINED`. The escape suite below runs through `pnpm sandbox:suite` (`plugin` rows of `crates/fin-sandbox/tests/cases/*.json`); the Windows rows prove the configuration only (plan 7C Decision 27).
- **A plugin escape suite** in CI on all three OSes: TCP, UDP, raw, netlink and unix-socket connects; reading server data, another plugin's data, another project's plugin data, another authority key's plugin data and temp directory, the project folder, `~/.ssh` and the user's Documents; writing to `in/`; replacing an `out/` file by a symlink, junction or hard link to a host file; truncating a ring; GPU device nodes without `gpu`; `ptrace` of the host; TCC access to Documents, camera and microphone; an Apple Event to Finder; spawning past the process limit. Each passes only with an observed denial.

### 16.2 Everything else

| From | Needs |
|---|---|
| 5A | principals with the reserved `plugin` kind; the scope table (adds `plugin.read|install|admin|suggest`); human-only enforcement; the console; the audit log |
| 5C | `fin-jobs`: kinds with resources, `owner`, `externalRef`, `dedupeKey`, idempotency, retries and bisection; hard kill; `jobs.watch`; owner-scoped budgets (agent §13.1) |
| 5D | the library stage framework keyed by fingerprint and stage version; catalog tables for transcripts (FTS5); library reach for principals |
| 5E | the reserved `pluginSuggestion` review item kind |
| 6 | the one `fin-worker` with a decode role for PCM at 16 kHz mono (6C) and frames at analysis sizes (6A–6B); **`fin-shm`** with the rules of §3.5 (6A); the process codec backend and its conformance suite; matte encoding and, per owner decision 3, `AssetKind::Matte` and the schema bump (6D); the WGSL effect ABI (6D) |
| 7 | X1 (§16.1); branch reach for agent principals (§5.1); command rules matching `plugin:<id>/<cmd>` (§7.2); approvals of kind `plugin` (§7.3); the hook engine calling plugin handlers in the `plugin` layer with `hooks:observe`/`hooks:modify` (§9); `SecretStore` and `ModelGateway` grant handles bound to (plugin, name, origins) (§10); budgets and the ledger (§13) |

**As built (8B).** *Sub-project 5:* the `plugin` family may declare up to 16 warm slots (other families still 1-2); `worker.exit` is `kv` 2 with `plugin?: PluginRef` (cp §6.12's residual that `journalSink` left the `plugin` field out is resolved); `FamilySpec.usable(job)` is checked when a warm job is handed to a ready worker; `FamilySpec.onStderr` and `CallCtx.workerKey` carry the plugin's stderr and key. *Sub-project 7:* 7A-2's `BranchReach.inSubtree` and session tree define agent reach; 7B's rules see a host commit as `PolicyInput.host` and match `plugin:<id>/<command>`. **X1 (§16.1) stays open in 8B:** 7C's `plugin` profile has been built but its `READY` set is filled by 7C Task 16; Task 16 (confinement) owes 7C a grant mapping that accepts the per-project data directory under `<server-data>/plugins/data/` (today it accepts one only outside server data), directory grants per authority key (verify Windows container ACLs), the developer folder of a `@dev` row outside server data, and warm operation under the profile (no per-job grants needed). 7C refuses a spawn only when `plugin` is in `READY` for the OS and the mechanism is unavailable (below kernel 6.2, no `sandbox-exec`); before `READY` holds `plugin` nothing is refused. macOS residuals from 7C: a plugin's open-files limit is not applied under `posix_spawn`, and the `gpu` grant has no macOS form yet.

---

## 17. Crate and file map

| Crate / package | Owner | Responsibility | wasm32 |
|---|---|---|---|
| `fin-plugin-api` | 8A (new) | manifest, permissions, contributions, licence classes, validation; protocol types (8B) | yes, pure |
| `fin-registry` | 8A (new) | TUF client and editor wrapper, filesystem transport (HTTP in test builds), trusted store per root, strict package reader and packer, store and staging, plans, advisories | no |
| `fin-plugin-cli` (`fin-plugin`) | 8A (new) | publisher tooling | no |
| `fin-core` | changed | §3.9 | yes |
| `fin-node` | changed | napi `Registry` on the worker thread; manifest validation; the bundled hash list (generated by `build.rs`); the analysis cache index (8C). **As built (8A):** also `registry/{registries,host,content,lifecycle}.rs`, the font manager's extra catalogs and `fin-fonts`' catalog merge | — |
| `fin-shm` | sub-project 6 (6A) | §3.5 | no |
| `fin-sandbox` | sub-project 7 (7C) | the `plugin` profile (§16.1) | no |
| `fin-plugin-sdk`, `@finisher/plugin-sdk`, `fin-plugin-test` | 8B (new) | SDKs and harness | no |
| `fin-analyze` | 8C (new) | the bundled analyzers binary | no |
| `finisherd` | changed | `src/plugins/` (registries, lifecycle RPCs, plans, bundled, content, host, process keys, reach, `fin-jobs` adapter, command registry, egress broker), `src/analysis/`, console `plugin` and `registry` | — |
| `fin` | changed | `plugin search|info|ls|validate|suggest`, `plugin install` (exit 7), `fin @<id> <cmd>`, analysis commands. **As built (8A):** `plugin search|info|ls|suggest|install|update|rm|enable|disable`, `registry ls|status|refresh|add|rm|forget` and `style add --from @pub/pack:<name>` (`commands/plugin.rs`); **Deviation: no `plugin validate`** (it is `fin-plugin validate`, §5.5); `fin registry …` is new; the human-only ones print the server's `HUMAN_ONLY` unchanged and never retry it | — |

**As built (8A).** `fin-plugin-cli` links `fin-plugin-api`, `fin-registry`, `fin-core` and `fin-fonts` (to check styles and catalogs the host's way); `fin` links `fin-core` and neither `fin-registry` nor `tough`; `fin-plugin-cli` is outside `default-members` and the app bundle. `finisherd` gained `src/plugins/{registries,lifecycle,plans,bundled,content,rpc,scrub,store-sync,tables}.ts`, the `plugins` migration chain, the scope rows and `console/{plugin,registry,plugin-render,safe}.ts`.

**Dependency rules:** `fin-core` and `fin-plugin-api` stay pure; `fin` links neither `fin-registry` nor ONNX; `fin-node` links `fin-registry` but no ONNX; only `fin-analyze` and plugins link ONNX Runtime; every `rustls::ClientConfig` names its provider (§6.6).

**As built (8B).** `crates/fin-plugin-api/src/{protocol/{mod,context,command,calls,errors,rights,rpc}.rs, schema_subset/}`; `crates/fin-plugin-sdk` and `crates/fin-plugin-test`, `examples/plugins/{rust-tools,ts-tools}`, `packages/plugin-sdk`; `crates/fin/src/commands/{plugin_cmd,plugin_args}.rs`; `fin-core` `view::screenplay` (moved from `fin` so finisherd renders the same text), `events/payloads/plugin_runtime.rs`, `tx::Via`; `fin-node` `plugin_calls.rs` and `journal/plugin_ref.rs`. In `apps/finisherd/src/plugins/host/`: `family.ts`, `program.ts`, `node-runtime.ts`, `keys.ts`, `authority.ts`, `processes.ts`, `crash-loop.ts`, `rights.ts`, `reach.ts`, `calls.ts`, `serve.ts`, `log.ts`, `egress*.ts`, `commands.ts`, `command-run.ts`, `command-rpc.ts`, `commit-channel.ts`, `command-journal.ts`, `stats.ts`, `dev.ts`; `jobs/kinds/plugin.command.ts`; `console/dev-plugin.ts`. `scripts/check-crate-deps.mjs` confines `fin-plugin-sdk` to `fin-core`, `fin-jobs`, `fin-plugin-api` and `fin-shm` (optional, behind `files`); `fin` and `fin-node` link neither SDK nor harness; `check-test-hooks` refuses the egress test hook outside tests.

---

## 18. Licences

### 18.1 Dependencies

| Need | Choice | Licence | Batch |
|---|---|---|---|
| TUF | `tough` 0.24 with the `http` feature off (fallback: a fork of its crypto onto `ring`) | MIT OR Apache-2.0 (`aws-lc-rs`/`aws-lc-sys`: a permissive expression, §6.6) | 8A |
| Archives | `zip` 9 (in the tree) | MIT | 8A |
| SPDX, semver | `spdx`, `semver` | `spdx` Apache-2.0 only (**Deviation:** this row said MIT OR Apache-2.0); `semver` MIT OR Apache-2.0 | 8A |
| JSON Schema validation (command arguments) | `jsonschema` | MIT | 8B |
| ONNX | ONNX Runtime (shared library), `ort` 2.0 rc | MIT; MIT OR Apache-2.0 | 8C |
| WGSL validation | `naga` | MIT OR Apache-2.0 | 8H |
| Vector index | exact scan (ours); `usearch` after its check | —; Apache-2.0 (declared) | 8E |

**Rejected or cut:** `tuf` (unmaintained), `gix` (cut with the git transport), `uv` and CPython (cut with Python), LanceDB (not in v1); Ultralytics, aubio, Essentia and whisper-timestamped (copyleft); WhisperX and the original PySceneDetect (Python and ffmpeg dependencies, arch §18).

**As built (8B).** No `jsonschema` crate (Decision 13): command arguments are validated by `fin-plugin-api::schema_subset`, pure and wasm-safe, with no `$ref`, no remote references and no formats. **Deviation:** its `pattern` keyword uses `regex-automata` 0.4.18's meta engine (no lazy DFA, so match memory is fixed; already in the tree through `globset`), not the `regex` crate Decision 13 named, with a compile budget per schema (at most 64 patterns, 64 KiB of automaton each, 2 MiB in all); `\d \w \s \b` and `\p` are refused (spell ASCII classes out). `fin-plugin-api` now depends on `fin-core` (read-only, for `CommandOut::Tx`). The Rust SDK adds `base64` 0.23 (already in the tree). Both SDKs are MIT; no new licence class enters the shipped graph.

### 18.2 Models (v1 defaults)

| Model | Use | Weight licence | Attribution | Training data (provenance note) | Status |
|---|---|---|---|---|---|
| Parakeet-TDT 0.6B v2 | English ASR | CC-BY-4.0 | **required** (notices, model manager, About; ONNX conversion stated) | NVIDIA's published English speech mix | default |
| whisper large-v3-turbo | multilingual ASR | MIT | notice | undisclosed web audio | default |
| wav2vec2-base-960h | CTC alignment | Apache-2.0 | notice | LibriSpeech (CC BY 4.0) | default |
| boundary correction (ours) | starts | MIT | data card | LibriSpeech, MLS (CC BY 4.0), Common Voice (CC0), MUSAN filtered per file | default (retrained in 8C) |
| Silero VAD | VAD | MIT | notice | not fully disclosed by its authors | default |
| YAMNet | speech/music classes | Apache-2.0 | notice | AudioSet: labels CC BY 4.0; the audio is YouTube content under its uploaders' rights | default |
| SigLIP2 base | embeddings | Apache-2.0 | notice | WebLI (not released) | default |
| BlazeFace | faces | Apache-2.0 (model card) | notice | not disclosed in the card | default |
| YOLOX-Nano/Tiny | people | Apache-2.0 (repository; weights unlabelled) | notice | COCO (annotations CC BY 4.0; images under Flickr terms) | default |

**After v1, with their notes recorded now:** pyannote segmentation-3.0 (MIT, gated upstream: Finisher mirrors it pinned by SHA-256; trained on corpora that include licensed ones such as DIHARD); 3D-Speaker CAM++ (Apache-2.0; its released checkpoints are each trained on VoxCeleb, CN-Celeb or the 3D-Speaker set); TransNetV2 (MIT; trained on synthetic transitions built from TRECVID IACC.3 and on ClipShots, both research collections); Beat This! (MIT; its authors note partly copyrighted training audio).

**Excluded:** MMS (CC-BY-NC), CrisperWhisper (NC), RMBG-2.0 (NC), InsightFace weights (NC), madmom models (CC BY-NC-SA), RVM (GPL-3.0), Ultralytics YOLO (AGPL-3.0).

- **Model gate** (bundled and first-party manifests): weights under `MIT`, `Apache-2.0`, `BSD-2-Clause`, `BSD-3-Clause`, `CC0-1.0` or `CC-BY-4.0`, **with no acceptable-use restriction** (OpenRAIL-style use clauses, Llama-style AUPs and NC, ND or SA terms are refused); third-party registries show the licence and flag anything off the list. Training-data notes are shown; the weight licence governs, and models whose authors warn that their training data is copyrighted or licensed are not defaults (owner decision 4).
- `THIRD_PARTY_NOTICES` gains ONNX Runtime, each default model with its attribution, the boundary model's data card, PySceneDetect (BSD-3, ported), `tough` and `aws-lc-rs`.

---

## 19. Decisions taken here

| # | Decision | Section |
|---|---|---|
| 1 | Two kinds: content packs and code plugins | §3.1 |
| 2 | v1 runtimes `native` and `node`; Python and WASM after v1; real-time audio effects built into `fin-audio`, de-noise as offline processors | §3.2, §10.2 |
| 3 | Every plugin process denies TCC and Apple Events on macOS, rather than running in a separate unentitled helper | §3.2 |
| 4 | Long-lived processes keyed by (plugin, project, authority key: principal class + effective-rights hash + policy generation); jobs one process each (amended by Decision 28) | §3.3 |
| 5 | `in/` read-only, `out/` the only write root, **for cold (one-process-per-job) kinds under 5C's job directory; warm processes get no per-call directories**; the host never maps a plugin-writable file; copy, then validate; all in `fin-shm` | §3.5 |
| 6 | Every host call is reach-checked against project, branch subtree and library | §3.6 |
| 7 | Plugin rights are invoker ∩ grants; jobs a plugin starts are owned by the invoker | §3.7 |
| 8 | Plugin command names are `plugin:<id>/<cmd>`; `Tx.via` is set only by the host | §3.8 |
| 9 | One `SCHEMA_VERSION` bump (to 5, after 6B-1's 4) carries pinned plugins, qualified effect kinds and matte assets | §3.9 |
| 10 | A contribution kind is accepted only once a batch consumes it | §4.2 |
| 11 | `gpu` is medium risk; `hooks:observe` with `net` is high | §4.3 |
| 12 | Install is plan → approve by a person; nothing runs at install | §5.3 |
| 13 | Code plugins and agent-facing content never auto-update; agent-triggered refreshes install nothing | §5.3 |
| 14 | Bundled plugins are hash-checked and covered by the app signature, without TUF | §5.2 |
| 15 | Git registries are local folders the user pulls | §5.2 |
| 16 | TUF with per-publisher delegations; `tough` chosen; a fork of its crypto onto `ring` as fallback; a provider named on every `ClientConfig` | §6 |
| 17 | `relaxed` freshness with `maxStaleDays` 7; staleness per plugin; trust kept on registry removal | §6.3 |
| 18 | Bundled analyzers are one Rust binary installed as several plugins | §8.1 |
| 19 | Artifacts are fin-core types keyed by recipe hash; times in integer source ms or frames | §8.1, §8.3 |
| 20 | The boundary rule is re-selected and gated on permissive human labels | §8.3 |
| 21 | Exact-scan vector search first; `usearch` after review; no LanceDB in v1 | §8.6 |
| 22 | Paid calls: key and `submitting` persisted before submit, never re-submitted | §9.2 |
| 23 | Brokered HTTPS only: secrets bound to (plugin, name, origins), private addresses refused after resolution, cross-origin redirects not followed | §9.3 |
| 24 | Plugin effects pinned by three ops, with their ParamSpecs in project state | §10.4 |
| 25 | Only the host emits diagnostics; third-party names are keyed hashes | §11 |
| 26 | Third-party code plugins run before 7C, unconfined, as high risk with an explicit acceptance; the `plugin` profile applies automatically when it lands; no plugin is gated on `SANDBOX_UNAVAILABLE` | §3.1, §5.3, §16.1 |
| 27 | The v1 first-party generation providers are OpenRouter and fal.ai, with curated catalogs | §9.4 |
| 28 | **Decision (2026-11-09, owner):** process memory, per-key temp and persistent plugin state are keyed by effective authority (class + rights hash + policy generation); a generation change retires the key's processes and discards its temp state; the shared per-project `pluginData` directory is for the `person` and `system` classes only, while their rights are identical; agent and MCP callers get `pluginData` per authority key, deleted at session end; an agent caller narrower than its root session runs cold or under its own key | §3.3, §5.1, §12 |
| 29 | **Decision (2026-11-09, owner):** warm plugin processes have no per-call directories; file-bearing kinds run cold under 5C's job directory; a future warm kind needing files uses 7C Decision 19's per-job directory inside the worker's own folder, per authority key; descriptor passing is rejected | §3.5, §16.1 |
| 30 | **Decision (2026-11-09, owner):** plugin effect kinds are always stored qualified as `@publisher/name:kind`; built-in kinds stay bare and reserved (a pack declaring one is refused at validation); two packs may both ship `glow`; pins, ParamSpecs, rendering, kind-keyed checks and unpin checks use the qualified identity; `fin` may accept an unambiguous short name; ships with the schema-5 plugins format | §10.4 |

---

## 20. Owner decisions (2026-11-07)

The owner answered revision 2's seven questions on 2026-11-07. Decisions 1 and 5 differ from revision 2's recommendations, and the body is revised for them; the others are as recommended.

1. **Does 8B wait for 7C?** **No, and third-party code plugins are allowed now, before 7C's sandbox** (the owner's choice; revision 2 recommended keeping registry-installed code plugins off until 7C). 8B builds the host, the protocol, the SDKs and `fin-plugin-test`, and registry-installed code plugins can be installed and enabled from 8B on (§3.1, §5.3):
   - installing a third-party code plugin shows a **high-risk warning** (`thirdPartyCode`, high risk whatever its permissions, §4.3) and needs an **explicit acceptance** by a person (`accept.thirdPartyCode`) on top of approving each permission; nothing installs, enables or widens without a person;
   - **nothing updates automatically**: code plugins never did (§5.3), and every update of a third-party code plugin is a person's plan with the acceptance again;
   - the env allow-list, no credentials, brokered egress and the `in/`/`out/` rules still apply, as host-side rules (§3.1);
   - the `plugin` profile is **enforced automatically once 7C lands** on an OS (§16.1), with no re-approval;
   - **no `SANDBOX_UNAVAILABLE` gating** for registry plugins: no plugin is refused for lack of a sandbox (§13); a host where Landlock is disabled runs plugins unconfined and shows the persistent warning `PLUGINS_UNCONFINED` (`server.status`, `fin status`) with a diagnostics event.
   *Consequence, accepted:* until 7C, a malicious or exploited plugin runs with the user's rights (arch §16's second threat with only Node's permission flags and the host's own rules in the way); the plugin escape suite (§16.1) gates 7C, not 8B.
2. **Do plugins ever see secrets?** **Brokered** (as recommended): the host injects credentials from `ModelGateway` grant handles bound to (plugin, secret name, origins) into HTTPS requests to those origins only, and the plugin never sees a key (§9.3); a raw per-call value exists only behind `secrets` with `mode: raw`, high risk and audited per call.
3. **Do mattes travel with the project?** **Yes, as project assets** (as recommended): `AssetKind::Matte` in `assets/`, named by `Mask.matte`, carried by the `SCHEMA_VERSION` 5 bump (4 → 5) together with `Project.plugins` (§3.9, §10.3). Sub-project 6 (6D) builds mask edits on it.
4. **Which models may ship as defaults?** **The weight licence governs** (as recommended): MIT, Apache-2.0, BSD, CC0 or CC-BY weights, with **no acceptable-use restriction** and no other limit on use; training-data provenance is recorded and shown, and models whose authors warn that their training data is copyrighted or licensed are not defaults (§18.2). The v1 defaults are Parakeet, whisper turbo, wav2vec2, Silero VAD, YAMNet, SigLIP2, BlazeFace and YOLOX.
5. **Which generation providers ship first-party in v1** (arch §26 question 8)? **OpenRouter and fal.ai** (the owner's choice, replacing revision 2's ElevenLabs and OpenAI images), as bundled Node plugins with brokered egress and curated catalogs (§9.4). OpenRouter's key is the one the user pastes for the built-in agent (agent §10); fal.ai needs a second pasted key. *Known v1 gaps:* dubbing and voice cloning, unless the curated catalogs cover them (to be confirmed against the providers' catalogues at 8G start); any modality neither covers.
6. **What evaluation data may Finisher use?** As recommended: (a) **retire the generated TTS clips** from selection and gating, and re-run the rule selection and the gate on human-labelled clips from permissive speech (Common Voice CC0, LibriSpeech and MLS CC BY 4.0) labelled for Finisher (§8.3); (b) **the owner's footage is evaluation-only**, never committed and never trained on, under a written licence from the owner to the project recorded in the data card; (c) **MUSAN filtered per file** to public domain, CC0 and CC BY, the file list in the data card, the AGENTS.md rule read as applying to the files used (§8.3, §15.3).
7. **Who holds the first-party signing keys, and when are they made?** As recommended: **the owner holds three offline, hardware-backed ed25519 root keys, threshold 2 of 3**, made at a recorded **ceremony before the first first-party registry** is published (no later than sub-project 11); the `@finisher` delegation key is held offline by the owner; snapshot and timestamp keys are online in release CI with a short timestamp expiry; development and tests use generated keys only (§6.1).

**Owner decisions for 8A (the build and its plan).**

8. **Third-party code plugins are allowed now, with warnings** (decision 1, as built in 8A): a `kind: code` package from a registry is high risk whatever its permissions, its plan carries `thirdPartyCode {confined: false}` first, and the install needs `accept.thirdPartyCode` (the console asks for the plugin's id typed back) on every install and every update, even one that widens nothing. 8A installs such a plugin disabled only because nothing can run it before 8B; no `SANDBOX_UNAVAILABLE` gate exists (§5.3, §13).
9. **Only humans install.** Plan, approval and the `plugin.install`/`plugin.admin` scopes are human-only: a plan a non-human made can never be approved, not even by a person submitting its id; `local` is not the owner and `fin plugin install` is `HUMAN_ONLY` for `agent` and `local` alike; an agent-triggered refresh installs nothing (§5.3).
10. **Registry folder paths are shown to `human` principals only** (2026-11-08). The first rule showed them to `console`, `local` and any human principal; the review tightened it, so `local` no longer sees them. `agent` and `mcp` never do, and errors for other callers are scrubbed of paths (§3.7, §13).
11. **The Windows key ACL is owner-only, with refusal on load** (2026-11-08): new key folders and files are created owner-only (the current user and `SYSTEM`), nothing inherited, and a ring or key anyone else can reach is refused ssh-style with `KEY_UNPROTECTED` (§5.6). `SYSTEM` is granted because it can read any file already and services that run as it without backup semantics would otherwise fail; Administrators are not granted.
12. **TUF conformance results are accepted with recorded exceptions:** the pinned upstream suite passes 80 cases with 39 strict expected failures (none accepts what the suite says to refuse), and the 23-case attack matrix passes 23 of 23 (§15.1, §23.4). The Linux CI job is defined but has not run on GitHub yet.

---

## 21. Risks

| Risk | Mitigation |
|---|---|
| `aws-lc-sys` needs CMake or NASM, grows the bundle, or a second `rustls` provider panics | 8A Task 1 gate with a real HTTPS fetch on three OSes; a provider named on every `ClientConfig`; fallback: fork `tough`'s crypto onto `ring` |
| `tough`'s `ExpirationEnforcement::Unsafe` skips more than expiry, or it does not save roots atomically or recover by fast-forward | Task 1 verifies each; `fin-registry` wraps root saving and recovery where the crate falls short |
| 7C's `plugin` profile lands late | every code plugin, third-party included, runs unconfined meanwhile (decision 1): high risk and explicit acceptance at install, the host's own rules (§3.1), no auto-updates, automatic confinement when the profile lands |
| An unconfined third-party plugin is malicious | the install warning and acceptance, audited installs, `disable` advisories applied on every refresh, crash-loop disable; residual: it acts with the user's rights until it is disabled |
| A curated provider model changes its terms or disappears | catalog entries name their terms; a model that fails is refused by the provider with `GEN_PROVIDER_ERROR`; the next app release drops or replaces it |
| `fin-shm`'s copy-then-validate costs throughput on big outputs | measured (§15.4); outputs stream into host staging in chunks while being copied |
| `ort` 2 stays a release candidate | exact pin, one wrapper module, our own ORT build per platform |
| Speech models are large (several GB at fp32) | on-demand download per content language; int8 where the gate passes; a shared store |
| ONNX results differ across OSes and providers | CPU default, providers in the recipe, A1 tolerances, cross-OS fixture runs |
| The re-labelled permissive set changes the boundary rule or fails the gate | 8C measures before it ships the rule; the gate stays the merge condition |
| Exact-scan search is too slow on big libraries | `usearch` once reviewed; budgets in §15.4 |
| Process keys multiply processes with many agent sessions | bounded by live sessions (agent §5.5) and the idle stop |
| Brokered egress misses an API shape | raw grants (owner decision 2); the provider conformance profile exercises streaming and uploads |
| Bundled plugins are verified only by compiled-in hashes before sub-project 11 signs releases (8A) | `bundledIntegrity: hashesOnly` is reported; it detects corruption and partial updates, not a user (or malware with the user's rights) who rewrites both the resources and the binary; sub-project 11 adds the code-signature check (§23.4) |
| `tough` accepts the content of a same-version timestamp (8A) | fail-closed (a lower version, a bad signature or a mismatched hash is still refused) but not what the TUF specification says; revisit with any generic-TUF client (§23.4) |
| A key file's ACL on Windows is wider than owner-only (8A) | keys are created owner-only and a reachable ring or key is refused on load; Windows-run tests of the new code are still owed (§23.5) |

---

## 22. Contradictions and gaps found in earlier documents (and in revision 1)

1. **Arch §21.1 `source`** has no `plugin` variant; `ActorKind::Plugin` exists. Resolved by §11: the host emits, and labels with the invoker's actor and `Tx.via`; no new source.
2. **Arch §13 and §18 name whisper.cpp and sherpa-onnx**, while §27.4 chose Parakeet and whisper turbo through ONNX Runtime. Resolved by §8.3; neither ships.
3. **Arch §13's "p95 under 50 ms"** is superseded by §27.4's window gate; §13 and §22 should point to it.
4. **Arch §6.2 lists mattes among regenerable artifacts**, but export needs them on any machine. Resolved by owner decision 3: mattes are project assets (§10.3); arch §6.2 should drop them from its list.
5. **No artifact type exists in fin-core.** Resolved by `ArtifactRef` and `fin-core::analysis` without a model change (§3.9).
6. **Arch §14.4's "per-project lockfile"** has no sync-safe format. Resolved by the pin ops (§10.4).
7. **Arch §15 "Package: signed archive"** vs TUF. Resolved by §6.4.
8. **Arch §15 "git and local registries use the same verification"** omits that TUF timestamps expire within days. Resolved by `relaxed` freshness (§6.3); git registries are folders (§5.2).
9. **Arch §16 "secrets … except per-call `ModelGateway` grants"** hands keys to plugins. Resolved by owner decision 2: brokered by default (§9.3).
10. **Arch §12.7 "audio effect chain … as plugins"** cannot run in the WebAudio preview with process plugins. Decision 2.
11. **Arch §14.1 "code plugins in Rust, Node, or Python"**: Python is cut from v1; Node under Electron needs `ELECTRON_RUN_AS_NODE` set explicitly for its children, which arch §27.7 forbids only when inherited (§3.2).
12. **Core §5.2's `&'static str` ParamSpecs** cannot hold plugin params. Resolved by `Cow<'static, str>` and pinned specs (§3.9).
13. **Two worker binaries** (cp's `fin-workers`, media's `fin-worker`) and two job systems. Resolved by the coordinator's ownership table (§1.6): one `fin-worker`, and `fin-jobs` from sub-project 5.
14. **Arch §13 does not cover CC-BY attribution in the UI.** Resolved by §18.2.
15. **Arch §27.4's reference set** includes generated TTS clips that are not permissively licensed, and the end-boundary rule was chosen on it. Resolved by owner decision 6: retired, rule re-selected on permissive human labels (§8.3).
16. **Arch §8.3's agent plugin suggestions** had no producer. Resolved by `plugin.suggest` (§5.3).
17. **Arch §13 names LanceDB**; v1 starts with an exact scan (§8.6).
18. **The 4C plan's registry-supplied font catalogs**: resolved by `fontCatalogs` (§4.2).
19. **D29's opt-in cloud embeddings** are cut from v1 with `embedders` (§1.3); D29 should say "after v1".
20. **Arch §8.4's sandbox** and agent §8 define agent, hook and worker policies but no `plugin` profile. X1 (§16.1).
21. **Agent §10's grant handles** carry no origin binding; §9.3 binds them to (plugin, name, origins) and asks sub-project 7 to add the field.
22. **Revision 1** of this design proposed a git transport, Python, WASM as an owner question, TUF for bundled plugins, `tuf` as the fallback, a 30-day staleness default, per-plugin rather than per-key processes and memory-mapped plugin outputs; revision 2 replaces each as the review asked (§1.3, §3.3, §3.5, §6).
23. **Arch §16's sandbox wording** reads as if every plugin is OS-confined; before 7C none is (owner decision 1, §3.1), and arch §16 should say so.
24. **Arch §26 question 8 (generation providers)** is answered by owner decision 5 (OpenRouter and fal.ai, §9.4).

25. **Sub-project 5C-1 against this section's JSON-RPC stream (found in 8B planning, resolved):** §3.4 specified a separate JSON-RPC 2.0 stream and `initialize` handshake while 5C-1 Decision 44 made plugins workers of the `fin-jobs` envelope. The envelope won: §3.4 is rewritten to the one wire, with every JSON-RPC method mapped to a job kind or a host call (§24.1, Decision 2).

---

## 23. Sub-batch 8A, as built (2026-11-08)

8A is built and merged into the integration branch. This section maps the plan's decisions, records the review fixes and owner decisions taken while building it, the results measured, and what is left. The numbers in §23.1 are the **plan's** Decision numbers (`docs/superpowers/plans/2026-11-07-sub-project-8a-packages-and-registries.md`), not §19's. **As built** = built as decided, with the detail in the section named; **Deviation** = built differently, with the reason in that section. Nothing here is a claim about 8B–8H.

### 23.1 The plan's decisions, as built

| # | Decision | Mark | Section |
|---|---|---|---|
| 1 | Manifest file `finisher-plugin.json`, at most 256 KiB, UTF-8 JSON | As built (`MANIFEST_TOO_LARGE`, exit 2) | §4.1 |
| 2 | Id grammar, reserved publishers outside bundled sources, `@dev` only in developer mode | As built; Windows device names are also refused in ids (Task 8 review) | §4.1 |
| 3 | Semver 2.0 without build metadata | As built | §4.1 |
| 4 | Consumable kinds in 8A: `fontCatalogs` and `captionStyles` only | As built; a content pack must contribute something | §4.2 |
| 5 | Risks over the whole permission set; third-party code is high risk in the plan | As built (the source-dependent rule lives in `fin-registry::plan`) | §4.3 |
| 6 | Licence classes | As built; `OFL-1.1` counts as permissive; **Deviation:** `spdx` is Apache-2.0 only | §4.3, §18.1 |
| 7 | Packages are reproducible zip files | As built; hash equality across the three OSes is partly verified (Windows re-packs the fixed vectors to the committed bytes), the rest is owed | §6.4, §23.5 |
| 8 | Task 1 decides the crypto path | As built: `tough` on `aws-lc-rs`, no fork; **Deviation:** the 6 MiB limit holds on the stripped `.node` (Linux unstripped +6.62 MiB) | §6.6 |
| 9 | ed25519 for every role; thresholds | As built; no first-party registry exists, so decision 7's ceremony is still ahead | §6.1 |
| 10 | Behaviour checks of `tough` | As built (17 of 17); six gaps covered in `fin-registry::tuf` | §6.2, §6.6 |
| 11 | Consistent snapshots, hash-prefixed names, `<n>.root.json` | As built; targets are fetched by us under the TUF 1.0 name; **Deviation:** registries without consistent snapshots are refused | §6.1 |
| 12 | `relaxed` freshness, `maxStaleDays` 7 (at most 90) from the signed root | As built; every trusted role counts, publisher roles included | §6.3 |
| 13 | Clock check, `CLOCK_SUSPECT` | As built; `lastRefreshAt` is capped at the newest timestamp's expiry and `reset-clock` clears it (review) | §6.3 |
| 14 | Trust per root, kept on removal | As built; `forget` refuses a root still trusted | §6.3 |
| 15 | Size caps | As built; `MAX_ROOT_UPDATES` is 33 so one refresh trusts at most 32 new roots | §6.2 |
| 16 | A pinned or typed-back root at add time | As built; **Deviation:** the console asks for the pasted hash before it describes the root (review) | §5.2 |
| 17 | Git registries are folders | As built; nothing runs git | §5.2 |
| 18 | Plans in `plugin_plans` and `staging/`, 10 minutes, digest = BLAKE3 | As built, with additions: `base`, the maker, `rootSha256`, at most 4 open plans per principal, non-human plans stored `unusable` | §5.1, §5.4 |
| 19 | Approval binds the exact plan; human plan maker and approver; acceptances | As built (`HUMAN_ONLY {reason: planMaker|approver}`) | §5.3 |
| 20 | No agent installs; `local` is not the owner | As built; **Deviation:** `fin plugin install|update` plans first, so a plan's own refusal precedes `HUMAN_ONLY` | §5.3, §17 |
| 21 | Code plugins install disabled; no sandbox gate | As built (`PLUGIN_RUNTIME_UNAVAILABLE` until 8B); `confined` is `false` until 7C | §5.3 |
| 22 | Content packs enable at install; catalog merge by id; styles copied by `style add --from` | As built; **Deviation:** the merge also refuses reused files, faces and family names (H1), at plan time | §4.2 |
| 23 | Automatic updates | As built; the agent-facing flag is test-only because no such kind is consumable; a plan's own refresh never updates | §5.3 |
| 24 | Bundled plugins without TUF | As built; `@finisher/captions-basic` is the one pack | §5.2 |
| 25 | Unsigned builds report `bundledIntegrity: hashesOnly` | As built; `signed` waits for sub-project 11 | §5.2, §23.4 |
| 26 | Remove refuses bundled plugins; pinned fonts stay cached | As built | §5.3 |
| 27 | `plugin.lifecycle` and `registry.refresh` events | As built; `unknown` kind and risk; `plugin` optional | §11 |
| 28 | Audit kinds and a closed `details` | As built; adds `rootSha256`, `auto`, and `planId`/`pluginId` on failures | §11 |
| 29 | Scope rows | As built; `plugin.plan {allowDowngrade}` escalates; human-only payloads carry `challenge`; registry paths for `human` only | §3.7 |

Items the plan listed that 8A did **not** build: the packaging-list gate that excludes `fin-plugin` from the app bundle (no packaging list exists yet; sub-project 11) and `finisherd --plugin-dir` (8B, §5.5).

### 23.2 Review fixes

Each is a `fix(…)` commit with tests that fail without it. `TASKS.md` has the full lists; these are the durable ones.

**Tasks 1–7** (TUF, packages, tooling, registries).
- A trust generation is numbered past every `gen-*` on disk and opening a store deletes generations `current` does not name, so a crash between the rename and `current` no longer wedges refreshes.
- A refresh keeps the verified advisories in the generation (`kept/<target>`); `Registry` reads them from there, and a missing, mismatched or unparseable copy is `REGISTRY_INVALID`.
- An exclusive lock per root store for a whole refresh and rotation; `lastRefreshAt` capped at the newest timestamp's expiry, and `reset-clock` (human-only) clears only it.
- `permissions_widen()` counts a changed `fs` scope; `requires_reapproval` for 8B.
- Key files 0600 and key folders 0700; `keys_inside` resolves `..`, symlinks, missing folders and case folding; target names, top-level ones too, must be plain package paths; NTFS 8.3 short names are refused in packages; transport components refuse `\`, `:` and control characters; `publish` reads the package once.

**Tasks 8 and 11** (plan, store, events).
- An installed directory is always whole (re-hash on commit, rename before delete, sweep of `.tmp-*`); the staged copy of a bundled tree is re-hashed against the compiled-in hash.
- `PlanRequest.version` (a package of another version is `versionMismatch`; a downgrade needs the version as well as `allowDowngrade`).
- A commit of a plan that is gone journals a refused `install` without a `plugin`; `PluginId` refuses Windows device names.
- Events gain `unknown` for a plan refused before its manifest parses; Task 9b took over three items from this review: M2 (installed state re-checked at approval), M3 and M4 (the plan maker stored and enforced).

**Task 9b** (lifecycle).
- **M1 root identity on rows:** installed rows carry the trust root they were verified under (`plugins.rootSha256`, `plugin_plans.rootSha256`). An automatic update skips, and a revocation does not apply to, a row whose registry id now has another root; a manual update then needs `registry` named (`REGISTRY_AMBIGUOUS {reason: rootChanged}`) and carries a `registryChanged` warning; an approval whose registry root changed since the plan is `PLAN_MISMATCH {reason: registry}`.
- **M2 non-human plans keep no staging:** a non-human principal's plan is returned, stored `unusable` and its staging deleted at once; at most 4 open plans per principal; `plugin.suggest` is idempotent per (principal, project, plugin) and capped at 20.
- **M3** the install re-reads the trusted advisories for the placed version under the lock, so a `disable` advisory that landed after the plan installs it revoked.
- **L1** registry paths and locations go to `human` principals only (`local` no longer sees them). **L2** `plugins/scrub.ts` removes paths from errors for other callers. **L3** purge on a gone folder. **L4** `store-sync.ts` repairs the store at start. **L5** audit `auto: true` and ids on failed installs. **L6** rollback, interleaving, scrubbing, registry id reuse and the advisory-between-plan-and-install case are tested.

**Tasks 10 and 12a–12c** (bundled packs, consumers, console, `fin`).
- **H1 font family shadow refusal:** `FontCatalog::merged` refuses a pack entry or face family equal to one the catalog (or an earlier pack) has, case-folded (`find_face` matches face families first, so a pack face named `Inter` beat the built-in `Inter Variable`). `first_unmergeable`, the plan and `fin-plugin validate` inherit it.
- **M1 `safe()` render sanitising:** `console/safe.ts` strips C0/C1 controls and bidi overrides from every registry, publisher and package text the console prints, prompts included; advisories are capped (500 characters) and rejected on control characters when parsed from signed metadata and when signed.
- **M2** `installBundled` plans a rebuilt same-version pack first and deletes the old copy after; `revokeBundled` changes `state` only, and `restoreBundled` puts back what the owner had (an owner-disabled pack stays off and writes no audit row); a rebuild keeps `autoUpdate`.
- **M3 `plugin.discard {planId}`** (`plugin.read`; the maker only, anyone else and a missing plan get `PLAN_EXPIRED`): the console calls it on every exit after a plan that is not installed, so a cancelled plan frees its slot at once.
- **L1 pasting the publisher hash first:** `registry add` without `--root` asks for the publisher's SHA-256 before it describes the folder's root; a `--root` file inside `--dir` draws a warning. **L2** `Registry.packStyle` parses only the style it serves and `plugin.content.get` caches served styles per store folder. **L3** `PluginContentGetSuccess` is a real schema. **L4** the plan-time merge counting disabled and revoked packs is documented and tested.

### 23.3 Owner decisions

§20, items 8–12: third-party code is allowed now, with warnings; only humans install; registry paths are shown to `human` principals only (`local` excluded); the Windows key ACL is owner-only, with refusal on load; the TUF conformance results (80 passed, 39 expected failures; attack matrix 23 of 23) are accepted with the recorded exceptions.

### 23.4 Measured results, residuals and follow-ups

**Verified.** On Linux x64 at the integration merge: the workspace and package tests, clippy with warnings denied (also for the Windows GNU target on the crates that carry platform code), fmt, `pnpm gates`, the six e2e scenarios, the attack matrix, and the dependency-age checks. The Task 1 gate ran on macOS arm64, Linux x64 and Windows 11 x64 (§6.6). The conformance run and attack matrix are in §15.1.

**Residuals.**
- **Bundled integrity is `hashesOnly`.** No signed release exists before sub-project 11; until it adds the code-signature check over the bundled resources, the compiled-in hashes detect corruption and partial updates but not someone who rewrites both the resources and the binary. `bundledIntegrity()` in `plugins/bundled.ts` is the one place to change; packaged apps must set `FIN_BUNDLED_PLUGINS_DIR` to their resources folder.
- **`tough` takes the content of a same-version timestamp** (`test_timestamp_content_changes`): fail-closed, but not what the TUF specification says. Revisit if a generic-TUF client ships.
- **39 expected conformance failures**, of which 17 (delegation graphs) and 10 (partial persistence) are design differences: generic layouts are not supported and trusted generations are written whole. None accepts what the suite says to refuse. Nested delegations are neither stored nor searched, and non-consistent-snapshot repositories are refused.
- **The Windows key file inheritance is fixed**: the Windows run found that nothing set an ACL explicitly, so keys inherited the folder's. Under the profile or `%TEMP%` that is the owner, `SYSTEM` and Administrators, but in a folder at the root of a data drive it adds Authenticated Users (modify) and Users (read), so any local user could read and replace keys. New folders and files are now created owner-only and loading refuses otherwise (§5.6). **The FAT/exFAT refusal** (a NULL DACL) is implemented and still unrun. macOS ACLs are not checked (modes only, as ssh).
- **Cross-platform validation is owed on macOS** (Task 15, §23.5; Windows ran on 2026-11-08), as is the first GitHub run of the conformance job (it installs `faketime` and a recent Python there).
- A revoked version stays revoked even if its advisory is later withdrawn: remove it and install again.
- `fin-node`'s size budget is stated on the stripped `.node` once it links `fin-registry`.
- Store integrity is checked at install only (7C decides whether the host re-hashes before a run, §16.1).
- `PLUGIN_MESSAGE_TOO_LARGE` is not raised yet.

**Follow-ups, by owner.**
- *Sub-project 11:* the code-signature check (`bundledIntegrity: signed`); the packaging-list gate that keeps `fin-plugin` out of the app; the first-party registry and the key ceremony of owner decision 7.
- *Sub-project 5E:* move `plugin_suggestions` rows into `pluginSuggestion` review items (cp §8.1) and drop the table in a later `plugins` migration.
- *8B:* replace `PLUGIN_RUNTIME_UNAVAILABLE` with the runtimes; use `requires_reapproval`; load `@dev` plugins (`--plugin-dir`); `confined` reads 7C's profile status once it lands; consume agent-facing kinds only when 7B, 7G or 10 do (the no-auto-update rule is already in the lifecycle).
- *Publishers' tooling:* `fin-plugin test` (8B, §7.4).

### 23.5 Cross-platform validation (8A Task 15)

**Done.**
- *Windows 11 x64* (Tasks 1–8 and 11 as merged, 2026-11-08): `fin-plugin-api`, `fin-registry` and `fin-plugin-cli` tests with `test-http` pass (the Windows-only tests replace the Unix ones), the `fin-node` registry test passes, clippy with warnings denied is clean, and the concurrency and crash-recovery tests pass repeatedly. Checked: AWS-LC builds with the `cc` builder under MSVC and copies all 26 `prebuilt-nasm` objects; the trust lock is `LockFileEx` per handle; `atomic::write` follows the share-delete rules (§14); generation recovery passes; re-packing the fixed vectors gives the committed bytes. **Fixed:** a UNC alias of a local folder was not seen as inside the registry (now compared by folder identity); a missing drive lost its letter in `keys_inside` (`Q:\x` equalled `R:\x`); an `atomic.rs` clippy failure; `feature_off_in_default_members` used `cargo tree --offline`, which fails before a host has fetched every crate (now `--locked`); `fin-workers`' test family needed `Win32_System_IO`.
- *Linux x64:* everything in §23.4 at the integration merge, including the e2e `plugins` scenario.

- *Windows 11 x64, the integrated branch* (2026-11-08, `3897add` plus `xwin-fixes`, non-elevated, Defender on): the full test suite, every `pnpm e2e` scenario (`plugins` included) and clippy pass. The Windows-only key-ACL tests now ran and pass (`private::imp::tests::*`, `tests/key_permissions.rs` `windows::*`), as do `keys_inside` with `\\?\`, case, 8.3 short names and junctions (`editor::paths::windows_tests`; UNC aliases by folder identity), the share-delete rules of `atomic::write`, the trust lock (`LockFileEx`) and `pack_is_reproducible`. **`fin-plugin pack` gives the same bytes on Windows and Linux** for one tree (`captions-basic` renamed to `@xwin/`: `.finpkg` SHA-256 `18113392…320d`, tree hash `969be5a3…a57f`). **Fixed:** a git-hosted registry broke on Windows under `core.autocrlf=true` (`init` writes `.gitattributes`, §14); the lifecycle leak check looked for raw paths in JSON text, where Windows paths are escaped, so its non-human assertions were vacuous there (`plugins-review.test.ts`); `plugin_cmd` built a file URL from a verbatim `\\?\` path.

**Owed (needs the hosts; paths come from the environment, never from the repository).**
- macOS arm64: the full test suite and `pnpm e2e` (the `plugins` scenario) on the integrated branch.
- `fin-plugin pack` hash equality on macOS (Windows and Linux match).
- A registry on a network share and on a synced folder.
- Windows-reserved package names refused and a bundled tree edited after the build ending `revoked` on macOS (on Windows: `store_files_are_read_only`, `reserved_names_and_trailing_dots` and the e2e `plugins` step 12 pass; Linux too).
- A key ring on a FAT/exFAT stick refused with the NTFS hint (no such volume on the Windows host).

Record the results here and in `TASKS.md` under 8A Task 15.


---

## 24. Sub-batch 8B, as built (2026-11-12)

8B (the plugin runtime: code plugins run, commands commit with `Tx.via`, the host API, brokered egress, the SDKs and developer mode) is built on branch `sp8b`, except three items that wait on other work (Tasks 14, 16 and 17) and the cross-platform validation (Task 19), which has not run. This section maps the plan's decisions, records the review fixes and owner decisions taken while building it, the results and the residuals. The numbers in §24.1 are the **plan's** Decision numbers (`docs/superpowers/plans/2026-11-08-sub-project-8b-plugin-runtime.md`), not §19's. **As built** = built as decided, with the detail in the section named; **Deviation** = built differently, with the reason there. Nothing here is a claim about 8C-8H.

### 24.1 The plan's decisions, as built

| # | Decision | Mark | Section |
|---|---|---|---|
| 1 | Owner decisions applied: third-party code plugins enableable before 7C, unconfined, with 8A's warning | As built; confinement waits for Task 16 | §3.1 |
| 2 | One wire: the `fin-jobs` envelope, not a JSON-RPC stream | As built; §3.4 rewritten | §3.4, §22 item 25 |
| 3 | One `plugin` family; `warmKey`; 16 warm slots; `worker.exit` names the job's plugin | As built; **Deviation:** the version is in the key, and the key's third part is the authority key (Decision 4) | §3.3 |
| 4 | Principal classes `person`, `agent:<root>`, `system`; calls only to their own key's process | As built, **amended** (owner "hybrid", §24.3): the key carries class, rights hash and policy generation | §3.3, §5.1 |
| 5 | Long-lived work is warm and exchanges data only through calls and results; no per-call directories | As built (Decision 29); **Deviation:** no `plugins/run/` tree, 1 MiB message cap, not 16 MiB | §3.5, §5.1 |
| 6 | Plugin kinds are never submitted by RPC | As built; `job.submit` host call is 8C's | §3.4 |
| 7 | A command run is a `plugin.command` v1 job (warm, interactive, `timeoutS` 60, `maxAttempts` 1) | As built; the command's own timeout counts from submit; the 20 ms p95 round-trip budget is **not measured yet (Task 19)** | §3.8, §15.1 |
| 8 | `JobRights` = invoker ∩ grants, re-checked on every call | As built | §3.7 |
| 9 | Process limits before 7C | As built; enforcement caveats in §3.2 | §3.2 |
| 10 | Egress is brokered HTTPS without credentials; `ModelGateway` binding is 8G's | As built; **Deviation:** eight egress codes, not four | §9.3, §13 |
| 11 | Bodies held by the host, read in pages | As built; pages are 768,000 bytes | §9.3 |
| 12 | `Tx.via`, stripped from client commits | As built; `Option<Box<Via>>` | §3.8 |
| 13 | Command arguments: a restricted JSON Schema subset, our own pure code | As built; **Deviation:** `regex-automata`, not `regex`; no `jsonschema` | §4.2, §18.1 |
| 14 | Host API 1.1.0 (`commands`); `hooks` waits | As built; `hooks` typed and refused by presence | §3.6, §4.2 |
| 15 | Crash loop: three crashes in ten minutes | As built; counted from job ends | §3.3 |
| 16 | No store tree re-hash before 7C; no `PLUGIN_TREE_CHANGED` | As built; residual in §24.4 | §3.3, §13 |
| 17 | `PLUGINS_UNCONFINED` on every host while an enabled code plugin runs unconfined | As built: `details.enabled` counts enabled, active code plugins; `reason` is `profileNotReady`, `landlockDisabled` or `spawnerUnconfined` (the profile could be enforced but the spawner does not apply it); the warning is absent while none is enabled | §3.1 |
| 18 | Manifests that declare `models` cannot be enabled before 8C | As built | §3.1, §5.3 |
| 19 | Developer mode (`finisherd --plugin-dir`) | As built; **Deviation:** a non-`@dev` id is refused, not renamed; content packs are refused | §5.5 |
| 20 | Hooks wait for 7F; 8B may merge without them | As built: Task 17 is open, `hooks` is unconsumable, the host API stays 1.1 | §3.6 |
| 21 | The SDKs implement the envelope's plugin rules | As built in both SDKs | §7 |
| 22 | `worker.exit` `kv` 2 with `plugin?: PluginRef`; `plugin.exit` | As built; hashed in `fin-node` | §11 |

Plan tasks not built: Task 16 (confinement, waits for 7C), Task 17 (hook handlers, waits for 7F). Task 19 (cross-platform validation) has not run.

### 24.2 Review fixes

Each is a `fix(…)` commit with tests that fail without it; `TASKS.md` has the full lists.

**Processes and lifecycle (Tasks 4-5 and the authority keying).**
- `FamilySpec.usable(job)` is checked when a warm job is handed to a ready worker: a plugin disabled, revoked or crash-looped while its process idled fails the job with the family's error and the worker is retired without seeing it. `JOB_TIMEOUT` maps to `PLUGIN_TIMEOUT` too. A warm spawn's `TMPDIR` comes from the key the engine tagged the worker with.
- The authority drain: one drain per class whose target moves with every bump, never removing a newer generation's data (B1); the start sweep of authority data runs at `bind`, after generations are raised to the queued jobs' and the highest on disk; a call from a process whose key is unknown fails closed `PLUGIN_AUTHORITY_RETIRED`; a drain that times out removes nothing; `close` interrupts drains. `bareKey` names a confined pool's temp directory by the bare key.

**Calls and egress (Tasks 7-8).**
- A crash's `stderrTail` is scrubbed; stderr lines are counted against the window before they are scrubbed; `scrubLogText` cuts by code point in one pass.
- Egress: at most 4 requests of a plugin in flight and 64 overall, 16 name lookups at once; body bytes are charged as they arrive against the job's 256 MiB and a server-wide 512 MiB; a job's end aborts its requests; the address check refuses every IANA special-purpose block that is not globally reachable; the private address goes to the server log and not to the plugin; header names must be tokens (no `__proto__`) and values free of CR, LF and NUL; a refused connection tries the next checked address.

**Commands (Tasks 9-10).**
- Each run holds its own snapshot under a handle in `CommandRun.snapshot`, so a rebuild or rewind that reaches the same rev in a newer history never serves one run another's pin (`HISTORY_REWOUND` for the run from before). Hold and job are acquired and released together: the job is cancelled on any failure or interrupt, the hold always released. A read-only command's transaction is refused before fin-core applies it. The chain hands the host's commit to the policy stage as `PolicyInput.host` (never from the payload). **`plugin.commands` needs `project.read`**, not `plugin.read`.
- `fin`: a run waits on a connection of its own for `timeoutS` + 15 s; `outcome: conflict` is `CONFLICT` (exit 4); fin's own flags stay fin's anywhere on the line and a property whose flag would collide has `--args-json` only; a writing run whose answer was lost says it may have run; help falls back to the cache on `SERVER_TIMEOUT` too; `--help` after the id is `fin help @<id>`; a connection that never opened is answered as a failure to run, not a lost write; a held snapshot's handle is random.

**Diagnostics (Task 15).** `mapPluginError` keeps only host-known codes (a plugin's own becomes `PLUGIN_COMMAND_FAILED` with no details); `plugin.exit` only for worker-role exits; `commandOutcome` maps `JOB_CANCELLED`, `JOB_QUEUE_FULL`, `WORKER_UNAVAILABLE` and `PLUGIN_RUNTIME_UNAVAILABLE` to `failed`; host-call counting never throws on sizes; `worker.exit`'s lifetime is whole seconds (the journal refused a float).

**SDKs and tests.** Both SDKs share one error contract (§7), pinned by tests on both; a plugin error's `fix` is carried to the caller; the SDK examples run through the real command path.

### 24.3 Owner decisions

- **Hybrid authority keying (2026-11-09; Decisions 28-29, from an external review's findings 2 and 3).** A plugin process, its temp state and `pluginData` are keyed by effective authority (class, rights hash, policy generation), not by principal class alone. The shared per-project data directory is for `person` and `system` only while every member of a class holds identical plugin-relevant rights; an agent caller narrower than its root session (a child session) runs under its own key, never in the root's warm process; agent and MCP callers get per-authority directories deleted at session end. Warm plugin processes have no per-call directories (§3.5; the stale text in §3.2, §3.5, §5.1 and §16.1 is corrected).
- Coordinator decisions of 2026-11-08 (no owner decision needed): plugin kinds are never submitted by RPC (6); `net.fetch` with every network rule and no secret injection (10); no store re-hash before 7C (16); `PLUGINS_UNCONFINED` on every host while an enabled code plugin runs unconfined (17); 8B may merge without hook handlers (20).
- Owner decisions 1, 2, 4, 5 and 7 (§20) bear on 8B as in the plan's table: nothing is refused for lack of a sandbox; no key reaches a plugin; no model is fetched or opened; providers and the registry's keys are later sub-projects.

### 24.4 Measured results, residuals and follow-ups

**Verified.** On Linux x64: the unit, integration and e2e suites of the tasks above (the `plugin-code` scenario included) and the repository gates; see `TASKS.md` for the commands and counts. **Not measured yet (Task 19):** the command round trip (p95 ≤ 20 ms over 5C's warm hand-off), process start cost, memory per warm process, the egress broker's throughput, and every result on macOS arm64 and Windows 11 x64 and on a Landlock-disabled Linux host.

**Open owner question.** A plugin command that returns an empty transaction (for example `trim-silence` with no silence markers) is refused `INVALID_OP {field: ops}` by the commit, not treated as a no-op. Should the host answer `{outcome: data}` with no ops instead?

**Residuals.**
- **No store re-hash before a run (Decision 16).** Until 7C's `plugin` profile is enforced on an OS, a user-level process can alter an installed plugin's files after install and nothing detects it; the install-time hash and 8A's update flow are the only checks.
- **Unconfined processes.** A `node` plugin can open sockets (Node 24.22's permission model has no network control); every plugin runs as the user's account. `PLUGINS_UNCONFINED` says so.
- **The `local` principal can run dev plugins** (the agent's shell before 7C); they are hidden from `agent` and `mcp` principals only. A program driving a pseudo-terminal passes the console's terminal check. A dev plugin's data directory persists across runs.
- Credentials are never narrowed in place, so the generation bump is shown by a credential's end; there is no narrowing hook. Session re-parenting has no generation hook (7A-2 cannot re-parent a session today; if it ever can, a re-parent must bump both trees' classes).
- A warm process that dies while idle is not counted toward a crash loop. The old version's warm process serves jobs of its own key after an update until the update drains it.
- Egress: bodies are held in memory (256 MiB a job, 512 MiB server-wide); `content-encoding` is passed through undecoded; a plugin's own `Authorization` or `Cookie` header is sent as given.
- Command and property names reach `fin`'s usage lines and errors as the manifest wrote them; the server should token-check property names before they reach a terminal.
- `PLUGIN_MESSAGE_TOO_LARGE` is not raised; `PLUGIN_OUTPUT_INVALID` is reserved for 8C's cold jobs.
- The `plugin-code` scenario does not type the plugin id back (the console's terminal prompt); it installs through the console's RPCs with the challenge code, as 8A's `plugins` scenario does.

**Follow-ups, by owner.**
- *7C (Task 16, confinement):* turn the `plugin` profile on automatically once it is in `READY` for an OS (fails closed); accept `<server-data>/plugins/data/` and the authority-keyed directories in the grant mapping; verify Windows container ACLs; the developer folder of a `@dev` row is outside server data; `plugin.exit {reason: sandbox}` (unused until then); the e2e scenario's enforced-profile steps (sibling authority reads denied).
- *7F (Task 17, hooks):* hook handlers (`plugin.hook` v1), `hooks` consumable at host API 1.2.
- *8C:* the `job.submit` host call, the model manager and the `models` licence gate (then lift Decision 18), analysis host methods, cold kinds with `in/` and `out/` and the first raiser of `PLUGIN_OUTPUT_INVALID`, models under `<server-data>/plugins/models/`.
- *8G:* secret injection and `ModelGateway` grant binding in the broker (`SECRET_MISSING` goes away), `secret.grant`.
- *Task 14 (built late in 8B):* `fin-plugin-test` and `fin-plugin test` (`command` profile, §7.4); its first macOS and Windows runs are CI's.
- *Task 19:* cross-platform validation and the budgets of §15.4.
- *Sub-project 11:* publication of the SDKs to crates.io and npm (both are MIT and unpublished now), and the packaging gate that keeps the harness out of the app.
- *7B:* read the dynamic `plugin:<id>/<command>` entry's identity per entry, not per RPC (it also stands for its `tx.commit`).