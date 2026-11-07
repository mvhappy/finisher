//! Sample tables (`stts`, `ctts` v0/v1, `stss`, `stsc`, `stsz`/`stz2`, `stco`/`co64`) expanded
//! into one list of samples in decode order. Counts are checked against box sizes before
//! `mp4-atom` decodes a box, and the total against `max_samples` before anything is expanded.

use std::io::{Read, Seek};

use fin_core::CoreError;
use mp4_atom::{Co64, Ctts, Stco, Stsc, Stss, Stsz, StszSamples, Stts};

use super::reader::{BoxHeader, Reader, check_count, decode, full_box_count};
use crate::error::{corrupt, limit};

/// One sample in decode order: decode time and composition offset in media ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sample {
    pub dts: i64,
    pub cto: i64,
    pub duration: u32,
    pub size: u32,
    pub offset: u64,
    pub key: bool,
}

impl Sample {
    /// A sample whose decode end, composition time and composition end all fit `i64`
    /// (`None` otherwise), so later arithmetic on them cannot overflow.
    pub fn checked(
        dts: i64,
        cto: i64,
        duration: u32,
        size: u32,
        offset: u64,
        key: bool,
    ) -> Option<Sample> {
        let d = i64::from(duration);
        dts.checked_add(d)?;
        dts.checked_add(cto)?.checked_add(d)?;
        Some(Sample {
            dts,
            cto,
            duration,
            size,
            offset,
            key,
        })
    }

    /// Composition time (fits: see [`Sample::checked`]).
    pub fn ct(&self) -> i64 {
        self.dts + self.cto
    }

    /// Composition end.
    pub fn ce(&self) -> i64 {
        self.ct() + i64::from(self.duration)
    }

    /// Decode end.
    pub fn end(&self) -> i64 {
        self.dts + i64::from(self.duration)
    }
}

/// The sample-table boxes of one `stbl`.
#[derive(Debug, Default)]
pub(crate) struct Boxes {
    pub stsd: Option<BoxHeader>,
    pub stts: Option<BoxHeader>,
    pub ctts: Option<BoxHeader>,
    pub stss: Option<BoxHeader>,
    pub stsc: Option<BoxHeader>,
    pub stsz: Option<BoxHeader>,
    pub stz2: Option<BoxHeader>,
    pub stco: Option<BoxHeader>,
    pub co64: Option<BoxHeader>,
    pub sgpd: Option<BoxHeader>,
}

impl Boxes {
    pub fn set(&mut self, h: BoxHeader) {
        let slot = match &h.kind {
            b"stsd" => &mut self.stsd,
            b"stts" => &mut self.stts,
            b"ctts" => &mut self.ctts,
            b"stss" => &mut self.stss,
            b"stsc" => &mut self.stsc,
            b"stsz" => &mut self.stsz,
            b"stz2" => &mut self.stz2,
            b"stco" => &mut self.stco,
            b"co64" => &mut self.co64,
            b"sgpd" => &mut self.sgpd,
            _ => return,
        };
        slot.get_or_insert(h);
    }
}

