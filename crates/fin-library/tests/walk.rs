//! The walker over the fake source (plan 5D Task 4): order, parallelism, batches, modes,
//! mounts, empty suspects, cancel and placeholders.

mod walk_util;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fin_library::source::fake::FakeSource;
use fin_library::walk::code;
use fin_library::{
    CancelToken, DirHandle, KnownDir, KnownDirs, RawEntry, RootStat, RootState, Source,
    VolumeIdentity, WalkError, WalkMode, WalkStop, walk,
};
use serde_json::{Value, json};
use walk_util::*;

fn fake(spec: &Value) -> Arc<FakeSource> {
    Arc::new(FakeSource::from_json(&spec.to_string()).unwrap())
}

fn src(f: &Arc<FakeSource>) -> Arc<dyn Source> {
    Arc::clone(f) as Arc<dyn Source>
}

/// `tree()` with a read delay on every directory, larger for earlier names, so parallel reads
/// finish out of walk order.
fn delayed_tree() -> Value {
    fn delay(v: &mut Value, ms: &mut u64) {
        if v.get("children").is_some() {
            v["readDelayMs"] = json!(*ms);
            *ms = ms.saturating_sub(4);
            for c in v["children"].as_array_mut().unwrap() {
                delay(c, ms);
            }
        }
    }
    let mut t = tree();
    let mut ms = 40;
    for c in t["children"].as_array_mut().unwrap() {
        delay(c, &mut ms);
    }
    t
}

#[test]
fn walk_order_is_deterministic() {
    let expect = ["", "a", "b", "c", "a/x", "a/y", "b/z"];
    let mut seen = Vec::new();
    for threads in [1, 3, 16] {
        let f = fake(&delayed_tree());
        let (r, batches) = run(src(&f), full(1, None), &opts(threads));
        let s = r.unwrap();
        assert_eq!(done_paths(&batches), expect, "threads {threads}");
        assert_eq!((s.listed, s.entries, s.incomplete), (7, 19, 0));
        seen.push(serde_json::to_string(&batches).unwrap());
    }
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "batches differ by thread count"
    );
}

/// Counts concurrent listings.
struct Counting {
    inner: Arc<FakeSource>,
    now: AtomicUsize,
    max: AtomicUsize,
}

impl Source for Counting {
    fn stat_root(&self) -> std::io::Result<RootStat> {
        self.inner.stat_root()
    }
    fn open_dir(&self, rel: &fin_library::RawRel) -> std::io::Result<DirHandle> {
        self.inner.open_dir(rel)
    }
    fn read_dir(&self, dir: &DirHandle) -> std::io::Result<Vec<RawEntry>> {
        let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(n, Ordering::SeqCst);
        let r = self.inner.read_dir(dir);
        self.now.fetch_sub(1, Ordering::SeqCst);
        r
    }
    fn volume(&self, dir: &DirHandle) -> std::io::Result<VolumeIdentity> {
        self.inner.volume(dir)
    }
}

#[test]
fn parallel_reads_bounded_by_threads() {
    let wide: Vec<Value> = (0..40)
        .map(|i| {
            let mut d = dir(&format!("d{i:02}"), i, vec![file("f", 1)]);
            d["readDelayMs"] = json!(15);
            d
        })
        .collect();
    for threads in [1, 4, 16] {
        let c = Arc::new(Counting {
            inner: fake(&root(wide.clone())),
            now: AtomicUsize::new(0),
            max: AtomicUsize::new(0),
        });
        let (r, batches) = run(
            Arc::clone(&c) as Arc<dyn Source>,
            full(1, None),
            &opts(threads),
        );
        assert_eq!(r.unwrap().listed, 41);
        assert_eq!(dones(&batches).len(), 41);
        let max = c.max.load(Ordering::SeqCst);
        assert!(
            max <= threads,
            "{max} concurrent reads with {threads} threads"
        );
        if threads > 1 {
            assert!(max > 1, "reads did not overlap with {threads} threads");
        }
    }
}

