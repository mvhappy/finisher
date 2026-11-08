//! Exclusion rules and globs (plan 5D Task 1, Decision 16).

use fin_library::exclude::{code, conflict_copy, default_rule};
use fin_library::glob::{Glob, GlobError};
use fin_library::{EntryType, ExcludeSpec, Excludes, RawEntry, RawRel, listing};

fn file(n: &str) -> RawEntry {
    RawEntry::new(n, EntryType::File, 1, 1)
}

fn dir(n: &str) -> RawEntry {
    RawEntry::new(n, EntryType::Dir, 0, 1)
}

#[test]
fn default_excludes() {
    for (name, is_dir, want) in [
        (".DS_Store", false, Some(code::DOTFILE)),
        (".nfs0000000001b2", false, Some(code::DOTFILE)),
        (".fuse_hidden0000001a", false, Some(code::DOTFILE)),
        (".git", true, Some(code::DOTFILE)),
        ("Thumbs.db", false, Some(code::OS_JUNK)),
        ("THUMBS.DB", false, Some(code::OS_JUNK)),
        ("desktop.ini", false, Some(code::OS_JUNK)),
        ("@eaDir", true, Some(code::OS_JUNK)),
        ("#recycle", true, Some(code::OS_JUNK)),
        ("$RECYCLE.BIN", true, Some(code::OS_JUNK)),
        ("System Volume Information", true, Some(code::OS_JUNK)),
        ("~$script.docx", false, Some(code::SYNC_TEMP)),
        ("~syncthing~clip.mov.tmp", false, Some(code::SYNC_TEMP)),
        ("clip.mov.!sync", false, Some(code::SYNC_TEMP)),
        ("clip.mov.crdownload", false, Some(code::SYNC_TEMP)),
        ("clip.mov.partial", false, Some(code::SYNC_TEMP)),
        ("render.TMP", false, Some(code::SYNC_TEMP)),
        // Temp patterns are for files; a directory called `x.tmp` is scanned.
        ("x.tmp", true, None),
        ("clip.mov", false, None),
        ("Take 2.mov", false, None),
        ("clip-final.mov", false, None),
        ("scan (1).png", false, None),
        ("thumbs.db.mov", false, None),
    ] {
        assert_eq!(default_rule(name, is_dir), want, "{name}");
    }
}

#[test]
fn conflict_copies_excluded() {
    for name in [
        "clip (Bob's conflicted copy 2026-11-04).mov",
        "clip (Case Conflict 1).mov",
        "clip (Selective Sync Conflict).mov",
        "clip (conflicted copy 2026-11-07 101500).mov",
        "clip.sync-conflict-20261107-101500-ABCDEFG.mov",
        "clip_conflict-20261107-101500.mov",
        "clip [Conflict].mov",
        "CLIP [CONFLICT].MOV",
    ] {
        assert!(conflict_copy(name), "{name}");
        assert_eq!(
            default_rule(name, false),
            Some(code::CONFLICT_COPY),
            "{name}"
        );
    }
    for name in [
        "clip (copy).mov",
        "my_conflict-notes.txt",
        "clip.sync-conflict-notes.mov",
        "Take 2.mov",
        "clip (2).mov",
        "clip-laptop.mov",
        "conflicted copy.mov",
    ] {
        assert!(!conflict_copy(name), "{name}");
    }
}

#[test]
fn automounts_excluded() {
    let mut auto = dir("net");
    auto.automount = true;
    let mut nested = dir("disk2");
    nested.mount_root = true;
    let out = listing::entries(
        &RawRel::root(),
        vec![auto, nested],
        &Excludes::defaults(true),
    );
    assert_eq!(out[0].name, b"disk2");
    assert_eq!(
        (out[0].mount_root, out[0].excluded.as_deref()),
        (true, None)
    );
    assert_eq!(out[1].name, b"net");
    assert_eq!(
        (out[1].automount, out[1].excluded.as_deref()),
        (true, Some(code::AUTOMOUNT))
    );
}

#[test]
fn project_folders_keep_only_sources() {
    let ex = Excludes::defaults(true);
    let raw = vec![
        file("finisher.project.json"),
        dir("sources"),
        dir(".finisher"),
        dir("branches"),
        file("notes.txt"),
    ];
    let out = listing::entries(&RawRel::root().join(b"Film"), raw, &ex);
    let got: Vec<(&[u8], Option<&str>)> = out
        .iter()
        .map(|e| (e.name.as_slice(), e.excluded.as_deref()))
        .collect();
    let p = Some(code::PROJECT_INTERNAL);
    assert_eq!(
        got,
        vec![
            (&b".finisher"[..], p),
            (b"branches", p),
            (b"finisher.project.json", p),
            (b"notes.txt", p),
            (b"sources", None),
        ]
    );
    // A `sources` file is not the sources folder.
    let out = listing::entries(
        &RawRel::root(),
        vec![file("finisher.project.json"), file("sources")],
        &ex,
    );
    assert!(out.iter().all(|e| e.excluded.as_deref() == p));
    // Without the project file, nothing special.
    let out = listing::entries(&RawRel::root(), vec![dir("sources"), dir("branches")], &ex);
    assert!(out.iter().all(|e| e.excluded.is_none()));
}

