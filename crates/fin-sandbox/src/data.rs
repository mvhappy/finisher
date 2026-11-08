//! Data files shipped with the crate (`data/<os>/`), parsed here so every list has one reader.

use crate::policy::Os;

const TOOLCHAIN_LINUX: &str = include_str!("../data/linux/toolchain-base.txt");
const TOOLCHAIN_MACOS: &str = include_str!("../data/macos/toolchain-base.txt");
const TOOLCHAIN_WINDOWS: &str = include_str!("../data/windows/toolchain-base.txt");

/// The base toolchain read list of `os` (spec §8.1 item 1): one absolute path per line;
/// `#` starts a comment. Windows entries may start with `%SystemRoot%` or `%ProgramFiles%`,
/// which the caller expands. The lists are placeholders until spike S9 (plan 7C Task 0).
pub fn toolchain_base(os: Os) -> Vec<&'static str> {
    let text = match os {
        Os::Linux => TOOLCHAIN_LINUX,
        Os::Macos => TOOLCHAIN_MACOS,
        Os::Windows => TOOLCHAIN_WINDOWS,
    };
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_absolute_for_its_os() {
        for p in toolchain_base(Os::Linux)
            .into_iter()
            .chain(toolchain_base(Os::Macos))
        {
            assert!(
                p.starts_with('/') && !p.ends_with('/') && !p.contains("//") && !p.contains("/."),
                "{p}"
            );
        }
        for p in toolchain_base(Os::Windows) {
            assert!(
                p.starts_with('%') || p.as_bytes().get(1) == Some(&b':'),
                "{p}"
            );
        }
        for os in [Os::Linux, Os::Macos, Os::Windows] {
            assert!(!toolchain_base(os).is_empty(), "{os:?}");
        }
    }
}
