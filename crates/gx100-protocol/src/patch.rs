//! Patch-level helpers: where patches and FX slots live, reading whole patches,
//! and reading/writing FX params.

use crate::address::Address;
use crate::session::{Device, Error, ParamValue, Result, Transport};
use crate::tables::{self, FxParamDef, FxTypeDef, ParamDef};
use std::sync::OnceLock;

pub const FX_SLOTS: u8 = 20;
pub const USER_PATCHES: u16 = 200;

/// The live patch being played/edited.
pub fn temp_patch() -> Address {
    tables::root("Temp_patch").map(|r| r.addr).unwrap_or(Address::from_wire([0x10, 0, 0, 0]))
}

/// Stored user patch `n` (1-based).
pub fn user_patch(n: u16) -> Option<Address> {
    tables::root(&format!("User_patch({n})")).map(|r| r.addr)
}

/// Start of FX slot `slot` (1-based) inside the patch at `patch`.
pub fn fx_slot_base(patch: Address, slot: u8) -> Option<Address> {
    let ty = tables::param(&format!("Temp_patch.fxItem({slot}).Type"))?;
    Some(patch.offset(ty.addr.since(temp_patch())?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxSlot {
    pub slot: u8,
    pub type_id: u8,
    pub on: bool,
}

impl FxSlot {
    pub fn fx_type(&self) -> Option<&'static FxTypeDef> {
        tables::fx_type(self.type_id)
    }
}

/// One block of a patch (`common`, `led`, `assign(n)`, `efct`, `fxItem(n)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchBlock {
    pub name: String,
    /// Bytes from the start of the patch.
    pub offset: u32,
    pub size: u32,
}

/// The blocks a patch is read in, one request each, as the official librarian does.
/// Derived from the `Temp_patch.*` parameters; FX slots are widened to fit every FX type.
pub fn patch_blocks() -> &'static [PatchBlock] {
    static BLOCKS: OnceLock<Vec<PatchBlock>> = OnceLock::new();
    BLOCKS.get_or_init(|| {
        let temp = temp_patch();
        let fx_span = tables::FX_TYPES
            .iter()
            .flat_map(|t| t.params)
            .map(|p| p.offset.linear() + p.encoding.size())
            .max()
            .unwrap_or(0);
        let mut blocks: Vec<PatchBlock> = Vec::new();
        for p in tables::PARAMS {
            let Some((name, _)) = p.path.strip_prefix("Temp_patch.").and_then(|r| r.split_once('.')) else { continue };
            let Some(start) = p.addr.since(temp) else { continue };
            let end = start + p.encoding.size();
            match blocks.iter_mut().find(|b| b.name == name) {
                Some(b) => {
                    let b_end = (b.offset + b.size).max(end);
                    b.offset = b.offset.min(start);
                    b.size = b_end - b.offset;
                }
                None => blocks.push(PatchBlock { name: name.to_string(), offset: start, size: end - start }),
            }
        }
        for b in blocks.iter_mut().filter(|b| b.name.starts_with("fxItem(")) {
            b.size = b.size.max(fx_span);
        }
        blocks.sort_by_key(|b| b.offset);
        blocks
    })
}

/// Bytes from the start of a patch to the end of its last block.
pub fn patch_span() -> u32 {
    patch_blocks().iter().map(|b| b.offset + b.size).max().unwrap_or(0)
}

/// Where a Temp-patch address falls inside a patch, if it does.
pub fn temp_offset(addr: Address) -> Option<u32> {
    addr.since(temp_patch()).filter(|&o| o < patch_span())
}

/// The signal path. Without a branch it is just `pre`. With one, the signal runs through `pre`,
/// the DIVIDER splits it into `a` and `b`, the MIXER joins them, and `post` follows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChainLayout {
    pub pre: Vec<u8>,
    pub branch: Option<Branch>,
    pub post: Vec<u8>,
}

/// A parallel section. In the stored chain it reads `DIVIDER a… SPLITTER b… MIXER`;
/// the SPLITTER only marks where branch B starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub divider: u8,
    pub a: Vec<u8>,
    pub splitter: u8,
    pub b: Vec<u8>,
    pub mixer: u8,
}

/// A whole patch as read from the pedal: raw bytes, laid out like the Temp patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchData {
    bytes: Vec<u8>,
}

