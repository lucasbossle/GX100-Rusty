//! Roland 4-byte, 7-bit-per-byte addresses.

use std::fmt;

/// A device address. Stored linearly (28 bits) so offsets can be added;
/// on the wire it is four 7-bit bytes, e.g. `10 00 11 01`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Address(u32);

impl Address {
    pub const fn from_wire(b: [u8; 4]) -> Self {
        Address(
            ((b[0] as u32 & 0x7f) << 21)
                | ((b[1] as u32 & 0x7f) << 14)
                | ((b[2] as u32 & 0x7f) << 7)
                | (b[3] as u32 & 0x7f),
        )
    }

    pub const fn to_wire(self) -> [u8; 4] {
        [
            ((self.0 >> 21) & 0x7f) as u8,
            ((self.0 >> 14) & 0x7f) as u8,
            ((self.0 >> 7) & 0x7f) as u8,
            (self.0 & 0x7f) as u8,
        ]
    }

    pub const fn from_linear(n: u32) -> Self {
        Address(n & 0x0fff_ffff)
    }

    pub const fn linear(self) -> u32 {
        self.0
    }

    /// `self + n` bytes, carrying across 7-bit byte boundaries.
    pub const fn offset(self, n: u32) -> Self {
        Address::from_linear(self.0 + n)
    }

    /// Byte distance from `base` to `self`, if `self >= base`.
    pub fn since(self, base: Address) -> Option<u32> {
        self.0.checked_sub(base.0)
    }

    /// Parses `"10 00 11 01"`, `"10001101"` or `"0x10001101"` (all wire notation).
    pub fn parse(s: &str) -> Option<Self> {
        let hex: String = s
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X")
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '.' && *c != ':')
            .collect();
        if hex.len() != 8 {
            return None;
        }
        let mut b = [0u8; 4];
        for (i, out) in b.iter_mut().enumerate() {
            *out = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
            if *out > 0x7f {
                return None;
            }
        }
        Some(Address::from_wire(b))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.to_wire();
        write!(f, "{a:02X} {b:02X} {c:02X} {d:02X}")
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_roundtrip() {
        let a = Address::from_wire([0x10, 0x00, 0x11, 0x01]);
        assert_eq!(a.to_wire(), [0x10, 0x00, 0x11, 0x01]);
        assert_eq!(a.linear(), (0x10 << 21) | (0x11 << 7) | 0x01);
        assert_eq!(a.to_string(), "10 00 11 01");
    }

    #[test]
    fn offset_carries_across_7bit_boundary() {
        let a = Address::from_wire([0x00, 0x00, 0x00, 0x7f]);
        assert_eq!(a.offset(1).to_wire(), [0x00, 0x00, 0x01, 0x00]);
        let user2 = Address::from_wire([0x20, 0x00, 0x00, 0x00]).offset(Address::from_wire([0, 6, 0, 0]).linear());
        assert_eq!(user2.to_string(), "20 06 00 00");
    }

    #[test]
    fn parse_variants() {
        let want = Some(Address::from_wire([0x7f, 0x00, 0x01, 0x04]));
        assert_eq!(Address::parse("7F 00 01 04"), want);
        assert_eq!(Address::parse("7f000104"), want);
        assert_eq!(Address::parse("0x7F000104"), want);
        assert_eq!(Address::parse("80 00 00 00"), None, "bytes must be 7-bit");
        assert_eq!(Address::parse("10 00 11"), None);
    }
}
