//! Building and parsing Roland SysEx messages for the GX-100.

use crate::address::Address;
use std::fmt;

pub const SOX: u8 = 0xF0;
pub const EOX: u8 = 0xF7;
pub const ROLAND: u8 = 0x41;
pub const DT1: u8 = 0x12;
pub const RQ1: u8 = 0x11;
/// Default device ID (the pedal reports its actual one in the identity reply).
pub const DEFAULT_DEVICE_ID: u8 = 0x10;
pub const MODEL_ID: [u8; 5] = [0x00, 0x00, 0x00, 0x00, 0x0B];
/// Family code reported in the identity reply.
pub const FAMILY_CODE: [u8; 2] = [0x0B, 0x04];
/// Bytes before the address in a DT1/RQ1: F0 41 dev <model:5> cmd
const HEADER_LEN: usize = 3 + MODEL_ID.len() + 1;

/// Roland checksum over address + data (or address + size).
pub fn checksum(bytes: &[u8]) -> u8 {
    let sum: u32 = bytes.iter().map(|&b| b as u32).sum();
    ((128 - (sum % 128)) & 0x7f) as u8
}

fn frame(device_id: u8, cmd: u8, body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(HEADER_LEN + body.len() + 2);
    m.extend_from_slice(&[SOX, ROLAND, device_id]);
    m.extend_from_slice(&MODEL_ID);
    m.push(cmd);
    m.extend_from_slice(body);
    m.push(checksum(body));
    m.push(EOX);
    m
}

/// Data Set 1: write `data` starting at `addr`.
pub fn dt1(device_id: u8, addr: Address, data: &[u8]) -> Vec<u8> {
    let mut body = addr.to_wire().to_vec();
    body.extend_from_slice(data);
    frame(device_id, DT1, &body)
}

/// Request Data 1: ask for `size` bytes starting at `addr` (answered with DT1s).
pub fn rq1(device_id: u8, addr: Address, size: u32) -> Vec<u8> {
    let mut body = addr.to_wire().to_vec();
    body.extend_from_slice(&Address::from_linear(size).to_wire());
    frame(device_id, RQ1, &body)
}

