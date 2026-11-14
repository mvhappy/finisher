//! Writes a frame-id pattern clip as AV1 in MP4:
//! `cargo run -p fin-codecs --features testing --example make_fixture -- <out.mp4>
//! [--frames N] [--size WxH] [--rate N[/D]] [--vfr] [--rotate 0|90|180|270] [--keyint N]
//! [--crf N] [--preset N] [--seed N] [--edits FRAMES:FROM|-,…]`.
//!
//! `--edits` writes that edit list in place of the computed one, in frame lengths: `5:-,10:5`
//! is 5 frames of nothing, then 10 frames of the media from its frame 5.

use fin_codecs::fixture::{ClipSpec, pattern_clip, timing};
use fin_core::Rational;
use fin_media::{Rotation, Size};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut out = None;
    let mut spec = ClipSpec::default();
    let mut edits = None;
    let num = |v: Option<String>, what: &str| -> u64 {
        v.and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{what} takes a number"))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => spec.frames = num(args.next(), "--frames") as u32,
            "--size" => {
                let v = args.next().expect("--size WxH");
                let (w, h) = v.split_once('x').expect("--size WxH");
                spec.size = Size::new(w.parse().expect("width"), h.parse().expect("height"));
            }
            "--rate" => {
                spec.rate = args
                    .next()
                    .expect("--rate N[/D]")
                    .parse::<Rational>()
                    .expect("a rate like 30 or 30000/1001")
            }
            "--vfr" => spec.vfr = true,
            "--rotate" => {
                spec.rotation = Rotation::try_from(num(args.next(), "--rotate") as u16)
                    .expect("0, 90, 180 or 270")
            }
            "--keyint" => spec.keyint = num(args.next(), "--keyint") as u32,
            "--crf" => spec.crf = num(args.next(), "--crf") as u8,
            "--preset" => spec.preset = num(args.next(), "--preset") as u8,
            "--seed" => spec.seed = num(args.next(), "--seed"),
            "--edits" => edits = Some(args.next().expect("--edits FRAMES:FROM|-,…")),
            path if out.is_none() && !path.starts_with("--") => out = Some(path.to_string()),
            other => panic!("unknown argument {other}"),
        }
    }
    let out = out.expect("usage: make_fixture <out.mp4> [options]");
    if let Some(list) = edits {
        // The track's ticks of one frame.
        let tick = u64::from(timing(spec.rate).1);
        let frames = |v: &str| v.parse::<u64>().expect("--edits takes frame counts") * tick;
        spec.edits = Some(
            list.split(',')
                .map(|e| {
                    let (len, from) = e.split_once(':').expect("--edits FRAMES:FROM|-");
                    (frames(len), (from != "-").then(|| frames(from)))
                })
                .collect(),
        );
    }
    let bytes = pattern_clip(&spec).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&out, bytes).unwrap_or_else(|e| panic!("{out}: {e}"));
    println!("{out}");
}
