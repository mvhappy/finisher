//! SubRip (`.srt`) cues → timed words, for `fin caption import --srt` (plan 4C Task 18).
//!
//! A cue is an optional index line (digits), a timing line `HH:MM:SS,mmm --> HH:MM:SS,mmm`
//! (`.` is accepted for `,`; anything after the end time, such as position hints, is ignored)
//! and one or more text lines; cues are separated by blank lines. A byte-order mark and CRLF
//! line ends are accepted. Tags (`<i>`, `<font …>`) and `{\…}` override codes are dropped. A
//! cue's words split its time by their characters, exactly ([`super::align::distribute`]).
//! Cues must not overlap (start ≥ the previous cue's end) and end at or after they start.

use serde_json::json;

use crate::captions::align::distribute;
use crate::error::CoreError;
use crate::model::Word;
use crate::time::Rational;

fn invalid(line: usize, why: &str) -> CoreError {
    CoreError::new("SRT_INVALID", format!("line {line}: {why}"))
        .with_details(json!({ "line": line }))
        .with_fix("fix the SRT at that line: cues are an index, `00:00:01,000 --> 00:00:02,500`, then text")
}

/// `HH:MM:SS,mmm` as seconds.
fn timestamp(s: &str) -> Option<Rational> {
    let (hms, ms) = s.trim().split_once([',', '.'])?;
    let mut parts = hms.split(':');
    let mut field = |max: i64| -> Option<i64> {
        let p = parts.next()?;
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse::<i64>().ok().filter(|v| *v <= max)
    };
    let (h, m, sec) = (field(999)?, field(59)?, field(59)?);
    if parts.next().is_some() || ms.len() != 3 || !ms.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms: i64 = ms.parse().ok()?;
    Rational::new(((h * 60 + m) * 60 + sec) * 1000 + ms, 1000).ok()
}

fn timing(line: &str) -> Option<(Rational, Rational)> {
    let (a, b) = line.split_once("-->")?;
    let b = b.split_whitespace().next()?;
    Some((timestamp(a)?, timestamp(b)?))
}

/// The text with tags and override codes removed.
fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skip: Option<char> = None;
    for c in text.chars() {
        match (skip, c) {
            (None, '<') => skip = Some('>'),
            (None, '{') => skip = Some('}'),
            (Some(end), c) if c == end => skip = None,
            (Some(_), _) => {}
            (None, c) => out.push(c),
        }
    }
    out
}

/// The words of `text` (an SRT file), with absolute times. `SRT_INVALID {line}` (1-based)
/// names the first line that breaks the format.
pub fn parse_srt(text: &str) -> Result<Vec<Word>, CoreError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut words = Vec::new();
    let mut last_end = Rational::zero();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        if !lines[i].contains("-->") {
            if !lines[i].trim().bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid(i + 1, "expected a cue number or a timing line"));
            }
            i += 1;
        }
        let at = i + 1;
        let Some(&line) = lines.get(i) else {
            return Err(invalid(at, "expected a timing line"));
        };
        let (start, end) =
            timing(line).ok_or_else(|| invalid(at, "expected `HH:MM:SS,mmm --> HH:MM:SS,mmm`"))?;
        if end < start {
            return Err(invalid(at, "the cue ends before it starts"));
        }
        if start < last_end {
            return Err(invalid(at, "the cue starts before the previous one ends"));
        }
        i += 1;
        let mut body = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() {
            body.push(strip(lines[i]));
            i += 1;
        }
        if body.is_empty() {
            return Err(invalid(at, "the cue has no text"));
        }
        let texts: Vec<String> = body
            .iter()
            .flat_map(|l| l.split_whitespace())
            .map(str::to_string)
            .collect();
        words.extend(distribute(&texts, start, end)?);
        last_end = end;
    }
    Ok(words)
}