impl PatchData {
    /// `bytes[i]` is the byte at offset `i` from the start of the patch.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        PatchData { bytes }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn slice(&self, offset: u32, len: u32) -> Option<&[u8]> {
        self.bytes.get(offset as usize..(offset + len) as usize)
    }

    /// The value of a `Temp_patch.*` parameter in this patch.
    pub fn value(&self, p: &ParamDef) -> Option<ParamValue> {
        let bytes = self.slice(p.addr.since(temp_patch())?, p.encoding.size())?;
        if p.encoding.is_text() {
            p.spec().decode_text(bytes).ok().map(ParamValue::Text)
        } else {
            p.spec().decode(bytes).ok().map(ParamValue::Num)
        }
    }

    fn num(&self, path: &str) -> Option<i32> {
        match self.value(tables::param(path)?)? {
            ParamValue::Num(n) => Some(n),
            ParamValue::Text(_) => None,
        }
    }

    /// The patch name, trailing spaces removed.
    pub fn name(&self) -> String {
        match tables::param("Temp_patch.common.Name").and_then(|p| self.value(p)) {
            Some(ParamValue::Text(s)) => s.trim_end().to_string(),
            _ => String::new(),
        }
    }

    /// FX slots (1-based) in signal order. The chain is a linked list:
    /// `ChainTopItem` names the first slot, `ChainNextItem<i>` the one after slot `i` (0-based, -1 ends it).
    /// Slots not in the list are unused, whatever they contain.
    pub fn chain(&self) -> Vec<u8> {
        let mut order = Vec::new();
        let mut cur = self.num("Temp_patch.efct.ChainTopItem");
        while let Some(i) = cur.filter(|&i| (0..FX_SLOTS as i32).contains(&i)) {
            let slot = i as u8 + 1;
            if order.contains(&slot) {
                break;
            }
            order.push(slot);
            cur = self.num(&format!("Temp_patch.efct.ChainNextItem{i}"));
        }
        order
    }

    /// Splits [`Self::chain`] into its parallel branches. Chains the official editor would reject
    /// (a DIVIDER without SPLITTER and MIXER, duplicates, wrong order) come back serial.
    pub fn chain_layout(&self) -> ChainLayout {
        let chain = self.chain();
        let find = |name: &str| -> Vec<usize> {
            chain
                .iter()
                .enumerate()
                .filter(|&(_, &s)| self.fx_slot(s).and_then(|s| s.fx_type()).is_some_and(|t| t.name == name))
                .map(|(i, _)| i)
                .collect()
        };
        let (div, spl, mix) = (find("DIVIDER"), find("SPLITTER"), find("MIXER"));
        match (&div[..], &spl[..], &mix[..]) {
            (&[d], &[s], &[m]) if d < s && s < m => ChainLayout {
                pre: chain[..d].to_vec(),
                branch: Some(Branch {
                    divider: chain[d],
                    a: chain[d + 1..s].to_vec(),
                    splitter: chain[s],
                    b: chain[s + 1..m].to_vec(),
                    mixer: chain[m],
                }),
                post: chain[m + 1..].to_vec(),
            },
            _ => ChainLayout { pre: chain, branch: None, post: Vec::new() },
        }
    }

    /// Applies a DT1 the pedal sent for this patch: `data` starting at `offset` (see [`temp_offset`]).
    pub fn apply(&mut self, offset: u32, data: &[u8]) {
        let start = (offset as usize).min(self.bytes.len());
        let end = (start + data.len()).min(self.bytes.len());
        self.bytes[start..end].copy_from_slice(&data[..end - start]);
    }

    pub fn fx_slot(&self, slot: u8) -> Option<FxSlot> {
        let b = self.slice(fx_slot_base(temp_patch(), slot)?.since(temp_patch())?, 2)?;
        Some(FxSlot { slot, type_id: b[0], on: b[1] != 0 })
    }

    pub fn fx_param(&self, slot: u8, p: &FxParamDef) -> Option<i32> {
        let base = fx_slot_base(temp_patch(), slot)?.since(temp_patch())?;
        p.spec().decode(self.slice(base + p.offset.linear(), p.encoding.size())?).ok()
    }
}

impl<T: Transport> Device<T> {
    /// Reads every block of the patch at `patch` (the Temp patch or a user patch).
    pub fn read_patch(&mut self, patch: Address) -> Result<PatchData> {
        let mut bytes = vec![0u8; patch_span() as usize];
        for b in patch_blocks() {
            let data = self.read(patch.offset(b.offset), b.size)?;
            bytes[b.offset as usize..(b.offset + b.size) as usize].copy_from_slice(&data);
        }
        Ok(PatchData::from_bytes(bytes))
    }

