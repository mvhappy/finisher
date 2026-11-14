//! The job engine's name tables (5C-1 plan, Decision 42), referenced from
//! [`super::names::TABLES`]. Drift tests in `fin-jobs` and `fin-workers` keep the registered
//! kinds and families inside them.

use serde::{Deserialize, Deserializer};

use super::class::Name;
use super::names::NameTable;

/// Job kinds. `test.*` (`test.stage` is the `library` family's test kind), 5D's `library.*`,
/// sub-project 6's `media.*` (6A: `media.look`, `media.capabilities`; 6B-1: `media.probeDeep`);
/// 8C adds its own.
pub static JOB_KINDS: NameTable = NameTable::new(
    "JOB_KINDS",
    &[
        "test.sleep",
        "test.echo",
        "test.progressFlood",
        "test.warm",
        "test.crash",
        "test.hang",
        "test.stall",
        "test.spawn",
        "test.files",
        "test.checkpoint",
        "test.oom",
        "test.call",
        "test.diag",
        "test.garbage",
        "test.exitAfterResult",
        "test.helper",
        "test.badHello",
        "test.pluginMinor",
        "test.fail",
        "test.stage",
        "library.scan",
        "library.probe",
        "library.hash",
        "media.look",
        "media.capabilities",
        "media.probeDeep",
        "media.proxy",
        "media.thumbs",
        "media.pcm",
    ],
);

/// Worker families (`fin-workers <family>`). `testbad` and `testplugin` are the test-only
/// families of `test.badHello` and `test.pluginMinor`.
pub static WORKER_FAMILIES: NameTable = NameTable::new(
    "WORKER_FAMILIES",
    &[
        "test",
        "testbad",
        "testplugin",
        "library",
        "media",
        "analyze",
        "plugin",
    ],
);

/// Terminating signals, lower case without `SIG`.
pub static SIGNALS: NameTable = NameTable::new(
    "SIGNALS",
    &[
        "kill", "term", "int", "hup", "segv", "bus", "abrt", "ill", "fpe", "pipe", "xcpu", "sys",
        "trap",
    ],
);

pub fn job_kind<'de, D: Deserializer<'de>>(d: D) -> Result<Name, D::Error> {
    Ok(JOB_KINDS.get_or_other(&String::deserialize(d)?))
}

pub fn worker_family<'de, D: Deserializer<'de>>(d: D) -> Result<Name, D::Error> {
    Ok(WORKER_FAMILIES.get_or_other(&String::deserialize(d)?))
}

pub fn signal_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Name>, D::Error> {
    let raw = Option::<String>::deserialize(d)?;
    Ok(raw.map(|s| SIGNALS.get_or_other(&s)))
}
