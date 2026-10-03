//! Bencode (BEP 3), strict.
//!
//! The decoder accepts only canonical input — integers without leading zeros or `-0`, string
//! lengths without leading zeros, dictionary keys strictly increasing — so that decoding and
//! re-encoding is the identity, and a parsed info dictionary hashes to the bytes it came from.
//! It is bounded in depth and in node count before any allocation grows with the input.

use std::collections::BTreeMap;

/// A bencoded value. A dictionary's keys sort as raw bytes, which is bencode's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn bytes(b: impl Into<Vec<u8>>) -> Value {
        Value::Bytes(b.into())
    }

    /// A dictionary from `(key, value)` pairs, in any order.
    pub fn dict<K: Into<Vec<u8>>>(pairs: impl IntoIterator<Item = (K, Value)>) -> Value {
        Value::Dict(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
}

/// Bounds on what the decoder will build.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Nesting depth; the outermost value is depth 1.
    pub max_depth: usize,
    /// Total values (integers, strings, lists, dictionaries, keys not counted).
    pub max_nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        // A rule-v1 info dictionary is 4 levels deep; a .torrent 5. 64 files make ~400 nodes.
        Limits { max_depth: 16, max_nodes: 100_000 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("unexpected end of input at byte {0}")]
    Eof(usize),
    #[error("unexpected byte {byte:#04x} at {at}")]
    Unexpected { at: usize, byte: u8 },
    #[error("non-canonical integer at byte {0}")]
    NonCanonicalInt(usize),
    #[error("integer out of range at byte {0}")]
    IntRange(usize),
    #[error("non-canonical string length at byte {0}")]
    NonCanonicalLength(usize),
    #[error("dictionary keys not strictly increasing at byte {0}")]
    KeyOrder(usize),
    #[error("nesting deeper than {0}")]
    TooDeep(usize),
    #[error("more than {0} values")]
    TooManyNodes(usize),
    #[error("{0} trailing bytes")]
    Trailing(usize),
}

/// Decodes exactly one canonical value spanning all of `input`.
pub fn decode(input: &[u8], limits: Limits) -> Result<Value, DecodeError> {
    let mut d = Decoder { input, pos: 0, nodes: 0, limits };
    let v = d.value(1)?;
    if d.pos != input.len() {
        return Err(DecodeError::Trailing(input.len() - d.pos));
    }
    Ok(v)
}

struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
    nodes: usize,
    limits: Limits,
}

