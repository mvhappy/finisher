//! Addressing: turns the strings agents type (`12.4s`, `f300`, `+0.5s`, `after:c12`, `end`,
//! `2s..5s`) into exact `Rational` seconds against a project. Callers echo what they resolved.

use fin_core::{CoreError, Item, Project, Rational, Timeline, Track, parse_time};
use serde_json::json;

/// At most this many `alternatives` a `NOT_FOUND` lists (review batch L): the closest ids.
pub const ALTERNATIVES_CAP: usize = 10;

/// `NOT_FOUND` for `kind` `id`, listing the existing ids closest to `id` (by edit distance,
/// ties in project order). Past [`ALTERNATIVES_CAP`] the list is cut, `alternativesTotal`
/// counts every id, and the fix names the command that lists them all.
pub fn not_found(kind: &str, id: &str, alternatives: Vec<String>) -> CoreError {
    let total = alternatives.len();
    let lister = match kind {
        "track" => "fin track ls",
        "timeline" => "fin status",
        "bin" => "fin bin ls",
        _ => "fin view",
    };
    let mut details = json!({ "id": id, "kind": kind });
    if total > ALTERNATIVES_CAP {
        details["alternatives"] = json!(closest(id, alternatives));
        details["alternativesTotal"] = json!(total);
    } else {
        details["alternatives"] = json!(alternatives);
    }
    CoreError::new("NOT_FOUND", format!("{kind} {id} does not exist"))
        .with_details(details)
        .with_fix(format!(
            "use the id of an existing {kind}; `{lister}` lists them"
        ))
}

/// The [`ALTERNATIVES_CAP`] of `ids` closest to `id`.
fn closest(id: &str, ids: Vec<String>) -> Vec<String> {
    let mut ranked: Vec<(usize, usize, String)> = ids
        .into_iter()
        .enumerate()
        .map(|(i, c)| (distance(id, &c), i, c))
        .collect();
    ranked.sort();
    ranked.truncate(ALTERNATIVES_CAP);
    ranked.into_iter().map(|(_, _, c)| c).collect()
}

/// Levenshtein distance over chars.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if ca == *cb {
                prev
            } else {
                1 + prev.min(cur).min(row[j])
            };
            prev = cur;
        }
    }
    row[b.len()]
}

/// Tags a time error with the flag it came from.
fn for_field(mut e: CoreError, field: &str) -> CoreError {
    if let Some(map) = e.details.as_object_mut() {
        map.insert("field".to_string(), json!(field));
    }
    e
}

/// An absolute time (`12.4s`, `00:12.4`, `f300`, `62/5`) at the project rate.
#[cfg(test)]
fn time(project: &Project, field: &str, input: &str) -> Result<Rational, CoreError> {
    time_at(project.rate, field, input)
}

/// An absolute time whose frames (`f300`) count at `rate` (a timeline's).
pub fn time_at(rate: Rational, field: &str, input: &str) -> Result<Rational, CoreError> {
    parse_time(input, rate).map_err(|e| for_field(e, field))
}

/// A signed relative time for trims: `+0.5s`, `-1s`, `+f3`, frames counted at `rate` (a
/// timeline's). The sign is required so a delta is never mistaken for a position.
pub fn delta_at(rate: Rational, field: &str, input: &str) -> Result<Rational, CoreError> {
    let (negative, body) = match (input.strip_prefix('+'), input.strip_prefix('-')) {
        (Some(rest), _) => (false, rest),
        (_, Some(rest)) => (true, rest),
        _ => {
            return Err(CoreError::new(
                "INVALID_TIME",
                format!("trim delta {input:?} needs a sign"),
            )
            .with_details(json!({ "input": input, "field": field }))
            .with_fix(format!(
                "write +{input} to move the edge later or -{input} to move it earlier"
            )));
        }
    };
    let magnitude = parse_time(body, rate).map_err(|e| for_field(e, field))?;
    if magnitude < Rational::zero() {
        return Err(CoreError::new(
            "INVALID_TIME",
            format!("trim delta {input:?} has two signs"),
        )
        .with_details(json!({ "input": input, "field": field }))
        .with_fix("use a single sign, e.g. +0.5s or -1s"));
    }
    if negative {
        magnitude.checked_neg()
    } else {
        Ok(magnitude)
    }
}

/// Every track of every timeline.
fn tracks(project: &Project) -> impl Iterator<Item = &Track> {
    project.timelines.iter().flat_map(|tl| tl.tracks.iter())
}

fn clip_ids(project: &Project) -> Vec<String> {
    tracks(project)
        .flat_map(|t| t.items.iter().map(|c| c.id.clone()))
        .collect()
}

