//! Patch-level helpers: where patches and FX slots live, and reading/writing FX params.

use crate::address::Address;
use crate::session::{Device, Error, Result, Transport};
use crate::tables::{self, FxParamDef, FxTypeDef};

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

impl<T: Transport> Device<T> {
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
}
