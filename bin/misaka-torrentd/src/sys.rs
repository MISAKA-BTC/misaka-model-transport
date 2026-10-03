//! Host queries: free space, volume size, randomness.

// The statvfs field widths differ between platforms; the casts are not no-ops everywhere.
#![allow(clippy::unnecessary_cast)]

use std::path::Path;

/// Bytes available to an unprivileged writer on the volume holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    let s = statvfs(path)?;
    Some((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
}

/// The volume's size.
pub fn volume_size(path: &Path) -> Option<u64> {
    let s = statvfs(path)?;
    Some((s.f_blocks as u64).saturating_mul(s.f_frsize as u64))
}

#[cfg(unix)]
fn statvfs(path: &Path) -> Option<libc::statvfs> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: a valid C string and an out-pointer to a zeroed struct.
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        (libc::statvfs(c.as_ptr(), &mut s) == 0).then_some(s)
    }
}

#[cfg(not(unix))]
fn statvfs(_: &Path) -> Option<()> {
    None
}

/// A random listen port in 49152–65535 (§6.3). The range holds none of misakas's ports
/// (P2P 26111…, RPC 26110–28610, 8545, 8787–8791, 3030, 11434).
pub fn random_port() -> u16 {
    let mut b = [0u8; 2];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b)).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        b = (t.subsec_nanos() as u16 ^ std::process::id() as u16).to_le_bytes();
    }
    49152 + u16::from_le_bytes(b) % (65535 - 49152 + 1)
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
