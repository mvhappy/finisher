//! The A0 corpus (plan 6C Task 11; Determinism, spec §8.2 and §13.4): the mixer with every
//! effect, the resampler, WSOLA, the limiter and the peaks give the same bits natively and on
//! `wasm32-wasip1`, whatever the block length.
//!
//! **The plans.** Each `tests/a0/plans/<name>.json` holds a small project (`project`, model JSON
//! read through the snapshot upcaster), the `range` of `timeline` it renders, the generated
//! `sources` its assets play (signals from `fin_audio::testing::signals` with fixed seeds,
//! summed per channel, `secs` long, from source frame 0; a source with a `rate` other than
//! 48,000 is generated at that rate and converted by [`Resampler`] fed in chunks of the render's
//! block length, as `fin-workers`' `DecodedSource` does), an optional `master` stage (the
//! true-peak limiter after the mix, and whether the output's waveform peaks are hashed) and
//! `plan`: the `fin_core::eval::eval_audio` output for that project and range. The plan is the
//! A0 input; `plans_follow_their_projects` checks it is still what `eval_audio` gives, and
//! `FIN_A0_REGEN=1` rewrites it (natively) when a fin-core change moves it.
//!
//! **The hashes.** The output is BLAKE3 over the interleaved (L, R) little-endian `f32` bytes;
//! `expected.json` holds it per plan with `chunks`, the 8-hex-digit BLAKE3 prefixes of every
//! [`CHUNK`]-frame (10 ms) chunk concatenated, the peaks file's hash where asked, and the
//! `audioVersion` it was blessed at. A mismatch names the plan, the first differing 10 ms
//! window (its frames and time) and the values computed there. Naming the exact sample needs
//! the reference samples themselves, which are not committed (no media files): use
//! `FIN_A0_DUMP` below. `FIN_A0_BLESS=1` rewrites it
//! natively after review; it is refused under `CI`, and refused when a plan's hash moves while
//! `fin_audio::AUDIO_VERSION` still equals the blessed one: a change that moves an A0 hash
//! bumps `AUDIO_VERSION` in the same commit. `a0_hash_change_requires_audio_version_bump`
//! fails until the bumped version is blessed, and `scripts/check-a0-history.mjs` (a gate)
//! walks every commit that touched `expected.json`, so editing or dropping a hash by hand
//! without a bump fails too.
//!
//! **Exact first differing sample across machines.** `FIN_A0_DUMP=<dir>` (inside the package
//! directory for wasm, with `--env FIN_A0_DUMP` in the runner) writes each plan's output as
//! `<dir>/<name>.f32` when the file is absent, and compares with it when present: run once
//! natively, then on the other target, and a mismatch names the frame, channel and both values.
//! Writing dumps is refused under `CI`; comparing with them is not.
//!
//! The files are read relative to the package directory (where `cargo test` runs), so the wasm
//! runner needs only `--dir .`. A panic aborts on wasm32-wasip1, so the harness shows its
//! message only with `-- --nocapture` (which CI passes).

// Reads the committed corpus files by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fin_audio::channels::Canonical;
use fin_audio::testing::signals::{db_to_amp, noise, pink, sine, sweep, tone_burst};
use fin_audio::testing::source::MemSource;
use fin_audio::{
    AUDIO_VERSION, Limiter, Mixer, PcmSource, RATE, Resampler, Stereo, peaks, sample_at,
};
use fin_core::eval::{AudioPlan, eval_audio};
use fin_core::{Rational, upcast_snapshot};
use serde_json::{Map, Value, json};

const DIR: &str = "tests/a0";
/// The block length of the hashed render.
const MAIN_BLOCK: usize = 1_000;
/// `block_size_invariant_corpus` renders every plan in each of these.
const BLOCKS: [usize; 3] = [128, 1_000, 48_000];
/// Frames per chunk hash (10 ms).
const CHUNK: usize = 480;
/// Hex digits kept of each chunk's BLAKE3.
const CHUNK_HEX: usize = 8;

/// One corpus file.
struct Case {
    name: String,
    file: Value,
    plan: Option<AudioPlan>,
}

