//! The pattern-checked `meta` strings of the event model (plan D, Decisions 4–7 and 20): each
//! is a private `String` that only a checking constructor or a checking `Deserialize` builds,
//! with its pattern and `x-fin-class: meta` in the schema. Re-exported by `class.rs`.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::class::Name;
use super::names;

macro_rules! pattern_string {
    ($(#[$doc:meta])* $ty:ident, $pattern:expr, $check:path) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $ty(String);

        impl $ty {
            pub const PATTERN: &'static str = $pattern;

            /// `Some` when `s` matches the pattern.
            pub fn parse(s: &str) -> Option<$ty> {
                $check(s).then(|| $ty(s.to_string()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($ty), self.0)
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        // The error names the type, never the value: a refused value may be raw.
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                if $check(&s) {
                    Ok($ty(s))
                } else {
                    Err(D::Error::custom(concat!("not a valid ", stringify!($ty))))
                }
            }
        }

        impl JsonSchema for $ty {
            fn schema_name() -> Cow<'static, str> {
                stringify!($ty).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({ "type": "string", "pattern": $pattern, "x-fin-class": "meta" })
            }
        }
    };
}

fn hex_lower(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

fn is_hashed(s: &str) -> bool {
    s.strip_prefix("k1:").is_some_and(|h| hex_lower(h, 32))
}

fn is_blob_hash(s: &str) -> bool {
    s.strip_prefix("kb:").is_some_and(|h| hex_lower(h, 64))
}

/// A canonical ULID: 26 Crockford base-32 characters, upper case, the first `0`–`7` (128 bits).
fn is_ulid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 26
        && matches!(b[0], b'0'..=b'7')
        && b.iter()
            .all(|c| matches!(c, b'0'..=b'9' | b'A'..=b'H' | b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z'))
}

fn is_id_ref(s: &str) -> bool {
    is_ulid(s) || is_hashed(s)
}

/// A Finisher project id: a ULID in lower case, as `fin-store` makes them.
fn is_project_id(s: &str) -> bool {
    s.len() == 26 && is_ulid(&s.to_ascii_uppercase()) && !s.bytes().any(|c| c.is_ascii_uppercase())
}

/// `main` or `br` and any number of digits, as `fin-store` accepts them (`valid_branch`).
fn is_branch_id(s: &str) -> bool {
    s == "main"
        || s.strip_prefix("br")
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
}

/// A registry kind name: `domain.verb`, each segment lower camel case.
fn is_kind_name(s: &str) -> bool {
    let seg = |p: &str| {
        p.starts_with(|c: char| c.is_ascii_lowercase())
            && p.bytes().all(|c| c.is_ascii_alphanumeric())
    };
    s.len() <= 64 && s.split_once('.').is_some_and(|(d, v)| seg(d) && seg(v))
}

fn is_blake3_hex(s: &str) -> bool {
    s.strip_prefix("blake3:").is_some_and(|h| hex_lower(h, 64))
}

fn is_code(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=64).contains(&b.len())
        && b[0].is_ascii_uppercase()
        && b[1..]
            .iter()
            .all(|c| matches!(c, b'A'..=b'Z' | b'0'..=b'9' | b'_'))
}

fn digits(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_digit())
}

fn is_version(s: &str) -> bool {
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| digits(p, 5))
        && pre.is_none_or(|p| {
            (1..=32).contains(&p.len())
                && p.bytes()
                    .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'z' | b'.'))
        })
}

/// `(crates|apps)/<path>:<line>`, the path in `[a-z0-9_./-]{1,160}` with no empty, `.`,
/// `..` or `node_modules` segment (a dependency inside an app is third-party).
fn is_src_loc(s: &str) -> bool {
    let Some((file, line)) = s.rsplit_once(':') else {
        return false;
    };
    let Some(rest) = file
        .strip_prefix("crates/")
        .or_else(|| file.strip_prefix("apps/"))
    else {
        return false;
    };
    digits(line, 7)
        && (1..=160).contains(&rest.len())
        && rest
            .bytes()
            .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'/' | b'-'))
        && rest
            .split('/')
            .all(|seg| !matches!(seg, "" | "." | ".." | "node_modules"))
}

pattern_string!(
    /// A keyed hash (Decision 6): `k1:` and the first 128 bits of BLAKE3 keyed with the
    /// per-install key, in lower-case hex. `fin-core` has no key: the journal makes these
    /// ([`Hashed::from_bytes`]).
    Hashed,
    r"^k1:[0-9a-f]{32}$",
    is_hashed
);

