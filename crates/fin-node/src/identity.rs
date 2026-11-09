//! The environment's identity key (5B Decisions 5, 6): one P-256 key, PKCS#8 in a PEM, kept in
//! the vault as `identity`, and its SPKI pin, `base64url(SHA-256(SubjectPublicKeyInfo DER))`
//! without padding (43 characters). The self-signed certificates of the remote listener are
//! made from this key (Task 3), so the pin changes only when the key does (a rotation). The PEM
//! is never logged and never put in an error. Napi-free: `tests/vault.rs` includes it by path.

use base64::Engine as _;
use fin_core::CoreError;
use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::json;

const PEM_LABEL: &str = "PRIVATE KEY";
/// `SEQUENCE { SEQUENCE { id-ecPublicKey, prime256v1 }, BIT STRING (65 bytes) }` up to the point.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

fn not_a_key() -> CoreError {
    CoreError::new("SECRET_CORRUPT", "the server's identity key is damaged")
        .with_details(json!({ "reason": "identityNotAKey" }))
        .with_fix("The stored identity key cannot be read. Move the damaged secret aside to make a new one: paired devices then pair again.")
}

/// PEM armour of `der` under `label`, 64 characters a line.
pub fn pem(label: &str, der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// The DER of the first PEM block labelled `label` in `text`.
pub fn pem_block(text: &str, label: &str) -> Option<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = text.find(&begin)? + begin.len();
    let stop = start + text[start..].find(&end)?;
    let body: String = text[start..stop]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// A new P-256 key as a PKCS#8 PEM.
pub fn generate_pem() -> Result<String, CoreError> {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
        .map_err(|_| CoreError::new("INTERNAL", "no P-256 key could be made"))?;
    Ok(pem(PEM_LABEL, pkcs8.as_ref()))
}

/// The SubjectPublicKeyInfo DER of a P-256 PKCS#8 PEM.
pub fn spki_der(pem_text: &str) -> Result<Vec<u8>, CoreError> {
    let der = pem_block(pem_text, PEM_LABEL).ok_or_else(not_a_key)?;
    let pair =
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &der, &SystemRandom::new())
            .map_err(|_| not_a_key())?;
    let point = pair.public_key().as_ref();
    let mut spki = P256_SPKI_PREFIX.to_vec();
    spki.extend_from_slice(point);
    Ok(spki)
}

/// `base64url(SHA-256(spki))` without padding.
pub fn pin_of_spki(spki: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest(&SHA256, spki).as_ref())
}

/// The SPKI pin of a P-256 PKCS#8 PEM.
pub fn spki_pin(pem_text: &str) -> Result<String, CoreError> {
    Ok(pin_of_spki(&spki_der(pem_text)?))
}

/// The identity as loaded: its PEM, its pin, and whether it was made now.
#[derive(Debug, Clone)]
pub struct Identity {
    pub pem: String,
    pub spki: String,
    pub created: bool,
}

/// `IDENTITY_MISMATCH`: the vault holds a key other than the one `control.db` recorded.
pub fn mismatch() -> CoreError {
    CoreError::new("IDENTITY_MISMATCH", "the server's identity key is not the one its data was made with")
        .with_details(json!({ "reason": "spki" }))
        .with_fix(
            "This server data is being used with the secrets of another server data. Start the server with the secrets folder or keychain \
             entries that belong to this data.",
        )
}

/// The vault's identity as `get` reads it, checked against `expected` (`IDENTITY_MISMATCH`);
/// `None` when there is none. Making and storing a new key is the caller's, in the order that
/// keeps `control.db` and the vault agreeing (`finisherd` `secrets/vault.ts`).
pub fn load(
    get: impl FnOnce() -> Result<Option<Vec<u8>>, CoreError>,
    expected: Option<&str>,
) -> Result<Option<Identity>, CoreError> {
    let Some(bytes) = get()? else { return Ok(None) };
    let pem = String::from_utf8(bytes).map_err(|_| not_a_key())?;
    let spki = spki_pin(&pem)?;
    if expected.is_some_and(|e| e != spki) {
        return Err(mismatch());
    }
    Ok(Some(Identity {
        pem,
        spki,
        created: false,
    }))
}

/// A new identity, not stored anywhere yet.
pub fn generate() -> Result<Identity, CoreError> {
    let pem = generate_pem()?;
    let spki = spki_pin(&pem)?;
    Ok(Identity {
        pem,
        spki,
        created: true,
    })
}
