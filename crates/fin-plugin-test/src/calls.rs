//! The fake host's answers to host calls (host API 1.1; `apps/finisherd/src/plugins/host/
//! calls.ts` and `egress.ts` are the real ones), in the real host's order: the method's name
//! (a later batch's is `INVALID_REQUEST {method, availableFrom}`, any other `{reason:
//! unknownMethod}`), the params decoded by the `fin-plugin-api` types (`PLUGIN_PROTOCOL`), the
//! method's permission against the manifest's (`PLUGIN_PERMISSION_DENIED {perm, missing:
//! grant}`), then the call: snapshot pages and the `fin view` text of the fixture, `log`
//! recorded, `net.fetch` to a declared `https` origin through the fake API and `net.read` of
//! the bodies it holds for the job.
//!
//! A call outside the declared permissions, to an undeclared origin, of a method the host API
//! does not have, or that does not decode is a violation: the profile records each one.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use fin_jobs::JobError;
use fin_plugin_api::Permission;
use fin_plugin_api::protocol::HostMethod;
use fin_plugin_api::protocol::calls::{
    HttpRequest, Log, NetFetch, NetRead, ProjectSnapshot, ProjectView, decode_params,
    reserved_method,
};
use serde_json::{Value, json};

use crate::fake_https::FakeApi;
use crate::fixtures::Snapshot;

/// A page of a paged reply, in bytes before base64 (the host's `PAGE_BYTES`, spec §24).
pub const PAGE_BYTES: usize = 768_000;
/// The largest `project.view` text, as JSON-encoded bytes (the host's `VIEW_BYTES`).
pub const VIEW_BYTES: usize = 768 * 1024;

/// A `log` call as the plugin made it (the real host scrubs it; the profile checks it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// The command and phase of the job that logged it.
    pub command: String,
    pub phase: &'static str,
    pub level: String,
    pub msg: String,
}

/// A call's reply and, when the call should not have been made, why.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub reply: Result<Value, JobError>,
    /// `(check, message)`: `permission` or `protocol`.
    pub violation: Option<(&'static str, String)>,
}

impl Answer {
    fn ok(value: Value) -> Answer {
        Answer {
            reply: Ok(value),
            violation: None,
        }
    }

    fn refused(e: JobError) -> Answer {
        Answer {
            reply: Err(e),
            violation: None,
        }
    }

    fn violation(e: JobError, check: &'static str, message: String) -> Answer {
        Answer {
            reply: Err(e),
            violation: Some((check, message)),
        }
    }
}

fn err(code: &str, message: impl Into<String>, details: Value) -> JobError {
    JobError::new(code, message).with_details(details)
}

