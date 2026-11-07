# Sub-project 8: Plugins and registry — Design

- **Status:** draft for review (2026-11-07). Nothing here is built yet. The plan for the first sub-batch (8A) is `docs/superpowers/plans/2026-11-07-sub-project-8a-packages-and-registries.md`.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited as *arch §n*). This document refines arch §4 rule 6, §6.2–6.3, §8.3, §9.11–9.12, §12.1, §12.6–12.7, §13, §14, §15, §16, §18, §21.1, §22 and §27.4 for sub-project 8. Where the two disagree, this document wins for sub-project 8, and arch §27 gets a pointer when each sub-batch merges.
- **Builds on:** sub-project 4 (*core §n*: `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`), the diagnostics hooks D1–D7, sub-project 5 (*cp §n*: branch `sp5-design`, `docs/specs/2026-11-07-sub-project-5-control-plane-design.md`: principals, scopes, the console, jobs, the library stage framework, review items) and sub-project 6 (*media §n*: branch `sp6-design`, `docs/specs/2026-11-07-sub-project-6-media-engine-design.md`: workers, PCM and frame feeds, the codec traits and their conformance suite, masks). Sub-project 7 (the OS sandbox broker, hooks, budgets and approvals) has no spec yet; §15 lists what sub-project 8 needs from it.
- **House style:** as the sub-project 4 spec. Each sub-batch gets its own plan; each section is rewritten "as built" when its sub-batch lands.
- **Licence facts** in §6.6, §8 and §17 were checked against primary sources (crates.io and npm metadata, each project's licence file or model card) on 2026-11-07. They are not legal advice.

---

## 1. Goal, scope and sub-batches

### 1.1 Goal

Sub-project 8 makes Finisher extensible without trusting what extends it, and ships the analysis that the v1 features stand on:
- a **package format, manifest and lifecycle** (install, update, enable, permissions, removal) shared by first-party and third-party plugins (arch §14.1);
- **local-folder and git registries** verified with **TUF**, with the public registry's protocol implemented in the client (arch §15);
- a **plugin host**: out-of-process plugins over JSON-RPC, a capability-scoped host API, and the `plugin` principal that sub-project 5 reserved (cp §3.2);
- **SDKs** for Rust and TypeScript (and Python for ML plugins), a test harness and conformance suites;
- the **bundled analyzers**: transcription and alignment, scenes, semantic search, reframe and audio (arch §13), whose results become fin-core data, then annotations or ops;
- the **generation-provider contract**, the **codec-backend** and **matte-producer** interfaces (with sub-project 6), and media content (WGSL effects, transitions, LUTs).

### 1.2 Non-goals

- **The public signed registry service** (arch §1 "deferred"), Sigstore keyless signing and the publisher website. The client speaks the protocol; nothing here hosts it.
- **The UI**: the plugin browser, permission dialogs, iframe panels and declarative surfaces (sub-project 9). §5.4 defines the plan objects those dialogs render; the console renders them until then.
- **The OS sandbox broker, the hook engine, agent budgets and approval cards** (sub-project 7). Sub-project 8 declares what plugins need from them (§15) and runs only bundled and developer plugins until they exist (owner question 1).
- **Background removal** (after v1, arch §1). The matte interface ships with a key-based reference producer (§10.3).
- **The FFmpeg codec plugin** (`finisher-plugins-copyleft`, arch §18). The interface and conformance suite are here.
- **Agent-provider plugins** (arch §8.2 "more providers via plugins"): the contribution point is reserved (§4.3) and built with sub-project 7's adapter contract.
- **Real-time audio-effect plugins** in preview (they would need WASM in the AudioWorklet; owner question 2).

### 1.3 Success, for every sub-batch

- Every capability is driven and tested through `fin` → `finisherd`, the `finisherd` console (cp §4.6) for human-only operations, or `fin-plugin` (the developer CLI, §5.6).
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS arm64, Windows 11 x64 and Linux x64.
- The licence gate covers plugin packages, model weights (by their recorded licence) and evaluation data (by its data card).
- No UI work. No host names, IPs, user names or key material in tracked files; test keys are generated in tests or are obviously fake fixtures.

### 1.4 Sub-batches and order

| Batch | Delivers | Depends on | Unblocks |
|---|---|---|---|
| **8A** Packages, registries and lifecycle | `fin-plugin-api` (manifest, permissions, licences; generated types), the `.finpkg` format and its strict reader, `fin-registry` (TUF client over local and git registries, trusted-metadata store, advisories), the plugin store, install plans and approvals, install/update/remove/enable, the bundled registry, content packs (font catalogs, caption styles, skills), `fin-plugin` publisher tooling, `fin plugin …`, `finisherd plugin …` | 5A | 8B; sub-project 11 (template and caption packs) |
| **8B** Plugin host and SDKs | the plugin protocol (JSON-RPC 2.0, NDJSON, shared-memory files), process runtimes (native, Node), the `plugin` principal, lazy start, crash and resource isolation, the capability-checked host API, server-side command expansion with `Tx.via`, plugin command groups in `fin` help, schema and MCP, hook handlers, `fin-plugin-sdk` (Rust), `@finisher/plugin-sdk` (TS), `fin-plugin-test` | 8A, 5C | 8C–8H |
| **8C** Analysis foundation and speech | fin-core `analysis` types and `ArtifactRef`, the artifact cache and recipes, analysis jobs and library stages, ONNX Runtime and the model manager, `fin-analyze` (bundled analyzers binary), VAD, transcription, number verbalisation, alignment and boundary correction (retrained, §8.3), optional diarization, transcript views, word addressing, captions from transcripts, re-sync and paper edits | 8B, 5C, 5D, 6C | 8D, 8E |
| **8D** Scenes and audio analysis | scene detectors (PySceneDetect port, optional TransNetV2), loudness curves, speech/music classes, beats, `compose::duck`, markers and splits from analysis | 8C, 6A–6B (frames), 6C | 8E, 8F |
| **8E** Semantic search | chunking, SigLIP2 embeddings, the vector index, transcript full-text, hybrid `library.search`, `fin search`, opt-in cloud embedders | 8C, 8D, 5D (8G for cloud embedders) | sub-project 11 (clip finder) |
| **8F** Reframe | face and person detection, tracking, shot-aware smoothing, crop paths, `compose::reframe` | 8D, 6B, 6D | sub-project 11 |
| **8G** Generation providers | the provider contract, catalogs, brokered egress and `ModelGateway` grants, paid-call safety, provenance, the first-party providers (owner question 6), cloud embedders | 8B; 7 for agent budgets and approvals | sub-project 11 |
| **8H** Media interfaces | codec backends as plugins, matte producers and the key matte, WGSL effects, transitions and LUT packs with `plugin.pin` ops, offline audio processors, the Python SDK and runtime | 8B, 6B, 6D, 6E | copyleft repo, after-v1 background removal |

**Order:** 8A → 8B → 8C → {8D, 8G} → {8E, 8F} → 8H. 8G needs only 8B and can run beside 8C–8F on its own branch; 8H needs 8B and sub-project 6's 6D/6E and can run beside 8D–8F. Arch §24 puts sub-project 7 before 8; 8A and 8B do not need it (their third-party code paths stay off until it lands, owner question 1), 8G needs it only for agent-initiated paid calls.

### 1.5 What exists today (the baseline)

- **fin-core** owns 50 primitive op modules (`crates/fin-core/src/ops/`), multi-op builders in `compose/` (`caption_import`, `overwrite`, `split_linked`, `transition_with_links`, `clip_with_audio`), the param registry (`params/`; built-in effects `colorAdjust`, `lut`, `blur`, "plugins may declare more kinds later"), eval, captions (`CaptionBlock.source: {artifact: String, wordRange}`), `Mask {matte: String, …}` and the events registry (`events/`: `Source` = `fin|finisherd|finNode|ui|agent`, `ActorKind` includes `plugin`, closed name tables, `ContentKind`).
- **No artifact type exists.** Analysis artifacts are referenced by bare strings in two places and produced nowhere.
- **fin-fonts** fetches pinned files over HTTPS (`ureq`, `rustls` on `ring`) into a content-addressed cache, unpacks zip archives (`zip` 9) and merges catalogs by entry id: the 4C plan leaves registry-supplied and signed font catalogs to sub-project 8.
- **`fin`'s command registry** is static (`crates/fin/src/registry.rs`); help, the map and `fin schema` are generated from it.
- **finisherd** has no plugin host. Sub-project 5 adds principals, scopes (`plugin.install` reserved, owner by default), the console, `control.db`, jobs (`fin-jobs`, `fin-workers`, `externalRef` reserved for paid calls) and the library stage framework. Sub-project 6 adds `fin-worker` with PCM and frame outputs in memory-mapped files and the process codec backend.

---

## 2. Principles

1. **A plugin is untrusted code with declared needs.** It gets exactly what its granted permissions name, through the host, checked per call. It never gets ambient authority: no inherited environment, no credentials, no project folder, no socket to `finisherd`.
2. **Trust comes from signed metadata, not from where a file was found.** Every package, bundled ones included, is verified through TUF metadata against a root the owner (or the build) pinned. A git commit, a folder path or an HTTPS origin proves nothing.
3. **Plugins propose; fin-core decides.** A plugin writes canonical state only by returning a transaction of fin-core's primitive ops, validated and committed under the **invoking principal's** rights (D24, arch §6.3). Plugins cannot define primitive ops.
4. **Analysis results are data first.** Every analyzer writes a typed, versioned fin-core artifact. Annotations are views of artifacts; ops come only from pure fin-core builders over artifacts, so a person or an agent can see, dry-run and undo them.
5. **First-party plugins use the third-party path.** Bundled analyzers are plugins with manifests, permissions and signed metadata, run through the same host. Anything they need, a third party can have.
6. **No install-time code, no auto-escalation.** Installing unpacks and verifies; nothing runs. A new permission always needs a person.
7. **Licences are data and are gated.** Every package, model and dataset carries an SPDX expression (or a named model licence) and a provenance note; copyleft is flagged, non-commercial and share-alike are refused (§17).
8. **Every error is `{code, message, details, fix?}`** with a stable exit code; every diagnostic field has a class (arch §21.1).

---

## 3. The plugin model

### 3.1 Kinds

| Kind | What runs | Examples | Isolation |
|---|---|---|---|
| **Content pack** | nothing: data validated in Rust | caption styles, Lottie templates, format templates, presets, QA profiles, ducking rules, skills, saved prompts, rubrics, font catalogs, LUTs, WGSL effects and transitions | none needed; WGSL runs in the compositor's own worker after validation (§10.4) |
| **Code plugin** | one OS process per plugin (per job for analyzers) | commands, hook handlers, analyzers, generation providers, codec backends, matte producers | process + OS sandbox (§15) + capability-checked host API |

### 3.2 Runtimes of code plugins

| Runtime | Manifest | Start | When to use |
|---|---|---|---|
| `native` | an executable per platform in the package | spawned directly | analyzers, codecs, mattes: anything with ONNX Runtime, GPU or SIMD. The bundled analyzers are one native binary, `fin-analyze` (§8.1) |
| `node` | `main`: one bundled `.js`/`.mjs` file (no `node_modules`, no install scripts) | finisherd's own Node (`process.execPath`; under Electron with `ELECTRON_RUN_AS_NODE=1` set for that child only) with `--permission --allow-fs-read=<plugin dir>,<job dir>` and no `--allow-child-process`, `--allow-addons` or `--allow-wasi` | commands, providers, hooks: glue around HTTP APIs |
| `python` | `module`, `requires-python`, a hash-pinned lock (`uv.lock` export) | a per-plugin virtual environment made by `uv` with `--no-build --require-hashes --only-binary :all:` (arch §14.4), run as `python -I -m <module>` | third-party ML (8H) |
| `wasm` | reserved | — | after v1 (owner question 2) |

- **Node's permission model is a seat belt, not the sandbox** (Node documents that it does not stop malicious code); the OS sandbox is the guarantee (§15).
- **ONNX Runtime is host-provided.** The Rust SDK's `onnx` feature loads the app's pinned ORT shared library from a path given at `initialize`, so third-party native plugins need not ship their own; Python plugins use the `onnxruntime` wheel from their lock.
- **Python is never bundled.** `uv` and a Python build are downloaded (pinned by SHA-256) the first time a person installs a Python plugin, into `<server-data>/plugins/python/`. They are outside the shipped app and its licence gate; the install plan names their licences (§5.4).

### 3.3 Process isolation

- **One process per plugin** for commands, hooks and providers, started lazily at the first activation (`onCommand`, `onHook`, `onProvider`, `onJob`) and stopped after 5 minutes idle. **Analyzers, codec backends and matte producers run one process per job** under sub-project 5's jobs (cp §6.2): the plugin process *is* the worker, with the job's resources, liveness, hard kill of the process tree, retries and crash bisection.
- **Environment:** `childEnv()` (arch §27.7) plus only `FIN_PLUGIN_PROTOCOL` and a private `TMPDIR`/`TEMP` inside the plugin's data directory. No `FIN_*` credential, no provider variable, no `FIN_TRAINING`.
- **Limits:** memory (`RLIMIT_AS` on unix where it does not break the runtime, a Job Object memory limit on Windows; default 2 GiB, analyzers up to the manifest's declared need ≤ 16 GiB), CPU priority from the job's priority class (cp §6.2), open files 1,024, a 16 MiB cap per protocol message.
- **Crashes:** in-flight calls fail with `PLUGIN_CRASHED` (retryable for jobs). Three crashes within 10 minutes disable the plugin (`PLUGIN_CRASH_LOOP`, audited) until the owner re-enables it.
- **Timeouts:** `initialize` 10 s; a command 10 s by default, at most 60 s by manifest; a hook as sub-project 7 orders it (arch §10); a host call's answer is the plugin's own business; jobs follow cp §6.2 liveness (`alive` every 5 s, killed after 30 s of silence).

### 3.4 The protocol

- **JSON-RPC 2.0** (arch D8), one JSON object per `\n`-terminated line on stdin/stdout, requests in both directions; stderr is the plugin's log, scrubbed and size-capped by the host before it reaches the server log.
- **Types** live in `fin-plugin-api::protocol` (Rust, pure), are exported to JSON Schema and generated into `@finisher/core-types` (TS) and the Python SDK's models. Nothing on either side is hand-written.
- **Handshake:** host → `initialize {hostApi, protocol, plugin: {id, version}, grants: Permission[], dirs: {plugin, data, job?}, ort?, locale, limits}` → `{capabilities: {commands, analyzers, …}, sdk: {name, version}}`; `initialized`; `shutdown`/`exit`.
- **Bulk data never goes over the pipe.** PCM, frames, snapshots over 1 MiB and artifact payloads are memory-mapped files in the job (or call) directory, as sub-project 6's worker protocol does (media §3.3): the host maps input files read-only for the plugin and the plugin writes outputs only into its job directory, where the host validates them before anything is moved.
- **Versioning:** `hostApi` is a semver of the host API (starting at `1.0.0`). A manifest's `hostApi` range must include the running host's version (`PLUGIN_HOST_API`); additive host methods bump the minor; a plugin calling a method its granted permissions do not cover gets `PLUGIN_PERMISSION_DENIED`.

### 3.5 The capability-scoped host API (host API 1.0)

| Method (plugin → host) | Permission | Notes |
|---|---|---|
| `project.snapshot {rev?}` | `project:read` | the snapshot of the call's project and branch at the call's pinned `rev` (arch §6.3), as JSON or a mapped file |
| `project.view {budget?, range?}` | `project:read` | `fin view` text |
| `analysis.get {ref}`, `analysis.find {fingerprint, kind}` | `project:read` or `media:read` | artifacts of assets the call may see |
| `media.pcm {asset \| fingerprint, range?, rate, channels}` | `media:read` | a mapped `f32` file from sub-project 6's audio cache; never a source path |
| `media.frames {asset \| fingerprint, times \| stride, maxSide, format: rgb8\|gray8}` | `media:read` | a ring of mapped frames from a `fin-worker` decode role |
| `media.file {asset}` | `media:file` (codec and importer plugins only) | a read-only grant on that one source file, through the sandbox broker |
| `net.fetch {request, auth?}` | `net` (origins) | brokered HTTPS to declared origins; credentials injected by the host (§9.3) |
| `secret.grant {name}` | `secrets` with `mode: raw` | a per-call value (owner question 3) |
| `job.progress`, `job.checkpoint`, `job.item`, `job.alive` | (in a job) | sub-project 5's worker messages, as notifications |
| `log {level, msg}` | — | rate-limited, scrubbed |

| Method (host → plugin) | Contribution |
|---|---|
| `command.run {command, args, project, branch, rev}` → `{tx?: {ops, baseRev}, data?}` | `commands` |
| `hook.<event> {payload}` → `allow \| block {reason} \| context {text} \| modify {payload}` | `hooks` (`modify` needs `hooks:modify` per hook, arch §10) |
| `analyze.run {analyzer, input, params, models}` (in a job) | `analyzers` |
| `provider.estimate \| submit \| poll \| cancel \| fetch \| find` | `providers` (§9) |
| `codec.*`, `demux.*` | `codecBackends` (§10.1) |
| `matte.run` (in a job) | `matteProducers` (§10.3) |

### 3.6 The `plugin` principal

- **Kind `plugin`, id `<@publisher/name>@<version>`** (cp §3.2). A plugin process holds no credential and cannot call `finisherd`'s RPC; the principal exists to authorise what the host does *on the plugin's behalf* and to label it in the audit log and the journal.
- **Its scopes are the intersection** of the plugin's grants and the invoker's principal: a plugin command run by an agent can write only what that agent can write (no confused deputy). A job a plugin starts runs as `{kind: plugin}` with scopes from its grants only.
- **Mapping** of permissions to sub-project 5 scopes: `project:read` → `project.read`; `project:write` → `project.write` restricted to `tx.commit` of the returned transaction; `media:read` → `library.read` and asset reads; `jobs:submit` → `jobs.submit` for the plugin's own job kinds. No plugin ever holds a human-only scope, `branch.protected`, `review.decide`, `project.admin`, `auth.admin` or `diagnostics.*`.
- **New scopes** (added to cp §4.4's table in 8A): `plugin.read` (`plugin.list|info|search|plans`, `registry.list`), `plugin.install` (install, update, remove; **owner**), `plugin.admin` (enable, disable, grants, registries, settings; **owner**), `plugin.suggest` (any principal with `project.read`).

### 3.7 Commands and `Tx.via`

1. `fin <group> <command> …` reaches `finisherd`, which looks the command up in the **one registry** (arch §9.12): the static fin registry plus every enabled plugin's `commands`. `fin` learns plugin commands from `plugin.commands` (cached per server, keyed by a registry digest) for `fin help`, `fin schema` and the MCP adapter.
2. The host validates `args` against the command's JSON Schema, pins `rev` = the branch head and calls `command.run`.
3. A returned `tx` is tagged `baseRev = rev`, gets `via: {plugin: "<id>@<version>", command}` and is committed through `tx.commit` under the invoker's principal and author. fin-core validates and rebases or rejects (`CONFLICT`). `--dry-run` works as for every write.
4. **`Tx.via`** (new optional field, 8B): stored with the record, so review change lists (cp §8) and the dataset builder can tell what a plugin made. It is ignored by apply and replay.

---

## 4. The manifest

### 4.1 Schema (`finisher-plugin.json`, `fin-plugin-api::manifest`, generated types)

```jsonc
{
  "manifestVersion": 1,
  "id": "@publisher/name",              // [a-z0-9][a-z0-9-]{1,38} / [a-z0-9][a-z0-9-]{1,63}
  "version": "1.4.2",                    // semver 2.0, immutable once published
  "kind": "content" | "code",
  "hostApi": "^1.0",                     // semver range
  "license": "MIT",                      // SPDX expression
  "publisher": { "name": "…", "url": "…" },
  "messages": "i18n",                    // ICU catalogues <locale>.json (arch §17.1); names and summaries are keys
  "locales": ["en"],
  "platforms": ["darwin-arm64", "win32-x64", "linux-x64", "linux-arm64"],   // code only; content packs are universal
  "runtime": { "type": "native", "exec": { "darwin-arm64": "bin/darwin-arm64/plugin", … } }
           | { "type": "node", "main": "dist/plugin.mjs" }
           | { "type": "python", "module": "pkg.main", "requiresPython": ">=3.11,<3.14", "lock": "pylock.toml" },
  "permissions": [
    { "perm": "project:read" }, { "perm": "project:write" }, { "perm": "media:read" }, { "perm": "media:file" },
    { "perm": "jobs:submit" }, { "perm": "gpu" },
    { "perm": "net", "origins": ["https://api.example.org"] },
    { "perm": "secrets", "names": ["example"], "mode": "brokered" | "raw" },
    { "perm": "fs", "scope": "pluginData" | "workspace", "access": "read" | "readWrite" },
    { "perm": "hooks:modify", "hooks": ["PreCommand"] }
  ],
  "activation": ["onCommand", "onHook", "onProvider", "onJob"],
  "contributes": { … §4.2 … },
  "models": [{ "id": "…", "url": "…", "sha256": "…", "bytes": 0, "license": "Apache-2.0",
               "attribution": "…", "provenance": "…", "purpose": "msg.key" }],
  "previews": ["previews/1.png"],
  "homepage": "…", "repository": "…"
}
```

- **Validation is Rust** (`fin-plugin-api::validate`, exposed through `fin-node`): grammar, required fields per kind, every referenced file present in the package, every JSON Schema in a contribution compiles (`jsonschema` crate, draft 2020-12), every ParamSpec well formed, every message key in the `en` catalogue, permissions closed and minimal (a content pack with permissions is invalid), `platforms` matches `runtime.exec`.
- **Reserved publishers:** `@finisher`, `@fin`, `@official`, `@system`, `@local` and the names of the bundled registry's publishers. `@dev/*` is the developer-mode scope (§5.5).

### 4.2 Contributions

| Area | Key | Payload (summary) | Batch |
|---|---|---|---|
| Content | `fontCatalogs` | a fin-fonts catalog file, merged by entry id (4C Decision 30) | 8A |
| | `captionStyles` | `CaptionStyle` values (fin-core), copied into a project by `style.add` | 8A |
| | `skills`, `prompts`, `rubrics`, `roles` | skill folders (progressive disclosure, `fin guide`), saved prompts, critic rubrics, agent roles (sub-projects 7, 10 consume) | 8A (skills), 7/10 |
| | `lottieTemplates`, `formatTemplates`, `exportPresets`, `qaProfiles`, `duckingRules`, `editPresets` | validated data, copied into projects as assets or settings by their sub-projects | 8A validates; 6, 10, 11 consume |
| | `luts`, `effects`, `transitions` | `.cube` files; WGSL with ParamSpecs (§10.4) | 8H |
| Agent | `commands` | `{group, name, summary, inputSchema, write, timeoutMs?}` | 8B |
| | `hooks` | `{event, method, timeoutMs, modify?}` | 8B (engine: 7) |
| | `agentProviders` | reserved | 7 |
| Media | `analyzers` | §8.1 | 8C |
| | `codecBackends`, `importers` | §10.1 | 8H |
| | `matteProducers` | §10.3 | 8H |
| | `audioProcessors` | offline processors that render a new audio asset (de-noise) | 8H |
| Generation | `providers`, `embedders` | §9 | 8G |
| Extension slots | `slots` | typed data under a JSON Schema the plugin declares, validated in Rust (arch §14.2) | 8B |
| UI | `ui` | reserved for sub-project 9 | 9 |

### 4.3 Permissions shown to people

Each permission has a fixed message key and a risk level the install plan carries: `low` (`project:read`, `media:read`, `gpu`, `fs pluginData`), `medium` (`project:write`, `jobs:submit`, `fs workspace`), `high` (`net`, `secrets`, `media:file`, `hooks:modify`). **Widening** is any added permission, an added origin, secret or hook, `brokered` → `raw`, or `read` → `readWrite`.

---

## 5. Lifecycle

### 5.1 Where things live

```
<server-data>/plugins/
  registries/<registryId>/
    registry.json            {id, kind: bundled|local|git, location, ref?, trust: {rootSha256, freshness}, agentInstall}
    tuf/                     trusted metadata: root.json (latest), timestamp.json, snapshot.json, targets.json, delegations/<role>.json
    git/                     bare repository (git registries only)
  store/@<publisher>/<name>/<version>-<sha12>/   unpacked package, read-only after install
  staging/<planId>/          verified, unpacked, not yet approved (deleted after 10 minutes)
  data/@<publisher>/<name>/  the plugin's own writable data (fs pluginData), kept across updates
  python/                    uv and Python builds (8H)
<server-data>/models/<sha256>/                    model files with model.json (§8.2)
control.db: plugins, plugin_grants, plugin_plans, registries (cp §3.1, migration m00xx in 8A)
```

- `plugins {id, version, registryId, targetSha256, kind, enabled, installedBy, installedAt, autoUpdate, state: active|disabled|revoked|crashLoop}`; `plugin_grants {pluginId, version, permission JSON, grantedBy, grantedAt}`.
- The store is local disk only (arch §6.7). A plugin's files are never written after install; updates install beside and switch.

### 5.2 Registries

- **Bundled:** the app ships a TUF repository of first-party packages in its resources, with the root's SHA-256 compiled into `fin-node` (`BUNDLED_ROOT_SHA256`). Bundled plugins are installed (verified like any other) at first start and after each app update, and can be disabled but not removed.
- **Local folder:** a directory holding a TUF repository (§6.1) on local disk, a NAS or a synced folder, read-only to Finisher. Teams publish into it with `fin-plugin` (§5.6).
- **Git:** a repository with the same layout at a configured ref, fetched with `gix` into a bare repository; the host reads `metadata/` and `targets/` from the fetched tree object without checking files out. Git history and commit signatures are not trusted; TUF is (principle 2). HTTPS (public, or a token from the `SecretStore` of cp §5.5) and SSH through the user's `ssh`.
- **Adding a registry** (`registry.add`, owner) needs its root: `--root <file>` or `--root-sha256 <hex>` checked against the fetched `1.root.json` (or the latest root it chains to). Without either, the console shows the root's hash and key ids and asks the owner to confirm them out of band; there is no silent trust on first use.

### 5.3 Install, update, enable, remove

1. **Plan** (`plugin.plan {id, version?, registry?, update?}`, `plugin.read`): refresh the registry's TUF metadata (§6.2), find the target, copy it to `staging/`, verify its length and hashes, unpack it strictly (§6.4), validate the manifest and check that it matches the target's signed custom metadata, check `hostApi` and the platform, compute the permission diff against the installed version, collect warnings (yanked, advisories, look-alike names, copyleft) → an **`InstallPlan`** (§5.4) with a `planId` and a `digest`. Nothing runs.
2. **Approve and install** (`plugin.install {planId, digest, accept: {copyleft?}}`, `plugin.install`, owner): re-checks the staging tree's hashes, renames it into `store/`, records the grants exactly as planned, audits `plugin.install`, journals `plugin.lifecycle`. Content packs become active; code plugins too once a sandbox exists (owner question 1).
3. **Update:** a plan with `update: true`. If nothing widens and the plugin is a content pack with `autoUpdate` on, the server may apply it on a registry refresh; **code plugins never update automatically** (arch §15 rule 6). A widening update keeps the old version running until a person approves the new grants.
4. **Downgrade** only by a person naming the exact version (`--allow-downgrade`); TUF still refuses rolled-back metadata.
5. **Enable/disable** (`plugin.enable|disable`, owner): per server. Disabling stops processes, unregisters contributions and fails queued jobs of its kinds with `PLUGIN_DISABLED`.
6. **Remove** (`plugin.remove`, owner): disable, then delete the store directory (and, with `--purge-data`, its data directory). Bundled plugins cannot be removed.
7. **Suggestions:** an agent can call `plugin.suggest {id, reason}` (arch §8.3): it opens a `pluginSuggestion` review item (kind reserved in cp §8.1), shown to people as a suggestion, never as an approval card worded by the agent.
8. **Agent installs** stay `HUMAN_ONLY` unless the owner sets `plugins.agentInstall` on a registry; even then an agent may install only content packs and code plugins whose plan is entirely `low` risk, and never a widening update.

### 5.4 The install plan (what the permission prompt shows)

`InstallPlan {planId, digest, plugin: {id, version, kind, displayName, publisher}, registry: {id, kind}, action: install|update|downgrade, from?, permissions: [{perm, risk, details}], diff: {added, widened, removed}, licence: {spdx, copyleft, categories}, bytes, models: [{id, bytes, licence, attribution}], runtimes: [{name, bytes, licence}] (Python: uv, CPython), warnings: [{code, details}], expiresAt}`. Sub-project 9 renders it; the console prints it and asks for `yes`; agents never see a `digest` they could submit (plans made by non-human principals are marked and refused by `plugin.install`).

### 5.5 Developer mode

`finisherd --plugin-dir <dir>` (console only) loads one unpacked plugin as `@dev/<name>` without signatures, enabled for that server run only, with its grants approved at start on the console. `fin plugin validate <dir>` runs manifest validation offline. Developer plugins are journaled as `registry: dev`.

### 5.6 Publisher tooling (`fin-plugin`, a separate binary, not shipped in the app)

`fin-plugin new <kind>`, `validate`, `pack` (reproducible: sorted entries, fixed timestamps, normalised modes), `test` (§14.2), and `registry init|add|publish|yank|advise|delegate|rotate-key|resign-timestamp`, built on the TUF editor of §6.6. Keys are ed25519 PEM files or, later, a KMS/HSM signer; `fin-plugin` never uploads anything.

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
  <publisher>/<name>/<sha256>.<version>.finpkg     consistent target names (hash-prefixed)
  <publisher>/advisories.json                       signed advisories (§6.5)
```

- **Roles** (arch §15): `root` (offline keys, threshold ≥ 2 for the bundled and any public registry; ≥ 1 allowed for team registries), `targets` (delegates only: one terminating delegation per publisher with paths `<publisher>/*`), `snapshot`, `timestamp`. Publishers sign their own delegated role.
- **Target custom metadata** (signed): `{id, version, kind, hostApi, platforms, license, permissionsDigest, manifestSha256, yanked?, bytesUnpacked}`. Search and `plugin.info` read it without downloading packages; the plan checks the unpacked manifest against it.
- **Signatures:** ed25519. Canonical JSON as TUF 1.0 specifies (the crate's implementation, never ours).

### 6.2 Client update workflow (TUF 1.0 §5, per registry)

1. Load the trusted root; fetch `<n+1>.root.json` until none is found (bounded: 32 per refresh), each verified by the old and the new root's thresholds; on a rotation of the timestamp or snapshot keys, delete the trusted timestamp and snapshot (fast-forward recovery).
2. Fetch `timestamp.json` (size cap 16 KiB), verify, refuse a lower version (rollback) and an expired one (freeze), unless the registry's freshness policy allows it (§6.3).
3. Fetch the snapshot named by the timestamp (hash and length checked), refuse lower role versions than trusted (rollback of any role).
4. Fetch top-level and delegated targets by snapshot version; delegations resolved in order; a publisher's role can only sign `<publisher>/*` (terminating), so `@a` can never vouch for `@b/x`.
5. Persist every newly trusted file atomically (temp then rename) only after the whole chain verifies; a failure keeps the previous trusted set and reports `TUF_*`.
6. Targets are downloaded with a length cap from the signed metadata (endless-data defence) and accepted only if every listed hash matches.

### 6.3 Freshness, clocks and offline use

- `freshness: strict` (bundled and future public registries): an expired timestamp is `TUF_EXPIRED`; nothing new installs, installed plugins keep running.
- `freshness: relaxed` (local and git registries, the default there): teams cannot re-sign a timestamp daily, so an expired timestamp is a warning (`REGISTRY_STALE`) for up to the registry's declared `maxStaleDays` (default 30, carried in the signed root's custom field; the client never extends it). Rollback checks stay strict.
- **Clocks:** TUF expiry needs the local clock (the spike recorded clients off by seconds for a day, arch §27.9; expiries are days, so that is harmless). Each registry records the wall-clock time of its last successful refresh; a clock more than a day *earlier* than that is reported (`CLOCK_SUSPECT`) and refreshes are refused until it is fixed, so a clock set back cannot make expired metadata look fresh.
- **Offline:** installed plugins never need the registry. The bundled registry ships with expiries beyond the app's support window and is refreshed by app updates.

### 6.4 The package (`.finpkg`)

- A zip archive (the `zip` crate already in the tree for fonts) holding `finisher-plugin.json` at the root and the files the manifest names. Packages carry no signature of their own: their identity is the SHA-256 and length in signed targets metadata.
- **Strict reader** (`fin-registry::package`), before anything touches the store:
  - entries are regular files or directories only (no symlinks, no hard links, no devices; zip external attributes checked);
  - names are relative, NFC, UTF-8, no `..`, no leading `/` or drive letters, no `\`, no NUL, ≤ 255 bytes per component and ≤ 1,024 per path, no Windows reserved names (`CON`, `NUL`, `COM1`, …), no trailing dot or space, and unique after case folding;
  - ≤ 10,000 entries, ≤ 512 MiB unpacked, ≤ 200 : 1 compression ratio per entry, declared sizes checked against written bytes;
  - extracted into `staging/` with modes 0644/0755 (executables only where the manifest names them), never through a path that resolves outside it.
- Models are not inside packages: they are listed in the manifest and fetched on demand (§8.2).

### 6.5 Yanks, advisories, revocation, key rotation

- **Versions are immutable** (arch §15 rule 2). Yanking sets `yanked` in the target's custom metadata: not offered to new installs, still installable by exact version with a warning.
- **Advisories** are signed targets per publisher (and a registry-wide one under the top-level `targets` role): `{id, plugin, versions (semver range), severity, action: warn|disable, summary (message key or text), url?}`. On refresh, `disable` advisories disable matching installed versions (`PLUGIN_REVOKED`, audited, journaled); `warn` advisories appear in status and plans.
- **Key rotation:** root keys rotate by a new root version signed by both thresholds; targets, snapshot and timestamp keys rotate by root; a publisher's key rotates or is revoked by the top-level targets role re-delegating its scope. A compromised publisher key is answered by a new delegation plus a `disable` advisory for the versions signed after the compromise.

### 6.6 TUF implementation

| Candidate | Version (2026-11-07) | Licence | Notes | Choice |
|---|---|---|---|---|
| `tough` (awslabs) | 0.24.0 (2026-07) | MIT OR Apache-2.0 | TUF 1.0 client and repository editor; delegations, consistent snapshots; used for Bottlerocket updates. Crypto through `aws-lc-rs` (`aws-lc-sys`: ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0): all allowed by `deny.toml` through an allowed branch), async on `tokio`; HTTP is an optional feature we leave off | **chosen**, behind the 8A Task 1 gate |
| `tuf` (rust-tuf) | 0.3.0-beta15 (2025-10) | MIT OR Apache-2.0 | `ring` only (already in the tree), no async runtime; still a beta with an unstable API; vendored by Fuchsia | fallback if `aws-lc-sys` fails the build or size gate on any OS |
| `tuf-js` + `@tufjs/models` | 6.0.2 | MIT | the npm CLI's client (sigstore-js); TypeScript, no repository editor | rejected: verification would run in Node next to a Rust package reader, and publisher tooling needs a Rust editor anyway |
| `python-tuf` | — | Apache-2.0 OR MIT | the reference implementation | test-time oracle only (`tuf-conformance`, §14.3) |
| own implementation | — | — | — | never: canonical JSON, threshold and delegation logic are where clients break |

- `fin-registry` wraps the chosen crate behind a small trait (`TrustedRepo::refresh`, `targets`, `fetch_target`) with filesystem and git-tree transports, so the fallback swap touches one module. `fin-node` runs it on its per-engine worker thread (core §3.10 Decision 7) inside a current-thread `tokio` runtime owned by that module; nothing async leaks into fin-node's API.

---

## 7. SDKs and developer experience

### 7.1 Rust: `fin-plugin-sdk` (MIT, crates.io)

```rust
use fin_plugin_sdk::{prelude::*, analysis::Transcript};

#[derive(Default)]
struct Speech;

impl Plugin for Speech {
    fn initialize(&mut self, cx: &InitCx) -> Result<(), PluginError> { Ok(()) }
    fn command(&mut self, host: &Host, call: CommandCall) -> Result<CommandOut, PluginError> {
        let snap = host.snapshot()?;                     // fin_core::model::Project
        let ops = fin_core::compose::caption_import(&snap, /* … */)?;
        Ok(CommandOut::tx(ops))
    }
    fn analyze(&mut self, host: &Host, job: AnalyzeJob, out: &mut ArtifactOut) -> Result<(), PluginError> {
        let pcm = host.pcm(job.input(), 16_000, 1)?;    // mapped f32 slice
        out.write(Transcript { /* … */ })                // validated by fin-core types
    }
}

