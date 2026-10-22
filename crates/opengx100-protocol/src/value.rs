//! How parameter values are packed into SysEx data bytes.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// One byte holding an `n`-bit value (`INTEGER1x1`..`INTEGER1x7`).
    Int1(u8),
    /// One nibble per byte, 2 bytes (8-bit value).
    Int2x4,
    /// Two 7-bit bytes (14-bit value).
    Int2x7,
    /// One nibble per byte, 4 bytes (16-bit value). Used by FX params with `ofs = 32768`.
    Int4x4,
    /// `n` raw bytes, e.g. a 16-character ASCII patch name.
    Bytes(u16),
    /// Reserved space.
    Padding(u16),
}

impl Encoding {
    /// Number of data bytes on the wire.
    pub const fn size(self) -> u32 {
        match self {
            Encoding::Int1(_) => 1,
            Encoding::Int2x4 | Encoding::Int2x7 => 2,
            Encoding::Int4x4 => 4,
            Encoding::Bytes(n) | Encoding::Padding(n) => n as u32,
        }
    }

    const fn raw_limit(self) -> Option<u32> {
        match self {
            Encoding::Int1(bits) => Some(1 << bits),
            Encoding::Int2x4 => Some(1 << 8),
            Encoding::Int2x7 => Some(1 << 14),
            Encoding::Int4x4 => Some(1 << 16),
            Encoding::Bytes(_) | Encoding::Padding(_) => None,
        }
    }

    pub const fn is_text(self) -> bool {
        matches!(self, Encoding::Bytes(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    OutOfRange { value: i32, min: i32, max: i32 },
    NotNumeric,
    NotText,
    BadLength { expected: u32, got: usize },
    BadText(String),
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueError::OutOfRange { value, min, max } => write!(f, "{value} is outside {min}..={max}"),
            ValueError::NotNumeric => write!(f, "this parameter holds text, not a number"),
            ValueError::NotText => write!(f, "this parameter holds a number, not text"),
            ValueError::BadLength { expected, got } => write!(f, "expected {expected} data bytes, got {got}"),
            ValueError::BadText(why) => write!(f, "invalid text: {why}"),
        }
    }
}

impl std::error::Error for ValueError {}

/// Range and encoding of a value; `wire = value + ofs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueSpec {
    pub encoding: Encoding,
    pub min: i32,
    pub max: i32,
    pub ofs: i32,
}

impl ValueSpec {
    pub fn encode(&self, value: i32) -> Result<Vec<u8>, ValueError> {
        let limit = self.encoding.raw_limit().ok_or(ValueError::NotNumeric)?;
        let range_err = ValueError::OutOfRange { value, min: self.min, max: self.max };
        if value < self.min || value > self.max {
            return Err(range_err);
        }
        let raw = value.checked_add(self.ofs).filter(|&r| r >= 0 && (r as u32) < limit).ok_or(range_err)? as u32;
        Ok(match self.encoding {
            Encoding::Int1(_) => vec![raw as u8],
            Encoding::Int2x4 => vec![(raw >> 4 & 0xF) as u8, (raw & 0xF) as u8],
            Encoding::Int2x7 => vec![(raw >> 7 & 0x7F) as u8, (raw & 0x7F) as u8],
            Encoding::Int4x4 => {
                vec![(raw >> 12 & 0xF) as u8, (raw >> 8 & 0xF) as u8, (raw >> 4 & 0xF) as u8, (raw & 0xF) as u8]
            }
            Encoding::Bytes(_) | Encoding::Padding(_) => unreachable!(),
        })
    }

    pub fn decode(&self, bytes: &[u8]) -> Result<i32, ValueError> {
        if self.encoding.raw_limit().is_none() {
            return Err(ValueError::NotNumeric);
        }
        self.check_len(bytes)?;
        let raw = match self.encoding {
            Encoding::Int1(bits) => (bytes[0] as u32) & ((1 << bits) - 1),
            Encoding::Int2x4 => (bytes[0] as u32 & 0xF) << 4 | (bytes[1] as u32 & 0xF),
            Encoding::Int2x7 => (bytes[0] as u32 & 0x7F) << 7 | (bytes[1] as u32 & 0x7F),
            Encoding::Int4x4 => bytes.iter().fold(0, |acc, &b| acc << 4 | (b as u32 & 0xF)),
            Encoding::Bytes(_) | Encoding::Padding(_) => unreachable!(),
        };
        Ok(raw as i32 - self.ofs)
    }

