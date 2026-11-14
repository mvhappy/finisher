//! The `pcm48` entries a `media.listen` job's manifest lists (6C plan Task 21):
//! [`Cache::pcm_for`] finds each fingerprint's current entry, pins it for the job and names its
//! directory and files; `inputs::with_pcm` puts them in the manifest by asset. The crate is a
//! napi `cdylib`, so this includes the napi-free modules.

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
#[path = "../src/media/cache/pcm.rs"]
mod pcm;
#[path = "../src/media/cache/read.rs"]
mod read;
#[path = "../src/media/cache/state.rs"]
mod state;

use std::fs;
use std::path::{Path, PathBuf};

use fin_media::backend::VendorClass;
use fin_media::cache::{CacheKey, EntryFile, EntryMeta, KeyInput};
use fin_media::jobs::{PcmLayout, PcmRecipe, chunk_name, pcm_key_input};
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

    /// Commits an entry of `input` with one chunk, made at `created_at`.
    fn commit(&self, c: &mut Cache, input: KeyInput, created_at: i64) -> String {
        let chunk = vec![0u8; 16];
        let meta = EntryMeta {
            key: CacheKey::compute(&input).to_string(),
            fingerprint: input.fingerprint.clone(),
            recipe: input.recipe.clone(),
            recipe_version: input.recipe_version,
            params: input.params.clone(),
            render_version: input.render_version,
            decoder_class: input.decoder_class,
            encoder: None,
            files: vec![EntryFile {
                name: chunk_name(0),
                bytes: chunk.len() as u64,
                blake3: blake3::hash(&chunk).to_hex().to_string(),
            }],
            media: None,
            tone_mapped: None,
            created_at,
        };
        let out = self.jobs.join(format!("job-{}", meta.key)).join("out");
        let dir = out.join(&meta.key);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(chunk_name(0)), &chunk).unwrap();
        fs::write(dir.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        c.commit(&out, &meta.key, &[]).unwrap();
        meta.key
    }
}

fn fps(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn the_pcm48_entry_is_listed_with_its_files_and_pinned_for_the_job() {
    let fx = Fx::new();
    let mut c = Cache::open(&fx.data, None, None).unwrap().0;
    let input = |fp: &str, recipe, layout, class| pcm_key_input(fp, recipe, layout, class);
    // An older entry of another decoder class, the newest one, a pcm16m entry (never listed) and
    // another fingerprint's.
    let older = fx.commit(
        &mut c,
        input("fp1", PcmRecipe::Pcm48, PcmLayout::Stereo, VendorClass::Os),
        1,
    );
    let key = fx.commit(
        &mut c,
        input(
            "fp1",
            PcmRecipe::Pcm48,
            PcmLayout::Stereo,
            VendorClass::Software,
        ),
        5,
    );
    fx.commit(
        &mut c,
        input(
            "fp1",
            PcmRecipe::Pcm16m,
            PcmLayout::Mono,
            VendorClass::Software,
        ),
        9,
    );
    let other = fx.commit(
        &mut c,
        input(
            "fp2",
            PcmRecipe::Pcm16m,
            PcmLayout::Mono,
            VendorClass::Software,
        ),
        5,
    );
    let found = c
        .pcm_for(&fps(&["fp1", "fp2", "fp3"]), "job:j1", &[])
        .unwrap();
    assert_eq!(
        found.keys().map(String::as_str).collect::<Vec<_>>(),
        ["fp1"]
    );
    let (entry, files) = &found["fp1"];
    assert_eq!(entry.key, key);
    let dir = Path::new(&entry.dir);
    assert!(dir.is_absolute() && dir.join("meta.json").is_file());
    assert_eq!(
        files,
        &[
            dir.join("meta.json").to_string_lossy().into_owned(),
            dir.join(chunk_name(0)).to_string_lossy().into_owned()
        ]
    );
    assert_eq!(c.pins_of(&key).unwrap(), ["job:j1"]);
    assert!(c.pins_of(&older).unwrap().is_empty());
    assert!(c.pins_of(&other).unwrap().is_empty());
    c.release("job:j1").unwrap();
    assert!(c.pins_of(&key).unwrap().is_empty());
    // An entry of another AUDIO_VERSION is not this build's.
    let mut stale = input(
        "fp4",
        PcmRecipe::Pcm48,
        PcmLayout::Mono,
        VendorClass::Software,
    );
    stale.params["audioVersion"] = serde_json::json!(fin_media::AUDIO_VERSION + 1);
    fx.commit(&mut c, stale, 1);
    assert!(c.pcm_for(&fps(&["fp4"]), "job:j2", &[]).unwrap().is_empty());
}
