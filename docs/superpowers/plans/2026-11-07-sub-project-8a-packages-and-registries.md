# Sub-project 8A: Packages, registries and lifecycle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the TUF client and its attack matrix, the strict package reader, manifest validation and permission diffs, plans and approvals, trust decisions, validation), `sonnet` for plumbing (publisher CLI, control-store tables, RPCs, console, `fin`, content consumers, e2e, docs).

**Revision 2** (after the review of `848f01f`): `tuf` is no longer the fallback; Task 1 builds `fin-node` with `fin-fonts` and `tough` and fetches over HTTPS on three OSes; the git transport, agent installs, TUF for bundled plugins and validation of unconsumed contributions are cut; Tasks 4, 9 and 12 are split; the editor is a non-test module; `maxStaleDays` defaults to 7; agent-facing content is medium risk and never auto-updates.

**Owner decisions (2026-11-07)** (spec §20): third-party code plugins are allowed before 7C, so 8A no longer gates enabling them on a sandbox (no `SANDBOX_UNAVAILABLE`); instead a third-party code plugin's plan is high risk with a `thirdPartyCode` warning and needs `accept.thirdPartyCode` (Decisions 5, 19, 21; Tasks 8, 9b, 12b, 13). Node is 24.22 (the repository's Node 24 target).

