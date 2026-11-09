//! The media cache (6B-1 plan Task 13, Decisions 10–12, review focus 5): commits are rename
//! then insert, the adoption at start keeps only whole entries whose key recomputes, rows
//! without a directory go, pins hold against eviction, eviction is least recently used, the
//! first commit of a key wins, a full volume is `CACHE_IO`, and the index maps entries to the
//! projects they serve. The crate is a napi `cdylib`, so this includes the napi-free modules.

#![allow(dead_code)]
// Tests build and inspect temporary trees by path; none is a library reader.
#![allow(clippy::disallowed_methods)]

#[path = "../src/media/cache/adopt.rs"]
mod adopt;
#[path = "../src/media/cache/commit.rs"]
mod commit;
#[path = "../src/media/cache/evict.rs"]
mod evict;
#[path = "../src/media/cache/index.rs"]
mod index;
#[path = "../src/media/cache/state.rs"]
mod state;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use evict::{GIB, default_cap, entry_dir};
use fin_media::backend::VendorClass;
use fin_media::cache::{CacheKey, EntryFile, EntryMeta, KeyInput, PROXY_540, PROXY_1080, THUMBS};
use serde_json::json;
use state::{Cache, Clock};

struct Fx {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    jobs: PathBuf,
    now: Arc<AtomicI64>,
}

impl Fx {
    fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let data = base.join("data");
        let jobs = base.join("jobs");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&jobs).unwrap();
        Fx {
            _tmp: tmp,
            data,
            jobs,
            now: Arc::new(AtomicI64::new(1_000)),
        }
    }

    fn clock(&self) -> Clock {
        let now = self.now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    }

    fn tick(&self) {
        self.now.fetch_add(1, Ordering::SeqCst);
    }

    fn open(&self, cap: Option<u64>) -> (Cache, adopt::Adopted) {
        Cache::open(&self.data, cap, Some(self.clock())).unwrap()
    }

    fn root(&self) -> PathBuf {
        fs::canonicalize(Cache::root_of(&self.data)).unwrap()
    }

    /// A job's `out/` with `out/<key>/` holding `files` and a `meta.json` for `input`.
    fn job(&self, id: &str, input: &KeyInput, files: &[(&str, &[u8])]) -> (PathBuf, String) {
        let out = self.jobs.join(id).join("out");
        let (key, _) = write_entry(&out, input, files);
        (out, key)
    }
}

fn input(fp: &str) -> KeyInput {
    KeyInput::new(
        fp,
        PROXY_1080,
        json!({ "tier": "t1080" }),
        VendorClass::Software,
    )
}

fn meta_for(input: &KeyInput, files: &[(&str, &[u8])]) -> EntryMeta {
    EntryMeta {
        key: CacheKey::compute(input).to_string(),
        fingerprint: input.fingerprint.clone(),
        recipe: input.recipe.clone(),
        recipe_version: input.recipe_version,
        params: input.params.clone(),
        render_version: input.render_version,
        decoder_class: input.decoder_class,
        encoder: None,
        files: files
            .iter()
            .map(|(name, bytes)| EntryFile {
                name: (*name).to_string(),
                bytes: bytes.len() as u64,
                blake3: blake3::hash(bytes).to_hex().to_string(),
            })
            .collect(),
        media: None,
        created_at: 7,
    }
}

/// Writes `<parent>/<key>/` with `files` and `meta.json`; the key and the meta.
fn write_entry(parent: &Path, input: &KeyInput, files: &[(&str, &[u8])]) -> (String, EntryMeta) {
    let meta = meta_for(input, files);
    write_dir(&parent.join(&meta.key), &meta, files);
    (meta.key.clone(), meta)
}

fn write_dir(dir: &Path, meta: &EntryMeta, files: &[(&str, &[u8])]) {
    fs::create_dir_all(dir).unwrap();
    for (name, bytes) in files {
        fs::write(dir.join(name), bytes).unwrap();
    }
    fs::write(dir.join("meta.json"), serde_json::to_vec(meta).unwrap()).unwrap();
}

fn key_of(k: &str) -> CacheKey {
    CacheKey::parse(k).unwrap()
}

const FILES: &[(&str, &[u8])] = &[("proxy.mp4", b"mp4 bytes"), ("table.bin", b"table")];

#[cfg(unix)]
fn inode(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(p).unwrap().ino()
}

