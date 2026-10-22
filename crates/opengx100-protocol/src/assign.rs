//! Assigns: each patch has 20. An assign lets a controller (SOURCE: a footswitch, an expression pedal,
//! the internal/wave pedal, the input level, a MIDI CC) move one parameter (TARGET) between TARGET MIN
//! and TARGET MAX.
//!
//! TARGET indexes [`ASSIGN_TARGETS`]. An effect target names a parameter (by `unique_name`) of the FX in
//! slot `TARGET_FX_ITEM` (0-based), so what it means follows the FX type in that slot. MASTER, TUNER and
//! MIDI targets ignore `TARGET_FX_ITEM`.
//!
//! The editing helpers change a [`PatchData`] the way the official editor does and return the DT1s it
//! sends for that change, as (offset in the patch, data):
//! - a field edit: that field, then the whole assign block (`assign_page_controller.js` `flush`);
//! - a target picked from the list: TARGET, then `TARGET_FX_ITEM` + TARGET together (5 bytes);
//! - a category picked: `TARGET_FX_ITEM` + TARGET only.

use crate::patch::{patch_blocks, temp_offset, PatchData};
use crate::session::ParamValue;
use crate::tables::{self, AssignCategoryDef, AssignTargetDef, FxParamDef, FxTypeDef, ParamDef, ASSIGN_CATEGORIES};
use crate::tables::{ASSIGN_TARGETS, FX_TYPES, MASTER};

pub const ASSIGNS: u8 = 20;

/// SOURCE values the official editor treats specially (`disable_dial_bg.js`).
pub mod source {
    pub const EXP1: i32 = 16;
    pub const EXP2: i32 = 17;
    pub const INT_PEDAL: i32 = 18;
    pub const WAVE_PEDAL: i32 = 19;
    pub const INPUT: i32 = 20;
}

/// One message: bytes at an offset from the start of the patch.
pub type Message = (u32, Vec<u8>);

/// The definition of field `name` of assign `n` (1-based), e.g. `SOURCE`.
pub fn field(n: u8, name: &str) -> Option<&'static ParamDef> {
    tables::param(&format!("Temp_patch.assign({n}).{name}"))
}

/// The value of field `name` of assign `n`.
pub fn get(data: &PatchData, n: u8, name: &str) -> Option<i32> {
    match data.value(field(n, name)?)? {
        ParamValue::Num(v) => Some(v),
        ParamValue::Text(_) => None,
    }
}

/// Assign `n`'s block: (offset in the patch, size).
pub fn block(n: u8) -> Option<(u32, u32)> {
    let name = format!("assign({n})");
    patch_blocks().iter().find(|b| b.name == name).map(|b| (b.offset, b.size))
}

pub fn target_def(target: i32) -> Option<&'static AssignTargetDef> {
    ASSIGN_TARGETS.get(usize::try_from(target).ok()?)
}

/// The category for an FX type (or `MASTER`, `TUNER`, `MIDI`, `----`).
pub fn category_of(block_name: &str) -> Option<usize> {
    ASSIGN_CATEGORIES.iter().position(|c| c.block_name == block_name)
}

pub fn category(i: usize) -> Option<&'static AssignCategoryDef> {
    ASSIGN_CATEGORIES.get(i)
}

/// The shared ON/OFF target of effects with a switch.
fn on_off() -> Option<i32> {
    ASSIGN_TARGETS.iter().position(|t| t.category == -1).map(|i| i as i32)
}

/// The targets offered for a category, in the official editor's order: ON/OFF first when the
/// effect has a switch, then the category's own targets in table order.
pub fn targets_for(category: usize) -> Vec<i32> {
    let mut out = Vec::new();
    if ASSIGN_CATEGORIES.get(category).is_some_and(|c| c.has_switch) {
        out.extend(on_off());
    }
    out.extend(
        ASSIGN_TARGETS
            .iter()
            .enumerate()
            .filter(|(_, t)| t.category as isize == category as isize)
            .map(|(i, _)| i as i32),
    );
    out
}

