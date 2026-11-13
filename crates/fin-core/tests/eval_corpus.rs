//! The determinism corpus (plan 4C Task 17, spec §5.4): frame and audio plans of the projects
//! in `eval_corpus/projects/`, hashed as BLAKE3 of their compact JSON and compared with
//! `eval_corpus/expected.json` on every OS and on wasm32 (`wasm32-wasip1` under
//! `wasmtime --dir .`).
//!
//! The files are read relative to the package directory (where `cargo test` runs), so the
//! wasm runner needs only `--dir .`. `FIN_EVAL_CORPUS_BLESS=1` rewrites `expected.json`
//! (natively) after review, and is refused when `CI` is set; a mismatch prints the first
//! differing plan.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use fin_core::eval::{AudioPlan, FramePlan, Layer, eval_audio, eval_frame};
use fin_core::layout::TextMeasurer;
use fin_core::layout::testing::FakeMeasurer;
use fin_core::params::{ParamType, Scope, specs};
use fin_core::{Curve, Interp, Item, ItemBody, Project, Rational, Timeline, upcast_snapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DIR: &str = "tests/eval_corpus";

/// Fixed advances (`layout::testing`), so the corpus pins plan shapes, not a font engine.
static FAKE: FakeMeasurer = FakeMeasurer {
    missing: Vec::new(),
};

/// One corpus file: a project and the times and ranges evaluated on `timeline`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    #[allow(dead_code)]
    what: String,
    timeline: String,
    /// Evaluate frames with [`FakeMeasurer`] instead of no measurer.
    #[serde(default)]
    measurer: bool,
    /// Whether some caption layer of the case's frames has a layout. Required on every case
    /// with a measurer and checked both ways, so the case says what its plans hold (captions are
    /// measured once their faces resolve to pins, plan 4C Task 20).
    #[serde(default)]
    requires_measured_caption: Option<bool>,
    frames: Vec<Rational>,
    audio: Vec<[Rational; 2]>,
    project: Value,
}

/// One line of `expected.json`: a frame (`t`) or an audio range (`range`) of a case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    case: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    t: Option<Rational>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<[Rational; 2]>,
    blake3: String,
}

impl Entry {
    fn key(&self) -> String {
        match (&self.t, &self.range) {
            (Some(t), _) => format!("{} t={t}", self.case),
            (_, Some([a, b])) => format!("{} range={a}..{b}", self.case),
            _ => format!("{} ?", self.case),
        }
    }
}

/// Every corpus file, by name (sorted), with its project read through the snapshot upcaster.
fn cases() -> Vec<(String, Case, Project)> {
    let dir = Path::new(DIR).join("projects");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e} (wasm runners need --dir .)", dir.display()))
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let text = std::fs::read_to_string(dir.join(&name)).unwrap();
            let case: Case = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            let project =
                upcast_snapshot(case.project.clone()).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let id = name.trim_end_matches(".json").to_string();
            (id, case, project)
        })
        .collect()
}

fn hash(plan: &impl Serialize) -> (String, String) {
    let json = serde_json::to_string(plan).unwrap();
    (blake3::hash(json.as_bytes()).to_hex().to_string(), json)
}

/// Every plan of the corpus: its entry and its compact JSON, in file then list order (frames,
/// then audio ranges).
fn computed() -> Vec<(Entry, String)> {
    let mut out = Vec::new();
    for (id, case, project) in cases() {
        let m: Option<&dyn TextMeasurer> = case.measurer.then_some(&FAKE);
        for &t in &case.frames {
            let plan = eval_frame(&project, &case.timeline, t, m)
                .unwrap_or_else(|e| panic!("{id} t={t}: {e:?}"));
            let (blake3, json) = hash(&plan);
            let entry = Entry {
                case: id.clone(),
                t: Some(t),
                range: None,
                blake3,
            };
            out.push((entry, json));
        }
        for &[from, to] in &case.audio {
            let plan = eval_audio(&project, &case.timeline, from, to)
                .unwrap_or_else(|e| panic!("{id} {from}..{to}: {e:?}"));
            let (blake3, json) = hash(&plan);
            let entry = Entry {
                case: id.clone(),
                t: None,
                range: Some([from, to]),
                blake3,
            };
            out.push((entry, json));
        }
    }
    out
}

