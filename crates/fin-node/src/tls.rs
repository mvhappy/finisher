//! 5B Task 3 as napi: `selfSignedCert` (`cert.rs`), `qrSvg` and `qrText` (`qr.rs`). Checking
//! an owner's certificate files (files mode, Decision 9) is `finisherd`'s, through Node's
//! `X509Certificate`, which reads every key type the files may hold.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::{cert, guarded, qr, reject};

/// A self-signed certificate (PEM) from the identity key, for `sans` and `not_after_days` (≤ 397).
#[napi(catch_unwind)]
pub fn self_signed_cert(
    identity_pem: String,
    sans: Vec<String>,
    not_after_days: u32,
) -> Result<String> {
    guarded(
        || {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            cert::self_signed(&identity_pem, &sans, not_after_days, (now / 86_400) as i64)
                .map_err(reject)
        },
        || {},
    )
}

/// The QR code of `text` as an SVG document.
#[napi(catch_unwind)]
pub fn qr_svg(text: String) -> Result<String> {
    guarded(|| qr::svg(&text).map_err(reject), || {})
}

/// The QR code of `text` as Unicode half blocks.
#[napi(catch_unwind)]
pub fn qr_text(text: String) -> Result<String> {
    guarded(|| qr::text(&text).map_err(reject), || {})
}