/// What an assign points at.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    /// "----": nothing.
    None,
    /// A parameter of the FX in `slot` (1-based). `fx` is that slot's FX type, if known.
    Fx { slot: u8, fx: Option<&'static FxTypeDef>, def: &'static AssignTargetDef },
    /// A MASTER setting of the patch (level, BPM, key…).
    Master(&'static AssignTargetDef),
    /// Tuner on/off.
    Tuner,
    /// Sends a MIDI control change; its range is MIDI_CC_VALUE_MIN/MAX.
    MidiCc,
    /// Sends a MIDI program change.
    MidiPc,
    /// A TARGET value the tables don't know.
    Unknown(i32),
}

/// Table entries are compared by identity.
impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        use std::ptr::eq;
        match (self, other) {
            (Target::Fx { slot: a, fx: fa, def: da }, Target::Fx { slot: b, fx: fb, def: db }) => {
                a == b && eq(*da, *db) && fa.zip(*fb).map_or(fa.is_none() && fb.is_none(), |(x, y)| eq(x, y))
            }
            (Target::Master(a), Target::Master(b)) => eq(*a, *b),
            (Target::Unknown(a), Target::Unknown(b)) => a == b,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other) && !matches!(self, Target::Fx { .. }),
        }
    }
}

/// What assign `n` targets in this patch.
pub fn target(data: &PatchData, n: u8) -> Option<Target> {
    let value = get(data, n, "TARGET")?;
    let Some(def) = target_def(value) else { return Some(Target::Unknown(value)) };
    if def.effect {
        let slot = get(data, n, "TARGET_FX_ITEM")? as u8 + 1;
        return Some(Target::Fx { slot, fx: data.fx_slot(slot).and_then(|s| s.fx_type()), def });
    }
    let block = category(def.category.max(0) as usize).map(|c| c.block_name).unwrap_or("");
    Some(match (block, def.unique_name) {
        ("MASTER", _) => Target::Master(def),
        ("TUNER", _) => Target::Tuner,
        ("MIDI", "midi-cc#") => Target::MidiCc,
        ("MIDI", _) => Target::MidiPc,
        _ => Target::None,
    })
}

impl Target {
    /// The parameter the target moves, which gives TARGET MIN/MAX their range and labels.
    /// `None` for MIDI (its own fields hold the range), tuner on/off (OFF/ON) and nothing.
    pub fn param(&self) -> Option<&'static FxParamDef> {
        match self {
            Target::Fx { fx: Some(t), def, .. } => t.param(def.unique_name),
            Target::Master(def) => MASTER.param(def.unique_name),
            _ => None,
        }
    }

    /// Whether the target is a parameter of the FX in its slot. False when the slot holds another FX
    /// type than the target belongs to (e.g. the FX type was changed after the assign was made).
    pub fn matches_slot(&self) -> bool {
        match self {
            Target::Fx { fx: Some(t), def, .. } => {
                def.category == -1 || category(def.category as usize).is_some_and(|c| c.block_name == t.name)
            }
            Target::Fx { fx: None, .. } => false,
            _ => true,
        }
    }

    /// TARGET MIN/MAX apply (not to MIDI targets, whose range is in the MIDI fields).
    pub fn has_range(&self) -> bool {
        !matches!(self, Target::MidiCc | Target::MidiPc)
    }

    /// "OVERDRIVE: DRIVE", "MASTER: BPM", "TUNER: ON/OFF".
    pub fn label(&self, data: &PatchData) -> String {
        match self {
            Target::None => "----".into(),
            Target::Fx { slot, def, .. } => format!("{}: {}", slot_label(data, *slot), def.name),
            Target::Master(def) => format!("MASTER: {}", def.name),
            Target::Tuner => "TUNER: ON/OFF".into(),
            Target::MidiCc => "MIDI: CC#".into(),
            Target::MidiPc => "MIDI: PC#".into(),
            Target::Unknown(v) => format!("unknown target {v}"),
        }
    }
}

