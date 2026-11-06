//! `FontCatalog::validate`: the Rust half of the catalog gate (4C Decision 31). The rule
//! names, the licence allow list, the formats and the size limit come from
//! `catalog/rules.json`, which `scripts/check-font-catalog.mjs` reads too.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use fin_core::CoreError;
use serde::Deserialize;
use serde_json::json;

use crate::catalog::{CatalogFile, CatalogSource, CatalogStyle, FontCatalog};

/// The shared rule list.
pub const RULES_JSON: &str = include_str!("../catalog/rules.json");

/// The largest file (or archive) a catalog may name: 64 MiB.
pub const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rules {
    licences: Vec<String>,
    formats: Vec<String>,
    archive_formats: Vec<String>,
    max_size: u64,
    source_hosts: Vec<String>,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let r: Rules = serde_json::from_str(RULES_JSON).expect("catalog/rules.json parses");
        assert_eq!(r.max_size, MAX_FILE_SIZE, "rules.json maxSize");
        r
    })
}

pub(crate) fn invalid(rule: &str, path: &str, message: impl Into<String>) -> CoreError {
    let message = message.into();
    CoreError::new(
        "FONT_CATALOG_INVALID",
        if path.is_empty() {
            message
        } else {
            format!("{path}: {message}")
        },
    )
    .with_details(json!({ "rule": rule, "path": path }))
}

