//! Parameter tables generated from the official editor's own data
//! (see `tools/extract`). Falls back to a tiny synthetic fixture when
//! no extracted tables are present.

use crate::address::Address;
use crate::value::{Encoding, ValueSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablesSource {
    /// Generated from an installed BOSS TONE STUDIO.
    Extracted,
    /// Synthetic test data: only a handful of parameters.
    Fixture,
}

#[derive(Debug)]
pub struct ProductDef {
    pub name: &'static str,
    pub bts_version: &'static str,
    pub bts_build: &'static str,
    pub model_id: [u8; 5],
    pub device_id: u8,
    pub communication_level: u8,
    pub family_code: [u8; 2],
}

#[derive(Debug, Clone, Copy)]
pub struct NamedAddress {
    pub name: &'static str,
    pub addr: Address,
}

/// A top-level memory area (System, Temp patch, User patch n, …).
#[derive(Debug, Clone, Copy)]
pub struct RootDef {
    pub name: &'static str,
    pub block: &'static str,
    pub addr: Address,
    /// Size in bytes.
    pub size: u32,
}

/// Default value as given by the editor (patch names default to text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Init {
    Num(i32),
    Text(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct ParamDef {
    /// e.g. `Temp_patch.fxItem(1).Switch`
    pub path: &'static str,
    pub desc: &'static str,
    pub addr: Address,
    pub encoding: Encoding,
    pub min: i32,
    pub max: i32,
    pub init: Init,
    pub ofs: i32,
    pub factor: i32,
}

impl ParamDef {
    pub fn spec(&self) -> ValueSpec {
        ValueSpec { encoding: self.encoding, min: self.min, max: self.max, ofs: self.ofs }
    }
}

/// Which field of an FX slot an [`FxParamDef`] lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxField {
    Type,
    Switch,
    DuplicationNumber,
    /// The generic `param(n)` slot, reinterpreted per FX type.
    Param(u8),
}

#[derive(Debug, Clone, Copy)]
pub struct FxParamDef {
    pub name: &'static str,
    /// Stable identifier, e.g. `drive`.
    pub unique_name: &'static str,
    pub field: FxField,
    /// Offset inside the FX slot.
    pub offset: Address,
    pub encoding: Encoding,
    pub min: i32,
    pub max: i32,
    pub init: Init,
    pub ofs: i32,
    /// Display divisor (e.g. BPM is stored ×10).
    pub factor: i32,
    /// Value labels, when the parameter is a list.
    pub values: &'static [&'static str],
    /// The editor's JS display expression, kept for reference.
    pub format: Option<&'static str>,
    pub template: &'static str,
}

impl FxParamDef {
    pub fn spec(&self) -> ValueSpec {
        ValueSpec { encoding: self.encoding, min: self.min, max: self.max, ofs: self.ofs }
    }

    /// Human-readable value: its label when the parameter is a list.
    pub fn display(&self, value: i32) -> String {
        if !self.values.is_empty() {
            let spans_range = self.values.len() as i64 == (self.max as i64 - self.min as i64 + 1);
            let idx = if spans_range { value - self.min } else { value };
            if let Some(label) = usize::try_from(idx).ok().and_then(|i| self.values.get(i)) {
                return (*label).to_string();
            }
        }
        if self.factor > 1 {
            let v = value as f64 / self.factor as f64;
            return if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
        }
        value.to_string()
    }

    /// Parses a value typed by a user: a label (case-insensitive) or a number.
    pub fn parse_value(&self, s: &str) -> Option<i32> {
        if let Some(i) = self.values.iter().position(|v| v.eq_ignore_ascii_case(s.trim())) {
            let spans_range = self.values.len() as i64 == (self.max as i64 - self.min as i64 + 1);
            return Some(if spans_range { self.min + i as i32 } else { i as i32 });
        }
        s.trim().parse().ok()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FxTypeDef {
    /// The value written to `fxItem(n).Type`.
    pub id: u8,
    pub name: &'static str,
    pub params: &'static [FxParamDef],
}

impl FxTypeDef {
    pub fn param(&self, unique_name: &str) -> Option<&'static FxParamDef> {
        self.params.iter().find(|p| p.unique_name.eq_ignore_ascii_case(unique_name))
    }
}

mod generated {
    use super::*;
    include!(concat!(env!("OUT_DIR"), "/tables.rs"));
}

pub use generated::{ADDRESSES, COMMANDS, FX_TYPES, PARAMS, PRODUCT, ROOT, SOURCE};

/// Looks up a parameter by its full path (e.g. `Temp_patch.common.Name`).
pub fn param(path: &str) -> Option<&'static ParamDef> {
    PARAMS.binary_search_by(|p| p.path.cmp(path)).ok().map(|i| &PARAMS[i])
}

