//! The longest common subsequence `caption.setText` times new words by
//! ([`super::align::align_text`]), in linear memory (review finding #14: the full table took
//! (n+1)·(m+1) cells, 100 MB for 5,000 words against 5,000).
//!
//! The pairing is the one the full table's walk gives. With `T[i][j]` the length of a common
//! subsequence of `a[i..]` and `b[j..]`, the walk starts at `(0, 0)` and at each step pairs
//! `a[i]` with `b[j]` when they are equal, else skips `a[i]` when that keeps the length
//! (`T[i + 1][j] >= T[i][j + 1]`), else skips `b[j]`; it stops at either end. It is
//! reproduced Hirschberg-style: split `a` at `mid`, find the column `c` where the walk first
//! reaches row `mid`, and solve `a[..mid]` against `b[..c]` and `a[mid..]` against `b[c..]`
//! on their own:
//! - below `mid` the walk is the sub-problem's own, as `T` there depends only on `a[mid..]`
//!   and `b[c..]`;
//! - above `mid` it is too: the walk's path to `(mid, c)` is a longest one there, and a step
//!   the walk does not take that is longest there would be longest for the whole as well,
//!   so the walk would have preferred it.
//!
//! `c` comes from one backward sweep: row `mid` of `T` from the rows below, then, row by row
//! up to 0, `T` with `G[i][j]` (the column at which the walk from `(i, j)` reaches row `mid`)
//! from the step the walk takes there. Two rows of `m + 1` cells at a time, and the
//! sub-problems' areas halve, so the work is at most twice the table's cells
//! (`2·n·m` steps; the `MAX_CAPTION_WORDS` policy bounds `n` and `m` on every edit).

/// The pairs `(i, j)` of `a[i] == b[j]` along the walk, in order.
pub(crate) fn lcs(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    // The walk pairs a common prefix as it goes (most edits keep one).
    let same = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let mut out: Vec<(usize, usize)> = (0..same).map(|k| (k, k)).collect();
    solve(&a[same..], &b[same..], same, same, &mut out);
    out
}

/// [`lcs`] of `a` and `b`, which start at `(ai, bj)` in the whole, appended to `out`.
fn solve(a: &[u32], b: &[u32], ai: usize, bj: usize, out: &mut Vec<(usize, usize)>) {
    if a.is_empty() || b.is_empty() {
        return;
    }
    if let [x] = a {
        // One row: the walk skips `b[j]` while `x` is still ahead, so it pairs the first `x`.
        if let Some(j) = b.iter().position(|y| y == x) {
            out.push((ai, bj + j));
        }
        return;
    }
    let mid = a.len() / 2;
    let c = crossing(a, b, mid);
    solve(&a[..mid], &b[..c], ai, bj, out);
    solve(&a[mid..], &b[c..], ai + mid, bj + c, out);
}

/// The column at which the walk from `(0, 0)` first reaches row `mid`.
fn crossing(a: &[u32], b: &[u32], mid: usize) -> usize {
    let m = b.len();
    // `t[j]` = T[i][j] for the row `i` being swept (`t[m]` is 0).
    let mut t = vec![0u32; m + 1];
    for &x in a[mid..].iter().rev() {
        let mut diag = 0; // T[i + 1][j + 1]
        for j in (0..m).rev() {
            let down = t[j]; // T[i + 1][j]
            t[j] = if x == b[j] {
                diag + 1
            } else {
                down.max(t[j + 1])
            };
            diag = down;
        }
    }
    // `g[j]` = G[i][j]; on row `mid` the walk is there, and at column `m` it has ended.
    let mut g: Vec<usize> = (0..=m).collect();
    for &x in a[..mid].iter().rev() {
        let (mut t_diag, mut g_diag) = (0, m);
        for j in (0..m).rev() {
            let (t_down, g_down) = (t[j], g[j]);
            if x == b[j] {
                (t[j], g[j]) = (t_diag + 1, g_diag);
            } else if t_down >= t[j + 1] {
                // Skipping `a[i]` keeps the length: the walk goes down.
                (t[j], g[j]) = (t_down, g_down);
            } else {
                (t[j], g[j]) = (t[j + 1], g[j + 1]);
            }
            (t_diag, g_diag) = (t_down, g_down);
        }
    }
    g[0]
}

#[cfg(test)]
#[path = "lcs_tests.rs"]
mod tests;
