//! Planning a rewrite of user patches: take some patches, put them in a new order, and write them
//! into a range of user patch slots.
//!
//! Only planning lives here; nothing in this crate sends these writes yet. The messages match what
//! the official librarian sends (`common/librarian.js` `Librarian.prototype.write` with
//! `config/librarian_setting.js`): plain DT1s straight into the `User_patch(n)` area, one block
//! after another (common, led, assign 1-20, efct, fxItem 1-20), with no PATCH_WRITE command and no
//! extra waits. Blocks longer than [`MAX_DT1_DATA`] bytes are split, as for every other write.

use crate::address::Address;
use crate::patch::{patch_blocks, user_patch, PatchData, USER_PATCHES};
use crate::session::MAX_DT1_DATA;
use crate::sysex;
use std::fmt;

/// User patch `target` gets the contents of user patch `source` (both 1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub target: u16,
    pub source: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    Empty,
    /// The patches would run past the last user patch.
    PastEnd {
        start: u16,
        count: usize,
    },
    /// A patch number outside 1..=200.
    NoSuchPatch(u16),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Empty => write!(f, "no patches selected"),
            PlanError::PastEnd { start, count } => write!(
                f,
                "{count} patches starting at {start} would end at {}, past the last patch ({USER_PATCHES})",
                *start as usize + count - 1
            ),
            PlanError::NoSuchPatch(n) => write!(f, "there is no user patch {n}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// Writes `order[0]` to `start`, `order[1]` to `start + 1`, and so on.
pub fn plan_moves(order: &[u16], start: u16) -> Result<Vec<Move>, PlanError> {
    if order.is_empty() {
        return Err(PlanError::Empty);
    }
    if let Some(&bad) = order.iter().chain([&start]).find(|&&n| !(1..=USER_PATCHES).contains(&n)) {
        return Err(PlanError::NoSuchPatch(bad));
    }
    if start as usize + order.len() - 1 > USER_PATCHES as usize {
        return Err(PlanError::PastEnd { start, count: order.len() });
    }
    Ok(order.iter().enumerate().map(|(i, &source)| Move { target: start + i as u16, source }).collect())
}

/// One DT1 of a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dt1 {
    pub addr: Address,
    pub data: Vec<u8>,
}

impl Dt1 {
    /// The complete SysEx message.
    pub fn to_sysex(&self, device_id: u8) -> Vec<u8> {
        sysex::dt1(device_id, self.addr, &self.data)
    }
}

/// The DT1s that store `data` (a whole patch, as read with `Device::read_patch`) into user patch `target`.
pub fn write_messages(target: u16, data: &PatchData) -> Option<Vec<Dt1>> {
    let base = user_patch(target)?;
    let mut out = Vec::new();
    for b in patch_blocks() {
        let bytes = data.bytes().get(b.offset as usize..(b.offset + b.size) as usize)?;
        for (i, chunk) in bytes.chunks(MAX_DT1_DATA).enumerate() {
            let at = base.offset(b.offset + (i * MAX_DT1_DATA) as u32);
            out.push(Dt1 { addr: at, data: chunk.to_vec() });
        }
    }
    Some(out)
}

/// The DT1s that load `data` into the live (Temp) patch, in the official librarian's order
/// (`Librarian.prototype.temporaryWrite`): the whole `efct` block first ("sent before CHAIN"), then the rest.
pub fn temp_write_messages(data: &PatchData) -> Vec<Dt1> {
    let base = crate::patch::temp_patch();
    let mut blocks: Vec<_> = patch_blocks().iter().collect();
    blocks.sort_by_key(|b| b.name != "efct");
    let mut out = Vec::new();
    for b in blocks {
        let Some(bytes) = data.bytes().get(b.offset as usize..(b.offset + b.size) as usize) else { continue };
        for (i, chunk) in bytes.chunks(MAX_DT1_DATA).enumerate() {
            out.push(Dt1 { addr: base.offset(b.offset + (i * MAX_DT1_DATA) as u32), data: chunk.to_vec() });
        }
    }
    out
}

/// Bytes on the wire for `msgs`, and how long sending them takes at MIDI speed (3125 bytes/s,
/// the pace the official editor keeps).
pub fn wire_cost(msgs: &[Dt1], device_id: u8) -> (usize, f32) {
    let bytes: usize = msgs.iter().map(|m| m.to_sysex(device_id).len()).sum();
    (bytes, bytes as f32 / 3125.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::patch_span;

    #[test]
    fn moves_fill_the_range_in_order() {
        let m = plan_moves(&[45, 3, 12], 10).unwrap();
        assert_eq!(
            m,
            vec![Move { target: 10, source: 45 }, Move { target: 11, source: 3 }, Move { target: 12, source: 12 }]
        );
        assert_eq!(plan_moves(&[], 1), Err(PlanError::Empty));
        assert_eq!(plan_moves(&[1, 2], 200), Err(PlanError::PastEnd { start: 200, count: 2 }));
        assert!(plan_moves(&[1], 199).is_ok());
        assert_eq!(plan_moves(&[201], 1), Err(PlanError::NoSuchPatch(201)));
        assert_eq!(plan_moves(&[1], 0), Err(PlanError::NoSuchPatch(0)));
    }

    #[test]
    fn messages_cover_every_block_at_the_target() {
        let bytes: Vec<u8> = (0..patch_span()).map(|i| (i % 128) as u8).collect();
        let data = PatchData::from_bytes(bytes);
        let msgs = write_messages(2, &data).unwrap();
        let base = user_patch(2).unwrap();
        let total: usize = msgs.iter().map(|m| m.data.len()).sum();
        assert_eq!(total, patch_blocks().iter().map(|b| b.size as usize).sum::<usize>());
        assert!(msgs.iter().all(|m| m.data.len() <= MAX_DT1_DATA));
        // every byte lands at the same offset inside the target patch
        for m in &msgs {
            let off = m.addr.since(base).unwrap();
            assert_eq!(m.data[0], data.bytes()[off as usize]);
        }
        assert_eq!(msgs[0].addr, base, "starts with the common block (name)");
        let (wire, secs) = wire_cost(&msgs, 0x10);
        assert!(wire > total && secs > 0.0);
        assert!(write_messages(0, &data).is_none());
    }
}
