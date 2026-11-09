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

## dav1d (BSD-2-Clause) — `fin-dav1d-sys` (AV1 decode)
status: used (sub-project 6A), built from source: the git submodule `third_party/dav1d` at
tag 1.5.4 (commit 54706fc6bc0cdecab7e9593974a4039cc038fca7), the decoder library only (8- and
16-bit, no tools), linked statically through `fin-dav1d-sys` (whose `csrc/dav1d_shim.c` is
Finisher's own, MIT). AV1 itself is under the Alliance for Open Media Patent License 1.0 (the
SVT-AV1 section below has it in full). The compiled sources carry the licences below, in full;
`third_party/native.json` pins each text by BLAKE3.

### dav1d: BSD-2-Clause (`COPYING`)
The library's licence. Individual files add copyright lines under the same text: Two Orioles,
LLC; Martin Storsjo; Janne Grunau; VideoLabs; Niklas Haas; Matthias Dressel; Arm Limited;
The rav1e contributors; Nathan Egge.
```text
Copyright © 2018-2025, VideoLAN and dav1d authors
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### dav1d: ISC for x264's `x86inc.asm` (`src/ext/x86/x86inc.asm`, x86_64 builds)
```text
Copyright (C) 2005-2024 x264 project

Authors: Loren Merritt <lorenm@u.washington.edu>
         Henrik Gramner <henrik@gramner.com>
         Anton Mitrofanov <BugMaster@narod.ru>
         Fiona Glaser <fiona@x264.com>

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

## SVT-AV1 (BSD-3-Clause-Clear) — `fin-svt-sys` (AV1 encode)
status: used (sub-project 6A), built from source: the git submodule `third_party/svt-av1` at
tag v4.3.0 (commit e3d63405f9ba8219913a7276c2c47d10e5d022fe), the encoder library only, linked
statically through `fin-svt-sys` (whose `csrc/svt_shim.c` is Finisher's own, MIT). The
compiled sources carry the licences below, in full; `third_party/native.json` pins each text by
BLAKE3.

### SVT-AV1: the Clear BSD License (`LICENSE.md`)
```text
BSD 3-Clause Clear License
The Clear BSD License

Copyright (c) 2021, Alliance for Open Media

All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted (subject to the limitations in the disclaimer below)
provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in
   the documentation and/or other materials provided with the distribution.

3. Neither the name of the Alliance for Open Media nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

NO EXPRESS OR IMPLIED LICENSES TO ANY PARTY'S PATENT RIGHTS ARE GRANTED BY THIS LICENSE.
THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL
THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT
OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### SVT-AV1: Alliance for Open Media Patent License 1.0 (`PATENTS.md`)
The BSD-3-Clause-Clear text grants no patent rights; the AV1 patent licence is this one.

```text
**Alliance for Open Media Patent License 1.0**

 1. **License Terms.**

    **Patent License.** Subject to the terms and conditions of this License, each
     Licensor, on behalf of itself and successors in interest and assigns,
     grants Licensee a non-sublicensable, perpetual, worldwide, non-exclusive,
     no-charge, royalty-free, irrevocable (except as expressly stated in this
     License) patent license to its Necessary Claims to make, use, sell, offer
     for sale, import or distribute any Implementation.

     **Conditions.**

    *Availability.* As a condition to the grant of rights to Licensee to make,
       sell, offer for sale, import or distribute an Implementation under
       Section 1.1, Licensee must make its Necessary Claims available under
       this License, and must reproduce this License with any Implementation
       as follows:

          a. For distribution in source code, by including this License in the
          root directory of the source code with its Implementation.

          b. For distribution in any other form (including binary, object form,
          and/or hardware description code (e.g., HDL, RTL, Gate Level Netlist,
          GDSII, etc.)), by including this License in the documentation, legal
          notices, and/or other written materials provided with the
          Implementation.

    *Additional Conditions.* This license is directly from Licensor to
       Licensee.  Licensee acknowledges as a condition of benefiting from it
       that no rights from Licensor are received from suppliers, distributors,
       or otherwise in connection with this License.

    **Defensive Termination**. If any Licensee, its Affiliates, or its agents
     initiates patent litigation or files, maintains, or voluntarily
     participates in a lawsuit against another entity or any person asserting
     that any Implementation infringes Necessary Claims, any patent licenses
     granted under this License directly to the Licensee are immediately
     terminated as of the date of the initiation of action unless 1) that suit
     was in response to a corresponding suit regarding an Implementation first
     brought against an initiating entity, or 2) that suit was brought to
     enforce the terms of this License (including intervention in a third-party
     action by a Licensee).

    **Disclaimers.** The Reference Implementation and Specification are provided
     "AS IS" and without warranty. The entire risk as to implementing or
     otherwise using the Reference Implementation or Specification is assumed
     by the implementer and user. Licensor expressly disclaims any warranties
     (express, implied, or otherwise), including implied warranties of
     merchantability, non-infringement, fitness for a particular purpose, or
     title, related to the material. IN NO EVENT WILL LICENSOR BE LIABLE TO
     ANY OTHER PARTY FOR LOST PROFITS OR ANY FORM OF INDIRECT, SPECIAL,
     INCIDENTAL, OR CONSEQUENTIAL DAMAGES OF ANY CHARACTER FROM ANY CAUSES OF
     ACTION OF ANY KIND WITH RESPECT TO THIS LICENSE, WHETHER BASED ON BREACH
     OF CONTRACT, TORT (INCLUDING NEGLIGENCE), OR OTHERWISE, AND WHETHER OR
     NOT THE OTHER PARTRY HAS BEEN ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

