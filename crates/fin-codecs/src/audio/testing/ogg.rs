//! Ogg pages (RFC 3533) around any packets, and a minimal Vorbis I stream: real identification,
//! comment and setup headers (one codebook, one unused floor 1, one residue, one mapping, one
//! short-block mode) and audio packets whose floors are all unused, so they decode to silence.
//! Enough to exercise the Ogg reader and the Vorbis mapping without an encoder (MP3, Vorbis and
//! ALAC audio come from the fixture cache).

/// CRC-32 as Ogg uses it: polynomial 0x04C11DB7, MSB first, no reflection, no final xor.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// One page: `segments` lacing values over `body`.
fn page(serial: u32, seq: u32, flags: u8, granule: i64, segments: &[u8], body: &[u8]) -> Vec<u8> {
    let mut p = b"OggS".to_vec();
    p.push(0);
    p.push(flags);
    p.extend_from_slice(&granule.to_le_bytes());
    p.extend_from_slice(&serial.to_le_bytes());
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&[0; 4]);
    p.push(segments.len() as u8);
    p.extend_from_slice(segments);
    p.extend_from_slice(body);
    let crc = crc32(&p);
    p[22..26].copy_from_slice(&crc.to_le_bytes());
    p
}

/// An Ogg stream of `packets`, each `(data, granule)` starting a new page (the granule is
/// written on the page where the packet ends; pages a long packet spans before that say −1).
/// The first page is BOS and the last EOS.
pub fn ogg(serial: u32, packets: &[(Vec<u8>, i64)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut seq = 0u32;
    for (i, (data, granule)) in packets.iter().enumerate() {
        let mut lacing: Vec<u8> = vec![255; data.len() / 255];
        lacing.push((data.len() % 255) as u8);
        let mut at = 0;
        let mut first = true;
        for (k, segs) in lacing.chunks(255).enumerate() {
            let last = (k + 1) * 255 >= lacing.len();
            let len: usize = segs.iter().map(|&s| s as usize).sum();
            let mut flags = 0u8;
            if !first {
                flags |= 1;
            }
            if i == 0 && first {
                flags |= 2;
            }
            if i + 1 == packets.len() && last {
                flags |= 4;
            }
            let g = if last { *granule } else { -1 };
            out.extend_from_slice(&page(serial, seq, flags, g, segs, &data[at..at + len]));
            seq += 1;
            at += len;
            first = false;
        }
    }
    out
}

/// An LSB-first bit writer (Vorbis packs its headers that way).
#[derive(Default)]
struct Lsb {
    bytes: Vec<u8>,
    n: u32,
}

impl Lsb {
    fn put(&mut self, value: u64, bits: u32) {
        for i in 0..bits {
            if self.n.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let bit = ((value >> i) & 1) as u8;
            *self.bytes.last_mut().expect("pushed") |= bit << (self.n % 8);
            self.n += 1;
        }
    }
}

/// The identification header: short blocks of 256, long of 2,048.
pub fn vorbis_ident(channels: u8, rate: u32) -> Vec<u8> {
    let mut p = vec![1];
    p.extend_from_slice(b"vorbis");
    p.extend_from_slice(&0u32.to_le_bytes());
    p.push(channels);
    p.extend_from_slice(&rate.to_le_bytes());
    p.extend_from_slice(&[0; 12]);
    p.push(0xB8);
    p.push(1);
    p
}

fn vorbis_comment() -> Vec<u8> {
    let vendor = b"finisher test writer";
    let mut p = vec![3];
    p.extend_from_slice(b"vorbis");
    p.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    p.extend_from_slice(vendor);
    p.extend_from_slice(&0u32.to_le_bytes());
    p.push(1);
    p
}

fn vorbis_setup() -> Vec<u8> {
    let mut b = Lsb::default();
    b.put(0, 8); // one codebook
    b.put(0x564342, 24);
    b.put(1, 16); // dimensions
    b.put(2, 24); // entries
    b.put(0, 1); // not length-ordered
    b.put(0, 1); // not sparse
    b.put(0, 5); // entry 0: length 1
    b.put(0, 5); // entry 1: length 1
    b.put(0, 4); // no lookup
    b.put(0, 6); // one time-domain transform
    b.put(0, 16);
    b.put(0, 6); // one floor
    b.put(1, 16); // floor 1
    b.put(0, 5); // no partitions
    b.put(0, 2); // multiplier 1
    b.put(8, 4); // rangebits
    b.put(0, 6); // one residue
    b.put(0, 16); // residue 0
    b.put(0, 24); // begin
    b.put(0, 24); // end
    b.put(0, 24); // partition size 1
    b.put(0, 6); // one classification
    b.put(0, 8); // classbook 0
    b.put(0, 3); // cascade: no books
    b.put(0, 1);
    b.put(0, 6); // one mapping
    b.put(0, 16); // mapping 0
    b.put(0, 1); // one submap
    b.put(0, 1); // no coupling
    b.put(0, 2); // reserved
    b.put(0, 8); // submap 0: time
    b.put(0, 8); // floor 0
    b.put(0, 8); // residue 0
    b.put(0, 6); // one mode
    b.put(0, 1); // short block
    b.put(0, 16);
    b.put(0, 16);
    b.put(0, 8); // mapping 0
    b.put(1, 1); // framing
    let mut p = vec![5];
    p.extend_from_slice(b"vorbis");
    p.extend_from_slice(&b.bytes);
    p
}

/// An Ogg Vorbis stream of `packets` silent short blocks (256 samples, 128 new samples each after
/// the first). Its length is `(packets − 1) × 128` frames.
pub fn vorbis_silence(channels: u8, rate: u32, packets: usize) -> Vec<u8> {
    let mut all = vec![
        (vorbis_ident(channels, rate), 0),
        (vorbis_comment(), 0),
        (vorbis_setup(), 0),
    ];
    for k in 0..packets {
        // Audio packet, mode 0, every channel's floor unused.
        all.push((vec![0u8; 2], (k as i64) * 128));
    }
    ogg(0x5EED, &all)
}

/// The Ogg Opus identification header (RFC 7845 §5.1) for an MP4 `dOps` body (ISO/IEC 23003-5
/// §4.3.2): the same fields, little-endian, behind `OpusHead` and version 1.
pub fn opus_head(dops: &[u8]) -> Vec<u8> {
    let be16 = |i: usize| u16::from_be_bytes([dops[i], dops[i + 1]]);
    let mut p = b"OpusHead".to_vec();
    p.push(1);
    p.push(dops[1]);
    p.extend_from_slice(&be16(2).to_le_bytes());
    p.extend_from_slice(&u32::from_be_bytes([dops[4], dops[5], dops[6], dops[7]]).to_le_bytes());
    p.extend_from_slice(&be16(8).to_le_bytes());
    p.extend_from_slice(&dops[10..]);
    p
}

fn opus_tags() -> Vec<u8> {
    let vendor = b"finisher test writer";
    let mut p = b"OpusTags".to_vec();
    p.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    p.extend_from_slice(vendor);
    p.extend_from_slice(&0u32.to_le_bytes());
    p
}

/// An Ogg Opus stream (RFC 7845) of encoded `packets` of `frame_size` samples each, with the
/// header from `dops`. Each packet ends a page whose granule position counts the samples
/// decoded so far (pre-skip included); the last page's is `pre-skip + valid`, which tells a
/// reader to drop the padding at the end.
pub fn opus(dops: &[u8], packets: &[Vec<u8>], frame_size: u32, valid: u64) -> Vec<u8> {
    let pre_skip = u16::from_be_bytes([dops[2], dops[3]]) as i64;
    let end = pre_skip + valid as i64;
    let mut all = vec![(opus_head(dops), 0), (opus_tags(), 0)];
    for (k, p) in packets.iter().enumerate() {
        let granule = ((k as i64 + 1) * frame_size as i64).min(end);
        all.push((p.clone(), granule));
    }
    ogg(0x0905, &all)
}
