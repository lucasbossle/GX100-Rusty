//! Editing the signal chain: inserting, removing, moving and replacing effects, the way the official
//! editor does it (`chain_controller.js`: `addAfter`, `delete`, `moveBefore`, `overwrite`).
//!
//! The chain is a linked list in the `efct` block (see [`PatchData::chain`]): `ChainTopItem`, then one
//! `ChainNextItem<i>` per item. An item pointing at itself is free. An edit:
//! 1. `Setup_temp.ChainEditTrigger` = 1;
//! 2. the FX type of each new slot (`fxItem(n).Type`, one byte); a DIVIDER comes with a SPLITTER and a
//!    MIXER, in the first three free slots;
//! 3. the whole list (`ChainTopItem` + every `ChainNextItem`, 50 bytes) in one DT1;
//! 4. `ChainEditTrigger` = 0 (sent when a drag ends).
//!
//! A replacement is a removal followed by an insertion into the same slot at the same place
//! (the official editor waits for its reply to an RQ1 in between). Removing is only the new list: the
//! pedal itself resets what the removed slot held.
//!
//! What fits depends on the DSP load of the current chain; the pedal says so in `Setup_temp2`
//! (can this FX type be added?) and `Setup_temp3` (can it replace the effect at position p?), see
//! [`Limits`]. The editor's own rules come on top: at most 15 effects, 9 of one type, 2 preamps, one
//! DIVIDER, PHRASE LOOP and SEND/RETURN.

use crate::address::Address;
use crate::librarian::Dt1;
use crate::patch::{fx_slot_base, temp_patch, PatchData, FX_SLOTS};
use crate::session::{Device, Result, Transport};
use crate::tables::{self, ParamDef};

/// At most this many effects of one type (`MAX_BLOCK_OF_SAME_TYPE`).
pub const MAX_SAME_TYPE: usize = 9;
/// At most this many effects; DIVIDER, PHRASE LOOP and SEND/RETURN don't count (`MAX_EFFECT_BLOCK`).
pub const MAX_EFFECTS: usize = 15;
/// AIRD PREAMP and AIRD BASS PREAMP together.
pub const MAX_PREAMPS: usize = 2;

/// What an FX type means to the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Divider,
    /// Only a marker: where branch B starts.
    Splitter,
    Mixer,
    Looper,
    SendReturn,
    Preamp,
    Effect,
}

impl Kind {
    pub fn of(type_id: u8) -> Kind {
        match tables::fx_type(type_id).map(|t| t.name).unwrap_or("") {
            "DIVIDER" => Kind::Divider,
            "SPLITTER" => Kind::Splitter,
            "MIXER" => Kind::Mixer,
            "PHRASE LOOP" => Kind::Looper,
            "SEND/RETURN" => Kind::SendReturn,
            "AIRD PREAMP" | "AIRD BASS PREAMP" => Kind::Preamp,
            _ => Kind::Effect,
        }
    }

    /// DIVIDER, SPLITTER and MIXER: they route the signal, and come and go together.
    pub fn is_routing(self) -> bool {
        matches!(self, Kind::Divider | Kind::Splitter | Kind::Mixer)
    }

    /// Counts towards [`MAX_EFFECTS`].
    fn is_effect(self) -> bool {
        matches!(self, Kind::Preamp | Kind::Effect)
    }

    /// At most one in a chain.
    fn is_single(self) -> bool {
        matches!(self, Kind::Divider | Kind::Looper | Kind::SendReturn)
    }
}

/// The FX types that can be put into a chain (everything but SPLITTER and MIXER, which come with a DIVIDER).
pub fn insertable_types() -> impl Iterator<Item = &'static tables::FxTypeDef> {
    tables::FX_TYPES.iter().filter(|t| !matches!(Kind::of(t.id), Kind::Splitter | Kind::Mixer))
}

/// `ChainTopItem` and every `ChainNextItem<i>`, in address order.
fn link_params() -> Vec<&'static ParamDef> {
    let mut ps: Vec<&'static ParamDef> = tables::PARAMS
        .iter()
        .filter(|p| p.path == "Temp_patch.efct.ChainTopItem" || p.path.starts_with("Temp_patch.efct.ChainNextItem"))
        .collect();
    ps.sort_by_key(|p| p.addr);
    ps
}

