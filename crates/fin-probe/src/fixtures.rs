//! Test-time fixture writers (feature `fixtures`): MP4/MOV/M4A, PNG, JPEG, WebP, SVG,
//! dotLottie, `.cube` and WAV, header-valid bytes made in code. No media is committed.

use std::io::{Cursor, Write};
use std::path::Path;

use fin_container::{
    AudioTrackConfig, Brands, CodecConfig, Mp4Options, Mp4Writer, VideoTrackConfig,
};
use fin_core::Rational;
use fin_media::{ColourTags, Rotation, Size};

/// Shape of the generated file: an H.264-tagged video track (320x180) and, optionally, a
/// stereo AAC-tagged audio track. Sample payloads are dummy bytes. Written by `fin-container`'s
/// muxer.
#[derive(Debug, Clone, Copy)]
pub struct FixtureOpts {
    pub seconds: u32,
    pub fps: u32,
    pub audio: bool,
    /// One of 96000, 48000, 44100, 32000, 24000, 22050, 16000, 8000.
    pub sample_rate: u32,
}

impl Default for FixtureOpts {
    /// 2 s, 25 fps, 48 kHz audio.
    fn default() -> Self {
        FixtureOpts {
            seconds: 2,
            fps: 25,
            audio: true,
            sample_rate: 48_000,
        }
    }
}

fn freq_index(rate: u32) -> u8 {
    match rate {
        96_000 => 0,
        48_000 => 3,
        44_100 => 4,
        32_000 => 5,
        24_000 => 6,
        22_050 => 7,
        16_000 => 8,
        8_000 => 11,
        other => panic!("unsupported fixture sample rate {other}"),
    }
}

/// An `avcC` body: version 1, Baseline 3.0, 4-byte lengths, one SPS and one PPS.
fn avcc() -> Vec<u8> {
    let (sps, pps) = (
        [0x67u8, 0x42, 0x00, 0x1e, 0x95, 0xa8],
        [0x68u8, 0xce, 0x3c, 0x80],
    );
    let mut b = vec![1, 0x42, 0x00, 0x1e, 0xff, 0xe1];
    b.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    b.extend_from_slice(&sps);
    b.push(1);
    b.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    b.extend_from_slice(&pps);
    b
}

/// An `esds` body for stereo AAC-LC at `rate`.
fn esds(rate: u32) -> Vec<u8> {
    let asc = (2u16 << 11) | (u16::from(freq_index(rate)) << 7) | (2 << 3);
    let dsi = [&[0x05, 2][..], &asc.to_be_bytes()].concat();
    let mut dcd = vec![0x40, 0x15, 0, 0, 0];
    dcd.extend_from_slice(&128_000u32.to_be_bytes());
    dcd.extend_from_slice(&128_000u32.to_be_bytes());
    dcd.extend(dsi);
    let dcd = [&[0x04, dcd.len() as u8][..], &dcd].concat();
    let sl = [0x06, 1, 0x02];
    let es = [&[0u8, 1, 0][..], &dcd, &sl].concat();
    [&[0u8, 0, 0, 0, 0x03, es.len() as u8][..], &es].concat()
}

pub fn write_fixture_mp4(path: &Path, opts: FixtureOpts) {
    write_fixture_mp4_rotated(path, opts, Rotation::R0);
}

/// [`write_fixture_mp4`] with a `tkhd` matrix of `rotation`: [`Rotation::R90`] is a portrait
/// phone clip (coded 320x180, displayed 180x320).
pub fn write_fixture_mp4_rotated(path: &Path, opts: FixtureOpts, rotation: Rotation) {
    write(path, opts, rotation, true, b"isom", &[b"isom", b"mp41"]);
}

/// [`write_fixture_mp4`] as a QuickTime file: `ftyp` major brand `qt  `.
pub fn write_fixture_mov(path: &Path, opts: FixtureOpts) {
    write(path, opts, Rotation::R0, true, b"qt  ", &[b"qt  "]);
}

