//! The process-group kill domain (Decision 17; macOS, and Linux without delegated cgroups): a
//! group is a pgid, made by its first process and joined by the others; kill is
//! `kill(-pgid, SIGKILL)`. There is no leaf: `JobLimits` are not enforced. A process that
//! `setsid`s out of its group escapes the kill (Decision 18, a documented deviation). Task 4
//! adds the watchdog registration (`placed` → `+g`, `close_group` → `-g`).

use std::io;

use crate::KillTree;
use crate::domain::{ExitFacts, GroupHandle, KillDomain, LeafHandle, Placement};
use crate::spawner::JobLimits;
use crate::{GroupId, GroupSpec};

pub(crate) struct ProcessGroups;

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

    fn open_group(&self, id: GroupId, spec: &GroupSpec) -> io::Result<GroupHandle> {
        Ok(GroupHandle::new(id, spec, Box::new(())))
    }

    fn open_leaf(&self, _group: &GroupHandle, _limits: &JobLimits) -> io::Result<LeafHandle> {
        Ok(LeafHandle::none())
    }

    fn place(&self, _g: &GroupHandle, _l: &LeafHandle, _p: &mut Placement) -> io::Result<()> {
        Ok(())
    }

    fn kill(&self, group: &GroupHandle) -> io::Result<()> {
        super::kill_pgid(group.pgid())
    }

    fn memory(&self, _group: &GroupHandle) -> Option<u64> {
        None
    }

    fn foreign(&self, _group: &GroupHandle, _spawned: u32) -> Option<u32> {
        None
    }

    fn exit_facts(&self, _group: &GroupHandle) -> ExitFacts {
        ExitFacts::default()
    }

    fn close_group(&self, _group: GroupHandle) {}
}
