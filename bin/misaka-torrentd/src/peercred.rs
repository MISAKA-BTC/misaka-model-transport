//! The peer of a Unix socket: `SO_PEERCRED` on Linux, `getpeereid` on the BSDs and macOS (§6.2).

use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCred {
    pub uid: u32,
    pub gid: u32,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn peer_cred(s: &UnixStream) -> std::io::Result<PeerCred> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: a valid fd and an out-pointer of the right size.
    let rc = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PeerCred { uid: cred.uid, gid: cred.gid })
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn peer_cred(s: &UnixStream) -> std::io::Result<PeerCred> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: a valid fd and two out-pointers.
    if unsafe { libc::getpeereid(s.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PeerCred { uid, gid })
}

pub fn own_uid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}
