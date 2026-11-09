//! Plugin → host calls of host API 1.1 (`call {id, method, params}` → `reply`, Decision 2;
//! spec §3.6). Each call's params decode with these types before the host checks rights and
//! reach; a malformed call is `PLUGIN_PROTOCOL`. Methods of later batches (`analysis.*`,
//! `media.*`, `job.submit`, `secret.grant`) are not here: the host answers them
//! `INVALID_REQUEST {method, availableFrom}`.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The host methods of host API 1.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum HostMethod {
    /// [`ProjectSnapshot`] → [`ProjectSnapshotPage`] (`project:read`).
    #[serde(rename = "project.snapshot")]
    ProjectSnapshot,
    /// [`ProjectView`] → [`ProjectViewText`] (`project:read`).
    #[serde(rename = "project.view")]
    ProjectView,
    /// [`NetFetch`] → [`NetFetchReply`] (`net`).
    #[serde(rename = "net.fetch")]
    NetFetch,
    /// [`NetRead`] → [`NetReadPage`] (`net`).
    #[serde(rename = "net.read")]
    NetRead,
    /// [`Log`] → `{}`.
    #[serde(rename = "log")]
    Log,
}

impl HostMethod {
    pub const ALL: [HostMethod; 5] = [
        HostMethod::ProjectSnapshot,
        HostMethod::ProjectView,
        HostMethod::NetFetch,
        HostMethod::NetRead,
        HostMethod::Log,
    ];

    pub fn name(self) -> &'static str {
        match self {
            HostMethod::ProjectSnapshot => "project.snapshot",
            HostMethod::ProjectView => "project.view",
            HostMethod::NetFetch => "net.fetch",
            HostMethod::NetRead => "net.read",
            HostMethod::Log => "log",
        }
    }

    pub fn from_name(name: &str) -> Option<HostMethod> {
        HostMethod::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// `project.snapshot`: one page of the call's project and branch at the job's pinned `rev`,
/// as canonical JSON (page 0 when absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
}

/// One page of a snapshot; every page of one job has the same `rev`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSnapshotPage {
    /// At most 768 KiB before base64.
    pub bytes_b64: String,
    pub page: u32,
    pub pages: u32,
    pub rev: u64,
}

/// `project.view`: the `fin view` text of the call's branch, capped at 768 KiB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectView {
    /// `fin view --budget`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u32>,
    /// `fin view --range` (`2s..5s`, `2s..`, `..5s`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectViewText {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// An HTTPS request to one of the plugin's approved `net` origins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpRequest {
    pub method: HttpMethod,
    /// An `https://` URL.
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<Header>,
    /// At most 768 KiB before base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_b64: Option<String>,
}

/// `net.fetch`: brokered HTTPS egress (Decision 10). `grant` names a secret grant; in 8B
/// every such request answers `SECRET_MISSING {name}` (injection is 8G's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetFetch {
    pub request: HttpRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<String>,
}

/// The response's head; the body (at most 64 MiB) stays with the host and is read by
/// `net.read` through `handle` until the job ends (Decision 11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetFetchReply {
    pub status: u16,
    pub headers: Vec<Header>,
    pub handle: String,
    /// The body's length in bytes.
    pub length: u64,
    /// A cross-origin redirect, returned unfollowed (its `location` is in `headers`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_not_followed: Option<bool>,
}

/// `net.read`: a page of a held response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetRead {
    pub handle: String,
    pub offset: u64,
    /// At most 768 KiB.
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetReadPage {
    pub bytes_b64: String,
    /// Whether the page reaches the end of the body.
    pub eof: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// `log`: rate-limited, scrubbed and capped by the host, written to the server log, never
/// journaled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Log {
    pub level: LogLevel,
    pub msg: String,
}

/// Methods of later host APIs, each with the batch that brings it: the host answers them
/// `INVALID_REQUEST {method, availableFrom}` (sp8b Task 7; `job.submit` is Decision 6's,
/// `secret.grant` Decision 10's).
pub const RESERVED_METHODS: &[(&str, &str)] = &[
    ("analysis.get", "8C"),
    ("analysis.find", "8C"),
    ("media.pcm", "8C"),
    ("media.frames", "8C"),
    ("job.submit", "8C"),
    ("media.file", "8H"),
    ("secret.grant", "8G"),
];

/// The batch a reserved method arrives in, if `method` is one.
pub fn reserved_method(method: &str) -> Option<&'static str> {
    RESERVED_METHODS
        .iter()
        .find(|(m, _)| *m == method)
        .map(|(_, b)| *b)
}

fn roundtrip<T: Serialize + DeserializeOwned>(params: &Value) -> Result<Value, String> {
    let typed: T = serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
    serde_json::to_value(typed).map_err(|e| e.to_string())
}

/// `params` decoded by `method`'s type (unknown fields refused), as the canonical JSON the
/// host acts on; the decoder's message otherwise (the host answers `PLUGIN_PROTOCOL`).
pub fn decode_params(method: HostMethod, params: &Value) -> Result<Value, String> {
    match method {
        HostMethod::ProjectSnapshot => roundtrip::<ProjectSnapshot>(params),
        HostMethod::ProjectView => roundtrip::<ProjectView>(params),
        HostMethod::NetFetch => roundtrip::<NetFetch>(params),
        HostMethod::NetRead => roundtrip::<NetRead>(params),
        HostMethod::Log => roundtrip::<Log>(params),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_params_refuses_unknown_fields_and_wrong_types() {
        let ok = decode_params(HostMethod::ProjectSnapshot, &json!({ "page": 2 })).unwrap();
        assert_eq!(ok, json!({ "page": 2 }));
        assert_eq!(
            decode_params(HostMethod::ProjectSnapshot, &json!({})).unwrap(),
            json!({})
        );
        assert!(decode_params(HostMethod::ProjectSnapshot, &json!({ "page": -1 })).is_err());
        assert!(decode_params(HostMethod::ProjectView, &json!({ "extra": 1 })).is_err());
        assert!(decode_params(HostMethod::Log, &json!({ "level": "loud", "msg": "x" })).is_err());
        assert!(decode_params(HostMethod::Log, &json!(null)).is_err());
        let log = decode_params(HostMethod::Log, &json!({ "level": "warn", "msg": "x" })).unwrap();
        assert_eq!(log, json!({ "level": "warn", "msg": "x" }));
    }

    #[test]
    fn reserved_methods_are_not_host_methods() {
        for (m, batch) in RESERVED_METHODS {
            assert!(HostMethod::from_name(m).is_none(), "{m}");
            assert_eq!(reserved_method(m), Some(*batch));
        }
        assert_eq!(reserved_method("log"), None);
    }
}
