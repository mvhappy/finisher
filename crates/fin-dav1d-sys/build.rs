//! Builds dav1d (decoder library only, static, 8- and 16-bit) from the pinned submodule
//! `third_party/dav1d` into `OUT_DIR`, and the shim `csrc/dav1d_shim.c` (sub-project 6 spec
//! §6.4: built from source at a pinned tag, never a prebuilt download).
//!
//! No meson or ninja: this script does what dav1d's `meson.build` and `src/meson.build` do
//! for the shipped targets. It writes `config.h` (and `config.asm` on x86_64), compiles the
//! bit-depth templates once per depth with `BITDEPTH`, assembles the x86_64 assembly with
//! nasm and the aarch64 assembly (`.S`) with the C compiler, and archives everything as one
//! `libdav1d`. The source lists mirror `src/meson.build` at `VERSION`; moving the submodule
//! fails the build until they are re-checked.
//! Assembly: x86_64 (nasm ≥ 2.14) and aarch64 except under MSVC, whose `cl` cannot assemble
//! GNU `.S` (dav1d's meson needs gas-preprocessor there): aarch64 Windows decodes with
//! dav1d's C paths.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The dav1d release `third_party/dav1d` is pinned to.
const VERSION: &str = "1.5.4";

/// `libdav1d_sources`.
const SOURCES: &[&str] = &[
    "cdf.c",
    "cpu.c",
    "ctx.c",
    "data.c",
    "decode.c",
    "dequant_tables.c",
    "getbits.c",
    "intra_edge.c",
    "itx_1d.c",
    "lf_mask.c",
    "lib.c",
    "log.c",
    "mem.c",
    "msac.c",
    "obu.c",
    "pal.c",
    "picture.c",
    "qm.c",
    "ref.c",
    "refmvs.c",
    "scan.c",
    "tables.c",
    "thread_task.c",
    "warpmv.c",
    "wedge.c",
];

/// `libdav1d_tmpl_sources`: compiled once per bit depth.
const TEMPLATES: &[&str] = &[
    "cdef_apply_tmpl.c",
    "cdef_tmpl.c",
    "fg_apply_tmpl.c",
    "filmgrain_tmpl.c",
    "ipred_prepare_tmpl.c",
    "ipred_tmpl.c",
    "itx_tmpl.c",
    "lf_apply_tmpl.c",
    "loopfilter_tmpl.c",
    "looprestoration_tmpl.c",
    "lr_apply_tmpl.c",
    "mc_tmpl.c",
    "recon_tmpl.c",
];

/// aarch64 assembly: both depths, then 8-bit, then 16-bit.
const ARM64_ASM: &[&str] = &[
    "arm/64/itx.S",
    "arm/64/looprestoration_common.S",
    "arm/64/msac.S",
    "arm/64/refmvs.S",
    "arm/64/cdef.S",
    "arm/64/filmgrain.S",
    "arm/64/ipred.S",
    "arm/64/loopfilter.S",
    "arm/64/looprestoration.S",
    "arm/64/mc.S",
    "arm/64/mc_dotprod.S",
    "arm/64/cdef16.S",
    "arm/64/filmgrain16.S",
    "arm/64/ipred16.S",
    "arm/64/itx16.S",
    "arm/64/loopfilter16.S",
    "arm/64/looprestoration16.S",
    "arm/64/mc16.S",
    "arm/64/mc16_sve.S",
];

