//! The holder's two looks at the folder, shared by [`super::Lease::check`] and the heartbeat:
//! its record ([`Inner::verify`]) and the claims ([`Inner::no_newer_claim`]).
//!
//! Each look either passes, finds **evidence** that the lease is gone, or **cannot tell**.
//! Evidence loses the lease at once: a valid record naming another server or another epoch, a
//! deleted `lease.json`, a young claim above our epoch. A look that cannot tell refuses only
//! the write (or beat) it guards: a read that failed (a Linux cifs `soft` mount returns `EIO`
//! while it reconnects), a record that is still not one valid record after
//! [`record::read_settled`] re-read it (a torn or short SMB read), a listing that failed. The
//! lease then stands until the self-fence deadline (last successful heartbeat + 2×TTL), as
//! spec §3.5 says: nothing is lost until then. That is safe because the holder never writes
//! after such a look: the heartbeat writes only after its record verified, and every folder
//! write takes a full check of its own. A look that cannot tell also stops a commit from
//! trusting an earlier check ([`Inner::doubt`]).
//!
//! A persistently damaged record therefore fences its holder at the deadline, not at once,
//! and is then held for everyone until a confirmed takeover replaces it (spec §3.5).

use super::claim;
use super::clock::lock;
use super::record::{self, Seen};
use super::{Inner, Io};

/// Why a look refused.
pub(crate) enum Fence {
    /// Evidence that another server writes, or that `lease.json` was deleted: the lease is
    /// lost.
    Lost(String),
    /// This look could not tell (module docs): this write is refused, the lease stands. The
    /// self-fence deadline bounds how long that can last.
    Unsure(String),
}

impl Inner {
    /// Claims are the authority on epochs (parent spec §27.9, fenced design: "no newer epoch
    /// claim"): a `lease.claim-<e>` above our epoch means another server is acquiring or has
    /// acquired, even before its record is visible (or after our record overwrote theirs).
    /// A claim hidden by Windows' readdir lag only delays this extra protection.
    ///
    /// Only while the claim is younger than stale-after (45 s) of server time, though, an
    /// unknown age counting as young: an acquirer that crashed right after claiming must not
    /// lock the holder out for good. If it was merely slow, its record write is still caught
    /// by the record compare. Old claims are left in place (deleting one would let a later
    /// acquirer reuse its epoch).
    ///
    /// `server_now`: our server time ([`super::ServerClock::now_server_ms`]). A listing that
    /// fails (after retrying transient errors) is [`Fence::Unsure`].
    pub(super) fn no_newer_claim(&self, server_now: Option<i64>) -> Result<(), Fence> {
        let claims = claim::list(&self.dir)
            .map_err(|e| Fence::Unsure(format!("cannot list {}: {e}", self.dir.display())))?;
        let stale_ms = i64::try_from(self.params.stale_after.as_millis()).unwrap_or(i64::MAX);
        let young = |c: &claim::ClaimFile| match (server_now, c.server_ms) {
            (Some(now), Some(m)) => now - m <= stale_ms,
            _ => true,
        };
        match claims
            .iter()
            .rev()
            .find(|c| c.epoch > self.epoch && young(c))
        {
            Some(c) => Err(Fence::Lost(format!(
                "a newer claim {} exists; this lease was epoch {}",
                claim::claim_name(c.epoch),
                self.epoch
            ))),
            None => Ok(()),
        }
    }

    /// Whether `lease.json` still names this lease: our server id **and** our epoch. Claims
    /// make the epoch ours alone, so any heartbeat counter will do; a later one (a heartbeat
    /// write that landed after its retry was given up) is adopted.
    ///
    /// Missing, or valid and naming anyone else → [`Fence::Lost`]. Unreadable, or not one
    /// valid record after re-reading → [`Fence::Unsure`] (module docs).
    pub(super) fn verify(&self, io: &mut Io) -> Result<(), Fence> {
        match record::read_settled(&self.path()) {
            Err(e) => Err(Fence::Unsure(format!("lease.json is unreadable: {e}"))),
            Ok(Seen::Missing) => Err(Fence::Lost("lease.json is missing".to_string())),
            Ok(Seen::Corrupt(_)) => Err(Fence::Unsure(
                "lease.json fails its checksum (damaged, torn, or being rewritten by another \
                 server)"
                    .to_string(),
            )),
            Ok(Seen::Valid(r)) if r.server_id == io.last.server_id && r.epoch == self.epoch => {
                if r.heartbeat > io.last.heartbeat {
                    io.last = r;
                }
                Ok(())
            }
            Ok(Seen::Valid(r)) => Err(Fence::Lost(format!(
                "lease.json now names server {} at epoch {} (heartbeat {}); this lease was epoch {}",
                r.server_id, r.epoch, r.heartbeat, self.epoch
            ))),
        }
    }

    /// A look could not tell: the next commit must not trust an earlier full check
    /// ([`super::Lease::check_recent`]), nor one running now, and runs one of its own.
    pub(super) fn doubt(&self) {
        let mut s = lock(&self.status);
        s.doubts += 1;
        s.last_checked = None;
    }
}