#[test]
fn batches_capped_at_4096_entries() {
    let big: Vec<Value> = (0..10_000)
        .map(|i| file(&format!("f{i:05}.mov"), i))
        .collect();
    let spec = root(vec![
        dir("big", 1, big),
        dir("small", 2, vec![file("s", 1)]),
    ]);
    let (r, batches) = run(src(&fake(&spec)), full(1, None), &opts(8));
    assert_eq!(r.unwrap().entries, 10_003);
    for (i, b) in batches.iter().enumerate() {
        assert_eq!(b.seq, i as u64);
        assert!(b.entries.len() + b.dirs_done.len() <= 4096, "batch {i}");
    }
    assert_eq!(entries(&batches).len(), 10_003);
    assert!(batches.len() >= 3);

    // The byte cap: a deep chain makes each entry large.
    let mut o = opts(4);
    o.batch_bytes = 64 * 1024;
    let long = "n".repeat(200);
    let deep = (0..8).rev().fold(
        dir(
            &long,
            1,
            (0..300).map(|i| file(&format!("{long}{i}"), 1)).collect(),
        ),
        |inner, _| dir(&long, 1, vec![inner]),
    );
    let (r, batches) = run(src(&fake(&root(vec![deep]))), full(1, None), &o);
    r.unwrap();
    for b in &batches {
        let n = serde_json::to_string(b).unwrap().len();
        assert!(n <= 64 * 1024, "a batch of {n} bytes");
    }
}

#[test]
fn listing_entries_precede_their_dir_done() {
    let big: Vec<Value> = (0..900).map(|i| file(&format!("f{i:04}"), i)).collect();
    let spec = root(vec![
        dir("a", 1, big.clone()),
        dir("b", 2, big),
        file("t", 1),
    ]);
    let mut o = opts(4);
    o.batch_entries = 256;
    let (r, batches) = run(src(&fake(&spec)), full(1, None), &o);
    r.unwrap();
    let mut closed = std::collections::HashSet::new();
    for b in &batches {
        for e in &b.entries {
            assert!(
                !closed.contains(&e.dir),
                "an entry of {} after its DirDone",
                e.dir
            );
        }
        for d in &b.dirs_done {
            let listed = batches
                .iter()
                .take_while(|x| x.seq <= b.seq)
                .flat_map(|x| &x.entries)
                .filter(|e| e.dir == d.dir)
                .count();
            assert_eq!(listed as u32, d.entries, "{}", d.dir);
            closed.insert(d.dir.clone());
        }
    }
}

#[test]
fn eio_and_eacces_dirs_incomplete() {
    let mut spec = tree();
    spec["children"][0]["error"] = json!("io");
    spec["children"][1]["openError"] = json!("permissionDenied");
    spec["children"][0]["children"][2]["error"] = json!("timedOut");
    let (r, batches) = run(src(&fake(&spec)), full(1, None), &opts(4));
    let s = r.unwrap();
    assert_eq!(
        done_paths(&batches),
        ["", "a!IO", "b!PERMISSION_DENIED", "c"]
    );
    assert_eq!((s.listed, s.incomplete), (2, 2));
    assert!(dones(&batches).iter().all(|d| d.complete || d.entries == 0));
}

#[test]
fn poll_relists_only_changed_mtimes() {
    let f = fake(&tree());
    let (r, batches) = run(src(&f), full(1, None), &opts(4));
    r.unwrap();
    let known = known_from(&batches, 1);
    let before = f.stats();

    // Nothing changed: no listing, leaves not even opened.
    let (r, batches) = run(
        src(&f),
        WalkMode::Poll {
            known: known.clone(),
        },
        &opts(4),
    );
    let s = r.unwrap();
    assert!(batches.is_empty());
    assert_eq!((s.listed, s.unchanged), (0, 7));
    let st = f.stats();
    assert_eq!(st.dir_reads, before.dir_reads);
    let opened: Vec<String> = st
        .opens_by_dir
        .iter()
        .filter(|(r, n)| before.opens_by_dir.get(*r) != Some(n))
        .map(|(r, _)| r.display())
        .collect();
    assert_eq!(
        opened,
        ["", "a", "b"],
        "only directories with known subdirectories"
    );

    // a/x changed and gained a new subtree; b's mtime changed.
    let mut t = tree();
    t["children"][0]["children"][2]["mtimeNs"] = json!(99);
    t["children"][0]["children"][2]["children"]
        .as_array_mut()
        .unwrap()
        .push(dir("new", 5, vec![dir("deeper", 6, vec![file("n", 1)])]));
    t["children"][1]["mtimeNs"] = json!(98);
    f.reload(&t.to_string()).unwrap();
    let reads = f.stats().reads_by_dir;
    let (r, batches) = run(src(&f), WalkMode::Poll { known }, &opts(4));
    let s = r.unwrap();
    assert_eq!(
        done_paths(&batches),
        ["b", "a/x", "a/x/new", "a/x/new/deeper"]
    );
    assert_eq!(s.listed, 4);
    let relisted: BTreeMap<String, u64> = f
        .stats()
        .reads_by_dir
        .into_iter()
        .filter(|(r, n)| reads.get(r) != Some(n))
        .map(|(r, n)| (r.display(), n - reads.get(&r).copied().unwrap_or(0)))
        .collect();
    assert_eq!(relisted.len(), 4, "{relisted:?}");
    assert!(relisted.values().all(|n| *n == 1));
    let x = dones(&batches)
        .into_iter()
        .find(|d| d.dir == rel("a/x"))
        .unwrap();
    assert_eq!(x.mtime_ns, Some(99));
}

