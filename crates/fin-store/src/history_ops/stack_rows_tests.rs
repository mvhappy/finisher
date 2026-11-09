//! `SqlLog::stack_records` and `crate::link_records` (plan 4B Task 20) read the stacks' and the
//! merged state's view of a branch from `txs` and `record_links` without parsing records; they
//! must equal the records parsed whole.

use fin_core::history::{LogView, StackRec};
use fin_core::{Edge, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};

use super::SqlLog;
use crate::{BranchCreate, HistoryReq, MAIN, MergeReq, Store};

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

fn edit(s: &mut Store, b: &str, id: &str, author: &str, ops: Vec<Op>) {
    let base_rev = s.head(b);
    s.commit(Tx {
        tx_id: id.into(),
        author: author.into(),
        branch: b.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    })
    .unwrap();
}

fn trim(clip: &str, d: i64) -> Op {
    fin_core::ItemTrim {
        item: clip.into(),
        edge: Edge::End,
        delta: secs(d),
        mode: fin_core::TrimMode::Ripple,
        alone: false,
    }
    .into()
}

fn history(id: &str, author: &str, b: &str) -> HistoryReq {
    HistoryReq {
        tx_id: id.into(),
        author: author.into(),
        branch: b.into(),
        dry_run: false,
        args_hash: None,
        to: None,
        after_seq: None,
    }
}

#[test]
fn stack_records_equal_the_parsed_records() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Store::create(
        &tmp.path().join("sd"),
        &tmp.path().join("p"),
        "Demo",
        secs(30),
        None,
    )
    .unwrap();
    let asset = fin_core::AssetAdd::media(
        "$a",
        "/m/a.mov",
        secs(600),
        Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
            display: None,
            rotation: None,
            pixel_aspect: None,
        }),
        None,
        "fp",
    )
    .into();
    edit(&mut s, MAIN, "s1", "setup", vec![asset]);
    let clips = (0..3)
        .map(|i| {
            fin_core::ItemInsert {
                id: format!("$c{i}"),
                track: "t1".into(),
                at: secs(10 * i),
                duration: secs(10),
                mode: fin_core::InsertMode::Ripple,
                body: fin_core::ItemBody::Clip {
                    asset: "a1".into(),
                    source_in: secs(0),
                    speed: fin_core::Rational::from_int(1),
                },
            }
            .into()
        })
        .collect();
    edit(&mut s, MAIN, "s2", "setup", clips);
    let side = s
        .create_branch(BranchCreate {
            tx_id: "b1".into(),
            author: "alice".into(),
            name: Some("side".into()),
            from: MAIN.into(),
            at: None,
            dry_run: false,
            args_hash: None,
        })
        .unwrap()
        .branch
        .id;
    edit(&mut s, &side, "e1", "alice", vec![trim("c1", 1)]);
    edit(&mut s, &side, "e2", "bob", vec![trim("c2", 2)]);
    s.undo(history("u1", "alice", &side)).unwrap();
    s.redo(history("r1", "alice", &side)).unwrap();
    edit(&mut s, MAIN, "m1", "carol", vec![trim("c3", 1)]);
    s.merge(MergeReq {
        tx_id: "mg".into(),
        author: "carol".into(),
        from: side.clone(),
        into: MAIN.into(),
        skip_conflicting: true,
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
    s.undo(history("u2", "carol", MAIN)).unwrap();
    s.redo(history("r2", "carol", MAIN)).unwrap();
    let log = SqlLog::new(&s.conn);
    for b in [MAIN, side.as_str()] {
        let fork = log.lineage(b).unwrap().fork_seq();
        let head = s.head(b);
        let cheap = log.stack_records(b, fork, head).unwrap();
        let whole: Vec<StackRec> = log
            .records(b, fork, head)
            .unwrap()
            .iter()
            .map(StackRec::of)
            .collect();
        assert_eq!(cheap, whole, "{b}");
        assert!(cheap.iter().any(|r| r.merge_id.is_some()) || b != MAIN);
        assert!(cheap.iter().any(|r| !r.undoes.is_empty()));
        assert!(cheap.iter().any(|r| r.redoes.is_some()));
        let links = |r: &fin_core::Record| {
            (
                r.tx_id.clone(),
                r.seq,
                r.kind,
                r.origin.clone(),
                r.origin_hash.clone(),
                r.from.clone(),
                r.merge_id.clone(),
                r.undoes.clone(),
                r.redoes.clone(),
                r.reverts.clone(),
            )
        };
        let light: Vec<_> = crate::link_records::records(&s.conn, b, fork, head)
            .unwrap()
            .into_iter()
            .map(|r| r.links_only())
            .map(|r| (r.branch.clone(), links(&r.record)))
            .collect();
        let parsed: Vec<_> = log
            .records(b, fork, head)
            .unwrap()
            .iter()
            .map(|r| (r.branch.clone(), links(&r.record)))
            .collect();
        assert_eq!(light, parsed, "{b}");
        for (_, l) in &parsed {
            let one = crate::link_records::find(&s.conn, &l.0)
                .unwrap()
                .unwrap()
                .links_only();
            assert_eq!(&links(&one.record), l);
        }
    }
}
