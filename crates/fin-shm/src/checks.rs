//! What a refusal says (`details.reason`) and the OS-neutral checks of an opened file.

/// Why `fin-shm` refused a file (`SHM_REFUSED.details.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A segment of the path is a symlink, a junction or another reparse point.
    Link,
    /// The file (or a segment that must be a directory) is of the wrong type: a FIFO, a
    /// socket, a device, a directory where a file is expected, a file where a directory is.
    NotRegular,
    /// The file has more than one link (`nlink > 1`).
    HardLinked,
    /// The file is larger than the cap (the kind's, or the caller's).
    OverCap,
    /// The file's size is not the size its ring header declares.
    SizeMismatch,
    /// A read came back short: the file was truncated while it was read.
    Truncated,
    /// The file changed size or identity between a check and its use.
    Changed,
    /// The ring header does not decode: magic, version, reserved bits, zero sizes, too many
    /// slots, or a `slots × slot_bytes` that overflows.
    Header,
    /// A slot index, length or sequence breaks the ring protocol.
    Protocol,
    /// The path is relative or has `.`/`..` segments.
    NotAbsolute,
    /// A name `fin-shm` makes from its input is not allowed (a helper id).
    BadName,
    /// The file does not exist.
    Missing,
    /// The file exists where `fin-shm` creates one.
    Exists,
    /// On Windows: another handle can still write the file (a sharing violation).
    Busy,
    /// Any other I/O failure (`details.cause` has the OS error).
    Io,
}

impl Reason {
    /// The wire spelling (`details.reason`).
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Link => "link",
            Reason::NotRegular => "notRegular",
            Reason::HardLinked => "hardLinked",
            Reason::OverCap => "overCap",
            Reason::SizeMismatch => "sizeMismatch",
            Reason::Truncated => "truncated",
            Reason::Changed => "changed",
            Reason::Header => "header",
            Reason::Protocol => "protocol",
            Reason::NotAbsolute => "notAbsolute",
            Reason::BadName => "badName",
            Reason::Missing => "missing",
            Reason::Exists => "exists",
            Reason::Busy => "busy",
            Reason::Io => "io",
        }
    }
}

/// What an open learns about a file from its handle (`fstat`, `GetFileInformationByHandle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Facts {
    pub kind: Kind,
    pub nlink: u64,
    pub size: u64,
    /// `(st_dev, st_ino)`; Windows: `(volume serial, file index)`.
    pub id: (u64, u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Dir,
    Link,
    Other,
}

/// A failure inside the crate: a reason and what to say. `open` turns it into `SHM_REFUSED`
/// with the path.
#[derive(Debug)]
pub(crate) struct Fail {
    pub reason: Reason,
    pub message: String,
    pub cause: Option<std::io::Error>,
}

impl Fail {
    pub fn new(reason: Reason, message: impl Into<String>) -> Fail {
        Fail {
            reason,
            message: message.into(),
            cause: None,
        }
    }

    pub fn io(e: std::io::Error) -> Fail {
        let reason = match e.kind() {
            std::io::ErrorKind::NotFound => Reason::Missing,
            std::io::ErrorKind::AlreadyExists => Reason::Exists,
            _ => Reason::Io,
        };
        Fail {
            reason,
            message: e.to_string(),
            cause: Some(e),
        }
    }
}

/// The file checks of every open (spec §3.4): a regular file, one link, at most `cap` bytes.
pub(crate) fn check_file(facts: &Facts, cap: u64) -> Result<(), Fail> {
    match facts.kind {
        Kind::File => {}
        Kind::Link => return Err(Fail::new(Reason::Link, "the file is a link")),
        Kind::Dir => return Err(Fail::new(Reason::NotRegular, "the path is a directory")),
        Kind::Other => {
            return Err(Fail::new(
                Reason::NotRegular,
                "the file is not a regular file (a FIFO, socket or device)",
            ));
        }
    }
    if facts.nlink != 1 {
        return Err(Fail::new(
            Reason::HardLinked,
            format!(
                "the file has {} links; a mapped file has exactly one",
                facts.nlink
            ),
        ));
    }
    if facts.size > cap {
        return Err(Fail::new(
            Reason::OverCap,
            format!("the file has {} bytes, over the cap of {cap}", facts.size),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(kind: Kind, nlink: u64, size: u64) -> Facts {
        Facts {
            kind,
            nlink,
            size,
            id: (1, 2),
        }
    }

    #[test]
    fn a_regular_single_link_file_under_the_cap_passes() {
        assert!(check_file(&facts(Kind::File, 1, 10), 10).is_ok());
    }

    #[test]
    fn each_failed_check_has_its_reason() {
        let r = |f: Facts, cap| check_file(&f, cap).unwrap_err().reason;
        assert_eq!(r(facts(Kind::Link, 1, 0), 9), Reason::Link);
        assert_eq!(r(facts(Kind::Dir, 1, 0), 9), Reason::NotRegular);
        assert_eq!(r(facts(Kind::Other, 1, 0), 9), Reason::NotRegular);
        assert_eq!(r(facts(Kind::File, 2, 0), 9), Reason::HardLinked);
        assert_eq!(r(facts(Kind::File, 0, 0), 9), Reason::HardLinked);
        assert_eq!(r(facts(Kind::File, 1, 10), 9), Reason::OverCap);
    }
}
