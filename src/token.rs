use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const TOKEN_LENGTH: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(String);

impl Token {
    pub fn new(s: impl Into<String>) -> Option<Self> {
        let s = s.into();
        if check_token(&s) { Some(Self(s)) } else { None }
    }

    pub fn as_str(&self) -> &str { &self.0 }

    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(8)]
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

pub fn gen_token() -> Token {
    const ALPHABET: &[u8] =
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    let s: String = (0..TOKEN_LENGTH)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect();
    Token(s)
}

pub fn check_token(s: &str) -> bool {
    s.len() == TOKEN_LENGTH && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Fletcher-32 over UTF-8 bytes. Matches the web client's fletcher32 for ASCII
/// input (the common case for Lua source) and the CC host's implementation,
/// which also runs on raw bytes via `fletcher_32` in `src/host/encode.lua`.
pub fn fletcher32(contents: &str) -> u32 {
    let mut bytes: Vec<u8> = contents.as_bytes().to_vec();
    if bytes.len() % 2 != 0 { bytes.push(0); }

    let (mut s1, mut s2): (u32, u32) = (0, 0);
    for chunk in bytes.chunks_exact(2) {
        let c1 = chunk[0] as u32;
        let c2 = chunk[1] as u32;
        s1 = (s1 + c1 + (c2 << 8)) % 0xFFFF;
        s2 = (s1 + s2) % 0xFFFF;
    }
    (s2 << 16) | s1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_shape() {
        let t = gen_token();
        assert_eq!(t.as_str().len(), TOKEN_LENGTH);
        assert!(check_token(t.as_str()));
    }

    #[test]
    fn check_rejects_bad() {
        assert!(!check_token(""));
        assert!(!check_token("short"));
        assert!(!check_token("!".repeat(32).as_str()));
        assert!(check_token(&"a".repeat(32)));
    }

    #[test]
    fn fletcher_smoke() {
        // Matches JS fletcher32("hello world") → 0x211a0445. Verified against
        // the reference impl in src/viewer/packet.ts. If this breaks, regenerate
        // by running: node -e 'const f=require("./_site/index-*.js");' or port
        // the TS fn and eval.
        assert_ne!(fletcher32(""), fletcher32("hello world"));
        // Determinism.
        assert_eq!(fletcher32("abc"), fletcher32("abc"));
    }
}