impl Case {
    fn plan(&self) -> &AudioPlan {
        self.plan.as_ref().unwrap_or_else(|| {
            panic!(
                "{}: no `plan`; write it with FIN_A0_REGEN=1 cargo test -p fin-audio --test a0 \
                 plans_follow_their_projects",
                self.name
            )
        })
    }

    fn limiter(&self) -> Option<f64> {
        self.file["master"]["limiterDbtp"].as_f64()
    }

    fn hashes_peaks(&self) -> bool {
        self.file["master"]["peaks"].as_bool() == Some(true)
    }
}

fn plans_dir() -> PathBuf {
    Path::new(DIR).join("plans")
}

/// Every corpus file, sorted by name.
fn cases() -> Vec<Case> {
    let dir = plans_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e} (wasm runners need --dir .)", dir.display()))
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|file_name| {
            let text = std::fs::read_to_string(dir.join(&file_name)).unwrap();
            let file: Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{file_name}: {e}"));
            let plan = file.get("plan").map(|p| {
                serde_json::from_value(p.clone())
                    .unwrap_or_else(|e| panic!("{file_name}: `plan` is not an AudioPlan: {e}"))
            });
            let name = file_name.trim_end_matches(".json").to_string();
            Case { name, file, plan }
        })
        .collect()
}

fn num(v: &Value, key: &str) -> f64 {
    v[key]
        .as_f64()
        .unwrap_or_else(|| panic!("signal {v}: no number `{key}`"))
}

/// One signal component, `secs` long at `rate`.
fn component(c: &Value, secs: f64, rate: u32) -> Vec<f32> {
    let db = num(c, "db");
    let scaled = |x: Vec<f32>| -> Vec<f32> {
        let a = db_to_amp(db) as f32;
        x.into_iter().map(|v| v * a).collect()
    };
    let seed = || c["seed"].as_u64().expect("a noise seed");
    match c["kind"].as_str() {
        Some("sine") => sine(num(c, "hz"), db, secs, rate),
        Some("sweep") => scaled(sweep(num(c, "from"), num(c, "to"), secs, rate)),
        Some("noise") => scaled(noise(seed(), secs, rate)),
        Some("pink") => scaled(pink(seed(), secs, rate)),
        Some("burst") => tone_burst(
            num(c, "hz"),
            db,
            num(c, "start"),
            num(c, "burst"),
            secs,
            rate,
        ),
        _ => panic!("unknown signal component {c}"),
    }
}

/// The components of one channel, summed in order.
fn channel(list: &Value, secs: f64, rate: u32) -> Vec<f32> {
    let mut out: Vec<f32> = Vec::new();
    for c in list.as_array().expect("a list of signal components") {
        let x = component(c, secs, rate);
        if out.is_empty() {
            out = x;
        } else {
            for (o, v) in out.iter_mut().zip(x) {
                *o += v;
            }
        }
    }
    out
}

/// `x` at `rate` converted to 48 kHz, fed to the resampler `block` frames at a time.
fn to_48k(x: Vec<f32>, rate: u32, block: usize) -> Vec<f32> {
    if rate == RATE {
        return x;
    }
    let mut rs = Resampler::new(rate, RATE).unwrap();
    let mut out = Vec::new();
    for part in x.chunks(block) {
        rs.process(part, &mut out);
    }
    rs.finish(&mut out);
    out
}

/// The plan's sources; one at another rate is resampled in chunks of `block` frames.
fn sources(spec: &Value, block: usize) -> BTreeMap<String, Box<dyn PcmSource>> {
    let spec = spec.as_object().expect("`sources` maps assets to signals");
    spec.iter()
        .map(|(asset, s)| {
            let secs = num(s, "secs");
            let rate = s["rate"].as_u64().map_or(RATE, |r| r as u32);
            let ch = |list: &Value| to_48k(channel(list, secs, rate), rate, block);
            let c = match s.get("mono") {
                Some(m) => Canonical::Mono(ch(m)),
                None => Canonical::Stereo(Stereo {
                    l: ch(&s["l"]),
                    r: ch(&s["r"]),
                }),
            };
            (asset.clone(), MemSource::boxed(c))
        })
        .collect()
}

