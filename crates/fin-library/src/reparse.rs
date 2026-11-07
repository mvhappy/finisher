//! Windows reparse tags (Decision 8), pure so every OS tests them.
//!
//! Only **name-surrogate** tags (the `0x2000_0000` bit, `IsReparseTagNameSurrogate`:
//! symlinks, junctions, mount points, WSL symlinks and any future tag with the bit) are links:
//! skipped and never followed. Every other tag is **data-bearing** (a hydrated Cloud Files
//! file, Data Deduplication, WOF compression, other filter tags): the file is ordinary unless
//! an attribute marks it a placeholder ([`crate::placeholder`]).

/// The name-surrogate bit of a reparse tag.
pub const NAME_SURROGATE_BIT: u32 = 0x2000_0000;

pub const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
pub const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
pub const IO_REPARSE_TAG_LX_SYMLINK: u32 = 0xA000_001D;
pub const IO_REPARSE_TAG_DEDUP: u32 = 0x8000_0013;
pub const IO_REPARSE_TAG_NFS: u32 = 0x8000_0014;
pub const IO_REPARSE_TAG_WOF: u32 = 0x8000_0017;
pub const IO_REPARSE_TAG_APPEXECLINK: u32 = 0x8000_001B;
pub const IO_REPARSE_TAG_AF_UNIX: u32 = 0x8000_0023;
/// `IO_REPARSE_TAG_CLOUD`; `CLOUD_1` … `CLOUD_F` set bits 12–15 (`0x9000_101A` …).
pub const IO_REPARSE_TAG_CLOUD: u32 = 0x9000_001A;
/// The bits that vary between the `CLOUD*` tags.
pub const IO_REPARSE_TAG_CLOUD_MASK: u32 = 0x0000_F000;

/// `IsReparseTagNameSurrogate`: the tag names another file (a link), never data.
pub fn is_name_surrogate(tag: u32) -> bool {
    tag & NAME_SURROGATE_BIT != 0
}

/// One of the Cloud Files tags (`IO_REPARSE_TAG_CLOUD` and `CLOUD_1` … `CLOUD_F`).
pub fn is_cloud(tag: u32) -> bool {
    tag & !IO_REPARSE_TAG_CLOUD_MASK == IO_REPARSE_TAG_CLOUD
}