fn sizes<R: Read + Seek>(rd: &mut Reader<R>, b: &Boxes, at: u64) -> Result<Vec<u32>, CoreError> {
    let max = rd.limits.max_samples as u64;
    if let Some(h) = &b.stsz {
        let body = rd.body(h)?;
        let count = super::reader::be_u32(&body, 8)
            .ok_or_else(|| corrupt("the stsz box is cut short", h.start, "stsz"))?
            as u64;
        if count > max {
            return Err(limit("maxSamples", count, max));
        }
        // Each sample is at least a byte of the file (zero-size samples are not media).
        if count > rd.len {
            return Err(corrupt(
                format!("stsz declares {count} samples in a {}-byte file", rd.len),
                h.start,
                "stsz",
            ));
        }
        let uniform = super::reader::be_u32(&body, 4).unwrap_or(0);
        if uniform == 0 {
            check_count(&body, 12, count, 4, h)?;
        }
        return Ok(match decode::<Stsz>(&body, h)?.samples {
            StszSamples::Identical { count, size } => vec![size; count as usize],
            StszSamples::Different { sizes } => sizes,
        });
    }
    if let Some(h) = &b.stz2 {
        // mp4-atom has no stz2: version/flags, 3 reserved bytes, field size, count, fields.
        let body = rd.body(h)?;
        let field = *body
            .get(7)
            .ok_or_else(|| corrupt("the stz2 box is cut short", h.start, "stz2"))?
            as u64;
        let count = full_box_count(&body[4..], h)? as u64;
        if count > max {
            return Err(limit("maxSamples", count, max));
        }
        if !matches!(field, 4 | 8 | 16) {
            return Err(corrupt(
                format!("stz2 field size {field} is not 4, 8 or 16"),
                h.start,
                "stz2",
            ));
        }
        if count.saturating_mul(field).div_ceil(8) > (body.len() as u64).saturating_sub(12) {
            return Err(corrupt(
                "the stz2 box declares more entries than it holds",
                h.start,
                "stz2",
            ));
        }
        let d = &body[12..];
        return Ok((0..count as usize)
            .map(|i| match field {
                4 => ((d[i / 2] >> if i % 2 == 0 { 4 } else { 0 }) & 0x0f) as u32,
                8 => d[i] as u32,
                _ => u16::from_be_bytes([d[2 * i], d[2 * i + 1]]) as u32,
            })
            .collect());
    }
    Err(corrupt("the sample table has no stsz or stz2", at, "stbl"))
}

/// Run lengths `(count, value)` expanded to exactly `n` values; `MEDIA_CORRUPT` when they
/// cover fewer (or, with `exact`, other than `n`) samples.
fn runs<T: Copy>(
    entries: impl Iterator<Item = (u32, T)>,
    n: usize,
    fill: Option<T>,
    h: &BoxHeader,
) -> Result<Vec<T>, CoreError> {
    let mut out = Vec::with_capacity(n);
    for (count, v) in entries {
        let take = (count as usize).min(n - out.len());
        out.extend(std::iter::repeat_n(v, take));
        if out.len() == n {
            break;
        }
    }
    match fill {
        Some(f) => out.resize(n, f),
        None if out.len() < n => {
            return Err(corrupt(
                format!("the {} box covers {} of {n} samples", h.name(), out.len()),
                h.start,
                &h.name(),
            ));
        }
        None => {}
    }
    Ok(out)
}

