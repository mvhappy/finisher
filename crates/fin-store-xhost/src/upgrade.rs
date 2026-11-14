//! `upgrade` (plan 4C Task 29 Step 4, the `upgrade4c` row): a 4C server opens the server data
//! a 4B `brancher` left (the same server data, so its database is migrated in place) and the
//! project it made.
//!
//! Events, in order: `upgraded` (the heads and hashes as opened, the read-only reason, and
//! the `user_version` of each project database in the server data before and after the open),
//! `upgradeVerify` (`Store::verify`), `writable` (its own stopped lease re-taken), the undo's
//! `commit` (kind `undo`: the top unit on `main` of the author of `main`'s head record, a 4B
//! record), `undone` (the hashes after it), `exported`, then `final` and `closed`. A fresh 4C
//! `verify` of the folder alone must then report the `undone` hashes.

use std::path::Path;
use std::time::{Duration, Instant};

use fin_store::{HistoryReq, MAIN};
use serde_json::{Value, json};

use crate::Args;
use crate::emit::Emitter;
use crate::hashes;
use crate::history::Pacer;
use crate::node;

/// `{projectId: user_version}` of every project database in server data `data`.
fn db_versions(data: &Path) -> Value {
    let mut out = serde_json::Map::new();
    for e in std::fs::read_dir(data.join("projects"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let f = e.path().join("project.fdb");
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
        let v = rusqlite::Connection::open_with_flags(&f, flags)
            .and_then(|c| c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)));
        if let Ok(v) = v {
            out.insert(e.file_name().to_string_lossy().to_string(), json!(v));
        }
    }
    Value::Object(out)
}

pub fn run(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let before = db_versions(&a.data);
    let mut n = node::open(a, em, "upgrade")?;
    let db_path = n.store.db_path().display().to_string();
    let pid = n.store.project_id().to_string();
    let end = Instant::now() + Duration::from_secs_f64(a.secs);
    let h = hashes::of(&n.store, &n.db);
    let l = n.lease();
    let ro = n.store.read_only();
    n.em.emit(
        "upgraded",
        Some(&l),
        json!({
            "hashes": h, "readOnly": ro, "head": n.store.head(MAIN), "dbPath": db_path, "projectId": pid,
            "dbVersionsBefore": before, "dbVersionsAfter": db_versions(&a.data),
        }),
    );
    let rep = n.store.verify();
    n.em.emit(
        "upgradeVerify",
        Some(&l),
        json!({ "ok": rep.ok, "checks": rep.checks }),
    );
    let mut p = Pacer::new();
    p.held(&mut n, end)?;
    // The author of main's head record: the 4B brancher.
    let rows = n.db.chain()?;
    let author = rows
        .last()
        .and_then(|r| r[5].as_str())
        .ok_or("main has no records")?
        .to_string();
    let id = n.next_tx_id();
    let before = n.lease();
    let t = Instant::now();
    let r = n.store.undo(HistoryReq {
        tx_id: id.clone(),
        author: author.clone(),
        branch: MAIN.into(),
        dry_run: false,
        args_hash: None,
        to: None,
        after_seq: None,
    });
    let ms = t.elapsed().as_millis() as u64;
    match r {
        Ok(res) => {
            for c in &res.committed {
                let extra = json!({ "phase": "undo", "undone": res.undone, "of": author });
                n.accepted_event(c, &before, ms, Vec::new(), extra);
            }
        }
        Err(e) => {
            n.refused_event(&id, MAIN, e, ms);
            return Err("undo refused".into());
        }
    }
    let h = hashes::of(&n.store, &n.db);
    let l = n.lease();
    n.em.emit("undone", Some(&l), json!({ "hashes": h }));
    p.exported(&mut n, end)?;
    n.finish(true)
}