/// x86 assembly (nasm): both depths, then 8-bit, then 16-bit.
const X86_ASM: &[&str] = &[
    "x86/cpuid.asm",
    "x86/msac.asm",
    "x86/pal.asm",
    "x86/refmvs.asm",
    "x86/itx_avx512.asm",
    "x86/cdef_avx2.asm",
    "x86/itx_avx2.asm",
    "x86/cdef_sse.asm",
    "x86/itx_sse.asm",
    "x86/cdef_avx512.asm",
    "x86/filmgrain_avx512.asm",
    "x86/ipred_avx512.asm",
    "x86/loopfilter_avx512.asm",
    "x86/looprestoration_avx512.asm",
    "x86/mc_avx512.asm",
    "x86/filmgrain_avx2.asm",
    "x86/ipred_avx2.asm",
    "x86/loopfilter_avx2.asm",
    "x86/looprestoration_avx2.asm",
    "x86/mc_avx2.asm",
    "x86/filmgrain_sse.asm",
    "x86/ipred_sse.asm",
    "x86/loopfilter_sse.asm",
    "x86/looprestoration_sse.asm",
    "x86/mc_sse.asm",
    "x86/cdef16_avx512.asm",
    "x86/filmgrain16_avx512.asm",
    "x86/ipred16_avx512.asm",
    "x86/itx16_avx512.asm",
    "x86/loopfilter16_avx512.asm",
    "x86/looprestoration16_avx512.asm",
    "x86/mc16_avx512.asm",
    "x86/cdef16_avx2.asm",
    "x86/filmgrain16_avx2.asm",
    "x86/ipred16_avx2.asm",
    "x86/itx16_avx2.asm",
    "x86/loopfilter16_avx2.asm",
    "x86/looprestoration16_avx2.asm",
    "x86/mc16_avx2.asm",
    "x86/cdef16_sse.asm",
    "x86/filmgrain16_sse.asm",
    "x86/ipred16_sse.asm",
    "x86/itx16_sse.asm",
    "x86/loopfilter16_sse.asm",
    "x86/looprestoration16_sse.asm",
    "x86/mc16_sse.asm",
];

/// The aarch64 extensions dav1d's assembly can use, with an instruction to try.
const ARM64_EXTENSIONS: &[(&str, &str)] = &[
    ("dotprod", "udot v0.4s, v0.16b, v0.16b"),
    ("i8mm", "usdot v0.4s, v0.16b, v0.16b"),
    ("sve", "whilelt p0.s, x0, x1"),
    ("sve2", "sqrdmulh z0.s, z0.s, z0.s"),
];

struct Target {
    arch: String,
    os: String,
    msvc: bool,
}

impl Target {
    fn from_env() -> Target {
        let var = |k: &str| env::var(k).unwrap_or_default();
        Target {
            arch: var("CARGO_CFG_TARGET_ARCH"),
            os: var("CARGO_CFG_TARGET_OS"),
            msvc: var("CARGO_CFG_TARGET_ENV") == "msvc",
        }
    }

    fn apple(&self) -> bool {
        matches!(self.os.as_str(), "macos" | "ios" | "tvos")
    }

    fn windows(&self) -> bool {
        self.os == "windows"
    }

    fn linux(&self) -> bool {
        self.os == "linux"
    }

