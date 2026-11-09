//! Clusters and their blocks: `SimpleBlock` and `BlockGroup` (`Block`, `ReferenceBlock`,
//! `BlockDuration`, `DiscardPadding`), keyframe flags, and lacing (none for video; Xiph, EBML
//! and fixed for audio). Each frame of the two tracks read becomes a byte range in the file;
//! frames of other tracks are skipped after their track number. Blocks count as samples
//! (`max_samples` per track), not as elements.

use std::io::{Read, Seek};

use fin_core::CoreError;

use super::ebml::*;
use crate::error::{corrupt, limit, unsupported_container};

/// One frame as stored: its block's timestamp in Segment ticks, its place in the file, and
/// its place in a laced block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RawFrame {
    pub ts: i64,
    pub offset: u64,
    pub size: u32,
    pub key: bool,
    /// `BlockDuration` in ticks (a `BlockGroup`'s).
    pub duration: Option<u64>,
    /// Index of the frame in its block, and the block's frame count.
    pub lace: (u8, u8),
    /// The cluster it came from (index in the layout's list).
    pub cluster: u32,
    /// `DiscardPadding` in nanoseconds.
    pub padding: Option<i64>,
}

/// Where an unknown-sized cluster stops: at the next Segment-level id, or at its parent's end.
pub(super) fn unknown_end<R: Read + Seek>(
    eb: &mut Ebml<R>,
    cluster: &Element,
) -> Result<u64, CoreError> {
    let mut at = cluster.body;
    while cluster.end - at >= 2 {
        let e = eb.header(at, cluster.end, 2, false)?;
        if ends_cluster(e.id) {
            return Ok(at);
        }
        at = e.end;
    }
    Ok(cluster.end)
}

/// The two tracks a scan keeps.
pub(super) struct Want {
    pub video: Option<u64>,
    pub audio: Option<u64>,
    /// `format` for `UNSUPPORTED_CONTAINER`.
    pub format: &'static str,
}

#[derive(Default)]
pub(super) struct Scanned {
    pub video: Vec<RawFrame>,
    pub audio: Vec<RawFrame>,
}

/// What a `BlockGroup` says about its `Block`.
#[derive(Clone, Copy)]
struct Group {
    key: bool,
    duration: Option<u64>,
    padding: Option<i64>,
}

