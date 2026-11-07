//! The server-level font calls (4C Decisions 33 and 34): `fonts.status` (every file's state)
//! and `fonts.fetch` (what to fetch, then fetching it now).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fin_core::CoreError;
use fin_fonts::{FileState, FontFileStatus, FontsStatus};
use serde_json::{Value, json};

use super::Manager;
use super::napi::FetchReq;
use crate::engine::lock;

impl Manager {
    /// `fonts.status`: every catalog file, then pinned files the catalog does not list.
    pub(crate) fn status(&self) -> FontsStatus {
        let mut keys: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut assets: BTreeMap<String, PathBuf> = BTreeMap::new();
        for a in lock(&self.engines).values() {
            for n in a.needs() {
                keys.entry(n.sha256.clone())
                    .or_default()
                    .insert(n.key.clone());
                if let Some(p) = n.asset {
                    assets.entry(n.sha256).or_insert(p);
                }
            }
        }
        let mut files: Vec<FontFileStatus> = self
            .catalog
            .styles()
            .map(|(e, s)| {
                let face = s.face();
                FontFileStatus {
                    sha256: s.file.sha256.clone(),
                    size: s.file.size,
                    entry: Some(e.id.clone()),
                    style: Some(s.id.clone()),
                    family: face.family.clone(),
                    weight: if s.file.variable.is_some() {
                        s.weight
                    } else {
                        face.weight
                    },
                    italic: face.italic,
                    keys: Vec::new(),
                    state: self.file_state(&s.file.sha256),
                }
            })
            .collect();
        assign_keys(&mut files, &mut keys);
        for (sha, ks) in keys {
            let first = ks.iter().next().cloned().unwrap_or_default();
            let mut parts = first.split('|');
            let family = parts.next().unwrap_or_default().to_string();
            let weight = parts.next().and_then(|w| w.parse().ok()).unwrap_or(400);
            let italic = parts.next() == Some("italic");
            let state = match assets.get(&sha) {
                Some(p) => match self.provider.check_asset(p, &sha) {
                    Ok(()) => FileState::Cached,
                    Err(e) => FileState::Failed {
                        reason: e.reason().to_string(),
                        retry_at_ms: 0,
                    },
                },
                None => self.file_state(&sha),
            };
            files.push(FontFileStatus {
                size: 0,
                entry: None,
                style: None,
                family,
                weight,
                italic,
                keys: ks.into_iter().collect(),
                state,
                sha256: sha,
            });
        }
        FontsStatus {
            catalog_version: self.catalog.catalog_version.clone(),
            auto_fetch: self.auto_fetch(),
            damaged: self.damaged.clone(),
            files,
        }
    }