    /// Which assembly this target builds.
    fn asm(&self) -> Asm {
        match self.arch.as_str() {
            "x86_64" => Asm::X86_64,
            "aarch64" if !self.msvc => Asm::Aarch64,
            _ => Asm::None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Asm {
    None,
    X86_64,
    Aarch64,
}

fn have(tool: &str, arg: &str) -> Option<String> {
    let o = Command::new(tool).arg(arg).output().ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest.join("../../third_party/dav1d");
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets it"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=csrc/dav1d_shim.c");
    println!(
        "cargo:rerun-if-changed={}",
        root.join("meson.build").display()
    );
    println!("cargo:rerun-if-changed={}", root.join("src").display());
    println!("cargo:rerun-if-changed={}", root.join("include").display());
    println!("cargo:rerun-if-env-changed=FIN_DAV1D_CHECK_ONLY");
    println!("cargo:rerun-if-env-changed=FIN_DAV1D_TSAN");
    // `cargo check --target <other>` type-checks without compiling C: nothing links.
    if env::var("FIN_DAV1D_CHECK_ONLY").as_deref() == Ok("1") {
        return;
    }
    let meson = fs::read_to_string(root.join("meson.build")).unwrap_or_else(|_| {
        panic!(
            "dav1d sources are missing at {}: run `git submodule update --init --depth 1 third_party/dav1d`",
            root.display()
        )
    });
    assert!(
        meson.contains(&format!("version: '{VERSION}'")),
        "third_party/dav1d is not dav1d {VERSION}: re-check build.rs's source lists against \
         its src/meson.build and update VERSION, third_party/native.json and the notices"
    );
    let t = Target::from_env();
    let asm = t.asm();
    if asm == Asm::X86_64 {
        check_nasm();
    }
    let gen_dir = out.join("gen");
    fs::create_dir_all(&gen_dir).expect("OUT_DIR is writable");
    let probes = if asm == Asm::Aarch64 {
        probe_arm64(&out)
    } else {
        Arm64Probes::default()
    };
    write(&gen_dir.join("config.h"), &config_h(&t, asm, &probes));
    write(
        &gen_dir.join("vcs_version.h"),
        &format!("/* auto-generated, do not edit */\n#define DAV1D_VERSION \"{VERSION}\"\n"),
    );
    let src = root.join("src");
    let base = || base_build(&t, &root, &gen_dir);

    // The bit-depth templates, once per depth, into the one archive.
    let mut objects = Vec::new();
    for depth in ["8", "16"] {
        let mut b = base();
        b.define("BITDEPTH", depth)
            .out_dir(out.join(format!("bd{depth}")));
        for f in TEMPLATES {
            b.file(src.join(f));
        }
        objects.extend(b.compile_intermediates());
    }
    if asm == Asm::X86_64 {
        write(&gen_dir.join("config.asm"), &config_asm(&t));
        objects.extend(assemble_x86(&t, &root, &gen_dir, &out.join("nasm")));
    }

    let mut lib = base();
    for f in SOURCES {
        lib.file(src.join(f));
    }
    match asm {
        Asm::Aarch64 => {
            lib.file(src.join("arm/cpu.c"));
            for f in ARM64_ASM {
                lib.file(src.join(f));
            }
        }
        Asm::X86_64 => {
            lib.file(src.join("x86/cpu.c"));
        }
        Asm::None => {}
    }
    if t.windows() {
        lib.file(src.join("win32/thread.c"));
    }
    lib.file(manifest.join("csrc/dav1d_shim.c"))
        .objects(&objects)
        .compile("dav1d");
    if t.linux() {
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=dl");
        println!("cargo:rustc-link-lib=m");
    }
}

fn write(path: &Path, text: &str) {
    // Unchanged files keep their mtime, so cc's objects do not rebuild for nothing.
    if fs::read_to_string(path).ok().as_deref() != Some(text) {
        fs::write(path, text).expect("OUT_DIR is writable");
    }
}

/// The compiler settings every dav1d object shares: dav1d's release build (`-O3`, `NDEBUG`,
/// its own warning set off), whatever Cargo profile is building.
fn base_build(t: &Target, root: &Path, gen_dir: &Path) -> cc::Build {
    let mut b = cc::Build::new();
    b.include(gen_dir)
        .include(root)
        .include(root.join("include/dav1d"))
        .include(root.join("include"))
        .opt_level(3)
        .debug(false)
        .define("NDEBUG", None)
        .warnings(false)
        .cargo_warnings(false);
    if t.linux() {
        b.define("_GNU_SOURCE", None);
    }
    if t.msvc {
        // dav1d's replacement for MSVC, which has no <stdatomic.h> in C mode.
        b.include(root.join("include/compat/msvc"));
        for w in ["/wd4028", "/wd4090", "/wd4996", "/wd5287"] {
            b.flag(w);
        }
    } else {
        b.flag("-std=c99").flag("-fvisibility=hidden");
        b.flag_if_supported("-fomit-frame-pointer");
        b.flag_if_supported("-ffast-math");
        if t.arch == "aarch64" {
            b.flag_if_supported("-fno-align-functions");
        }
    }
    // A ThreadSanitizer build of dav1d's C (TASKS.md: the stress run). Rust stays
    // uninstrumented; link with `-C link-arg=-fsanitize=thread`.
    if env::var("FIN_DAV1D_TSAN").as_deref() == Ok("1") {
        b.flag("-fsanitize=thread").flag("-g");
    }
    b
}

fn check_nasm() {
    let Some(v) = have("nasm", "-v") else {
        panic!(
            "nasm is needed to build dav1d's x86_64 assembly: install it (macOS `brew install nasm`, Debian/Ubuntu `apt install nasm`, Windows `winget install NASM.NASM`)"
        );
    };
    // "NASM version 2.16.03 compiled on …"
    let ver: Vec<u32> = v
        .split_whitespace()
        .nth(2)
        .unwrap_or("")
        .split('.')
        .map_while(|p| p.parse().ok())
        .collect();
    assert!(
        ver.as_slice() >= [2, 14].as_slice(),
        "dav1d needs nasm 2.14 or later; found {}",
        v.trim()
    );
}

fn assemble_x86(t: &Target, root: &Path, gen_dir: &Path, dir: &Path) -> Vec<PathBuf> {
    fs::create_dir_all(dir).expect("OUT_DIR is writable");
    let format = if t.windows() {
        "win64"
    } else if t.apple() {
        "macho64"
    } else {
        "elf64"
    };
    // nasm wants include paths to end in a separator.
    let inc = |p: &Path| format!("{}{}", p.display(), std::path::MAIN_SEPARATOR);
    let mut objs = Vec::new();
    for f in X86_ASM {
        let src = root.join("src").join(f);
        let name = Path::new(f).file_stem().expect("a file name");
        let obj = dir
            .join(name)
            .with_extension(if t.windows() { "obj" } else { "o" });
        let status = Command::new("nasm")
            .args(["-f", format])
            .arg(format!("-I{}", inc(&root.join("src"))))
            .arg(format!("-I{}", inc(gen_dir)))
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .status()
            .expect("nasm runs");
        assert!(status.success(), "nasm failed on {}", src.display());
        objs.push(obj);
    }
    objs
}

#[derive(Default)]
struct Arm64Probes {
    func: bool,
    /// `AS_ARCH_LEVEL`, when the assembler takes `.arch`.
    arch_level: Option<String>,
    /// Per `ARM64_EXTENSIONS`: (`.arch_extension` accepted, instruction assembles).
    ext: Vec<(bool, bool)>,
}

/// What the assembler accepts, as dav1d's meson.build checks it (each a file-scope `__asm__`).
fn probe_arm64(out: &Path) -> Arm64Probes {
    let dir = out.join("probe");
    fs::create_dir_all(&dir).expect("OUT_DIR is writable");
    let mut n = 0;
    let mut compiles = |asm: &str| {
        n += 1;
        let f = dir.join(format!("probe{n}.c"));
        let code = format!("__asm__ (\n{asm}\n);\n");
        fs::write(&f, code).expect("OUT_DIR is writable");
        cc::Build::new()
            .file(&f)
            .out_dir(&dir)
            .cargo_metadata(false)
            .warnings(false)
            .cargo_warnings(false)
            .try_compile_intermediates()
            .is_ok()
    };
    let func = compiles(r#"".func meson_test\n" ".endfunc\n""#);
    let mut arch_level = None;
    if compiles(r#"".arch armv8-a\n""#) {
        let mut level = "armv8-a".to_string();
        for l in ["armv8.2-a", "armv8.4-a", "armv8.6-a"] {
            if compiles(&format!(r#"".arch {l}\n""#)) {
                level = l.into();
            }
        }
        if compiles(&format!(r#"".arch {level}+crc\n""#)) {
            level.push_str("+crc");
        }
        arch_level = Some(level);
    }
    let arch = arch_level
        .as_ref()
        .map(|l| format!(r#"".arch {l}\n" "#))
        .unwrap_or_default();
    let mut ext = Vec::new();
    for (name, instr) in ARM64_EXTENSIONS {
        let directive = compiles(&format!(r#"{arch}".arch_extension {name}\n""#));
        let enable = if directive {
            format!(r#"".arch_extension {name}\n" "#)
        } else {
            String::new()
        };
        let instruction = compiles(&format!(r#"{arch}{enable}"{instr}\n""#));
        ext.push((directive, instruction));
    }
    Arm64Probes {
        func,
        arch_level,
        ext,
    }
}

/// dav1d's `config.h` for this target, with the values its meson.build would find there.
fn config_h(t: &Target, asm: Asm, p: &Arm64Probes) -> String {
    let unix = !t.windows();
    let x86 = t.arch == "x86_64";
    let arm64 = t.arch == "aarch64";
    let mut s =
        String::from("/* dav1d config.h, written by fin-dav1d-sys/build.rs */\n#pragma once\n");
    let mut def = |name: &str, on: bool| {
        writeln!(s, "#define {name} {}", on as u8).expect("a string");
    };
    def("CONFIG_8BPC", true);
    def("CONFIG_16BPC", true);
    def("HAVE_ASM", asm != Asm::None);
    def("TRIM_DSP_FUNCTIONS", true);
    // No logging: errors come back as codes; dav1d never writes to stderr.
    def("CONFIG_LOG", false);
    def("ENDIANNESS_BIG", false);
    def("ARCH_X86", x86);
    def("ARCH_X86_64", x86);
    def("ARCH_X86_32", false);
    def("ARCH_AARCH64", arm64);
    def("ARCH_ARM", false);
    for a in [
        "ARCH_PPC64LE",
        "ARCH_RISCV",
        "ARCH_RV32",
        "ARCH_RV64",
        "ARCH_LOONGARCH",
        "ARCH_LOONGARCH32",
        "ARCH_LOONGARCH64",
    ] {
        def(a, false);
    }
    def("HAVE_CLOCK_GETTIME", unix);
    def("HAVE_SIGACTION", unix);
    def("HAVE_POSIX_MEMALIGN", unix);
    def("HAVE_MEMALIGN", t.linux());
    def("HAVE_ALIGNED_ALLOC", unix);
    def("HAVE_DLSYM", t.linux());
    def("HAVE_SYS_TYPES_H", true);
    def("HAVE_UNISTD_H", unix);
    def("HAVE_IO_H", t.windows());
    def("HAVE_PTHREAD_NP_H", false);
    def("HAVE_GETAUXVAL", t.linux() && arm64);
    def("HAVE_ELF_AUX_INFO", false);
    def("HAVE_PTHREAD_GETAFFINITY_NP", t.linux());
    def("HAVE_PTHREAD_SETAFFINITY_NP", t.linux());
    def("HAVE_PTHREAD_SETNAME_NP", t.linux() || t.apple());
    def("HAVE_PTHREAD_SET_NAME_NP", false);
    def("HAVE_C11_GENERIC", !t.msvc);
    def("HAVE_AS_FUNC", p.func);
    def("HAVE_AS_ARCH_DIRECTIVE", p.arch_level.is_some());
    for (i, (name, _)) in ARM64_EXTENSIONS.iter().enumerate() {
        let (directive, instruction) = p.ext.get(i).copied().unwrap_or_default();
        let up = name.to_uppercase();
        def(&format!("HAVE_AS_ARCHEXT_{up}_DIRECTIVE"), directive);
        def(&format!("HAVE_{up}"), instruction);
    }
    if let Some(l) = &p.arch_level {
        writeln!(s, "#define AS_ARCH_LEVEL {l}").expect("a string");
    }
    // meson defines `PIC` only for arm assembly, when the compiler builds position-independent
    // code (`check_pic_code`; cc passes `-fPIC` there). nasm takes it from `config.asm`.
    if asm == Asm::Aarch64 {
        s.push_str("#define PIC 3\n");
    }
    if t.apple() {
        s.push_str("#define PREFIX 1\n");
    }
    if t.windows() {
        s.push_str(
            "#define _WIN32_WINNT 0x0601\n#define UNICODE 1\n#define _UNICODE 1\n\
             #define __USE_MINGW_ANSI_STDIO 1\n#define _CRT_DECLARE_NONSTDC_NAMES 1\n",
        );
    }
    s
}

/// dav1d's `config.asm` (nasm) for x86_64.
fn config_asm(t: &Target) -> String {
    let mut s = String::from("; dav1d config.asm, written by fin-dav1d-sys/build.rs\n");
    for (k, v) in [
        ("private_prefix", "dav1d"),
        ("ARCH_X86_64", "1"),
        ("ARCH_X86_32", "0"),
        ("PIC", "1"),
        ("FORCE_VEX_ENCODING", "0"),
        ("STACK_ALIGNMENT", "16"),
    ] {
        writeln!(s, "%define {k} {v}").expect("a string");
    }
    if t.apple() {
        s.push_str("%define PREFIX 1\n");
    }
    s
}