fn bless() -> bool {
    let bless = std::env::var("FIN_EVAL_CORPUS_BLESS").is_ok_and(|v| v == "1");
    assert!(
        !(bless && std::env::var_os("CI").is_some()),
        "FIN_EVAL_CORPUS_BLESS is refused under CI: bless locally after review and commit"
    );
    bless
}

/// The cross-machine check: every plan hash equals the committed one.
#[test]
fn corpus_plans_match_expected() {
    let plans = computed();
    let entries: Vec<Entry> = plans.iter().map(|(e, _)| e.clone()).collect();
    let path = Path::new(DIR).join("expected.json");
    if bless() {
        let text = serde_json::to_string_pretty(&entries).unwrap() + "\n";
        std::fs::write(&path, text).unwrap();
        return;
    }
    // One line per plan, for comparing runs across machines (`--nocapture`).
    for (e, _) in &plans {
        println!("{} {}", e.key(), e.blake3);
    }
    let expected: Vec<Entry> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let want: BTreeMap<String, &str> = expected
        .iter()
        .map(|e| (e.key(), e.blake3.as_str()))
        .collect();
    assert_eq!(want.len(), expected.len(), "expected.json repeats a plan");
    for (e, json) in &plans {
        let key = e.key();
        if want.get(&key) != Some(&e.blake3.as_str()) {
            let pretty =
                serde_json::to_string_pretty(&serde_json::from_str::<Value>(json).unwrap())
                    .unwrap();
            panic!(
                "{key}: expected {:?}, computed {}; {} plans computed, {} expected.\n\
                 The plan:\n{pretty}\n\
                 Rerun with FIN_EVAL_CORPUS_BLESS=1 after review.",
                want.get(&key),
                e.blake3,
                plans.len(),
                expected.len()
            );
        }
    }
    let computed: BTreeSet<String> = plans.iter().map(|(e, _)| e.key()).collect();
    let stale: Vec<&String> = want.keys().filter(|k| !computed.contains(*k)).collect();
    assert!(
        stale.is_empty(),
        "expected.json lists plans the corpus no longer has: {stale:?}"
    );
}

/// The corpus files are model JSON exactly: a field the model does not know (dropped on read)
/// or one at its default (omitted on write) would make the file say something the plan does
/// not.
#[test]
fn corpus_projects_round_trip() {
    for (id, case, project) in cases() {
        assert_eq!(
            serde_json::to_value(&project).unwrap(),
            case.project,
            "{id}: the project does not round-trip"
        );
    }
}

/// Every item of a timeline, nested ones included (each timeline once).
fn items(p: &Project) -> impl Iterator<Item = (&Timeline, &Item)> {
    p.timelines.iter().flat_map(|tl| {
        tl.tracks
            .iter()
            .flat_map(move |t| t.items.iter().map(move |i| (tl, i)))
    })
}

fn is_bezier(kfs: &[fin_core::Keyframe]) -> bool {
    kfs.iter()
        .any(|k| matches!(k.interp, Interp::Bezier { .. }))
}

