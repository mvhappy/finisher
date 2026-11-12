//! Text shaping and measurement over exact font bytes (4C Decisions 36–38).
//!
//! Pure: no I/O, no clocks, no system fonts, no platform fallback, no locale lookup; builds
//! for `wasm32-unknown-unknown`. Font bytes come from a [`FontBytes`] provider, looked up by the
//! SHA-256 a [`TextRequest`] names, so every machine with those bytes measures the same layout.
//! `cosmic-text` runs `no_std` (its float math goes through `libm`) with our own fallback list
//! and the fixed locale `en-US`; output is in 1/64 px integers.

mod fallback;
mod info;
mod ink;
mod intern;
mod measure;
mod system;
mod tracking;

use std::sync::{Arc, Mutex};

use fin_core::CoreError;
use fin_core::layout::{Measured, TextMeasurer, TextRequest};
use serde_json::json;

pub use info::{FaceInfo, MIN_OTHER, font_info};
pub use intern::{intern, interned, interned_count};

/// Font bytes by SHA-256 (lowercase hex). The provider returns only bytes it has verified to
/// have that hash; `None` when it has none.
pub trait FontBytes: Send + Sync {
    fn get(&self, sha256: &str) -> Option<Arc<[u8]>>;
}

/// Bumped whenever this crate changes a rule its layouts depend on (line breaking, bidi
/// handling, ink boxes, rounding, coverage), so a plan or golden names the code that produced
/// it.
pub const FIN_TEXT_REVISION: u32 = 3;

/// The code whose rules a layout depends on: this crate's version and [`FIN_TEXT_REVISION`],
/// then the shaping crates' versions from `Cargo.lock` (`cosmic-text`, and the `harfrust`,
/// `skrifa`, `read-fonts`, `unicode-bidi`, `unicode-linebreak`, `unicode-segmentation` and
/// `unicode-script` it builds with).
pub fn fingerprint() -> String {
    format!(
        "fin-text/{} rev/{FIN_TEXT_REVISION} cosmic-text/{} harfrust/{} skrifa/{} read-fonts/{} \
         unicode-bidi/{} unicode-linebreak/{} unicode-segmentation/{} unicode-script/{}",
        env!("CARGO_PKG_VERSION"),
        env!("FIN_TEXT_COSMIC_TEXT"),
        env!("FIN_TEXT_HARFRUST"),
        env!("FIN_TEXT_SKRIFA"),
        env!("FIN_TEXT_READ_FONTS"),
        env!("FIN_TEXT_UNICODE_BIDI"),
        env!("FIN_TEXT_UNICODE_LINEBREAK"),
        env!("FIN_TEXT_UNICODE_SEGMENTATION"),
        env!("FIN_TEXT_UNICODE_SCRIPT"),
    )
}

/// How many face lists keep a built `FontSystem` (least recently used out first).
pub const SYSTEMS_CACHED: usize = 8;

/// The [`TextMeasurer`]: one per process serves every project and branch.
pub struct Measurer {
    bytes: Arc<dyn FontBytes>,
    /// Most recently used first. A system is taken out while it measures, so two threads never
    /// share one; a second thread with the same faces builds its own.
    systems: Mutex<Vec<(system::Key, system::System)>>,
}

impl Measurer {
    pub fn new(bytes: Arc<dyn FontBytes>) -> Measurer {
        Measurer {
            bytes,
            systems: Mutex::new(Vec::new()),
        }
    }

    fn take(&self, key: &system::Key) -> Option<system::System> {
        let mut systems = self.systems.lock().unwrap_or_else(|e| e.into_inner());
        let i = systems.iter().position(|(k, _)| k == key)?;
        Some(systems.remove(i).1)
    }

    fn put(&self, key: system::Key, sys: system::System) {
        let mut systems = self.systems.lock().unwrap_or_else(|e| e.into_inner());
        systems.retain(|(k, _)| *k != key);
        systems.insert(0, (key, sys));
        systems.truncate(SYSTEMS_CACHED);
    }

    /// How many systems are cached now.
    pub fn cached_systems(&self) -> usize {
        self.systems.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

fn check(path: &str, v: f64, min: f64, max: f64, unit: &str) -> Result<(), CoreError> {
    if v.is_finite() && v >= min && v <= max {
        return Ok(());
    }
    Err(CoreError::new(
        "INVALID_RANGE",
        format!("{path} must be between {min} and {max} {unit}"),
    )
    .with_details(json!({ "path": path, "min": min, "max": max, "unit": unit })))
}

fn validate(req: &TextRequest) -> Result<(), CoreError> {
    let st = &req.style;
    check("size", st.size, 0.25, 4096.0, "px")?;
    check("lineHeight", st.line_height, 0.25, 16.0, "em")?;
    check("letterSpacing", st.letter_spacing, -0.5, 4.0, "em")?;
    check("weight", f64::from(st.weight), 1.0, 1000.0, "")?;
    if let Some(w) = req.max_width {
        check("maxWidth", w, 0.0, 1_000_000.0, "px")?;
    }
    if req.faces.is_empty() {
        return Err(CoreError::new("INVALID_OP", "a text request names no face")
            .with_details(json!({ "path": "faces" }))
            .with_fix("resolve the faces from the project's font pins (layout::faces_for) first"));
    }
    fin_core::layout::check_words(&req.words)
}

impl TextMeasurer for Measurer {
    /// `Unavailable` lists, in request order, every face file the provider lacks, or else the
    /// first file whose bytes do not parse as the requested face. `INVALID_RANGE` for a style
    /// value out of range; `INVALID_OP` for no faces or a word with a control character.
    fn measure(&self, req: &TextRequest) -> Result<Measured, CoreError> {
        validate(req)?;
        let mut faces: Vec<(String, u32)> = Vec::with_capacity(req.faces.len());
        for f in &req.faces {
            let key = (f.sha256.clone(), f.face);
            if !faces.contains(&key) {
                faces.push(key);
            }
        }
        let key = system::Key {
            faces,
            weight: req.style.weight,
        };
        let mut sys = match self.take(&key) {
            Some(sys) => sys,
            None => {
                let mut missing: Vec<String> = Vec::new();
                let mut data = Vec::with_capacity(key.faces.len());
                for (sha, _) in &key.faces {
                    match self.bytes.get(sha) {
                        Some(b) => data.push(b),
                        None if !missing.contains(sha) => missing.push(sha.clone()),
                        None => {}
                    }
                }
                if !missing.is_empty() {
                    return Ok(Measured::Unavailable { missing });
                }
                match system::build(&key, data) {
                    Ok(sys) => sys,
                    Err(sha) => return Ok(Measured::Unavailable { missing: vec![sha] }),
                }
            }
        };
        let layout = measure::layout(&mut sys, req);
        self.put(key, sys);
        Ok(Measured::Layout(layout))
    }

    fn fingerprint(&self) -> String {
        fingerprint()
    }
}
