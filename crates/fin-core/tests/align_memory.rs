//! Aligning a long caption's text keeps memory linear (review finding #14): the old longest
//! common subsequence held an (n+1)·(m+1) matrix, 100 MB for 5,000 words against 5,000. One
//! test per binary: the allocator counts every thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use fin_core::captions::align_text;
use fin_core::{Rational, Word};

struct Peak;

static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Peak {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = NOW.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        NOW.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOC: Peak = Peak;

fn words(n: usize) -> Vec<Word> {
    (0..n)
        .map(|i| Word {
            text: format!("w{}", i % 97),
            start: Rational::new(i as i64, 1000).unwrap(),
            end: Rational::new(i as i64 + 1, 1000).unwrap(),
        })
        .collect()
}

/// The peak bytes allocated beyond what was live before, and the result.
fn measured(old: &[Word], text: &str) -> (usize, Vec<Word>) {
    let base = NOW.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let got = align_text(old, text).unwrap();
    (PEAK.load(Ordering::SeqCst) - base, got)
}

#[test]
fn aligning_5000_words_against_5000_stays_linear() {
    let old = words(5_000);
    // Repeated words, every seventh one edited: many equal-length alignments to choose from.
    let text: Vec<String> = (0..5_000)
        .map(|i| {
            if i % 7 == 3 {
                format!("x{}", i % 5)
            } else {
                format!("w{}", (i + i / 50) % 97)
            }
        })
        .collect();
    let text = text.join(" ");
    let t = Instant::now();
    let (peak, got) = measured(&old, &text);
    let took = t.elapsed();
    assert_eq!(got.len(), 5_000);
    // The words themselves are about 0.5 MB; the matrix was 100 MB.
    assert!(peak < 8 << 20, "peak {peak} bytes");
    assert!(took.as_secs() < 60, "took {took:?}");
    // 20,000 old words against 5,000 new (a caption committed before the limit, on replay).
    let (peak, got) = measured(&words(20_000), &text);
    assert_eq!(got.len(), 5_000);
    assert!(peak < 16 << 20, "peak {peak} bytes");
}
