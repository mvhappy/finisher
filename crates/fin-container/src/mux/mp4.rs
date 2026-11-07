//! The `moov`: sample tables, chunk offsets and edit lists computed here, leaf boxes encoded by
//! `mp4-atom`, the containers nested by [`boxed`].

use fin_core::CoreError;
use fin_media::Rotation;
use mp4_atom::{
    Co64, Ctts, CttsEntry, Dinf, Dref, DrefEntry, Edts, Elst, ElstEntry, FixedPoint, FourCC, Free,
    Ftyp, Hdlr, Matrix, Mdhd, Mvex, Mvhd, Smhd, Stco, Stsc, StscEntry, Stss, Stsz, StszSamples,
    Stts, SttsEntry, Tkhd, Trex, Url, Vmhd, Zeroed,
};
use serde_json::json;

use super::entries::{audio_entry, boxed, enc, tkhd_size, video_entry};
use super::{Track, TrackKind};
use crate::error::invalid;

pub(crate) fn ftyp(tracks: &[Track], brands: Option<&super::Brands>) -> Result<Vec<u8>, CoreError> {
    let (major, compatible) = match brands {
        Some(b) => (
            FourCC::new(&b.major),
            b.compatible.iter().map(FourCC::new).collect(),
        ),
        None => {
            let mut c: Vec<FourCC> = [b"isom", b"iso6", b"mp41"]
                .iter()
                .map(|b| FourCC::new(b))
                .collect();
            if tracks
                .iter()
                .any(|t| matches!(&t.kind, TrackKind::Video(v) if v.config.fourcc() == *b"av01"))
            {
                c.push(FourCC::new(b"av01"));
            }
            (FourCC::new(b"isom"), c)
        }
    };
    enc(
        &Ftyp {
            major_brand: major,
            minor_version: 0x200,
            compatible_brands: compatible,
        },
        "ftyp",
    )
}

/// A `free` box of `size` bytes in all (at least 8).
pub(crate) fn free(size: u64) -> Result<Vec<u8>, CoreError> {
    let body = usize::try_from(size.saturating_sub(8))
        .map_err(|_| invalid("reserve too large", json!({})))?;
    enc(
        &Free {
            zeroed: Zeroed::new(body),
        },
        "free",
    )
}

/// A 64-bit `mdat` header for `size` bytes in all.
pub(crate) fn mdat_header(size: u64) -> [u8; 16] {
    let mut h = [0u8; 16];
    h[..4].copy_from_slice(&1u32.to_be_bytes());
    h[4..8].copy_from_slice(b"mdat");
    h[8..].copy_from_slice(&size.to_be_bytes());
    h
}

const ONE: i32 = 0x0001_0000;

fn matrix(m: [i32; 9]) -> Matrix {
    let [a, b, u, c, d, v, x, y, w] = m;
    Matrix {
        a,
        b,
        u,
        c,
        d,
        v,
        x,
        y,
        w,
    }
}

/// The `tkhd` matrix for a clockwise rotation of a `width × height` picture (the translation
/// keeps it in the positive quadrant, as QuickTime and ffmpeg write it).
fn rotation_matrix(r: Rotation, width: u32, height: u32) -> [i32; 9] {
    let fx = |n: u32| i32::try_from((n as i64) << 16).unwrap_or(0);
    let w = 0x4000_0000;
    match r {
        Rotation::R0 => [ONE, 0, 0, 0, ONE, 0, 0, 0, w],
        Rotation::R90 => [0, ONE, 0, -ONE, 0, 0, fx(height), 0, w],
        Rotation::R180 => [-ONE, 0, 0, 0, -ONE, 0, fx(width), fx(height), w],
        Rotation::R270 => [0, -ONE, 0, ONE, 0, 0, 0, fx(width), w],
    }
}

/// `ticks` of `from` per second in `to` per second, halves up.
fn rescale(ticks: u64, from: u32, to: u32) -> u64 {
    ((ticks as u128 * to as u128 + from as u128 / 2) / from as u128) as u64
}