impl Decoder<'_> {
    fn peek(&self) -> Result<u8, DecodeError> {
        self.input.get(self.pos).copied().ok_or(DecodeError::Eof(self.pos))
    }

    fn value(&mut self, depth: usize) -> Result<Value, DecodeError> {
        if depth > self.limits.max_depth {
            return Err(DecodeError::TooDeep(self.limits.max_depth));
        }
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return Err(DecodeError::TooManyNodes(self.limits.max_nodes));
        }
        match self.peek()? {
            b'i' => {
                self.pos += 1;
                let v = self.int_until(b'e')?;
                Ok(Value::Int(v))
            }
            b'0'..=b'9' => Ok(Value::Bytes(self.string()?)),
            b'l' => {
                self.pos += 1;
                let mut items = Vec::new();
                while self.peek()? != b'e' {
                    items.push(self.value(depth + 1)?);
                }
                self.pos += 1;
                Ok(Value::List(items))
            }
            b'd' => {
                self.pos += 1;
                let mut map = BTreeMap::new();
                let mut last: Option<Vec<u8>> = None;
                while self.peek()? != b'e' {
                    let at = self.pos;
                    if !self.peek()?.is_ascii_digit() {
                        return Err(DecodeError::Unexpected { at, byte: self.peek()? });
                    }
                    let k = self.string()?;
                    if last.as_ref().is_some_and(|l| *l >= k) {
                        return Err(DecodeError::KeyOrder(at));
                    }
                    let v = self.value(depth + 1)?;
                    last = Some(k.clone());
                    map.insert(k, v);
                }
                self.pos += 1;
                Ok(Value::Dict(map))
            }
            byte => Err(DecodeError::Unexpected { at: self.pos, byte }),
        }
    }

    /// A canonical decimal integer terminated by `end`; the cursor ends past `end`.
    fn int_until(&mut self, end: u8) -> Result<i64, DecodeError> {
        let start = self.pos;
        let rel = self.input[start..].iter().position(|&b| b == end).ok_or(DecodeError::Eof(self.input.len()))?;
        let digits = &self.input[start..start + rel];
        self.pos = start + rel + 1;
        let (neg, body) = match digits.split_first() {
            Some((b'-', rest)) => (true, rest),
            _ => (false, digits),
        };
        let canonical = !body.is_empty()
            && body.iter().all(u8::is_ascii_digit)
            && (body == b"0" || body[0] != b'0')
            && !(neg && body == b"0");
        if !canonical {
            return Err(DecodeError::NonCanonicalInt(start));
        }
        std::str::from_utf8(digits).expect("ASCII").parse().map_err(|_| DecodeError::IntRange(start))
    }

    fn string(&mut self) -> Result<Vec<u8>, DecodeError> {
        let start = self.pos;
        let len = self.int_until(b':').map_err(|_| DecodeError::NonCanonicalLength(start))?;
        if len < 0 {
            return Err(DecodeError::NonCanonicalLength(start));
        }
        let len = len as usize;
        let end =
            self.pos.checked_add(len).filter(|&e| e <= self.input.len()).ok_or(DecodeError::Eof(self.input.len()))?;
        let s = self.input[self.pos..end].to_vec();
        self.pos = end;
        Ok(s)
    }
}

/// Encodes `v` canonically.
pub fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(v, &mut out);
    out
}

pub fn encode_into(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Int(i) => {
            out.push(b'i');
            out.extend_from_slice(i.to_string().as_bytes());
            out.push(b'e');
        }
        Value::Bytes(b) => {
            out.extend_from_slice(b.len().to_string().as_bytes());
            out.push(b':');
            out.extend_from_slice(b);
        }
        Value::List(l) => {
            out.push(b'l');
            for item in l {
                encode_into(item, out);
            }
            out.push(b'e');
        }
        Value::Dict(d) => {
            out.push(b'd');
            for (k, v) in d {
                encode_into(&Value::Bytes(k.clone()), out);
                encode_into(v, out);
            }
            out.push(b'e');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &[u8]) -> Result<Value, DecodeError> {
        decode(s, Limits::default())
    }

    #[test]
    fn round_trip() {
        let inputs: [&[u8]; 6] = [b"i0e", b"i-42e", b"0:", b"4:spam", b"l4:spami42ee", b"d3:bar4:spam3:fooi42ee"];
        for i in inputs {
            assert_eq!(encode(&dec(i).unwrap()), i);
        }
    }

    #[test]
    fn refuses_noncanonical() {
        let bad: [&[u8]; 14] = [
            b"i-0e",
            b"i03e",
            b"ie",
            b"i-e",
            b"i+1e",
            b"03:abc",
            b"-1:",
            b"d3:fooi1e3:bari2ee",
            b"d3:fooi1e3:fooi2ee",
            b"di1ei2ee",
            b"i1ei2e",
            b"5:abc",
            b"l",
            b"i99999999999999999999e",
        ];
        for b in bad {
            assert!(dec(b).is_err(), "{:?}", String::from_utf8_lossy(b));
        }
    }

    #[test]
    fn bounded() {
        let deep = [b"l".repeat(100), b"e".repeat(100)].concat();
        assert_eq!(dec(&deep), Err(DecodeError::TooDeep(16)));
        let wide = [b"l".to_vec(), b"i0e".repeat(200_000), b"e".to_vec()].concat();
        assert_eq!(dec(&wide), Err(DecodeError::TooManyNodes(100_000)));
        // A string length past the end does not allocate it.
        assert!(matches!(dec(b"999999999999:x"), Err(DecodeError::Eof(_))));
    }
}
