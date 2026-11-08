//! The strict package reader and the reproducible packer (spec §6.4, 8A Decision 7).

mod support;

use std::path::Path;

use fin_plugin_api::Manifest;
use fin_registry::package::{Limits, PackageError, Unpacked, pack, tree_hash, unpack};
use support::hostile::{E, zip};
use support::{content_manifest, listing, manifest_with_exec, write_tree};

fn exec_of(m: &Manifest) -> Vec<String> {
    m.executables()
}

/// Unpacks `bytes` into a fresh `out/`; on failure checks that nothing is left anywhere.
fn try_unpack(
    bytes: &[u8],
    limits: &Limits,
) -> Result<(tempfile::TempDir, Unpacked), PackageError> {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("p.finpkg");
    std::fs::write(&archive, bytes).unwrap();
    let dest = dir.path().join("out");
    match unpack(&archive, &dest, exec_of, limits) {
        Ok(u) => Ok((dir, u)),
        Err(e) => {
            assert!(!dest.exists(), "{e}: dest left behind");
            assert_eq!(listing(dir.path()), ["p.finpkg"], "{e}: wrote outside dest");
            Err(e)
        }
    }
}

fn with_manifest(mut entries: Vec<E>) -> Vec<E> {
    entries.insert(0, E::file("finisher-plugin.json", &content_manifest()));
    entries
}

fn refused(entries: Vec<E>, limits: &Limits) -> PackageError {
    match try_unpack(&zip(&entries), limits) {
        Ok(_) => panic!("accepted"),
        Err(e) => e,
    }
}