#[test]
fn poll_relists_root_whose_mtime_changed_and_a_dir_whose_child_vanished() {
    let f = fake(&tree());
    let (_, batches) = run(src(&f), full(1, None), &opts(2));
    let known = known_from(&batches, 1);
    let mut t = tree();
    t["mtimeNs"] = json!(50);
    // b/z vanished while b kept its mtime (a stale attribute cache): b is listed after all.
    t["children"][1]["children"].as_array_mut().unwrap().pop();
    f.reload(&t.to_string()).unwrap();
    let (r, batches) = run(src(&f), WalkMode::Poll { known }, &opts(2));
    r.unwrap();
    assert_eq!(done_paths(&batches), ["", "b"]);
}

#[test]
fn dirs_mode_targets_only_given_dirs() {
    let f = fake(&tree());
    let (_, batches) = run(src(&f), full(1, None), &opts(2));
    let known = known_from(&batches, 1);
    let targets = vec![rel("b/z"), rel("a"), rel("a"), rel(".hidden")];

    let mode = |known| WalkMode::Dirs {
        dirs: targets.clone(),
        known,
    };
    let (r, batches) = run(src(&f), mode(Some(known.clone())), &opts(2));
    r.unwrap();
    assert_eq!(done_paths(&batches), ["a", "b/z"]);
    assert!(
        entries(&batches)
            .iter()
            .all(|e| e.dir == rel("a") || e.dir == rel("b/z"))
    );

    // A subtree moved into `a`: listed with known dirs, not without.
    let mut t = tree();
    t["children"][0]["children"]
        .as_array_mut()
        .unwrap()
        .push(dir("moved", 3, vec![dir("in", 4, vec![file("m", 1)])]));
    f.reload(&t.to_string()).unwrap();
    let (r, batches) = run(src(&f), mode(Some(known)), &opts(2));
    r.unwrap();
    assert_eq!(done_paths(&batches), ["a", "b/z", "a/moved", "a/moved/in"]);
    let (r, batches) = run(src(&f), mode(None), &opts(2));
    r.unwrap();
    assert_eq!(done_paths(&batches), ["a", "b/z"]);
}

#[test]
fn resume_skips_listed_dirs_but_descends() {
    let f = fake(&tree());
    let (_, batches) = run(src(&f), full(6, None), &opts(2));
    // Generation 7 had listed the root and `a` before a restart.
    let known = KnownDirs::new(dones(&batches).into_iter().map(|d| KnownDir {
        listed_gen: Some(if d.dir == rel("") || d.dir == rel("a") {
            7
        } else {
            6
        }),
        dir: d.dir,
        mtime_ns: d.mtime_ns,
        entries: d.entries,
        mount_root: false,
        volume: None,
    }));
    let before = f.stats().reads_by_dir;
    let (r, batches) = run(src(&f), full(7, Some(known)), &opts(3));
    let s = r.unwrap();
    assert_eq!(done_paths(&batches), ["b", "c", "a/x", "a/y", "b/z"]);
    assert_eq!((s.skipped, s.listed), (2, 5));
    let after = f.stats().reads_by_dir;
    for skipped in ["", "a"] {
        assert_eq!(
            after.get(&rel(skipped)),
            before.get(&rel(skipped)),
            "{skipped:?} re-read"
        );
    }
}

