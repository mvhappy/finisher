//! Measurement through the store (plan 4C Task 20, Decision 36): commits, history writes and
//! recovery re-runs check caption overflow with the store's measurer; replay (open, import,
//! rebuild) never measures; missing bytes never block and leave no trace in the log; equal
//! pinned bytes give equal layouts whatever provider holds them, and a newer catalog does not
//! move a pinned layout.

mod integration_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use fin_core::eval::eval_frame;
use fin_core::layout::TextMeasurer;
use fin_core::layout::testing::{FakeMeasurer, PanicMeasurer};
use fin_core::{
    CaptionBlock, CaptionSetText, CoreError, FontFormat, FontPin, FontPinOp, FontRepin, FontSource,
    InsertMode, ItemBody, ItemInsert, Op, ParamValue, Project, Rational, Record, StyleAdd,
    StyleSet, TrackAdd, TrackKind, Word,
};
use fin_store::{HistoryReq, MAIN, Store};
use fin_text::FontBytes;
use integration_support::diverge::*;
use integration_support::*;
use rusqlite::Connection;

const LONG: &str = "abcdefghijkl";

fn sha(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

/// A pin of `key`'s face to `sha256` (catalog source, `catalog` version).
fn pin_of(family: &str, weight: u16, sha256: &str, catalog: &str) -> FontPin {
    FontPin {
        family: family.into(),
        weight,
        italic: false,
        face: 0,
        sha256: sha256.into(),
        size: 1000,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Catalog {
            entry: "test".into(),
            style: "regular".into(),
            catalog: catalog.into(),
        },
    }
}

fn key(family: &str, weight: u16) -> String {
    fin_core::font_key(family, weight, false)
}

fn caption(id: &str, at: i64, text: &str) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t3".into(),
        at: Rational::from_int(at),
        duration: Rational::from_int(2),
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words: text
                    .split_whitespace()
                    .map(|t| Word {
                        text: t.into(),
                        start: Rational::zero(),
                        end: Rational::zero(),
                    })
                    .collect(),
                style: "st1".into(),
                overrides: BTreeMap::new(),
                source: None,
            },
        },
    }
    .into()
}

fn set_text(item: &str, text: &str) -> Op {
    CaptionSetText {
        item: item.into(),
        text: text.into(),
    }
    .into()
}

fn size(v: f64) -> Op {
    StyleSet {
        style: "st1".into(),
        path: "text.size".into(),
        value: Some(ParamValue::Number(v)),
    }
    .into()
}

/// A caption track `t3`, style `st1` in `family`/`weight` (64 px, the generic 9:16 zone:
/// 972 px wide), that face pinned to `sha256`, and `cap1` "hi there".
fn setup(family: &str, weight: u16, sha256: &str) -> Vec<Op> {
    vec![
        TrackAdd {
            id: "$t".into(),
            timeline: "tl1".into(),
            kind: TrackKind::Caption,
            index: None,
            name: None,
        }
        .into(),
        StyleAdd {
            id: "$st".into(),
            name: "Mine".into(),
            from: None,
            set: [
                ("text.family".to_string(), ParamValue::Text(family.into())),
                (
                    "text.weight".to_string(),
                    ParamValue::Number(f64::from(weight)),
                ),
            ]
            .into(),
        }
        .into(),
        FontPinOp {
            key: key(family, weight),
            pin: pin_of(family, weight, sha256, "2026.11.1"),
            fallback: None,
        }
        .into(),
        caption("$c", 0, "hi there"),
    ]
}

fn fake_setup() -> Vec<Op> {
    setup("Inter", 700, &sha('a'))
}

/// Every record of the log (`committedAt` and `parentHash` cleared: they differ between
/// stores) re-encoded, with its diff; and the snapshots (project id cleared).
fn log_of(s: &Store) -> (Vec<(Vec<u8>, String)>, Vec<String>) {
    let db = Connection::open(s.db_path()).unwrap();
    let mut q = db
        .prepare("SELECT record_bytes, diff FROM txs ORDER BY branch, seq")
        .unwrap();
    let records = q
        .query_map([], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|r| {
            let (bytes, diff) = r.unwrap();
            let mut rec: Record = serde_json::from_slice(&bytes).unwrap();
            rec.committed_at = 0;
            rec.parent_hash = None;
            (fin_core::encode_record(&rec), diff)
        })
        .collect();
    let mut q = db
        .prepare("SELECT json FROM snapshots ORDER BY branch, seq")
        .unwrap();
    let snaps = q
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|j| {
            let mut p: Project = serde_json::from_str(&j.unwrap()).unwrap();
            p.id.clear();
            serde_json::to_string(&p).unwrap()
        })
        .collect();
    (records, snaps)
}

