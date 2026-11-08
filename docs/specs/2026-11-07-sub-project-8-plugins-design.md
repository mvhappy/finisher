# Sub-project 8: Plugins and registry — Design

- **Status:** draft for review, revision 3 (2026-11-07). Revision 2 answers the independent review of revision 1 (`848f01f`): the confused-deputy, shared-memory, sandbox-profile and TUF blockers, the high and medium findings, the v1 cuts and the coordinator's ownership decisions (§1.3, §1.6, §22). Revision 3 records the owner's decisions of 2026-11-07 (§20) and revises the body where they differ from revision 2's recommendations: third-party code plugins run before 7C (§3.1, §5.3, §16.1), and the v1 generation providers are OpenRouter and fal.ai (§9.4). Nothing here is built yet. The plan for the first sub-batch (8A) is `docs/superpowers/plans/2026-11-07-sub-project-8a-packages-and-registries.md`.
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
| **8H** Media interfaces | codec backends as plugins, matte producers and the key matte, WGSL effects, transitions and LUT packs with `plugin.pin`/`unpin`/`repin` and pinned ParamSpecs, offline audio processors, schema version 3 (with matte assets, owner decision 3) | 8B, 6B, 6D, 6E | copyleft repo, after-v1 background removal |

**Order:** 8A → 8B → 8C → {8D, 8G} → {8E, 8F} → 8H. 8G needs only 8B and 7A and can run beside 8C–8F; 8H needs 8B and sub-project 6's 6D/6E and can run beside 8D–8F. Arch §24 puts sub-project 7 before 8; no part of 8B waits for 7C (owner decision 1): 7C adds OS confinement to plugins that already run.

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

### 3.2 Runtimes of code plugins

| Runtime | Manifest | Start | When to use |
|---|---|---|---|
| `native` | an executable per platform in the package | through `fin-sandbox` once 7C lands (unconfined before, §3.1) | analyzers, codecs, mattes: anything with ONNX Runtime, GPU or SIMD. The bundled analyzers are one native binary, `fin-analyze` (§8.1) |
| `node` | `main` (one bundled `.mjs` file: no `node_modules`, no install scripts) and **`node`: a minimum Node version** (a semver range; the host refuses with `PLUGIN_NODE_VERSION` when its own Node is older) | finisherd's own Node (`process.execPath`), through `fin-sandbox` once 7C lands, with `--permission --allow-fs-read=<plugin dir> --allow-fs-read=<call dir>/in --allow-fs-write=<call dir>/out` (one path per flag) and no `--allow-child-process`, `--allow-addons`, `--allow-worker` or `--allow-wasi`. Under Electron, `ELECTRON_RUN_AS_NODE=1` is set for that child only | commands, providers, hooks: glue around HTTP APIs |
| `python` | — | — | **after v1** (design note: per-plugin virtual environment by `uv` with `--no-build --require-hashes --only-binary :all:`, run as `python -I -m <module>`; `uv` and CPython downloaded by hash on first use, never bundled) |
| `wasm` | — | — | **after v1** (decision 2) |

- **Node's permission model is a seat belt, not the sandbox** (Node documents that it does not stop malicious code). The `plugin` profile is the guarantee once 7C lands; before it, the seat belt is all a `node` plugin has (§3.1).
- **Node plugins under the signed app (macOS).** A child the app spawns runs with the app's code signature and entitlements when it is the app's own Electron binary, and macOS attributes TCC decisions (camera, microphone, Documents, Desktop, Downloads, Photos, contacts, screen recording) to the *responsible* process, which is the app for any child, whatever binary it runs. A separate unentitled helper would therefore drop the entitlements but **not** the inherited TCC grants, unless it disclaims responsibility through a private launch attribute. **Decision:** once 7C's `plugin` profile lands on macOS, every plugin process (`node` and `native`) runs under a Seatbelt profile that denies `mach-lookup` of `com.apple.tccd` and `com.apple.tccd.system`, denies `appleevent-send` and `mach-lookup` of `com.apple.coreservices.appleevents`, and grants no read root inside TCC-protected folders. Requests then fail closed with no prompt, for every runtime alike, and the plugin escape suite tests it (§16.1). Before then, a plugin's TCC requests are the app's and may prompt the user in its name (§3.1). The app's only entitlement is `com.apple.security.cs.allow-jit` (arch §27.8), which a Node child needs anyway.
- **ONNX Runtime is host-provided.** The Rust SDK's `onnx` feature loads the app's pinned ORT shared library from a read-only path given at `initialize`.

### 3.3 Processes and process keys

