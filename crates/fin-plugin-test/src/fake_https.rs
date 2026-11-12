//! The fake HTTPS API behind `net.fetch` (spec §7.4): a TLS server on loopback whose
//! certificate is made for this run and names the plugin's declared origins' hosts, and the
//! broker's client, which trusts only that certificate and connects to loopback whatever the
//! host. The plugin never connects: the fake host is its broker, as the real host is.
//!
//! The API answers every `GET` with an HTML page whose `<title>` is [`PAGE_TITLE`] and which
//! sets a cookie holding the run's canary secret (a plugin must not log it); `HEAD` with the
//! same head; any other method with `{"received": <bytes>}`. Every request is recorded.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};

/// The `<title>` of the API's page.
pub const PAGE_TITLE: &str = "Finisher test page";
/// The largest request head or body the server reads.
const MAX_BYTES: usize = 1 << 20;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// A request the API received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub host: String,
    pub method: String,
    pub path: String,
    pub body_len: usize,
}

/// A response as the broker read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct FakeApi {
    addr: SocketAddr,
    client: Arc<ClientConfig>,
    requests: Arc<Mutex<Vec<ApiRequest>>>,
    stop: Arc<AtomicBool>,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

impl FakeApi {
    /// Starts the server for `hosts` (the certificate's names); `canary` is the secret its
    /// page's cookie carries.
    pub fn start(hosts: &[String], canary: &str) -> Result<FakeApi, String> {
        let mut names: Vec<String> = hosts.to_vec();
        if names.is_empty() {
            names.push("localhost".into());
        }
        let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
        let cert = rcgen::CertificateParams::new(names)
            .and_then(|p| p.self_signed(&key))
            .map_err(|e| e.to_string())?;
        let der: CertificateDer<'static> = cert.der().clone();
        let mut roots = RootCertStore::empty();
        roots.add(der.clone()).map_err(|e| e.to_string())?;
        let client = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let server = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(vec![der], private)
            .map_err(|e| e.to_string())?;
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (server, seen, stopped, canary) = (
            Arc::new(server),
            Arc::clone(&requests),
            Arc::clone(&stop),
            canary.to_owned(),
        );
        std::thread::spawn(move || {
            for sock in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(sock) = sock {
                    // One connection at a time: the broker makes one request at a time.
                    let _ = serve(&server, sock, &seen, &canary);
                }
            }
        });
        Ok(FakeApi {
            addr,
            client: Arc::new(client),
            requests,
            stop,
        })
    }

    /// The requests the API received so far.
    pub fn requests(&self) -> Vec<ApiRequest> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// One HTTPS request to `host` (verified against the run's certificate), sent to loopback.
    pub fn fetch(
        &self,
        host: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<ApiResponse, String> {
        let name = ServerName::try_from(host.to_owned()).map_err(|e| e.to_string())?;
        let sock = TcpStream::connect(self.addr).map_err(|e| e.to_string())?;
        sock.set_read_timeout(Some(IO_TIMEOUT))
            .map_err(|e| e.to_string())?;
        let conn =
            ClientConnection::new(Arc::clone(&self.client), name).map_err(|e| e.to_string())?;
        let mut tls = StreamOwned::new(conn, sock);
        let mut head = format!("{method} {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n");
        for (k, v) in headers {
            if !k.eq_ignore_ascii_case("host") && !k.eq_ignore_ascii_case("connection") {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
        }
        head.push_str(&format!("content-length: {}\r\n\r\n", body.len()));
        tls.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
        tls.write_all(body).map_err(|e| e.to_string())?;
        tls.flush().map_err(|e| e.to_string())?;
        let mut r = BufReader::new(tls);
        let (first, headers) = read_head(&mut r)?;
        let status = first
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or_else(|| format!("a bad status line {first:?}"))?;
        let mut body = vec![0; content_length(&headers)?];
        if method != "HEAD" {
            r.read_exact(&mut body).map_err(|e| e.to_string())?;
        } else {
            body.clear();
        }
        Ok(ApiResponse {
            status,
            headers,
            body,
        })
    }
}

impl Drop for FakeApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it sees `stop`.
        let _ = TcpStream::connect(self.addr);
    }
}

/// The request or status line and the headers (names lowercased) of one HTTP/1.1 head.
fn read_head(r: &mut impl BufRead) -> Result<(String, Vec<(String, String)>), String> {
    let mut first = None;
    let mut headers = Vec::new();
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        let n = r.read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
        total += n;
        if n == 0 || total > MAX_BYTES {
            return Err("the head ended early or is too long".into());
        }
        let line = String::from_utf8_lossy(&line).trim_end().to_owned();
        if line.is_empty() {
            break;
        }
        if first.is_none() {
            first = Some(line);
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    Ok((first.unwrap_or_default(), headers))
}

fn content_length(headers: &[(String, String)]) -> Result<usize, String> {
    let n = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .map_or(Ok(0), |(_, v)| {
            v.parse::<usize>().map_err(|e| e.to_string())
        })?;
    if n > MAX_BYTES {
        return Err(format!("a body of {n} bytes"));
    }
    Ok(n)
}

fn serve(
    config: &Arc<ServerConfig>,
    sock: TcpStream,
    seen: &Mutex<Vec<ApiRequest>>,
    canary: &str,
) -> Result<(), String> {
    sock.set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let conn = ServerConnection::new(Arc::clone(config)).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(StreamOwned::new(conn, sock));
    let (first, headers) = read_head(&mut r)?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut body = vec![0; content_length(&headers)?];
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    let host = headers
        .iter()
        .find(|(k, _)| k == "host")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    if let Ok(mut s) = seen.lock() {
        s.push(ApiRequest {
            host,
            method: method.clone(),
            path,
            body_len: body.len(),
        });
    }
    let (kind, extra, payload) = match method.as_str() {
        "GET" | "HEAD" => (
            "text/html; charset=utf-8",
            format!("set-cookie: fin_test_session={canary}; Secure; HttpOnly\r\n"),
            format!(
                "<!doctype html><html><head><title>{PAGE_TITLE}</title></head><body><p>A page of the fake API.</p></body></html>"
            ),
        ),
        _ => (
            "application/json",
            String::new(),
            format!("{{\"received\":{}}}", body.len()),
        ),
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n",
        payload.len()
    );
    let mut tls = r.into_inner();
    tls.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    if method != "HEAD" {
        tls.write_all(payload.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    tls.conn.send_close_notify();
    tls.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_https_with_the_runs_certificate() {
        let api = FakeApi::start(&["api.example.org".into()], "canary-1").unwrap();
        let r = api
            .fetch(
                "api.example.org",
                "GET",
                "/about",
                &[("accept".into(), "text/html".into())],
                b"",
            )
            .unwrap();
        assert_eq!(r.status, 200);
        assert!(String::from_utf8_lossy(&r.body).contains(PAGE_TITLE));
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "set-cookie" && v.contains("canary-1"))
        );
        let posted = api
            .fetch("api.example.org", "POST", "/", &[], b"abc")
            .unwrap();
        assert_eq!(posted.body, b"{\"received\":3}");
        let seen = api.requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            (seen[0].host.as_str(), seen[0].path.as_str()),
            ("api.example.org", "/about")
        );
        // A name the certificate does not hold fails verification.
        assert!(
            api.fetch("other.example.org", "GET", "/", &[], b"")
                .is_err()
        );
    }
}