2. **Definitions.**

     **Affiliate.**  "Affiliate" means an entity that directly or indirectly
     Controls, is Controlled by, or is under common Control of that party.

    **Control.** "Control" means direct or indirect control of more than 50% of
     the voting power to elect directors of that corporation, or for any other
     entity, the power to direct management of such entity.

    **Decoder.**  "Decoder" means any decoder that conforms fully with all
     non-optional portions of the Specification.

    **Encoder.**  "Encoder" means any encoder that produces a bitstream that can
     be decoded by a Decoder only to the extent it produces such a bitstream.

    **Final Deliverable.**  "Final Deliverable" means the final version of a
     deliverable approved by the Alliance for Open Media as a Final
     Deliverable.

    **Implementation.**  "Implementation" means any implementation, including the
     Reference Implementation, that is an Encoder and/or a Decoder. An
     Implementation also includes components of an Implementation only to the
     extent they are used as part of an Implementation.

    **License.** "License" means this license.

    **Licensee.** "Licensee" means any person or entity who exercises patent
     rights granted under this License.

    **Licensor.**  "Licensor" means (i) any Licensee that makes, sells, offers
     for sale, imports or distributes any Implementation, or (ii) a person
     or entity that has a licensing obligation to the Implementation as a
     result of its membership and/or participation in the Alliance for Open
     Media working group that developed the Specification.

    **Necessary Claims.**  "Necessary Claims" means all claims of patents or
      patent applications, (a) that currently or at any time in the future,
      are owned or controlled by the Licensor, and (b) (i) would be an
      Essential Claim as defined by the W3C Policy as of February 5, 2004
      (https://www.w3.org/Consortium/Patent-Policy-20040205/#def-essential)
      as if the Specification was a W3C Recommendation; or (ii) are infringed
      by the Reference Implementation.

     **Reference Implementation.** "Reference Implementation" means an Encoder
      and/or Decoder released by the Alliance for Open Media as a Final
      Deliverable.

     **Specification.** "Specification" means the specification designated by
      the Alliance for Open Media as a Final Deliverable for which this
      License was issued.
```

### SVT-AV1: BSD-2-Clause for the files derived from libaom (`LICENSE-BSD2.md`)
```text
Copyright (c) 2019, Alliance for Open Media. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in
   the documentation and/or other materials provided with the
   distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN
ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

### SVT-AV1: BSD-2-Clause for the assembly derived from dav1d (`Source/Lib/ASM_NEON/dav1d_*.S`, `Source/Lib/ASM_AVX2/*`)
The text as `Source/Lib/ASM_NEON/dav1d_asm.S` carries it; the other files' copyright lines
read "Copyright © 2018–2023, VideoLAN and dav1d authors", some adding "Two Orioles, LLC",
"Janne Grunau" or "Martin Storsjo", under the same terms.

```text
Copyright © 2018, VideoLAN and dav1d authors
Copyright © 2018, Janne Grunau
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### SVT-AV1: ISC for x264's `x86inc.asm` (`Source/Lib/ASM_SSE2/x86inc.asm`, `Source/Lib/ASM_AVX2/dav1d_x86inc.asm`)
```text
Copyright (C) 2005-2016 x264 project

Authors: Loren Merritt <lorenm@u.washington.edu>
         Anton Mitrofanov <BugMaster@narod.ru>
         Fiona Glaser <fiona@x264.com>
         Henrik Gramner <henrik@gramner.com>

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### SVT-AV1: MIT for `Source/Lib/Codec/vector.c` and `vector.h`
```text
The MIT License(MIT)
Copyright(c) 2016 Peter Goldsborough

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files(the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions :

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

### SVT-AV1: BSD-3-Clause for FAST corner detection (`third_party/fastfeat/LICENSE`)
```text
Copyright (c) 2006, 2008 Edward Rosten
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:


	*Redistributions of source code must retain the above copyright
	 notice, this list of conditions and the following disclaimer.

	*Redistributions in binary form must reproduce the above copyright
	 notice, this list of conditions and the following disclaimer in the
	 documentation and/or other materials provided with the distribution.

	*Neither the name of the University of Cambridge nor the names of 
	 its contributors may be used to endorse or promote products derived 
	 from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE COPYRIGHT OWNER OR
CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### SVT-AV1: MIT for safestringlib (`third_party/safestringlib/LICENSE`)
```text
MIT License

Copyright (c) 2014-2018 Intel Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

================================================================================

Copyright (C) 2012, 2013 Cisco Systems
All rights reserved.

Permission is hereby granted, free of charge, to any person
obtaining a copy of this software and associated documentation
files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use,
copy, modify, merge, publish, distribute, sublicense, and/or
sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES
OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT.  IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.
```

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
would pull them, which `deny.toml` `[graph] targets` leaves out). `aws-lc-rs` is used since
sub-project 8A (`fin-registry`, below).