fn store_with(e: &Env, m: Option<Arc<dyn TextMeasurer>>) -> Store {
    let mut s = e.create("a");
    s.set_measurer(m);
    s
}

#[test]
fn missing_font_never_blocks_and_logs_nothing() {
    let edits = |s: &mut Store| {
        commit(s, "t1", fake_setup());
        // Overflows (1088 px), but its face's bytes are missing on the measuring store.
        commit(
            s,
            "t2",
            vec![caption("$d", 3, "supercalifragilisticexpialidocious")],
        );
        commit(s, "t3", vec![set_text("cap1", "hello"), size(80.0)]);
    };
    let (e1, e2) = (env(), env());
    let mut lacking = store_with(&e1, Some(Arc::new(FakeMeasurer::lacking(&[&sha('a')]))));
    let mut plain = store_with(&e2, None);
    edits(&mut lacking);
    edits(&mut plain);
    let (a, b) = (log_of(&lacking), log_of(&plain));
    assert_eq!(a.0.len(), 3);
    assert_eq!(a, b, "records, diffs and snapshots are byte-equal");
    // With the bytes, the same overflowing insert is refused, and a fitting edit's record is
    // byte-equal too.
    let e3 = env();
    let mut present = store_with(&e3, Some(Arc::new(FakeMeasurer::default())));
    commit(&mut present, "t1", fake_setup());
    let err = try_commit(
        &mut present,
        "t2",
        vec![caption("$d", 3, "supercalifragilisticexpialidocious")],
    )
    .unwrap_err();
    assert_eq!(err.code, "CAPTION_OVERFLOW");
    let e4 = env();
    let mut none = store_with(&e4, None);
    commit(&mut none, "t1", fake_setup());
    for s in [&mut present, &mut none] {
        commit(s, "t3", vec![set_text("cap1", "hello"), size(80.0)]);
    }
    assert_eq!(log_of(&present), log_of(&none));
}

#[test]
fn replay_never_calls_the_measurer() {
    let e = env();
    let mut s = store_with(&e, None);
    commit(&mut s, "t1", fake_setup());
    commit(&mut s, "t2", vec![set_text("cap1", LONG), size(200.0)]);
    let want = s.snapshot(MAIN).unwrap();
    e.flush(&mut s);
    drop(s);
    // Opening (replay to the head), rebuilding and following replay without measuring.
    let mut s = e.open("a");
    s.set_measurer(Some(Arc::new(PanicMeasurer)));
    assert_eq!(s.snapshot(MAIN).unwrap(), want);
    s.rebuild().unwrap();
    assert_eq!(s.snapshot(MAIN).unwrap(), want);
    let mut f = e.open("f");
    f.set_measurer(Some(Arc::new(PanicMeasurer)));
    e.poll(&mut f);
    assert_eq!(f.snapshot(MAIN).unwrap(), want);
}

fn tx_as(s: &Store, id: &str, author: &str, base_rev: u64, ops: Vec<Op>) -> fin_core::Tx {
    let _ = s;
    fin_core::Tx {
        tx_id: id.into(),
        author: author.into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: fin_core::SCHEMA_VERSION,
        args_hash: None,
        ops,
    }
}

fn conflict(e: fin_store::StoreError) -> String {
    let e = CoreError::from(e);
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    e.details["conflicts"][0]["reason"]
        .as_str()
        .unwrap_or_else(|| e.details["reason"].as_str().unwrap())
        .to_string()
}

#[test]
fn commits_and_history_recheck_overflow() {
    let e = env();
    let mut s = store_with(&e, Some(Arc::new(FakeMeasurer::default())));
    commit(&mut s, "t1", fake_setup());
    commit(&mut s, "t2", vec![set_text("cap1", LONG)]);
    commit(&mut s, "t3", vec![set_text("cap1", "ok")]);
    // Another author makes the text 200 px: "ok" still fits, the long word would not.
    let head = s.head(MAIN);
    s.commit(tx_as(&s, "t4", "b", head, vec![size(200.0)]))
        .unwrap();
    // A stale commit written before t4: fits at its base, not at the head.
    let stale = s.commit(tx_as(&s, "t5", "test", head, vec![set_text("cap1", LONG)]));
    assert_eq!(conflict(stale.unwrap_err()), "CAPTION_OVERFLOW");
    // Undoing t3 brings the long word back at 200 px.
    let undo = s.undo(HistoryReq {
        tx_id: "u1".into(),
        author: "test".into(),
        branch: MAIN.into(),
        to: None,
        dry_run: false,
        args_hash: None,
    });
    assert_eq!(conflict(undo.unwrap_err()), "CAPTION_OVERFLOW");
    // Without a measurer both go through.
    s.set_measurer(None);
    s.commit(tx_as(&s, "t5", "test", head, vec![set_text("cap1", LONG)]))
        .unwrap();
}

