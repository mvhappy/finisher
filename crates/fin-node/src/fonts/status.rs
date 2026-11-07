//! What a project's pins lack, as its status reports it (4C Decision 34): `fonts {missing,
//! fetching, failed}` and the warnings `FONT_MISSING`, `FONT_FETCH_FAILED` and
//! `FONTS_DAMAGED`, and `FONT_UNPINNED` for what captions need that nothing pins (Decision
//! 35), each with its fix. Warnings, never errors: a missing or unpinned font leaves captions
//! unmeasured and never blocks an edit (Decision 36).

use std::collections::{BTreeMap, BTreeSet};

use fin_fonts::{FileState, FontCatalog};
use serde_json::{Value, json};

use super::Manager;
use super::scan::Need;

/// One project's font summary. `shape` is it without download progress: a change of shape
/// is published at once, progress alone at most every [`super::PROGRESS_EVERY`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Summary {
    pub(crate) fonts: Value,
    pub(crate) warnings: Vec<Value>,
    pub(crate) shape: Value,
}

fn warning(code: &str, message: String, details: Value, fix: String) -> Value {
    json!({ "code": code, "message": message, "details": details, "fix": fix })
}

fn entry_of(catalog: &FontCatalog, sha: &str) -> (String, String) {
    catalog
        .by_sha(sha)
        .map(|(e, s)| (e.id.clone(), s.id.clone()))
        .unwrap_or_default()
}

fn missing_fix(key: &str, reason: &str, asset: bool) -> String {
    if asset {
        return format!(
            "restore the font asset's file as it was imported, or fin fonts repin {key}"
        );
    }
    match reason {
        "offline" => "fin fonts fetch --pinned (background fetching is off: fonts.autoFetch is \
                      false or FIN_FONTS_OFFLINE is set)"
            .to_string(),
        "notInCatalog" => format!(
            "the font catalog does not list this file: fin fonts repin {key}, or fetch it on a \
             server whose catalog does"
        ),
        "damaged" => "see the FONTS_DAMAGED warning".to_string(),
        _ => "fin fonts fetch --pinned".to_string(),
    }
}

/// `FONT_UNPINNED` for `key` (a face key or `script:<code>`) and the captions needing it.
fn unpinned_warning(key: &str, items: &BTreeSet<String>) -> Value {
    let fix = match key.strip_prefix("script:") {
        Some(script) => format!(
            "pin a font that covers {script} in the fallback order: fin fonts pin <family> --fallback <n> (fin fonts ls --catalog lists them)"
        ),
        None => {
            let mut parts = key.split('|');
            let (family, weight, slope) = (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or("400"),
                parts.next().unwrap_or("normal"),
            );
            let italic = if slope == "italic" { " --italic" } else { "" };
            format!("fin fonts pin \"{family}\" --weight {weight}{italic}")
        }
    };
    warning(
        "FONT_UNPINNED",
        format!(
            "{} caption(s) need {key}, which no font pin provides; they are unmeasured",
            items.len()
        ),
        json!({ "key": key, "items": items }),
        fix,
    )
}

