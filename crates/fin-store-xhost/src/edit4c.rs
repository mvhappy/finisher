//! The 4C rows of the harness (plan 4C Task 29 Steps 5–6; `tests/storage-xhost/`):
//!
//! - `editor` (the `edit4c` row): holds the lease and runs the edit script: a caption track,
//!   then `caption import` of Latin, Arabic, CJK and emoji words in `preset:bold` (Inter 700)
//!   with the `font.pin` ops `fin_fonts::plan::plan_pins` plans against the built-in catalog
//!   prepended (as `fin` does with `fonts.plan`), committed with a measurer over its own font
//!   cache, filled first by the real fetcher (seeded from `FIN_FONT_CACHE` when set); then a
//!   project style from `preset:bold` used by one caption, a `caption.setText` and an
//!   `item.move`. Once exported: `fonts` (what the fetch did), `plans4c` (the hashes and the
//!   frame plans, `fonts4c::plans`), the done marker (`--done-marker`), and close.
//! - `holder --hold-secs H` (the `mixed4c` and `newer4c` rows): waits until it may write
//!   (a takeover, the ordering rule), commits a small op every `--commit-every-ms` for H s,
//!   waits for the export, writes the done marker (`--done-marker`) and closes.
//! - `follower --until-done L [--fonts fetch|offline]`: a passive follower that stops one
//!   follow cycle after reaching the heads node L's done marker names (`followed`, with the
//!   hashes). With `--fonts fetch` it then fetches every pinned file itself into an emptied
//!   cache of its own server data (`fonts`); with `--fonts offline` it fetches nothing (the
//!   cache emptied too). Either way it then reports `plans4c`, measured over that cache.
//!
//! The done marker is `xhost-done-<label>.json` in the project folder: `{label, heads}`,
//! every branch's `[seq, txHash]` once all of it is exported.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use fin_core::{
    CaptionSetStyle, CaptionSetText, InsertMode, ItemMove, Op, ParamValue, Rational, StyleAdd,
    TrackAdd, TrackKind, Word,
};
use fin_store::MAIN;
use serde_json::{Value, json};

use crate::emit::Emitter;
use crate::fonts4c::{self, Fonts};
use crate::hashes;
use crate::history::Pacer;
use crate::node::{self, Node};
use crate::ops::asset_op;
use crate::{Args, FontsMode};

/// The import's words: one block per script (Latin, then Arabic, CJK and emoji), as the reel's
/// real-font pass.
const WORDS: &[(&str, &str, &str)] = &[
    ("Hello", "0", "1/2"),
    ("world", "1/2", "1"),
    ("مرحبا", "3/2", "2"),
    ("بالعالم", "2", "5/2"),
    ("你好", "3", "7/2"),
    ("世界", "7/2", "4"),
    ("🎉", "9/2", "5"),
    ("✨", "5", "11/2"),
];

fn r(s: &str) -> Result<Rational, String> {
    s.parse::<Rational>()
        .map_err(|e| format!("{s}: {}", e.message))
}

fn marker_path(dir: &Path, label: &str) -> std::path::PathBuf {
    dir.join(format!("xhost-done-{label}.json"))
}

/// Writes this node's done marker (temp, then rename).
fn write_marker(n: &mut Node) -> Result<(), String> {
    let h = hashes::of(&n.store, &n.db);
    let body = json!({ "label": n.a.label, "heads": h["heads"] });
    let path = marker_path(&n.a.dir, &n.a.label);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&body).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    let l = n.lease();
    n.em.emit("doneMarker", Some(&l), body);
    Ok(())
}

/// Node `label`'s done marker, once it can be read.
fn read_marker(dir: &Path, label: &str) -> Option<Value> {
    let b = std::fs::read(marker_path(dir, label)).ok()?;
    serde_json::from_slice(&b).ok()
}

/// Commits `ops` on `main`; `Err` when refused (the refusal is logged).
fn commit(n: &mut Node, step: &str, ops: Vec<Op>) -> Result<fin_core::CommittedTx, String> {
    let id = n.next_tx_id();
    n.commit_on(id, MAIN, ops, json!({ "phase": "edit4c", "step": step }))
        .ok_or_else(|| format!("{step}: commit refused"))
}