## imagesize and roxmltree — `fin-probe` (image and SVG sizes)
status: used (sub-project 4C), as Cargo dependencies; no code is copied.
- `imagesize` 0.15 (MIT): PNG, JPEG and WebP sizes from the header;
- `roxmltree` 0.21 (MIT OR Apache-2.0): the SVG root's size attributes.

dotLottie archives use the `zip` and `flate2` stack listed under `fin-fonts`.

## png and fdeflate — `fin-workers` (the `media.look` PNG writer)
status: used (sub-project 6A, Task 11), as Cargo dependencies; no code is copied.
- `png` 0.18 (MIT OR Apache-2.0): encodes and decodes PNG; `media.look` writes its frames with it;
- `fdeflate` 0.3 (MIT OR Apache-2.0): `png`'s fast deflate; the rest of its stack (`flate2`,
  `crc32fast`, `miniz_oxide`) is listed under `fin-fonts` and "Other Cargo dependencies".

## keyring, secret-service and zbus — `fin-node` (the vault's OS keychain)
status: used (sub-project 5B, Task 2), as Cargo dependencies; no code is copied.
- `keyring` **=3.6.3** (MIT OR Apache-2.0), `default-features = false` with `apple-native`,
  `windows-native`, `async-secret-service`, `async-io`, `crypto-rust`: the macOS Keychain
  through `security-framework` 3 (MIT OR Apache-2.0), Windows Credential Manager through
  `windows-sys` (MIT OR Apache-2.0), the Linux Secret Service through `secret-service` 4.0.0
  (MIT OR Apache-2.0) over the pure-Rust D-Bus `zbus` 4.4.0, `zbus_names`, `zvariant` (MIT),
  with `aes`, `cbc`, `hkdf`, `sha2` (MIT OR Apache-2.0) and the `async-io` executor stack
  (`async-io`, `async-executor`, `async-channel`, `async-broadcast`, `event-listener`,
  `futures-lite`, `polling`: Apache-2.0 OR MIT);
