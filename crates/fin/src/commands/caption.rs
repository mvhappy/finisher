//! `fin caption import|set-text|split|merge|style` (plan 4C Task 24).
//!
//! `import` reads timed words (`--words <file.json>`: `[{text, start, end}]`, times as
//! rationals or decimals, converted exactly) or SubRip (`--srt <file>`, through
//! `fin_core::captions::parse_srt` with the timeline's frame as the shortest cue; its repairs
//! are echoed as `resolved.warnings`), and expands through `compose::caption_import` into one
//! `item.insert` per block (`$cap1`, `$cap2`, …) in one tx, transcript time 0 at `--at`
//! (default 0). Font pins are planned through `fonts.plan` like every caption write; the
//! server's overflow check refuses a block that does not fit (`CAPTION_OVERFLOW`).

use std::collections::BTreeMap;

use clap::{Args, Subcommand};
use fin_core::captions::parse_srt;
use fin_core::compose::caption_import;
use fin_core::params::ScopeOf;
use fin_core::{
    CaptionMerge, CaptionSetStyle, CaptionSetText, CaptionSplit, CoreError, Op, Rational, Word,
    frame_duration, parse_time,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::builder::rename_json;
use super::clip::{merge_echo, one_of};
use super::param::parse_sets;
use crate::addr;
use crate::output::usage;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum CaptionCmd {
    Import(ImportArgs),
    SetText(SetTextArgs),
    Split(SplitArgs),
    Merge(MergeArgs),
    Style(StyleArgs),
}

/// Import timed words or SubRip as caption blocks on a caption track; binds $cap1, $cap2, ….
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportArgs {
    #[arg(long, value_name = "t")]
    pub track: String,
    /// `[{text, start, end}]`, times as rationals ("62/5") or decimals.
    #[arg(
        long,
        value_name = "file.json",
        required_unless_present = "srt",
        conflicts_with = "srt"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
    #[arg(long, value_name = "file.srt")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srt: Option<String>,
    /// A preset (`preset:bold`) or project style.
    #[arg(long, value_name = "style")]
    pub style: String,
    /// Where the transcript's time 0 lands (default 0).
    #[arg(long, value_name = "time")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

/// Replace a caption's text; its words are re-timed over the caption.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTextArgs {
    #[arg(value_name = "capN")]
    pub item: String,
    #[arg(value_name = "text", allow_hyphen_values = true)]
    pub text: String,
}

/// Cut a caption in two before word `word` (0-based); binds $cap.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitArgs {
    #[arg(value_name = "capN")]
    pub item: String,
    #[arg(long, value_name = "n")]
    pub word: u32,
}

/// Join a caption with the adjacent caption `with` (which goes away).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeArgs {
    #[arg(value_name = "capN")]
    pub item: String,
    #[arg(value_name = "capN")]
    pub with: String,
}

/// Give a caption another style; `set` replaces its per-caption overrides, `clear` drops them.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StyleArgs {
    #[arg(value_name = "capN")]
    pub item: String,
    #[arg(value_name = "style")]
    pub style: String,
    #[arg(long = "set", value_name = "p=v", allow_hyphen_values = true)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<String>,
    #[arg(long, conflicts_with = "set")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear: bool,
}

pub fn name(cmd: &CaptionCmd) -> &'static str {
    match cmd {
        CaptionCmd::Import(_) => "caption.import",
        CaptionCmd::SetText(_) => "caption.set-text",
        CaptionCmd::Split(_) => "caption.split",
        CaptionCmd::Merge(_) => "caption.merge",
        CaptionCmd::Style(_) => "caption.style",
    }
}

/// The hashed input of a caption command: an import's file is the file it names (canonical
/// when it exists), as `asset add` hashes its path.
pub fn args(cmd: &CaptionCmd) -> Value {
    let mut v = json!(cmd);
    if let CaptionCmd::Import(a) = cmd {
        for (key, path) in [("words", &a.words), ("srt", &a.srt)] {
            if let Some(p) = path {
                let p = crate::paths::resolve_str(p);
                let p = std::fs::canonicalize(&p).unwrap_or(p);
                v[key] = json!(p.display().to_string());
            }
        }
    }
    v
}

fn bad_words(file: &str, why: String) -> CoreError {
    usage(
        format!("{file}: {why}"),
        json!({ "file": file }),
        "--words takes a JSON array of {\"text\", \"start\", \"end\"} with times in seconds as numbers, decimals (\"1.25\") or rationals (\"5/4\")",
    )
}

/// A word time: a JSON number or a string, as `n`, `n/d` or a decimal, exactly.
fn word_time(file: &str, n: usize, field: &str, v: &Value) -> Result<Rational, CoreError> {
    let text = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return Err(bad_words(file, format!("word {n} has no {field} time"))),
    };
    if let Ok(r) = text.parse::<Rational>() {
        return Ok(r);
    }
    parse_time(&format!("{text}s"), Rational::from_int(1))
        .map_err(|_| bad_words(file, format!("word {n}: {field} {text:?} is not a time")))
}

fn read_text(file: &str, code: &'static str) -> Result<String, CoreError> {
    let path = crate::paths::resolve_str(file);
    let bytes = std::fs::read(&path).map_err(|e| {
        CoreError::new("ASSET_UNREADABLE", format!("cannot read {file}: {e}"))
            .with_details(json!({ "path": path.display().to_string() }))
            .with_fix("check the path (relative to the working directory)")
    })?;
    String::from_utf8(bytes).map_err(|_| {
        CoreError::new(code, format!("{file} is not UTF-8 text"))
            .with_details(json!({ "path": path.display().to_string() }))
            .with_fix("save the file as UTF-8")
    })
}