/// `ops` with the pins `plan_pins` plans for them prepended (logged as `planned`).
fn with_pins(n: &mut Node, fonts: &Fonts, step: &str, ops: Vec<Op>) -> Result<Vec<Op>, String> {
    let (_, head) = n.store.snapshot(MAIN).map_err(|e| e.to_string())?;
    let plan = fin_fonts::plan::plan_pins(&head, &ops, &fonts.fetcher.catalog())
        .map_err(|e| format!("{step}: plan_pins: {}: {}", e.code, e.message))?;
    let pins: Vec<Value> = plan
        .ops
        .iter()
        .map(|o| serde_json::to_value(o).unwrap_or(Value::Null))
        .map(|o| json!({ "key": o["key"], "sha256": o["pin"]["sha256"], "entry": o["pin"]["source"]["entry"], "fallback": o["fallback"] }))
        .collect();
    let notes: Vec<&str> = plan.notes.iter().map(|e| e.code.as_str()).collect();
    let l = n.lease();
    n.em.emit(
        "planned",
        Some(&l),
        json!({ "step": step, "pins": pins, "notes": notes }),
    );
    Ok(plan.ops.into_iter().chain(ops).collect())
}

fn binding(c: &fin_core::CommittedTx, k: &str) -> Result<String, String> {
    c.bindings
        .get(k)
        .cloned()
        .ok_or_else(|| format!("no binding {k}"))
}

