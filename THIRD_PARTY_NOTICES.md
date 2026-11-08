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

## rav1d (BSD-2-Clause) — `fin-codecs` (AV1 decode)
status: used (sub-project 6A), as a Cargo dependency (`rav1d` 1.1), the Rust port of dav1d,
with its x86_64 assembly compiled by its build script. AV1 itself is under the Alliance for
Open Media Patent License 1.0 (above). Its licence (`COPYING`), in full:

```text
Copyright © 2018-2019, VideoLAN and dav1d authors
Copyright © 2023-2024, VideoLAN, dav1d authors, and Internet Security Research Group
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