- `secret-service` is also a direct dependency on Linux, only to ask whether the default
  collection is locked before `keyring` reads it (so a locked keychain never prompts);
- **not used:** `keyring`'s `sync-secret-service`/`linux-native` backends: `dbus` and
  `libdbus-sys` (libdbus is AFL-2.1 OR GPL-2.0+) appear in `Cargo.lock` as unselected optional
  dependencies and are never built (`crates/fin-node/tests/vault.rs` `no_dbus_feature`);
- `ring` 0.17 (the identity key, the wrapped file store) and `base64` 0.22 (MIT OR Apache-2.0)
  were already in the tree.
- the rest of that graph, new with it (each crate's licence text is its own `LICENSE-*` file
  in its package on crates.io):
  - MIT OR Apache-2.0: `block-padding` 0.3.3, `cipher` 0.4.4, `crossbeam-utils` 0.8.23, `enumflags2_derive` 0.7.12, `enumflags2` 0.7.12, `inout` 0.1.4, `ordered-stream` 0.2.0, `piper` 0.2.5, `proc-macro-crate` 3.5.0, `serde_repr` 0.1.21, `sha1` 0.10.7, `toml_datetime` 1.1.2+spec-1.1.0, `toml_edit` 0.25.17+spec-1.1.0, `toml_parser` 1.1.5+spec-1.1.0, `windows_x86_64_msvc` 0.53.1;
  - Apache-2.0 OR MIT: `async-fs` 2.2.0, `async-lock` 3.4.2, `async-task` 4.7.1, `atomic-waker` 1.1.2, `blocking` 1.7.0, `concurrent-queue` 2.5.0, `event-listener-strategy` 0.5.4, `parking` 2.2.1, `zeroize_derive` 1.5.0;
  - MIT: `endi` 1.1.1, `generic-array` 0.14.7, `memoffset` 0.9.1, `winnow` 1.0.4, `xdg-home` 1.3.0, `zbus_macros` 4.4.0, `zvariant_derive` 4.2.0, `zvariant_utils` 2.1.0;
  - Unlicense OR MIT: `byteorder` 1.5.0;

## rcgen and qrcode — `fin-node` (remote-listener certificates and pairing QR codes)
status: used (sub-project 5B, Task 3), as Cargo dependencies; no code is copied.
- `rcgen` 0.14.10 (MIT OR Apache-2.0), `default-features = false` with `ring` and `pem`:
  self-signed certificates from the identity key; with it `yasna` 0.6.0, `time` 0.3.55,
  `time-core` 0.1.9, `time-macros` 0.2.32, `deranged` 0.5.9, `num-conv` 0.2.2, `powerfmt`
  0.2.1 (all MIT OR Apache-2.0); `pem` is listed below;
- `qrcode` 0.14.1 (MIT OR Apache-2.0), `default-features = false` with `svg` only (its Unicode
  renderer needs no feature): the pairing link as SVG and as text;
- tests only (never shipped): `rqrr` 0.11 ((MIT OR Apache-2.0) AND ISC) decodes the QR code
  back, with `g2p`, `g2gen`, `g2poly`, `lru` and `hashbrown` (MIT or MIT OR Apache-2.0);
- **not used:** `rcgen`'s `x509-parser` feature: `x509-parser`, `asn1-rs`, `der-parser`, `nom`
  and `oid-registry` appear in `Cargo.lock` as unselected optional dependencies and are never
  built.

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

