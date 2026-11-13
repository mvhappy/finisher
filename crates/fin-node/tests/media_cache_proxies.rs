//! The proxies a media job's manifest lists (6B-1 plan Task 16): [`Cache::proxies_for`] finds
//! the current `1080` entry of each fingerprint in the index, pins it for the job and names its
//! directory; `inputs::with_proxies` puts them in the manifest. The crate is a napi `cdylib`, so
//! this includes the napi-free modules.

#![allow(dead_code)]
// Tests build and inspect temporary trees by path; none is a library reader.
#![allow(clippy::disallowed_methods)]

#[path = "../src/media/cache/adopt.rs"]
mod adopt;
#[path = "../src/media/cache/commit.rs"]
mod commit;
#[path = "../src/media/cache/evict.rs"]
mod evict;
#[path = "../src/media/cache/index.rs"]
mod index;
#[path = "../src/media/cache/proxies.rs"]
mod proxies;
#[path = "../src/media/cache/read.rs"]
mod read;
#[path = "../src/media/cache/state.rs"]
mod state;

use std::fs;
use std::path::{Path, PathBuf};

use fin_media::backend::VendorClass;
use fin_media::cache::{CacheKey, EntryFile, EntryMeta, KeyInput, PROXY_540, PROXY_1080, Recipe};
use fin_media::jobs::{ProxyCodec, ProxyTier};
use serde_json::json;
use state::Cache;

struct Fx {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    jobs: PathBuf,
}

impl Fx {
    fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let (data, jobs) = (base.join("data"), base.join("jobs"));
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&jobs).unwrap();
        Fx {
            _tmp: tmp,
            data,
            jobs,
        }
    }

    fn open(&self) -> Cache {
        Cache::open(&self.data, None, None).unwrap().0
    }

    /// Commits a proxy entry of `fp` at `recipe` (`codec` in its params) with `created_at`.
    fn commit(
        &self,
        c: &mut Cache,
        fp: &str,
        recipe: Recipe,
        codec: &str,
        created_at: i64,
        files: &[(&str, &[u8])],
    ) -> String {
        let input = KeyInput::new(fp, recipe, json!({ "codec": codec }), VendorClass::Software);
        let meta = EntryMeta {
            key: CacheKey::compute(&input).to_string(),
            fingerprint: fp.into(),
            recipe: input.recipe.clone(),
            recipe_version: input.recipe_version,
            params: input.params.clone(),
            render_version: input.render_version,
            decoder_class: input.decoder_class,
            encoder: None,
            files: files
                .iter()
                .map(|(n, b)| EntryFile {
                    name: (*n).into(),
                    bytes: b.len() as u64,
                    blake3: blake3::hash(b).to_hex().to_string(),
                })
                .collect(),
            media: None,
            tone_mapped: None,
            created_at,
        };
        let out = self.jobs.join(format!("job-{}", meta.key)).join("out");
        let dir = out.join(&meta.key);
        fs::create_dir_all(&dir).unwrap();
        for (n, b) in files {
            fs::write(dir.join(n), b).unwrap();
        }
        fs::write(dir.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        c.commit(&out, &meta.key, &[]).unwrap();
        meta.key
    }
}

const FILES: &[(&str, &[u8])] = &[("proxy.mp4", b"mp4"), ("table.bin", b"tbl")];