#[test]
fn commit_is_rename_then_insert() {
    let fx = Fx::new();
    let (mut c, _) = fx.open(None);
    let (out, key) = fx.job("j1", &input("fp1"), FILES);
    #[cfg(unix)]
    let before = inode(&out.join(&key).join("proxy.mp4"));
    let meta = c.commit(&out, &key, &["p1".into()]).unwrap();
    assert_eq!(meta.key, key);
    let dir = entry_dir(&fx.root(), &key_of(&key));
    assert_eq!(dir, c.entry_dir(&key_of(&key)));
    assert_eq!(fs::read(dir.join("proxy.mp4")).unwrap(), b"mp4 bytes");
    // Moved by rename: the same inode, nothing left in the job's output.
    #[cfg(unix)]
    assert_eq!(inode(&dir.join("proxy.mp4")), before);
    assert!(!out.join(&key).join("proxy.mp4").exists());
    assert_eq!(c.lookup(&key).unwrap(), Some(meta));
    let st = c.status().unwrap();
    assert_eq!(st.entries, 1);
    let meta_len = fs::metadata(dir.join("meta.json")).unwrap().len();
    assert_eq!(st.bytes, 14 + meta_len);
    assert_eq!(st.by_recipe["proxy-1080"].entries, 1);

    // The order: with the hook stopping after the rename, the directory is in place and the
    // index has no row yet.
    let (out2, key2) = fx.job("j2", &input("fp2"), FILES);
    c.faults.stop_after_rename = true;
    let e = c.commit(&out2, &key2, &[]).unwrap_err();
    assert_eq!(e.code, "INTERNAL");
    assert!(
        entry_dir(&fx.root(), &key_of(&key2))
            .join("meta.json")
            .is_file()
    );
    assert_eq!(c.lookup(&key2).unwrap(), None);
    assert_eq!(c.status().unwrap().entries, 1);
}

#[test]
fn crash_between_rename_and_insert() {
    let fx = Fx::new();
    let (out, key) = fx.job("j1", &input("fp1"), FILES);
    {
        let (mut c, _) = fx.open(None);
        c.faults.stop_after_rename = true;
        assert!(c.commit(&out, &key, &["p1".into()]).is_err());
    }
    let (mut c, adopted) = fx.open(None);
    assert_eq!(adopted.adopted, 1, "{adopted:?}");
    assert_eq!(adopted.deleted, 0);
    let meta = c.lookup(&key).unwrap().expect("adopted");
    assert_eq!(meta.files.len(), 2);
    // The projects were not recorded before the crash; the next commit of the key adds them.
    assert!(c.projects_of(&key).unwrap().is_empty());
    let (out2, _) = fx.job("j2", &input("fp1"), FILES);
    assert_eq!(c.commit(&out2, &key, &["p1".into()]).unwrap(), meta);
    assert_eq!(c.projects_of(&key).unwrap(), ["p1"]);
}

#[test]
fn foreign_dir_not_adopted() {
    let fx = Fx::new();
    drop(fx.open(None));
    let root = fx.root();
    let place = |meta: &EntryMeta, files: &[(&str, &[u8])]| {
        let dir = entry_dir(&root, &key_of(&meta.key));
        write_dir(&dir, meta, files);
        dir
    };
    // The key does not recompute from meta.json (params changed after it was computed).
    let mut forged = meta_for(&input("a"), FILES);
    forged.params = json!({ "tier": "t540" });
    let d1 = place(&forged, FILES);
    // A file whose bytes differ from what meta.json lists.
    let honest = meta_for(&input("b"), FILES);
    let d2 = place(
        &honest,
        &[("proxy.mp4", b"other bytes"), ("table.bin", b"table")],
    );
    // A file meta.json does not list.
    let m3 = meta_for(&input("c"), FILES);
    let d3 = place(&m3, FILES);
    fs::write(d3.join("extra.bin"), b"x").unwrap();
    // A whole entry under the wrong shard.
    let m4 = meta_for(&input("d"), FILES);
    let wrong = if m4.key.starts_with("00") { "01" } else { "00" };
    let d4 = root.join(wrong).join(&m4.key);
    write_dir(&d4, &m4, FILES);
    // A listed file missing; a name that is not a key; a stray file in a shard.
    let m5 = meta_for(&input("e"), FILES);
    let d5 = place(&m5, FILES);
    fs::remove_file(d5.join("table.bin")).unwrap();
    let d6 = root.join("ab").join("not-a-key");
    fs::create_dir_all(&d6).unwrap();
    fs::write(root.join("ab").join("stray"), b"x").unwrap();
    // meta.json naming another key than its directory.
    let m7 = meta_for(&input("f"), FILES);
    let other = CacheKey::compute(&input("g"));
    let d7 = entry_dir(&root, &other);
    write_dir(&d7, &m7, FILES);
    // A link where an entry would be.
    #[cfg(unix)]
    let d8 = {
        let m8 = meta_for(&input("h"), FILES);
        let real = fx.jobs.join("elsewhere");
        write_dir(&real, &m8, FILES);
        let link = entry_dir(&root, &key_of(&m8.key));
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        link
    };
    let (c, adopted) = fx.open(None);
    assert_eq!(adopted.adopted, 0, "{adopted:?}");
    for d in [&d1, &d2, &d3, &d4, &d5, &d6, &d7] {
        assert!(!d.exists(), "{} kept", d.display());
    }
    assert!(!root.join("ab").join("stray").exists());
    #[cfg(unix)]
    {
        assert!(fs::symlink_metadata(&d8).is_err(), "the link is removed");
        assert!(
            fx.jobs.join("elsewhere").join("meta.json").exists(),
            "never followed"
        );
    }
    assert_eq!(c.status().unwrap().entries, 0);
}

