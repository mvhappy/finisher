//! Fitting `fin view` to `--budget <tokens>` (plan 4C Decision 46). The estimate is
//! `ceil(chars / 4)`. The budget caps the whole response `fin` prints (owner decision
//! 2026-11-09, review #43): the serialized JSON document with its newline, envelope, escaping
//! and `elided` entries included. `lineTokens` ([`tokens`]) is the lines alone, each counted
//! with its newline. While the response is over the budget, the steps run in this fixed order,
//! each over every section:
//!
//! 1. `detail`: item attributes go (source range, speed, transform, opacity, keyframe paths,
//!    effects, link, slots, a transition's curve, a marker's colour);
//! 2. `text`: caption text is cut to its first [`SHORT_WORDS`] words and `…`;
//! 3. `merge`: each run of two or more consecutive items of one kind on a track becomes one
//!    line (`c1…c4 4 clips`, first start to last end), and transitions inside a run go;
//! 4. `tail`: lines are dropped from the end (the last timeline first) until the rest fits.
//!    Headers and the envelope are never dropped, so a budget below them is exceeded.
//!
//! `elided` lists each step that changed something: `{step, items}` (how many lines it
//! changed or folded), and per timeline the tail cut `{step: "tail", timeline, from, to,
//! items}` (the dropped lines' first start and last end). The same project and budget give
//! the same lines.

use crate::Rational;
use serde_json::{Value, json};

use super::{Entry, Section, lines};

/// The words the `text` step keeps of a caption.
pub const SHORT_WORDS: usize = 3;

#[derive(Debug, Clone, Copy, Default)]
struct Level {
    no_detail: bool,
    short_text: bool,
}

fn line(e: &Entry, level: Level) -> String {
    let mut body = e.head.clone();
    if let Some(words) = &e.words {
        let text = if level.short_text && words.len() > SHORT_WORDS {
            format!("{}…", words[..SHORT_WORDS].join(" "))
        } else {
            words.join(" ")
        };
        body.push_str(&format!(" {text:?}"));
    }
    if !level.no_detail && !e.detail.is_empty() {
        body.push(' ');
        body.push_str(&e.detail.join(" "));
    }
    if !e.tail.is_empty() {
        body.push(' ');
        body.push_str(&e.tail);
    }
    format!("{}  {}  {body}", lines::span(e.start, e.end), e.label)
}

/// The lines; `headless[i]`: section `i`'s header was cut (the tail's last resort).
fn render_cut(sections: &[Section], level: Level, headless: &[bool]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        if !headless.get(i).copied().unwrap_or(false) {
            out.push(s.header.clone());
        }
        out.extend(s.entries.iter().map(|e| line(e, level)));
    }
    out
}

fn chars(l: &str) -> u64 {
    l.chars().count() as u64 + 1
}

/// `ceil(chars / 4)`, each line counted with its newline.
pub fn tokens(lines: &[String]) -> u64 {
    lines.iter().map(|l| chars(l)).sum::<u64>().div_ceil(4)
}

fn plural(kind: &str) -> String {
    match kind {
        "nested" => "nested".into(),
        k => format!("{k}s"),
    }
}

