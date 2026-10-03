#![no_main]
//! Admission never panics, and what it admits obeys the profile's bounds.
use libfuzzer_sys::fuzz_target;
use misaka_transport_policy::limits::{MAX_FILES, MAX_TOTAL_BYTES};
use misaka_transport_policy::{AdmissionEnv, Expectation, admit, names};

fuzz_target!(|data: &[u8]| {
    let ih = misaka_btv2::infohash(data);
    if let Ok(b) = admit(data, &ih, &Expectation::default(), &AdmissionEnv::default()) {
        assert!(b.info().files.len() <= MAX_FILES);
        assert!(b.total_bytes() <= MAX_TOTAL_BYTES);
        for (f, _) in b.files() {
            assert!(names::check_name(&f.name).is_ok());
            assert!(!f.name.contains('/') && !f.name.contains(".."));
        }
    }
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = names::check_name(s);
    }
});
