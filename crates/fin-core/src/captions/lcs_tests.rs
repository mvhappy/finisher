//! The linear-memory [`lcs`] against the full-table walk it replaces (review finding #14),
//! kept here only as the reference: random inputs over small alphabets (many ties and
//! repeated words), every input up to a size over two letters, and long ones.

use proptest::prelude::*;

use super::lcs;

/// The full-table walk `caption.setText` used before (verbatim but for the key type).
fn matrix(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// The digits of `k` in base 2, `len` of them.
fn bits(k: u32, len: usize) -> Vec<u32> {
    (0..len).map(|d| (k >> d) & 1).collect()
}

#[test]
fn every_binary_input_up_to_six_by_six() {
    for n in 0..=6 {
        for m in 0..=6 {
            for x in 0..1u32 << n {
                for y in 0..1u32 << m {
                    let (a, b) = (bits(x, n), bits(y, m));
                    assert_eq!(lcs(&a, &b), matrix(&a, &b), "{a:?} {b:?}");
                }
            }
        }
    }
}

#[test]
fn edge_cases() {
    let cases: [(&[u32], &[u32]); 8] = [
        (&[], &[]),
        (&[1], &[]),
        (&[], &[1]),
        (&[1], &[1, 1]),
        (&[1, 1], &[1]),
        (&[2, 1], &[1, 2]),
        (&[1, 2, 3], &[4, 5, 6]),
        (&[1, 1, 1, 1], &[1, 1, 1, 1]),
    ];
    for (a, b) in cases {
        assert_eq!(lcs(a, b), matrix(a, b), "{a:?} {b:?}");
    }
}

#[test]
fn long_inputs_with_repeats() {
    // A deterministic pseudo-random walk over a few words, edited here and there.
    let mut s = 0x9e37_79b9_u32;
    let mut next = |k: u32| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s % k
    };
    for (n, m, k) in [(300, 280, 3), (500, 500, 7), (257, 640, 2), (900, 120, 40)] {
        let a: Vec<u32> = (0..n).map(|_| next(k)).collect();
        let b: Vec<u32> = (0..m)
            .map(|j| if next(4) == 0 { next(k) } else { a[j % n] })
            .collect();
        assert_eq!(lcs(&a, &b), matrix(&a, &b), "{n} {m} {k}");
    }
}

fn prop_config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);
    ProptestConfig {
        cases,
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(prop_config())]

    #[test]
    fn same_pairs_as_the_full_table(
        k in 1u32..6,
        a in prop::collection::vec(0u32..6, 0..40),
        b in prop::collection::vec(0u32..6, 0..40),
    ) {
        let (a, b): (Vec<u32>, Vec<u32>) =
            (a.iter().map(|x| x % k).collect(), b.iter().map(|x| x % k).collect());
        prop_assert_eq!(lcs(&a, &b), matrix(&a, &b));
    }

    #[test]
    fn same_pairs_on_edited_copies(
        a in prop::collection::vec(0u32..4, 0..80),
        edits in prop::collection::vec((0usize..80, 0u32..3, 0u32..5), 0..12),
    ) {
        // `b` is `a` with words replaced, inserted and deleted: the shape of a text edit.
        let mut b = a.clone();
        for (at, kind, word) in edits {
            let at = if b.is_empty() { 0 } else { at % b.len() };
            match kind {
                0 if !b.is_empty() => b[at] = word,
                1 => b.insert(at, word),
                _ if !b.is_empty() => {
                    b.remove(at);
                }
                _ => b.push(word),
            }
        }
        prop_assert_eq!(lcs(&a, &b), matrix(&a, &b));
    }
}
