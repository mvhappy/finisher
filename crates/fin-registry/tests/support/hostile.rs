//! Zip archives written byte by byte, so a test can build what no well-behaved writer would:
//! lying sizes, duplicate names, any name bytes, any mode, any flag.
#![allow(dead_code)]

use std::io::Write;

/// One entry of a hand-built archive.
#[derive(Clone)]
pub struct E {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    pub deflate: bool,
    /// The full unix mode (type and permissions); `None` writes a DOS entry.
    pub mode: Option<u32>,
    /// The uncompressed size to declare instead of the real one.
    pub declared: Option<u32>,
    /// Stored as the compressed bytes as is (never a valid deflate stream).
    pub fake_compressed: Option<Vec<u8>>,
    /// `None`: set when the name is non-ASCII UTF-8.
    pub utf8_flag: Option<bool>,
    pub encrypted: bool,
}

impl E {
    pub fn file(name: &str, data: &[u8]) -> E {
        E {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            deflate: false,
            mode: Some(0o100_644),
            declared: None,
            fake_compressed: None,
            utf8_flag: None,
            encrypted: false,
        }
    }

    pub fn dir(name: &str) -> E {
        E {
            mode: Some(0o040_755),
            ..E::file(&format!("{}/", name.trim_end_matches('/')), b"")
        }
    }

    pub fn raw(name: &[u8], data: &[u8]) -> E {
        E {
            name: name.to_vec(),
            ..E::file("x", data)
        }
    }

    pub fn mode(mut self, mode: u32) -> E {
        self.mode = Some(mode);
        self
    }

    pub fn deflated(mut self) -> E {
        self.deflate = true;
        self
    }

    pub fn declared(mut self, n: u32) -> E {
        self.declared = Some(n);
        self
    }

    pub fn fake(mut self, compressed: Vec<u8>, declared: u32) -> E {
        self.deflate = true;
        self.fake_compressed = Some(compressed);
        self.declared = Some(declared);
        self
    }

    pub fn utf8_flag(mut self, on: bool) -> E {
        self.utf8_flag = Some(on);
        self
    }

    pub fn encrypted(mut self) -> E {
        self.encrypted = true;
        self
    }
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut d = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
    d.write_all(data).unwrap();
    d.finish().unwrap()
}

fn u16le(v: &mut Vec<u8>, n: u16) {
    v.extend_from_slice(&n.to_le_bytes());
}

fn u32le(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}

/// The archive's bytes.
pub fn zip(entries: &[E]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let compressed = match (&e.fake_compressed, e.deflate) {
            (Some(c), _) => c.clone(),
            (None, true) => deflate(&e.data),
            (None, false) => e.data.clone(),
        };
        let crc = crc32fast::hash(&e.data);
        let size = e.declared.unwrap_or(e.data.len() as u32);
        let utf8 = e
            .utf8_flag
            .unwrap_or(!e.name.is_ascii() && std::str::from_utf8(&e.name).is_ok());
        let flags = if utf8 { 0x0800 } else { 0 } | u16::from(e.encrypted);
        let method = if e.deflate { 8 } else { 0 };
        let (time, date) = (0u16, 0x21u16);
        let offset = out.len() as u32;
        u32le(&mut out, 0x0403_4b50);
        u16le(&mut out, 20);
        u16le(&mut out, flags);
        u16le(&mut out, method);
        u16le(&mut out, time);
        u16le(&mut out, date);
        u32le(&mut out, crc);
        u32le(&mut out, compressed.len() as u32);
        u32le(&mut out, size);
        u16le(&mut out, e.name.len() as u16);
        u16le(&mut out, 0);
        out.extend_from_slice(&e.name);
        out.extend_from_slice(&compressed);

        let (made_by, external) = match e.mode {
            Some(m) => ((3u16 << 8) | 20, m << 16),
            None => (20u16, if e.name.ends_with(b"/") { 0x10 } else { 0 }),
        };
        u32le(&mut central, 0x0201_4b50);
        u16le(&mut central, made_by);
        u16le(&mut central, 20);
        u16le(&mut central, flags);
        u16le(&mut central, method);
        u16le(&mut central, time);
        u16le(&mut central, date);
        u32le(&mut central, crc);
        u32le(&mut central, compressed.len() as u32);
        u32le(&mut central, size);
        u16le(&mut central, e.name.len() as u16);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u32le(&mut central, external);
        u32le(&mut central, offset);
        central.extend_from_slice(&e.name);
    }
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    u32le(&mut out, 0x0605_4b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, entries.len() as u16);
    u16le(&mut out, entries.len() as u16);
    u32le(&mut out, cd_size);
    u32le(&mut out, cd_offset);
    u16le(&mut out, 0);
    out
}
