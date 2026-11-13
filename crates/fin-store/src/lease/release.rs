//! [`Lease::release`]: a clean close lets go of the lease, so other servers need not wait out
//! staleness (45 s of server time plus confirmation).
//!
//! The release is the holder's last write, a heartbeat with [`LeaseRecord::released`] set:
//! 1. the heartbeat thread is stopped and joined (one still stuck in I/O after
//!    [`super::JOIN_WAIT`]: nothing is released, as after a crash);
//! 2. under the record lock, the same looks a heartbeat takes before writing: not lost, the
//!    self-fence deadline not passed, `lease.json` still names our server id and epoch, and no
//!    claim above our epoch younger than 45 s exists (an unknown age counts as young);
//! 3. the lease is marked lost ("released"), so nothing of it writes again;
//! 4. our record is rewritten in place with `heartbeat + 1` and `released`, then read back.
//!    A torn read-back is rewritten once while no claim above ours exists (as an acquisition
//!    does: a damaged record would need a confirmed takeover); a failed write puts our previous
//!    record back.
//!
//! Anything that fails leaves the record as a held one, and others wait out its staleness as
//! today. A release can never create a second holder: an observer takes a released record only
//! through a fresh claim above every epoch (`Contender::take`), and a release that lands over
//! a newer holder's record (a holder stalled between its fence and its write, the window every
//! heartbeat has too) only makes that holder lose its lease at its next fence.
//!
//! Read-only openers hold no [`Lease`], so they never release anything.

use std::io::ErrorKind;

use super::clock::lock;
use super::record::{self, LeaseRecord, Seen};
use super::{Fence, Inner, Lease, claim, registry};
use crate::fsio;

impl Lease {
    /// Lets go of the lease on a clean close (module docs): `true` if `lease.json` now reads
    /// as released. Either way the lease is gone. Folder I/O: never call it under a database
    /// lock. Dropping a lease (or a crash) releases nothing.
    pub fn release(mut self) -> bool {
        if !self.stop_heartbeat() {
            return false;
        }
        self.inner.release().is_ok()
    }
}

impl Inner {
    fn release(&self) -> Result<(), String> {
        let mut io = lock(&self.io);
        self.deadline().map_err(|l| l.reason)?;
        let server_now = io.clock.now_server_ms(self.now());
        let refused = |f: Fence| match f {
            Fence::Lost(r) | Fence::Unsure(r) => r,
        };
        self.verify(&mut io).map_err(refused)?;
        self.no_newer_claim(server_now).map_err(refused)?;
        self.deadline().map_err(|l| l.reason)?;
        self.set_lost("released by a clean close".to_string());
        let path = self.path();
        crate::reserved::no_links(path.parent().unwrap_or(&path)).map_err(|e| e.to_string())?;
        let next = LeaseRecord {
            heartbeat: io.last.heartbeat + 1,
            probe_server_ms: server_now.unwrap_or(io.last.probe_server_ms),
            released: true,
            ..io.last.clone()
        };
        let bytes = record::encode(&next);
        if let Err(e) = fsio::write_in_place(&path, &bytes) {
            if e.kind() != ErrorKind::NotFound {
                let _ = fsio::write_in_place(&path, &record::encode(&io.last));
            }
            return Err(format!("cannot write lease.json: {e}"));
        }
        let mut seen = record::read_settled(&path);
        if matches!(seen, Ok(Seen::Corrupt(_)))
            && claim::list(&self.dir).is_ok_and(|c| c.iter().all(|c| c.epoch <= self.epoch))
            && fsio::write_in_place(&path, &bytes).is_ok()
        {
            seen = record::read_settled(&path);
        }
        match seen {
            Ok(Seen::Valid(r)) if r == next => {
                registry::wrote(&self.key, &next);
                io.last = next;
                Ok(())
            }
            other => Err(format!("the released record did not read back: {other:?}")),
        }
    }
}