fn disk_tree(mounted: bool) -> Value {
    let disk = if mounted {
        json!({ "name": "disk", "type": "dir", "mtimeNs": 3, "mountRoot": true, "dev": "9",
                "children": [ file("d1.mov", 1), dir("sub", 4, vec![file("d2.mov", 1)]) ] })
    } else {
        json!({ "name": "disk", "type": "dir", "mtimeNs": 8, "children": [] })
    };
    root(vec![disk, dir("other", 5, vec![file("o", 1)])])
}

#[test]
fn nested_mount_unmounted_is_offline_subtree() {
    let f = fake(&disk_tree(true));
    let (_, batches) = run(src(&f), full(1, None), &opts(2));
    assert_eq!(done_paths(&batches), ["", "disk", "other", "disk/sub"]);
    let known = known_from(&batches, 1);
    assert!(known.get(&rel("disk")).unwrap().mount_root);

    f.reload(&disk_tree(false).to_string()).unwrap();
    let opens = f.stats().opens_by_dir.get(&rel("disk")).copied();
    for mode in [
        full(2, Some(known.clone())),
        WalkMode::Poll {
            known: known.clone(),
        },
        WalkMode::Dirs {
            dirs: vec![rel("")],
            known: Some(known.clone()),
        },
    ] {
        let (r, batches) = run(src(&f), mode, &opts(2));
        r.unwrap();
        let paths = done_paths(&batches);
        assert!(
            paths.contains(&"disk!ROOT_UNMOUNTED".to_owned()),
            "{paths:?}"
        );
        assert!(!paths.iter().any(|p| p.starts_with("disk/")), "{paths:?}");
        assert!(
            entries(&batches)
                .iter()
                .all(|e| !e.dir.starts_with(&rel("disk")))
        );
    }
    assert_eq!(
        f.stats().opens_by_dir.get(&rel("disk")).copied(),
        opens,
        "disk opened"
    );
}

/// `disk` as a nested mount of volume `key` (its root id `rid`, its files' mtime `m`).
fn mount_tree(key: &str, rid: &str, m: i64) -> Value {
    let disk = json!({ "name": "disk", "type": "dir", "mtimeNs": 3, "mountRoot": true,
        "dev": "9", "volumeKey": key, "fileId": rid,
        "children": [ file("d1.mov", m), dir("sub", 4, vec![file("d2.mov", m)]) ] });
    root(vec![disk, dir("other", 5, vec![file("o", 1)])])
}

fn modes(known: &KnownDirs) -> [WalkMode; 3] {
    [
        full(2, Some(known.clone())),
        WalkMode::Poll {
            known: known.clone(),
        },
        WalkMode::Dirs {
            dirs: vec![rel(""), rel("disk")],
            known: Some(known.clone()),
        },
    ]
}

#[test]
fn nested_mount_volume_is_catalogued_and_checked() {
    let f = fake(&mount_tree("v1", "0d", 1));
    let (_, batches) = run(src(&f), full(1, None), &opts(2));
    assert_eq!(done_paths(&batches), ["", "disk", "other", "disk/sub"]);
    let volume = |b: &[fin_library::ScanBatch], d: &str| {
        dones(b)
            .into_iter()
            .find(|x| x.dir == rel(d))
            .unwrap()
            .volume
    };
    assert_eq!(volume(&batches, "disk").unwrap().key, "v1");
    assert!(volume(&batches, "other").is_none() && volume(&batches, "disk/sub").is_none());
    let known = known_from(&batches, 1);
    assert_eq!(
        known
            .get(&rel("disk"))
            .unwrap()
            .volume
            .as_ref()
            .unwrap()
            .key,
        "v1"
    );

    // Another volume mounted there (another key, root id and content): offline, not missing.
    f.reload(&mount_tree("v2", "0e", 2).to_string()).unwrap();
    for mode in modes(&known) {
        let (r, batches) = run(src(&f), mode, &opts(2));
        r.unwrap();
        let paths = done_paths(&batches);
        assert!(
            paths.contains(&"disk!ROOT_VOLUME_CHANGED".to_owned()),
            "{paths:?}"
        );
        assert!(!paths.iter().any(|p| p.starts_with("disk/")), "{paths:?}");
        assert!(
            entries(&batches)
                .iter()
                .all(|e| !e.dir.starts_with(&rel("disk")))
        );
    }

    // The same volume under another key (its root id and sample confirm it): walked.
    f.reload(&mount_tree("v3", "0d", 1).to_string()).unwrap();
    let (r, batches) = run(src(&f), full(2, Some(known.clone())), &opts(2));
    r.unwrap();
    assert_eq!(done_paths(&batches), ["", "disk", "other", "disk/sub"]);
    assert_eq!(volume(&batches, "disk").unwrap().key, "v3");
}

