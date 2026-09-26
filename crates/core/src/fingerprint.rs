//! Tiny, dependency-free FNV-1a 64-bit hasher used to detect whether captured values changed.

#[derive(Debug, Clone, Copy)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    pub fn new() -> Self {
        Self(Self::OFFSET)
    }

    pub fn write(&mut self, bytes: &[u8]) -> &mut Self {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
        self
    }

    pub fn write_str(&mut self, s: &str) -> &mut Self {
        self.write(s.as_bytes()).write(&[0xff])
    }

    pub fn write_opt_str(&mut self, s: Option<&str>) -> &mut Self {
        match s {
            Some(s) => self.write(&[1]).write_str(s),
            None => self.write(&[0]),
        }
    }

    pub fn write_i64(&mut self, v: i64) -> &mut Self {
        self.write(&v.to_le_bytes())
    }

    pub fn write_u64(&mut self, v: u64) -> &mut Self {
        self.write(&v.to_le_bytes())
    }

    /// Hashes the float's bit pattern after normalising -0.0 and NaN.
    pub fn write_f32(&mut self, v: f32) -> &mut Self {
        let v = if v.is_nan() {
            f32::NAN
        } else if v == 0.0 {
            0.0
        } else {
            v
        };
        self.write(&v.to_bits().to_le_bytes())
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(Fnv64::new().finish(), 0xcbf2_9ce4_8422_2325);
        assert_eq!(Fnv64::new().write(b"a").finish(), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn negative_zero_matches_zero() {
        assert_eq!(Fnv64::new().write_f32(-0.0).finish(), Fnv64::new().write_f32(0.0).finish());
    }
}
