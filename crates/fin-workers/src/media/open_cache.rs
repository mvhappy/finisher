//! Open sources, kept across the jobs of a warm worker (plan 6A Decision 6): a
//! [`FrameReader`] (the parsed container, its `MediaInfo` and sample table, a decoder and its
//! recent frames) per original, keyed by the asset's fingerprint and the file's path, size
//! and mtime as the manifest recorded them, so a changed file is a new entry. A source that
//! cannot be read is kept as its issue. At most [`MAX_OPEN`] entries, the least recently used
//! out first.
//!
//! Originals open through `fin-library`'s no-follow opener ([`open_original`]): the file's
//! directory is the root, the file name the one component below it, and the open is refused
//! when the file's size, mtime or id differ from the manifest's.

use std::fs::File;
use std::path::Path;

use fin_codecs::{Capabilities, FrameReader};
use fin_compositor::LayerIssue;
use fin_container::Limits;
use fin_library::{Expect, OpenError, RawRel};
use fin_media::jobs::{ManifestAsset, OriginalFile};
use serde_json::{Value, json};

/// Sources kept open.
pub const MAX_OPEN: usize = 8;

/// Why an original could not be opened: `ORIGINALS_MISSING {reason}`.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRefusal {
    pub reason: &'static str,
    pub message: String,
}

/// Opens `o` read-only through the no-follow opener, checked against its recorded facts.
pub fn open_original(o: &OriginalFile) -> Result<File, OpenRefusal> {
    let refuse = |reason, message: String| OpenRefusal { reason, message };
    let path = Path::new(&o.path);
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
}

pub enum Entry {
    Open(Box<FrameReader<File>>),
    Failed(LayerIssue),
}

#[derive(Default)]
pub struct OpenCache {
    /// Most recently used last.
    entries: Vec<(Key, Entry)>,
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
        };
        let got = match self.entries.iter().position(|(k, _)| *k == key) {
            Some(i) => {
                let e = self.entries.remove(i);
                self.entries.push(e);
                Got::Reused
            }
            None => {
                if self.entries.len() >= MAX_OPEN {
                    self.entries.remove(0);
                }
                let entry = open(&asset.asset, o, caps);
                self.entries.push((key, entry));
                Got::Opened
            }
        };
        let (_, entry) = self.entries.last_mut().expect("just pushed");
        (entry, got)
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

fn open(asset: &str, o: &OriginalFile, caps: &Capabilities) -> Entry {
    let file = match open_original(o) {
        Ok(f) => f,
        Err(r) => return Entry::Failed(missing(asset, r.reason, Some(&r.message))),
    };
    match FrameReader::open(file, &Limits::default(), caps) {
        Ok(reader) => Entry::Open(Box::new(reader)),
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