/// Universal non-realtime identity request, sent to all devices.
pub fn identity_request() -> Vec<u8> {
    vec![SOX, 0x7E, 0x7F, 0x06, 0x01, EOX]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityReply {
    pub device_id: u8,
    pub manufacturer: u8,
    pub family: [u8; 2],
    pub number: [u8; 2],
    pub revision: [u8; 4],
}

impl IdentityReply {
    pub fn is_gx100(&self) -> bool {
        self.manufacturer == ROLAND && self.family == FAMILY_CODE
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Dt1 {
        device_id: u8,
        addr: Address,
        data: Vec<u8>,
    },
    IdentityReply(IdentityReply),
    /// Anything else (other SysEx, channel messages…).
    Other(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    BadChecksum { expected: u8, got: u8 },
    Truncated,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::BadChecksum { expected, got } => {
                write!(f, "bad checksum: expected {expected:02X}, got {got:02X}")
            }
            ParseError::Truncated => write!(f, "truncated SysEx message"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses one complete MIDI message (as delivered by a [`SysexAssembler`]).
pub fn parse(msg: &[u8]) -> Result<Message, ParseError> {
    // F0 7E dev 06 02 mfr fam(2) num(2) rev(4) F7
    if msg.len() >= 5 && msg[0] == SOX && msg[1] == 0x7E && msg[3] == 0x06 && msg[4] == 0x02 {
        if msg.len() < 15 {
            return Err(ParseError::Truncated);
        }
        return Ok(Message::IdentityReply(IdentityReply {
            device_id: msg[2],
            manufacturer: msg[5],
            family: [msg[6], msg[7]],
            number: [msg[8], msg[9]],
            revision: [msg[10], msg[11], msg[12], msg[13]],
        }));
    }

    let is_our_dt1 = msg.len() > HEADER_LEN
        && msg[0] == SOX
        && msg[1] == ROLAND
        && msg[3..3 + MODEL_ID.len()] == MODEL_ID
        && msg[HEADER_LEN - 1] == DT1;
    if !is_our_dt1 {
        return Ok(Message::Other(msg.to_vec()));
    }
    // header + addr(4) + data(>=0) + sum + F7
    if msg.len() < HEADER_LEN + 4 + 2 || *msg.last().unwrap() != EOX {
        return Err(ParseError::Truncated);
    }
    let body = &msg[HEADER_LEN..msg.len() - 2];
    let got = msg[msg.len() - 2];
    let expected = checksum(body);
    if got != expected {
        return Err(ParseError::BadChecksum { expected, got });
    }
    Ok(Message::Dt1 {
        device_id: msg[2],
        addr: Address::from_wire([body[0], body[1], body[2], body[3]]),
        data: body[4..].to_vec(),
    })
}

/// Reassembles SysEx messages that arrive split across several MIDI packets,
/// dropping realtime bytes (clock, active sensing…) interleaved with them.
#[derive(Debug, Default)]
pub struct SysexAssembler {
    buf: Vec<u8>,
    in_sysex: bool,
}

impl SysexAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one packet; returns every message completed by it.
    pub fn push(&mut self, packet: &[u8]) -> Vec<Vec<u8>> {
        let mut done = Vec::new();
        if !self.in_sysex && packet.first() != Some(&SOX) {
            // A non-SysEx message (MIDI drivers deliver those whole).
            if !packet.is_empty() && !packet.iter().all(|&b| b >= 0xF8) {
                done.push(packet.to_vec());
            }
            return done;
        }
        for &b in packet {
            match b {
                0xF8..=0xFF => {} // realtime: may appear anywhere, never part of SysEx
                SOX => {
                    self.buf.clear();
                    self.buf.push(b);
                    self.in_sysex = true;
                }
                EOX if self.in_sysex => {
                    self.buf.push(b);
                    done.push(std::mem::take(&mut self.buf));
                    self.in_sysex = false;
                }
                _ if self.in_sysex => self.buf.push(b),
                _ => {}
            }
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FX1_ON: [u8; 16] =
        [0xF0, 0x41, 0x10, 0x00, 0x00, 0x00, 0x00, 0x0B, 0x12, 0x10, 0x00, 0x11, 0x01, 0x01, 0x5D, 0xF7];

    #[test]
    fn dt1_matches_worked_example() {
        let addr = Address::from_wire([0x10, 0x00, 0x11, 0x01]);
        assert_eq!(dt1(DEFAULT_DEVICE_ID, addr, &[0x01]), FX1_ON);
    }

    #[test]
    fn rq1_layout() {
        let m = rq1(0x10, Address::from_wire([0x7F, 0, 0, 0]), 1);
        assert_eq!(m, [0xF0, 0x41, 0x10, 0, 0, 0, 0, 0x0B, 0x11, 0x7F, 0, 0, 0, 0, 0, 0, 1, 0x00, 0xF7]);
        // sum = 0x7F + 1 = 128 -> checksum 0
    }

    #[test]
    fn rq1_size_is_7bit_packed() {
        let m = rq1(0x10, Address::from_wire([0x10, 0, 0, 0]), 7219);
        assert_eq!(&m[13..17], &[0x00, 0x00, 0x38, 0x33]);
    }

    #[test]
    fn parse_dt1_roundtrip() {
        match parse(&FX1_ON).unwrap() {
            Message::Dt1 { device_id, addr, data } => {
                assert_eq!(device_id, 0x10);
                assert_eq!(addr.to_string(), "10 00 11 01");
                assert_eq!(data, [0x01]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_bad_checksum() {
        let mut m = FX1_ON;
        m[14] ^= 1;
        assert!(matches!(parse(&m), Err(ParseError::BadChecksum { .. })));
    }

    #[test]
    fn parse_identity_reply() {
        let m = [0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0x0B, 0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0xF7];
        let Message::IdentityReply(r) = parse(&m).unwrap() else { panic!() };
        assert!(r.is_gx100());
        assert_eq!(r.device_id, 0x10);
        assert_eq!(r.revision, [0, 1, 0, 0]);
    }

    #[test]
    fn other_messages_pass_through() {
        assert_eq!(parse(&[0xC0, 0x05]).unwrap(), Message::Other(vec![0xC0, 0x05]));
        assert_eq!(parse(&identity_request()).unwrap(), Message::Other(identity_request()));
    }

    #[test]
    fn assembler_joins_split_packets_and_drops_realtime() {
        let mut a = SysexAssembler::new();
        assert!(a.push(&FX1_ON[..5]).is_empty());
        assert!(a.push(&[0xF8]).is_empty());
        let mut mid = FX1_ON[5..10].to_vec();
        mid.insert(2, 0xFE);
        assert!(a.push(&mid).is_empty());
        let out = a.push(&FX1_ON[10..]);
        assert_eq!(out, vec![FX1_ON.to_vec()]);
    }

    #[test]
    fn assembler_splits_two_messages_in_one_packet() {
        let mut a = SysexAssembler::new();
        let mut both = FX1_ON.to_vec();
        both.extend_from_slice(&FX1_ON);
        assert_eq!(a.push(&both).len(), 2);
        assert_eq!(a.push(&[0xB0, 7, 100]), vec![vec![0xB0, 7, 100]]);
    }
}