**Goal:** Finisher can verify, install, update, enable, disable and remove plugins from bundled sources and local-folder registries, with every registry package checked through TUF metadata and every permission approved by a person. Nothing a plugin ships runs in 8A: content packs with consumers become active (font catalogs and caption styles), code plugins install but cannot be enabled until 8B, which runs them (registry-installed ones too, before 7C's `plugin` profile: spec owner decision 1); a third-party code plugin installs only with an explicit high-risk acceptance. After this plan:
- `fin-plugin-api` (pure) defines the manifest, permissions, risks, contributions and licence classes, validates them in Rust, and generates their TypeScript types;
- `.finpkg` packages are read by a strict reader into a staging directory, and packed reproducibly;
- `fin-registry` runs the TUF 1.0 client workflow over local folders (a git registry is a folder the user pulls), keeps trusted metadata per root (also after a registry is removed), refuses rollback, freeze beyond `maxStaleDays`, mix-and-match, delegation escapes and tampered targets, reads signed yanks and advisories, and has a test-only HTTP transport for `tuf-conformance`;
- `fin-registry::editor` and `fin-plugin registry …` build and sign registries for teams and tests;
- `finisherd` keeps registries, installed plugins and grants in `control.db`, runs **plan → approve → install**, installs bundled plugins at start after checking compiled-in hashes, audits every change and journals `plugin.lifecycle` and `registry.refresh`;
- `finisherd plugin|registry …` (console, owner) and `fin plugin search|info|ls|validate|suggest|install` (install is always `HUMAN_ONLY` for agents).

**What can start now** (no sub-project 5 code needed): **Tasks 1–3** at once; **Tasks 4–7 after Task 1**; **Task 8** (its `fin-registry` and `fin-node` parts, without `finisherd`); **Task 11** (fin-core event kinds). Tasks 9a, 9b, 10, 12a–12c and 13–15 need `sp5a-principals`.

**Architecture:**
- **`crates/fin-plugin-api`** (new, pure, wasm-safe): `manifest/` (types, ids, versions, runtimes, contributions), `permission.rs` (closed permissions, risks including combinations, diff), `licence.rs` (SPDX classes), `contribution.rs` (the consumable-kind table per host API), `validate/` (one module per rule family), `schema.rs`.
- **`crates/fin-registry`** (new, I/O): `package/` (strict reader, packer), `tuf/` (the `TrustedRepo` wrapper over `tough`, trust store per root, freshness, clock check), `editor/` (repository building and signing: the non-test "RepoBuilder", used by `fin-plugin` and tests), `transport/` (`fs`; `http` behind feature `test-http`), `registry.rs`, `advisory.rs`, `store.rs`, `plan.rs`.
- **`crates/fin-plugin-cli`** (new binary `fin-plugin`, not shipped).
- **`fin-node`** gains `registry.rs` (a napi `Registry` class on the per-engine worker thread), `validateManifest`, `bundled.rs` (the compiled-in `BUNDLED_PLUGINS` hash list); the font manager accepts extra catalogs.
- **`fin-core::events`** gains `plugin.lifecycle` and `registry.refresh` (v1), `PluginRef`, and the `BUNDLED_PLUGINS` and `PLUGIN_ACTIONS` name tables.
- **`finisherd`** gains `src/plugins/` (`registries.ts`, `lifecycle.ts`, `plans.ts`, `content.ts`, `bundled.ts`, `rpc.ts`), migration `control/migrations/m00NN-plugins.ts`, scope rows, console `plugin` and `registry` commands.
- **`fin`** gains `commands/plugin.rs` and `--from @pub/pack:<style>` on `style add`.

**Tech Stack:**
- Rust 2024; `tough` 0.24 (MIT OR Apache-2.0; `http` feature off) and its `aws-lc-rs` crypto, or, if Task 1's gate fails, a fork of `tough` with its key parsing, signature verification and signing moved onto `ring`; `tokio` (a current-thread runtime inside `fin-registry` only); `zip` 9 (in the tree); `spdx`, `semver` (MIT OR Apache-2.0); `blake3`/`sha2` (in the tree); for `test-http` only, a minimal HTTP server and client already in the dev tree (Task 5 picks it and records the licence).
- Node 24.22 (`node:sqlite`; the Node Electron embeds, and the repository's Node since 2026-11-08), Effect 4.0.6, vitest. Only APIs present in Node 24.22 are used.
- **No other new dependency.** No `gix`, no `jsonschema` (8B), no `tuf`. Each task that adds one runs `cargo deny` and the JS licence gate and records the licence in its commit body.

**Spec:** `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §1, §3.7 (scopes), §4–6, §11–13, §15, §17–20. Parent: arch §8.3, §14, §15, §16, §18, §21.1. Related: sub-project 5 spec §3.1–3.3, §4.4–4.6 (branch `sp5-design`); sub-project 7 spec §7.2 (`deny plugin.install`), §10 (`SecretStore`) (branch `sp7-design`); the 4C plan Decision 30 (font catalogs).

**Branch:** `sp8a-packages`. Tasks 1–8 and 11 run on a branch from `sp4c-edit-breadth` and are rebased onto **`sp5a-principals` after its Task 12** before Task 9a; Tasks 9a onward need 5A's `control.db`, audit log, scope table, human-only enforcement and console.

**Out of scope:** running code plugins, the protocol, the `plugin` principal at run time, SDKs, `fin-plugin-test` (8B); every contribution kind without an 8A consumer (refused, spec §4.2); `plugin.pin` ops and WGSL effects (8H); analysis (8C+); providers (8G); HTTPS registries outside tests; git, Python, agent installs (cut); the UI dialogs (9); release signing of the app and its bundled plugins (11).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate.
- **Primitive ops live only in `fin-core`.** This plan adds none.
- **`fin-plugin-api` stays pure:** no I/O, clock, environment or randomness; it builds for `wasm32-unknown-unknown` (added to `scripts/check-wasm-deps.mjs`).
- **TypeScript model types are generated, never hand-written.** Manifest, permission, plan, registry status and event payload types derive `JsonSchema`, are added to `crates/fin-store/examples/export_schema.rs` in the task that creates them, and are regenerated with `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). Control-plane rows owned by TS are Effect Schemas in `@finisher/contracts`.
- **Dependency rules:** `fin` links neither `fin-registry` nor `tough` (it calls RPCs, except `fin plugin validate`, which links `fin-plugin-api`); `fin-core` does not depend on `fin-plugin-api`; `fin-plugin-api` depends on no crate of ours; `fin-node` links `fin-registry`; `fin-plugin-cli` is not in the app bundle (a gate checks the packaging list); the `test-http` feature is never enabled in default members (extend `scripts/check-test-hooks.mjs`).
- **`rustls` providers:** every `rustls::ClientConfig` in our crates is built with `builder_with_provider(...)`; nothing calls `CryptoProvider::install_default`; a test asserts it (Task 1).
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. New logic goes into new files; `apps/finisherd/src/handlers.ts`, `main.ts` and `crates/fin/src/main.rs`, `registry.rs` take call-site lines only.
- Errors are `{ code, message, details, fix? }`. New codes go into `crates/fin/src/output.rs` with the exits of spec §13 and into `@finisher/contracts`' code list.
- **No key material in tracked files.** Test keys are generated per test; the few fixed vectors (Task 4a) use keys whose file names and ids say `test-only`.

**Testing and process**
- Tests never use the network. Task 1's HTTPS fetch is a gate run by the controller, not a CI test.
- Clocks are injected (`fin_registry::Clock`) for expiry, freshness and the clock check.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures included.
- **Tracker:** each task ticks its line in `TASKS.md` under the heading "Sub-project 8A: packages, registries and lifecycle", in the same commit.
- **Batches.** Each batch must be green before the next starts:
  - A, gate, types and packages: Tasks 1–3 (parallel);
  - B, TUF and registries: Tasks 4a, 4b, 5, 6, 7 (4a first; then 4b, 5, 6, 7 in parallel); Task 8 and Task 11 may run beside batch B;
  - C, server and consumers (needs 5A): Tasks 9a, 9b, 10, 12a, 12b, 12c (9a first; 12a–12c after 9b);
  - D, e2e, docs and validation: Tasks 13–15.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Types and packages
1. **Manifest file name** `finisher-plugin.json` at the package root, ≤ 256 KiB (`MANIFEST_TOO_LARGE`), UTF-8 JSON (the spec's `jsonc` is illustration).
2. **Ids:** `@<publisher>/<name>`, publisher `[a-z0-9][a-z0-9-]{1,38}`, name `[a-z0-9][a-z0-9-]{1,63}`, no `--`, no leading or trailing `-`. Reserved publishers (spec §4.1) are refused outside bundled sources; `@dev` only in developer mode.
3. **Versions:** semver 2.0 without build metadata.
4. **Consumable kinds in 8A: `fontCatalogs` and `captionStyles` only.** `fin-plugin-api::contribution::consumable(host_api)` lists kinds per host-API minor; any other key in `contributes` is `PLUGIN_CONTRIBUTION_UNSUPPORTED {kind, hostApi}`, so no unconsumed kind is half-validated. Code plugins with an empty `contributes` (8A cannot run them anyway) install, so the lifecycle can be tested with code packages.
5. **Risks** (spec §4.3) are computed over the whole permission set, so combinations count (`hooks:observe` + `net` is high); `gpu` is medium. **Third-party code** (a `kind: code` package from a registry, not bundled, not `@dev`) is high risk whatever its permissions; that rule needs the source, so it is applied in the plan (Task 8), not in `fin-plugin-api::risks`.
6. **Licence classes:** `permissive` (every required branch allowed by `deny.toml`), `weakCopyleft` (MPL-2.0, EPL-2.0 as an allowed branch), `copyleft` (GPL, LGPL, AGPL, SSPL, EUPL, CC-BY-SA in any required branch), `nonCommercial` (any NC licence: **refused**), `unknown` (not SPDX: refused for bundled plugins; a warning elsewhere). `OR` takes its most permissive branch, `AND` its least.
7. **Packages are zip files** (`zip` 9). `fin-plugin pack` writes entries sorted by name, stored or deflate, with the DOS epoch timestamp, modes 0644/0755 and no extra fields, so the same input packs to the same bytes on every OS (checked in Task 15).

### TUF and registries
8. **Task 1 decides the crypto path.** `tough` with `aws-lc-rs` stays if (a) `fin-node` builds with `fin-fonts` and `tough` together on macOS arm64, Windows x64 (MSVC) and Linux x64 with only a C toolchain (CMake and NASM absent, or `prebuilt-nasm` used and recorded), (b) a real HTTPS fetch through `fin-fonts` and one through a `rustls` client using the `aws_lc_rs` provider both succeed in the same `.node` with no process default installed, (c) the release `.node` grows by ≤ 6 MiB, and (d) `tough` passes the three behaviour checks of Decision 10. Otherwise: fork `tough` (path dependency, `vendor/tough/`, upstream licence and `SOURCE.json` provenance as for thorvg) with its key parsing, verification and signing on `ring`, keeping every other module unchanged.
9. **Signing keys:** ed25519 for every role. Thresholds: root ≥ 1 for team registries (tests use 1 and 2); first-party registries use 2 of 3 owner-held offline keys (spec owner decision 7), which 8A does not create.
10. **Behaviour checks of `tough`** (Task 1, then pinned by tests in 4b): `ExpirationEnforcement::Unsafe` skips expiry checks only (signatures, thresholds, versions and hashes still enforced); each newly trusted root is saved atomically before the next is fetched; fast-forward recovery deletes the trusted timestamp and snapshot after a rotation of their keys. Where the crate falls short, `fin-registry::tuf` adds the behaviour around it and the test proves it.
11. **Consistent snapshots on**, target names hash-prefixed, root files kept as `<n>.root.json`.
12. **Freshness:** local registries are `relaxed`: loaded with `ExpirationEnforcement::Unsafe`, then `TUF_EXPIRED` when expired by more than `maxStaleDays` (from `custom.finisher.maxStaleDays` in the signed root, default **7**, at most 90), `REGISTRY_STALE` (warning) within it. `strict` exists for future HTTPS registries and tests.
13. **Clock check:** `trust/<rootSha256>/state.json` records `lastRefreshAt` (wall clock); a refresh more than 24 h before it is `CLOCK_SUSPECT` (exit 5) and changes nothing.
14. **Trust per root, kept on removal:** trusted metadata lives under `<server-data>/plugins/trust/<rootSha256>/`; `registry.remove` deletes the registry's config only; re-adding with the same root resumes from the trusted versions; `finisherd registry forget --root <sha>` deletes trust explicitly.
15. **Size caps:** root and targets ≤ 4 MiB, timestamp ≤ 16 KiB, snapshot ≤ 1 MiB, a delegated role ≤ 4 MiB, ≤ 32 root versions per refresh, ≤ 500 delegations.
16. **Root at add time:** `--root <file>` or `--root-sha256 <hex>`; otherwise the console prints the root's SHA-256 and key ids and the owner types the hash back. No RPC adds a registry without one of the three.
17. **Git registries** are folders: the docs and the console help say "point `--dir` at your clone and `git pull` it"; nothing in 8A runs git.

### Lifecycle
18. **Plans** live in `control.db` `plugin_plans {id, digest, pluginId, version, source, registryId?, action, by (principal kind and id), human, createdAt, expiresAt}` and in `staging/<planId>/`; expiry 10 minutes, swept every minute and at start. `digest` = BLAKE3 of the canonical plan JSON (permissions, risks, licence, staging tree hash).
19. **Approval:** `plugin.install {planId, digest, accept: {copyleft?, thirdPartyCode?}}` succeeds only for a human principal with `plugin.install` (owner) **and** a plan made by a human principal; an agent-made plan is `HUMAN_ONLY` even when a person submits its id. A plan carrying the `thirdPartyCode` warning also needs `accept.thirdPartyCode: true`, else `THIRD_PARTY_CODE_NOT_ACCEPTED` (exit 7); this holds for updates of third-party code too, even when nothing widens.
20. **No agent installs:** `fin plugin install` is always `HUMAN_ONLY` for `agent` principals and for `local` too (installs are owner-only; `local` is not the owner, cp §4.4).
21. **Code plugins** install with `enabled = false`; `plugin.enable` on a code plugin is `PLUGIN_RUNTIME_UNAVAILABLE` until 8B adds the runtimes. No sandbox gate exists: 8B enables registry-installed code plugins before 7C, unconfined (spec §3.1, owner decision 1), and nothing in 8A returns `SANDBOX_UNAVAILABLE`. Every third-party code plan carries `{code: thirdPartyCode, details: {confined: false}}` in 8A (no `plugin` profile exists yet); `confined` reads 7C's profile status once it lands.
22. **Content packs** enable at install. Font catalogs merge into the server's catalog by entry id (new ids only; an id taken by the built-in catalog or another pack is refused at plan time, `PLUGIN_MANIFEST_INVALID {reason: "catalogIdTaken"}`). Caption styles are copied into the project by `fin style add --from @pub/pack:<name>` through the existing `style.add` op.
23. **Automatic updates** (spec §5.3): only for content packs with `autoUpdate` on (default off), only when nothing widens, never for agent-facing kinds (none are consumable in 8A; the rule is implemented and tested with a test-only kind flag), and **never from a refresh made by a non-human principal**: such a refresh updates trusted metadata and applies `disable` advisories only.
24. **Bundled plugins (no TUF):** sources in `resources/plugins/bundled/<id>/` (unpacked); `crates/fin-node/build.rs` hashes each tree (`fin-registry::package::tree_hash`) and generates `BUNDLED_PLUGINS: &[(id, version, tree_hash)]`; at start `finisherd` re-hashes each tree, installs or refreshes matching ones without approval (a bundled update that widens permissions is applied, because the app update was the approval, and is audited with its diff) and marks mismatching ones `revoked` with `PLUGIN_BUNDLED_MISMATCH` in status. 8A ships one bundled pack, `@finisher/captions-basic` (three caption styles from the built-in presets).
25. **What release builds ship before sub-project 11's signing:** there are no signed releases before sub-project 11; any build made before it (CI artifacts, local release builds) is an unsigned development build. Its bundled plugins are verified **only** by the compiled-in hashes, which detects corruption and partial updates but not a local user (or malware with the user's rights) who rewrites both the resources and the binary. `finisherd status` reports `bundledIntegrity: hashesOnly` on unsigned builds and `signed` once sub-project 11 adds the code-signature check; a TASKS follow-up for sub-project 11 adds that check.
26. **Remove** refuses bundled plugins (`PLUGIN_BUNDLED`). Removing a pack whose fonts a project pinned keeps the font files (fonts are content-addressed and pinned by hash, 4C); its catalog entries disappear for new pins.

### Events, audit, scopes
27. `plugin.lifecycle` and `registry.refresh` are emitted by `fin-node` (`source: finNode`) from the registry worker; `PluginRef = {bundled: Name(BUNDLED_PLUGINS)} | {hashed: Hashed}`; `registry.refresh` carries `byHuman`.
28. Audit kinds `plugin.install|update|remove|enable|disable|revoke`, `plugin.grant`, `registry.add|remove|trust|forget|verifyFail` are added to sub-project 5's `AuditKind`, with `details` limited to `{pluginId, version, registryId, permissionsAdded, risk, code, thirdPartyCode?}` (`thirdPartyCode: {accepted, confined}` on installs and updates of third-party code).
29. Scope rows (spec §3.7): `plugin.read` (`plugin.list|info|search|plan|content.get`, `registry.list|status|refresh`), `plugin.install` (owner: `plugin.install|remove`), `plugin.admin` (owner: `plugin.enable|disable`, `registry.add|remove|set|forget`), `plugin.suggest` (with `project.read`). `local` and `agent` defaults get `plugin.read` and `plugin.suggest` only.

## Owner decisions (2026-11-07)

Spec §20 records them. **Decision 1** (third-party code plugins allowed before 7C, the owner's choice against revision 2's recommendation): 8A installs code plugins disabled only because nothing can run them before 8B; there is no sandbox gate, and third-party code is high risk with an explicit acceptance (Decisions 5, 19, 21). **Decision 7** (2 of 3 owner-held offline root keys, ceremony before the first first-party registry) does not block 8A: 8A builds no first-party registry, and tests use generated keys. Decisions 2–6 do not affect 8A.

## Review Focus

1. **Nothing unverified reaches the store:** every byte in `store/` is from a target whose hash and length match signed metadata (or, for bundled plugins, a compiled-in tree hash), unpacked by the strict reader; the staging tree is re-hashed at approval.
   → Task 3 `hostile_archive_corpus`; Task 4b `tampered_target_refused`, `length_cap_enforced`; Task 8 `install_rechecks_staging_hash`; Task 10 `tampered_bundled_tree_revoked`.
2. **TUF semantics:** rollback of every role, freshness per policy, mix-and-match, root rotation with atomic saves, fast-forward recovery, delegation confinement, trusted metadata written only after a full chain verifies, trust kept on removal.
   → Task 4b attack matrix; Task 7 `remove_and_readd_keeps_trust`; Task 13 `tuf-conformance` run.
3. **Approval is human and binds the exact plan:** digests, expiry, agent-made plans refused, widening updates need a new approval, agent-triggered refreshes install nothing, third-party code needs its explicit acceptance on every install and update.
   → Task 9b `approval_matrix`, `agent_plan_refused_for_human_submit`, `widening_update_needs_approval`, `agent_refresh_installs_nothing`, `third_party_code_needs_acceptance`.
4. **Nothing runs at install:** no process is spawned by plan or install, and executables keep only the modes the manifest names.
   → Task 8 `install_spawns_nothing`.
5. **Trust is explicit:** no registry without a pinned or typed-back root.
   → Task 7 `registry_add_needs_root`.
6. **One TLS stack does not break the other:** both `rustls` providers coexist with named providers everywhere.
   → Task 1 gate; `no_process_default_provider` test.
7. **Privacy:** journal entries hold no non-bundled plugin id in clear, no registry path, no publisher name.
   → Task 11 `lifecycle_events_hold_no_free_text`.
8. **Cross-platform paths:** reserved Windows names, case-folded duplicates and NFD names; read-only store files on Windows.
   → Task 3, Task 15.

---

### Task 1: Gate `tough` inside `fin-node` with `fin-fonts` on three OSes — tier: opus (controller-run where hosts are needed)

**Files:**
- Create: `archive/spikes/sp8a-tuf/` (git-ignored): a branch-local patch that adds `tough` (no `http`) to `fin-node` beside `fin-fonts`, a throwaway napi function that does one HTTPS GET through `fin-fonts`' client and one through a `rustls` client with the `aws_lc_rs` provider, and a small program that exercises `tough` on a repository built with its editor; build logs and sizes.
- Create (kept): `crates/fin-fonts/tests/provider.rs` (`no_process_default_provider`: builds every `fin-fonts` client config and asserts none relies on a process default).
- Modify: `crates/fin-fonts/src/http.rs` (if any `ClientConfig` is built without `builder_with_provider`), `TASKS.md`.

- [ ] **Step 1: Build** on macOS arm64, Windows 11 x64 (MSVC) and Linux x64 from a clean target dir: record whether `aws-lc-sys` needs CMake, NASM (Windows x64) or Go, whether `prebuilt-nasm` avoids NASM, build time, the size added to the release `fin-node` `.node`, and `cargo deny check licenses` with the current `deny.toml`.
- [ ] **Step 2: Fetch over HTTPS** on all three OSes from the same `.node` through both clients with no process default installed (the URL comes from the environment of the run; none is tracked). Record that neither panics.
- [ ] **Step 3: Check `tough`'s behaviour:** (a) `ExpirationEnforcement::Unsafe` loads an expired repository but still refuses a bad signature, a lower version and a wrong target hash; (b) during a root chain update, each new root is on disk (temp then rename) before the next is fetched (inspect the datastore code and kill the process between roots in a test); (c) after a timestamp-key rotation, a lower timestamp version signed by the new key is accepted (fast-forward recovery). Record each result.
- [ ] **Step 4: Decide** per Decision 8 and write the result (path, versions, features, sizes, behaviour gaps that `fin-registry` must cover) into `TASKS.md` and the commit body. **Commit** `docs: record the TUF crate gate for 8A` (plus `fix(fonts): name the rustls provider on every client config` if Step 2 needed a change).

### Task 2: `fin-plugin-api`: manifest, permissions, risks, licences, consumable kinds — tier: opus

**Files:**
- Create: `crates/fin-plugin-api/{Cargo.toml, src/lib.rs, src/manifest/{mod.rs, id.rs, runtime.rs, contributes.rs}, src/permission.rs, src/risk.rs, src/licence.rs, src/contribution.rs, src/validate/{mod.rs, files.rs, messages.rs, content.rs}, src/schema.rs}`.
- Test: `crates/fin-plugin-api/tests/{manifest.rs, permission.rs, licence.rs, validate.rs}`, small JSON fixtures under `tests/fixtures/`.
- Modify: `Cargo.toml` (member, default member), `crates/fin-store/examples/export_schema.rs`, `packages/core-types` (regenerate), `scripts/check-wasm-deps.mjs`, `TASKS.md`.

**Interfaces:**
```rust
pub struct Manifest { pub manifest_version: u16, pub id: PluginId, pub version: semver::Version, pub kind: PluginKind,
    pub host_api: semver::VersionReq, pub license: String, pub publisher: Publisher, pub messages: Option<String>,
    pub locales: Vec<String>, pub platforms: Vec<Platform>, pub runtime: Option<Runtime>, pub permissions: Vec<Permission>,
    pub contributes: Contributes, pub models: Vec<ModelRef> }
pub enum Runtime { Native { exec: BTreeMap<Platform, String> }, Node { main: String, node: semver::VersionReq } }
pub enum Permission { ProjectRead, ProjectWrite, MediaRead, MediaFile, JobsSubmit, Gpu,
    Net { origins: Vec<Origin> }, Secrets { names: Vec<String>, mode: SecretMode },
    Fs { scope: FsScope /* PluginData */, access: FsAccess }, HooksObserve { hooks: Vec<String> }, HooksModify { hooks: Vec<String> } }
pub enum Risk { Low, Medium, High }
pub fn risks(perms: &[Permission]) -> Vec<(Permission, Risk)>;        // combination rules (spec §4.3)
pub fn overall(perms: &[Permission], agent_facing: bool) -> Risk;
pub struct PermissionDiff { pub added: Vec<Permission>, pub widened: Vec<(Permission, Permission)>, pub removed: Vec<Permission>, pub risk_up: bool }
pub fn diff(old: &[Permission], new: &[Permission]) -> PermissionDiff;
pub enum LicenceClass { Permissive, WeakCopyleft, Copyleft, NonCommercial, Unknown }
pub fn classify(spdx: &str) -> LicenceClass;
pub fn consumable(host_api: &semver::Version) -> &'static [ContributionKind];   // 1.0: FontCatalogs, CaptionStyles
pub trait PackageFiles { fn exists(&self, path: &str) -> bool; fn read(&self, path: &str) -> Option<Vec<u8>>; }
pub struct ValidateCtx<'a> { pub host_api: semver::Version, pub platform: Option<Platform>, pub source: SourceClass /* Bundled|Registry|Dev */,
    pub node: Option<semver::Version>, pub content_checks: &'a dyn ContentChecks }   // fin-fonts and fin-core checks injected by fin-node
pub fn validate(manifest_json: &[u8], files: &dyn PackageFiles, ctx: &ValidateCtx) -> Result<Manifest, Vec<Invalid>>;
```

- [ ] **Step 1: Write failing tests:** `ids_follow_the_grammar`, `reserved_publishers_refused_outside_bundled`, `build_metadata_refused`, `content_pack_with_permissions_is_invalid`, `code_plugin_needs_runtime_and_platforms`, `node_runtime_needs_min_version`, `exec_per_platform_matches_platforms`, `every_referenced_file_exists`, `message_keys_in_en_catalogue`, `unconsumable_kind_refused` (every spec §4.2 kind other than the two), `font_catalog_contribution_uses_injected_check`, `caption_style_contribution_uses_injected_check`, `risk_combinations` (`hooks:observe` alone medium, with `net` high; `gpu` medium), `permission_diff_widening_cases` (each rule; reordering is no change; risk increase by combination), `licence_classes`, `unknown_fields_refused`, `manifest_schema_round_trips`.
- [ ] **Step 2: Implement.** `fin-plugin-api` has no crate dependency of ours.
- [ ] **Step 3:** Export the schema, regenerate TS, run the gates → PASS. **Commit** `feat(plugin-api): plugin manifest, permissions, risks and licence classes with Rust validation`.

### Task 3: The strict package reader and the reproducible packer — tier: opus

**Files:**
- Create: `crates/fin-registry/{Cargo.toml, src/lib.rs, src/package/{mod.rs, read.rs, names.rs, limits.rs, pack.rs, tree.rs}}`.
- Test: `crates/fin-registry/tests/package.rs`, `crates/fin-registry/tests/support/hostile.rs` (hostile archives built in code).
- Modify: `Cargo.toml`, `TASKS.md`.

**Interfaces:**
```rust
pub struct Limits { pub entries: u32, pub unpacked: u64, pub ratio: u32, pub path_bytes: usize, pub component_bytes: usize }
pub struct Unpacked { pub root: PathBuf, pub tree_hash: String, pub manifest: Vec<u8>, pub files: BTreeMap<String, FileMeta> }
pub fn unpack(archive: &Path, dest: &Path, executables: impl Fn(&Manifest) -> Vec<String>, limits: &Limits) -> Result<Unpacked, PackageError>;
pub fn tree_hash(root: &Path) -> Result<String, PackageError>;   // blake3 over sorted (path, mode class, sha256); also used by fin-node's build.rs
pub fn pack(src: &Path, executables: &[String], out: &Path) -> Result<PackInfo, PackageError>;
```

- [ ] **Step 1: Write failing tests** (`hostile_archive_corpus`, one case each): `..`, absolute and drive-letter paths, backslashes, NUL, symlink and hard-link entries, device entries, `CON`/`aux.txt`/`COM1`, trailing dot and space, case-folded and NFC/NFD duplicates, > 10,000 entries, > 512 MiB declared, ratio over 200 : 1, declared size smaller than written, no root manifest; plus `unpack_sets_modes_only_for_executables`, `pack_is_reproducible`, `tree_hash_changes_on_any_byte_or_mode`.
- [ ] **Step 2: Implement.** Entries are streamed to temp files inside `dest`, then renamed; nothing is written through a path that canonicalises outside `dest`.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(registry): strict plugin package reader and reproducible packer`.

### Task 4a: The TUF editor, trust store and refresh — tier: opus (after Task 1)

**Files:**
- Create: `crates/fin-registry/src/tuf/{mod.rs, trusted.rs, store.rs, custom.rs}`, `crates/fin-registry/src/editor/{mod.rs, keys.rs, publish.rs, rotate.rs}` (**non-test**: the RepoBuilder that `fin-plugin` and the tests share), `crates/fin-registry/src/transport/{mod.rs, fs.rs}`, `tests/fixtures/tuf/vectors/` (two tiny `test-only` repositories).
- Test: `crates/fin-registry/tests/{tuf_happy.rs, editor.rs}`.
- Modify: `crates/fin-registry/Cargo.toml` (the crypto path chosen in Task 1), `TASKS.md`.

**Interfaces:**
```rust
pub trait Transport: Send + Sync { fn fetch(&self, path: &str, max: u64) -> Result<Vec<u8>, TransportError>; }
pub struct TrustStore { /* <server-data>/plugins/trust/<rootSha256>/ */ }
pub struct TrustedRepo { /* TrustStore + policy */ }
impl TrustedRepo {
    pub fn init(store: TrustStore, root: &[u8], expect_sha256: &str) -> Result<Self, TufError>;   // REGISTRY_UNTRUSTED on mismatch
    pub fn open(store: TrustStore) -> Result<Self, TufError>;
    pub fn refresh(&mut self, t: &dyn Transport, policy: &Policy, clock: &dyn Clock) -> Result<RefreshReport, TufError>;
    pub fn targets(&self) -> impl Iterator<Item = TargetInfo>;
    pub fn fetch_target(&self, t: &dyn Transport, name: &str, out: &Path) -> Result<(), TufError>;
}
pub mod editor {
    pub struct RepoBuilder { /* dir, keys, thresholds, expiries, maxStaleDays */ }
    impl RepoBuilder {
        pub fn init(dir: &Path, root_keys: &[SigningKey], threshold: u32, max_stale_days: Option<u16>) -> Result<Self, EditorError>;
        pub fn delegate(&mut self, publisher: &str, key: &PublicKey) -> Result<(), EditorError>;
        pub fn publish(&mut self, pkg: &Path, publisher_key: &SigningKey) -> Result<TargetInfo, EditorError>;   // refuses an existing version
        pub fn yank(&mut self, id: &str, version: &semver::Version) -> Result<(), EditorError>;
        pub fn advise(&mut self, a: Advisory, key: &SigningKey) -> Result<(), EditorError>;
        pub fn rotate(&mut self, role: Role, new: &[SigningKey], threshold: u32) -> Result<(), EditorError>;
        pub fn resign_timestamp(&mut self, expires_in_days: u32) -> Result<(), EditorError>;
    }
}
```

- [ ] **Step 1: Write failing tests:** `refresh_from_root_to_targets`, `delegated_targets_resolve`, `custom_metadata_round_trips`, `fetch_target_checks_length_and_hashes`, `trusted_files_written_only_after_full_chain` (a failure at snapshot leaves the trusted set byte-identical), `editor_publish_refuses_existing_version`, `editor_round_trip_with_rotation`, `fixture_vectors_verify_identically`.
- [ ] **Step 2: Implement** over the chosen path: a current-thread `tokio` runtime owned by `TrustedRepo`; `Transport` adapted to the crate's transport trait; everything persisted temp-then-rename.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(registry): TUF trust store, refresh workflow and repository editor`.

### Task 4b: The TUF attack matrix, rotation, freshness and clocks — tier: opus (after 4a)

**Files:**
- Create: `crates/fin-registry/src/tuf/{freshness.rs, clock.rs, recovery.rs}` (only what Task 1 found missing in the crate).
- Test: `crates/fin-registry/tests/{tuf_attacks.rs, tuf_rotation.rs, tuf_freshness.rs}`.
- Modify: `TASKS.md`.

- [ ] **Step 1: Write failing tests** (one each): tampered target bytes (`tampered_target_refused`); target longer than its signed length (`length_cap_enforced`); timestamp, snapshot, targets and delegated-role version rollback; mix-and-match (a snapshot naming an older targets version); timestamp signed by a revoked key; root rotation without the old threshold; `@a`'s role listing `@b/x` (`TufError::Delegation`); oversized timestamp; 33 root versions in one refresh; root chain 1→2→3 with threshold 1→2→1, killed between roots and resumed (`each_root_saved_atomically`); timestamp-key rotation with fast-forward recovery; publisher key rotation by re-delegation; a revoked publisher's new versions refused; freshness: expired within 7 days → `REGISTRY_STALE`, beyond → `TUF_EXPIRED`, a root with `maxStaleDays: 30` and with `200` (clamped to 90); `Unsafe` still refuses bad signatures (`unsafe_expiry_only`); `CLOCK_SUSPECT` per Decision 13.
- [ ] **Step 2: Implement** the gaps.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `test(registry): TUF attack, rotation and freshness matrix`.

### Task 5: The test-only HTTP transport — tier: sonnet (after 4a)

**Files:**
- Create: `crates/fin-registry/src/transport/http.rs` (behind `test-http`), `crates/fin-registry/examples/tuf_conformance_client.rs` (the CLI that `tuf-conformance` drives: `init`, `refresh`, `download`).
- Test: `crates/fin-registry/tests/http_transport.rs` (a loopback server in the test).
- Modify: `crates/fin-registry/Cargo.toml` (feature `test-http`, off by default), `scripts/check-test-hooks.mjs` (no default member enables it), `TASKS.md`.

- [ ] **Step 1: Write failing tests:** `http_transport_matches_fs_transport` (the same repository served both ways refreshes to the same trusted set), `http_respects_length_caps`, `feature_off_in_default_members` (the gate).
- [ ] **Step 2: Implement** with a client whose `rustls` config names its provider (Decision 8 rule).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `test(registry): HTTP transport for the TUF conformance suite (test builds only)`.

### Task 6: Publisher tooling `fin-plugin` — tier: sonnet (after 4a)

**Files:**
- Create: `crates/fin-plugin-cli/{Cargo.toml, src/main.rs, src/cmd/{new.rs, validate.rs, pack.rs, registry.rs}, src/keys.rs}`.
- Test: `crates/fin-plugin-cli/tests/cli.rs`.
- Modify: `Cargo.toml` (member, not in the app packaging list), `TASKS.md`.

**Interfaces:** `fin-plugin new content|code <dir>`; `validate <dir>`; `pack <dir> -o <file>`; `registry init <dir> --root-keys <k…> --threshold <n> [--max-stale-days <d>]`, `registry delegate <dir> --publisher <p> --key <pub>`, `registry publish <dir> <pkg> --key <k>`, `registry yank <dir> <id>@<v>`, `registry advise <dir> --plugin <id> --versions <range> --action warn|disable --summary <text> --key <k>`, `registry rotate-key <dir> --role <r> --new <k…> [--threshold <n>]`, `registry resign-timestamp <dir> [--expires <days>]`. JSON on stdout, errors as `{code, message, details, fix?}`, `fin`'s exit codes. A `--help` note: "a git registry is a clone of a repository holding this folder; commit and push it, and users `git pull`".

- [ ] **Step 1: Write failing tests:** each subcommand's effect verified by `TrustedRepo`; `publish_refuses_existing_version`, `publish_fills_custom_metadata_from_manifest`, `keys_are_never_printed`.
- [ ] **Step 2: Implement** over `fin-registry::editor`.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(plugin-cli): fin-plugin publisher tooling for local-folder registries`.

### Task 7: Local registries: config, search, yanks, advisories, staleness — tier: opus (after 4a)

**Files:**
- Create: `crates/fin-registry/src/{registry.rs, advisory.rs, search.rs}`.
- Test: `crates/fin-registry/tests/{registry.rs, advisories.rs}`.
- Modify: `TASKS.md`.

**Interfaces:**
```rust
pub struct RegistryConfig { pub id: String, pub path: PathBuf, pub root_sha256: String }
pub struct Registry { /* config + TrustedRepo + fs Transport */ }
impl Registry {
    pub fn add(base: &Path, cfg: RegistryConfig, root: RootSource /* File | Sha256 | Confirmed(sha) */) -> Result<Self, RegistryError>;
    pub fn remove(base: &Path, id: &str) -> Result<(), RegistryError>;                // keeps trust/<root>/
    pub fn forget(base: &Path, root_sha256: &str) -> Result<(), RegistryError>;
    pub fn refresh(&mut self, clock: &dyn Clock) -> Result<RefreshReport, RegistryError>;
    pub fn search(&self, q: &SearchQuery) -> Vec<Listing>;
    pub fn info(&self, id: &PluginId) -> Option<PluginInfo>;
    pub fn stale_days(&self, clock: &dyn Clock) -> Option<u16>;
    pub fn advisories(&self) -> Vec<Advisory>;
}
```

- [ ] **Step 1: Write failing tests:** `registry_add_needs_root`, `search_reads_custom_metadata_only`, `yanked_hidden_but_exact_installable`, `advisory_from_other_publisher_ignored`, `registry_wide_advisory_applies`, `advisory_version_ranges`, `remove_and_readd_keeps_trust` (a repository rolled back between remove and re-add is refused), `forget_deletes_trust`, `stale_days_reported`, `git_pulled_folder_rollback_refused` (a folder moved to an older state, as `git reset` would).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(registry): local-folder registries with search, yanks, advisories and kept trust`.

### Task 8: The store, staging and plans in `fin-registry` and `fin-node` — tier: opus (can start now; `finisherd` parts are Task 9b)

**Files:**
- Create: `crates/fin-registry/src/{store.rs, plan.rs}`, `crates/fin-node/src/registry.rs`.
- Test: `crates/fin-registry/tests/{plan.rs, store.rs}`, `packages/fin-node/test/registry.test.ts`.
- Modify: `crates/fin-node/src/lib.rs`, `crates/fin-node/Cargo.toml`, `packages/fin-node` (typings), `crates/fin-store/examples/export_schema.rs` (`InstallPlan`, `RegistryStatus`, `InstalledPlugin`), `TASKS.md`.

**Interfaces:**
```rust
pub struct InstallPlan { pub plan_id: String, pub digest: String, pub plugin: PlanPlugin, pub source: PlanSource, pub action: PlanAction,
    pub from: Option<semver::Version>, pub permissions: Vec<PlannedPermission>, pub diff: PermissionDiff, pub risk: Risk,
    pub licence: PlanLicence, pub bytes: u64, pub models: Vec<PlanModel>, pub warnings: Vec<PlanWarning>,
    pub registry_stale_days: Option<u16>, pub expires_at: u64 }
pub fn make_plan(src: PlanInput /* Registry | Bundled(dir) */, store: &Store, req: &PlanRequest, ctx: &ValidateCtx, clock: &dyn Clock) -> Result<InstallPlan, PlanError>;
impl Store {
    pub fn commit(&self, plan: &InstallPlan) -> Result<InstalledRef, StoreError>;   // re-hash staging (PLAN_MISMATCH), rename, read-only
    pub fn remove(&self, id: &PluginId, version: &semver::Version, purge_data: bool) -> Result<(), StoreError>;
    pub fn sweep_staging(&self, now: u64) -> u32;
}
```
```ts
export class Registry {
  static open(serverData: string): Registry;
  add(cfg: RegistryConfig, root: RootSource): Promise<RegistryStatus>;
  remove(id: string): Promise<void>; forget(rootSha256: string): Promise<void>;
  refresh(id: string, byHuman: boolean): Promise<RefreshReport>;
  search(q: SearchQuery): Promise<Listing[]>; info(id: string): Promise<PluginInfo | null>;
  plan(req: PlanRequest): Promise<InstallPlan>;
  commit(planId: string, digest: string): Promise<InstalledRef>;
  remove_plugin(id: string, version: string, purgeData: boolean): Promise<void>;
  sweep(): number;
}
export function validateManifest(dir: string): ValidationReport;
```

- [ ] **Step 1: Write failing tests:** `plan_verifies_unpacks_and_validates`, `plan_manifest_must_match_custom_metadata`, `plan_warns_on_lookalike_id`, `plan_flags_copyleft_and_refuses_nonCommercial`, `plan_refuses_incompatible_host_api_platform_and_node`, `plan_refuses_unconsumable_kind`, `plan_reports_staleness`, `third_party_code_plan_is_high_risk` (a registry code package with only `project:read` is high risk and carries `thirdPartyCode {confined: false}`; a bundled code plugin and a content pack do not), `install_rechecks_staging_hash`, `install_spawns_nothing`, `store_files_are_read_only`, `remove_keeps_data_unless_purged`, `staging_swept_after_expiry`, `catalog_id_taken_refused`, napi `long_calls_do_not_block_event_loop`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(node): plugin store, staging and install plans`.

### Task 9a: `finisherd`: control store, scopes and read RPCs — tier: sonnet (needs 5A)

**Files:**
- Create: `apps/finisherd/src/plugins/{registries.ts, rpc.ts}`, `apps/finisherd/src/control/migrations/m00NN-plugins.ts`, `packages/contracts/src/plugins.ts`.
- Test: `apps/finisherd/test/plugins-scopes.test.ts`, `apps/finisherd/test/plugins-registries.test.ts`.
- Modify: `packages/contracts/src/{rpc.ts, scopes.ts, index.ts, errors.ts}`, `apps/finisherd/src/handlers.ts` (call sites), `apps/finisherd/src/control/audit.ts` (kinds), `crates/fin/src/output.rs` (codes), `TASKS.md`.

**RPCs:** `registry.list`, `registry.status {id}`, `registry.add {cfg, root}`, `registry.remove {id}`, `registry.forget {rootSha256}`, `registry.refresh {id?}`; `plugin.list` (with `registryStaleDays`), `plugin.search`, `plugin.info`. Scopes per Decision 29.

- [ ] **Step 1: Write failing tests:** `every_new_rpc_has_a_scope`, `registry_admin_is_owner_only`, `refresh_records_by_human`, `plugin_list_shows_staleness`, migration up from 5A's last migration.
- [ ] **Step 2: Implement** (tables `registries`, `plugins`, `plugin_grants`, `plugin_plans`, spec §5.1).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): plugin registries, scopes and read RPCs`.

### Task 9b: `finisherd`: plans, approvals, install, update, enable, remove, audit — tier: opus (after 9a)

**Files:**
- Create: `apps/finisherd/src/plugins/{lifecycle.ts, plans.ts}`.
- Test: `apps/finisherd/test/plugins-lifecycle.test.ts`.
- Modify: `apps/finisherd/src/plugins/rpc.ts`, `TASKS.md`.

**RPCs:** `plugin.plan {id, version?, registry?, update?, allowDowngrade?}`, `plugin.install {planId, digest, accept}`, `plugin.remove {id, purgeData?}`, `plugin.enable {id}`, `plugin.disable {id}`, `plugin.suggest {id, reason, projectId}`.

**Requirements from the review of Task 8 (`sp8a-t8-11`):**
- **M2, the installed state at commit:** `plugin.install` runs `Registry.commit` and the `plugins`/`plugin_grants` writes in **one `control.db` transaction**, and inside it re-checks that the installed version and grants still equal the plan's base (`plan.from`, and the grants `diff` was taken against: `PlanRequest.installed` as recorded with the plan) and re-runs the catalog-id check (`taken_font_ids` over the built-in catalog and the packs installed now). Any difference is `PLAN_MISMATCH {reason: "installed"}` and nothing is installed; the person makes a new plan.
- **M3, downgrades:** `plugin.plan {version}` sets `PlanRequest.version` (`fin-registry` refuses a package of another version, `versionMismatch`, and a downgrade without `version`, `PLUGIN_DOWNGRADE_REFUSED`; Task 8 fix); `allowDowngrade` is accepted only from a human principal together with `version`, and the console asks for the version typed back.
- **M4, who made the plan:** `plugin_plans` stores `by` (principal kind and id) and `human`; `plugin.install` reads them inside the same transaction and refuses an agent-made (or `system`-made) plan with `HUMAN_ONLY` whoever submits it, before `Registry.commit` runs.

- [ ] **Step 1: Write failing tests:** `commit_rechecks_installed_state` (M2: the installed version, its grants or the taken catalog ids changed between plan and install → `PLAN_MISMATCH {reason: installed}`, nothing installed, `control.db` unchanged); `downgrade_needs_named_version_and_human` (M3); `plan_maker_recorded_and_enforced` (M4: `by`/`human` stored; an agent-made plan is `HUMAN_ONLY` at install, from the console too); `approval_matrix` (console, owner-device stub, `local`, `agent` × plan maker human/agent × risk low/high); `agent_plan_refused_for_human_submit`; `fin_install_is_human_only_for_local_and_agent`; `digest_must_match`; `plan_expires`; `widening_update_needs_approval`; `content_autoupdate_only_when_nothing_widens`; `agent_facing_kind_never_autoupdates` (test-only kind flag); `agent_refresh_installs_nothing` (an agent's refresh updates metadata and applies a `disable` advisory, installs no update); `code_plugin_installs_disabled`, `enable_code_plugin_is_runtime_unavailable` (never `SANDBOX_UNAVAILABLE`); `third_party_code_needs_acceptance` (`THIRD_PARTY_CODE_NOT_ACCEPTED` without `accept.thirdPartyCode`; the audit row records the acceptance); `third_party_code_update_needs_acceptance_again` (a non-widening update too); `advisory_disable_on_refresh`; `refresh_failure_keeps_installed_plugins_running`; `audit_details_allow_list`; `suggest_records_plugin_suggestion` (a `pluginSuggestion` review item if 5E has landed, else a `plugin_suggestions` row with the same fields and a TASKS follow-up).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): plugin lifecycle with plans, owner approval and audit`.

### Task 10: Bundled plugins without TUF — tier: sonnet (needs 9b)

**Files:**
- Create: `resources/plugins/bundled/captions-basic/` (manifest, three styles, `en` catalogue), `crates/fin-node/build.rs` (generates `BUNDLED_PLUGINS`), `crates/fin-node/src/bundled.rs`, `apps/finisherd/src/plugins/bundled.ts`.
- Test: `apps/finisherd/test/plugins-bundled.test.ts`, `crates/fin-node/tests/bundled.rs`.
- Modify: `apps/finisherd/src/main.ts` (call site after the control store opens), `apps/finisherd/src/status.ts` or 5A's status handler (`bundledIntegrity`), `TASKS.md` (follow-up for sub-project 11: verify the app's code signature over bundled resources and report `signed`).

- [ ] **Step 1: Write failing tests:** `bundled_installs_at_first_start`, `bundled_updates_with_app` (a newer compiled-in version replaces the old one without a plan; a widening is applied and audited with its diff), `tampered_bundled_tree_revoked` (server starts; the plugin is `revoked` with `PLUGIN_BUNDLED_MISMATCH` in status), `bundled_cannot_be_removed`, `status_reports_hashes_only_on_unsigned_builds` (Decision 25).
- [ ] **Step 2: Implement.** The build script reuses `fin_registry::package::tree_hash` (a build-dependency on `fin-registry` with default features off).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): bundled plugins verified by compiled-in hashes`.

### Task 11: Diagnostics kinds — tier: sonnet (can start now)

**Files:**
- Create: `crates/fin-core/src/events/payloads/plugin.rs`.
- Modify: `crates/fin-core/src/events/{kinds.rs, names.rs, payloads/mod.rs}`, `crates/fin-store/examples/export_schema.rs`, `packages/core-types` (regenerate), `TASKS.md`. (Emitters land with Task 8's `fin-node` registry code once both exist; if Task 8 lands first, this task adds them.)

**Interfaces:** `PluginLifecycle { action: Name(PLUGIN_ACTIONS), outcome, code: Option<Code>, plugin: PluginRef, source: SourceName, kind: PluginKindName, permissions_added: u8, risk: RiskName, bytes_log2: u8, micros: u64 }`; `RegistryRefresh { outcome, code: Option<Code>, roles_updated: u16, targets_log2: u8, stale_days: Option<u16>, by_human: bool, micros: u64 }`; `PluginRef = Bundled(Name(BUNDLED_PLUGINS)) | Hashed(Hashed)`.

- [ ] **Step 1: Write failing tests:** `kinds_registered_v1`, `plugin_ref_never_free_text`, `lifecycle_events_hold_no_free_text` (an install from a local registry: no plugin id, publisher, registry path in clear), `bundled_ref_is_a_name`, `events_off_means_nothing_built`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(core): plugin.lifecycle and registry.refresh diagnostics kinds`.