pub fn editor(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let fonts = Fonts::open(&a.data, false, false)?;
    let mut n = node::open(a, em, "editor")?;
    n.store.set_measurer(Some(fonts.measurer()));
    let end = Instant::now() + Duration::from_secs_f64(a.secs);
    let every = Duration::from_millis(a.commit_every_ms);
    let mut p = Pacer::new();
    p.held(&mut n, end)?;
    // A caption track.
    let (_, head) = n.store.snapshot(MAIN).map_err(|e| e.to_string())?;
    let tl = head.timelines.first().ok_or("no timeline")?.id.clone();
    let add = TrackAdd {
        id: "$t".into(),
        timeline: tl,
        kind: TrackKind::Caption,
        index: None,
        name: None,
    };
    let c = commit(&mut n, "track", vec![add.into()])?;
    let track = binding(&c, "$t")?;
    p.until(&mut n, Instant::now() + every)?;
    // The import, its planned pins fetched first, so the commit measures.
    let words = WORDS
        .iter()
        .map(|(t, s, e)| {
            Ok(Word {
                text: (*t).into(),
                start: r(s)?,
                end: r(e)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let (_, head) = n.store.snapshot(MAIN).map_err(|e| e.to_string())?;
    let imp = fin_core::compose::caption_import(
        &head,
        &track,
        Rational::from_int(0),
        &words,
        "preset:bold",
    )
    .map_err(|e| format!("caption import: {}: {}", e.code, e.message))?;
    let ops = with_pins(&mut n, &fonts, "import", imp.ops)?;
    let mut planned = head.clone();
    for op in &ops {
        if let Op::FontPin(p) = op {
            planned.fonts.pins.insert(p.key.clone(), p.pin.clone());
        }
    }
    let fetched = fonts.fetch_pins(&planned, fonts4c::seed_dir().as_deref());
    let l = n.lease();
    n.em.emit(
        "fonts",
        Some(&l),
        json!({ "step": "import", "fetch": fetched }),
    );
    let c = commit(&mut n, "import", ops)?;
    let caps: Vec<String> = (1..=imp.echo["blocks"].as_u64().unwrap_or(0))
        .filter_map(|i| c.bindings.get(&format!("$cap{i}")).cloned())
        .collect();
    if caps.len() < 4 {
        return Err(format!(
            "the import made {} captions, expected 4",
            caps.len()
        ));
    }
    p.until(&mut n, Instant::now() + every)?;
    // A project style from preset:bold (Inter 700, smaller), used by the second caption.
    let style = StyleAdd {
        id: "$st".into(),
        name: "Bold small".into(),
        from: Some("preset:bold".into()),
        set: BTreeMap::from([("text.size".to_string(), ParamValue::Number(60.0))]),
    };
    let set = CaptionSetStyle {
        item: caps[1].clone(),
        style: "$st".into(),
        overrides: None,
    };
    let ops = with_pins(&mut n, &fonts, "style", vec![style.into(), set.into()])?;
    commit(&mut n, "style", ops)?;
    p.until(&mut n, Instant::now() + every)?;
    // A text edit and a move.
    let text = CaptionSetText {
        item: caps[0].clone(),
        text: "Hello brave world".into(),
    };
    let ops = with_pins(&mut n, &fonts, "setText", vec![text.into()])?;
    commit(&mut n, "setText", ops)?;
    p.until(&mut n, Instant::now() + every)?;
    let (_, head) = n.store.snapshot(MAIN).map_err(|e| e.to_string())?;
    let last = head
        .timelines
        .iter()
        .flat_map(|t| t.tracks.iter())
        .flat_map(|t| t.items.iter())
        .find(|i| i.id == caps[3])
        .ok_or("the emoji caption is gone")?;
    let mv = ItemMove {
        item: caps[3].clone(),
        track: track.clone(),
        at: last.start.checked_add(r("2")?).map_err(|e| e.message)?,
        mode: InsertMode::Overwrite,
    };
    commit(&mut n, "move", vec![mv.into()])?;
    p.exported(&mut n, end)?;
    report(&mut n, &fonts)?;
    if a.done_marker {
        write_marker(&mut n)?;
    }
    n.finish(true)
}

/// `plans4c`: the hashes and the frame plans of `main`, measured over `fonts`.
fn report(n: &mut Node, fonts: &Fonts) -> Result<(), String> {
    let (seq, project) = n.store.snapshot(MAIN).map_err(|e| e.to_string())?;
    let h = hashes::of(&n.store, &n.db);
    let m = fonts.measurer();
    let plans = fonts4c::plans(&project, Some(&*m));
    let pins: BTreeMap<&String, &String> = project
        .fonts
        .pins
        .iter()
        .map(|(k, p)| (k, &p.sha256))
        .collect();
    let l = n.lease();
    n.em.emit(
        "plans4c",
        Some(&l),
        json!({ "head": seq, "hashes": h, "pins": pins, "plans": plans }),
    );
    Ok(())
}

pub fn holder(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let mut n = node::open(a, em, "holder")?;
    let end = Instant::now() + Duration::from_secs_f64(a.secs);
    let every = Duration::from_millis(a.commit_every_ms);
    let mut p = Pacer::new();
    p.held(&mut n, end)?;
    let stop = Instant::now() + Duration::from_secs_f64(a.hold_secs.unwrap_or(0.0));
    while Instant::now() < stop.min(end) {
        let id = n.next_tx_id();
        let op = asset_op(&id);
        n.commit_on(id, MAIN, vec![op], json!({ "phase": "hold" }));
        p.until(&mut n, Instant::now() + every)?;
    }
    p.exported(&mut n, end)?;
    if a.done_marker {
        write_marker(&mut n)?;
    }
    n.finish(true)
}

/// Whether every branch `marker` names is at least at its seq here.
fn reached(n: &Node, marker: &Value) -> bool {
    marker["heads"].as_object().is_some_and(|h| {
        h.iter()
            .all(|(b, v)| n.store.head(b) >= v[0].as_u64().unwrap_or(u64::MAX))
    })
}

pub fn follow(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let fonts = match a.fonts {
        FontsMode::None => None,
        FontsMode::Fetch => Some(Fonts::open(&a.data, false, true)?),
        FontsMode::Offline => Some(Fonts::open(&a.data, true, true)?),
    };
    let mut n = node::open(a, em, "follower")?;
    if let Some(f) = &fonts {
        let l = n.lease();
        n.em.emit(
            "fontCache",
            Some(&l),
            json!({ "mode": format!("{:?}", a.fonts).to_lowercase(), "cleared": f.cleared, "dir": f.cache.dir().display().to_string() }),
        );
    }
    let label = a.until_done.clone().unwrap_or_default();
    let end = Instant::now() + Duration::from_secs_f64(a.secs);
    let mut p = Pacer::new();
    let mut marker: Option<Value> = None;
    let mut reached_at: Option<Instant> = None;
    let mut next_look = Instant::now();
    let followed = loop {
        p.step(&mut n)?;
        let now = Instant::now();
        if marker.is_none() && now >= next_look {
            marker = read_marker(&a.dir, &label);
            next_look = now + Duration::from_millis(1000);
            if let Some(m) = &marker {
                let l = n.lease();
                n.em.emit("markerSeen", Some(&l), m.clone());
            }
        }
        if let Some(m) = &marker {
            if reached_at.is_none() && reached(&n, m) {
                reached_at = Some(now);
            }
            if reached_at.is_none() {
                // Behind the marker: look at the folder now rather than at the next poll.
                n.store.poll_now_for_test();
            }
        }
        // `--settle-ms` after reaching the marker's heads, one full follow cycle, and stop.
        if reached_at.is_some_and(|t| now >= t + Duration::from_millis(a.settle_ms)) {
            n.store.poll_now_for_test();
            n.tick()?;
            break true;
        }
        if now >= end {
            break false;
        }
        std::thread::sleep(Duration::from_millis(a.tick_ms));
    };
    let h = hashes::of(&n.store, &n.db);
    let l = n.lease();
    n.em.emit(
        "followed",
        Some(&l),
        json!({ "until": label, "reached": followed, "marker": marker, "hashes": h }),
    );
    if let Some(f) = &fonts {
        if a.fonts == FontsMode::Fetch {
            let (_, project) = n.store.snapshot(MAIN).map_err(|e| e.to_string())?;
            let fetched = f.fetch_pins(&project, None);
            let l = n.lease();
            n.em.emit(
                "fonts",
                Some(&l),
                json!({ "step": "follow", "fetch": fetched }),
            );
        }
        report(&mut n, f)?;
    }
    n.finish(true)
}