#[test]
fn nested_mount_target_unmounted_is_not_listed() {
    let f = fake(&disk_tree(true));
    let (_, batches) = run(src(&f), full(1, None), &opts(2));
    let known = known_from(&batches, 1);
    f.reload(&disk_tree(false).to_string()).unwrap();
    // Targeted directly (no parent listing to see it): its own check closes it.
    let mode = WalkMode::Dirs {
        dirs: vec![rel("disk")],
        known: Some(known),
    };
    let (r, batches) = run(src(&f), mode, &opts(2));
    r.unwrap();
    assert_eq!(done_paths(&batches), ["disk!ROOT_UNMOUNTED"]);
    assert!(entries(&batches).is_empty());
}

#[test]
fn root_empty_with_catalog_is_suspect() {
    let f = fake(&tree());
    let (r, batches) = run(src(&f), full(1, None), &opts(2));
    let identity = r.unwrap().identity;
    let known = known_from(&batches, 1);
    f.reload(&root(vec![]).to_string()).unwrap();
    for mode in [
        full(2, Some(known.clone())),
        WalkMode::Poll {
            known: known.clone(),
        },
    ] {
        let (r, batches) = run(src(&f), mode, &opts(2));
        assert!(matches!(offline(&r), Some(RootState::Empty)), "{r:?}");
        assert!(batches.is_empty());
    }
    // Without the catalog's dirs, the stored identity's sample tells.
    let mut o = opts(2);
    o.stored = Some(identity);
    let (r, _) = run(src(&f), full(2, None), &o);
    assert!(matches!(offline(&r), Some(RootState::Empty)), "{r:?}");
    // A new library may be empty.
    let (r, _) = run(src(&f), full(1, None), &opts(2));
    assert_eq!(r.unwrap().listed, 1);

    // A directory that drops from N to 0 is incomplete, not emptied.
    let mut t = tree();
    t["children"][2]["children"] = json!([]);
    f.reload(&t.to_string()).unwrap();
    let (r, batches) = run(src(&f), full(2, Some(known)), &opts(2));
    r.unwrap();
    let c = dones(&batches)
        .into_iter()
        .find(|d| d.dir == rel("c"))
        .unwrap();
    assert!(!c.complete);
    assert_eq!(c.code.as_deref(), Some(code::DIR_EMPTY_SUSPECT));
}

#[test]
fn volume_change_and_unreachable_root_are_offline() {
    let f = fake(&tree());
    let (r, _) = run(src(&f), full(1, None), &opts(2));
    let mut stored = r.unwrap().identity;
    stored.key = "other".into();
    stored.root_file_id = Some(vec![9]);
    let mut o = opts(2);
    o.stored = Some(stored);
    let (r, b) = run(src(&f), full(2, None), &o);
    assert!(
        matches!(offline(&r), Some(RootState::VolumeChanged { .. })),
        "{r:?}"
    );
    assert!(b.is_empty());

    let mut t = tree();
    t["statError"] = json!("notFound");
    f.reload(&t.to_string()).unwrap();
    let (r, _) = run(src(&f), full(2, None), &opts(2));
    match offline(&r) {
        Some(RootState::Unreachable { code }) => assert_eq!(code, "NOT_FOUND"),
        other => panic!("{other:?}"),
    }
    let mut t = tree();
    t["error"] = json!("io");
    f.reload(&t.to_string()).unwrap();
    let (r, _) = run(src(&f), full(2, None), &opts(2));
    assert!(
        matches!(offline(&r), Some(RootState::Unreachable { .. })),
        "{r:?}"
    );
}