#[test]
fn row_without_dir_dropped() {
    let fx = Fx::new();
    let (out, key) = fx.job("j1", &input("fp1"), FILES);
    let (out2, key2) = fx.job("j2", &input("fp2"), FILES);
    {
        let (mut c, _) = fx.open(None);
        c.commit(&out, &key, &[]).unwrap();
        c.commit(&out2, &key2, &[]).unwrap();
    }
    fs::remove_dir_all(entry_dir(&fx.root(), &key_of(&key))).unwrap();
    let (mut c, adopted) = fx.open(None);
    assert_eq!(adopted.dropped, 1, "{adopted:?}");
    assert_eq!(c.lookup(&key).unwrap(), None);
    assert_eq!(c.status().unwrap().entries, 1);
    // While open: a lookup that finds the directory gone drops the row.
    fs::remove_dir_all(entry_dir(&fx.root(), &key_of(&key2))).unwrap();
    assert_eq!(c.lookup(&key2).unwrap(), None);
    assert_eq!(c.status().unwrap().entries, 0);
}

/// Three committed entries a, b, c (in that order of use); their keys.
fn three(fx: &Fx, c: &mut Cache, projects: &[&[&str]; 3]) -> [String; 3] {
    let mut keys: Vec<String> = Vec::new();
    for (i, name) in ["a", "b", "c"].iter().enumerate() {
        let (out, key) = fx.job(&format!("j{name}"), &input(name), FILES);
        let p: Vec<String> = projects[i].iter().map(|s| s.to_string()).collect();
        c.commit(&out, &key, &p).unwrap();
        fx.tick();
        keys.push(key);
    }
    keys.try_into().unwrap()
}

fn present(c: &mut Cache, key: &str) -> bool {
    c.index.contains(key).unwrap()
}

#[test]
fn pinned_not_evicted() {
    let fx = Fx::new();
    let (mut c, _) = fx.open(None);
    let [a, b, k3] = three(&fx, &mut c, &[&[], &["p1"], &[]]);
    assert!(c.pin(&a, "job:01j").unwrap());
    assert_eq!(c.pin_project("p1").unwrap(), 1);
    assert_eq!(c.pins_of(&b).unwrap(), ["project:p1"]);
    assert!(
        !c.pin(&CacheKey::compute(&input("zz")).to_string(), "job:01j")
            .unwrap()
    );
    assert!(c.pin(&a, "bad holder").is_err());
    assert_eq!(c.status().unwrap().pinned, 2);
    let r = c.gc(Some(0)).unwrap();
    assert_eq!(r.evicted, 1);
    assert!(present(&mut c, &a) && present(&mut c, &b) && !present(&mut c, &k3));
    assert!(r.bytes > 0, "the pinned entries stay above the target");
    // A job's end releases its pins; a closed project's go with it.
    assert_eq!(c.release("job:01j").unwrap(), 1);
    assert_eq!(c.gc(Some(0)).unwrap().evicted, 1);
    assert!(!present(&mut c, &a) && present(&mut c, &b));
    // An entry committed for an open project is pinned at once.
    let (out, key) = fx.job("jd", &input("d"), FILES);
    c.commit(&out, &key, &["p1".into()]).unwrap();
    assert_eq!(c.pins_of(&key).unwrap(), ["project:p1"]);
    assert_eq!(c.unpin_project("p1").unwrap(), 2);
    assert_eq!(c.gc(Some(0)).unwrap().evicted, 2);
    assert_eq!(c.status().unwrap().entries, 0);
}

