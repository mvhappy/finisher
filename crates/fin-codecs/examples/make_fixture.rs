//! Writes a frame-id pattern clip as AV1 in MP4:
//! `cargo run -p fin-codecs --features testing --example make_fixture -- <out.mp4>
//! [--frames N] [--size WxH] [--rate N[/D]] [--vfr] [--rotate 0|90|180|270] [--keyint N]
//! [--crf N] [--preset N] [--seed N]`.

use fin_codecs::fixture::{ClipSpec, pattern_clip};
use fin_core::Rational;
use fin_media::{Rotation, Size};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut out = None;
    let mut spec = ClipSpec::default();
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
            path if out.is_none() && !path.starts_with("--") => out = Some(path.to_string()),
            other => panic!("unknown argument {other}"),
        }
    }
    let out = out.expect("usage: make_fixture <out.mp4> [options]");
    let bytes = pattern_clip(&spec).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&out, bytes).unwrap_or_else(|e| panic!("{out}: {e}"));
    println!("{out}");
}
