//! Where the key ring may live: never inside the registry folder, which is published or pushed
//! (spec §5.6). Paths are compared after resolving them as the OS would, so `..`, symlinks,
//! folders that do not exist yet and case-insensitive file systems cannot hide a key ring
//! inside the registry.

use std::path::{Component, Path, PathBuf};

/// `path` made absolute and resolved: the longest prefix that exists is canonicalised (the OS
/// follows its symlinks and `..`), and the rest is normalised lexically (`new/../keys` of a
/// folder `new` that does not exist yet is `keys`).
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut tail = Vec::new();
    let mut cur = abs.as_path();
    let base = loop {
        if let Ok(c) = cur.canonicalize() {
            break c;
        }
        match (cur.parent(), cur.components().next_back()) {
            (Some(parent), Some(last)) => {
                tail.push(last.as_os_str().to_os_string());
                cur = parent;
            }
            _ => break PathBuf::new(),
        }
    };
    let mut out = base;
    for c in tail.iter().rev() {
        match Path::new(c).components().next() {
            Some(Component::ParentDir) => {
                out.pop();
            }
            Some(Component::CurDir) | None => {}
            Some(_) => out.push(c),
        }
    }
    out
}

/// The components of `p`, case-folded: a case-insensitive file system (macOS and Windows by
/// default) treats `Registry/keys` as inside `registry`. Folding on every OS refuses at most a
/// key folder whose path differs from the registry's only by case.
fn folded(p: &Path) -> Vec<String> {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// Whether the key ring `keys_dir` is the registry folder `dir` or inside it.
pub fn keys_inside(dir: &Path, keys_dir: &Path) -> bool {
    folded(&resolve(keys_dir)).starts_with(&folded(&resolve(dir)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_after_resolving() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("registry");
        std::fs::create_dir_all(reg.join("metadata")).unwrap();
        let outside = tmp.path().join("keys");
        for keys in [
            reg.clone(),
            reg.join("keys"),
            reg.join("./x/../keys"),
            reg.join("new/deeper/../../keys"),
            tmp.path().join("other/../registry/keys"),
            tmp.path().join("REGISTRY/Keys"),
            reg.join("metadata/../metadata"),
        ] {
            assert!(keys_inside(&reg, &keys), "{}", keys.display());
        }
        for keys in [
            outside.clone(),
            reg.join("../keys"),
            tmp.path().join("registry-keys"),
            tmp.path().join("new/../keys"),
        ] {
            assert!(!keys_inside(&reg, &keys), "{}", keys.display());
        }
        // A registry folder that does not exist yet.
        let fresh = tmp.path().join("fresh/registry");
        assert!(keys_inside(
            &fresh,
            &tmp.path().join("fresh/x/../registry/keys")
        ));
        assert!(!keys_inside(&fresh, &tmp.path().join("fresh/keys")));
        #[cfg(unix)]
        {
            // A symlink outside that points inside the registry.
            let link = tmp.path().join("link");
            std::os::unix::fs::symlink(&reg, &link).unwrap();
            assert!(keys_inside(&reg, &link.join("keys")));
            // `..` after a symlink follows the link's target, as the OS does.
            std::fs::create_dir_all(reg.join("sub")).unwrap();
            let into = tmp.path().join("into-sub");
            std::os::unix::fs::symlink(reg.join("sub"), &into).unwrap();
            assert!(keys_inside(&reg, &into.join("../keys")));
        }
    }
}
