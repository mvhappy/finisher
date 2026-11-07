//! TUF failures as `{code, message, details}` (spec §13; exit 7 for every `TUF_*` and
//! `REGISTRY_UNTRUSTED`: a person must look).

use std::fmt;

use serde_json::{Map, Value, json};
use tough::schema::RoleType;

use crate::transport::TransportError;

/// - `REGISTRY_UNTRUSTED {expected, got}`: the root is not the pinned one;
/// - `REGISTRY_INVALID {reason, ...}`: metadata that does not parse, breaks a cap
///   (`tooLarge`, `tooManyRoots`, `tooManyDelegations`) or the registry layout;
/// - `REGISTRY_UNREACHABLE {path, reason}`: the transport failed;
/// - `TUF_SIGNATURE {role}` (no valid signature), `TUF_THRESHOLD {role, threshold, valid}`;
/// - `TUF_ROLLBACK {role, trusted, got}`, `TUF_EXPIRED {role}`;
/// - `TUF_TARGET_MISMATCH {target, reason: hash|length}`, `TUF_DELEGATION {role, target?}`;
/// - `PLUGIN_NOT_FOUND {target}`: no such target in the trusted metadata;
/// - `CLOCK_SUSPECT {now, lastRefreshAt}`; `IO_ERROR {op, path}`.
#[derive(Debug, Clone, PartialEq)]
pub struct TufError {
    pub code: &'static str,
    pub message: String,
    pub details: Map<String, Value>,
}

impl TufError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>, details: Value) -> TufError {
        let Value::Object(details) = details else {
            unreachable!("details are an object")
        };
        TufError {
            code,
            message: message.into(),
            details,
        }
    }

    pub(crate) fn invalid(reason: &str, message: impl Into<String>) -> TufError {
        TufError::new("REGISTRY_INVALID", message, json!({ "reason": reason }))
    }

    pub(crate) fn io(op: &str, path: &std::path::Path, e: impl fmt::Display) -> TufError {
        TufError::new(
            "IO_ERROR",
            format!("{op} {}: {e}", path.display()),
            json!({ "op": op, "path": path.display().to_string() }),
        )
    }

    pub(crate) fn rollback(role: &str, trusted: u64, got: u64) -> TufError {
        TufError::new(
            "TUF_ROLLBACK",
            format!("{role} version {got} is older than the trusted version {trusted}"),
            json!({ "role": role, "trusted": trusted, "got": got }),
        )
    }

    pub(crate) fn delegation(role: &str, message: impl Into<String>) -> TufError {
        TufError::new("TUF_DELEGATION", message, json!({ "role": role }))
    }

    pub(crate) fn target_mismatch(target: &str, reason: &str) -> TufError {
        TufError::new(
            "TUF_TARGET_MISMATCH",
            format!("target {target:?} does not match its signed {reason}"),
            json!({ "target": target, "reason": reason }),
        )
    }

    pub(crate) fn transport(e: &TransportError) -> TufError {
        match e {
            TransportError::TooLarge { path, max } => TufError::new(
                "REGISTRY_INVALID",
                format!("{path} is over its {max}-byte cap"),
                json!({ "reason": "tooLarge", "path": path, "max": max }),
            ),
            TransportError::BadPath { path } => TufError::new(
                "REGISTRY_INVALID",
                format!("{path:?} is not a repository path"),
                json!({ "reason": "badPath", "path": path }),
            ),
            TransportError::NotFound { path } => TufError::new(
                "REGISTRY_UNREACHABLE",
                format!("{path} is missing"),
                json!({ "path": path, "reason": "notFound" }),
            ),
            TransportError::Unreachable { path, reason } => TufError::new(
                "REGISTRY_UNREACHABLE",
                format!("{path}: {reason}"),
                json!({ "path": path, "reason": reason }),
            ),
        }
    }

    /// `details.reason`, when the code has one.
    pub fn reason(&self) -> Option<&str> {
        self.details.get("reason").and_then(Value::as_str)
    }

    /// `details.role`, when the code has one.
    pub fn role(&self) -> Option<&str> {
        self.details.get("role").and_then(Value::as_str)
    }
}

impl fmt::Display for TufError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TufError {}

pub(crate) fn role_name(r: RoleType) -> &'static str {
    match r {
        RoleType::Root => "root",
        RoleType::Snapshot => "snapshot",
        RoleType::Targets => "targets",
        RoleType::Timestamp => "timestamp",
        RoleType::DelegatedTargets => "delegated",
    }
}