/// The `merge` step on one section; returns how many lines it folded.
fn merge(s: &mut Section) -> usize {
    let mut tracks: Vec<usize> = s.entries.iter().map(|e| e.track).collect();
    tracks.sort_unstable();
    tracks.dedup();
    let mut folded = 0;
    let mut merged: Vec<Entry> = s.entries.iter().filter(|e| e.rank == 0).cloned().collect();
    for t in tracks {
        let items: Vec<&Entry> = s
            .entries
            .iter()
            .filter(|e| e.rank == 1 && e.track == t)
            .collect();
        let mut groups: Vec<Vec<&Entry>> = Vec::new();
        for e in items {
            match groups.last_mut() {
                Some(g) if g[0].kind == e.kind => g.push(e),
                _ => groups.push(vec![e]),
            }
        }
        // Each folded item id, with the merged line it went into.
        let mut inside: Vec<(&str, usize)> = Vec::new();
        for g in &groups {
            if g.len() < 2 {
                merged.push(g[0].clone());
                continue;
            }
            folded += g.len();
            inside.extend(g.iter().map(|e| (e.id.as_str(), merged.len())));
            let (first, last) = (g[0], g[g.len() - 1]);
            let end = g.iter().filter_map(|e| e.end).max();
            merged.push(Entry {
                end,
                id: format!("{}…{}", first.id, last.id),
                head: format!(
                    "{}…{} {} {}",
                    first.id,
                    last.id,
                    g.len(),
                    plural(first.kind)
                ),
                detail: Vec::new(),
                words: None,
                tail: String::new(),
                folded: g.len(),
                ..first.clone()
            });
        }
        let run = |id: &str| inside.iter().find(|(i, _)| *i == id).map(|(_, n)| *n);
        for e in s.entries.iter().filter(|e| e.rank == 2 && e.track == t) {
            let within = e
                .between
                .as_ref()
                .and_then(|[a, b]| match (run(a), run(b)) {
                    (Some(x), Some(_)) => Some(x),
                    _ => None,
                });
            match within {
                Some(n) => {
                    folded += 1;
                    merged[n].folded += 1;
                }
                None => merged.push(e.clone()),
            }
        }
    }
    merged.sort_by_key(|a| (a.start, a.rank, a.track));
    s.entries = merged;
    folded
}

fn render(sections: &[Section], level: Level) -> Vec<String> {
    render_cut(sections, level, &[])
}

/// What a line adds to the response's `lines` array: its JSON string and a comma.
fn json_chars(l: &str) -> u64 {
    serde_json::to_string(l)
        .expect("strings serialize")
        .chars()
        .count() as u64
        + 1
}

/// The lines of `sections` whose whole response fits `budget` (none: every line at full
/// detail), and what was elided. `response(lines, elided)` is the size in chars of the response
/// those would make (the serialized JSON document with its newline), so the budget covers the
/// envelope (rev, timeline, token fields, `resolved`), JSON escaping and the `elided` entries
/// themselves, not only the lines (review #43).
pub fn fit(
    mut sections: Vec<Section>,
    budget: Option<u64>,
    response: &dyn Fn(&[String], &[Value]) -> u64,
) -> (Vec<String>, Vec<Value>) {
    let mut level = Level::default();
    let mut elided = Vec::new();
    let Some(budget) = budget else {
        return (render(&sections, level), elided);
    };
    let all = || sections.iter().flat_map(|s| s.entries.iter());
    let over =
        |s: &[Section], l: Level, e: &[Value]| response(&render(s, l), e).div_ceil(4) > budget;
    if over(&sections, level, &elided) {
        let n = all().filter(|e| !e.detail.is_empty()).count();
        level.no_detail = true;
        if n > 0 {
            elided.push(json!({ "step": "detail", "items": n }));
        }
    }
    if over(&sections, level, &elided) {
        let n = all()
            .filter(|e| e.words.as_ref().is_some_and(|w| w.len() > SHORT_WORDS))
            .count();
        level.short_text = true;
        if n > 0 {
            elided.push(json!({ "step": "text", "items": n }));
        }
    }
    if over(&sections, level, &elided) {
        let n: usize = sections.iter_mut().map(merge).sum();
        if n > 0 {
            elided.push(json!({ "step": "merge", "items": n }));
        }
    }
    let steps = elided.iter().map(|e| e["step"].clone()).collect::<Vec<_>>();
    if !over(&sections, level, &elided) {
        let shown = shown_steps(&sections, &steps, level);
        return (render(&sections, level), shown);
    }
    // The tail: first cut by an estimate (the response without lines, plus each line's JSON),
    // last timeline first; then one more line at a time while the exact response (with its
    // tail entries) is still over. Only once no item line is left do headers go, last
    // timeline first (`header: true` on its tail entry).
    let mut total = response(&[], &elided)
        + render(&sections, level)
            .iter()
            .map(|l| json_chars(l))
            .sum::<u64>();
    let mut cuts: Vec<Cut> = vec![None; sections.len()];
    for i in (0..sections.len()).rev() {
        while total.div_ceil(4) > budget {
            let first = cuts[i].is_none();
            let Some(c) = pop(&mut sections, &mut cuts, i, level) else {
                break;
            };
            total -= c;
            if first {
                // The timeline's tail entry, as it will be listed (its numbers may grow).
                let entry = &tails(&sections[i..=i], &cuts[i..=i], &[false])[0];
                total += entry.to_string().chars().count() as u64 + 1;
            }
        }
    }
    let mut headless = vec![false; sections.len()];
    loop {
        let e = shown_steps(&sections, &steps, level)
            .into_iter()
            .chain(tails(&sections, &cuts, &headless))
            .collect::<Vec<_>>();
        let out = render_cut(&sections, level, &headless);
        if response(&out, &e).div_ceil(4) <= budget {
            return (out, e);
        }
        if let Some(i) = sections.iter().rposition(|s| !s.entries.is_empty()) {
            pop(&mut sections, &mut cuts, i, level);
        } else if let Some(i) = headless.iter().rposition(|h| !h) {
            headless[i] = true;
        } else {
            // Nothing left but the envelope: a budget below it is exceeded.
            return (out, e);
        }
    }
}