#[test]
fn pins_do_not_outlive_a_run() {
    let fx = Fx::new();
    let a = {
        let (mut c, _) = fx.open(None);
        let [a, ..] = three(&fx, &mut c, &[&["p1"], &[], &[]]);
        c.pin(&a, "job:01j").unwrap();
        c.pin_project("p1").unwrap();
        a
    };
    let (mut c, _) = fx.open(None);
    assert!(c.pins_of(&a).unwrap().is_empty());
    // The server pins again what it opens.
    c.pin_project("p1").unwrap();
    assert_eq!(c.pins_of(&a).unwrap(), ["project:p1"]);
}

#[test]
fn lru_order() {
    let fx = Fx::new();
    let (mut c, _) = fx.open(None);
    let [a, b, k3] = three(&fx, &mut c, &[&[], &[], &[]]);
    // A lookup is a use: a becomes the most recent.
    assert!(c.lookup(&a).unwrap().is_some());
    let one = c.status().unwrap().bytes / 3;
    let r = c.gc(Some(2 * one)).unwrap();
    assert_eq!(r.evicted, 1);
    assert_eq!(r.freed_bytes, one);
    assert!(!present(&mut c, &b) && present(&mut c, &k3) && present(&mut c, &a));
    assert!(!entry_dir(&fx.root(), &key_of(&b)).exists());
    let r = c.gc(Some(one)).unwrap();
    assert_eq!(r.evicted, 1);
    assert!(!present(&mut c, &k3) && present(&mut c, &a));
    // `gc` never goes above the cap; the cap evicts after each commit, never the new entry.
    let (out, big) = fx.job("jbig", &input("big"), &[("p.mp4", &[7u8; 4096])]);
    c.set_cap(Some(one)).unwrap();
    assert_eq!(c.gc(Some(10 * GIB)).unwrap().bytes, one);
    c.commit(&out, &big, &[]).unwrap();
    assert!(present(&mut c, &big) && !present(&mut c, &a));
}

#[test]
fn concurrent_commit_same_key_first_wins() {
    let fx = Fx::new();
    let (c, _) = fx.open(None);
    let c = Arc::new(Mutex::new(c));
    let i = input("same");
    // Two jobs made the same key with different bytes (an encoder need not be deterministic).
    let (out1, key) = fx.job("j1", &i, &[("proxy.mp4", b"first")]);
    let (out2, key2) = fx.job("j2", &i, &[("proxy.mp4", b"second")]);
    assert_eq!(key, key2);
    let gate = Arc::new(Barrier::new(2));
    let run = |out: PathBuf, project: &'static str| {
        let (c, gate, key) = (c.clone(), gate.clone(), key.clone());
        std::thread::spawn(move || {
            gate.wait();
            c.lock()
                .unwrap()
                .commit(&out, &key, &[project.into()])
                .unwrap()
        })
    };
    let t1 = run(out1.clone(), "p1");
    let t2 = run(out2.clone(), "p2");
    let (m1, m2) = (t1.join().unwrap(), t2.join().unwrap());
    assert_eq!(m1, m2, "both answer the winner's entry");
    let mut c = c.lock().unwrap();
    assert_eq!(c.status().unwrap().entries, 1);
    let kept = fs::read(entry_dir(&fx.root(), &key_of(&key)).join("proxy.mp4")).unwrap();
    let winner_first = kept == b"first";
    assert!(winner_first || kept == b"second");
    assert_eq!(m1.files[0].blake3, blake3::hash(&kept).to_hex().to_string());
    // The loser's output was not moved; both projects are served.
    let loser = if winner_first { &out2 } else { &out1 };
    assert!(loser.join(&key).join("proxy.mp4").is_file());
    assert_eq!(c.projects_of(&key).unwrap(), ["p1", "p2"]);
    assert!(c.lookup(&key).unwrap().is_some());
}