impl Manager {
    /// The summary of `needs` (and of what captions need with no pin, `unpinned`) now.
    pub(crate) fn summary(
        &self,
        needs: &BTreeSet<Need>,
        unpinned: &BTreeMap<String, BTreeSet<String>>,
    ) -> Summary {
        let (mut missing, mut fetching, mut failed) = (Vec::new(), Vec::new(), Vec::new());
        let mut warnings = Vec::new();
        if let Some(e) = &self.damaged {
            warnings.push(warning(
                "FONTS_DAMAGED",
                format!("the font cache or catalog cannot be used: {}", e.message),
                json!({ "error": e }),
                "check the server data folder's fonts/ directory (permissions, disk) and \
                 restart finisherd; captions stay unmeasured meanwhile"
                    .to_string(),
            ));
        }
        let mut seen = BTreeSet::new();
        for need in needs {
            if !seen.insert((&need.key, &need.sha256)) {
                continue;
            }
            let (key, sha) = (need.key.as_str(), need.sha256.as_str());
            let lacking = match &need.asset {
                Some(path) => self
                    .provider
                    .check_asset(path, sha)
                    .err()
                    .map(|p| p.reason().to_string()),
                None => match self.file_state(sha) {
                    FileState::Cached => None,
                    FileState::Missing => Some(self.missing_reason(sha).to_string()),
                    FileState::Queued => {
                        fetching.push(json!({ "key": key, "sha256": sha, "bytes": 0, "total": self.size_of(sha) }));
                        None
                    }
                    FileState::Fetching { bytes, total } => {
                        fetching.push(
                            json!({ "key": key, "sha256": sha, "bytes": bytes, "total": total }),
                        );
                        None
                    }
                    FileState::Failed { reason, .. } if reason == "hashMismatch" => Some(reason),
                    FileState::Failed {
                        reason,
                        retry_at_ms,
                    } => {
                        let (entry, style) = entry_of(&self.catalog, sha);
                        failed.push(json!({ "key": key, "sha256": sha, "reason": reason, "retryAt": retry_at_ms }));
                        warnings.push(warning(
                            "FONT_FETCH_FAILED",
                            format!("font {key} could not be fetched ({reason}); captions using it are unmeasured"),
                            json!({ "key": key, "entry": entry, "style": style, "sha256": sha,
                                    "reason": reason, "retryAt": retry_at_ms }),
                            format!("check the network (behind a proxy, set HTTPS_PROXY), then: fin fonts fetch {entry}"),
                        ));
                        None
                    }
                },
            };
            if let Some(reason) = lacking {
                missing.push(json!({ "key": key, "sha256": sha, "reason": reason }));
                warnings.push(warning(
                    "FONT_MISSING",
                    format!("font {key} is not available on this server ({reason}); captions using it are unmeasured"),
                    json!({ "key": key, "sha256": sha, "reason": reason }),
                    missing_fix(key, &reason, need.asset.is_some()),
                ));
            }
        }
        warnings.extend(unpinned.iter().map(|(k, items)| unpinned_warning(k, items)));
        let strip = |v: &Vec<Value>| -> Vec<Value> {
            v.iter()
                .map(|f| json!({ "key": f["key"], "sha256": f["sha256"] }))
                .collect()
        };
        let shape = json!({ "missing": missing, "fetching": strip(&fetching), "failed": failed,
                            "damaged": self.damaged.is_some(), "autoFetch": self.auto_fetch() });
        Summary {
            fonts: json!({ "missing": missing, "fetching": fetching, "failed": failed }),
            warnings,
            shape,
        }
    }

    /// Why a catalog-pinned file that is neither cached nor being fetched is missing.
    fn missing_reason(&self, sha: &str) -> &'static str {
        if self.fetcher.is_none() {
            "damaged"
        } else if self.catalog.by_sha(sha).is_none() {
            "notInCatalog"
        } else if !self.auto_fetch() {
            "offline"
        } else {
            "notFetched"
        }
    }

    fn size_of(&self, sha: &str) -> u64 {
        self.catalog.by_sha(sha).map_or(0, |(_, s)| s.file.size)
    }

    /// The file's state in this server: the fetcher's, or `Missing` without one (a damaged
    /// cache or catalog), whatever the cache holds.
    pub(crate) fn file_state(&self, sha: &str) -> FileState {
        match &self.fetcher {
            Some(f) => f.state(sha),
            None => FileState::Missing,
        }
    }
}

/// `status` (a fin-store `ProjectStatus` JSON) with the summary merged in: `fonts`, and its
/// warnings after the store's.
pub(crate) fn merge(mut status: Value, s: &Summary) -> Value {
    if let Some(obj) = status.as_object_mut() {
        obj.insert("fonts".to_string(), s.fonts.clone());
        match obj.get_mut("warnings").and_then(Value::as_array_mut) {
            Some(w) => w.extend(s.warnings.iter().cloned()),
            None => {
                obj.insert("warnings".to_string(), Value::Array(s.warnings.clone()));
            }
        }
    }
    status
}
