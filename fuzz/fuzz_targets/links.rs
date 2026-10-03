#![no_main]
//! `misaka-model://` and `magnet:` parse strictly and round-trip.
use libfuzzer_sys::fuzz_target;
use misaka_bundle::link::ModelLink;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    if let Ok(l) = s.parse::<ModelLink>() {
        assert_eq!(l.to_string(), s);
    }
    if let Ok(m) = misaka_btv2::magnet::parse(s) {
        let again = misaka_btv2::magnet::format(&m.infohash, m.display_name.as_deref().unwrap_or(""));
        assert_eq!(misaka_btv2::magnet::parse(&again).unwrap().infohash, m.infohash);
    }
});