#[test]
fn cache_io_on_full_volume() {
    let fx = Fx::new();
    let (mut c, _) = fx.open(None);
    let (out, key) = fx.job("j1", &input("fp1"), FILES);
    c.faults.free_bytes = Some(1000);
    let e = c.commit(&out, &key, &[]).unwrap_err();
    assert_eq!(e.code, "CACHE_IO");
    assert_eq!(e.details["reason"], "full");
    assert_eq!(e.details["freeBytes"], 1000);
    assert!(e.details["neededBytes"].as_u64().unwrap() > 1000);
    assert!(e.fix.is_some());
    // Nothing moved, nothing indexed.
    assert!(out.join(&key).join("proxy.mp4").is_file());
    assert!(!entry_dir(&fx.root(), &key_of(&key)).exists());
    assert_eq!(c.status().unwrap().entries, 0);
    // With room, the same output commits.
    c.faults.free_bytes = None;
    c.commit(&out, &key, &[]).unwrap();
}

/// The real ENOSPC: `FIN_TEST_CACHE_SMALL_FS` names a writable directory on a small
/// filesystem (a loop-mounted image, a quota'd volume) that is not the temp directory's: the
/// server data goes there, the pre-move check is told there is room, the cross-volume copy
/// runs out of space and the check after it reads the real volume. Skips without the variable.
#[test]
fn cache_io_on_full_volume_real_enospc() {
    let Some(small) = std::env::var_os("FIN_TEST_CACHE_SMALL_FS") else {
        eprintln!(
            "cache_io_on_full_volume_real_enospc: skipped; set FIN_TEST_CACHE_SMALL_FS to a \
             directory on a small filesystem to run it"
        );
        return;
    };
    let fx = Fx::new();
    let data = tempfile::tempdir_in(&small).unwrap();
    let (mut c, _) = Cache::open(data.path(), None, Some(fx.clock())).unwrap();
    let (_, free) = evict::volume_space(c.root()).unwrap();
    let big = vec![9u8; usize::try_from(free + (1 << 20)).unwrap()];
    let (out, key) = fx.job("j1", &input("fp1"), &[("proxy.mp4", &big)]);
    c.faults.free_bytes = Some(u64::MAX);
    let e = c.commit(&out, &key, &[]).unwrap_err();
    assert_eq!(e.code, "CACHE_IO", "{e:?}");
    assert_eq!(e.details["reason"], "full");
    assert_eq!(c.status().unwrap().entries, 0);
}

#[test]
fn projects_mapping_for_reach() {
    let fx = Fx::new();
    let (mut c, _) = fx.open(None);
    let i = input("fp1");
    let (out, key) = fx.job("j1", &i, FILES);
    c.commit(&out, &key, &["p1".into(), "p1".into()]).unwrap();
    assert_eq!(c.projects_of(&key).unwrap(), ["p1"]);
    // Another project using the same source: the same entry serves both.
    let (out2, _) = fx.job("j2", &i, FILES);
    c.commit(&out2, &key, &["p2".into()]).unwrap();
    assert_eq!(c.projects_of(&key).unwrap(), ["p1", "p2"]);
    // A second recipe of the same source; another source.
    let thumbs = KeyInput::new(
        "fp1",
        THUMBS,
        json!({ "times": ["0"] }),
        VendorClass::Software,
    );
    let (out3, tkey) = fx.job("j3", &thumbs, &[("t0.png", b"png")]);
    c.commit(&out3, &tkey, &["p1".into()]).unwrap();
    let low = KeyInput::new("fp9", PROXY_540, json!({ "tier": "t540" }), VendorClass::Os);
    let (out4, lkey) = fx.job("j4", &low, FILES);
    c.commit(&out4, &lkey, &["p3".into()]).unwrap();
    let mut recipes: Vec<String> = c
        .entries_for("fp1")
        .unwrap()
        .into_iter()
        .map(|m| m.recipe)
        .collect();
    recipes.sort();
    assert_eq!(recipes, ["proxy-1080", "thumbs"]);
    assert_eq!(c.projects_of(&lkey).unwrap(), ["p3"]);
    assert!(c.projects_of(&"0".repeat(64)).unwrap().is_empty());
    // Ids are checked before anything moves.
    let (out5, k5) = fx.job("j5", &input("fp5"), FILES);
    let e = c.commit(&out5, &k5, &["../x".into()]).unwrap_err();
    assert_eq!(e.code, "INVALID_OP");
    assert!(out5.join(&k5).join("meta.json").is_file());
}