/// The movie timescale: the first video track's, else the first track's.
fn movie_timescale(tracks: &[Track]) -> u32 {
    tracks
        .iter()
        .find(|t| matches!(t.kind, TrackKind::Video(_)))
        .or(tracks.first())
        .map_or(1000, |t| t.timescale)
}

/// Composition times relative to the first decode time: the earliest and the end.
fn presentation(t: &Track) -> (i64, i64) {
    let dts0 = t.samples.first().map_or(0, |s| s.dts);
    let start = t.samples.iter().map(|s| s.pts - dts0).min().unwrap_or(0);
    let end = t
        .samples
        .iter()
        .map(|s| s.pts - dts0 + s.duration as i64)
        .max()
        .unwrap_or(0);
    (start, end)
}

/// The track's edit list: an optional empty edit (test knob), then the media from its first
/// presented sample (after the audio priming) to its end.
fn edits(t: &Track, movie_ts: u32, fragmented: bool) -> Result<Vec<ElstEntry>, CoreError> {
    if fragmented {
        return Ok(Vec::new());
    }
    let rate = FixedPoint::new(1, 0);
    #[cfg(feature = "testing")]
    if let Some(e) = &t.knobs.edits {
        return Ok(e
            .iter()
            .map(|&(d, m)| ElstEntry {
                segment_duration: d,
                media_time: m,
                media_rate: rate,
            })
            .collect());
    }
    let mut out = Vec::new();
    #[cfg(feature = "testing")]
    if let Some(e) = t.knobs.empty_edit {
        out.push(ElstEntry {
            segment_duration: e,
            media_time: None,
            media_rate: rate,
        });
    }
    let (start, end) = presentation(t);
    let priming = match &t.kind {
        TrackKind::Audio(a) => a.priming as i64,
        TrackKind::Video(_) => 0,
    };
    let media_time = start + priming;
    if media_time < 0 {
        return Err(invalid(
            "a sample is presented before the track's first decode time",
            json!({ "start": start }),
        ));
    }
    if media_time != 0 || !out.is_empty() {
        out.push(ElstEntry {
            segment_duration: rescale((end - media_time).max(0) as u64, t.timescale, movie_ts),
            media_time: Some(media_time as u64),
            media_rate: rate,
        });
    }
    Ok(out)
}

fn rle<T: PartialEq + Copy>(values: impl Iterator<Item = T>) -> Vec<(u32, T)> {
    let mut out: Vec<(u32, T)> = Vec::new();
    for v in values {
        match out.last_mut() {
            Some((n, last)) if *last == v => *n += 1,
            _ => out.push((1, v)),
        }
    }
    out
}

/// The `stbl` of `t` with chunk offsets moved by `shift`.
fn stbl(t: &Track, shift: u64, entry: Vec<u8>) -> Result<Vec<u8>, CoreError> {
    let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stsd.extend(entry);
    let mut body = boxed(b"stsd", &stsd);
    let s = &t.samples;
    let stts = Stts {
        entries: rle(s.iter().map(|s| s.duration))
            .into_iter()
            .map(|(n, d)| SttsEntry {
                sample_count: n,
                sample_delta: d,
            })
            .collect(),
    };
    body.extend(enc(&stts, "stts")?);
    if s.iter().any(|s| s.pts != s.dts) {
        let ctts = Ctts {
            entries: rle(s.iter().map(|s| s.pts - s.dts))
                .into_iter()
                .map(|(n, o)| CttsEntry {
                    sample_count: n,
                    sample_offset: o,
                })
                .collect(),
        };
        body.extend(enc(&ctts, "ctts")?);
    }
    if s.iter().any(|s| !s.key) {
        let stss = Stss {
            entries: (1..=s.len() as u32)
                .filter(|&i| s[i as usize - 1].key)
                .collect(),
        };
        body.extend(enc(&stss, "stss")?);
    }
    let mut stsc = Vec::new();
    for (i, c) in t.chunks.iter().enumerate() {
        if stsc
            .last()
            .is_none_or(|e: &StscEntry| e.samples_per_chunk != c.count)
        {
            stsc.push(StscEntry {
                first_chunk: i as u32 + 1,
                samples_per_chunk: c.count,
                sample_description_index: 1,
            });
        }
    }
    body.extend(enc(&Stsc { entries: stsc }, "stsc")?);
    let samples = match s.first() {
        Some(first) if s.iter().all(|x| x.size == first.size) && first.size > 0 => {
            StszSamples::Identical {
                count: s.len() as u32,
                size: first.size,
            }
        }
        _ => StszSamples::Different {
            sizes: s.iter().map(|s| s.size).collect(),
        },
    };
    body.extend(enc(&Stsz { samples }, "stsz")?);
    let offsets: Vec<u64> = t.chunks.iter().map(|c| c.offset + shift).collect();
    if offsets.iter().any(|&o| o > u32::MAX as u64) {
        body.extend(enc(&Co64 { entries: offsets }, "co64")?);
    } else {
        body.extend(enc(
            &Stco {
                entries: offsets.iter().map(|&o| o as u32).collect(),
            },
            "stco",
        )?);
    }
    Ok(boxed(b"stbl", &body))
}

