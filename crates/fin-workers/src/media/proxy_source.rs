//! A proxy cache entry as a look source (plan 6B-1 Task 16; spec §7.1, §10.2): the entry's
//! `proxy.mp4` opened as a [`FrameReader`], the original's sample table from `table.bin`, and
//! the original's rotation and pixel aspect from `meta.json`. A layer maps its source time to
//! an original frame index with `frame_at` on the table, then reads proxy frame `i` (plan
//! Decision 7: one proxy frame per original frame).
//!
//! The entry is the host's, read-only to the worker, but it is still opened under the rules of
//! spec §3.4 (`fin_shm::checked_open`: no link at any segment, one regular file, capped by the
//! size `meta.json` lists) and checked against the manifest before it is believed: its key
//! recomputes, it is the manifest's asset's fingerprint and tier, the files are the listed
//! sizes, `table.bin` is its listed digest and decodes, and the proxy holds exactly the
//! original's presented frames. Anything else is a [`Refusal`], which the layer answers by
//! reading the original: never an error that names a path.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use fin_codecs::{DecodeThreads, FrameReader, ProbeResult};
use fin_container::Limits;
use fin_core::Rational;
use fin_media::cache::{EntryMeta, META_FILE};
use fin_media::jobs::{ManifestProxy, ProxyTier};
use fin_media::sample::SampleTable;
use fin_media::table_bin::{self, TABLE_FILE};
use fin_media::{Rotation, cache::CacheKey};

use super::open_cache::WORKER_MAX_PIXELS;

/// The proxy's video file in an entry (`media.proxy`'s output).
pub const PROXY_FILE: &str = "proxy.mp4";

/// The most `meta.json` a worker reads.
const META_CAP: u64 = 1 << 20;

/// Why a listed proxy could not be used: the `reason` of the frame's `proxyFallbacks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The entry, its directory or a file of it is not there.
    Missing,
    /// A file is not the size or digest the entry lists (or is gone from under its handle).
    Changed,
    /// The entry does not agree with the manifest, with itself, or with the original's frame
    /// count, or a file is not a plain regular file.
    Invalid,
    /// A proxy frame did not decode.
    Decode,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::Missing => "missing",
            Refusal::Changed => "changed",
            Refusal::Invalid => "invalid",
            Refusal::Decode => "decode",
        }
    }
}

/// What an open proxy needs beside its reader: the original's table and display facts.
pub struct ProxySide {
    /// The original's sample table (`table.bin`): `frame_at` runs on it.
    pub table: SampleTable,
    pub rotation: Rotation,
    pub pixel_aspect: Rational,
}

/// An entry opened: the reader, a second handle on `proxy.mp4` for the identity check on
/// reuse, and the side facts.
pub struct Opened {
    pub reader: FrameReader<File>,
    pub held: File,
    pub side: ProxySide,
}

/// `SHM_REFUSED {reason}` as a [`Refusal`].
fn refusal(e: &fin_core::CoreError) -> Refusal {
    match e.details.get("reason").and_then(|r| r.as_str()) {
        Some("missing" | "io") => Refusal::Missing,
        Some("changed" | "overCap" | "truncated" | "sizeMismatch") => Refusal::Changed,
        _ => Refusal::Invalid,
    }
}

/// Opens `dir/name` no-follow, as a regular file of at most `cap` bytes.
fn open_file(dir: &Path, name: &str, cap: u64) -> Result<File, Refusal> {
    fin_shm::checked_open(&dir.join(name), cap).map_err(|e| refusal(&e))
}

fn read_all(mut f: File, cap: u64) -> Result<Vec<u8>, Refusal> {
    let mut out = Vec::new();
    (&mut f)
        .take(cap)
        .read_to_end(&mut out)
        .map_err(|_| Refusal::Changed)?;
    Ok(out)
}

/// The entry directory `p` names, when it is what the manifest says it is: absolute and
/// `<shard>/<key>` at its end for the key the manifest lists.
fn entry_dir(p: &ManifestProxy) -> Result<&Path, Refusal> {
    let key = CacheKey::parse(&p.key).map_err(|_| Refusal::Invalid)?;
    let dir = Path::new(&p.dir);
    let ends = dir.file_name().is_some_and(|n| n == key.as_str())
        && dir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == key.shard());
    if dir.is_absolute() && ends {
        Ok(dir)
    } else {
        Err(Refusal::Invalid)
    }
}

/// Opens the entry `p` of the asset with `fingerprint` (module docs).
pub fn open(p: &ManifestProxy, fingerprint: &str, caps: &ProbeResult) -> Result<Opened, Refusal> {
    let dir = entry_dir(p)?;
    let meta: EntryMeta = {
        let f = open_file(dir, META_FILE, META_CAP)?;
        serde_json::from_slice(&read_all(f, META_CAP)?).map_err(|_| Refusal::Invalid)?
    };
    let key = meta.check().map_err(|_| Refusal::Invalid)?;
    if key.as_str() != p.key
        || meta.fingerprint != fingerprint
        || meta.recipe != p.tier.recipe().name
        || meta.recipe_version != p.tier.recipe().version
    {
        return Err(Refusal::Invalid);
    }
    let listed = |name: &str| meta.files.iter().find(|f| f.name == name);
    let (Some(table_file), Some(proxy_file)) = (listed(TABLE_FILE), listed(PROXY_FILE)) else {
        return Err(Refusal::Invalid);
    };
    let table = {
        let f = open_file(dir, TABLE_FILE, table_file.bytes)?;
        let bytes = read_all(f, table_file.bytes)?;
        if bytes.len() as u64 != table_file.bytes
            || blake3::hash(&bytes).to_hex().as_str() != table_file.blake3
        {
            return Err(Refusal::Changed);
        }
        table_bin::decode(&bytes).map_err(|_| Refusal::Invalid)?
    };
    let file = open_file(dir, PROXY_FILE, proxy_file.bytes)?;
    if file.metadata().map_err(|_| Refusal::Changed)?.len() != proxy_file.bytes {
        return Err(Refusal::Changed);
    }
    let held = file.try_clone().map_err(|_| Refusal::Missing)?;
    let reader = FrameReader::open_with_limit(
        file,
        &Limits::default(),
        caps,
        DecodeThreads::default(),
        WORKER_MAX_PIXELS,
    )
    .map_err(|_| Refusal::Invalid)?;
    // One proxy frame per original frame (Decision 7).
    if reader.table().len() != table.len() {
        return Err(Refusal::Invalid);
    }
    let own = reader.info().video.as_ref().ok_or(Refusal::Invalid)?;
    // The original's rotation and pixel aspect, which a proxy does not bake in.
    let shown = meta
        .media
        .as_ref()
        .and_then(|m| m.video.as_ref())
        .unwrap_or(own);
    let side = ProxySide {
        table,
        rotation: shown.rotation,
        pixel_aspect: shown.pixel_aspect,
    };
    Ok(Opened { reader, held, side })
}

/// Whether the entry `p` still opens to the file `held` (`proxy.mp4`): the reuse check of a
/// warm worker's open sources (the entry may have been evicted and made again).
pub fn still_there(p: &ManifestProxy, held: &File) -> bool {
    entry_dir(p)
        .and_then(|dir| open_file(dir, PROXY_FILE, u64::MAX))
        .is_ok_and(|now| fin_library::same_file(&now, held))
}

/// The tier a look reads.
pub const LOOK_TIER: ProxyTier = ProxyTier::Preview;
