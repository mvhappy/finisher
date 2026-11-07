//! Probes of every asset kind, from fixtures made in code (plan 4C Task 21).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fin_core::layout::faces_for;
use fin_core::{
    AssetKind, CaptionStyle, FaceRef, FontFormat, FontPin, FontSource, Measured, Rational,
    SlotSpec, TextMeasurer, TextRequest, TextStyle, Word, font_key, new_project,
};
use fin_probe::fixtures::{
    FixtureOpts, sample_lottie, write_cube, write_dotlottie, write_jpeg, write_m4a, write_png,
    write_svg, write_wav, write_webp, write_zip,
};
use fin_probe::{Probe, probe};
use serde_json::json;
use sha2::{Digest, Sha256};

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

fn file(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn unsupported(path: &Path) -> String {
    let err = probe(path).unwrap_err();
    assert_eq!(err.code, "UNSUPPORTED_CONTAINER", "{err:?}");
    assert!(err.fix.is_some());
    err.details["reason"].as_str().unwrap().to_string()
}

fn image(p: &Probe) -> (u32, u32) {
    let i = p.image.as_ref().unwrap();
    (i.width, i.height)
}

#[test]
fn png_jpeg_and_webp_are_images() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.png", "a.jpg", "a.webp"] {
        // The kind comes from the bytes, not the name.
        let path = dir.path().join(format!("{name}.bin"));
        match name {
            "a.png" => write_png(&path, 64, 36),
            "a.jpg" => write_jpeg(&path, 64, 36),
            _ => write_webp(&path, 64, 36),
        }
        let p = probe(&path).unwrap();
        assert_eq!(p.kind, AssetKind::Image, "{name}");
        assert_eq!(image(&p), (64, 36), "{name}");
        assert_eq!((p.duration, &p.video, &p.audio), (None, &None, &None));
        assert_eq!(p.size, std::fs::metadata(&path).unwrap().len());
        assert!(p.fingerprint.starts_with("fp1:"));
    }
}

