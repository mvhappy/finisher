//! The host's view of one process's stdout (Decisions 3, 4, 9a): `hello` first and once,
//! checked against the request; then every line decoded with `seq` checked, `alive`
//! consumed, messages naming another job refused, and unknown types of a newer plugin answered
//! or dropped. Pure: the reader thread feeds it lines.

use fin_jobs::codec::{self, Decoded};
use fin_jobs::msg_worker::Role as HelloRole;
use fin_jobs::{Hello, PROTOCOL, WorkerMsg};

use crate::exit::ExitReason;

/// What a process was spawned as, for its `hello`.
pub(crate) struct Expect {
    /// `Program::Bundled`: the envelope must match exactly (Decision 3).
    pub bundled: bool,
    pub helper: bool,
    /// The self-sandbox profile requested, which `hello.selfSandbox` must equal.
    pub self_sandbox: Option<String>,
    /// `hello.selfSandbox: "none"` is accepted for a requested profile only when the host's own
    /// mechanism is disabled (F2, [`crate::HostOptions::unconfined_ok`]).
    pub unconfined_ok: bool,
    pub host_minor: u32,
}

/// What to do with a line.
#[derive(Debug, PartialEq)]
pub(crate) enum Frame {
    Hello(Hello),
    Msg(WorkerMsg),
    /// `alive`: consumed here.
    Alive,
    /// An unknown type from a newer plugin, with an `id`: reply `UNKNOWN_MESSAGE`.
    ReplyUnknown(u64),
    /// An unknown type from a newer plugin, without an `id`: dropped and counted.
    DropUnknown,
    /// Kill the process with this reason.
    Fail(ExitReason),
}

pub(crate) struct Framing {
    expect: Expect,
    seq: u64,
    hello: bool,
    lenient: bool,
    pub dropped: u64,
}

fn protocol(what: &str) -> Frame {
    Frame::Fail(ExitReason::ProtocolError(what.to_owned()))
}

impl Framing {
    pub fn new(expect: Expect) -> Framing {
        Framing {
            expect,
            seq: 0,
            hello: false,
            lenient: false,
            dropped: 0,
        }
    }

    /// A line over the cap.
    pub fn too_long(&mut self) -> Frame {
        protocol("long")
    }

    /// One line; `job` is the job last started on this process.
    pub fn line(&mut self, line: &[u8], job: Option<&str>) -> Frame {
        self.seq += 1;
        let decoded = match codec::decode_worker(line, self.seq, self.lenient) {
            Ok(d) => d,
            Err(e) => return protocol(e.what()),
        };
        let msg = match decoded {
            Decoded::Msg(m) => m,
            Decoded::Unknown { id: Some(id), .. } => return Frame::ReplyUnknown(id),
            Decoded::Unknown { id: None, .. } => {
                self.dropped += 1;
                return Frame::DropUnknown;
            }
        };
        if !self.hello {
            return match msg {
                WorkerMsg::Hello(h) => self.check_hello(h),
                _ => protocol("order"),
            };
        }
        match msg {
            WorkerMsg::Hello(_) => protocol("hello"),
            WorkerMsg::Alive {} => Frame::Alive,
            m => match m.job() {
                Some(j) if Some(j) != job => protocol("otherJob"),
                _ => Frame::Msg(m),
            },
        }
    }

