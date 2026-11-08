//! Where the key ring may live: never inside the registry folder, which is published or pushed
//! (spec §5.6). Paths are compared after resolving them as the OS would, so `..`, symlinks,
//! folders that do not exist yet and case-insensitive file systems cannot hide a key ring
//! inside the registry. Another name for an existing folder that resolving does not unify
//! (on Windows, a UNC path such as `\\localhost\C$\…` for a local folder, or a second host
//! name for one share) is caught by comparing folder identities (volume and file id).

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
            // Nothing exists, not even the root (a drive that is not there): keep the root,
            // so `Q:\x` and `R:\x` stay different folders.
            _ => break cur.to_path_buf(),
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
    let (dir, keys) = (resolve(dir), resolve(keys_dir));
    folded(&keys).starts_with(&folded(&dir)) || same_folder_inside(&dir, &keys)
}

/// Whether some folder of `keys` is, by identity, the deepest existing folder of `dir`, and the
/// rest of `keys` continues `dir`'s missing tail. Both paths are already resolved.
fn same_folder_inside(dir: &Path, keys: &Path) -> bool {
    let Some(base) = dir.ancestors().find(|a| a.is_dir()) else {
        return false;
    };
    let Ok(id) = same_file::Handle::from_path(base) else {
        return false;
    };
    let tail = folded(dir.strip_prefix(base).unwrap_or(Path::new("")));
    keys.ancestors().any(|a| {
        same_file::Handle::from_path(a).is_ok_and(|h| h == id)
            && keys
                .strip_prefix(a)
                .is_ok_and(|rest| folded(rest).starts_with(&tail))
    })
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// The 8.3 short form of an existing path, if the volume makes short names.
    fn short_path(p: &Path) -> Option<PathBuf> {
        let cmd = format!(
            "(New-Object -ComObject Scripting.FileSystemObject).GetFolder('{}').ShortPath",
            p.display()
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &cmd])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (s.contains('~')).then(|| PathBuf::from(s))
    }

    #[test]
    fn prefixes_case_short_names_and_junctions() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("registry-with-a-long-name");
        std::fs::create_dir_all(&reg).unwrap();
        let s = reg.to_str().unwrap().to_string();
        let verbatim = reg.canonicalize().unwrap();
        assert!(verbatim.to_str().unwrap().starts_with(r"\\?\"));
        let upper = s[..1].to_lowercase() + &s[1..].to_uppercase();
        let mut inside = vec![
            verbatim.join("keys"),
            PathBuf::from(format!(r"\\?\{s}\keys")),
            Path::new(&upper).join("keys"),
            PathBuf::from(format!("{}/keys", s.replace('\\', "/"))),
            reg.join(r"new\..\keys"),
        ];
        let junction = tmp.path().join("junction");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&reg)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink /J failed");
        inside.push(junction.join("keys"));
        match short_path(&reg) {
            Some(short) => {
                inside.push(short.join("keys"));
                // A registry named by its short form, the key ring by its long one.
                assert!(keys_inside(&short, &reg.join("keys")));
                assert!(keys_inside(&short.join("new"), &reg.join(r"new\keys")));
            }
            None => eprintln!("no 8.3 short names on this volume; short-name cases skipped"),
        }
        for keys in &inside {
            assert!(keys_inside(&reg, keys), "{}", keys.display());
            assert!(keys_inside(&verbatim, keys), "{}", keys.display());
        }
        for keys in [
            tmp.path().join("keys"),
            tmp.path().join("registry-with-a-long-name-keys"),
            PathBuf::from(format!(r"\\?\{s}-keys")),
        ] {
            assert!(!keys_inside(&reg, &keys), "{}", keys.display());
        }
    }

    /// A UNC name of a local folder (the administrative share) is the same folder: resolving
    /// gives `\\?\UNC\localhost\C$\…` against `\\?\C:\…`, so only identity unifies them.
    #[test]
    fn unc_alias_of_a_local_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("registry");
        std::fs::create_dir_all(&reg).unwrap();
        let s = reg.to_str().unwrap();
        let unc = PathBuf::from(format!(r"\\localhost\{}${}", &s[..1], &s[2..]));
        if !unc.is_dir() {
            eprintln!("{} is not reachable; skipped", unc.display());
            return;
        }
        let vunc = PathBuf::from(format!(r"\\?\UNC\localhost\{}${}", &s[..1], &s[2..]));
        for keys in [unc.join("keys"), vunc.join("keys"), unc.join(r"new\keys")] {
            assert!(keys_inside(&reg, &keys), "{}", keys.display());
        }
        assert!(keys_inside(&unc, &reg.join("keys")));
        // A registry folder that does not exist yet, under a folder that does.
        assert!(keys_inside(&reg.join("fresh"), &unc.join(r"fresh\keys")));
        assert!(!keys_inside(&reg.join("fresh"), &unc.join(r"other\keys")));
        let outside = PathBuf::from(format!(r"\\localhost\{}${}-keys", &s[..1], &s[2..]));
        std::fs::create_dir_all(&outside).unwrap();
        assert!(!keys_inside(&reg, &outside));
        assert!(!keys_inside(&unc, &tmp.path().join("keys")));
    }

    /// When not even the drive exists, the drive letter still tells folders apart.
    #[test]
    fn missing_drives_keep_their_letter() {
        let Some(d) = ('D'..='Z')
            .rev()
            .find(|d| !Path::new(&format!(r"{d}:\")).exists())
        else {
            return;
        };
        let e = ('D'..d)
            .rev()
            .find(|e| !Path::new(&format!(r"{e}:\")).exists())
            .unwrap_or('A');
        let reg = PathBuf::from(format!(r"{d}:\x"));
        assert!(keys_inside(&reg, Path::new(&format!(r"{d}:\x\keys"))));
        assert!(!keys_inside(&reg, Path::new(&format!(r"{e}:\x\keys"))));
        assert!(!keys_inside(&reg, Path::new(r"C:\x\keys")));
    }
}
