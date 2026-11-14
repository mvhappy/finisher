//! The worker profile's base GPU rows on Linux (spec §8.10, plan 7C Decision 8): render nodes
//! read-write, the rest of `/dev/dri` read-only, and the files the Vulkan loader and the
//! driver read, from `data/linux/gpu.txt`. Sub-project 6 passes them in
//! `WorkerInput.devices` for its GPU kinds.
//!
//! A driver named by the environment (review of 6B-1 Task 14): when this process's environment
//! names Vulkan ICD manifests ([`ICD_VARS`]: `VK_ICD_FILENAMES`, `VK_DRIVER_FILES`,
//! `VK_ADD_DRIVER_FILES`, which `finisherd` passes to its media workers), each manifest and the
//! driver library it names are granted read-only, canonical, the files only (never their
//! directories). A directory entry grants the `.json` manifests directly in it; a library named
//! without a `/` is found by the dynamic loader in the system paths the list already grants.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::policy::{DeviceAccess, DeviceRule};

const GPU_LINUX: &str = include_str!("../../data/linux/gpu.txt");

/// The Vulkan loader's variables naming ICD manifests (older loaders read only the first).
pub const ICD_VARS: [&str; 3] = ["VK_ICD_FILENAMES", "VK_DRIVER_FILES", "VK_ADD_DRIVER_FILES"];

/// The largest manifest read (a real one is a few hundred bytes).
const MANIFEST_CAP: u64 = 64 << 10;

/// The rows of `data/linux/gpu.txt` that exist on this host and the ICD files this process's
/// environment names, canonical, one per path (the widest access wins), sorted.
pub fn devices() -> Vec<DeviceRule> {
    with_icds(rows(GPU_LINUX), &icd_files(|k| std::env::var_os(k)))
}

fn with_icds(rows: Vec<DeviceRule>, icds: &[PathBuf]) -> Vec<DeviceRule> {
    let mut out: BTreeMap<PathBuf, DeviceAccess> =
        rows.into_iter().map(|d| (d.path, d.access)).collect();
    for p in icds {
        out.entry(p.clone()).or_insert(DeviceAccess::Read);
    }
    out.into_iter()
        .map(|(path, access)| DeviceRule { path, access })
        .collect()
}

/// The manifests the [`ICD_VARS`] of `env` name and the libraries they name: canonical regular
/// files that exist, sorted, without duplicates. Anything unreadable or malformed grants nothing.
fn icd_files(env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut out = std::collections::BTreeSet::new();
    for var in ICD_VARS {
        let Some(value) = env(var) else { continue };
        for entry in std::env::split_paths(&value) {
            if !entry.is_absolute() {
                continue;
            }
            for manifest in manifests(&entry) {
                let Some(canonical) = regular(&manifest) else {
                    continue;
                };
                out.insert(canonical);
                if let Some(lib) = library_of(&manifest).and_then(|l| regular(&l)) {
                    out.insert(lib);
                }
            }
        }
    }
    out.into_iter().collect()
}

/// `entry` itself, or the `.json` files directly in it when it is a directory.
fn manifests(entry: &Path) -> Vec<PathBuf> {
    if !entry.is_dir() {
        return vec![entry.to_path_buf()];
    }
    let Ok(rd) = std::fs::read_dir(entry) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    v
}

/// `p` canonical, when it is a regular file.
fn regular(p: &Path) -> Option<PathBuf> {
    let c = std::fs::canonicalize(p).ok()?;
    c.metadata().ok()?.is_file().then_some(c)
}

/// The driver library `manifest` names (`ICD.library_path`), as the loader resolves it: a path
/// with a `/` relative to the manifest's directory; a bare name is the dynamic loader's (none).
fn library_of(manifest: &Path) -> Option<PathBuf> {
    let mut text = String::new();
    std::fs::File::open(manifest)
        .ok()?
        .take(MANIFEST_CAP)
        .read_to_string(&mut text)
        .ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let lib = v.get("ICD")?.get("library_path")?.as_str()?;
    if !lib.contains('/') {
        return None;
    }
    let lib = Path::new(lib);
    Some(if lib.is_absolute() {
        lib.to_path_buf()
    } else {
        manifest.parent()?.join(lib)
    })
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
    fn icd_manifests_and_their_libraries_are_granted_as_files() {
        let d = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(d.path()).unwrap();
        let drv = root.join("drv");
        std::fs::create_dir_all(drv.join("more")).unwrap();
        std::fs::write(drv.join("libsw.so"), b"").unwrap();
        std::fs::write(root.join("libabs.so"), b"").unwrap();
        let icd = |lib: &str| {
            format!(r#"{{"file_format_version":"1.0.0","ICD":{{"library_path":"{lib}"}}}}"#)
        };
        std::fs::write(drv.join("sw.json"), icd("./libsw.so")).unwrap();
        std::fs::write(
            drv.join("more/abs.json"),
            icd(&root.join("libabs.so").display().to_string()),
        )
        .unwrap();
        std::fs::write(drv.join("more/bare.json"), icd("libvulkan_lvp.so")).unwrap();
        std::fs::write(drv.join("more/notes.txt"), b"").unwrap();
        std::fs::write(drv.join("bad.json"), b"{not json").unwrap();
        // A link to a manifest: granted at its target, its library beside the link (as the loader).
        std::os::unix::fs::symlink(drv.join("sw.json"), root.join("link.json")).unwrap();
        let env = |k: &str| -> Option<OsString> {
            match k {
                "VK_ICD_FILENAMES" => Some(
                    format!(
                        "{}:relative.json:{}",
                        drv.join("sw.json").display(),
                        root.join("missing.json").display()
                    )
                    .into(),
                ),
                "VK_DRIVER_FILES" => Some(drv.join("more").into()),
                "VK_ADD_DRIVER_FILES" => Some(
                    format!(
                        "{}:{}",
                        drv.join("bad.json").display(),
                        root.join("link.json").display()
                    )
                    .into(),
                ),
                _ => None,
            }
        };
        assert_eq!(
            icd_files(env),
            vec![
                drv.join("bad.json"),
                drv.join("libsw.so"),
                drv.join("more/abs.json"),
                drv.join("more/bare.json"),
                drv.join("sw.json"),
                root.join("libabs.so"),
            ]
        );
        assert!(icd_files(|_| None).is_empty());
        // Merged with the list's rows: read-only files, never their directories; a listed path keeps its access.
        let rw = DeviceRule {
            path: drv.join("sw.json"),
            access: DeviceAccess::ReadWrite,
        };
        let got = with_icds(vec![rw.clone()], &icd_files(env));
        assert!(got.contains(&rw));
        assert!(got.contains(&DeviceRule {
            path: drv.join("libsw.so"),
            access: DeviceAccess::Read
        }));
        assert!(
            !got.iter()
                .any(|r| r.path == drv || r.path == drv.join("more"))
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