    fn check_hello(&mut self, h: Hello) -> Frame {
        if h.v != PROTOCOL {
            return Frame::Fail(ExitReason::ProtocolMismatch {
                expected: format!("v{PROTOCOL}"),
                got: format!("v{}", h.v),
            });
        }
        let want_role = if self.expect.helper {
            HelloRole::Helper
        } else {
            HelloRole::Worker
        };
        if h.role != want_role {
            return protocol("role");
        }
        if self.expect.bundled && h.envelope.as_deref() != Some(fin_jobs::envelope_id()) {
            return Frame::Fail(ExitReason::ProtocolMismatch {
                expected: fin_jobs::envelope_id().to_owned(),
                got: h.envelope.unwrap_or_else(|| "none".to_owned()),
            });
        }
        // A requested profile, or `none` where the mechanism is disabled on this host (F2): the
        // process runs unconfined and says so. Anything else is a protocol error.
        let unconfined = self.expect.unconfined_ok
            && self.expect.self_sandbox.is_some()
            && h.self_sandbox.as_deref() == Some(fin_jobs::sandbox::NONE);
        if h.self_sandbox != self.expect.self_sandbox && !unconfined {
            return protocol("selfSandbox");
        }
        self.hello = true;
        self.lenient = !self.expect.bundled && h.minor > self.expect.host_minor;
        Frame::Hello(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_jobs::msg_worker::KindVersion;
    use serde_json::json;

    fn expect(bundled: bool) -> Expect {
        Expect {
            bundled,
            helper: false,
            self_sandbox: None,
            unconfined_ok: false,
            host_minor: 0,
        }
    }

    fn hello(minor: u32, envelope: Option<&str>) -> Hello {
        Hello {
            v: 1,
            minor,
            role: HelloRole::Worker,
            family: "test".into(),
            kinds: vec![KindVersion::new("test.echo", 1)],
            warm: false,
            envelope: envelope.map(str::to_owned),
            self_sandbox: None,
            pid: 1,
            kind: None,
            id: None,
        }
    }

    fn line(seq: u64, msg: &WorkerMsg) -> Vec<u8> {
        codec::encode(seq, msg).unwrap()
    }

    #[test]
    fn hello_first_then_messages_of_the_started_job() {
        let mut f = Framing::new(expect(true));
        let h = WorkerMsg::Hello(hello(0, Some(fin_jobs::envelope_id())));
        assert!(matches!(f.line(&line(1, &h), None), Frame::Hello(_)));
        assert_eq!(f.line(&line(2, &WorkerMsg::Alive {}), None), Frame::Alive);
        let r = WorkerMsg::Result {
            job: "j1".into(),
            value: json!(1),
        };
        assert_eq!(f.line(&line(3, &r), Some("j1")), Frame::Msg(r.clone()));
        assert_eq!(f.line(&line(4, &r), Some("j2")), protocol("otherJob"));
    }

    #[test]
    fn order_seq_and_second_hello_are_errors() {
        let mut f = Framing::new(expect(true));
        assert_eq!(
            f.line(&line(1, &WorkerMsg::Alive {}), None),
            protocol("order")
        );
        let mut f = Framing::new(expect(true));
        let h = WorkerMsg::Hello(hello(0, Some(fin_jobs::envelope_id())));
        f.line(&line(1, &h), None);
        assert_eq!(
            f.line(&line(3, &WorkerMsg::Alive {}), None),
            protocol("seq")
        );
        let mut f = Framing::new(expect(true));
        f.line(&line(1, &h), None);
        assert_eq!(f.line(&line(2, &h), None), protocol("hello"));
    }

    #[test]
    fn bundled_needs_the_envelope_and_plugins_do_not() {
        let mut f = Framing::new(expect(true));
        let h = WorkerMsg::Hello(hello(0, Some("0000000000000000")));
        assert!(matches!(
            f.line(&line(1, &h), None),
            Frame::Fail(ExitReason::ProtocolMismatch { .. })
        ));
        let mut f = Framing::new(expect(false));
        let h = WorkerMsg::Hello(hello(1, None));
        assert!(matches!(f.line(&line(1, &h), None), Frame::Hello(_)));
        let unknown = br#"{"v":1,"t":"future","seq":2,"id":7}"#;
        assert_eq!(f.line(unknown, None), Frame::ReplyUnknown(7));
        let unknown = br#"{"v":1,"t":"future","seq":3}"#;
        assert_eq!(f.line(unknown, None), Frame::DropUnknown);
        assert_eq!(f.dropped, 1);
    }

    #[test]
    fn unknown_type_at_the_hosts_minor_is_an_error() {
        let mut f = Framing::new(expect(false));
        f.line(&line(1, &WorkerMsg::Hello(hello(0, None))), None);
        let unknown = br#"{"v":1,"t":"future","seq":2,"id":7}"#;
        assert_eq!(f.line(unknown, None), protocol("unknownType"));
    }

    #[test]
    fn self_sandbox_and_role_must_match() {
        let mut f = Framing::new(Expect {
            self_sandbox: Some("worker".into()),
            ..expect(true)
        });
        let h = WorkerMsg::Hello(hello(0, Some(fin_jobs::envelope_id())));
        assert_eq!(f.line(&line(1, &h), None), protocol("selfSandbox"));
        // `none`: requested, nothing applied (F2). Unrequested, `none` is still a mismatch.
        let mut hh = hello(0, Some(fin_jobs::envelope_id()));
        hh.self_sandbox = Some(fin_jobs::sandbox::NONE.into());
        let none = WorkerMsg::Hello(hh);
        let mut f = Framing::new(Expect {
            self_sandbox: Some("worker".into()),
            unconfined_ok: true,
            ..expect(true)
        });
        assert!(matches!(f.line(&line(1, &none), None), Frame::Hello(_)));
        // A host whose own mechanism is not disabled never takes `none` (review of Task 3, B1).
        let mut f = Framing::new(Expect {
            self_sandbox: Some("worker".into()),
            ..expect(true)
        });
        assert_eq!(f.line(&line(1, &none), None), protocol("selfSandbox"));
        let mut f = Framing::new(expect(true));
        assert_eq!(f.line(&line(1, &none), None), protocol("selfSandbox"));
        let mut f = Framing::new(Expect {
            helper: true,
            ..expect(true)
        });
        assert_eq!(f.line(&line(1, &h), None), protocol("role"));
    }
}
