//! Closed permissions (spec §4.1) and permission diffs (spec §4.3 "widening"). Risks are in
//! [`crate::risk`].

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::risk::{overall, risk_of};

/// An `https://host[:port]` origin; its grammar is checked by validation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Origin(pub String);

impl JsonSchema for Origin {
    fn schema_name() -> Cow<'static, str> {
        "Origin".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "description": "`https://host[:port]`.", "type": "string" })
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SecretMode {
    /// The host injects the secret into brokered requests; the plugin never sees it.
    Brokered,
    /// The plugin receives the secret's value.
    Raw,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FsScope {
    /// `<server-data>/plugins/data/<id>/<projectId|_>/` (spec §5.1). `workspace` is not in v1.
    PluginData,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FsAccess {
    Read,
    ReadWrite,
}

/// One permission, tagged by `perm`. The set is closed: an unknown `perm` or field fails the
/// parse (a unit permission with any other field too, which serde's tagged enums allow).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "perm", deny_unknown_fields)]
pub enum Permission {
    #[serde(rename = "project:read")]
    ProjectRead,
    #[serde(rename = "project:write")]
    ProjectWrite,
    #[serde(rename = "media:read")]
    MediaRead,
    #[serde(rename = "media:file")]
    MediaFile,
    #[serde(rename = "jobs:submit")]
    JobsSubmit,
    #[serde(rename = "gpu")]
    Gpu,
    #[serde(rename = "net")]
    Net { origins: Vec<Origin> },
    #[serde(rename = "secrets")]
    Secrets {
        names: Vec<String>,
        mode: SecretMode,
    },
    #[serde(rename = "fs")]
    Fs { scope: FsScope, access: FsAccess },
    #[serde(rename = "hooks:observe")]
    HooksObserve { hooks: Vec<String> },
    #[serde(rename = "hooks:modify")]
    HooksModify { hooks: Vec<String> },
}

/// The permissions with fields, as serde parses them.
#[derive(Deserialize)]
#[serde(tag = "perm", deny_unknown_fields)]
enum WithFields {
    #[serde(rename = "net")]
    Net { origins: Vec<Origin> },
    #[serde(rename = "secrets")]
    Secrets {
        names: Vec<String>,
        mode: SecretMode,
    },
    #[serde(rename = "fs")]
    Fs { scope: FsScope, access: FsAccess },
    #[serde(rename = "hooks:observe")]
    HooksObserve { hooks: Vec<String> },
    #[serde(rename = "hooks:modify")]
    HooksModify { hooks: Vec<String> },
}

impl<'de> Deserialize<'de> for Permission {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = Map::<String, Value>::deserialize(d)?;
        let perm = match map.get("perm") {
            Some(Value::String(s)) => s.as_str(),
            Some(_) => return Err(D::Error::custom("perm is a string")),
            None => return Err(D::Error::missing_field("perm")),
        };
        let unit = match perm {
            "project:read" => Some(Permission::ProjectRead),
            "project:write" => Some(Permission::ProjectWrite),
            "media:read" => Some(Permission::MediaRead),
            "media:file" => Some(Permission::MediaFile),
            "jobs:submit" => Some(Permission::JobsSubmit),
            "gpu" => Some(Permission::Gpu),
            _ => None,
        };
        if let Some(p) = unit {
            return match map.keys().find(|k| *k != "perm") {
                Some(extra) => Err(D::Error::unknown_field(extra, &["perm"])),
                None => Ok(p),
            };
        }
        Ok(
            match serde_json::from_value(Value::Object(map)).map_err(D::Error::custom)? {
                WithFields::Net { origins } => Permission::Net { origins },
                WithFields::Secrets { names, mode } => Permission::Secrets { names, mode },
                WithFields::Fs { scope, access } => Permission::Fs { scope, access },
                WithFields::HooksObserve { hooks } => Permission::HooksObserve { hooks },
                WithFields::HooksModify { hooks } => Permission::HooksModify { hooks },
            },
        )
    }
}

impl Permission {
    /// The `perm` tag; a manifest holds each at most once.
    pub fn key(&self) -> &'static str {
        match self {
            Permission::ProjectRead => "project:read",
            Permission::ProjectWrite => "project:write",
            Permission::MediaRead => "media:read",
            Permission::MediaFile => "media:file",
            Permission::JobsSubmit => "jobs:submit",
            Permission::Gpu => "gpu",
            Permission::Net { .. } => "net",
            Permission::Secrets { .. } => "secrets",
            Permission::Fs { .. } => "fs",
            Permission::HooksObserve { .. } => "hooks:observe",
            Permission::HooksModify { .. } => "hooks:modify",
        }
    }

    /// The names a list-valued permission grants (origins, secret names, hooks), as a set.
    fn items(&self) -> BTreeSet<&str> {
        match self {
            Permission::Net { origins } => origins.iter().map(|o| o.0.as_str()).collect(),
            Permission::Secrets { names, .. } => names.iter().map(String::as_str).collect(),
            Permission::HooksObserve { hooks } | Permission::HooksModify { hooks } => {
                hooks.iter().map(String::as_str).collect()
            }
            _ => BTreeSet::new(),
        }
    }

    /// Whether `self` grants something `old` (the same key) did not: an added origin, secret or
    /// hook, `brokered` → `raw`, `read` → `readWrite`, or another `fs` scope.
    fn widens(&self, old: &Permission) -> bool {
        let more_items = !self.items().is_subset(&old.items());
        match (old, self) {
            (Permission::Secrets { mode: a, .. }, Permission::Secrets { mode: b, .. }) => {
                more_items || (*a == SecretMode::Brokered && *b == SecretMode::Raw)
            }
            (
                Permission::Fs {
                    scope: sa,
                    access: a,
                },
                Permission::Fs {
                    scope: sb,
                    access: b,
                },
            ) => sa != sb || (*a == FsAccess::Read && *b == FsAccess::ReadWrite),
            _ => more_items,
        }
    }
}

