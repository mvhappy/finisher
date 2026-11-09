//! Self-signed certificates for the remote listener (5B Decision 8): made from the identity
//! key with `rcgen` (`ring`), subject `CN=finisher`, SANs for every current route's host plus
//! `localhost`, `127.0.0.1` and `::1`, valid from a day before now (clock skew) for
//! `not_after_days`. A remake (other SANs, or 30 days before expiry) uses the same key, so the
//! SPKI pin never changes but by rotation. Napi-free: `tests/cert.rs` includes it by path; the
//! napi wrappers are in `tls.rs`. The key's PEM is never put in an error.

use fin_core::CoreError;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair,
    KeyUsagePurpose, SerialNumber, date_time_ymd,
};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::json;

/// The longest life a certificate may be given (the CA/Browser Forum's 397 days).
pub const MAX_DAYS: u32 = 397;
/// Names every self-signed certificate covers, whatever the routes.
pub const ALWAYS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

fn invalid(reason: &str) -> CoreError {
    CoreError::new(
        "INVALID_REQUEST",
        "a certificate cannot be made from this request",
    )
    .with_details(json!({ "reason": reason }))
}

/// `(year, month, day)` of `days` since 1970-01-01 (proleptic Gregorian; Howard Hinnant's
/// `civil_from_days`).
pub fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// One SAN as the certificate takes it: an IP address (brackets and an IPv6 zone dropped) or
/// a DNS name of ASCII letters, digits, dashes and dots (a trailing dot dropped); anything else
/// (spaces, non-ASCII host names) is `None` and left out, never failing the certificate.
pub fn san_of(raw: &str) -> Option<String> {
    let s = raw.trim().trim_start_matches('[').trim_end_matches(']');
    let s = s.split('%').next().unwrap_or("").to_ascii_lowercase();
    if s.parse::<std::net::IpAddr>().is_ok() {
        return Some(s);
    }
    let s = s.trim_end_matches('.');
    let ok = !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    ok.then(|| s.to_owned())
}

/// The SANs a certificate gets: `sans` that are valid names (`san_of`), de-duplicated, then `ALWAYS`.
pub fn sans_of(sans: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in sans
        .iter()
        .filter_map(|s| san_of(s))
        .chain(ALWAYS.iter().map(|s| (*s).to_owned()))
    {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// A self-signed certificate (PEM) for `identity_pem`'s key, valid `not_after_days` from
/// `now_days` (days since the epoch).
pub fn self_signed(
    identity_pem: &str,
    sans: &[String],
    not_after_days: u32,
    now_days: i64,
) -> Result<String, CoreError> {
    if not_after_days == 0 || not_after_days > MAX_DAYS {
        return Err(invalid("notAfterDays"));
    }
    let key = KeyPair::from_pem(identity_pem).map_err(|_| invalid("identityKey"))?;
    let mut params = CertificateParams::new(sans_of(sans)).map_err(|_| invalid("san"))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "finisher");
    params.distinguished_name = dn;
    let (y, m, d) = civil_from_days(now_days - 1);
    params.not_before = date_time_ymd(y, m, d);
    let (y, m, d) = civil_from_days(now_days + i64::from(not_after_days));
    params.not_after = date_time_ymd(y, m, d);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let mut serial = [0u8; 16];
    SystemRandom::new()
        .fill(&mut serial)
        .map_err(|_| CoreError::new("INTERNAL", "no random bytes"))?;
    serial[0] &= 0x7f;
    params.serial_number = Some(SerialNumber::from_slice(&serial));
    let cert = params.self_signed(&key).map_err(|_| invalid("sign"))?;
    Ok(cert.pem())
}