#[test]
fn hostile_archive_corpus() {
    let d = Limits::default();
    let unsafe_cases: Vec<(&str, Vec<E>, &str)> = vec![
        ("dot-dot", vec![E::file("../evil", b"x")], "dotComponent"),
        (
            "dot-dot inside",
            vec![E::file("a/../../evil", b"x")],
            "dotComponent",
        ),
        ("dot", vec![E::file("a/./b", b"x")], "dotComponent"),
        ("absolute", vec![E::file("/etc/evil", b"x")], "absolute"),
        (
            "drive letter",
            vec![E::file("C:/evil", b"x")],
            "windowsChar",
        ),
        (
            "drive relative",
            vec![E::file("C:evil", b"x")],
            "windowsChar",
        ),
        ("backslash", vec![E::file("a\\b", b"x")], "backslash"),
        ("nul", vec![E::file("a\0b", b"x")], "nul"),
        ("control", vec![E::file("a\x01b", b"x")], "control"),
        (
            "empty component",
            vec![E::file("a//b", b"x")],
            "emptyComponent",
        ),
        ("windows char", vec![E::file("a<b", b"x")], "windowsChar"),
        ("stream", vec![E::file("a.txt:zone", b"x")], "windowsChar"),
        (
            "symlink",
            vec![E::file("link", b"/etc/passwd").mode(0o120_777)],
            "symlink",
        ),
        (
            "hard link (a second entry of one path)",
            vec![E::file("a.txt", b"x"), E::file("a.txt", b"y")],
            "duplicate",
        ),
        (
            "char device",
            vec![E::file("dev", b"").mode(0o020_644)],
            "notRegular",
        ),
        (
            "block device",
            vec![E::file("dev", b"").mode(0o060_644)],
            "notRegular",
        ),
        (
            "fifo",
            vec![E::file("fifo", b"").mode(0o010_644)],
            "notRegular",
        ),
        (
            "socket",
            vec![E::file("sock", b"").mode(0o140_644)],
            "notRegular",
        ),
        (
            "dir mode on a file",
            vec![E::file("a", b"x").mode(0o040_755)],
            "notRegular",
        ),
        ("CON", vec![E::file("CON", b"x")], "reservedName"),
        (
            "8.3 alias of another entry",
            vec![
                E::file("styles/something.json", b"x"),
                E::file("styles/SOMETH~1.JSO", b"y"),
            ],
            "shortName",
        ),
        (
            "8.3 directory",
            vec![E::file("PROGRA~1/a", b"x")],
            "shortName",
        ),
        ("aux.txt", vec![E::file("aux.txt", b"x")], "reservedName"),
        ("COM1", vec![E::file("COM1", b"x")], "reservedName"),
        (
            "lpt9.log in a dir",
            vec![E::file("logs/lpt9.log", b"x")],
            "reservedName",
        ),
        (
            "con with a space",
            vec![E::file("con .txt", b"x")],
            "reservedName",
        ),
        (
            "COM superscript",
            vec![E::file("COM\u{b9}", b"x")],
            "reservedName",
        ),
        (
            "trailing dot",
            vec![E::file("trailing.", b"x")],
            "trailingDotOrSpace",
        ),
        (
            "trailing space",
            vec![E::file("trailing ", b"x")],
            "trailingDotOrSpace",
        ),
        (
            "dir with a trailing dot",
            vec![E::file("dir./x", b"x")],
            "trailingDotOrSpace",
        ),
        (
            "case-folded duplicates",
            vec![E::file("Readme.md", b"x"), E::file("README.md", b"y")],
            "caseFoldDuplicate",
        ),
        (
            "case-folded directories",
            vec![E::file("Lib/a", b"x"), E::file("lib/b", b"y")],
            "caseFoldDuplicate",
        ),
        (
            "sharp s",
            vec![
                E::file("stra\u{df}e.txt", b"x"),
                E::file("STRASSE.txt", b"y"),
            ],
            "caseFoldDuplicate",
        ),
        (
            "NFC then NFD",
            vec![
                E::file("caf\u{e9}.txt", b"x"),
                E::file("cafe\u{301}.txt", b"y"),
            ],
            "notNfc",
        ),
        (
            "NFD alone",
            vec![E::file("cafe\u{301}.txt", b"y")],
            "notNfc",
        ),
        ("not UTF-8", vec![E::raw(b"\xffa", b"x")], "notUtf8"),
        (
            "UTF-8 without the flag",
            vec![E::file("caf\u{e9}.txt", b"x").utf8_flag(false)],
            "notUtf8",
        ),
        (
            "encrypted",
            vec![E::file("a", b"x").encrypted()],
            "encrypted",
        ),
        (
            "file used as a directory",
            vec![E::file("a", b"x"), E::file("a/b", b"y")],
            "fileAsDirectory",
        ),
        (
            "directory then a file of its name",
            vec![E::file("a/b", b"x"), E::file("a", b"y")],
            "fileAsDirectory",
        ),
        (
            "component over 255 bytes",
            vec![E::file(&"c".repeat(256), b"x")],
            "componentTooLong",
        ),
        (
            "path over 1,024 bytes",
            vec![E::file(&vec!["d".repeat(200); 6].join("/"), b"x")],
            "pathTooLong",
        ),
        (
            "directory with data",
            vec![
                E::dir("d"),
                E {
                    data: b"x".to_vec(),
                    ..E::dir("e")
                },
            ],
            "dirWithData",
        ),
        (
            "ratio over 200:1",
            vec![E::file("zeros", &vec![0u8; 1 << 20]).deflated()],
            "ratio",
        ),
        (
            "declared size smaller than written",
            vec![E::file("a", &[7u8; 1000]).deflated().declared(500)],
            // `zip` stops at the declared size itself; the reader's own count is the second line.
            "sizeMismatch|corrupt",
        ),
        (
            "declared size larger than written",
            vec![E::file("a", &[7u8; 1000]).deflated().declared(1100)],
            "sizeMismatch|corrupt",
        ),
        (
            "stored sizes differ",
            vec![E::file("a", b"abc").declared(2)],
            "sizeMismatch",
        ),
    ];
    for (what, entries, reason) in unsafe_cases {
        let Err(e) = try_unpack(&zip(&with_manifest(entries)), &d) else {
            panic!("{what}: accepted");
        };
        assert_eq!(e.code, "PACKAGE_UNSAFE", "{what}: {e}");
        let got = e.reason().unwrap_or_default();
        assert!(
            reason.split('|').any(|r| r == got),
            "{what}: {got} is not {reason}: {e}"
        );
    }

    // More than 10,000 entries.
    let many: Vec<E> = (0..10_001)
        .map(|i| E::file(&format!("f{i}"), b""))
        .collect();
    let e = refused(with_manifest(many), &d);
    assert_eq!(
        (e.code, e.reason()),
        ("PACKAGE_TOO_LARGE", Some("entries")),
        "{e}"
    );
    // Over 512 MiB declared, each entry within the ratio (never decompressed: refused first).
    let fake: Vec<u8> = (0..1_700_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let big = vec![
        E::file("big1", b"").fake(fake.clone(), 300 << 20),
        E::file("big2", b"").fake(fake, 300 << 20),
    ];
    let e = refused(with_manifest(big), &d);
    assert_eq!(
        (e.code, e.reason()),
        ("PACKAGE_TOO_LARGE", Some("unpackedBytes")),
        "{e}"
    );
    let small = Limits { unpacked: 100, ..d };
    let e = refused(with_manifest(vec![E::file("a", &[1u8; 200])]), &small);
    assert_eq!(e.reason(), Some("unpackedBytes"));

    // No root manifest (one in a subdirectory does not count).
    let e = refused(
        vec![E::file("sub/finisher-plugin.json", &content_manifest())],
        &d,
    );
    assert_eq!(
        (e.code, e.reason()),
        ("PLUGIN_MANIFEST_INVALID", Some("missing"))
    );
    let e = refused(vec![E::file("finisher-plugin.json", b"{\"id\": 1}")], &d);
    assert_eq!(
        (e.code, e.reason()),
        ("PLUGIN_MANIFEST_INVALID", Some("schema"))
    );
    let huge = vec![b' '; fin_plugin_api::MAX_MANIFEST_BYTES + 1];
    let e = refused(vec![E::file("finisher-plugin.json", &huge)], &d);
    assert_eq!(e.code, "MANIFEST_TOO_LARGE");
    // A corrupt archive.
    let e = try_unpack(b"PK\x03\x04 not a zip", &d).unwrap_err();
    assert_eq!(e.reason(), Some("corrupt"));

    // A well-formed package of the same shapes passes, NFC non-ASCII and directories included.
    let ok = with_manifest(vec![
        E::dir("styles"),
        E::file("styles/bold.json", b"{}").deflated(),
        E::file("caf\u{e9}/notes.txt", b"hi"),
        E::file("styles/.hidden", b""),
    ]);
    let (_dir, u) = try_unpack(&zip(&ok), &d).unwrap();
    assert_eq!(
        u.files.keys().collect::<Vec<_>>(),
        [
            "caf\u{e9}/notes.txt",
            "finisher-plugin.json",
            "styles/.hidden",
            "styles/bold.json"
        ]
    );
    assert_eq!(
        listing(&u.root),
        ["caf\u{e9}", "finisher-plugin.json", "styles"]
    );
    assert_eq!(u.tree_hash, tree_hash(&u.root).unwrap());
}

#[test]
fn unpack_sets_modes_only_for_executables() {
    let manifest =
        manifest_with_exec(&[("linux-x64", "bin/tool"), ("darwin-arm64", "bin/tool-mac")]);
    let entries = vec![
        E::file("finisher-plugin.json", &manifest),
        // The archive's own modes are ignored: not executable here, setuid and 0777 there.
        E::file("bin/tool", b"\x7fELF").mode(0o100_600),
        E::file("bin/tool-mac", b"\xcf\xfa").mode(0o100_644),
        E::file("data/x.bin", b"x").mode(0o104_777),
        E::dir("lib").mode(0o040_700),
        E::file("readme", b"r").mode(0o100_755),
    ];
    let (_dir, u) = try_unpack(&zip(&entries), &Limits::default()).unwrap();
    let exec: Vec<(&str, bool)> = u
        .files
        .iter()
        .map(|(p, m)| (p.as_str(), m.executable))
        .collect();
    assert_eq!(
        exec,
        [
            ("bin/tool", true),
            ("bin/tool-mac", true),
            ("data/x.bin", false),
            ("finisher-plugin.json", false),
            ("readme", false),
        ]
    );
    assert_eq!(u.manifest, manifest);
    assert_eq!(u.tree_hash, tree_hash(&u.root).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| {
            std::fs::metadata(u.root.join(p))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777
        };
        assert_eq!(mode("bin/tool"), 0o755);
        assert_eq!(mode("bin/tool-mac"), 0o755);
        assert_eq!(mode("data/x.bin"), 0o644);
        assert_eq!(mode("readme"), 0o644);
        assert_eq!(mode("finisher-plugin.json"), 0o644);
        assert_eq!(mode("lib"), 0o755);
        assert_eq!(mode("bin"), 0o755);
        assert_eq!(mode(""), 0o755);
    }
}

fn sample_tree(root: &Path, order_reversed: bool) {
    let manifest = manifest_with_exec(&[("linux-x64", "bin/tool")]);
    let mut files: Vec<(&str, &[u8])> = vec![
        ("finisher-plugin.json", &manifest),
        ("bin/tool", b"\x7fELF tool"),
        ("styles/a.json", b"{\"a\": 1}"),
        ("styles/b.json", b"{\"b\": 2}"),
        ("Z-upper.txt", b"upper first in byte order"),
        ("caf\u{e9}.txt", b"non-ascii"),
        ("empty", b""),
    ];
    if order_reversed {
        files.reverse();
    }
    write_tree(root, &files);
}

#[test]
fn pack_is_reproducible() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    sample_tree(a.path(), false);
    sample_tree(b.path(), true);
    // Different mtimes and file-system modes must not matter.
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    std::fs::File::options()
        .write(true)
        .open(b.path().join("styles/a.json"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    std::fs::create_dir(b.path().join("empty-dir")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let set =
            |p: &Path, m| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
        set(&b.path().join("styles/b.json"), 0o700);
        set(&a.path().join("bin/tool"), 0o755);
    }
    let exec = vec!["bin/tool".to_string()];
    let out = tempfile::tempdir().unwrap();
    let pa = pack(a.path(), &exec, &out.path().join("a.finpkg")).unwrap();
    let pb = pack(b.path(), &exec, &out.path().join("b.finpkg")).unwrap();
    let bytes_a = std::fs::read(out.path().join("a.finpkg")).unwrap();
    let bytes_b = std::fs::read(out.path().join("b.finpkg")).unwrap();
    assert_eq!(bytes_a, bytes_b);
    assert_eq!(pa, pb);
    assert_eq!(pa.entries, 7);
    assert_eq!(pa.bytes, bytes_a.len() as u64);
    assert_eq!(
        listing(out.path()),
        ["a.finpkg", "b.finpkg"],
        "no temp file left"
    );

    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes_a)).unwrap();
    let mut names = Vec::new();
    for i in 0..z.len() {
        let f = z.by_index(i).unwrap();
        names.push(f.name().unwrap().into_owned());
        assert_eq!(
            f.last_modified(),
            Some(zip::DateTime::default()),
            "{}",
            f.name().unwrap()
        );
        assert_eq!(f.system(), zip::System::Unix);
        assert_eq!(f.compression(), zip::CompressionMethod::Deflated);
        assert!(
            f.extra_data().is_none_or(|x| x.is_empty()),
            "extra field on {}",
            f.name().unwrap()
        );
        let want = if f.name().unwrap() == "bin/tool" {
            0o755
        } else {
            0o644
        };
        assert_eq!(f.unix_mode().map(|m| m & 0o777), Some(want));
    }
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);

    // Unpacking gives the tree the packer described, and the source tree's own hash.
    let dir = tempfile::tempdir().unwrap();
    let u = unpack(
        &out.path().join("a.finpkg"),
        &dir.path().join("u"),
        exec_of,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(u.tree_hash, pa.tree_hash);
    assert_eq!(tree_hash(a.path()).unwrap(), pa.tree_hash);

    // The packer refuses what the reader would.
    let bad = tempfile::tempdir().unwrap();
    sample_tree(bad.path(), false);
    write_tree(bad.path(), &[("aux.txt", b"x")]);
    let e = pack(bad.path(), &exec, &out.path().join("bad.finpkg")).unwrap_err();
    assert_eq!(e.reason(), Some("reservedName"));
    assert!(!out.path().join("bad.finpkg").exists() && !out.path().join("bad.finpkg.tmp").exists());
    let e = pack(a.path(), &["bin/nope".into()], &out.path().join("c.finpkg")).unwrap_err();
    assert_eq!(e.reason(), Some("executableMissing"));
    std::fs::remove_file(bad.path().join("finisher-plugin.json")).unwrap();
    std::fs::remove_file(bad.path().join("aux.txt")).unwrap();
    let e = pack(bad.path(), &exec, &out.path().join("d.finpkg")).unwrap_err();
    assert_eq!(
        (e.code, e.reason()),
        ("PLUGIN_MANIFEST_INVALID", Some("missing"))
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", a.path().join("link")).unwrap();
        let e = pack(a.path(), &exec, &out.path().join("e.finpkg")).unwrap_err();
        assert_eq!(e.reason(), Some("symlink"));
        assert_eq!(tree_hash(a.path()).unwrap_err().reason(), Some("symlink"));
    }
}

