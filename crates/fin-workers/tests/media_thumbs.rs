//! `media.thumbs` v1 (plan 6B-1 Task 15, Decision 17), driven over the envelope as a host
//! drives it (`fin-workers media`, cold): thumbnails at the frames `frame_at` picks (ties
//! included), filmstrip sprites and their `strip.json`, items that survive a crash, the `1080`
//! proxy read in place of the original, and the typed refusals. Needs a GPU adapter for the
//! scale pass (`FIN_GPU`; skipped with a notice without one, failed with `FIN_REQUIRE_GPU=1`).
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::fs::File;
use std::path::{Path, PathBuf};

use fin_codecs::fixture::{ClipSpec, pattern_clip};
use fin_codecs::{FrameReader, ProbeResult};
use fin_container::{Demuxer, Limits};
use fin_core::Rational;
use fin_jobs::HostMsg;
use fin_media::backend::VendorClass;
use fin_media::cache::{EntryMeta, META_FILE};
use fin_media::colour::{ChromaSiting, Matrix, Primaries, Range, Transfer};
use fin_media::jobs::{
    MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile, ProxyCodec, ProxyParams,
    ProxyResult, ProxyTier, RenderManifest, StripIndex, ThumbItem, ThumbsMode, ThumbsParams,
    ThumbsProxy, ThumbsRun, ToneMapped, proxy_key, thumbs_key,
};
use fin_media::testing::{Rect, read_id_rgba, read_id_y};
use fin_media::{ColourTags, Rotation, SampleTable, Size, frame_at};
use media_support::{gpu_or_skip, read_png, worker_env};
use serde_json::{Value, json};
use support::{End, Ev, Job, Proc, Run, expect_of, no_calls, spawn_with};

const FP: &str = "fp";

const PQ: ColourTags = ColourTags {
    primaries: Primaries::Bt2020,
    transfer: Transfer::Pq,
    matrix: Matrix::Bt2020Ncl,
    range: Range::Limited,
    siting: ChromaSiting::Left,
};

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

fn original(p: &Path) -> OriginalFile {
    let e = serde_json::to_value(expect_of(p)).unwrap();
    OriginalFile {
        path: p.to_str().unwrap().into(),
        size: e["size"].as_u64().unwrap(),
        mtime_ns: e["mtimeNs"].as_str().unwrap().into(),
        file_id: e.get("fileId").and_then(Value::as_str).map(str::to_owned),
    }
}