/// The chain's linked list: `[0]` is `ChainTopItem`, `[1 + i]` is `ChainNextItem<i>`. Values are item
/// numbers (slot - 1); -1 ends the chain; an item pointing at itself is free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Links(pub Vec<i32>);

impl Links {
    pub fn read(data: &PatchData) -> Option<Links> {
        let ps = link_params();
        if ps.first()?.path != "Temp_patch.efct.ChainTopItem" {
            return None;
        }
        let values = ps.iter().map(|p| match data.value(p)? {
            crate::ParamValue::Num(n) => Some(n),
            crate::ParamValue::Text(_) => None,
        });
        values.collect::<Option<Vec<i32>>>().map(Links)
    }

    /// Where the list starts on the pedal (`ChainTopItem`).
    pub fn address() -> Option<Address> {
        link_params().first().filter(|p| p.path == "Temp_patch.efct.ChainTopItem").map(|p| p.addr)
    }

    /// The list as stored, one byte per link.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let ps = link_params();
        if ps.len() != self.0.len() {
            return None;
        }
        let mut out = Vec::new();
        for (p, &v) in ps.iter().zip(&self.0) {
            out.extend(p.spec().encode(v).ok()?);
        }
        Some(out)
    }

    fn next(&self, item: i32) -> Option<i32> {
        usize::try_from(item + 1).ok().and_then(|i| self.0.get(i)).copied()
    }

    /// FX slots (1-based) in signal order, as [`PatchData::chain`].
    pub fn order(&self) -> Vec<u8> {
        let mut order = Vec::new();
        let mut cur = self.0.first().copied();
        while let Some(i) = cur.filter(|&i| (0..FX_SLOTS as i32).contains(&i)) {
            let slot = i as u8 + 1;
            if order.contains(&slot) {
                break;
            }
            order.push(slot);
            cur = self.next(i);
        }
        order
    }

    /// Slots that are free (their item points at itself), lowest first, as `findTheFirstUnusedIndex` picks them.
    pub fn free_slots(&self) -> Vec<u8> {
        let order = self.order();
        (1..=FX_SLOTS).filter(|&s| self.next(s as i32 - 1) == Some(s as i32 - 1) && !order.contains(&s)).collect()
    }

    /// The list with the chain changed to `order`: slots that left the chain become free, every other
    /// link stays as it was.
    pub fn relinked(&self, order: &[u8]) -> Links {
        let mut l = self.clone();
        let mut set = |slot: u8, v: i32| {
            if let Some(x) = l.0.get_mut(slot as usize) {
                *x = v;
            }
        };
        for s in self.order() {
            if !order.contains(&s) {
                set(s, s as i32 - 1);
            }
        }
        for w in order.windows(2) {
            set(w[0], w[1] as i32 - 1);
        }
        if let Some(&last) = order.last() {
            set(last, -1);
        }
        l.0[0] = order.first().map_or(-1, |&s| s as i32 - 1);
        l
    }
}

/// What the pedal says still fits, read from `Setup_temp2` and `Setup_temp3`. `None`: not known
/// (tables without them); then only the editor's own rules apply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    /// One byte per FX type: 1 = can be added (`FxItemSelectable`).
    pub insert: Option<Vec<u8>>,
    /// Three sections (positions 0-6, 7-13, 14-20) of one byte per FX type; bit `p % 7` = can replace
    /// the effect at position `p` of the chain (`FxItemSelectableIfOverwriteA/B/C`).
    pub overwrite: Option<Vec<u8>>,
}

impl Limits {
    /// Nothing known: every rule of the pedal passes.
    pub fn unknown() -> Limits {
        Limits::default()
    }

    pub fn can_insert(&self, fx: u8) -> bool {
        self.insert.as_ref().is_none_or(|v| v.get(fx as usize) == Some(&1))
    }