/// The samples of one track's `stbl`, in decode order.
pub(crate) fn expand<R: Read + Seek>(
    rd: &mut Reader<R>,
    b: &Boxes,
    stbl: &BoxHeader,
) -> Result<Vec<Sample>, CoreError> {
    let sizes = sizes(rd, b, stbl.start)?;
    let n = sizes.len();
    let stts_h = b
        .stts
        .ok_or_else(|| corrupt("the sample table has no stts", stbl.start, "stbl"))?;
    let body = rd.body(&stts_h)?;
    check_count(&body, 8, full_box_count(&body, &stts_h)? as u64, 8, &stts_h)?;
    let stts = decode::<Stts>(&body, &stts_h)?;
    let durations = runs(
        stts.entries
            .iter()
            .map(|e| (e.sample_count, e.sample_delta)),
        n,
        None,
        &stts_h,
    )?;
    let offsets_cto = match &b.ctts {
        Some(h) => {
            let body = rd.body(h)?;
            check_count(&body, 8, full_box_count(&body, h)? as u64, 8, h)?;
            let version = body[0];
            let ctts = decode::<Ctts>(&body, h)?;
            // Version 0 is unsigned on paper; writers put negative offsets in it as two's
            // complement, so values past i32::MAX are read as negative.
            let fix = |o: i64| {
                if version == 0 && o > i32::MAX as i64 {
                    o - (1i64 << 32)
                } else {
                    o
                }
            };
            runs(
                ctts.entries
                    .iter()
                    .map(|e| (e.sample_count, fix(e.sample_offset))),
                n,
                Some(0),
                h,
            )?
        }
        None => vec![0; n],
    };
    let mut key = vec![b.stss.is_none(); n];
    if let Some(h) = &b.stss {
        let body = rd.body(h)?;
        check_count(&body, 8, full_box_count(&body, h)? as u64, 4, h)?;
        for s in decode::<Stss>(&body, h)?.entries {
            match key.get_mut((s as usize).wrapping_sub(1)) {
                Some(k) => *k = true,
                None => {
                    return Err(corrupt(
                        format!("stss names sample {s} of {n}"),
                        h.start,
                        "stss",
                    ));
                }
            }
        }
    }
    let chunks: Vec<u64> = match (&b.stco, &b.co64) {
        (Some(h), _) => {
            let body = rd.body(h)?;
            check_count(&body, 8, full_box_count(&body, h)? as u64, 4, h)?;
            decode::<Stco>(&body, h)?
                .entries
                .into_iter()
                .map(u64::from)
                .collect()
        }
        (None, Some(h)) => {
            let body = rd.body(h)?;
            check_count(&body, 8, full_box_count(&body, h)? as u64, 8, h)?;
            decode::<Co64>(&body, h)?.entries
        }
        (None, None) if n == 0 => Vec::new(),
        (None, None) => {
            return Err(corrupt(
                "the sample table has no stco or co64",
                stbl.start,
                "stbl",
            ));
        }
    };
    let stsc = match &b.stsc {
        Some(h) => {
            let body = rd.body(h)?;
            check_count(&body, 8, full_box_count(&body, h)? as u64, 12, h)?;
            (decode::<Stsc>(&body, h)?, *h)
        }
        None if n == 0 => (Stsc::default(), *stbl),
        None => return Err(corrupt("the sample table has no stsc", stbl.start, "stbl")),
    };
    let offsets = sample_offsets(&stsc.0, &stsc.1, &chunks, &sizes, rd.len)?;
    let mut dts = 0i64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let s = Sample::checked(
            dts,
            offsets_cto[i],
            durations[i],
            sizes[i],
            offsets[i],
            key[i],
        )
        .ok_or_else(|| corrupt("sample times overflow", stbl.start, "stts"))?;
        dts = s.end();
        out.push(s);
    }
    Ok(out)
}

/// Each sample's file offset from the chunk map; `MEDIA_CORRUPT` for a sample past the end of
/// the file or chunks that hold fewer samples than the table.
fn sample_offsets(
    stsc: &Stsc,
    h: &BoxHeader,
    chunks: &[u64],
    sizes: &[u32],
    file_len: u64,
) -> Result<Vec<u64>, CoreError> {
    let n = sizes.len();
    let mut out = Vec::with_capacity(n);
    let e = &stsc.entries;
    if n > 0 && e.first().is_none_or(|f| f.first_chunk != 1) {
        return Err(corrupt("stsc does not start at chunk 1", h.start, "stsc"));
    }
    let mut k = 0;
    for (c, &chunk) in chunks.iter().enumerate() {
        if out.len() == n {
            break;
        }
        while k + 1 < e.len() && e[k + 1].first_chunk as usize <= c + 1 {
            if e[k + 1].first_chunk <= e[k].first_chunk {
                return Err(corrupt(
                    "stsc chunk numbers do not increase",
                    h.start,
                    "stsc",
                ));
            }
            k += 1;
        }
        let mut at = chunk;
        for _ in 0..e[k].samples_per_chunk {
            if out.len() == n {
                break;
            }
            let size = sizes[out.len()] as u64;
            if at.saturating_add(size) > file_len {
                return Err(corrupt(
                    format!("sample {} at {at} runs past the end of the file", out.len()),
                    at,
                    "co64",
                ));
            }
            out.push(at);
            at += size;
        }
    }
    if out.len() < n {
        return Err(corrupt(
            format!("the chunks hold {} of {n} samples", out.len()),
            h.start,
            "stsc",
        ));
    }
    Ok(out)
}