/// Every block of the wanted tracks, cluster by cluster.
pub(super) fn scan<R: Read + Seek>(
    eb: &mut Ebml<R>,
    clusters: &[Element],
    want: &Want,
) -> Result<Scanned, CoreError> {
    let mut out = Scanned::default();
    for (index, c) in clusters.iter().enumerate() {
        let mut ts: Option<u64> = None;
        let mut at = c.body;
        while c.end - at >= 2 {
            let e = eb.header(at, c.end, 2, false)?;
            at = e.end;
            match e.id {
                TIMESTAMP => ts = Some(eb.uint(&e)?),
                SIMPLE_BLOCK | BLOCK_GROUP => {
                    let cluster_ts = ts.ok_or_else(|| {
                        corrupt(
                            "a block comes before its cluster's Timestamp",
                            e.start,
                            e.name(),
                        )
                    })?;
                    let (block, group) = match e.id {
                        SIMPLE_BLOCK => (e, None),
                        _ => match group(eb, &e)? {
                            Some((b, g)) => (b, Some(g)),
                            None => continue,
                        },
                    };
                    let ctx = Ctx {
                        cluster_ts,
                        cluster: index as u32,
                        group,
                    };
                    read_block(eb, &block, &ctx, want, &mut out)?;
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

/// A `BlockGroup`'s `Block` and what its siblings say (`None` without a `Block`).
fn group<R: Read + Seek>(
    eb: &mut Ebml<R>,
    g: &Element,
) -> Result<Option<(Element, Group)>, CoreError> {
    let mut block = None;
    let mut info = Group {
        key: true,
        duration: None,
        padding: None,
    };
    let mut at = g.body;
    while g.end - at >= 2 {
        let e = eb.header(at, g.end, 3, false)?;
        at = e.end;
        match e.id {
            BLOCK if block.is_none() => block = Some(e),
            REFERENCE_BLOCK => info.key = false,
            BLOCK_DURATION => info.duration = Some(eb.uint(&e)?),
            DISCARD_PADDING => {
                let raw = eb.uint(&e)?;
                // Sign-extend from the element's width.
                let bits = (e.len() * 8) as u32;
                info.padding = Some(match bits {
                    0 => 0,
                    64 => raw as i64,
                    b => ((raw << (64 - b)) as i64) >> (64 - b),
                });
            }
            _ => {}
        }
    }
    Ok(block.map(|b| (b, info)))
}

struct Ctx {
    cluster_ts: u64,
    cluster: u32,
    group: Option<Group>,
}

fn push(list: &mut Vec<RawFrame>, f: RawFrame, max: u64) -> Result<(), CoreError> {
    if list.len() as u64 >= max {
        return Err(limit("maxSamples", list.len() as u64 + 1, max));
    }
    list.push(f);
    Ok(())
}

fn read_block<R: Read + Seek>(
    eb: &mut Ebml<R>,
    b: &Element,
    ctx: &Ctx,
    want: &Want,
    out: &mut Scanned,
) -> Result<(), CoreError> {
    let bad = |what: &str| corrupt(format!("a block {what}"), b.start, name(b.id));
    let mut head = vec![0u8; b.len().min(11) as usize];
    eb.read_at(b.body, &mut head)?;
    let (track, tlen) = vint(&head, 8).ok_or_else(|| bad("has a malformed track number"))?;
    let video = want.video == Some(track);
    if !video && want.audio != Some(track) {
        return Ok(());
    }
    let fixed = head
        .get(tlen..tlen + 3)
        .ok_or_else(|| bad("header is cut short"))?;
    let rel = i16::from_be_bytes([fixed[0], fixed[1]]) as i64;
    let flags = fixed[2];
    let ts = i64::try_from(ctx.cluster_ts)
        .ok()
        .and_then(|t| t.checked_add(rel))
        .ok_or_else(|| bad("timestamp overflows"))?;
    let key = match ctx.group {
        None => flags & 0x80 != 0,
        Some(g) => g.key,
    };
    let data = b.body + tlen as u64 + 3;
    let lacing = (flags >> 1) & 3;
    if video && lacing != 0 {
        return Err(unsupported_container(want.format, "laced video frames"));
    }
    let sizes = if lacing == 0 {
        vec![b.end - data]
    } else {
        laced(eb, b, data, lacing)?
    };
    let mut offset = match lacing {
        0 => data,
        _ => b.end - sizes.iter().sum::<u64>(),
    };
    let count = sizes.len() as u8;
    if lacing != 0 && sizes.contains(&0) {
        return Err(bad("laces an empty frame"));
    }
    // As MP4: a track's samples are also at most the file's length in bytes.
    let max = (eb.limits.max_samples as u64).min(eb.len);
    for (i, &size) in sizes.iter().enumerate() {
        if video && size == 0 {
            return Err(bad("holds an empty video frame"));
        }
        let size = u32::try_from(size).map_err(|_| bad("frame is over 4 GiB"))?;
        let f = RawFrame {
            ts,
            offset,
            size,
            key: key || !video,
            duration: ctx.group.and_then(|g| g.duration),
            lace: (i as u8, count),
            cluster: ctx.cluster,
            padding: ctx.group.and_then(|g| g.padding),
        };
        offset += size as u64;
        match video {
            true => push(&mut out.video, f, max)?,
            false => push(&mut out.audio, f, max)?,
        }
    }
    Ok(())
}

/// The frame sizes of a laced block whose lacing header starts at `at`: `lacing` 1 is Xiph,
/// 2 fixed, 3 EBML. Every size is checked against the bytes the block has left.
fn laced<R: Read + Seek>(
    eb: &mut Ebml<R>,
    b: &Element,
    mut at: u64,
    lacing: u8,
) -> Result<Vec<u64>, CoreError> {
    let over = || {
        corrupt(
            "a block's lacing declares more bytes than the block has",
            b.start,
            name(b.id),
        )
    };
    let mut byte = [0u8; 1];
    if at >= b.end {
        return Err(over());
    }
    eb.read_at(at, &mut byte)?;
    at += 1;
    let n = byte[0] as usize + 1;
    let mut sizes: Vec<u64> = Vec::with_capacity(n);
    match lacing {
        // Fixed: the rest divided evenly.
        2 => {
            let rest = b.end - at;
            if !rest.is_multiple_of(n as u64) {
                return Err(corrupt(
                    "a fixed-laced block does not divide into equal frames",
                    b.start,
                    name(b.id),
                ));
            }
            return Ok(vec![rest / n as u64; n]);
        }
        1 => {
            for _ in 0..n - 1 {
                let mut size = 0u64;
                loop {
                    if at >= b.end {
                        return Err(over());
                    }
                    eb.read_at(at, &mut byte)?;
                    at += 1;
                    size += byte[0] as u64;
                    if byte[0] != 255 {
                        break;
                    }
                }
                sizes.push(size);
            }
        }
        _ => {
            for i in 0..n - 1 {
                let mut raw = vec![0u8; (b.end - at).min(8) as usize];
                eb.read_at(at, &mut raw)?;
                let size = if i == 0 {
                    let (v, len) = vint(&raw, 8).ok_or_else(over)?;
                    at += len as u64;
                    v as i64
                } else {
                    let (d, len) = svint(&raw).ok_or_else(over)?;
                    at += len as u64;
                    (*sizes.last().expect("one size") as i64)
                        .checked_add(d)
                        .ok_or_else(over)?
                };
                sizes.push(u64::try_from(size).map_err(|_| over())?);
            }
        }
    }
    let declared = sizes
        .iter()
        .try_fold(0u64, |s, &v| s.checked_add(v))
        .ok_or_else(over)?;
    let rest = b.end - at;
    if declared > rest {
        return Err(over());
    }
    sizes.push(rest - declared);
    Ok(sizes)
}