    /// Whether `fx` can replace the effect at `position` (0-based, in signal order).
    pub fn can_overwrite(&self, fx: u8, position: usize) -> bool {
        let Some(v) = &self.overwrite else { return true };
        let section = v.len() / 3;
        v.get((position / 7) * section + fx as usize).is_some_and(|b| (b >> (position % 7)) & 1 == 1)
    }
}

/// The `Setup_temp2`/`Setup_temp3` parameters whose path starts with `prefix`, in address order.
fn limit_params(prefix: &str) -> Vec<&'static ParamDef> {
    let mut ps: Vec<&'static ParamDef> = tables::PARAMS.iter().filter(|p| p.path.starts_with(prefix)).collect();
    ps.sort_by_key(|p| p.addr);
    ps
}

const INSERT_PREFIX: &str = "Setup_temp2.FxItemSelectable(";
const OVERWRITE_PREFIX: &str = "Setup_temp3.FxItemSelectableIfOverwrite";

/// A chain change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Add an effect of type `fx` before position `at` of the chain (`chain.len()`: at the end).
    Insert { at: usize, fx: u8 },
    /// Take the effect in `slot` out of the chain. A DIVIDER or MIXER takes the whole parallel
    /// section's routing with it (DIVIDER, SPLITTER, MIXER); the branches' effects stay, in series.
    Remove { slot: u8 },
    /// Move the effect in `slot` before position `to` of the chain as it is now (`chain.len()`: to the
    /// end). Moving a DIVIDER or MIXER moves the whole parallel section.
    Move { slot: u8, to: usize },
    /// Put an effect of type `fx` in place of the one in `slot`.
    Replace { slot: u8, fx: u8 },
}

/// One step of a chain edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Write(Dt1),
    /// Ask the pedal something and wait for the answer, so it has handled everything before.
    Sync,
}

/// How to carry out an [`Edit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// The chain afterwards (1-based slots, signal order).
    pub chain: Vec<u8>,
    /// The slot to show afterwards: the new or moved effect.
    pub select: Option<u8>,
}

fn type_name(fx: u8) -> String {
    tables::fx_type(fx).map(|t| t.name.to_string()).unwrap_or_else(|| format!("FX type {fx}"))
}

/// What's in the chain, by slot.
struct Snapshot<'a> {
    data: &'a PatchData,
    links: Links,
    chain: Vec<u8>,
}

impl Snapshot<'_> {
    fn type_of(&self, slot: u8) -> Option<u8> {
        self.data.fx_slot(slot).map(|s| s.type_id)
    }

    fn kind_of(&self, slot: u8) -> Kind {
        self.type_of(slot).map(Kind::of).unwrap_or(Kind::Effect)
    }

    fn position(&self, slot: u8) -> Option<usize> {
        self.chain.iter().position(|&s| s == slot)
    }

    /// Positions of the DIVIDER and the MIXER.
    fn section(&self) -> Option<(usize, usize)> {
        let find = |k: Kind| self.chain.iter().position(|&s| self.kind_of(s) == k);
        Some((find(Kind::Divider)?, find(Kind::Mixer)?))
    }

    /// The editor's rules for adding `fx`, not counting `replacing` (the slot an effect is replaced in).
    fn check_rules(&self, fx: u8, replacing: Option<u8>) -> std::result::Result<(), String> {
        let kind = Kind::of(fx);
        let name = type_name(fx);
        if tables::fx_type(fx).is_none() || matches!(kind, Kind::Splitter | Kind::Mixer) {
            return Err(format!("{name} can't be added on its own"));
        }
        let others: Vec<u8> = self.chain.iter().copied().filter(|&s| Some(s) != replacing).collect();
        let kinds: Vec<Kind> = others.iter().map(|&s| self.kind_of(s)).collect();
        if kind.is_single() && kinds.contains(&kind) {
            return Err(format!("the chain already has a {name}"));
        }
        if others.iter().filter(|&&s| self.type_of(s) == Some(fx)).count() >= MAX_SAME_TYPE {
            return Err(format!("the chain already has {MAX_SAME_TYPE} × {name}"));
        }
        if kind == Kind::Preamp && kinds.iter().filter(|&&k| k == Kind::Preamp).count() >= MAX_PREAMPS {
            return Err(format!("the chain already has {MAX_PREAMPS} preamps"));
        }
        if kind.is_effect() && kinds.iter().filter(|k| k.is_effect()).count() >= MAX_EFFECTS {
            return Err(format!("the chain already has {MAX_EFFECTS} effects"));
        }
        Ok(())
    }
}

