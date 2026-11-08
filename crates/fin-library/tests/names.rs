//! Display names, keys and collisions (plan 5D Task 1, Decisions 10–11).

use fin_library::names::{dir_key, fold, nfc, rel_key};
use fin_library::raw::from_wide;
use fin_library::{Collision, RawRel, collisions};

#[test]
fn nfc_keys_and_lossy_names() {
    let n = nfc("Cafe\u{301}.mov".as_bytes());
    assert_eq!(n.nfc, "Caf\u{e9}.mov");
    assert!(!n.lossy);
    let n = nfc(b"a\xffb");
    assert_eq!(n.nfc, "a\u{FFFD}b");
    assert!(n.lossy);
    // A Windows name with a lone surrogate is lossy too.
    let n = nfc(&from_wide(&[0xD800, 0x61]));
    assert_eq!(n.nfc, "\u{FFFD}\u{FFFD}\u{FFFD}a");
    assert!(n.lossy);
    // A valid name that holds U+FFFD itself is not lossy.
    assert!(!nfc("\u{FFFD}".as_bytes()).lossy);

    assert_eq!(rel_key("", "Caf\u{e9}.mov", true), "Caf\u{e9}.mov");
    assert_eq!(rel_key("Media/2026", "x.mov", true), "Media/2026/x.mov");
    let rel = RawRel::root().join("Me\u{301}dia".as_bytes()).join(b"\xff");
    assert_eq!(dir_key(&rel, true), "M\u{e9}dia/\u{FFFD}");
    assert_eq!(dir_key(&RawRel::root(), false), "");
}

#[test]
fn case_folded_keys_only_on_insensitive_roots() {
    assert_eq!(rel_key("Media", "Clip.MOV", true), "Media/Clip.MOV");
    assert_eq!(rel_key("media", "Clip.MOV", false), "media/clip.mov");
    let rel = RawRel::root()
        .join(b"Media")
        .join("\u{C9}T\u{C9}".as_bytes());
    assert_eq!(dir_key(&rel, false), "media/\u{e9}t\u{e9}");
    assert_eq!(dir_key(&rel, true), "Media/\u{C9}T\u{C9}");
    // Keys stay NFC after folding: İ lower-cases to i + U+0307 (no precomposed form).
    assert_eq!(fold("\u{130}", false), "i\u{307}");
    assert_eq!(fold("\u{1E9E}", false), "\u{DF}");
    assert_eq!(fold("\u{212A}", false), "k");
}

fn bits(names: &[&[u8]], case_sensitive: bool) -> Vec<(usize, u8)> {
    collisions(names, case_sensitive)
        .into_iter()
        .map(|(i, c)| (i, c.bits()))
        .collect()
}

#[test]
fn collision_bits() {
    const N: u8 = Collision::NFC;
    const C: u8 = Collision::CASE;
    const L: u8 = Collision::LOSSY;
    let nfd = "e\u{301}.mov".as_bytes();
    let nfc_ = "\u{e9}.mov".as_bytes();
    // NFC/NFD twins collide on every root.
    for cs in [true, false] {
        assert_eq!(bits(&[nfd, b"x", nfc_], cs), vec![(0, N), (2, N)]);
    }
    // Case twins only on a case-insensitive root.
    assert_eq!(bits(&[b"A.mov", b"a.mov"], false), vec![(0, C), (1, C)]);
    assert!(bits(&[b"A.mov", b"a.mov"], true).is_empty());
    // Two lossy names both decode to U+FFFD; so does a valid name holding U+FFFD.
    assert_eq!(bits(&[b"\xff", b"\xfe"], true), vec![(0, L), (1, L)]);
    assert_eq!(
        bits(&[b"\xff", "\u{FFFD}".as_bytes()], true),
        vec![(0, L), (1, L)]
    );
    let w1 = from_wide(&[0xD800]);
    let w2 = from_wide(&[0xDC00]);
    assert_eq!(bits(&[&w1, &w2], true), vec![(0, L), (1, L)]);
    // `to_lowercase` folds ß/ẞ together (NTFS's upcase table keeps them apart, so both can
    // exist on disk): flagged, never fatal.
    assert_eq!(
        bits(
            &["stra\u{DF}e".as_bytes(), "stra\u{1E9E}e".as_bytes()],
            false
        ),
        vec![(0, C), (1, C)]
    );
    // Turkish dotted I: İ folds to i + U+0307, so it meets that spelling; I meets i; the
    // dotless ı meets nothing.
    let dotted = "\u{130}.mov".as_bytes();
    let i_dot = "i\u{307}.mov".as_bytes();
    assert_eq!(bits(&[dotted, i_dot], false), vec![(0, C), (1, C)]);
    assert_eq!(
        bits(&[b"I.mov", b"i.mov", "\u{131}.mov".as_bytes()], false),
        vec![(0, C), (1, C)]
    );
    // Kelvin sign and k.
    assert_eq!(
        bits(&["\u{212A}".as_bytes(), b"k"], false),
        vec![(0, C), (1, C)]
    );
    // Three ways: É (NFD), É (NFC), é: the É pair is NFC twins, each meets é by case.
    let e_nfd = "E\u{301}".as_bytes();
    let e_nfc = "\u{C9}".as_bytes();
    let e_low = "\u{e9}".as_bytes();
    assert_eq!(
        bits(&[e_low, e_nfd, b"z", e_nfc], false),
        vec![(0, C), (1, N | C), (3, N | C)]
    );
    assert_eq!(
        bits(&[e_low, e_nfd, b"z", e_nfc], true),
        vec![(1, N), (3, N)]
    );
    // Distinct names with distinct keys never collide.
    assert!(bits(&[b"a", b"b", b"\xff"], false).is_empty());
    let c = Collision(N | L);
    assert!(c.has(N) && c.has(L) && !c.has(C) && !c.is_empty());
}
