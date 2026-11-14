//! Typed host calls (spec §3.6; plan 8B Decisions 5, 10, 11) over `fin_jobs::worker`'s
//! `call`/`reply`. Calls are cancel-aware: after the job's `cancel` a call is not sent, and one
//! in flight fails `JOB_CANCELLED` at once (`Reporter::call_until`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use fin_jobs::worker::{CancelToken, Reporter};
use fin_plugin_api::protocol::calls::{
    Header, HostMethod, HttpMethod, HttpRequest, Log, LogLevel, NetFetch, NetFetchReply, NetRead,
    NetReadPage, ProjectSnapshot, ProjectSnapshotPage, ProjectView, ProjectViewText,
};
use fin_plugin_api::protocol::{PAGE_BYTES, PluginContext, PluginError};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::command::{cancelled, error_from_job, protocol};

/// The largest response body the host holds (Decision 10).
pub const MAX_BODY_BYTES: u64 = 64 << 20;

/// The host, as one command or hook job sees it.
pub struct Host {
    reporter: Reporter,
    token: CancelToken,
    context: PluginContext,
    /// A command's pinned rev; none for a hook job (whose snapshot pages need only agree).
    rev: Option<u64>,
    #[cfg(feature = "files")]
    pub(crate) files: Vec<fin_jobs::FileGrant>,
}

/// A response read through `net.fetch` and `net.read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
    /// A cross-origin redirect, returned unfollowed (its `location` is in `headers`).
    pub redirect_not_followed: bool,
}

impl HttpResponse {
    /// The first header named `name` (ASCII case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    /// The body as UTF-8, invalid sequences replaced.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A `GET` of `url` with no headers and no body.
pub fn http_get(url: impl Into<String>) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        url: url.into(),
        headers: Vec::new(),
        body_b64: None,
    }
}

impl Host {
    pub(crate) fn new(
        reporter: Reporter,
        token: CancelToken,
        context: PluginContext,
        rev: Option<u64>,
    ) -> Host {
        Host {
            reporter,
            token,
            context,
            rev,
            #[cfg(feature = "files")]
            files: Vec::new(),
        }
    }

    /// The job's context (`start.extra`): host API, plugin, grants, directories, locale, limits.
    pub fn context(&self) -> &PluginContext {
        &self.context
    }

    /// Whether the host cancelled the job. A command checks it between steps and returns
    /// [`crate::command::cancelled`].
    pub fn cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// One host call; blocks until its reply or the job's cancel.
    fn call<T: DeserializeOwned>(
        &self,
        method: HostMethod,
        params: impl Serialize,
    ) -> Result<T, PluginError> {
        if self.cancelled() {
            return Err(cancelled());
        }
        let params = serde_json::to_value(params)
            .map_err(|e| protocol("params", format!("{}: {e}", method.name())))?;
        let value = self
            .reporter
            .call_until(method.name(), params, &self.token)
            .map_err(error_from_job)?;
        serde_json::from_value(value).map_err(|e| {
            protocol(
                "reply",
                format!("the reply to {} does not decode: {e}", method.name()),
            )
            .with("method", method.name())
        })
    }

    /// The project at the job's pinned `rev` (`project.snapshot`, `project:read`): every page
    /// fetched and joined. Each page must name its own index, the same page count (at least 1)
    /// and the job's `rev` (a hook job pins none: the first page's); otherwise `PLUGIN_PROTOCOL {reason: snapshotPage | snapshotPages |
    /// snapshotRev, page, expected, got}`; joined bytes that are not a project
    /// `{reason: snapshotJson}`.
    pub fn snapshot(&self) -> Result<fin_core::model::Project, PluginError> {
        let method = HostMethod::ProjectSnapshot;
        let mut bytes = Vec::new();
        let mut pages = None;
        let mut rev = self.rev;
        let mut page = 0u32;
        loop {
            let ask = ProjectSnapshot {
                page: (page > 0).then_some(page),
            };
            let got: ProjectSnapshotPage = self.call(method, ask)?;
            let bad = |reason: &str, msg: String, expected: u64, got: u64| {
                protocol(reason, msg)
                    .with("method", method.name())
                    .with("page", page)
                    .with("expected", expected)
                    .with("got", got)
            };
            let want = *rev.get_or_insert(got.rev);
            if got.rev != want {
                let msg = format!("page {page} is of rev {}, the job's is {want}", got.rev);
                return Err(bad("snapshotRev", msg, want, got.rev));
            }
            if got.page != page {
                let msg = format!("asked page {page}, got {}", got.page);
                return Err(bad("snapshotPage", msg, page.into(), got.page.into()));
            }
            let total = *pages.get_or_insert(got.pages);
            if got.pages == 0 || got.pages != total {
                let msg = format!("page count {} after {total}", got.pages);
                return Err(bad("snapshotPages", msg, total.into(), got.pages.into()));
            }
            bytes.extend(decode_b64(&got.bytes_b64, method)?);
            page += 1;
            if page == total {
                break;
            }
        }
        serde_json::from_slice(&bytes).map_err(|e| {
            protocol("snapshotJson", format!("the snapshot does not decode: {e}"))
                .with("method", method.name())
        })
    }