fn trigger(on: bool) -> Option<Step> {
    let p = tables::param("Setup_temp.ChainEditTrigger")?;
    Some(Step::Write(Dt1 { addr: p.addr, data: vec![on as u8] }))
}

fn type_message(slot: u8, fx: u8) -> std::result::Result<Step, String> {
    let addr = fx_slot_base(temp_patch(), slot).ok_or_else(|| format!("no FX slot {slot}"))?;
    Ok(Step::Write(Dt1 { addr, data: vec![fx] }))
}

fn links_message(links: &Links) -> std::result::Result<Step, String> {
    let addr = Links::address().ok_or("the chain's address is unknown")?;
    let data = links.encode().ok_or("the chain can't be encoded")?;
    Ok(Step::Write(Dt1 { addr, data }))
}

/// The new slots for an effect of type `fx`: one, or three for a DIVIDER (with SPLITTER and MIXER),
/// taken from `free` in order, with the type each gets.
fn new_slots(fx: u8, free: &[u8]) -> std::result::Result<Vec<(u8, u8)>, String> {
    let types = if Kind::of(fx) == Kind::Divider {
        let id = |n: &str| tables::fx_type_by_name(n).map(|t| t.id).ok_or(format!("FX type {n} is unknown"));
        vec![fx, id("SPLITTER")?, id("MIXER")?]
    } else {
        vec![fx]
    };
    if free.len() < types.len() {
        return Err(match types.len() {
            1 => "no free FX slot".to_string(),
            n => format!("a DIVIDER needs {n} free FX slots"),
        });
    }
    Ok(free.iter().copied().zip(types).collect())
}

