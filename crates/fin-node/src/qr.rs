//! Pairing QR codes (5B Decision 17): the link as an SVG (for the app and a page) and as
//! Unicode half blocks (the console's `pair`), error correction level M, with the quiet zone.
//! Napi-free: `tests/cert.rs` includes it by path; the napi wrappers are in `tls.rs`.

use fin_core::CoreError;
use qrcode::render::{svg, unicode};
use qrcode::{EcLevel, QrCode};
use serde_json::json;

fn code(text: &str) -> Result<QrCode, CoreError> {
    QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).map_err(|_| {
        CoreError::new("INVALID_REQUEST", "this text does not fit a QR code")
            .with_details(json!({ "length": text.len() }))
    })
}

/// The QR code of `text` as an SVG document (black on white).
pub fn svg(text: &str) -> Result<String, CoreError> {
    Ok(code(text)?
        .render::<svg::Color<'_>>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .build())
}

/// The QR code of `text` as Unicode half blocks, two modules per character. Dark modules are
/// printed as blanks and light ones as blocks, so on a terminal with a dark background (the
/// common case) the code reads dark on light; on a light theme it prints inverted, which most
/// phone scanners still read.
pub fn text(text: &str) -> Result<String, CoreError> {
    Ok(code(text)?
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}
