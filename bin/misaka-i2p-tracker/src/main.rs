//! `misaka-i2p-tracker` — an HTTP tracker for I2P swarms (RFC-0002 §4.1, MP-11).
//!
//! It listens on loopback; an I2P server tunnel publishes it as `<b32>.b32.i2p`. It speaks the
//! convention libtorrent implements for I2P torrents:
//! - an announce carries `ip=<base64 destination>.i2p`, in I2P's base64 alphabet (`-` and `~`);
//! - the tracker stores the SHA-256 of the decoded destination, which is its `.b32.i2p` name;
//! - the response's `peers` is a string of 32-byte destination hashes.
//!
//! Announces live in memory only and expire after two intervals. Nothing is logged but counts: no
//! destination, and no infohash beside a destination.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(name = "misaka-i2p-tracker", version, about = "HTTP tracker for I2P swarms (RFC-0002)")]
struct Cli {
    /// Loopback address the I2P server tunnel forwards to.
    #[arg(long, default_value = "127.0.0.1:7662")]
    listen: String,
    /// Seconds between announces asked of clients.
    #[arg(long, default_value_t = 900)]
    interval: u64,
}

const MAX_SWARMS: usize = 10_000;
const MAX_PEERS: usize = 2_000;
const MAX_WANT: usize = 50;
const MAX_REQUEST: usize = 8 << 10;

type Swarms = HashMap<Vec<u8>, HashMap<[u8; 32], Instant>>;

struct Tracker {
    swarms: Mutex<Swarms>,
    interval: Duration,
}

/// Decodes I2P's base64: the standard alphabet with `-` for `+` and `~` for `/`, `=` padding.
fn i2p_base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'~' => 63,
            _ => return None,
        } as u32)
    };
    let t = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(t.len() * 3 / 4);
    for chunk in t.chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(out)
}

fn bstr(out: &mut Vec<u8>, b: &[u8]) {
    out.extend(b.len().to_string().as_bytes());
    out.push(b':');
    out.extend(b);
}

fn failure(reason: &str) -> Vec<u8> {
    let mut o = b"d14:failure reason".to_vec();
    bstr(&mut o, reason.as_bytes());
    o.push(b'e');
    o
}

impl Tracker {
    /// Handles one announce query; returns the bencoded body.
    fn announce(&self, query: &str) -> Vec<u8> {
        let mut info_hash = None;
        let mut ip = None;
        let mut event = None;
        let mut numwant = MAX_WANT;
        for kv in query.split('&') {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match k {
                "info_hash" => info_hash = percent_decode(v),
                "ip" => ip = percent_decode(v).and_then(|b| String::from_utf8(b).ok()),
                "event" => event = Some(v.to_owned()),
                "numwant" => numwant = v.parse::<usize>().unwrap_or(MAX_WANT).min(MAX_WANT),
                _ => {}
            }
        }
        let Some(ih) = info_hash.filter(|h| h.len() == 20 || h.len() == 32) else {
            return failure("info_hash must be 20 or 32 bytes");
        };
        let Some(dest) =
            ip.as_deref().and_then(|d| d.strip_suffix(".i2p")).and_then(i2p_base64).filter(|d| d.len() >= 387)
        else {
            return failure("ip must be a base64 I2P destination ending in .i2p");
        };
        let me: [u8; 32] = Sha256::digest(&dest).into();
        let now = Instant::now();
        let ttl = self.interval * 2;
        let mut swarms = self.swarms.lock().expect("unpoisoned");
        if !swarms.contains_key(&ih) && swarms.len() >= MAX_SWARMS {
            swarms.retain(|_, peers| {
                peers.retain(|_, t| now.duration_since(*t) < ttl);
                !peers.is_empty()
            });
            if swarms.len() >= MAX_SWARMS {
                return failure("tracker full");
            }
        }
        let peers = swarms.entry(ih.clone()).or_default();
        peers.retain(|_, t| now.duration_since(*t) < ttl);
        if event.as_deref() == Some("stopped") {
            peers.remove(&me);
        } else if peers.len() < MAX_PEERS || peers.contains_key(&me) {
            peers.insert(me, now);
        }
        let mut list = Vec::with_capacity(numwant * 32);
        for p in peers.keys().filter(|p| **p != me).take(numwant) {
            list.extend(p);
        }
        let count = peers.len();
        if peers.is_empty() {
            swarms.remove(&ih);
        }
        let mut o = b"d8:completei0e10:incompletei".to_vec();
        o.extend(count.to_string().as_bytes());
        o.extend(b"e8:intervali");
        o.extend(self.interval.as_secs().to_string().as_bytes());
        o.extend(b"e5:peers");
        bstr(&mut o, &list);
        o.push(b'e');
        o
    }