/// What an effect slot is called in the target list: its category name ("PREAMP" for AIRD PREAMP),
/// plus its duplication number when there's more than one of that FX.
pub fn slot_label(data: &PatchData, slot: u8) -> String {
    let Some(t) = data.fx_slot(slot).and_then(|s| s.fx_type()) else { return format!("slot {slot}") };
    let name = category_of(t.name).and_then(category).map(|c| c.name).filter(|n| !n.is_empty()).unwrap_or(t.name);
    let dup = t
        .params
        .iter()
        .find(|p| p.field == tables::FxField::DuplicationNumber)
        .and_then(|p| data.fx_param(slot, p))
        .unwrap_or(0);
    if dup > 0 {
        format!("{name} {dup}")
    } else {
        name.to_string()
    }
}

/// One entry of the category list.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryChoice {
    pub label: String,
    pub category: usize,
    /// The FX slot (1-based), for effect categories.
    pub slot: Option<u8>,
}

/// The category list for a patch, as the official editor builds it: "----", the effects in the chain
/// (in signal order; the SPLITTER marker left out), MASTER, TUNER, MIDI.
pub fn category_choices(data: &PatchData) -> Vec<CategoryChoice> {
    let fixed = |block: &str| {
        category_of(block).map(|c| CategoryChoice {
            label: ASSIGN_CATEGORIES[c].name.to_string(),
            category: c,
            slot: None,
        })
    };
    let mut out: Vec<CategoryChoice> = fixed("----").into_iter().collect();
    for slot in data.chain() {
        let Some(t) = data.fx_slot(slot).and_then(|s| s.fx_type()) else { continue };
        if let Some(c) = category_of(t.name) {
            out.push(CategoryChoice { label: slot_label(data, slot), category: c, slot: Some(slot) });
        }
    }
    out.extend(["MASTER", "TUNER", "MIDI"].into_iter().filter_map(fixed));
    out
}

/// Which entry of [`category_choices`] assign `n` is on, if any (an effect slot outside the chain isn't).
pub fn current_choice(data: &PatchData, n: u8, choices: &[CategoryChoice]) -> Option<usize> {
    let t = target(data, n)?;
    match t {
        Target::Fx { slot, .. } => choices.iter().position(|c| c.slot == Some(slot)),
        Target::Unknown(_) => None,
        _ => {
            let def = target_def(get(data, n, "TARGET")?)?;
            choices.iter().position(|c| c.slot.is_none() && c.category as i16 == def.category)
        }
    }
}

// ------------------------------------------------------------------ edits

fn set(data: &mut PatchData, p: &ParamDef, value: i32) -> Option<Message> {
    let offset = temp_offset(p.addr)?;
    let bytes = p.spec().encode(value).ok()?;
    data.apply(offset, &bytes);
    Some((offset, bytes))
}

/// Assign `n`'s whole block as one message (the official editor's `flush`).
pub fn block_message(data: &PatchData, n: u8) -> Option<Message> {
    let (offset, size) = block(n)?;
    Some((offset, data.bytes().get(offset as usize..(offset + size) as usize)?.to_vec()))
}

/// Sets field `name` of assign `n` and returns the DT1s the official editor sends: the field, then
/// the whole block. Like the official editor, keeps ACT RANGE LO below HI (moving the other one) and
/// clears BANK LSB when BANK MSB is set to OFF.
pub fn set_field(data: &mut PatchData, n: u8, name: &str, value: i32) -> Option<Vec<Message>> {
    let p = field(n, name)?;
    let first = set(data, p, value)?;
    let step = p.factor.max(1);
    match name {
        "ACT_RANGE_LO" if get(data, n, "ACT_RANGE_HI").is_some_and(|hi| value >= hi) => {
            set(data, field(n, "ACT_RANGE_HI")?, value + step)?;
        }
        "ACT_RANGE_HI" if get(data, n, "ACT_RANGE_LO").is_some_and(|lo| value <= lo) => {
            set(data, field(n, "ACT_RANGE_LO")?, value - step)?;
        }
        "MIDI_BANK_MSB" if value == 0 => {
            set(data, field(n, "MIDI_BANK_LSB")?, 0)?;
        }
        _ => {}
    }
    Some(vec![first, block_message(data, n)?])
}

