//! The folder snapshots a rebuild offers as seeds (`.finisher/snapshots/<branch>/*.fsnap`,
//! spec §3.4). Only files that verify, whose name, directory and header agree, that belong
//! to this project, name the genesis the rebuild starts from (`genesisHash`) and whose
//! state upcasts are offered; whether one seeds is decided against
//! the resolved chain (`import::apply`, step 3).

use std::fs;
use std::path::Path;

use crate::import::Seed;
use crate::names::{SnapName, hash12};
use crate::scan::SNAPSHOT_CAP;
use crate::snapfile;

pub(super) fn read(project_dir: &Path, project_id: &str, genesis_hash: &str) -> Vec<Seed> {
    let root = project_dir.join(".finisher").join("snapshots");
    let Ok(dirs) = fs::read_dir(&root) else {
        return vec![];
    };
    let mut out = vec![];
    for dir in dirs.filter_map(Result::ok) {
        let Ok(branch) = dir.file_name().into_string() else {
            continue;
        };
        let Ok(files) = fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.filter_map(Result::ok) {
            let Some(name) = f.file_name().to_str().and_then(SnapName::parse) else {
                continue;
            };
            let Ok(meta) = fs::symlink_metadata(f.path()) else {
                continue;
            };
            if !meta.is_file() || meta.len() > SNAPSHOT_CAP {
                continue;
            }
            // Reading a placeholder downloads it; a first read that times out is retried.
            let path = f.path();
            let Ok(bytes) = crate::fsio::hydrating(&path, || fs::read(&path)) else {
                continue;
            };
            let Ok(snap) = snapfile::decode(&bytes) else {
                continue;
            };
            let h = &snap.header;
            if h.branch != branch
                || h.project_id != project_id
                || h.seq != name.seq()
                || hash12(&snap.hash) != Some(name.hash12())
                || h.min_reader_version > snapfile::FORMAT_VERSION
                || h.genesis_hash != genesis_hash
            {
                continue;
            }
            let Ok(value) = serde_json::from_slice(&snap.body) else {
                continue;
            };
            let Ok(project) = fin_core::upcast_snapshot(value) else {
                continue;
            };
            if project.id != project_id {
                continue;
            }
            out.push(Seed {
                branch: branch.clone(),
                seq: h.seq,
                tx_hash: h.tx_hash.clone(),
                project,
            });
        }
    }
    out
}
