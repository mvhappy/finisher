//! `Tx.via` through the store (plan 8B Task 3, spec §3.8's round trip): a tagged commit is
//! rebased over a newer head, cherry-picked onto a branch, exported and imported by another
//! server, and every record made from it on the way keeps its `via`.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fin_core::tx::Via;
use fin_core::{CommittedTx, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::{BranchCreate, CherryPickReq, MAIN, MergeReq, Store, db_path};
use rusqlite::Connection;
use tempfile::TempDir;

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

fn asset(path: &str) -> Op {
    fin_core::AssetAdd::media(
        "$a",
        path,
        secs(600),
        Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
        }),
        None,
        "fp",
    )
    .into()
}

fn insert(at: i64) -> Op {
    fin_core::ItemInsert {
        id: "$c".into(),
        track: "t1".into(),
        at: secs(at),
        duration: secs(5),
        mode: fin_core::InsertMode::Ripple,
        body: fin_core::ItemBody::Clip {
            asset: "a1".into(),
            source_in: secs(0),
            speed: secs(1),
        },
    }
    .into()
}

fn via() -> Via {
    Via {
        plugin: "@acme/tools@1.2.0".into(),
        command: "trim-silence".into(),
    }
}

fn commit(
    s: &mut Store,
    id: &str,
    base_rev: u64,
    ops: Vec<Op>,
    via: Option<Box<Via>>,
) -> CommittedTx {
    s.commit(Tx {
        tx_id: id.into(),
        author: "alice".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via,
        ops,
    })
    .unwrap()
}

fn find(s: &Store, branch: &str, tx_id: &str) -> CommittedTx {
    s.ops_since(branch, 0)
        .unwrap()
        .into_iter()
        .find(|c| c.tx.tx_id == tx_id)
        .unwrap_or_else(|| panic!("{tx_id} on {branch}"))
}

/// Exports everything pending now (the 2 s delay passed).
fn export(s: &mut Store) {
    let t = Instant::now();
    s.tick_report(t).unwrap();
    let r = s.tick_report(t + Duration::from_secs(3)).unwrap().export;
    assert_eq!(r.error, None);
}

#[test]
fn via_survives_commit_rebase_pick_export_import() {
    let tmp = TempDir::new().unwrap();
    let data = |n: &str| -> PathBuf { tmp.path().join(format!("server-{n}")) };
    let dir = tmp.path().join("p");
    let mut a = Store::create(&data("a"), &dir, "Demo", secs(30), None).unwrap();
    // Server `b` starts from `a`'s DB at rev 0 and learns the rest only by import.
    let to = db_path(&data("b"), a.project_id());
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    Connection::open(a.db_path())
        .unwrap()
        .execute("VACUUM INTO ?1", [to.to_string_lossy()])
        .unwrap();

    commit(&mut a, "s1", 0, vec![asset("/m/a.mov")], None);
    let feature = a
        .create_branch(BranchCreate {
            tx_id: "b1".into(),
            author: "alice".into(),
            name: Some("feature".into()),
            from: MAIN.into(),
            at: None,
            dry_run: false,
            args_hash: None,
        })
        .unwrap()
        .branch
        .id;
    let base = a.head(MAIN);
    commit(&mut a, "s2", base, vec![insert(0)], None);
    // Built against `base`, committed over `s2`: the store rebases it.
    let p1 = commit(&mut a, "p1", base, vec![insert(10)], Some(Box::new(via())));
    assert!(p1.rev > base + 1, "rebased over s2");
    assert_eq!(p1.tx.via, Some(Box::new(via())));
    assert_eq!(find(&a, MAIN, "p1").tx.via, Some(Box::new(via())), "stored");
    assert_eq!(find(&a, MAIN, "s2").tx.via, None);

    a.cherry_pick(CherryPickReq {
        tx_id: "k1".into(),
        author: "dave".into(),
        origin: "p1".into(),
        into: feature.clone(),
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
    let k1 = find(&a, &feature, "k1");
    assert_eq!(k1.tx.author, "dave");
    assert_eq!(k1.tx.via, Some(Box::new(via())), "the pick carries it");

    export(&mut a);
    let mut b = Store::open(&data("b"), &dir).unwrap();
    b.set_test_lease(0, true);
    b.import().unwrap();
    assert_eq!(
        find(&b, MAIN, "p1").tx.via,
        Some(Box::new(via())),
        "imported"
    );
    assert_eq!(
        find(&b, &feature, "k1").tx.via,
        Some(Box::new(via())),
        "imported pick"
    );
    assert_eq!(find(&b, MAIN, "s2").tx.via, None);
}

#[test]
fn via_survives_merge() {
    let tmp = TempDir::new().unwrap();
    let mut s = Store::create(
        &tmp.path().join("sd"),
        &tmp.path().join("p"),
        "Demo",
        secs(30),
        None,
    )
    .unwrap();
    commit(&mut s, "s1", 0, vec![asset("/m/a.mov")], None);
    let feature = s
        .create_branch(BranchCreate {
            tx_id: "b1".into(),
            author: "alice".into(),
            name: Some("feature".into()),
            from: MAIN.into(),
            at: None,
            dry_run: false,
            args_hash: None,
        })
        .unwrap()
        .branch
        .id;
    let base = s.head(&feature);
    s.commit(Tx {
        tx_id: "f1".into(),
        author: "alice".into(),
        branch: feature.clone(),
        base_rev: base,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: Some(Box::new(via())),
        ops: vec![insert(0)],
    })
    .unwrap();
    // Main moves on, so the merge rebases the copy over it.
    let head = s.head(MAIN);
    commit(&mut s, "s2", head, vec![insert(20)], None);
    let report = s
        .merge(MergeReq {
            tx_id: "m1".into(),
            author: "carol".into(),
            from: feature.clone(),
            into: MAIN.into(),
            skip_conflicting: false,
            dry_run: false,
            args_hash: None,
            include: None,
            source_up_to: None,
        })
        .unwrap();
    assert_eq!(report.merged, vec!["f1".to_string()], "{report:?}");
    let copy = report
        .committed
        .iter()
        .find(|c| !c.tx.ops.is_empty())
        .expect("the merged copy");
    assert_eq!(copy.tx.author, "carol");
    assert_eq!(copy.tx.via, Some(Box::new(via())), "the merge carries it");
}
