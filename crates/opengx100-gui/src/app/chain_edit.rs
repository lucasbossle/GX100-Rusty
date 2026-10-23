//! Changing the signal chain on the Patch tab, as in the official editor: drag an effect to move it,
//! click a gap between two blocks to add one there, or use the buttons next to the selected effect
//! (◀ ▶ to move it, Replace…, Remove). A DIVIDER brings its SPLITTER and MIXER; dragging the DIVIDER or
//! the MIXER moves the whole parallel section, removing either removes the section's routing.
//!
//! Each change goes to the pedal the way the official editor sends it ([`chain`]); the live patch is
//! read again afterwards, since the pedal resets the slots involved. What's offered is checked with
//! the limits the pedal reported last; the worker reads them again before writing.

use super::editing::Dialog;
use super::{EditorApp, AMBER, BLOCK_H, GAP, SIGNAL};
use crate::worker::{Cmd, PatchId};
use eframe::egui::{self, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke};
use opengx100_protocol::chain::{self, Edit, Kind, Plan};
use opengx100_protocol::patch::PatchData;
use opengx100_protocol::tables;

/// What the FX list is open for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PickerMode {
    /// Add an effect before this position of the chain.
    Insert(usize),
    /// Put another effect in this slot.
    Replace(u8),
}

pub(super) struct Picker {
    mode: PickerMode,
    filter: String,
    pos: Pos2,
    focus: bool,
}

impl PickerMode {
    fn edit(self, fx: u8) -> Edit {
        match self {
            PickerMode::Insert(at) => Edit::Insert { at, fx },
            PickerMode::Replace(slot) => Edit::Replace { slot, fx },
        }
    }
}

fn slot_name(data: &PatchData, slot: u8) -> &'static str {
    data.fx_slot(slot).and_then(|s| s.fx_type()).map(|t| t.name).unwrap_or("?")
}

impl EditorApp {
    pub(super) fn can_edit_chain(&self) -> bool {
        self.connected() && !self.chain_busy && self.patches.contains_key(&PatchId::Temp)
    }

    /// How `edit` would be done, or why it can't, by the limits the pedal reported last.
    fn chain_check(&self, edit: &Edit) -> Result<Plan, String> {
        let data = self.patches.get(&PatchId::Temp).ok_or("the live patch isn't read yet")?;
        chain::plan(data, &self.chain_limits.clone().unwrap_or_default(), edit)
    }

    /// Asks first when the pedal's DELETE / OVERWRITE WARNING is on (as the official editor does).
    fn request_chain_edit(&mut self, edit: Edit) {
        let Some(data) = self.patches.get(&PatchId::Temp) else { return };
        let warn = |path: &str| self.sys_value(path) != Some(0);
        let question = match edit {
            Edit::Remove { slot } if warn("System_common.WarningDelete") => {
                if Kind::of(data.fx_slot(slot).map(|s| s.type_id).unwrap_or(0)).is_routing() {
                    Some((
                        "Remove the parallel section? The DIVIDER, SPLITTER and MIXER go; the effects of \
                         branches A and B stay, one after the other."
                            .to_string(),
                        "Remove",
                    ))
                } else {
                    let name = slot_name(data, slot);
                    Some((format!("Remove {name} (slot {slot}) from the chain? Its settings are lost."), "Remove"))
                }
            }
            Edit::Replace { slot, fx } if warn("System_common.WarningOverwrite") => {
                let new = tables::fx_type(fx).map(|t| t.name).unwrap_or("?");
                let old = slot_name(data, slot);
                Some((format!("Replace {old} (slot {slot}) with {new}? The settings of {old} are lost."), "Replace"))
            }
            _ => None,
        };
        match question {
            Some((text, ok)) => self.dialog = Some(Dialog::ConfirmChainEdit { edit, text, ok }),
            None => self.send_chain_edit(edit),
        }
    }