### Task 12a: Content consumers: font catalogs and caption styles — tier: sonnet (after 9b)

**Files:**
- Create: `apps/finisherd/src/plugins/content.ts`.
- Test: `apps/finisherd/test/plugins-content.test.ts`, `crates/fin-fonts/tests/merged.rs`.
- Modify: `crates/fin-fonts/src/catalog.rs` (`FontCatalog::merged(&self, extra: &[FontCatalog]) -> Result<FontCatalog, CoreError>`), `crates/fin-node/src/fonts/*` (`FontManager::set_extra_catalogs`), `packages/contracts/src/plugins.ts` (`plugin.content.get`), `TASKS.md`.

- [ ] **Step 1: Write failing tests:** `pack_fonts_join_the_catalog` (listed by `fonts.catalog`; disabling the pack removes them; a pinned font stays usable), `merge_refuses_taken_ids`, `content_get_returns_style`, `content_get_respects_enabled_state`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat: font catalogs and caption styles from content packs`.

### Task 12b: Console `plugin` and `registry` commands — tier: sonnet (after 9b)

**Files:**
- Create: `apps/finisherd/src/console/plugin.ts`.
- Test: `apps/finisherd/test/console-plugin.test.ts`.
- Modify: 5A's console dispatch (call site), `TASKS.md`.

**Commands** (owner, cp §4.6): `finisherd registry add <id> --dir <path> [--root <file> | --root-sha256 <hex>]`, `registry ls|status|refresh|rm|forget`; `finisherd plugin search|info|ls`, `plugin install <id>[@<v>] [--registry <r>] [--accept-copyleft]` (prints the plan with risks and asks for `yes`; for third-party code it prints the `thirdPartyCode` high-risk warning first, lists each permission on its own line and asks for the plugin's id typed back instead of `yes`, which sets `accept.thirdPartyCode`), `plugin update <id>`, `plugin rm <id> [--purge-data]`, `plugin enable|disable <id>`.

- [ ] **Step 1: Write failing tests:** `console_install_prints_plan_and_needs_yes` (scripted stdin), `console_third_party_code_needs_typed_id` (`yes` refused; the warning precedes the permissions), `console_add_without_root_requires_typed_hash`, `console_update_shows_widening`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): plugin and registry console commands`.

