//! A registry in a local folder (spec §5.2): a directory holding `metadata/` and `targets/`,
//! read-only to Finisher. Only regular files are read; a symlink is refused, so a pulled
//! clone cannot point a role file outside the folder.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;

use super::{Transport, TransportError, components};

#[derive(Debug, Clone)]
pub struct FsTransport {
    root: PathBuf,
}

impl FsTransport {
    pub fn new(root: impl Into<PathBuf>) -> FsTransport {
        FsTransport { root: root.into() }
    }
}

impl Transport for FsTransport {
    fn fetch(&self, path: &str, max: u64) -> Result<Vec<u8>, TransportError> {
        let parts = components(path)?;
        let unreachable = |e: std::io::Error| TransportError::Unreachable {
            path: path.into(),
            reason: e.to_string(),
        };
        let mut p = self.root.clone();
        for (i, c) in parts.iter().enumerate() {
            p.push(c);
            let meta = match std::fs::symlink_metadata(&p) {
                Ok(m) => m,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    return Err(TransportError::NotFound { path: path.into() });
                }
                Err(e) => return Err(unreachable(e)),
            };
            let last = i + 1 == parts.len();
            let ok = if last { meta.is_file() } else { meta.is_dir() };
            if !ok {
                return Err(TransportError::Unreachable {
                    path: path.into(),
                    reason: "not a regular file or directory".into(),
                });
            }
        }
        let f = File::open(&p).map_err(unreachable)?;
        let mut out = Vec::new();
        f.take(max.saturating_add(1))
            .read_to_end(&mut out)
            .map_err(unreachable)?;
        if out.len() as u64 > max {
            return Err(TransportError::TooLarge {
                path: path.into(),
                max,
            });
        }
        Ok(out)
    }
}