    fn serve(&self, mut s: TcpStream) {
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend(&chunk[..n]),
            }
            if buf.len() > MAX_REQUEST {
                return;
            }
        }
        let line = String::from_utf8_lossy(&buf[..buf.iter().position(|&b| b == b'\r').unwrap_or(0)]).into_owned();
        let mut parts = line.split(' ');
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let (status, body) = if method != "GET" {
            ("405 Method Not Allowed", failure("GET only"))
        } else if path == "/announce" || path == "/a" {
            ("200 OK", self.announce(query))
        } else {
            ("404 Not Found", failure("announce at /announce"))
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(&body));
    }
}

fn main() {
    let cli = Cli::parse();
    let listener = TcpListener::bind(&cli.listen).unwrap_or_else(|e| {
        eprintln!("misaka-i2p-tracker: {}: {e}", cli.listen);
        std::process::exit(1)
    });
    if !listener.local_addr().is_ok_and(|a| a.ip().is_loopback()) {
        eprintln!("misaka-i2p-tracker: refusing a non-loopback listen address; the I2P tunnel forwards to loopback");
        std::process::exit(1);
    }
    let t =
        Arc::new(Tracker { swarms: Mutex::new(HashMap::new()), interval: Duration::from_secs(cli.interval.max(60)) });
    eprintln!("misaka-i2p-tracker: listening on {} (interval {}s)", cli.listen, cli.interval);
    {
        let t = t.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(300));
                let s = t.swarms.lock().expect("unpoisoned");
                let peers: usize = s.values().map(HashMap::len).sum();
                eprintln!("misaka-i2p-tracker: {} swarms, {peers} peers", s.len());
            }
        });
    }
    for s in listener.incoming().flatten() {
        let t = t.clone();
        std::thread::spawn(move || t.serve(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(seed: u8) -> String {
        // 387 bytes of a fake destination, in I2P's base64.
        let raw: Vec<u8> = (0..387u32).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
        let std = |b: &[u8]| -> String {
            const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-~";
            let mut o = String::new();
            for c in b.chunks(3) {
                let n = (u32::from(c[0]) << 16)
                    | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                    | u32::from(*c.get(2).unwrap_or(&0));
                for i in 0..=c.len() {
                    o.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
                }
            }
            while o.len() % 4 != 0 {
                o.push('=');
            }
            o
        };
        std(&raw)
    }

    #[test]
    fn base64_round_trip() {
        let d = dest(7);
        let raw = i2p_base64(&d).unwrap();
        assert_eq!(raw.len(), 387);
        assert_eq!(raw[1], 31u8.wrapping_add(7));
    }

    #[test]
    fn announces_return_other_peers_as_hashes() {
        let t = Tracker { swarms: Mutex::new(HashMap::new()), interval: Duration::from_secs(900) };
        let ih = "%01".repeat(20);
        let a = dest(1);
        let b = dest(2);
        let ra = t.announce(&format!("info_hash={ih}&ip={a}.i2p&port=6881"));
        assert!(ra.ends_with(b"5:peers0:e"), "{}", String::from_utf8_lossy(&ra));
        let rb = t.announce(&format!("info_hash={ih}&ip={b}.i2p&port=6881"));
        let ha: [u8; 32] = Sha256::digest(i2p_base64(&a).unwrap()).into();
        let mut want = b"5:peers32:".to_vec();
        want.extend(ha);
        want.push(b'e');
        assert!(rb.ends_with(&want));
        // Stopped removes the announcer.
        t.announce(&format!("info_hash={ih}&ip={a}.i2p&event=stopped"));
        let rb = t.announce(&format!("info_hash={ih}&ip={b}.i2p"));
        assert!(rb.ends_with(b"5:peers0:e"));
        // Garbage is refused.
        assert!(t.announce("info_hash=x&ip=nope").starts_with(b"d14:failure reason"));
        assert!(t.announce(&format!("info_hash={ih}&ip=1.2.3.4")).starts_with(b"d14:failure reason"));
    }
}