/// One permission an update widens: the same `perm`, granting more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Widening {
    pub from: Permission,
    pub to: Permission,
}

/// What an update changes in a plugin's grants. Order inside lists never matters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDiff {
    /// Permissions whose key the old set did not have.
    pub added: Vec<Permission>,
    /// Permissions of one key where the new one grants more.
    pub widened: Vec<Widening>,
    /// Keys the new set no longer has.
    pub removed: Vec<Permission>,
    /// The overall risk rose, or a kept permission's risk rose through a combination.
    pub risk_up: bool,
}

impl PermissionDiff {
    /// Whether the permissions widen (spec §4.3): an added or widened permission, or a risk
    /// increase. Not the whole rule: agent-facing content and kind or runtime changes need a
    /// person too, so updates decide with [`crate::requires_reapproval`].
    pub fn permissions_widen(&self) -> bool {
        !self.added.is_empty() || !self.widened.is_empty() || self.risk_up
    }
}

/// Permissions by key; a repeated key (refused by validation) merges into the first.
fn by_key(perms: &[Permission]) -> BTreeMap<&'static str, Permission> {
    let mut m: BTreeMap<&'static str, Permission> = BTreeMap::new();
    for p in perms {
        match m.get_mut(p.key()) {
            None => {
                m.insert(p.key(), p.clone());
            }
            Some(have) => merge(have, p),
        }
    }
    m
}

fn merge(have: &mut Permission, more: &Permission) {
    match (have, more) {
        (Permission::Net { origins: a }, Permission::Net { origins: b }) => a.extend(b.clone()),
        (
            Permission::Secrets { names: a, mode: ma },
            Permission::Secrets { names: b, mode: mb },
        ) => {
            a.extend(b.clone());
            *ma = (*ma).max(*mb);
        }
        (Permission::Fs { access: a, .. }, Permission::Fs { access: b, .. }) => *a = (*a).max(*b),
        // Two scopes under one key (refused by validation) widen against either.
        (Permission::HooksObserve { hooks: a }, Permission::HooksObserve { hooks: b })
        | (Permission::HooksModify { hooks: a }, Permission::HooksModify { hooks: b }) => {
            a.extend(b.clone())
        }
        _ => {}
    }
}

pub fn diff(old: &[Permission], new: &[Permission]) -> PermissionDiff {
    let (o, n) = (by_key(old), by_key(new));
    let mut d = PermissionDiff {
        added: Vec::new(),
        widened: Vec::new(),
        removed: Vec::new(),
        risk_up: overall(new, false) > overall(old, false),
    };
    for (k, np) in &n {
        match o.get(k) {
            None => d.added.push(np.clone()),
            Some(op) => {
                if np.widens(op) {
                    d.widened.push(Widening {
                        from: op.clone(),
                        to: np.clone(),
                    });
                }
                if risk_of(np, new) > risk_of(op, old) {
                    d.risk_up = true;
                }
            }
        }
    }
    d.removed = o
        .iter()
        .filter(|(k, _)| !n.contains_key(*k))
        .map(|(_, p)| p.clone())
        .collect();
    d
}
