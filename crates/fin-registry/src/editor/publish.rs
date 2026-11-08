//! Delegating publishers, publishing packages, yanks and advisories.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use fin_plugin_api::{SourceClass, StructuralChecks, ValidateCtx, host_api};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tough::TargetName;
use tough::schema::{DelegatedRole, Hashes, PathPattern, PathSet, Signed, Target, Targets};

use super::state::{Change, role_keys};
use super::{EditorError, PublicKey, RepoBuilder, SigningKey};
use crate::advisory::{Advisory, AdvisoryFile};
use crate::package::{DirFiles, Limits, unpack_bytes};
use crate::target::TargetCustom;
use crate::tuf::TargetInfo;
use crate::tuf::custom;

fn target_name(name: &str) -> Result<TargetName, EditorError> {
    TargetName::new(name).map_err(|e| EditorError::invalid("targetName", e.to_string()))
}

fn info(name: &str, role: &str, t: &Target) -> TargetInfo {
    TargetInfo {
        name: name.to_string(),
        role: role.to_string(),
        length: t.length,
        sha256: hex::encode(t.hashes.sha256.as_ref()),
        custom: t.custom.clone().into_iter().collect(),
    }
}

fn target_of(bytes: &[u8], custom: serde_json::Map<String, Value>) -> Target {
    Target {
        length: bytes.len() as u64,
        hashes: Hashes {
            sha256: Sha256::digest(bytes).to_vec().into(),
            _extra: HashMap::new(),
        },
        custom: custom.into_iter().collect(),
        _extra: HashMap::new(),
    }
}

impl RepoBuilder {
    fn delegation(&self, publisher: &str) -> Option<&DelegatedRole> {
        self.st
            .targets
            .signed
            .delegations
            .as_ref()?
            .roles
            .iter()
            .find(|r| r.name == publisher)
    }

    /// Whether `key` signs `publisher`'s role (delegated, or pending its first package).
    fn signs_for(&self, publisher: &str, key: &PublicKey) -> bool {
        let id = key.key_id();
        match self.delegation(publisher) {
            Some(r) => r.keyids.iter().any(|k| hex::encode(k.as_ref()) == id),
            None => self.st.pending.get(publisher) == Some(key),
        }
    }

    /// Delegates `<publisher>/*` to `key` (one terminating delegation per publisher). The
    /// delegation is signed into `targets` with the publisher's first package; re-delegating
    /// an existing publisher is [`RepoBuilder::rotate`] with [`super::Role::Publisher`].
    pub fn delegate(&mut self, publisher: &str, key: &PublicKey) -> Result<(), EditorError> {
        if !crate::tuf::checks_publisher_name(publisher) {
            return Err(EditorError::invalid(
                "publisher",
                format!("{publisher:?} is not a publisher name"),
            ));
        }
        if self.delegation(publisher).is_some() {
            return Err(EditorError::new(
                "PUBLISHER_EXISTS",
                format!("{publisher} is delegated; rotate its key instead"),
                json!({ "publisher": publisher }),
            ));
        }
        self.st.pending.insert(publisher.to_string(), *key);
        self.save_pending()
    }

    /// Adds the delegation of a pending publisher and its empty role.
    fn activate(&mut self, publisher: &str) -> Result<(), EditorError> {
        let key = *self.st.pending.get(publisher).ok_or_else(|| {
            EditorError::new(
                "PUBLISHER_NOT_DELEGATED",
                format!("{publisher} is not delegated"),
                json!({ "publisher": publisher }),
            )
        })?;
        let d = self
            .st
            .targets
            .signed
            .delegations
            .get_or_insert_with(Default::default);
        let id = key.key_id();
        d.keys
            .insert(hex::decode(&id).expect("hex").into(), key.tuf_key());
        let rk = role_keys(vec![id], 1);
        d.roles.push(DelegatedRole {
            name: publisher.to_string(),
            keyids: rk.keyids,
            threshold: rk.threshold,
            paths: PathSet::Paths(vec![
                PathPattern::new(format!("{publisher}/*"))
                    .map_err(|e| EditorError::invalid("publisher", e.to_string()))?,
            ]),
            terminating: true,
            targets: None,
        });
        let mut role = Targets::new(
            "1.0.0".into(),
            std::num::NonZeroU64::MIN,
            jiff::Timestamp::UNIX_EPOCH,
        );
        role.delegations = None;
        self.st.roles.insert(
            publisher.to_string(),
            Signed {
                signed: role,
                signatures: Vec::new(),
            },
        );
        self.next.insert(publisher.to_string(), 1);
        Ok(())
    }

