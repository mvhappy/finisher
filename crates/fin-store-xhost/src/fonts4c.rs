//! Fonts and frame plans for the 4C rows (plan 4C Task 29, `edit4c`): a node's own font cache
//! (`<server-data>/fonts`, as `finisherd` keeps it) over the built-in catalog, the real fetcher
//! filling it, `fin-text` measuring over it, and the frame plans every node of a round reports.
//!
//! - [`Fonts::open`]: the cache, a [`Fetcher`] (offline or not) and the measurer. With
//!   `cold`, the cache directory is emptied first, so a follower fetches every pinned file
//!   itself each round.
//! - [`Fonts::fetch_pins`]: every file the project pins, fetched now (or linked from a seed
//!   cache, `FIN_FONT_CACHE`, when one is given): what was already cached, seeded, downloaded
//!   or failed.
//! - [`plans`]: a frame plan at a time inside each caption item, each with `sha256` (the
//!   compact plan JSON without `textEngine`, as the reel e2e hashes it), `shapeSha256` (also
//!   without each caption's `layout` and `unmeasured`: what must not depend on fonts), and
//!   the captions' measured shape.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use fin_core::layout::TextMeasurer;
use fin_core::{ItemBody, Project, Rational, TrackKind};
use fin_fonts::{FetchOptions, Fetcher, FontCache, FontCatalog, sha256_hex};
use serde_json::{Value, json};

pub struct Fonts {
    pub cache: Arc<FontCache>,
    pub fetcher: Fetcher,
    pub measurer: Arc<fin_text::Measurer>,
    /// Files removed by a cold open.
    pub cleared: usize,
}

fn is_sha(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Fonts {
    /// The font cache of server data `data`, its fetcher (`offline`: it never downloads) and
    /// the measurer over it. `cold` empties the cache first.
    pub fn open(data: &Path, offline: bool, cold: bool) -> Result<Fonts, String> {
        let dir = data.join("fonts");
        let mut cleared = 0;
        if cold && dir.exists() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                cleared = rd
                    .flatten()
                    .filter(|e| is_sha(&e.file_name().to_string_lossy()))
                    .count();
            }
            std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let cache = Arc::new(FontCache::open(&dir).map_err(|e| e.message)?);
        let catalog = Arc::new(FontCatalog::builtin().clone());
        let opts = FetchOptions {
            offline,
            ..FetchOptions::default()
        };
        let fetcher = Fetcher::new(cache.clone(), catalog, opts);
        let measurer = Arc::new(fin_text::Measurer::new(cache.clone()));
        Ok(Fonts {
            cache,
            fetcher,
            measurer,
            cleared,
        })
    }

    pub fn measurer(&self) -> Arc<dyn TextMeasurer> {
        self.measurer.clone()
    }

    /// Links (else copies) `<seed>/<sha256>` into the cache; whether it did. The cache verifies
    /// the bytes before any use.
    fn seed(&self, sha: &str, seed: &Path) -> bool {
        let from = seed.join(sha);
        let to = self.cache.dir().join(sha);
        if !from.is_file() || to.exists() {
            return false;
        }
        std::fs::hard_link(&from, &to).is_ok() || std::fs::copy(&from, &to).is_ok()
    }

    /// Fetches every file `project` pins (seeded from `seed` first, when given). `{files:
    /// [{key, sha256, size, how, ms, error?}], cached, seeded, downloaded, failed, bytes, ms}`;
    /// `how` is `cached`, `seeded`, `downloaded` or `failed`.
    pub fn fetch_pins(&self, project: &Project, seed: Option<&Path>) -> Value {
        let t0 = Instant::now();
        let mut files = Vec::new();
        let (mut cached, mut seeded, mut downloaded, mut failed, mut bytes) = (0, 0, 0, 0, 0u64);
        let mut seen = std::collections::BTreeSet::new();
        for (key, pin) in &project.fonts.pins {
            if !seen.insert(pin.sha256.clone()) {
                continue;
            }
            let t = Instant::now();
            let (how, error) = if self.cache.has(&pin.sha256) {
                cached += 1;
                ("cached", None)
            } else if seed.is_some_and(|s| self.seed(&pin.sha256, s)) && self.cache.has(&pin.sha256)
            {
                seeded += 1;
                ("seeded", None)
            } else {
                match self.fetcher.fetch_now(&pin.sha256) {
                    Ok(()) if self.cache.has(&pin.sha256) => {
                        downloaded += 1;
                        bytes += pin.size;
                        ("downloaded", None)
                    }
                    Ok(()) => {
                        failed += 1;
                        ("failed", Some(json!("fetched but not in the cache")))
                    }
                    Err(e) => {
                        failed += 1;
                        (
                            "failed",
                            Some(
                                json!({ "code": e.code, "message": e.message, "details": e.details }),
                            ),
                        )
                    }
                }
            };
            files.push(json!({
                "key": key, "sha256": pin.sha256, "size": pin.size, "how": how,
                "ms": t.elapsed().as_millis() as u64, "error": error,
            }));
        }
        json!({
            "files": files, "cached": cached, "seeded": seeded, "downloaded": downloaded,
            "failed": failed, "bytes": bytes, "ms": t0.elapsed().as_millis() as u64,
            "cacheDir": self.cache.dir().display().to_string(),
        })
    }
}