## Native libraries
Native code compiled into Finisher's binaries by a crate's build script, as
`third_party/native.json` lists it (sub-project 6 spec §16.3; `scripts/check-native-deps.mjs`
checks that every such package is listed, its licence allowed and named here).

### SQLite (public domain) — `libsqlite3-sys` 0.35, bundled by `rusqlite`
status: used (`fin-store`), the SQLite amalgamation compiled from the crate's sources. SQLite
is dedicated to the public domain (`blessing`); `libsqlite3-sys` itself is MIT.

### BLAKE3 (CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception) — `blake3` 1
status: used, the crate's own C and assembly implementations compiled by its build script.
Finisher uses them under Apache-2.0.

### ring (Apache-2.0 AND ISC) — `ring` 0.17
status: used (sub-project 4C), its bundled C and assembly compiled by its build script; see
"ureq, rustls and ring" above.

### AWS-LC (ISC AND Apache-2.0 AND MIT AND BSD-3-Clause) — `aws-lc-sys` 0.47, `aws-lc-rs` 1.19
status: used (sub-project 8A, `fin-registry`): `tough` 0.24's signature checks and the
registry editor's ed25519 keys. `aws-lc-sys` compiles AWS-LC's bundled C and assembly with its
cc builder (on Windows x64, its bundled `prebuilt-nasm` objects); `aws-lc-rs` compiles nothing.
AWS-LC is ISC (`aws-lc-rs`: ISC AND (Apache-2.0 OR ISC)), with Apache-2.0, MIT and
BSD-3-Clause parts and parts under (Apache-2.0 OR ISC OR MIT) or (Apache-2.0 OR ISC OR MIT-0);
all of them apply.

## MPL-2.0 §3.2 — source of MPL-licensed crates
Finisher's executables include the MPL-2.0 crates listed here, unmodified. Under MPL-2.0 §3.2
their source code is available from crates.io (`https://crates.io/crates/<name>/<version>`)
and they are licensed under the Mozilla Public License 2.0
(`https://mozilla.org/MPL/2.0/`). The native build gate checks that every MPL-2.0 package in
the shipped graph is named here as `<name> <version>`.

No MPL-2.0 crate is in the shipped graph yet (symphonia arrives in sub-project 6C).

## openai (Apache-2.0) — the built-in agent's Chat Completions transport
status: used (sub-project 7A), as an npm dependency of `finisherd` (`openai` 7.47.0, exact
pin, no runtime dependencies; its optional peers are not installed); no code is copied. It
carries request bodies Finisher builds and parses the SSE stream (`apps/finisherd/src/agent/
builtin/compat.ts`), with OpenRouter's or a local server's base URL.

## BLAKE3 (CC0-1.0 OR Apache-2.0) — test vectors
status: used (sub-project 7A): `packages/fin-node/test/hash.test.ts` embeds hashes from
BLAKE3-team/BLAKE3 `test_vectors/test_vectors.json`, used under CC0-1.0. The hashing itself is
the `blake3` crate, already a dependency, exposed through fin-node; no code is copied.

## fast-check (MIT) — property tests
status: used (sub-project 7A) as a development dependency of `finisherd` (4.10.2, with
`pure-rand`, MIT); not shipped.

## tough, spdx and semver — registries and plugin manifests (sub-project 8A)
status: used (sub-project 8A) by `fin-registry`, `fin-plugin-api`, `fin-plugin-cli` and `fin-node`,
as Cargo dependencies; no code is copied and nothing is modified.
- `tough` 0.24 (MIT OR Apache-2.0), the TUF 1.0 client and signer, without its `http` feature
  (Finisher's transports are its own). Its crypto is `aws-lc-rs` (see "AWS-LC" above), and its
  tree adds `async-recursion`, `async-trait`, `globset`, `jiff`, `olpc-cjson`, `pem`,
  `serde_plain`, `snafu`, `tokio` (rt only), `tokio-util`, `typed-path` and `url`, all listed
  under "Other Cargo dependencies" below.
- `spdx` 0.13 (**Apache-2.0** only), default features off: it parses and evaluates SPDX licence
  expressions for plugin manifests and carries the SPDX licence list's ids and flags (the list is
  published by SPDX under CC0-1.0); it carries no licence texts without its `text` feature.
