//! The `pcm48` entries a `media.listen` job's manifest lists (plan 6C Task 21, Decision 21): for
//! each asset's fingerprint, the newest entry of this build (recipe `pcm48` at its version, this
//! build's `RENDER_VERSION` and `AUDIO_VERSION`, with chunks), pinned for the job (`job:<id>`,
//! released by the job's end hook) and serving the job's projects, as `proxies.rs` lists proxies.

use std::collections::BTreeMap;

use fin_core::CoreError;
use fin_media::cache::{CacheKey, EntryMeta};
use fin_media::jobs::{ManifestPcm, PcmRecipe};
use fin_media::{AUDIO_VERSION, RENDER_VERSION};

use super::state::{Cache, check_holder, check_projects};

/// Whether `meta` is a `pcm48` entry this build reads.
fn current(meta: &EntryMeta) -> bool {
    let recipe = PcmRecipe::Pcm48.recipe();
    meta.recipe == recipe.name
        && meta.recipe_version == recipe.version
        && meta.render_version == RENDER_VERSION
        && meta.params.get("audioVersion").and_then(|v| v.as_u64())
            == Some(u64::from(AUDIO_VERSION))
        && !meta.files.is_empty()
}

impl Cache {
    /// The current `pcm48` entry of each of `fingerprints` (module docs), by fingerprint (one
    /// with none is absent), with the files a worker reads from it (absolute).
    pub fn pcm_for(
        &mut self,
        fingerprints: &[String],
        holder: &str,
        projects: &[String],
    ) -> Result<BTreeMap<String, (ManifestPcm, Vec<String>)>, CoreError> {
        check_holder(holder)?;
        check_projects(projects)?;
        let mut out = BTreeMap::new();
        for fp in fingerprints {
            if out.contains_key(fp) {
                continue;
            }
            let mut entries: Vec<EntryMeta> =
                self.entries_for(fp)?.into_iter().filter(current).collect();
            entries.sort_by(|a, b| (b.created_at, &a.key).cmp(&(a.created_at, &b.key)));
            for meta in entries {
                // `lookup` also drops a row whose directory has gone.
                if self.lookup(&meta.key)?.is_none()
                    || !self.pin(&meta.key, holder)?
                    || !self.serve(&meta.key, projects)?
                {
                    continue;
                }
                let Ok(key) = CacheKey::parse(&meta.key) else {
                    continue;
                };
                let dir = self.entry_dir(&key);
                let files = std::iter::once(fin_media::cache::META_FILE.to_string())
                    .chain(meta.files.iter().map(|f| f.name.clone()))
                    .map(|f| dir.join(f).to_string_lossy().into_owned())
                    .collect();
                let entry = ManifestPcm {
                    key: meta.key.clone(),
                    dir: dir.to_string_lossy().into_owned(),
                };
                out.insert(fp.clone(), (entry, files));
                break;
            }
        }
        Ok(out)
    }
}