/// `TARGET_FX_ITEM` and TARGET in one message, as the official editor sends after a target change.
fn target_message(data: &PatchData, n: u8) -> Option<Message> {
    let p = field(n, "TARGET_FX_ITEM")?;
    let offset = temp_offset(p.addr)?;
    let len = p.encoding.size() + field(n, "TARGET")?.encoding.size();
    Some((offset, data.bytes().get(offset as usize..(offset + len) as usize)?.to_vec()))
}

/// Picks target `target` from the current category's list.
pub fn set_target(data: &mut PatchData, n: u8, target: i32) -> Option<Vec<Message>> {
    let first = set(data, field(n, "TARGET")?, target)?;
    Some(vec![first, target_message(data, n)?])
}

/// Picks an entry of the category list: points the assign at that slot (for effects) and at the
/// category's first target (ON/OFF for effects with a switch). TARGET MIN/MAX are left alone, as the
/// official editor does.
pub fn set_category(data: &mut PatchData, n: u8, choice: &CategoryChoice) -> Option<Vec<Message>> {
    if let Some(slot) = choice.slot {
        set(data, field(n, "TARGET_FX_ITEM")?, slot as i32 - 1)?;
    }
    let first = *targets_for(choice.category).first()?;
    set(data, field(n, "TARGET")?, first)?;
    Some(vec![target_message(data, n)?])
}

/// Which settings of an assign apply, given its source and target (the official editor greys out the rest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applies {
    pub target_range: bool,
    pub mode: bool,
    pub int_pedal: bool,
    pub wave_pedal: bool,
    pub input_sens: bool,
    pub midi_ch: bool,
    pub midi_cc: bool,
    pub midi_pc: bool,
    pub bank_lsb: bool,
}

pub fn applies(data: &PatchData, n: u8) -> Applies {
    let t = target(data, n).unwrap_or(Target::None);
    let src = get(data, n, "SOURCE").unwrap_or(-1);
    let (cc, pc) = (t == Target::MidiCc, t == Target::MidiPc);
    let continuous = matches!(src, source::EXP1 | source::EXP2 | source::INT_PEDAL | source::WAVE_PEDAL);
    Applies {
        target_range: t.has_range(),
        mode: !continuous && !pc,
        int_pedal: src == source::INT_PEDAL,
        wave_pedal: src == source::WAVE_PEDAL,
        input_sens: src == source::INPUT,
        midi_ch: cc || pc,
        midi_cc: cc,
        midi_pc: pc,
        bank_lsb: pc && get(data, n, "MIDI_BANK_MSB").is_some_and(|v| v != 0),
    }
}

