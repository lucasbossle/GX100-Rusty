//! Setlists: saved, ordered lists of patches, and what writing one into a range of the pedal would do.
//!
//! Each entry keeps a copy of its patch from when it was added, so a setlist can be recovered at any
//! time, even after the patches on the pedal have changed (the list flags entries where they have).
//! Every change is saved automatically. Setlists can be exported to a file and imported back.
//! Writing one to the pedal asks for confirmation, then writes each patch and reads it back.

use super::editing::Dialog;
use super::{Conn, EditorApp, AMBER, GREEN, RED, SCROLL_NOT_DRAG};
use crate::store::{fmt_time, now, Entry, Setlist, Store};
use crate::worker::{Cmd, PatchId};
use eframe::egui::{self, vec2, RichText, Sense, Stroke};
use gx100_protocol::librarian::{self, Move};
use gx100_protocol::patch::{self, PatchData, USER_PATCHES};
use std::time::{Duration, Instant};

/// How long after the last change a setlist is saved.
const SAVE_DELAY: Duration = Duration::from_millis(500);

/// How an entry's saved copy compares with that patch on the pedal now.
enum Freshness {
    /// Not read from the pedal yet.
    Reading,
    Same,
    /// The pedal's patch differs from the saved copy.
    Changed(PatchData),
    /// The pedal's version isn't known (not connected, or still being read).
    Unknown,
}

impl EditorApp {
    // ------------------------------------------------------------------ state

    pub(super) fn active_setlist(&self) -> Option<&Setlist> {
        self.active.and_then(|i| self.setlists.get(i))
    }

    fn active_setlist_mut(&mut self) -> Option<&mut Setlist> {
        self.active.and_then(|i| self.setlists.get_mut(i))
    }

    pub(super) fn in_setlist(&self, n: u16) -> bool {
        self.active_setlist().is_some_and(|s| s.entries.iter().any(|e| e.source == n))
    }

    /// Records that the active setlist changed; it's saved shortly after.
    fn touch(&mut self) {
        if let Some(s) = self.active_setlist_mut() {
            s.modified = now();
            self.unsaved_since = Some(Instant::now());
        }
    }

    pub(super) fn save_active_setlist(&mut self) {
        if self.unsaved_since.take().is_none() {
            return;
        }
        let (Some(store), Some(s)) = (&self.store, self.active.and_then(|i| self.setlists.get(i))) else { return };
        if let Err(e) = store.save_setlist(s) {
            self.last_error = Some(format!("couldn't save setlist {:?}: {e}", s.name));
        }
    }

    /// Saves the active setlist once it has been left alone for a moment.
    pub(super) fn autosave(&mut self, ctx: &egui::Context) {
        match self.unsaved_since {
            Some(t) if t.elapsed() >= SAVE_DELAY => self.save_active_setlist(),
            Some(t) => ctx.request_repaint_after(SAVE_DELAY - t.elapsed()),
            None => {}
        }
    }

    fn new_setlist(&mut self, s: Setlist) {
        self.save_active_setlist();
        self.setlists.insert(0, s);
        self.active = Some(0);
        self.unsaved_since = Some(Instant::now().checked_sub(SAVE_DELAY).unwrap_or_else(Instant::now)); // save now
        self.request_setlist_sources();
    }

    fn switch_setlist(&mut self, i: usize) {
        if self.active == Some(i) {
            return;
        }
        self.save_active_setlist();
        self.active = Some(i);
        self.dragging = None;
        self.confirm_delete_setlist = false;
        self.request_setlist_sources();
    }

    fn delete_active_setlist(&mut self) {
        let Some(i) = self.active else { return };
        let s = self.setlists.remove(i);
        self.unsaved_since = None;
        if let Some(store) = &self.store {
            if let Err(e) = store.delete_setlist(&s.id) {
                self.last_error = Some(format!("couldn't delete setlist {:?}: {e}", s.name));
            }
        }
        self.active = (!self.setlists.is_empty()).then_some(0);
        self.confirm_delete_setlist = false;
    }

    /// Reads the pedal's current version of every patch in the active setlist, to compare with the saved copies.
    pub(super) fn request_setlist_sources(&mut self) {
        if !matches!(self.conn, Conn::Connected(_)) {
            return;
        }
        let sources: Vec<u16> =
            self.active_setlist().map(|s| s.entries.iter().map(|e| e.source).collect()).unwrap_or_default();
        for n in sources {
            self.request(PatchId::User(n));
        }
    }