/// `FIN_FONT_CACHE`, when it names a directory: a warmed cache to seed from.
pub fn seed_dir() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var_os("FIN_FONT_CACHE")?);
    d.is_dir().then_some(d)
}

/// A time inside each caption item of `timeline` (its start plus a fifth of a second, or its
/// midpoint when shorter), in time order.
pub fn caption_times(project: &Project, timeline: &str) -> Vec<Rational> {
    let fifth = Rational::new(1, 5).unwrap_or_default();
    let half = Rational::new(1, 2).unwrap_or_default();
    let mut ts: Vec<Rational> = project
        .timelines
        .iter()
        .filter(|t| t.id == timeline)
        .flat_map(|t| t.tracks.iter())
        .filter(|tr| tr.kind == TrackKind::Caption)
        .flat_map(|tr| tr.items.iter())
        .filter(|i| matches!(i.body, ItemBody::Caption { .. }))
        .filter_map(|i| {
            let off = if i.duration > fifth {
                fifth
            } else {
                i.duration.checked_mul(half).ok()?
            };
            i.start.checked_add(off).ok()
        })
        .collect();
    ts.sort();
    ts.dedup();
    ts
}

/// Removes every caption layer's `layout` and `unmeasured` (nested layers too).
fn strip_measurement(v: &mut Value) {
    if let Some(layers) = v.get_mut("layers").and_then(Value::as_array_mut) {
        for l in layers {
            strip_layer(l);
        }
    }
}

fn strip_layer(l: &mut Value) {
    if let Some(c) = l.get_mut("caption").and_then(Value::as_object_mut) {
        c.remove("layout");
        c.remove("unmeasured");
    }
    if let Some(ch) = l.get_mut("children").and_then(Value::as_array_mut) {
        for c in ch {
            strip_layer(c);
        }
    }
}

/// The measured shape of each caption layer (as the reel's golden `captions`).
fn captions_of(plan: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for l in plan["layers"].as_array().into_iter().flatten() {
        let Some(c) = l.get("caption") else { continue };
        let lay = &c["layout"];
        if lay.is_null() {
            out.push(json!({ "item": l["item"], "unmeasured": c["unmeasured"]["reason"], "keys": c["unmeasured"]["keys"] }));
        } else {
            out.push(json!({
                "item": l["item"], "lines": lay["lines"].as_array().map_or(0, Vec::len),
                "faces": lay["faces"], "width": lay["width"], "height": lay["height"],
                "words": lay["words"].as_array().map_or(0, Vec::len), "missing": lay["missing"],
            }));
        }
    }
    out
}

/// The frame plans of `project`'s first timeline at [`caption_times`], measured with `m`.
/// `{timeline, frames: [{t, sha256, shapeSha256, textEngine, captions, plan}]}` (or
/// `{error}`).
pub fn plans(project: &Project, m: Option<&dyn TextMeasurer>) -> Value {
    let Some(tl) = project.timelines.first().map(|t| t.id.clone()) else {
        return json!({ "error": "no timeline" });
    };
    let mut frames = Vec::new();
    for t in caption_times(project, &tl) {
        let plan = match fin_core::eval::eval_frame(project, &tl, t, m) {
            Ok(p) => p,
            Err(e) => return json!({ "error": format!("eval at {t}: {}: {}", e.code, e.message) }),
        };
        let mut v = serde_json::to_value(&plan).unwrap_or(Value::Null);
        let engine = v.as_object_mut().and_then(|o| o.remove("textEngine"));
        let sha = sha256_hex(&serde_json::to_vec(&v).unwrap_or_default());
        let mut shape = v.clone();
        strip_measurement(&mut shape);
        let shape_sha = sha256_hex(&serde_json::to_vec(&shape).unwrap_or_default());
        frames.push(json!({
            "t": t.to_string(), "sha256": sha, "shapeSha256": shape_sha, "textEngine": engine,
            "captions": captions_of(&v), "plan": v,
        }));
    }
    json!({ "timeline": tl, "frames": frames })
}