/// How to carry out `edit` on the live patch `data`, or why it can't be done.
pub fn plan(data: &PatchData, limits: &Limits, edit: &Edit) -> std::result::Result<Plan, String> {
    let links = Links::read(data).ok_or("the chain can't be read")?;
    let snap = Snapshot { data, chain: links.order(), links };
    let chain = &snap.chain;
    let mut steps: Vec<Step> = trigger(true).into_iter().collect();
    let (new_chain, select) = match *edit {
        Edit::Insert { at, fx } => {
            if at > chain.len() {
                return Err("no such position".into());
            }
            snap.check_rules(fx, None)?;
            if !limits.can_insert(fx) {
                return Err(format!("the pedal can't fit a {} into this patch", type_name(fx)));
            }
            let added = new_slots(fx, &snap.links.free_slots())?;
            for &(slot, ty) in &added {
                steps.push(type_message(slot, ty)?);
            }
            let mut c = chain.clone();
            c.splice(at..at, added.iter().map(|&(s, _)| s));
            steps.push(links_message(&snap.links.relinked(&c))?);
            (c, Some(added[0].0))
        }
        Edit::Remove { slot } => {
            let at = snap.position(slot).ok_or(format!("slot {slot} isn't in the chain"))?;
            let gone: Vec<u8> = if snap.kind_of(slot).is_routing() {
                chain.iter().copied().filter(|&s| snap.kind_of(s).is_routing()).collect()
            } else {
                vec![slot]
            };
            let c: Vec<u8> = chain.iter().copied().filter(|s| !gone.contains(s)).collect();
            steps.push(links_message(&snap.links.relinked(&c))?);
            // Show the next effect, else the one before.
            let effect = |s: &&u8| !snap.kind_of(**s).is_routing();
            let at = at.min(c.len());
            let select = c[at..].iter().find(effect).or_else(|| c[..at].iter().rev().find(effect)).copied();
            (c, select)
        }
        Edit::Move { slot, to } => {
            let at = snap.position(slot).ok_or(format!("slot {slot} isn't in the chain"))?;
            if to > chain.len() {
                return Err("no such position".into());
            }
            let (from, end) = match snap.kind_of(slot) {
                Kind::Splitter => return Err("the SPLITTER can't be moved".into()),
                Kind::Divider | Kind::Mixer => {
                    let (d, m) = snap.section().ok_or("the parallel section is incomplete")?;
                    if d < to && to <= m {
                        return Err("the parallel section can't go inside itself".into());
                    }
                    (d, m + 1)
                }
                _ => (at, at + 1),
            };
            if to == from || to == end {
                return Err("it's already there".into());
            }
            let mut c = chain.clone();
            let moved: Vec<u8> = c.drain(from..end).collect();
            let to = if to > from { to - moved.len() } else { to };
            c.splice(to..to, moved);
            steps.push(links_message(&snap.links.relinked(&c))?);
            (c, Some(slot))
        }
        Edit::Replace { slot, fx } => {
            let at = snap.position(slot).ok_or(format!("slot {slot} isn't in the chain"))?;
            if snap.kind_of(slot).is_routing() {
                return Err(format!("the {} can't be replaced", type_name(snap.type_of(slot).unwrap_or(0))));
            }
            snap.check_rules(fx, Some(slot))?;
            if !limits.can_overwrite(fx, at) {
                return Err(format!("the pedal can't fit a {} here", type_name(fx)));
            }
            // First take the effect out (its slot becomes free)…
            let mut c = chain.clone();
            c.remove(at);
            let removed = snap.links.relinked(&c);
            steps.push(links_message(&removed)?);
            steps.push(Step::Sync);
            // …then add the new one in the same slot, at the same place. A DIVIDER takes the first
            // free slots, as when it's added.
            let added =
                if Kind::of(fx) == Kind::Divider { new_slots(fx, &removed.free_slots())? } else { vec![(slot, fx)] };
            for &(s, ty) in &added {
                steps.push(type_message(s, ty)?);
            }
            c.splice(at..at, added.iter().map(|&(s, _)| s));
            steps.push(links_message(&removed.relinked(&c))?);
            (c, Some(added[0].0))
        }
    };
    steps.extend(trigger(false));
    Ok(Plan { steps, chain: new_chain, select })
}

/// The messages of a chain edit that changes nothing (the list as it is), to check that a pedal takes
/// the sequence without changing anything.
pub fn unchanged(data: &PatchData) -> std::result::Result<Plan, String> {
    let links = Links::read(data).ok_or("the chain can't be read")?;
    let mut steps: Vec<Step> = trigger(true).into_iter().collect();
    steps.push(links_message(&links)?);
    steps.extend(trigger(false));
    Ok(Plan { steps, chain: links.order(), select: None })
}

impl<T: Transport> Device<T> {
    /// Reads what the pedal says still fits in the live patch (see [`Limits`]).
    pub fn read_chain_limits(&mut self) -> Result<Limits> {
        let mut read = |prefix: &str| -> Result<Option<Vec<u8>>> {
            let ps = limit_params(prefix);
            let (Some(first), Some(last)) = (ps.first(), ps.last()) else { return Ok(None) };
            let size = last.addr.since(first.addr).unwrap_or(0) + last.encoding.size();
            self.read(first.addr, size).map(Some)
        };
        Ok(Limits { insert: read(INSERT_PREFIX)?, overwrite: read(OVERWRITE_PREFIX)? })
    }

