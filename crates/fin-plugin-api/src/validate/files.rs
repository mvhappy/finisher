//! Every file the manifest names is a safe package path and present in the package: native
//! executables, a node runtime's `main`, the message catalogues and contributed files. The
//! archive reader (`fin-registry::package`) applies the stricter entry rules (reserved names,
//! NFC, case-folded duplicates) to every entry.

use super::{Invalid, PackageFiles};
use crate::manifest::{MANIFEST_FILE, Manifest, Runtime};

/// A relative `/`-separated path with no empty, `.` or `..` component, no `\`, NUL or
/// control character, and no drive letter; at most 1,024 bytes.
pub fn is_package_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 1024
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.chars().any(char::is_control)
        && !p
            .split('/')
            .next()
            .is_some_and(|c| c.len() == 2 && c.ends_with(':'))
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// The files `m` names, as (JSON pointer, package path).
pub(super) fn referenced(m: &Manifest) -> Vec<(String, String)> {
    let mut v = Vec::new();
    match &m.runtime {
        Some(Runtime::Native { exec }) => {
            for (platform, file) in exec {
                v.push((format!("/runtime/exec/{}", platform.as_str()), file.clone()));
            }
        }
        Some(Runtime::Node { main, .. }) => v.push(("/runtime/main".into(), main.clone())),
        None => {}
    }
    if let Some(dir) = &m.messages {
        for (i, locale) in m.locales.iter().enumerate() {
            v.push((format!("/locales/{i}"), catalogue_path(dir, locale)));
        }
    }
    for (i, c) in m.contributes.font_catalogs.iter().enumerate() {
        v.push((
            format!("/contributes/fontCatalogs/{i}/file"),
            c.file.clone(),
        ));
    }
    for (i, c) in m.contributes.caption_styles.iter().enumerate() {
        v.push((
            format!("/contributes/captionStyles/{i}/file"),
            c.file.clone(),
        ));
    }
    v
}

/// `<messages>/<locale>.json`.
pub(super) fn catalogue_path(dir: &str, locale: &str) -> String {
    format!("{}/{locale}.json", dir.trim_end_matches('/'))
}

pub(super) fn check(m: &Manifest, files: &dyn PackageFiles, out: &mut Vec<Invalid>) {
    if let Some(dir) = &m.messages
        && !is_package_path(dir.trim_end_matches('/'))
    {
        out.push(Invalid::manifest(
            "/messages",
            "path",
            format!("{dir:?} is not a package path"),
        ));
    }
    for (path, file) in referenced(m) {
        if !is_package_path(&file) {
            out.push(
                Invalid::manifest(&path, "path", format!("{file:?} is not a package path"))
                    .with("file", file),
            );
        } else if file == MANIFEST_FILE || !files.exists(&file) {
            out.push(
                Invalid::manifest(
                    &path,
                    "fileMissing",
                    format!("{file} is not in the package"),
                )
                .with("file", file),
            );
        }
    }
}