#[test]
fn cancel_stops_within_one_batch() {
    let wide: Vec<Value> = (0..200)
        .map(|i| {
            dir(
                &format!("d{i:03}"),
                i,
                (0..20).map(|j| file(&format!("f{j}"), j)).collect(),
            )
        })
        .collect();
    let f = fake(&root(wide));
    let mut o = opts(8);
    o.batch_entries = 50;
    let cancel = CancelToken::new();
    let mut seen = 0;
    let r = walk(
        src(&f),
        full(1, None),
        &o,
        &mut |_| {
            seen += 1;
            if seen == 3 {
                cancel.cancel();
            }
            Ok(())
        },
        &cancel,
    );
    assert!(matches!(r, Err(WalkError::Cancelled)), "{r:?}");
    assert!(seen <= 4, "{seen} batches");

    let mut calls = 0;
    let r = walk(
        src(&f),
        full(1, None),
        &o,
        &mut |_| {
            calls += 1;
            Err(WalkStop("host call failed".into()))
        },
        &CancelToken::new(),
    );
    assert!(matches!(r, Err(WalkError::Stopped(_))), "{r:?}");
    assert_eq!(calls, 1);
}

const SF_DATALESS: u32 = 0x4000_0000;
const RECALL_ON_OPEN: u32 = 0x0004_0000;

fn cloud_tree() -> Value {
    root(vec![
        json!({ "name": "cloud", "type": "dir", "mtimeNs": 2, "macosFlags": SF_DATALESS,
                "children": [ file("c.mov", 1), dir("deep", 3, vec![file("d", 1)]) ] }),
        json!({ "name": "recall", "type": "dir", "mtimeNs": 3, "windowsAttributes": RECALL_ON_OPEN,
                "children": [ file("r.mov", 1) ] }),
        json!({ "name": "net", "type": "dir", "mtimeNs": 4, "automount": true, "children": [] }),
        dir(
            "plain",
            5,
            vec![
                json!({ "name": "evicted.mov", "size": 9, "mtimeNs": 1, "macosFlags": SF_DATALESS }),
                file("ok.mov", 1),
            ],
        ),
    ])
}

#[test]
fn fake_placeholders_open_counter_is_zero() {
    let f = fake(&cloud_tree());
    let (r, batches) = run(src(&f), full(1, None), &opts(4));
    r.unwrap();
    assert_eq!(
        done_paths(&batches),
        [
            "",
            "cloud!CLOUD_ONLY",
            "net!AUTOMOUNT",
            "recall!CLOUD_ONLY",
            "plain"
        ]
    );
    let known = known_from(&batches, 1);
    let modes = [
        full(2, None),
        full(2, Some(known.clone())),
        WalkMode::Dirs {
            dirs: vec![rel(""), rel("plain")],
            known: Some(known.clone()),
        },
        WalkMode::Poll {
            known: known.clone(),
        },
    ];
    // A poll after the root changed re-lists it and meets the placeholders again.
    let mut t = cloud_tree();
    t["mtimeNs"] = json!(77);
    for (i, mode) in modes.into_iter().enumerate() {
        if i == 3 {
            f.reload(&t.to_string()).unwrap();
        }
        let (r, batches) = run(src(&f), mode, &opts(4));
        r.unwrap();
        assert!(!done_paths(&batches).is_empty(), "mode {i}");
    }
    let st = f.stats();
    assert_eq!(
        (st.placeholder_touches, st.automount_triggers, st.file_opens),
        (0, 0, 0)
    );
    assert!(!st.opens_by_dir.keys().any(|r| {
        ["cloud", "recall", "net"]
            .iter()
            .any(|p| r.starts_with(&rel(p)))
    }));
}

#[test]
fn known_dirs_read_from_ndjson() {
    let f = fake(&tree());
    let (_, batches) = run(src(&f), full(3, None), &opts(2));
    let known = known_from(&batches, 3);
    let mut text = String::new();
    for d in dones(&batches) {
        let k = known.get(&d.dir).unwrap();
        text.push_str(&serde_json::to_string(k).unwrap());
        text.push_str("\n\n");
    }
    let back = KnownDirs::from_ndjson(text.as_bytes(), 3).unwrap();
    assert_eq!(back.len(), 7);
    assert_eq!(back.children(&rel("a")), [b"x".to_vec(), b"y".to_vec()]);
    assert_eq!(back.get(&rel("b")), known.get(&rel("b")));
    let line = serde_json::to_value(known.get(&rel("a")).unwrap()).unwrap();
    assert_eq!(
        line,
        json!({ "dir": { "components": ["YQ=="] }, "mtimeNs": "10", "entries": 4, "listedGen": 3, "mountRoot": false })
    );
    // The indexer's own line: `listed` stands for "listed completely in this generation".
    let line = br#"{"dir":{"components":["YQ=="]},"mtimeNs":"10","listed":true}
{"dir":{"components":["Yg=="]},"mtimeNs":null,"listed":false}
"#;
    let seven = KnownDirs::from_ndjson(&line[..], 7).unwrap();
    let a = seven.get(&rel("a")).unwrap();
    assert_eq!(
        (a.mtime_ns, a.listed_gen, a.entries, a.mount_root),
        (Some(10), Some(7), 0, false)
    );
    let b = seven.get(&rel("b")).unwrap();
    assert_eq!((b.mtime_ns, b.listed_gen), (None, None));
    assert!(KnownDirs::from_ndjson(&b"{\"dir\": 1}"[..], 3).is_err());
    assert!(KnownDirs::from_ndjson(&b"{\"dir\": {\"components\": [\"Lg==\"]}}"[..], 3).is_err());
}