/// A plan's output: interleaved stereo and, where asked, the peaks file.
struct Output {
    pcm: Vec<f32>,
    peaks: Option<Vec<u8>>,
}

/// Renders `case` from its plan's `from` to its `to` in blocks of `block` frames, through the
/// master limiter when it has one.
fn render(case: &Case, block: usize) -> Output {
    let plan = case.plan();
    let mut mixer = Mixer::new(plan, sources(&case.file["sources"], block))
        .unwrap_or_else(|e| panic!("{}: {e:?}", case.name));
    assert!(mixer.missing().is_empty(), "{}: missing sources", case.name);
    assert!(
        mixer.skipped_effects().is_empty(),
        "{}: skipped effects {:?}",
        case.name,
        mixer.skipped_effects()
    );
    let total = (sample_at(plan.to).unwrap() - sample_at(plan.from).unwrap()) as usize;
    let mut limiter = case.limiter().map(Limiter::new);
    let mut out = Stereo::default();
    let mut done = 0;
    while done < total {
        let n = block.min(total - done);
        let mut b = Stereo {
            l: vec![0.0; n],
            r: Vec::new(),
        };
        mixer.render(&mut b).unwrap();
        if let Some(lim) = &mut limiter {
            lim.process(&mut b);
        }
        out.l.extend_from_slice(&b.l);
        out.r.extend_from_slice(&b.r);
        done += n;
    }
    if let Some(lim) = &mut limiter {
        lim.finish(&mut out);
    }
    assert_eq!(out.len(), total, "{}: output length", case.name);
    let all = out.l.iter().chain(&out.r);
    assert!(
        all.clone().all(|v| v.is_finite()) && all.into_iter().any(|&v| v != 0.0),
        "{}: the output must be finite and not silent",
        case.name
    );
    let peaks = case.hashes_peaks().then(|| peaks(&out).to_bytes());
    let pcm = out
        .l
        .iter()
        .zip(&out.r)
        .flat_map(|(&l, &r)| [l, r])
        .collect();
    Output { pcm, peaks }
}

fn bytes(pcm: &[f32]) -> Vec<u8> {
    pcm.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn hash(b: &[u8]) -> String {
    blake3::hash(b).to_hex().to_string()
}

/// The chunk hash prefixes, concatenated.
fn chunk_hashes(pcm: &[f32]) -> String {
    pcm.chunks(CHUNK * 2)
        .map(|c| hash(&bytes(c))[..CHUNK_HEX].to_string())
        .collect()
}

/// Every case with its output at [`MAIN_BLOCK`], computed once per test process.
fn computed() -> &'static [(Case, Output)] {
    static ALL: OnceLock<Vec<(Case, Output)>> = OnceLock::new();
    ALL.get_or_init(|| {
        cases()
            .into_iter()
            .map(|c| {
                let o = render(&c, MAIN_BLOCK);
                (c, o)
            })
            .collect()
    })
}

