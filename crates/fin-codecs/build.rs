//! Builds SVT-AV1's encoder (static, no apps, no tests) from the pinned submodule
//! `third_party/svt-av1` with cmake into `OUT_DIR`, and the shim `csrc/svt_shim.c`
//! (sub-project 6 spec §6.4: built from source at a pinned tag, never a prebuilt download).

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let src = manifest.join("../../third_party/svt-av1");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=csrc/svt_shim.c");
    println!(
        "cargo:rerun-if-changed={}",
        src.join("CMakeLists.txt").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        src.join("Source/API").display()
    );
    if !src.join("CMakeLists.txt").exists() {
        panic!(
            "SVT-AV1 sources are missing at {}: run `git submodule update --init --depth 1 third_party/svt-av1`",
            src.display()
        );
    }
    if !have("cmake") {
        panic!(
            "cmake is needed to build SVT-AV1: install it (macOS `brew install cmake`, Debian/Ubuntu `apt install cmake`, Windows `winget install Kitware.CMake`)"
        );
    }
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if arch == "x86_64" && !have("nasm") {
        panic!(
            "nasm is needed to build SVT-AV1's x86_64 assembly: install it (macOS `brew install nasm`, Debian/Ubuntu `apt install nasm`, Windows `winget install NASM.NASM`)"
        );
    }
    let dst = cmake::Config::new(&src)
        .profile("Release")
        .define("BUILD_APPS", "OFF")
        .define("BUILD_TESTING", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("SVT_AV1_LTO", "OFF")
        .define("EXCLUDE_HASH", "ON")
        .define("REPRODUCIBLE_BUILDS", "ON")
        // Errors come back as codes; the encoder never writes to stderr.
        .define("LOG_QUIET", "ON")
        .define("CMAKE_INSTALL_LIBDIR", "lib")
        .build();
    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=SvtAv1Enc");
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "linux" {
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=m");
    }
    cc::Build::new()
        .file(manifest.join("csrc/svt_shim.c"))
        .include(src.join("Source/API"))
        .warnings(true)
        .compile("fin_svt_shim");
}