struct Clip {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn clip(spec: &ClipSpec) -> Clip {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap().join("clip.mp4");
    std::fs::write(&path, pattern_clip(spec).unwrap()).unwrap();
    Clip { _dir: dir, path }
}

fn spec(frames: u32, size: Size) -> ClipSpec {
    ClipSpec {
        frames,
        size,
        keyint: 10,
        ..ClipSpec::default()
    }
}

/// A job whose manifest names asset `a1` (fingerprint `fp`) at `path`, or no file.
fn job_for(fp: &str, path: Option<&Path>) -> Job {
    let job = Job::new();
    let m = RenderManifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestAsset {
            asset: "a1".into(),
            kind: fin_core::AssetKind::Media,
            fingerprint: fp.into(),
            original: path.map(original),
            proxies: vec![],
        }],
        fonts: ManifestFonts::default(),
    };
    std::fs::write(
        job.path("in/manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    job
}

fn params(fp: &str, mode: ThumbsMode) -> ThumbsParams {
    ThumbsParams {
        fingerprint: fp.into(),
        asset: "a1".into(),
        key: thumbs_key(fp, &mode, VendorClass::Software).to_string(),
        mode,
        decoder_class: VendorClass::Software,
        proxy: None,
        only: None,
        project_id: None,
        branch: None,
        display: None,
    }
}

fn times_mode(times: Vec<Rational>, max_side: u32) -> ThumbsMode {
    ThumbsMode::Thumbs { times, max_side }
}

fn run_params(job: &Job, p: &Value) -> Run {
    let mut w: Proc = spawn_with(&["media"], &worker_env());
    w.hello();
    let start = job.start("media.thumbs", p.clone(), &["in/manifest.json", "out/"]);
    w.send(&HostMsg::Start(start));
    w.drive(no_calls)
}

fn go(job: &Job, p: &ThumbsParams) -> Run {
    run_params(job, &serde_json::to_value(p).unwrap())
}

fn items(run: &Run) -> Vec<(String, ThumbItem)> {
    run.events
        .iter()
        .filter_map(|e| match e {
            Ev::Item(i, Some(v)) => Some((i.clone(), serde_json::from_value(v.clone()).unwrap())),
            _ => None,
        })
        .collect()
}

fn failed(run: &Run) -> Vec<(String, String)> {
    run.events
        .iter()
        .filter_map(|e| match e {
            Ev::ItemFailed(i, code) => Some((i.clone(), code.clone())),
            _ => None,
        })
        .collect()
}

fn result(run: &Run) -> ThumbsRun {
    match &run.end {
        End::Result(v) => serde_json::from_value(v.clone()).unwrap(),
        other => panic!("{other:?}"),
    }
}

fn entry(job: &Job, key: &str) -> PathBuf {
    job.out().join(key)
}

fn meta(job: &Job, key: &str) -> EntryMeta {
    serde_json::from_slice(&std::fs::read(entry(job, key).join(META_FILE)).unwrap()).unwrap()
}

fn table_of(path: &Path) -> SampleTable {
    Demuxer::open(File::open(path).unwrap(), &Limits::default())
        .unwrap()
        .video()
        .unwrap()
        .table
        .clone()
}

/// Every frame id of `path` in presentation order.
fn ids(path: &Path) -> Vec<Option<u32>> {
    let mut reader = FrameReader::open(
        File::open(path).unwrap(),
        &Limits::default(),
        &ProbeResult::software_only(),
    )
    .unwrap();
    (0..reader.table().len())
        .map(|i| {
            let f = reader.frame(i).unwrap();
            let p = &f.planes[0];
            let y: Vec<u8> = (0..f.size.height)
                .flat_map(|row| p.row(row, 1).to_vec())
                .collect();
            read_id_y(&y, f.size.width, f.size.height)
        })
        .collect()
}

/// The frame id a PNG (or a tile of a sprite at `x`) shows.
fn id_of(png: &Path, x: u32, tile: Size) -> Option<u32> {
    let (size, rgba) = read_png(png);
    read_id_rgba(
        &rgba,
        size.width,
        size.height,
        Rect {
            x: x as f64,
            y: 0.0,
            width: tile.width as f64,
            height: tile.height as f64,
        },
    )
}

#[test]
fn thumb_frame_ids_follow_frame_at() {
    if !gpu_or_skip("thumb_frame_ids_follow_frame_at") {
        return;
    }
    let c = clip(&ClipSpec {
        vfr: true,
        ..spec(20, Size::new(320, 180))
    });
    let table = table_of(&c.path);
    let want_ids = ids(&c.path);
    assert!(want_ids.iter().all(Option::is_some));
    // Every midpoint between two frames (a tie: the earlier frame), just after it (the later),
    // before the first frame and past the end.
    let mut times = vec![r(-0, 1)];
    let f = table.frames();
    for i in 0..table.len() - 1 {
        let mid = r(f[i].pts + f[i + 1].pts, 2 * table.timescale() as i64);
        times.push(mid);
        times.push(r(
            (f[i].pts + f[i + 1].pts) * 1_000_000 + 1,
            2 * table.timescale() as i64 * 1_000_000,
        ));
    }
    times.push(r(1000, 1));
    times.truncate(64);
    let p = params(FP, times_mode(times.clone(), 320));
    let job = job_for(FP, Some(&c.path));
    let run = go(&job, &p);
    let got = items(&run);
    assert_eq!(got.len(), times.len(), "{:?}", run.end);
    assert!(failed(&run).is_empty());
    let r = result(&run);
    assert!(r.complete, "{r:?}");
    assert_eq!(r.source, "original");
    let dir = entry(&job, &p.key);
    for (n, (item, v)) in got.iter().enumerate() {
        assert_eq!(item, &n.to_string());
        let at = frame_at(&table, times[n]);
        assert_eq!(v.frame, Some(at.index as u64), "time {}", times[n]);
        assert_eq!(v.at, Some(times[n]));
        assert_eq!((v.width, v.height), (320, 180), "never upscaled");
        assert_eq!(v.file, format!("t{n}.png"));
        assert_eq!(
            id_of(&dir.join(&v.file), 0, Size::new(320, 180)),
            want_ids[at.index],
            "time {} (frame {})",
            times[n],
            at.index
        );
    }
    // The entry is what the cache commits: listed files, a key that recomputes.
    let m = meta(&job, &p.key);
    m.check().unwrap();
    assert_eq!(m.recipe, "thumbs");
    let names: Vec<_> = m.files.iter().map(|f| f.name.clone()).collect();
    assert_eq!(
        names,
        (0..times.len())
            .map(|i| format!("t{i}.png"))
            .collect::<Vec<_>>()
    );
    assert_eq!(m.decoder_class, VendorClass::Software);
}

#[test]
fn thumbnails_fit_max_side_and_stand_upright() {
    if !gpu_or_skip("thumbnails_fit_max_side_and_stand_upright") {
        return;
    }
    // 1280 × 720 to 512 × 288; a clip coded 320 × 180 and rotated 90° is shown 180 × 320.
    let wide = clip(&spec(6, Size::new(1280, 720)));
    let rotated = clip(&ClipSpec {
        rotation: Rotation::R90,
        ..spec(6, Size::new(320, 180))
    });
    for (c, max_side, want) in [
        (&wide, 512, (512, 288)),
        (&wide, 100, (100, 56)),
        (&rotated, 256, (144, 256)),
        (&rotated, 512, (180, 320)),
    ] {
        let p = params(FP, times_mode(vec![r(0, 1), r(1, 10)], max_side));
        let job = job_for(FP, Some(&c.path));
        let run = go(&job, &p);
        let got = items(&run);
        assert_eq!(got.len(), 2, "{:?}", run.end);
        for (_, v) in &got {
            assert_eq!((v.width, v.height), want, "max side {max_side}");
            let (size, rgba) = read_png(&entry(&job, &p.key).join(&v.file));
            assert_eq!((size.width, size.height), want);
            assert_eq!(rgba.len(), (want.0 * want.1 * 4) as usize);
        }
    }
    // The ids survive the scale on the unrotated clip.
    let p = params(FP, times_mode(vec![r(2, 30)], 512));
    let job = job_for(FP, Some(&wide.path));
    let run = go(&job, &p);
    let v = &items(&run)[0].1;
    assert_eq!(
        id_of(&entry(&job, &p.key).join(&v.file), 0, Size::new(512, 288)),
        Some(2)
    );
}

#[test]
fn strip_index_times() {
    if !gpu_or_skip("strip_index_times") {
        return;
    }
    // 25 frames at 2 fps: 12.5 s, so 13 tiles (one per second) in 2 sprites (10 s each).
    let c = clip(&ClipSpec {
        rate: Rational::from_int(2),
        ..spec(25, Size::new(640, 360))
    });
    let table = table_of(&c.path);
    let want_ids = ids(&c.path);
    let mode = ThumbsMode::Strip {
        tile_height: 180,
        sprites: 2,
    };
    let p = params(FP, mode);
    let job = job_for(FP, Some(&c.path));
    let run = go(&job, &p);
    let got = items(&run);
    assert_eq!(
        got.iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>(),
        ["0", "1"],
        "{:?}",
        run.end
    );
    assert!(result(&run).complete);
    let dir = entry(&job, &p.key);
    let index: StripIndex =
        serde_json::from_slice(&std::fs::read(dir.join("strip.json")).unwrap()).unwrap();
    assert_eq!((index.tile_width, index.tile_height), (320, 180));
    assert_eq!((index.tile_seconds, index.sprite_seconds), (1, 10));
    assert_eq!(index.sprites.len(), 2);
    assert_eq!(index.sprites[0].file, "s0.png");
    assert_eq!(index.sprites[1].file, "s1.png");
    assert_eq!(index.sprites[0].tiles.len(), 10);
    assert_eq!(index.sprites[1].tiles.len(), 3);
    let tile = Size::new(index.tile_width, index.tile_height);
    for (k, sprite) in index.sprites.iter().enumerate() {
        let (size, _) = read_png(&dir.join(&sprite.file));
        assert_eq!(
            (size.width, size.height),
            (tile.width * sprite.tiles.len() as u32, 180),
            "sprite {k}"
        );
        assert_eq!(got[k].1.tiles, Some(sprite.tiles.len() as u32));
        assert_eq!(got[k].1.file, sprite.file);
        for (n, t) in sprite.tiles.iter().enumerate() {
            let j = (10 * k + n) as i64;
            assert_eq!(t.at, r(j, 1), "sprite {k} tile {n}");
            let at = frame_at(&table, t.at);
            assert_eq!(t.frame, at.index as u64);
            assert_eq!(at.index as i64, 2 * j, "2 fps: one frame per half second");
            assert_eq!(
                id_of(&dir.join(&sprite.file), n as u32 * tile.width, tile),
                want_ids[at.index],
                "sprite {k} tile {n}"
            );
        }
    }
    let m = meta(&job, &p.key);
    m.check().unwrap();
    assert_eq!(m.recipe, "strip");
    let names: Vec<_> = m.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["s0.png", "s1.png", "strip.json"]);
    // A host that planned another sprite count is refused before anything is written.
    let wrong = params(
        FP,
        ThumbsMode::Strip {
            tile_height: 180,
            sprites: 3,
        },
    );
    let job = job_for(FP, Some(&c.path));
    let run = go(&job, &wrong);
    let codes: Vec<_> = failed(&run).into_iter().map(|(_, c)| c).collect();
    assert_eq!(codes, ["PLAN_MISMATCH"; 3], "{:?}", run.end);
    assert!(!entry(&job, &wrong.key).join("s0.png").exists());
}

