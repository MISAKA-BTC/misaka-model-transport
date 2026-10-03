#![no_main]
//! Strict bencode: whatever decodes encodes back to the same bytes.
use libfuzzer_sys::fuzz_target;
use misaka_btv2::bencode::{Limits, decode, encode};

fuzz_target!(|data: &[u8]| {
    if let Ok(v) = decode(data, Limits::default()) {
        assert_eq!(encode(&v), data);
    }
});
