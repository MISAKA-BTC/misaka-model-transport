#![no_main]
//! A descriptor that parses re-serializes to exactly its input.
use libfuzzer_sys::fuzz_target;
use misaka_bundle::Descriptor;

fuzz_target!(|data: &[u8]| {
    if let Ok(d) = Descriptor::parse_canonical(data) {
        assert_eq!(d.to_canonical_bytes(), data);
        let _ = misaka_bundle::bundle_commitment(data);
    }
});