pub fn track_ids(project: &Project) -> Vec<String> {
    tracks(project).map(|t| t.id.clone()).collect()
}

pub fn timeline_ids(project: &Project) -> Vec<String> {
    project.timelines.iter().map(|t| t.id.clone()).collect()
}

/// The timeline `id` names (`--timeline`), or the first one.
pub fn timeline<'p>(project: &'p Project, id: Option<&str>) -> Result<&'p Timeline, CoreError> {
    match id {
        None => project
            .timelines
            .first()
            .ok_or_else(|| not_found("timeline", "(first)", Vec::new())),
        Some(id) => project
            .timelines
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| for_field(not_found("timeline", id, timeline_ids(project)), "timeline")),
    }
}

/// The timeline and track holding track `id`.
pub fn track<'p>(project: &'p Project, id: &str) -> Result<(&'p Timeline, &'p Track), CoreError> {
    project
        .timelines
        .iter()
        .find_map(|tl| tl.tracks.iter().find(|t| t.id == id).map(|t| (tl, t)))
        .ok_or_else(|| not_found("track", id, track_ids(project)))
}

/// The timeline, track and item of item `id`.
pub fn item<'p>(
    project: &'p Project,
    id: &str,
) -> Result<(&'p Timeline, &'p Track, &'p Item), CoreError> {
    project
        .timelines
        .iter()
        .find_map(|tl| {
            tl.tracks
                .iter()
                .find_map(|t| t.items.iter().find(|i| i.id == id).map(|i| (tl, t, i)))
        })
        .ok_or_else(|| not_found("item", id, clip_ids(project)))
}

/// The members of `item`'s link group (the item alone when it has none), named item first.
pub fn link_members(project: &Project, item: &Item) -> Vec<String> {
    let mut out = vec![item.id.clone()];
    if let Some(link) = &item.common.link {
        out.extend(
            tracks(project)
                .flat_map(|t| t.items.iter())
                .filter(|i| i.id != item.id && i.common.link.as_deref() == Some(link))
                .map(|i| i.id.clone()),
        );
    }
    out
}

/// The end of the last clip on `track` (0 when empty).
pub fn track_end(project: &Project, track: &str) -> Result<Rational, CoreError> {
    let t = tracks(project)
        .find(|t| t.id == track)
        .ok_or_else(|| not_found("track", track, track_ids(project)))?;
    t.items
        .iter()
        .try_fold(Rational::zero(), |acc, c| Ok(acc.max(c.end()?)))
}

/// An insert position on `track`: `end`, `after:<clip>`, or an absolute time.
pub fn position(
    project: &Project,
    track: &str,
    field: &str,
    input: &str,
) -> Result<Rational, CoreError> {
    if input == "end" {
        return track_end(project, track);
    }
    if let Some(id) = input.strip_prefix("after:") {
        let clip = tracks(project)
            .flat_map(|t| t.items.iter())
            .find(|c| c.id == id);
        return match clip {
            Some(c) => c.end(),
            None => Err(for_field(not_found("clip", id, clip_ids(project)), field)),
        };
    }
    time_at(rate_of(project, track), field, input)
}

/// The frame rate of the timeline holding `track` (the project rate when there is none).
pub fn rate_of(project: &Project, track: &str) -> Rational {
    self::track(project, track).map_or(project.rate, |(tl, _)| tl.rate)
}