    /// Carries out a chain edit (see [`plan`]), then waits until the pedal accepts reads again.
    /// The live patch should be re-read afterwards: the pedal resets the slots it changed.
    pub fn edit_chain(&mut self, plan: &Plan) -> Result<()> {
        for step in &plan.steps {
            match step {
                Step::Write(m) => self.write(m.addr, &m.data)?,
                Step::Sync => {
                    self.read_chain_limits()?;
                }
            }
        }
        self.wait_until_ready()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::patch_span;
    use crate::sim::SimulatedGx100;
    use crate::tables::TablesSource;
    use std::time::Duration;

    fn real_tables() -> bool {
        *tables::SOURCE == TablesSource::Extracted
    }

    fn id(name: &str) -> u8 {
        tables::fx_type_by_name(name).unwrap().id
    }

    /// The default patch with chain `slots`, slot `s` holding FX type `types[s - 1]`; other slots free.
    fn patch(slots: &[u8], types: &[u8]) -> PatchData {
        let sim = SimulatedGx100::new();
        let mut data = PatchData::from_bytes(sim.peek(temp_patch(), patch_span()));
        let free = Links::read(&data).unwrap();
        let n = free.0.len();
        let links = Links((0..n as i32).map(|i| i - 1).collect()).relinked(slots);
        assert_eq!(links.0.len(), n);
        let at = Links::address().unwrap().since(temp_patch()).unwrap();
        data.apply(at, &links.encode().unwrap());
        for (i, &t) in types.iter().enumerate() {
            data.apply(fx_slot_base(temp_patch(), i as u8 + 1).unwrap().since(temp_patch()).unwrap(), &[t]);
        }
        data
    }

    fn writes(p: &Plan) -> Vec<&Dt1> {
        p.steps
            .iter()
            .filter_map(|s| match s {
                Step::Write(m) => Some(m),
                Step::Sync => None,
            })
            .collect()
    }

    #[test]
    fn relinking_frees_and_links() {
        // items 0..4; chain 2 -> 0 -> 3; 1 and 4 free
        let l = Links(vec![2, 3, 1, 0, -1, 4]);
        assert_eq!(l.order(), vec![3, 1, 4]);
        assert_eq!(l.free_slots(), vec![2, 5]);
        // drop slot 1, add slot 5 at the front
        let r = l.relinked(&[5, 3, 4]);
        assert_eq!(r.order(), vec![5, 3, 4]);
        assert_eq!(r.0, vec![4, 0, 1, 3, -1, 2], "slot 1 points at itself again");
        assert_eq!(r.free_slots(), vec![1, 2]);
        assert_eq!(l.relinked(&[]).0[0], -1, "empty chain");
        assert_eq!(l.relinked(&l.order()), l, "same order, same list");
    }

    #[test]
    fn limits_bits() {
        let mut ow = vec![0u8; 3 * 4];
        ow[2] = 0b000_0100; // type 2 at position 2
        ow[4 + 1] = 0b000_0001; // type 1 at position 7
        let l = Limits { insert: Some(vec![1, 0, 1, 1]), overwrite: Some(ow) };
        assert!(l.can_insert(0) && !l.can_insert(1) && !l.can_insert(9));
        assert!(l.can_overwrite(2, 2) && !l.can_overwrite(2, 3) && !l.can_overwrite(1, 2));
        assert!(l.can_overwrite(1, 7) && !l.can_overwrite(1, 21));
        assert!(Limits::unknown().can_insert(5) && Limits::unknown().can_overwrite(5, 20));
    }

    #[test]
    fn insert_sends_type_then_links() {
        if !real_tables() {
            return;
        }
        let (od, ds) = (id("OVERDRIVE"), id("DISTORTION"));
        let data = patch(&[1, 2], &[od, ds]);
        let p = plan(&data, &Limits::unknown(), &Edit::Insert { at: 1, fx: id("CHORUS") }).unwrap();
        assert_eq!(p.chain, vec![1, 3, 2]);
        assert_eq!(p.select, Some(3));
        let w = writes(&p);
        let trig = tables::param("Setup_temp.ChainEditTrigger").unwrap().addr;
        assert_eq!((w[0].addr, &w[0].data[..]), (trig, &[1][..]));
        assert_eq!((w[1].addr, &w[1].data[..]), (fx_slot_base(temp_patch(), 3).unwrap(), &[id("CHORUS")][..]));
        assert_eq!(w[2].addr, Links::address().unwrap());
        assert_eq!(w[2].data.len(), 50, "ChainTopItem + 49 ChainNextItems, as the official editor flushes");
        assert_eq!(&w[2].data[..4], &[1, 3, 0, 2], "top = item 0; 0 -> 2; 1 -> end; 2 -> 1 (stored + 1)");
        assert_eq!((w[3].addr, &w[3].data[..]), (trig, &[0][..]));
        assert_eq!(w.len(), 4);
    }

    #[test]
    fn divider_comes_with_splitter_and_mixer() {
        if !real_tables() {
            return;
        }
        let od = id("OVERDRIVE");
        let data = patch(&[1, 2], &[od, od]);
        let p = plan(&data, &Limits::unknown(), &Edit::Insert { at: 2, fx: id("DIVIDER") }).unwrap();
        assert_eq!(p.chain, vec![1, 2, 3, 4, 5]);
        let types: Vec<u8> = writes(&p)[1..4].iter().map(|m| m.data[0]).collect();
        assert_eq!(types, vec![id("DIVIDER"), id("SPLITTER"), id("MIXER")]);
        // a second one is refused, and so are SPLITTER/MIXER on their own
        let mut with = data.clone();
        with.apply(Links::address().unwrap().since(temp_patch()).unwrap(), &writes(&p)[4].data);
        for (i, &t) in [id("DIVIDER"), id("SPLITTER"), id("MIXER")].iter().enumerate() {
            with.apply(fx_slot_base(temp_patch(), 3 + i as u8).unwrap().since(temp_patch()).unwrap(), &[t]);
        }
        assert_eq!(with.chain(), vec![1, 2, 3, 4, 5]);
        assert!(with.chain_layout().branch.is_some());
        assert!(plan(&with, &Limits::unknown(), &Edit::Insert { at: 0, fx: id("DIVIDER") }).is_err());
        assert!(plan(&with, &Limits::unknown(), &Edit::Insert { at: 0, fx: id("MIXER") }).is_err());

        // removing the MIXER removes all three, the branch's effects stay in place
        let r = plan(&with, &Limits::unknown(), &Edit::Remove { slot: 5 }).unwrap();
        assert_eq!(r.chain, vec![1, 2]);
        assert_eq!(writes(&r).len(), 3, "trigger, links, trigger: no type writes");
    }

    #[test]
    fn moves() {
        if !real_tables() {
            return;
        }
        let (od, div, spl, mix) = (id("OVERDRIVE"), id("DIVIDER"), id("SPLITTER"), id("MIXER"));
        // 1 OD, 2 DIV, 3 OD, 4 SPL, 5 OD, 6 MIX, 7 OD
        let data = patch(&[1, 2, 3, 4, 5, 6, 7], &[od, div, od, spl, od, mix, od]);
        let mv = |slot, to| plan(&data, &Limits::unknown(), &Edit::Move { slot, to }).map(|p| p.chain);
        assert_eq!(mv(1, 7).unwrap(), vec![2, 3, 4, 5, 6, 7, 1]);
        assert_eq!(mv(7, 0).unwrap(), vec![7, 1, 2, 3, 4, 5, 6]);
        assert_eq!(mv(1, 3).unwrap(), vec![2, 3, 1, 4, 5, 6, 7], "to the end of branch A");
        assert_eq!(mv(3, 4).unwrap(), vec![1, 2, 4, 3, 5, 6, 7], "from A to the start of B");
        assert!(mv(1, 0).is_err() && mv(1, 1).is_err(), "no change");
        // the whole section moves with its DIVIDER or MIXER
        assert_eq!(mv(2, 0).unwrap(), vec![2, 3, 4, 5, 6, 1, 7]);
        assert_eq!(mv(6, 7).unwrap(), vec![1, 7, 2, 3, 4, 5, 6]);
        assert!(mv(2, 3).is_err(), "into itself");
        assert!(mv(4, 0).is_err(), "SPLITTER");
    }

    #[test]
    fn replace_removes_then_adds_in_the_same_slot() {
        if !real_tables() {
            return;
        }
        let (od, ds) = (id("OVERDRIVE"), id("DISTORTION"));
        let data = patch(&[2, 1], &[od, ds]);
        let p = plan(&data, &Limits::unknown(), &Edit::Replace { slot: 1, fx: id("FUZZ") }).unwrap();
        assert_eq!(p.chain, vec![2, 1]);
        assert_eq!(p.select, Some(1));
        let w = writes(&p);
        assert_eq!(&w[1].data[..3], &[2, 1, 0], "slot 1 left the chain: top = item 1, item 0 free, 1 -> end");
        assert_eq!(p.steps[2], Step::Sync);
        assert_eq!((w[2].addr, &w[2].data[..]), (fx_slot_base(temp_patch(), 1).unwrap(), &[id("FUZZ")][..]));
        assert_eq!(&w[3].data[..3], &[2, 0, 1], "back in: 1 -> 0 -> end");
        // the pedal's overwrite table decides by position
        let no = Limits { insert: None, overwrite: Some(vec![0; 3 * tables::FX_TYPES.len()]) };
        assert!(plan(&data, &no, &Edit::Replace { slot: 1, fx: id("FUZZ") }).is_err());
    }

    #[test]
    fn editor_rules() {
        if !real_tables() {
            return;
        }
        let (od, amp, lp) = (id("OVERDRIVE"), id("AIRD PREAMP"), id("PHRASE LOOP"));
        let ins = |data: &PatchData, fx| plan(data, &Limits::unknown(), &Edit::Insert { at: 0, fx });
        // 9 of a kind
        let nine = patch(&[1, 2, 3, 4, 5, 6, 7, 8, 9], &[od; 9]);
        assert!(ins(&nine, od).is_err());
        assert!(ins(&nine, id("FUZZ")).is_ok());
        // 2 preamps; replacing one is fine
        let amps = patch(&[1, 2], &[amp, id("AIRD BASS PREAMP")]);
        assert!(ins(&amps, amp).is_err());
        assert!(plan(&amps, &Limits::unknown(), &Edit::Replace { slot: 1, fx: amp }).is_ok());
        // 15 effects; the looper doesn't count
        let types: Vec<u8> = (0..15).map(|i| if i % 2 == 0 { od } else { id("FUZZ") }).collect();
        let full = patch(&(1..=15).collect::<Vec<u8>>(), &types);
        assert!(ins(&full, id("CHORUS")).is_err());
        assert!(ins(&full, lp).is_ok());
        // one looper
        let looped = patch(&[1], &[lp]);
        assert!(ins(&looped, lp).is_err());
        // what the pedal says
        let mut sel = vec![1; tables::FX_TYPES.len()];
        sel[od as usize] = 0;
        let l = Limits { insert: Some(sel), overwrite: None };
        assert!(plan(&looped, &l, &Edit::Insert { at: 0, fx: od }).is_err());
        assert!(plan(&looped, &l, &Edit::Insert { at: 0, fx: id("FUZZ") }).is_ok());
    }

    #[test]
    fn edit_on_the_simulator() {
        if !real_tables() {
            return;
        }
        let od = id("OVERDRIVE");
        let mut d = Device::new(SimulatedGx100::new()).without_pacing().with_timeout(Duration::from_millis(50));
        let data = patch(&[1], &[od]);
        d.load_into_temp(&data).unwrap();
        let limits = d.read_chain_limits().unwrap();
        assert_eq!(limits.insert.as_ref().map(Vec::len), Some(tables::FX_TYPES.len()));
        assert_eq!(limits.overwrite.as_ref().map(Vec::len), Some(3 * tables::FX_TYPES.len()));
        let p = plan(&data, &limits, &Edit::Insert { at: 0, fx: id("CHORUS") }).unwrap();
        d.edit_chain(&p).unwrap();
        let back = d.read_patch(temp_patch()).unwrap();
        assert_eq!(back.chain(), vec![2, 1]);
        assert_eq!(back.fx_slot(2).unwrap().type_id, id("CHORUS"));
        let r = plan(&back, &limits, &Edit::Replace { slot: 1, fx: id("FUZZ") }).unwrap();
        d.edit_chain(&r).unwrap();
        let back = d.read_patch(temp_patch()).unwrap();
        assert_eq!(back.chain(), vec![2, 1]);
        assert_eq!(back.fx_slot(1).unwrap().type_id, id("FUZZ"));
    }
}
