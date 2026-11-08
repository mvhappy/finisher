//! The pure parts of a media job's inputs (plan 6A Task 12): the plan hashes, the assets the
//! plans reference, the manifest and the atomic writes. Nothing here touches the store or
//! napi, so the unit tests below run natively.
//!
//! - **Plan samples** (Decision 4): [`plan_hash`] is the worker's `plan_hash` (BLAKE3 of the
//!   plan's compact JSON); `evalFrame` answers the same bytes.
//! - **The manifest** lists the assets the plans reference and no others. `original` is the
//!   file the model's stored path names (relative to the project folder when it is relative)
//!   with the facts the worker's opener checks (size, mtime, file id), or `null` when no
//!   regular file is there (Decision 5). Those facts come from the library walker's own
//!   `Source`, serialised as [`fin_library::Expect`] so the encodings are the opener's.
//! - **Writes** go to a temporary name created exclusively and are renamed over the target, so
//!   a half-written file is never read and a link planted at the final name is replaced, not
//!   followed.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use fin_core::eval::{EffectPlan, FramePlan, GraphicPlan, Layer, PlanValue};
use fin_core::{AssetKind, CoreError, Project};
use fin_library::RawRel;
use fin_library::source::{NativeSource, Source};
use fin_library::types::{EntryType, Expect};
use fin_media::jobs::{
    FontFileRef, MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile, RenderManifest,
};
use serde::Serialize;
use serde_json::json;

/// BLAKE3 of the plan's compact JSON (plan 6A Decision 4), lowercase hex; the `media` worker
/// computes the same.
pub fn plan_hash(plan: &FramePlan) -> String {
    let json = serde_json::to_string(plan).expect("a plan serialises");
    blake3::hash(json.as_bytes()).to_hex().to_string()
}

fn value_asset(v: &PlanValue, out: &mut BTreeSet<String>) {
    if let PlanValue::Asset { asset } = v {
        out.insert(asset.clone());
    }
}

fn effect_assets(e: &EffectPlan, out: &mut BTreeSet<String>) {
    e.params.values().for_each(|v| value_asset(v, out));
}

fn graphic_assets(g: &GraphicPlan, out: &mut BTreeSet<String>) {
    out.insert(g.asset.clone());
    g.slots.values().for_each(|v| value_asset(v, out));
}

fn layer_assets(l: &Layer, out: &mut BTreeSet<String>) {
    if let Some(s) = &l.source {
        out.insert(s.asset.clone());
    }
    l.effects.iter().for_each(|e| effect_assets(e, out));
    if let Some(g) = &l.graphic {
        graphic_assets(g, out);
    }
    l.children
        .iter()
        .flatten()
        .for_each(|c| layer_assets(c, out));
}

/// The ids of every asset `plans` reference: sources, graphic templates, asset-valued effect
/// parameters and slots, through nested layers.
pub fn referenced_assets(plans: &[FramePlan]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    plans
        .iter()
        .flat_map(|p| &p.layers)
        .for_each(|l| layer_assets(l, &mut out));
    out
}

/// `stored` as an absolute path: itself, else below `project_dir`.
fn resolve(project_dir: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        project_dir.join(p)
    }
}

/// What the library walker's source says `path` is now, as the worker's opener will expect it,
/// or `None` unless a regular file is there (a link is never followed, a directory is no
/// original, a name that is not UTF-8 cannot be written to the manifest).
pub fn original_file(path: &Path) -> Option<OriginalFile> {
    let name = path.file_name()?.to_str()?;
    let source = NativeSource::new(path.parent()?);
    let dir = source.open_dir(&RawRel::root()).ok()?;
    let entry = source.stat_child(&dir, name.as_bytes()).ok()??;
    if entry.entry_type != EntryType::File {
        return None;
    }
    let expect = Expect {
        size: entry.size,
        mtime_ns: entry.mtime_ns,
        file_id: entry.file_id,
        dev: None,
    };
    // `Expect`'s own serde form carries the encodings (`mtimeNs` decimal, `fileId` base64).
    let mut wire = serde_json::to_value(&expect).ok()?;
    wire["path"] = json!(path.to_str()?);
    wire.as_object_mut()?.remove("dev");
    serde_json::from_value(wire).ok()
}

/// The manifest for `plans` over `project` (spec §4.2): the referenced assets, once each in id
/// order, and the font bytes captions may use. `font_cache` is the server's font cache root.
pub fn build_manifest(
    project: &Project,
    project_dir: &Path,
    plans: &[FramePlan],
    font_cache: Option<&Path>,
) -> RenderManifest {
    let assets = referenced_assets(plans)
        .into_iter()
        .filter_map(|id| {
            let a = project.assets.get(&id)?;
            Some(ManifestAsset {
                asset: id,
                kind: a.kind,
                fingerprint: a.fingerprint.clone(),
                original: original_file(&resolve(project_dir, &a.path)),
            })
        })
        .collect();
    let mut files: Vec<FontFileRef> = project
        .assets
        .values()
        .filter(|a| a.kind == AssetKind::Font)
        .filter_map(|a| {
            Some(FontFileRef {
                sha256: a.font.as_ref()?.sha256.clone(),
                original: original_file(&resolve(project_dir, &a.path))?,
            })
        })
        .collect();
    files.sort_by(|a, b| (&a.sha256, &a.original.path).cmp(&(&b.sha256, &b.original.path)));
    RenderManifest {
        version: MANIFEST_VERSION,
        assets,
        fonts: ManifestFonts {
            cache: font_cache.and_then(|p| p.to_str()).map(str::to_owned),
            files,
        },
    }
}

fn io(dir: &Path, name: &str, step: &str, e: std::io::Error) -> CoreError {
    CoreError::new(
        "IO_ERROR",
        format!("cannot write the job input {name}: {step}: {e}"),
    )
    .with_details(json!({ "file": name, "step": step, "dir": dir.to_string_lossy() }))
}

#[allow(clippy::disallowed_methods)] // the job's own private `in/` directory, never a library path
/// Writes `value` as `dir/name` (compact JSON): to `<name>.tmp` created exclusively (0600),
/// synced, then renamed over the target. `dir` is the job's own `in/` directory.
pub fn write_json(dir: &Path, name: &str, value: &impl Serialize) -> Result<(), CoreError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| CoreError::new("INTERNAL", format!("cannot encode {name}: {e}")))?;
    let tmp = dir.join(format!("{name}.tmp"));
    // A leftover of a crashed earlier attempt (ours: the directory is private).
    let _ = fs::remove_file(&tmp);
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut file = opts.open(&tmp).map_err(|e| io(dir, name, "create", e))?;
    let done = file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| io(dir, name, "write", e));
    drop(file);
    if let Err(e) = done {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, dir.join(name)).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io(dir, name, "rename", e)
    })
}
