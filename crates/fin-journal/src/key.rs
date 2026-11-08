//! The per-install key (plan D, Decisions 6, 7 and 20): 32 random bytes in `events/key`
//! (0600), made on the first allowed write. Ids, authors and argument hashes are BLAKE3 keyed
//! with it and truncated to 128 bits (`k1:`); blob addresses keep all 256 (`kb:`). The key
//! never leaves the machine, so a stored hash cannot be reversed by a dictionary elsewhere.
//! `purge --all` deletes it: old and new hashes are then unlinkable.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fin_core::events::{BlobHash, Hashed, IdRef};

use crate::files;
use crate::options::KeyFn;

pub(crate) const KEY_FILE: &str = "key";

/// The install key, and whether `events/key` holds it yet.
pub(crate) struct Key {
    bytes: [u8; 32],
    persisted: AtomicBool,
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

impl Key {
    /// A new key, in memory only until [`Key::persist`]. `None` only if the OS has no
    /// random source.
    pub fn generate() -> Option<Key> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).ok()?;
        Some(Key {
            bytes,
            persisted: AtomicBool::new(false),
        })
    }

    /// The key in `dir/key`, if there is a valid one. Reads only.
    pub fn load(dir: &Path) -> Option<Key> {
        let mut text = String::new();
        fs::File::open(dir.join(KEY_FILE))
            .ok()?
            .read_to_string(&mut text)
            .ok()?;
        let text = text.trim();
        if text.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(Key {
            bytes,
            persisted: AtomicBool::new(true),
        })
    }

    /// The key on disk, or a new one.
    pub fn load_or_generate(dir: &Path) -> Option<Key> {
        Key::load(dir).or_else(Key::generate)
    }

    /// `events/key` was deleted (`purge all`): the writer writes it again before the next
    /// line.
    pub fn forget_persisted(&self) {
        self.persisted.store(false, Ordering::Release);
    }

    pub fn persisted(&self) -> bool {
        self.persisted.load(Ordering::Acquire)
    }

    /// Writes `dir/key` (temp-then-rename, 0600, fsynced) unless it is already there.
    pub fn persist(&self, dir: &Path) -> io::Result<()> {
        if self.persisted() {
            return Ok(());
        }
        files::ensure_dir(dir)?;
        let hex: String = self.bytes.iter().map(|b| format!("{b:02x}")).collect();
        write_private(dir, KEY_FILE, format!("{hex}\n").as_bytes())?;
        self.persisted.store(true, Ordering::Release);
        Ok(())
    }

    /// The first 128 bits of BLAKE3 keyed with the install key.
    pub fn hash16(&self, data: &[u8]) -> [u8; 16] {
        let full = blake3::keyed_hash(&self.bytes, data);
        let mut out = [0u8; 16];
        out.copy_from_slice(&full.as_bytes()[..16]);
        out
    }

    pub fn blob_hash(&self, data: &[u8]) -> BlobHash {
        BlobHash::from_bytes(blake3::keyed_hash(&self.bytes, data).as_bytes())
    }
}

/// Writes `dir/name` temp-then-rename, 0600 on unix, fsynced with its directory.
pub(crate) fn write_private(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp = write_temp(dir, name, bytes)?;
    files::rename(&tmp, &dir.join(name))?;
    files::sync_dir(dir)
}

/// The first half of [`write_private`]: `dir/.<name>.tmp`, written and fsynced. A crash
/// before the rename leaves it, which the next scan removes.
pub(crate) fn write_temp(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut o = OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp)?;
    f.write_all(bytes)?;
    files::sync_file(&f)?;
    Ok(tmp)
}

/// Removes `dir/name`; a missing file is fine.
pub(crate) fn remove(dir: &Path, name: &str) -> io::Result<()> {
    files::remove(&dir.join(name))
}

/// The 16-byte keyed hash a journal uses: the test override, else the install key.
pub(crate) fn hasher(over: Option<&KeyFn>, key: Option<Arc<Key>>) -> Option<KeyFn> {
    if let Some(f) = over {
        return Some(f.clone());
    }
    let key = key?;
    Some(Arc::new(move |d: &[u8]| key.hash16(d)))
}

/// [`Hashed`] and [`IdRef`] under a hash function.
pub(crate) fn hashed(f: &KeyFn, raw: &str) -> Hashed {
    Hashed::keyed(raw.as_bytes(), &**f)
}

pub(crate) fn id_ref(f: &KeyFn, raw: &str) -> IdRef {
    IdRef::keyed(raw, &**f)
}

impl crate::Journal {
    /// A keyed hash that is always this journal's install key in force (after `purge all`,
    /// the new one), for a second store whose ids and hashes must join this one's
    /// ([`crate::JournalOptions::key`] of the agent-trace store; its blob addresses stay its
    /// own). Once this journal is gone it keys with a random key of its own (unlinkable).
    pub fn shared_keyer(&self) -> Option<KeyFn> {
        let weak = std::sync::Arc::downgrade(self.h.as_ref()?);
        let fallback = Key::generate()?;
        Some(std::sync::Arc::new(move |b: &[u8]| {
            match weak.upgrade().and_then(|h| h.sh.hasher()) {
                Some(f) => f(b),
                None => fallback.hash16(b),
            }
        }))
    }

    /// The keyed hash of `raw` (`k1:…`, Decision 6), or `None` while off: no key is made
    /// or read for it then.
    pub fn hash(&self, raw: &str) -> Option<Hashed> {
        let h = self.h.as_ref()?;
        self.gate()?;
        Some(hashed(&h.sh.hasher()?, raw))
    }

    /// The keyed hash in force (16 bytes), or `None` while off: for content an emitter keys
    /// itself (an edit intent, `TxIntent::from_txs`). Take it once per entry.
    pub fn keyer(&self) -> Option<crate::KeyFn> {
        let h = self.h.as_ref()?;
        self.gate()?;
        h.sh.hasher()
    }

    /// `raw` as an [`IdRef`] (Decision 7): a canonical ULID kept, anything else keyed-hashed;
    /// `None` while off.
    pub fn id_ref(&self, raw: &str) -> Option<IdRef> {
        let h = self.h.as_ref()?;
        self.gate()?;
        Some(id_ref(&h.sh.hasher()?, raw))
    }
}