- `semver` 1 (MIT OR Apache-2.0): plugin and host API versions and ranges.

## ws (MIT) — WebSocket server of `finisherd`
status: used (sub-project 8A) as an npm dependency of `finisherd` (`ws` 8.22.0, exact pin,
no runtime dependencies); no code is copied. It carries the console's and clients' sockets
(`apps/finisherd/src/upgrade.ts`); `@types/ws` (MIT) is a development dependency and is not shipped.

## jose (MIT) — DPoP proofs in `finisherd`
status: used (sub-project 5B, Task 7) as an npm dependency of `finisherd` (`jose` 6.2.14, exact
pin, no runtime dependencies); no code is copied. It verifies devices' DPoP proofs (ES256 JWS)
and computes RFC 7638 key thumbprints (`apps/finisherd/src/auth/dpop.ts`).

## Other Cargo dependencies — everything else that ships
status: used, as Cargo dependencies; no code is copied. These crates are in the shipped graph
(`scripts/check-notices.mjs`: every workspace member but the dev-only ones, normal and build
edges, for each `deny.toml` target) and are named nowhere above. They are grouped by the licence
expression in their `Cargo.toml`, with the count; each crate's licence text is its own
`LICENSE-*` file in its package on crates.io. Where the expression has an `OR`, Finisher uses the
crate under an alternative `deny.toml` allows.

