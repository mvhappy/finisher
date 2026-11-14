//! Which vo-aacenc `third_party/vo-aacenc` holds, and the exact source list this crate compiles
//! (6C plan Decision 12). Shared by `build.rs` (which refuses any other release) and the crate's
//! unit tests, so the check itself is tested.
//!
//! The release is recognised by `configure.ac`'s `AC_INIT` version and libtool version, and by
//! `Makefile.am`'s `libvo_aacenc_la_SOURCES`, which must equal [`SOURCES`] exactly (the C files;
//! the ARMv5E/ARMv7 assembly that `Makefile.am` adds under `--enable-armv5e`/`--enable-armv7neon`
//! is never built): moving the submodule fails the build until they are re-checked.

/// The vo-aacenc release `third_party/vo-aacenc` is pinned to.
pub const VERSION: &str = "0.1.3";

/// `VO_AACENC_VERSION` (libtool `current:revision:age`) in 0.1.3's `configure.ac`.
pub const LIBTOOL: &str = "0:4:0";

/// `Makefile.am` `libvo_aacenc_la_SOURCES`, with `$(VO_COMMON)` and `$(ENC_SRC)` expanded.
pub const SOURCES: &[&str] = &[
    "common/cmnMemory.c",
    "aacenc/basic_op/basicop2.c",
    "aacenc/basic_op/oper_32b.c",
    "aacenc/src/aac_rom.c",
    "aacenc/src/aacenc.c",
    "aacenc/src/aacenc_core.c",
    "aacenc/src/adj_thr.c",
    "aacenc/src/band_nrg.c",
    "aacenc/src/bit_cnt.c",
    "aacenc/src/bitbuffer.c",
    "aacenc/src/bitenc.c",
    "aacenc/src/block_switch.c",
    "aacenc/src/channel_map.c",
    "aacenc/src/dyn_bits.c",
    "aacenc/src/grp_data.c",
    "aacenc/src/interface.c",
    "aacenc/src/line_pe.c",
    "aacenc/src/memalign.c",
    "aacenc/src/ms_stereo.c",
    "aacenc/src/pre_echo_control.c",
    "aacenc/src/psy_configuration.c",
    "aacenc/src/psy_main.c",
    "aacenc/src/qc_main.c",
    "aacenc/src/quantize.c",
    "aacenc/src/sf_estim.c",
    "aacenc/src/spreading.c",
    "aacenc/src/stat_bits.c",
    "aacenc/src/tns.c",
    "aacenc/src/transform.c",
];

/// The files of `libvo_aacenc_la_SOURCES` in a `Makefile.am` text (`VAR = \` then one file a
/// line, to the first line without a trailing backslash), with the two directory variables
/// expanded.
pub fn sources(makefile_am: &str) -> Option<Vec<String>> {
    let var = |name: &str| {
        makefile_am.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
    };
    let (common, enc) = (var("VO_COMMON")?, var("ENC_SRC")?);
    let mut lines = makefile_am.lines();
    let first = lines.find(|l| {
        let mut w = l.split_whitespace();
        w.next() == Some("libvo_aacenc_la_SOURCES") && w.next() == Some("=")
    })?;
    let mut words: Vec<String> = Vec::new();
    let mut more = first.trim_end().ends_with('\\');
    for l in lines {
        if !more {
            break;
        }
        more = l.trim_end().ends_with('\\');
        words.extend(l.split_whitespace().filter(|w| *w != "\\").map(|w| {
            w.replace("$(VO_COMMON)", &common)
                .replace("$(ENC_SRC)", &enc)
        }));
    }
    Some(words)
}

/// `Ok` when the sources are vo-aacenc [`VERSION`]: `configure.ac` initialises that version
/// with libtool version [`LIBTOOL`], and `Makefile.am`'s C source list equals [`SOURCES`]. The
/// error says what differs.
pub fn check(configure_ac: &str, makefile_am: &str) -> Result<(), String> {
    let init = format!("AC_INIT([vo-aacenc], [{VERSION}]");
    if !configure_ac
        .lines()
        .any(|l| l.trim_start().starts_with(&init))
    {
        return Err(format!(
            "third_party/vo-aacenc is not vo-aacenc {VERSION}: configure.ac has no `{init}`"
        ));
    }
    let lt = format!("VO_AACENC_VERSION={LIBTOOL}");
    if !configure_ac.lines().any(|l| l.trim() == lt) {
        return Err(format!(
            "third_party/vo-aacenc is not vo-aacenc {VERSION}: configure.ac's libtool version \
             is not {LIBTOOL}"
        ));
    }
    let got = sources(makefile_am)
        .ok_or_else(|| "Makefile.am has no libvo_aacenc_la_SOURCES".to_string())?;
    if got != SOURCES {
        return Err(format!(
            "Makefile.am libvo_aacenc_la_SOURCES differs from fin-aacenc-sys's list: re-check \
             build.rs's source list against vo-aacenc and update VERSION, \
             third_party/native.json and the notices (got {got:?})"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AC: &str =
        "AC_INIT([vo-aacenc], [0.1.3], [http://example.invalid/])\nVO_AACENC_VERSION=0:4:0\n";

    fn am(list: &[&str]) -> String {
        let files: Vec<String> = list
            .iter()
            .map(|f| {
                f.replace("common/", "$(VO_COMMON)/")
                    .replace("aacenc/src/", "$(ENC_SRC)/")
            })
            .collect();
        format!(
            "VO_COMMON = common\nENC_SRC = aacenc/src\n\nlibvo_aacenc_la_SOURCES = \\\n    {}\n\n\
             if ARMV7NEON\n    libvo_aacenc_la_SOURCES += \\\n        $(ENC_SRC)/asm/ARMV7/x.s\nendif\n",
            files.join(" \\\n    ")
        )
    }

    #[test]
    fn aacenc_right_version_accepted() {
        assert_eq!(check(AC, &am(SOURCES)), Ok(()));
    }

    #[test]
    fn aacenc_wrong_version_refused_by_build() {
        let e = check(&AC.replace("0.1.3", "0.1.2"), &am(SOURCES)).unwrap_err();
        assert!(e.contains("not vo-aacenc 0.1.3"), "{e}");
        let e = check(&AC.replace("0:4:0", "0:3:0"), &am(SOURCES)).unwrap_err();
        assert!(e.contains("libtool version"), "{e}");
    }

    #[test]
    fn a_changed_source_list_is_refused() {
        let mut moved: Vec<&str> = SOURCES.to_vec();
        moved.push("aacenc/src/new.c");
        let e = check(AC, &am(&moved)).unwrap_err();
        assert!(e.contains("differs"), "{e}");
        assert!(
            check(AC, "")
                .unwrap_err()
                .contains("no libvo_aacenc_la_SOURCES")
        );
    }

    /// The pinned submodule itself passes (the build ran this check too, on Linux).
    #[test]
    fn the_submodule_is_the_pinned_release() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../third_party/vo-aacenc/");
        let read = |f: &str| std::fs::read_to_string(format!("{root}{f}")).ok();
        let ac = read("configure.ac").expect("the submodule is checked out");
        let am = read("Makefile.am").expect("the submodule is checked out");
        assert_eq!(check(&ac, &am), Ok(()));
    }
}
