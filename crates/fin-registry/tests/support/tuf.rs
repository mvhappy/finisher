//! A registry built with the editor, a client trusting it, and packages to publish.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use fin_registry::editor::{RepoBuilder, SigningKey};
use fin_registry::package::pack;
use fin_registry::transport::{FsTransport, Transport};
use fin_registry::tuf::{
    Clock, FixedClock, Policy, RefreshReport, TrustStore, TrustedRepo, TufError, sha256_hex,
};
use serde_json::json;

use super::write_tree;

/// Packs a content pack `id`@`version` (one caption style) into `out_dir`.
pub fn content_pkg(out_dir: &Path, id: &str, version: &str) -> PathBuf {
    let src = out_dir.join(format!("src-{}-{version}", id.replace(['@', '/'], "_")));
    let manifest = serde_json::to_vec(&json!({
        "manifestVersion": 1,
        "id": id,
        "version": version,
        "kind": "content",
        "hostApi": "^1.0",
        "license": "MIT",
        "publisher": { "name": "Test" },
        "contributes": { "captionStyles": [{ "name": "bold", "file": "styles/bold.json" }] },
    }))
    .unwrap();
    let style = format!("{{\"weight\": 700, \"v\": \"{version}\"}}");
    write_tree(
        &src,
        &[
            ("finisher-plugin.json", &manifest),
            ("styles/bold.json", style.as_bytes()),
        ],
    );
    let out = out_dir.join(format!("{}-{version}.finpkg", id.replace(['@', '/'], "_")));
    pack(&src, &[], &out).unwrap();
    out
}

/// A registry in a temp dir with one root key, a client store beside it, and a frozen clock.
pub struct Reg {
    pub tmp: tempfile::TempDir,
    pub repo: PathBuf,
    pub keys: PathBuf,
    pub trust: PathBuf,
    pub b: RepoBuilder,
    pub root_key: SigningKey,
    pub clock: Arc<FixedClock>,
}

impl Reg {
    pub fn new(max_stale_days: Option<u16>) -> Reg {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("registry");
        let keys = tmp.path().join("keys");
        let trust = tmp.path().join("server/plugins/trust");
        let root_key = SigningKey::generate();
        let clock = Arc::new(FixedClock::new(SystemTime::now()));
        let b = RepoBuilder::init_with(
            &repo,
            &keys,
            std::slice::from_ref(&root_key),
            1,
            max_stale_days,
            clock.clone(),
        )
        .unwrap();
        Reg {
            tmp,
            repo,
            keys,
            trust,
            b,
            root_key,
            clock,
        }
    }

    pub fn transport(&self) -> Arc<dyn Transport> {
        Arc::new(FsTransport::new(&self.repo))
    }

    pub fn pkg(&self, id: &str, version: &str) -> PathBuf {
        let dir = self.tmp.path().join("pkgs");
        std::fs::create_dir_all(&dir).unwrap();
        content_pkg(&dir, id, version)
    }

    /// A publisher key delegated `publisher/*`.
    pub fn publisher(&mut self, publisher: &str) -> SigningKey {
        let k = SigningKey::generate();
        self.b.delegate(publisher, &k.public()).unwrap();
        k
    }

    /// A client trusting `1.root.json`.
    pub fn client(&self) -> TrustedRepo {
        self.client_in(&self.trust)
    }

    /// A client trusting `1.root.json` with its trust store under `trust`.
    pub fn client_in(&self, trust: &Path) -> TrustedRepo {
        let root = self.b.first_root_bytes().unwrap();
        let sha = sha256_hex(&root);
        let store = TrustStore::new(trust, &sha).unwrap();
        TrustedRepo::init(store, &root, &sha).unwrap()
    }

    pub fn refresh(&self, c: &mut TrustedRepo) -> Result<RefreshReport, TufError> {
        c.refresh(
            self.transport(),
            &Policy::default(),
            self.clock.as_ref() as &dyn Clock,
        )
    }

    /// Every file under `dir`, path → bytes.
    pub fn files(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        fn walk(base: &Path, d: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    let rel = p
                        .strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(rel, std::fs::read(&p).unwrap());
                }
            }
        }
        if dir.exists() {
            walk(dir, dir, &mut out);
        }
        out
    }
}

/// A folder transport that records every fetch and can fail or panic at one path.
pub struct Faulty {
    pub inner: FsTransport,
    pub panic_at: Option<String>,
    pub fail_at: Option<(String, fin_registry::transport::TransportError)>,
    pub log: std::sync::Mutex<Vec<(String, u64)>>,
}

impl Faulty {
    pub fn new(repo: &Path) -> Faulty {
        Faulty {
            inner: FsTransport::new(repo),
            panic_at: None,
            fail_at: None,
            log: Default::default(),
        }
    }

    pub fn fetched(&self) -> Vec<(String, u64)> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Transport for Faulty {
    fn fetch(
        &self,
        path: &str,
        max: u64,
    ) -> Result<Vec<u8>, fin_registry::transport::TransportError> {
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((path.to_string(), max));
        if self.panic_at.as_deref() == Some(path) {
            panic!("killed while fetching {path}");
        }
        if let Some((p, e)) = &self.fail_at
            && p == path
        {
            return Err(e.clone());
        }
        self.inner.fetch(path, max)
    }
}

impl Reg {
    pub fn refresh_with(
        &self,
        c: &mut TrustedRepo,
        t: Arc<dyn Transport>,
        policy: Policy,
        clock: &dyn Clock,
    ) -> Result<RefreshReport, TufError> {
        c.refresh(t, &policy, clock)
    }

    /// Parses `metadata/<name>` of the registry.
    pub fn meta<T: serde::de::DeserializeOwned>(&self, name: &str) -> T {
        serde_json::from_slice(&std::fs::read(self.repo.join("metadata").join(name)).unwrap())
            .unwrap()
    }

    pub fn meta_path(&self, name: &str) -> PathBuf {
        self.repo.join("metadata").join(name)
    }

    /// The newest root of the registry, parsed.
    pub fn newest_root(&self) -> tough::schema::Signed<tough::schema::Root> {
        let mut v = 1;
        while self.meta_path(&format!("{}.root.json", v + 1)).exists() {
            v += 1;
        }
        self.meta(&format!("{v}.root.json"))
    }

    /// The private keys of a top-level role of the newest root, from the key ring.
    pub fn role_keys(&self, role: tough::schema::RoleType) -> Vec<SigningKey> {
        let root = self.newest_root();
        root.signed.roles[&role]
            .keyids
            .iter()
            .filter_map(|id| self.b.keys().get(&hex::encode(id.as_ref())).unwrap())
            .collect()
    }
}
