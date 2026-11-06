# Sub-project 4C: Edit breadth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (model and upcasters, ops with their inverses, footprints, shift and content maps, rebase, evaluation, captions, text measurement, `fin` composition), `sonnet` for plumbing (fixtures, probes, bindings, RPC, CLI, docs).

**Goal:** The model covers everything v1 edits, and evaluation tells the media engine exactly what to draw and play:
- Several timelines, bins and markers. Video, audio and caption tracks. Items of kind `clip`, `caption`, `graphic`, `image`, `adjustment` and `nested`, each with transform, crop, opacity, blend, effects, mask, audio params, keyframes and link groups. Transitions at cuts.
- A settings registry drives generic `param.set`, `keyframe.*` and `effect.*` edits.
- Structural edits with explicit modes (`ripple`, `overwrite`, `lift`, `roll`), each with:
  - an exact inverse;
  - a footprint;
  - a shift map, plus a content map for item-relative times.

  So 4B's rebase, merge and undo work on all of them.
- `fin-core::eval` produces deterministic frame and audio plans. Caption layouts come from the new `fin-text` crate with bundled fonts (CJK and emoji included), identical on every OS.
- New asset kinds are probed. `fin view`, `fin status`, and a `fin` command for every edit.
- Every pre-4C project (sub-project 2, 4A, 4B) opens with the same state in the new model.

Everything is driven and tested through `fin` → `finisherd`. There is no UI work.

**Architecture:**
- **`fin-core`** gains:
  - the v2 model (`model/`), schema version 2, and frozen upcasters for snapshots, records and stored diffs (`upgrade/`);
  - **per-op diffs:** every change and shift carries its op index, and start-edge changes carry their content offset. So a footprint is computed from the op's own diff even when the op touched items it does not name (overwrite, links, transition pruning). The rebase engine maps item-relative values through other edits' start-edge moves;
  - about 40 primitive ops, one module each, with inverse, footprint, positions and item offsets. `clip.*` stay as client aliases;
  - `params/` (the registry), `eval/` (frame and audio plans), `captions/` (blocks, SRT, text alignment, styles, safe zones), `layout` (the `TextMeasurer` contract), and `compose/` (pure multi-op builders that `fin` uses: linked audio, overwrite inside an item, split of a linked item, transitions on linked partners);
  - a measurer threaded through the edit path (`ApplyEnv`), the rebase engine and the history planners. **Replay never takes one.**
- **`fin-text`** (new): cosmic-text shaping and measurement over fonts injected as bytes, with a platform-independent fallback list. It is pure and builds for wasm32. It is also the one source of font family names, which `fin-probe` uses too.
- **`fin-store`** gains:
  - DB migration 0003 (a version bump only);
  - stored v1 diffs upcast on read;
  - alias normalisation at commit;
  - an injected measurer;
  - a project file at a newer schema opens read-only. This is a cherry-pickable fix that lets 4B degrade too.
- **`fin-probe`** probes images, SVG, dotLottie, fonts, `.cube` LUTs and audio-only files.
- **`fin-node`, `@finisher/contracts`, `finisherd`** get bundled-font loading, a per-project measurer (bundled fonts plus project font assets) and the `eval.frame` / `eval.audio` RPCs. Edits keep going through `tx.commit`.
- **`fin`** gets:
  - a command for every edit (`--dry-run`, the mode and resolved values echoed). `fin clip insert` of an A/V asset adds a linked audio item by default;
  - `fin view` (screenplay text with a token budget), `fin status`, `fin schema param` and `fin eval`.

**Tech Stack:**
- Rust 2024, rusqlite 0.37 bundled, `blake3`, `proptest` (dev). `serde_json` gains the `float_roundtrip` feature in `fin-core`; feature unification makes it workspace-wide.
- New crates, all permissive:
  - **`fin-text`:** `cosmic-text` 0.19 (MIT OR Apache-2.0) with `default-features = false, features = ["no_std"]`. That brings:
    - `fontdb` 0.23 (MIT), `harfrust` 0.5 (MIT), `skrifa` 0.40 / `read-fonts` (MIT OR Apache-2.0);
    - `unicode-bidi`, `unicode-script`, `unicode-segmentation` (MIT OR Apache-2.0), `unicode-linebreak` (Apache-2.0), `libm` (MIT OR Apache-2.0);
    - `self_cell` (Apache-2.0 OR GPL-2.0-only, **used under Apache-2.0**: `cargo deny` accepts the `OR`, and `THIRD_PARTY_NOTICES.md` records the choice).

    No `swash` in 4C, because rasterising is sub-project 6.
  - **`fin-probe`:** `imagesize` 0.15 (MIT), `zip` 9 (MIT; `default-features = false, features = ["deflate-flate2-zlib-rs"]`: inflate through pure-Rust `zlib-rs` (Zlib); no zopfli, bzip2, zstd, lzma or AES), `roxmltree` 0.21 (MIT OR Apache-2.0), `fin-text` (font names).
  - **`fin-core` dev only:** `blake3` (plan hashes).
  - **CI only, never shipped:** `wasmtime` (Apache-2.0 WITH LLVM-exception) for the wasm32 corpus runs.
- **Fonts** (SIL OFL-1.1, upstream files, unmodified, no subsets, so no Reserved Font Name issue):
  - **committed** (about 2.5 MB): Inter, Noto Sans, Noto Sans Arabic, Noto Sans Hebrew, Regular and Bold;
  - **fetched, pinned and hash-checked** (Decision 30, about 50 MB): Noto Sans CJK Regular and Bold (`NotoSansCJK-{Regular,Bold}.ttc`, 19.5 and 20.1 MB) and Noto Color Emoji (`NotoColorEmoji.ttf`, CBDT, 10.7 MB). The emoji font file is OFL-1.1; the noto-emoji repository's tools and images are Apache-2.0 and are not used.
- Node 26, Effect 4, vitest.

**Spec:** `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §5 (4C). The plan also depends on:
- §1–2;
- §3.1–3.2 (versioning, frozen migrations), §3.3, §3.4 (`finisher.project.json`);
- §4.3 (rebase, footprints, shift maps), §4.4–4.6, §4.8;
- §6 and §8.

Parent: `docs/specs/2026-11-01-finisher-architecture-design.md` §6.2–6.4, §7, §9, §12.3–12.7 and §27.10 (S11b `fin-text`).

**Branch:** `sp4c-edit-breadth` at `36fe7e0` (4B merged with the latest 4A, including 4B's Task 22/23 docs and the xhost harness). 4A Task 19 and 4B Task 23 still gate their merges. Task 28 here runs 4C's validation.

**Out of scope** (spec §1): pixels and sound (compositor, mixer, rasterising text, sanitising SVG), transcription, `.otio`, `fin apply` / `fin batch` changes / `fin tx`, review items, `fin-wasm` replay tests, and UI. **Also moved to sub-project 6** (Decision 41): the edits that make `nested` items and masks.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate. Task 4 adds a font licence gate.
- **`fin-core` is pure:** no I/O, clocks or randomness. It builds for `wasm32-unknown-unknown`, and so does `fin-text` (Task 4 adds it to `pnpm gates`).
- **Dependency rules:** `fin-core` never depends on `fin-text`. `fin` links neither `fin-text` nor `fin-probe`.
- **Primitive ops live only in `fin-core`, one module per op** under `crates/fin-core/src/ops/`. Each module holds that op's validate, apply, inverse, `footprint`, `positions_mut`, `offsets_mut` (item-relative values) and `refs_mut`. Shared helpers live in `ops/util/`. Multi-op builders live in `compose/` and are never ops.
- **TypeScript model types are generated, never hand-written.**
  - Every new wire type derives `JsonSchema` and is reachable from `fin_core::json_schema` or added in `crates/fin-store/examples/export_schema.rs`, in the task that creates it.
  - Then run `pnpm --filter @finisher/core-types generate`. The drift gate is `scripts/check-generated-types.mjs`.
- **File sizes:** non-test source files ≤ 600 lines, tests ≤ 1,500.
  - Near the limit at `36fe7e0`:
    - `fin-store`: `lease/clock.rs` 600, `tick.rs` 587, `fsio.rs` 582, `derived.rs` 519, `project_file.rs` 469 (grew in the merge);
    - `fin-store-xhost`: `node.rs` 560;
    - `fin`: `client.rs` 525, `commands/mod.rs` 487;
    - `fin-core`: `history/merge.rs` 478, `ops/mod.rs` 448.
  - **`fin-core/tests/props_history.rs` is at 1,483 lines**, so 4C property code goes into new files.
  - Task 1 makes room in `ops/` and `fin`. Later tasks add new modules rather than growing those files.
- Errors are `{ code, message, details, fix? }`. Every new code goes into `crates/fin/src/output.rs` with the exit code Decision 38 gives.
- No media files are committed. Test media is generated in code (`fin-probe` `fixtures`). **The one exception is the small bundled fonts** (`assets/fonts/`), which the spec requires, with their licences beside them.

**Formats and migrations**
- **Migrations are frozen once shipped.** `m0001_records.rs` and `m0002_history.rs` and their pins (`migration_0001_output_is_pinned`, `fresh_and_migrated_schemas_match`) never change. m0003 writes literal values only and never runs `ALTER TABLE`.
- **Upcasters are frozen the same way.** Each step works on `serde_json::Value` with literal field names, uses no current model types, and has a golden test (frozen old shape in, expected new shape out). Until 4C merges, a step may still be edited together with its golden.
- **Stored bytes are never re-encoded.** Old records, stored diffs and snapshot rows stay as written and are upgraded in memory on read. The record format (`RECORD_FORMAT` 2) and segment framing do not change.
- **Replay applies; it never re-judges** (spec §2). Every new rule is a policy check unless eval or the model's invariants need it (Decision 25 lists the structural ones). Replay never needs a measurer.

**Determinism** (spec §5.4, §5.6)
- **Eval** (`fin-core/src/eval/`, `params/`, `captions/`, `layout.rs`) uses only:
  - `f64` `+ − × ÷`, `sqrt`, comparisons, `abs`, `min`/`max`, `floor` (exact);
  - conversions from integers.

  It never uses `sin`, `cos`, `tan`, `pow*`, `exp`, `ln`, `log*`, `hypot`, `atan*`, `mul_add` or any `libm::` function. Rotation is output as an angle. Task 16 adds a gate.
- **Text:**
  - fonts come only from the bundle or from project assets, never from the system;
  - the fallback list is our own, and the locale is fixed;
  - all float functions go through `libm` (`no_std`);
  - output is in 1/64 px integers.
- Plan and layout hashes are BLAKE3 of compact JSON. They are compared across macOS, Windows 11, Linux and wasm32 (Tasks 4, 16, 28).

**Testing and process**
- **Property tests run inside `cargo test`.** All properties (4B's included) take under 30 s in a debug build on the Linux VM. Each `proptest!` block sets `ProptestConfig { cases, .. }` explicitly and honours `PROPTEST_CASES`.
- **Every edit gets golden tests:** apply, inverse (state equals the state before), shift map (the recorded `shifts`), and `inverse_shifts_mirror` (Decision 23). The goldens live in `crates/fin-core/tests/ops_<group>.rs`. **Every op with nested ids gets `ids_cover_every_id_in_json`** (Decision 24).
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch the TS side or `fin` also run `rtk pnpm e2e` once at the end. Tests that need the fetched fonts run after `rtk pnpm fonts:fetch`; CI sets `FIN_REQUIRE_FONTS=1`, so they fail rather than skip.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh` (Node 26).
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures included. Cross-host details stay in `archive/hosts.md` (git-ignored). Spikes and scratch work go in `archive/`.
- **Tracker:** each task ticks its line in `TASKS.md` under a new section "Sub-project 4C: edit breadth", in the same commit.
- **Batches** (spec §8 "4C is large"). Each batch must be green before the next starts:
  - A, foundations, model and structural edits: Tasks 1–9;
  - B, params, structure ops and eval: Tasks 10–16;
  - C, captions, measurement, assets and server: Tasks 17–21;
  - D, `fin`: Tasks 22–24;
  - E, properties, e2e, docs and validation: Tasks 25–28.

  Task 4 may run beside Tasks 3 and 5 (in `fin-core` it touches only a new `layout.rs`). Task 20 may run beside Tasks 14–19.

## Decisions taken here (the spec leaves them open; Task 27 records them in the spec)

### Versions, migrations and upgrade

1. **One schema version for ops and model: `SCHEMA_VERSION` 1 → 2.**
   - Every record 4C writes is version 2, plain clip edits included. A 4B (or 4A) follower reads such a record as `NEWER_FORMAT` and goes read-only (the record-level rule, principle 6). **This is accepted (coordinator):** the spec and the README upgrade notes say that the first 4C edit on a folder turns its 4B machines read-only.
   - The segment format, the record format and the `.fsnap` framing do not change; `.fsnap` headers say `modelSchemaVersion: 2`.
2. **`finisher.project.json` `schemaVersion`:**
   - new projects write 2;
   - an existing file is never bumped (its other rewrites keep the value);
   - reads accept 1 and 2;
   - **a higher value opens read-only `NEWER_FORMAT`**: `ProjectFile::is_newer()` counts it, and `read_project_file_once` no longer refuses it (today it fails `SCHEMA_VERSION` at open; only the poll path treats it as newer, `tick.rs` `reread_project_file`).

   Task 3 lands this as its own commit, which also applies to `sp4b-history` unchanged. The coordinator cherry-picks it there before 4B ships, so a 4B machine opening a 4C-created project goes read-only instead of failing. If it is not cherry-picked, Task 27 documents that 4B refuses 4C-created projects with `SCHEMA_VERSION`.
3. **DB schema v3 (`m0003_version`, frozen) is a version bump only:**
   - `user_version` 3, `formatVersion` and `minReaderVersion` `"3"`;
   - no table changes, and no data rewritten.

   *Why:* a 4C database holds v2 snapshots and records, and a 4B binary must refuse it (`DB_TOO_NEW`) rather than misread it. **Nothing is cleared or replayed** (coordinator):
   - stored v1 diffs are upgraded on read (Decision 5);
   - old `snapshots` rows are upcast on every read and never rewritten. `diffs::fill` only inserts the missing snapshots at multiples of 200 (`diffs.rs:70`), so it does not upgrade old ones.
4. **Per-op diffs:**
   - `Change { kind, id, change, op, edge?, path?, by? }` and `Shift { track, from, by, op }`. `op` is the index of the op in its transaction;
   - `by` on a change is the **content offset** of a start-edge move: every item-relative time of that item moved by it (Decision 13);
   - an op may record any number of shifts and changes. The 4B rule "clip ops record exactly one shift" is gone, and `split_diff` splits by `op`;
   - **ripple never records shifted items as changes** (the shift covers them);
   - entity kinds: `asset, track, timeline, item, transition, marker, bin, effect, link, style`. `item` replaces 4B's `clip`; the item's kind is in its id.
5. **Upcasters v1 → v2** (frozen, `upgrade/`):
   - **Record step.** In `intent.ops`, `resolved` and `inverse`:
     - `clip.insert` → `item.insert` (`mode: "ripple"`, `kind: "clip"`);
     - `clip.trim` → `item.trim` (`mode: "ripple"`);
     - `clip.delete` → `item.delete` (`ripple`);
     - `clip.remove` → `item.remove` (`ripple` → `"ripple"`/`"lift"`);
     - `clip.restore` → `item.restore` (**a missing `ripple` means `true`**, the v1 serde default);
     - `asset.add` gains `kind: "media"`.
   - **Snapshot step:** builds the v2 model with the defaults of Decision 9.
   - **Diff step:** a stored diff is v1-shaped when **no** entry has `op` (an empty diff reads the same in both shapes; a mix is `INVALID_FORMAT`). 4B's v1 ops each recorded exactly one change, and each clip op exactly one shift (`ops/*` at `36fe7e0`; `clip.delete` goes through `clip.remove`). So the step gives entries their op indexes in order. A `clip.trim` change gains `edge` (from the op) and, on the start edge, `by = −delta`; `kind` `clip` becomes `item`. It takes the record's raw v1 ops, and its output must equal what 4C's replay of the upcast record records (`verify` `diffs_match` on all three fixtures).
   - **`new_project(genesis)` must equal `upcast_snapshot(<v1 rev-0 JSON of the same genesis>)`.** A test pins it, because a rebuild of an old project starts from `genesis` in the new shape and must meet the migrated database (`replay_matches_live`).
