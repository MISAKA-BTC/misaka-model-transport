//! `magnet:?xt=urn:btmh:1220<64 hex>&dn=<title>` (BEP 9 with a BEP 52 multihash). `1220` is the
//! multihash prefix of a 32-byte SHA-256. A bare magnet is **unanchored**: it says the bytes are
//! the torrent's, nothing more (RFC-0001 §2.4).

use crate::torrent::Infohash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Magnet {
    pub infohash: Infohash,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MagnetError {
    #[error("not a magnet link")]
    Scheme,
    #[error("the magnet carries no `xt=urn:btmh:1220…` (a v2 infohash), or more than one")]
    NoV2Hash,
    #[error("parameter {0:?} is not accepted (xt, dn and tr only)")]
    Parameter(String),
    #[error("parameter {0:?} is not valid percent-encoded UTF-8")]
    Encoding(String),
}

/// The canonical magnet of a bundle.
pub fn format(infohash: &Infohash, title: &str) -> String {
    format!("magnet:?xt=urn:btmh:1220{infohash}&dn={}", percent_encode(title))
}

/// Parses a magnet that names exactly one v2 infohash. Hex is accepted in either case and
/// normalized.
pub fn parse(s: &str) -> Result<Magnet, MagnetError> {
    let query = s.strip_prefix("magnet:?").ok_or(MagnetError::Scheme)?;
    let mut hash = None;
    let mut dn = None;
    let mut trackers = Vec::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').ok_or_else(|| MagnetError::Parameter(pair.to_owned()))?;
        match k {
            "xt" => {
                let hex = v.strip_prefix("urn:btmh:1220").ok_or(MagnetError::NoV2Hash)?;
                let h = hex.to_ascii_lowercase().parse::<Infohash>().map_err(|_| MagnetError::NoV2Hash)?;
                if hash.replace(h).is_some() {
                    return Err(MagnetError::NoV2Hash);
                }
            }
            "dn" => dn = Some(percent_decode(v).ok_or_else(|| MagnetError::Encoding(k.to_owned()))?),
            "tr" => trackers.push(percent_decode(v).ok_or_else(|| MagnetError::Encoding(k.to_owned()))?),
            _ => return Err(MagnetError::Parameter(k.to_owned())),
        }
    }
    Ok(Magnet { infohash: hash.ok_or(MagnetError::NoV2Hash)?, display_name: dn, trackers })
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
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
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let h = Infohash([0xab; 32]);
        let m = format(&h, "qwen25-1.5b-a16");
        assert_eq!(m, format!("magnet:?xt=urn:btmh:1220{}&dn=qwen25-1.5b-a16", "ab".repeat(32)));
        let p = parse(&m).unwrap();
        assert_eq!(p.infohash, h);
        assert_eq!(p.display_name.as_deref(), Some("qwen25-1.5b-a16"));
        let upper = format!("magnet:?xt=urn:btmh:1220{}&tr=udp%3A%2F%2Ft.example%3A80", "AB".repeat(32));
        let p = parse(&upper).unwrap();
        assert_eq!(p.infohash, h);
        assert_eq!(p.trackers, ["udp://t.example:80"]);
    }

    #[test]
    fn refuses() {
        let hex = "ab".repeat(32);
        for bad in [
            format!("magnet:?xt=urn:btih:{}", "ab".repeat(20)),
            format!("magnet:?xt=urn:btmh:1114{hex}"),
            format!("magnet:?xt=urn:btmh:1220{hex}&xt=urn:btmh:1220{hex}"),
            format!("magnet:?xt=urn:btmh:1220{hex}&xs=http://x"),
            format!("magnet:?xt=urn:btmh:1220{}", &hex[1..]),
            "magnet:?dn=x".to_string(),
            "https://example.com".to_string(),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }
}
