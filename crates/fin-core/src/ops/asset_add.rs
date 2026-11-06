//! `asset.add`: registers an asset of any kind under a placeholder-allocated id.
//!
//! Structural (value shapes, plan 4C Decision 25): each kind carries its own fields and no
//! other kind's ([`crate::ops::AssetAdd`]); a `media` or `lottieTemplate` asset has a positive
//! duration (clips need a source range to point into); a font names its whole-file SHA-256
//! (64 lowercase hex), a size and at least one face; a LUT has an edge of at least 2; a `bin`
//! exists. Policy: a `media` asset has a video or an audio stream.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Asset, AssetKind, Project};
use crate::ops::util::{OpDiff, invalid_range, not_found, wrong_op};
use crate::ops::{AssetAdd, Bindings, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

fn payload(op: &Op) -> Result<&AssetAdd, CoreError> {
    match op {
        Op::AssetAdd(a) => Ok(a),
        _ => Err(wrong_op(op)),
    }
}

fn kind_name(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Media => "media",
        AssetKind::Image => "image",
        AssetKind::Svg => "svg",
        AssetKind::LottieTemplate => "lottieTemplate",
        AssetKind::Font => "font",
        AssetKind::Lut => "lut",
    }
}

fn shape(a: &AssetAdd, why: impl Into<String>, fix: &str) -> CoreError {
    CoreError::new("INVALID_OP", why.into())
        .with_details(json!({ "id": a.id, "kind": kind_name(a.kind) }))
        .with_fix(fix)
}

/// The fields `a` carries, by name.
fn present(a: &AssetAdd) -> Vec<&'static str> {
    [
        ("duration", a.duration.is_some()),
        ("video", a.video.is_some()),
        ("audio", a.audio.is_some()),
        ("image", a.image.is_some()),
        ("lottie", a.lottie.is_some()),
        ("font", a.font.is_some()),
        ("lut", a.lut.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, on)| on.then_some(name))
    .collect()
}

/// Checks the op against `project`; on success the op can be applied.
pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<(), CoreError> {
    let a = payload(op)?;
    let (needs, allows): (&[&str], &[&str]) = match a.kind {
        AssetKind::Media => (&["duration"], &["duration", "video", "audio"]),
        AssetKind::Image | AssetKind::Svg => (&["image"], &["image"]),
        AssetKind::LottieTemplate => (&["duration", "lottie"], &["duration", "lottie"]),
        AssetKind::Font => (&["font"], &["font"]),
        AssetKind::Lut => (&["lut"], &["lut"]),
    };
    let kind = kind_name(a.kind);
    let has = present(a);
    if let Some(field) = has.iter().find(|f| !allows.contains(f)) {
        return Err(shape(
            a,
            format!("a {kind} asset carries no {field}"),
            "send only the fields of the asset's kind (probe the file again)",
        ));
    }
    if let Some(field) = needs.iter().find(|f| !has.contains(f)) {
        return Err(shape(
            a,
            format!("a {kind} asset needs {field}"),
            "probe the file again",
        ));
    }
    if let Some(d) = a.duration
        && !d.is_positive()
    {
        return Err(invalid_range(
            "asset duration must be positive",
            json!({ "duration": d }),
            "probe the file again; a media asset has a positive duration",
        ));
    }
    if let Some(f) = &a.font {
        let hex = f.sha256.len() == 64
            && f.sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex || f.size == 0 || f.faces.is_empty() {
            return Err(shape(
                a,
                "a font asset needs its whole-file sha256 (64 lowercase hex), its size and a face",
                "probe the font file again",
            ));
        }
    }
    if let Some(l) = &a.lut
        && l.size < 2
    {
        return Err(invalid_range(
            "a LUT has an edge of at least 2",
            json!({ "size": l.size, "min": 2 }),
            "probe the .cube file again",
        ));
    }
    if let Some(bin) = &a.bin {
        let bin = bindings.resolve(bin)?;
        if !project.bins.contains_key(&bin) {
            return Err(not_found("bin", &bin));
        }
    }
    if bindings.policy() && a.kind == AssetKind::Media && a.video.is_none() && a.audio.is_none() {
        return Err(shape(
            a,
            "a media asset has a video or an audio stream",
            "probe the file again",
        ));
    }
    bindings.check_new(project, &a.id, IdKind::Asset)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    validate(project, op, bindings)?;
    let a = payload(op)?;
    let id = bindings.create(project, &a.id, IdKind::Asset, ids)?;
    let bin = a.bin.as_deref().map(|b| bindings.resolve(b)).transpose()?;
    let asset = Asset {
        id: id.clone(),
        kind: a.kind,
        path: a.path.clone(),
        fingerprint: a.fingerprint.clone(),
        bin,
        duration: a.duration,
        video: a.video.clone(),
        audio: a.audio.clone(),
        image: a.image.clone(),
        lottie: a.lottie.clone(),
        font: a.font.clone(),
        lut: a.lut.clone(),
    };
    project.assets.insert(id.clone(), asset);
    diff.record(EntityKind::Asset, &id, ChangeKind::Added);
    Ok(vec![Op::AssetRemove { id }])
}

/// Creates the asset (using its bin); binds no track.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::AssetAdd(a) = op {
        f.creates.push(a.id.clone());
        f.uses.extend(a.bin.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "asset.add";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the new id and the bin.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::AssetAdd(a) => {
            let AssetAdd { id, bin, .. } = &mut **a;
            let mut out = vec![id];
            out.extend(bin.as_mut());
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::AssetAdd(a) => {
            let mut out = vec![a.id.as_str()];
            out.extend(a.bin.as_deref());
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::AssetAdd(a) => Some(a.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
