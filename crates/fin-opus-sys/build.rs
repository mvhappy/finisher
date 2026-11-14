//! Builds libopus 1.6.1 (float, static) from the pinned submodule `third_party/opus` into
//! `OUT_DIR`, and the shim `shim/fin_opus_shim.c` (6C plan Decision 11; built from source at a
//! pinned tag, never a prebuilt download).
//!
//! No meson, cmake or autotools: this script compiles `celt_sources.mk` `CELT_SOURCES`,
//! `silk_sources.mk` `SILK_SOURCES` + `SILK_SOURCES_FLOAT` and `opus_sources.mk`
//! `OPUS_SOURCES` + `OPUS_SOURCES_FLOAT` with the `cc` crate and a `config.h` of its own. Not
//! compiled: `dnn/` and `lpcnet_sources.mk` (`ENABLE_DEEP_PLC`, `ENABLE_DRED`, `ENABLE_OSCE`
//! stay undefined), the x86 SSE/AVX and ARM NEON intrinsics and assembly files, run-time CPU
//! detection (no `OPUS_HAVE_RTCD`, no `OPUS_*_MAY_HAVE_*`), fixed point. So the build needs no
//! model download, no Python and no assembler. Floating-point contraction is off
//! (`-ffp-contract=off`; MSVC does not contract by default). The source lists live in
//! `src/version.rs`, which also recognises the release: moving the submodule fails the build
//! until they are re-checked (the dav1d rule).

#[path = "src/version.rs"]
mod version;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest.join("../../third_party/opus");
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets it"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/version.rs");
    println!("cargo:rerun-if-changed=shim");
    for d in ["configure.ac", "celt", "silk", "src", "include"] {
        println!("cargo:rerun-if-changed={}", root.join(d).display());
    }
    println!("cargo:rerun-if-env-changed=FIN_OPUS_CHECK_ONLY");
    // `cargo check --target <other>` type-checks without compiling C: nothing links.
    if env::var("FIN_OPUS_CHECK_ONLY").as_deref() == Ok("1") {
        return;
    }
    let read = |f: &str| fs::read_to_string(root.join(f)).ok();
    let configure = read("configure.ac").unwrap_or_else(|| {
        panic!(
            "libopus sources are missing at {}: run `git submodule update --init --depth 1 third_party/opus`",
            root.display()
        )
    });
    if let Err(why) = version::check(&configure, read("package_version").as_deref(), read) {
        panic!("{why}");
    }

    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let gen_dir = out.join("gen");
    fs::create_dir_all(&gen_dir).expect("OUT_DIR is writable");
    write(&gen_dir.join("config.h"), &config_h(&os, msvc));

    let mut b = cc::Build::new();
    b.include(&gen_dir)
        .include(&root)
        .include(root.join("include"))
        .include(root.join("celt"))
        .include(root.join("silk"))
        .include(root.join("silk/float"))
        .define("HAVE_CONFIG_H", None)
        .define("OPUS_BUILD", None)
        .define("NDEBUG", None)
        .opt_level(2)
        .debug(false)
        .warnings(false)
        .cargo_warnings(false);
    if msvc {
        b.define("_CRT_SECURE_NO_WARNINGS", None)
            .flag("/fp:precise");
    } else {
        b.flag("-std=gnu99")
            .flag("-ffp-contract=off")
            .flag("-fvisibility=hidden");
    }
    for (_, _, list) in version::LISTS {
        for f in *list {
            b.file(root.join(f));
        }
    }
    b.include(manifest.join("shim"))
        .file(manifest.join("shim/fin_opus_shim.c"))
        .compile("opus");
    if os == "linux" {
        println!("cargo:rustc-link-lib=m");
    }
}

fn write(path: &Path, text: &str) {
    // Unchanged files keep their mtime, so cc's objects do not rebuild for nothing.
    if fs::read_to_string(path).ok().as_deref() != Some(text) {
        fs::write(path, text).expect("OUT_DIR is writable");
    }
}

/// libopus's `config.h` for a float build: alloca for scratch (MSVC has no variable-length
/// arrays, so every target takes the same path), `lrintf`, the default hardening checks, and
/// nothing that enables the DNN, SIMD or run-time CPU detection.
fn config_h(os: &str, msvc: bool) -> String {
    let mut s = String::from("/* libopus config.h, written by fin-opus-sys/build.rs */\n");
    s.push_str(&format!(
        "#define PACKAGE_NAME \"opus\"\n#define PACKAGE_VERSION \"{v}\"\n\
         #define PACKAGE_STRING \"opus {v}\"\n",
        v = version::VERSION
    ));
    s.push_str("#define USE_ALLOCA 1\n");
    if os != "windows" {
        s.push_str("#define HAVE_ALLOCA_H 1\n");
    }
    s.push_str("#define HAVE_LRINTF 1\n#define HAVE_LRINT 1\n#define HAVE_STDINT_H 1\n");
    s.push_str("#define ENABLE_HARDENING 1\n#define DISABLE_DEBUG_FLOAT 1\n");
    if msvc {
        s.push_str("#define restrict __restrict\n");
    }
    s
}