impl Hashed {
    /// `k1:` and the hex of `digest` (the first 16 bytes of the keyed hash).
    pub fn from_bytes(digest: &[u8; 16]) -> Hashed {
        Hashed(format!("k1:{}", hex(digest)))
    }

    /// The keyed hash of `data`; `key` is the journal's keyed BLAKE3, truncated to 16 bytes.
    pub fn keyed(data: &[u8], key: &dyn Fn(&[u8]) -> [u8; 16]) -> Hashed {
        Hashed::from_bytes(&key(data))
    }
}

pattern_string!(
    /// A blob's content address (Decision 20): `kb:` and the full 256-bit keyed BLAKE3 of its
    /// bytes, in lower-case hex.
    BlobHash,
    r"^kb:[0-9a-f]{64}$",
    is_blob_hash
);

impl BlobHash {
    pub fn from_bytes(digest: &[u8; 32]) -> BlobHash {
        BlobHash(format!("kb:{}", hex(digest)))
    }

    /// The hex part: the blob's file name under `events/blobs/`.
    pub fn hex(&self) -> &str {
        &self.0[3..]
    }
}

pattern_string!(
    /// An error code (`CONFLICT`, `EVENT_INVALID`, ...).
    Code,
    r"^[A-Z][A-Z0-9_]{1,63}$",
    is_code
);

pattern_string!(
    /// An id (Decision 7): a canonical ULID, stored raw, or a keyed hash (`k1:…`) of anything
    /// else. [`IdRef::classify`] keeps a ULID and asks the caller, who has the key, to hash
    /// the rest; an idempotency key or a session name may carry a user or host name, and a
    /// lower-case ULID is not canonical, so both are hashed.
    IdRef,
    r"^([0-7][0-9A-HJKMNP-TV-Z]{25}|k1:[0-9a-f]{32})$",
    is_id_ref
);

/// [`IdRef::classify`] refused a raw id: the caller must hash it ([`IdRef::from_hashed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeedsHash;

impl IdRef {
    /// A canonical ULID is kept; anything else needs a keyed hash.
    pub fn classify(raw: &str) -> Result<IdRef, NeedsHash> {
        if is_ulid(raw) {
            Ok(IdRef(raw.to_string()))
        } else {
            Err(NeedsHash)
        }
    }

    pub fn from_hashed(h: Hashed) -> IdRef {
        IdRef(h.0)
    }

    pub fn is_hashed(&self) -> bool {
        self.0.starts_with("k1:")
    }

    /// [`IdRef::classify`], hashing with `key` what it refuses.
    pub fn keyed(raw: &str, key: &dyn Fn(&[u8]) -> [u8; 16]) -> IdRef {
        IdRef::classify(raw)
            .unwrap_or_else(|_| IdRef::from_hashed(Hashed::keyed(raw.as_bytes(), key)))
    }
}

pattern_string!(
    /// A Finisher project id (`ctx.project`): a lower-case ULID, as `fin-store` makes them.
    /// Generated by the store, never typed, so it is stored raw and compared with the
    /// project flags (Decision 13).
    ProjectId,
    r"^[0-7][0-9a-hjkmnp-tv-z]{25}$",
    is_project_id
);

pattern_string!(
    /// A branch id (`ctx.branch`): `main` or `br<n>`, allocated by the store. Branch names,
    /// which a user types, are never stored.
    BranchId,
    r"^(main|br[0-9]+)$",
    is_branch_id
);

pattern_string!(
    /// A canonical ULID (an envelope's `id` and `session`): 26 Crockford base-32 characters,
    /// upper case, the first `0`–`7`.
    Ulid,
    r"^[0-7][0-9A-HJKMNP-TV-Z]{25}$",
    is_ulid
);

impl Ulid {
    /// The canonical text of a 128-bit ULID (48 bits of milliseconds, 80 random), which the
    /// emitter makes.
    pub fn from_u128(n: u128) -> Ulid {
        const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let text = (0..26)
            .rev()
            .map(|i| CROCKFORD[((n >> (5 * i)) & 31) as usize] as char)
            .collect();
        Ulid(text)
    }
}

impl From<Ulid> for IdRef {
    fn from(u: Ulid) -> IdRef {
        IdRef(u.0)
    }
}