/// An audio-only M4A (`ftyp` brand `M4A `): one AAC-tagged stereo track, `opts.audio` ignored.
pub fn write_m4a(path: &Path, opts: FixtureOpts) {
    let opts = FixtureOpts {
        audio: true,
        ..opts
    };
    write(
        path,
        opts,
        Rotation::R0,
        false,
        b"M4A ",
        &[b"M4A ", b"mp42", b"isom"],
    );
}

fn write(
    path: &Path,
    opts: FixtureOpts,
    rotation: Rotation,
    video: bool,
    brand: &[u8; 4],
    compatible: &[&[u8; 4]],
) {
    let brands = Brands {
        major: *brand,
        compatible: compatible.iter().map(|b| **b).collect(),
    };
    let mux_opts = Mp4Options {
        brands: Some(brands),
        ..Default::default()
    };
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), mux_opts).unwrap();
    // Timescale fps*1000 with 1000-tick samples gives an exact fps rate.
    let v = video.then(|| {
        w.add_video(VideoTrackConfig {
            config: CodecConfig::Avc { avcc: avcc() },
            coded: Size::new(320, 180),
            timescale: opts.fps * 1000,
            rotation,
            pixel_aspect: Rational::from_int(1),
            colour: ColourTags::BT709,
            hdr: None,
        })
        .unwrap()
    });
    let a = opts.audio.then(|| {
        w.add_audio(AudioTrackConfig {
            config: CodecConfig::Aac {
                esds: esds(opts.sample_rate),
            },
            sample_rate: opts.sample_rate,
            channels: 2,
            priming: 0,
            presented: None,
        })
        .unwrap()
    });
    if let Some(v) = v {
        for i in 0..i64::from(opts.seconds * opts.fps) {
            w.write(v, &[0u8; 64], i * 1000, i * 1000, 1000, i == 0)
                .unwrap();
        }
    }
    if let Some(a) = a {
        // One sample per second, `sample_rate` ticks each.
        for i in 0..i64::from(opts.seconds) {
            let t = i * i64::from(opts.sample_rate);
            w.write(a, &[0u8; 32], t, t, opts.sample_rate, true)
                .unwrap();
        }
    }
    put(path, &w.finish().unwrap().into_inner());
}

