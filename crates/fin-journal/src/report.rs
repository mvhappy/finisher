//! What `diagnostics.status` and `diagnostics.export` answer (plan D, Decisions 30 and 32):
//! the consent, the journal's state and counts per kind and per project, and an export in
//! pages. Both read the files after a flush, so they block: call them off the JS thread.

use std::collections::BTreeMap;

use fin_core::events::{Envelope, Purposes, Ulid, kind, redact};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConsentState, Filter, Journal, JournalError, JournalStatus, ProjectPolicy};

/// The most entries one export page holds.
pub const MAX_PAGE: usize = 1000;

/// `diagnostics.status` (`fin diagnostics show`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub consent: ConsentState,
    #[serde(flatten)]
    pub status: JournalStatus,
    /// Entries stored per kind.
    pub kinds: BTreeMap<String, u64>,
    /// Every project this server was told about, with its flag now.
    pub projects: BTreeMap<String, ProjectPolicy>,
}

/// One page of `diagnostics.export`, oldest first. `next` is an opaque position: pass it as
/// the next request's `cursor`; absent on the last page. `redacted`: content fields (and the
/// blob references inside them) were removed, as for a caller that is not a person (5A
/// Decision 22); absent otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportPage {
    pub lines: Vec<Envelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redacted: bool,
}

impl ExportPage {
    /// Removes every content field, as the diagnostics-only upload view does (a training-only
    /// kind keeps its metadata part), and marks the page `redacted`.
    pub fn redact(&mut self) {
        for e in &mut self.lines {
            if let Some(spec) = kind(e.kind.as_str(), e.kv) {
                redact(spec, &mut e.payload, Purposes::DIAGNOSTICS);
            }
        }
        self.redacted = true;
    }
}

impl Journal {
    /// The consent, the state and the stored counts (a flush and a read of every file).
    pub fn report(&self) -> DiagnosticsReport {
        let status = self.status();
        let mut kinds = BTreeMap::new();
        for e in self.read(&Filter::default()) {
            *kinds.entry(e.kind.as_str().to_owned()).or_insert(0) += 1;
        }
        DiagnosticsReport {
            consent: self.consent(),
            status,
            kinds,
            projects: self.project_policies(),
        }
    }

    /// One page of [`Journal::export`]: the first `limit` (1–[`MAX_PAGE`]) entries after
    /// `cursor` (a previous page's `next`). Entry ids grow in write order, so a page is
    /// stable while entries are appended. `INVALID_ARGS` for a cursor this did not make.
    pub fn export_page(
        &self,
        filter: &Filter,
        as_upload: Option<Purposes>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ExportPage, JournalError> {
        if let Some(c) = cursor
            && Ulid::parse(c).is_none()
        {
            return Err(JournalError::new(
                "INVALID_ARGS",
                "the export cursor is not one a page gave",
            ));
        }
        let limit = limit.clamp(1, MAX_PAGE);
        let mut it = self
            .export(filter, as_upload)
            .filter(|e| cursor.is_none_or(|c| e.id.as_str() > c));
        let lines: Vec<Envelope> = it.by_ref().take(limit).collect();
        let more = it.next().is_some();
        let next = more
            .then(|| lines.last().map(|e| e.id.as_str().to_owned()))
            .flatten();
        Ok(ExportPage {
            lines,
            next,
            redacted: false,
        })
    }
}
