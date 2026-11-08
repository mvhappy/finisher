//! Key collisions inside one directory listing (Decision 11).
//!
//! Two raw names collide when their keys ([`crate::names`]) are equal. Every such name keeps
//! its own row and gets bits saying how its key met another's. A name's key is computed in
//! three steps (decode, NFC, case fold); a pair is flagged by the first step after which
//! the two are equal:
//! - [`Collision::LOSSY`]: their decodings are equal already (two invalid names that both
//!   became U+FFFD, or an invalid name and a valid one holding U+FFFD);
//! - [`Collision::NFC`]: their decodings differ, their NFC forms are equal (NFC/NFD twins);
//! - [`Collision::CASE`]: their NFC forms differ, their folded forms are equal (case twins,
//!   `ß`/`ẞ`, `İ`/`i̇`, `K`/`K` (Kelvin): only on a case-insensitive root).
//!
//! The bit values are the catalog's `collision` column (Task 7b computes the same per
//! closed listing).

use std::collections::HashMap;

use crate::names;

/// Collision bits of one name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Collision(pub u8);

impl Collision {
    pub const NFC: u8 = 1;
    pub const CASE: u8 = 2;
    pub const LOSSY: u8 = 4;

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// The colliding names of a listing, as `(index into names, bits)` sorted by index; names
/// that collide with nothing are absent. `names` are a directory's raw names (distinct).
pub fn collisions(names: &[&[u8]], case_sensitive: bool) -> Vec<(usize, Collision)> {
    struct Forms {
        decoded: String,
        nfc: String,
    }
    let forms: Vec<Forms> = names
        .iter()
        .map(|raw| {
            let decoded = names::decode(raw).0;
            let nfc = names::nfc(raw).nfc;
            Forms { decoded, nfc }
        })
        .collect();
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in forms.iter().enumerate() {
        groups
            .entry(names::fold(&f.nfc, case_sensitive))
            .or_default()
            .push(i);
    }
    let mut out: Vec<(usize, Collision)> = Vec::new();
    for members in groups.values().filter(|m| m.len() > 1) {
        for &i in members {
            let mut bits = 0;
            for &j in members.iter().filter(|&&j| j != i) {
                bits |= if forms[i].decoded == forms[j].decoded {
                    Collision::LOSSY
                } else if forms[i].nfc == forms[j].nfc {
                    Collision::NFC
                } else {
                    Collision::CASE
                };
            }
            out.push((i, Collision(bits)));
        }
    }
    out.sort_by_key(|&(i, _)| i);
    out
}