    /// Switches the pedal to user patch `n` (1-based), as selecting it in the official editor does
    /// (`patch_controller.js` `updateCurrentPatch`): `CurrentPatchNum` gets the 0-based index.
    /// The pedal loads it into the Temp patch; this returns once it accepts reads again.
    pub fn select_patch(&mut self, n: u16) -> Result<()> {
        if !(1..=USER_PATCHES).contains(&n) {
            return Err(Error::Refused(format!("there is no user patch {n}")));
        }
        let p = tables::param("System_common.CurrentPatchNum")
            .ok_or_else(|| Error::Unknown("current patch parameter".into()))?;
        self.write_param(p, &ParamValue::Num(n as i32 - 1))?;
        self.wait_until_ready()
    }

    /// Writes `data` straight into user patch `n`, as the official librarian does (see [`crate::librarian`]).
    pub fn write_user_patch(&mut self, n: u16, data: &PatchData) -> Result<()> {
        let msgs =
            crate::librarian::write_messages(n, data).ok_or_else(|| Error::Unknown(format!("user patch {n}")))?;
        for m in msgs {
            self.write(m.addr, &m.data)?;
        }
        Ok(())
    }

    /// Loads `data` into the live (Temp) patch without storing it anywhere, as the official librarian's
    /// preview does (see [`crate::librarian::temp_write_messages`]).
    pub fn load_into_temp(&mut self, data: &PatchData) -> Result<()> {
        for m in crate::librarian::temp_write_messages(data) {
            self.write(m.addr, &m.data)?;
        }
        Ok(())
    }

    /// Just the name of the patch at `patch`, trailing spaces removed.
    pub fn read_patch_name(&mut self, patch: Address) -> Result<String> {
        let p = tables::param("Temp_patch.common.Name").ok_or_else(|| Error::Unknown("patch name parameter".into()))?;
        let at = patch.offset(p.addr.since(temp_patch()).unwrap_or(0));
        let bytes = self.read(at, p.encoding.size())?;
        Ok(p.spec().decode_text(&bytes)?.trim_end().to_string())
    }

    pub fn read_fx_slot(&mut self, patch: Address, slot: u8) -> Result<FxSlot> {
        let base = fx_slot_base(patch, slot).ok_or_else(|| Error::Unknown(format!("FX slot {slot}")))?;
        let b = self.read(base, 2)?;
        Ok(FxSlot { slot, type_id: b[0], on: b[1] != 0 })
    }

    /// Every FX slot the tables know about (20 with real tables).
    pub fn read_fx_slots(&mut self, patch: Address) -> Result<Vec<FxSlot>> {
        (1..=FX_SLOTS).filter(|&s| fx_slot_base(patch, s).is_some()).map(|s| self.read_fx_slot(patch, s)).collect()
    }

    pub fn read_fx_param(&mut self, patch: Address, slot: u8, p: &FxParamDef) -> Result<i32> {
        let base = fx_slot_base(patch, slot).ok_or_else(|| Error::Unknown(format!("FX slot {slot}")))?;
        let bytes = self.read(base.offset(p.offset.linear()), p.encoding.size())?;
        Ok(p.spec().decode(&bytes)?)
    }