6. **Aliases** (spec §5.3).
   - `clip.insert|trim|delete` are accepted from **clients** for schema version 2 only. A client tx at `schemaVersion: 1` holding only v1 op kinds is accepted too, and a v1 `asset.add` gains `kind: "media"`.
   - `fin_core::normalize(tx)` rewrites them to `item.*` and stamps version 2. It runs at the commit entry (`fin-store` `commit.rs`, `fin`'s `Builder`), so a stored v2 intent never holds an alias.
   - An alias in `Internal` or `Replay` mode is `INVALID_OP`.
   - Removing the aliases at schema 3 is a TASKS follow-up.
7. **`argsHash` is stable across the upgrade.** New `fin clip insert` arguments (`--mode`, `--no-audio`, `--audio-track`) are skipped from the hashed input when absent, so an equivalent call hashes as it did in 4B; a golden test pins the 4B hashes. A retry across the upgrade is answered from the log (`tx.get`), never re-sent, so the upgrade cannot cause `IDEM_KEY_REUSED`. The default linked audio (Decision 21) changes what a *new* call does, not the hash.

### Model

8. **Layout** (spec §5.1). `Project { id, name, schemaVersion, rate, timelines: Vec<Timeline>, bins, assets, styles }`.
   - `rate` stays: it is the genesis rate and the default for new timelines.
   - **`fonts` is derived, not stored:** `layout::fonts(project)` lists the bundled families plus the font assets. *Deviation from spec §5.1 "fonts (registry)". Why:* one source of truth, and `asset.add`/`asset.remove` need no second edit.
9. **Rev-0 and upcast defaults** (owner answer 1):
   - timeline `tl1` named `"Timeline 1"`, rate = the project rate, **resolution 1080×1920 (vertical)**, frozen in the snapshot upcaster;
   - tracks named by kind in track order (`V1`, `V2`…, `A1`…, `C1`…); mute, solo and lock false; gain 0 dB;
   - **track order is compositing order, bottom first.**
10. **Ids: one prefix per kind** (owner answer 3), each with its own counter, all prefix + digits:
    - items: `c` clip, `cap` caption, `g` graphic, `img` image, `adj` adjustment, `nst` nested;
    - `m` marker, `tr` transition, `b` bin, `fx` effect, `lk` link group, `st` style;
    - 4B's `a`, `t`, `tl`, `br` are unchanged.

    Effect ids come from the global counter but are addressed as `(item, effect)`, and a split copies its effects with their ids. *Why:* agents read an entity's kind from its id (arch §9.2), and digits-only suffixes keep prefixes unambiguous (`cap1` is not a `c` id).
11. **Defaults are omitted in JSON** (`skip_serializing_if`), so snapshots stay small and the upcast output is minimal.
12. **Value types:**
    - times are `Rational`;
    - parameter values are finite `f64` (NaN and ±∞ are refused when parsed), `bool`, `[f64; 2]` or strings (enums, colours `#rrggbb[aa]`, asset refs);
    - `serde_json` `float_roundtrip` makes record bytes parse back to the same bits;
    - **gains are in dB.** `fin-core` never converts dB to amplitude: an `AudioPlan` carries dB ramps, and the mixer (sub-project 6) converts, because the conversion needs `pow`;
    - plans carry colours as `[f64; 4]` (0–1, unrounded), interpolated linearly per sRGB component. `fin view` prints hex with `floor(c × 255 + 0.5)`.
13. **Item-relative times are content-anchored.** Keyframe `at`, caption word times and a split's `offset` are relative to the item start.
    - A start-edge move by `d` (start trim, roll on a start edge, a slide's neighbours) shifts all of that item's keyframes and words by `−d`, and records `by = −d` on its change.
    - A split rebases the right part's times. A slip moves no item-relative time.
    - Values that fall outside the item are kept, never dropped, so every inverse is exact; eval clamps.
14. **Speed** (clips): source time = `sourceIn + (t − start) × speed`, exact `Rational`. `sourceIn + duration × speed ≤ asset.duration` is a policy check.

### Edits and rebase

15. **Positions are in the frame before the op** (`Assoc::After`), `item.move`'s `at` included: it is the drop point on the timeline as the client saw it. Lengths are never mapped: slip, slide and trim deltas.
16. **Item-relative values are mapped through content offsets** (blocker B3; option "map" chosen over "conflict").
    - `StepMap` gains `content: Vec<(item, Rational)>`, from the steps' `by` changes, and `Mapping::map_content(item, v)` adds them up; an inverse map negates them.
    - Ops expose `offsets_mut() -> Vec<(String, &mut Rational)>`: `item.split` `offset`, `keyframe.set`/`keyframe.remove` `at`, `caption.split` none (it takes a word index).
    - The engine maps them like positions, through `[inverse(our earlier ops), steps, our rebased earlier ops]`.
    - *Why map rather than conflict:* a start-edge move is a pure translation of that item's relative times, so translations compose by addition. Mirrored pairs (an undo with its unit) cancel exactly, and nothing is ever deleted or needs recovering. Making these ops touch the start edge would refuse common, unambiguous edits (a keyframe set while someone trims the head) and would also make two keyframe edits conflict with each other. **Keyframe identity survives:** the trim moved every keyframe of the item by the same `by`, so the mapped `at` names the same keyframe. A mapped `offset` that leaves the item fails the re-run's policy check, which is a conflict with that code.
17. **At most one id allocation per op:**
    - `caption.import` expands in `fin` into one `item.insert` per block (`captions::blocks`);
    - an overwrite never splits an item: one landing strictly inside a single item is `OVERWRITE_SPLITS_ITEM`, and `compose::overwrite` sends `item.split` first;
    - `item.split` of a linked item splits that item only, and `compose::split_linked` splits each member and links the right parts;
    - `transition.set` creates with a `$` id and updates with a literal one.

    *Why:* `BoundIds` and recovery's remap assume at most one placeholder per op, known from the op alone.
18. **Modes are required fields:**
    - `item.insert` and `item.move`: `ripple | overwrite`;
    - `item.delete`: `ripple | lift`;
    - `item.trim`: `ripple | roll | overwrite`, plus **internal `strict`** (blocker B4).

    **`strict`** moves the edge with no ripple and no carving, and an overlap is refused (`CLIP_OVERLAP`) in every mode. It is used by the inverses of an overwrite (trimmed neighbours go back), a split (the left part grows back) and a caption merge, so an undo meets a later edit as a conflict, not as silent carving. `fin` defaults to `ripple` for clips and to `overwrite` (`lift` for delete) for every other kind, and echoes `resolved.mode`.
19. **Link groups** (`lk<n>`, spec §5.1 `link?`):
    - `item.move` moves every member by the same time delta, and only the named item changes track;
    - `item.trim` moves the same edge of every member by the same delta, all or nothing;
    - `item.delete` deletes every member, rippling each track;
    - `item.slip`, `item.slide` and `item.split` act on the named item only.

    Members are found at apply time and recorded in the op's diff.
20. **Footprints:**
    - Ops keep their 4B rules for what they name. `Footprint::with_changes(own_diff)` adds what the diff records: `Removed` → removes, `Added` → creates, `Updated` with `edge` → `Touch::Edge`, `Updated` with `path` → `Touch::Param`, other `Updated` → `Touch::Exists`.
    - Sub-entity ids are `"<item>/<fx>"` and `"<track>/<transition>"`.
    - Per op:
      - `item.slip` touches `Param("sourceIn")`;
      - `item.split` touches `Exists` on the item and creates the right part;
      - `item.slide` touches `Param("start")` on the named item, and its neighbours' edges from the diff.
    - `Touch::Param` keys:
      - the registry path for `param.set`;
      - `"<path>@<at>"` for keyframes;
      - `"effects"` for effect order;
      - `"caption.words"`, `"caption.style"`;
      - `"transition.out"` on A and `"transition.in"` on B.

      So two transitions set at one cut conflict `SAME_PARAM`.
21. **Linked audio by default** (owner answer 4).
    - `fin clip insert` of an asset with video and audio on a video track sends, in one tx: the video clip, an audio clip item (same asset, `sourceIn`, duration, speed and mode) on the audio track, and `item.link` of the two (`compose::clip_with_audio`).
    - The audio track is `--audio-track <t>`, else the timeline's first audio track; with none, it is `USAGE` with `fix` "`--no-audio`, or `fin track add --kind audio`".
    - `--no-audio` opts out.
22. **Transitions** live on the track and are centred on the cut.
    - Handles are a policy check for clip items (`TRANSITION_NO_HANDLES`): A needs `duration/2 × speed` of source after its out point, B needs as much before its `sourceIn`.
    - **A generic post-op pass prunes** every transition whose two items no longer touch on its track: a `Removed` change, plus a restoring `transition.set` appended to that op's inverse. It runs in every mode, replay included.
    - Adjacency is also checked structurally (`TRANSITION_INVALID`).
    - **Linked partners:** `compose::transition_with_links` also sets a `crossfade` (same duration and curve) between the A and B items' link partners when those touch on another track. `--no-linked` opts out.
23. **Inverse shift order** (blocker B5). An op's inverse ops, applied in order, record exactly the inverses of the op's shifts **in reverse order**, because `Mapping::set_mirror` pairs shifts in reverse (`rebase/map.rs` `set_mirror`: "b's shifts are a's in reverse order").
    - Example: a ripple move records `[close src {end, −dur}, open dst {at', +dur}]`, and its inverse must record `[{at' + dur, −dur} on dst, {start, +dur} on src]`.
    - Every golden checks it (`inverse_shifts_mirror` helper), property 1 checks it for every op, and Task 25 seeds the reordered bug.
24. **Ids nested in ops** (blocker B6). `Op::ids()` and `refs_mut()` list **every** id an op carries, nested ones included, because the store's counters and recovery's remap rely on them:
    - `item.restore`: the item's id, asset, style, link, slot asset refs, nested timeline and effect ids;
    - `track.restore`: the track, its items (as above) and its transitions;
    - `timeline.restore`: everything in it;
    - `effect.restore`, `style.add`, the literal `*.add` inverses.

    `ids_cover_every_id_in_json` walks an op's JSON and checks that every id-shaped string outside the free-text fields (`label`, `name`, `text`, `path`, `fingerprint`, `words[].text`, `color`) is listed. It runs per op in the goldens and over every generated op in property 1.
25. **Structural checks for replay** (blocker B1):
    - The track check (`CLIP_OVERLAP`/`CLIP_EMPTY`/`CLIP_NEGATIVE` for every item kind, codes unchanged, plus `TRANSITION_INVALID`) runs after each op on **every track its diff touched**:
      - the tracks of its shifts;
      - the tracks that hold, after the op, any item a change names;
      - tracks named by track or transition changes.

      4B checked shift tracks only, which would miss no-shift ops (overwrite, lift, roll, slip, slide, split, strict).
    - Value shapes come from deserialisation.
    - **Everything else is policy:** registry ranges and types, handles, lock, slot types, caption word order, caption overflow.
26. **Markers never ripple** (owner answer 5): they are timeline-level, with no track positions to map. **Track lock** is a policy check (`TRACK_LOCKED`) on every item op touching a locked track's items.
27. **Effects:**
    - `effect.add` allocates `$fx` from the global counter; the effect list order is the render order;
    - built-in kinds: `colorAdjust`, `lut`, `blur` (crop is part of the item);
    - effect params are a sparse map validated by the effect's specs, so plugin-declared specs fit later;
    - easing presets (`ease`, `easeIn`, `easeOut`, `easeInOut`, CSS values) are resolved by `fin` into stored `bezier` values.
28. **Styles:**
    - caption styles are built-in presets (`fin-core` data, `preset:<name>`) plus project styles `st<n>`;
    - `style.add`, `style.set` and `style.remove` are new ops, not in the spec §5.3 table;
    - safe zones are named data (`tiktok`, `reels`, `shorts`, `generic916`, `generic169`) with their source noted;
    - a font asset whose family a style names is in use (`ASSET_IN_USE`).

### Text and evaluation

29. **`fin-text`:**
    - `cosmic-text` with `no_std`: all float functions go through `libm`, pure Rust, the same on every target;
    - always our own `Fallback`. `cosmic-text`'s `PlatformFallback` differs per OS, so `FontSystem::new*` without ours is never called;
    - fixed locale `"en-US"`;
    - faces loaded in manifest order, then project fonts by asset id, into an empty `fontdb::Database`;
    - **one process-wide name interner**: the `Fallback` trait needs `&'static str`, and each distinct family name is leaked once, never per measurer rebuild;
    - output in 1/64 px `i32` at the timeline resolution.
    - **Fallback order:**
      - common: `Inter, Noto Sans, Noto Sans Arabic, Noto Sans Hebrew, Noto Sans CJK SC, Noto Color Emoji`, then project families;
      - per script: Han → `Noto Sans CJK SC, JP, TC, KR, HK`; Hiragana/Katakana → `JP`; Hangul → `KR`.
    - **`fingerprint()`** = `fin-text` version + the `cosmic-text`, `harfrust` and `skrifa` versions (from `Cargo.lock` via `build.rs`) + the BLAKE3 of the font set. It is carried in plans.
    - **`fin_text::font_info(bytes)`** is the one source of family, weight and italic names: `fin-probe`'s font probe calls it, so a font asset's family is exactly the name the measurer matches.
30. **Bundled fonts** (owner answer 2):
    - all upstream files, unmodified, listed in `assets/fonts/fonts.json` (file, family, weight, italic, `sha256` for fetching, `blake3` for loading, licence, upstream URL at a pinned commit);
    - the four small families are **committed**;
    - **Noto Sans CJK and Noto Color Emoji (about 50 MB) are fetched** by `pnpm fonts:fetch` (`scripts/fetch-fonts.mjs`, pinned URLs, `sha256` checked) into the git-ignored `assets/fonts/fetched/`; CI caches them. *Why:* 50 MB of binaries in a public git history weighs on every clone forever, and each file stays under GitHub's limits either way. The owner may prefer committing them; only the fetch step changes;
    - a gate checks hashes and licences, and checks fetched files only when present (or required, with `FIN_REQUIRE_FONTS=1`);
    - loaded at run time by `fin-node` (`FIN_FONTS_DIR`, else `<binary dir>/fonts`, else the repo's `assets/fonts` in debug builds), never embedded. The `fin-node` package grows by about 52 MB; `fin` does not change.
31. **Measurer injection:**
    - `fin_core::layout::TextMeasurer: Send + Sync`;
    - `ApplyEnv { measurer }` goes through `Applier`, the rebase engine (`Rebase.env`, `rebase_tx(.., env)`) and the history planners (`LogView::measurer()`, default `None`);
    - the store takes it from `OpenOptions.measurer` / `Store::set_measurer`, and `fin-node` builds it.
    - **Overflow is checked once per transaction, at its end** (`Applier::finish`, and after the last op of a rebase), over every caption the transaction changed. The error names the last op that changed the item.
    - **Without a measurer it is not checked:** always in replay; in `fin`'s offline dry runs (`measured: false`); and when the fonts are damaged (Decision 32).
32. **Damaged or missing fonts never block editing.** A bundle that fails verification gives status warning `FONTS_DAMAGED {files}` and **no measurer**: no overflow checks, and eval plans carry `layout: null` and `textEngine: null`. A project font asset whose file is missing is left out with warning `FONT_MISSING {asset, path}`.
33. **Eval runs on the server.** `fin eval frame|audio` needs `finisherd` (fonts and project font assets live there), like the 4B history commands.
34. **Plans:**
    - layers come bottom-up in track order; a `nested` item is a group layer with `children`; an `adjustment` item is a layer with effects only; a transition yields A (`role: out`) then B (`role: in`) with `progress`;
    - a video or caption track with `mute` set is hidden;
    - only **audio tracks** produce audio items (Decision 21 links audio items to video clips);
    - `solo` on any audio track silences the non-solo ones;
    - `AudioPlan` gain segments break at every keyframe, fade and transition edge, each with `db: [d0, d1]`, its `interp`, and `shapes` (fade or crossfade, `linear` or `equalPower`, progress `[p0, p1]`).
35. **Bézier:** CSS `cubic-bezier`. `x(t) = u` is solved by 8 Newton steps, accepted when `|x(t) − u| < 1e-7` and `|x'(t)| > 1e-6`, else by 32 bisection steps on `[0, 1]`.

### Interfaces and CLI

36. **No per-edit RPCs.** Every edit is an op in `tx.commit`, and Rust validates it. The new RPCs are `eval.frame` and `eval.audio`.
37. **`fin` commands:**
    - `fin clip insert|trim|delete` stay (they build `item.*`);
    - `fin item move|split|delete|trim|slip|slide|link|unlink` work on any kind;
    - `fin image|graphic insert`, then the track, timeline, marker, bin, asset, transition, param, keyframe, effect, caption, style, schema and eval groups;
    - **`fin` alone keeps its map under 1.5k tokens** by listing command names per group, with usage lines in `fin help <group>`;
    - no `fin batch` changes (sub-project 7).

    `fin` itself has no editing semantics: every multi-op command calls a `fin_core::compose` builder.
38. **New codes** (exit 3 unless noted):
    - `OVERWRITE_SPLITS_ITEM`, `NOT_ADJACENT`, `TRANSITION_NO_HANDLES`, `TRANSITION_EXISTS`, `TRANSITION_INVALID`, `TRACK_LOCKED`;
    - `TRACK_NOT_EMPTY`, `TIMELINE_NOT_EMPTY`, `TIMELINE_NESTED`, `BIN_NOT_EMPTY`, `BIN_CYCLE`, `ALREADY_LINKED`, `SLIDE_BLOCKED`, `MOVE_INTO_ITSELF`;
    - `PARAM_UNKNOWN`, `PARAM_TYPE`, `PARAM_NOT_ANIMATABLE`, `PARAM_ANIMATED`, `SLOT_UNKNOWN`, `STYLE_IN_USE`, `CAPTION_OVERFLOW`, `SRT_INVALID`;
    - `NESTED_CYCLE` (eval only, defensive).

    `FONTS_DAMAGED` and `FONT_MISSING` are status warnings (Decision 32). `INVALID_RANGE` gains `details {path, min, max, unit}` for parameters, and `ASSET_IN_USE` gains `details.items`. `INSERT_INSIDE_CLIP` keeps its name for every item kind and gains `details.item`. *Why:* codes are stable API (arch §9.7).
39. **`fin view`:**
    - JSON `{rev, timeline, lines, elided, tokens}`, one screenplay line per item per track in time order, captions' text inline, markers on their own lines;
    - tokens estimated as `ceil(chars / 4)`;
    - degrades in fixed steps until it fits: drop parameter notes → merge runs of adjacent clips of one asset → shorten caption text to 6 words + `…` → elide middle time ranges. Each step is listed in `elided`.
40. **Upgrade notes** (README and spec): a 4C machine turns 4B followers read-only at its first edit (Decision 1); a 4C-created project opens read-only on a patched 4B, or not at all on an unpatched one (Decision 2); a 4C database is refused by 4B (`DB_TOO_NEW`, Decision 3).
41. **Last-batch kinds** (coordinator: keep only what is cheap):
    - `adjustment` items are enabled by one `item.insert` rule (Task 6), and `blend` is a registry path (Task 10). Both stay in 4C.
    - **The edits that make `nested` items and masks move to sub-project 6:** a nested insert with its cycle rule, and `mask.set`/`mask.remove`. They need ops, guards and tests of their own, and nothing consumes them before sub-project 6. *Why:* the plan is already 28 tasks.
    - The model, the registry's `mask.*` paths, eval of `nested` and masks (tested on projects built literally), the `TIMELINE_NESTED` guard and the defensive `NESTED_CYCLE` stay, so sub-project 6 adds only ops.
    - A TASKS follow-up records it.

## Questions for the owner

None open. All five were answered and are recorded as Decisions 9, 30, 10, 21 and 26. Decision 30's fetch-instead-of-commit for the large fonts is a choice the owner may reverse without other changes.

## Review Focus

1. **The upgrade path.**
   - The SP2, 4A and 4B fixtures open in 4C with states equal to hand-checked v2 goldens, and their upcast v1 diffs equal a fresh replay.
   - `new_project` equals the upcast v1 genesis.
   - An undo, a merge and a cherry-pick of pre-4C records work after upgrade.
   - A patched 4B opens a 4C project read-only.
   - → Tasks 3, 5, 6: `diff_upcast_equals_replay_on_fixtures`, `newer_project_file_opens_read_only`, `sp2_4a_4b_fixtures_open_equal_v2_goldens`, `new_project_equals_upcast_genesis`, `undo_of_4b_record_after_upgrade`, `merge_of_4b_branch_after_upgrade`; Task 28 mixed-version row.
2. **Footprints from the op's own diff** catch conflicts on items the op did not name. → Tasks 7–9: `overwrite_vs_trim_of_overwritten_item_conflicts`, `linked_delete_footprint_covers_members`, `pruned_transition_restored_by_undo`.
3. **Content offsets** (B3). A stale split or keyframe edit after a concurrent start trim lands on the same content, and a stale keyframe edit names the same keyframe. → Tasks 7, 13: `stale_split_after_start_trim_maps_offset`, `stale_keyframe_after_start_trim_keeps_identity`; property 3.
4. **Exact inverses**, with shifts in reverse order (B5), for every op, pruned transitions and link groups included; the strict trim (B4) refuses overlap. → Task 25 property 1 plus seeded bugs; Task 7 `undo_of_overwrite_conflicts_with_later_insert`.
5. **Replay's track check** covers no-shift ops (B1). → Task 6 `replay_checks_tracks_touched_by_changes`; property 6.
6. **Numbers round-trip** through record bytes (B2). → Task 5 `param_values_round_trip_through_record_bytes`.
7. **Nested ids** are counted and remapped (B6). → Tasks 1, 6, 11, 13: `ids_cover_every_id_in_json`; fin-store `counters_see_ids_inside_restores`, `recovery_remaps_nested_ids`.
8. **Determinism.** Plan hashes and text layouts are identical on macOS, Windows, Linux and wasm32, and the float gate holds. → Tasks 4, 16, 28.
9. **Measurer threading.** Replay never calls it; commit, rebase, merge, undo and recovery re-check overflow once per tx; damaged fonts never block. → Tasks 19, 21.
10. **Frozen code.** The m0001 and m0002 pins are unchanged, m0003 is pinned, and every upcaster step has its golden. → Tasks 3, 5, 6.

---

### Task 1: Make room (pure refactor) — tier: sonnet

**Files:**
- `crates/fin-core/src/model.rs` → `model/mod.rs` (no change).
- `crates/fin-core/src/ops/mod.rs`:
  - move the `Op` enum and `Edge` into `ops/defs.rs`;
  - move each op's arm of `name`, `is_internal`, `refs_mut`, `allocates` and `records_shift` into its own module (`pub(crate) const NAME`, `INTERNAL`, `fn refs_mut`, `fn allocates`);
  - dispatch through one `ops_table!` macro listing `Variant => module` once, generating `name`, `is_internal`, `refs_mut`, `ids`, `allocates`, `positions_mut`, `apply` and `footprint`.
- `crates/fin-core/src/ops/util.rs` → `ops/util/{mod.rs, lookup.rs, ripple.rs, check.rs}`.
- `crates/fin/src/commands/mod.rs` → move `Builder` into `commands/builder.rs`. `Builder::push` substitutes working ids through `Op::refs_mut()` instead of per-op matches.
- Create: `crates/fin-core/tests/common/ids_json.rs` (`ids_cover_every_id_in_json`, Decision 24) and use it in `ops_table_lists_every_variant`.

**Interfaces:** none new. All items keep their names and visibility.

- [x] **Step 1:** Move the code with no logic changes. Doc comments move with their items. Add:
  - `ops_table_lists_every_variant` (every variant's `name()` equals its serde tag, round-tripped through JSON, and passes `ids_cover_every_id_in_json`);
  - `ids_json_helper_flags_an_unlisted_id` (a hand-made op JSON with an extra `"c9"` in an id field fails the helper).
- [x] **Step 2:** `rtk wc -l` the touched files: each must be ≤ 450.
- [x] **Step 3: Run** `rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates`. Expected: PASS with the same test count plus two (record it in the commit body).
- [x] **Step 4: Commit** `refactor(core): one op table, split ops and model modules ahead of 4C`.

### Task 2: Scrubbed 4B fixture, generated from 4B code — tier: sonnet

**Files:**
- Create: `crates/fin-store/tests/fixtures/4b/folder/finisher.project.json`, `.../folder/.finisher/segments/{main,br1}/*.fseg`, `crates/fin-store/tests/fixtures/4b/project.fdb`, `.../4b/expected.json` (`{branches: {main: {rev, project}, br1: {rev, project}}, stacks, argsHashes}`), `.../4b/README.md` (the commit hash and the scrub steps).
- Modify: `crates/fin-store/tests/integration_support/mod.rs` (`fixture_4b`).
- Create: `crates/fin-store/tests/fixture_4b.rs`.
- The generator is **not** committed. It lives in a 4B worktree under `archive/`.

- [ ] **Step 1: Generate from 4B code.**
  - `rtk git worktree add archive/wt-4b 36fe7e0` (4B as merged with the latest 4A).
  - In it, write `crates/fin-store/examples/make_4b_fixture.rs`:
    1. create a project; `asset.add`;
    2. on `main`, 4 inserts on `t1` and `t2`, 2 trims (one on each edge), 1 delete;
    3. `branch new feature` and 2 edits there; checkpoint `v1` on `main`;
    4. one more edit on `main`; `merge feature`; `undo`, `redo`; a cherry-pick of a `feature` record back onto `main`;
    5. `tick` until exported; `close`; `PRAGMA wal_checkpoint(TRUNCATE)`.
  - Also record in `expected.json` the `argsHash` that 4B's `fin` computes for three fixed `clip insert|trim|delete` inputs (Decision 7). Compute them with 4B's `fin` `args_hash` in the worktree.
- [ ] **Step 2: Scrub** as 4B Task 2 did:
  - copy only `finisher.project.json`, the `.fseg` files and `project.fdb`;
  - `DELETE FROM meta WHERE key IN ('projectDir','serverIds','createKey'); DELETE FROM lease_state; VACUUM;`;
  - write the states and the author's stacks.
- [ ] **Step 3: Check for leaks:** `rtk proxy bash -c 'grep -rIl -e "$(hostname)" -e "$USER" crates/fin-store/tests/fixtures/4b || echo clean'`, plus `strings project.fdb` with the same patterns. Expected: clean.
- [ ] **Step 4: Add the helper** `fixture_4b(tmp) -> (server_data, folder)` (copy, place the DB, open with `relocate: true`) and the smoke test `fixture_4b_opens_with_relocate`: both branch states equal `expected.json` with today's code.
- [ ] **Step 5: Run** `rtk cargo test -p fin-store`, `rtk pnpm gates`. Remove the worktree with `rtk git worktree remove archive/wt-4b`.
- [ ] **Step 6: Commit** `test(fin-store): scrubbed 4B fixture for 4C upgrade tests`.

### Task 3: Per-op diffs, the v1 diff upcaster, migration 0003, newer project files read-only — tier: opus

**Files:**
- **Commit A** (cherry-pickable onto `sp4b-history`, Decision 2): `crates/fin-store/src/project_file.rs` (`is_newer()` counts `schemaVersion > SCHEMA_VERSION`; `read_project_file_once` stops refusing it), its tests, and `crates/fin-store/tests/store.rs` (`newer_project_file_opens_read_only`).
- **Commit B:**
  - `crates/fin-core/src/tx.rs` (`Change`, `Shift`, `EntityKind`), `crates/fin-core/src/apply.rs` (the `Applier` hands each op its index);
  - `crates/fin-core/src/ops/util/ripple.rs` (`OpDiff`), every op module (pass the sink through);
  - `crates/fin-core/src/rebase/footprint.rs` (`split_diff`, `Footprint::with_changes`), `crates/fin-core/src/rebase/steps.rs`;
  - create `crates/fin-core/src/upgrade/diff_v1.rs` (frozen step; `upgrade.rs` → `upgrade/mod.rs`).
- **Commit B, store side:**
  - create `crates/fin-store/src/migrations/m0003_version.rs`; modify `crates/fin-store/src/migrations/mod.rs` (`DB_VERSION = 3`, `MIN_READER_VERSION = 3`, step), `crates/fin-store/src/db/mod.rs` (a fresh `meta` writes `"3"`);
  - **every reader of `txs.diff` goes through one helper**, `db::records::stored_diff(record_value, diff_json)`, which upcasts. The readers are `db/records.rs` `decode`, `history_ops/mod.rs` (the `Diff` parse near line 210) and `verify/replay.rs` (the `diffs_match` compare).
  - modify the RPC fixtures under `crates/fin/tests/` and the TS tests that carry diffs.
- Test: `crates/fin-core/tests/diff_split.rs` (new), `crates/fin-core/tests/upgrade_golden.rs` (new, the diff step), `crates/fin-store/tests/{migrations.rs, fixture_4b.rs}`.

**Interfaces:**
```rust
// fin_core::tx
pub struct Change {
    pub kind: EntityKind, pub id: String, pub change: ChangeKind,
    pub op: u32, // index of the op in its transaction
    #[serde(default, skip_serializing_if = "Option::is_none")] pub edge: Option<Edge>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub path: Option<String>,
    /// Content offset of a start-edge move (Decision 13).
    #[serde(default, skip_serializing_if = "Option::is_none")] pub by: Option<Rational>,
}
pub struct Shift { pub track: String, pub from: Rational, pub by: Rational, pub op: u32 }
pub enum EntityKind { Asset, Track, Timeline, Item, Transition, Marker, Bin, Effect, Link, Style }
// ops::util
pub(crate) struct OpDiff<'a> { diff: &'a mut Diff, op: u32 }
impl OpDiff<'_> {
    pub(crate) fn record(&mut self, kind: EntityKind, id: &str, change: ChangeKind);
    pub(crate) fn record_edge(&mut self, kind: EntityKind, id: &str, edge: Edge, by: Option<Rational>);
    pub(crate) fn record_path(&mut self, kind: EntityKind, id: &str, path: &str);
    pub(crate) fn shift(&mut self, track: &str, from: Rational, by: Rational);
}
// fin_core::rebase::footprint
pub fn split_diff(ops: &[Op], diff: &Diff) -> Result<Vec<Diff>, CoreError>; // by `op`; INVALID_FORMAT out of range/order
impl Footprint { pub(crate) fn with_changes(self, own: &Diff) -> Footprint; } // Decision 20
// fin_core::upgrade (frozen)
/// A v1-shaped stored diff (no entry has `op`) of a record whose raw v1 ops are `v1_ops`, in the
/// v2 shape (Decision 5). A v2 diff is returned unchanged; a mix is INVALID_FORMAT.
pub fn upcast_diff(diff: Value, v1_ops: &[Value]) -> Result<Diff, CoreError>;
```
In this task `SCHEMA_VERSION` is still 1, so `kind` stays `clip` until Task 6 renames it in the model, and in the diff step together with its golden.

- [ ] **Step 1: Commit A.**
  - Tests: `newer_schema_version_is_newer`, `newer_project_file_opens_read_only` (open answers read-only `NEWER_FORMAT`, reads work, commits refuse), `older_schema_version_still_opens`.
  - Implement, run `rtk cargo test -p fin-store`, commit `fix(store): a project file at a newer schemaVersion opens read-only`.
  - Check that it applies cleanly with `rtk git cherry-pick --no-commit` onto a scratch checkout of `sp4b-history`, run `rtk cargo test -p fin-store` there, then discard the checkout. Report the commit hash to the coordinator.
- [ ] **Step 2: Write failing tests for commit B:**
  - `split_diff_by_op_index`, `split_diff_rejects_out_of_range_or_unordered_op`, `ops_record_their_own_index` (a tx of `asset.add`, two inserts, a start trim: the trim's change carries `edge` and `by`);
  - `with_changes_maps_removed_added_edge_path`;
  - `upcast_v1_diff_golden` (frozen v1 diffs of each v1 op kind in, v2 out), `mixed_diff_is_invalid_format`, `empty_diff_reads_the_same`;
  - `diff_upcast_equals_replay_on_fixtures` (SP2, 4A and 4B: every row's upcast stored diff equals its replay; `verify` ok, `diffs_match` included);
  - `migration_0003_is_frozen` (pins `user_version` and the meta values); `fresh_and_migrated_schemas_match` (unchanged test); `db_v3_is_refused_by_a_v2_reader` (through the existing refusal helper);
  - every 4B rebase and history test still passes unchanged.
- [ ] **Step 3: Run** `rtk cargo test -p fin-core --test diff_split --test upgrade_golden` and `rtk cargo test -p fin-store --test migrations --test fixture_4b`. Expected: FAIL.
- [ ] **Step 4: Implement.** m0003 runs
  ```sql
  UPDATE meta SET value = '3' WHERE key IN ('formatVersion', 'minReaderVersion');
  ```
  and the runner sets `user_version` 3. It leaves `txs.diff` and `snapshots` alone. `stored_diff` parses the record's raw JSON once, takes the raw `intent.ops` when `schemaVersion` is 1, and calls `upcast_diff`.
- [ ] **Step 5: Regenerate types** (`rtk pnpm --filter @finisher/core-types generate`); update the RPC fixtures and TS tests that hold diffs.
- [ ] **Step 6: Run** the three gates → PASS. **Commit** `feat(core,store): per-op diffs; v1 diffs upcast on read; DB v3`.

### Task 4: The `TextMeasurer` contract, `fin-text`, bundled fonts — tier: opus

The riskiest unknown goes first: deterministic shaping across OSes. The S11b spike code is not available, so `fin-text` is written fresh from arch §27.10 item 10 and this plan (Task 27 records it).

**Files:**
- Create: `crates/fin-core/src/layout.rs` (contract types; re-exported). Modify `crates/fin-core/src/lib.rs`.
- Create: `crates/fin-text/Cargo.toml`, `crates/fin-text/build.rs` (dependency versions from `Cargo.lock`), `crates/fin-text/src/{lib.rs, fonts.rs, fallback.rs, intern.rs, info.rs, measure.rs}`.
- Create: `crates/fin-text/tests/corpus.rs`, `crates/fin-text/tests/corpus/{cases.json, expected.json, layouts/*.json}`.
- Create: `assets/fonts/{Inter-Regular.ttf, Inter-Bold.ttf, NotoSans-Regular.ttf, NotoSans-Bold.ttf, NotoSansArabic-Regular.ttf, NotoSansArabic-Bold.ttf, NotoSansHebrew-Regular.ttf, NotoSansHebrew-Bold.ttf, fonts.json, OFL-Inter.txt, OFL-Noto.txt, OFL-NotoCJK.txt, OFL-NotoEmoji.txt}`, with `assets/fonts/fetched/` in `.gitignore`.
- Create: `scripts/fetch-fonts.mjs` (`pnpm fonts:fetch`), `scripts/check-fonts.mjs`, `scripts/check-fonts.test.mjs`.
- Modify: `package.json`, `scripts/gates.mjs` (the font gate; `cargo build -p fin-text --target wasm32-unknown-unknown`), `.github/workflows/ci.yml` (cache plus `pnpm fonts:fetch` before the tests, `FIN_REQUIRE_FONTS=1`; actions pinned to SHAs), `THIRD_PARTY_NOTICES.md`.

**Interfaces:**
```rust
// fin_core::layout (re-exported)
pub struct TextStyle { pub family: String, pub weight: u16, pub italic: bool, pub size: f64 /* px */,
    pub line_height: f64 /* × size */, pub letter_spacing: f64 /* px */, pub align: Align }
pub enum Align { Start, Center, End }
pub struct TextRequest { pub words: Vec<String> /* joined by one space */, pub style: TextStyle,
    pub max_width: Option<f64> /* px */, pub max_lines: Option<u32> }
/// 1/64 px integers. `overflow`: more lines than `max_lines`, or a word wider than `max_width`.
pub struct TextLayout { pub lines: Vec<LineBox>, pub words: Vec<WordBox>, pub width: i32, pub height: i32,
    pub overflow: bool, pub missing: Vec<u32> /* code points no font has */, pub engine: String }
pub struct LineBox { pub x: i32, pub y: i32, pub width: i32, pub height: i32, pub baseline: i32, pub rtl: bool, pub words: [u32; 2] }
pub struct WordBox { pub word: u32, pub line: u32, pub x: i32, pub y: i32, pub width: i32, pub height: i32 }
pub trait TextMeasurer: Send + Sync {
    fn measure(&self, req: &TextRequest) -> Result<TextLayout, CoreError>;
    fn fingerprint(&self) -> String; // Decision 29
    fn families(&self) -> Vec<String>;
}
// fin_text
pub struct FontFile { pub name: String, pub bytes: std::sync::Arc<[u8]> }
pub struct FontInfo { pub family: String, pub weight: u16, pub italic: bool, pub face: u32 }
pub fn font_info(bytes: &[u8]) -> Result<Vec<FontInfo>, CoreError>; // one per face (TTC too)
pub struct FontSet { /* faces in load order */ }
impl FontSet {
    /// `manifest` is `fonts.json`; every file's BLAKE3 must match, else Err(FONTS_DAMAGED {files}).
    pub fn bundled(manifest: &str, files: Vec<FontFile>) -> Result<FontSet, CoreError>;
    pub fn with_project_fonts(self, fonts: Vec<FontFile>) -> Result<FontSet, CoreError>;
    pub fn fingerprint(&self) -> String;
}
pub struct Measurer { /* Mutex<cosmic_text::FontSystem> */ }
impl Measurer { pub fn new(set: FontSet) -> Measurer; }
impl fin_core::layout::TextMeasurer for Measurer { /* ... */ }
```

- [ ] **Step 1: Licences and sources first.**
  - Add `cosmic-text = { version = "0.19", default-features = false, features = ["no_std"] }` and `unicode-script` at the version `cosmic-text` uses. Run `rtk cargo deny check licenses` and record each transitive licence in the commit body.
  - Pin the upstream fonts at release tags or commits: Inter 4.x; `notofonts` Sans, Arabic and Hebrew releases; `notofonts/noto-cjk` `Sans/OTC/NotoSansCJK-{Regular,Bold}.ttc`; `googlefonts/noto-emoji` `2D/fonts/NotoColorEmoji.ttf` (CBDT; colour bitmaps that `swash` can paint in sub-project 6). Record versions, URLs, `sha256` and `blake3` in `fonts.json`.
  - Confirm every licence text is OFL-1.1 and that the files are unmodified.
- [ ] **Step 2: Write failing tests** (`corpus.rs`). Each case is a `TextRequest` (and a font set) → its `TextLayout` JSON. The BLAKE3 hashes go in `expected.json`; the full layouts in `layouts/` are for review. Cases:
  - Latin with kerning (`"AVATAR Wave"`); Cyrillic; Greek;
  - Arabic shaping (RTL, joined forms); Hebrew RTL; mixed bidi (`"abc عربي 123"`);
  - wrapping at `max_width`; `max_lines` overflow; one over-wide word;
  - Bold weight; letter spacing; centre and end alignment;
  - Simplified Chinese, Japanese kana, Korean Hangul (each through its per-script fallback); emoji; an unassigned private-use code point (`missing`).

  Plus:
  - `measure_is_repeatable` (two calls, and two `Measurer`s, are equal);
  - `fallback_lists_are_ours` (`PlatformFallback` is never constructed; the lists are Decision 29's);
  - `interner_leaks_each_name_once`;
  - `damaged_font_is_refused` (`FONTS_DAMAGED`); `fingerprint_names_versions_and_fonts`;
  - `font_info_reads_ttc_faces`.

  `scripts/check-fonts.test.mjs`: a missing licence, a hash mismatch and an unlisted font each fail; a missing fetched font passes unless `FIN_REQUIRE_FONTS=1`.
- [ ] **Step 3: Run** `rtk pnpm fonts:fetch`, then `rtk cargo test -p fin-text`. Expected: FAIL.
- [ ] **Step 4: Implement.**
  - Build the `FontSystem` with `new_with_locale_and_db_and_fallback("en-US", db, OurFallback)`.
  - Lay out with `Buffer` (`Shaping::Advanced`, `Wrap::Word`), reading `layout_runs()` glyph clusters back to word boxes.
  - Round `f32` to 1/64 px with `libm::roundf`.
  - Each file stays < 400 lines.
- [ ] **Step 5: Goldens.** Generate `expected.json` once, then review `layouts/` by eye: Arabic and Hebrew runs are `rtl`, wrapping is at word boundaries, CJK and emoji glyphs come from their fonts, and boxes don't overlap.
- [ ] **Step 6: Early cross-OS signal** (controller). Run `rtk cargo test -p fin-text --test corpus` on the macOS and Windows hosts now, and on wasm32-wasip1 locally (wasmtime with `--dir assets/fonts`). Record the results in `archive/sp4c-text/`. A mismatch is fixed here, before anything depends on `fin-text`. Task 28 repeats it.
- [ ] **Step 7: Run** the three gates (the wasm32 build of `fin-text` included) → PASS. **Commit** `feat(text): fin-text measurement over bundled fonts; TextMeasurer contract`.

### Task 5: Model v2, the snapshot upcaster, new id kinds — tier: opus

The existing 7 ops are ported onto the v2 model unchanged in meaning (`clip.*` still apply, now to `Item` of kind `clip` on any timeline). Task 6 renames them.

**Files:**
- Create: `crates/fin-core/src/model/{timeline.rs, item.rs, asset.rs, caption.rs}`. Modify: `model/mod.rs` (`Project`, `new_project`, `SCHEMA_VERSION = 2`).
- Create: `crates/fin-core/src/upgrade/snapshot_v1.rs` (frozen step) and `upgrade/record_v1.rs` (an identity step for now; Task 6 fills it).
- Modify:
  - `crates/fin-core/Cargo.toml` (`serde_json` `float_roundtrip`);
  - `crates/fin-core/src/ids.rs` (Decision 10 kinds; `MemIds::after` walks every entity);
  - `crates/fin-core/src/ops/util/lookup.rs` (item, track and timeline lookup across timelines; `exists` for every kind);
  - every op module (now on `Item`), `crates/fin-core/src/time.rs` (`Rational::to_f64`).
- Modify the readers of `project.timeline`:
  - `crates/fin-store/src/{counters.rs (+ kind names), recover/remap.rs, branch/lineage_tests.rs}`;
  - `crates/fin/src/{addr.rs, commands/view.rs}`;
  - `crates/fin-store-xhost/src/{history.rs, ops.rs, hashes.rs}`;
  - `scripts/e2e/history-lib.mjs`, `scripts/e2e.test.mjs`, `apps/finisherd/test/server.test.ts`, `packages/fin-node/test/{engine,history}.test.ts`.
- Test: `crates/fin-core/tests/model.rs`, `crates/fin-core/tests/upgrade_golden.rs` with `tests/upgrade_golden/{snapshot_v1.json, snapshot_v2.json}`, `crates/fin-core/tests/record.rs`, and `crates/fin-store/tests/{migrations.rs, fixture_4a.rs, fixture_4b.rs}`.
- Create: `crates/fin-store/tests/fixtures/{sp2,4a,4b}/expected_v2.json` (generated once, reviewed by hand, then frozen).

**Interfaces** (`fin_core::model`; `#[serde(rename_all = "camelCase")]`; defaults omitted per Decision 11):
```rust
pub const SCHEMA_VERSION: u32 = 2;
pub struct Project { pub id: String, pub name: String, pub schema_version: u32, pub rate: Rational,
    pub timelines: Vec<Timeline>, pub bins: BTreeMap<String, Bin>, pub assets: BTreeMap<String, Asset>,
    pub styles: BTreeMap<String, CaptionStyle> /* Task 17 fills the struct; empty until then */ }
pub struct Timeline { pub id: String, pub name: String, pub rate: Rational, pub resolution: Resolution,
    pub markers: Vec<Marker>, pub tracks: Vec<Track> }
pub struct Resolution { pub width: u32, pub height: u32 }
pub struct Marker { pub id: String, pub at: Rational, pub duration: Option<Rational>, pub label: String, pub color: Option<String> }
pub enum TrackKind { Video, Audio, Caption }
pub struct Track { pub id: String, pub kind: TrackKind, pub name: String, pub mute: bool, pub solo: bool,
    pub lock: bool, pub gain: f64 /* dB */, pub items: Vec<Item>, pub transitions: Vec<Transition> }
pub struct Transition { pub id: String, pub between: [String; 2], pub kind: TransitionKind /* crossfade|dissolve */,
    pub duration: Rational, pub curve: Curve /* linear (default) | equalPower */ }
pub struct Item { pub id: String, pub start: Rational, pub duration: Rational,
    #[serde(flatten)] pub body: ItemBody, #[serde(flatten)] pub common: Common }
#[serde(tag = "kind")]
pub enum ItemBody {
    Clip { asset: String, source_in: Rational, speed: Rational /* default 1 */ },
    Caption(CaptionBlock),
    Graphic { asset: String, slots: BTreeMap<String, ParamValue> },
    Image { asset: String },
    Adjustment {},
    Nested { timeline: String, source_in: Rational },
}
pub struct Common { pub transform: Transform, pub crop: Crop, pub opacity: f64 /* 1 */, pub blend: BlendMode,
    pub effects: Vec<Effect>, pub mask: Option<Mask>, pub audio: Option<AudioParams>,
    pub keyframes: BTreeMap<String, Vec<Keyframe>>, pub link: Option<String> }
pub struct Transform { pub position: [f64; 2] /* px from frame centre */, pub scale: [f64; 2] /* 1,1 */,
    pub rotation: f64 /* degrees */, pub anchor: [f64; 2] /* item-normalised, 0.5,0.5 */ }
pub struct Crop { pub left: f64, pub right: f64, pub top: f64, pub bottom: f64 } // fractions
pub enum BlendMode { Normal, Multiply, Screen, Overlay, Add, Darken, Lighten }
pub struct Effect { pub id: String, pub kind: String, pub enabled: bool /* true */, pub params: BTreeMap<String, ParamValue> }
pub struct Mask { pub matte: String /* artifact ref */, pub feather: f64, pub choke: f64, pub invert: bool }
pub struct AudioParams { pub gain: f64 /* dB */, pub pan: f64, pub fade_in: Rational, pub fade_out: Rational }
pub struct Keyframe { pub at: Rational, pub value: ParamValue, pub interp: Interp }
#[serde(tag = "type")] pub enum Interp { Hold, Linear, Bezier { x1: f64, y1: f64, x2: f64, y2: f64 } }
#[serde(untagged)] pub enum ParamValue { Bool(bool), Number(f64), Vec2([f64; 2]), Text(String) }
pub struct CaptionBlock { pub words: Vec<Word>, pub style: String, pub overrides: BTreeMap<String, ParamValue>,
    pub source: Option<CaptionSource> }
pub struct Word { pub text: String, pub start: Rational, pub end: Rational } // item-relative
pub struct CaptionSource { pub artifact: String, pub word_range: [u32; 2] }
pub struct Bin { pub id: String, pub name: String, pub parent: Option<String> }
pub enum AssetKind { Media, Image, Svg, LottieTemplate, Font, Lut }
pub struct Asset { pub id: String, pub kind: AssetKind, pub path: String, pub fingerprint: String,
    pub bin: Option<String>, pub duration: Option<Rational>, pub video: Option<VideoInfo>, pub audio: Option<AudioInfo>,
    pub image: Option<ImageInfo>, pub lottie: Option<LottieInfo>, pub font: Option<Vec<FontFace>>, pub lut: Option<LutInfo> }
// ImageInfo {width, height}; LottieInfo {width, height, rate, duration, slots: Vec<SlotSpec{id, kind}>, fonts: Vec<String>};
// FontFace {family, weight, italic, face}; LutInfo {size}
pub fn new_project(id: String, name: String, rate: Rational) -> Result<Project, CoreError>; // Decision 9
// fin_core::ids: IdKind gains Caption, Graphic, Image, Adjustment, Nested, Marker, Transition, Bin,
// Effect, Link, Style; ID_KINDS lists all; fin-store `counters::kind_name` names them.
```

- [ ] **Step 1: Write failing tests:**
  - `upcast_v1_snapshot_golden` (frozen `snapshot_v1.json` → `snapshot_v2.json`, compared as decoded `Project` and as re-encoded JSON);
  - `new_project_equals_upcast_genesis` (rates 30, 30000/1001, 25, 60; resolution 1080×1920);
  - `item_json_round_trips_every_kind`, `defaults_are_omitted` (a rev-0 project encodes to the golden bytes);
  - `param_values_round_trip_through_record_bytes` (B2; a prop, 256 cases: random finite `f64`s, subnormals and `±0.0` included, in `ParamValue`s inside ops → `encode_record` → `decode_record` → bit-identical);
  - `ids_after_counts_every_kind`, `new_id_prefixes_are_unambiguous`;
  - every existing `ops_*`, `rebase*`, `undo`, `merge`, `pick` and props test passes on the v2 model;
  - `sp2_4a_4b_fixtures_open_equal_v2_goldens` (fin-store: the states equal `expected_v2.json`, and `upcast_snapshot(expected.project)` equals it too).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test upgrade_golden --test model --test record`. Expected: FAIL.
- [ ] **Step 3: Implement.** The snapshot step is written on `Value` with literal names:
  - it reproduces `new_project`'s defaults (track names by kind order, `"Timeline 1"`, 1080×1920);
  - sets `kind: "media"` on assets;
  - moves `timeline` into `timelines[0]` with the project rate.

  Tests and `fin` send version 2; client v1 txs come in Task 6 through `normalize`.
- [ ] **Step 4: Golden review.** Generate `expected_v2.json` for the three fixtures, read each against `expected.json` by hand (ids, times, assets equal; only shape and defaults differ), and commit them.
- [ ] **Step 5: Run** the three gates and `rtk pnpm e2e` (scripts read `timelines[0]`) → PASS. **Commit** `feat(core): v2 model with timelines, item kinds and common fields; snapshot upcaster`.

### Task 6: `item.*` ops replace `clip.*`; aliases; record upcaster; asset kinds; replay track check — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{item_insert.rs, item_delete.rs, item_trim.rs, item_remove.rs, item_restore.rs, alias.rs}`, `crates/fin-core/src/ops/util/{content.rs (Decision 13), lock.rs (TRACK_LOCKED), touched.rs (Decision 25)}`.
- Delete: `ops/{clip_insert, clip_trim, clip_delete, clip_remove, clip_restore}.rs` (their logic moves into the `item_*` modules).
- Modify:
  - `ops/asset_add.rs` (kinds), `ops/defs.rs`, `upgrade/{record_v1.rs, diff_v1.rs}` (`clip` → `item` in the diff step);
  - `apply.rs` (aliases only through `normalize`; the replay track check over `touched` tracks), `tx.rs` (`normalize`);
  - `history/*` and `rebase/*` where `Op::ClipRestore`/`ClipRemove` are named;
  - `crates/fin-store/src/{commit.rs (normalize), recover/remap.rs, history_ops/stack_rows_tests.rs, write_tests.rs, branch/lineage_tests.rs}`;
  - `crates/fin-store-xhost/src/{history.rs, ops.rs}`;
  - `crates/fin/src/commands/clip.rs` (builds `item.*`; linked audio comes in Task 22), `crates/fin/src/output.rs`.
- Test: `crates/fin-core/tests/{ops_item.rs (new; replaces ops_insert.rs and ops_trim_delete.rs), upgrade_golden.rs, normalize.rs (new)}`, `crates/fin-store/tests/{fixture_4b.rs, records.rs, recover.rs}`, `crates/fin/src/commands/tests.rs`.

**Interfaces** (new ops are newtype variants, `Op::ItemInsert(ItemInsert)`, wire tag `"item.insert"`):
```rust
pub enum InsertMode { Ripple, Overwrite }   pub enum DeleteMode { Ripple, Lift }
pub enum TrimMode { Ripple, Roll, Overwrite, Strict /* internal */ }
pub struct ItemInsert { pub id: String /* $ */, pub track: String, pub at: Rational, pub duration: Rational,
    pub mode: InsertMode, #[serde(flatten)] pub body: ItemBody }
pub struct ItemDelete { pub item: String, pub mode: DeleteMode }
pub struct ItemTrim { pub item: String, pub edge: Edge /* start|end */, pub delta: Rational, pub mode: TrimMode }
pub struct ItemRemove { pub id: String, pub mode: DeleteMode }                    // internal
pub struct ItemRestore { pub item: Item, pub track: String, pub mode: DeleteMode } // internal; $ id allocates
pub struct AssetAdd { pub id: String, pub kind: AssetKind, pub path: String, pub fingerprint: String,
    pub duration: Option<Rational>, pub video: Option<VideoInfo>, pub audio: Option<AudioInfo>, pub image: Option<ImageInfo>,
    pub lottie: Option<LottieInfo>, pub font: Option<Vec<FontFace>>, pub lut: Option<LutInfo>, pub bin: Option<String> }
// ops::alias: `Op::ClipInsert {..}`, `ClipTrim {..}`, `ClipDelete {..}` stay (struct variants, client only)
pub fn normalize(tx: Tx) -> Result<Tx, CoreError>; // Decision 6; re-exported from fin_core
// ops::util::touched
pub(crate) fn touched_tracks(project: &Project, own: &Diff) -> Vec<usize>; // Decision 25
```
Insert validation per kind (policy unless noted):
- `clip` on a video or audio track needs a `media` asset with that stream (`TRACK_KIND_MISMATCH`, structural as today);
- `image` (`image`/`svg` assets), `graphic` (`lottieTemplate`; slots checked in Task 10) and `adjustment` go on video tracks (Decision 41);
- `caption` goes on caption tracks (word rules in Task 18);
- `nested` is `INVALID_OP` with `fix` "nested items arrive in sub-project 6".

`item.trim` on the start edge records `by` and rebases keyframes and words (Decision 13). `strict` is `INVALID_OP` from clients. Every op on a locked track's items is `TRACK_LOCKED`.

- [ ] **Step 1: Write failing tests:**
  - goldens for `item.insert` (each allowed kind), `item.delete` (`ripple`, `lift`), `item.trim` (`ripple` on both edges, `strict`, speed ≠ 1), and the internal remove and restore. Each checks apply, inverse, shifts and `inverse_shifts_mirror`, and `ids_cover_every_id_in_json` for `item.restore` of an item with effects, a link, keyframes and a style;
  - `start_trim_rebases_keyframes_and_words` (exact inverse, `by` recorded); `strict_trim_refuses_overlap_in_every_mode`;
  - `replay_checks_tracks_touched_by_changes` (B1: a hand-made `lift` record whose restore overlaps is `REPLAY_FAILED` `CLIP_OVERLAP` though it records no shift);
  - `asset_add_validates_each_kind`;
  - `upcast_v1_record_golden_per_op` (each of the 7 v1 op shapes in `intent`, `resolved` and `inverse`; `clip.restore` without `ripple` → `ripple`), `upcast_v1_record_golden_whole`;
  - `normalize_maps_aliases`, `normalize_adds_media_kind_to_v1_asset_add`, `alias_outside_client_mode_is_invalid_op`, `v1_client_tx_is_accepted_and_upgraded`;
  - `track_locked_refuses_item_ops` (policy; replay applies);
  - fin-store:
    - `undo_of_4b_record_after_upgrade`, `merge_of_4b_branch_after_upgrade`, `cherry_pick_of_4b_record_after_upgrade` (4B fixture);
    - `replay_of_upcast_records_equals_v2_golden` (all three fixtures, also through a rebuild from the folder alone);
    - `diff_upcast_equals_replay_on_fixtures` again with `item`;
    - `counters_see_ids_inside_restores`, `recovery_remaps_nested_ids` (B6);
  - `fin`: `args_hash_of_4b_inputs_is_unchanged` (the pinned hashes from Task 2, Decision 7).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_item --test upgrade_golden --test normalize`. Expected: FAIL.
- [ ] **Step 3: Implement.** The record step maps:
  - `clip.insert {id, track, at, asset, sourceIn, duration}` → `item.insert {id, track, at, duration, mode: "ripple", kind: "clip", asset, sourceIn}`;
  - `clip.trim {clip, edge, delta}` → `item.trim {item, edge, delta, mode: "ripple"}`;
  - `clip.delete {clip}` → `item.delete {item, mode: "ripple"}`;
  - `clip.remove {id, ripple}` → `item.remove {id, mode}`;
  - `clip.restore {clip, track, ripple = true}` → `item.restore {item: {id, start, duration, kind: "clip", asset, sourceIn}, track, mode}`;
  - `asset.add` + `kind: "media"`.

  It never touches ids, timestamps or bindings.
- [ ] **Step 4: Run** the three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(core): item.* ops with modes; clip.* as client aliases; record upcaster v1→v2`.

### Task 7: Overwrite, roll, slip, split; content offsets in the rebase engine; `compose::overwrite` — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/util/carve.rs`, `crates/fin-core/src/ops/{item_slip.rs, item_split.rs}`, `crates/fin-core/src/compose/{mod.rs, overwrite.rs}`.
- Modify: `ops/item_insert.rs` (overwrite), `ops/item_trim.rs` (`roll`, `overwrite`), `ops/defs.rs` (`offsets_mut` in the op table), `crates/fin-core/src/rebase/{map.rs (content maps), engine.rs (map offsets), steps.rs}`.
- Test: `crates/fin-core/tests/{ops_modes.rs, rebase_modes.rs, content_map.rs}`.

**Interfaces:**
```rust
pub struct ItemSlip { pub item: String, pub delta: Rational }                        // clip, nested
pub struct ItemSplit { pub item: String, pub offset: Rational, pub id: String /* $ right part */ }
// ops::util::carve
/// Items fully inside [s, e) are removed (inverse: item.restore lift); one overlapping s is trimmed at
/// its end, one overlapping e at its start (`Updated` with `edge`, and `by` on a start edge); one
/// strictly containing [s, e) is OVERWRITE_SPLITS_ITEM. Returns the inverse ops (strict trims back,
/// then restores).
pub(crate) fn carve(track: &mut Track, s: Rational, e: Rational, d: &mut OpDiff<'_>) -> Result<Vec<Op>, CoreError>;
// rebase::map
impl StepMap { pub fn content(&self) -> &[(String, Rational)]; } // from the diff's `by` changes; invert negates
impl Mapping { pub fn map_content(&self, start: usize, item: &str, v: Rational) -> Result<Rational, Unmapped>; } // Overflow only
// Op
pub(crate) fn offsets_mut(&mut self) -> Vec<(String /* item */, &mut Rational)>;
// compose
/// Ops for an overwrite of `[at, at + duration)` on `track`: `item.split` of the one item strictly
/// containing it (with `$split`), then the overwrite insert. Echo data says what it sent.
pub fn overwrite(project: &Project, track: &str, at: Rational, duration: Rational, body: ItemBody, id: &str)
    -> Result<Composed, CoreError>;
pub struct Composed { pub ops: Vec<Op>, pub echo: serde_json::Value }
```
Rules:
- **Overwrite** insert and trim record no shifts.
- **Roll:** moves the cut between the item and its touching neighbour (`NOT_ADJACENT` otherwise). On a start edge, the neighbour's `sourceIn` moves by `delta × speed`, and the start-edge item records `by`.
- **Slip:** moves `sourceIn` only; the source range is policy.
- **Split:**
  - `offset` lies strictly inside `(0, duration)`, at least one frame from each end (policy);
  - the right part copies `body` and `common` (effects keep their ids; keyframes and words are rebased by `−offset`) and gets no `link`;
  - a caption is `INVALID_OP` with `fix: "use caption.split"`;
  - the inverse is internal: `item.remove` of the right part (lift), then `item.trim` of the left part (`strict`, +right length).

- [ ] **Step 1: Write failing tests:**
  - goldens for every mode (with `inverse_shifts_mirror`);
  - `overwrite_removes_covered_and_trims_partial`, `overwrite_inside_one_item_is_refused`, `compose_overwrite_splits_first`;
  - `roll_requires_adjacent_neighbour`, `roll_moves_both_edges_and_source`;
  - `slip_keeps_timeline_positions`, `slip_source_range_is_policy` (replay applies a slip past the asset), `split_copies_effects_keyframes_rebased`, `split_on_caption_is_invalid_op_with_fix`;
  - B4: `undo_of_overwrite_conflicts_with_later_insert` (an insert into the vacated range, then undo: `CLIP_OVERLAP` conflict, nothing carved), `undo_of_split_after_neighbour_extend_conflicts`;
  - B3: `stale_split_after_start_trim_maps_offset`, `content_map_inverse_negates`, `mirrored_trim_and_undo_cancel`;
  - rebase: `overwrite_vs_trim_of_overwritten_item_conflicts` (`SAME_EDGE` from the diff's edge change), `overwrite_vs_delete_of_covered_item` (not a conflict), `roll_vs_neighbour_start_trim_same_edge`, `stale_overwrite_maps_over_ripple_insert`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_modes --test rebase_modes --test content_map`. Expected: FAIL.
- [ ] **Step 3: Implement** (each module < 250 lines).
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): overwrite, roll, slip and split; content offsets in rebase`.

### Task 8: Moves, slides, link groups; `compose::clip_with_audio`, `compose::split_linked` — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{item_move.rs, item_slide.rs, item_link.rs, item_unlink.rs}`, `crates/fin-core/src/ops/util/group.rs`, `crates/fin-core/src/compose/{audio.rs, split.rs}`.
- Modify: `ops/{item_delete.rs, item_trim.rs}` (group behaviour, Decision 19).
- Test: `crates/fin-core/tests/{ops_move_link.rs, rebase_moves.rs, compose.rs}`.

**Interfaces:**
```rust
pub struct ItemMove { pub item: String, pub track: String, pub at: Rational, pub mode: InsertMode }
pub struct ItemSlide { pub item: String, pub delta: Rational }
pub struct ItemLink { pub items: Vec<String>, pub id: String /* $lk */ }
pub struct ItemUnlink { pub group: String }
pub(crate) fn members(project: &Project, item: &str) -> Result<Vec<(usize, usize, usize)>, CoreError>; // ops::util::group
// compose (Decision 21; Decision 17)
pub fn clip_with_audio(project: &Project, track: &str, at: Rational, duration: Rational, asset: &str,
    source_in: Rational, speed: Rational, mode: InsertMode, audio_track: Option<&str>) -> Result<Composed, CoreError>;
pub fn split_linked(project: &Project, item: &str, at: Rational) -> Result<Composed, CoreError>;
```
Rules:
- **Move:**
  - `at` is in the pre-op frame (Decision 15). Ripple records, per member, a close on its source track (`{end, −dur}`), then an open on its target (`{at', +dur}`, where `at'` is `at` mapped through the close);
  - the inverse records them mirrored in reverse (Decision 23);
  - a same-track ripple move whose `at` lies inside the item is `MOVE_INTO_ITSELF`;
  - overwrite carves the target range for each member;
  - the target track kind must fit the item kind; only the named item changes track.
- **Slide:** moves the item by `delta`, trimming the touching neighbour on each side (start-edge `by` on the next item). A side without a touching neighbour needs room in the gap (`SLIDE_BLOCKED`). It never ripples.
- **Link:** all items on one timeline, none already linked (`ALREADY_LINKED`), at least 2. **Unlink** clears `link` on every member; its inverse is internal `item.link` with the literal id.

- [ ] **Step 1: Write failing tests:**
  - goldens for both move modes, across and within tracks (`inverse_shifts_mirror`); `move_into_itself_is_refused`; `slide_trims_both_neighbours`, `slide_blocked_by_gap`;
  - `link_unlink_inverse_exact`;
  - `linked_move_moves_members_same_delta`, `linked_trim_is_all_or_nothing`, `linked_delete_ripples_each_track`, `linked_delete_footprint_covers_members`;
  - compose: `clip_with_audio_links_video_and_audio`, `clip_with_audio_needs_an_audio_track`, `split_linked_splits_members_and_links_rights`;
  - rebase: `move_vs_concurrent_trim_composes`, `move_vs_delete_removed`, `move_to_other_track_meets_its_edits`, `stale_move_at_maps_over_target_track_insert`, `linked_delete_vs_member_trim_is_not_a_conflict`, `slide_vs_slide_same_param`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_move_link --test rebase_moves --test compose`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): item.move, item.slide, link groups; linked-audio and linked-split builders`.

### Task 9: Transitions; `compose::transition_with_links` — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{transition_set.rs, transition_remove.rs}`, `crates/fin-core/src/ops/util/prune.rs`, `crates/fin-core/src/compose/transition.rs`.
- Modify: `crates/fin-core/src/ops/mod.rs` (the post-op prune pass in `apply`), `ops/util/check.rs` (`TRANSITION_INVALID`).
- Test: `crates/fin-core/tests/ops_transitions.rs`, `crates/fin-core/tests/compose.rs`.

**Interfaces:**
```rust
pub struct TransitionSet { pub id: String /* $ creates; literal updates (or restores, Internal) */,
    pub track: String, pub between: [String; 2], pub kind: TransitionKind, pub duration: Rational,
    #[serde(default)] pub curve: Curve }
pub struct TransitionRemove { pub transition: String }
/// After any op but transition.*: removes every transition on the tracks the op touched (Decision 25)
/// whose items no longer touch. Records `Removed` and returns `transition.set` restores, appended
/// after the op's own inverse.
pub(crate) fn prune(project: &mut Project, d: &mut OpDiff<'_>) -> Result<Vec<Op>, CoreError>;
pub fn transition_with_links(project: &Project, a: &str, b: &str, kind: TransitionKind, duration: Rational,
    curve: Curve, linked: bool) -> Result<Composed, CoreError>; // Decision 22
```
Rules (policy unless noted):
- `between` must touch (`NOT_ADJACENT`; also structural);
- `duration` is ≥ one frame and ≤ the shorter item;
- clip handles follow Decision 22;
- a `$` id at a cut that already has a transition is `TRANSITION_EXISTS`;
- the footprint touches `transition.out` on A and `transition.in` on B.

- [ ] **Step 1: Write failing tests:**
  - goldens for set, update and remove;
  - `ripple_delete_of_b_prunes_and_undo_restores`, `pruned_transition_restored_by_undo`, `trim_separating_items_prunes`, `move_prunes`, `overwrite_prunes`, `lift_prunes`;
  - `ripple_elsewhere_keeps_transition`;
  - `handles_are_policy` (replay applies), `replay_rejects_non_adjacent_transition`;
  - `two_sets_at_one_cut_conflict_same_param`;
  - compose: `transition_with_links_crossfades_audio_partners`, `transition_with_links_skips_partners_that_do_not_touch`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_transitions --test compose`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(core): transitions at cuts, pruned by a post-op pass; linked-partner builder`. **Batch A is done.**

### Task 10: The settings registry and `param.set` — tier: opus

**Files:**
- Create: `crates/fin-core/src/params/{mod.rs (ParamSpec, ParamType, find, validate), item.rs, effects.rs, scopes.rs (track, timeline, marker, style), access.rs (get/set on the typed model)}`, `crates/fin-core/src/ops/param_set.rs`.
- Modify: `ops/item_insert.rs` (graphic slots checked against `LottieInfo.slots`: `SLOT_UNKNOWN`, `PARAM_TYPE`).
- Test: `crates/fin-core/tests/params.rs`.

**Interfaces:**
```rust
#[derive(Debug, Serialize, JsonSchema)]
pub struct ParamSpec { pub path: &'static str, pub scope: Scope, pub ty: ParamType, pub animatable: bool,
    pub default: DefaultValue, pub unit: Option<&'static str>, /** `{min}`, `{max}`, `{unit}` filled */ pub fix: &'static str }
pub enum Scope { Item { kinds: &'static [&'static str] }, Effect { kind: &'static str }, Track, Timeline, Marker, Style }
pub enum ParamType { Number { min: f64, max: f64 }, Bool, Enum { values: &'static [&'static str] }, Color,
    Vec2 { min: [f64; 2], max: [f64; 2] }, Time { min: Option<&'static str> }, Text { max_len: u32 },
    AssetRef { kinds: &'static [&'static str] } }
pub fn specs() -> &'static [ParamSpec];
pub fn find(scope: &ScopeOf<'_>, path: &str) -> Result<&'static ParamSpec, CoreError>; // PARAM_UNKNOWN + alternatives
pub fn validate(spec: &ParamSpec, value: &ParamValue) -> Result<(), CoreError>; // PARAM_TYPE / INVALID_RANGE
pub fn get(item: &Item, path: &str) -> Result<ParamValue, CoreError>;
pub fn set(item: &mut Item, path: &str, value: ParamValue) -> Result<ParamValue /* old */, CoreError>;
pub struct ParamSet { pub item: String, pub path: String, pub value: ParamValue } // op `param.set`
```
Paths:
- **Items:**
  - `transform.position|scale|rotation|anchor`, `crop.left|right|top|bottom`, `opacity`, `blend`;
  - `audio.gain|pan|fadeIn|fadeOut`;
  - `mask.feather|choke|invert` (`PARAM_UNKNOWN` on an item without a mask, until sub-project 6 adds mask edits);
  - `speed` (clip, not animatable; the source extent is policy);
  - `slots.<id>` (graphic), `effects.<fx>.<param>`.
- **Effects:** `colorAdjust.{exposure −4..4 EV, contrast −1..1, saturation 0..2, temperature −1..1, tint −1..1}`, `lut.{asset (lut), intensity 0..1}`, `blur.radius 0..200 px`.
- **Tracks:** `name, mute, solo, lock, gain`. **Timelines:** `name, resolution, rate` (rate only while it has no items: `TIMELINE_NOT_EMPTY`). **Markers:** `label, color, at, duration`.

`param.set`:
- touches `Param(path)`;
- a path with keyframes is `PARAM_ANIMATED`;
- an unknown typed path is structural (`REPLAY_FAILED` on replay), and a range is policy.

- [ ] **Step 1: Write failing tests:**
  - `every_spec_path_gets_and_sets`, `defaults_match_model_defaults`;
  - `range_errors_carry_details_and_fix_text` (exact strings, spec §8 risk);
  - `param_set_inverse_exact`, `param_set_on_animated_path_refused`;
  - `same_param_conflicts_disjoint_params_merge`;
  - `graphic_slots_validated_against_template`;
  - `specs_serialise_deterministically`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test params`. Expected: FAIL.
- [ ] **Step 3: Implement.** `access.rs` is one `match` per path group; files < 400 lines.
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): settings registry and param.set`.

### Task 11: Tracks and timelines — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{track_add, track_remove, track_restore, track_move, track_set, timeline_add, timeline_set, timeline_remove, timeline_restore}.rs`.
- Test: `crates/fin-core/tests/ops_tracks.rs`, `crates/fin-store/tests/recover.rs` (an addition).

**Interfaces:**
```rust
pub struct TrackAdd { pub id: String, pub timeline: String, pub kind: TrackKind, pub index: Option<u32>, pub name: Option<String> }
pub struct TrackRemove { pub track: String, #[serde(default)] pub with_items: bool }
pub struct TrackRestore { pub track: Track, pub timeline: String, pub index: u32 } // internal
pub struct TrackMove { pub track: String, pub index: u32 }
pub struct TrackSet { pub track: String, pub path: String, pub value: ParamValue }
pub struct TimelineAdd { pub id: String, pub name: String, pub rate: Rational, pub resolution: Resolution }
pub struct TimelineSet { pub timeline: String, pub path: String, pub value: ParamValue }
pub struct TimelineRemove { pub timeline: String }
pub struct TimelineRestore { pub timeline: Timeline, pub index: u32 }             // internal
```
Rules:
- `track.remove` without `with_items` on a non-empty track is `TRACK_NOT_EMPTY`. With items, it removes them and their transitions; other members of their link groups keep `link`.
- `timeline.remove` is refused while a `nested` item names it (`TIMELINE_NESTED`, `details.items`) and for the last timeline (`INVALID_OP`).
- Footprints:
  - removals list every removed entity (diff `Removed`), and adds create;
  - `track.move` touches `Param("index")`; the `*_set` ops touch `Param(path)`;
  - none records shifts.

- [ ] **Step 1: Write failing tests:**
  - goldens for every op (`ids_cover_every_id_in_json` for both restores);
  - `track_remove_with_items_inverse_restores_items_transitions_links`, `timeline_remove_refused_while_nested` (a project built literally), `last_timeline_is_kept`;
  - `stale_edit_on_removed_track_conflicts_removed`;
  - fin-store: `recovery_remaps_ids_inside_track_restore` (B6).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_tracks`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): track and timeline ops`.

### Task 12: Markers, bins and assets — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{marker_add, marker_set, marker_remove, bin_add, bin_move, bin_remove, asset_move}.rs`.
- Modify: `ops/asset_remove.rs` (now a client op: `ASSET_IN_USE` with `details.items` over clips, graphics, images, and `lut` effect params; styles naming a font family join in Task 17).
- Test: `crates/fin-core/tests/ops_library.rs`.

**Interfaces:**
```rust
pub struct MarkerAdd { pub id: String, pub timeline: String, pub at: Rational, pub duration: Option<Rational>,
    pub label: String, pub color: Option<String> }
pub struct MarkerSet { pub marker: String, pub path: String, pub value: ParamValue }
pub struct MarkerRemove { pub marker: String }
pub struct BinAdd { pub id: String, pub name: String, pub parent: Option<String> }
pub struct BinMove { pub bin: String, pub parent: Option<String> }
pub struct BinRemove { pub bin: String }
pub struct AssetMove { pub asset: String, pub bin: Option<String> }
```
Rules:
- markers never ripple (Decision 26);
- `bin.remove` is refused while assets or bins are in it (`BIN_NOT_EMPTY`); `bin.move` into its own subtree is `BIN_CYCLE`;
- inverses are the add op with a literal id (`Internal`) or the inverse set/move;
- footprints: `marker.set` touches `Param(path)`, `bin.move` touches `Param("parent")`, `asset.move` touches `Param("bin")`.

- [ ] **Step 1: Write failing tests:** goldens; `marker_positions_never_map` (a ripple before a marker leaves it); `bin_cycle_refused`; `asset_remove_in_use_lists_items`; `asset_remove_then_undo_restores_kind_fields`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_library`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): marker, bin and asset ops`.

### Task 13: Effects and keyframes — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{effect_add, effect_remove, effect_restore, effect_move, keyframe_set, keyframe_remove}.rs`.
- Test: `crates/fin-core/tests/ops_effects_keyframes.rs`.

**Interfaces:**
```rust
pub struct EffectAdd { pub item: String, pub id: String /* $fx */, pub kind: String, pub at: Option<u32>,
    #[serde(default)] pub params: BTreeMap<String, ParamValue> }
pub struct EffectRemove { pub item: String, pub effect: String }
pub struct EffectRestore { pub item: String, pub effect: Effect, pub index: u32 } // internal
pub struct EffectMove { pub item: String, pub effect: String, pub to: u32 }
pub struct KeyframeSet { pub item: String, pub path: String, pub at: Rational, pub value: ParamValue, pub interp: Interp }
pub struct KeyframeRemove { pub item: String, pub path: String, pub at: Rational }
```
Rules (policy unless noted):
- the effect kind is a registry effect (`PARAM_UNKNOWN`);
- the path is animatable (`PARAM_NOT_ANIMATABLE`); the value follows the spec;
- bézier `x1`, `x2` lie in `[0, 1]`; `at` lies in `[0, duration]` (stored times outside are fine on replay, Decision 13);
- `keyframe.set` at an existing `at` replaces it, and its inverse restores the old keyframe (or removes it);
- `keyframe.*` `at` is an item offset (`offsets_mut`, Decision 16);
- footprints follow Decision 20.

- [ ] **Step 1: Write failing tests:**
  - goldens (`ids_cover_every_id_in_json` for `effect.restore`);
  - `effect_ids_come_from_the_global_counter_and_split_keeps_them`;
  - `keyframe_set_replaces_at_same_time`, `keyframe_on_static_path_refused`;
  - B3: `stale_keyframe_after_start_trim_keeps_identity` (the stale `keyframe.set` at `a` after a concurrent start trim by `d` replaces the keyframe now at `a − d`), `stale_keyframe_remove_after_start_trim`;
  - `concurrent_keyframes_at_different_times_merge`, `same_keyframe_conflicts_same_param`, `effect_move_vs_move_same_param`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_effects_keyframes`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): effects and keyframes`.

### Task 14: `eval_frame` — tier: opus

Eval covers every item kind, `nested` and masks included (built literally in tests, Decision 41).

**Files:**
- Create: `crates/fin-core/src/eval/{mod.rs, plan.rs, interp.rs, source.rs, frame.rs, transition.rs}`.
- Modify: `crates/fin-core/src/schema.rs` (`FramePlan`), `crates/fin-core/src/time.rs`.
- Test: `crates/fin-core/tests/{eval_frame.rs, eval_interp.rs}`.

**Interfaces:**
```rust
pub fn eval_frame(project: &Project, timeline: &str, t: Rational, m: Option<&dyn TextMeasurer>) -> Result<FramePlan, CoreError>;
pub struct FramePlan { pub timeline: String, pub t: Rational, pub resolution: Resolution, pub layers: Vec<Layer>,
    pub text_engine: Option<String> }
pub struct Layer { pub item: String, pub kind: ItemKind, pub track: String, pub local: Rational, pub source: Option<SourceRef>,
    pub transform: Transform, pub crop: Crop, pub opacity: f64, pub blend: BlendMode, pub effects: Vec<EffectPlan>,
    pub mask: Option<MaskPlan>, pub transition: Option<TransitionState>, pub caption: Option<CaptionPlan>,
    pub graphic: Option<GraphicPlan>, pub children: Option<Vec<Layer>> }
pub struct SourceRef { pub asset: String, pub time: Rational }
pub struct EffectPlan { pub id: String, pub kind: String, pub params: BTreeMap<String, PlanValue> }
pub enum PlanValue { Bool(bool), Number(f64), Vec2([f64; 2]), Color([f64; 4]), Text(String) } // Decision 12
pub struct TransitionState { pub id: String, pub kind: TransitionKind, pub curve: Curve, pub progress: f64, pub role: Role }
pub struct CaptionPlan { pub words: Vec<WordPlan>, pub style: String, pub layout: Option<TextLayout> } // layout: Task 19
pub struct GraphicPlan { pub asset: String, pub time: Rational, pub slots: BTreeMap<String, PlanValue> }
pub fn interpolate(kfs: &[Keyframe], at: Rational, base: &ParamValue, ty: &ParamType) -> PlanValue;
pub fn bezier_y(c: [f64; 4], u: f64) -> f64; // Decision 35
pub const PRESETS: &[(&str, [f64; 4])];   // ease, easeIn, easeOut, easeInOut (CSS)
```
Rules:
- An item is active for `start ≤ t < start + duration`. Within a transition `[cut − d/2, cut + d/2)`, A continues (its source time extends) and B starts early; `progress = (t − (cut − d/2)) / d`.
- Nested: the child time is `sourceIn + local`, recursing (a cycle is `NESTED_CYCLE`, depth ≤ 16).
- Hold, bool, enum and text values step; numbers, `vec2` and colours interpolate.
- Never NaN or ∞ (checked; `INTERNAL` if violated).

- [ ] **Step 1: Write failing tests:**
  - `bezier_matches_reference_table` (values computed offline at high precision, kept in the test), `presets_are_css_values`, `hold_linear_bezier_interp`, `colour_interpolates_per_component`, `interp_outside_keyframes_clamps`;
  - `speed_maps_source_time`, `transition_layers_and_progress`, `nested_source_time_and_group_layer`, `adjustment_layer_has_effects_only`, `mask_values_resolved`;
  - `muted_video_track_is_hidden`, `layer_order_is_track_order`;
  - `no_nan_in_plans` (prop, 128), `plan_json_is_stable`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test eval_frame --test eval_interp`. Expected: FAIL.
- [ ] **Step 3: Implement** within the float rules.
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): eval_frame with keyframes, transitions and nesting`.

### Task 15: `eval_audio` — tier: opus

**Files:**
- Create: `crates/fin-core/src/eval/{audio.rs, segments.rs}`.
- Test: `crates/fin-core/tests/eval_audio.rs`.

**Interfaces:**
```rust
pub fn eval_audio(project: &Project, timeline: &str, from: Rational, to: Rational) -> Result<AudioPlan, CoreError>;
pub struct AudioPlan { pub timeline: String, pub from: Rational, pub to: Rational, pub items: Vec<AudioItem> }
pub struct AudioItem { pub item: String, pub track: String, pub asset: String, pub from: Rational, pub to: Rational,
    pub source_from: Rational, pub speed: Rational, pub gain: Vec<GainSegment>, pub pan: Vec<PanSegment> }
pub struct GainSegment { pub from: Rational, pub to: Rational, pub db: [f64; 2], pub interp: Interp, pub shapes: Vec<Shape> }
pub struct Shape { pub kind: ShapeKind /* fadeIn|fadeOut|crossIn|crossOut */, pub curve: Curve, pub p: [f64; 2] }
pub struct PanSegment { pub from: Rational, pub to: Rational, pub pan: [f64; 2], pub interp: Interp }
```
Rules (Decision 34):
- `db` = track gain + item `audio.gain` (keyframed in dB);
- segments break at keyframes, fade ends and transition edges, clipped to `[from, to)`;
- mute omits; any solo keeps only soloed tracks;
- crossfades come from audio-track transitions.

- [ ] **Step 1: Write failing tests:** `segments_break_at_every_edge`, `gain_sums_track_and_item_db`, `fades_are_shapes`, `crossfade_shapes_with_curve`, `mute_and_solo`, `range_clipping`, `video_tracks_are_silent`, `no_nan` (prop, 64).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test eval_audio`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): eval_audio gain and pan plans`.

### Task 16: The determinism corpus, the float gate and the wasm32 runner — tier: opus

**Files:**
- Create: `crates/fin-core/tests/eval_corpus.rs`, `crates/fin-core/tests/eval_corpus/{projects/*.json, expected.json}`.
- Create: `scripts/check-eval-floats.mjs`, `scripts/check-eval-floats.test.mjs`. Modify: `scripts/gates.mjs`.
- Modify: `.github/workflows/ci.yml`, on ubuntu only:
  - `rustup target add wasm32-wasip1`;
  - wasmtime through a SHA-pinned `bytecodealliance/actions/wasmtime/setup`;
  - `CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime --dir ."`, then `cargo test -p fin-core --target wasm32-wasip1 --test eval_corpus` and `cargo test -p fin-text --target wasm32-wasip1 --test corpus`. `--dir` gives the tests the corpus files and the fonts.
- Modify: `crates/fin-core/Cargo.toml` (`blake3` dev-dependency).

**Corpus:** about 20 projects × 6–10 times each. They cover:
- bézier keyframes on every animatable number path, plus colour keyframes;
- transitions with both curves; speeds 1/2, 3/2 and 30000/1001;
- nested 2 deep, an adjustment layer, masks;
- captions with a fake measurer (real-font layouts are pinned by `fin-text` and e2e);
- audio plans over ranges that cut segments.

`expected.json` lists `{case, t | range, blake3}`; a mismatch prints the first differing plan.

**Gate:** `check-eval-floats.mjs` refuses, in `crates/fin-core/src/{eval,params,captions}/**` and `layout.rs`:
- method calls `\.(sin|cos|tan|asin|acos|atan2?|sinh|cosh|tanh|exp|exp2|exp_m1|ln|ln_1p|log|log2|log10|powf|powi|cbrt|hypot|mul_add)\(`;
- path calls `f(32|64)::(sin|cos|…|mul_add)\(`;
- any `libm::` and `f64::consts`.

Its test feeds it `x.powf(2.0)`, `f64::sin(x)` and `libm::exp(x)` and expects three failures.

- [ ] **Step 1: Write** the corpus test with an empty `expected.json` (it fails, listing the computed hashes), and the gate with its test.
- [ ] **Step 2:** Fill `expected.json` from a Linux run, and check by hand that 5 sampled plans make sense (layer order, progress values, source times).
- [ ] **Step 3: wasm32:**
  - run `rtk rustup target add wasm32-wasip1`, install wasmtime locally in `archive/tools/`, and run both corpora under it with `--dir .`;
  - if dev-dependencies (proptest) keep the target from building, gate the property code behind `cfg(not(target_os = "wasi"))`;
  - if wasm cannot run in CI, keep the native run and say so in TASKS.md and spec §5.4 (the spec allows it).
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `test(core): plan-hash determinism corpus, float gate, wasm32 runner`. **Batch B is done.**

### Task 17: Captions: blocks, SRT, alignment, styles and style ops — tier: opus

**Files:**
- Create: `crates/fin-core/src/captions/{mod.rs, blocks.rs, srt.rs, align.rs, presets.rs}`, `crates/fin-core/src/model/caption.rs` (`CaptionStyle`), `crates/fin-core/src/ops/{style_add, style_set, style_remove}.rs`.
- Modify: `params/scopes.rs` (style paths), `ops/asset_remove.rs` (a font family a style names is in use, Decision 28).
- Test: `crates/fin-core/tests/{captions.rs, ops_styles.rs}`.

**Interfaces:**
```rust
pub struct CaptionStyle { pub id: String, pub name: String, pub text: TextStyle, pub color: String,
    pub stroke: Option<Stroke>, pub background: Option<CaptionBox>, pub highlight: Option<String>,
    pub position: CaptionPosition /* {anchor: top|center|bottom, offset: f64} */, pub safe_zone: String,
    pub max_chars: u32, pub max_lines: u32, pub gap: Rational, pub max_duration: Option<Rational> }
pub struct SafeZone { pub name: &'static str, pub insets: [Rational; 4] /* left, top, right, bottom; fractions */ }
pub fn presets() -> &'static [CaptionStyle];   // "preset:<name>"
pub fn safe_zones() -> &'static [SafeZone];
pub fn style<'a>(project: &'a Project, reference: &str) -> Result<Cow<'a, CaptionStyle>, CoreError>;
pub fn blocks(words: &[Word], style: &CaptionStyle) -> Vec<BlockDraft /* {start, duration, words} */>;
pub fn parse_srt(text: &str) -> Result<Vec<Word>, CoreError>; // SRT_INVALID {line}; a cue's words share its span by chars
pub fn align_text(old: &[Word], text: &str) -> Vec<Word>;    // LCS keep; each new run takes the replaced span, by chars
pub struct StyleAdd { pub id: String, pub name: String, pub from: Option<String>, #[serde(default)] pub set: BTreeMap<String, ParamValue> }
pub struct StyleSet { pub style: String, pub path: String, pub value: ParamValue }
pub struct StyleRemove { pub style: String } // STYLE_IN_USE {items}
```
Safe-zone values come from the platforms' public creator guidelines; the module doc names each source and date.

- [ ] **Step 1: Write failing tests:**
  - `blocks_split_on_gap_chars_lines_duration`;
  - `srt_parses_and_distributes`, `srt_errors_name_the_line`;
  - `align_keeps_matching_times`, `align_distributes_by_chars`, `align_is_exact_rational`;
  - style op goldens (`ids_cover_every_id_in_json` for `style.add`); `style_remove_in_use_refused`; `font_named_by_a_style_is_in_use`;
  - `presets_and_zones_are_valid`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test captions --test ops_styles`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): caption blocks, SRT, text alignment, styles`.

### Task 18: Caption item edits — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{caption_set_text, caption_set_words, caption_split, caption_merge, caption_set_style}.rs`.
- Modify: `ops/item_insert.rs` (caption word rules).
- Test: `crates/fin-core/tests/ops_captions.rs`.

**Interfaces:**
```rust
pub struct CaptionSetText { pub item: String, pub text: String }
pub struct CaptionSetWords { pub item: String, pub words: Vec<Word> }   // internal (inverse)
pub struct CaptionSplit { pub item: String, pub word: u32, pub id: String /* $cap */ }
pub struct CaptionMerge { pub item: String, pub with: String }
pub struct CaptionSetStyle { pub item: String, pub style: String, #[serde(default)] pub overrides: Option<BTreeMap<String, ParamValue>> }
```
Rules (policy):
- words are non-empty and ordered without overlap, within `[0, duration]`;
- the style reference must exist (structural `NOT_FOUND`);
- `caption.merge` needs `with` to be the next caption item on the same track; the result spans to its end;
- the merge's inverse is `caption.setWords`, then `item.trim` (`strict`), then `item.restore` (lift);
- footprints follow Decision 20.

- [ ] **Step 1: Write failing tests:** goldens; `caption_merge_inverse_exact`; `set_text_vs_set_text_conflicts_same_param`; `split_after_set_text_reruns` (a policy failure is a conflict with that code); `caption_split_vs_start_trim` (word index unaffected; times rebased).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_captions`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): caption set-text, split, merge and style edits`.

### Task 19: Measurement in the edit path and in eval — tier: opus

**Files:**
- Modify (fin-core):
  - `apply.rs` (`ApplyEnv`, `apply_tx_env`; the overflow check in `finish`);
  - `rebase/engine.rs` (`Rebase.env`, `PreparedRebase.env`, `rebase_tx(.., env)`; the check after the last op);
  - `history/mod.rs` (`LogView::measurer`) and the planners (`undo`, `checkpoint`, `merge`, `pick`);
  - `captions/overflow.rs` (new), `eval/frame.rs` (layouts, `text_engine`);
  - `layout.rs` (`pub mod testing { FakeMeasurer, PanicMeasurer }` behind feature `test-measurer`).
- Modify (fin-store): `open.rs` (`OpenOptions.measurer`), `lib.rs` (`Store::set_measurer`), `commit.rs`, `history_ops/mod.rs` (`SqlLog::measurer`), `recover/rows.rs`.
- Modify: `crates/fin-core/tests/common/memlog.rs` (`MemLog::with_measurer`).
- Test: `crates/fin-core/tests/overflow.rs`, `crates/fin-store/tests/measure.rs`.

**Interfaces:**
```rust
#[derive(Clone, Copy, Default)]
pub struct ApplyEnv<'a> { pub measurer: Option<&'a dyn TextMeasurer> }
pub fn apply_tx_env(project: &Project, tx: &Tx, ids: &mut dyn IdAlloc, mode: ApplyMode, env: ApplyEnv<'_>)
    -> Result<Applied, CoreError>; // apply_tx / apply_internal / apply_tx_mode keep their signatures (env none)
pub fn rebase_tx(base: &Project, head: &Project, tx: &Tx, since: &[CommittedTx], ids: &mut dyn IdAlloc,
    env: ApplyEnv<'_>) -> Result<Rebased, CoreError>;
pub trait LogView { /* 4B methods */ fn measurer(&self) -> Option<&dyn TextMeasurer> { None } }
/// Once per tx (Decision 31): every caption item the tx changed, every user of a changed style, and
/// every caption of a timeline whose resolution changed, laid out in its safe zone. CAPTION_OVERFLOW
/// {item, lines, maxLines, width, maxWidth, missing, opIndex} with `fix` "shorten the text, split the
/// block (fin caption split), or reduce the style's size".
pub(crate) fn check_overflow(project: &Project, diff: &Diff, m: &dyn TextMeasurer) -> Result<(), CoreError>;
pub struct OpenOptions { /* 4A/4B fields */ pub measurer: Option<Arc<dyn TextMeasurer>> }
impl Store { pub fn set_measurer(&self, m: Option<Arc<dyn TextMeasurer>>); }
```

- [ ] **Step 1: Write failing tests:**
  - `replay_never_calls_the_measurer` (`PanicMeasurer` through `apply_committed`, import and rebuild);
  - `client_insert_overflow_refused` (code, details, fix text);
  - `overflow_checked_once_at_tx_end` (a tx that sets long text, then splits the block, passes);
  - `style_set_rechecks_every_user`, `resolution_change_rechecks_timeline`;
  - `stale_commit_rechecks_overflow`, `merge_rechecks_overflow` (a wider-glyph `FakeMeasurer` on the target: `CAPTION_OVERFLOW` conflict, spec §5.6), `undo_rechecks_overflow`, `recovery_rechecks_overflow`;
  - `no_measurer_skips_the_check`;
  - `eval_attaches_layouts_and_engine`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --features test-measurer --test overflow` and `rtk cargo test -p fin-store --test measure`. Expected: FAIL.
- [ ] **Step 3: Implement.** Thread `env` explicitly; there is no global state.
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core,store): caption overflow once per tx through an injected measurer`.

### Task 20: Probes for the new asset kinds — tier: sonnet

**Files:**
- Modify: `crates/fin-probe/src/lib.rs` (dispatch by magic bytes; `Probe` gains `kind` and the kind fields), `crates/fin-probe/Cargo.toml`.
- Create: `crates/fin-probe/src/{image.rs, svg.rs, lottie.rs, font.rs, lut.rs, wav.rs}`.
- Modify:
  - `crates/fin-probe/src/fixtures.rs` (`write_png`, `write_jpeg`, `write_webp`, `write_svg`, `write_dotlottie` (stored and deflated entries), `write_cube`, `write_wav`, `write_m4a`: header-valid bytes made in code, nothing committed);
  - `crates/fin-node/src/test_hooks.rs` (expose them for e2e), `crates/fin/src/backend.rs` (`Probed`), `crates/fin/src/commands/asset.rs` (`asset.add` per kind).
- Test: `crates/fin-probe/tests/probe_kinds.rs`.

**Interfaces:**
```rust
pub struct Probe { pub kind: AssetKind, pub duration: Option<Rational>, pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>, pub image: Option<ImageInfo>, pub lottie: Option<LottieInfo>,
    pub font: Option<Vec<FontFace>>, pub lut: Option<LutInfo>, pub fingerprint: String, pub size: u64 }
```
Kinds:
- **PNG, JPEG, WebP:** `imagesize` reads the header only.
- **SVG:** `roxmltree` reads the root's `width`/`height`/`viewBox` (sizes in px; anything else is `UNSUPPORTED_CONTAINER`). Sanitising is sub-project 6.
- **dotLottie** (`.lottie`, zip):
  - `manifest.json` plus `animations/<id>.json`; `w`, `h`, `fr`, `ip`/`op` → `LottieInfo`;
  - slots from `slots` (id plus kind: text, color, image, number);
  - font names from `fonts.list`;
  - malformed JSON or `chars` is `UNSUPPORTED_CONTAINER` with `details.reason` (arch §27.10).
- **Font:** `fin_text::font_info` (Decision 29), TTF/OTF/TTC.
- **`.cube`:** `LUT_3D_SIZE` 2–64 with exactly size³ rows.
- **WAV:** a RIFF/`WAVE` `fmt ` + `data` parser.
- **M4A/AAC:** the existing MP4 path with no video track → `media` with audio only.

- [ ] **Step 1:** Add the crates and run `rtk cargo deny check licenses` (expected: MIT, MIT OR Apache-2.0, Zlib).
- [ ] **Step 2: Write failing tests:** one per kind from the fixture writers; `truncated_inputs_are_unsupported_container`; `unknown_magic_is_unsupported`; `lottie_slots_and_fonts_listed`; `font_family_matches_the_measurer` (the probed family resolves in a `FontSet` with that asset); `probe_is_deterministic`.
- [ ] **Step 3: Implement.** **Run** `rtk cargo test -p fin-probe`, then the three gates → PASS.
- [ ] **Step 4: Commit** `feat(probe): image, SVG, dotLottie, font, LUT and audio-only probes`.

### Task 21: Fonts, the measurer and eval in `fin-node`; the eval RPCs — tier: sonnet

**Files:**
- Create: `crates/fin-node/src/fonts.rs`, `crates/fin-node/src/eval.rs`.
- Modify:
  - `crates/fin-node/src/{engine.rs, lib.rs}`, `crates/fin-node/Cargo.toml` (`fin-text`);
  - `packages/fin-node/` (ship `fonts/` beside the binary, the fetched files included; `index.js` exports);
  - `packages/contracts/src/rpc.ts` (`eval.frame`, `eval.audio`), `apps/finisherd/src/handlers.ts`;
  - `crates/fin-store/examples/export_schema.rs` (`FramePlan`, `AudioPlan`, `TextLayout`, `ParamSpec`), `crates/fin/tests/rpc_contract.rs` (fixtures), generated types.
- Test: `packages/fin-node/test/eval.test.ts`, `apps/finisherd/test/eval.test.ts`, `crates/fin-node/src/fonts.rs` unit tests.

**Interfaces:**
- **Font discovery:** `FIN_FONTS_DIR`, else `<dir of the fin-node binary>/fonts`, else (debug builds) `<repo>/assets/fonts` (+ `fetched/`).
- **Failure never blocks** (Decision 32). A bundle that does not verify → `Store::set_measurer(None)` and status warning `FONTS_DAMAGED {files}`; a missing font asset file → left out, warning `FONT_MISSING {asset, path}`.
- **The measurer** is `FontSet::bundled(..).with_project_fonts(<font assets, by id>)`, cached per engine by fingerprint. It is rebuilt after a commit or import whose diff adds or removes a `font` asset.
- **napi:** `evalFrame(projectId, branch, timeline?, at) -> FramePlan`, `evalAudio(projectId, branch, timeline?, from, to) -> AudioPlan` (AsyncTask).
- **RPC:**
  ```ts
  Rpc.make("eval.frame", { payload: EvalFramePayload /* {projectId, branch?, timeline?, at} */, success: FramePlanSchema, error: FinError })
  Rpc.make("eval.audio", { payload: EvalAudioPayload /* {projectId, branch?, timeline?, from, to} */, success: AudioPlanSchema, error: FinError })
  ```

- [ ] **Step 1: Write failing tests:** `fonts_dir_discovery_order`, `damaged_bundle_degrades_to_no_measurer_with_warning` (edits still commit; eval layouts are `null`), `measurer_rebuilt_after_font_asset_add`, `font_missing_warns`, `eval_frame_rpc_returns_layouts`, `eval_audio_rpc`, RPC fixtures replayed by `rpc_contract.rs`.
- [ ] **Step 2: Implement.** **Run** the three gates and `rtk pnpm e2e` → PASS. Record the `fin-node` package size before and after.
- [ ] **Step 3: Commit** `feat(node,rpc): bundled fonts, per-project measurer, eval.frame and eval.audio`. **Batch C is done.**

### Task 22: `fin` edit commands — structure — tier: sonnet

All multi-op behaviour comes from the `fin_core::compose` builders (Tasks 7–9). This task parses arguments, calls them, and echoes.

**Files:**
- Create: `crates/fin/src/commands/{item.rs, track.rs, timeline.rs, marker.rs, bin.rs, transition.rs}`.
- Modify: `crates/fin/src/commands/{clip.rs, asset.rs, builder.rs}`, `crates/fin/src/{registry.rs, map.rs, addr.rs, main.rs, output.rs}`.
- Test: `crates/fin/tests/cli_edit.rs` (new; offline snapshot hook plus a live server), `crates/fin/tests/cli_offline.rs` (map size).

**Commands** (every write takes `--dry-run`; global `--timeline <tl>`, default the first timeline):
- `fin clip insert --track --at --asset [--in] (--dur|--out) [--mode] [--no-audio] [--audio-track <t>]` (linked audio by default, Decision 21), `fin clip trim|delete` (now `item.*`);
- `fin image insert`, `fin graphic insert --template <a> [--slot id=value]…`;
- `fin item move <i> --to <time> [--track <t>] [--mode]`, `item split <i> --at <time>` (`compose::split_linked`), `item delete <i> [--mode]`, `item trim <i> (--start|--end) <delta> [--mode]`, `item slip <i> <delta>`, `item slide <i> <delta>`, `item link <i>…`, `item unlink <i|lk>`;
- `fin transition set --between <a> <b> --kind --dur [--curve] [--no-linked]` (`compose::transition_with_links`), `transition remove`;
- `fin track add|remove|move|set`, `fin timeline add|set|remove`, `fin marker add|set|remove`, `fin bin add|move|remove`, `fin asset move|remove`.

Behaviour:
- Mode defaults follow Decision 18, and every write echoes `resolved.mode` and what each builder sent.
- New optional arguments are omitted from the hashed input when absent (Decision 7).
- **SP2 follow-ups:**
  - trim echoes the resulting edge (`resolved.edgeAt`);
  - `--out < --in` says "`--out` must be after `--in`" with both values;
  - a negative `details.max` reads "sourceIn is at or past the asset's end" with `fix` naming the asset duration;
  - `project open --dry-run` is `USAGE` instead of being ignored.

- [ ] **Step 1: Write failing tests:** per command: the dry-run JSON, a live commit, and its main refusal with exit code and `fix`. Plus:
  - `clip_insert_links_audio_by_default`, `clip_insert_no_audio`, `clip_insert_without_audio_track_is_usage`;
  - `mode_default_and_echo`, `overwrite_inside_item_composes_split`, `transition_set_crossfades_linked_audio`;
  - `args_hash_of_4b_inputs_is_unchanged` (end to end);
  - `trim_echoes_resulting_edge`, `out_before_in_wording`, `negative_max_wording`, `project_open_dry_run_is_usage`;
  - `map_is_small_and_lists_every_group` (< 1.5k tokens).
- [ ] **Step 2: Implement.** **Run** `rtk cargo test -p fin` (`FIN_REQUIRE_SERVER=1`), the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(fin): structural edit commands with modes, linked audio and echoed values`.

### Task 23: `fin` edit commands — settings, captions, eval — tier: sonnet

**Files:**
- Create: `crates/fin/src/commands/{param.rs, keyframe.rs, effect.rs, caption.rs, style.rs, eval.rs}`, `crates/fin/src/values.rs` (parse a value by `ParamType`).
- Modify: `crates/fin/src/commands/schema.rs` (`fin schema param [--scope] [--path]`), `crates/fin/src/registry.rs`, `crates/fin/src/client.rs` (keep it ≤ 600 lines; eval calls go in `client_eval.rs`).
- Test: `crates/fin/tests/{cli_settings.rs, cli_captions.rs}`.

**Commands:**
- `fin param set <item> <path> <value>`;
- `fin keyframe set <item> <path> --at <time> <value> [--interp hold|linear|ease|easeIn|easeOut|easeInOut|bezier:x1,y1,x2,y2]`, `keyframe remove`;
- `fin effect add <item> <kind> [--at <n>] [--set p=v]…`, `effect remove|move`;
- `fin caption import --track <t> (--words <file.json>|--srt <file>) --style <s> [--at <time>]`, which expands through `captions::blocks` into one `item.insert` per block in one tx;
- `fin caption set-text|split|merge|style`, `fin style add|set|remove|ls`, `fin schema param`;
- `fin eval frame --at <time>`, `fin eval audio --range a..b` (server only).

Values:
- numbers take an optional unit (`-6dB`, `45deg`, `120px`, `50%` for fractions);
- `x,y` for `vec2`, `#rgb`/`#rrggbb`/`#rrggbbaa` for colours, enum names, times as in `addr`.

`--words` is `[{text, start, end}]` with times as rationals or decimals, converted exactly. Offline dry runs answer `measured: false`.

- [ ] **Step 1: Write failing tests:** per command as in Task 22, plus `values_parse_by_type`, `bad_value_errors_name_the_spec_range`, `caption_import_words_and_srt_make_blocks`, `caption_import_overflow_refused_online`, `schema_param_lists_the_registry`, `eval_needs_a_server`.
- [ ] **Step 2: Implement.** **Run** `rtk cargo test -p fin`, the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(fin): param, keyframe, effect, caption, style, schema param and eval commands`.

### Task 24: `fin view` and `fin status` — tier: sonnet

**Files:**
- Modify: `crates/fin/src/commands/view.rs`. Create `crates/fin/src/view/{mod.rs, lines.rs, budget.rs}`.
- Test: `crates/fin/tests/cli_view.rs` with a golden `crates/fin/tests/view_golden/{project.json, full.txt, budget_200.txt}`.

**Interfaces:**
- `fin view [--timeline] [--range a..b] [--budget <tokens>]` returns `{rev, timeline, lines: [String], elided: [{step, from?, to?, items?}], tokens}`. Example lines:
  - `00:04.0–00:07.5  V1  c3 clip a1 [2.0s+3.5s] x1.5 opacity 0.8 fx: blur  ⟷ lk1`
  - `00:05.0–00:06.2  C1  cap2 "Let's go" (preset:bold)`
  - `00:06.0  ── marker m1 "drop"`
- Degrades as Decision 39 says; deterministic for a given project and budget.
- `fin status` returns `{project {id, name}, branch, head, lease, readOnly, lastExport, unmergedBranches: [{id, name}], warnings}`. It merges the snapshot, `project.status` and `branch.list`; warnings include `FONTS_DAMAGED` and `FONT_MISSING`. Offline, it returns `{project, branch, head, server: false}`.

- [ ] **Step 1: Write failing tests:** `view_full_matches_golden`, `view_budget_degrades_in_order_and_says_so`, `view_range_and_timeline`, `view_is_deterministic`, `status_online_fields`, `status_offline`.
- [ ] **Step 2: Implement.** **Run** `rtk cargo test -p fin`, the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(fin): screenplay view with a token budget; fuller status`. **Batch D is done.**

### Task 25: Property suite and time budget — tier: opus

**Files:**
- Create:
  - `crates/fin-core/tests/props_ops_4c.rs` (properties 1 and 6 over every op);
  - `crates/fin-core/tests/props_rebase_4c.rs` (property 3);
  - `crates/fin-core/tests/props_history_4c.rs` (property 4 over 4C ops);
  - `crates/fin-core/tests/common/gen4c.rs` (generators).
- Modify: `crates/fin-store/tests/props_replay.rs` (property 5 draws 4C ops), `crates/fin-core/tests/common/memlog.rs`, `TASKS.md`.

**Properties** (spec §4.6 extended, §5.9):
1. `apply_then_inverse_is_exact_for_every_4c_op` (256).
   - Chains of 1–6 txs of 1–4 ops from every client and internal op (`strict` included), on projects with 2 timelines, linked groups, transitions, keyframes, effects and captions.
   - The inverse is exact; the inverse of the inverse gives the result again; **the inverse's shifts are the op's inverted in reverse order** (B5); every op passes `ids_cover_every_id_in_json` (B6).
   - A fixed-seed test checks that the generator reaches every op kind.
2. 4B's `rebase_over_empty_interval_is_identity`, run over the 4C generators (256).
3. `stale_and_fresh_agree_4c` (128).
   - An anchor model over 3 tracks (2 video, 1 audio), with 0–6 committed edits, each sometimes followed by its undo, drawn from: insert (both modes), delete (both), trim (all three, **start trims included**), move across tracks, **slide**, split, slip, link, transitions, **keyframe set and remove**, **effect add and move**, **caption split and merge**.
   - Our stale edit is drawn from the same set, so **a split or keyframe edit after a concurrent start trim** is common (B3).
   - When both succeed, the states are equal. A stale refusal where the fresh edit applies must be `SAME_EDGE`, `SAME_PARAM`, `REMOVED` or a policy code the head state explains.
4. `undo_redo_matches_reference_model_4c` (64). 4B's list-op model per track, extended with ripple moves between tracks and splits, on `main` and a side branch with merges and picks. The predicted reasons include `SAME_PARAM` for `param.set` on the same path.
5. `replay_and_export_import_roundtrip` (16, fin-store), with 4C ops and a `font` asset in the log. The follower gets the same `stateHash`.
6. `apply_tx_never_panics_and_success_keeps_invariants_4c` (512).
   - Any ids, times and parameter values in every mode, with **no-shift ops (overwrite, lift, roll, slip, slide, split, strict) given values past their policy limits in `Replay` mode** (B1).
   - Success keeps the structural invariants on every track (Decision 25) and, outside `Replay`, the policy ones.

- [ ] **Step 1: Write the files.** Then seed each bug locally, confirm it is caught, and revert it:
  - (a) `carve` leaves a 1-frame overlap → props 1 and 6;
  - (b) the prune pass skips the inverse restore → prop 1;
  - (c) `split_diff` swaps two op indexes → props 3 and 4;
  - (d) **a move's inverse records its shifts in forward order** (B5) → props 1, 3 and 4;
  - (e) content offsets not mapped (B3) → prop 3;
  - (f) the replay track check looks at shift tracks only (B1) → prop 6.
- [ ] **Step 2: Measure** every property binary:
  - `rtk proxy bash -c 'time cargo test -p fin-core --test props_ops_4c --test props_rebase_4c --test props_history_4c --test props_apply --test props_history --test rebase_props --test pick_props --test undo_to_props --test shift_map'`;
  - `rtk proxy bash -c 'time cargo test -p fin-store --test props_replay --test chain_props'`.

  The total must stay < 30 s in debug; record the numbers in TASKS.md.
- [ ] **Step 3:** Fix what the properties find, each fix with a unit regression test in the owning module's tests. Run the three gates → PASS. **Commit** `test(core): 4C property suite`.

### Task 26: e2e reel scenario — tier: sonnet

**Files:**
- Create: `scripts/e2e/reel.mjs`, `scripts/e2e/reel-golden/{view.json, frames.json, audio.json}`.
- Modify: `scripts/e2e.mjs` (`SCENARIOS.reel`), `scripts/e2e.test.mjs`, `scripts/e2e/fixture.mjs` (write the image, dotLottie and WAV fixtures through `testHooks`).

- [ ] **Step 1: Write the scenario** (spec §5.9), by `fin` only:
  1. Create a project (1080×1920 by default); `track add --kind audio` (A2, music) and `track add --kind caption`.
  2. `asset add` an MP4 (A/V), a PNG, a dotLottie template with a text slot, and a WAV.
  3. Clips:
     - V1: three clips (one at speed 3/2), each with its **linked audio on A1 by default**;
     - V2: an image overlay and a graphic with its slot set;
     - **A2: the music** (WAV) with a fade-in, `track set A2 gain -6dB`. Music and linked audio never share a track.
  4. `caption import --words words.json --style preset:bold` (about 40 words → several blocks; CJK words and one emoji included, so the fetched fonts are exercised).
  5. `keyframe set` opacity on the overlay (`0s` 0 → `1s` 1, `--interp easeOut`).
  6. `transition set` a dissolve between two V1 clips. The linked audio items get their crossfade (Decision 22), and the echo says so.
  7. Total length 30 s.
  8. `fin view` equals `view.json`.
  9. `fin eval frame` at 0, 0.5, 4.9 (inside the crossfade), 12, 20 and 29.9 s, compared as BLAKE3 hashes with `frames.json` (the file also keeps the full plans for review).
  10. `fin eval audio --range 0..30` equals `audio.json` (A1 crossfade shapes, A2 fade and dB).
  11. An overflowing `caption set-text` is refused `CAPTION_OVERFLOW` (exit 3). `item delete` of the second V1 clip prunes both transitions, and `undo` restores them.
- [ ] **Step 2: Run** `rtk pnpm fonts:fetch && rtk pnpm e2e`. Expected: every scenario passes (basic, storage, history, reel).
- [ ] **Step 3: Commit** `test(e2e): 30-second reel built by fin, checked by view and eval goldens`.

### Task 27: Spec and tracker — tier: sonnet

**Files:**
- `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`:
  - §5 as built: the decision table (1–41 plus any added during implementation), codes, RPCs, commands, the header state line, and "§5.10 Validation results (Task 28) — pending";
  - §3.1: DB v3 and m0003 (a version bump only); §3.2: schema version 2, the record, snapshot and diff upcasters, the aliases, per-op diffs, `float_roundtrip`; §3.4: the project-file `schemaVersion` rule (and whether 4B got the cherry-pick);
  - §4.3: per-op diff footprints, content offsets, the reverse shift order of inverses, the `SAME_PARAM` keys, replay's touched-track check;
  - §5.1: `fonts` derived; §5.3: `style.*` ops, `strict` trims, `caption.import` as a command; §5.4: the wasm32 status; §5.6: the fonts (CJK and emoji bundled, the large ones fetched); §6: the crate map as built.
  - **Deviations, said plainly:**
    - no `swash` in 4C;
    - `fin-text` was written fresh (the S11b spike code was unavailable);
    - `fin` neither links `fin-text` nor injects a measurer (offline dry runs are unmeasured);
    - no per-edit RPCs;
    - gains are dB ramps, converted by the mixer;
    - the binary and package sizes were measured by hand (Task 21, Task 28), not in CI;
    - nested and mask edits moved to sub-project 6.
- `docs/specs/2026-11-01-finisher-architecture-design.md`: a §27 item 15 pointer for 4C (model, registry, eval, `fin-text`, fonts, assets).
- `README.md`:
  - "Run it" gains a reel example (`fin clip insert`, `fin caption import`, `fin view`, `fin eval frame`) and `pnpm fonts:fetch`;
  - **"Upgrading from 4B"** states Decision 40.
- `TASKS.md`:
  - tick the 4C section;
  - tick the SP2 follow-up "fin output: trim echoes…";
  - add the follow-ups: drop the `clip.*` aliases at schema 3; nested and mask edits (sub-project 6); cache caption layouts between transactions; colour emoji painting (`swash`, sub-project 6); fonts in CI caches versus the repo (owner's call, Decision 30).

- [ ] **Step 1:** Write the edits. Check every decision against the code, as 4B Task 22 did, and note in the commit body any that changed.
- [ ] **Step 2: Run** `rtk pnpm gates` → ok. **Commit** `docs: record sub-project 4C as built`.

### Task 28: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

**Files:**
- Results go to `archive/sp4c-xhost/`; durable conclusions go into spec §5.10.
- Create: `crates/fin-store-xhost/src/edit4c.rs` (an `editor` subcommand committing a fixed 4C edit script; `node.rs` is already at 560 lines), `tests/storage-xhost/analyse-edit4c.mjs` and its test (following `analyse-history.mjs`).
- Modify: `crates/fin-store-xhost/src/main.rs`, `tests/storage-xhost/run.mjs` (row `edit4c`), `tests/storage-xhost/README.md` (no host details).

- [ ] **Step 1: Full suites** on macOS (arm64), Windows 11 (x64) and the Linux VM (x64), after `pnpm fonts:fetch`: `cargo test --workspace` (`FIN_REQUIRE_SERVER=1`, `FIN_REQUIRE_FONTS=1`), `pnpm test`, `pnpm gates`, `pnpm e2e`. Record the counts.
- [ ] **Step 2: Determinism.** On each OS:
  - the `fin-text` corpus and the eval corpus match the committed hashes, and the e2e reel goldens match;
  - on Linux, the wasm32-wasip1 runs match too.
  - **Any mismatch blocks the merge.** Bisect it to the first differing layout or plan, fix the source (a float function, a fallback path, a hash-map order, a font file), add a regression case, and re-run all three OSes.
- [ ] **Step 3: Upgrade rows.**
  - Copy a real 4B project (made on each OS by the 4B build) and open it with 4C: the state equals the 4B state passed through `upcast_snapshot`; `verify` is ok (`diffs_match` through the diff upcaster); an undo of a 4B record works.
  - Rebuild it from the folder alone: the same `stateHash`.
- [ ] **Step 4: Mixed-version row**, per pair (Mac↔Windows, Mac↔Linux, Windows↔Linux), on one NAS folder:
  - a 4B follower and a 4C holder: the first 4C record turns the 4B follower read-only `NEWER_FORMAT` (Decision 1), with no lost records and no crash;
  - a 4C-created project opened by 4B goes read-only (with the Task 3 cherry-pick) or refuses `SCHEMA_VERSION` (without it); record which;
  - a 4C follower follows to the same `stateHash` and `indexHash`.
- [ ] **Step 5: `edit4c` row** per NAS pair, 5 runs each: host A holds and runs the edit script; host B follows; after one follow cycle both report the same `stateHash` and the same eval plan hashes for the script's sample times.
- [ ] **Step 6: Packaging.** Each OS's `fin-node` package finds its `fonts/`, the fetched ones included. Record the sizes of `fin` (unchanged by `fin-text`) and of the `fin-node` package (about +52 MB of fonts; spec §8 risk).
- [ ] **Step 7:** Fix any defect, each with a regression test in the owning crate, and re-run the affected rows. Record the conclusions in spec §5.10 and TASKS.md. **Commit** `test(storage): cross-host 4C rows; record 4C validation`.
