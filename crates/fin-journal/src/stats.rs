//! Per-window histograms of high-rate timings (plan D, Decision 19). The windows live on the
//! writer; samples reach it over the data channel. Each window writes one `rpc.stats` or
//! `eval.stats` entry per key, and holds at most [`MAX_KEYS`] keys per metric: the rest fold
//! into `other`.

use std::collections::BTreeMap;

use fin_core::events::payloads::{EvalStats, Histogram, RpcStats};
use fin_core::events::{AnyPayload, Name, Source};

use crate::Metric;
use crate::control::Sample;

/// Keys per metric per window.
pub const MAX_KEYS: usize = 256;

#[derive(Debug, Default)]
pub(crate) struct Windows {
    /// The open window's start (a multiple of its length), once a sample arrived.
    start: Option<u64>,
    /// Where the last closed window ended: a late sample joins the open window.
    closed_until: u64,
    keys: BTreeMap<(Metric, Name), Histogram>,
    per_metric: [usize; 2],
}

fn slot(m: Metric) -> usize {
    match m {
        Metric::Rpc => 0,
        Metric::Eval => 1,
    }
}

impl Windows {
    /// When the open window ends, if one is open.
    pub fn due_at(&self, window_ms: u64) -> Option<u64> {
        self.start.map(|s| s + window_ms)
    }

    pub fn add(&mut self, s: &Sample, window_ms: u64) {
        let at = s.ts.max(self.closed_until);
        let start = *self.start.get_or_insert(at - at % window_ms.max(1));
        let mut key = (s.metric, s.key);
        if !self.keys.contains_key(&key) {
            if self.per_metric[slot(s.metric)] >= MAX_KEYS {
                key.1 = Name::OTHER;
            }
            if !self.keys.contains_key(&key) {
                self.per_metric[slot(s.metric)] += 1;
            }
        }
        let h = self.keys.entry(key).or_insert_with(|| Histogram {
            start,
            window_ms,
            ..Histogram::default()
        });
        h.count += 1;
        h.max_micros = h.max_micros.max(s.micros);
        let b = Histogram::bucket(s.micros);
        if h.buckets.len() <= b {
            h.buckets.resize(b + 1, 0);
        }
        h.buckets[b] += 1;
        if let Some(code) = &s.code {
            h.errors.add(code.clone(), 1);
        }
    }

    /// Closes the window: one payload per key, with the source that emits it. `end`, when
    /// given, shortens the last window (a close before it was due).
    pub fn take(&mut self, end: Option<u64>, window_ms: u64) -> Vec<(Source, AnyPayload)> {
        if let Some(start) = self.start.take() {
            self.closed_until = end.unwrap_or(start + window_ms);
        }
        self.per_metric = [0, 0];
        std::mem::take(&mut self.keys)
            .into_iter()
            .map(|((metric, key), mut histogram)| {
                if let Some(end) = end {
                    histogram.window_ms = end.saturating_sub(histogram.start).max(1);
                }
                match metric {
                    Metric::Rpc => (Source::Finisherd, RpcStats { key, histogram }.into()),
                    Metric::Eval => (Source::FinNode, EvalStats { key, histogram }.into()),
                }
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}
