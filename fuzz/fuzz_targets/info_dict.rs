#![no_main]
//! Rule v1: an info dictionary that parses encodes back to the same bytes; a torrent that parses
//! has piece layers that imply its roots.
use libfuzzer_sys::fuzz_target;
use misaka_btv2::torrent::{InfoDict, Torrent};
use misaka_transport_policy::limits::{INFO_DECODE_LIMITS, TORRENT_DECODE_LIMITS};

fuzz_target!(|data: &[u8]| {
    if let Ok(i) = InfoDict::parse_canonical(data, INFO_DECODE_LIMITS) {
        assert_eq!(i.encode(), data);
    }
    if let Ok(t) = Torrent::parse(data, TORRENT_DECODE_LIMITS) {
        t.check_piece_layers().expect("parse checked them");
    }
});
