//! New caption text over old timed words (`caption.setText`, spec §5.5): words that still match
//! keep their times, the rest share the time around them by characters. Exact rationals.

use crate::error::CoreError;
use crate::model::Word;
use crate::time::Rational;

/// Characters of a word for sharing time (at least 1, so an empty word still gets a share).
fn weight(text: &str) -> i64 {
    i64::try_from(text.chars().count())
        .unwrap_or(i64::MAX)
        .max(1)
}

/// `texts` spread over `[start, end]` in order, each word's share proportional to its
/// characters; the last word ends exactly at `end`.
pub(crate) fn distribute(
    texts: &[String],
    start: Rational,
    end: Rational,
) -> Result<Vec<Word>, CoreError> {
    let total: i64 = texts.iter().map(|t| weight(t)).sum();
    let span = end.checked_sub(start)?;
    let mut out = Vec::with_capacity(texts.len());
    let mut done = 0;
    let mut at = start;
    for (i, t) in texts.iter().enumerate() {
        done += weight(t);
        let next = if i + 1 == texts.len() {
            end
        } else {
            start.checked_add(span.checked_mul(Rational::new(done, total)?)?)?
        };
        out.push(Word {
            text: t.clone(),
            start: at,
            end: next,
        });
        at = next;
    }
    Ok(out)
}

/// What two words are compared by: lowercase letters and digits, or the text itself when it
/// holds none (punctuation-only words).
fn key(text: &str) -> String {
    let k: String = text
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if k.is_empty() { text.to_string() } else { k }
}

/// The longest common subsequence of `a` and `b` as index pairs, in order (ties prefer the
/// earliest pairing, so the result is deterministic).
fn lcs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
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

/// The words of `text` (split on whitespace) timed against `old`:
/// - a word matching an old one (by [`key`], along the longest common subsequence) keeps its
///   times, with the new text;
/// - a run of new words between two matched ones shares the time between them (from the
///   earlier one's end to the later one's start; the old words' span before the first or after
///   the last match) by characters;
/// - a run with no time between its neighbours shares the previous matched word's span with
///   it (the next one's, at the start), so no word is empty and none overlap;
/// - with no match at all, every word shares the old words' whole span.
///
/// Empty when `text` holds no word, or `old` is empty. Errors only on arithmetic overflow,
/// which yields the words with the old span's start (never in practice).
pub fn align_text(old: &[Word], text: &str) -> Vec<Word> {
    align(old, text).unwrap_or_default()
}

fn align(old: &[Word], text: &str) -> Result<Vec<Word>, CoreError> {
    let new: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    let (Some(first), Some(last)) = (old.first(), old.last()) else {
        return Ok(Vec::new());
    };
    if new.is_empty() {
        return Ok(Vec::new());
    }
    let pairs = lcs(
        &old.iter().map(|w| key(&w.text)).collect::<Vec<_>>(),
        &new.iter().map(|t| key(t)).collect::<Vec<_>>(),
    );
    if pairs.is_empty() {
        return distribute(&new, first.start, last.end);
    }
    let mut out: Vec<Word> = Vec::with_capacity(new.len());
    // Runs of unmatched new words: (new range, the matched pair before, the one after).
    let mut j = 0;
    let mut before: Option<(usize, usize)> = None;
    for &(oi, nj) in pairs
        .iter()
        .chain(std::iter::once(&(usize::MAX, new.len())))
    {
        let after = (oi != usize::MAX).then_some((oi, nj));
        let run = &new[j..nj];
        if !run.is_empty() {
            let from = before.map_or(first.start, |(o, _)| old[o].end);
            let to = after.map_or(last.end, |(o, _)| old[o].start);
            if from < to {
                out.extend(distribute(run, from, to)?);
            } else if let Some(prev) = out.pop() {
                // Share the previous matched word's span.
                let mut texts = vec![prev.text];
                texts.extend(run.iter().cloned());
                out.extend(distribute(&texts, prev.start, prev.end)?);
            } else if let Some((o, n)) = after {
                // At the start: share the next matched word's span (it is pushed here).
                let mut texts: Vec<String> = run.to_vec();
                texts.push(new[n].clone());
                out.extend(distribute(&texts, old[o].start, old[o].end)?);
                before = after;
                j = nj + 1;
                continue;
            } else {
                out.extend(distribute(run, from, from)?);
            }
        }
        if let Some((o, n)) = after {
            out.push(Word {
                text: new[n].clone(),
                start: old[o].start,
                end: old[o].end,
            });
            j = n + 1;
        }
        before = after;
    }
    Ok(out)
}