fn trak(
    t: &Track,
    id: u32,
    movie_ts: u32,
    shift: u64,
    fragmented: bool,
) -> Result<(Vec<u8>, u64), CoreError> {
    let media_duration: u64 = if fragmented {
        0
    } else {
        t.samples.iter().map(|s| s.duration as u64).sum()
    };
    let edits = edits(t, movie_ts, fragmented)?;
    let duration = match edits.is_empty() {
        true => rescale(presentation(t).1.max(0) as u64, t.timescale, movie_ts),
        false => edits.iter().map(|e| e.segment_duration).sum(),
    };
    let (entry, handler, name, width, height, m) = match &t.kind {
        TrackKind::Video(v) => {
            #[cfg(feature = "testing")]
            let entry = video_entry(v, &t.knobs)?;
            #[cfg(not(feature = "testing"))]
            let entry = video_entry(v)?;
            let (w, h) = tkhd_size(v);
            #[allow(unused_mut)]
            let mut m = rotation_matrix(v.rotation, w, h);
            #[cfg(feature = "testing")]
            if let Some(k) = t.knobs.matrix {
                m = k;
            }
            (entry, b"vide", "VideoHandler", w, h, m)
        }
        TrackKind::Audio(a) => (
            audio_entry(a)?,
            b"soun",
            "SoundHandler",
            0,
            0,
            rotation_matrix(Rotation::R0, 0, 0),
        ),
    };
    let video = handler == b"vide";
    let tkhd = Tkhd {
        track_id: id,
        duration,
        enabled: true,
        in_movie: true,
        volume: FixedPoint::new(if video { 0 } else { 1 }, 0),
        matrix: matrix(m),
        width: FixedPoint::new(width.min(u16::MAX as u32) as u16, 0),
        height: FixedPoint::new(height.min(u16::MAX as u32) as u16, 0),
        ..Default::default()
    };
    let mut body = enc(&tkhd, "tkhd")?;
    if !edits.is_empty() {
        body.extend(enc(
            &Edts {
                elst: Some(Elst { entries: edits }),
            },
            "edts",
        )?);
    }
    let mdhd = Mdhd {
        timescale: t.timescale,
        duration: media_duration,
        language: "und".into(),
        ..Default::default()
    };
    let hdlr = Hdlr {
        handler: FourCC::new(handler),
        name: name.into(),
    };
    let mut minf = match video {
        true => enc(&Vmhd::default(), "vmhd")?,
        false => enc(&Smhd::default(), "smhd")?,
    };
    let dinf = Dinf {
        dref: Dref {
            entries: vec![DrefEntry::Url(Url::default())],
        },
    };
    minf.extend(enc(&dinf, "dinf")?);
    minf.extend(match fragmented {
        true => stbl(
            &Track {
                samples: Vec::new(),
                chunks: Vec::new(),
                ..t.clone()
            },
            shift,
            entry,
        )?,
        false => stbl(t, shift, entry)?,
    });
    let mut mdia = enc(&mdhd, "mdhd")?;
    mdia.extend(enc(&hdlr, "hdlr")?);
    mdia.extend(boxed(b"minf", &minf));
    body.extend(boxed(b"mdia", &mdia));
    Ok((boxed(b"trak", &body), duration))
}

