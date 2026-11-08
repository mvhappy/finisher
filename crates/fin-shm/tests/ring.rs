//! Ring round trips: in one process, across two processes (this test binary re-run as the
//! writer), sealed and untrusted readers, `open_sized` on a 5C-made file, and the mapping rule
//! (an untrusted reader never maps; counted by the `test-hooks` `mmap` counter).

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use fin_core::CoreError;
use fin_shm::{RingReader, RingSpec, RingWriter, SlotState, encode_header, hooks};

const CAP: u64 = 64 << 20;
const CHILD_ENV: &str = "FIN_SHM_TEST_CHILD_RING";
const ROUNDS: u64 = 1500;
const SLOTS: u32 = 4;
const SLOT_BYTES: u64 = 4096;

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    (t, root)
}

fn reason(e: &CoreError) -> &str {
    assert_eq!(e.code, "SHM_REFUSED", "{e:?}");
    e.details["reason"].as_str().unwrap()
}

/// The payload of slot `i` in round `r`: its length and every byte follow from `(r, i)`, so a
/// torn copy (bytes from two rounds) never verifies.
fn payload(r: u64, i: u32) -> Vec<u8> {
    let len = 16 + ((r * 7 + u64::from(i) * 13) % (SLOT_BYTES - 16)) as usize;
    let mut v: Vec<u8> = (0..len)
        .map(|k| (r as usize * 31 + i as usize * 17 + k) as u8)
        .collect();
    v[..8].copy_from_slice(&r.to_le_bytes());
    v[8..12].copy_from_slice(&i.to_le_bytes());
    v
}

fn verify(buf: &[u8], i: u32) -> u64 {
    let r = u64::from_le_bytes(buf[..8].try_into().unwrap());
    assert_eq!(buf, payload(r, i).as_slice(), "slot {i} round {r} is torn");
    r
}

fn spec() -> RingSpec {
    RingSpec {
        slot_bytes: SLOT_BYTES,
        slots: SLOTS,
    }
}

/// A zero-filled file at the ring's final size, as 5C creates a `MappedFile`.
fn sized_file(path: &Path, spec: RingSpec) {
    let f = File::create_new(path).unwrap();
    f.set_len(spec.file_bytes().unwrap()).unwrap();
}

#[test]
fn a_ring_round_trips_in_one_process() {
    let (_t, root) = tmp();
    let path = root.join("ring");
    let mut w = RingWriter::create(&path, spec(), CAP).unwrap();
    assert_eq!(w.write_slot(0, b"hello").unwrap(), 2);
    assert_eq!(w.write_slot(0, b"again").unwrap(), 4);
    assert_eq!(
        w.write_slot_with(2, 3, |b| b.copy_from_slice(b"abc"))
            .unwrap(),
        2
    );
    let mut r = RingReader::open_untrusted(&path, CAP).unwrap();
    assert_eq!(r.spec(), spec());
    let mut buf = vec![9u8; 7];
    assert_eq!(
        r.read_slot(0, &mut buf).unwrap(),
        SlotState::Ready { seq: 4, len: 5 }
    );
    assert_eq!(buf, b"again");
    assert_eq!(r.read_slot(1, &mut buf).unwrap(), SlotState::Empty);
    assert!(buf.is_empty());
    assert_eq!(
        r.read_slot(2, &mut buf).unwrap(),
        SlotState::Ready { seq: 2, len: 3 }
    );
    assert_eq!(buf, b"abc");
    assert_eq!(
        reason(&r.read_slot(SLOTS, &mut buf).unwrap_err()),
        "protocol"
    );
    assert_eq!(reason(&w.write_slot(SLOTS, b"x").unwrap_err()), "protocol");
    let big = vec![0u8; SLOT_BYTES as usize + 1];
    assert_eq!(reason(&w.write_slot(0, &big).unwrap_err()), "protocol");
}

#[test]
fn an_untrusted_reader_never_maps_and_a_sealed_one_does() {
    let (_t, root) = tmp();
    let path = root.join("ring");
    let before = hooks::mmap_calls();
    let mut w = RingWriter::create(&path, spec(), CAP).unwrap();
    assert_eq!(
        hooks::mmap_calls(),
        before + 1,
        "the writer maps its own file"
    );
    w.write_slot(1, b"frame").unwrap();
    w.flush().unwrap();
    let mut buf = Vec::new();
    let mut r = RingReader::open_untrusted(&path, CAP).unwrap();
    for i in 0..SLOTS {
        r.read_slot(i, &mut buf).unwrap();
    }
    assert!(!r.is_mapped());
    assert_eq!(
        hooks::mmap_calls(),
        before + 1,
        "open_untrusted and read_slot never map"
    );
    drop(w);
    let mut s = RingReader::open_sealed(&path, CAP).unwrap();
    assert!(s.is_mapped());
    assert_eq!(hooks::mmap_calls(), before + 2);
    assert_eq!(
        s.read_slot(1, &mut buf).unwrap(),
        SlotState::Ready { seq: 2, len: 5 }
    );
    assert_eq!(buf, b"frame");
    assert_eq!(s.read_slot(0, &mut buf).unwrap(), SlotState::Empty);
}