    pub(super) fn send_chain_edit(&mut self, edit: Edit) {
        let Some(data) = self.patches.get(&PatchId::Temp).cloned() else { return };
        if !self.can_edit_chain() {
            return;
        }
        self.chain_busy = true;
        self.picker = None;
        let _ = self.tx.send(Cmd::EditChain { data: Box::new(data), edit });
    }

    fn open_picker(&mut self, mode: PickerMode, pos: Pos2) {
        self.picker = Some(Picker { mode, filter: String::new(), pos, focus: true });
        let _ = self.tx.send(Cmd::ReadChainLimits);
    }

    /// The gaps of the chain diagram: a "+" to add an effect there; while an effect is dragged, the
    /// places it can go, and the move when it's dropped near one.
    pub(super) fn chain_gaps(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        data: &PatchData,
        gaps: &[(Pos2, usize)],
    ) {
        if let Some(slot) = self.chain_drag {
            let targets: Vec<(Pos2, usize)> =
                gaps.iter().copied().filter(|&(_, to)| self.chain_check(&Edit::Move { slot, to }).is_ok()).collect();
            for &(c, _) in &targets {
                painter.circle_stroke(c, 5.0, Stroke::new(1.5, SIGNAL));
            }
            let pointer = ui.ctx().pointer_latest_pos();
            let target = pointer.and_then(|p| {
                targets
                    .iter()
                    .filter(|(c, _)| c.distance(p) < 60.0)
                    .min_by(|a, b| a.0.distance(p).total_cmp(&b.0.distance(p)))
                    .copied()
            });
            if let Some((c, _)) = target {
                let half = vec2(0.0, BLOCK_H / 2.0);
                painter.line_segment([c - half, c + half], Stroke::new(3.0, AMBER));
            }
            if let Some(p) = pointer {
                // The block follows the pointer, above everything else.
                let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("chain-drag"));
                let fg = ui.ctx().layer_painter(layer);
                let r = Rect::from_center_size(p, vec2(100.0, 30.0));
                fg.rect(
                    r,
                    5.0,
                    Color32::from_rgba_unmultiplied(60, 60, 60, 220),
                    Stroke::new(1.0, AMBER),
                    egui::StrokeKind::Inside,
                );
                let name = slot_name(data, slot);
                fg.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    name,
                    egui::FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if ui.input(|i| !i.pointer.primary_down()) {
                self.chain_drag = None;
                if let Some((_, to)) = target {
                    self.send_chain_edit(Edit::Move { slot, to });
                }
            }
            return;
        }
        if !self.can_edit_chain() {
            return;
        }
        for &(c, at) in gaps {
            let rect = Rect::from_center_size(c, vec2(GAP - 4.0, BLOCK_H));
            let r = ui.interact(rect, egui::Id::unique(("chain-gap", at)), Sense::click());
            if r.hovered() {
                painter.circle(c, 8.0, SIGNAL, Stroke::NONE);
                painter.text(c, egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(14.0), Color32::BLACK);
            }
            if r.on_hover_text("Add an effect here").clicked() {
                self.open_picker(PickerMode::Insert(at), c + vec2(0.0, BLOCK_H));
            }
        }
    }