fn env_is(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

fn refuse_under_ci(name: &str) {
    assert!(
        std::env::var_os("CI").is_none(),
        "{name} is refused under CI: run it locally after review and commit"
    );
}

fn expected_path() -> PathBuf {
    Path::new(DIR).join("expected.json")
}

fn read_expected() -> Value {
    let path = expected_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (wasm runners need --dir .)", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn blessed_version(expected: &Value) -> u64 {
    expected["audioVersion"]
        .as_u64()
        .expect("expected.json: `audioVersion`")
}

fn entry(o: &Output) -> Value {
    let mut e = Map::new();
    e.insert("blake3".into(), json!(hash(&bytes(&o.pcm))));
    if let Some(p) = &o.peaks {
        e.insert("peaks".into(), json!(hash(p)));
    }
    e.insert("chunks".into(), json!(chunk_hashes(&o.pcm)));
    Value::Object(e)
}

/// Rewrites `expected.json`; refused when a blessed hash moves under the same `AUDIO_VERSION`.
fn bless(all: &[(Case, Output)]) {
    refuse_under_ci("FIN_A0_BLESS");
    let old = read_expected();
    let mut plans = Map::new();
    for (c, o) in all {
        let e = entry(o);
        if blessed_version(&old) == u64::from(AUDIO_VERSION) {
            let was = &old["plans"][&c.name];
            let moved = |k: &str| !was.is_null() && was[k] != e[k];
            assert!(
                !moved("blake3") && !moved("peaks"),
                "A0 plan {}: its hash moves but fin_audio::AUDIO_VERSION is still {AUDIO_VERSION} \
                 (the blessed version): bump AUDIO_VERSION in the same commit, then bless",
                c.name
            );
        }
        plans.insert(c.name.clone(), e);
    }
    let doc = json!({ "audioVersion": AUDIO_VERSION, "chunkFrames": CHUNK, "plans": plans });
    std::fs::write(
        expected_path(),
        serde_json::to_string_pretty(&doc).unwrap() + "\n",
    )
    .unwrap();
}

/// With `FIN_A0_DUMP` set: the exact first difference from a dump of another run, or writes
/// this run's dump. `None` when there is nothing to report.
fn dump_or_compare(name: &str, pcm: &[f32]) -> Option<String> {
    let dir = PathBuf::from(std::env::var_os("FIN_A0_DUMP")?);
    let path = dir.join(format!("{name}.f32"));
    match std::fs::read(&path) {
        Ok(b) => {
            let theirs: Vec<f32> = b
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            first_difference(&theirs, pcm).map(|d| format!("against {}: {d}", path.display()))
        }
        Err(_) => {
            refuse_under_ci("writing FIN_A0_DUMP");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, bytes(pcm)).unwrap();
            None
        }
    }
}

/// The first differing sample of two interleaved stereo outputs, as text.
fn first_difference(want: &[f32], got: &[f32]) -> Option<String> {
    let i = want
        .iter()
        .zip(got)
        .position(|(a, b)| a.to_bits() != b.to_bits());
    match i {
        Some(i) => Some(format!(
            "first differing sample: frame {}, channel {}: expected {:e} ({:#010x}), got {:e} \
             ({:#010x})",
            i / 2,
            ["L", "R"][i % 2],
            want[i],
            want[i].to_bits(),
            got[i],
            got[i].to_bits()
        )),
        None if want.len() != got.len() => Some(format!(
            "lengths differ: expected {} samples, got {}",
            want.len(),
            got.len()
        )),
        None => None,
    }
}

/// What differs between a plan's output and its expected entry, or `None`.
fn mismatch(c: &Case, o: &Output, want: &Value) -> Option<String> {
    let got = entry(o);
    if want.is_null() {
        return Some(format!(
            "{}: not in expected.json (bless with FIN_A0_BLESS=1)",
            c.name
        ));
    }
    let mut msg = Vec::new();
    if want["blake3"] != got["blake3"] {
        let (w, g) = (
            want["chunks"].as_str().unwrap_or(""),
            got["chunks"].as_str().unwrap(),
        );
        let per = |x: &str| {
            x.as_bytes()
                .chunks(CHUNK_HEX)
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>()
        };
        let (w, g) = (per(w), per(g));
        let at = (0..g.len().max(w.len()))
            .find(|&i| g.get(i) != w.get(i))
            .unwrap_or(0);
        let f0 = at * CHUNK;
        let frames = o.pcm.len() / 2;
        let here: Vec<String> = o.pcm[(f0 * 2).min(o.pcm.len())..]
            .chunks(2)
            .take(3)
            .map(|f| format!("({:e}, {:e})", f[0], f[1]))
            .collect();
        msg.push(format!(
            "{}: output hash {} (expected {}); the first differing 10 ms window is frames \
             {f0}..{} ({:.3} s into the render; {} expected windows, {} computed), whose first \
             frames here are [{}]; for the exact sample, compare with a dump (FIN_A0_DUMP)",
            c.name,
            got["blake3"],
            want["blake3"],
            (f0 + CHUNK).min(frames),
            f0 as f64 / f64::from(RATE),
            w.len(),
            g.len(),
            here.join(", ")
        ));
    }
    if want["peaks"] != got["peaks"] {
        msg.push(format!(
            "{}: peaks hash {} (expected {})",
            c.name, got["peaks"], want["peaks"]
        ));
    }
    if let Some(d) = dump_or_compare(&c.name, &o.pcm) {
        msg.push(format!("{}: {d}", c.name));
    }
    (!msg.is_empty()).then(|| msg.join("\n"))
}