    /// Ticks or unticks a patch. A ticked patch goes to the end of the active setlist (one is
    /// created if there's none yet), with a copy of its current data.
    pub(super) fn toggle_pick(&mut self, n: u16) {
        if self.active.is_none() {
            let name = format!("Setlist {}", self.setlists.len() + 1);
            self.new_setlist(Setlist::new(name));
        }
        let name = self.names.get(n as usize - 1).cloned().flatten().unwrap_or_default();
        let data = self.patches.get(&PatchId::User(n)).cloned();
        let Some(s) = self.active_setlist_mut() else { return };
        if let Some(i) = s.entries.iter().position(|e| e.source == n) {
            s.entries.remove(i);
        } else {
            if s.entries.is_empty() {
                s.target_start = n;
            }
            s.entries.push(Entry { source: n, name, data });
            self.request(PatchId::User(n));
        }
        self.touch();
    }

    /// A patch arrived from the pedal: entries still waiting for their copy take it.
    pub(super) fn fill_setlist_entries(&mut self, n: u16, data: &PatchData) {
        let Some(s) = self.active_setlist_mut() else { return };
        let mut changed = false;
        for e in s.entries.iter_mut().filter(|e| e.source == n && e.data.is_none()) {
            e.name = data.name();
            e.data = Some(data.clone());
            changed = true;
        }
        if changed {
            self.touch();
        }
    }

    fn freshness(&self, e: &Entry) -> Freshness {
        let Some(saved) = &e.data else { return Freshness::Reading };
        match self.patches.get(&PatchId::User(e.source)) {
            Some(p) if p == saved => Freshness::Same,
            Some(p) => Freshness::Changed(p.clone()),
            None => Freshness::Unknown,
        }
    }

    fn export_setlist(&mut self, s: &Setlist) {
        let file_name: String =
            s.name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export setlist")
            .add_filter("GX-100 setlist", &["json"])
            .set_file_name(format!("{}.json", file_name.trim()))
            .save_file()
        else {
            return;
        };
        match Store::export_setlist(s, &path) {
            Ok(()) => self.notice = Some(format!("Setlist exported to {}", path.display())),
            Err(e) => self.last_error = Some(format!("couldn't export the setlist: {e}")),
        }
    }

    fn import_setlist(&mut self) {
        let Some(path) =
            rfd::FileDialog::new().set_title("Import setlist").add_filter("GX-100 setlist", &["json"]).pick_file()
        else {
            return;
        };
        match Store::import_setlist(&path) {
            Ok(s) => {
                let (name, n) = (s.name.clone(), s.entries.len());
                self.new_setlist(s);
                self.notice = Some(format!("Imported setlist “{name}” ({n} patches)."));
            }
            Err(e) => self.last_error = Some(format!("couldn't import {}: {e}", path.display())),
        }
    }

    fn name_of(&self, n: u16) -> &str {
        self.names.get(n as usize - 1).and_then(|n| n.as_deref()).unwrap_or("…")
    }

    // ------------------------------------------------------------------ view