/// The `moov` with chunk offsets moved by `shift`; `fragmented` writes empty tables and `mvex`.
pub(crate) fn moov(tracks: &[Track], shift: u64, fragmented: bool) -> Result<Vec<u8>, CoreError> {
    let movie_ts = movie_timescale(tracks);
    let mut traks = Vec::new();
    let mut duration = 0;
    for (i, t) in tracks.iter().enumerate() {
        let (bytes, d) = trak(t, i as u32 + 1, movie_ts, shift, fragmented)?;
        traks.extend(bytes);
        duration = duration.max(d);
    }
    let mvhd = Mvhd {
        timescale: movie_ts,
        duration,
        rate: FixedPoint::new(1, 0),
        volume: FixedPoint::new(1, 0),
        matrix: matrix(rotation_matrix(Rotation::R0, 0, 0)),
        next_track_id: tracks.len() as u32 + 1,
        ..Default::default()
    };
    let mut body = enc(&mvhd, "mvhd")?;
    body.extend(traks);
    if fragmented {
        let mvex = Mvex {
            mehd: None,
            trex: (1..=tracks.len() as u32)
                .map(|id| Trex {
                    track_id: id,
                    default_sample_description_index: 1,
                    ..Default::default()
                })
                .collect(),
        };
        body.extend(enc(&mvex, "mvex")?);
    }
    Ok(boxed(b"moov", &body))
}

/// One `moof` and its `mdat` holding samples `pending[k].0 .. pending[k].1` of each track.
#[cfg(feature = "testing")]
pub(crate) fn fragment(
    tracks: &[Track],
    pending: &[(usize, usize)],
    seq: u32,
) -> Result<(Vec<u8>, Vec<u8>), CoreError> {
    use mp4_atom::{Mfhd, Moof, Tfdt, Tfhd, Traf, Trun, TrunEntry, TrunSamples};
    let build = |offsets: &[i32]| -> Result<Vec<u8>, CoreError> {
        let mut traf = Vec::new();
        for (k, t) in tracks.iter().enumerate() {
            let (from, to) = pending[k];
            if from == to {
                continue;
            }
            let dts0 = t.samples[0].dts;
            let samples = &t.samples[from..to];
            let cts = samples.iter().any(|s| s.pts != s.dts);
            let entries = samples
                .iter()
                .map(|s| TrunEntry {
                    duration: Some(s.duration),
                    size: Some(s.size),
                    flags: Some(if s.key { 0x0200_0000 } else { 0x0101_0000 }),
                    cts: cts.then_some(s.pts - s.dts),
                })
                .collect();
            traf.push(Traf {
                tfhd: Tfhd {
                    track_id: k as u32 + 1,
                    default_base_is_moof: true,
                    ..Default::default()
                },
                tfdt: Some(Tfdt {
                    base_media_decode_time: (samples[0].dts - dts0) as u64,
                }),
                trun: vec![Trun {
                    data_offset: Some(offsets[k]),
                    samples: TrunSamples::Explicit(entries),
                }],
                ..Default::default()
            });
        }
        enc(
            &Moof {
                mfhd: Mfhd {
                    sequence_number: seq,
                },
                traf,
            },
            "moof",
        )
    };
    let zero = vec![0i32; tracks.len()];
    let size = build(&zero)?.len();
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    for t in tracks {
        offsets.push((size + 8 + data.len()) as i32);
        data.extend_from_slice(&t.pending);
    }
    Ok((build(&offsets)?, boxed(b"mdat", &data)))
}
