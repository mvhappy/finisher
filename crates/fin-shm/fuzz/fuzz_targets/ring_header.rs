//! Any bytes decode as a ring header or fail with `Reason::Header`, never a panic; a decoded
//! header re-encodes to the same bytes, and its layout is consistent and fits in a `u64`.
#![no_main]

use fin_shm::{
    DATA_ALIGN, HEADER_BYTES, MAX_SLOTS, Reason, SLOT_ENTRY_BYTES, decode_header, encode_header,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match decode_header(data) {
        Err(r) => assert_eq!(r, Reason::Header),
        Ok(l) => {
            assert_eq!(encode_header(&l.spec)[..], data[..HEADER_BYTES as usize]);
            assert!((1..=MAX_SLOTS).contains(&l.spec.slots) && l.spec.slot_bytes > 0);
            let last = l.spec.slots - 1;
            assert_eq!(l.data_offset % DATA_ALIGN, 0);
            assert!(l.seq_offset(last) + SLOT_ENTRY_BYTES <= l.data_offset);
            assert_eq!(l.slot_offset(last) + l.spec.slot_bytes, l.file_bytes);
            assert_eq!(l.spec.file_bytes(), Some(l.file_bytes));
        }
    }
});
