//! A console-proved RPC helper (plan 5A Task 5): the human-only operations (`project.takeover
//! {confirm: true}`, `project.open {relocate: true}`) answer `HUMAN_ONLY` to `fin`, so tests that
//! need them talk to the server as `console`, the way `finisherd <command>` will (Task 8).

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, Stream};
use serde_json::{Value, json};

use super::Server;

/// A connection to `server` proved with `console.key`.
pub struct Console {
    io: BufReader<Stream>,
    next: u64,
}

impl Console {
    pub fn open(server: &Server) -> Console {
        let name = server
            .socket
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .unwrap();
        let stream = Stream::connect(name).unwrap();
        #[cfg(unix)]
        stream
            .set_recv_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let mut io = BufReader::new(stream);
        let principal = super::auth::prove(&mut io, &server.sd, &server.socket, "console");
        assert_eq!(principal["kind"], "console", "{principal}");
        Console { io, next: 1 }
    }

    /// Sends `tag` with `payload` and returns its `exit` (`{_tag: "Success", value}` or a
    /// `Failure` with its cause).
    pub fn call(&mut self, tag: &str, payload: Value) -> Value {
        let id = format!("console-{}", self.next);
        self.next += 1;
        let frame =
            json!({ "_tag": "Request", "id": id, "tag": tag, "payload": payload, "headers": [] });
        let out = self.io.get_mut();
        out.write_all(format!("{frame}\n").as_bytes()).unwrap();
        out.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(
                self.io.read_line(&mut line).unwrap() > 0,
                "server closed the connection"
            );
            let f: Value = serde_json::from_str(&line).unwrap();
            if f["_tag"] == "Exit" && f["requestId"] == id {
                return f["exit"].clone();
            }
        }
    }
}

/// The error code of a `Failure` exit (`None` for a success).
pub fn error_code(exit: &Value) -> Option<String> {
    exit["cause"][0]["error"]["code"]
        .as_str()
        .map(str::to_string)
}