    fn require_signer(&self, publisher: &str, key: &SigningKey) -> Result<(), EditorError> {
        if self.signs_for(publisher, &key.public()) {
            return Ok(());
        }
        Err(EditorError::new(
            "PUBLISHER_NOT_DELEGATED",
            format!("this key does not sign for {publisher}"),
            json!({ "publisher": publisher }),
        ))
    }

    /// Activates `publisher`'s pending delegation if it has none yet; whether it did.
    fn ensure_active(&mut self, publisher: &str) -> Result<bool, EditorError> {
        if self.delegation(publisher).is_some() {
            return Ok(false);
        }
        self.activate(publisher)?;
        Ok(true)
    }

    /// Re-signs `publisher`'s role with `key` (and `targets`, when `new` delegated it).
    fn commit_role(
        &mut self,
        publisher: &str,
        key: &SigningKey,
        new: bool,
    ) -> Result<(), EditorError> {
        self.commit(Change {
            targets: new,
            roles: BTreeMap::from([(publisher.to_string(), vec![key.clone()])]),
            ..Change::default()
        })?;
        if new {
            self.st.pending.remove(publisher);
            self.save_pending()?;
        }
        Ok(())
    }

    /// Publishes the package at `pkg`, signed by its publisher's key. The package is read by
    /// the strict reader and its manifest validated; a version already listed is refused
    /// (`PLUGIN_VERSION_EXISTS`: versions are immutable).
    pub fn publish(
        &mut self,
        pkg: &Path,
        publisher_key: &SigningKey,
    ) -> Result<TargetInfo, EditorError> {
        let tmp = tempfile::tempdir().map_err(|e| EditorError::io("create", pkg, e))?;
        let dest = tmp.path().join("p");
        // Read once: the bytes unpacked and validated are the bytes hashed and published, even
        // if the file changes meanwhile.
        let bytes = std::fs::read(pkg).map_err(|e| EditorError::io("read", pkg, e))?;
        let unpacked = unpack_bytes(&bytes, &dest, |m| m.executables(), &Limits::default())?;
        let ctx = ValidateCtx {
            host_api: host_api(),
            platform: None,
            source: SourceClass::Registry,
            node: None,
            content_checks: &StructuralChecks,
        };
        let manifest = fin_plugin_api::validate(&unpacked.manifest, &DirFiles(&dest), &ctx)
            .map_err(|problems| {
                let first = problems
                    .first()
                    .map(|p| p.message.clone())
                    .unwrap_or_default();
                EditorError::new(
                    "PLUGIN_MANIFEST_INVALID",
                    format!("the package does not validate: {first}"),
                    json!({ "problems": problems }),
                )
            })?;
        let publisher = manifest.id.publisher().to_string();
        self.require_signer(&publisher, publisher_key)?;
        let version = manifest.version.to_string();
        let name = custom::package_target(&publisher, manifest.id.name(), &version);
        let tn = target_name(&name)?;
        if self
            .st
            .roles
            .get(&publisher)
            .is_some_and(|r| r.signed.targets.contains_key(&tn))
        {
            return Err(EditorError::new(
                "PLUGIN_VERSION_EXISTS",
                format!("{} {version} is already published", manifest.id),
                json!({ "id": manifest.id.to_string(), "version": version }),
            ));
        }
        let bytes_unpacked = unpacked.files.values().map(|f| f.size).sum();
        let custom = TargetCustom::from_manifest(&manifest, &unpacked.manifest, bytes_unpacked);
        let target = target_of(&bytes, custom.to_map());
        let sha = hex::encode(target.hashes.sha256.as_ref());
        self.write_target(&custom::consistent_target_path(&name, &sha), &bytes)?;
        let new = self.ensure_active(&publisher)?;
        let out = info(&name, &publisher, &target);
        self.st
            .roles
            .get_mut(&publisher)
            .expect("an active publisher has a role")
            .signed
            .targets
            .insert(tn, target);
        self.commit_role(&publisher, publisher_key, new)?;
        Ok(out)
    }

    /// Marks `id`@`version` yanked in its publisher's role (signed by `publisher_key`).
    pub fn yank(
        &mut self,
        id: &str,
        version: &semver::Version,
        publisher_key: &SigningKey,
    ) -> Result<(), EditorError> {
        let pid = fin_plugin_api::PluginId::parse(id)
            .map_err(|e| EditorError::invalid("pluginId", format!("{id:?}: {}", e.reason())))?;
        let publisher = pid.publisher().to_string();
        self.require_signer(&publisher, publisher_key)?;
        let name = custom::package_target(&publisher, pid.name(), &version.to_string());
        let tn = target_name(&name)?;
        let not_found = || {
            EditorError::new(
                "PLUGIN_NOT_FOUND",
                format!("{id} {version} is not published"),
                json!({ "id": id, "version": version.to_string() }),
            )
        };
        let role = self.st.roles.get_mut(&publisher).ok_or_else(not_found)?;
        let t = role.signed.targets.get_mut(&tn).ok_or_else(not_found)?;
        t.custom.insert("yanked".into(), Value::Bool(true));
        self.commit_role(&publisher, publisher_key, false)
    }

