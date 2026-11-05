//! `verify`: the folder as a fresh reader resolves it, and the raw folder evidence.
//!
//! Events: `opened`, `verify` (`Store::verify`), `status`, `chain` (the resolved `main`
//! rows and the quarantine), `folder` (every segment with its records' seq, txId and txHash,
//! the lease record, claims and probes, and anything else found).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fin_core::CoreError;
use fin_store::lease::LeaseState;
use fin_store::scan::{self, Entry};
use fin_store::{MAIN, Store};
use serde_json::{Value, json};

use crate::Args;
use crate::db::Db;
use crate::emit::Emitter;

fn mtime_ms(m: &std::fs::Metadata) -> Option<i64> {
    let t = m.modified().ok()?;
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(d.as_millis()).ok()
}

fn hash(b: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(b).to_hex())
}

/// The lease files of `.finisher/`.
fn lease_files(dir: &std::path::Path) -> Value {
    let fin = dir.join(".finisher");
    let record = std::fs::read(fin.join(fin_store::lease::record::FILE))
        .ok()
        .map(|b| match fin_store::lease::record::decode(&b) {
            Some(r) => json!(r),
            None => json!({ "damaged": true, "len": b.len() }),
        });
    let mut claims = Vec::new();
    let mut probes = Vec::new();
    let mut temps = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&fin) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let meta = e.metadata().ok();
            let mt = meta.as_ref().and_then(mtime_ms);
            if let Some(ep) = name.strip_prefix("lease.claim-") {
                let owner = std::fs::read_to_string(e.path()).unwrap_or_default();
                claims.push(json!({ "epoch": ep.parse::<u64>().ok(), "name": name, "owner": owner.trim(), "mtimeMs": mt }));
            } else if let Some(id) = name.strip_prefix("lease.probe-") {
                probes.push(json!({ "server": id, "mtimeMs": mt }));
            } else if name.starts_with('.') || name.contains(".tmp") {
                temps.push(json!({ "name": name, "mtimeMs": mt }));
            }
        }
    }
    json!({ "record": record, "claims": claims, "probes": probes, "temps": temps })
}

/// Every verified segment with its records; other entries by kind.
fn segments(dir: &std::path::Path) -> Value {
    let entries = match scan::scan(dir) {
        Ok(e) => e,
        Err(e) => return json!({ "error": e.to_string() }),
    };
    let mut segs = Vec::new();
    let mut other = Vec::new();
    for e in entries {
        match e {
            Entry::Segment { branch, name, seg } => {
                let recs: Vec<Value> = seg
                    .records
                    .iter()
                    .map(|b| {
                        let r: Value = serde_json::from_slice(b).unwrap_or(Value::Null);
                        json!([r["seq"], r["txId"], hash(b), r["parentHash"], r["author"]])
                    })
                    .collect();
                let h = &seg.header;
                segs.push(json!({
                    "branch": branch, "name": name.to_string(), "hash": seg.hash,
                    "epoch": h.epoch, "writer": h.writer_id, "firstSeq": h.first_seq,
                    "lastSeq": h.last_seq, "prev": h.prev_segment_hash, "pack": h.pack,
                    "createdAt": h.created_at, "records": recs,
                }));
            }
            other_entry => other.push(
                format!("{other_entry:?}")
                    .chars()
                    .take(300)
                    .collect::<String>(),
            ),
        }
    }
    json!({ "segments": segs, "other": other })
}

pub fn run(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let t0 = Instant::now();
    let mut store = Store::open(&a.data, &a.dir).map_err(|e| {
        let e: CoreError = e.into();
        format!("open: {}: {}", e.code, e.message)
    })?;
    em.server = Some(store.server_id().to_string());
    let lease = store.lease_state();
    if matches!(lease, LeaseState::Held { .. }) {
        em.emit(
            "warning",
            Some(&lease),
            json!({ "what": "verifierAcquired" }),
        );
    }
    // Never acquires from here on, never exports.
    store.set_test_lease(0, false);
    em.emit(
        "opened",
        Some(&lease),
        json!({ "role": "verify", "projectId": store.project_id(), "storage": store.storage_kind(), "openMs": t0.elapsed().as_millis() as u64 }),
    );
    // Let directory caches settle, then import once more.
    let settle = Instant::now() + Duration::from_millis(a.settle_ms);
    while Instant::now() < settle {
        std::thread::sleep(Duration::from_millis(250));
    }
    store.poll_now_for_test();
    let r = store
        .tick_report(Instant::now())
        .map_err(|e| e.to_string())?;
    let applied = r
        .events
        .iter()
        .filter(|e| matches!(e, fin_store::StoreEvent::Applied(_)))
        .count();
    let report = store.verify();
    em.emit(
        "verify",
        None,
        json!({ "ok": report.ok, "checks": report.checks, "appliedAfterSettle": applied }),
    );
    let status = store.status();
    em.emit(
        "status",
        None,
        json!({ "status": status, "head": store.head(MAIN) }),
    );
    let db = Db::open(store.db_path())?;
    em.emit(
        "chain",
        None,
        json!({ "rows": db.chain()?, "quarantine": db.quarantine()? }),
    );
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    em.emit(
        "folder",
        None,
        json!({ "lease": lease_files(&a.dir), "scan": segments(&a.dir), "readAtMs": now_ms }),
    );
    Ok(())
}