/// The `detail`, `text` and `merge` entries of `elided` (those of `steps` that ran), counted
/// over the lines still shown: a step whose every line the tail cut is not listed.
fn shown_steps(sections: &[Section], steps: &[Value], level: Level) -> Vec<Value> {
    let shown = || sections.iter().flat_map(|s| s.entries.iter());
    steps
        .iter()
        .filter_map(|step| {
            let n: usize = match step.as_str()? {
                "detail" if level.no_detail => shown().filter(|e| !e.detail.is_empty()).count(),
                "text" if level.short_text => shown()
                    .filter(|e| e.words.as_ref().is_some_and(|w| w.len() > SHORT_WORDS))
                    .count(),
                "merge" => shown().map(|e| e.folded).sum(),
                _ => 0,
            };
            (n > 0).then(|| json!({ "step": step, "items": n }))
        })
        .collect()
}

/// The `tail` entries: per timeline cut, `{step, timeline, from, to, items}` (the dropped item
/// lines' first start and last end), plus `header: true` when its header went too (`from`
/// and `to` only when item lines went).
fn tails(sections: &[Section], cuts: &[Cut], headless: &[bool]) -> Vec<Value> {
    let mut out = Vec::new();
    for ((s, cut), &header) in sections.iter().zip(cuts).zip(headless) {
        let mut e = match cut {
            Some((from, to, n)) => {
                json!({ "step": "tail", "timeline": s.timeline, "from": from, "to": to, "items": n })
            }
            None if header => json!({ "step": "tail", "timeline": s.timeline, "items": 0 }),
            None => continue,
        };
        if header {
            e["header"] = json!(true);
        }
        out.push(e);
    }
    out
}

/// A timeline's tail cut so far: the dropped lines' first start, last end and count.
type Cut = Option<(Rational, Rational, usize)>;

/// Drops the last line of section `i` into its cut; returns what it added to the response.
fn pop(sections: &mut [Section], cuts: &mut [Cut], i: usize, level: Level) -> Option<u64> {
    let e = sections[i].entries.pop()?;
    let end = e.end.unwrap_or(e.start);
    cuts[i] = Some(match cuts[i] {
        None => (e.start, end, 1),
        Some((from, to, n)) => (from.min(e.start), to.max(end), n + 1),
    });
    Some(json_chars(&line(&e, level)))
}
