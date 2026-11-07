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

## ureq, rustls and ring — `fin-fonts` (HTTP and TLS for font fetching)
status: used (sub-project 4C), as Cargo dependencies; no code is copied.
- `ureq` 3.5 (MIT OR Apache-2.0), `ureq-proto`, `http`, `httparse` (MIT OR Apache-2.0);
- `rustls` 0.23 (Apache-2.0 OR ISC OR MIT) with the **`ring`** 0.17 provider (Apache-2.0 AND
  ISC; both terms apply), `rustls-webpki` (ISC), `untrusted` (ISC), `rustls-pki-types`
  (MIT OR Apache-2.0), `subtle` (BSD-3-Clause);
- `rustls-platform-verifier` 0.7 (MIT OR Apache-2.0) over the OS trust store:
  `rustls-native-certs` (Apache-2.0 OR ISC OR MIT) on Linux, `security-framework`
  (MIT OR Apache-2.0) on macOS; `winreg` (MIT) for the Windows system proxy;
- `sha2` (MIT OR Apache-2.0), `zip` 9 (MIT) with `flate2` (MIT OR Apache-2.0) and `zlib-rs`
  (Zlib).

Not used: `webpki-roots`/`webpki-root-certs` (CDLA-Permissive-2.0; only wasm32 and Android
would pull them, which `deny.toml` `[graph] targets` leaves out) and `aws-lc-rs`.

## imagesize and roxmltree — `fin-probe` (image and SVG sizes)
status: used (sub-project 4C), as Cargo dependencies; no code is copied.
- `imagesize` 0.15 (MIT): PNG, JPEG and WebP sizes from the header;
- `roxmltree` 0.21 (MIT OR Apache-2.0): the SVG root's size attributes.

dotLottie archives use the `zip` and `flate2` stack listed under `fin-fonts`.

## Catalog fonts (OFL-1.1) — fetched at run time, never distributed
status: used (sub-project 4C) by URL only: `crates/fin-fonts/catalog/catalog.json` names
upstream files by SHA-256 at pinned commits and release tags, and each machine downloads
them from there. Licence texts (checked by hand, 2026-11-06):
- Inter 4.1 (rsms/inter `docs/font-files/InterVariable.ttf`, `LICENSE.txt` at tag v4.1), SIL Open Font
  License 1.1;
- Noto Sans, Noto Sans Arabic, Noto Sans Hebrew (notofonts `latin-greek-cyrillic`, `arabic`,
  `hebrew`, `OFL.txt`), SIL Open Font License 1.1;
- Noto Sans CJK (notofonts/noto-cjk `Sans/LICENSE`), SIL Open Font License 1.1;
- Noto Color Emoji (googlefonts/noto-emoji `2D/fonts/LICENSE`), SIL Open Font License 1.1 (the
  repository's tools and images are Apache-2.0 and are not used).
