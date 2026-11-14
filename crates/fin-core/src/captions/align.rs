//! New caption text over old timed words (`caption.setText`, spec §5.5): words that still match
//! keep their times, the rest share the time around them by characters. Exact rationals: the
//! shared-out boundaries are rounded to a fixed millisecond grid ([`GRID`]), so repeated edits
//! never grow the denominators.

use std::collections::BTreeMap;

use crate::captions::lcs::lcs;
use crate::error::CoreError;
use crate::model::Word;
use crate::time::{Rational, overflow};

/// Characters of a word for sharing time (at least 1, so an empty word still gets a share).
fn weight(text: &str) -> i64 {
    i64::try_from(text.chars().count())
        .unwrap_or(i64::MAX)
        .max(1)
}

/// The grid shared-out word boundaries are rounded to: 1/1000 s.
pub const GRID: i64 = 1000;

/// `x` rounded to the nearest multiple of 1/[`GRID`] (halves up).
fn snap(x: Rational) -> Result<Rational, CoreError> {
    let (n, d) = (i128::from(x.num()), i128::from(x.den()));
    let k = (2 * n * i128::from(GRID) + d).div_euclid(2 * d);
    let k = i64::try_from(k).map_err(|_| overflow())?;
    Rational::new(k, GRID)
}

/// `texts` spread over `[start, end]` in order, each word's share proportional to its
/// characters. Each boundary between two words is rounded to the [`GRID`] (kept inside the
/// span and in order, so a span under a millisecond a word can give empty words); the first
/// word starts at `start` and the last ends exactly at `end`. `TIME_OVERFLOW` when the
/// arithmetic overflows.
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
            let exact = start.checked_add(span.checked_mul(Rational::new(done, total)?)?)?;
            snap(exact)?.clamp(at, end)
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

/// `a` and `b` as numbers, equal where the strings are (for [`lcs`]).
fn interned(a: &[String], b: &[String]) -> (Vec<u32>, Vec<u32>) {
    fn number<'a>(ids: &mut BTreeMap<&'a str, u32>, list: &'a [String]) -> Vec<u32> {
        list.iter()
            .map(|s| {
                let next = u32::try_from(ids.len()).unwrap_or(u32::MAX);
                *ids.entry(s.as_str()).or_insert(next)
            })
            .collect()
    }
    let mut ids = BTreeMap::new();
    (number(&mut ids, a), number(&mut ids, b))
}

/// The words of `text` (split on whitespace) timed against `old`:
/// - a word matching an old one (by [`key`], along the longest common subsequence, [`lcs`]:
///   ties prefer the earliest pairing, so the result is deterministic) keeps its
///   times, with the new text;
/// - a run of new words between two matched ones shares the time between them (from the
///   earlier one's end to the later one's start; the old words' span before the first or after
///   the last match) by characters;
/// - a run with no time between its neighbours shares the previous matched word's span with
///   it (the next one's, at the start), so no word is empty and none overlap;
/// - with no match at all, every word shares the old words' whole span.
///
/// Shared-out times are rounded to the millisecond [`GRID`] ([`distribute`]); kept times are
/// the old words' own. Empty when `text` holds no word, or `old` is empty. `TIME_OVERFLOW`
/// when the arithmetic overflows.
pub fn align_text(old: &[Word], text: &str) -> Result<Vec<Word>, CoreError> {
    let new: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    let (Some(first), Some(last)) = (old.first(), old.last()) else {
        return Ok(Vec::new());
    };
    if new.is_empty() {
        return Ok(Vec::new());
    }
    let (a, b) = interned(
        &old.iter().map(|w| key(&w.text)).collect::<Vec<_>>(),
        &new.iter().map(|t| key(t)).collect::<Vec<_>>(),
    );
    let pairs = lcs(&a, &b);
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