fin_plugin_sdk::main!(Speech);
```

- The protocol loop, framing, typed host calls, mapped-file access, progress and cancellation (`job.cancelled()`), artifact writers over `fin-core::analysis` types, and an `onnx` feature (`ort` 2 with `load-dynamic`, loading the host's ORT).
- Plugins may depend on `fin-core` (MIT, pure) to build and dry-run transactions offline.

### 7.2 TypeScript: `@finisher/plugin-sdk` (MIT, npm)

`definePlugin({ commands: { "hello": async (args, host) => ({ tx: [...] }) }, providers: { … } })`, generated protocol and model types, a `host.fetch` wrapper over brokered egress, and a bundling recipe (esbuild to one ES module, no runtime dependencies at install).

### 7.3 Python: `finisher-plugin` (MIT, PyPI; 8H)

Pure Python, typed models generated from the same JSON Schema, a `numpy` view over mapped PCM and frames when `numpy` is installed, and a template with a hash-pinned lock and wheels-only dependencies (`onnxruntime`, `numpy`).

### 7.4 `fin-plugin-test`

A harness that runs a plugin against a fake host: fixture snapshots, generated PCM and frames (the sub-project 6 test-pattern generators), a fake HTTP server for providers, assertions on calls, permissions and outputs. **Conformance profiles:** `command`, `analyzer` (artifact validity, determinism declaration, cancellation within 2 s, crash recovery), `provider` (§9.5), `codec` (media §13.5), `matte` (§10.3). CI runs every bundled plugin through its profile.

---

## 8. Bundled analyzers

### 8.1 Shape

- **`fin-analyze`** is one native Rust binary built with the Rust SDK, shipped in the app and published in the bundled registry as several plugins that share it, each with its own manifest and permissions (`media:read` and `gpu` only; no `net`, no `project:write`): `@finisher/speech`, `@finisher/scenes`, `@finisher/audio`, `@finisher/search`, `@finisher/reframe`, `@finisher/key-matte` (8H).
- **Analyzer contribution:**
  ```jsonc
  { "id": "transcribe", "version": 3, "artifact": "transcript", "kv": 1,
    "inputs": { "audio": { "rate": 16000, "channels": 1 } },     // or frames {maxSide, fps, format}, or artifacts [kinds]
    "params": { /* JSON Schema: language, model choice */ },
    "appliesTo": { "assetKinds": ["media"], "needs": ["audio"] },
    "resources": ["cpu"], "models": ["parakeet-tdt-0.6b-v2", "…"],
    "stage": { "library": true, "default": "on", "dependsOn": ["@finisher/speech:vad"] } }
  ```
  The host registers a job kind `analyze` (one kind for every analyzer; `params.analyzer` selects) and, when `stage.library`, a library stage in cp §7.3's framework keyed by `(analyzer, version, model hashes)`.
- **Recipes and caching:** an artifact's id is `ar:<kind>:<hex16>` where the hex is BLAKE3 of the canonical recipe `{analyzer id and version, model SHA-256s, canonical params, input fingerprint and stream, input recipe (PCM or frame feed version, decoder class), runtime class (ORT version, execution provider class cpu|gpu)}`. Identical recipes hit the cache (`<server-data>/cache/analysis/`, indexed by `fin-node`, LRU-evicted, regenerable); the job's `dedupeKey` is the recipe, so concurrent requests coalesce (cp §6.3). The artifact also records its content hash.
- **Determinism:** pure Rust stages are bit-exact across OSes and wasm32 (tested by hash). ONNX stages run on the CPU provider with fixed thread counts and deterministic compute, and are compared with tolerances (§14.1). GPU providers are part of the recipe, so a GPU result never silently replaces a CPU one.
- **Results become data, annotations and ops** (principle 4): artifacts live in the cache; `fin` and RPC views (`analysis.get`, `fin transcript`, `fin scenes`, `fin beats`, `fin search`) show them; ops come from fin-core builders (§8.8).

### 8.2 ONNX Runtime and the model manager

- **ONNX Runtime** (MIT) is the app's single, pinned shared library per platform, loaded through `ort` 2 (MIT OR Apache-2.0; still `2.0.0-rc.15`, so the version is pinned exactly and wrapped in `fin-analyze::onnx`). The CPU provider is the default everywhere: the spike measured DirectML 3–4× slower than CPU for speech (arch §27.4). The CoreML provider (an OS framework) is used for vision models on macOS when it passes the tolerance tests; GPU providers on Windows and Linux are opt-in, user-installed (CUDA), and their redistributable terms are checked before any is bundled (DirectML's redistributable is not under ORT's licence).
- **Models** are downloaded on demand into `<server-data>/models/<sha256>/` with `model.json {id, sha256, bytes, licence, attribution, provenance, source, plugin}`: resumable (HTTP Range), verified, pausable, through `model.fetch` jobs on the `net` pool, from commit-pinned upstream URLs listed in the signed manifest (arch §13, §21 "Model downloads"). A licence that requires attribution is shown in the model manager and the app's notices (§17.2).

### 8.3 Speech: VAD, transcription, alignment (`@finisher/speech`, 8C)

| Stage | Model / method | Licence | Output |
|---|---|---|---|
| VAD and chunking | Silero VAD (ONNX) | MIT | speech regions; chunks ≤ 30 s cut at pauses |
| Transcript, English | Parakeet-TDT 0.6B v2, TDT greedy decode in Rust over ONNX | CC-BY-4.0 (attribution; the ONNX conversion and any quantisation are stated) | words with ASR timing |
| Transcript, other languages (uk, de, es, fr, pt-BR first; any language whisper supports) | whisper large-v3-turbo, language forced from the content language (arch §17.1), detected from the first 30 s when unset | MIT | words |
| Number verbalisation | own Rust rules per language (cardinals, ordinals, currency, times, years; Ukrainian case agreement), NeMo text-processing grammars (Apache-2.0) as reference only | ours (MIT) | spoken forms aligned back to display words (`"$20"` ↔ "twenty dollars") |
| Alignment | wav2vec2-base-960h CTC (fp32 ONNX), Viterbi word spans in Rust; accents stripped, Cyrillic transliterated | Apache-2.0 | word spans |
| Boundaries | **starts:** learned correction (gradient-boosted trees, evaluated in Rust from a data file); **ends:** raw CTC span end (arch §27.4) | ours | aligned word times |
| Diarization (optional, off by default) | pyannote segmentation-3.0 (MIT; gated download; training sets include licensed corpora) + a 3D-Speaker CAM++ embedding (Apache-2.0), clustering in Rust | MIT / Apache-2.0 | speaker ids per word (owner question 5) |

- **Excluded:** MMS (CC-BY-NC), CrisperWhisper (NC weights), whisper-timestamped (AGPL), WhisperX (copyleft dependencies; copyleft repo only), Qwen3-ForcedAligner and NeMo Forced Aligner (80 ms steps), Parakeet v3 for non-English (cannot force the language) (arch §27.4).
- **Quality gate** (arch §27.4, unchanged): ≥ 95 % of word starts and of word ends within [−125, +45] ms of human labels per supported language; p95 against MFA references ≤ 50 ms tracked, not gated; WER tracked.
- **Retraining the start correction** (8C, the spike's "next step"):
  - data: LibriSpeech train-clean-100/360 (CC BY 4.0), Multilingual LibriSpeech (CC BY 4.0) for de, es, fr and pt, Common Voice (CC0) for uk; references from our own Montreal Forced Aligner runs (MFA: MIT; each acoustic model's licence checked, else trained by us on the same data);
  - augmentation: music from MUSAN (corpus CC BY 4.0; each music file's own LICENSE entry is read and any NC, ND or SA file is dropped) mixed at 0–20 dB SNR, because the reel's misses sit under background music;
  - objective: the gate window's hit rate; speaker-disjoint splits; held-out evaluation on the human-labelled reference set;
  - outputs: the model file (≈ 0.5 MB, shipped) and its data card (sources, licences, attribution, splits) in `THIRD_PARTY_NOTICES` and the model manager. Training code and runs live in `archive/` (AGENTS.md); only the result and its card are tracked.
- **Transcript artifact** (`fin-core::analysis::Transcript`, kv 1): `{language, models: {asr, aligner, corrector}, words: [{text, spoken?, startMs, endMs, conf, timing: aligned|interpolated|asr, speaker?, filler}], segments: [{startMs, endMs, words: [first, end)}], silences: [{startMs, endMs}]}`. Times are integer milliseconds of **source** time (the 1/1000 s grid captions already use, core §5.5), so artifacts are compact and exact.
- **Fillers** come from a per-language closed list plus ASR tokens; they are flags, never removed from the artifact.

### 8.4 Scenes (`@finisher/scenes`, 8D)

- **Detectors** (pure Rust, bit-exact): ports of PySceneDetect's `ContentDetector` (HSV and optional edge deltas), `AdaptiveDetector` and `ThresholdDetector` (fades) (BSD-3-Clause, attributed), over a frame feed at ≤ 256 px on the long side, every frame up to 30 fps and every second frame above.
- **Optional model:** TransNetV2 (MIT; ONNX, 48×27 input) for dissolves, off by default and selected per library.
- **Artifact** `Scenes {rate, cuts: [{frame, kind: cut|fadeIn|fadeOut|dissolve, score, detector}]}` in **source frame numbers** at the source rate, so cuts land on exact frames.

### 8.5 Audio (`@finisher/audio`, 8D)

- **Loudness:** momentary and short-term LUFS curves and integrated loudness from `fin-audio`'s EBU R128 meter (media §8.4), so analysis and export agree.
- **Classes:** speech, music, silence, other per 0.5 s, from YAMNet (Apache-2.0, ONNX conversion) mapped to four classes, merged with Silero VAD speech regions.
- **Beats:** own DSP in Rust (spectral-flux onset envelope, tempo by autocorrelation, dynamic-programming beat tracking after Ellis 2007), with downbeats from a meter estimate. Beat This! (MIT code and weights, trained on files its authors say are partly copyrighted) is an opt-in model under owner question 5. **Excluded:** madmom's models (CC BY-NC-SA), aubio (GPL), Essentia (AGPL).
- **Artifacts:** `Loudness`, `AudioClasses`, `Beats {tempo, beatsMs, downbeatsMs, confidence}`.

### 8.6 Semantic search (`@finisher/search`, 8E)

- **Chunks:** per shot (from `Scenes`), split into ≤ 2 s windows; one to three keyframes per chunk (middle, plus extremes for long windows).
- **Embeddings:** SigLIP2 base (Apache-2.0; ONNX image and text towers; multilingual text) → 768-d, L2-normalised, stored as `f16`. Query text is embedded with the text tower; transcripts give a second signal through full-text search.
- **Index:** per library, `<server-data>/libraries/<id>/vectors/`, keyed by embedder id and version (vectors from different models are never mixed). Arch §13 names LanceDB (Apache-2.0); 8E measures it against an exact `f16` scan plus an HNSW index (`usearch`, Apache-2.0) on a 1M-chunk corpus for build size, build time, binary size and recall@10, and keeps LanceDB unless it misses the budgets in §14.4.
- **Hybrid `library.search {mode: semantic|text|hybrid}`** (cp §7.4 reserved `mode: semantic`): reciprocal-rank fusion of vector hits, transcript FTS5 hits (a `transcripts` table in each catalog) and name hits, with probe filters → `{file, fingerprint, startMs, endMs, score, signals: [visual|speech|name], thumb}`.
- **Cloud embedders** (D29): `embedders` contributions (8G) such as Gemini or Qwen3-VL APIs, opt-in per library through `library.set embeddings` (owner, cp §4.5), with a cost estimate from the provider catalog shown before indexing starts.

### 8.7 Reframe (`@finisher/reframe`, 8F)

| Stage | Model / method | Licence |
|---|---|---|
| Faces | MediaPipe BlazeFace short- and full-range (model card: Apache-2.0), converted to ONNX | Apache-2.0 |
| People and subjects | YOLOX-Nano/Tiny (code Apache-2.0; the released weights carry no separate licence and are taken under the repository's, recorded as such) | Apache-2.0 |
| Tracking | ByteTrack-style association (Kalman + IoU), own Rust implementation | ours |
| Smoothing | per shot: dead zone, critically damped spring, maximum pan speed, hold on lost tracks; resets at scene cuts | ours |

- **Excluded:** Ultralytics YOLO models (AGPL-3.0), InsightFace SCRFD/RetinaFace weights (non-commercial), MediaPipe Face Mesh is not needed.
- **Subject choice:** the largest stable face, else the most central person; later the active speaker (diarization × face overlap) once both exist.
- **Artifacts:** `Tracks {rate, tracks: [{id, kind: face|person, samples: [{frame, box, score}]}]}` and `CropPath {aspect, rate, keys: [{frame, cx, cy, scale}]}` (normalised to the source frame).

### 8.8 From results to ops (fin-core builders, no new primitive ops)

| Builder (`fin-core::compose`) | From | Ops | `fin` |
|---|---|---|---|
| `captions_from_transcript` (wraps `caption_import`) | `Transcript` word range of a clip, mapped through `sourceIn` and `speed` | `caption.*`, `item.insert` | `fin captions add --from-transcript c12 [--words w:a..w:b]` |
| `caption_resync` | a caption block's `source` and the current transcript | `caption.setWords` | `fin captions resync <item>` (core §5.5 "explicit edit") |
| `paper_edit` | kept or removed word ranges (`w:182..w:240`, `--quote`) | `item.split`, `item.delete` (ripple) | `fin transcript cut …`, `fin transcript keep …`, `--no-fillers` |
| `split_at_scenes` | `Scenes` within an item | `item.split` | `fin scenes split <item>` |
| `markers_from` | `Scenes`, `Beats` | `marker.add` | `fin scenes markers`, `fin beats markers` |
| `duck` (media §8.5) | `AudioClasses`/VAD + a ducking preset | `keyframe.set` on gain | `fin audio duck <music track>` |
| `reframe` | `CropPath` + item | `param.set`/`keyframe.set` on transform and crop, keys thinned by Ramer–Douglas–Peucker to ≤ 4 per second | `fin reframe <item> --aspect 9:16` |

- **Word addressing** (arch §9.2): `w:<n>` resolves to the word of the transcript of the clip's asset under the cursor item (or `--item`), echoing the resolved text and times.
- Every builder is pure, dry-runnable and produces ordinary ops: undo, merge and review treat them like any edit.

---

## 9. Generation providers (8G)

### 9.1 Contribution and catalog

`providers: [{id, modalities: [image|video|voice|music|sfx|dubbing], catalog: "catalog.json"}]`; `embedders: [{id, modality: image|text, dims, catalog}]`. The **catalog is data**, signed with the package (arch §14.3): `models: [{id, modality, params (JSON Schema), cost: {unit: call|second|character|image|token, amountMicros, currency}, terms (URL), outputLicence: {spdx | "provider-terms", commercialUse, attributionRequired}, limits, outputs: [mediaTypes]}]`.

### 9.2 Calls and paid-call safety

1. `provider.estimate {model, params}` → `{costMicros, currency, basis}`; budgets and approvals (sub-project 7) decide; human calls from the console or UI need no approval card.
2. The host creates the job (cp §6.1) with state `submitting` and an idempotency key **persisted before** `provider.submit {model, params, idemKey}` is sent; the plugin passes the key to the provider where the API supports one.
3. `submit` → `{externalRef}`; the host persists it into the job's `externalRef` before polling (arch §14.3, cp §6.4).
4. `provider.poll {externalRef}` with backoff → `{state, progress?, etaS?}`; `provider.cancel`; `provider.fetch` → files in the job directory.
5. **A crash between 2 and 3** leaves `submitting` without a ref: the host never re-submits. It calls `provider.find {idemKey}` when the plugin implements it, else fails with `GEN_SUBMIT_UNKNOWN` naming the key, for a person to check the provider's dashboard.
6. Outputs are probed, moved into `workspace/<session>/gen/` (or `workspace/shared/gen/`) and registered by `asset.add` with **provenance** `{provider, model, paramsHash, prompt (content), costMicros, currency, outputLicence, externalRef (keyed hash), createdAt}` (arch §8.5).

### 9.3 Credentials and egress (`ModelGateway`, arch §5, §16)

- **Brokered (default):** a provider's HTTP goes through `net.fetch` to declared origins. The manifest declares how a secret is applied (`{secret, header, scheme}`); the host reads the secret from the `SecretStore` at call time and adds the header. The plugin never sees the key. Streaming responses, multipart uploads and WebSockets are supported by the broker (WebSocket as a framed channel over the protocol).
- **Raw grants** (`secrets` with `mode: raw`, a `high`-risk permission): `secret.grant {name}` returns the value for one call; the plugin must not persist it; every grant is audited (`grant`, cp §3.3 reserved). For SDKs that cannot be brokered (owner question 3).
- **The cloud `ModelGateway`** (finisher-cloud) replaces the secret lookup with managed credits and metering behind the same broker; nothing in a plugin changes.

### 9.4 First-party providers

Arch §26 question 8 is answered here as owner question 6. Each first-party provider is a Node plugin in the bundled registry with brokered egress only.

### 9.5 Conformance (`provider` profile)

A fake provider API: estimates match the catalog, a crash after submit never re-submits, `find` recovers, cancel stops billing calls, poll backoff respects `Retry-After`, fetched files match declared types and sizes, no secret appears in the plugin's logs or outputs.

---

## 10. Media interfaces (with sub-project 6)

### 10.1 Codec backends

- **Contribution:** `codecBackends: [{id, decode: [{codec, profiles, bitDepths, chroma}], encode: [...], demux: [containers], mux: [containers], priority, bitExact, licence}]`. The capability report (media §6.2) lists plugin backends beside the built-in ones, and format templates still ask for capabilities, never backends (media §9.2).
- **Protocol:** sub-project 6's process backend (media §6.1) is the host side: `codec.open {kind, stream, config}` → `{session, caps}`; packets in and frames out through mapped rings (`codec.packets`, `codec.frames`, `codec.flush`, `codec.close`); `demux.open` and `demux.read` work on a read-only grant of the one source file (`media:file`), never a directory.
- **Licence:** a backend's manifest licence is shown in the plan; copyleft backends (the FFmpeg plugin) need the copyleft acknowledgement and come only from a registry the owner added (the copyleft repo's).
- **Conformance:** media §13.5 as the `codec` profile of `fin-plugin-test`; a backend that fails is installable but not selectable (`CODEC_BACKEND_NONCONFORMANT` in the capability report).

### 10.2 Importers and offline audio processors

- `importers` turn a file Finisher cannot read into a new asset (for example an MXF rewrap) through a job; the output is probed and registered like generated media.
- `audioProcessors` (de-noise, voice isolation) render a processed copy of an item's audio range as a new audio asset through a job (for example RNNoise, BSD-3-Clause, or DeepFilterNet, MIT OR Apache-2.0); the builder swaps the item's audio to it. They are not real-time effects (owner question 2).

### 10.3 Matte producers

- **Contribution:** `matteProducers: [{id, subjects: [person|foreground|object|key], prompted, temporal, maxSide, resources}]`.
- **Protocol:** `matte.run {asset, range, size, prompts?}` in a job; frames in (rgb8 ring from the host's decode), alpha frames out (gray8 ring), **exactly one per source frame, in order**; the host encodes the luma video with sub-project 6's encoder at the source rate (media §11.3).
- **Where mattes live:** owner question 4 (recommended: a matte is registered as an asset of kind `matte` in `assets/`, so it travels with the project and export never depends on a cache).
- **Reference producer:** `@finisher/key-matte` (Rust, no model): chroma and luma keying with spill and edge controls, which doubles as the conformance fixture and gives green-screen users a v1 feature.
- **Conformance (`matte` profile):** frame count and order, value range, size, declared determinism, cancel within 2 s, crash recovery.
- **After v1:** background removal from BiRefNet (MIT), MODNet (Apache-2.0), SAM2 (Apache-2.0) or MediaPipe (Apache-2.0) (arch §12.6); **excluded:** RVM (GPL-3.0; copyleft repo only) and RMBG-2.0 (non-commercial).

### 10.4 WGSL effects, transitions and LUT packs

- A content pack's `effects` entry is `{kind, wgsl, params: [ParamSpec]}` with media §5.1's ABI (`fn effect(uv: vec2f, src: vec4f) -> vec4f` plus a uniform block generated from the ParamSpecs). Validation at install: `naga` (MIT OR Apache-2.0) parses and validates the module, refuses bindings, storage buffers, atomics and textures beyond the ABI's, and limits the uniform block; render workers keep their GPU watchdog and kill a hung shader.
- **The param registry accepts plugin specs** (core §5.2): `ParamSpec.path` and `Scope::Effect.kind` move from `&'static str` to an interned name, and plugin kinds are namespaced `@publisher/pack:kind`.
- **`plugin.pin`, `plugin.unpin`, `plugin.repin`** (new fin-core ops, modelled on `font.pin`): a project that uses a pack's effect pins its `{id, version, sha256}`, so every machine renders the same shader and a missing pack is reported by item (`PLUGIN_UNPINNED`, `PLUGIN_MISSING`), never rendered differently. This is the per-project lockfile of arch §14.4, kept in the op log so it branches, merges and syncs like every edit.