#[test]
fn glob_matrix() {
    let cases: &[(&str, &str, bool, bool)] = &[
        // Unanchored patterns match the name at any depth.
        ("*.tmp", "x.tmp", true, true),
        ("*.tmp", "a/b/x.tmp", true, true),
        ("*.tmp", "a/x.tmp/y", true, false),
        ("*.MOV", "media/x.mov", false, true),
        ("*.MOV", "media/x.mov", true, false),
        // Anchored patterns match the whole key.
        ("/Media/*.mov", "Media/a.mov", true, true),
        ("/Media/*.mov", "media/a.mov", false, true),
        ("Media/*.mov", "Media/sub/a.mov", true, false),
        ("Media/*.mov", "Other/Media/a.mov", true, false),
        ("Media/**", "Media/a/b/c.mov", true, true),
        ("Media/**", "Media", true, false),
        ("a/**/b", "a/b", true, true),
        ("a/**/b", "a/x/b", true, true),
        ("a/**/b", "a/x/y/b", true, true),
        ("a/**/b", "a/xb", true, false),
        ("**/cache", "cache", true, true),
        ("**/cache", "x/y/cache", true, true),
        ("**/cache", "x/cache/y", true, false),
        ("raw/**/*.dng", "raw/2026/11/a.dng", true, true),
        ("raw/**.dng", "raw/2026/a.dng", true, true),
        ("a/*", "a/b/c", true, false),
        // ? and classes never match `/`.
        ("?.mov", "a.mov", true, true),
        ("?.mov", "ab.mov", true, false),
        ("a?b", "a/b", true, false),
        ("a/[bc]/d", "a/c/d", true, true),
        ("[abc].mov", "b.mov", true, true),
        ("[!abc].mov", "b.mov", true, false),
        ("[^abc].mov", "d.mov", true, true),
        ("[a-c]x", "bx", true, true),
        ("[a-c]x", "dx", true, false),
        ("[A-C]x", "bx", false, true),
        ("[]]x", "]x", true, true),
        ("[a-]x", "-x", true, true),
        ("*", "anything", true, true),
        ("clip*", "clip", true, true),
        ("*é*", "café.mov", true, true),
    ];
    for &(p, key, cs, want) in cases {
        assert_eq!(
            Glob::new(p, cs).unwrap().matches(key),
            want,
            "{p} ~ {key} (cs={cs})"
        );
    }
    assert_eq!(Glob::new("", true), Err(GlobError::Empty));
    assert_eq!(Glob::new("/", true), Err(GlobError::Empty));
    assert_eq!(Glob::new("[abc", true), Err(GlobError::UnclosedClass));
    assert_eq!(Glob::new("[]", true), Err(GlobError::UnclosedClass));
}

#[test]
fn library_globs_exclude_and_include() {
    let spec = ExcludeSpec {
        exclude: vec!["raw/**".into(), "*.xmp".into()],
        include: Some(vec!["*.mov".into(), "*.wav".into()]),
        case_sensitive: false,
    };
    let v = serde_json::to_value(&spec).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"exclude": ["raw/**", "*.xmp"], "include": ["*.mov", "*.wav"], "caseSensitive": false})
    );
    assert_eq!(serde_json::from_value::<ExcludeSpec>(v).unwrap(), spec);
    let ex = Excludes::new(&spec).unwrap();
    assert!(!ex.case_sensitive());
    assert_eq!(
        ex.check("raw/a.mov", "a.mov", false),
        Some(code::EXCLUDE_GLOB)
    );
    assert_eq!(ex.check("raw", "raw", true), None);
    assert_eq!(ex.check("raw/x", "x", true), Some(code::EXCLUDE_GLOB));
    assert_eq!(
        ex.check("a/b.xmp", "b.xmp", false),
        Some(code::EXCLUDE_GLOB)
    );
    assert_eq!(
        ex.check("a/b.txt", "b.txt", false),
        Some(code::NOT_INCLUDED)
    );
    assert_eq!(ex.check("a/b.mov", "B.MOV", false), None);
    // Directories are descended even when include is set.
    assert_eq!(ex.check("docs", "docs", true), None);
    // Defaults come first.
    assert_eq!(ex.check(".x.mov", ".x.mov", false), Some(code::DOTFILE));
    // Keys from a listing are folded like the patterns.
    let out = listing::entries(
        &RawRel::root().join(b"Shoot"),
        vec![file("A.MOV"), file("notes.TXT"), dir("RAW")],
        &ex,
    );
    let got: Vec<(&str, Option<&str>)> = out
        .iter()
        .map(|e| (e.rel_key.as_str(), e.excluded.as_deref()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("shoot/a.mov", None),
            ("shoot/raw", None),
            ("shoot/notes.txt", Some(code::NOT_INCLUDED)),
        ]
    );
    assert!(
        Excludes::new(&ExcludeSpec {
            exclude: vec!["[".into()],
            ..ExcludeSpec::default()
        })
        .is_err()
    );
}
