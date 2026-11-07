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
                    keys: keys
                        .remove(&s.file.sha256)
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                    state: self.file_state(&s.file.sha256),
                }
            })
            .collect();
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
            if let Err(e) = fetcher.fetch_now(sha) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }
}

fn not_in_catalog(details: Value, message: String) -> CoreError {
    CoreError::new("FONT_NOT_IN_CATALOG", message)
        .with_details(details)
        .with_fix("fin fonts ls --catalog")
}