---

## 11. Diagnostics and training capture (arch §21.1)

- **Who emits:** the host (`source: finisherd` or `finNode`) only. Plugin processes cannot write the journal and never learn the consent state; no `Source::Plugin` is added. A commit a plugin command produced carries the invoker's actor and `Tx.via`; a job a plugin started carries `ctx.actor.kind: plugin`.
- **Plugin references:** a bundled plugin, analyzer, model or provider is a `Name` from new closed tables (`BUNDLED_PLUGINS`, `ANALYZERS`, `MODELS`, `PROVIDERS`); any other plugin, command or model id is free text from a registry, so it is a keyed `Hashed`, never `meta` text.
- **Kinds** (all `meta` unless noted):

| Kind | Batch | Payload |
|---|---|---|
| `plugin.lifecycle` | 8A | `action: plan\|install\|update\|downgrade\|remove\|enable\|disable\|revoke`, `outcome`, `code?`, `plugin: PluginRef`, `registry: bundled\|local\|git\|dev`, `kind: code\|content`, `permissionsAdded`, `risk`, `bytes` (log₂ bucket), `micros` |
| `registry.refresh` | 8A | `registry` kind, `outcome`, `code?` (`TUF_*`), roles updated, targets count bucket, `stale`, `micros` |
| `plugin.stats` | 8B | per minute, per plugin and method family: calls, errors by code, timeouts, crashes, p50/p95 micros, bytes in and out buckets, egress calls per origin class |
| `plugin.exit` | 8B | `plugin`, `reason: ok\|crash\|timeout\|killed\|oom\|protocol\|sandbox\|idle`, exit code or signal, uptime |
| `plugin.command` | 8B | `plugin`, `command: PluginRef`, `outcome: accepted\|rebased\|conflict\|refused\|invalid\|timeout\|data`, op-kind counts, `txId`; training: `args: Content<toolArgs>` |
| `analysis.run` | 8C | `analyzer`, `models`, `language: Name` (BCP-47 table), `outcome`, `code?`, input seconds or frames, real-time factor, provider class, cache hit, attempts, `artifact: IdRef` (keyed recipe) |
| `model.fetch` | 8C | `model`, `bytes`, `outcome`, `code?`, `resumed`, `micros` |
| `gen.call` | 8G | `provider`, `model`, `modality`, `phase: estimate\|submit\|poll\|fetch\|cancel\|find`, `outcome`, `code?`, cost bucket, latency; training: `prompt: Content<prompt>`, `params: Content<toolArgs>`, `outputs: Content<mediaHash>[]` |

