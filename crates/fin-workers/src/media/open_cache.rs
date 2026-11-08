//! Open sources, kept across the jobs of a warm worker (plan 6A Decision 6): a
//! [`FrameReader`] (the parsed container, its `MediaInfo` and sample table, a decoder and its
//! recent frames) per original, keyed by the asset's fingerprint and the file's path, size,
//! mtime and file id as the manifest recorded them, so a changed or swapped file is a new
//! entry. A source that cannot be read is kept as its issue. At most [`MAX_OPEN`] entries, the
//! least recently used out first.
//!
//! **Reuse re-checks the file** (plan 6A Task 11-12 review M4): the path is opened again through
//! the no-follow opener, which checks it against the manifest's size, mtime and id, and the
//! handle it gives must be the file the entry holds (device and inode); otherwise the entry is
//! dropped and the source opened afresh. A file replaced or rewritten between two jobs of one
//! worker is therefore never read through the old handle.
//!
//! **Memory** (review M2): the decoded frames of all entries together are kept under
//! [`FRAME_BUDGET`] (each entry under a quarter of it), least recently used entry first, and
//! the decoder refuses pictures over [`WORKER_MAX_PIXELS`] (`LOOK_MAX_SIDE`²), so the family's
//! `MEDIA_MEMORY` limit (half the RAM, at most 6 GiB, `apps/finisherd/src/jobs/kinds/media.look.ts`)
//! holds with room for the GPU's staging, the PNG encoder and the decoders' frames in flight.
//!
//! Originals open through `fin-library`'s no-follow opener ([`open_original`]): the file's
//! directory is the root, the file name the one component below it, and the open is refused
//! when the file's size, mtime or id differ from the manifest's.

use std::fs::File;
use std::path::{Path, PathBuf};

use fin_codecs::{Capabilities, DecodeThreads, FrameReader};
use fin_compositor::LayerIssue;
use fin_container::Limits;
use fin_library::{Expect, OpenError, RawRel};
use fin_media::jobs::{LOOK_MAX_SIDE, ManifestAsset, OriginalFile};
use serde_json::{Value, json};

/// Sources kept open.
pub const MAX_OPEN: usize = 8;

/// The decoded frames all open sources keep, together (1 GiB).
pub const FRAME_BUDGET: usize = 1 << 30;

/// The largest picture a worker decodes: a frame it could not composite is not worth
/// allocating for.
pub const WORKER_MAX_PIXELS: u32 = LOOK_MAX_SIDE * LOOK_MAX_SIDE;

/// Why an original could not be opened: `ORIGINALS_MISSING {reason}`.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRefusal {
    pub reason: &'static str,
    pub message: String,
}

/// Opens `o` read-only through the no-follow opener, checked against its recorded facts.
/// The path may be absolute anywhere and its parent directories may be links: only the file
/// itself is opened no-follow (review L3; the manifest is the host's, not the worker's).
pub fn open_original(o: &OriginalFile) -> Result<File, OpenRefusal> {
    open_original_in(o, None)
}

/// [`open_original`], and with `confine` only below that directory (no `..`): `--replay`'s
/// guard against a bug report's manifest naming files elsewhere on the machine.
pub fn open_original_in(o: &OriginalFile, confine: Option<&Path>) -> Result<File, OpenRefusal> {
    let refuse = |reason, message: String| OpenRefusal { reason, message };
    let path = Path::new(&o.path);
    if let Some(root) = confine
        && (!path.starts_with(root)
            || path
                .components()
                .any(|c| c == std::path::Component::ParentDir))
    {
        return Err(refuse(
            "outsideReplay",
            format!("{} is outside the replay directory", o.path),
        ));
    }
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(refuse("notAbsolute", format!("{} names no file", o.path)));
    };
    if !path.is_absolute() {
        return Err(refuse("notAbsolute", format!("{} is not absolute", o.path)));
    }
    let name = name
        .to_str()
        .ok_or_else(|| refuse("badName", format!("{} is not UTF-8", o.path)))?;
    let rel = RawRel::new(vec![name.as_bytes().to_vec()])
        .map_err(|e| refuse("badName", format!("{}: {e}", o.path)))?;
    let mut wire = json!({ "size": o.size, "mtimeNs": o.mtime_ns });
    if let Some(id) = &o.file_id {
        wire["fileId"] = json!(id);
    }
    let expect: Expect = serde_json::from_value(wire)
        .map_err(|e| refuse("badManifest", format!("{}: {e}", o.path)))?;
    fin_library::open_for_read(dir, &rel, &expect, false).map_err(|e| {
        let reason = match &e {
            OpenError::CloudOnly => "cloudOnly",
            OpenError::Changed => "changed",
            OpenError::NotFound => "notFound",
            OpenError::Escapes => "escapes",
            OpenError::NameSurrogate => "link",
            OpenError::Io(_) => "io",
        };
        refuse(reason, e.to_string())
    })
}