    /// Adds (or replaces, by id) an advisory. Signed by a publisher's key it goes into that
    /// publisher's `advisories.json`; signed by the registry's targets key, into the
    /// registry-wide one.
    pub fn advise(&mut self, a: Advisory, key: &SigningKey) -> Result<(), EditorError> {
        let pubkey = key.public();
        let targets_ids =
            super::state::ids(&self.st.root.signed.roles[&tough::schema::RoleType::Targets]);
        let publisher = self
            .st
            .roles
            .keys()
            .chain(self.st.pending.keys())
            .find(|p| self.signs_for(p, &pubkey))
            .cloned();
        let registry_wide = publisher.is_none() && targets_ids.contains(&pubkey.key_id());
        if publisher.is_none() && !registry_wide {
            return Err(EditorError::new(
                "PUBLISHER_NOT_DELEGATED",
                "this key signs for no publisher and is not the registry's targets key",
                json!({}),
            ));
        }
        let name = custom::advisories_target(publisher.as_deref());
        let tn = target_name(&name)?;
        let existing = match &publisher {
            Some(p) => self.st.roles.get(p).and_then(|r| r.signed.targets.get(&tn)),
            None => self.st.targets.signed.targets.get(&tn),
        };
        let mut file = match existing {
            Some(t) => {
                let sha = hex::encode(t.hashes.sha256.as_ref());
                let p = self
                    .targets_dir()
                    .join(custom::consistent_target_path(&name, &sha));
                let b = std::fs::read(&p).map_err(|e| EditorError::io("read", &p, e))?;
                serde_json::from_slice::<AdvisoryFile>(&b)
                    .map_err(|e| EditorError::invalid("advisories", e.to_string()))?
            }
            None => AdvisoryFile::default(),
        };
        file.advisories.retain(|x| x.id != a.id);
        file.advisories.push(a);
        file.advisories.sort_by(|x, y| x.id.cmp(&y.id));
        let mut bytes = serde_json::to_vec_pretty(&file).expect("advisories serialise");
        bytes.push(b'\n');
        let target = target_of(&bytes, serde_json::Map::new());
        let sha = hex::encode(target.hashes.sha256.as_ref());
        self.write_target(&custom::consistent_target_path(&name, &sha), &bytes)?;
        match publisher {
            Some(p) => {
                let new = self.ensure_active(&p)?;
                self.st
                    .roles
                    .get_mut(&p)
                    .expect("active")
                    .signed
                    .targets
                    .insert(tn, target);
                self.commit_role(&p, key, new)
            }
            None => {
                self.st.targets.signed.targets.insert(tn, target);
                self.commit(Change {
                    targets: true,
                    ..Change::default()
                })
            }
        }
    }

    /// The current content of `publisher`'s role.
    pub fn role(&self, publisher: &str) -> Option<&Targets> {
        self.st.roles.get(publisher).map(|r| &r.signed)
    }

    /// Replaces `publisher`'s role with `role` as given and signs it with `signers`, then the
    /// snapshot and timestamp: no check that the keys or the targets belong to the publisher.
    /// The attack tests use it to forge what a compromised or revoked key would sign.
    #[doc(hidden)]
    pub fn commit_unchecked_role(
        &mut self,
        publisher: &str,
        role: Targets,
        signers: &[SigningKey],
    ) -> Result<(), EditorError> {
        let new = self.ensure_active(publisher)?;
        self.st.roles.get_mut(publisher).expect("active").signed = role;
        self.commit(Change {
            targets: new,
            roles: BTreeMap::from([(publisher.to_string(), signers.to_vec())]),
            ..Change::default()
        })
    }

    /// The SHA-256 of every package byte the registry lists, by target name (for tests and
    /// `fin-plugin`'s output).
    pub fn listed(&self) -> Vec<TargetInfo> {
        let mut out: Vec<TargetInfo> = self
            .st
            .roles
            .iter()
            .flat_map(|(role, s)| {
                s.signed
                    .targets
                    .iter()
                    .map(move |(n, t)| info(n.raw(), role, t))
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}
