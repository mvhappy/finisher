//! The HTTP transport (feature `test-http`, never in shipped builds; spec §5.2): what the
//! `tuf-conformance` suite drives. `metadata/<file>` is fetched from the metadata base URL
//! and `targets/<path>` from the targets base URL. The `rustls` config names the `aws_lc_rs`
//! provider, so it never relies on a process default (spec §6.6).

use std::sync::Arc;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use super::{Transport, TransportError, components};

#[derive(Debug, Clone)]
pub struct HttpTransport {
    metadata: String,
    targets: String,
    agent: ureq::Agent,
}

fn base(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

impl HttpTransport {
    /// Fetches `metadata/…` under `metadata_url` and `targets/…` under `targets_url`.
    pub fn new(metadata_url: &str, targets_url: &str) -> HttpTransport {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .proxy(None)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!(
                "finisher-registry-test/",
                env!("CARGO_PKG_VERSION")
            ))
            .build();
        HttpTransport {
            metadata: base(metadata_url),
            targets: base(targets_url),
            agent: config.into(),
        }
    }

    /// One repository root holding `metadata/` and `targets/`.
    pub fn at(root_url: &str) -> HttpTransport {
        let root = base(root_url);
        HttpTransport::new(&format!("{root}metadata/"), &format!("{root}targets/"))
    }
}

impl Transport for HttpTransport {
    fn fetch(&self, path: &str, max: u64) -> Result<Vec<u8>, TransportError> {
        components(path)?;
        let url = if let Some(rest) = path.strip_prefix("metadata/") {
            format!("{}{rest}", self.metadata)
        } else if let Some(rest) = path.strip_prefix("targets/") {
            format!("{}{rest}", self.targets)
        } else {
            return Err(TransportError::BadPath { path: path.into() });
        };
        let unreachable = |reason: String| TransportError::Unreachable {
            path: path.into(),
            reason,
        };
        let mut resp = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| unreachable(e.to_string()))?;
        match resp.status().as_u16() {
            200 => {}
            404 | 410 => return Err(TransportError::NotFound { path: path.into() }),
            s => return Err(unreachable(format!("HTTP {s}"))),
        }
        let too_large = || TransportError::TooLarge {
            path: path.into(),
            max,
        };
        // `ureq` refuses a body of exactly its limit, so read one byte past the cap.
        match resp
            .body_mut()
            .with_config()
            .limit(max.saturating_add(1))
            .read_to_vec()
        {
            Ok(b) if b.len() as u64 > max => Err(too_large()),
            Ok(b) => Ok(b),
            Err(ureq::Error::BodyExceedsLimit(_)) => Err(too_large()),
            Err(e) => Err(unreachable(e.to_string())),
        }
    }
}
