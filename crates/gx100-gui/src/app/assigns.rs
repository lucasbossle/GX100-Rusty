//! The live patch's 20 assigns (CTL/EXP → Assigns): each lets a controller move one parameter
//! between a minimum and a maximum. Edits go to the pedal as the official editor sends them
//! (see [`gx100_protocol::assign`]); they're part of the patch, kept with WRITE.

use super::editing::{doc_label, fx_options, option_note, param_widget, value_widget};
use super::{EditorApp, AMBER};
use crate::worker::{Cmd, PatchId};
use eframe::egui::{self, RichText};
use gx100_protocol::assign::{self, Applies, Message, Target, ASSIGNS};
use gx100_protocol::patch::{self, PatchData};
use gx100_protocol::session::ParamValue;
use gx100_protocol::tables;

/// The patch's input sensitivity, used when an assign's SOURCE is INPUT.
const INPUT_SENS: &str = "Temp_patch.efct.MasterInputSens";

impl EditorApp {
    /// Changes the live patch with `edit` and sends the messages it returns.
    fn edit_assign(&mut self, edit: impl FnOnce(&mut PatchData) -> Option<Vec<Message>>) {
        if !self.connected() {
            return;
        }
        let Some(msgs) = self.patches.get_mut(&PatchId::Temp).and_then(edit) else { return };
        for (offset, data) in msgs {
            let _ = self.tx.send(Cmd::WriteBytes { addr: patch::temp_patch().offset(offset), data });
        }
    }

    /// Changes an assign's target, then re-reads the assign: the pedal resets TARGET MIN/MAX itself
    /// when TARGET is written (seen on the GX-100; the official editor re-reads or gets them pushed).
    fn edit_assign_target(&mut self, n: u8, edit: impl FnOnce(&mut PatchData) -> Option<Vec<Message>>) {
        self.edit_assign(edit);
        if let Some((offset, size)) = assign::block(n).filter(|_| self.connected()) {
            let _ = self.tx.send(Cmd::ReadTemp { offset, size });
        }
    }