/// Fails one entry's stat in `a`'s listing (an unreadable entry, `EIO`).
struct OneBad(Arc<FakeSource>);

impl Source for OneBad {
    fn stat_root(&self) -> std::io::Result<RootStat> {
        self.0.stat_root()
    }
    fn open_dir(&self, rel: &fin_library::RawRel) -> std::io::Result<DirHandle> {
        self.0.open_dir(rel)
    }
    fn read_dir(&self, dir: &DirHandle) -> std::io::Result<Vec<RawEntry>> {
        self.0.read_dir(dir)
    }
    fn read_dir_with(
        &self,
        dir: &DirHandle,
        progress: &dyn Fn(),
    ) -> std::io::Result<fin_library::Listing> {
        let names = self.0.read_dir(dir)?;
        let bad = *dir.rel() == rel("a");
        fin_library::Listing::collect(
            names,
            |e| {
                if bad && e.name == b"1.mov" {
                    Err(std::io::Error::from_raw_os_error(EIO))
                } else {
                    Ok(Some(e))
                }
            },
            progress,
        )
    }
    fn volume(&self, dir: &DirHandle) -> std::io::Result<VolumeIdentity> {
        self.0.volume(dir)
    }
}

/// `EIO` (5 on Linux and macOS).
const EIO: i32 = 5;

#[test]
fn one_unreadable_entry_keeps_the_rest_and_marks_nothing_missing() {
    let f = fake(&tree());
    let (r, batches) = run(Arc::new(OneBad(f)), full(1, None), &opts(4));
    let s = r.unwrap();
    assert_eq!(
        done_paths(&batches),
        ["", "a!ENTRY_UNREADABLE", "b", "c", "a/x", "a/y", "b/z"]
    );
    assert_eq!((s.listed, s.incomplete), (6, 1));
    let a = dones(&batches)
        .into_iter()
        .find(|d| d.dir == rel("a"))
        .unwrap();
    assert!(!a.complete);
    assert_eq!((a.entries, a.mtime_ns), (3, None));
    let names: Vec<Vec<u8>> = entries(&batches)
        .into_iter()
        .filter(|e| e.dir == rel("a"))
        .map(|e| e.name)
        .collect();
    assert_eq!(names, [b"2.mov".to_vec(), b"x".to_vec(), b"y".to_vec()]);
}

#[test]
fn listing_collect_is_partial_unless_every_entry_fails() {
    use fin_library::{EntryType, Listing};
    let eio = || std::io::Error::from_raw_os_error(EIO);
    let names = vec!["a", "b", "c"];
    let l = Listing::collect(
        names.clone(),
        |n| match n {
            "b" => Err(eio()),
            "c" => Ok(None),
            n => Ok(Some(RawEntry::new(n, EntryType::File, 1, 1))),
        },
        &|| {},
    )
    .unwrap();
    assert_eq!(l.entries.len(), 1);
    assert_eq!(
        l.unreadable.as_ref().and_then(|e| e.raw_os_error()),
        Some(5)
    );
    assert!(l.strict().is_err());
    let all = Listing::collect(names, |_| Err(eio()), &|| {});
    assert_eq!(all.unwrap_err().raw_os_error(), Some(5));
    let empty = Listing::collect(Vec::<&str>::new(), |_| Err(eio()), &|| {}).unwrap();
    assert!(empty.entries.is_empty() && empty.unreadable.is_none());
}