#[test]
fn recovery_rechecks_overflow() {
    let e = env();
    // A commits an overflowing caption while it measures nothing; it diverges.
    let Split { mut a, .. } = diverged_with_setup(&e, |a| {
        commit(
            a,
            "x1",
            vec![set_text("cap1", "supercalifragilisticexpialidocious")],
        );
    });
    // Recovered on a store that measures: the record conflicts.
    a.set_measurer(Some(Arc::new(FakeMeasurer::default())));
    let groups = a.recover_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert!(!groups[0].recoverable, "{groups:#?}");
    assert_eq!(groups[0].problems[0].reason, "CAPTION_OVERFLOW");
    // Without a measurer it is recoverable.
    a.set_measurer(None);
    assert!(a.recover_groups().unwrap()[0].recoverable);
}

fn diverged_with_setup(e: &Env, tail: impl FnOnce(&mut Store)) -> Split {
    let mut s = split(
        e,
        |a| {
            commit(a, "s1", fake_setup());
        },
        |_| {},
        winner,
    );
    tail(&mut s.a);
    s.a.set_test_lease(1, false);
    e.poll(&mut s.a);
    s.a.set_test_lease(3, true);
    s
}

/// Font bytes from a map (a provider holding verified bytes by SHA-256).
#[derive(Default)]
struct MapBytes(BTreeMap<String, Arc<[u8]>>);

impl MapBytes {
    fn with(bytes: &[u8]) -> (Arc<MapBytes>, String) {
        let sha = fin_fonts::sha256_hex(bytes);
        let mut m = MapBytes::default();
        m.0.insert(sha.clone(), bytes.into());
        (Arc::new(m), sha)
    }
}

impl FontBytes for MapBytes {
    fn get(&self, sha256: &str) -> Option<Arc<[u8]>> {
        self.0.get(sha256).cloned()
    }
}

fn plan_json(p: &Project, m: &dyn TextMeasurer) -> String {
    let plan = eval_frame(p, "tl1", Rational::from_int(1), Some(m)).unwrap();
    serde_json::to_string(&plan).unwrap()
}

/// TestSans pinned on a fresh store, its family and weight as `font_info` reads them.
fn sans_project(e: &Env, sha256: &str, bytes: &[u8]) -> (Store, String, u16) {
    let info = fin_text::font_info(bytes).unwrap().remove(0);
    let mut s = store_with(e, None);
    commit(&mut s, "t1", setup(&info.family, info.weight, sha256));
    (s, info.family, info.weight)
}

#[test]
fn same_pins_same_layout_across_providers() {
    let bytes = fin_testfonts::test_sans();
    let (one, sha) = MapBytes::with(&bytes);
    let (two, _) = MapBytes::with(&bytes);
    let e = env();
    let (s, _, _) = sans_project(&e, &sha, &bytes);
    let (_, p) = s.snapshot(MAIN).unwrap();
    let (m1, m2) = (fin_text::Measurer::new(one), fin_text::Measurer::new(two));
    let a = plan_json(&p, &m1);
    assert!(a.contains("\"layout\":{"), "{a}");
    assert_eq!(a, plan_json(&p, &m2));
}

#[test]
fn catalog_update_keeps_pinned_layout() {
    let old = fin_testfonts::test_sans();
    let (bytes, sha) = MapBytes::with(&old);
    let e = env();
    let (mut s, family, weight) = sans_project(&e, &sha, &old);
    let m = fin_text::Measurer::new(bytes.clone());
    let before = plan_json(&s.snapshot(MAIN).unwrap().1, &m);
    // A newer catalog with another file for the same face changes nothing pinned: the
    // project's pin names the old bytes until `font.repin`.
    let newer = fin_testfonts::test_rtl();
    let mut both = MapBytes::default();
    both.0.insert(sha.clone(), old.clone().into());
    let new_sha = fin_fonts::sha256_hex(&newer);
    both.0.insert(new_sha.clone(), newer.into());
    let m2 = fin_text::Measurer::new(Arc::new(both));
    assert_eq!(plan_json(&s.snapshot(MAIN).unwrap().1, &m2), before);
    // A repin moves it.
    commit(
        &mut s,
        "t2",
        vec![
            FontRepin {
                key: key(&family, weight),
                pin: pin_of(&family, weight, &new_sha, "2026.12.1"),
            }
            .into(),
        ],
    );
    assert_ne!(plan_json(&s.snapshot(MAIN).unwrap().1, &m2), before);
}
