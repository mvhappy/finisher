#![cfg(feature = "fetch")]
//! Plugin packs' font catalogs merged into the built-in one (8A Task 12a): new ids only, the
//! merged catalog validated whole, the first pack catalog that does not merge (Task 12c), and a
//! fetcher that follows a replaced catalog.

mod support;

use std::sync::Arc;

use fin_fonts::{Fetcher, FontCache, FontCatalog};
use serde_json::Value;
use support::{COMMIT, File as CatFile, Route, Server, catalog, catalog_json, font, opts};

fn pack(entry: &'static str, seed: u8) -> (FontCatalog, CatFile) {
    let file = CatFile::plain(
        entry,
        "regular",
        font(seed, 4096),
        vec![format!(
            "https://raw.githubusercontent.com/example/fonts/{COMMIT}/{entry}.ttf"
        )],
    );
    let cat = FontCatalog::from_json(&catalog_json(std::slice::from_ref(&file)).to_string())
        .expect("a pack catalog validates");
    (cat, file)
}

fn with_fallback(mut c: FontCatalog, ids: &[&str]) -> FontCatalog {
    c.fallback = ids.iter().map(|s| (*s).to_string()).collect();
    c.validate().unwrap();
    c
}

#[test]
fn merge_appends_new_entries_after_the_built_in_ones() {
    let base = FontCatalog::builtin();
    let (a, file_a) = pack("pack-a", 1);
    let (b, _) = pack("pack-b", 2);
    let (a, b) = (with_fallback(a, &["pack-a"]), with_fallback(b, &["pack-b"]));
    let merged = base.merged(&[a, b]).unwrap();

    let ids: Vec<&str> = merged.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        &ids[..base.entries.len()],
        &base
            .entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>()[..]
    );
    assert_eq!(&ids[base.entries.len()..], ["pack-a", "pack-b"]);
    // Pins name the version they were planned at: a pack does not move it.
    assert_eq!(merged.catalog_version, base.catalog_version);
    // The built-in fallbacks keep their order; a pack's go after them.
    assert_eq!(&merged.fallback[..base.fallback.len()], &base.fallback[..]);
    assert_eq!(
        &merged.fallback[base.fallback.len()..],
        ["pack-a", "pack-b"]
    );
    // A pack's file is found like a built-in one, and nothing built in moved.
    assert_eq!(merged.by_sha(&file_a.sha()).unwrap().0.id, "pack-a");
    assert!(merged.find_face("Test pack-a", 400, false).is_some());
    for (e, s) in base.styles() {
        assert_eq!(merged.by_sha(&s.file.sha256).unwrap().0.id, e.id);
    }
}

#[test]
fn merge_refuses_taken_ids() {
    let base = FontCatalog::builtin();
    // An id the built-in catalog has.
    let (clash, _) = pack("inter", 3);
    let err = base.merged(&[clash]).unwrap_err();
    assert_eq!(err.code, "FONT_CATALOG_INVALID");
    assert_eq!(err.details["rule"], "catalogIdTaken");
    assert_eq!(err.details["id"], "inter");
    assert_eq!(err.details["path"], "/extra/0/entries/0/id");

    // An id an earlier pack in the same merge has.
    let (a, _) = pack("shared", 4);
    let (b, _) = pack("shared", 5);
    let err = base.merged(&[a.clone(), b]).unwrap_err();
    assert_eq!(err.details["rule"], "catalogIdTaken");
    assert_eq!(err.details["path"], "/extra/1/entries/0/id");

    // The same pack twice is the same clash.
    assert!(base.merged(&[a.clone(), a]).is_err());
}

/// Review H1 (Tasks 10 and 12): `find_face` matches face families before entry families, and
/// the built-in `inter` has `Inter Variable` faces, so a pack face called `Inter` would win
/// new pins for the style family `Inter`.
#[test]
fn merge_refuses_a_family_the_catalog_already_has() {
    let base = FontCatalog::builtin();
    assert!(base.entry("inter").is_some_and(|e| e.family == "Inter"));
    let taken = |c: FontCatalog| {
        let err = base.merged(&[c]).unwrap_err();
        assert_eq!(err.code, "FONT_CATALOG_INVALID");
        assert_eq!(err.details["rule"], "catalogIdTaken");
        err
    };

    // A pack face named like a built-in entry family (the shadowing attack) …
    let (mut shadow, _) = pack("pack-shadow", 11);
    shadow.entries[0].styles[0].file.faces[0].family = "Inter".into();
    let err = taken(shadow);
    assert_eq!(err.details["family"], "Inter");
    assert_eq!(
        err.details["path"],
        "/extra/0/entries/0/styles/0/file/faces/0/family"
    );
    // … a pack entry family named like one (any case) …
    let (mut entry, _) = pack("pack-entry", 12);
    entry.entries[0].family = "inter".into();
    assert_eq!(taken(entry).details["path"], "/extra/0/entries/0/family");
    // … and either named like a built-in face family.
    let face = &base.entry("inter").unwrap().styles[0].file.faces[0].family;
    let (mut other, _) = pack("pack-face", 13);
    other.entries[0].family = face.clone();
    taken(other);

    // Packs do not shadow each other's families either, faces or entries.
    let (mut a, _) = pack("pack-a", 14);
    a.entries[0].styles[0].file.faces[0].family = "Shared Family".into();
    let (mut b, _) = pack("pack-b", 15);
    b.entries[0].family = "Shared Family".into();
    let err = base.merged(&[a.clone(), b]).unwrap_err();
    assert_eq!(err.details["path"], "/extra/1/entries/0/family");

    // The plan and validate rule is the same one, naming the catalog that fails.
    let (mut late, _) = pack("pack-late", 16);
    late.entries[0].styles[0].file.faces[0].family = "Inter Variable".into();
    let (ok, _) = pack("pack-ok", 17);
    let (k, err) = base.first_unmergeable(&[ok, late]).unwrap();
    assert_eq!(
        (k, err.details["rule"].clone()),
        (1, "catalogIdTaken".into())
    );
}