/// `ORIGINALS_MISSING` for `asset`.
pub fn missing(asset: &str, reason: &str, message: Option<&str>) -> LayerIssue {
    let mut details = json!({ "asset": asset, "reason": reason });
    if let Some(m) = message {
        details["message"] = json!(m);
    }
    LayerIssue {
        code: "ORIGINALS_MISSING".into(),
        details,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    fingerprint: String,
    path: String,
    size: u64,
    mtime_ns: String,
    file_id: Option<String>,
}

/// An open source: its reader and a second handle on the same file, for the identity check
/// on reuse.
pub struct Opened {
    pub reader: FrameReader<File>,
    held: File,
}

pub enum Entry {
    Open(Box<Opened>),
    Failed(LayerIssue),
}

/// Whether `a` and `b` are the same file: device and inode, or the Windows volume serial and
/// file id (`fin_library::same_file`; size and mtime alone missed a swap there).
fn same_file(a: &File, b: &File) -> bool {
    fin_library::same_file(a, b)
}

/// Whether the path `o` names still opens, under the manifest's checks, to the file `held`.
fn still_there(o: &OriginalFile, held: &File, confine: Option<&Path>) -> bool {
    open_original_in(o, confine).is_ok_and(|now| same_file(&now, held))
}

#[derive(Default)]
pub struct OpenCache {
    /// Most recently used last.
    entries: Vec<(Key, Entry)>,
    /// `--replay`: originals open only below this directory.
    confine: Option<PathBuf>,
}

/// What [`OpenCache::get`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Got {
    /// Opened now.
    Opened,
    /// Already open (or already known unreadable).
    Reused,
}

impl OpenCache {
    /// Originals open only below `root` from now on (`None`: anywhere).
    pub fn confine(&mut self, root: Option<PathBuf>) {
        self.confine = root;
    }

    /// The entry of `asset` (its `original` present), opened now if it is not open.
    pub fn get(
        &mut self,
        asset: &ManifestAsset,
        o: &OriginalFile,
        caps: &Capabilities,
    ) -> (&mut Entry, Got) {
        let key = Key {
            fingerprint: asset.fingerprint.clone(),
            path: o.path.clone(),
            size: o.size,
            mtime_ns: o.mtime_ns.clone(),
            file_id: o.file_id.clone(),
        };
        let mut got = Got::Opened;
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            let held_ok = match &self.entries[i].1 {
                Entry::Open(open) => still_there(o, &open.held, self.confine.as_deref()),
                Entry::Failed(_) => true,
            };
            let e = self.entries.remove(i);
            if held_ok {
                self.entries.push(e);
                got = Got::Reused;
            }
        }
        if got == Got::Opened {
            if self.entries.len() >= MAX_OPEN {
                self.entries.remove(0);
            }
            let entry = open(&asset.asset, o, caps, self.confine.as_deref());
            self.entries.push((key, entry));
        }
        self.enforce_budget(FRAME_BUDGET);
        let (_, entry) = self.entries.last_mut().expect("just pushed");
        (entry, got)
    }

    /// The decoded frames all entries keep, in bytes.
    pub fn cached_bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|(_, e)| match e {
                Entry::Open(o) => o.reader.cached_bytes(),
                Entry::Failed(_) => 0,
            })
            .sum()
    }

    /// Drops the cached frames of the least recently used entries until all together fit
    /// `budget`; the entry in use (the last) goes last.
    fn enforce_budget(&mut self, budget: usize) {
        let mut total = self.cached_bytes();
        for (_, e) in &mut self.entries {
            if total <= budget {
                break;
            }
            if let Entry::Open(o) = e {
                total -= o.reader.cached_bytes();
                o.reader.drop_cache();
            }
        }
    }

    /// Forgets the failed entries (a failure that may pass, such as an I/O error, is
    /// asked again by the next job).
    pub fn forget_failures(&mut self) {
        self.entries.retain(|(_, e)| match e {
            Entry::Open(_) => true,
            Entry::Failed(issue) => !transient(issue),
        });
    }
}