#[test]
fn crash_keeps_done_items() {
    if !gpu_or_skip("crash_keeps_done_items") {
        return;
    }
    let c = clip(&spec(12, Size::new(320, 180)));
    // The test hook kills the worker after it acknowledged its second item.
    let fp = "test-crash-after-2-fp";
    let times: Vec<_> = (0..5).map(|i| r(i, 10)).collect();
    let p = params(fp, times_mode(times, 320));
    let job = job_for(fp, Some(&c.path));
    let first = go(&job, &p);
    assert!(matches!(first.end, End::Died(_)), "{:?}", first.end);
    let acked: Vec<_> = items(&first).into_iter().map(|(i, _)| i).collect();
    assert_eq!(acked, ["0", "1"], "{:?}", first.events);
    let dir = entry(&job, &p.key);
    assert!(
        !dir.join(META_FILE).exists(),
        "an unfinished entry has no meta.json"
    );
    let before: Vec<_> = ["t0.png", "t1.png"]
        .iter()
        .map(|f| {
            let m = std::fs::metadata(dir.join(f)).unwrap();
            (std::fs::read(dir.join(f)).unwrap(), m.modified().unwrap())
        })
        .collect();

    // The retry the engine makes: the same params restricted to the items it has not
    // acknowledged, in the same job directory.
    let mut retry = p.clone();
    retry.only = Some(vec!["2".into(), "3".into(), "4".into()]);
    let second = go(&job, &retry);
    let got: Vec<_> = items(&second).into_iter().map(|(i, _)| i).collect();
    assert_eq!(got, ["2", "3", "4"], "{:?}", second.end);
    let r = result(&second);
    assert!(r.complete);
    assert_eq!(r.made, 3);
    for (i, f) in ["t0.png", "t1.png"].iter().enumerate() {
        let m = std::fs::metadata(dir.join(f)).unwrap();
        assert_eq!(
            (std::fs::read(dir.join(f)).unwrap(), m.modified().unwrap()),
            before[i],
            "{f} was kept, not made again"
        );
    }
    let m = meta(&job, &p.key);
    m.check().unwrap();
    assert_eq!(m.files.len(), 5);
    assert_eq!(
        m.bytes().unwrap(),
        m.files.iter().map(|f| f.bytes).sum::<u64>()
    );
}