/// The cross-machine check: every plan's output (and peaks) hash equals the committed one.
fn corpus_matches_expected() {
    let all = computed();
    assert_eq!(all.len(), 11, "the A0 corpus has 11 plans");
    if env_is("FIN_A0_BLESS") {
        bless(all);
        return;
    }
    let expected = read_expected();
    // One line per plan, for comparing runs across machines (`--nocapture`).
    for (c, o) in all {
        println!("{} {}", c.name, hash(&bytes(&o.pcm)));
    }
    let failures: Vec<String> = all
        .iter()
        .filter_map(|(c, o)| mismatch(c, o, &expected["plans"][&c.name]))
        .collect();
    let version = blessed_version(&expected);
    assert!(
        failures.is_empty(),
        "A0 mismatch:\n{}\n\nOn this target the engine's bits moved, or a change moved them. A \
         change that moves an A0 hash bumps fin_audio::AUDIO_VERSION (blessed at {version}, now \
         {AUDIO_VERSION}) in the same commit and re-blesses with FIN_A0_BLESS=1 natively.",
        failures.join("\n")
    );
    let names: Vec<&str> = all.iter().map(|(c, _)| c.name.as_str()).collect();
    let stale: Vec<&String> = expected["plans"]
        .as_object()
        .map(|m| m.keys().filter(|k| !names.contains(&k.as_str())).collect())
        .unwrap_or_default();
    assert!(
        stale.is_empty(),
        "expected.json lists plans the corpus no longer has: {stale:?}"
    );
}

#[cfg(not(target_os = "wasi"))]
#[test]
fn a0_corpus_native_matches_expected() {
    corpus_matches_expected();
}

/// The same hashes on `wasm32-wasip1` (CI runs it under `wasmtime --dir .`).
#[cfg(target_os = "wasi")]
#[test]
fn a0_corpus_native_matches_wasm() {
    corpus_matches_expected();
}

/// `expected.json` was blessed at this `AUDIO_VERSION`: bumping it without re-blessing fails
/// here, and blessing a moved hash without bumping it is refused by the bless.
#[test]
fn a0_hash_change_requires_audio_version_bump() {
    let expected = read_expected();
    assert_eq!(
        blessed_version(&expected),
        u64::from(AUDIO_VERSION),
        "tests/a0/expected.json was blessed at audioVersion {} but fin_audio::AUDIO_VERSION is \
         {AUDIO_VERSION}: re-bless the A0 corpus (FIN_A0_BLESS=1) in the commit that bumps it",
        blessed_version(&expected)
    );
    let plans = expected["plans"].as_object().map_or(0, Map::len);
    assert!(plans > 0, "tests/a0/expected.json holds no plans: bless it");
}

/// Block length never changes the output: every plan in blocks of 128, 1,000 and 48,000
/// frames gives the same bits.
#[test]
fn block_size_invariant_corpus() {
    for (c, main) in computed() {
        for block in BLOCKS.into_iter().filter(|&b| b != MAIN_BLOCK) {
            let o = render(c, block);
            if let Some(d) = first_difference(&main.pcm, &o.pcm) {
                panic!(
                    "{}: blocks of {block} differ from blocks of {MAIN_BLOCK}: {d}",
                    c.name
                );
            }
            assert_eq!(main.peaks, o.peaks, "{}: peaks, blocks of {block}", c.name);
        }
    }
}