- MIT OR Apache-2.0 (133):
  `allocator-api2` 0.2.21, `anstyle` 1.0.14, `arrayvec` 0.7.8, `ash` 0.38.0+1.3.281,
  `base64` 0.22.1, 0.23.1, `bitflags` 2.13.2, `block-buffer` 0.12.1, `bstr` 1.13.1,
  `cc` 1.10.0, `cfg-if` 1.0.5, `clap_builder` 4.6.7, `clap_derive` 4.6.7, `clap_lex` 1.1.1,
  `clap` 4.6.7, `cmake` 0.1.58, `core-foundation-sys` 0.8.7, `core-foundation` 0.10.1,
  `cpufeatures` 0.3.1, `crc32fast` 1.5.3, `crypto-common` 0.2.2, `digest` 0.11.3,
  `displaydoc` 0.2.7, `document-features` 0.2.12, `dyn-clone` 1.0.20,
  `enumflags2_derive` 0.7.12, `enumflags2` 0.7.12, `errno` 0.3.14,
  `find-msvc-tools` 0.1.15, `font-types` 0.11.3, 0.12.7, `form_urlencoded` 1.2.2,
  `futures-channel` 0.3.34, `futures-core` 0.3.34, `futures-executor` 0.3.34,
  `futures-io` 0.3.34, `futures-macro` 0.3.34, `futures-sink` 0.3.34, `futures-task` 0.3.34,
  `futures-util` 0.3.34, `futures` 0.3.34, `getrandom` 0.2.17, 0.3.4, 0.4.3,
  `gpu-allocator` 0.28.0, `half` 2.7.1, `hashbrown` 0.15.5, 0.16.1, 0.17.1, `hashlink` 0.10.0,
  `heck` 0.5.0, `hex` 0.4.3, `hmac` 0.13.0, `hybrid-array` 0.4.15, `idna` 1.1.0, `itoa` 1.0.18,
  `jobserver` 0.1.35, `landlock` 0.4.7, `libc` 0.2.191, `litrs` 1.0.1, `lock_api` 0.4.14, `log` 0.4.34,
  `memmap2` 0.9.12, `mp4-atom` 0.17.0, `naga-types` 30.0.1, `naga` 30.0.1, `num-bigint` 0.4.8,
  `num-complex` 0.4.6, `num-integer` 0.1.47, `num-iter` 0.1.46, `num-rational` 0.4.2,
  `num-traits` 0.2.19, `num` 0.4.3, `once_cell` 1.21.4, `openssl-probe` 0.2.2,
  `parking_lot_core` 0.9.12, `parking_lot` 0.12.5, `pastey` 0.2.3, `percent-encoding` 2.3.2,
  `pkg-config` 0.3.34, `polycool` 0.4.0, `ppv-lite86` 0.2.21, `presser` 0.3.1,
  `proc-macro2` 1.0.107, `profiling` 1.0.18, `quote` 1.0.47, `rand_chacha` 0.9.0,
  `rand_core` 0.10.1, 0.9.5, `rand` 0.9.5, `range-alloc` 0.1.5, `raw-window-metal` 1.1.0,
  `ref-cast-impl` 1.0.27, `ref-cast` 1.0.27, `regex-automata` 0.4.18, `regex-syntax` 0.8.11,
  `renderdoc-sys` 1.1.0, `rustc_version` 0.4.1, `scopeguard` 1.2.0,
  `security-framework-sys` 2.17.0, `serde_core` 1.0.229, `serde_derive_internals` 0.30.0,
  `serde_derive` 1.0.229, `serde_json` 1.0.154, `serde` 1.0.229, `shlex` 2.0.1,
  `smallvec` 1.16.4, `smol_str` 0.3.7, `snafu-derive` 0.8.9, `stable_deref_trait` 1.2.1,
  `static_assertions` 1.1.0, `syn` 2.0.119, 3.0.9, `tempfile` 3.27.0,
  `thiserror-impl` 1.0.69, 2.0.21, `thiserror` 1.0.69, 2.0.21, `typenum` 1.20.1,
  `unicode-width` 0.2.3, `utf8-zero` 0.8.1, `wgpu-core-deps-apple` 30.0.1,
  `wgpu-core-deps-windows-linux-android` 30.0.1, `wgpu-core` 30.0.1, `wgpu-hal` 30.0.1,
  `wgpu-naga-bridge` 30.0.1, `wgpu-types` 30.0.1, `wgpu` 30.0.1, `widestring` 1.2.1,
  `windows-collections` 0.3.2, `windows-core` 0.62.2, `windows-future` 0.3.2,
  `windows-implement` 0.60.2, `windows-interface` 0.59.3, `windows-link` 0.2.1,
  `windows-numerics` 0.3.1, `windows-result` 0.4.1, `windows-strings` 0.5.1,
  `windows-sys` 0.52.0, 0.61.2, `windows-targets` 0.52.6, `windows-threading` 0.2.1,
  `windows_aarch64_msvc` 0.52.6, `windows` 0.62.2.
- MIT (28):
  `block2` 0.6.2, `bytes` 1.12.1, `cfg_aliases` 0.2.2, `convert_case` 0.12.0,
  `derive_more-impl` 2.1.1, `derive_more` 2.1.1, `fs_extra` 1.3.0, `napi-build` 2.6.0,
  `napi-derive-backend` 6.1.6, `napi-derive` 3.6.13, `napi-sys` 3.4.1, `napi` 3.15.0,
  `nix` 0.31.3, `objc2-encode` 4.1.0, `objc2-foundation` 0.3.2, `objc2` 0.6.5,
  `ordered-float` 5.5.1, `schemars_derive` 1.2.3, `schemars` 1.2.3, `simd-adler32` 0.3.10,
  `slab` 0.4.12, `strsim` 0.11.1, `synstructure` 0.14.0, `tracing-attributes` 0.1.31,
  `tracing-core` 0.1.36, `tracing` 0.1.44, `ulid` 1.2.1, `zmij` 1.0.23.