#[test]
fn merge_refuses_the_same_bytes_under_two_ids() {
    let base = FontCatalog::builtin();
    let (a, _) = pack("pack-a", 7);
    let (mut b, _) = pack("pack-b", 7); // the same seed: the same SHA-256
    b.entries[0].styles[0].file.faces[0].family = "Another".into();
    let err = base.merged(&[a, b]).unwrap_err();
    assert_eq!(err.code, "FONT_CATALOG_INVALID");
    assert_eq!(err.details["rule"], "unique");
}

/// 8A Task 12c: the plan's rule, one catalog after the other, naming the first that fails.
#[test]
fn first_unmergeable_names_the_catalog_that_fails() {
    let base = FontCatalog::builtin();
    let (a, _) = pack("pack-a", 7);
    let (b, _) = pack("pack-b", 8);
    assert!(base.first_unmergeable(&[a.clone(), b.clone()]).is_none());
    assert!(base.first_unmergeable(&[]).is_none());
    // The same file under another id, after a catalog that merges.
    let (mut twin, _) = pack("pack-twin", 7);
    twin.entries[0].styles[0].file.faces[0].family = "Another".into();
    let (k, err) = base
        .first_unmergeable(&[b.clone(), a.clone(), twin])
        .unwrap();
    assert_eq!(k, 2);
    assert_eq!(err.details["rule"], "unique");
    // A built-in id.
    let (taken, _) = pack("noto-sans", 9);
    let (k, err) = base.first_unmergeable(&[taken, b]).unwrap();
    assert_eq!(
        (k, err.details["rule"].clone()),
        (0, "catalogIdTaken".into())
    );
    assert_eq!(err.details["id"], "noto-sans");
}

#[test]
fn merge_keeps_each_packs_own_rules() {
    // A pack catalog is parsed by `from_json`, so the built-in catalog's licence, https and
    // source-host rules hold for it: nothing reaches `merged` that broke them.
    let (_, file) = pack("pack-a", 1);
    let mut doc: Value = catalog_json(&[file]);
    doc["entries"][0]["licence"]["spdx"] = "CC-BY-NC-4.0".into();
    let err = FontCatalog::from_json(&doc.to_string()).unwrap_err();
    assert_eq!(err.details["rule"], "licence");

    let mut doc: Value = catalog_json(&[pack("pack-a", 1).1]);
    doc["entries"][0]["styles"][0]["sources"][0]["url"] = "https://example.com/latest/a.ttf".into();
    let err = FontCatalog::from_json(&doc.to_string()).unwrap_err();
    assert_eq!(err.details["rule"], "immutableUrl");
}

#[test]
fn fetcher_follows_a_replaced_catalog() {
    let srv = Server::start();
    let bytes = font(9, 8192);
    let path = format!("/{COMMIT}/late.ttf");
    srv.route(&path, Route::new(bytes.clone()));
    let file = CatFile::plain("late", "regular", bytes, vec![srv.url(&path)]);
    let sha = file.sha();

    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(FontCache::open(dir.path()).unwrap());
    let fetcher = Fetcher::new(
        cache.clone(),
        Arc::new(FontCatalog::builtin().clone()),
        opts(),
    );
    // Not in the catalog yet: nothing is fetched.
    assert_eq!(
        fetcher.fetch_now(&sha).unwrap_err().code,
        "FONT_NOT_IN_CATALOG"
    );
    fetcher.request(&sha);
    assert!(srv.hits(&path).is_empty());

    let merged = FontCatalog::builtin()
        .merged(&[(*catalog(&[file])).clone()])
        .unwrap();
    fetcher.set_catalog(Arc::new(merged));
    fetcher.fetch_now(&sha).unwrap();
    assert!(cache.has(&sha));
    assert!(fetcher.catalog().by_sha(&sha).is_some());

    // Taken away again, the cached bytes stay; a new fetch is refused.
    fetcher.set_catalog(Arc::new(FontCatalog::builtin().clone()));
    assert!(cache.has(&sha));
    assert!(fetcher.catalog().by_sha(&sha).is_none());
}
