//! Fonts on the server side (4C Decisions 30–34): the pinned font catalog, the shared
//! content-addressed cache `<server-data>/fonts/<sha256>`, and the fetcher that fills it.
//!
//! - [`FontCatalog`]: a versioned JSON manifest (`catalog/catalog.json`) whose files are named
//!   by SHA-256 and fetched from immutable `https` URLs. [`FontCatalog::validate`] is the
//!   licence gate's Rust half; `scripts/check-font-catalog.mjs` is the other, and both read
//!   `catalog/rules.json`.
//! - [`FontCache`]: verified bytes by SHA-256, written temp-then-rename without replacing,
//!   safe across processes. It is the server's [`fin_text::FontBytes`].
//! - [`Fetcher`]: downloads catalog files into the cache over `ureq` + `rustls` (`ring`, the
//!   OS trust store), with proxies, resumable `Range` requests, backoff and mirrors.
//!   Proxies come from `ALL_PROXY`, then `HTTPS_PROXY`, then `HTTP_PROXY` (`NO_PROXY`
//!   bypasses them), else the Windows system proxy when it is one `host:port`; per-protocol
//!   Windows settings, PAC files and WPAD are not read (set `HTTPS_PROXY` instead).
//!
//! Nothing here is ever written to a record, a diff or a snapshot.

mod archive;
mod cache;
mod catalog;
mod download;
mod fetch;
mod http;
mod lock;
mod slot;
mod validate;

use fin_core::CoreError;
use serde_json::json;
use sha2::{Digest, Sha256};

pub use cache::{FontCache, STRAY_AFTER};
pub use catalog::{
    ArchiveFormat, ArchiveMember, CatalogEntry, CatalogFace, CatalogFile, CatalogSource,
    CatalogStyle, FontCatalog, FontFormat, Licence,
};
pub use fetch::{FetchOptions, Fetcher, FileState};
pub use slot::MAX_DOWNLOADS;
pub use validate::{
    MAX_FILE_SIZE, RULES_JSON, is_allowed_source_host, is_immutable_url, is_sha256,
};

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

pub(crate) fn not_in_catalog(sha256: &str) -> CoreError {
    CoreError::new(
        "FONT_NOT_IN_CATALOG",
        format!("no font catalog file has SHA-256 {sha256}"),
    )
    .with_details(json!({ "sha256": sha256 }))
    .with_fix("fin fonts ls --catalog")
}
