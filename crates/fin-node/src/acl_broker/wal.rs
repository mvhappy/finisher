//! The broker's write-ahead log: one JSON record per line, each appended and fsynced before
//! the operation it names is applied (`grant`, `revoke`), and a `done` marker after. A torn
//! last line (a crash while it was written) is dropped; any other line that does not parse is
//! a damaged log, which the broker refuses to guess about.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Grant, same_key};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Record {
    Grant {
        id: u64,
        path: PathBuf,
        sid: String,
        mask: u32,
        inherit: bool,
        owner: String,
    },
    Revoke {
        id: u64,
        path: PathBuf,
        sid: String,
        owner: String,
    },
    Done {
        id: u64,
    },
}

impl Record {
    pub fn id(&self) -> u64 {
        match self {
            Record::Grant { id, .. } | Record::Revoke { id, .. } | Record::Done { id } => *id,
        }
    }

    pub fn grant(id: u64, g: &Grant) -> Record {
        Record::Grant {
            id,
            path: g.path.clone(),
            sid: g.sid.clone(),
            mask: g.mask,
            inherit: g.inherit,
            owner: g.owner.clone(),
        }
    }
}

/// What a log says: the done grants, and the operations not done, in log order.
#[derive(Debug, Default)]
pub struct Replay {
    pub granted: BTreeMap<u64, Grant>,
    pub pending: Vec<Record>,
}

/// Replays `records`: a done grant is granted (replacing its pair's earlier record); a revoke
/// removes its pair at once (done or not) and drops an earlier grant of the pair that is not
/// done (a rolled-back grant); what is not done is pending.
pub fn replay(records: &[Record]) -> Replay {
    let mut granted: BTreeMap<u64, Grant> = BTreeMap::new();
    let mut pending: BTreeMap<u64, Record> = BTreeMap::new();
    let pair = |r: &Record| match r {
        Record::Grant { path, sid, .. } | Record::Revoke { path, sid, .. } => {
            Some((path.clone(), sid.clone()))
        }
        Record::Done { .. } => None,
    };
    for r in records {
        match r {
            Record::Grant { id, .. } => {
                pending.insert(*id, r.clone());
            }
            Record::Revoke { id, path, sid, .. } => {
                granted.retain(|_, g| !(same_key(&g.path, path) && &g.sid == sid));
                pending.retain(|_, p| {
                    !matches!(p, Record::Grant { .. })
                        || pair(p).is_none_or(|(pp, ps)| !(same_key(&pp, path) && &ps == sid))
                });
                pending.insert(*id, r.clone());
            }
            // A revoke's marker, or a second marker of one id, changes nothing more.
            Record::Done { id } => {
                if let Some(Record::Grant {
                    path,
                    sid,
                    mask,
                    inherit,
                    owner,
                    ..
                }) = pending.remove(id)
                {
                    granted.retain(|_, g| !(same_key(&g.path, &path) && g.sid == sid));
                    granted.insert(
                        *id,
                        Grant {
                            path,
                            sid,
                            mask,
                            inherit,
                            owner,
                        },
                    );
                }
            }
        }
    }
    Replay {
        granted,
        pending: pending.into_values().collect(),
    }
}

/// The open log.
pub struct Wal {
    path: PathBuf,
    file: File,
    lines: usize,
}

fn open_append(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).append(true).read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path)
}

/// Fsyncs the directory of `path` (Unix: makes a rename durable; Windows' rename is).
fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl Wal {
    /// Opens (creating) the log at `path` and reads its records.
    pub fn open(path: &Path) -> io::Result<(Wal, Vec<Record>)> {
        let mut file = open_append(path)?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let mut records = Vec::new();
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            let last = i + 1 == lines.len();
            match serde_json::from_str::<Record>(line.trim_end()) {
                Ok(r) if line.ends_with('\n') => records.push(r),
                // Torn while written: never applied (the apply waits for the fsync).
                _ if last => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("line {} of {} is damaged", i + 1, path.display()),
                    ));
                }
            }
        }
        let n = records.len();
        Ok((
            Wal {
                path: path.to_path_buf(),
                file,
                lines: n,
            },
            records,
        ))
    }

    /// Appends `r` and fsyncs it.
    pub fn append(&mut self, r: &Record) -> io::Result<()> {
        let mut line = serde_json::to_string(r).map_err(io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_data()?;
        self.lines += 1;
        Ok(())
    }

    /// Replaces the log with `records`: a temp file, fsynced, renamed over it.
    pub fn rewrite(&mut self, records: &[Record]) -> io::Result<()> {
        let tmp = self.path.with_extension("wal.tmp");
        let mut o = OpenOptions::new();
        o.create(true).write(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&tmp)?;
        let mut text = String::new();
        for r in records {
            text.push_str(&serde_json::to_string(r).map_err(io::Error::other)?);
            text.push('\n');
        }
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        // The old handle closes before the rename (Windows), and the new one, at the end of
        // what it wrote, follows its file to the log's name.
        self.file = f;
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            self.file = open_append(&self.path)?;
            return Err(e);
        }
        sync_dir(&self.path)?;
        self.lines = records.len();
        Ok(())
    }

    pub fn lines(&self) -> usize {
        self.lines
    }
}