### Task 12c: `fin plugin …` and `style add --from` — tier: sonnet (after 9b and 12a)

**Files:**
- Create: `crates/fin/src/commands/plugin.rs`.
- Test: `crates/fin/tests/plugin_cmd.rs`.
- Modify: `crates/fin/src/commands/style.rs` (`--from @pub/pack:<name>` → `plugin.content.get`), `crates/fin/src/registry.rs` (new commands), `TASKS.md`.

**Commands:** `fin plugin search <q>`, `fin plugin info <id>`, `fin plugin ls`, `fin plugin validate <dir>` (offline, links `fin-plugin-api`), `fin plugin suggest <id> --reason <text>`, `fin plugin install <id>` (exit 7 `HUMAN_ONLY` naming the console command).

- [ ] **Step 1: Write failing tests:** `style_from_pack_copies_into_project`, `fin_plugin_install_is_human_only`, `fin_plugin_validate_offline`, `fin_plugin_search_json_shape`, `fin_plugin_ls_shows_staleness`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `pnpm e2e` → PASS. **Commit** `feat(fin): plugin commands and caption styles from packs`.

### Task 13: e2e `plugins` scenario and the TUF conformance run — tier: sonnet

**Files:**
- Create: `scripts/e2e/plugins.mjs`, `scripts/tuf-conformance/` (job definition, suite pinned by commit SHA; Python only in CI).
- Modify: `scripts/e2e.mjs`, `.github/workflows/*` (a Linux-only job; actions pinned to SHAs), `TASKS.md`.

