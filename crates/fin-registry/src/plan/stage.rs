//! Staging a plan's source into `staging/.tmp-<planId>/pkg/`: a registry target is copied in
//! with its signed length as a cap and its SHA-256 checked again, then unpacked by the strict
//! reader; a bundled tree must match its compiled-in hash and is packed and unpacked through
//! the same reader, so both reach the store by one path.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use fin_plugin_api::{MANIFEST_FILE, Manifest};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::error::RegistryError;
use crate::package::{Limits, Unpacked, hex, pack, tree_hash, unpack};
use crate::target::VerifiedTarget;

pub(super) fn registry(
    t: &VerifiedTarget,
    tmp: &Path,
    pkg: &Path,
) -> Result<Unpacked, RegistryError> {
    let copy = tmp.join("target.finpkg");
    copy_verified(&t.archive, &copy, t.length, &t.sha256)?;
    let out = unpack(
        &copy,
        pkg,
        |m: &Manifest| m.executables(),
        &Limits::default(),
    );
    let _ = std::fs::remove_file(&copy);
    Ok(out?)
}

/// Copies at most `length + 1` bytes of `from` to `to`; anything but exactly `length` bytes
/// with SHA-256 `sha256` is `TUF_TARGET_MISMATCH {reason: length | hash}`.
fn copy_verified(from: &Path, to: &Path, length: u64, sha256: &str) -> Result<(), RegistryError> {
    let src = File::open(from).map_err(|e| RegistryError::io("open", from, e))?;
    let mut dst = File::create(to).map_err(|e| RegistryError::io("create", to, e))?;
    let mut reader = src.take(length.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut got = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| RegistryError::io("read", from, e))?;
        if n == 0 {
            break;
        }
        got += n as u64;
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n])
            .map_err(|e| RegistryError::io("write", to, e))?;
    }
    dst.sync_all()
        .map_err(|e| RegistryError::io("sync", to, e))?;
    let mismatch = |reason: &str, message: String| {
        RegistryError::new("TUF_TARGET_MISMATCH", message, json!({ "reason": reason }))
    };
    if got != length {
        return Err(mismatch(
            "length",
            format!("the target has {got} bytes or more; signed {length}"),
        ));
    }
    let have = hex(&hasher.finalize());
    if have != sha256 {
        return Err(mismatch(
            "hash",
            "the target's SHA-256 is not the signed one".into(),
        ));
    }
    Ok(())
}

pub(super) fn bundled(
    dir: &Path,
    expect: &str,
    tmp: &Path,
    pkg: &Path,
) -> Result<Unpacked, RegistryError> {
    let have = tree_hash(dir)?;
    if have != expect {
        return Err(RegistryError::new(
            "PLUGIN_BUNDLED_MISMATCH",
            format!(
                "the bundled tree {} does not match the hash built into the app",
                dir.display()
            ),
            json!({ "reason": "treeHash" }),
        ));
    }
    // The executables the manifest names; an unreadable manifest fails validation later.
    let executables = std::fs::read(dir.join(MANIFEST_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
        .map(|m| m.executables())
        .unwrap_or_default();
    let archive = tmp.join("bundled.finpkg");
    pack(dir, &executables, &archive)?;
    let out = unpack(
        &archive,
        pkg,
        |m: &Manifest| m.executables(),
        &Limits::default(),
    );
    let _ = std::fs::remove_file(&archive);
    Ok(out?)
}
