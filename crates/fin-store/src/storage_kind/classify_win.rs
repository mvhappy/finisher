//! Pure Windows classification over gathered facts (tested on every OS).

use super::StorageKind;
use super::classify::{home_folder_provider, provider_from_name};
use crate::conflict_names::Provider;

pub const DRIVE_REMOTE: u32 = 4;
pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
pub const FILE_ATTRIBUTE_PINNED: u32 = 0x0008_0000;
pub const FILE_ATTRIBUTE_UNPINNED: u32 = 0x0010_0000;
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

/// What the Windows probe found out about a folder. Roots are already canonicalised where
/// possible; every field may be absent.
#[derive(Debug, Clone, Default)]
pub struct WinFacts {
    pub onedrive_roots: Vec<String>,
    pub dropbox_roots: Vec<String>,
    pub user_profile: Option<String>,
    pub local_app_data: Option<String>,
    /// `GetDriveTypeW` of the volume root that holds the folder.
    pub drive_type: Option<u32>,
    /// File attributes of the folder and each ancestor.
    pub attr_chain: Vec<u32>,
    /// `Some(provider name)` when the folder is inside a registered Cloud Files sync root
    /// (`CfGetSyncRootInfoByPath` succeeded); the name may be empty.
    pub cloud_root: Option<String>,
    pub volume_label: Option<String>,
    pub volume_fs: Option<String>,
}

/// Windows path text normalised for comparison: `\\?\` and `\\?\UNC\` forms resolved,
/// separators `\`, lower-cased, no trailing separator.
pub fn normalize_windows(p: &str) -> String {
    let s = p.replace('/', "\\");
    let s = if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        s
    };
    let s = s.to_lowercase();
    let t = s.trim_end_matches('\\');
    if t.is_empty() { s } else { t.to_string() }
}

/// `\\server\share` (any form, including `\\?\UNC\`), not a `\\.\` device path.
pub fn is_unc(normalized: &str) -> bool {
    normalized.starts_with("\\\\") && !normalized.starts_with("\\\\.\\") && normalized.len() > 2
}

fn win_under(path: &str, root: &str) -> bool {
    let root = normalize_windows(root);
    !root.is_empty()
        && path
            .strip_prefix(root.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

/// The first path component below `base`, in the normalised (lower-case) text.
fn below<'a>(path: &'a str, base: &str) -> Option<&'a str> {
    let base = normalize_windows(base);
    let rest = path.strip_prefix(base.as_str())?.strip_prefix('\\')?;
    Some(rest.split('\\').next().unwrap_or(""))
}

/// Windows. Sync evidence is checked first (the dangerous error is missing it): a Cloud
/// Files sync root, the OneDrive/Dropbox roots, Google Drive (volume label or file system,
/// `My Drive`, `DriveFS`), well-known sync folders in the profile, and recall/pin
/// attributes on the folder or an ancestor. Then UNC and remote drives are network.
pub fn classify_windows(path: &str, f: &WinFacts) -> StorageKind {
    let p = normalize_windows(path);
    if let Some(name) = &f.cloud_root {
        return StorageKind::Sync(provider_from_name(name));
    }
    if f.onedrive_roots.iter().any(|r| win_under(&p, r)) {
        return StorageKind::Sync(Provider::OneDrive);
    }
    if f.dropbox_roots.iter().any(|r| win_under(&p, r)) {
        return StorageKind::Sync(Provider::Dropbox);
    }
    let label = f.volume_label.as_deref().unwrap_or("").to_lowercase();
    let fs = f.volume_fs.as_deref().unwrap_or("").to_lowercase();
    if label == "google drive" || fs.contains("drivefs") {
        return StorageKind::Sync(Provider::GoogleDrive);
    }
    if let Some(app) = &f.local_app_data
        && win_under(&p, &format!("{app}\\Google\\DriveFS"))
    {
        return StorageKind::Sync(Provider::GoogleDrive);
    }
    if let Some(name) = f.user_profile.as_deref().and_then(|u| below(&p, u))
        && let Some(provider) = home_folder_provider(name)
    {
        return StorageKind::Sync(provider);
    }
    let marks = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
        | FILE_ATTRIBUTE_RECALL_ON_OPEN
        | FILE_ATTRIBUTE_PINNED
        | FILE_ATTRIBUTE_UNPINNED;
    if f.attr_chain.iter().any(|a| a & marks != 0) {
        return StorageKind::Sync(Provider::Unknown);
    }
    // DRIVE_UNKNOWN (0): could not be determined, so the conservative non-sync kind.
    if is_unc(&p) || matches!(f.drive_type, Some(DRIVE_REMOTE | 0)) {
        return StorageKind::Network;
    }
    StorageKind::Local
}