/// The parameter starting exactly at `addr`, if any.
pub fn param_at(addr: Address) -> Option<&'static ParamDef> {
    PARAMS.iter().find(|p| p.addr == addr)
}

/// Case-insensitive search over parameter paths and descriptions.
pub fn search_params(query: &str) -> impl Iterator<Item = &'static ParamDef> + '_ {
    let q = query.to_ascii_lowercase();
    PARAMS.iter().filter(move |p| p.path.to_ascii_lowercase().contains(&q) || p.desc.to_ascii_lowercase().contains(&q))
}

fn named(list: &'static [NamedAddress], name: &str) -> Option<Address> {
    list.binary_search_by(|n| n.name.cmp(name)).ok().map(|i| list[i].addr)
}

/// A special command address (e.g. `PATCH_WRITE`).
pub fn command(name: &str) -> Option<Address> {
    named(COMMANDS, name)
}

/// A fixed address the editor uses directly (e.g. `CHAIN`).
pub fn address(name: &str) -> Option<Address> {
    named(ADDRESSES, name)
}

pub fn root(name: &str) -> Option<&'static RootDef> {
    ROOT.iter().find(|r| r.name == name)
}

pub fn fx_type(id: u8) -> Option<&'static FxTypeDef> {
    FX_TYPES.get(id as usize)
}

pub fn fx_type_by_name(name: &str) -> Option<&'static FxTypeDef> {
    FX_TYPES.iter().find(|t| t.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_for_binary_search() {
        assert!(PARAMS.windows(2).all(|w| w[0].path < w[1].path));
        assert!(COMMANDS.windows(2).all(|w| w[0].name < w[1].name));
        assert!(ADDRESSES.windows(2).all(|w| w[0].name < w[1].name));
    }

    #[test]
    fn fx_type_ids_match_positions() {
        assert!(FX_TYPES.iter().enumerate().all(|(i, t)| t.id as usize == i));
    }

    #[test]
    fn core_lookups() {
        let sw = param("Temp_patch.fxItem(1).Switch").expect("fx slot switch");
        assert_eq!(sw.addr.to_string(), "10 00 11 01");
        assert_eq!(param_at(sw.addr).map(|p| p.path), Some(sw.path));
        assert_eq!(command("EDITOR_COMMUNICATION_MODE").unwrap().to_string(), "7F 00 00 01");
        assert_eq!(root("Temp_patch").unwrap().addr.to_string(), "10 00 00 00");
        assert_eq!(PRODUCT.model_id, crate::sysex::MODEL_ID);
        assert!(param("nope").is_none());
    }

    #[test]
    fn fixture_specific() {
        if SOURCE != TablesSource::Fixture {
            return;
        }
        let alpha = fx_type_by_name("fx alpha").unwrap();
        let mode = alpha.param("MODE").unwrap();
        assert_eq!(mode.field, FxField::Param(0));
        assert_eq!(mode.display(2), "HARD");
        assert_eq!(mode.parse_value("mid"), Some(1));
        let rate = fx_type(1).unwrap().param("rate").unwrap();
        assert_eq!(rate.display(1200), "120");
        assert_eq!(rate.display(1205), "120.5");
        assert!(fx_type(2).unwrap().params.is_empty());
        assert_eq!(param("Temp_patch.common.Name").unwrap().init, Init::Text("INIT MEMORY     "));
        assert_eq!(param("Temp_patch.common.Level").unwrap().init, Init::Num(100));
    }
}