    /// Choose/create/rename/export/import setlists → order → preview → write.
    pub(super) fn setlists_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Setlists");
        self.setlist_bar(ui);
        ui.separator();
        let Some(s) = self.active_setlist() else {
            ui.add_space(12.0);
            ui.label("A setlist is a list of patches in the order you want them on the pedal.");
            ui.label("Tick patches in the list on the left to start one, or press “New setlist”.");
            ui.label("Setlists are saved automatically and keep a copy of each patch.");
            return;
        };
        if s.entries.is_empty() {
            ui.label("Tick patches in the list on the left to add them.");
            return;
        }
        // Dragging inside must move rows, not scroll.
        egui::ScrollArea::vertical().auto_shrink(false).scroll_source(SCROLL_NOT_DRAG).show(ui, |ui| {
            ui.columns(2, |cols| {
                self.order_list(&mut cols[0]);
                self.rewrite_preview(&mut cols[1]);
            });
            ui.separator();
            self.write_plan(ui);
        });
    }

    /// Pick a setlist, make a new one, duplicate, rename, delete; and whether it's saved.
    fn setlist_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let current = self.active_setlist().map(|s| s.name.clone()).unwrap_or_else(|| "(none)".into());
            let mut pick = None;
            egui::ComboBox::from_id_salt("setlist-picker").selected_text(current).width(200.0).show_ui(ui, |ui| {
                for (i, s) in self.setlists.iter().enumerate() {
                    let label = format!("{}  ·  {} patches  ·  {}", s.name, s.entries.len(), fmt_time(s.modified));
                    if ui.selectable_label(self.active == Some(i), label).clicked() {
                        pick = Some(i);
                    }
                }
            });
            if let Some(i) = pick {
                self.switch_setlist(i);
            }
            if ui.button("New setlist").clicked() {
                let name = format!("Setlist {}", self.setlists.len() + 1);
                self.new_setlist(Setlist::new(name));
            }
            if ui.button("Import…").on_hover_text("Load a setlist file exported earlier").clicked() {
                self.import_setlist();
            }
            let Some(s) = self.active_setlist().cloned() else { return };
            if ui.button("Export…").on_hover_text("Save this setlist (with its patches) to a file").clicked() {
                self.export_setlist(&s);
            }
            if ui.button("Duplicate").clicked() {
                let mut copy = Setlist::new(format!("{} (copy)", s.name));
                copy.entries = s.entries.clone();
                copy.target_start = s.target_start;
                self.new_setlist(copy);
            }
            if self.confirm_delete_setlist {
                ui.label(RichText::new(format!("Delete “{}”?", s.name)).color(RED));
                if ui.button("Yes, delete").clicked() {
                    self.delete_active_setlist();
                }
                if ui.button("Keep").clicked() {
                    self.confirm_delete_setlist = false;
                }
            } else if ui.button("Delete…").clicked() {
                self.confirm_delete_setlist = true;
            }
        });
        if self.active.is_none() {
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Name");
            let mut changed = false;
            if let Some(s) = self.active_setlist_mut() {
                changed = ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(240.0)).changed();
            }
            if changed {
                self.touch();
            }
            let status = match (self.unsaved_since, self.active_setlist()) {
                (Some(_), _) => RichText::new("saving…").weak(),
                (None, Some(s)) => RichText::new(format!("✓ saved {}", fmt_time(s.modified))).color(GREEN),
                _ => RichText::new(""),
            };
            let folder = self.store.as_ref().map(|s| s.root().display().to_string()).unwrap_or_default();
            ui.label(status).on_hover_text(format!("Setlists are saved automatically in {folder}"));
        });
    }

    /// The setlist's patches, in order. Drag a row to move it.
    fn order_list(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Order").strong());
        ui.horizontal(|ui| {
            let (rev, num, name) =
                (ui.small_button("reverse"), ui.small_button("sort by number"), ui.small_button("sort by name"));
            let Some(s) = self.active_setlist_mut() else { return };
            if rev.clicked() {
                s.entries.reverse();
            } else if num.clicked() {
                s.entries.sort_by_key(|e| e.source);
            } else if name.clicked() {
                s.entries.sort_by_key(|e| e.name.to_lowercase());
            } else {
                return;
            }
            self.touch();
        });
        ui.label(RichText::new("drag rows to reorder").weak());
        ui.add_space(2.0);

        let entries = self.active_setlist().map(|s| s.entries.clone()).unwrap_or_default();
        let connected = matches!(self.conn, Conn::Connected(_));
        let row_h = 26.0;
        let mut rects = Vec::new();
        let mut remove = None;
        let mut refresh = None;
        for (i, e) in entries.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::drag());
            rects.push(rect);
            if resp.drag_started() {
                self.dragging = Some(i);
            }
            let fill = if self.dragging == Some(i) {
                ui.visuals().selection.bg_fill
            } else if resp.hovered() {
                ui.visuals().widgets.hovered.bg_fill
            } else {
                ui.visuals().faint_bg_color
            };
            ui.painter().rect_filled(rect.shrink2(vec2(0.0, 1.0)), 4.0, fill);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            let mut row = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(vec2(6.0, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            row.label(RichText::new("⠿").weak());
            row.label(RichText::new(format!("{:>2}.", i + 1)).weak().monospace());
            let name = if e.name.is_empty() { "…" } else { &e.name };
            row.label(RichText::new(format!("{:03}  {name}", e.source)).monospace());
            row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |row| {
                if row.small_button("✕").on_hover_text("remove from the setlist").clicked() {
                    remove = Some(i);
                }
                match self.freshness(e) {
                    Freshness::Reading if connected => {
                        row.spinner().on_hover_text("reading from the pedal…");
                    }
                    Freshness::Reading => {
                        row.label(RichText::new("not read").color(RED))
                            .on_hover_text("This patch hasn't been read yet; connect the pedal to take its copy");
                    }
                    Freshness::Same => {
                        row.label(RichText::new("✓").color(GREEN)).on_hover_text("saved copy matches the pedal");
                    }
                    Freshness::Changed(p) => {
                        if row.small_button("use pedal's").clicked() {
                            refresh = Some((i, p));
                        }
                        row.label(RichText::new("⚠ changed").color(AMBER)).on_hover_text(format!(
                            "Patch {:03} on the pedal has changed since it was added to this setlist. The setlist \
                             keeps the saved copy unless you take the pedal's version.",
                            e.source
                        ));
                    }
                    Freshness::Unknown => {
                        row.label(RichText::new("saved copy").weak())
                            .on_hover_text("Connect the pedal to compare with its current version");
                    }
                }
            });
        }
        if let Some(i) = remove {
            if let Some(s) = self.active_setlist_mut() {
                s.entries.remove(i);
            }
            self.dragging = None;
            self.touch();
        }
        if let Some((i, p)) = refresh {
            if let Some(e) = self.active_setlist_mut().and_then(|s| s.entries.get_mut(i)) {
                e.name = p.name();
                e.data = Some(p);
            }
            self.touch();
        }

        // Where the dragged row would land: before the first row whose middle is below the pointer.
        if let (Some(from), Some(pointer)) = (self.dragging, ui.ctx().pointer_interact_pos()) {
            let to = rects.iter().position(|r| pointer.y < r.center().y).unwrap_or(rects.len());
            if let (Some(first), Some(last)) = (rects.first(), rects.last()) {
                let y = rects.get(to).map(|r| r.top()).unwrap_or(last.bottom());
                ui.painter().hline(first.x_range(), y, Stroke::new(2.0, AMBER));
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if !ui.input(|i| i.pointer.any_down()) {
                let mut moved = false;
                if let Some(s) = self.active_setlist_mut() {
                    if from < s.entries.len() {
                        let e = s.entries.remove(from);
                        let to = if to > from { to - 1 } else { to };
                        moved = to != from;
                        s.entries.insert(to.min(s.entries.len()), e);
                    }
                }
                self.dragging = None;
                if moved {
                    self.touch();
                }
            }
        }
    }

    /// Target range and, slot by slot, what's there now and what would replace it.
    fn rewrite_preview(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.active_setlist().cloned() else { return };
        let sources: Vec<u16> = s.entries.iter().map(|e| e.source).collect();
        ui.label(RichText::new("Write into").strong());
        ui.horizontal(|ui| {
            ui.label("patches starting at");
            let max = (USER_PATCHES as usize + 1).saturating_sub(sources.len()).max(1) as u16;
            let mut start = s.target_start.min(max);
            if ui.add(egui::DragValue::new(&mut start).range(1..=max)).changed() {
                if let Some(s) = self.active_setlist_mut() {
                    s.target_start = start;
                }
                self.touch();
            }
            ui.label(format!("→ {:03}–{:03}", start, start as usize + sources.len() - 1));
        });
        let moves = match librarian::plan_moves(&sources, s.target_start) {
            Ok(m) => m,
            Err(e) => {
                ui.colored_label(RED, e.to_string());
                return;
            }
        };
        ui.add_space(4.0);
        egui::Grid::new("rewrite-preview").num_columns(4).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
            for h in ["slot", "now on the pedal", "would become", ""] {
                ui.label(RichText::new(h).weak());
            }
            ui.end_row();
            for (m, e) in moves.iter().zip(&s.entries) {
                ui.label(RichText::new(format!("{:03}", m.target)).monospace());
                let lost = !sources.contains(&m.target);
                let now = RichText::new(self.name_of(m.target)).monospace();
                ui.label(if lost { now.color(RED).strikethrough() } else { now });
                let unchanged = m.source == m.target && matches!(self.freshness(e), Freshness::Same);
                let becomes = RichText::new(&e.name).monospace();
                ui.label(if unchanged { becomes.weak() } else { becomes.strong() });
                ui.label(
                    RichText::new(if unchanged { "unchanged".into() } else { format!("from {:03}", m.source) }).weak(),
                );
                ui.end_row();
            }
        });

        // Consequences worth spelling out.
        let lost: Vec<String> = moves
            .iter()
            .filter(|m| !sources.contains(&m.target))
            .map(|m| format!("{:03} {}", m.target, self.name_of(m.target)))
            .collect();
        let range = s.target_start..s.target_start + moves.len() as u16;
        let doubled: Vec<String> = s
            .entries
            .iter()
            .filter(|e| !range.contains(&e.source))
            .map(|e| format!("{:03} {}", e.source, e.name))
            .collect();
        ui.add_space(6.0);
        if !lost.is_empty() {
            ui.colored_label(RED, format!("⚠ {} patch(es) not in this setlist would be overwritten:", lost.len()));
            ui.label(RichText::new(lost.join(", ")).color(RED));
            ui.label(RichText::new("Make a backup first (Backups tab) so they can be recovered.").weak());
        }
        if !doubled.is_empty() {
            ui.colored_label(AMBER, "These are outside the range, so they'd stay where they are as well (two copies):");
            ui.label(RichText::new(doubled.join(", ")).color(AMBER));
        }
        if lost.is_empty() && doubled.is_empty() {
            ui.colored_label(GREEN, "Only reorders the setlist's patches; nothing else is touched.");
        }
    }

    /// The exact DT1 messages a real write would send, from the setlist's saved copies.
    fn write_plan(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.active_setlist().cloned() else { return };
        let sources: Vec<u16> = s.entries.iter().map(|e| e.source).collect();
        let Ok(moves) = librarian::plan_moves(&sources, s.target_start) else { return };
        let missing = s.entries.iter().filter(|e| e.data.is_none()).count();
        if missing > 0 {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Waiting for {missing} patch(es) to be read from the pedal…"));
            });
            return;
        }
        let device_id = gx100_protocol::sysex::DEFAULT_DEVICE_ID;
        // Skip patches already in place, unless the pedal's version differs from the saved copy.
        let plans: Vec<(Move, &Entry, Vec<librarian::Dt1>)> = moves
            .iter()
            .zip(&s.entries)
            .filter(|(m, e)| m.source != m.target || !matches!(self.freshness(e), Freshness::Same))
            .filter_map(|(&m, e)| Some((m, e, librarian::write_messages(m.target, e.data.as_ref()?)?)))
            .collect();
        let all: Vec<librarian::Dt1> = plans.iter().flat_map(|(_, _, msgs)| msgs.iter().cloned()).collect();
        let (bytes, secs) = librarian::wire_cost(&all, device_id);

        let connected = matches!(self.conn, Conn::Connected(_));
        let busy = self.setlist_write.is_some() || self.backup_progress.is_some();
        let can_write = connected && !busy && !plans.is_empty();
        let mut start = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Writing to the pedal").strong());
            if let Some((done, total)) = self.setlist_write {
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .desired_width(220.0)
                        .text(format!("writing… {done} of {total}")),
                );
                cancel = ui.button("Stop").on_hover_text("Stop after the patch being written").clicked();
            } else {
                let why = if !connected {
                    "Connect the pedal first"
                } else if busy {
                    "Wait for the backup to finish"
                } else {
                    "Nothing to write"
                };
                start = ui
                    .add_enabled(can_write, egui::Button::new(RichText::new("Write to pedal…").strong()))
                    .on_disabled_hover_text(why)
                    .clicked();
            }
        });
        if start {
            self.dialog = Some(Dialog::ConfirmSetlistWrite);
        }
        if cancel {
            let _ = self.tx.send(Cmd::CancelSetlistWrite);
        }
        if plans.is_empty() {
            ui.label(RichText::new("Nothing to send: every patch is already in place.").weak());
            return;
        }
        ui.label(format!(
            "{} patch(es) · {} DT1 messages · {:.1} KB · about {:.0} s at MIDI speed",
            plans.len(),
            all.len(),
            bytes as f32 / 1024.0,
            secs.ceil()
        ));
        ui.label(
            RichText::new(
                "Writes use the setlist's saved copies, so overlapping ranges are safe. Like the official \
                 librarian, each patch is sent as plain DT1s into its user-patch memory, block by block.",
            )
            .weak(),
        );
        for (m, e, msgs) in &plans {
            let title = format!("{:03} ← {:03} {}  ({} messages)", m.target, m.source, e.name, msgs.len());
            egui::CollapsingHeader::new(RichText::new(title).monospace()).id_salt(("plan", m.target)).show(ui, |ui| {
                let base = patch::user_patch(m.target);
                egui::Grid::new(("plan-grid", m.target)).num_columns(3).striped(true).show(ui, |ui| {
                    for d in msgs {
                        let off = base.and_then(|b| d.addr.since(b)).unwrap_or(0);
                        let block = patch::patch_blocks()
                            .iter()
                            .find(|b| b.offset <= off && off < b.offset + b.size)
                            .map(|b| b.name.as_str())
                            .unwrap_or("?");
                        ui.label(RichText::new(d.addr.to_string()).monospace());
                        ui.label(format!("{block} · {} bytes", d.data.len()));
                        let sx = d.to_sysex(device_id);
                        let hex: Vec<String> = sx.iter().take(24).map(|b| format!("{b:02X}")).collect();
                        let more = if sx.len() > 24 { " …" } else { "" };
                        ui.label(RichText::new(format!("{}{more}", hex.join(" "))).monospace().weak())
                            .on_hover_text(sx.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "));
                        ui.end_row();
                    }
                });
            });
        }
    }
}
