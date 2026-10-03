//! # misaka-torrentd
//!
//! The MISAKA Torrent daemon (RFC-0001 §6). It holds its store, its resume state and its
//! sockets; it downloads, seeds, verifies pieces (L1) and refuses by policy. It knows no chain,
//! holds no key, executes nothing it receives, and listens on nothing but its peer port and its
//! local socket.

pub mod config;
pub mod confine;
pub mod core;
#[cfg(unix)]
pub mod peercred;
#[cfg(unix)]
pub mod server;
pub mod store;
pub mod sys;
