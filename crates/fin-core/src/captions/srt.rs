//! SubRip (`.srt`) cues → timed words, for `fin caption import --srt` (plan 4C Task 18).
//!
//! The input is UTF-8 text (the caller decodes the file and refuses other encodings); a
//! byte-order mark and CRLF line ends are accepted. A cue is an optional index line (digits), a
//! timing line `HH:MM:SS,mmm --> HH:MM:SS,mmm` and one or more text lines; cues are separated
//! by blank lines. In a time `.` is accepted for `,`, and the fraction may have 1 to 3 digits
//! (a decimal fraction: `,5` is 500 ms); anything after the end time, such as position hints,
//! is ignored. A cue that ends before it starts, or a line that breaks the format, is
//! `SRT_INVALID {line}`.
//!
//! The parser is lenient where real files are sloppy, and says so in [`Srt::warnings`]:
//! - only real tags are dropped: `<b>`, `<i>`, `<u>`, `<font …>` and their closing tags, and
//!   `{\…}` override codes (`{\an8}`); any other `<` or `{` is text;
//! - a cue whose text is only tags is dropped (`SRT_EMPTY_CUE`);
//! - cues are sorted by start (stably); a cue that overlaps the next one ends where the next
//!   starts (`SRT_OVERLAP_TRIMMED`);
//! - a cue shorter than a frame (after that trim) is dropped (`SRT_SHORT_CUE`).
//!
//! A cue's words split its time by their characters ([`super::align::distribute`], on the
//! millisecond grid).

use serde_json::json;

use crate::captions::align::distribute;
use crate::error::CoreError;
use crate::model::Word;
use crate::time::Rational;

/// Parsed SubRip: the words with absolute times, and what was repaired or dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Srt {
    pub words: Vec<Word>,
    pub warnings: Vec<SrtWarning>,
}

/// One repair: `code` (`SRT_EMPTY_CUE`, `SRT_OVERLAP_TRIMMED`, `SRT_SHORT_CUE`) at the cue's
/// timing line (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtWarning {
    pub code: &'static str,
    pub line: usize,
    pub message: String,
}

fn invalid(line: usize, why: &str) -> CoreError {
    CoreError::new("SRT_INVALID", format!("line {line}: {why}"))
        .with_details(json!({ "line": line }))
        .with_fix("fix the SRT at that line: cues are an index, `00:00:01,000 --> 00:00:02,500`, then text")
}

/// `HH:MM:SS,f` (1 to 3 fraction digits) as seconds.
fn timestamp(s: &str) -> Option<Rational> {
    let (hms, frac) = s.trim().split_once([',', '.'])?;
    let mut parts = hms.split(':');
    let mut field = |max: i64| -> Option<i64> {
        let p = parts.next()?;
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse::<i64>().ok().filter(|v| *v <= max)
    };
    let (h, m, sec) = (field(999)?, field(59)?, field(59)?);
    let digits = frac.len();
    if parts.next().is_some()
        || !(1..=3).contains(&digits)
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let ms = frac.parse::<i64>().ok()? * 10_i64.pow(3 - digits as u32);
    Rational::new(((h * 60 + m) * 60 + sec) * 1000 + ms, 1000).ok()
}

fn timing(line: &str) -> Option<(Rational, Rational)> {
    let (a, b) = line.split_once("-->")?;
    let b = b.split_whitespace().next()?;
    Some((timestamp(a)?, timestamp(b)?))
}

/// The length of the real tag at the start of `rest` (`<b>`, `</i>`, `<font color=…>`,
/// `{\an8}`), if it is one.
fn tag_len(rest: &str) -> Option<usize> {
    if let Some(body) = rest.strip_prefix("{\\") {
        return body.find('}').map(|i| i + 3);
    }
    let body = rest.strip_prefix('<')?;
    let close = body.find('>')?;
    let inner = body[..close].strip_prefix('/').unwrap_or(&body[..close]);
    let lower = inner.to_ascii_lowercase();
    let real = matches!(lower.as_str(), "b" | "i" | "u" | "font")
        || lower.starts_with("font ")
        || lower.starts_with("font\t");
    real.then_some(close + 2)
}

/// The text with its real tags and override codes removed.
fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        match tag_len(&text[i..]) {
            Some(n) => i += n,
            None => {
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

/// One cue as read: its timing line, times and words' texts.
struct Cue {
    line: usize,
    start: Rational,
    end: Rational,
    texts: Vec<String>,
}

fn warn(warnings: &mut Vec<SrtWarning>, code: &'static str, line: usize, message: String) {
    warnings.push(SrtWarning {
        code,
        line,
        message,
    });
}

/// The cues of `text` in file order; tag-only cues are dropped with a warning.
fn read_cues(text: &str, warnings: &mut Vec<SrtWarning>) -> Result<Vec<Cue>, CoreError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut cues = Vec::new();
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
        let line = i + 1;
        let Some(&timing_line) = lines.get(i) else {
            return Err(invalid(line, "expected a timing line"));
        };
        let (start, end) = timing(timing_line)
            .ok_or_else(|| invalid(line, "expected `HH:MM:SS,mmm --> HH:MM:SS,mmm`"))?;
        if end < start {
            return Err(invalid(line, "the cue ends before it starts"));
        }
        i += 1;
        let mut body = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() {
            body.push(strip(lines[i]));
            i += 1;
        }
        if body.is_empty() {
            return Err(invalid(line, "the cue has no text"));
        }
        let texts: Vec<String> = body
            .iter()
            .flat_map(|l| l.split_whitespace())
            .map(str::to_string)
            .collect();
        if texts.is_empty() {
            warn(
                warnings,
                "SRT_EMPTY_CUE",
                line,
                format!("line {line}: the cue holds only tags; dropped"),
            );
            continue;
        }
        cues.push(Cue {
            line,
            start,
            end,
            texts,
        });
    }
    Ok(cues)
}

/// The words of `text` (an SRT file, see the module docs), with absolute times; `frame` is
/// the shortest cue kept. `SRT_INVALID {line}` (1-based) names the first line that breaks the
/// format.
pub fn parse_srt(text: &str, frame: Rational) -> Result<Srt, CoreError> {
    let mut warnings = Vec::new();
    let mut cues = read_cues(text, &mut warnings)?;
    cues.sort_by_key(|c| c.start);
    for k in 1..cues.len() {
        let next = cues[k].start;
        let prev = &mut cues[k - 1];
        if prev.end > next {
            let line = prev.line;
            warn(
                &mut warnings,
                "SRT_OVERLAP_TRIMMED",
                line,
                format!(
                    "line {line}: the cue ends at {}, after the next starts; it now ends at {next}",
                    prev.end
                ),
            );
            prev.end = next;
        }
    }
    let mut words = Vec::new();
    for c in cues {
        if c.end.checked_sub(c.start)? < frame {
            let line = c.line;
            warn(
                &mut warnings,
                "SRT_SHORT_CUE",
                line,
                format!("line {line}: the cue is shorter than a frame; dropped"),
            );
            continue;
        }
        words.extend(distribute(&c.texts, c.start, c.end)?);
    }
    Ok(Srt { words, warnings })
}
