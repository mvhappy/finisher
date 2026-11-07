//! The reader's caps (spec §6.4).

/// Caps on a package; [`Limits::default`] is spec §6.4's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Entries in the archive, directories included.
    pub entries: u32,
    /// Bytes of all files once unpacked.
    pub unpacked: u64,
    /// Unpacked bytes per compressed byte, per entry.
    pub ratio: u32,
    /// Bytes of a package path.
    pub path_bytes: usize,
    /// Bytes of one path component.
    pub component_bytes: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            entries: 10_000,
            unpacked: 512 * 1024 * 1024,
            ratio: 200,
            path_bytes: 1024,
            component_bytes: 255,
        }
    }
}
