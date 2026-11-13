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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fin_core::eval::{EffectPlan, FramePlan, GraphicPlan, Layer, PlanValue};
use fin_core::{AssetKind, CoreError, Project};
use fin_library::RawRel;
use fin_library::source::{NativeSource, Source};
use fin_library::types::{EntryType, Expect};
use fin_media::jobs::{
    FontFileRef, MANIFEST_VERSION, ManifestAsset, ManifestFonts, ManifestProxy, OriginalFile,
    RenderManifest,
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

/// `stored` as an absolute path: itself, else below `project_dir`, component by component, so
/// a stored `assets/x.mp4` takes the platform's separator (`…\assets\x.mp4` on Windows, not
/// `…\assets/x.mp4`).
pub(super) fn resolve(project_dir: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        let mut out = project_dir.to_path_buf();
        out.extend(p.components());
        out
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
                proxies: Vec::new(),
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

/// `manifest` with each asset's `proxies` set from `found` (by fingerprint; an asset with no
/// entry there keeps none).
pub fn with_proxies(manifest: &mut RenderManifest, found: &BTreeMap<String, Vec<ManifestProxy>>) {
    for a in &mut manifest.assets {
        a.proxies = found.get(&a.fingerprint).cloned().unwrap_or_default();
    }
}

/// The files a worker reads from the proxy entries `manifest` lists (absolute, sorted, once
/// each): the job's `reads`.
pub fn proxy_files(manifest: &RenderManifest) -> Vec<String> {
    let mut out: Vec<String> = manifest
        .assets
        .iter()
        .flat_map(|a| &a.proxies)
        .flat_map(|p| {
            ["proxy.mp4", "table.bin", "meta.json"]
                .map(|f| Path::new(&p.dir).join(f).to_string_lossy().into_owned())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The job's `dir/manifest.json`, read as `write_json` writes: through `fin_shm`'s checked
/// open, below the job directory with the host's own links above it resolved.
pub fn read_manifest(dir: &Path) -> Result<RenderManifest, CoreError> {
    use std::io::Read;
    let dir = match (dir.parent(), dir.file_name()) {
        (Some(job), Some(last)) => fin_shm::JobDir::resolve(job)?.root.join(last),
        _ => dir.to_path_buf(),
    };
    let cap = 16 << 20;
    let file = fin_shm::checked_open(&dir.join("manifest.json"), cap)?;
    let mut text = Vec::new();
    file.take(cap)
        .read_to_end(&mut text)
        .map_err(|e| CoreError::new("IO_ERROR", format!("cannot read manifest.json: {e}")))?;
    serde_json::from_slice(&text)
        .map_err(|e| CoreError::new("JOB_INPUT_INVALID", format!("manifest.json: {e}")))
}

/// Writes `value` as `dir/name` (compact JSON) through `fin_shm::write_input`: a temporary
/// file made new (0600), synced and renamed over the target, below `dir` with no link at any
/// segment. `dir` is the job's own `in/` directory.
pub fn write_json(dir: &Path, name: &str, value: &impl Serialize) -> Result<(), CoreError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| CoreError::new("INTERNAL", format!("cannot encode {name}: {e}")))?;
    // The host's own links above the job directory (macOS `/var`, a symlinked home) are
    // resolved once, as the worker does; the job directory and `in/` themselves may not be links.
    let dir = match (dir.parent(), dir.file_name()) {
        (Some(job), Some(last)) => fin_shm::JobDir::resolve(job)?.root.join(last),
        _ => dir.to_path_buf(),
    };
    fin_shm::write_input(&dir, name, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_media::jobs::{ProxyCodec, ProxyTier};

    fn asset(id: &str, fp: &str) -> ManifestAsset {
        ManifestAsset {
            asset: id.into(),
            kind: AssetKind::Media,
            fingerprint: fp.into(),
            original: None,
            proxies: Vec::new(),
        }
    }

    fn proxy(dir: &str) -> ManifestProxy {
        ManifestProxy {
            tier: ProxyTier::Preview,
            key: "k".repeat(64),
            dir: dir.into(),
            codec: ProxyCodec::Av1,
        }
    }

    #[test]
    fn proxies_are_set_by_fingerprint_and_their_files_listed_once() {
        let mut m = RenderManifest {
            version: MANIFEST_VERSION,
            assets: vec![asset("a1", "fpA"), asset("a2", "fpB"), asset("a3", "fpA")],
            fonts: ManifestFonts::default(),
        };
        let found = BTreeMap::from([("fpA".to_string(), vec![proxy("/c/ab/key")])]);
        with_proxies(&mut m, &found);
        assert_eq!(m.assets[0].proxies, [proxy("/c/ab/key")]);
        assert!(m.assets[1].proxies.is_empty());
        assert_eq!(m.assets[2].proxies, [proxy("/c/ab/key")]);
        assert_eq!(
            proxy_files(&m),
            [
                Path::new("/c/ab/key").join("meta.json").to_str().unwrap(),
                Path::new("/c/ab/key").join("proxy.mp4").to_str().unwrap(),
                Path::new("/c/ab/key").join("table.bin").to_str().unwrap(),
            ]
        );
        // Run again with nothing found: the manifest holds no proxies.
        with_proxies(&mut m, &BTreeMap::new());
        assert!(m.assets.iter().all(|a| a.proxies.is_empty()));
    }

    #[test]
    fn a_relative_stored_path_takes_the_platform_separator() {
        let dir = std::env::temp_dir().join("p");
        assert_eq!(
            resolve(&dir, "assets/rel.mp4"),
            dir.join("assets").join("rel.mp4")
        );
        assert_eq!(
            resolve(&dir, "assets/rel.mp4").to_str().unwrap(),
            dir.join("assets").join("rel.mp4").to_str().unwrap()
        );
    }
}
