//! Our [`Transport`] as `tough`'s, covering what `tough` leaves out (8A Task 1 gaps):
//!
//! - **Per-file caps by name** (Decision 15): a role whose length the snapshot lists is read
//!   up to that length by `tough`, so the cap is applied here before any byte is returned.
//! - **Roots saved as fetched, before the next is fetched** (gap 2): when `tough` asks for
//!   `<n+1>.root.json` it has verified `<n>`, so the adapter first saves the bytes of `<n>`
//!   it served (temp then rename) and, if `<n>` rotated the timestamp or snapshot keys,
//!   drops the trusted timestamp and snapshot in the same step (gap 4, fast-forward recovery).
//! - **Root-walk errors surface** (gap 3): `tough` ends the walk on any failure to fetch
//!   `<n+1>.root.json`; only "not found" may end it, so any other failure is recorded and
//!   fails the refresh.
//! - **Every served byte is kept**, so trusted files are stored exactly as fetched.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use tough::schema::{RoleType, Root, Signed};
use tough::{TransportErrorKind, TransportStream};
use url::Url;

use super::{TrustStore, TufError};
use crate::transport::{Transport, TransportError};

/// The base URL `tough` joins names onto; never dereferenced.
pub(crate) const BASE: &str = "fin-tuf://registry/";

pub(crate) const MIB: u64 = 1024 * 1024;
pub(crate) const MAX_ROOT: u64 = 4 * MIB;
pub(crate) const MAX_TARGETS: u64 = 4 * MIB;
pub(crate) const MAX_DELEGATED: u64 = 4 * MIB;
pub(crate) const MAX_TIMESTAMP: u64 = 16 * 1024;
pub(crate) const MAX_SNAPSHOT: u64 = MIB;
/// `tough` stops when the trusted root reaches `anchor + max_root_updates` before looking for
/// the next, so 33 lets one refresh trust at most 32 new roots (Decision 15).
pub(crate) const MAX_ROOT_UPDATES: u64 = 33;
pub(crate) const MAX_DELEGATIONS: usize = 500;

#[derive(Default)]
pub(crate) struct State {
    /// Repository path → bytes served.
    pub served: BTreeMap<String, Vec<u8>>,
    /// The first failure that must fail the refresh even if `tough` carries on.
    pub fatal: Option<TufError>,
    /// The cause of the last failed fetch, more precise than `tough`'s wrapping.
    pub last_error: Option<TufError>,
    /// The newest root saved to the store.
    pub saved_root: u64,
    /// Roots saved by this refresh.
    pub roots_added: u64,
    /// The delegated role of the last metadata fetch, to name it in errors.
    pub last_role: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Adapter {
    pub transport: Arc<dyn Transport>,
    pub store: TrustStore,
    pub state: Arc<Mutex<State>>,
}

impl fmt::Debug for Adapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Adapter")
    }
}

/// The cap and kind of a metadata file name.
fn metadata_cap(name: &str) -> (u64, Option<u64>, Option<String>) {
    if name == "timestamp.json" {
        return (MAX_TIMESTAMP, None, None);
    }
    let Some((v, rest)) = name.split_once('.') else {
        return (MAX_DELEGATED, None, None);
    };
    let version = v.parse::<u64>().ok();
    match rest {
        "root.json" => (MAX_ROOT, version, None),
        "snapshot.json" => (MAX_SNAPSHOT, None, None),
        "targets.json" => (MAX_TARGETS, None, None),
        other => (
            MAX_DELEGATED,
            None,
            Some(other.trim_end_matches(".json").to_string()),
        ),
    }
}

fn role_keys(root: &Signed<Root>, r: RoleType) -> Vec<String> {
    let mut v: Vec<String> = root
        .signed
        .roles
        .get(&r)
        .map(|k| k.keyids.iter().map(|id| hex::encode(id.as_ref())).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Whether `new` changes the timestamp or snapshot keys of `old` (TUF 1.0 §5.3.11).
pub(crate) fn rotates_online_keys(old: &[u8], new: &[u8]) -> bool {
    let (Ok(old), Ok(new)) = (
        serde_json::from_slice::<Signed<Root>>(old),
        serde_json::from_slice::<Signed<Root>>(new),
    ) else {
        return true;
    };
    [RoleType::Timestamp, RoleType::Snapshot]
        .iter()
        .any(|r| role_keys(&old, *r) != role_keys(&new, *r))
}

impl Adapter {
    /// Saves root `version` (served by this refresh) and applies fast-forward recovery.
    fn save_served_root(&self, st: &mut State, version: u64) -> Result<(), TufError> {
        let path = format!("metadata/{version}.root.json");
        let Some(bytes) = st.served.get(&path).cloned() else {
            return Err(TufError::invalid(
                "rootChain",
                format!("{path} was not served before {} was asked for", version + 1),
            ));
        };
        let prev = self.store.root(version - 1)?.ok_or_else(|| {
            TufError::invalid("rootChain", format!("root {} is not trusted", version - 1))
        })?;
        self.store.save_root(version, &bytes)?;
        if rotates_online_keys(&prev, &bytes) {
            let mut files = self.store.current()?;
            files.remove("timestamp.json");
            files.remove("snapshot.json");
            self.store.commit(&files)?;
        }
        st.saved_root = version;
        st.roots_added += 1;
        Ok(())
    }

    fn fetch_sync(&self, url: &Url) -> Result<Vec<u8>, tough::TransportError> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let path = url.path().trim_start_matches('/').to_string();
        let other = |st: &mut State, e: TufError| {
            st.last_error = Some(e);
            tough::TransportError::new(TransportErrorKind::Other, url.as_str())
        };
        let (cap, root_version, role) = match path.strip_prefix("metadata/") {
            Some(name) => metadata_cap(name),
            None => (u64::MAX, None, None),
        };
        if path.starts_with("metadata/") {
            st.last_role = role;
        }
        if let Some(n) = root_version {
            // Asked for <n>: <n-1> is verified, so save it before fetching <n>.
            if n >= 2
                && n - 1 > st.saved_root
                && let Err(e) = self.save_served_root(&mut st, n - 1)
            {
                st.fatal.get_or_insert(e.clone());
                return Err(other(&mut st, e));
            }
        }
        // Targets are fetched by `TrustedRepo::fetch_target`, never through `tough`.
        if !path.starts_with("metadata/") {
            let e = TufError::invalid("tuf", format!("unexpected fetch of {path}"));
            return Err(other(&mut st, e));
        }
        match self.transport.fetch(&path, cap) {
            Ok(bytes) => {
                st.served.insert(path, bytes.clone());
                st.last_error = None;
                Ok(bytes)
            }
            Err(TransportError::NotFound { .. }) if root_version.is_some() => Err(
                tough::TransportError::new(TransportErrorKind::FileNotFound, url.as_str()),
            ),
            Err(e) => {
                let err = TufError::transport(&e);
                if root_version.is_some() {
                    // `tough` would read this as "no newer root" and carry on.
                    st.fatal.get_or_insert(err.clone());
                }
                Err(other(&mut st, err))
            }
        }
    }
}

#[async_trait]
impl tough::Transport for Adapter {
    async fn fetch(&self, url: Url) -> Result<TransportStream, tough::TransportError> {
        let bytes = self.fetch_sync(&url)?;
        let once = futures::stream::once(async move { Ok(Bytes::from(bytes)) });
        Ok(Box::pin(once))
    }
}