pattern_string!(
    /// A registry kind name (an envelope's `kind`): `domain.verb`, each segment lower camel
    /// case (`log.commit`, `agent.toolCall`). Whether the registry has it at the entry's `kv`
    /// is [`super::kinds::require_kind`]'s to say.
    KindName,
    r"^[a-z][A-Za-z0-9]*\.[a-z][A-Za-z0-9]*$",
    is_kind_name
);

impl KindName {
    /// The name of a registry kind.
    pub fn of(spec: &super::kinds::KindSpec) -> KindName {
        KindName(spec.name.to_string())
    }
}

pattern_string!(
    /// `fin`'s unkeyed `argsHash` as it arrives on the wire (`blake3:<64 hex>`, Decision 6).
    /// Wire-only: never stored. [`super::wire`] keys it into a [`Hashed`].
    Blake3Hex,
    r"^blake3:[0-9a-f]{64}$",
    is_blake3_hex
);

/// The longest raw id a wire payload may carry, in bytes.
pub const MAX_RAW_ID: usize = 256;

/// A raw id as it arrives on the wire (a txId that may be an idempotency key, Decision 7).
/// Wire-only: never stored. [`super::wire`] turns it into an [`IdRef`] (a ULID kept, anything
/// else keyed). At most [`MAX_RAW_ID`] bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct RawId(String);

impl RawId {
    pub fn new(s: &str) -> Option<RawId> {
        (s.len() <= MAX_RAW_ID).then(|| RawId(s.to_string()))
    }

    /// The [`IdRef`] of the id: kept when it is a canonical ULID, else keyed with `key`.
    pub fn into_id_ref(self, key: &dyn Fn(&[u8]) -> [u8; 16]) -> IdRef {
        IdRef::keyed(&self.0, key)
    }
}

// The value is not shown: it may be raw.
impl fmt::Debug for RawId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RawId({} bytes)", self.0.len())
    }
}

impl Serialize for RawId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RawId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        RawId::new(&s).ok_or_else(|| D::Error::custom("a raw id is too long"))
    }
}

pattern_string!(
    /// A release version from the build (`0.4.0`, `1.2.3-rc.1`).
    Version,
    r"^[0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,5}(-[0-9a-z.]{1,32})?$",
    is_version
);

pattern_string!(
    /// A repo-relative source location, `crates/…:<line>` or `apps/…:<line>` (Decision 26),
    /// with no empty, `.`, `..` or `node_modules` segment. A location outside our tree (the cargo registry,
    /// `node_modules`, an absolute path) has none: the payload says `thirdParty`.
    SrcLoc,
    r"^(crates|apps)/[a-z0-9_./-]{1,160}:[0-9]{1,7}$",
    is_src_loc
);

impl SrcLoc {
    /// A location from a file and a line, with `\` read as `/` (Windows paths of
    /// `Location::file()`). `None` outside our tree.
    pub fn from_file_line(file: &str, line: u32) -> Option<SrcLoc> {
        SrcLoc::parse(&format!("{}:{line}", file.replace('\\', "/")))
    }
}

/// The root a [`PathRef`] is relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PathRoot {
    Project,
    Library,
    Workspace,
    ServerData,
    Other,
}

/// A path, never stored raw (Decision 5): its root, its extension if it is a known media
/// extension ([`names::MEDIA_EXTS`], lower-cased; absent otherwise), and the keyed hash of the
/// path. No field holds the path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PathRef {
    pub root: PathRoot,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "names::media_ext_opt"
    )]
    #[schemars(with = "Name")]
    pub ext: Option<Name>,
    pub hash: Hashed,
}

impl PathRef {
    /// `ext` is the raw extension (without the dot), kept only when it is in `MEDIA_EXTS`.
    pub fn new(root: PathRoot, ext: Option<&str>, hash: Hashed) -> PathRef {
        let ext = ext.and_then(|e| names::MEDIA_EXTS.get(&e.to_ascii_lowercase()));
        PathRef { root, ext, hash }
    }
}

/// Large content, stored as a blob (Decision 20): its content address, its length and its
/// media type ([`names::MEDIA_TYPES`]). Allowed only inside a [`super::class::Content`] field
/// (`x-fin-blob` marks it for the schema checks and [`super::redact::blob_refs`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("x-fin-blob" = true))]
pub struct BlobRef {
    pub hash: BlobHash,
    pub bytes: u64,
    #[serde(deserialize_with = "names::media_type")]
    pub media_type: Name,
}
