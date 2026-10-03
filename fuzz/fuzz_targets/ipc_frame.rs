#![no_main]
//! A frame from a local peer never panics the daemon's reader.
use libfuzzer_sys::fuzz_target;
use misaka_transport_ipc::{Request, Response, read_frame};

fuzz_target!(|data: &[u8]| {
    let _ = read_frame::<Request, _>(&mut &data[..]);
    let _ = read_frame::<Response, _>(&mut &data[..]);
    if let Ok(r) = borsh::from_slice::<Request>(data) {
        assert_eq!(borsh::to_vec(&r).unwrap(), data);
    }
});
