//! Random tokens and hashing for sessions.

use sha2::{Digest, Sha256};

/// Random token of `bytes` bytes from the OS RNG, hex-encoded.
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS random number generator unavailable");
    hex::encode(buf)
}

/// Uniform random integer in `min..=max`.
pub fn random_range(min: u32, max: u32) -> u32 {
    if max <= min {
        return min;
    }
    let mut buf = [0u8; 8];
    getrandom::fill(&mut buf).expect("OS random number generator unavailable");
    let span = u64::from(max - min) + 1;
    min + (u64::from_le_bytes(buf) % span) as u32
}

pub fn sha256_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_tokens_differ() {
        let a = random_token(32);
        assert_eq!(a.len(), 64);
        assert_ne!(a, random_token(32));
    }

    #[test]
    fn sha256_is_stable() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
