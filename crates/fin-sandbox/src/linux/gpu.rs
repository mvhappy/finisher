//! The worker profile's base GPU rows on Linux (spec §8.10, plan 7C Decision 8): render nodes
//! read-write, the rest of `/dev/dri` read-only, and the files the Vulkan loader and the
//! driver read, from `data/linux/gpu.txt`. Sub-project 6 passes them in
//! `WorkerInput.devices` for its GPU kinds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::policy::{DeviceAccess, DeviceRule};

const GPU_LINUX: &str = include_str!("../../data/linux/gpu.txt");

/// The rows of `data/linux/gpu.txt` that exist on this host, canonical, one per path (the
/// widest access wins), sorted.
pub fn devices() -> Vec<DeviceRule> {
    rows(GPU_LINUX)
}

fn rows(text: &str) -> Vec<DeviceRule> {
    let mut out: BTreeMap<PathBuf, DeviceAccess> = BTreeMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((access, path)) = line.split_once(' ') else {
            continue;
        };
        let access = match access {
            "rw" => DeviceAccess::ReadWrite,
            _ => DeviceAccess::Read,
        };
        for p in expand(path.trim()) {
            // A row the host does not have grants nothing; a link is granted at its target.
            let Ok(p) = std::fs::canonicalize(&p) else {
                continue;
            };
            let e = out.entry(p).or_insert(access);
            *e = (*e).max(access);
        }
    }
    out.into_iter()
        .map(|(path, access)| DeviceRule { path, access })
        .collect()
}

/// `dir/prefix*` → the entries of `dir` starting with `prefix`; anything else → itself.
fn expand(pattern: &str) -> Vec<PathBuf> {
    let Some(stem) = pattern.strip_suffix('*') else {
        return vec![PathBuf::from(pattern)];
    };
    let p = Path::new(stem);
    let (Some(dir), Some(prefix)) = (p.parent(), p.file_name()) else {
        return vec![];
    };
    let prefix = prefix.to_string_lossy();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    rd.filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(&*prefix))
        .map(|e| e.path())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_canonical_and_deduplicated() {
        let d = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(d.path()).unwrap();
        std::fs::create_dir_all(root.join("dri")).unwrap();
        for f in ["renderD128", "renderD129", "card0"] {
            std::fs::write(root.join("dri").join(f), b"").unwrap();
        }
        std::os::unix::fs::symlink(root.join("dri"), root.join("link")).unwrap();
        let text = format!(
            "# c\nrw {r}/dri/renderD*\nr {r}/dri\nr {r}/link\nr {r}/missing\n",
            r = root.display()
        );
        let got = rows(&text);
        assert_eq!(
            got,
            vec![
                DeviceRule {
                    path: root.join("dri"),
                    access: DeviceAccess::Read
                },
                DeviceRule {
                    path: root.join("dri/renderD128"),
                    access: DeviceAccess::ReadWrite
                },
                DeviceRule {
                    path: root.join("dri/renderD129"),
                    access: DeviceAccess::ReadWrite
                },
            ]
        );
    }

    #[test]
    fn the_shipped_list_parses() {
        for line in GPU_LINUX.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (access, path) = line.split_once(' ').expect(line);
            assert!(["r", "rw"].contains(&access), "{line}");
            assert!(path.starts_with('/') && !path.ends_with('/'), "{line}");
        }
    }
}