fn put(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

/// CRC-32 (IEEE), as PNG chunks use it.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A valid `width`×`height` black RGB PNG (its pixels in stored zlib blocks).
pub fn write_png(path: &Path, width: u32, height: u32) {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = width.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, deflate, no filter, no interlace
    png_chunk(&mut out, b"IHDR", &ihdr);
    let row = 1 + 3 * width as usize; // filter byte + pixels
    let raw = vec![0u8; row * height as usize];
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(0xFFFF).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    png_chunk(&mut out, b"IDAT", &z);
    png_chunk(&mut out, b"IEND", &[]);
    put(path, &out);
}

/// A baseline JPEG header (SOI, APP0 JFIF, SOF0 with three components, EOI): enough for a
/// size probe, no scan data.
pub fn write_jpeg(path: &Path, width: u16, height: u16) {
    let mut out = vec![0xFF, 0xD8];
    out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
    out.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&[0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    out.extend_from_slice(&[0xFF, 0xD9]);
    put(path, &out);
}

/// An extended-format WebP (`RIFF`/`WEBP` with a `VP8X` canvas header).
pub fn write_webp(path: &Path, width: u32, height: u32) {
    let mut vp8x = vec![0u8; 4];
    vp8x.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    vp8x.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    let mut body = b"WEBPVP8X".to_vec();
    body.extend_from_slice(&(vp8x.len() as u32).to_le_bytes());
    body.extend_from_slice(&vp8x);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    put(path, &out);
}

/// An SVG with `width`, `height` (px, unitless) and a matching `viewBox`.
pub fn write_svg(path: &Path, width: u32, height: u32) {
    let svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" \
         width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">\n  \
         <rect width=\"{width}\" height=\"{height}\" fill=\"#123456\"/>\n</svg>\n"
    );
    put(path, svg.as_bytes());
}

/// A template animation: 1920×1080 at 30 fps, frames 0–90 (3 s), slots `title` (text),
/// `accent` (color), `logo` (image) and `fade` (number), and fonts `Inter` and `Roboto Mono`.
pub fn sample_lottie() -> serde_json::Value {
    serde_json::json!({
        "v": "5.12.0", "w": 1920, "h": 1080, "fr": 30, "ip": 0, "op": 90,
        "layers": [],
        "fonts": { "list": [
            { "fName": "Inter-Regular", "fFamily": "Inter", "fStyle": "Regular", "ascent": 72 },
            { "fName": "RobotoMono-Bold", "fFamily": "Roboto Mono", "fStyle": "Bold", "ascent": 75 },
        ] },
        "slots": {
            "title": { "p": { "k": [{ "s": { "t": "Hello", "f": "Inter-Regular", "s": 48 }, "t": 0 }] } },
            "accent": { "p": { "a": 0, "k": [1, 0.5, 0, 1] } },
            "logo": { "p": { "w": 256, "h": 256, "u": "/i/", "p": "logo.png", "e": 0 } },
            "fade": { "p": { "a": 0, "k": 100 } },
        },
    })
}

/// A dotLottie (`.lottie`) holding `manifest.json` and `animations/main.json` (`animation`),
/// its members stored or deflated.
pub fn write_dotlottie(path: &Path, animation: &serde_json::Value, deflated: bool) {
    let manifest = serde_json::json!({
        "version": "1.0", "generator": "fin-probe fixtures", "animations": [{ "id": "main" }],
    });
    let members = [
        ("manifest.json", manifest.to_string()),
        ("animations/main.json", animation.to_string()),
    ];
    write_zip(path, &members, deflated);
}

/// A zip of `members`, stored or deflated, with fixed timestamps.
pub fn write_zip(path: &Path, members: &[(&str, String)], deflated: bool) {
    let method = if deflated {
        zip::CompressionMethod::Deflated
    } else {
        zip::CompressionMethod::Stored
    };
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(method)
        .last_modified_time(zip::DateTime::default());
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in members {
        w.start_file(*name, opts).unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    put(path, &w.finish().unwrap().into_inner());
}

/// An identity 3D `.cube` LUT with edge `size`.
pub fn write_cube(path: &Path, size: u32) {
    let mut out = format!("# fin-probe fixture\nTITLE \"identity\"\nLUT_3D_SIZE {size}\n");
    out.push_str("DOMAIN_MIN 0.0 0.0 0.0\nDOMAIN_MAX 1.0 1.0 1.0\n\n");
    let v = |i: u32| f64::from(i) / f64::from(size - 1);
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                out.push_str(&format!("{:.6} {:.6} {:.6}\n", v(r), v(g), v(b)));
            }
        }
    }
    put(path, out.as_bytes());
}

/// A 16-bit PCM WAV of `frames` silent frames, with a `LIST` chunk (odd-sized, so padded)
/// between `fmt ` and `data`.
pub fn write_wav(path: &Path, sample_rate: u32, channels: u16, frames: u32) {
    let block = 2 * channels;
    let mut body = b"WAVE".to_vec();
    let mut chunk = |id: &[u8; 4], data: &[u8]| {
        body.extend_from_slice(id);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        if data.len() % 2 == 1 {
            body.push(0);
        }
    };
    let mut fmt = 1u16.to_le_bytes().to_vec(); // PCM
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&sample_rate.to_le_bytes());
    fmt.extend_from_slice(&(sample_rate * u32::from(block)).to_le_bytes());
    fmt.extend_from_slice(&block.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    chunk(b"fmt ", &fmt);
    chunk(b"LIST", b"INFOISFT\x03\0\0\0fin");
    chunk(b"data", &vec![0u8; frames as usize * usize::from(block)]);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    put(path, &out);
}
