//! An in-process HTTP/1.1 server on loopback for the fetcher's tests (no internet in tests):
//! routes serve fixed bytes with injectable faults (wrong bytes, a truncated body then a
//! dropped connection, `Range` on or off, an error status, slow bodies, no
//! `Content-Length`). It also answers `CONNECT` as a proxy would, recording it, and then
//! serves the tunnelled requests itself.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Route {
    pub body: Arc<Vec<u8>>,
    /// Answer `Range: bytes=N-` with `206`.
    pub range: bool,
    /// The first `n` requests get this status and no body.
    pub fail_first: usize,
    pub status: u16,
    /// The first `n` requests send only `truncate_at` bytes of the body, then hang up.
    pub truncate_first: usize,
    pub truncate_at: usize,
    /// A pause after every 16 KiB of body.
    pub chunk_delay: Duration,
    /// No `Content-Length`: the body ends when the connection closes.
    pub close_delimited: bool,
    /// `Content-Range` states this total instead of the body's length.
    pub range_total: Option<usize>,
}

impl Route {
    pub fn new(body: Vec<u8>) -> Route {
        Route {
            body: Arc::new(body),
            range: true,
            status: 500,
            ..Route::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub method: String,
    pub path: String,
    pub range: Option<String>,
}

#[derive(Default)]
struct State {
    routes: HashMap<String, Route>,
    hits: Vec<Hit>,
    connects: Vec<String>,
}

pub struct Server {
    port: u16,
    state: Arc<Mutex<State>>,
    connections: Arc<AtomicUsize>,
}

impl Server {
    pub fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        let connections = Arc::new(AtomicUsize::new(0));
        let (st, conns) = (state.clone(), connections.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                conns.fetch_add(1, Ordering::SeqCst);
                let st = st.clone();
                std::thread::spawn(move || serve(stream, &st));
            }
        });
        Server {
            port,
            state,
            connections,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn route(&self, path: &str, route: Route) {
        self.state
            .lock()
            .unwrap()
            .routes
            .insert(path.to_string(), route);
    }

    /// Requests for `path` (`GET` and `HEAD`).
    pub fn hits(&self, path: &str) -> Vec<Hit> {
        let st = self.state.lock().unwrap();
        st.hits.iter().filter(|h| h.path == path).cloned().collect()
    }

    /// Accepted TCP connections.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// `CONNECT` targets seen.
    pub fn connects(&self) -> Vec<String> {
        self.state.lock().unwrap().connects.clone()
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut out = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let (method, target) = (
            parts.next().unwrap_or("").to_string(),
            parts.next().unwrap_or("").to_string(),
        );
        let mut range = None;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':')
                && k.eq_ignore_ascii_case("range")
            {
                range = Some(v.trim().to_string());
            }
        }
        if method == "CONNECT" {
            state.lock().unwrap().connects.push(target);
            if out
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .is_err()
            {
                return;
            }
            continue;
        }
        let path = target
            .strip_prefix("http://")
            .and_then(|r| r.find('/').map(|i| r[i..].to_string()))
            .unwrap_or(target);
        let (route, nth) = {
            let mut st = state.lock().unwrap();
            let nth = st.hits.iter().filter(|h| h.path == path).count();
            st.hits.push(Hit {
                method: method.clone(),
                path: path.clone(),
                range: range.clone(),
            });
            (st.routes.get(&path).cloned(), nth)
        };
        if !respond(&mut out, route, nth, range.as_deref()) {
            let _ = out.shutdown(Shutdown::Both);
            return;
        }
    }
}

/// Writes one response; `false` when the connection must close.
fn respond(out: &mut TcpStream, route: Option<Route>, nth: usize, range: Option<&str>) -> bool {
    let Some(r) = route else {
        let _ = out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return true;
    };
    if nth < r.fail_first {
        let head = format!("HTTP/1.1 {} Failing\r\nContent-Length: 0\r\n\r\n", r.status);
        return out.write_all(head.as_bytes()).is_ok();
    }
    let from = range
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.strip_suffix('-'))
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&f| r.range && f < r.body.len());
    let body: &[u8] = &r.body[from.unwrap_or(0)..];
    let mut head = match from {
        Some(f) => format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {f}-{}/{}\r\n",
            r.body.len() - 1,
            r.range_total.unwrap_or(r.body.len())
        ),
        None => "HTTP/1.1 200 OK\r\n".to_string(),
    };
    if r.close_delimited {
        head.push_str("Connection: close\r\n\r\n");
    } else {
        head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    }
    if out.write_all(head.as_bytes()).is_err() {
        return false;
    }
    let cut = (nth < r.truncate_first).then_some(r.truncate_at.min(body.len()));
    let send = &body[..cut.unwrap_or(body.len())];
    for chunk in send.chunks(16 * 1024) {
        if out.write_all(chunk).is_err() {
            return false;
        }
        let _ = out.flush();
        if !r.chunk_delay.is_zero() {
            std::thread::sleep(r.chunk_delay);
        }
    }
    cut.is_none() && !r.close_delimited
}