- **Long-lived processes** (commands, hook handlers, providers) are keyed by **(plugin, project, principal class)**, where the principal class is `person` (the `console`, `local` and `device` principals), `agent:<root session id>` (one agent session's tree, agent §5.1) or `system` (work the host starts itself, such as library stages). A call is routed only to the process of its own key, so state a plugin keeps in memory for one project or one agent session is unreachable from another. A key's process starts lazily at its first activation and stops after 5 minutes idle; the number of live keys is bounded by live sessions (agent §5.5) and open projects. Calls without a project (registry-wide provider estimates) use the key `(plugin, none, class)`.
- **Jobs** (analyzers, codec backends, matte producers, generation calls) run **one process per job** under `fin-jobs` (cp §6.2): the plugin process is the job's worker through the `fin-jobs` adapter, with the job's resources, liveness, hard kill of the process tree, retries and crash bisection.
- **Environment:** `childEnv()` (arch §27.7) plus only `FIN_PLUGIN_PROTOCOL` and a private `TMPDIR`/`TEMP` in the process's own directory. No `FIN_*` credential, no provider variable, no `FIN_TRAINING`.
- **Limits** (applied by `fin-sandbox`): memory (default 2 GiB; analyzers up to the manifest's declared need ≤ 16 GiB), process count (8), open files (1,024), CPU priority from the job's priority class, a 16 MiB cap per protocol message.
- **Crashes:** in-flight calls fail with `PLUGIN_CRASHED` (retryable for jobs). Three crashes of one plugin within 10 minutes disable it (`PLUGIN_CRASH_LOOP`, audited) until the owner re-enables it.
- **Timeouts:** `initialize` 10 s; a command 10 s by default, at most 60 s by manifest; hooks as agent §9.1 orders them; jobs by `fin-jobs` liveness (`alive` every 5 s, killed after 30 s of silence).

### 3.4 The protocol

- **JSON-RPC 2.0** (arch D8), one JSON object per `\n`-terminated line on stdin/stdout, requests in both directions; stderr is the plugin's log, scrubbed and size-capped by the host before it reaches the server log.
- **Types** live in `fin-plugin-api::protocol` (Rust, pure), exported to JSON Schema and generated into `@finisher/core-types`. Nothing on either side is hand-written.
- **Handshake:** host → `initialize {hostApi, protocol, plugin: {id, version}, grants, dirs: {plugin, data?, call}, ort?, locale, limits}` → `{capabilities, sdk: {name, version}}`; `initialized`; `shutdown`/`exit`.
- **Versioning:** `hostApi` is a semver (starting at `1.0.0`); a manifest's `hostApi` range must include the running host's (`PLUGIN_HOST_API`); additive host methods and newly consumable contribution kinds bump the minor.

### 3.5 Data exchange: `in/`, `out/` and rings (`fin-shm`, owned by sub-project 6)

Every call and every job has a directory `<server-data>/plugins/run/<key>/<callId|jobId>/` with two halves:
- **`in/`**: written by the host before the call, **read-only to the plugin** (a sandbox read root, never a write root). Snapshots over 1 MiB, PCM, frames, artifacts the call may read.
- **`out/`**: the plugin's only write root for the call. Outputs, output rings, temp files.

Rules, all implemented once in **`fin-shm`**, which `fin-worker` and the plugin host both use:
1. **The host never maps a file the plugin can write.** Output files are opened with `O_NOFOLLOW` (`FILE_FLAG_OPEN_REPARSE_POINT` on Windows, then a reparse-tag check); `fstat`/`GetFileInformationByHandle` on the handle must show a **regular file** with **`nlink == 1`** whose **size equals the size the plugin declared** and is **≤ the call's cap**. The host then **copies** it with positional reads into host-private memory or `<server-data>/plugins/staging/` and validates **the copy**. A short read (a file truncated during the copy) is `PLUGIN_OUTPUT_INVALID`, never a `SIGBUS`.
2. **Job outputs are validated after the plugin process has exited**, so nothing can change while the host checks them; long-lived processes' outputs are copied first and validated as copies (rule 1), which makes their liveness irrelevant.
3. **Rings are fixed-size and host-created.** An input ring (`in/<name>.ring`: header, N slots of a fixed size) is created and sized by the host, written only by the host, and mapped read-only by the plugin. An output ring (`out/<name>.ring`) is also created and sized by the host before the call and then falls under the plugin's write grant; the host reads its slots with positional reads only (rule 1), never through a mapping.
4. **Slot ownership moves by control messages:** `slot {ring, index, seq, len}` from the writer, `release {ring, index, seq}` from the reader. A slot is read only after its `slot` message and reused only after its `release`; a message naming a wrong `seq`, an out-of-range index or a length over the slot size is `PLUGIN_PROTOCOL`.
5. The host deletes a call's directory after the call (a job's after its result is stored); a startup sweep removes leftovers.
6. **The TS SDK reads files** with positional reads (`fs.readSync`) and never maps them; Node plugins get whole input files, not streaming rings, in v1.

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

### 3.7 The `plugin` principal

- **Kind `plugin`, id `<@publisher/name>@<version>`** (cp §3.2). A plugin process holds no credential and cannot call `finisherd`'s RPC; the principal authorises what the host does on the plugin's behalf and labels it in the audit log and the journal.
- **Effective rights = invoker ∩ grants**, for everything: a command run by an agent can write only what that agent can write, on that agent's branch subtree; a hook handler sees only events the hook's subscriber may see.
- **Jobs a plugin starts** (`job.submit`, or a command that queues analysis) carry the invoker's principal intersected with the plugin's grants, are **owned by the invoker** (cp §6.1 `owner`; agent §13.1 counts them against the invoker's render-minute budget), and run under the key `(plugin, project, the invoker's class)`. Jobs the host starts itself (library stages) are owned by `system` with the plugin's grants and library reach only.
- **Mapping** of permissions to sub-project 5 scopes: `project:read` → `project.read`; `project:write` → `project.write` restricted to `tx.commit` of the returned transaction; `media:read` → `library.read` and asset reads; `jobs:submit` → `jobs.submit` for the plugin's own kinds. No plugin ever holds a human-only scope, `branch.protected`, `review.decide`, `project.admin`, `auth.admin` or `diagnostics.*`.
- **New scopes** (added to cp §4.4 in 8A): `plugin.read` (`plugin.list|info|search|plan`, `plugin.content.get`, `registry.list|status|refresh`), `plugin.install` (install, update, remove; **owner**), `plugin.admin` (enable, disable, registries; **owner**), `plugin.suggest` (any principal with `project.read`).

### 3.8 Commands, names and `Tx.via`

1. **Names:** a plugin command's registry name is **`plugin:<id>/<command>`** (for example `plugin:@acme/tools/trim-silence`). That is the name sub-project 7's command rules match (agent §7.2: `deny plugin:@acme/*`, `ask plugin:*/publish`), the name `commandOf` returns for the RPC, and the name in `fin schema --registry`. `fin` spells it `fin @acme/tools trim-silence …`; `fin help @acme/tools` lists a plugin's commands from `plugin.commands` (cached per server, keyed by a registry digest), and the MCP adapter exposes them under the same names.
2. The host validates `args` against the command's JSON Schema, pins `rev` = the head of the call's branch and calls `command.run` on the process of the call's key.
3. A returned `tx` is tagged `baseRev = rev`, gets `via: {plugin: "<id>@<version>", command}` and is committed through `tx.commit` under the invoker's principal and author. fin-core validates and rebases or rejects (`CONFLICT`). `--dry-run` works as for every write.
4. **`Tx.via`** (new optional field, 8B): stored with the record; ignored by apply and replay. It needs no upcaster (absent = none). **The server strips any client-sent `via`**: only the plugin host sets it, so no client can attribute its edits to a plugin. Rebase and cherry-pick carry it into the new record (a round-trip test covers commit → rebase → pick → export → import). Review change lists (cp §8) and the dataset builder read it.

### 3.9 fin-core changes, all together

| Change | Batch | Shape and compatibility |
|---|---|---|
| `Tx.via: Option<Via>` | 8B | optional, absent by default; no upcaster; server-stripped from clients; carried by rebase and pick |
| `fin-core::analysis` artifact types, `ArtifactRef` | 8C | artifacts are not project state; `CaptionSource.artifact` keeps its `String` type with a documented grammar `ar:<kind>:<hex16>`, so the serialised model does not change |
| `compose` builders (§8.8) | 8C–8F | pure functions; no model change |
| `plugin.pin`, `plugin.unpin`, `plugin.repin` | 8H | **three ops, one module each**, modelled on `font.pin`/`font.unpin`/`font.repin`, with inverses and footprints |
| `Project.plugins: BTreeMap<PluginId, PluginPin {version, sha256, effects: [{kind, params: [ParamSpec]}]}>` | 8H | the pack's ParamSpecs are **stored in project state at pin time**; lookup is built-ins ∪ pinned, so the same op log validates identically on every machine whether or not the pack is installed |
| `ParamSpec.path`, `Scope::Effect.kind` as `Cow<'static, str>` | 8H | built-ins stay `Borrowed`; pinned specs are `Owned`; serialisation unchanged |
| `AssetKind::Matte`, `Mask.matte` naming an asset | 8H / 6D (owner decision 3) | a new enum variant changes the serialised shape |
| **`SCHEMA_VERSION` 2 → 3** | 8H (or 6D if it lands first) | one bump carries `Project.plugins` and `AssetKind::Matte` together: an upgrade step (`snapshot_v2`, `record_v2`, frozen, with goldens) adds the empty `plugins` map; newer projects are refused by older code per core §2.6 |

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

### 4.3 Permission risks shown to people

| Risk | Permissions |
|---|---|
| low | `project:read`, `media:read`, `fs pluginData read` |
| medium | `project:write`, `jobs:submit`, `fs pluginData readWrite`, `gpu` (kernel GPU drivers become reachable), `hooks:observe` alone, agent-facing content (§4.2) |
| high | `net`, `secrets`, `media:file`, `hooks:modify`, **`hooks:observe` together with `net`** (hook payloads carry prompts, commands and ops, and `net` can send them away), and **any third-party code plugin**, whatever its permissions (§3.1) |

**Widening** is any added permission, an added origin, secret or hook, `brokered` → `raw`, `read` → `readWrite`, a risk increase from a combination (adding `net` to a plugin that has `hooks:observe`), or any change to agent-facing content.

---

## 5. Lifecycle

### 5.1 Where things live

```
<server-data>/plugins/
  registries/<registryId>/registry.json      {id, kind: local, path, trust: {rootSha256, freshness}}
  trust/<rootSha256>/                        trusted TUF metadata per root (kept when a registry is removed, §6.3)
  store/@<publisher>/<name>/<version>-<sha12>/   unpacked package, read-only after install
  staging/<planId>/                          verified, unpacked, not yet approved (10-minute life)
  data/@<publisher>/<name>/<projectId|_>/    the plugin's writable data (fs pluginData), per project
  run/<key>/<callId|jobId>/{in,out}/         per-call and per-job exchange (§3.5)
<server-data>/models/<sha256>/               model files with model.json (§8.2)
control.db: registries, plugins, plugin_grants, plugin_plans (cp §3.1)
```

- `plugins {id, version, source: bundled|registry|dev, registryId?, targetSha256, kind, enabled, installedBy, installedAt, autoUpdate, state: active|disabled|revoked|crashLoop}`; `plugin_grants {pluginId, version, permission JSON, grantedBy, grantedAt}`.
- The store is local disk only (arch §6.7). Packages are never written after install; updates install beside and switch.
- **Plugin data is per project** (and `_` for calls without one), so state a plugin persists for one project is not readable from another project's process key.

### 5.2 Sources of plugins

- **Bundled** (no TUF in v1): the app ships first-party plugins unpacked in its resources. `fin-node` carries a compiled-in list `BUNDLED_PLUGINS: [{id, version, treeHash}]` generated at build time; at start each bundled tree is re-hashed and installed or refreshed only if it matches. On signed releases the operating system's code signature covers the resources (macOS notarisation, Windows Authenticode), so the hash list detects corruption and the signature detects tampering. Bundled plugins can be disabled but not removed, and update only with the app.
- **Local-folder registries:** a directory holding a TUF repository (§6.1) on local disk, a NAS or a synced folder, read-only to Finisher. **A git registry is such a folder** that the user keeps current with `git pull` (or a sync tool); Finisher never runs git and trusts nothing about its history or commits. A pull that rolls the repository back is refused by TUF (§6.2).
- **Adding a registry** (`registry.add`, owner) needs its root: `--root <file>` or `--root-sha256 <hex>` checked against `1.root.json` (or the newest root it chains to); otherwise the console prints the root's hash and key ids and the owner must type the hash back. There is no silent trust on first use.
- **HTTPS registries** (the public registry after v1) use the same client with an HTTPS transport; in v1 an HTTP transport exists **only in test builds** (feature `test-http`), so the `tuf-conformance` suite can run against the client.

### 5.3 Install, update, enable, remove

1. **Plan** (`plugin.plan {id, version?, registry?, update?}`, `plugin.read`): refresh the registry (§6.2), copy the target to `staging/`, verify length and hashes, unpack strictly (§6.4), validate the manifest and match it against the target's signed custom metadata, check `hostApi`, platform and Node version, compute the permission diff, collect warnings (yanked, advisories, look-alike ids, copyleft, staleness) → an **`InstallPlan`** with a `planId` and a `digest`. Nothing runs.
2. **Approve and install** (`plugin.install {planId, digest, accept: {copyleft?, thirdPartyCode?}}`, `plugin.install`, owner, from a plan a **human** made): re-hash the staging tree, rename it into `store/`, record the grants exactly as planned, audit, journal. A third-party code plugin's plan needs `accept.thirdPartyCode: true` (else `THIRD_PARTY_CODE_NOT_ACCEPTED`): the person has seen the `thirdPartyCode` high-risk warning, and whether the plugin will run confined on this OS, and approves each listed permission (the console prints them one per line and asks for the plugin's id typed back, not `yes`). Content packs become active; code plugins become enableable from 8B on, before 7C too (owner decision 1, §3.1).
3. **Update:** a plan with `update: true`. **Automatic updates** apply only to content packs with `autoUpdate` on, only when nothing widens, **never to agent-facing content** (skills, prompts, roles, rubrics: an update can change what agents are told) and **never as the result of a refresh a non-human principal triggered**: an agent's or `system`'s `registry.refresh` updates trusted metadata and applies `disable` advisories, but installs nothing. **Code plugins never update automatically** (arch §15 rule 6), bundled ones aside (they update with the app), and a third-party code plugin's update needs a person's plan and `accept.thirdPartyCode` again even when nothing widens. A widening update keeps the old version running until a person approves the new grants.
4. **Downgrade** only by a person naming the exact version (`--allow-downgrade`); TUF still refuses rolled-back metadata.
5. **Enable/disable** (owner): per server. Disabling stops the plugin's processes, unregisters its contributions and fails its queued jobs with `PLUGIN_DISABLED`.
6. **Remove** (owner): disable, delete the store directory, and with `--purge-data` its data. Bundled plugins cannot be removed (`PLUGIN_BUNDLED`).
7. **Suggestions:** an agent may call `plugin.suggest {id, reason}` (arch §8.3); it opens a `pluginSuggestion` review item (cp §8.1), shown as a suggestion, never as an approval card worded by the agent. Agents never install: `fin plugin install` is `HUMAN_ONLY` (agent §7.2's default `deny plugin.install` stays as a second layer).

### 5.4 The install plan (what the permission prompt shows)

`InstallPlan {planId, digest, plugin: {id, version, kind, displayName, publisher}, source: {kind, registryId?}, action: install|update|downgrade, from?, permissions: [{perm, risk, details}], diff: {added, widened, removed}, licence: {spdx, class}, bytes, models: [{id, bytes, licence, attribution}], warnings: [{code, details}], registryStaleDays?, expiresAt}`. A third-party code plugin's plan always carries the warning `{code: thirdPartyCode, details: {confined}}`, shown first and as high risk (§3.1). Sub-project 9 renders it; the console prints it and asks for `yes`. A plan made by a non-human principal is marked and refused by `plugin.install`, even when a person submits its id.

### 5.5 Developer mode

`finisherd --plugin-dir <dir>` (console only) loads one unpacked plugin as `@dev/<name>` without signatures, for that server run only, with grants approved at start on the console. Before 7C's `plugin` profile exists on the OS, a developer plugin runs unconfined like every code plugin (§3.1), with a console warning; it is never reachable from an agent principal. `fin plugin validate <dir>` validates offline.

### 5.6 Publisher tooling (`fin-plugin`, a separate binary, not shipped in the app)

`fin-plugin new <kind>`, `validate`, `pack` (reproducible), `test` (§7.4), and `registry init|delegate|publish|yank|advise|rotate-key|resign-timestamp`, built on `fin-registry`'s editor (§6.6). Keys are ed25519 PEM files; `fin-plugin` never uploads anything. Private keys and the key-ring folder are owner-only from creation: 0600 and 0700 on Unix; on Windows a protected DACL (nothing inherited) granting only the current user and `SYSTEM` (the OS, which can read any file already; services that run as it without backup semantics would otherwise fail). A key folder or private key that anyone else can reach is refused, ssh-style, with `KEY_UNPROTECTED {path, problems}` and the `chmod` or `icacls` command that fixes it. On Unix that means any group or other mode bit; on Windows it means an owner other than the user, `SYSTEM` or Administrators, a NULL DACL (FAT, exFAT), or an allow ACE for any other SID.

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

### 6.2 Client update workflow (TUF 1.0 §5, per registry)

1. Load the trusted root; fetch `<n+1>.root.json` until none is found (≤ 32 per refresh), each verified by the old and the new threshold; **save each newly trusted root atomically** (temp then rename) before fetching the next. On a rotation of the timestamp or snapshot keys, delete the trusted timestamp and snapshot (**fast-forward recovery**).
2. Fetch `timestamp.json` (≤ 16 KiB), verify, refuse a lower version (rollback); expiry per §6.3.
3. Fetch the snapshot named by the timestamp (hash and length checked); refuse lower role versions than trusted.
4. Fetch top-level and delegated targets by snapshot version; a publisher's role signs only `<publisher>/*` (terminating), so `@a` can never vouch for `@b/x`.
5. Persist every trusted file atomically and only after the whole chain verifies; a failure keeps the previous trusted set and reports `TUF_*`.
6. Targets are fetched with the signed length as a cap and accepted only if every listed hash matches.

### 6.3 Freshness, staleness, clocks, removal

- **`strict`** (future public registries): an expired timestamp is `TUF_EXPIRED`; nothing new installs; installed plugins keep running.
- **`relaxed`** (local folders, the only v1 kind): metadata is loaded with expiry enforcement off (`tough`'s `ExpirationEnforcement::Unsafe`, which 8A Task 1 verifies skips *only* expiry checks) and the host applies its own rule: expired by more than **`maxStaleDays` (default 7)**, read from the signed root's `custom.finisher.maxStaleDays` (≤ 90), is `TUF_EXPIRED`; within it, `REGISTRY_STALE` is a warning. Rollback checks stay strict.
- **Staleness is shown per plugin:** `plugin.list` and `plugin.info` carry `registryStaleDays` for every installed plugin from a registry, and status warns when any is stale.
- **Clocks:** each root's trust directory records the wall-clock time of its last successful refresh, never later than the newest trusted timestamp's expiry (so a clock that ran fast cannot lock refreshes out for longer than that); a clock more than a day earlier than it is `CLOCK_SUSPECT`, and refreshes change nothing until it is fixed. After correcting a clock that ran fast, the owner can clear the recorded time on the server with `fin-plugin registry trust reset-clock` (human-only; it changes nothing else).
- **Trust store:** each root's directory changes under an exclusive lock file (`trust/<rootSha256>.lock`) held for a whole refresh, root rotations included. A refresh writes a whole generation (the metadata as fetched, and the verified bytes of the advisories files it lists) and then names it current; a generation a crash left unnamed is never reused and is deleted on the next open. Advisories are read from that kept copy, never re-read from the folder: a kept copy that is missing, does not match its signed hash or does not parse is `REGISTRY_INVALID`, never an empty list.
- **Removing a registry keeps its trusted metadata** (`trust/<rootSha256>/` stays); re-adding a registry with the same root resumes from the trusted versions, so remove-and-re-add cannot roll it back. `registry.forget --root <sha>` (owner, console) deletes it explicitly.

### 6.4 The package (`.finpkg`)

- A zip archive (`zip` 9, already in the tree) with `finisher-plugin.json` at the root and the files the manifest names. Packages carry no signature of their own: their identity is the hash and length in signed targets metadata.
- **Strict reader** (`fin-registry::package`): regular files and directories only (no symlinks, hard links or devices); relative NFC UTF-8 names without `..`, leading `/`, drive letters, `\`, NUL, Windows reserved names or trailing dots and spaces, ≤ 255 bytes per component and ≤ 1,024 per path, unique after case folding; ≤ 10,000 entries, ≤ 512 MiB unpacked, ≤ 200 : 1 ratio per entry, declared sizes checked against written bytes; extracted into `staging/` with modes 0644/0755 (executables only where the manifest names them), never through a path that resolves outside it.
- Models are not inside packages (§8.2).

### 6.5 Yanks, advisories, revocation, key rotation

- **Versions are immutable** (arch §15 rule 2). A yanked version is not offered to new installs and stays installable by exact version with a warning.
- **Advisories** are signed targets (per publisher, and registry-wide under top-level `targets`): `{id, plugin, versions, severity, action: warn|disable, summary, url?}`. A refresh by any principal applies `disable` advisories (`PLUGIN_REVOKED`, audited, journaled): disabling is a safety action, not an update.
- **Key rotation:** root by a new root version signed by both thresholds; targets, snapshot and timestamp keys by root; a publisher's key by re-delegation. A compromised publisher key is answered by re-delegation plus a `disable` advisory.

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

- **Training capture of plugin traffic** is typed, never raw HTTP: command arguments, provider prompts and parameters, output media hashes. Brokered request and response bodies are never journaled.
- **Never recorded:** plugin stderr, registry paths, publisher names, package file names, model URLs, prompts outside `Content`, artifact payloads.
- **Audit log** (cp §3.3): `plugin.install|update|remove|enable|disable|revoke|crashLoop` (an install or update of third-party code records the `thirdPartyCode` acceptance and `confined`), `plugin.grant` (with the diff), `registry.add|remove|trust|forget`, `registry.verifyFail`, `grant` (secret grant handles and raw grants), `hook.modify`, `sandbox.violation` (from 7C, with `profile: plugin`).

---

## 12. Security and threat model

Arch §16 priority: injected agent > malicious plugin > stolen device > network attacker. Sub-project 8 adds the supply chain.

| Threat | Mitigation | Residual |
|---|---|---|
| Malicious plugin reads or writes the user's files | `plugin` sandbox profile (§16.1): read roots limited to the plugin, the runtime, system libraries, ORT, its models and the call's `in/`; write only `out/`, its per-project data and temp | **before 7C lands on an OS, every code plugin there, third-party ones included, is unconfined and runs with the user's rights** (owner decision 1, §3.1): the host's rules hold, the OS enforces nothing; third-party code is high risk at install and explicitly accepted; confinement applies automatically when 7C lands |
| **Confused deputy**: a plugin uses one principal's call to act for another, or a job outlives its invoker's rights | invoker ∩ grants on every call, job and commit; jobs owned by the invoker; reach checks (project, branch subtree, library) on every host call; process keys per (plugin, project, principal class); per-project plugin data | a plugin may still misuse what one invoker could do anyway |
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
| 2 | `MANIFEST_TOO_LARGE`, `PLUGIN_MESSAGE_TOO_LARGE` |
| 3 | `PLUGIN_NOT_FOUND`, `PLUGIN_NOT_INSTALLED`, `PLUGIN_BUNDLED`, `PLUGIN_MANIFEST_INVALID {path, reason}`, `PLUGIN_CONTRIBUTION_UNSUPPORTED {kind, hostApi}`, `PACKAGE_UNSAFE {entry, reason}`, `PACKAGE_TOO_LARGE`, `PLUGIN_HOST_API {range, host}`, `PLUGIN_PLATFORM {platforms}`, `PLUGIN_NODE_VERSION {range, node}`, `PLAN_EXPIRED`, `PLAN_MISMATCH`, `PLUGIN_ALREADY_INSTALLED {installed, version}`, `PLUGIN_DOWNGRADE_REFUSED {installed, version}`, `REGISTRY_NOT_FOUND`, `REGISTRY_INVALID`, `PLUGIN_COMMAND_ARGS`, `PLUGIN_TX_INVALID`, `ARTIFACT_NOT_FOUND`, `ANALYZER_NOT_FOUND`, `ANALYSIS_LANGUAGE_UNSUPPORTED`, `WORD_RANGE_INVALID`, `PLUGIN_UNPINNED`, `PLUGIN_PIN_IN_USE`, `GEN_PARAMS_INVALID` |
| 4 | `PLUGIN_CRASHED`, `PLUGIN_TIMEOUT`, `PLUGIN_PROTOCOL`, `PLUGIN_OUTPUT_INVALID {reason: link\|notRegular\|linked\|size\|truncated\|schema}`, `ANALYSIS_FAILED`, `GEN_PROVIDER_ERROR {status, providerCode?}`, `GEN_SUBMIT_UNKNOWN {idemKey}`, `MODEL_HASH_MISMATCH`, `ARTIFACT_STALE` |
| 5 | `PLUGIN_DISABLED`, `PLUGIN_CRASH_LOOP`, `PLUGIN_RUNTIME_UNAVAILABLE`, `PLUGIN_MISSING`, `REGISTRY_UNREACHABLE`, `REGISTRY_STALE` (warning), `PLUGINS_UNCONFINED` (status warning, §3.1), `MODEL_DOWNLOAD_FAILED`, `MODEL_MISSING`, `GEN_RATE_LIMITED`, `CLOCK_SUSPECT` |
| 7 | `PLUGIN_PERMISSION_DENIED {perm}`, `PROJECT_NOT_FOUND` and `BRANCH_NOT_ALLOWED` for reach (cp §4.4, agent §5.1), `PLUGIN_REVOKED {advisory}`, `PLUGIN_BUNDLED_MISMATCH {reason: treeHash}` (a bundled tree that does not match the hash built into the app), `REGISTRY_UNTRUSTED`, `TUF_SIGNATURE`, `TUF_THRESHOLD`, `TUF_ROLLBACK {role, trusted, got}`, `TUF_EXPIRED {role}`, `TUF_TARGET_MISMATCH {hash \| length}`, `TUF_DELEGATION`, `PERMISSIONS_NEED_APPROVAL {diff}`, `COPYLEFT_NOT_ACCEPTED`, `THIRD_PARTY_CODE_NOT_ACCEPTED`, `EGRESS_DENIED {origin}`, `EGRESS_NOT_HTTPS`, `EGRESS_PRIVATE_ADDRESS`, `SECRET_MISSING {name}`, `GEN_BUDGET_EXCEEDED`, `GEN_APPROVAL_REQUIRED`; and cp's `HUMAN_ONLY` for installs from agents |

No plugin is refused for lack of a sandbox: a host with Landlock disabled runs plugins unconfined under `PLUGINS_UNCONFINED`, and sub-project 7's `SANDBOX_UNAVAILABLE` is not used for it (owner decision 1 and follow-up decision F2; it exists only for a failure to apply a profile where the mechanism is enabled, agent §8.9). A cross-origin redirect is not an error: `net.fetch` returns the 3xx response with `redirectNotFollowed: true`. Codes go into `crates/fin/src/output.rs` and `@finisher/contracts` in the batch that introduces them. TUF failures are exit 7 (a person must look), not 5.

---

## 14. Cross-platform notes

- **Paths:** Windows reserved names, case-folded duplicates and NFD names are refused in packages on every OS (§6.4); the store is read-only (unix modes; on Windows the read-only attribute and ACLs from the broker).
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
- **Files:** read roots = the plugin's store directory, the runtime (finisherd's Node, or the Electron framework, for `node` plugins), system libraries and frameworks (`/usr/lib`, `/lib`, `/System/Library`, `%SystemRoot%\System32` and the like), ONNX Runtime's library, the model files the manifest names, and the call's `in/`; write roots = the call's `out/`, the plugin's per-project data directory (with `fs pluginData readWrite`) and a private temp directory; everything else denied, server data included.
- **Per-job grants on Windows** through the ACL broker (agent §8.4): one container profile per (plugin, project); ACL grants on the call or job directory added before the call and removed after it, write-ahead logged like the broker's session grants; `media:file` grants on a single source file per job.
- **GPU** only with the `gpu` grant: Linux `/dev/dri/renderD*` (never `card*`) and, when CUDA is used, `/dev/nvidia*`; macOS the IOKit user clients Metal needs (the `IOGPU`/`AGXDevice` classes) and nothing else under `iokit-open`; Windows D3D11/D3D12 through the container's graphics access.
- **macOS TCC and Apple Events:** deny `mach-lookup` of `com.apple.tccd`, `com.apple.tccd.system` and `com.apple.coreservices.appleevents`, and `appleevent-send` (§3.2).
- **Limits:** memory, processes and wall clock per §3.3, from a kill-on-close job object or rlimits.
- **Store integrity (follow-up for 7C):** `store/` directories stay writable to the user's account after install (only their files are marked read-only, which the same account can undo), so a package's tree hash is checked only when it is installed (8A Task 8): a re-install of a changed tree replaces it, nothing checks it at start or before a run. The profile makes the store read-only to plugin processes; whether the host also re-hashes a tree before starting it, or keeps the store in a directory only the server can write, is decided with 7C.
- **A plugin escape suite** in CI on all three OSes: TCP, UDP, raw, netlink and unix-socket connects; reading server data, another plugin's data, another project's plugin data, the project folder, `~/.ssh` and the user's Documents; writing to `in/`; replacing an `out/` file by a symlink, junction or hard link to a host file; truncating a ring; GPU device nodes without `gpu`; `ptrace` of the host; TCC access to Documents, camera and microphone; an Apple Event to Finder; spawning past the process limit. Each passes only with an observed denial.

### 16.2 Everything else

| From | Needs |
|---|---|
| 5A | principals with the reserved `plugin` kind; the scope table (adds `plugin.read|install|admin|suggest`); human-only enforcement; the console; the audit log |
| 5C | `fin-jobs`: kinds with resources, `owner`, `externalRef`, `dedupeKey`, idempotency, retries and bisection; hard kill; `jobs.watch`; owner-scoped budgets (agent §13.1) |
| 5D | the library stage framework keyed by fingerprint and stage version; catalog tables for transcripts (FTS5); library reach for principals |
| 5E | the reserved `pluginSuggestion` review item kind |
| 6 | the one `fin-worker` with a decode role for PCM at 16 kHz mono (6C) and frames at analysis sizes (6A–6B); **`fin-shm`** with the rules of §3.5 (6A); the process codec backend and its conformance suite; matte encoding and, per owner decision 3, `AssetKind::Matte` and the schema bump (6D); the WGSL effect ABI (6D) |
| 7 | X1 (§16.1); branch reach for agent principals (§5.1); command rules matching `plugin:<id>/<cmd>` (§7.2); approvals of kind `plugin` (§7.3); the hook engine calling plugin handlers in the `plugin` layer with `hooks:observe`/`hooks:modify` (§9); `SecretStore` and `ModelGateway` grant handles bound to (plugin, name, origins) (§10); budgets and the ledger (§13) |

---

## 17. Crate and file map

| Crate / package | Owner | Responsibility | wasm32 |
|---|---|---|---|
| `fin-plugin-api` | 8A (new) | manifest, permissions, contributions, licence classes, validation; protocol types (8B) | yes, pure |
| `fin-registry` | 8A (new) | TUF client and editor wrapper, filesystem transport (HTTP in test builds), trusted store per root, strict package reader and packer, store and staging, plans, advisories | no |
| `fin-plugin-cli` (`fin-plugin`) | 8A (new) | publisher tooling | no |
| `fin-core` | changed | §3.9 | yes |
| `fin-node` | changed | napi `Registry` on the worker thread; manifest validation; the bundled hash list; the analysis cache index (8C) | — |
| `fin-shm` | sub-project 6 (6A) | §3.5 | no |
| `fin-sandbox` | sub-project 7 (7C) | the `plugin` profile (§16.1) | no |
| `fin-plugin-sdk`, `@finisher/plugin-sdk`, `fin-plugin-test` | 8B (new) | SDKs and harness | no |
| `fin-analyze` | 8C (new) | the bundled analyzers binary | no |
| `finisherd` | changed | `src/plugins/` (registries, lifecycle RPCs, plans, bundled, content, host, process keys, reach, `fin-jobs` adapter, command registry, egress broker), `src/analysis/`, console `plugin` and `registry` | — |
| `fin` | changed | `plugin search|info|ls|validate|suggest`, `plugin install` (exit 7), `fin @<id> <cmd>`, analysis commands | — |

**Dependency rules:** `fin-core` and `fin-plugin-api` stay pure; `fin` links neither `fin-registry` nor ONNX; `fin-node` links `fin-registry` but no ONNX; only `fin-analyze` and plugins link ONNX Runtime; every `rustls::ClientConfig` names its provider (§6.6).

---

## 18. Licences

### 18.1 Dependencies

| Need | Choice | Licence | Batch |
|---|---|---|---|
| TUF | `tough` 0.24 with the `http` feature off (fallback: a fork of its crypto onto `ring`) | MIT OR Apache-2.0 (`aws-lc-rs`/`aws-lc-sys`: a permissive expression, §6.6) | 8A |
| Archives | `zip` 9 (in the tree) | MIT | 8A |
| SPDX, semver | `spdx`, `semver` | MIT OR Apache-2.0 | 8A |
| JSON Schema validation (command arguments) | `jsonschema` | MIT | 8B |
| ONNX | ONNX Runtime (shared library), `ort` 2.0 rc | MIT; MIT OR Apache-2.0 | 8C |
| WGSL validation | `naga` | MIT OR Apache-2.0 | 8H |
| Vector index | exact scan (ours); `usearch` after its check | —; Apache-2.0 (declared) | 8E |

**Rejected or cut:** `tuf` (unmaintained), `gix` (cut with the git transport), `uv` and CPython (cut with Python), LanceDB (not in v1); Ultralytics, aubio, Essentia and whisper-timestamped (copyleft); WhisperX and the original PySceneDetect (Python and ffmpeg dependencies, arch §18).

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
| 4 | Long-lived processes keyed by (plugin, project, principal class); jobs one process each | §3.3 |
| 5 | `in/` read-only, `out/` the only write root; the host never maps a plugin-writable file; copy, then validate; all in `fin-shm` | §3.5 |
| 6 | Every host call is reach-checked against project, branch subtree and library | §3.6 |
| 7 | Plugin rights are invoker ∩ grants; jobs a plugin starts are owned by the invoker | §3.7 |
| 8 | Plugin command names are `plugin:<id>/<cmd>`; `Tx.via` is set only by the host | §3.8 |
| 9 | One `SCHEMA_VERSION` bump (to 3) carries pinned plugins and matte assets | §3.9 |
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
3. **Do mattes travel with the project?** **Yes, as project assets** (as recommended): `AssetKind::Matte` in `assets/`, named by `Mask.matte`, carried by the `SCHEMA_VERSION` 3 bump together with `Project.plugins` (§3.9, §10.3). Sub-project 6 (6D) builds mask edits on it.
4. **Which models may ship as defaults?** **The weight licence governs** (as recommended): MIT, Apache-2.0, BSD, CC0 or CC-BY weights, with **no acceptable-use restriction** and no other limit on use; training-data provenance is recorded and shown, and models whose authors warn that their training data is copyrighted or licensed are not defaults (§18.2). The v1 defaults are Parakeet, whisper turbo, wav2vec2, Silero VAD, YAMNet, SigLIP2, BlazeFace and YOLOX.
5. **Which generation providers ship first-party in v1** (arch §26 question 8)? **OpenRouter and fal.ai** (the owner's choice, replacing revision 2's ElevenLabs and OpenAI images), as bundled Node plugins with brokered egress and curated catalogs (§9.4). OpenRouter's key is the one the user pastes for the built-in agent (agent §10); fal.ai needs a second pasted key. *Known v1 gaps:* dubbing and voice cloning, unless the curated catalogs cover them (to be confirmed against the providers' catalogues at 8G start); any modality neither covers.
6. **What evaluation data may Finisher use?** As recommended: (a) **retire the generated TTS clips** from selection and gating, and re-run the rule selection and the gate on human-labelled clips from permissive speech (Common Voice CC0, LibriSpeech and MLS CC BY 4.0) labelled for Finisher (§8.3); (b) **the owner's footage is evaluation-only**, never committed and never trained on, under a written licence from the owner to the project recorded in the data card; (c) **MUSAN filtered per file** to public domain, CC0 and CC BY, the file list in the data card, the AGENTS.md rule read as applying to the files used (§8.3, §15.3).
7. **Who holds the first-party signing keys, and when are they made?** As recommended: **the owner holds three offline, hardware-backed ed25519 root keys, threshold 2 of 3**, made at a recorded **ceremony before the first first-party registry** is published (no later than sub-project 11); the `@finisher` delegation key is held offline by the owner; snapshot and timestamp keys are online in release CI with a short timestamp expiry; development and tests use generated keys only (§6.1).

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