/// `a..b`, `a..` or `..b` over the shown `timelines`; both ends absolute times. Rejects
/// `a >= b` (fin-core's [`fin_core::view::range`], shared with the plugin host's
/// `project.view`): frames count at the rate the shown timelines share, and a frame over
/// timelines of different rates is `USAGE` (`MIXED_RATES`) whose fix names the flags.
pub fn range(
    project: &Project,
    timelines: &[&Timeline],
    input: &str,
) -> Result<(Option<Rational>, Option<Rational>), CoreError> {
    fin_core::view::range(timelines, project.rate, input).map_err(|e| {
        if e.details["reason"] == "MIXED_RATES" {
            e.with_fix(
                "name one timeline with --timeline (or a track of it with --track), or write \
                 the range in seconds such as 2s..5s",
            )
        } else {
            e
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_core::{Asset, AssetKind, Item, new_project};

    /// Review batch L: a missing id among many lists the 10 closest, the total and the lister.
    #[test]
    fn not_found_caps_alternatives_to_the_closest() {
        let ids: Vec<String> = (1..=10_000).map(|i| format!("c{i}")).collect();
        let e = not_found("item", "c99999", ids);
        let alts = e.details["alternatives"].as_array().unwrap();
        assert_eq!(alts.len(), ALTERNATIVES_CAP);
        assert_eq!(alts[0], "c9999", "one edit away comes first");
        assert_eq!(e.details["alternativesTotal"], 10_000);
        assert!(e.fix.unwrap().contains("`fin view` lists them"));
        let few = not_found("track", "t9", vec!["t1".into(), "t2".into()]);
        assert_eq!(
            few.details["alternatives"],
            json!(["t1", "t2"]),
            "short lists as they are"
        );
        assert!(few.details.get("alternativesTotal").is_none());
        assert!(few.fix.unwrap().contains("`fin track ls`"));
        assert_eq!(distance("kitten", "sitting"), 3);
    }

    fn r(s: &str) -> Rational {
        s.parse().unwrap()
    }

    fn project() -> Project {
        let mut p = new_project("p1".into(), "Demo".into(), r("30")).unwrap();
        p.assets.insert(
            "a1".into(),
            Asset {
                id: "a1".into(),
                kind: AssetKind::Media,
                path: "/m.mp4".into(),
                fingerprint: "x".into(),
                bin: None,
                duration: Some(r("60")),
                video: None,
                audio: None,
                image: None,
                lottie: None,
                font: None,
                lut: None,
            },
        );
        let clip =
            |id: &str, start: &str, dur: &str| Item::clip(id, "a1", r(start), r("0"), r(dur));
        p.timelines[0].tracks[0].items = vec![clip("c1", "0", "10"), clip("c2", "10", "5/2")];
        p
    }

    #[test]
    fn times_use_the_project_rate() {
        let p = project();
        assert_eq!(time(&p, "at", "12.4s").unwrap(), r("62/5"));
        assert_eq!(time(&p, "at", "f45").unwrap(), r("3/2"));
        assert_eq!(time(&p, "at", "nope").unwrap_err().details["field"], "at");
    }

    #[test]
    fn deltas_need_a_sign() {
        let p = project();
        assert_eq!(delta_at(p.rate, "end", "+0.5s").unwrap(), r("1/2"));
        assert_eq!(delta_at(p.rate, "end", "-1s").unwrap(), r("-1"));
        assert_eq!(
            delta_at(p.rate, "end", "1s").unwrap_err().code,
            "INVALID_TIME"
        );
        assert_eq!(
            delta_at(p.rate, "end", "+-1s").unwrap_err().code,
            "INVALID_TIME"
        );
    }

    #[test]
    fn positions_resolve_end_and_after() {
        let p = project();
        assert_eq!(position(&p, "t1", "at", "end").unwrap(), r("25/2"));
        assert_eq!(position(&p, "t2", "at", "end").unwrap(), r("0"));
        assert_eq!(position(&p, "t1", "at", "after:c1").unwrap(), r("10"));
        assert_eq!(
            position(&p, "t1", "at", "after:c9").unwrap_err().code,
            "NOT_FOUND"
        );
        assert_eq!(
            position(&p, "t9", "at", "end").unwrap_err().code,
            "NOT_FOUND"
        );
    }

    #[test]
    fn ranges_parse_open_ends() {
        let p = project();
        let all: Vec<&Timeline> = p.timelines.iter().collect();
        let range = |input| range(&p, &all, input);
        assert_eq!(range("2s..5s").unwrap(), (Some(r("2")), Some(r("5"))));
        assert_eq!(range("..5s").unwrap(), (None, Some(r("5"))));
        assert_eq!(range("f45..").unwrap(), (Some(r("3/2")), None));
        assert_eq!(range("5s..2s").unwrap_err().code, "INVALID_RANGE");
        assert_eq!(range("5s").unwrap_err().code, "INVALID_RANGE");
    }

    #[test]
    fn range_frames_follow_the_shown_timelines() {
        let mut p = project();
        let mut tl2 = p.timelines[0].clone();
        tl2.id = "tl2".into();
        tl2.rate = r("30000/1001");
        p.timelines.push(tl2);
        let (tl1, tl2) = (&p.timelines[0], &p.timelines[1]);
        assert_eq!(range(&p, &[tl2], "f30..").unwrap().0, Some(r("1001/1000")));
        assert_eq!(range(&p, &[tl1], "f30..").unwrap().0, Some(r("1")));
        let e = range(&p, &[tl1, tl2], "..f30").unwrap_err();
        assert_eq!(
            (&*e.code, &e.details["reason"]),
            ("USAGE", &json!("MIXED_RATES"))
        );
        assert!(e.fix.unwrap().contains("--timeline"));
        assert_eq!(range(&p, &[tl1, tl2], "..1s").unwrap().1, Some(r("1")));
    }
}