#[test]
fn tree_hash_changes_on_any_byte_or_mode() {
    let t = tempfile::tempdir().unwrap();
    sample_tree(t.path(), false);
    let h0 = tree_hash(t.path()).unwrap();
    assert_eq!(h0.len(), 64);
    let p = t.path().join("styles/a.json");
    let orig = std::fs::read(&p).unwrap();
    let mut changed = orig.clone();
    changed[3] ^= 1;
    std::fs::write(&p, &changed).unwrap();
    assert_ne!(tree_hash(t.path()).unwrap(), h0, "a byte");
    std::fs::write(&p, &orig).unwrap();
    assert_eq!(tree_hash(t.path()).unwrap(), h0, "restored");
    std::fs::write(t.path().join("empty"), b"x").unwrap();
    assert_ne!(tree_hash(t.path()).unwrap(), h0, "an empty file filled");
    std::fs::write(t.path().join("empty"), b"").unwrap();
    std::fs::rename(
        t.path().join("styles/b.json"),
        t.path().join("styles/c.json"),
    )
    .unwrap();
    assert_ne!(tree_hash(t.path()).unwrap(), h0, "a rename");
    std::fs::rename(
        t.path().join("styles/c.json"),
        t.path().join("styles/b.json"),
    )
    .unwrap();
    std::fs::create_dir(t.path().join("nothing-here")).unwrap();
    assert_eq!(
        tree_hash(t.path()).unwrap(),
        h0,
        "an empty directory adds nothing"
    );
    write_tree(t.path(), &[("extra", b"")]);
    assert_ne!(tree_hash(t.path()).unwrap(), h0, "an added empty file");
    std::fs::remove_file(t.path().join("extra")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(tree_hash(t.path()).unwrap(), h0, "the execute bit");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            tree_hash(t.path()).unwrap(),
            h0,
            "other mode bits do not count"
        );
    }
}