- **Training capture of plugin traffic** is typed, never raw HTTP: command arguments, provider prompts and parameters, output media hashes; LLM turns of agent-provider plugins go through sub-project 7's `agent.*` kinds like every provider's. Brokered egress bodies are never journaled (they may hold secrets or other people's data).
- **Never recorded:** plugin stderr, registry URLs or paths, publisher names, package file names, model URLs, prompts outside `Content`, transcripts and other artifact payloads (they stay in the cache; the dataset builder joins them locally through `analysis.run.artifact`).
- **Audit log** (cp §3.3): `plugin.install|update|remove|enable|disable|revoke|crashLoop`, `plugin.grant` (with the permission diff), `registry.add|remove|trust`, `registry.verifyFail`, `grant` (raw secret grants), `hook.modify`, `sandbox.violation`. Plugin ids and registry locations may appear in audit rows (they are not personal data); secrets and tokens never.

---

## 12. Security and threat model

Arch §16 priority: injected agent > malicious plugin > stolen device > network attacker. Sub-project 8 adds the supply chain.

| Threat | Mitigation | Residual |
|---|---|---|
| Malicious plugin reads or writes the user's files | process + OS sandbox (§15) with only `pluginData`, job and declared `workspace` roots; no source paths (media arrives decoded); Node permission flags as a second layer | before sub-project 7, only bundled and developer plugins run (owner question 1) |
| Plugin writes the project behind the user's back | ops only, through `tx.commit` under the invoker's principal ∩ grants; `Tx.via`; protected branches (cp §8.5) | a granted plugin can make bad edits; they are reviewable and undoable |
| Plugin exfiltrates media or prompts | `net` only to declared origins through the broker; origins shown at install; egress counted per plugin | a granted origin can receive what the plugin sends |
| Plugin steals credentials | brokered injection; raw grants per call, audited, opt-in permission; no env, no credential files | a raw grant can be misused for that call |
| Compromised publisher key | per-publisher terminating delegations; rotation by the top-level targets role; `disable` advisories | versions installed before the advisory ran until the next refresh |
| Compromised registry host or git remote | TUF: threshold signatures, consistent snapshots, rollback and freeze checks, length caps | freeze on `relaxed` registries up to `maxStaleDays` |
| Rollback to a vulnerable version | trusted versions never decrease; immutable versions; downgrade is a person's exact choice | — |
| Malicious archive (zip slip, symlinks, bombs, reserved names) | strict reader (§6.4) into a staging directory | — |
| Typosquatting and look-alike names | ASCII-only ids, reserved publishers, edit-distance warning against installed and bundled ids in the plan | a person can still approve |
| Injected agent installs, enables or widens a plugin | `plugin.install`/`plugin.admin` are owner scopes; plans from non-humans are refused; suggestions only | owner-enabled `agentInstall` for low-risk packages |
| Hostile model file (ORT parser bugs) | models only from signed manifests, pinned by SHA-256; inference runs inside the sandboxed plugin process | an ORT exploit runs with the plugin's rights |
| Hostile media exploiting a plugin decoder | decoding happens in sub-project 6's workers; plugins receive pixels and samples | codec plugins parse files and rely on the sandbox |
| Plugin starves the server | per-job resources and pools (cp §6.3), memory limits, idle stop, crash-loop disable | — |
| Hook `modify` rewrites agent commands | per-hook `hooks:modify` grant; both payloads logged (arch §10) | — |
| Python supply chain | wheels only, hashes from the signed lock, no builds, no network at run time without `net` | a malicious wheel pinned by its publisher |

---

## 13. Error codes

| Exit | Codes |
|---|---|
| 2 | `MANIFEST_TOO_LARGE`, `PLUGIN_MESSAGE_TOO_LARGE` |
| 3 | `PLUGIN_NOT_FOUND`, `PLUGIN_NOT_INSTALLED`, `PLUGIN_MANIFEST_INVALID {path, reason}`, `PACKAGE_UNSAFE {entry, reason}`, `PACKAGE_TOO_LARGE`, `PLUGIN_HOST_API {range, host}`, `PLUGIN_PLATFORM {platforms}`, `PLAN_EXPIRED`, `PLAN_MISMATCH` (staging changed since the plan), `REGISTRY_NOT_FOUND`, `REGISTRY_INVALID`, `PLUGIN_COMMAND_ARGS`, `PLUGIN_TX_INVALID` (the plugin's transaction failed validation; `details.error`), `ARTIFACT_NOT_FOUND`, `ANALYZER_NOT_FOUND`, `ANALYSIS_LANGUAGE_UNSUPPORTED`, `WORD_RANGE_INVALID`, `PLUGIN_UNPINNED`, `GEN_PARAMS_INVALID` |
| 4 | `PLUGIN_CRASHED`, `PLUGIN_TIMEOUT`, `PLUGIN_PROTOCOL`, `ANALYSIS_FAILED`, `GEN_PROVIDER_ERROR {status, providerCode?}`, `GEN_SUBMIT_UNKNOWN {idemKey}`, `MODEL_HASH_MISMATCH`, `ARTIFACT_STALE` (a newer recipe exists) |
| 5 | `PLUGIN_DISABLED`, `PLUGIN_CRASH_LOOP`, `PLUGIN_RUNTIME_UNAVAILABLE` (no sandbox, no Python, no Node), `PLUGIN_MISSING`, `REGISTRY_UNREACHABLE`, `REGISTRY_STALE` (warning), `MODEL_DOWNLOAD_FAILED`, `MODEL_MISSING`, `GEN_RATE_LIMITED`, `CLOCK_SUSPECT` |
| 7 | `PLUGIN_PERMISSION_DENIED {perm}`, `PLUGIN_REVOKED {advisory}`, `REGISTRY_UNTRUSTED`, `TUF_SIGNATURE`, `TUF_THRESHOLD`, `TUF_ROLLBACK {role, trusted, got}`, `TUF_EXPIRED {role}`, `TUF_TARGET_MISMATCH {hash \| length}`, `TUF_DELEGATION` (a role signed outside its paths), `PERMISSIONS_NEED_APPROVAL {diff}`, `COPYLEFT_NOT_ACCEPTED`, `EGRESS_DENIED {origin}`, `SECRET_MISSING {name}` (fix: the console), `GEN_BUDGET_EXCEEDED`, `GEN_APPROVAL_REQUIRED`; plus sub-project 5's `HUMAN_ONLY` for installs from agents |

All codes go into `crates/fin/src/output.rs` and `@finisher/contracts`' code list in the batch that introduces them. TUF failures are exit 7 ("do not retry; a person must look"), not 5.

---

## 14. Test strategy

### 14.1 Layers

| Layer | What | Where |
|---|---|---|
| Manifest and packages | grammar and schema property tests; every contribution's validator; the strict reader against a corpus of hostile archives (zip slip, symlink entries, `CON`, case-folded duplicates, bombs, NFD names, absolute paths) | `fin-plugin-api`, `fin-registry` |
| TUF | repositories generated in tests with the editor and throwaway keys: valid chain; tampered target; wrong length; rollback of each role; expired timestamp (strict and relaxed); mix-and-match snapshot; root rotation 1→2→3 with threshold changes; timestamp-key rotation with fast-forward recovery; publisher delegation escape; revoked publisher; endless data; advisories | `fin-registry` |
| TUF conformance | the `tuf-conformance` suite (MIT OR Apache-2.0, Python, CI-only) against a small client binary over `fin-registry` | CI, Linux |
| Lifecycle | plan → approve → install; widening update needs approval; agent install refused; bundled plugins verified at start; disable/remove; advisories disable; content packs activate their contributions | `finisherd` vitest, e2e `plugins` |
| Host and protocol | protocol round-trips from generated types; permission matrix per host method; crash, hang, oversize message, crash loop; lazy start and idle stop; `Tx.via`; a plugin command cannot exceed its invoker's scopes | 8B |
| SDKs | the example plugins of each SDK pass their conformance profiles | 8B, 8H |
| Analyzers | **A0** pure Rust stages bit-exact by hash on the three OSes and wasm32 (detectors, beat DSP, Viterbi, boundary trees, tracking, smoothing); **A1** ONNX stages on fixtures: identical transcript text across OSes, word times within ±10 ms, embeddings cosine ≥ 0.999 to the reference, detection boxes within 2 px; **A2** quality metrics on evaluation sets (§14.3) as trends, with the speech gate as a merge gate | 8C–8F |
| Builders | golden ops for each `compose` builder, inverses, and inclusion in the 4B property generators | fin-core |
| Providers, codecs, mattes | conformance profiles against fakes; the key matte against generated footage | 8G, 8H |
| Security | sandbox escape tests for plugins (the arch §22 suite run with a plugin process), secret-leak grep over logs, journal and outputs | 8B with 7 |

### 14.2 Fixtures

- No media in the repository (media §2.8): PCM and frames are generated; speech fixtures come from the permissive corpora below, downloaded into a local fixture cache by a script that verifies hashes and licences.
- Test TUF keys are generated per test run; a few fixed vectors (to pin canonical JSON and signatures across platforms) use keys whose names say `test-only`.

### 14.3 Evaluation data (all permissive; AGENTS.md)

| Use | Sources | Licence |
|---|---|---|
| ASR, alignment | LibriSpeech, Multilingual LibriSpeech, Common Voice, our MFA references | CC BY 4.0, CC BY 4.0, CC0 |
| Music under speech | MUSAN music, per-file licences filtered (no NC, ND, SA) | CC BY 4.0 corpus |
| Human-labelled boundaries | the owner's footage and the owner-generated reference clips of arch §27.4, evaluation only, never committed | the owner's |
| Scenes, reframe, search | Blender Foundation open films (CC BY), the owner's rights-cleared footage, with our own labels | CC BY / the owner's |
| Beats | synthetic click and drum tracks; CC BY music from MUSAN with our labels | ours / CC BY 4.0 |

**Excluded:** GTZAN (copyright unclear), MUSDB18 (NC), BBC Planet Earth shot sets (NC), WIDER FACE (NC), TIMIT and Buckeye (restricted), VoxCeleb only where its terms are verified for the use.

### 14.4 Budgets (validation gates on the reference machines of media §14)

| Area | Budget |
|---|---|
| Registry refresh, local registry with 500 targets | ≤ 300 ms |
| Install of a 50 MiB package after the plan | ≤ 2 s |
| Plugin cold start to `initialize` answered (native) | ≤ 150 ms; Node ≤ 400 ms |
| Command round trip (warm, small snapshot) | ≤ 20 ms p95 |
| Transcription + alignment, CPU | RTF ≤ 0.15 (measured ≈ 0.1 English, 0.14 multilingual) |
| Scenes at 256 px | ≥ 20× real time |
| SigLIP2 embedding, CPU | ≥ 3× real time at one keyframe per 2 s |
| Vector search, 1M chunks | ≤ 50 ms p95, recall@10 ≥ 0.95 against exact |
| Reframe (detect every 2nd frame at 480 px + track) | ≥ 2× real time on CPU |

---

## 15. What sub-project 8 needs from sub-projects 5, 6 and 7

| From | Needs |
|---|---|
| 5A | principals with a reserved `plugin` kind; the scope table (adds `plugin.read|install|admin|suggest`); human-only enforcement; the console; the audit log |
| 5C | job kinds with resources, `externalRef`, `dedupeKey`, idempotency, retries and bisection; worker processes with hard kill; `jobs.watch` |
| 5D | the library stage framework keyed by fingerprint and stage version; catalog tables for transcripts (FTS5) |
| 5E | the reserved `pluginSuggestion` review item kind |
| 6 | PCM at 16 kHz mono through the audio cache (6C); a frame feed role at analysis sizes (a `fin-worker` role, 6A–6B); the process codec backend and its conformance suite; matte encoding and the `matte` asset kind (6D, owner question 4); the WGSL effect ABI (6D) |
| 7 | the OS sandbox broker for plugin processes (write roots: plugin data, job directory, declared workspace; read roots: the plugin's store directory and mapped inputs; network off unless brokered egress is the only path); the hook engine calling plugin handlers; budgets and approval rules for `gen`; the agent-provider adapter contract |

---

## 16. Crate and file map

| Crate / package | New? | Responsibility | wasm32 |
|---|---|---|---|
| `fin-plugin-api` | new (8A) | manifest, permissions, contributions, licence classification, validation; protocol types (8B); generated JSON Schema | yes, pure |
| `fin-registry` | new (8A) | TUF client wrapper, filesystem and git-tree transports, trusted store, strict package reader, store and staging, advisories | no |
| `fin-plugin-cli` (`fin-plugin` binary) | new (8A) | publisher tooling: pack, validate, registry editing and signing | no |
| `fin-core` | changed | `analysis/` artifact types and `ArtifactRef` (8C); `compose` builders (8C–8F); `Tx.via` (8B); `plugin.pin|unpin|repin` and interned param names (8H); new event kinds and name tables | yes |
| `fin-node` | changed | napi `Registry` (plans, installs, refresh) on the worker thread; manifest validation; the analysis cache index (8C) | — |
| `fin-plugin-sdk` | new (8B) | Rust SDK | no |
| `@finisher/plugin-sdk` | new (8B) | TS SDK | — |
| `fin-plugin-test` | new (8B) | harness and conformance profiles | no |
| `fin-analyze` | new (8C) | bundled analyzers binary: ONNX glue, speech, scenes, audio, search, reframe, key matte | no |
| `finisherd` | changed | `src/plugins/` (registries, lifecycle RPCs, host, processes, broker client, command registry, egress), `src/analysis/` (jobs, stages, views), console `plugin` and `registry` commands | — |
| `fin` | changed | `plugin search|info|ls|validate|suggest`, `plugin install` (exit 7 unless allowed), dynamic plugin commands, `transcript`, `captions add --from-transcript|resync`, `scenes`, `beats`, `audio duck`, `reframe`, `search`, `gen`, `analysis` | — |
| `finisher-plugin` (Python) | new (8H) | Python SDK | — |

**Dependency rules:** `fin-core` and `fin-plugin-api` stay pure (no I/O, clock or randomness); `fin` links neither `fin-registry` nor ONNX; `fin-node` links `fin-registry` but no ONNX; only `fin-analyze` and plugins link ONNX Runtime.

---

## 17. Licences

### 17.1 Dependencies

| Need | Choice | Licence | Batch |
|---|---|---|---|
| TUF | `tough` 0.24 (fallback `tuf` 0.3 beta) | MIT OR Apache-2.0 (crypto `aws-lc-rs`/`aws-lc-sys`: permissive expression, §6.6) | 8A |
| Git | `gix` | MIT OR Apache-2.0 | 8A |
| Archives | `zip` 9 (in the tree) | MIT | 8A |
| JSON Schema validation | `jsonschema` | MIT | 8A |
| SPDX parsing | `spdx` | MIT OR Apache-2.0 | 8A |
| Semver | `semver` | MIT OR Apache-2.0 | 8A |
| ONNX | ONNX Runtime (shared library), `ort` 2.0 rc | MIT; MIT OR Apache-2.0 | 8C |
| WGSL validation | `naga` | MIT OR Apache-2.0 | 8H |
| Vector index | LanceDB (`lancedb`) or `usearch` | Apache-2.0 | 8E |
| Python plugins | `uv`; CPython builds (downloaded on demand, never shipped) | MIT OR Apache-2.0; PSF-2.0 and component licences shown in the plan | 8H |

**Rejected:** Ultralytics, aubio, Essentia, whisper-timestamped (copyleft); `node-forge` (dual GPL); any JWT stack (none is needed); WhisperX and the original PySceneDetect in the shipped app (ffmpeg dependencies; copyleft repo only, arch §18).

### 17.2 Models (the v1 defaults and candidates)

| Model | Use | Weight licence | Attribution | Training-data note | Status |
|---|---|---|---|---|---|
| Parakeet-TDT 0.6B v2 | English ASR | CC-BY-4.0 | **required** (notice + model manager + About; state the ONNX conversion) | NVIDIA's published data mix | default |
| whisper large-v3-turbo | multilingual ASR | MIT | notice | undisclosed web data | default |
| wav2vec2-base-960h | CTC alignment | Apache-2.0 | notice | LibriSpeech (CC BY 4.0) | default |
| boundary correction (ours) | starts | MIT (ours) | data card (LibriSpeech, MLS: CC BY 4.0; Common Voice: CC0; MUSAN: CC BY 4.0) | permissive only | default (retrained in 8C) |
| Silero VAD | VAD | MIT | notice | — | default |
| pyannote segmentation-3.0 | diarization | MIT (gated download) | notice | includes licensed corpora (e.g. DIHARD) | opt-in (owner question 5) |
| 3D-Speaker CAM++ | speaker embedding | Apache-2.0 | notice | — | opt-in with diarization |
| TransNetV2 | dissolves | MIT | notice | — | opt-in |
| YAMNet | speech/music classes | Apache-2.0 | notice | AudioSet labels (CC BY 4.0) | default |
| Beat This! | beats | MIT | notice | partly copyrighted training audio (authors' note) | opt-in (owner question 5) |
| SigLIP2 base | embeddings | Apache-2.0 | notice | WebLI (not released) | default |
| BlazeFace | faces | Apache-2.0 (model card) | notice | — | default |
| YOLOX-Nano/Tiny | people | Apache-2.0 (repository; weights unlabelled) | notice | COCO | default, recorded as such |
| **Excluded** | | MMS (CC-BY-NC), CrisperWhisper (NC), RMBG-2.0 (NC), InsightFace weights (NC), madmom models (CC BY-NC-SA), RVM (GPL-3.0), Ultralytics YOLO (AGPL-3.0) | | | |

- Model licences are recorded in `model.json`, shown before download (arch §18) and checked by a gate: in the bundled and first-party registries, a manifest model whose licence is not on the allow-list (`MIT`, `Apache-2.0`, `BSD-2-Clause`, `BSD-3-Clause`, `CC0-1.0`, `CC-BY-4.0`) fails validation. Use-restricted licences (OpenRAIL and similar), NC, ND and SA licences are refused; third-party registries show the licence in the plan and flag anything off the list.
- `THIRD_PARTY_NOTICES` gains ONNX Runtime, each default model with its attribution, the boundary model's data card, PySceneDetect (BSD-3, ported), and the TUF and git crates.

---

## 18. Decisions taken here

| # | Decision | Section |
|---|---|---|
| 1 | Two kinds: content packs (data, no process) and code plugins (one OS process; per job for analyzers, codecs and mattes) | §3.1, §3.3 |
| 2 | Runtimes `native`, `node` (finisherd's Node with permission flags), `python` (uv, wheels only); `wasm` reserved | §3.2 |
| 3 | JSON-RPC 2.0 over NDJSON stdio; bulk data in mapped files; protocol types generated from Rust | §3.4 |
| 4 | A capability-checked host API; plugins never get a socket to `finisherd`, a source path or a credential | §3.5 |
| 5 | The `plugin` principal acts with grants ∩ the invoker's scopes; four new scopes | §3.6 |
| 6 | Plugin commands return transactions committed under the invoker; `Tx.via` records the plugin | §3.7 |
| 7 | Manifest validation in Rust; ids `@publisher/name`; reserved publishers | §4 |
| 8 | Install is plan → approve: a signed-metadata-checked staging tree, an `InstallPlan` with a digest, owner-only approval; nothing runs at install | §5.3 |
| 9 | Code plugins never auto-update; content packs may, if nothing widens | §5.3 |
| 10 | Bundled plugins come from a bundled TUF registry with a compiled-in root hash | §5.2 |
| 11 | Git registries are read from fetched tree objects; git itself is not trusted | §5.2 |
| 12 | TUF with per-publisher terminating delegations, consistent snapshots and ed25519; `tough` chosen, `tuf` as fallback | §6 |
| 13 | Freshness `strict` for bundled/public, `relaxed` with a signed `maxStaleDays` for local and git | §6.3 |
| 14 | Packages are zip files without their own signature, read by a strict reader | §6.4 |
| 15 | Bundled analyzers are one Rust binary (`fin-analyze`) published as several plugins | §8.1 |
| 16 | Artifacts are fin-core types keyed by recipe hash, cached locally, regenerable; times in integer source ms or source frames | §8.1, §8.3 |
| 17 | One pinned ONNX Runtime from the host; CPU default; GPU providers in the recipe | §8.2 |
| 18 | Speech pipeline of arch §27.4 kept; start correction retrained on permissive data with music mixed in | §8.3 |
| 19 | Analysis becomes ops only through pure fin-core builders; no new primitive op except `plugin.pin|unpin|repin` | §8.8, §10.4 |
| 20 | Paid calls: idempotency key and `submitting` persisted before submit; never re-submitted | §9.2 |
| 21 | Credentials are brokered by default; raw grants are an opt-in, high-risk permission | §9.3 |
| 22 | Plugin effects are pinned per project by ops (the lockfile lives in the op log) | §10.4 |
| 23 | Only the host emits diagnostics; third-party names are keyed hashes; no `Source::Plugin` | §11 |
| 24 | TUF failures are exit 7 | §13 |

---

## 19. Owner questions

1. **May third-party code plugins run before sub-project 7's OS sandbox broker exists?** *Recommended:* no. 8A and 8B install and verify them, but only bundled plugins and developer-mode plugins (`--plugin-dir`, started by a person at the console) run; enabling any other code plugin answers `PLUGIN_RUNTIME_UNAVAILABLE` until the broker lands. *Trade-off:* no third-party commands, providers or codec plugins (including the FFmpeg plugin for Windows HEVC and Linux gaps) until sub-project 7 ships the broker. The alternative runs them with the env allow-list, no credentials and Node's permission flags only, which a malicious native or Python plugin defeats immediately (threat 2 of arch §16).
2. **Is WASM a plugin runtime in v1?** *Recommended:* no: v1 ships process plugins and content packs (as arch §14.1 says), and real-time audio effects are built into `fin-audio` (EQ, compressor, gate, de-esser) while de-noise and voice isolation are offline processors (§10.2). WASM (wasmtime, Apache-2.0 WITH LLVM-exception) comes after v1, mainly for real-time audio effects that must run in both the native mixer and the AudioWorklet. *Trade-off:* arch §12.7 "audio effect chain … as plugins" is only partly met in v1 (no third-party real-time audio effects), and small pure command plugins pay a process start instead of an in-process sandbox. Adding WASM now means a second sandbox model, a component-model SDK in three languages and ~10–20 MB more binary, before the process path is proven.
3. **Do plugins ever see secrets?** *Recommended:* brokered egress by default: the host injects credentials into requests to declared origins, and the plugin never sees a key (§9.3); a raw per-call grant exists only behind a `secrets` permission with `mode: raw`, shown as high risk at install and audited per call. *Trade-off:* the host must proxy streaming, multipart and WebSocket traffic, and vendor SDKs that insist on holding the key need the raw mode. Arch §16's wording ("per-call `ModelGateway` grants") would hand every provider plugin its key on every call.
4. **Do mattes travel with the project?** *Recommended:* yes: a matte a mask uses is registered as an asset of kind `matte` in `assets/` (write-once, synced), and `Mask.matte` names that asset, so export on another machine never depends on a regenerable cache or on a GPU model producing the same pixels twice. *Trade-off:* project folders grow by the encoded matte (a few MB per minute of 1080p luma), and arch §6.2's list of "analysis artifacts" loses mattes. Keeping them as cache artifacts saves space but makes export fail (`MATTE_MISSING`) or differ after a move until they are recomputed. Sub-project 6 (6D) needs the answer before it builds mask edits.
5. **Which models with permissive weights but restricted or undisclosed training data may ship as defaults?** *Recommended:* the weight licence governs; a model is allowed when its weights are MIT, Apache-2.0, BSD, CC0 or CC-BY (with attribution) and its authors place no use restriction; training-data provenance is recorded and shown, and models whose authors warn that training data is copyrighted or licensed (Beat This!, pyannote segmentation) are **opt-in**, not defaults. Defaults stay Parakeet, whisper turbo, wav2vec2, Silero, YAMNet, SigLIP2, BlazeFace and YOLOX. *Trade-off:* diarization and learned beat tracking are off until a user turns them on, and the DSP beat tracker is weaker on dense music. A stricter rule (training data must be permissive too) would also exclude whisper and SigLIP2 (undisclosed data), leaving no viable multilingual ASR or visual search.
6. **Which generation providers ship first-party in v1** (arch §26 question 8)? *Recommended:* ElevenLabs (voice, SFX, music, dubbing) and OpenAI images, as Node plugins with brokered egress; fal.ai (video and an image aggregator) as the first community-style plugin after the contract has been used twice. *Trade-off:* no first-party video generation in v1. Shipping all three in 8G triples the conformance and terms-of-use review, and fal.ai's catalog changes weekly, which tests the "catalog as data" update path harder than v1 needs.

---

## 20. Risks

| Risk | Mitigation |
|---|---|
| `tough`'s `aws-lc-sys` adds a C build (and NASM on Windows) and binary size | 8A Task 1 gate on all three OSes; `tuf` (ring) fallback behind one trait |
| TUF expiry is unworkable for team registries | `relaxed` freshness with a signed `maxStaleDays`; `fin-plugin registry resign-timestamp` in CI recipes |
| `ort` 2 stays a release candidate | exact pin, one wrapper module, our own ORT build per platform |
| Speech models are large (Parakeet + whisper turbo + wav2vec2: several GB at fp32) | on-demand download per content language; int8 where the gate passes; shared model store |
| ONNX results differ across OSes and providers | CPU default, providers in the recipe, A1 tolerances, cross-OS fixture runs |
| The boundary correction overfits MFA conventions again | gate-window objective, human-labelled held-out set, music augmentation; the spike's raw-end rule stays |
| LanceDB's dependency tree (Arrow, DataFusion) bloats the build | 8E measurement against `usearch` and exact scan, with budgets |
| Plugins wait on sub-project 7's broker | owner question 1; bundled analyzers do not depend on third-party code |
| Brokered egress misses an API shape a provider needs | raw grants (owner question 3); the provider conformance profile exercises streaming and uploads |
| Node plugins under Electron's runtime differ from plain Node | the Node runtime is finisherd's own in both cases; e2e runs a Node plugin in the packaged app (sub-project 9 validation) |
| Git registries behind authentication | HTTPS tokens from the `SecretStore`; SSH through the user's `ssh` |

---

## 21. Contradictions and gaps found in earlier documents

1. **Arch §21.1 `source`** has no `plugin` variant (the brief for this sub-project expected one); `ActorKind::Plugin` exists. Resolved by §11: the host emits plugin-related events and labels them with the actor and `Tx.via`; no new source, so plugins cannot forge events. (Sub-project 5 adds `console`, sub-project 6 `worker`.)
2. **Arch §13 names whisper.cpp and sherpa-onnx** for transcription and §18 lists both in `THIRD_PARTY_NOTICES`, while §27.4 chose Parakeet-TDT and whisper turbo through ONNX Runtime. Resolved by §8.3: one ORT through `ort`, decoders in Rust; neither whisper.cpp nor sherpa-onnx is shipped (sherpa-onnx's C API is the fallback if a decoder port fails its gate).
3. **Arch §13 alignment target "p95 under 50 ms"** is superseded by §27.4's window gate; §13 and §22 should point to it.
4. **Arch §6.2 lists mattes among regenerable analysis artifacts**, but masks reference them and export must read them on any machine (media §11.3 `MATTE_MISSING`). Owner question 4.
5. **No artifact type exists in fin-core**; `CaptionSource.artifact` and `Mask.matte` are free strings. Resolved by `ArtifactRef` and `fin-core::analysis` (§8.1).
6. **Arch §14.4 "per-project lockfile"** has no sync-safe format; a file in the project folder would need its own conflict rules. Resolved by `plugin.pin` ops (§10.4), following `font.pin`.
7. **Arch §15 "Package: signed archive"** vs TUF, where signatures cover metadata and the archive is identified by hash and length. Resolved by §6.4.
8. **Arch §15 "git and local registries use the same verification"** omits that TUF timestamps expire within days, which git-hosted and folder registries cannot meet without CI. Resolved by `relaxed` freshness (§6.3).
9. **Arch §16 "secrets … except per-call `ModelGateway` grants"** hands keys to plugins; §9.3 proposes brokered injection instead. Owner question 3.
10. **Arch §12.7 "audio effect chain (EQ, compressor, limiter, de-noise as plugins)"** cannot run in the WASM AudioWorklet preview if plugins are processes. Owner question 2.
11. **Arch §14.1 "code plugins in Rust, Node, or Python"** says nothing about which Node runs them; under Electron that is Electron's binary with `ELECTRON_RUN_AS_NODE=1`, which arch §27.7 says must never be *inherited*. Resolved by §3.2: set explicitly for Node plugin children only.
12. **Arch §14.2 "plugins cannot define primitive ops"** and core §5.2's registry of `&'static str` ParamSpecs: plugin effects need runtime-declared params. Resolved by interned names and namespaced kinds (§10.4).
13. **Two worker and job designs:** sub-project 5C builds `fin-workers` and a general job system; sub-project 6 builds `fin-worker` with its own media jobs until 5C absorbs them (media owner question 5). Sub-project 8 uses 5C's jobs for analysis and generation and sub-project 6's worker only as the decode service; the two binaries' names and the absorption should be settled before 8C.
14. **Arch §13 "Models download on demand … licences recorded"** does not cover CC-BY attribution in the app's UI, which Parakeet requires. Resolved by §17.2.
15. **Arch §27.4's human-labelled reference set** includes generated clips from a commercial TTS service; they are evaluation-only, owner-held and never committed, and AGENTS.md's "permissively licensed evaluation data" should say whether owner-held, uncommitted references are allowed (this design assumes yes, and adds permissive public sets for everything in CI, §14.3).
16. **Arch §8.3 "agents may … suggest a plugin, which appears as a UI suggestion"** needs a carrier; cp §8.1 reserved `pluginSuggestion` review items without a producer. Resolved by `plugin.suggest` (§5.3).
17. **Arch §13 uses LanceDB** without a budget for its build and binary cost; 8E measures it (§8.6).
18. **The 4C plan defers "catalogs supplied by the registry, and signed catalogs" to sub-project 8**: resolved by `fontCatalogs` content contributions verified through TUF (§4.2).
