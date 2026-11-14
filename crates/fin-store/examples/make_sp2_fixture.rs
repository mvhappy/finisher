//! Generates `tests/fixtures/sp2/`: a project written by the sub-project 2 store, kept so later
//! storage formats can prove they migrate it to identical state.
//!
//! Run from the repo root with `cargo run -p fin-store --example make_sp2_fixture`. It replaces
//! the fixture directory. Only a checkout from before DB migrations existed writes the SP2 layout
//! (the fixture was generated at commit 30e2983); later stores write the current schema, so
//! do not regenerate the committed fixture from a newer checkout. The output is:
//! - `folder/finisher.project.json`;
//! - `server-data/projects/<id>/project.fdb` (WAL checkpointed, so a single file);
//! - `expected.json`: `{"rev", "project"}` = `Store::snapshot("main")` after the last commit.
//!
//! The history covers `asset.add`, two `clip.insert` (the second sent with a stale base so the
//! store rebases it), `clip.trim`, `clip.delete`, then 200 alternating inserts and deletes so a
//! DB snapshot exists at rev 200.

use std::fs;
use std::path::Path;

use fin_core::{Edge, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::{MAIN, SNAPSHOT_EVERY, Store, db_path};

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

/// Deterministic ULID-shaped tx ids, so reruns differ only in the project id and timestamps.
fn tx_id(n: u64) -> String {
    format!("01JC{n:022}")
}

fn tx(n: u64, base_rev: u64, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: tx_id(n),
        author: "fixture".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    }
}

fn insert(id: &str, at: i64, duration: i64) -> Op {
    fin_core::ItemInsert {
        id: id.into(),
        track: "t1".into(),
        at: secs(at),
        duration: secs(duration),
        mode: fin_core::InsertMode::Ripple,
        body: fin_core::ItemBody::Clip {
            asset: "a1".into(),
            source_in: secs(0),
            speed: fin_core::Rational::from_int(1),
        },
    }
    .into()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sp2");
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    let folder = root.join("folder");
    let data = root.join("server-data");

    let mut store = Store::create(&data, &folder, "SP2 fixture", secs(30), None)?;
    let mut n = 0;
    let mut next = || {
        n += 1;
        n
    };
    let asset = fin_core::AssetAdd::media(
        "$a",
        "media/clip.mov",
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
        "fixture-fingerprint",
    )
    .into();
    store.commit(tx(next(), 0, vec![asset]))?; // rev 1
    store.commit(tx(next(), 1, vec![insert("$c", 0, 10)]))?; // rev 2
    // Stale base: sent against rev 1, so the store rebases it over rev 2.
    let rebased = store.commit(tx(next(), 1, vec![insert("$c", 0, 5)]))?; // rev 3
    assert_eq!(rebased.tx.base_rev, 2, "the stale insert was rebased");
    let trimmed = rebased.bindings["$c"].clone();
    let trim = fin_core::ItemTrim {
        item: trimmed,
        edge: Edge::End,
        delta: secs(2),
        mode: fin_core::TrimMode::Ripple,
        alone: false,
    }
    .into();
    store.commit(tx(next(), 3, vec![trim]))?; // rev 4
    let (_, p) = store.snapshot(MAIN)?;
    let first = p.timelines[0].tracks[0].items[0].id.clone();
    store.commit(tx(
        next(),
        4,
        vec![
            fin_core::ItemDelete {
                item: first,
                mode: fin_core::DeleteMode::Ripple,
            }
            .into(),
        ],
    ))?; // rev 5

    for _ in 0..100 {
        let head = store.head(MAIN);
        let c = store.commit(tx(next(), head, vec![insert("$c", 0, 3)]))?;
        let clip = c.bindings["$c"].clone();
        store.commit(tx(
            next(),
            c.rev,
            vec![
                fin_core::ItemDelete {
                    item: clip,
                    mode: fin_core::DeleteMode::Ripple,
                }
                .into(),
            ],
        ))?;
    }
    let (rev, project) = store.snapshot(MAIN)?;
    assert!(rev >= SNAPSHOT_EVERY, "a DB snapshot exists at rev 200");
    let db = db_path(&data, store.project_id());
    drop(store);

    // Fold the WAL into the main file so a single `.fdb` is committed.
    let conn = rusqlite::Connection::open(&db)?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    conn.close().map_err(|(_, e)| e)?;
    for side in ["project.fdb-wal", "project.fdb-shm"] {
        let path = db.with_file_name(side);
        assert!(!path.exists(), "{} is left over", path.display());
    }

    let expected = serde_json::json!({ "rev": rev, "project": project });
    let mut bytes = serde_json::to_vec_pretty(&expected)?;
    bytes.push(b'\n');
    fs::write(root.join("expected.json"), bytes)?;
    println!("wrote {} (rev {rev})", root.display());
    Ok(())
}