    pub(super) fn assigns_view(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Each assign lets a controller (SOURCE) move one parameter (TARGET) between MIN and MAX. \
             Assigns are part of the patch: WRITE it to keep them.",
        );
        ui.add_space(4.0);
        let Some(data) = self.patches.get(&PatchId::Temp).cloned() else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the active patch…");
            });
            return;
        };
        if assign::field(1, "SW").is_none() {
            ui.label(RichText::new("These parameter tables have no assigns.").weak());
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            self.assign_list(ui, &data);
            ui.separator();
            let n = self.selected_assign.clamp(1, ASSIGNS);
            self.assign_editor(ui, &data, n);
        });
    }

    /// One row per assign: on/off, what it moves, the range, the source. Click a row to edit it.
    fn assign_list(&mut self, ui: &mut egui::Ui, data: &PatchData) {
        egui::Grid::new("assigns").num_columns(7).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
            for h in ["", "on", "target", "min → max", "now", "source", "mode"] {
                ui.label(RichText::new(h).weak());
            }
            ui.end_row();
            for n in 1..=ASSIGNS {
                let on = assign::get(data, n, "SW") == Some(1);
                let selected = self.selected_assign == n;
                if ui.selectable_label(selected, RichText::new(format!("{n:>2}")).monospace()).clicked() {
                    self.selected_assign = n;
                }
                let mut sw = on;
                if ui.checkbox(&mut sw, "").on_hover_text("Turn this assign on/off").changed() {
                    self.edit_assign(|d| assign::set_field(d, n, "SW", sw as i32));
                }
                let target = assign::target(data, n).unwrap_or(Target::None);
                let dim = |t: String| if on { RichText::new(t) } else { RichText::new(t).weak() };
                let mut label = target.label(data);
                if !target.matches_slot() {
                    label.push_str(" ⚠");
                }
                if ui.selectable_label(selected, dim(label)).clicked() {
                    self.selected_assign = n;
                }
                ui.label(dim(range_text(data, n, &target)));
                self.live_cell(ui, data, n, &target);
                ui.label(dim(field_text(data, n, "SOURCE")));
                let mode = if assign::applies(data, n).mode { field_text(data, n, "MODE") } else { "—".into() };
                ui.label(dim(mode));
                ui.end_row();
            }
        });
    }

    /// The target's current value on the pedal, amber while the pedal is changing it (press the
    /// assign's footswitch or move its pedal to see it react), with a bar between MIN and MAX.
    fn live_cell(&self, ui: &mut egui::Ui, data: &PatchData, n: u8, target: &Target) {
        let Some(live) = live_target(data, target) else {
            ui.label(RichText::new("—").weak());
            return;
        };
        let flash = self.flashing(live.offset, live.size);
        ui.horizontal(|ui| {
            let (lo, hi) = match (assign::get(data, n, "TARGET_MIN"), assign::get(data, n, "TARGET_MAX")) {
                (Some(lo), Some(hi)) if lo != hi => (lo, hi),
                _ => (live.min, live.max),
            };
            if live.max - live.min > 1 {
                let frac = ((live.value - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 6.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
                let fill = if flash { AMBER } else { ui.visuals().weak_text_color() };
                let mut bar = rect;
                bar.set_width(rect.width() * frac);
                ui.painter().rect_filled(bar, 2.0, fill);
            }
            let text = RichText::new(&live.text);
            ui.label(if flash { text.color(AMBER).strong() } else { text });
        })
        .response
        .on_hover_text("The target's value on the pedal right now; it lights up when the pedal changes it");
    }

    /// Every setting of assign `n`. Settings that don't apply to its source/target are greyed out,
    /// as in the official editor.
    fn assign_editor(&mut self, ui: &mut egui::Ui, data: &PatchData, n: u8) {
        let target = assign::target(data, n).unwrap_or(Target::None);
        let applies = assign::applies(data, n);
        ui.horizontal(|ui| {
            ui.heading(format!("Assign {n}"));
            let mut sw = assign::get(data, n, "SW") == Some(1);
            if ui.checkbox(&mut sw, "on").changed() {
                self.edit_assign(|d| assign::set_field(d, n, "SW", sw as i32));
            }
        });
        if !target.matches_slot() {
            ui.colored_label(
                AMBER,
                "⚠ The target slot doesn't hold the effect this target belongs to (its FX type was changed?). \
                 Pick the target again.",
            );
        }
        egui::Grid::new(("assign", n)).num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            // ---- target
            section(ui, "Target");
            let choices = assign::category_choices(data);
            let current = assign::current_choice(data, n, &choices);
            ui.label("Category").on_hover_text("An effect of this patch's chain, MASTER, TUNER or MIDI");
            let shown = match (current, &target) {
                (Some(i), _) => choices[i].label.clone(),
                (None, Target::Fx { slot, .. }) => format!("{} (slot {slot}, not in the chain)", target.label(data)),
                (None, _) => "?".into(),
            };
            let mut picked = None;
            egui::ComboBox::from_id_salt(("assign-cat", n)).selected_text(shown).width(220.0).show_ui(ui, |ui| {
                for (i, c) in choices.iter().enumerate() {
                    let text = match c.slot {
                        Some(s) => format!("{}  (slot {s})", c.label),
                        None => c.label.clone(),
                    };
                    let item = ui.selectable_label(current == Some(i), text);
                    let about = assign::category_fx(c.category).and_then(|t| tables::fx_doc(t.name));
                    let item = match about.filter(|d| !d.text.is_empty()) {
                        Some(d) => item.on_hover_text(d.text),
                        None => item,
                    };
                    if item.clicked() && current != Some(i) {
                        picked = Some(c.clone());
                    }
                }
            });
            if let Some(c) = picked {
                self.edit_assign_target(n, |d| assign::set_category(d, n, &c));
            }
            ui.end_row();

            ui.label("Parameter");
            let value = assign::get(data, n, "TARGET").unwrap_or(0);
            let list: Vec<i32> = current.map(|i| assign::targets_for(choices[i].category)).unwrap_or_default();
            let name = |t: i32| assign::target_def(t).map(|d| d.name).unwrap_or("?");
            let mut picked = None;
            egui::ComboBox::from_id_salt(("assign-target", n)).selected_text(name(value)).width(220.0).show_ui(
                ui,
                |ui| {
                    let fx = current.and_then(|i| assign::category_fx(choices[i].category));
                    for &t in &list {
                        let item = ui.selectable_label(t == value, name(t));
                        let about = fx
                            .and_then(|f| tables::fx_doc(f.name))
                            .zip(assign::target_def(t))
                            .and_then(|(d, def)| d.param(def.unique_name))
                            .filter(|d| !d.text.is_empty());
                        let item = match about {
                            Some(d) => item.on_hover_text(d.text),
                            None => item,
                        };
                        if item.clicked() && t != value {
                            picked = Some(t);
                        }
                    }
                },
            );
            if let Some(t) = picked {
                self.edit_assign_target(n, |d| assign::set_target(d, n, t));
            }
            ui.end_row();

            for (label, field) in [("Min", "TARGET_MIN"), ("Max", "TARGET_MAX")] {
                doc_label(ui, label, assign::field(n, field).and_then(|p| tables::param_doc(p.path)));
                ui.add_enabled_ui(applies.target_range, |ui| {
                    if let Some(v) = self.target_value_widget(ui, data, n, &target, field) {
                        self.edit_assign(|d| assign::set_field(d, n, field, v));
                    }
                });
                ui.end_row();
            }
            if live_target(data, &target).is_some() {
                ui.label("Now");
                self.live_cell(ui, data, n, &target);
                ui.end_row();
            }

            // ---- source
            section(ui, "Source");
            self.assign_row(ui, data, n, "SOURCE", true);
            self.assign_row(ui, data, n, "MODE", applies.mode);
            self.assign_row(ui, data, n, "ACT_RANGE_LO", true);
            self.assign_row(ui, data, n, "ACT_RANGE_HI", true);
            if let (Some(p), Some(v)) = (tables::param(INPUT_SENS), self.temp_value(INPUT_SENS)) {
                ui.label("INPUT SENS").on_hover_text("The patch's input sensitivity, for SOURCE = INPUT");
                ui.add_enabled_ui(applies.input_sens, |ui| {
                    if let Some(v) = param_widget(ui, p, v) {
                        // The official editor writes the setting, then the whole assign.
                        self.set_temp_param(p, v);
                        self.edit_assign(|d| assign::block_message(d, n).map(|m| vec![m]));
                    }
                });
                ui.end_row();
            }

            section(ui, "Internal pedal");
            self.assign_row(ui, data, n, "INT_PDL_TRIGGER", applies.int_pedal);
            self.assign_row(ui, data, n, "INT_PDL_TIME", applies.int_pedal);
            self.assign_row(ui, data, n, "INT_PDL_CURVE", applies.int_pedal);
            section(ui, "Wave pedal");
            self.assign_row(ui, data, n, "WAVE_RATE", applies.wave_pedal);
            self.assign_row(ui, data, n, "WAVE_FORM", applies.wave_pedal);

            section(ui, "MIDI");
            let Applies { midi_ch, midi_cc, midi_pc, bank_lsb, .. } = applies;
            self.assign_row(ui, data, n, "MIDI_CH", midi_ch);
            self.assign_row(ui, data, n, "MIDI_CC_NO", midi_cc);
            self.assign_row(ui, data, n, "MIDI_CC_VALUE_MIN", midi_cc);
            self.assign_row(ui, data, n, "MIDI_CC_VALUE_MAX", midi_cc);
            self.assign_row(ui, data, n, "MIDI_PC_NO", midi_pc);
            self.assign_row(ui, data, n, "MIDI_BANK_MSB", midi_pc);
            self.assign_row(ui, data, n, "MIDI_BANK_LSB", bank_lsb);
        });
    }

    /// A labelled row for one field of assign `n`.
    fn assign_row(&mut self, ui: &mut egui::Ui, data: &PatchData, n: u8, field: &str, enabled: bool) {
        let (Some(p), Some(v)) = (assign::field(n, field), assign::get(data, n, field)) else { return };
        doc_label(ui, p.desc, tables::param_doc(p.path));
        ui.add_enabled_ui(enabled, |ui| {
            if let Some(v) = param_widget(ui, p, v) {
                self.edit_assign(|d| assign::set_field(d, n, field, v));
            }
        });
        ui.end_row();
    }

    /// TARGET_MIN / TARGET_MAX, shown in the units and labels of the parameter the target moves.
    fn target_value_widget(
        &self,
        ui: &mut egui::Ui,
        data: &PatchData,
        n: u8,
        target: &Target,
        field: &str,
    ) -> Option<i32> {
        let value = assign::get(data, n, field)?;
        let id = ("assign-range", n, field.to_string());
        if *target == Target::Tuner {
            let opts = [(0, "OFF"), (1, "ON")];
            return value_widget(ui, id, value, (0, 1), &opts, &|v| if v == 0 { "OFF" } else { "ON" }.into(), 1, None);
        }
        let Some(p) = target.param() else {
            ui.label(RichText::new(if target.has_range() { "—" } else { "set by the MIDI settings below" }).weak());
            return None;
        };
        let opts = fx_options(p);
        let shown = value.clamp(p.min, p.max);
        let doc = target_doc(target);
        let new = value_widget(ui, id, shown, (p.min, p.max), &opts, &|v| p.display(v), p.factor, doc);
        option_note(ui, doc, &p.display(shown));
        if shown != value {
            ui.colored_label(AMBER, format!("stored {value} is outside {}–{}", p.display(p.min), p.display(p.max)));
        }
        new
    }
}

