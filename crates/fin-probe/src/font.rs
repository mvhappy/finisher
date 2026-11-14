//! Fonts (TTF, OTF, TTC): the faces from `fin_text::font_info` (the one source of family,
//! weight, italic and script coverage, Decision 38) and the whole file's SHA-256
//! (Decision 37), which is what a pin of this asset names.

use std::io::{Read, Seek};
use std::path::Path;

use fin_core::{AssetKind, CoreError, FontFace, FontFile, FontFormat};
use sha2::{Digest, Sha256};

use crate::{Probe, read_all, unsupported};

fn be32(b: &[u8], i: usize) -> Option<u32> {
    let s = b.get(i..i + 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Every table of the font at `at` lies inside the file (a truncated file fails here, before
/// any table is read lazily as empty).
fn tables_in_bounds(b: &[u8], at: usize) -> bool {
    let Some(n) = b
        .get(at + 4..at + 6)
        .map(|s| usize::from(u16::from_be_bytes([s[0], s[1]])))
    else {
        return false;
    };
    (0..n).all(|i| {
        let rec = at + 12 + 16 * i;
        match (be32(b, rec + 8), be32(b, rec + 12)) {
            (Some(off), Some(len)) => (off as usize)
                .checked_add(len as usize)
                .is_some_and(|end| end <= b.len()),
            _ => false,
        }
    })
}

/// The offset tables: one for a single font, each listed one for a collection.
fn offsets(b: &[u8], format: FontFormat) -> Option<Vec<usize>> {
    if format != FontFormat::Ttc {
        return Some(vec![0]);
    }
    let n = be32(b, 8)? as usize;
    (0..n)
        .map(|i| be32(b, 12 + 4 * i).map(|o| o as usize))
        .collect()
}

pub(crate) fn probe<R: Read + Seek>(
    file: &mut R,
    size: u64,
    format: FontFormat,
    path: &Path,
) -> Result<Probe, CoreError> {
    let bytes = read_all(file, size, path)?;
    let whole = offsets(&bytes, format)
        .is_some_and(|o| !o.is_empty() && o.iter().all(|&at| tables_in_bounds(&bytes, at)));
    if !whole {
        return Err(unsupported(
            path,
            "truncated font: a table lies outside the file",
        ));
    }
    let faces = fin_text::font_info(&bytes).map_err(|e| unsupported(path, &e.message))?;
    let sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let faces = faces
        .into_iter()
        .map(|f| FontFace {
            index: f.index,
            family: f.family,
            weight: f.weight,
            italic: f.italic,
            scripts: f.scripts,
        })
        .collect();
    Ok(Probe {
        font: Some(FontFile {
            sha256,
            size: bytes.len() as u64,
            format,
            faces,
        }),
        ..Probe::of(AssetKind::Font)
    })
}
