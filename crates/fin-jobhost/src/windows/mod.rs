//! Windows: until Task 5 (host → group → leaf Job Objects, suspended spawns), the host
//! answers `JOBHOST_UNSUPPORTED` at start and these stubs keep the crate building.

use std::io::{self, PipeReader, PipeWriter};

use crate::domain::{ExitFacts, GroupHandle, KillDomain, LeafHandle, Placement};
use crate::exit::RawExit;
use crate::spawner::{JobLimits, PlatformCommand, Qos};
use crate::{GroupId, GroupSpec, HostError, HostOptions, KillTree};

/// The OS's part of a process (Task 5: the process handle the waiter waits on).
pub(crate) type OsProc = ();

pub(crate) struct Spawned {
    pub pid: u32,
    pub stdin: PipeWriter,
    pub stdout: PipeReader,
    pub stderr: PipeReader,
    pub os: OsProc,
}

pub(crate) struct SpawnCtx<'a> {
    pub domain: &'a dyn KillDomain,
    pub group: &'a GroupHandle,
    pub leaf: &'a LeafHandle,
    pub qos: Qos,
    pub host_pid: u32,
    #[cfg(feature = "test-hooks")]
    #[allow(dead_code)]
    pub resume_delay: Option<std::time::Duration>,
}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "JOBHOST_UNSUPPORTED until Task 5",
    )
}

pub(crate) fn default_domain(_opts: &HostOptions) -> Result<Box<dyn KillDomain>, HostError> {
    Err(HostError::Unsupported)
}

pub(crate) fn spawn(_cmd: PlatformCommand, _ctx: &SpawnCtx<'_>) -> io::Result<Spawned> {
    Err(unsupported())
}

pub(crate) fn wait(_os: &OsProc, _pid: u32, _reap: bool) -> io::Result<RawExit> {
    Err(unsupported())
}

pub(crate) fn reap(_pid: u32) {}

/// The Job Object domain (Task 5). Every call fails.
#[allow(dead_code)]
pub(crate) struct JobObjects;

impl KillDomain for JobObjects {
    fn kill_tree(&self) -> KillTree {
        KillTree::JobObject
    }
    fn leaf_limits(&self) -> bool {
        true
    }
    fn kernel_memory(&self) -> bool {
        true
    }
    fn open_group(&self, _id: GroupId, _spec: &GroupSpec) -> io::Result<GroupHandle> {
        Err(unsupported())
    }
    fn open_leaf(&self, _group: &GroupHandle, _limits: &JobLimits) -> io::Result<LeafHandle> {
        Err(unsupported())
    }
    fn place(&self, _g: &GroupHandle, _l: &LeafHandle, _p: &mut Placement) -> io::Result<()> {
        Err(unsupported())
    }
    fn kill(&self, _group: &GroupHandle) -> io::Result<()> {
        Err(unsupported())
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
    fn close_group(&self, _group: &GroupHandle) {}
}