fn fps(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn the_1080_entry_is_listed_with_its_directory_and_pinned_for_the_job() {
    let fx = Fx::new();
    let mut c = fx.open();
    let key = fx.commit(&mut c, "fp1", PROXY_1080, "av1", 5, FILES);
    // The 540 tier, another asset's entry and a thumbnail recipe are not listed.
    fx.commit(&mut c, "fp1", PROXY_540, "av1", 5, FILES);
    let other = fx.commit(&mut c, "fp2", PROXY_1080, "av1", 5, FILES);
    let found = c
        .proxies_for(&fps(&["fp1", "fp3"]), "job:j1", &[ProxyTier::Preview])
        .unwrap();
    assert_eq!(
        found.keys().map(String::as_str).collect::<Vec<_>>(),
        ["fp1"]
    );
    let p = &found["fp1"][0];
    assert_eq!(
        (p.tier, p.codec, p.key.as_str()),
        (ProxyTier::Preview, ProxyCodec::Av1, key.as_str())
    );
    let dir = Path::new(&p.dir);
    assert!(dir.is_absolute());
    assert_eq!(dir.file_name().unwrap().to_str().unwrap(), key);
    assert_eq!(
        dir.parent().unwrap().file_name().unwrap().to_str().unwrap(),
        &key[..2]
    );
    assert!(dir.join("proxy.mp4").is_file());
    assert_eq!(c.pins_of(&key).unwrap(), ["job:j1"]);
    assert!(c.pins_of(&other).unwrap().is_empty());
    // The job's end releases it.
    c.release("job:j1").unwrap();
    assert!(c.pins_of(&key).unwrap().is_empty());
}

#[test]
fn a_pinned_entry_survives_eviction_for_the_jobs_life() {
    let fx = Fx::new();
    let mut c = fx.open();
    let key = fx.commit(&mut c, "fp1", PROXY_1080, "av1", 5, FILES);
    c.proxies_for(&fps(&["fp1"]), "job:j1", &[ProxyTier::Preview])
        .unwrap();
    c.gc(Some(0)).unwrap();
    assert!(c.lookup(&key).unwrap().is_some(), "pinned: not evicted");
    c.release("job:j1").unwrap();
    c.gc(Some(0)).unwrap();
    assert!(c.lookup(&key).unwrap().is_none());
}

#[test]
fn the_newest_current_entry_wins_and_stale_ones_are_not_listed() {
    let fx = Fx::new();
    let mut c = fx.open();
    let old = fx.commit(&mut c, "fp1", PROXY_1080, "h264", 5, FILES);
    let new = fx.commit(&mut c, "fp1", PROXY_1080, "av1", 9, FILES);
    let found = c
        .proxies_for(&fps(&["fp1"]), "job:j1", &[ProxyTier::Preview])
        .unwrap();
    assert_eq!(found["fp1"].len(), 1, "one entry per tier");
    assert_eq!(found["fp1"][0].key, new);
    assert_eq!(found["fp1"][0].codec, ProxyCodec::Av1);
    assert!(c.pins_of(&old).unwrap().is_empty());
    // An entry of an older recipe version is not current.
    let stale = Recipe {
        version: PROXY_1080.version + 1,
        ..PROXY_1080
    };
    let c2 = fx.commit(&mut c, "fp4", stale, "av1", 9, FILES);
    let found = c
        .proxies_for(&fps(&["fp4"]), "job:j2", &[ProxyTier::Preview])
        .unwrap();
    assert!(found.is_empty(), "{found:?}");
    assert!(c.pins_of(&c2).unwrap().is_empty());
}

#[test]
fn an_entry_without_its_files_or_with_an_unknown_codec_is_not_listed() {
    let fx = Fx::new();
    let mut c = fx.open();
    fx.commit(
        &mut c,
        "fp1",
        PROXY_1080,
        "av1",
        5,
        &[("proxy.mp4", b"mp4")],
    );
    fx.commit(&mut c, "fp2", PROXY_1080, "vp9", 5, FILES);
    let found = c
        .proxies_for(&fps(&["fp1", "fp2"]), "job:j1", &[ProxyTier::Preview])
        .unwrap();
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_bad_holder_is_refused() {
    let fx = Fx::new();
    let mut c = fx.open();
    let e = c
        .proxies_for(&fps(&["fp1"]), "nobody", &[ProxyTier::Preview])
        .unwrap_err();
    assert_eq!(e.code, "INVALID_OP");
}
