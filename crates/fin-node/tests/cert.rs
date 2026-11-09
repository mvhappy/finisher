//! Self-signed certificates and QR codes (5B Task 3, Decisions 6, 8, 17). The crate is a napi
//! `cdylib`, so this includes the napi-free modules by path. The Node TLS round trip and the
//! files-mode checks are in `apps/finisherd/test/cert.test.ts`.

#![allow(dead_code)]

#[path = "../src/cert.rs"]
mod cert;
#[path = "../src/identity.rs"]
mod identity;
#[path = "../src/qr.rs"]
mod qr;

const DAY0: i64 = 20_400; // 2025-11-08

fn der_of(pem: &str) -> Vec<u8> {
    identity::pem_block(pem, "CERTIFICATE").expect("a CERTIFICATE block")
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn cert_spki_is_identity_spki() {
    let key = identity::generate_pem().unwrap();
    let pem = cert::self_signed(&key, &["127.0.0.2".into()], 397, DAY0).unwrap();
    assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
    // The certificate carries the identity key's SubjectPublicKeyInfo, so its pin is the identity's.
    assert!(contains(&der_of(&pem), &identity::spki_der(&key).unwrap()));
    // The key itself is never in it.
    assert!(!pem.contains("PRIVATE KEY"));
}

#[test]
fn remade_cert_keeps_pin() {
    let key = identity::generate_pem().unwrap();
    let spki = identity::spki_der(&key).unwrap();
    let a = cert::self_signed(&key, &["127.0.0.2".into()], 397, DAY0).unwrap();
    let b =
        cert::self_signed(&key, &["127.0.0.3".into(), "studio".into()], 397, DAY0 + 30).unwrap();
    assert_ne!(a, b);
    assert!(contains(&der_of(&a), &spki));
    assert!(contains(&der_of(&b), &spki));
    // Another key: another SPKI.
    let other = identity::spki_der(&identity::generate_pem().unwrap()).unwrap();
    assert!(!contains(&der_of(&b), &other));
}

#[test]
fn sans_cover_ips_and_names() {
    let sans = cert::sans_of(&[
        "127.0.0.2".into(),
        "Studio-Box".into(),
        "[fd7a:115c:a1e0::1]".into(),
        "localhost".into(),
        " ".into(),
        "bad host".into(),
        "été".into(),
        "fe80::1%eth0".into(),
    ]);
    assert_eq!(
        sans,
        vec![
            "127.0.0.2",
            "studio-box",
            "fd7a:115c:a1e0::1",
            "localhost",
            "fe80::1",
            "127.0.0.1",
            "::1"
        ]
    );
    let key = identity::generate_pem().unwrap();
    let der = der_of(
        &cert::self_signed(&key, &["127.0.0.2".into(), "studio-box".into()], 30, DAY0).unwrap(),
    );
    // IP SANs are 4 or 16 raw bytes, DNS SANs their text.
    assert!(contains(&der, &[127, 0, 0, 2]));
    assert!(contains(&der, &[127, 0, 0, 1]));
    assert!(contains(&der, b"studio-box"));
    assert!(contains(&der, b"localhost"));
    assert!(contains(&der, b"finisher"));
}

#[test]
fn lifetimes_are_bounded() {
    let key = identity::generate_pem().unwrap();
    assert_eq!(
        cert::self_signed(&key, &[], 0, DAY0).unwrap_err().code,
        "INVALID_REQUEST"
    );
    assert_eq!(
        cert::self_signed(&key, &[], 398, DAY0).unwrap_err().code,
        "INVALID_REQUEST"
    );
    assert_eq!(
        cert::self_signed("not a key", &[], 30, DAY0)
            .unwrap_err()
            .code,
        "INVALID_REQUEST"
    );
    // A refused key never echoes its text.
    let e = cert::self_signed(
        "-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----\n",
        &[],
        30,
        DAY0,
    )
    .unwrap_err();
    assert!(!serde_json::to_string(&e).unwrap().contains("secret"));
}

#[test]
fn civil_dates() {
    assert_eq!(cert::civil_from_days(0), (1970, 1, 1));
    assert_eq!(cert::civil_from_days(DAY0), (2025, 11, 8));
    assert_eq!(cert::civil_from_days(DAY0 + 397), (2026, 12, 10));
    assert_eq!(cert::civil_from_days(11_016), (2000, 2, 29));
}

/// A pairing link as spec §5.2 builds it, with `routes` routes in `r=`.
fn link(routes: usize) -> String {
    use base64::Engine as _;
    let rs: Vec<String> = (0..routes)
        .map(|i| {
            format!(r#"{{"kind":"lan","url":"https://studio-{i}.local:7311","pin":"identity"}}"#)
        })
        .collect();
    let r = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("[{}]", rs.join(",")));
    format!(
        "finisher://pair?v=1&env=01envenvenvenvenvenvenvenv&spki={}&r={r}#01offerofferofferofferoffe.{}",
        "A".repeat(43),
        "s".repeat(43)
    )
}

/// Reads `qr::text`'s half blocks back into pixels (blank = a dark module) and decodes them.
fn decode_text(text: &str) -> String {
    let rows: Vec<Vec<char>> = text.lines().map(|l| l.chars().collect()).collect();
    let width = rows[0].len();
    let height = rows.len() * 2;
    let scale = 4;
    let lit = |c: char, top: bool| match c {
        '█' => true,
        '▀' => top,
        '▄' => !top,
        _ => false,
    };
    let mut img =
        rqrr::PreparedImage::prepare_from_greyscale(width * scale, height * scale, |x, y| {
            let (col, row) = (x / scale, y / scale);
            let c = rows[row / 2].get(col).copied().unwrap_or(' ');
            if lit(c, row % 2 == 0) { 255 } else { 0 }
        });
    let grids = img.detect_grids();
    assert_eq!(grids.len(), 1);
    grids[0].decode().unwrap().1
}

#[test]
fn qr_decodes_to_link() {
    // The rendering the console prints decodes back to the link, for a link with three routes.
    let three = link(3);
    assert_eq!(decode_text(&qr::text(&three).unwrap()), three);
    let svg = qr::svg(&three).unwrap();
    assert!(svg.starts_with("<?xml") && svg.contains("<svg"));
    // Its size: two modules a character plus the quiet zone. Tasks 8 and 12 keep `r=` short
    // enough for an 80-column terminal (a route cap); a long one still renders.
    let code =
        qrcode::QrCode::with_error_correction_level(three.as_bytes(), qrcode::EcLevel::M).unwrap();
    assert_eq!(
        qr::text(&three).unwrap().lines().count(),
        (code.width() + 8).div_ceil(2)
    );
    assert!(qr::text(&link(7)).is_ok());
}

#[test]
fn qr_refuses_what_does_not_fit() {
    assert_eq!(
        qr::svg(&"x".repeat(5000)).unwrap_err().code,
        "INVALID_REQUEST"
    );
}