pub(crate) fn verify_error(role: &str, e: &tough::schema::Error) -> TufError {
    use tough::schema::Error as S;
    match e {
        S::SignatureThreshold {
            threshold, valid, ..
        } if *valid > 0 => TufError::new(
            "TUF_THRESHOLD",
            format!("{role}: {valid} valid signatures, {threshold} needed"),
            json!({ "role": role, "threshold": threshold, "valid": valid }),
        ),
        S::SignatureThreshold { .. } => TufError::new(
            "TUF_SIGNATURE",
            format!("{role}: no valid signature"),
            json!({ "role": role }),
        ),
        S::UnmatchedPath { .. } | S::TargetNotFound { .. } | S::RoleNotFound { .. } => {
            TufError::delegation(role, format!("{role}: {e}"))
        }
        other => TufError::new(
            "TUF_SIGNATURE",
            format!("{role}: {other}"),
            json!({ "role": role }),
        ),
    }
}

/// The role a metadata file name holds: `2.acme.json` → `acme`, `timestamp.json` →
/// `timestamp`.
fn file_role(name: &str) -> String {
    let stem = name.trim_end_matches(".json");
    match stem.split_once('.') {
        Some((v, rest)) if v.bytes().all(|b| b.is_ascii_digit()) => rest.to_string(),
        _ => stem.to_string(),
    }
}

/// Maps a `tough` error to ours. Transport failures are classified by the adapter, which
/// records the precise cause; this is the fallback for everything else.
pub(crate) fn from_tough(e: &tough::error::Error) -> TufError {
    use tough::error::Error as E;
    match e {
        E::VerifyMetadata { role, source, .. } => verify_error(role_name(*role), source),
        E::VerifyRoleMetadata { role, source, .. } => verify_error(role, source),
        E::VerifyTrustedMetadata { source, .. } => verify_error("root", source),
        E::OlderMetadata {
            role,
            current_version,
            new_version,
            ..
        } => TufError::rollback(role_name(*role), *current_version, *new_version),
        E::OlderSnapshotInTimestamp {
            snapshot_new,
            snapshot_old,
            ..
        } => TufError::rollback("snapshot", *snapshot_old, *snapshot_new),
        E::SnapshotRoleRollback {
            role,
            old_role_version,
            new_role_version,
            ..
        } => TufError::rollback(
            role.trim_end_matches(".json"),
            *old_role_version,
            *new_role_version,
        ),
        E::SnapshotRoleMissing { role, .. } => TufError::new(
            "TUF_ROLLBACK",
            format!("{role} is no longer listed by the snapshot"),
            json!({ "role": role.trim_end_matches(".json"), "reason": "roleMissing" }),
        ),
        E::VersionMismatch {
            role,
            fetched,
            expected,
            ..
        } => TufError::new(
            "TUF_ROLLBACK",
            format!(
                "{} version {fetched} is not the version {expected} its parent names",
                role_name(*role)
            ),
            json!({ "role": role_name(*role), "trusted": expected, "got": fetched,
                    "reason": "mixAndMatch" }),
        ),
        E::ExpiredMetadata { role, .. } => TufError::new(
            "TUF_EXPIRED",
            format!("{} metadata is expired", role_name(*role)),
            json!({ "role": role_name(*role) }),
        ),
        E::HashMismatch { context, .. } => TufError::target_mismatch(context, "hash"),
        E::Transport { url, source, .. } => {
            let role = url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .map(file_role)
                .unwrap_or_default();
            let mut cur = std::error::Error::source(source);
            while let Some(c) = cur {
                if let Some(inner) = c.downcast_ref::<tough::error::Error>() {
                    let mut m = from_tough(inner);
                    if m.code == "TUF_TARGET_MISMATCH" {
                        m.details
                            .insert("target".into(), format!("{role}.json").into());
                    }
                    m.details.insert("role".into(), role.clone().into());
                    return m;
                }
                cur = c.source();
            }
            TufError::new(
                "REGISTRY_UNREACHABLE",
                format!("{e}"),
                json!({ "path": url.path().trim_start_matches('/'), "reason": "transport" }),
            )
        }
        E::MaxSizeExceeded { max_size, .. } => TufError::new(
            "REGISTRY_INVALID",
            format!("a metadata file is over its signed or capped size {max_size}"),
            json!({ "reason": "tooLarge", "max": max_size }),
        ),
        E::MaxUpdatesExceeded {
            max_root_updates, ..
        } => {
            let max = max_root_updates - 1;
            TufError::new(
                "REGISTRY_INVALID",
                format!("more than {max} new root versions in one refresh; refresh again"),
                json!({ "reason": "tooManyRoots", "max": max }),
            )
        }
        E::InvalidPath { source, .. } => TufError::delegation("targets", format!("{source}")),
        E::ParseMetadata { role, .. } => TufError::new(
            "REGISTRY_INVALID",
            format!("{e}"),
            json!({ "reason": "parse", "role": role_name(*role) }),
        ),
        E::ParseTrustedMetadata { .. } => TufError::new(
            "REGISTRY_INVALID",
            format!("{e}"),
            json!({ "reason": "parse", "role": "root" }),
        ),
        other => TufError::invalid("tuf", other.to_string()),
    }
}