/// The FX type an effect category belongs to.
pub fn category_fx(category: usize) -> Option<&'static FxTypeDef> {
    let c = ASSIGN_CATEGORIES.get(category)?;
    FX_TYPES.iter().find(|t| t.name == c.block_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::patch_span;

    fn blank() -> PatchData {
        PatchData::from_bytes(vec![0; patch_span() as usize])
    }

    #[test]
    fn every_category_offers_its_own_targets() {
        for (i, c) in ASSIGN_CATEGORIES.iter().enumerate() {
            let list = targets_for(i);
            if c.name.is_empty() {
                continue;
            }
            assert!(!list.is_empty() || c.block_name.is_empty(), "{} has no targets", c.name);
            assert_eq!(c.has_switch, list.first() == on_off().as_ref(), "{}", c.name);
        }
    }

    #[test]
    fn effect_targets_name_parameters_of_their_fx() {
        // The official tables have two targets without a matching parameter (AIRD BASS PREAMP mic-dist,
        // DIVIDER signal-mix); its editor shows no range for them, and so do we.
        let mut missing = Vec::new();
        for (i, t) in ASSIGN_TARGETS.iter().enumerate() {
            if !t.effect || t.category < 0 {
                continue;
            }
            let Some(fx) = category_fx(t.category as usize) else { continue };
            if fx.param(t.unique_name).is_none() {
                missing.push(format!("target {i} {} not in {}", t.unique_name, fx.name));
            }
        }
        assert!(missing.len() <= 2, "{missing:?}");
    }

    #[test]
    fn every_assign_has_the_labels_of_assign_1() {
        let Some(first) = field(1, "SOURCE").and_then(|p| p.labels()) else { return };
        for n in 2..=ASSIGNS {
            let labels = field(n, "SOURCE").and_then(|p| p.labels()).map(|l| l.labels);
            assert_eq!(labels, Some(first.labels), "assign {n}");
        }
    }

    #[test]
    fn field_edits_send_the_field_then_the_whole_block() {
        let Some((offset, size)) = block(1) else { return };
        let mut d = blank();
        let msgs = set_field(&mut d, 1, "SW", 1).unwrap();
        let sw = temp_offset(field(1, "SW").unwrap().addr).unwrap();
        assert_eq!(msgs[0], (sw, vec![1]));
        assert_eq!(msgs[1].0, offset);
        assert_eq!(msgs[1].1.len(), size as usize);
        assert_eq!(get(&d, 1, "SW"), Some(1));
    }

    #[test]
    fn act_range_and_bank_stay_consistent() {
        if field(1, "ACT_RANGE_LO").is_none() {
            return;
        }
        let mut d = blank();
        set_field(&mut d, 1, "ACT_RANGE_HI", 64 * 128).unwrap();
        set_field(&mut d, 1, "ACT_RANGE_LO", 64 * 128).unwrap();
        assert_eq!(get(&d, 1, "ACT_RANGE_HI"), Some(65 * 128));
        set_field(&mut d, 1, "ACT_RANGE_HI", 10 * 128).unwrap();
        assert_eq!(get(&d, 1, "ACT_RANGE_LO"), Some(9 * 128));
        set_field(&mut d, 1, "MIDI_BANK_LSB", 5).unwrap();
        set_field(&mut d, 1, "MIDI_BANK_MSB", 0).unwrap();
        assert_eq!(get(&d, 1, "MIDI_BANK_LSB"), Some(0));
    }

    #[test]
    fn choosing_targets_sends_fx_item_and_target() {
        if block(1).is_none() {
            return;
        }
        let mut d = blank();
        let fx_item = temp_offset(field(1, "TARGET_FX_ITEM").unwrap().addr).unwrap();
        let tuner = category_of("TUNER").unwrap();
        let msgs =
            set_category(&mut d, 1, &CategoryChoice { label: "TUNER".into(), category: tuner, slot: None }).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, fx_item);
        assert_eq!(msgs[0].1.len(), 5);
        assert_eq!(target(&d, 1), Some(Target::Tuner));

        // An effect: its slot and ON/OFF (or its first target).
        let fx = (0..ASSIGN_CATEGORIES.len()).find(|&c| category_fx(c).is_some()).unwrap();
        set_category(&mut d, 1, &CategoryChoice { label: String::new(), category: fx, slot: Some(3) }).unwrap();
        assert_eq!(get(&d, 1, "TARGET_FX_ITEM"), Some(2));
        assert_eq!(get(&d, 1, "TARGET"), targets_for(fx).first().copied());

        let midi = targets_for(category_of("MIDI").unwrap());
        let msgs = set_target(&mut d, 1, midi[0]).unwrap();
        let target_at = temp_offset(field(1, "TARGET").unwrap().addr).unwrap();
        assert_eq!((msgs[0].0, msgs[1].0), (target_at, fx_item));
        assert_eq!(target(&d, 1), Some(Target::MidiCc));
        assert!(!applies(&d, 1).target_range && applies(&d, 1).midi_cc);
    }

    #[test]
    fn source_decides_what_applies() {
        if block(1).is_none() {
            return;
        }
        let mut d = blank();
        set_field(&mut d, 1, "SOURCE", source::EXP1).unwrap();
        assert!(!applies(&d, 1).mode);
        if field(1, "SOURCE").unwrap().max >= source::INPUT {
            set_field(&mut d, 1, "SOURCE", source::INPUT).unwrap();
            let a = applies(&d, 1);
            assert!(a.mode && a.input_sens && !a.int_pedal);
        }
    }
}