/// Formats are checked on the JSON (before the typed enums would refuse them as `schema`),
/// so both halves of the gate name the same rule.
pub(crate) fn check_formats(v: &serde_json::Value) -> Result<(), CoreError> {
    let r = rules();
    let empty = Vec::new();
    let arr = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|a| a.as_array())
            .unwrap_or(&empty)
            .clone()
    };
    for (i, e) in arr(v, "entries").iter().enumerate() {
        for (j, st) in arr(e, "styles").iter().enumerate() {
            let path = format!("/entries/{i}/styles/{j}");
            if let Some(f) = st.pointer("/file/format").and_then(|f| f.as_str())
                && !r.formats.iter().any(|x| x == f)
            {
                return Err(invalid(
                    "format",
                    &format!("{path}/file/format"),
                    format!("{f} is not a font format"),
                ));
            }
            for (k, src) in arr(st, "sources").iter().enumerate() {
                if let Some(f) = src.pointer("/archive/format").and_then(|f| f.as_str())
                    && !r.archive_formats.iter().any(|x| x == f)
                {
                    return Err(invalid(
                        "format",
                        &format!("{path}/sources/{k}/archive/format"),
                        format!("{f} is not an archive format"),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 64 lowercase hex digits.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_id(s: &str) -> bool {
    s.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_script(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 4 && b[0].is_ascii_uppercase() && b[1..].iter().all(u8::is_ascii_lowercase)
}

fn is_version(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts[0].len() == 4
        && (1..=2).contains(&parts[1].len())
        && !parts[2].is_empty()
        && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether `url` can only ever serve the same bytes: a path segment that is a 40-hex commit
/// (`raw.githubusercontent.com/<owner>/<repo>/<commit>/…`), a segment ending `@<commit>`
/// (`cdn.jsdelivr.net/gh/<owner>/<repo>@<commit>/…`), or a `/releases/download/<tag>/` path.
pub fn is_immutable_url(url: &str) -> bool {
    let Some(rest) = url.split_once("://").map(|(_, r)| r) else {
        return false;
    };
    let path = rest.split(['?', '#']).next().unwrap_or("");
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let pinned_segment = segments.iter().any(|s| {
        is_commit(s)
            || s.rsplit_once('@')
                .is_some_and(|(n, c)| !n.is_empty() && is_commit(c))
    });
    let release = segments
        .windows(4)
        .any(|w| w[0] == "releases" && w[1] == "download" && !w[2].is_empty() && !w[3].is_empty());
    pinned_segment || release
}

/// The host of `url` when its authority has no user info (`user@host` is refused).
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next(),
        None => authority.split(':').next(),
    }
}

/// Whether a source URL's host is on the `sourceHosts` list (in `test-hooks` builds also a
/// loopback `http` URL, the tests' local server).
pub fn is_allowed_source_host(url: &str) -> bool {
    if cfg!(feature = "test-hooks") && crate::http::is_loopback_http(url) {
        return true;
    }
    host_of(url).is_some_and(|h| {
        rules()
            .source_hosts
            .iter()
            .any(|a| a.eq_ignore_ascii_case(h))
    })
}

/// `https`; in `test-hooks` builds also plain `http` to loopback (the tests' local server).
fn is_https(url: &str) -> bool {
    if url.starts_with("https://") && url.len() > "https://".len() {
        return true;
    }
    cfg!(feature = "test-hooks") && crate::http::is_loopback_http(url)
}

fn check_url(path: &str, url: &str) -> Result<(), CoreError> {
    if !is_https(url) {
        return Err(invalid("https", path, format!("{url} is not https")));
    }
    Ok(())
}

fn check_sha(path: &str, sha: &str) -> Result<(), CoreError> {
    if !is_sha256(sha) {
        return Err(invalid("sha256", path, "not 64 lowercase hex digits"));
    }
    Ok(())
}

fn check_size(path: &str, size: u64) -> Result<(), CoreError> {
    if size == 0 || size > rules().max_size {
        return Err(invalid(
            "size",
            path,
            format!("{size} bytes is outside 1..={}", rules().max_size),
        ));
    }
    Ok(())
}

fn check_weight(path: &str, weight: u16) -> Result<(), CoreError> {
    if !(1..=1000).contains(&weight) {
        return Err(invalid(
            "weight",
            path,
            format!("{weight} is outside 1..=1000"),
        ));
    }
    Ok(())
}

fn check_scripts(path: &str, scripts: &[String]) -> Result<(), CoreError> {
    match scripts.iter().find(|s| !is_script(s)) {
        Some(s) => Err(invalid(
            "scripts",
            path,
            format!("{s} is not an ISO 15924 code"),
        )),
        None => Ok(()),
    }
}

fn check_source(path: &str, src: &CatalogSource) -> Result<(), CoreError> {
    check_url(&format!("{path}/url"), &src.url)?;
    if !is_immutable_url(&src.url) {
        return Err(invalid(
            "immutableUrl",
            &format!("{path}/url"),
            format!("{} names no commit or release tag", src.url),
        ));
    }
    if !is_allowed_source_host(&src.url) {
        return Err(invalid(
            "sourceHost",
            &format!("{path}/url"),
            format!("{} is not on a catalog source host", src.url),
        ));
    }
    match (&src.archive, &src.sha256, src.size) {
        (Some(m), Some(sha), Some(size)) => {
            check_sha(&format!("{path}/sha256"), sha)?;
            check_size(&format!("{path}/size"), size)?;
            let member = &m.member;
            if member.is_empty()
                || member.starts_with('/')
                || member.contains('\\')
                || member.split('/').any(|p| p == ".." || p.is_empty())
            {
                return Err(invalid(
                    "archive",
                    &format!("{path}/archive/member"),
                    "not a relative path",
                ));
            }
            Ok(())
        }
        (Some(_), _, _) => Err(invalid(
            "archive",
            path,
            "an archive source states the archive's sha256 and size",
        )),
        (None, None, None) => Ok(()),
        (None, _, _) => Err(invalid(
            "archive",
            path,
            "only an archive source states a sha256 or size",
        )),
    }
}

fn check_style(
    path: &str,
    style: &CatalogStyle,
    shas: &mut BTreeMap<String, CatalogFile>,
    keys: &mut BTreeSet<String>,
) -> Result<(), CoreError> {
    if !is_id(&style.id) {
        return Err(invalid(
            "id",
            &format!("{path}/id"),
            format!("{:?}", style.id),
        ));
    }
    check_weight(&format!("{path}/weight"), style.weight)?;
    let file = &style.file;
    check_sha(&format!("{path}/file/sha256"), &file.sha256)?;
    check_size(&format!("{path}/file/size"), file.size)?;
    if let Some(v) = &file.variable {
        let wp = format!("{path}/file/variable/wght");
        check_weight(&format!("{wp}/0"), v.wght[0])?;
        check_weight(&format!("{wp}/1"), v.wght[1])?;
        if v.wght[0] > v.wght[1] || !(v.wght[0]..=v.wght[1]).contains(&style.weight) {
            return Err(invalid(
                "variable",
                &wp,
                format!(
                    "{:?} does not cover the style's weight {}",
                    v.wght, style.weight
                ),
            ));
        }
    }
    // One file may serve several styles only as a variable font, declared identically.
    let repeat = match shas.get(&file.sha256) {
        None => false,
        Some(_) if file.variable.is_none() => {
            return Err(invalid(
                "unique",
                &format!("{path}/file/sha256"),
                "listed twice",
            ));
        }
        Some(seen) if seen != file => {
            return Err(invalid(
                "variable",
                &format!("{path}/file"),
                "a variable file listed again must be declared identically",
            ));
        }
        Some(_) => true,
    };
    shas.insert(file.sha256.clone(), file.clone());
    if file.faces.is_empty() {
        return Err(invalid("faces", &format!("{path}/file/faces"), "no faces"));
    }
    let mut indexes = BTreeSet::new();
    for (k, face) in file.faces.iter().enumerate() {
        let fp = format!("{path}/file/faces/{k}");
        check_weight(&format!("{fp}/weight"), face.weight)?;
        check_scripts(&format!("{fp}/scripts"), &face.scripts)?;
        if !indexes.insert(face.index) {
            return Err(invalid("unique", &format!("{fp}/index"), "listed twice"));
        }
        let key = format!("{}|{}|{}", face.family, face.weight, face.italic);
        if face.family.is_empty() || (!repeat && !keys.insert(key.clone())) {
            return Err(invalid(
                "unique",
                &fp,
                format!("face key {key} is listed twice"),
            ));
        }
    }
    let default = file.faces.iter().find(|f| f.index == style.default_face);
    let fits = |f: &&crate::catalog::CatalogFace| {
        f.italic == style.italic && (f.weight == style.weight || file.variable.is_some())
    };
    if !default.is_some_and(|f| fits(&f)) {
        return Err(invalid(
            "faces",
            &format!("{path}/defaultFace"),
            "not a face of the file with the style's weight (or a variable one) and italic",
        ));
    }
    if style.sources.is_empty() {
        return Err(invalid("sources", &format!("{path}/sources"), "no sources"));
    }
    for (k, src) in style.sources.iter().enumerate() {
        check_source(&format!("{path}/sources/{k}"), src)?;
    }
    Ok(())
}

impl FontCatalog {
    /// Checks every rule of `catalog/rules.json`; the error names the first one broken.
    pub fn validate(&self) -> Result<(), CoreError> {
        if !is_version(&self.catalog_version) {
            return Err(invalid("version", "/catalogVersion", "not YYYY.MM.N"));
        }
        let (mut ids, mut shas, mut keys) = (BTreeSet::new(), BTreeMap::new(), BTreeSet::new());
        for (i, entry) in self.entries.iter().enumerate() {
            let path = format!("/entries/{i}");
            if !is_id(&entry.id) {
                return Err(invalid(
                    "id",
                    &format!("{path}/id"),
                    format!("{:?}", entry.id),
                ));
            }
            if !ids.insert(entry.id.as_str()) {
                return Err(invalid("unique", &format!("{path}/id"), "listed twice"));
            }
            if !rules().licences.contains(&entry.licence.spdx) {
                return Err(invalid(
                    "licence",
                    &format!("{path}/licence/spdx"),
                    format!("{} is not on the permissive allow list", entry.licence.spdx),
                ));
            }
            check_url(&format!("{path}/licence/url"), &entry.licence.url)?;
            check_url(&format!("{path}/homepage"), &entry.homepage)?;
            if entry.scripts.is_empty() {
                return Err(invalid("scripts", &format!("{path}/scripts"), "none"));
            }
            check_scripts(&format!("{path}/scripts"), &entry.scripts)?;
            let mut style_ids = BTreeSet::new();
            for (j, style) in entry.styles.iter().enumerate() {
                let sp = format!("{path}/styles/{j}");
                check_style(&sp, style, &mut shas, &mut keys)?;
                if !style_ids.insert(style.id.as_str()) {
                    return Err(invalid("unique", &format!("{sp}/id"), "listed twice"));
                }
            }
            if entry.styles.is_empty() {
                return Err(invalid("faces", &format!("{path}/styles"), "no styles"));
            }
        }
        let mut seen = BTreeSet::new();
        for (i, id) in self.fallback.iter().enumerate() {
            if !ids.contains(id.as_str()) || !seen.insert(id) {
                return Err(invalid(
                    "fallback",
                    &format!("/fallback/{i}"),
                    format!("{id} is unknown or listed twice"),
                ));
            }
        }
        Ok(())
    }
}