/// The corpus covers what plan 4C Task 17 lists: bézier keyframes on every animatable number,
/// `vec2` and colour path (items, effects and graphic slots), transitions with both curves,
/// the three speeds, nesting two deep, an adjustment layer, masks, captions with a measurer
/// and audio ranges. Audio effects' numbers count too: they reach the audio plan as segments
/// (plan 6C Task 6).
#[test]
fn corpus_covers_the_plan() {
    let cases = cases();
    let mut keyed: BTreeSet<String> = BTreeSet::new();
    let mut curves = BTreeSet::new();
    let mut speeds = BTreeSet::new();
    let (mut depth2, mut adjustment, mut mask, mut captions, mut audio) =
        (false, false, false, false, false);
    for (_, case, p) in &cases {
        audio |= !case.audio.is_empty();
        for (tl, item) in items(p) {
            for (path, kfs) in &item.common.keyframes {
                if !is_bezier(kfs) {
                    continue;
                }
                // Registry form: `effects.<fx>.<param>` is `<kind>.<param>`; a slot is keyed
                // by its template's slot kind.
                let name = match path.split_once('.') {
                    Some(("effects", rest)) => {
                        let (fx, param) = rest.split_once('.').unwrap();
                        let e = item.common.effects.iter().find(|e| e.id == fx).unwrap();
                        format!("{}.{param}", e.kind)
                    }
                    Some(("slots", slot)) => {
                        let ItemBody::Graphic { asset, .. } = &item.body else {
                            panic!("slot keyframes on a non-graphic")
                        };
                        let lottie = p.assets[asset].lottie.as_ref().unwrap();
                        let s = lottie.slots.iter().find(|s| s.id == slot).unwrap();
                        format!("slots.<{}>", s.kind)
                    }
                    _ => path.clone(),
                };
                keyed.insert(name);
            }
            match &item.body {
                ItemBody::Clip { speed, .. } => {
                    speeds.insert(speed.to_string());
                }
                ItemBody::Adjustment {} => adjustment = true,
                ItemBody::Caption { .. } => captions |= case.measurer,
                ItemBody::Nested { timeline, .. } => {
                    let inner = p.timelines.iter().find(|t| &t.id == timeline).unwrap();
                    depth2 |= tl.id != *timeline
                        && inner
                            .tracks
                            .iter()
                            .flat_map(|t| &t.items)
                            .any(|i| matches!(i.body, ItemBody::Nested { .. }));
                }
                _ => {}
            }
            mask |= item.common.mask.is_some();
        }
        for t in p.timelines.iter().flat_map(|tl| &tl.tracks) {
            curves.extend(t.transitions.iter().map(|tr| format!("{:?}", tr.curve)));
        }
    }
    let mut want: BTreeSet<String> = BTreeSet::new();
    for s in specs() {
        let numeric = matches!(
            s.ty,
            ParamType::Number { .. } | ParamType::Vec2 { .. } | ParamType::Color
        );
        if !(s.animatable && numeric) {
            continue;
        }
        match s.scope {
            Scope::Item { .. } if s.path == "slots.<id>" => {
                let kind = if s.ty == ParamType::Color {
                    "color"
                } else {
                    "number"
                };
                want.insert(format!("slots.<{kind}>"));
            }
            Scope::Item { .. } | Scope::Effect { .. } => {
                want.insert(s.path.to_string());
            }
            _ => panic!("{}: an animatable path outside items and effects", s.path),
        }
    }
    let missing: Vec<&String> = want.difference(&keyed).collect();
    assert!(missing.is_empty(), "no bézier keyframes on {missing:?}");
    let both = [Curve::Linear, Curve::EqualPower].map(|c| format!("{c:?}"));
    assert_eq!(curves, BTreeSet::from(both));
    for s in ["1/2", "3/2", "30000/1001"] {
        assert!(speeds.contains(s), "no clip at speed {s}");
    }
    assert!(depth2 && adjustment && mask && captions && audio);
    assert!(
        (15..=25).contains(&cases.len()),
        "{} cases (the plan asks for about 20)",
        cases.len()
    );
}