#[test]
fn svg_sizes_come_from_width_height_and_view_box() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logo.svg");
    write_svg(&path, 320, 180);
    let p = probe(&path).unwrap();
    assert_eq!((p.kind, image(&p)), (AssetKind::Svg, (320, 180)));
    let ns = r#"xmlns="http://www.w3.org/2000/svg""#;
    for (attrs, size) in [
        (r#"width="100px" height="50.4""#, (100, 50)),
        (r#"viewBox="0 0 640,360""#, (640, 360)),
        (r#"width="200" viewBox="0 0 400 100""#, (200, 50)),
        (r#"height="30" viewBox="-5 -5 40 10""#, (120, 30)),
    ] {
        let svg = format!("\u{feff}<!-- c -->\n<svg {ns} {attrs}/>");
        let path = file(&dir, "s.svg", svg.as_bytes());
        assert_eq!(image(&probe(&path).unwrap()), size, "{attrs}");
    }
    // DOCTYPE allowed; no namespace is still an <svg> root.
    let doctype = "<?xml version=\"1.0\"?>\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \
                   \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n<svg width=\"8\" height=\"9\"/>";
    assert_eq!(
        image(&probe(&file(&dir, "d.svg", doctype.as_bytes())).unwrap()),
        (8, 9)
    );
}

#[test]
fn svg_sizes_not_in_px_are_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    for (attrs, reason) in [
        (r#"width="10cm" height="5cm""#, "not in px"),
        (r#"width="100%" height="100%""#, "not in px"),
        (r#"width="-4" height="5""#, "not in px"),
        (r#"viewBox="0 0 0 10""#, "viewBox"),
        ("", "no width and height or viewBox"),
        (r#"width="0.2" height="9""#, "no pixels"),
    ] {
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" {attrs}/>"#);
        let path = file(&dir, "s.svg", svg.as_bytes());
        assert!(unsupported(&path).contains(reason), "{attrs}");
    }
    let html = file(&dir, "x.html", b"<html><body/></html>");
    assert_eq!(unsupported(&html), "the XML root is not <svg>");
    let other_ns = file(
        &dir,
        "x.svg",
        br#"<svg xmlns="urn:x" width="1" height="1"/>"#,
    );
    assert_eq!(unsupported(&other_ns), "the XML root is not <svg>");
}

#[test]
fn dotlottie_stored_and_deflated() {
    let dir = tempfile::tempdir().unwrap();
    for deflated in [false, true] {
        let path = dir.path().join(format!("t{deflated}.lottie"));
        write_dotlottie(&path, &sample_lottie(), deflated);
        let p = probe(&path).unwrap();
        assert_eq!(p.kind, AssetKind::LottieTemplate);
        let l = p.lottie.as_ref().unwrap();
        assert_eq!(
            (l.width, l.height, l.rate, l.duration),
            (1920, 1080, r(30, 1), r(3, 1))
        );
        assert_eq!(p.duration, Some(r(3, 1)));
        assert_eq!((&p.image, &p.video, &p.audio), (&None, &None, &None));
    }
}

#[test]
fn lottie_slots_and_fonts_listed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.lottie");
    write_dotlottie(&path, &sample_lottie(), true);
    let l = probe(&path).unwrap().lottie.unwrap();
    let slot = |id: &str, kind: &str| SlotSpec {
        id: id.into(),
        kind: kind.into(),
    };
    assert_eq!(
        l.slots,
        [
            slot("accent", "color"),
            slot("fade", "number"),
            slot("logo", "image"),
            slot("title", "text")
        ]
    );
    assert_eq!(l.fonts, ["Inter", "Roboto Mono"]);

    // Animated colors and numbers, NTSC rates, offset frames, no slots or fonts, and
    // well-formed glyph `chars`.
    let mut anim = sample_lottie();
    anim["fr"] = json!(29.97);
    anim["ip"] = json!(10);
    anim["op"] = json!(40);
    anim["fonts"] = json!({ "list": [{ "fFamily": "Inter" }, { "fFamily": "Inter" }] });
    anim["slots"] = json!({
        "c": { "p": { "a": 1, "k": [{ "t": 0, "s": [1, 0, 0] }, { "t": 9, "s": [0, 0, 1] }] } },
        "n": { "p": { "a": 1, "k": [{ "t": 0, "s": [0] }, { "t": 9, "s": [100] }] } },
    });
    anim["chars"] = json!([{ "ch": "A", "size": 48, "style": "Regular", "w": 60,
        "fFamily": "Inter", "data": { "shapes": [{ "ty": "gr", "it": [] }] } }]);
    write_dotlottie(&path, &anim, false);
    let l = probe(&path).unwrap().lottie.unwrap();
    assert_eq!(l.rate, r(30000, 1001));
    assert_eq!(l.duration, r(1001, 1000)); // 30 frames at 29.97
    assert_eq!(l.slots, [slot("c", "color"), slot("n", "number")]);
    assert_eq!(l.fonts, ["Inter"]);
    for key in ["slots", "fonts", "chars"] {
        anim.as_object_mut().unwrap().remove(key);
    }
    write_dotlottie(&path, &anim, true);
    let l = probe(&path).unwrap().lottie.unwrap();
    assert!(l.slots.is_empty() && l.fonts.is_empty());
}

#[test]
fn malformed_lottie_is_unsupported_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.lottie");
    let with = |key: &str, v: serde_json::Value| {
        let mut anim = sample_lottie();
        anim[key] = v;
        anim
    };
    for (anim, reason) in [
        (with("chars", json!([{ "ch": "A" }])), "malformed chars[0]"),
        (with("chars", json!({})), "malformed chars"),
        (
            with(
                "chars",
                json!([{ "ch": "A", "size": 1, "style": "R", "w": 1, "fFamily": "I",
                "data": { "shapes": [3] } }]),
            ),
            "malformed chars[0]",
        ),
        (
            with("slots", json!({ "v": { "p": { "k": [1, 2] } } })),
            "slot \"v\"",
        ),
        (with("slots", json!([])), "slots is not an object"),
        (
            with("fonts", json!({ "list": [{ "fName": "x" }] })),
            "fonts.list[0] has no fFamily",
        ),
        (with("op", json!(0)), "op must be after ip"),
        (with("fr", json!(0)), "fr must be"),
        (with("w", json!(10.5)), "w must be a positive whole number"),
        (with("h", json!("tall")), "no numeric h"),
    ] {
        write_dotlottie(&path, &anim, true);
        let got = unsupported(&path);
        assert!(got.contains(reason), "{got} / {reason}");
    }
    let manifest = |m: &str| ("manifest.json", m.to_string());
    let anim = ("animations/main.json", sample_lottie().to_string());
    for (members, reason) in [
        (vec![anim.clone()], "manifest.json"),
        (
            vec![manifest("{"), anim.clone()],
            "malformed JSON in manifest.json",
        ),
        (
            vec![manifest(r#"{"animations":[]}"#), anim.clone()],
            "no animation id",
        ),
        (
            vec![manifest(r#"{"animations":[{"id":"../x"}]}"#)],
            "not a file name",
        ),
        (
            vec![manifest(r#"{"animations":[{"id":"other"}]}"#), anim.clone()],
            "animations/other.json",
        ),
        (
            vec![
                manifest(r#"{"animations":[{"id":"main"}]}"#),
                ("animations/main.json", "[1".into()),
            ],
            "malformed JSON in animations/main.json",
        ),
    ] {
        write_zip(&path, &members, false);
        let got = unsupported(&path);
        assert!(got.contains(reason), "{got} / {reason}");
    }
}

fn font_bytes() -> Vec<u8> {
    fin_testfonts::test_sans()
}

#[test]
fn fonts_list_their_faces() {
    let dir = tempfile::tempdir().unwrap();
    let ttf = file(&dir, "a.ttf", &font_bytes());
    let p = probe(&ttf).unwrap();
    assert_eq!((p.kind, p.duration), (AssetKind::Font, None));
    let f = p.font.unwrap();
    assert_eq!(f.format, FontFormat::Ttf);
    assert_eq!(f.size, std::fs::metadata(&ttf).unwrap().len());
    let info = fin_text::font_info(&font_bytes()).unwrap();
    assert_eq!(f.faces.len(), 1);
    let face = &f.faces[0];
    assert_eq!(
        (
            face.index,
            &face.family,
            face.weight,
            face.italic,
            &face.scripts
        ),
        (
            info[0].index,
            &info[0].family,
            info[0].weight,
            info[0].italic,
            &info[0].scripts
        )
    );
    assert_eq!(face.family, "TestSans");
    assert!(face.scripts.contains(&"Latn".to_string()));

    let ttc = fin_testfonts::collection(&[font_bytes(), fin_testfonts::test_han()]);
    let f = probe(&file(&dir, "a.ttc", &ttc)).unwrap().font.unwrap();
    assert_eq!(f.format, FontFormat::Ttc);
    let faces: Vec<(u32, &str)> = f
        .faces
        .iter()
        .map(|f| (f.index, f.family.as_str()))
        .collect();
    assert_eq!(faces, [(0, "TestSans"), (1, "TestHan")]);
    assert_eq!(f.sha256, hex_sha256(&ttc));
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn font_probe_sha256_is_whole_file() {
    let dir = tempfile::tempdir().unwrap();
    // Large enough that the fingerprint samples only the head, middle and tail.
    let mut bytes = fin_testfonts::test_big(1024 * 1024);
    let path = file(&dir, "big.ttf", &bytes);
    let f = probe(&path).unwrap().font.unwrap();
    assert_eq!(f.sha256, hex_sha256(&bytes));
    assert_eq!(f.sha256.len(), 64);
    // A byte outside the fingerprint's samples (in the padding table) changes the hash.
    let at = 100 * 1024;
    bytes[at] ^= 0x55;
    std::fs::write(&path, &bytes).unwrap();
    let changed = probe(&path).unwrap().font.unwrap();
    assert_ne!(changed.sha256, f.sha256);
    assert_eq!(changed.sha256, hex_sha256(&bytes));
}

struct OneFile(String, Arc<[u8]>);

impl fin_text::FontBytes for OneFile {
    fn get(&self, sha256: &str) -> Option<Arc<[u8]>> {
        (sha256 == self.0).then(|| self.1.clone())
    }
}

/// The probed face, as a pin of the asset would name it, resolves and measures with the
/// asset's bytes: the family the probe reports is the one the measurer finds.
#[test]
fn font_family_matches_the_measurer() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = font_bytes();
    let f = probe(&file(&dir, "a.ttf", &bytes)).unwrap().font.unwrap();
    let face = &f.faces[0];
    // The probed face, pinned as an asset font (Decision 37), is what `faces_for` resolves a
    // style of that family to.
    let mut project = new_project("p1".into(), "P".into(), r(30, 1)).unwrap();
    let key = font_key(&face.family, face.weight, face.italic);
    project.fonts.pins.insert(
        key.clone(),
        FontPin {
            family: face.family.clone(),
            weight: face.weight,
            italic: face.italic,
            face: face.index,
            sha256: f.sha256.clone(),
            size: f.size,
            format: f.format,
            scripts: face.scripts.clone(),
            source: FontSource::Asset { asset: "a1".into() },
        },
    );
    project.fonts.fallback.push(key);
    let mut style = CaptionStyle::default();
    style.text.family = face.family.clone();
    style.text.weight = face.weight;
    style.text.italic = face.italic;
    let words = ["Hello", "world"].map(|t| Word {
        text: t.into(),
        start: r(0, 1),
        end: r(1, 1),
    });
    let faces = faces_for(&project, &style, &words).unwrap();
    let face_ref = FaceRef {
        sha256: f.sha256.clone(),
        face: face.index,
        family: face.family.clone(),
        weight: face.weight,
        italic: face.italic,
    };
    assert_eq!(faces, vec![face_ref]);
    let m = fin_text::Measurer::new(Arc::new(OneFile(f.sha256.clone(), bytes.into())));
    let req = TextRequest {
        words: vec!["Hello".into(), "world".into()],
        style: TextStyle {
            family: face.family.clone(),
            weight: face.weight,
            italic: face.italic,
            size: 48.0,
            line_height: 1.2,
            letter_spacing: 0.0,
            align: fin_core::layout::Align::Left,
        },
        faces,
        max_width: None,
        max_lines: None,
    };
    let Measured::Layout(layout) = m.measure(&req).unwrap() else {
        panic!("the probed face is unavailable");
    };
    assert_eq!(layout.faces, std::slice::from_ref(&f.sha256));
    assert!(layout.missing.is_empty() && layout.width > 0);
}

#[test]
fn cube_luts() {
    let dir = tempfile::tempdir().unwrap();
    for size in [2, 17] {
        let path = dir.path().join(format!("l{size}.cube"));
        write_cube(&path, size);
        let p = probe(&path).unwrap();
        assert_eq!(
            (p.kind, p.lut.unwrap().size, p.duration),
            (AssetKind::Lut, size, None)
        );
    }
    let rows = |n: usize| "0 0 0\n".repeat(n);
    for (text, reason) in [
        (
            format!("LUT_3D_SIZE 2\n{}", rows(7)),
            "7 data rows; LUT_3D_SIZE 2 needs 8",
        ),
        (format!("LUT_3D_SIZE 2\n{}", rows(9)), "9 data rows"),
        (
            format!("LUT_3D_SIZE 1\n{}", rows(1)),
            "LUT_3D_SIZE must be 2 to 64",
        ),
        (
            format!("LUT_3D_SIZE 65\n{}", rows(1)),
            "LUT_3D_SIZE must be 2 to 64",
        ),
        (format!("LUT_1D_SIZE 4\n{}", rows(4)), "1D LUTs"),
        (format!("TITLE \"x\"\n{}", rows(8)), "no LUT_3D_SIZE"),
        (format!("LUT_3D_SIZE 2\n{}0 0\n", rows(7)), "data line 9"),
        (
            format!("LUT_3D_SIZE 2\n{}0 0 nan\n", rows(7)),
            "data line 9",
        ),
        (
            format!("LUT_3D_SIZE 2\n{}DOMAIN_MIN 0 0 0\n", rows(8)),
            "after the data rows",
        ),
        (
            format!("LUT_3D_SIZE 2\nSHAPER 1\n{}", rows(8)),
            "unknown keyword SHAPER",
        ),
    ] {
        let got = unsupported(&file(&dir, "x.cube", text.as_bytes()));
        assert!(got.contains(reason), "{got} / {reason}");
    }
}

#[test]
fn wav_is_audio_only_media() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.wav");
    write_wav(&path, 48_000, 2, 72_000);
    let p = probe(&path).unwrap();
    assert_eq!(
        (p.kind, p.duration, &p.video),
        (AssetKind::Media, Some(r(3, 2)), &None)
    );
    let a = p.audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (48_000, 2));
    write_wav(&path, 44_100, 1, 1);
    assert_eq!(probe(&path).unwrap().duration, Some(r(1, 44_100)));
    write_wav(&path, 44_100, 1, 0);
    assert_eq!(unsupported(&path), "WAV has no audio frames");
}

#[test]
fn m4a_is_audio_only_media() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.m4a");
    write_m4a(
        &path,
        FixtureOpts {
            seconds: 3,
            ..FixtureOpts::default()
        },
    );
    assert_eq!(&std::fs::read(&path).unwrap()[4..12], b"ftypM4A ");
    let p = probe(&path).unwrap();
    assert_eq!(
        (p.kind, p.duration, &p.video),
        (AssetKind::Media, Some(r(3, 1)), &None)
    );
    let a = p.audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (48_000, 2));
}

/// Every kind's fixture, cut short, is `UNSUPPORTED_CONTAINER` (never a panic or a probe of
/// what is left).
#[test]
fn truncated_inputs_are_unsupported_container() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, f: &dyn Fn(&Path)| {
        let path = dir.path().join(name);
        f(&path);
        std::fs::read(&path).unwrap()
    };
    let cases: Vec<(&str, Vec<u8>, Vec<usize>)> = vec![
        (
            "png",
            write("a.png", &|p| write_png(p, 8, 8)),
            vec![8, 12, 20],
        ),
        (
            "jpeg",
            write("a.jpg", &|p| write_jpeg(p, 8, 8)),
            vec![3, 10, 24],
        ),
        (
            "webp",
            write("a.webp", &|p| write_webp(p, 8, 8)),
            vec![12, 20, 25],
        ),
        (
            "svg",
            write("a.svg", &|p| write_svg(p, 8, 8)),
            vec![1, 30, 120],
        ),
        (
            "lottie",
            write("a.lottie", &|p| write_dotlottie(p, &sample_lottie(), true)),
            vec![4, 100],
        ),
        (
            "cube",
            write("a.cube", &|p| write_cube(p, 3)),
            vec![60, 200],
        ),
        (
            "wav",
            write("a.wav", &|p| write_wav(p, 8000, 1, 100)),
            vec![12, 30, 60, 100],
        ),
        (
            "m4a",
            write("a.m4a", &|p| write_m4a(p, FixtureOpts::default())),
            vec![12, 40, 200],
        ),
        (
            "ttf",
            font_bytes(),
            vec![4, 12, 40, font_bytes().len() / 2, font_bytes().len() - 1],
        ),
    ];
    for (kind, bytes, cuts) in cases {
        for cut in cuts {
            let path = file(&dir, "cut", &bytes[..cut]);
            let err = probe(&path).unwrap_err();
            assert_eq!(
                err.code, "UNSUPPORTED_CONTAINER",
                "{kind} cut at {cut}: {err:?}"
            );
            assert!(err.details["reason"].is_string());
        }
    }
}

#[test]
fn unknown_magic_is_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    for (bytes, reason) in [
        (&b""[..], "unknown file type"),
        (b"\x00\x01\x02\x03\x04binary junk\xff", "unknown file type"),
        (b"just some notes\nnot a lut\n", "unknown file type"),
        (b"GIF89a\x01\x00\x01\x00\x00\x00\x00;", "unknown file type"),
        (b"wOFF\x00\x01\x00\x00", "WOFF"),
        (b"RIFF\x04\x00\x00\x00AVI ", "unknown file type"),
        (b"\xff\xfe<\x00s\x00", "unknown file type"),
    ] {
        let got = unsupported(&file(&dir, "x.bin", bytes));
        assert!(got.contains(reason), "{bytes:?}: {got}");
    }
    let path = dir.path().join("x.zip");
    write_zip(&path, &[("readme.txt", "hi".into())], true);
    assert!(unsupported(&path).contains("manifest.json"));
}

#[test]
fn probe_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = [
        ("a.png", &(|p: &Path| write_png(p, 3, 2)) as &dyn Fn(&Path)),
        ("a.svg", &|p| write_svg(p, 3, 2)),
        ("a.lottie", &|p| write_dotlottie(p, &sample_lottie(), true)),
        ("a.cube", &|p| write_cube(p, 4)),
        ("a.wav", &|p| write_wav(p, 8000, 1, 80)),
        ("a.m4a", &|p| write_m4a(p, FixtureOpts::default())),
    ]
    .into_iter()
    .map(|(name, w)| {
        let path = dir.path().join(name);
        w(&path);
        path
    })
    .chain([file(&dir, "a.ttf", &font_bytes())])
    .collect();
    for path in paths {
        let a = probe(&path).unwrap();
        let b = probe(&path).unwrap();
        assert_eq!(a, b);
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, serde_json::to_string(&b).unwrap());
        assert_eq!(serde_json::from_str::<Probe>(&json).unwrap(), a);
    }
}

/// The JSON omits absent fields: an image carries `kind`, `image`, `fingerprint` and `size`.
#[test]
fn probe_json_omits_absent_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.png");
    write_png(&path, 2, 2);
    let v = serde_json::to_value(probe(&path).unwrap()).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["fingerprint", "image", "kind", "size"]);
    assert_eq!(v["kind"], "image");
    assert_eq!(v["image"], json!({ "width": 2, "height": 2 }));
}

/// An `<!ENTITY` is refused before any expansion: this 30 KB file expands to 150 MB.
#[test]
fn svg_entities_are_refused_before_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let ns = r#"xmlns="http://www.w3.org/2000/svg""#;
    let entity = "a".repeat(30_000);
    let refs = "&a;".repeat(5_000);
    let bomb = format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE svg [<!ENTITY a \"{entity}\">]>\n\
         <svg {ns} width=\"8\" height=\"8\"><text>{refs}</text></svg>"
    );
    let path = file(&dir, "bomb.svg", bomb.as_bytes());
    let started = std::time::Instant::now();
    assert_eq!(unsupported(&path), "the SVG declares XML entities");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // Even a single small entity, used or not.
    let one = format!("<!DOCTYPE svg [<!ENTITY w \"8\">]><svg {ns} width=\"&w;\" height=\"8\"/>");
    let path = file(&dir, "one.svg", one.as_bytes());
    assert_eq!(unsupported(&path), "the SVG declares XML entities");
}

#[test]
fn svg_percent_sizes_use_the_view_box() {
    let dir = tempfile::tempdir().unwrap();
    let ns = r#"xmlns="http://www.w3.org/2000/svg""#;
    for (attrs, size) in [
        (
            r#"width="100%" height="100%" viewBox="0 0 640 360""#,
            (640, 360),
        ),
        (r#"width="50%" viewBox="0 0 640 360""#, (640, 360)),
        (
            r#"width="100%" height="90" viewBox="0 0 640 360""#,
            (160, 90),
        ),
    ] {
        let path = file(&dir, "s.svg", format!("<svg {ns} {attrs}/>").as_bytes());
        assert_eq!(image(&probe(&path).unwrap()), size, "{attrs}");
    }
    let path = file(
        &dir,
        "s.svg",
        format!(r#"<svg {ns} width="100%" height="5"/>"#).as_bytes(),
    );
    assert!(unsupported(&path).contains("no viewBox"));
}

#[test]
fn images_and_svgs_past_32768_px_are_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a");
    write_jpeg(&path, 32_768, 8);
    assert_eq!(image(&probe(&path).unwrap()), (32_768, 8));
    write_jpeg(&path, 32_769, 8);
    assert!(unsupported(&path).contains("at most 32768 px"));
    write_webp(&path, 8, 40_000);
    assert!(unsupported(&path).contains("8×40000"));
    write_svg(&path, 40_000, 8);
    assert!(unsupported(&path).contains("at most 32768 px"));
    write_svg(&path, 32_768, 32_768);
    assert_eq!(image(&probe(&path).unwrap()), (32_768, 32_768));
}

/// A dotLottie whose animation is `head`, then `filler` `count` times, then `tail`, written
/// in pieces (the test never holds the whole member).
fn streamed_lottie(
    path: &Path,
    head: &str,
    filler: &str,
    count: usize,
    tail: &str,
    deflated: bool,
) {
    use std::io::Write;
    let method = if deflated {
        zip::CompressionMethod::Deflated
    } else {
        zip::CompressionMethod::Stored
    };
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(method)
        .large_file(false);
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    w.start_file("manifest.json", opts).unwrap();
    w.write_all(br#"{"animations":[{"id":"main"}]}"#).unwrap();
    w.start_file("animations/main.json", opts).unwrap();
    w.write_all(head.as_bytes()).unwrap();
    let chunk = filler.repeat(64 * 1024 / filler.len().max(1));
    let per = 64 * 1024 / filler.len().max(1);
    let mut left = count;
    while left > 0 {
        let n = left.min(per);
        w.write_all(&chunk.as_bytes()[..n * filler.len()]).unwrap();
        left -= n;
    }
    w.write_all(tail.as_bytes()).unwrap();
    w.finish().unwrap();
}

const ANIM_HEAD: &str = r#"{"w":64,"h":36,"fr":30,"ip":0,"op":30,"layers":["#;

/// Big ignored keys cost no tree: a 24 MiB `layers` array (12M numbers, ~400 MB as a JSON
/// tree) probes fine.
#[test]
fn dotlottie_skips_keys_it_does_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.lottie");
    streamed_lottie(&path, ANIM_HEAD, "0,", 12 * 1024 * 1024, "0]}", false);
    let l = probe(&path).unwrap().lottie.unwrap();
    assert_eq!((l.width, l.height, l.duration), (64, 36, r(1, 1)));
}

#[test]
fn dotlottie_members_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.lottie");
    // Past 32 MiB decompressed (stored, so not by ratio).
    streamed_lottie(&path, ANIM_HEAD, "0,", 17 * 1024 * 1024, "0]}", false);
    assert_eq!(
        unsupported(&path),
        "dotLottie member animations/main.json is larger than 32 MiB"
    );
    // A deflate bomb: ~16 KB expanding to 16 MiB (refused at 100× the compressed size).
    streamed_lottie(&path, ANIM_HEAD, "0,", 8 * 1024 * 1024, "0]}", true);
    assert!(std::fs::metadata(&path).unwrap().len() < 100_000);
    assert_eq!(
        unsupported(&path),
        "dotLottie member animations/main.json expands more than 100 times"
    );
    // The manifest is at most 1 MiB.
    let big = format!(
        r#"{{"animations":[{{"id":"main"}}],"x":"{}"}}"#,
        "a".repeat(1 << 20)
    );
    write_zip(&path, &[("manifest.json", big)], false);
    assert_eq!(
        unsupported(&path),
        "dotLottie member manifest.json is larger than 1 MiB"
    );
    // `slots` and `fonts` become trees only up to 1 MiB; `chars` glyphs are read one by one.
    let head = r#"{"w":64,"h":36,"fr":30,"ip":0,"op":30,"slots":{"s":{"p":{"k":["#;
    streamed_lottie(&path, head, "0,", 600 * 1024, "0]}}}}", false);
    assert_eq!(unsupported(&path), "slots is larger than 1 MiB");
    let glyph = r#"{"ch":"A","size":1,"style":"R","w":1,"fFamily":"I"},"#;
    let head = r#"{"w":64,"h":36,"fr":30,"ip":0,"op":30,"chars":["#;
    streamed_lottie(&path, head, glyph, 100_000, r#"{"ch":"B"}]}"#, false);
    assert_eq!(unsupported(&path), "malformed chars[100000]");
    streamed_lottie(
        &path,
        head,
        glyph,
        100_000,
        &format!("{}]}}", &glyph[..glyph.len() - 1]),
        false,
    );
    assert!(probe(&path).is_ok());
}

/// A streamed WAV's `data` length of `0xFFFFFFFF` runs to the end of the file.
#[test]
fn streamed_wav_data_runs_to_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.wav");
    write_wav(&path, 8_000, 1, 800);
    let mut bytes = std::fs::read(&path).unwrap();
    let at = bytes.windows(4).position(|w| w == b"data").unwrap();
    bytes[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(probe(&path).unwrap().duration, Some(r(1, 10)));
    // An odd trailing byte is not a frame.
    bytes.push(0);
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(probe(&path).unwrap().duration, Some(r(1, 10)));
}
