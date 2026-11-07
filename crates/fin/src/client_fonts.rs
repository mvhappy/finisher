//! `fin fonts fetch` (plan 4C Task 24): `fonts.fetch` blocks until every requested file is
//! cached or has failed, which can take minutes (a CJK collection is 20 MB, and each source
//! is tried three times with backoff), so it runs on its own connection with a long timeout.
//! While it runs, a second connection polls `fonts.status` once a second and writes the files
//! still downloading to stderr as one JSON line each (`{"progress": [...]}`); a fetch that
//! finishes within the first second writes nothing.

use std::sync::mpsc;
use std::time::Duration;

use fin_core::CoreError;
use serde_json::{Value, json};

use crate::client::{Client, TIMEOUT};
use crate::endpoint::Endpoint;

/// How long one `fonts.fetch` may take before `fin` stops waiting (the server keeps going).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const POLL: Duration = Duration::from_secs(1);

/// The files of a `fonts.status` answer still queued or downloading, compactly.
fn in_progress(status: &Value) -> Vec<Value> {
    let files = status["files"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    files
        .iter()
        .filter(|f| matches!(f["state"]["state"].as_str(), Some("queued" | "fetching")))
        .map(|f| {
            json!({
                "entry": f.get("entry"),
                "style": f.get("style"),
                "sha256": f["sha256"],
                "state": f["state"]["state"],
                "bytes": f["state"].get("bytes"),
                "total": f["state"].get("total").or(f.get("size")),
            })
        })
        .collect()
}

/// Sends `fonts.fetch {payload}` and waits for its answer (the server's `FontsStatus`).
pub fn fetch(endpoint: &Endpoint, payload: Value) -> Result<Value, CoreError> {
    let mut client = Client::connect(endpoint, FETCH_TIMEOUT)?;
    let (done, stop) = mpsc::channel::<()>();
    let poller = {
        let endpoint = endpoint.clone();
        std::thread::spawn(move || {
            let mut status = None;
            while let Err(mpsc::RecvTimeoutError::Timeout) = stop.recv_timeout(POLL) {
                if status.is_none() {
                    status = Client::connect(&endpoint, TIMEOUT).ok();
                }
                let Some(c) = status.as_mut() else { continue };
                match c.call("fonts.status", json!({})) {
                    Ok(s) => {
                        let files = in_progress(&s);
                        if !files.is_empty() {
                            eprintln!("{}", json!({ "progress": files }));
                        }
                    }
                    Err(_) => status = None,
                }
            }
        })
    };
    let answer = client.call("fonts.fetch", payload);
    let _ = done.send(());
    let _ = poller.join();
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lists_queued_and_fetching_files_only() {
        let status = json!({ "files": [
            { "sha256": "a", "size": 10, "entry": "e", "style": "regular", "state": { "state": "cached" } },
            { "sha256": "b", "size": 20, "state": { "state": "fetching", "bytes": 5, "total": 20 } },
            { "sha256": "c", "size": 30, "state": { "state": "queued" } },
        ] });
        let p = in_progress(&status);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0]["bytes"], 5);
        assert_eq!(p[1]["total"], 30);
    }
}