    /// `fonts.fetch`'s targets: SHA-256s, catalog entries (each of their styles), font keys
    /// (the catalog face), and what an open project pins (its catalog files).
    pub(crate) fn targets(&self, req: &FetchReq) -> Result<Vec<String>, CoreError> {
        let mut out = Vec::new();
        for sha in &req.sha256 {
            if self.catalog.by_sha(sha).is_none() {
                return Err(not_in_catalog(
                    json!({ "sha256": sha }),
                    format!("no catalog file has SHA-256 {sha}"),
                ));
            }
            out.push(sha.clone());
        }
        for id in &req.entries {
            let entry = self.catalog.entry(id).ok_or_else(|| {
                not_in_catalog(
                    json!({ "entry": id }),
                    format!("the font catalog has no entry {id}"),
                )
            })?;
            out.extend(entry.styles.iter().map(|s| s.file.sha256.clone()));
        }
        for key in &req.keys {
            let mut parts = key.split('|');
            let (family, weight, slope) = (parts.next(), parts.next(), parts.next());
            let face = match (family, weight.and_then(|w| w.parse::<u16>().ok()), slope) {
                (Some(f), Some(w), Some(s @ ("normal" | "italic"))) if parts.next().is_none() => {
                    self.catalog.find_face(f, w, s == "italic")
                }
                _ => None,
            };
            let (_, style, _) = face.ok_or_else(|| {
                not_in_catalog(
                    json!({ "key": key }),
                    format!("the font catalog has no face {key}"),
                )
            })?;
            out.push(style.file.sha256.clone());
        }
        if let Some(project) = &req.pinned {
            let needs = self.pinned_by(project).ok_or_else(|| {
                CoreError::new(
                    "PROJECT_NOT_FOUND",
                    format!("project {project} is not open here"),
                )
                .with_details(json!({ "projectId": project }))
                .with_fix("open the project first")
            })?;
            out.extend(
                needs
                    .into_iter()
                    .filter(|n| n.asset.is_none() && self.catalog.by_sha(&n.sha256).is_some())
                    .map(|n| n.sha256),
            );
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Fetches every file now (the off switch does not stop this), then answers the first
    /// failure, if any (`FONT_FETCH_FAILED`).
    pub(crate) fn fetch_now(&self, shas: &[String]) -> Result<(), CoreError> {
        let Some(fetcher) = &self.fetcher else {
            let e = self
                .damaged
                .clone()
                .unwrap_or_else(|| CoreError::new("INTERNAL", "no fetcher"));
            return Err(CoreError::new(
                "FONTS_DAMAGED",
                format!("fonts cannot be fetched: {}", e.message),
            )
            .with_details(json!({ "error": e }))
            .with_fix("check the server data folder's fonts/ directory and restart finisherd"));
        };
        let mut first = None;
        for sha in shas {
            if fetcher.cache().has(sha) {
                self.fetches.already_cached(&self.catalog, sha);
            }
            if let Err(e) = fetcher.fetch_now(sha) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }
}

/// Moves each key of `keys` whose file has rows in `rows` onto a row: the row of its weight
/// and slope (a variable file has a row per style), else the file's first row.
fn assign_keys(rows: &mut [FontFileStatus], keys: &mut BTreeMap<String, BTreeSet<String>>) {
    let face_of = |key: &str| {
        let mut parts = key.rsplitn(3, '|');
        let italic = parts.next() == Some("italic");
        let weight = parts.next().and_then(|w| w.parse::<u16>().ok());
        (weight, italic)
    };
    for row in rows.iter_mut() {
        if let Some(ks) = keys.get_mut(&row.sha256) {
            let (mine, rest) = std::mem::take(ks)
                .into_iter()
                .partition(|k| face_of(k) == (Some(row.weight), row.italic));
            *ks = rest;
            row.keys = mine.into_iter().collect();
        }
    }
    for row in rows.iter_mut() {
        if let Some(ks) = keys.remove(&row.sha256) {
            row.keys.extend(ks);
            row.keys.sort();
        }
    }
}

fn not_in_catalog(details: Value, message: String) -> CoreError {
    CoreError::new("FONT_NOT_IN_CATALOG", message)
        .with_details(details)
        .with_fix("fin fonts ls --catalog")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sha: &str, style: &str, weight: u16) -> FontFileStatus {
        FontFileStatus {
            sha256: sha.into(),
            size: 1,
            entry: Some("inter".into()),
            style: Some(style.into()),
            family: "Inter Variable".into(),
            weight,
            italic: false,
            keys: Vec::new(),
            state: FileState::Missing,
        }
    }

    #[test]
    fn keys_go_to_the_row_of_their_style() {
        // One variable file serving three styles (regular, bold, italic), and a static file.
        let mut italic = row("v", "italic", 400);
        italic.italic = true;
        let mut rows = [
            row("v", "regular", 400),
            row("v", "bold", 700),
            italic,
            row("s", "regular", 400),
        ];
        let mut keys: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut add = |sha: &str, key: &str| {
            keys.entry(sha.into()).or_default().insert(key.into());
        };
        add("v", "Inter|700|normal");
        add("v", "Inter|400|italic"); // the italic row, not the regular one of the same weight
        add("v", "Inter|900|normal"); // no row of its own: the file's first row
        add("s", "Other|400|italic"); // a different face of the same file: its only row
        add("x", "Asset|400|normal"); // not in the catalog: left for the caller
        assign_keys(&mut rows, &mut keys);
        let got: Vec<Vec<String>> = rows.iter().map(|r| r.keys.clone()).collect();
        assert_eq!(
            got,
            [
                vec!["Inter|900|normal".to_string()],
                vec!["Inter|700|normal".to_string()],
                vec!["Inter|400|italic".to_string()],
                vec!["Other|400|italic".to_string()],
            ]
        );
        assert_eq!(keys.keys().collect::<Vec<_>>(), ["x"]);
    }
}
