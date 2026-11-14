//! Measurers for tests (feature `test-measurer`; never in a shipped build): [`FakeMeasurer`]
//! with fixed advances, [`PanicMeasurer`] for paths that must never measure, and
//! [`CountingMeasurer`] to count calls. Real-font layouts are pinned by `fin-text`'s corpora.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::CoreError;
use crate::layout::{LineBox, Measured, TextLayout, TextMeasurer, TextRequest, WordBox};

/// A measurer with fixed advances (1/64 px integers only): each character 0.5 em wide, a
/// space 0.25 em, one line unless `max_width` breaks it. Faces whose SHA-256 is in
/// `missing` are unavailable.
#[derive(Debug, Default)]
pub struct FakeMeasurer {
    pub missing: Vec<String>,
}

impl FakeMeasurer {
    /// A fake measurer lacking the bytes of `missing`.
    pub fn lacking(missing: &[&str]) -> FakeMeasurer {
        FakeMeasurer {
            missing: missing.iter().map(|s| s.to_string()).collect(),
        }
    }
}

fn line(n: usize, first: u32, end: u32, width: i32, height: i32) -> LineBox {
    LineBox {
        x: 0,
        top: n as i32 * height,
        width,
        height,
        baseline: n as i32 * height + height * 4 / 5,
        rtl: false,
        first_word: first,
        end_word: end,
    }
}

impl TextMeasurer for FakeMeasurer {
    fn measure(&self, req: &TextRequest) -> Result<Measured, CoreError> {
        let missing: Vec<String> = req
            .faces
            .iter()
            .filter(|f| self.missing.contains(&f.sha256))
            .map(|f| f.sha256.clone())
            .collect();
        if !missing.is_empty() {
            return Ok(Measured::Unavailable { missing });
        }
        let em = (req.style.size * 64.0).floor() as i32;
        let line_h = (req.style.size * req.style.line_height * 64.0).floor() as i32;
        let space = em / 4;
        let max = req.max_width.map(|w| (w * 64.0).floor() as i32);
        let mut lines: Vec<LineBox> = Vec::new();
        let mut words = Vec::new();
        let (mut x, mut first) = (0, 0u32);
        for (i, w) in req.words.iter().enumerate() {
            let width = w.chars().count() as i32 * em / 2;
            if x > 0 && max.is_some_and(|m| x + space + width > m) {
                lines.push(line(lines.len(), first, i as u32, x - space, line_h));
                (x, first) = (0, i as u32);
            }
            words.push(WordBox {
                line: lines.len() as u32,
                x,
                width,
            });
            x += width + space;
        }
        lines.push(line(
            lines.len(),
            first,
            req.words.len() as u32,
            (x - space).max(0),
            line_h,
        ));
        let width = lines.iter().map(|l| l.width).max().unwrap_or(0);
        let overflow =
            max.is_some_and(|m| width > m) || req.max_lines.is_some_and(|n| lines.len() as u32 > n);
        Ok(Measured::Layout(TextLayout {
            height: line_h * lines.len() as i32,
            lines,
            words,
            width,
            overflow,
            missing: Vec::new(),
            faces: req.faces.iter().map(|f| f.sha256.clone()).collect(),
        }))
    }

    fn fingerprint(&self) -> String {
        "fake/1".into()
    }
}

/// Panics when asked to measure: for paths that must never measure (replay, import,
/// rebuild).
#[derive(Debug, Default)]
pub struct PanicMeasurer;

impl TextMeasurer for PanicMeasurer {
    fn measure(&self, req: &TextRequest) -> Result<Measured, CoreError> {
        panic!("measured {:?} on a path that must never measure", req.words)
    }

    fn fingerprint(&self) -> String {
        "panic/1".into()
    }
}

/// [`FakeMeasurer`] that counts its calls.
#[derive(Debug, Default)]
pub struct CountingMeasurer {
    pub inner: FakeMeasurer,
    calls: AtomicUsize,
}

impl CountingMeasurer {
    /// How many requests it measured.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl TextMeasurer for CountingMeasurer {
    fn measure(&self, req: &TextRequest) -> Result<Measured, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.measure(req)
    }

    fn fingerprint(&self) -> String {
        self.inner.fingerprint()
    }
}