/// Failures the next job asks again: an original that was missing, changed or unreadable may
/// be back; a codec the worker has no decoder for or a corrupt file stays as it is for the same
/// key.
fn transient(issue: &LayerIssue) -> bool {
    issue.code == "ORIGINALS_MISSING" || issue.code == "IO_ERROR"
}

fn open(asset: &str, o: &OriginalFile, caps: &Capabilities, confine: Option<&Path>) -> Entry {
    let file = match open_original_in(o, confine) {
        Ok(f) => f,
        Err(r) => return Entry::Failed(missing(asset, r.reason, Some(&r.message))),
    };
    let held = match file.try_clone() {
        Ok(h) => h,
        Err(e) => return Entry::Failed(missing(asset, "io", Some(&e.to_string()))),
    };
    let reader = FrameReader::open_with_limit(
        file,
        &Limits::default(),
        caps,
        DecodeThreads::default(),
        WORKER_MAX_PIXELS,
    );
    match reader {
        Ok(mut reader) => {
            reader.set_cache_cap(FRAME_BUDGET / 4);
            Entry::Open(Box::new(Opened { reader, held }))
        }
        Err(e) => {
            let mut details = match e.details {
                Value::Object(m) => Value::Object(m),
                _ => json!({}),
            };
            details["asset"] = json!(asset);
            details["message"] = json!(e.message);
            Entry::Failed(LayerIssue {
                code: e.code,
                details,
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests write fixtures and stat files by path
mod tests {
    use super::*;
    use fin_codecs::capabilities;
    use fin_codecs::fixture::{ClipSpec, pattern_clip};
    use fin_core::AssetKind;
    use std::time::UNIX_EPOCH;

    struct Clip {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    fn clip(frames: u32) -> Clip {
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path()).unwrap().join("a.mp4");
        let spec = ClipSpec {
            frames,
            keyint: 8,
            ..ClipSpec::default()
        };
        std::fs::write(&path, pattern_clip(&spec).unwrap()).unwrap();
        Clip { _dir: dir, path }
    }

    fn facts(path: &Path) -> OriginalFile {
        let m = std::fs::metadata(path).unwrap();
        let ns = m.modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
        OriginalFile {
            path: path.to_str().unwrap().into(),
            size: m.len(),
            mtime_ns: ns.as_nanos().to_string(),
            file_id: None,
        }
    }

    fn asset(o: &OriginalFile) -> ManifestAsset {
        ManifestAsset {
            asset: "a1".into(),
            kind: AssetKind::Media,
            fingerprint: "fp".into(),
            original: Some(o.clone()),
        }
    }

    fn get(cache: &mut OpenCache, o: &OriginalFile) -> (Got, bool) {
        let (entry, got) = cache.get(&asset(o), o, &capabilities());
        (got, matches!(entry, Entry::Open(_)))
    }

    /// Decodes frames `0..n` of the entry for `o`.
    fn decode(cache: &mut OpenCache, o: &OriginalFile, n: usize) {
        let (entry, _) = cache.get(&asset(o), o, &capabilities());
        let Entry::Open(open) = entry else {
            panic!("not open")
        };
        for i in 0..n {
            open.reader.frame(i).unwrap();
        }
    }

    /// Review M4: the file id is part of the key.
    #[test]
    fn the_file_id_is_part_of_the_key() {
        let c = clip(8);
        let mut cache = OpenCache::default();
        let mut o = facts(&c.path);
        assert_eq!(get(&mut cache, &o), (Got::Opened, true));
        assert_eq!(get(&mut cache, &o).0, Got::Reused);
        // A different id is another file as far as the manifest says (the opener, which checks
        // the id against the file, will refuse it: the entry is its own, not the old one).
        o.file_id = Some("AAAAAAAAAAA=".into());
        assert_eq!(get(&mut cache, &o).0, Got::Opened);
        assert_eq!(cache.entries.len(), 2);
    }

    /// Review M4: a file swapped for another of the same size and mtime is not read through
    /// the old handle.
    #[test]
    fn a_swapped_source_of_the_same_size_and_mtime_is_opened_afresh() {
        let c = clip(8);
        let mut cache = OpenCache::default();
        let o = facts(&c.path);
        assert_eq!(get(&mut cache, &o), (Got::Opened, true));
        assert_eq!(get(&mut cache, &o).0, Got::Reused);
        // Another file with the same bytes length and mtime renamed over the path.
        let other = c.path.with_file_name("b.mp4");
        std::fs::copy(&c.path, &other).unwrap();
        let mtime = std::fs::metadata(&c.path).unwrap().modified().unwrap();
        std::fs::File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        std::fs::rename(&other, &c.path).unwrap();
        assert_eq!(facts(&c.path), o, "the manifest cannot tell them apart");
        let (got, open) = get(&mut cache, &o);
        assert_eq!(
            (got, open),
            (Got::Opened, true),
            "a new handle, a new reader"
        );
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(get(&mut cache, &o).0, Got::Reused);
    }

    /// Review M4: a file rewritten in place (its mtime moves) or removed is not reused.
    #[test]
    fn a_rewritten_or_removed_source_is_not_reused() {
        let c = clip(8);
        let mut cache = OpenCache::default();
        let o = facts(&c.path);
        assert_eq!(get(&mut cache, &o).0, Got::Opened);
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&c.path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let (got, open) = get(&mut cache, &o);
        assert_eq!(
            (got, open),
            (Got::Opened, false),
            "changed: ORIGINALS_MISSING"
        );
        let Some((_, Entry::Failed(issue))) = cache.entries.last() else {
            panic!("a failed entry")
        };
        assert_eq!(issue.code, "ORIGINALS_MISSING");
        assert_eq!(issue.details["reason"], "changed");
    }

    /// Review M2: the frames all entries keep fit one budget, least recently used first.
    #[test]
    fn decoded_frames_of_all_sources_fit_one_budget() {
        let (a, b) = (clip(8), clip(8));
        let (oa, ob) = (facts(&a.path), facts(&b.path));
        let mut cache = OpenCache::default();
        decode(&mut cache, &oa, 6);
        let one = cache.cached_bytes();
        assert!(one > 0);
        decode(&mut cache, &ob, 6);
        assert!(cache.cached_bytes() >= 2 * one - 1);
        // A budget of one source's frames: the older source's go first.
        cache.enforce_budget(one + 1);
        assert!(cache.cached_bytes() <= one + 1, "{}", cache.cached_bytes());
        let (_, Entry::Open(oldest)) = &cache.entries[0] else {
            panic!("open")
        };
        assert_eq!(
            oldest.reader.cached_bytes(),
            0,
            "the least recently used went first"
        );
        let (_, Entry::Open(newest)) = &cache.entries[1] else {
            panic!("open")
        };
        assert!(newest.reader.cached_bytes() > 0);
        // A frame that was dropped is decoded again, not lost.
        decode(&mut cache, &oa, 3);
    }

    /// Review M2: the worker's decoder refuses a picture over its own pixel limit.
    #[test]
    fn the_worker_refuses_pictures_over_its_pixel_limit() {
        assert_eq!(WORKER_MAX_PIXELS, 8192 * 8192);
        let c = clip(4);
        let file = File::open(&c.path).unwrap();
        // A limit below the clip's 320×180 makes the decoder fail at the first picture.
        let mut reader = FrameReader::open_with_limit(
            file,
            &Limits::default(),
            &capabilities(),
            DecodeThreads::default(),
            100 * 100,
        )
        .unwrap();
        assert_eq!(reader.frame(0).unwrap_err().code, "DECODE_FAILED");
    }
}