#[test]
fn open_sized_writes_the_header_of_a_5c_file_and_keeps_an_existing_ring() {
    let (_t, root) = tmp();
    let path = root.join("ring");
    sized_file(&path, spec());
    let mut w = RingWriter::open_sized(&path, spec(), CAP).unwrap();
    assert_eq!(w.write_slot(3, b"one").unwrap(), 2);
    drop(w);
    let mut w = RingWriter::open_sized(&path, spec(), CAP).unwrap();
    assert_eq!(w.write_slot(3, b"two").unwrap(), 4, "sequences continue");
    drop(w);
    let other = RingSpec {
        slot_bytes: SLOT_BYTES / 2,
        slots: SLOTS,
    };
    let e = RingWriter::open_sized(&path, other, CAP).unwrap_err();
    assert_eq!(reason(&e), "sizeMismatch");
    let same_size = RingSpec {
        slot_bytes: SLOT_BYTES * 2,
        slots: SLOTS / 2,
    };
    let p2 = root.join("ring2");
    sized_file(&p2, spec());
    std::fs::OpenOptions::new()
        .write(true)
        .open(&p2)
        .unwrap()
        .write_all(&encode_header(&same_size))
        .unwrap();
    // The right size for `spec()`, but another ring's header.
    let e = RingWriter::open_sized(&p2, spec(), CAP).unwrap_err();
    assert_eq!(reason(&e), "header");
}

#[test]
fn a_writer_abandoned_mid_write_leaves_a_busy_slot() {
    let (_t, root) = tmp();
    let path = root.join("ring");
    let mut w = RingWriter::create(&path, spec(), CAP).unwrap();
    w.write_slot(0, b"done").unwrap();
    drop(w);
    let l = spec().layout().unwrap();
    let mut f = OpenOptions::new().write(true).open(&path).unwrap();
    f.seek(SeekFrom::Start(l.seq_offset(0))).unwrap();
    f.write_all(&3u64.to_le_bytes()).unwrap();
    drop(f);
    let mut buf = Vec::new();
    let mut r = RingReader::open_untrusted(&path, CAP).unwrap();
    assert_eq!(r.read_slot(0, &mut buf).unwrap(), SlotState::Busy);
    let mut s = RingReader::open_sealed(&path, CAP).unwrap();
    assert_eq!(s.read_slot(0, &mut buf).unwrap(), SlotState::Busy);
}

#[test]
fn a_length_over_the_slot_size_is_a_protocol_error() {
    let (_t, root) = tmp();
    let path = root.join("ring");
    drop(RingWriter::create(&path, spec(), CAP).unwrap());
    let l = spec().layout().unwrap();
    let mut f = OpenOptions::new().write(true).open(&path).unwrap();
    f.seek(SeekFrom::Start(l.seq_offset(1))).unwrap();
    f.write_all(&2u64.to_le_bytes()).unwrap();
    f.write_all(&(SLOT_BYTES + 1).to_le_bytes()).unwrap();
    drop(f);
    let mut buf = Vec::new();
    let mut r = RingReader::open_untrusted(&path, CAP).unwrap();
    assert_eq!(reason(&r.read_slot(1, &mut buf).unwrap_err()), "protocol");
    let mut s = RingReader::open_sealed(&path, CAP).unwrap();
    assert_eq!(reason(&s.read_slot(1, &mut buf).unwrap_err()), "protocol");
}

/// The writer half of the two-process test: a no-op unless the parent set `CHILD_ENV`.
#[test]
fn child_writer() {
    let Some(path) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let mut w = RingWriter::open_sized(Path::new(&path), spec(), CAP).unwrap();
    for r in 1..=ROUNDS {
        for i in 0..SLOTS {
            w.write_slot(i, &payload(r, i)).unwrap();
        }
    }
}

#[test]
fn a_ring_round_trips_across_two_processes() {
    let (_t, root) = tmp();
    let path = root.join("ring");
    sized_file(&path, spec());
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_writer", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, &path)
        .spawn()
        .unwrap();
    // The header appears once the child has opened the file.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut r = loop {
        match RingReader::open_untrusted(&path, CAP) {
            Ok(r) => break r,
            Err(e) if reason(&e) == "header" && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("{e:?}"),
        }
    };
    let mut buf = Vec::new();
    let mut last = [0u64; SLOTS as usize];
    let (mut ready, mut busy) = (0u64, 0u64);
    while last.iter().any(|r| *r < ROUNDS) {
        assert!(
            Instant::now() < deadline,
            "the reader saw rounds {last:?} of {ROUNDS}"
        );
        for i in 0..SLOTS {
            match r.read_slot(i, &mut buf).unwrap() {
                SlotState::Ready { len, .. } => {
                    assert_eq!(len as usize, buf.len());
                    let round = verify(&buf, i);
                    assert!(
                        round >= last[i as usize],
                        "slot {i} went back from {} to {round}",
                        last[i as usize]
                    );
                    last[i as usize] = round;
                    ready += 1;
                }
                SlotState::Busy => busy += 1,
                SlotState::Empty => {}
            }
        }
        if let Some(status) = child.try_wait().unwrap()
            && !status.success()
        {
            panic!("the writer failed: {status}");
        }
    }
    assert!(child.wait().unwrap().success());
    assert!(ready > 0, "busy {busy}");
    let mut s = RingReader::open_sealed(&path, CAP).unwrap();
    for i in 0..SLOTS {
        let state = s.read_slot(i, &mut buf).unwrap();
        assert_eq!(
            state,
            SlotState::Ready {
                seq: 2 * ROUNDS,
                len: payload(ROUNDS, i).len() as u64
            }
        );
        assert_eq!(verify(&buf, i), ROUNDS);
    }
}