    /// ◀ ▶ Replace… Remove, next to the selected effect's name.
    pub(super) fn chain_buttons(&mut self, ui: &mut egui::Ui, data: &PatchData, slot: u8) {
        let chain = data.chain();
        let Some(at) = chain.iter().position(|&s| s == slot) else {
            ui.label(RichText::new("not in the chain").weak());
            return;
        };
        let kind = Kind::of(data.fx_slot(slot).map(|s| s.type_id).unwrap_or(0));
        if kind == Kind::Splitter {
            return;
        }
        // A DIVIDER or MIXER moves the whole section: from the DIVIDER to the MIXER.
        let kind_at = |k: Kind| chain.iter().position(|&s| data.fx_slot(s).is_some_and(|f| Kind::of(f.type_id) == k));
        let (first, last) = match (kind.is_routing(), kind_at(Kind::Divider), kind_at(Kind::Mixer)) {
            (true, Some(d), Some(m)) => (d, m),
            _ => (at, at),
        };
        ui.separator();
        ui.add_enabled_ui(self.can_edit_chain(), |ui| {
            let what = if kind.is_routing() { "the parallel section" } else { "the effect" };
            let moves = [("◀", first.checked_sub(1), "earlier"), ("▶", Some(last + 2), "later")];
            for (label, to, dir) in moves {
                let edit = to.map(|to| Edit::Move { slot, to });
                let ok = edit.as_ref().is_some_and(|e| self.chain_check(e).is_ok());
                if ui
                    .add_enabled(ok, egui::Button::new(label))
                    .on_hover_text(format!("Move {what} {dir} in the chain"))
                    .clicked()
                {
                    if let Some(e) = edit {
                        self.send_chain_edit(e);
                    }
                }
            }
            if !kind.is_routing() {
                let r = ui.button("Replace…").on_hover_text("Put another effect in its place");
                if r.clicked() {
                    self.open_picker(PickerMode::Replace(slot), r.rect.left_bottom());
                }
            }
            let (label, tip) = if kind.is_routing() {
                (
                    "Remove branch",
                    "Remove the DIVIDER, SPLITTER and MIXER; the effects of both branches stay, in series",
                )
            } else {
                ("Remove", "Take the effect out of the chain")
            };
            if ui.button(label).on_hover_text(tip).clicked() {
                self.request_chain_edit(Edit::Remove { slot });
            }
        });
    }

    /// The list of FX types to add or to replace an effect with. Types that don't fit are greyed out,
    /// with the reason on hover.
    pub(super) fn fx_picker(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.picker.take() else { return };
        let Some(data) = self.patches.get(&PatchId::Temp).cloned() else { return };
        let limits = self.chain_limits.clone().unwrap_or_default();
        let chain = data.chain();
        let (title, place) = match picker.mode {
            PickerMode::Insert(at) => {
                let place = match (at.checked_sub(1).and_then(|i| chain.get(i)), chain.get(at)) {
                    (None, None) => "into the empty chain".to_string(),
                    (None, Some(&n)) => format!("before {}", slot_name(&data, n)),
                    (Some(&p), None) => format!("after {}", slot_name(&data, p)),
                    (Some(&p), Some(&n)) => format!("between {} and {}", slot_name(&data, p), slot_name(&data, n)),
                };
                ("Add effect", place)
            }
            PickerMode::Replace(slot) => {
                ("Replace effect", format!("in place of {} (slot {slot})", slot_name(&data, slot)))
            }
        };
        let mut open = true;
        let mut chosen = None;
        egui::Window::new(title)
            .id(egui::Id::unique("fx-picker"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(picker.pos)
            .show(ctx, |ui| {
                ui.label(place);
                if self.chain_limits.is_none() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("asking the pedal what fits…").weak());
                    });
                }
                let r = ui.add(egui::TextEdit::singleline(&mut picker.filter).hint_text("filter").desired_width(240.0));
                if picker.focus {
                    r.request_focus();
                    picker.focus = false;
                }
                let filter = picker.filter.to_uppercase();
                let mut types: Vec<_> = chain::insertable_types().filter(|t| t.name.contains(&filter)).collect();
                types.sort_by_key(|t| t.name);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for t in types {
                        let edit = picker.mode.edit(t.id);
                        let check = chain::plan(&data, &limits, &edit);
                        let button = egui::Button::new(t.name).min_size(vec2(240.0, 0.0));
                        let r = ui.add_enabled(check.is_ok(), button);
                        let about = tables::fx_doc(t.name).and_then(|d| d.text.lines().next()).unwrap_or("");
                        let r = match &check {
                            Err(e) => r.on_disabled_hover_text(e),
                            Ok(_) if !about.is_empty() => r.on_hover_text(about),
                            Ok(_) => r,
                        };
                        // Enter takes the first type that fits.
                        if r.clicked() || (enter && check.is_ok() && chosen.is_none()) {
                            chosen = Some(edit);
                        }
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        match chosen {
            Some(edit) => self.request_chain_edit(edit),
            None if open => self.picker = Some(picker),
            None => {}
        }
    }
}
