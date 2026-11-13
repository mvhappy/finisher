//! The inputs of a media job about one asset (plan 6B-1 Task 11: `media.probeDeep`): the
//! manifest of that asset alone, written into the job's `in/` like `prepareMediaJob`'s, from
//! the branch head's model. Nothing is evaluated and no snapshot is written: the worker needs
//! only the file and the facts its opener checks.

use std::path::PathBuf;

use fin_core::{AssetKind, CoreError};
use fin_media::jobs::{MANIFEST_VERSION, ManifestAsset, ManifestFonts, RenderManifest};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Serialize;
use serde_json::json;

use super::inputs;
use crate::engine::{Engine, Shared, with_store};
use crate::{encode, reject};

/// What `prepareAssetJob` resolves to.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Prepared {
    rev: u64,
    /// The asset's fingerprint in the model at `rev`: the kind compares it with the one the
    /// job was submitted for (`ASSET_CHANGED`).
    fingerprint: String,
    /// The original the manifest names (none when it is missing), for the worker's grant.
    sources: Vec<String>,
}

fn prepare(store: &Shared, branch: &str, asset: &str, dir: PathBuf) -> Result<String> {
    let (rev, project, project_dir) = with_store(store, |s| {
        let (rev, project) = s.snapshot(branch).map_err(reject)?;
        Ok((rev, project, s.project_dir().to_path_buf()))
    })?;
    let a = project.assets.get(asset).ok_or_else(|| {
        reject(
            CoreError::new("ASSET_NOT_FOUND", format!("no asset {asset} on {branch}"))
                .with_details(json!({ "asset": asset, "branch": branch })),
        )
    })?;
    if a.kind != AssetKind::Media {
        return Err(reject(
            CoreError::new(
                "ASSET_TYPE_UNSUPPORTED",
                format!("asset {asset} is not a media asset"),
            )
            .with_details(json!({ "asset": asset, "kind": a.kind }))
            .with_fix("probe a video asset"),
        ));
    }
    let original = inputs::original_file(&inputs::resolve(&project_dir, &a.path));
    let sources = original.iter().map(|o| o.path.clone()).collect();
    let manifest = RenderManifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestAsset {
            asset: asset.to_string(),
            kind: a.kind,
            fingerprint: a.fingerprint.clone(),
            original,
            proxies: Vec::new(),
        }],
        fonts: ManifestFonts::default(),
    };
    inputs::write_json(&dir, "manifest.json", &manifest).map_err(reject)?;
    encode(&Prepared {
        rev,
        fingerprint: a.fingerprint.clone(),
        sources,
    })
}

#[napi]
impl Engine {
    /// Writes `manifest.json` into `inputDir` (the job's `in/` directory, which must exist)
    /// naming only `asset` of `branch`'s head, and resolves to `{rev, fingerprint, sources}`
    /// JSON. Rejects `ASSET_NOT_FOUND`, and `ASSET_TYPE_UNSUPPORTED` for an asset that is not
    /// `media`.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn prepare_asset_job<'e>(
        &self,
        env: &'e Env,
        branch: String,
        asset: String,
        input_dir: String,
    ) -> Result<Object<'e>> {
        let store = self.inner.clone();
        self.offload(env, move || {
            prepare(&store, &branch, &asset, input_dir.into())
        })
    }
}
