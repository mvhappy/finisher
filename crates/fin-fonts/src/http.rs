//! The HTTP client (4C Decision 33): `ureq` over `rustls` with the `ring` provider and the
//! platform verifier (the OS trust store, so corporate CAs work); connect timeout 10 s, idle
//! read timeout 30 s. `https` only, except plain `http` to loopback in `test-hooks` builds
//! when asked.
//!
//! **Proxies** are `ureq::Proxy::try_from_env`'s:
//! - the first of `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY` (and their lowercase forms) that
//!   parses is used for every request, so `ALL_PROXY` wins over `HTTPS_PROXY`; `NO_PROXY`
//!   (exact hosts, `*.x`, `.x`, `*`) bypasses it. An HTTP proxy is used with `CONNECT`;
//!   SOCKS proxies are not supported (they are skipped);
//! - with none set, on Windows, the user's system proxy (`ProxyEnable` and a single
//!   `host:port` `ProxyServer` in the registry). A per-protocol `ProxyServer`
//!   (`http=…;https=…`) does not parse and the fetch goes direct; PAC files and WPAD are not
//!   read. Behind such a proxy, set `HTTPS_PROXY`.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

pub(crate) use crate::validate::is_loopback_http;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a request failed, and whether another attempt at the same URL may succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    /// A short camelCase reason (`http500`, `network`, `insecureUrl`, …).
    pub reason: String,
    pub message: String,
    pub retry: bool,
}

impl Failure {
    pub fn new(reason: impl Into<String>, message: impl Into<String>, retry: bool) -> Failure {
        Failure {
            reason: reason.into(),
            message: message.into(),
            retry,
        }
    }
}

/// A response body to read: from the start (`200`) or from `start` (`206`).
pub(crate) struct Body {
    pub start: u64,
    /// The whole representation's size a `206` states (`Content-Range: …/<total>`).
    pub total: Option<u64>,
    /// `Content-Length` when sent.
    pub length: Option<u64>,
    pub reader: Box<dyn Read + Send>,
}

pub(crate) struct Http {
    agent: ureq::Agent,
    allow_loopback_http: bool,
}

impl Http {
    /// `allow_loopback_http` takes effect only in `test-hooks` builds.
    pub fn new(allow_loopback_http: bool, proxy_from_env: bool) -> Http {
        let allow_loopback_http = allow_loopback_http && cfg!(feature = "test-hooks");
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .proxy(if proxy_from_env {
                ureq::Proxy::try_from_env()
            } else {
                None
            })
            .https_only(!allow_loopback_http)
            .http_status_as_error(false)
            .max_redirects(5)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_per_read(Some(READ_TIMEOUT))
            .user_agent(concat!("finisher-fonts/", env!("CARGO_PKG_VERSION")))
            .build();
        Http {
            agent: config.into(),
            allow_loopback_http,
        }
    }

    /// Refuses anything but `https` (and loopback `http` when allowed).
    pub fn check_url(&self, url: &str) -> Result<(), Failure> {
        if url.starts_with("https://") || (self.allow_loopback_http && is_loopback_http(url)) {
            return Ok(());
        }
        Err(Failure::new(
            "insecureUrl",
            format!("{url} is not https"),
            false,
        ))
    }

    /// `GET url`, asking for the bytes from `from` on when `from > 0`. A `206` that does not
    /// start at `from` is refused (`badRange`); a `200` is a body from the start.
    pub fn get(&self, url: &str, from: u64) -> Result<Body, Failure> {
        self.check_url(url)?;
        let mut req = self.agent.get(url);
        if from > 0 {
            req = req.header("Range", format!("bytes={from}-"));
        }
        let resp = req.call().map_err(|e| {
            let (reason, retry) = classify(&e);
            Failure::new(reason, e.to_string(), retry)
        })?;
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let length = header("content-length").and_then(|v| v.trim().parse::<u64>().ok());
        let range = header("content-range").unwrap_or_default();
        let (start, total) = match status {
            200 => (0, None),
            206 if from > 0 => {
                let (first, total) = content_range(&range);
                if first != Some(from) {
                    return Err(Failure::new(
                        "badRange",
                        format!("asked for bytes from {from}, got {range:?}"),
                        true,
                    ));
                }
                (from, total)
            }
            s => {
                // 408, 425, 429 and 5xx may pass; other statuses will not.
                let retry = matches!(s, 408 | 425 | 429) || s >= 500;
                return Err(Failure::new(
                    format!("http{s}"),
                    format!("{url} answered {s}"),
                    retry,
                ));
            }
        };
        let reader = Box::new(resp.into_body().into_reader());
        Ok(Body {
            start,
            total,
            length,
            reader,
        })
    }
}

/// A failed call's reason and whether the same URL is worth another attempt: TLS and
/// certificate errors, redirect loops, refused redirects and bad URLs fail over to the next
/// source at once; network trouble is retried.
fn classify(e: &ureq::Error) -> (&'static str, bool) {
    use ureq::Error as E;
    match e {
        E::Rustls(_) | E::Tls(_) | E::Pem(_) | E::TlsRequired => ("tls", false),
        E::Io(io) if io.get_ref().is_some_and(|i| i.is::<rustls::Error>()) => ("tls", false),
        E::TooManyRedirects | E::RedirectFailed => ("redirect", false),
        E::RequireHttpsOnly(_) => ("insecureUrl", false),
        E::BadUri(_) | E::Http(_) | E::InvalidProxyUrl => ("badUrl", false),
        _ => ("network", true),
    }
}

/// The first byte and the total of `bytes <first>-<last>/<total>`.
fn content_range(v: &str) -> (Option<u64>, Option<u64>) {
    let Some(rest) = v.trim().strip_prefix("bytes ") else {
        return (None, None);
    };
    let first = rest.split('-').next().and_then(|f| f.trim().parse().ok());
    let total = rest.rsplit('/').next().and_then(|t| t.trim().parse().ok());
    (first, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_http_is_recognised() {
        assert!(is_loopback_http("http://127.0.0.1:8080/x"));
        assert!(is_loopback_http("http://localhost/x"));
        assert!(is_loopback_http("http://[::1]:9/x"));
        assert!(!is_loopback_http("http://127.0.0.1.example.com/x"));
        assert!(!is_loopback_http("https://127.0.0.1/x"));
        assert!(!is_loopback_http("http://example.com/x"));
    }

    #[test]
    fn tls_and_redirect_errors_are_not_retried() {
        let cert = ureq::Error::Rustls(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer,
        ));
        assert_eq!(classify(&cert), ("tls", false));
        let wrapped = ureq::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::Expired),
        ));
        assert_eq!(classify(&wrapped), ("tls", false));
        assert_eq!(
            classify(&ureq::Error::TooManyRedirects),
            ("redirect", false)
        );
        assert_eq!(classify(&ureq::Error::ConnectionFailed), ("network", true));
        assert_eq!(classify(&ureq::Error::HostNotFound), ("network", true));
    }

    #[test]
    fn content_range_parses() {
        assert_eq!(content_range("bytes 100-199/200"), (Some(100), Some(200)));
        assert_eq!(content_range("bytes 100-199/*"), (Some(100), None));
        assert_eq!(content_range("bytes */200"), (None, Some(200)));
    }
}
