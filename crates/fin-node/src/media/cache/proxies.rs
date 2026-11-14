//! The proxies a media job's manifest lists (6B-1 plan Task 16; spec §4.2): for each source
//! fingerprint, the current entry of each tier in the index, pinned for the job so eviction cannot
//! take it while a worker reads it (`job:<id>`, released by the job's end hook), with its directory
//! on this machine. A listed entry is a consumer's hit (Task 13): it now serves the job's project
//! too, so that project reaches it and pins it while open. Which entry is current: the tier's
//! recipe at this build's recipe and render versions, listing `proxy.mp4` and `table.bin`, made by
//! a codec the manifest knows; of several (an AV1 and an H.264 one), the newest.

use std::collections::BTreeMap;

use fin_core::CoreError;
use fin_media::RENDER_VERSION;
use fin_media::cache::{CacheKey, EntryMeta};
use fin_media::jobs::{ManifestProxy, ProxyCodec, ProxyTier};

use super::state::{Cache, check_holder, check_projects};

/// The proxy video and the original's sample table in an entry (`media.proxy`'s output).
const PROXY_FILES: [&str; 2] = ["proxy.mp4", "table.bin"];

fn codec_of(meta: &EntryMeta) -> Option<ProxyCodec> {
    serde_json::from_value(meta.params.get("codec")?.clone()).ok()
}

/// Whether `meta` is a proxy of `tier` that this build reads.
fn current(meta: &EntryMeta, tier: ProxyTier) -> bool {
    let recipe = tier.recipe();
    meta.recipe == recipe.name
        && meta.recipe_version == recipe.version
        && meta.render_version == RENDER_VERSION
        && PROXY_FILES
            .iter()
            .all(|f| meta.files.iter().any(|e| e.name == *f))
        && codec_of(meta).is_some()
}

impl Cache {
    /// The current `tier` proxies of `fingerprint` in the index, newest first (the key breaks a
    /// tie so the choice is the same every time).
    fn current_entries(
        &mut self,
        fingerprint: &str,
        tier: ProxyTier,
    ) -> Result<Vec<EntryMeta>, CoreError> {
        let mut entries: Vec<EntryMeta> = self
            .entries_for(fingerprint)?
            .into_iter()
            .filter(|m| current(m, tier))
            .collect();
        entries.sort_by(|a, b| (b.created_at, &a.key).cmp(&(a.created_at, &b.key)));
        Ok(entries)
    }

    /// The `tier` proxy of `fingerprint` that [`proxies_for`](Self::proxies_for) would list,
    /// serving `projects` (pinned for those open) and its last use set to now; `None` when the
    /// cache holds none. Pins nothing for a job: a planner reads its `meta.json` (the
    /// original's media information and decoder class) without the original (plan 6B-1 Task
    /// 15, review).
    pub fn current_proxy(
        &mut self,
        fingerprint: &str,
        tier: ProxyTier,
        projects: &[String],
    ) -> Result<Option<EntryMeta>, CoreError> {
        check_projects(projects)?;
        for meta in self.current_entries(fingerprint, tier)? {
            // `lookup` also drops a row whose directory has gone.
            if self.lookup(&meta.key)?.is_some() && self.serve(&meta.key, projects)? {
                return Ok(Some(meta));
            }
        }
        Ok(None)
    }

    /// The `tiers` proxies of each of `fingerprints` the cache holds (module docs), by
    /// fingerprint (a fingerprint with none is absent), each pinned for `holder`, serving
    /// `projects` (the job's) and its last use set to now. An entry whose directory has gone is
    /// dropped, not listed.
    pub fn proxies_for(
        &mut self,
        fingerprints: &[String],
        holder: &str,
        projects: &[String],
        tiers: &[ProxyTier],
    ) -> Result<BTreeMap<String, Vec<ManifestProxy>>, CoreError> {
        check_holder(holder)?;
        check_projects(projects)?;
        let mut out = BTreeMap::new();
        for fp in fingerprints {
            let mut listed = Vec::new();
            for &tier in tiers {
                for meta in self.current_entries(fp, tier)? {
                    // `lookup` also drops a row whose directory has gone.
                    if self.lookup(&meta.key)?.is_none()
                        || !self.pin(&meta.key, holder)?
                        || !self.serve(&meta.key, projects)?
                    {
                        continue;
                    }
                    let (Ok(key), Some(codec)) = (CacheKey::parse(&meta.key), codec_of(&meta))
                    else {
                        continue;
                    };
                    listed.push(ManifestProxy {
                        tier,
                        key: meta.key.clone(),
                        dir: self.entry_dir(&key).to_string_lossy().into_owned(),
                        codec,
                    });
                    break;
                }
            }
            if !listed.is_empty() {
                out.insert(fp.clone(), listed);
            }
        }
        Ok(out)
    }
}