    pub fn write_fx_param(&mut self, patch: Address, slot: u8, p: &FxParamDef, value: i32) -> Result<()> {
        let base = fx_slot_base(patch, slot).ok_or_else(|| Error::Unknown(format!("FX slot {slot}")))?;
        let data = p.spec().encode(value)?;
        self.write(base.offset(p.offset.linear()), &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ParamValue;
    use crate::sim::SimulatedGx100;
    use std::time::Duration;

    #[test]
    fn slot_addresses() {
        assert_eq!(fx_slot_base(temp_patch(), 1).unwrap().to_string(), "10 00 11 00");
        assert_eq!(fx_slot_base(temp_patch(), 2).unwrap().to_string(), "10 00 13 00");
        let u2 = user_patch(2).unwrap();
        assert_eq!(u2.to_string(), "20 06 00 00");
        assert_eq!(fx_slot_base(u2, 1).unwrap().to_string(), "20 06 11 00");
    }

    #[test]
    fn fx_slot_roundtrip() {
        let mut sim = SimulatedGx100::new();
        sim.poke(fx_slot_base(temp_patch(), 1).unwrap(), &[1, 1]);
        let mut d = Device::new(sim).without_pacing().with_timeout(Duration::from_millis(50));
        let slots = d.read_fx_slots(temp_patch()).unwrap();
        assert_eq!(slots[0], FxSlot { slot: 1, type_id: 1, on: true });
        assert_eq!(slots[1].type_id, 0, "untouched slot keeps type 0");
        assert_eq!(slots[0].fx_type().map(|t| t.id), Some(1));

        let ty = tables::fx_type(0).unwrap();
        let Some(p) = ty.params.iter().find(|p| matches!(p.field, tables::FxField::Param(_)) && p.max >= 100) else {
            return;
        };
        d.write_fx_param(temp_patch(), 2, p, 100).unwrap();
        assert_eq!(d.read_fx_param(temp_patch(), 2, p).unwrap(), 100);
        let at = fx_slot_base(temp_patch(), 2).unwrap().offset(p.offset.linear());
        assert_eq!(d.transport().peek(at, 4), vec![0x08, 0x00, 0x06, 0x04]);
    }

    #[test]
    fn blocks_cover_name_chain_and_slots() {
        let blocks = patch_blocks();
        assert!(blocks.windows(2).all(|w| w[0].offset + w[0].size <= w[1].offset), "blocks overlap: {blocks:?}");
        let covered = |p: &ParamDef| {
            let off = p.addr.since(temp_patch()).unwrap();
            blocks.iter().any(|b| b.offset <= off && off + p.encoding.size() <= b.offset + b.size)
        };
        assert!(tables::PARAMS.iter().filter(|p| p.path.starts_with("Temp_patch.")).all(covered));
        assert!(blocks.iter().any(|b| b.name == "fxItem(2)"));
    }

    #[test]
    fn read_user_patch() {
        let mut sim = SimulatedGx100::new();
        let u2 = user_patch(2).unwrap();
        let at = |path: &str| u2.offset(tables::param(path).unwrap().addr.since(temp_patch()).unwrap());
        sim.poke(at("Temp_patch.common.Name"), b"CRUNCH          ");
        // slot 2 -> slot 1 -> end, stored with ofs 1
        sim.poke(at("Temp_patch.efct.ChainTopItem"), &[2]);
        sim.poke(at("Temp_patch.efct.ChainNextItem1"), &[1]);
        sim.poke(at("Temp_patch.efct.ChainNextItem0"), &[0]);
        sim.poke(fx_slot_base(u2, 2).unwrap(), &[1, 1]);
        let mut d = Device::new(sim).without_pacing().with_timeout(Duration::from_millis(50));

        assert_eq!(d.read_patch_name(u2).unwrap(), "CRUNCH");
        let p = d.read_patch(u2).unwrap();
        assert_eq!(p.name(), "CRUNCH");
        assert_eq!(p.chain(), vec![2, 1]);
        assert_eq!(p.fx_slot(2), Some(FxSlot { slot: 2, type_id: 1, on: true }));
        assert_eq!(p.fx_slot(1).map(|s| s.on), Some(false));
        let reads = d.transport().sent().len();
        assert_eq!(reads, 1 + patch_blocks().len());
    }

    /// A patch whose chain is `slots`, slot `s` holding FX type `types[s - 1]`.
    fn patch_with_chain(slots: &[u8], types: &[u8]) -> PatchData {
        let off = |path: String| tables::param(&path).unwrap().addr.since(temp_patch()).unwrap() as usize;
        let mut bytes = vec![0u8; patch_span() as usize];
        let mut prev: Option<u8> = None;
        for &s in slots {
            let at = match prev {
                None => off("Temp_patch.efct.ChainTopItem".into()),
                Some(p) => off(format!("Temp_patch.efct.ChainNextItem{}", p - 1)),
            };
            bytes[at] = s; // slot s is item s - 1, stored + 1
            prev = Some(s);
        }
        if let Some(p) = prev {
            bytes[off(format!("Temp_patch.efct.ChainNextItem{}", p - 1))] = 0;
        }
        for (i, &t) in types.iter().enumerate() {
            let base = fx_slot_base(temp_patch(), i as u8 + 1).unwrap().since(temp_patch()).unwrap() as usize;
            bytes[base] = t;
        }
        PatchData::from_bytes(bytes)
    }

    #[test]
    fn chain_layout_finds_the_branch() {
        if tables::SOURCE != tables::TablesSource::Extracted {
            return; // needs the real FX type list and all 20 ChainNextItems
        }
        let id = |n: &str| tables::fx_type_by_name(n).unwrap().id;
        let (div, spl, mix, od) = (id("DIVIDER"), id("SPLITTER"), id("MIXER"), id("OVERDRIVE"));
        // slots: 1 OD, 2 DIV, 3 OD, 4 SPL, 5 OD, 6 OD, 7 MIX, 8 OD
        let types = [od, div, od, spl, od, od, mix, od];
        let p = patch_with_chain(&[1, 2, 3, 4, 5, 6, 7, 8], &types);
        let want = Branch { divider: 2, a: vec![3], splitter: 4, b: vec![5, 6], mixer: 7 };
        assert_eq!(p.chain_layout(), ChainLayout { pre: vec![1], branch: Some(want), post: vec![8] });

        // MIXER before SPLITTER: not a valid branch, shown serial
        let p = patch_with_chain(&[1, 2, 7, 4, 8], &types);
        assert_eq!(p.chain_layout(), ChainLayout { pre: vec![1, 2, 7, 4, 8], branch: None, post: vec![] });
    }

    #[test]
    fn apply_updates_bytes() {
        let mut p = PatchData::from_bytes(vec![0u8; patch_span() as usize]);
        let sw = tables::param("Temp_patch.fxItem(2).Switch").unwrap();
        p.apply(temp_offset(sw.addr).unwrap(), &[1]);
        assert!(p.fx_slot(2).unwrap().on);
        p.apply(patch_span() - 1, &[1, 2, 3]); // clipped, no panic
        assert_eq!(temp_offset(user_patch(1).unwrap()), None);
    }

    fn dev(sim: SimulatedGx100) -> Device<SimulatedGx100> {
        Device::new(sim).without_pacing().with_timeout(Duration::from_millis(50))
    }

    fn patch_named(name: &str, fill: u8) -> PatchData {
        let mut bytes = vec![fill; patch_span() as usize];
        let at = tables::param("Temp_patch.common.Name").unwrap().addr.since(temp_patch()).unwrap() as usize;
        bytes[at..at + 16].copy_from_slice(format!("{name:<16}").as_bytes());
        PatchData::from_bytes(bytes)
    }

    #[test]
    fn write_user_patch_then_read_it_back() {
        let mut d = dev(SimulatedGx100::new());
        let p = patch_named("SETLIST", 5);
        d.write_user_patch(2, &p).unwrap();
        let back = d.read_patch(user_patch(2).unwrap()).unwrap();
        // every block came back; gaps between blocks aren't written, so they read as 0
        for b in patch_blocks() {
            let r = b.offset as usize..(b.offset + b.size) as usize;
            assert_eq!(back.bytes()[r.clone()], p.bytes()[r], "block {}", b.name);
        }
        assert_eq!(back.name(), "SETLIST");
    }

    #[test]
    fn load_into_temp_sends_efct_first() {
        let p = patch_named("PREVIEW", 3);
        let msgs = crate::librarian::temp_write_messages(&p);
        let efct = patch_blocks().iter().find(|b| b.name == "efct").unwrap();
        assert_eq!(msgs[0].addr, temp_patch().offset(efct.offset));
        let mut d = dev(SimulatedGx100::new());
        d.load_into_temp(&p).unwrap();
        assert_eq!(d.read_patch(temp_patch()).unwrap().name(), "PREVIEW");
    }

    #[test]
    fn save_and_select_patches() {
        if tables::param("System_common.CurrentPatchNum").is_none() || tables::command("PATCH_WRITE").is_none() {
            return; // fixture tables
        }
        let mut d = dev(SimulatedGx100::new());
        d.load_into_temp(&patch_named("EDITED", 1)).unwrap();
        d.save_temp_patch(2, "MY TONE").unwrap();
        let stored = d.read_patch(user_patch(2).unwrap()).unwrap();
        assert_eq!(stored.name(), "MY TONE");
        assert!(d.save_temp_patch(0, "x").is_err() && d.save_temp_patch(201, "x").is_err());

        d.write_user_patch(1, &patch_named("OTHER", 2)).unwrap();
        d.select_patch(1).unwrap();
        assert_eq!(d.read_patch(temp_patch()).unwrap().name(), "OTHER");
        let cur = tables::param("System_common.CurrentPatchNum").unwrap();
        assert_eq!(d.read_param(cur).unwrap(), ParamValue::Num(0), "0-based");
    }

    #[test]
    fn chain_stops_on_loops() {
        let top = tables::param("Temp_patch.efct.ChainTopItem").unwrap();
        let next0 = tables::param("Temp_patch.efct.ChainNextItem0").unwrap();
        let mut bytes = vec![0u8; patch_span() as usize];
        bytes[top.addr.since(temp_patch()).unwrap() as usize] = 1; // slot 1
        bytes[next0.addr.since(temp_patch()).unwrap() as usize] = 1; // slot 1 again
        assert_eq!(PatchData::from_bytes(bytes).chain(), vec![1]);
    }
}