/// Makes the `1080` proxy of `spec` and returns its job (the entry is `out/<key>/`).
fn made_proxy(c: &Clip) -> (Job, String) {
    let tier = ProxyTier::Preview;
    let key = proxy_key(FP, tier, ProxyCodec::Av1, VendorClass::Software).to_string();
    let p = ProxyParams {
        fingerprint: FP.into(),
        tier,
        codec: ProxyCodec::Av1,
        decoder_class: VendorClass::Software,
        key: key.clone(),
        asset: Some("a1".into()),
        file: None,
        project_id: None,
        branch: None,
        display: None,
        intra_ok: false,
    };
    let job = job_for(FP, Some(&c.path));
    let mut w: Proc = spawn_with(&["media"], &worker_env());
    w.hello();
    let start = job.start(
        "media.proxy",
        serde_json::to_value(&p).unwrap(),
        &["in/manifest.json", "out/"],
    );
    w.send(&HostMsg::Start(start));
    let run = w.drive(no_calls);
    let made = run
        .events
        .iter()
        .find_map(|e| match e {
            Ev::Item(_, Some(v)) => Some(serde_json::from_value::<ProxyResult>(v.clone()).unwrap()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no proxy: {:?}", run.end));
    assert_eq!(made.key, key);
    (job, key)
}

fn proxy_of(job: &Job, key: &str) -> ThumbsProxy {
    ThumbsProxy {
        key: key.into(),
        dir: entry(job, key).to_str().unwrap().into(),
        codec: ProxyCodec::Av1,
    }
}

#[test]
fn reads_proxy_when_present() {
    if !gpu_or_skip("reads_proxy_when_present") {
        return;
    }
    // A VFR clip, so the proxy's frame i only matches the original's through `table.bin`.
    let c = clip(&ClipSpec {
        vfr: true,
        ..spec(20, Size::new(1280, 720))
    });
    let table = table_of(&c.path);
    let want_ids = ids(&c.path);
    let (proxy_job, key) = made_proxy(&c);
    let f = table.frames();
    let times: Vec<Rational> = (0..table.len() - 1)
        .step_by(3)
        .map(|i| r(f[i].pts + f[i + 1].pts, 2 * table.timescale() as i64))
        .chain([r(0, 1), r(1000, 1)])
        .collect();
    let mut p = params(FP, times_mode(times.clone(), 512));
    p.proxy = Some(proxy_of(&proxy_job, &key));
    // The asset's file is not on this machine: only the proxy can answer.
    let job = job_for(FP, None);
    let run = go(&job, &p);
    let r = result(&run);
    assert_eq!(r.source, "proxy", "{:?}", run.end);
    assert_eq!(r.proxy_failed, None);
    assert!(r.complete);
    let got = items(&run);
    assert_eq!(got.len(), times.len());
    let dir = entry(&job, &p.key);
    for (n, (_, v)) in got.iter().enumerate() {
        let at = frame_at(&table, times[n]);
        assert_eq!(v.frame, Some(at.index as u64), "time {}", times[n]);
        assert_eq!((v.width, v.height), (512, 288));
        assert_eq!(
            id_of(&dir.join(&v.file), 0, Size::new(512, 288)),
            want_ids[at.index],
            "time {}",
            times[n]
        );
    }
    // The entry names the original's decoder class and content, as a thumbnail from the
    // original would.
    let m = meta(&job, &p.key);
    m.check().unwrap();
    assert_eq!(m.decoder_class, VendorClass::Software);

    // The same request without the proxy needs the original, which is missing.
    p.proxy = None;
    let job = job_for(FP, None);
    let run = go(&job, &p);
    assert_eq!(failed(&run)[0].1, "ORIGINALS_MISSING", "{:?}", run.events);

    // A proxy that cannot be read (its table is gone) falls back to the original, and says so.
    let (proxy_job, key) = made_proxy(&c);
    std::fs::remove_file(entry(&proxy_job, &key).join("table.bin")).unwrap();
    let mut p = params(FP, times_mode(times.clone(), 512));
    p.proxy = Some(proxy_of(&proxy_job, &key));
    let job = job_for(FP, Some(&c.path));
    let run = go(&job, &p);
    let r = result(&run);
    assert_eq!(r.source, "original", "{:?}", run.end);
    assert!(r.proxy_failed.is_some());
    assert_eq!(items(&run).len(), times.len());
    // No absolute path of this host reaches the answer.
    let text = format!("{:?}", run.events);
    assert!(
        !text.contains(proxy_job.dir.path().to_str().unwrap()),
        "{text}"
    );
    assert!(!text.contains(job.dir.path().to_str().unwrap()), "{text}");
}

#[test]
fn a_proxy_of_other_content_is_not_read() {
    if !gpu_or_skip("a_proxy_of_other_content_is_not_read") {
        return;
    }
    let c = clip(&spec(8, Size::new(640, 360)));
    let (proxy_job, key) = made_proxy(&c);
    // The meta.json of the entry names fingerprint `fp`: a request for other content that was
    // handed this proxy reads the original instead.
    let mut p = params("other", times_mode(vec![r(0, 1)], 256));
    p.proxy = Some(proxy_of(&proxy_job, &key));
    let job = job_for("other", Some(&c.path));
    let run = go(&job, &p);
    let r = result(&run);
    assert_eq!(r.source, "original", "{:?}", run.end);
    assert!(r.proxy_failed.is_some());
}

#[test]
fn typed_refusals() {
    if !gpu_or_skip("typed_refusals") {
        return;
    }
    let c = clip(&spec(8, Size::new(320, 180)));
    // The manifest's content is not the one asked for.
    let p = params("fp-b", times_mode(vec![r(0, 1)], 256));
    let job = job_for("fp-a", Some(&c.path));
    let run = go(&job, &p);
    assert_eq!(failed(&run)[0].1, "ASSET_CHANGED", "{:?}", run.events);
    assert!(!entry(&job, &p.key).join("meta.json").exists());

    // The key was planned for another decoder class.
    let mut p = params(FP, times_mode(vec![r(0, 1)], 256));
    p.decoder_class = VendorClass::Os;
    p.key = thumbs_key(FP, &p.mode, VendorClass::Os).to_string();
    let job = job_for(FP, Some(&c.path));
    let run = go(&job, &p);
    assert_eq!(failed(&run)[0].1, "PLAN_MISMATCH", "{:?}", run.events);
    assert!(items(&run).is_empty());

    // A key that is not the params' own.
    let mut p = params(FP, times_mode(vec![r(0, 1)], 256));
    p.key = "0".repeat(64);
    let job = job_for(FP, Some(&c.path));
    let run = go(&job, &p);
    assert_eq!(failed(&run)[0].1, "PLAN_MISMATCH", "{:?}", run.events);

    // Params that do not parse or check are the job's error.
    let job = job_for(FP, Some(&c.path));
    let mut v = serde_json::to_value(params(FP, times_mode(vec![r(0, 1)], 256))).unwrap();
    v["mode"]["thumbs"]["maxSide"] = json!(9999);
    let run = run_params(&job, &v);
    match &run.end {
        End::Error(e) => assert_eq!(e.code, "JOB_PARAMS_INVALID"),
        other => panic!("{other:?}"),
    }

    // A file that is not a video.
    let dir = tempfile::tempdir().unwrap();
    let junk = std::fs::canonicalize(dir.path()).unwrap().join("x.mp4");
    std::fs::write(&junk, b"not a movie at all, not even close").unwrap();
    let p = params(FP, times_mode(vec![r(0, 1)], 256));
    let job = job_for(FP, Some(&junk));
    let run = go(&job, &p);
    assert!(matches!(run.end, End::Result(_)), "{:?}", run.end);
    assert_eq!(failed(&run).len(), 1, "{:?}", run.events);
}

#[test]
fn hdr_source_is_tone_mapped() {
    if !gpu_or_skip("hdr_source_is_tone_mapped") {
        return;
    }
    let c = clip(&ClipSpec {
        colour: PQ,
        ..spec(6, Size::new(320, 180))
    });
    let p = params(FP, times_mode(vec![r(0, 1)], 320));
    let job = job_for(FP, Some(&c.path));
    let run = go(&job, &p);
    assert_eq!(items(&run).len(), 1, "{:?}", run.end);
    assert_eq!(meta(&job, &p.key).tone_mapped, Some(ToneMapped::Shader));
}
