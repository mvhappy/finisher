//! The process-group kill domain (Decision 17; macOS, and Linux without delegated cgroups): a
//! group is a pgid, made by its first process and joined by the others; kill is
//! `kill(-pgid, SIGKILL)`. There is no leaf: `JobLimits` are not enforced, and memory limits
//! are checked by the host's sampler. A process that `setsid`s out of its group escapes the
//! kill (Decision 18, a documented deviation).
//!
//! With a watchdog (Decision 15), `placed` writes `+g <pgid>` once the group's first process
//! exists, and `close_group` writes `-g <pgid>`; the host reaps the leader only after that.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::foreign::{Seen, pgid_members};
use super::watchdog::Watchdog;
use crate::KillTree;
use crate::domain::{ExitFacts, GroupHandle, KillDomain, LeafHandle, Placement};
use crate::spawner::JobLimits;
use crate::{GroupId, GroupSpec};

#[derive(Default)]
pub(crate) struct ProcessGroups {
    pub watchdog: Option<Arc<Watchdog>>,
}

/// A group's own state.
#[derive(Default)]
struct PgState {
    seen: Seen,
    /// `+g` was written.
    listed: AtomicBool,
}

fn state(group: &GroupHandle) -> &PgState {
    group
        .os
        .downcast_ref::<PgState>()
        .expect("a process-group handle")
}

impl ProcessGroups {
    fn scan(&self, group: &GroupHandle) -> Option<u32> {
        let pgid = group.pgid();
        (pgid > 1).then(|| state(group).seen.note(pgid_members(pgid)))
    }
}

impl KillDomain for ProcessGroups {
    fn kill_tree(&self) -> KillTree {
        KillTree::ProcessGroup
    }

    fn leaf_limits(&self) -> bool {
        false
    }

    fn kernel_memory(&self) -> bool {
        false
    }

    fn watchdog(&self) -> Option<&Watchdog> {
        self.watchdog.as_deref()
    }

    fn open_group(&self, id: GroupId, spec: &GroupSpec) -> io::Result<GroupHandle> {
        Ok(GroupHandle::new(id, spec, Box::<PgState>::default()))
    }

    fn open_leaf(&self, _group: &GroupHandle, _limits: &JobLimits) -> io::Result<LeafHandle> {
        Ok(LeafHandle::none())
    }

    fn place(&self, _g: &GroupHandle, _l: &LeafHandle, _p: &mut Placement) -> io::Result<()> {
        Ok(())
    }

    fn placed(&self, group: &GroupHandle, _leaf: &LeafHandle, pid: u32) -> io::Result<()> {
        let st = state(group);
        st.seen.spawned(pid);
        // The group's pgid is stored once this succeeds; until then the first process's pid
        // is the pgid.
        let pgid = match group.pgid() {
            0 => pid as i32,
            g => g,
        };
        if let Some(w) = &self.watchdog
            && !st.listed.swap(true, Ordering::AcqRel)
        {
            w.add_group(pgid);
        }
        Ok(())
    }

    fn kill(&self, group: &GroupHandle) -> io::Result<()> {
        self.scan(group);
        super::kill_pgid(group.pgid())
    }

    fn memory(&self, _group: &GroupHandle) -> Option<u64> {
        None
    }

    fn foreign(&self, group: &GroupHandle, _spawned: u32) -> Option<u32> {
        self.scan(group)
    }

    fn exit_facts(&self, _group: &GroupHandle) -> ExitFacts {
        ExitFacts::default()
    }

    /// Ends what a member left in the group (its leader is still unreaped, so the pgid is
    /// still ours), then takes the group off the watchdog's list.
    fn close_group(&self, group: &GroupHandle) {
        let _ = super::kill_pgid(group.pgid());
        if let Some(w) = &self.watchdog
            && state(group).listed.load(Ordering::Acquire)
        {
            w.remove_group(group.pgid());
        }
    }
}