/// Plan 6C Task 6: the audio-effects case is pinned (its rows are in `expected.json`, one per
/// range, compared by [`corpus_plans_match_expected`]) and its plans carry what it is for:
/// effects of all four kinds, keyframed numbers cut into several segments, and effects of an
/// item inside a nested timeline.
#[test]
fn corpus_21_hash() {
    const CASE: &str = "21-audio-effects";
    let (_, case, project) = cases()
        .into_iter()
        .find(|(id, ..)| id == CASE)
        .expect("the audio-effects corpus case");
    let expected: Vec<Entry> = serde_json::from_str(
        &std::fs::read_to_string(Path::new(DIR).join("expected.json")).unwrap(),
    )
    .unwrap();
    let rows = expected.iter().filter(|e| e.case == CASE).count();
    assert!(!case.audio.is_empty());
    assert_eq!(rows, case.frames.len() + case.audio.len());
    let (mut kinds, mut cut, mut nested) = (BTreeSet::new(), false, false);
    for &[from, to] in &case.audio {
        let plan = eval_audio(&project, &case.timeline, from, to).unwrap();
        for item in &plan.items {
            for e in &item.effects {
                kinds.insert(e.kind.clone());
                cut |= e.numbers.values().any(|s| s.len() > 1);
                nested |= !item.via.is_empty();
            }
        }
    }
    let want = ["compressor", "deEsser", "eq", "gate"].map(String::from);
    assert_eq!(kinds, BTreeSet::from(want));
    assert!(cut && nested, "cut segments {cut}, nested effects {nested}");
}

/// Every layer of a frame plan, children (adjustment and nested groups) included.
fn layers(plan: &FramePlan) -> Vec<&Layer> {
    fn walk<'a>(ls: &'a [Layer], out: &mut Vec<&'a Layer>) {
        for l in ls {
            out.push(l);
            walk(l.children.as_deref().unwrap_or_default(), out);
        }
    }
    let mut out = Vec::new();
    walk(&plan.layers, &mut out);
    out
}

/// A case with a measurer says whether its captions are measured, and its plans agree: once
/// a measurer lays captions out (Task 20) at least one caption plan has a layout.
#[test]
fn measured_caption_flag_matches_the_plans() {
    let mut declared = false;
    for (id, case, project) in cases() {
        let Some(want) = case.requires_measured_caption else {
            assert!(
                !case.measurer,
                "{id}: a measurer case must set requiresMeasuredCaption"
            );
            continue;
        };
        assert!(
            case.measurer,
            "{id}: requiresMeasuredCaption without a measurer"
        );
        declared = true;
        let mut captions = 0;
        let mut measured = false;
        for &t in &case.frames {
            let plan = eval_frame(&project, &case.timeline, t, Some(&FAKE)).unwrap();
            for c in layers(&plan).into_iter().filter_map(|l| l.caption.as_ref()) {
                captions += 1;
                measured |= c.layout.is_some();
            }
        }
        assert!(captions > 0, "{id}: no caption plans");
        assert_eq!(
            measured,
            want,
            "{id}: requiresMeasuredCaption is {want} but a caption layout is {}",
            if measured { "present" } else { "absent" }
        );
    }
    assert!(declared, "no corpus case declares requiresMeasuredCaption");
}

/// The audio ranges test clipping: in every case with ranges over sounding items (a range
/// over a silent timeline only checks the empty plan), some range starts or ends
/// strictly inside a gain segment of the whole timeline's plan, and its plan has that
/// segment cut at the range edge.
#[test]
fn audio_ranges_cut_segments() {
    let whole = |p: &Project, tl: &str| -> AudioPlan {
        eval_audio(p, tl, Rational::zero(), Rational::from_int(1_000_000)).unwrap()
    };
    let mut cases_with_audio = 0;
    for (id, case, project) in cases() {
        let full = whole(&project, &case.timeline);
        if case.audio.is_empty() || full.items.is_empty() {
            continue;
        }
        cases_with_audio += 1;
        let mut cut = false;
        for &[from, to] in &case.audio {
            let plan = eval_audio(&project, &case.timeline, from, to).unwrap();
            for item in &full.items {
                let key = (&item.item, &item.track, &item.via);
                let Some(part) = plan
                    .items
                    .iter()
                    .find(|i| (&i.item, &i.track, &i.via) == key)
                else {
                    continue;
                };
                for s in &item.gain {
                    for edge in [from, to] {
                        if s.from < edge && edge < s.to {
                            let at = part.gain.iter().any(|g| g.from == edge || g.to == edge);
                            assert!(at, "{id} {from}..{to}: {} not cut at {edge}", item.item);
                            cut = true;
                        }
                    }
                }
            }
        }
        assert!(cut, "{id}: no audio range cuts a gain segment");
    }
    assert!(cases_with_audio > 0);
}