    /// Encodes ASCII text for a `Bytes(n)` field, space-padded to `n`.
    pub fn encode_text(&self, text: &str) -> Result<Vec<u8>, ValueError> {
        let Encoding::Bytes(n) = self.encoding else { return Err(ValueError::NotText) };
        if text.chars().count() > n as usize {
            return Err(ValueError::BadText(format!("longer than {n} characters")));
        }
        let mut out = Vec::with_capacity(n as usize);
        for c in text.chars() {
            let code = c as u32;
            if code < self.min as u32 || code > self.max as u32 {
                return Err(ValueError::BadText(format!("character {c:?} not allowed")));
            }
            out.push(code as u8);
        }
        out.resize(n as usize, b' ');
        Ok(out)
    }

    pub fn decode_text(&self, bytes: &[u8]) -> Result<String, ValueError> {
        if !self.encoding.is_text() {
            return Err(ValueError::NotText);
        }
        self.check_len(bytes)?;
        Ok(bytes.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '?' }).collect())
    }

    fn check_len(&self, bytes: &[u8]) -> Result<(), ValueError> {
        let expected = self.encoding.size();
        if bytes.len() as u32 != expected {
            return Err(ValueError::BadLength { expected, got: bytes.len() });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(encoding: Encoding, min: i32, max: i32, ofs: i32) -> ValueSpec {
        ValueSpec { encoding, min, max, ofs }
    }

    #[test]
    fn int4x4_with_offset() {
        let s = spec(Encoding::Int4x4, 0, 120, 32768);
        assert_eq!(s.encode(0).unwrap(), [0x08, 0x00, 0x00, 0x00]);
        assert_eq!(s.encode(100).unwrap(), [0x08, 0x00, 0x06, 0x04]);
        assert_eq!(s.decode(&[0x08, 0x00, 0x06, 0x04]).unwrap(), 100);
        let neg = spec(Encoding::Int4x4, -50, 50, 32768);
        assert_eq!(neg.decode(&neg.encode(-50).unwrap()).unwrap(), -50);
    }

    #[test]
    fn small_encodings() {
        let s = spec(Encoding::Int2x4, 0, 200, 0);
        assert_eq!(s.encode(200).unwrap(), [0x0C, 0x08]);
        assert_eq!(s.decode(&[0x0C, 0x08]).unwrap(), 200);
        let s = spec(Encoding::Int2x7, 0, 9999, 0);
        assert_eq!(s.decode(&s.encode(9999).unwrap()).unwrap(), 9999);
        let s = spec(Encoding::Int1(7), 0, 127, 0);
        assert_eq!(s.encode(127).unwrap(), [0x7F]);
    }

    #[test]
    fn range_and_kind_errors() {
        let s = spec(Encoding::Int1(7), 0, 4, 0);
        assert!(matches!(s.encode(5), Err(ValueError::OutOfRange { .. })));
        assert!(matches!(s.encode(-1), Err(ValueError::OutOfRange { .. })));
        assert_eq!(s.decode(&[1, 2]), Err(ValueError::BadLength { expected: 1, got: 2 }));
        assert_eq!(s.encode_text("x"), Err(ValueError::NotText));
        // declared range wider than the encoding can hold
        let s = spec(Encoding::Int1(7), 0, 300, 0);
        assert!(matches!(s.encode(200), Err(ValueError::OutOfRange { .. })));
    }

    #[test]
    fn text() {
        let s = spec(Encoding::Bytes(16), 32, 126, 0);
        let b = s.encode_text("CRUNCH").unwrap();
        assert_eq!(b.len(), 16);
        assert_eq!(s.decode_text(&b).unwrap(), "CRUNCH          ");
        assert!(s.encode_text("é").is_err());
        assert!(s.encode_text("01234567890123456").is_err());
        assert_eq!(s.encode(1), Err(ValueError::NotNumeric));
    }
}
