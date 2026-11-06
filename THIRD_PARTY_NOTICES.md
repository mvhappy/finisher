# Third-party notices

This file records third-party code, content, and models used by Finisher. Each entry
is filled in with its licence text and copied paths when the material is actually used.

## T3 Code (MIT) — vendored slices
status: planned

## Uncut (MIT) — skills and critic rubrics
status: planned

## PySceneDetect (BSD-3-Clause) — ported detectors
status: planned

## TransNetV2 (MIT)
status: planned

## sentrysearch (Apache-2.0) — design reference
status: planned

## thorvg (MIT)
status: planned

## resvg (Apache-2.0 OR MIT)
status: planned

## whisper.cpp (MIT)
status: planned

## sherpa-onnx (Apache-2.0)
status: planned

## ONNX Runtime (MIT)
status: planned

## LanceDB (Apache-2.0)
status: planned

## dav1d (BSD-2-Clause)
status: planned

## SVT-AV1 (BSD-3-Clause-Clear)
status: planned

## libvpx (BSD-3-Clause)
status: planned

## OpenTimelineIO (Apache-2.0) — file format
status: planned

## Codex (Apache-2.0) — bundled agent CLI
status: planned

## cosmic-text (MIT OR Apache-2.0) and its shaping stack — `fin-text`
status: used (sub-project 4C), as Cargo dependencies; no code is copied.
- `cosmic-text` 0.19 (MIT OR Apache-2.0), built `no_std` without `swash`;
- `harfrust` 0.5 (MIT), `fontdb` 0.23 (MIT), `skrifa` 0.40 and `read-fonts` (MIT OR Apache-2.0),
  `ttf-parser` (MIT OR Apache-2.0);
- `unicode-bidi`, `unicode-script`, `unicode-segmentation` (MIT OR Apache-2.0),
  `unicode-linebreak` (Apache-2.0), `libm` and `core_maths` (MIT);
- **`self_cell`** 1.x is offered under Apache-2.0 OR GPL-2.0-only. **Finisher uses it under
  Apache-2.0 only.**

Fonts are never bundled; test fonts are generated at test time by the dev-only
`fin-testfonts` crate (`write-fonts`, MIT OR Apache-2.0).