- Apache-2.0 OR MIT (21):
  `autocfg` 1.5.1, `bit-set` 0.10.0, `bit-vec` 0.9.1, `cmov` 0.5.4, `const-oid` 0.10.2,
  `ctor` 1.0.13, `ctutils` 0.4.7, `equivalent` 1.0.3, `fastrand` 2.5.0, `idna_adapter` 1.2.2,
  `indexmap` 2.14.2, `kurbo` 0.13.1, `linebender_resource_handle` 0.1.1, `nohash-hasher` 0.2.0,
  `pin-project-internal` 1.1.13, `pin-project-lite` 0.2.17, `pin-project` 1.1.13,
  `rustc-hash` 1.1.0, 2.1.3, `utf8_iter` 1.1.0, `utf_types` 1.0.0, `zeroize` 1.9.1.
- Unicode-3.0 (18):
  `icu_collections` 2.4.0, `icu_locale_core` 2.4.0, `icu_normalizer_data` 2.4.0,
  `icu_normalizer` 2.4.0, `icu_properties_data` 2.4.0, `icu_properties` 2.4.0,
  `icu_provider` 2.4.0, `litemap` 0.8.3, `potential_utf` 0.1.6, `tinystr` 0.8.5,
  `writeable` 0.6.4, `yoke-derive` 0.8.4, `yoke` 0.8.4, `zerofrom-derive` 0.1.8,
  `zerofrom` 0.1.8, `zerotrie` 0.2.5, `zerovec-derive` 0.11.7, `zerovec` 0.11.9.
- Unlicense OR MIT (8):
  `aho-corasick` 1.1.5, `jiff-core` 0.1.1, `jiff-static` 0.2.38, `jiff-tzdb-platform` 0.1.3,
  `jiff-tzdb` 0.1.9, `memchr` 2.8.3, `termcolor` 1.4.1, `winapi-util` 0.1.11.
- Zlib OR Apache-2.0 OR MIT (10):
  `bytemuck_derive` 1.12.1, `bytemuck` 1.25.2, `objc2-core-foundation` 0.3.2,
  `objc2-core-graphics` 0.3.2, `objc2-core-media` 0.3.2, `objc2-core-video` 0.3.2,
  `objc2-metal` 0.3.2, `objc2-quartz-core` 0.3.2, `objc2-video-toolbox` 0.3.2,
  `tinyvec` 1.13.3.
- MIT/Apache-2.0 (5):
  `fallible-iterator` 0.3.0, `fallible-streaming-iterator` 0.1.9, `rangemap` 1.8.0,
  `vcpkg` 0.2.15, `version_check` 0.9.5.
- (MIT OR Apache-2.0) AND Unicode-3.0 (2):
  `unicode-ident` 1.0.27, `unicode-normalization` 0.1.26.
- 0BSD (2):
  `doctest-file` 1.1.1, `recvmsg` 1.0.0.
- Apache-2.0 (2):
  `codespan-reporting` 0.13.1, `spirv` 0.4.0+sdk-1.4.341.0.
- Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT (2):
  `linux-raw-sys` 0.12.1, `rustix` 1.1.6.
- Apache-2.0/MIT (2):
  `crc32c` 0.6.8, `pollster` 0.4.0.
- BSD-2-Clause OR Apache-2.0 OR MIT (2):
  `zerocopy-derive` 0.8.63, `zerocopy` 0.8.63.
- CC0-1.0 OR MIT-0 OR Apache-2.0 (2):
  `constant_time_eq` 0.4.2, `dunce` 1.0.5.
- Unlicense/MIT (2):
  `same-file` 1.0.6, `walkdir` 2.5.0.
- Zlib (2):
  `foldhash` 0.1.5, 0.2.0, `slotmap` 1.1.1.
- 0BSD OR Apache-2.0 (1):
  `interprocess` 2.4.7.
- 0BSD OR MIT OR Apache-2.0 (1):
  `adler2` 2.0.1.
- ISC (1):
  `libloading` 0.8.9, 0.9.0.
- MIT OR Apache-2.0 OR Zlib (1):
  `raw-window-handle` 0.6.2.
- MIT OR Zlib OR Apache-2.0 (1):
  `miniz_oxide` 0.9.1.

