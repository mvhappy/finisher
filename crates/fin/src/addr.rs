//! Addressing: turns the strings agents type (`12.4s`, `f300`, `+0.5s`, `after:c12`, `end`,
//! `2s..5s`) into exact `Rational` seconds against a project. Callers echo what they resolved.

use fin_core::{CoreError, Project, Rational, Track, parse_time};
use serde_json::json;

fn not_found(kind: &str, id: &str, alternatives: Vec<String>) -> CoreError {
    CoreError::new("NOT_FOUND", format!("{kind} {id} does not exist"))
        .with_details(json!({ "id": id, "kind": kind, "alternatives": alternatives }))
        .with_fix(format!(
            "use the id of an existing {kind}; `fin view` lists them"
        ))
}

/// Tags a time error with the flag it came from.
fn for_field(mut e: CoreError, field: &str) -> CoreError {
    if let Some(map) = e.details.as_object_mut() {
        map.insert("field".to_string(), json!(field));
    }
    e
}

/// An absolute time (`12.4s`, `00:12.4`, `f300`, `62/5`) at the project rate.
pub fn time(project: &Project, field: &str, input: &str) -> Result<Rational, CoreError> {
    parse_time(input, project.rate).map_err(|e| for_field(e, field))
}

/// A signed relative time for trims: `+0.5s`, `-1s`, `+f3`. The sign is required so a delta is
/// never mistaken for a position.
pub fn delta(project: &Project, field: &str, input: &str) -> Result<Rational, CoreError> {
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
    let magnitude = parse_time(body, project.rate).map_err(|e| for_field(e, field))?;
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
    time(project, field, input)
}

/// `a..b`, `a..` or `..b`; both ends absolute times. Rejects `a >= b`.
pub fn range(
    project: &Project,
    input: &str,
) -> Result<(Option<Rational>, Option<Rational>), CoreError> {
    let Some((a, b)) = input.split_once("..") else {
        return Err(
            CoreError::new("INVALID_RANGE", format!("range {input:?} is not a..b"))
                .with_details(json!({ "input": input, "field": "range" }))
                .with_fix("write the range as 2s..5s, 2s.. or ..5s"),
        );
    };
    let end = |s: &str| {
        if s.is_empty() {
            Ok(None)
        } else {
            time(project, "range", s).map(Some)
        }
    };
    let (from, to) = (end(a)?, end(b)?);
    if let (Some(f), Some(t)) = (from, to)
        && f >= t
    {
        return Err(
            CoreError::new("INVALID_RANGE", format!("range {input:?} is empty"))
                .with_details(json!({ "input": input, "from": f, "to": t }))
                .with_fix("make the start of the range earlier than its end"),
        );
    }
    Ok((from, to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_core::{Asset, AssetKind, Item, new_project};

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
        assert_eq!(delta(&p, "end", "+0.5s").unwrap(), r("1/2"));
        assert_eq!(delta(&p, "end", "-1s").unwrap(), r("-1"));
        assert_eq!(delta(&p, "end", "1s").unwrap_err().code, "INVALID_TIME");
        assert_eq!(delta(&p, "end", "+-1s").unwrap_err().code, "INVALID_TIME");
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
        assert_eq!(range(&p, "2s..5s").unwrap(), (Some(r("2")), Some(r("5"))));
        assert_eq!(range(&p, "..5s").unwrap(), (None, Some(r("5"))));
        assert_eq!(range(&p, "5s..2s").unwrap_err().code, "INVALID_RANGE");
        assert_eq!(range(&p, "5s").unwrap_err().code, "INVALID_RANGE");
    }
}