/// The guide's text about the parameter a target moves.
fn target_doc(target: &Target) -> Option<&'static tables::ParamDoc> {
    let (fx, def) = match target {
        Target::Fx { fx: Some(t), def, .. } => (t.name, *def),
        Target::Master(def) => ("MASTER", *def),
        _ => return None,
    };
    tables::fx_doc(fx)?.param(def.unique_name)
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).strong());
    ui.end_row();
}

/// A field's value as the pedal shows it.
fn field_text(data: &PatchData, n: u8, field: &str) -> String {
    match (assign::field(n, field), assign::get(data, n, field)) {
        (Some(p), Some(v)) => p.display(v),
        _ => "—".into(),
    }
}

/// "0 → 120", in the target parameter's units.
/// Where an assign's target lives in the live patch, and its current value.
struct LiveTarget {
    offset: u32,
    size: u32,
    value: i32,
    min: i32,
    max: i32,
    text: String,
}

/// The current value of an effect or MASTER target. `None` for tuner and MIDI targets, whose state
/// isn't part of the patch.
fn live_target(data: &PatchData, target: &Target) -> Option<LiveTarget> {
    match target {
        Target::Fx { slot, .. } if target.matches_slot() => {
            let p = target.param()?;
            let value = data.fx_param(*slot, p)?;
            let offset = super::slot_offset(*slot)? + p.offset.linear();
            let (size, min, max, text) = (p.encoding.size(), p.min, p.max, p.display(value));
            Some(LiveTarget { offset, size, value, min, max, text })
        }
        Target::Master(_) => {
            // MASTER targets are the patch's efct.Master* settings, matched by name.
            let squash = |s: &str| s.replace([' ', '.'], "").to_uppercase();
            let name = squash(target.param()?.name);
            let p = tables::PARAMS
                .iter()
                .find(|p| p.path.starts_with("Temp_patch.efct.Master") && squash(p.desc) == name)?;
            let ParamValue::Num(value) = data.value(p)? else { return None };
            let offset = patch::temp_offset(p.addr)?;
            Some(LiveTarget { offset, size: p.encoding.size(), value, min: p.min, max: p.max, text: p.display(value) })
        }
        _ => None,
    }
}

fn range_text(data: &PatchData, n: u8, target: &Target) -> String {
    let get = |f| assign::get(data, n, f);
    match target {
        Target::None => "—".into(),
        Target::MidiCc => {
            format!("{} → {}", field_text(data, n, "MIDI_CC_VALUE_MIN"), field_text(data, n, "MIDI_CC_VALUE_MAX"))
        }
        Target::MidiPc => format!("PC# {}", field_text(data, n, "MIDI_PC_NO")),
        Target::Tuner => {
            let s = |v: Option<i32>| if v == Some(0) { "OFF" } else { "ON" };
            format!("{} → {}", s(get("TARGET_MIN")), s(get("TARGET_MAX")))
        }
        _ => match (target.param(), get("TARGET_MIN"), get("TARGET_MAX")) {
            (Some(p), Some(lo), Some(hi)) => format!("{} → {}", p.display(lo), p.display(hi)),
            (None, Some(lo), Some(hi)) => format!("{lo} → {hi}"),
            _ => "—".into(),
        },
    }
}
