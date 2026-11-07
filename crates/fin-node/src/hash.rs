//! BLAKE3 for `finisherd` (review M3): the agent runtime hashes a model request's exact bytes
//! (`bodyHash`), its system prompt and its tool definitions with the same `blake3` crate the
//! rest of Finisher uses, rather than a copy in TypeScript. These hashes are unkeyed; the
//! journal keys them when it records an `agent.turn` (`fin_core::events::wire_agent`).

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::reject;

/// The BLAKE3 of `data`: 64 lower-case hex digits.
#[napi(catch_unwind)]
pub fn blake3_hex(data: Buffer) -> String {
    blake3::hash(&data).to_hex().to_string()
}

/// The BLAKE3 of `data` keyed with the 32-byte `key`: 64 lower-case hex digits. Not the
/// journal's install key (that never leaves the journal); for callers with a key of their own.
#[napi(catch_unwind)]
pub fn blake3_keyed_hex(key: Buffer, data: Buffer) -> Result<String> {
    let key: [u8; 32] = key.as_ref().try_into().map_err(|_| {
        reject(fin_core::CoreError::new(
            "INTERNAL",
            format!("a BLAKE3 key is 32 bytes, not {}", key.len()),
        ))
    })?;
    Ok(blake3::keyed_hash(&key, &data).to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_the_crate() {
        let data = Buffer::from(b"abc".to_vec());
        assert_eq!(blake3_hex(data), blake3::hash(b"abc").to_hex().to_string());
        let key = Buffer::from(vec![7u8; 32]);
        let data = Buffer::from(b"abc".to_vec());
        assert_eq!(
            blake3_keyed_hex(key, data).unwrap(),
            blake3::keyed_hash(&[7; 32], b"abc").to_hex().to_string()
        );
        let short = Buffer::from(vec![7u8; 16]);
        assert!(blake3_keyed_hex(short, Buffer::from(b"abc".to_vec())).is_err());
    }
}