/// The words of a `--words` file.
pub fn read_words(file: &str) -> Result<Vec<Word>, CoreError> {
    let text = read_text(file, "USAGE")?;
    let list: Vec<Value> = serde_json::from_str(&text)
        .map_err(|e| bad_words(file, format!("not a JSON array: {e}")))?;
    let mut words = Vec::with_capacity(list.len());
    for (n, w) in list.iter().enumerate() {
        let Some(t) = w.get("text").and_then(Value::as_str) else {
            return Err(bad_words(file, format!("word {n} has no text")));
        };
        words.push(Word {
            text: t.to_string(),
            start: word_time(file, n, "start", &w["start"])?,
            end: word_time(file, n, "end", &w["end"])?,
        });
    }
    Ok(words)
}

fn plan_import(b: &mut Builder<'_>, a: &ImportArgs, suffix: &str) -> Result<Value, CoreError> {
    let (_, words_file) = one_of("caption.import", ("words", &a.words), ("srt", &a.srt))?;
    let track = b.id(&a.track);
    let rate = addr::rate_of(&b.work, &track);
    addr::track(&b.work, &track)?;
    let at = match &a.at {
        Some(t) => addr::time_at(rate, "at", t)?,
        None => Rational::zero(),
    };
    let (words, warnings) = match &a.srt {
        Some(file) => {
            let srt = parse_srt(&read_text(file, "SRT_INVALID")?, frame_duration(rate)?)?;
            let warnings: Vec<Value> = srt
                .warnings
                .iter()
                .map(|w| json!({ "code": w.code, "line": w.line, "message": w.message }))
                .collect();
            (srt.words, warnings)
        }
        None => (read_words(words_file)?, Vec::new()),
    };
    if words.is_empty() {
        return Err(usage(
            "the transcript has no words",
            json!({ "file": words_file }),
            "import a file with at least one timed word or SRT cue",
        ));
    }
    let style = b.id(&a.style);
    let composed = caption_import(&b.work, &track, at, &words, &style)?;
    let names = b.push_composed(composed.ops, suffix)?;
    let mut echo = composed.echo;
    rename_json(&mut echo, &names);
    let caps: Vec<String> = (1..=echo["blocks"].as_u64().unwrap_or(0))
        .map(|n| {
            names
                .get(&format!("$cap{n}"))
                .cloned()
                .unwrap_or(format!("$cap{n}"))
        })
        .collect();
    let mut resolved = json!({
        "track": a.track, "at": at, "style": a.style, "words": words.len(), "items": caps,
    });
    if a.srt.is_some() {
        resolved["warnings"] = json!(warnings);
    }
    merge_echo(&mut resolved, echo);
    Ok(resolved)
}

pub fn plan(b: &mut Builder<'_>, cmd: &CaptionCmd, suffix: &str) -> Result<Value, CoreError> {
    match cmd {
        CaptionCmd::Import(a) => plan_import(b, a, suffix),
        CaptionCmd::SetText(a) => {
            b.push(Op::CaptionSetText(CaptionSetText {
                item: a.item.clone(),
                text: a.text.clone(),
            }))?;
            Ok(json!({ "item": a.item, "text": a.text, "words": caption_words(b, &a.item) }))
        }
        CaptionCmd::Split(a) => {
            let id = format!("$cap{suffix}");
            b.push(Op::CaptionSplit(CaptionSplit {
                item: a.item.clone(),
                word: a.word,
                id: id.clone(),
            }))?;
            Ok(json!({ "item": a.item, "word": a.word, "id": id }))
        }
        CaptionCmd::Merge(a) => {
            b.push(Op::CaptionMerge(CaptionMerge {
                item: a.item.clone(),
                with: a.with.clone(),
            }))?;
            Ok(json!({ "item": a.item, "with": a.with, "words": caption_words(b, &a.item) }))
        }
        CaptionCmd::Style(a) => {
            let overrides: Option<BTreeMap<String, fin_core::ParamValue>> = if a.clear {
                Some(BTreeMap::new())
            } else if a.set.is_empty() {
                None
            } else {
                Some(parse_sets(&ScopeOf::Style, &a.set, b.work.rate)?)
            };
            b.push(Op::CaptionSetStyle(CaptionSetStyle {
                item: a.item.clone(),
                style: a.style.clone(),
                overrides: overrides.clone(),
            }))?;
            Ok(json!({ "item": a.item, "style": a.style, "overrides": overrides }))
        }
    }
}

/// How many words caption `item` holds after the planned ops.
fn caption_words(b: &Builder<'_>, item: &str) -> Option<usize> {
    let (_, _, it) = addr::item(&b.work, &b.id(item)).ok()?;
    match &it.body {
        fin_core::ItemBody::Caption { block } => Some(block.words.len()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_times_convert_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("w.json");
        std::fs::write(
            &file,
            r#"[{"text":"a","start":0.1,"end":"0.35"},{"text":"b","start":"7/20","end":1}]"#,
        )
        .unwrap();
        let words = read_words(file.to_str().unwrap()).unwrap();
        let r = |s: &str| s.parse::<Rational>().unwrap();
        assert_eq!((words[0].start, words[0].end), (r("1/10"), r("7/20")));
        assert_eq!((words[1].start, words[1].end), (r("7/20"), r("1")));
    }
}
