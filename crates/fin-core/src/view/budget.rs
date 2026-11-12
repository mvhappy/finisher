//! Fitting `fin view` to `--budget <tokens>` (plan 4C Decision 46). The estimate is
//! `ceil(chars / 4)` over the lines, each counted with its newline. While the lines are over
//! the budget, the steps run in this fixed order, each over every section:
//!
//! 1. `detail`: item attributes go (source range, speed, transform, opacity, keyframe paths,
//!    effects, link, slots, a transition's curve, a marker's colour);
//! 2. `text`: caption text is cut to its first [`SHORT_WORDS`] words and `…`;
//! 3. `merge`: each run of two or more consecutive items of one kind on a track becomes one
//!    line (`c1…c4 4 clips`, first start to last end), and transitions inside a run go;
//! 4. `tail`: lines are dropped from the end (the last timeline first) until the rest fits.
//!    Headers are never dropped, so a budget below them is exceeded.
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

fn render(sections: &[Section], level: Level) -> Vec<String> {
    let mut out = Vec::new();
    for s in sections {
        out.push(s.header.clone());
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
        let mut inside: Vec<&str> = Vec::new();
        for g in &groups {
            if g.len() < 2 {
                merged.push(g[0].clone());
                continue;
            }
            folded += g.len();
            inside.extend(g.iter().map(|e| e.id.as_str()));
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
                ..first.clone()
            });
        }
        for e in s.entries.iter().filter(|e| e.rank == 2 && e.track == t) {
            let within = e
                .between
                .as_ref()
                .is_some_and(|[a, b]| inside.contains(&a.as_str()) && inside.contains(&b.as_str()));
            if within {
                folded += 1;
            } else {
                merged.push(e.clone());
            }
        }
    }
    merged.sort_by_key(|a| (a.start, a.rank, a.track));
    s.entries = merged;
    folded
}

/// The lines of `sections` within `budget` (none: every line at full detail), what was
/// elided, and the lines' token estimate.
pub fn fit(mut sections: Vec<Section>, budget: Option<u64>) -> (Vec<String>, Vec<Value>, u64) {
    let mut level = Level::default();
    let mut elided = Vec::new();
    let Some(budget) = budget else {
        let out = render(&sections, level);
        let t = tokens(&out);
        return (out, elided, t);
    };
    let all = || sections.iter().flat_map(|s| s.entries.iter());
    let over = |s: &[Section], l: Level| tokens(&render(s, l)) > budget;
    if over(&sections, level) {
        let n = all().filter(|e| !e.detail.is_empty()).count();
        level.no_detail = true;
        if n > 0 {
            elided.push(json!({ "step": "detail", "items": n }));
        }
    }
    if over(&sections, level) {
        let n = all()
            .filter(|e| e.words.as_ref().is_some_and(|w| w.len() > SHORT_WORDS))
            .count();
        level.short_text = true;
        if n > 0 {
            elided.push(json!({ "step": "text", "items": n }));
        }
    }
    if over(&sections, level) {
        let n: usize = sections.iter_mut().map(merge).sum();
        if n > 0 {
            elided.push(json!({ "step": "merge", "items": n }));
        }
    }
    let mut total: u64 = render(&sections, level).iter().map(|l| chars(l)).sum();
    let mut cuts: Vec<(String, Rational, Rational, usize)> = Vec::new();
    for s in sections.iter_mut().rev() {
        let mut cut: Option<(Rational, Rational, usize)> = None;
        while total.div_ceil(4) > budget {
            let Some(e) = s.entries.pop() else { break };
            total -= chars(&line(&e, level));
            let end = e.end.unwrap_or(e.start);
            cut = Some(match cut {
                None => (e.start, end, 1),
                Some((from, to, n)) => (from.min(e.start), to.max(end), n + 1),
            });
        }
        if let Some((from, to, n)) = cut {
            cuts.push((s.timeline.clone(), from, to, n));
        }
    }
    for (timeline, from, to, n) in cuts.into_iter().rev() {
        elided.push(
            json!({ "step": "tail", "timeline": timeline, "from": from, "to": to, "items": n }),
        );
    }
    let out = render(&sections, level);
    let t = tokens(&out);
    (out, elided, t)
}