    /// The `fin view` text of the job's branch (`project.view`, `project:read`).
    pub fn view(&self, budget: Option<u32>) -> Result<String, PluginError> {
        self.view_range(budget, None)
    }

    /// [`Host::view`] of a time range (`2s..5s`, `2s..`, `..5s`).
    pub fn view_range(
        &self,
        budget: Option<u32>,
        range: Option<&str>,
    ) -> Result<String, PluginError> {
        let ask = ProjectView {
            budget,
            range: range.map(str::to_owned),
        };
        let got: ProjectViewText = self.call(HostMethod::ProjectView, ask)?;
        Ok(got.text)
    }

    /// An HTTPS request through the host's broker (`net.fetch`, `net`), its body read in pages
    /// (`net.read`) until `length` bytes or `eof`. `PLUGIN_PROTOCOL` when the body does not
    /// add up: `{reason: bodyLength}` (it ends before or runs past `length`), `{reason:
    /// bodyPage}` (an empty page before `eof`, or one longer than asked), `{reason:
    /// bodyTooLarge}` (a `length` over [`MAX_BODY_BYTES`]).
    pub fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        self.fetch_with(request, None)
    }

    /// [`Host::fetch`] with a secret grant's name (`net.fetch {grant}`; in host API 1.1 every
    /// such request answers `SECRET_MISSING {name}`, injection is 8G's).
    pub fn fetch_with(
        &self,
        request: HttpRequest,
        grant: Option<&str>,
    ) -> Result<HttpResponse, PluginError> {
        let head: NetFetchReply = self.call(
            HostMethod::NetFetch,
            NetFetch {
                request,
                grant: grant.map(str::to_owned),
            },
        )?;
        if head.length > MAX_BODY_BYTES {
            return Err(body_error(
                "bodyTooLarge",
                format!("a body of {} bytes passes {MAX_BODY_BYTES}", head.length),
            )
            .with("length", head.length)
            .with("max", MAX_BODY_BYTES));
        }
        let page = u64::from(self.context.limits.page_bytes.clamp(1, PAGE_BYTES));
        let mut body = Vec::with_capacity(head.length as usize);
        let mut offset = 0u64;
        while offset < head.length {
            let length = page.min(head.length - offset) as u32;
            let got: NetReadPage = self.call(
                HostMethod::NetRead,
                NetRead {
                    handle: head.handle.clone(),
                    offset,
                    length,
                },
            )?;
            let bytes = decode_b64(&got.bytes_b64, HostMethod::NetRead)?;
            if bytes.len() as u64 > u64::from(length) || (bytes.is_empty() && !got.eof) {
                let msg = format!("asked {length} bytes at {offset}, got {}", bytes.len());
                return Err(body_error("bodyPage", msg)
                    .with("offset", offset)
                    .with("asked", length)
                    .with("got", bytes.len()));
            }
            offset += bytes.len() as u64;
            body.extend(bytes);
            if got.eof {
                break;
            }
        }
        if offset != head.length {
            let msg = format!("the body ended at {offset} of {} bytes", head.length);
            return Err(body_error("bodyLength", msg)
                .with("length", head.length)
                .with("got", offset));
        }
        Ok(HttpResponse {
            status: head.status,
            headers: head.headers,
            body,
            redirect_not_followed: head.redirect_not_followed.unwrap_or(false),
        })
    }

    /// A line in the server log (`log`; rate-limited, scrubbed and capped by the host, never
    /// journaled). Fire and forget: it is not waited for, and its reply (a refusal included)
    /// is dropped. Not sent once the job is cancelled.
    pub fn log(&self, level: LogLevel, msg: &str) {
        if self.cancelled() {
            return;
        }
        let log = Log {
            level,
            msg: msg.to_owned(),
        };
        if let Ok(params) = serde_json::to_value(log) {
            self.reporter.call_detached(HostMethod::Log.name(), params);
        }
    }
}

fn body_error(reason: &str, message: String) -> PluginError {
    protocol(reason, message).with("method", HostMethod::NetRead.name())
}

fn decode_b64(text: &str, method: HostMethod) -> Result<Vec<u8>, PluginError> {
    B64.decode(text).map_err(|e| {
        protocol("base64", format!("bad base64 in {}: {e}", method.name()))
            .with("method", method.name())
    })
}