/// Each `plan` is `eval_audio` of its file's project over its range; `FIN_A0_REGEN=1`
/// rewrites the plans (natively) when they are not.
#[test]
fn plans_follow_their_projects() {
    let regen = env_is("FIN_A0_REGEN");
    if regen {
        refuse_under_ci("FIN_A0_REGEN");
    }
    for c in cases() {
        let project = upcast_snapshot(c.file["project"].clone())
            .unwrap_or_else(|e| panic!("{}: {e:?}", c.name));
        let range: [Rational; 2] = serde_json::from_value(c.file["range"].clone()).unwrap();
        let timeline = c.file["timeline"].as_str().expect("`timeline`");
        let plan = eval_audio(&project, timeline, range[0], range[1])
            .unwrap_or_else(|e| panic!("{}: {e:?}", c.name));
        if regen {
            let mut file = c.file.clone();
            file["plan"] = serde_json::to_value(&plan).unwrap();
            let path = plans_dir().join(format!("{}.json", c.name));
            std::fs::write(path, serde_json::to_string_pretty(&file).unwrap() + "\n").unwrap();
        } else {
            assert!(
                c.plan.as_ref() == Some(&plan),
                "{}: `plan` is not eval_audio of the project; rewrite it with FIN_A0_REGEN=1 \
                 (the A0 hashes then move: re-bless, with an AUDIO_VERSION bump only if the \
                 engine changed)",
                c.name
            );
        }
    }
}

/// Mixer throughput, informational (the gate is Task 23's): 16 stereo items, each with an EQ
/// and a compressor, 10 s. `cargo test --release -p fin-audio --test a0 -- --ignored perf`.
#[test]
#[ignore]
fn perf() {
    const ITEMS: usize = 16;
    const SECS: i64 = 10;
    let eq = json!({"id": "fx1", "kind": "eq", "params": {
        "band1Type": "highPass", "band3Type": "peak", "band3Gain": 4.0,
        "band6Type": "peak", "band6Gain": -3.0, "band7Type": "highShelf", "band7Gain": 2.0}});
    let comp = json!({"id": "fx2", "kind": "compressor",
        "params": {"threshold": -20.0, "ratio": 4.0, "makeup": 3.0}});
    let mut tracks = Vec::new();
    let mut assets = Map::new();
    let mut srcs = Map::new();
    for i in 0..ITEMS {
        let (a, t) = (format!("a{i}"), format!("t{i}"));
        tracks.push(json!({"id": t, "kind": "audio", "name": t, "items": [{
            "id": format!("c{i}"), "start": "0", "duration": SECS.to_string(), "kind": "clip",
            "asset": a, "sourceIn": "0", "audio": {"gain": -12.0},
            "effects": [eq, comp]}]}));
        assets.insert(
            a.clone(),
            json!({"id": a, "kind": "media", "path": "x.bin",
            "fingerprint": format!("fp-{i}"), "duration": "600",
            "audio": {"sampleRate": 48000, "channels": 2}}),
        );
        let seed = 100 + 2 * i as u64;
        srcs.insert(
            a,
            json!({"secs": SECS,
            "l": [{"kind": "pink", "seed": seed, "db": -3.0}],
            "r": [{"kind": "pink", "seed": seed + 1, "db": -3.0}]}),
        );
    }
    let project = upcast_snapshot(json!({"id": "p1", "name": "perf", "schemaVersion": 6,
        "rate": "30", "assets": assets, "timelines": [{"id": "tl1", "name": "tl1", "rate": "30",
        "resolution": {"width": 1920, "height": 1080}, "tracks": tracks}]}))
    .unwrap();
    let plan = eval_audio(
        &project,
        "tl1",
        Rational::from_int(0),
        Rational::from_int(SECS),
    )
    .unwrap();
    let mut mixer = Mixer::new(&plan, sources(&Value::Object(srcs), 1_024)).unwrap();
    let total = SECS as usize * RATE as usize;
    let start = std::time::Instant::now();
    let mut done = 0;
    while done < total {
        let n = 1_024.min(total - done);
        let mut b = Stereo {
            l: vec![0.0; n],
            r: Vec::new(),
        };
        mixer.render(&mut b).unwrap();
        done += n;
    }
    let wall = start.elapsed().as_secs_f64();
    println!(
        "A0 perf: {ITEMS} stereo items × (EQ + compressor), {SECS} s mixed in {wall:.3} s = \
         {:.1}× real time",
        SECS as f64 / wall
    );
}
