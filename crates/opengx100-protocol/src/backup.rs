//! What a full backup contains: the same data the official editor's "backup all"
//! (`businesslogic/backup/all_data.js`) saves — the system areas of `SystemSetting`
//! (`config/editor_setting.js`) and every user patch.

use crate::address::Address;
use crate::session::{Device, Error, Result, Transport};
use crate::tables::{self, RootDef};

/// System memory areas in a backup, in the official editor's order.
/// (`Setup_comm` is left out there too; it is empty.)
pub const SYSTEM_AREAS: &[&str] = &[
    "System_common",
    "System_control",
    "System_midi",
    "System_inout",
    "System_efct",
    "System_pitch",
    "Pcmap_bank(1)",
    "Pcmap_bank(2)",
    "Pcmap_bank(3)",
    "Setup_temp",
    "Setup_temp2",
    "Setup_temp3",
    "Setup_efct",
    "System_inputSetting(1)",
    "System_inputSetting(2)",
    "System_inputSetting(3)",
    "System_inputSetting(4)",
    "System_inputSetting(5)",
    "System_inputSetting(6)",
    "System_inputSetting(7)",
    "System_inputSetting(8)",
    "System_inputSetting(9)",
    "System_inputSetting(10)",
    "System_globalEq(1)",
];

/// The system areas the tables know about (all of them with real tables).
pub fn system_areas() -> impl Iterator<Item = &'static RootDef> {
    SYSTEM_AREAS.iter().filter_map(|n| tables::root(n)).filter(|r| r.size > 0)
}

/// One system area as read from the pedal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreaData {
    pub name: &'static str,
    pub addr: Address,
    pub data: Vec<u8>,
}

impl<T: Transport> Device<T> {
    /// Reads every system area of a backup.
    pub fn read_system_areas(&mut self) -> Result<Vec<AreaData>> {
        let areas: Vec<&RootDef> = system_areas().collect();
        if areas.is_empty() {
            return Err(Error::Unknown("system areas (tables have none)".into()));
        }
        areas
            .into_iter()
            .map(|r| Ok(AreaData { name: r.name, addr: r.addr, data: self.read(r.addr, r.size)? }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimulatedGx100;
    use std::time::Duration;

    #[test]
    fn reads_every_known_area() {
        let mut sim = SimulatedGx100::new();
        let common = tables::root("System_common").unwrap();
        sim.poke(common.addr, &[7, 8, 9]);
        let mut d = Device::new(sim).without_pacing().with_timeout(Duration::from_millis(50));
        let areas = d.read_system_areas().unwrap();
        assert_eq!(areas.len(), system_areas().count());
        assert_eq!(areas[0].name, "System_common");
        assert_eq!(areas[0].data.len(), common.size as usize);
        assert_eq!(&areas[0].data[..3], &[7, 8, 9]);
        if *tables::SOURCE == tables::TablesSource::Extracted {
            assert_eq!(areas.len(), SYSTEM_AREAS.len());
        }
    }
}