#[test]
fn cap_default_is_min_50gib_and_15_percent() {
    assert_eq!(default_cap(Some(100 * GIB)), 15 * GIB);
    assert_eq!(default_cap(Some(1000 * GIB)), 50 * GIB);
    assert_eq!(default_cap(Some(334 * GIB)), 50 * GIB);
    assert_eq!(default_cap(None), 50 * GIB);
    let fx = Fx::new();
    let (c, _) = fx.open(None);
    let (total, _) = evict::volume_space(c.root()).unwrap();
    assert_eq!(c.status().unwrap().cap_bytes, default_cap(Some(total)));
    let (mut c, _) = fx.open(Some(3 * GIB));
    assert_eq!(c.status().unwrap().cap_bytes, 3 * GIB);
    c.set_cap(None).unwrap();
    assert_eq!(c.status().unwrap().cap_bytes, default_cap(Some(total)));
}

#[test]
fn bad_outputs_are_refused_and_nothing_is_placed() {
    let fx = Fx::new();
    let (mut c, _) = fx.open(None);
    let none_placed = |c: &Cache, key: &str| {
        assert!(!entry_dir(&fx.root(), &key_of(key)).exists());
        assert_eq!(c.status().unwrap().entries, 0);
    };
    // The committed key is not the one meta.json computes.
    let (out, key) = fx.job("j1", &input("fp1"), FILES);
    let other = CacheKey::compute(&input("fp2")).to_string();
    fs::rename(out.join(&key), out.join(&other)).unwrap();
    let e = c.commit(&out, &other, &[]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("PLAN_MISMATCH", &json!("cacheKey"))
    );
    none_placed(&c, &other);
    // A file meta.json does not list.
    let (out, key) = fx.job("j2", &input("fp3"), FILES);
    fs::write(out.join(&key).join("extra"), b"x").unwrap();
    let e = c.commit(&out, &key, &[]).unwrap_err();
    assert_eq!(e.details["file"], "extra");
    none_placed(&c, &key);
    // A file whose bytes differ from meta.json's hash (same size).
    let (out, key) = fx.job("j3", &input("fp4"), FILES);
    fs::write(out.join(&key).join("table.bin"), b"TABLE").unwrap();
    let e = c.commit(&out, &key, &[]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["file"]),
        ("PLAN_MISMATCH", &json!("table.bin"))
    );
    none_placed(&c, &key);
    // Bigger than meta.json says: fin-shm's cap refuses it.
    let (out, key) = fx.job("j4", &input("fp5"), FILES);
    fs::write(out.join(&key).join("table.bin"), b"table and more").unwrap();
    assert_eq!(c.commit(&out, &key, &[]).unwrap_err().code, "SHM_REFUSED");
    none_placed(&c, &key);
    // A malformed key, and a missing output.
    assert_eq!(
        c.commit(&out, "../etc", &[]).unwrap_err().code,
        "PLAN_MISMATCH"
    );
    assert!(c.commit(&out, &"a".repeat(64), &[]).is_err());
}

#[cfg(unix)]
#[test]
fn links_in_the_output_are_refused() {
    let fx = Fx::new();
    let (mut c, _) = fx.open(None);
    let (out, key) = fx.job("j1", &input("fp1"), FILES);
    let outside = fx.jobs.join("outside.bin");
    fs::write(&outside, b"table").unwrap();
    fs::remove_file(out.join(&key).join("table.bin")).unwrap();
    std::os::unix::fs::symlink(&outside, out.join(&key).join("table.bin")).unwrap();
    let e = c.commit(&out, &key, &[]).unwrap_err();
    assert!(
        matches!(e.code.as_str(), "PLAN_MISMATCH" | "SHM_REFUSED"),
        "{e:?}"
    );
    assert!(!entry_dir(&fx.root(), &key_of(&key)).exists());
}

#[test]
fn a_damaged_index_is_rebuilt_from_the_entries() {
    let fx = Fx::new();
    let (out, key) = fx.job("j1", &input("fp1"), FILES);
    {
        let (mut c, _) = fx.open(None);
        c.commit(&out, &key, &[]).unwrap();
    }
    let db = fx.root().join("index.db");
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", db.display()));
    }
    fs::write(&db, b"not a database").unwrap();
    let (mut c, adopted) = fx.open(None);
    assert!(adopted.index_replaced);
    assert_eq!(adopted.adopted, 1);
    assert!(c.lookup(&key).unwrap().is_some());
}

#[test]
fn leftovers_of_a_crash_are_cleared_at_open() {
    let fx = Fx::new();
    drop(fx.open(None));
    let root = fx.root();
    fs::create_dir_all(root.join(".staging").join("x")).unwrap();
    fs::create_dir_all(root.join(".trash").join("y")).unwrap();
    drop(fx.open(None));
    assert!(!root.join(".staging").exists());
    assert!(!root.join(".trash").exists());
}