- [ ] **Step 1:** e2e: start a server; the bundled pack is installed; `fin-plugin` builds a local registry (once as a plain folder and once as a local git clone the test commits to and resets, without network) with a content pack and a code plugin; the console adds it (root by hash); `fin plugin search` finds both; `fin plugin install` is exit 7; the console installs the pack (permission plan printed) and the code plugin (high-risk `thirdPartyCode` warning, accepted by typing its id; installed, disabled until 8B); `fin style add --from` works; publish 1.1 with an added `net` permission → the console update shows the widening and needs `yes`; an agent-credential refresh installs nothing; a `disable` advisory revokes 1.0; a reset of the git clone to an older commit is refused (`TUF_ROLLBACK`) and installed plugins keep working; removing and re-adding the registry keeps trust; journal grep for free text; audit rows present.
- [ ] **Step 2:** CI: `tuf-conformance` against `tuf_conformance_client` (feature `test-http`) on Linux; record skipped cases in the job summary.
- [ ] **Step 3: Run** → PASS. **Commit** `test: e2e plugin lifecycle scenario and TUF conformance job`.

### Task 14: Spec "as built", architecture pointer, notices, tracker — tier: sonnet

**Files:**
- Modify: `docs/specs/2026-11-07-sub-project-8-plugins-design.md` (§4–6 rewritten as built, with a "Decisions taken by the 8A plan" table), `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 pointer for 8A; §14.4 and §15 cross-references; §15 "git registries are local folders"), `THIRD_PARTY_NOTICES.md` (`tough`, `aws-lc-rs` or the `ring` fork, `spdx`, `semver`), `README.md` (plugins: registries and the console), `TASKS.md`.

- [ ] **Step 1:** Write the as-built text; list deviations from the spec.
- [ ] **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 8A as built`.

### Task 15: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

- [ ] **Step 1:** On macOS arm64, Windows 11 x64 and the Linux VM: the full test suite, `pnpm e2e` (plugins scenario), `fin-plugin pack` hash equality across the three OSes for the same tree, a local registry on the NAS share used by 4A Task 19 and on a synced folder (paths from the environment, never in the repo), read-only store attributes on Windows, Windows-reserved names refused on all three, a bundled tree edited after build → `revoked`.
- [ ] **Step 2:** Record results in the spec's validation subsection and `TASKS.md`. **Commit** `docs: record 8A cross-platform validation`.
