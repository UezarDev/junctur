//! FNV-1a 64, the one hash behind every cache key and stamp Junctur writes. Hand-rolled because std's
//! `DefaultHasher` may change between Rust releases, and a key on disk must survive a toolchain
//! upgrade.

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x100_0000_01b3;

/// FNV-1a 64 fed piece by piece.
#[derive(Debug, Clone, Copy)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(OFFSET)
    }
}

impl Fnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(PRIME);
        }
        self
    }

    /// `x` as its little-endian bytes.
    pub fn u64(&mut self, x: u64) -> &mut Self {
        self.bytes(&x.to_le_bytes())
    }

    pub fn finish(&self) -> u64 {
        self.0
    }

    /// [`finish`](Self::finish) as 16 hex digits, the form keys are written in.
    pub fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// FNV-1a 64 of `bytes`.
pub fn fnv(bytes: &[u8]) -> u64 {
    Fnv::new().bytes(bytes).finish()
}

/// Bytes as lower-case hex, two digits each.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference: FNV-1a written out three ways.
    fn old_seeded(bytes: &[u8], mut h: u64) -> u64 {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    fn old_fold(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        })
    }

    fn old_values(values: &[u64]) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for x in values {
            for b in x.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100_0000_01b3);
            }
        }
        format!("{h:016x}")
    }

    #[test]
    fn matches_the_reference_vector_and_forms() {
        assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv(b"foobar"), 0x8594_4171_f739_67e8);
        let texts: [&[u8]; 4] = [
            b"",
            b"v2|C:\\x.mp4|12|99|s",
            "Ünïcødé ✓".as_bytes(),
            &[0, 255, 7],
        ];
        for t in texts {
            assert_eq!(fnv(t), old_seeded(t, 0xcbf2_9ce4_8422_2325));
            assert_eq!(fnv(t), old_fold(t));
            let mut split = Fnv::new();
            let (a, b) = t.split_at(t.len() / 2);
            split.bytes(a).bytes(b);
            assert_eq!(split.finish(), fnv(t), "fed in pieces");
        }
        let values = [0, 1, u64::MAX, 2.5f64.to_bits(), (-0.0f64).to_bits()];
        for n in 0..=values.len() {
            let mut h = Fnv::new();
            for &x in &values[..n] {
                h.u64(x);
            }
            assert_eq!(h.hex(), old_values(&values[..n]));
        }
        assert_eq!(hex(&[0, 0xab, 0x0f]), "00ab0f");
    }
}