/// The host side of the calls for one plugin and one fixture.
pub struct Calls {
    grants: Vec<Permission>,
    snapshot: Snapshot,
    api: Option<FakeApi>,
    bodies: HashMap<String, Vec<u8>>,
    next_handle: u64,
    /// The running job's command and phase.
    job: (String, &'static str),
    /// Every `log` call.
    pub logs: Vec<LogLine>,
}

impl Calls {
    pub fn new(grants: Vec<Permission>, snapshot: Snapshot, api: Option<FakeApi>) -> Calls {
        Calls {
            grants,
            snapshot,
            api,
            bodies: HashMap::new(),
            next_handle: 0,
            job: (String::new(), ""),
            logs: Vec::new(),
        }
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn api(&self) -> Option<&FakeApi> {
        self.api.as_ref()
    }

    /// A job of `command` starts (its `log` calls are named by it).
    pub fn start_job(&mut self, command: &str, phase: &'static str) {
        self.job = (command.into(), phase);
    }

    /// The job ended: the bodies it held are released (`net.read` of them fails).
    pub fn end_job(&mut self) {
        self.bodies.clear();
    }

    fn origins(&self) -> Vec<&str> {
        self.grants
            .iter()
            .flat_map(|g| match g {
                Permission::Net { origins } => origins.iter().map(|o| o.0.as_str()).collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    pub fn answer(&mut self, method: &str, params: &Value) -> Answer {
        let Some(m) = HostMethod::from_name(method) else {
            let e = match reserved_method(method) {
                Some(batch) => err(
                    "INVALID_REQUEST",
                    format!("{method} is not part of host API 1.1"),
                    json!({ "method": method, "availableFrom": batch }),
                ),
                None => err(
                    "INVALID_REQUEST",
                    format!("{method} is not a host method"),
                    json!({ "method": method, "reason": "unknownMethod" }),
                ),
            };
            return Answer::violation(
                e,
                "protocol",
                format!("called {method}, which host API 1.1 does not have"),
            );
        };
        let params = match decode_params(m, params) {
            Ok(p) => p,
            Err(error) => {
                let e = err(
                    "PLUGIN_PROTOCOL",
                    format!("the {method} call's params do not decode: {error}"),
                    json!({ "method": method, "error": error }),
                );
                return Answer::violation(
                    e,
                    "protocol",
                    format!("{method}'s params do not decode"),
                );
            }
        };
        let perm = match m {
            HostMethod::ProjectSnapshot | HostMethod::ProjectView => Some("project:read"),
            HostMethod::NetFetch | HostMethod::NetRead => Some("net"),
            HostMethod::Log => None,
        };
        if let Some(perm) = perm
            && !self.grants.iter().any(|g| g.key() == perm)
        {
            let e = err(
                "PLUGIN_PERMISSION_DENIED",
                format!("the plugin was not granted {perm}"),
                json!({ "perm": perm, "missing": "grant" }),
            );
            return Answer::violation(
                e,
                "permission",
                format!("called {method} without declaring {perm}"),
            );
        }
        // The params decoded by their type above, so these conversions hold.
        match m {
            HostMethod::Log => {
                let log: Log = serde_json::from_value(params).expect("decoded");
                let level = serde_json::to_value(log.level).ok();
                self.logs.push(LogLine {
                    command: self.job.0.clone(),
                    phase: self.job.1,
                    level: level
                        .and_then(|l| l.as_str().map(String::from))
                        .unwrap_or_default(),
                    msg: log.msg,
                });
                Answer::ok(json!({}))
            }
            HostMethod::ProjectSnapshot => {
                let p: ProjectSnapshot = serde_json::from_value(params).expect("decoded");
                self.snapshot_page(p.page.unwrap_or(0) as usize)
            }
            HostMethod::ProjectView => {
                let p: ProjectView = serde_json::from_value(params).expect("decoded");
                self.view(&p)
            }
            HostMethod::NetFetch => {
                let p: NetFetch = serde_json::from_value(params).expect("decoded");
                self.fetch(p)
            }
            HostMethod::NetRead => {
                let p: NetRead = serde_json::from_value(params).expect("decoded");
                self.read(&p)
            }
        }
    }

    fn snapshot_page(&self, page: usize) -> Answer {
        let bytes = &self.snapshot.bytes;
        let pages = bytes.len().div_ceil(PAGE_BYTES).max(1);
        if page >= pages {
            return Answer::refused(err(
                "INVALID_REQUEST",
                format!("page {page} is past the snapshot's {pages} pages"),
                json!({ "page": page, "pages": pages }),
            ));
        }
        let end = ((page + 1) * PAGE_BYTES).min(bytes.len());
        let chunk = &bytes[(page * PAGE_BYTES).min(end)..end];
        Answer::ok(json!({
            "bytesB64": B64.encode(chunk),
            "page": page,
            "pages": pages,
            "rev": self.snapshot.rev,
        }))
    }

    fn view(&self, p: &ProjectView) -> Answer {
        match fin_core::view::screenplay(
            &self.snapshot.project,
            p.range.as_deref(),
            p.budget.map(u64::from),
        ) {
            Ok(v) => {
                let lines: Vec<String> = v["lines"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                Answer::ok(json!({ "text": cap_view(&lines) }))
            }
            Err(e) => Answer::refused(err(&e.code, e.message.clone(), e.details.clone())),
        }
    }

    fn fetch(&mut self, p: NetFetch) -> Answer {
        if let Some(name) = p.grant {
            return Answer::refused(err(
                "SECRET_MISSING",
                format!(
                    "no secret named {name} is bound to this plugin (secret injection is not served before 8G)"
                ),
                json!({ "name": name }),
            ));
        }
        let HttpRequest {
            method,
            url,
            headers,
            body_b64,
        } = p.request;
        let target = match parse_https(&url) {
            Ok(t) => t,
            Err(e) => return Answer::refused(e),
        };
        if !self.origins().contains(&target.origin.as_str()) {
            let e = err(
                "EGRESS_DENIED",
                format!(
                    "{} is not an origin this plugin was approved to reach",
                    target.origin
                ),
                json!({ "origin": target.origin }),
            );
            let why = format!(
                "fetched {}, an origin its net permission does not name",
                target.origin
            );
            return Answer::violation(e, "permission", why);
        }
        let body = match body_b64.map(|b| B64.decode(b)).transpose() {
            Ok(b) => b.unwrap_or_default(),
            Err(_) => {
                return Answer::refused(err(
                    "PLUGIN_PROTOCOL",
                    "the request body is not base64",
                    json!({ "method": "net.fetch", "reason": "bodyB64" }),
                ));
            }
        };
        if body.len() > PAGE_BYTES {
            return Answer::refused(err(
                "EGRESS_TOO_LARGE",
                format!("the request body is over {PAGE_BYTES} bytes"),
                json!({ "what": "request", "limit": PAGE_BYTES }),
            ));
        }
        let method = serde_json::to_value(method)
            .ok()
            .and_then(|m| m.as_str().map(String::from))
            .unwrap_or_default();
        let headers: Vec<(String, String)> =
            headers.into_iter().map(|h| (h.name, h.value)).collect();
        let Some(api) = &self.api else {
            return Answer::refused(err(
                "EGRESS_UNREACHABLE",
                "the fake API is not running",
                json!({ "origin": target.origin, "reason": "noApi" }),
            ));
        };
        match api.fetch(&target.host, &method, &target.path, &headers, &body) {
            Ok(r) => {
                self.next_handle += 1;
                let handle = format!("b{}", self.next_handle);
                let length = r.body.len();
                self.bodies.insert(handle.clone(), r.body);
                let headers: Vec<Value> = r
                    .headers
                    .iter()
                    .map(|(k, v)| json!({ "name": k, "value": v }))
                    .collect();
                Answer::ok(
                    json!({ "status": r.status, "headers": headers, "handle": handle, "length": length }),
                )
            }
            Err(reason) => Answer::refused(err(
                "EGRESS_UNREACHABLE",
                format!("{} does not answer: {reason}", target.origin),
                json!({ "origin": target.origin, "reason": "connect" }),
            )),
        }
    }

    fn read(&self, p: &NetRead) -> Answer {
        let invalid = |message: String, details: Value| {
            Answer::refused(err("INVALID_REQUEST", message, details))
        };
        if p.length as usize > PAGE_BYTES {
            return invalid(
                format!("a net.read page is at most {PAGE_BYTES} bytes"),
                json!({ "reason": "pageTooLarge", "limit": PAGE_BYTES }),
            );
        }
        let Some(body) = self.bodies.get(&p.handle) else {
            return invalid(
                "net.read names no body this job holds".into(),
                json!({ "reason": "unknownHandle" }),
            );
        };
        let offset = p.offset as usize;
        if offset > body.len() {
            return invalid(
                format!("offset {offset} is past the body's {} bytes", body.len()),
                json!({ "reason": "offset", "length": body.len() }),
            );
        }
        let end = (offset + p.length as usize).min(body.len());
        Answer::ok(json!({ "bytesB64": B64.encode(&body[offset..end]), "eof": end == body.len() }))
    }
}

/// `lines` cut to whole lines so that their JSON encoding stays within [`VIEW_BYTES`].
fn cap_view(lines: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut size = 2;
    for l in lines {
        let n = Value::String(l.clone()).to_string().len() - 1;
        if size + n > VIEW_BYTES - 64 {
            out.push(format!(
                "… {} more lines cut at {VIEW_BYTES} bytes",
                lines.len() - out.len()
            ));
            break;
        }
        out.push(l.clone());
        size += n;
    }
    out.join("\n")
}

/// An `https` URL's origin (`https://host[:port]`, port 443 left out), host and path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub origin: String,
    pub host: String,
    pub path: String,
}

pub fn parse_https(url: &str) -> Result<Target, JobError> {
    let invalid = |reason: &str| {
        err(
            "INVALID_REQUEST",
            format!("net.fetch: {url:?} is not a URL the broker takes ({reason})"),
            json!({ "method": "net.fetch", "reason": reason }),
        )
    };
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(invalid("url"));
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" {
        return Err(err(
            "EGRESS_NOT_HTTPS",
            format!("plugins reach the network over HTTPS only, not {scheme}:"),
            json!({ "scheme": scheme }),
        ));
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return Err(invalid("userinfo"));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || authority.starts_with('[') => {
            let port: u16 = p.parse().map_err(|_| invalid("port"))?;
            (h.to_ascii_lowercase(), Some(port))
        }
        _ => (authority.to_ascii_lowercase(), None),
    };
    if host.is_empty() {
        return Err(invalid("host"));
    }
    let origin = match port {
        Some(p) if p != 443 => format!("https://{host}:{p}"),
        _ => format!("https://{host}"),
    };
    let tail = tail.split('#').next().unwrap_or("");
    let path = if tail.starts_with('/') {
        tail.to_owned()
    } else {
        format!("/{tail}")
    };
    Ok(Target { origin, host, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_origins() {
        let t = parse_https("https://API.example.org/about?x=1#f").unwrap();
        assert_eq!(t.origin, "https://api.example.org");
        assert_eq!(t.path, "/about?x=1");
        assert_eq!(parse_https("https://h:443").unwrap().origin, "https://h");
        assert_eq!(
            parse_https("https://h:8443/").unwrap().origin,
            "https://h:8443"
        );
        assert_eq!(
            parse_https("http://h/").unwrap_err().code,
            "EGRESS_NOT_HTTPS"
        );
        assert_eq!(
            parse_https("https://u@h/").unwrap_err().code,
            "INVALID_REQUEST"
        );
    }
}
