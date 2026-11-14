//! Builds vo-aacenc 0.1.3 (fixed point, static) from the pinned submodule
//! `third_party/vo-aacenc` into `OUT_DIR`, and the shim `shim/fin_aacenc_shim.c` (6C plan
//! Decision 12; built from source at a pinned commit, never a prebuilt download).
//!
//! **Linux only** (Decision 9, spec §16.4: Finisher's own AAC code ships in Linux binaries
//! only): on every other target this script compiles nothing and the crate is empty;
//! `fin-codecs` takes it only under `cfg(target_os = "linux")` (`scripts/check-crate-deps.mjs`).
//!
//! No autotools: this script compiles `Makefile.am`'s `libvo_aacenc_la_SOURCES` (the
//! `common/` and `aacenc/` C files) with the `cc` crate. Not compiled: the ARMv5E/ARMv7
//! assembly (`--enable-armv5e`/`--enable-armv7neon` are off; no `ARMV5E`, `ARMV7Neon` or
//! `ARM_INASM` defines), the example program. The codec is fixed point; floating-point
//! contraction is off anyway (`-ffp-contract=off`). The source list lives in `src/version.rs`,
//! which also recognises the release: moving the submodule fails the build until it is
//! re-checked (the dav1d rule).

#[path = "src/version.rs"]
mod version;

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest.join("../../third_party/vo-aacenc");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/version.rs");
    println!("cargo:rerun-if-changed=shim");
    for d in ["configure.ac", "Makefile.am", "common", "aacenc"] {
        println!("cargo:rerun-if-changed={}", root.join(d).display());
    }
    println!("cargo:rerun-if-env-changed=FIN_AACENC_CHECK_ONLY");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    // `cargo check --target <linux triple>` from another host type-checks without compiling C.
    if env::var("FIN_AACENC_CHECK_ONLY").as_deref() == Ok("1") {
        return;
    }
    let read = |f: &str| fs::read_to_string(root.join(f)).ok();
    let missing = || {
        panic!(
            "vo-aacenc sources are missing at {}: run `git submodule update --init --depth 1 third_party/vo-aacenc`",
            root.display()
        )
    };
    let configure = read("configure.ac").unwrap_or_else(missing);
    let makefile = read("Makefile.am").unwrap_or_else(missing);
    if let Err(why) = version::check(&configure, &makefile) {
        panic!("{why}");
    }

    let mut b = cc::Build::new();
    b.include(root.join("common/include"))
        .include(root.join("aacenc/inc"))
        .include(root.join("aacenc/basic_op"))
        .define("__unused", Some(""))
        .define("NDEBUG", None)
        .define(
            "FIN_AACENC_VERSION",
            Some(format!("\"{}\"", version::VERSION).as_str()),
        )
        .opt_level(2)
        .debug(false)
        .warnings(false)
        .cargo_warnings(false)
        .flag("-std=gnu99")
        .flag("-ffp-contract=off")
        .flag("-fvisibility=hidden");
    for f in version::SOURCES {
        b.file(root.join(f));
    }
    b.include(manifest.join("shim"))
        .file(manifest.join("shim/fin_aacenc_shim.c"))
        .compile("vo-aacenc");
}
