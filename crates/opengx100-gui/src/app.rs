//! The editor window: patch list on the left; in the middle, the selected patch's chain and
//! parameters, the setlists ([`setlists`]) or the backups ([`backups`]).

mod assigns;
mod backups;
mod chain_edit;
mod editing;
mod setlists;

use crate::store::{Backup, BackupInfo, Setlist, Store};
use crate::worker::{self, Cmd, Event, PatchId};
use editing::Dialog;
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke};
use opengx100_protocol::backup::AreaData;
use opengx100_protocol::chain::Limits;
use opengx100_protocol::patch::{self, ChainLayout, PatchData, FX_SLOTS, USER_PATCHES};
use opengx100_protocol::tables::{self, FxField, FxParamDef, FxTypeDef};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Patch-level settings shown above the chain, as (parameter path, label).
const PATCH_SETTINGS: &[(&str, &str)] = &[
    ("Temp_patch.efct.MasterPatchLevel", "Level"),
    ("Temp_patch.efct.MasterBpm", "BPM"),
    ("Temp_patch.efct.MasterKey", "Key"),
    ("Temp_patch.efct.MasterInputSens", "Input sens"),
    ("Temp_patch.efct.MasterTempoHold", "Tempo hold"),
    ("Temp_patch.efct.MasterCarryOver", "Carryover"),
];

const KEYS: &[&str] = &["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// FX types that route the signal rather than process it.
const ROUTING_TYPES: &[&str] = &["DIVIDER", "SPLITTER", "MIXER"];

/// How long a value changed on the pedal stays highlighted.
const FLASH: Duration = Duration::from_millis(800);

const AMBER: Color32 = Color32::from_rgb(240, 190, 80);
const GREEN: Color32 = Color32::from_rgb(90, 200, 120);
const RED: Color32 = Color32::from_rgb(230, 90, 80);
/// Wires the signal flows through.
const SIGNAL: Color32 = Color32::from_rgb(110, 175, 235);

// Chain diagram geometry.
const BLOCK_W: f32 = 112.0;
const ROUTING_W: f32 = 76.0;
const BLOCK_H: f32 = 46.0;
const GAP: f32 = 30.0;
const ROW_GAP: f32 = 16.0;
const TERMINAL_W: f32 = 34.0;

/// Scrolling by scroll bar and wheel, but not by dragging content (dragging moves rows instead).
const SCROLL_NOT_DRAG: egui::scroll_area::ScrollSource = egui::scroll_area::ScrollSource {
    drag: egui::scroll_area::DragScroll::Never,
    ..egui::scroll_area::ScrollSource::ALL
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    /// One patch: chain and parameters.
    Patch,
    /// Saved lists of patches in a new order, and what writing them into a range would do.
    Setlists,
    /// Full backups of the pedal.
    Backups,
    /// What the footswitches, CTLs and expression pedals do.
    CtlExp,
    /// Input/output settings and global EQ.
    InOut,
    /// System settings.
    System,
}

/// The two pages of the CTL/EXP tab, as in the official editor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CtlExpPage {
    /// What each footswitch, CTL and expression pedal does.
    Functions,
    /// The live patch's 20 assigns.
    Assigns,
}

enum Conn {
    Connecting,
    Connected(String),
    Failed(String),
}

pub struct EditorApp {
    tx: Sender<Cmd>,
    rx: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
    conn: Conn,
    /// The pedal pushes changes (editor mode is on).
    live: bool,
    current: Option<u16>,
    names: Vec<Option<String>>,
    patches: HashMap<PatchId, PatchData>,
    /// Patches requested but not yet received.
    loading: Vec<PatchId>,
    /// Live-patch byte ranges the pedal changed recently, for highlighting.
    recent: Vec<(u32, u32, Instant)>,
    /// The system settings, as read at connect and kept current.
    system: Vec<AreaData>,
    selected_slot: Option<u8>,
    show_unused: bool,
    /// What the pedal says still fits into the live chain (read again with every live-patch read).
    chain_limits: Option<Limits>,
    /// A chain edit is on its way to the pedal.
    chain_busy: bool,
    /// The chain block being dragged (its slot).
    chain_drag: Option<u8>,
    /// The FX type list, open to add or replace an effect.
    picker: Option<chain_edit::Picker>,
    filter: String,
    last_error: Option<String>,
    /// Good news to show once (e.g. where a backup was saved).
    notice: Option<String>,
    view: View,
    ctl_exp_page: CtlExpPage,
    /// The assign being edited (1-based).
    selected_assign: u8,
    /// Where setlists and backups live; `None` if the data folder can't be used.
    store: Option<Store>,
    setlists: Vec<Setlist>,
    /// Index in `setlists` of the one being edited.
    active: Option<usize>,
    /// When the active setlist was last changed without being saved yet.
    unsaved_since: Option<Instant>,
    confirm_delete_setlist: bool,
    /// Index of the setlist row being dragged.
    dragging: Option<usize>,
    backups: Vec<BackupInfo>,
    /// (done, total) while a backup runs.
    backup_progress: Option<(usize, usize)>,
    open_backup: Option<(PathBuf, Backup)>,
    confirm_delete_backup: Option<PathBuf>,
    dialog: Option<Dialog>,
    /// (done, total) while a setlist is being written.
    setlist_write: Option<(usize, usize)>,
    /// The current patch and its new data, when a setlist write rewrites it.
    setlist_rewrites_current: Option<(u16, PatchData)>,
}

impl EditorApp {
    pub fn new(egui_ctx: &egui::Context, sim: bool) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let ctx = egui_ctx.clone();
        let wake = cmd_tx.clone();
        let worker = std::thread::Builder::new()
            .name("gx100-worker".into())
            .spawn(move || worker::run(cmd_rx, wake, ev_tx, move || ctx.request_repaint()))
            .expect("spawning the MIDI worker thread");
        let mut app = EditorApp {
            tx: cmd_tx,
            rx: ev_rx,
            worker: Some(worker),
            conn: Conn::Connecting,
            live: false,
            current: None,
            names: vec![None; USER_PATCHES as usize],
            patches: HashMap::new(),
            loading: Vec::new(),
            recent: Vec::new(),
            system: Vec::new(),
            selected_slot: None,
            show_unused: false,
            chain_limits: None,
            chain_busy: false,
            chain_drag: None,
            picker: None,
            filter: String::new(),
            last_error: None,
            notice: None,
            view: View::Patch,
            ctl_exp_page: CtlExpPage::Functions,
            selected_assign: 1,
            store: None,
            setlists: Vec::new(),
            active: None,
            unsaved_since: None,
            confirm_delete_setlist: false,
            dragging: None,
            backups: Vec::new(),
            backup_progress: None,
            open_backup: None,
            confirm_delete_backup: None,
            dialog: None,
            setlist_write: None,
            setlist_rewrites_current: None,
        };
        app.open_store();
        app.connect(sim);
        app
    }

    /// Loads the saved setlists (opening the most recently changed one) and the backup list.
    fn open_store(&mut self) {
        match Store::open_default() {
            Ok(store) => {
                let (lists, errors) = store.load_setlists();
                if !errors.is_empty() {
                    self.last_error = Some(format!("couldn't read some setlists: {}", errors.join("; ")));
                }
                self.setlists = lists;
                self.active = (!self.setlists.is_empty()).then_some(0);
                self.backups = store.list_backups();
                self.store = Some(store);
            }
            Err(e) => {
                self.last_error = Some(format!("can't use the data folder, setlists and backups won't be saved: {e}"))
            }
        }
    }

    fn connect(&mut self, sim: bool) {
        self.conn = Conn::Connecting;
        self.live = false;
        self.current = None;
        self.names.iter_mut().for_each(|n| *n = None);
        self.patches.clear();
        self.loading.clear();
        self.recent.clear();
        self.system.clear();
        self.chain_limits = None;
        self.chain_busy = false;
        self.chain_drag = None;
        self.picker = None;
        self.dialog = None;
        self.setlist_write = None;
        self.last_error = None;
        let _ = self.tx.send(Cmd::Connect { sim });
    }

    /// Reads a patch unless it's cached or on its way. Stored patches don't change behind our back;
    /// the live one is kept current by the pedal, but is re-read anyway, in case live updates are off.
    fn request(&mut self, id: PatchId) {
        if (id == PatchId::Temp || !self.patches.contains_key(&id)) && !self.loading.contains(&id) {
            self.loading.push(id);
            let _ = self.tx.send(Cmd::LoadPatch(id));
        }
    }

    fn reload(&mut self) {
        self.patches.clear();
        self.names.iter_mut().for_each(|n| *n = None);
        let _ = self.tx.send(Cmd::LoadNames);
        let _ = self.tx.send(Cmd::ReadSystem);
        self.request(PatchId::Temp);
        if let Some(n) = self.current {
            self.request(PatchId::User(n));
        }
        self.request_setlist_sources();
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Connected { desc, current, live } => {
                    self.conn = Conn::Connected(desc);
                    self.current = current;
                    self.live = live;
                    self.backup_progress = None;
                    self.request(PatchId::Temp);
                    if let Some(n) = current {
                        self.request(PatchId::User(n)); // the stored copy, to spot unsaved changes
                    }
                    self.request_setlist_sources();
                }
                Event::Name(n, name) => {
                    if let Some(slot) = self.names.get_mut(n as usize - 1) {
                        *slot = Some(name);
                    }
                }
                Event::Patch(id, data) => {
                    self.loading.retain(|&l| l != id);
                    if let PatchId::User(n) = id {
                        self.names[n as usize - 1] = Some(data.name());
                    }
                    if id == PatchId::Temp && self.patches.get(&id).is_some_and(|old| old.chain() != data.chain()) {
                        self.selected_slot = None;
                    }
                    if let PatchId::User(n) = id {
                        self.fill_setlist_entries(n, &data);
                    }
                    if id == PatchId::Temp && !self.chain_busy {
                        let _ = self.tx.send(Cmd::ReadChainLimits);
                    }
                    self.patches.insert(id, data);
                }
                Event::TempChanged { offset, data } => {
                    if let Some(p) = self.patches.get_mut(&PatchId::Temp) {
                        p.apply(offset, &data);
                    }
                    self.recent.push((offset, offset + data.len() as u32, Instant::now()));
                }
                Event::CurrentPatch(n) => {
                    self.current = Some(n);
                    self.selected_slot = None;
                    self.request(PatchId::User(n));
                    // The worker re-reads the live patch now; show that it's coming.
                    if !self.loading.contains(&PatchId::Temp) {
                        self.loading.push(PatchId::Temp);
                    }
                }
                Event::BackupProgress { done, total } => self.backup_progress = Some((done, total)),
                Event::BackupCancelled => self.backup_progress = None,
                Event::BackupDone(b) => {
                    self.backup_progress = None;
                    self.backup_finished(*b);
                }
                Event::System(areas) => self.system = areas,
                Event::SystemChanged { addr, data } => self.apply_system(addr, &data),
                Event::Saved(n) => {
                    if self.current == Some(n) || self.view == View::Patch {
                        self.notice = Some(format!("Patch {n:03} written."));
                    }
                }
                Event::ChainLimits(l) => self.chain_limits = Some(l),
                Event::ChainEdited { select } => {
                    self.chain_busy = false;
                    if select.is_some() {
                        self.selected_slot = select;
                    }
                }
                Event::SetlistProgress { done, total } => self.setlist_write = Some((done, total)),
                Event::SetlistDone { written, total } => self.setlist_write_finished(written, total),
                Event::Error(e) => {
                    self.loading.clear();
                    if matches!(self.conn, Conn::Connecting) {
                        self.conn = Conn::Failed(e);
                    } else {
                        self.last_error = Some(e);
                    }
                }
            }
        }
        self.recent.retain(|&(_, _, t)| t.elapsed() < FLASH);
    }

    /// Whether the pedal changed any of `len` bytes at `offset` of the live patch just now.
    fn flashing(&self, offset: u32, len: u32) -> bool {
        self.recent.iter().any(|&(s, e, _)| s < offset + len && offset < e)
    }

    // ------------------------------------------------------------------ panels

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("OpenGX100 Editor");
            ui.separator();
            ui.selectable_value(&mut self.view, View::Patch, "Patch").on_hover_text("The patch active on the pedal");
            ui.selectable_value(&mut self.view, View::CtlExp, "CTL/EXP");
            ui.selectable_value(&mut self.view, View::InOut, "IN/OUT");
            ui.selectable_value(&mut self.view, View::System, "System");
            ui.selectable_value(&mut self.view, View::Setlists, "Setlists")
                .on_hover_text("Saved lists of patches in a new order, to write into a range of the pedal");
            let backups = if self.backup_progress.is_some() { "Backups ⟳" } else { "Backups" };
            ui.selectable_value(&mut self.view, View::Backups, backups)
                .on_hover_text("Back up everything on the pedal");
            ui.separator();
            match &self.conn {
                Conn::Connecting => {
                    ui.spinner();
                    ui.label("Connecting…");
                }
                Conn::Connected(desc) => {
                    ui.colored_label(GREEN, "●");
                    ui.label(desc);
                    if self.live {
                        ui.label(RichText::new("LIVE").color(GREEN).strong()).on_hover_text(
                            "The pedal sends every change as it happens (footswitches, expression pedal, knobs, \
                             patch changes)",
                        );
                    }
                }
                Conn::Failed(e) => {
                    ui.colored_label(RED, "●");
                    ui.label(e).on_hover_text(e);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Use simulator").clicked() {
                    self.connect(true);
                }
                if ui.button("Connect pedal").clicked() {
                    self.connect(false);
                }
                let connected = matches!(self.conn, Conn::Connected(_));
                if ui.add_enabled(connected, egui::Button::new("Reload")).on_hover_text("Re-read everything").clicked()
                {
                    self.reload();
                }
            });
        });
        if let Some(e) = self.last_error.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(RED, format!("Error: {e}"));
                if ui.small_button("dismiss").clicked() {
                    self.last_error = None;
                }
            });
        }
        if let Some(n) = self.notice.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(GREEN, n);
                if ui.small_button("dismiss").clicked() {
                    self.notice = None;
                }
            });
        }
    }

    fn patch_list(&mut self, ui: &mut egui::Ui) {
        let connected = matches!(self.conn, Conn::Connected(_));
        ui.add_enabled_ui(connected, |ui| {
            if self.view != View::Setlists {
                ui.label(RichText::new("click a patch to switch the pedal to it").weak());
            }
            ui.horizontal(|ui| {
                ui.label("Filter");
                ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(f32::INFINITY));
            });
            let loaded = self.names.iter().filter(|n| n.is_some()).count();
            if connected && loaded < self.names.len() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Reading names… {loaded}/{}", self.names.len()));
                });
            }
            ui.separator();

            let filter = self.filter.to_lowercase();
            if self.view == View::Setlists {
                let n = self.active_setlist().map(|s| s.entries.len()).unwrap_or(0);
                ui.label(RichText::new(format!("tick patches to add them to the setlist ({n} in it)")).weak());
            }
            let rows: Vec<u16> = (1..=USER_PATCHES)
                .filter(|&n| {
                    filter.is_empty()
                        || self.names[n as usize - 1].as_deref().is_some_and(|s| s.to_lowercase().contains(&filter))
                        || format!("{n:03}").contains(&filter)
                })
                .collect();
            let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, rows.len(), |ui, range| {
                for &n in &rows[range] {
                    let name = self.names[n as usize - 1].as_deref().unwrap_or("…");
                    let mut text = RichText::new(format!("{n:03}  {name}")).monospace();
                    if self.current == Some(n) {
                        text = text.color(AMBER);
                    }
                    if self.view == View::Setlists {
                        let mut on = self.in_setlist(n);
                        if ui.checkbox(&mut on, text).changed() {
                            self.toggle_pick(n);
                        }
                        continue;
                    }
                    let r = ui.selectable_label(self.current == Some(n), text);
                    let r = if self.current == Some(n) { r.on_hover_text("The pedal is on this patch") } else { r };
                    if r.clicked() {
                        self.switch_to(n);
                    }
                }
            });
        });
    }

    /// The live editor: always the patch active on the pedal, as it is now (including changes made on
    /// the pedal). Edits go to the pedal right away; WRITE stores them.
    fn patch_view(&mut self, ui: &mut egui::Ui) {
        let id = PatchId::Temp;
        let Some(data) = self.patches.get(&id).cloned() else {
            ui.heading("Live patch");
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the active patch…");
            });
            return;
        };

        ui.horizontal(|ui| {
            let number = self.current.map(|n| format!("{n:03}  ")).unwrap_or_default();
            ui.heading(RichText::new(format!("{number}{}", data.name())).strong());
            ui.label(RichText::new("active on the pedal").weak());
            if self.loading.contains(&id) {
                ui.spinner();
            }
            let unsaved = self.unsaved_changes();
            match unsaved {
                Some(true) => {
                    ui.label(RichText::new("● edited").color(AMBER).strong())
                        .on_hover_text("The live patch differs from the stored one. WRITE stores the changes.");
                }
                Some(false) => {
                    ui.label(RichText::new("saved").color(GREEN));
                }
                None => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let connected = matches!(self.conn, Conn::Connected(_));
                if ui
                    .add_enabled(connected, egui::Button::new("Write…"))
                    .on_hover_text("Store the live patch")
                    .clicked()
                {
                    self.open_save_dialog();
                }
                if ui
                    .add_enabled(connected && unsaved == Some(true), egui::Button::new("Revert"))
                    .on_hover_text("Drop the changes: reload the stored patch")
                    .clicked()
                {
                    self.revert();
                }
            });
        });
        self.patch_settings_editor(ui, PATCH_SETTINGS);
        ui.separator();

        ui.horizontal(|ui| {
            ui.label(RichText::new("Signal chain").strong());
            ui.checkbox(&mut self.show_unused, "show unused slots");
            if self.chain_busy {
                ui.spinner();
                ui.label("changing the chain…");
            } else if self.connected() {
                ui.label(RichText::new("drag an effect to move it · click between two to add one").weak());
            }
        });
        let layout = data.chain_layout();
        let chain = data.chain();
        if self.selected_slot.is_none() {
            self.selected_slot = chain.iter().copied().find(|&s| !is_routing(&data, s));
        }
        egui::ScrollArea::horizontal()
            .id_salt("chain")
            .scroll_source(SCROLL_NOT_DRAG)
            .show(ui, |ui| self.chain_diagram(ui, &data, &layout));
        if self.show_unused {
            let unused: Vec<u8> = (1..=FX_SLOTS).filter(|s| !chain.contains(s)).collect();
            if !unused.is_empty() {
                ui.label(RichText::new("Unused slots (not in the chain; the pedal keeps old settings here)").weak());
                ui.horizontal_wrapped(|ui| {
                    for s in unused {
                        let (rect, _) = ui.allocate_exact_size(vec2(BLOCK_W, BLOCK_H), Sense::hover());
                        if self.fx_block(ui, &data, s, rect, false).clicked() {
                            self.selected_slot = Some(s);
                        }
                    }
                });
            }
        }
        ui.separator();

        if let Some(slot) = self.selected_slot {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.fx_params(ui, &data, slot));
        }
    }

    /// The chain as boxes and wires. A parallel section is drawn as two rows that leave the
    /// DIVIDER and meet again at the MIXER; the SPLITTER (only a marker) isn't drawn.
    /// The path the signal takes is drawn bright; a branch the DIVIDER doesn't feed is dashed and dimmed.
    /// Every gap between two blocks is a place an effect can be added or moved to.
    fn chain_diagram(&mut self, ui: &mut egui::Ui, data: &PatchData, layout: &ChainLayout) {
        let chain = data.chain();
        let pos_of = |s: u8| chain.iter().position(|&c| c == s).unwrap_or(chain.len());
        // (centre, position in the chain an effect put there would get)
        let mut gaps: Vec<(Pos2, usize)> = Vec::new();
        let width_of = |s: u8| if is_routing(data, s) { ROUTING_W } else { BLOCK_W };
        let row_w = |slots: &[u8]| slots.iter().map(|&s| width_of(s) + GAP).sum::<f32>();
        let par_w = layout.branch.as_ref().map(|b| row_w(&b.a).max(row_w(&b.b)).max(GAP)).unwrap_or(0.0);
        let branch_w =
            layout.branch.as_ref().map(|b| width_of(b.divider) + GAP + par_w + width_of(b.mixer) + GAP).unwrap_or(0.0);
        let total_w = TERMINAL_W + GAP + row_w(&layout.pre) + branch_w + row_w(&layout.post) + TERMINAL_W;
        let total_h = if layout.branch.is_some() { 2.0 * BLOCK_H + ROW_GAP } else { BLOCK_H } + 8.0;

        let (canvas, _) = ui.allocate_exact_size(vec2(total_w, total_h), Sense::hover());
        let painter = ui.painter_at(canvas.expand(2.0));
        let live = Wire { stroke: Stroke::new(2.0, SIGNAL), dashed: false };
        let dead = Wire { stroke: Stroke::new(1.0, Color32::from_gray(85)), dashed: true };
        let mid_y = canvas.center().y;
        let (row_a, row_b) = (mid_y - (BLOCK_H + ROW_GAP) / 2.0, mid_y + (BLOCK_H + ROW_GAP) / 2.0);

        let terminal = |x: f32, text: &str| {
            let r = Rect::from_center_size(pos2(x + TERMINAL_W / 2.0, mid_y), vec2(TERMINAL_W, 20.0));
            painter.rect_stroke(r, 10.0, live.stroke, egui::StrokeKind::Inside);
            painter.text(r.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::monospace(10.0), SIGNAL);
            r
        };
        let mut x = canvas.left();
        let mut prev = terminal(x, "IN").right_center();
        x += TERMINAL_W + GAP;

        // A run of blocks on one row, wired in sequence starting from `from`. Returns the last output.
        let run = |this: &mut Self,
                   ui: &mut egui::Ui,
                   gaps: &mut Vec<(Pos2, usize)>,
                   slots: &[u8],
                   x: &mut f32,
                   y: f32,
                   from: Pos2,
                   w: Wire| {
            let mut from = from;
            for &s in slots {
                let rect = Rect::from_min_size(pos2(*x, y - BLOCK_H / 2.0), vec2(width_of(s), BLOCK_H));
                connect(&painter, w, from, rect.left_center());
                gaps.push((pos2((from.x + rect.left()) / 2.0, y), pos_of(s)));
                let r = this.fx_block(ui, data, s, rect, w.dashed);
                if r.clicked() {
                    this.selected_slot = Some(s);
                }
                if r.drag_started() && this.can_edit_chain() {
                    this.chain_drag = Some(s);
                    this.selected_slot = Some(s);
                }
                from = rect.right_center();
                *x += width_of(s) + GAP;
            }
            from
        };

        prev = run(self, ui, &mut gaps, &layout.pre, &mut x, mid_y, prev, live);
        if let Some(b) = &layout.branch {
            let route = divider_route(data, b.divider);
            prev = run(self, ui, &mut gaps, &[b.divider], &mut x, mid_y, prev, live);
            let split = prev;
            let par_end = x + par_w - GAP;
            let mut ends = Vec::new();
            // The end of branch A is just before the SPLITTER; the end of B just before the MIXER.
            for (slots, y, active, end) in
                [(&b.a, row_a, route.a, pos_of(b.splitter)), (&b.b, row_b, route.b, pos_of(b.mixer))]
            {
                let w = if active { live } else { dead };
                let mut bx = x;
                let out = run(self, ui, &mut gaps, slots, &mut bx, y, split, w);
                // Run the wire along the row to the end of the parallel section, so both rows meet together.
                let row_end = pos2(par_end.max(out.x), y);
                if slots.is_empty() {
                    connect(&painter, w, split, row_end);
                    gaps.push((pos2((split.x + row_end.x) / 2.0, y), end));
                } else {
                    w.segment(&painter, &[out, row_end]);
                    gaps.push((pos2(out.x + GAP / 2.0, y), end));
                }
                ends.push((row_end, w));
            }
            x += par_w;
            let mixer_rect = Rect::from_min_size(pos2(x, mid_y - BLOCK_H / 2.0), vec2(width_of(b.mixer), BLOCK_H));
            // Inactive wires first, so the live one is drawn on top where they meet.
            ends.sort_by_key(|(_, w)| !w.dashed);
            for (end, w) in ends {
                connect(&painter, w, end, mixer_rect.left_center());
            }
            prev = run(self, ui, &mut gaps, &[b.mixer], &mut x, mid_y, mixer_rect.left_center(), live);
            gaps.pop(); // the MIXER's input is where both rows meet, not a place in the chain
            for (label, y, active) in [("A", row_a, route.a), ("B", row_b, route.b)] {
                let color = if active { SIGNAL } else { dead.stroke.color };
                painter.text(
                    pos2(split.x + 4.0, y - BLOCK_H / 2.0 + 2.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(11.0),
                    color,
                );
            }
        }
        prev = run(self, ui, &mut gaps, &layout.post, &mut x, mid_y, prev, live);
        let out = terminal(x, "OUT");
        connect(&painter, live, prev, out.left_center());
        gaps.push((pos2((prev.x + out.left()) / 2.0, mid_y), chain.len()));
        self.chain_gaps(ui, &painter, data, &gaps);
    }

    /// One clickable box: FX type name, lit when on, flashing when the pedal just changed it,
    /// with a bar for the expression-pedal position on pedal effects. `dimmed`: no signal reaches it.
    fn fx_block(&self, ui: &mut egui::Ui, data: &PatchData, slot: u8, rect: Rect, dimmed: bool) -> egui::Response {
        let resp = ui.interact(rect, egui::Id::unique(("fx-block", slot)), Sense::click_and_drag());
        let Some(s) = data.fx_slot(slot) else { return resp };
        let ty = s.fx_type();
        let name = ty.map(|t| t.name).unwrap_or("UNKNOWN");
        let routing = ROUTING_TYPES.contains(&name);
        let selected = self.selected_slot == Some(slot);
        let flash = slot_offset(slot).is_some_and(|o| self.flashing(o, 2));

        let fill = if routing {
            Color32::from_gray(70)
        } else if s.on {
            Color32::from_rgb(40, 110, 70)
        } else {
            Color32::from_gray(45)
        };
        let fill = if resp.hovered() { fill.gamma_multiply(1.25) } else { fill };
        let fill = if dimmed { fill.gamma_multiply(0.45) } else { fill };
        let stroke = if selected {
            Stroke::new(2.0, AMBER)
        } else if flash {
            Stroke::new(2.0, Color32::WHITE)
        } else {
            Stroke::new(1.0, Color32::from_gray(90))
        };
        let painter = ui.painter();
        painter.rect(rect, 5.0, fill, stroke, egui::StrokeKind::Inside);
        let text_color = if s.on || routing { Color32::WHITE } else { Color32::from_gray(150) };
        let text_color = if dimmed { text_color.gamma_multiply(0.5) } else { text_color };
        // The DIVIDER says where it sends the signal.
        let caption = (name == "DIVIDER").then(|| divider_route(data, slot).label());
        if let Some(c) = &caption {
            painter.text(
                pos2(rect.center().x, rect.bottom() - 9.0),
                egui::Align2::CENTER_CENTER,
                c,
                egui::FontId::proportional(10.0),
                SIGNAL,
            );
        }
        let galley =
            painter.layout(name.to_string(), egui::FontId::proportional(11.5), text_color, rect.width() - 10.0);
        let lift = if caption.is_some() { 7.0 } else { 3.0 };
        let text_pos = rect.center() - galley.size() / 2.0 - vec2(0.0, lift);
        painter.galley(text_pos, galley, text_color);

        if let Some((p, v)) = ty.and_then(|t| pedal_position(data, slot, t)) {
            let frac = ((v - p.min) as f32 / (p.max - p.min).max(1) as f32).clamp(0.0, 1.0);
            let track = Rect::from_min_max(
                pos2(rect.left() + 6.0, rect.bottom() - 8.0),
                pos2(rect.right() - 6.0, rect.bottom() - 4.0),
            );
            painter.rect_filled(track, 2.0, Color32::from_gray(30));
            painter.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * frac, track.height())), 2.0, AMBER);
        }
        let about = ty.and_then(|t| tables::fx_doc(t.name)).and_then(|d| d.text.lines().next()).unwrap_or("");
        let drag = if self.can_edit_chain() && name != "SPLITTER" { " · drag to move" } else { "" };
        let state = if routing {
            ""
        } else if s.on {
            " · ON"
        } else {
            " · OFF"
        };
        resp.on_hover_text(format!("slot {slot}{state}{drag}\n{about}").trim_end().to_string())
    }

    /// Every parameter of the effect in `slot`, editable. Values the pedal just changed are highlighted.
    fn fx_params(&mut self, ui: &mut egui::Ui, data: &PatchData, slot: u8) {
        let Some(s) = data.fx_slot(slot) else { return };
        let Some(t) = s.fx_type() else {
            ui.label(format!("Slot {slot} holds unknown FX type {}", s.type_id));
            return;
        };
        let doc = tables::fx_doc(t.name);
        ui.horizontal(|ui| {
            ui.heading(t.name);
            let (txt, col) = if s.on { ("ON", GREEN) } else { ("OFF", Color32::from_gray(140)) };
            if ui
                .add(egui::Button::new(RichText::new(txt).color(col).strong()))
                .on_hover_text("Turn the effect on/off")
                .clicked()
            {
                self.set_fx_switch(slot, !s.on);
            }
            ui.label(RichText::new(format!("slot {slot}")).weak());
            self.chain_buttons(ui, data, slot);
        });
        // What the effect does (and what it models), from the parameter guide.
        if let Some(d) = doc.filter(|d| !d.text.is_empty()) {
            ui.add(egui::Label::new(RichText::new(d.text).weak()).wrap());
        }
        ui.add_space(4.0);
        let base = slot_offset(slot);
        egui::Grid::new(("fx-params", slot)).num_columns(2).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
            for p in t.params.iter().filter(|p| matches!(p.field, FxField::Param(_))) {
                let flash = base.is_some_and(|b| self.flashing(b + p.offset.linear(), p.encoding.size()));
                let pdoc = doc.and_then(|d| d.param(p.unique_name));
                let color = if flash { AMBER } else { ui.visuals().text_color() };
                editing::doc_label(ui, RichText::new(p.name).color(color), pdoc);
                match data.fx_param(slot, p) {
                    Some(v) => {
                        ui.vertical(|ui| {
                            if let Some(n) = editing::fx_param_widget(ui, slot, p, v, pdoc) {
                                self.set_fx_param(slot, p, n);
                            }
                            if !p.values.is_empty() {
                                editing::option_note(ui, pdoc, &p.display(v));
                            }
                        });
                    }
                    None => {
                        ui.label("—");
                    }
                }
                ui.end_row();
            }
        });
    }
}

impl eframe::App for EditorApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        self.autosave(ctx);
        if !self.recent.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100)); // let highlights fade
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::left("patches").resizable(true).default_size(240.0).show(ui, |ui| {
            ui.add_space(4.0);
            self.patch_list(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| match &self.conn {
            Conn::Failed(e) if self.patches.is_empty() => {
                let e = e.clone();
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.heading("Couldn't reach the pedal");
                    ui.label(e);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Try again").clicked() {
                            self.connect(false);
                        }
                        if ui.button("Use simulator").clicked() {
                            self.connect(true);
                        }
                    });
                });
            }
            _ => match self.view {
                View::Setlists => self.setlists_view(ui),
                View::Backups => self.backups_view(ui),
                View::CtlExp => self.ctl_exp_view(ui),
                View::InOut => self.in_out_view(ui),
                View::System => self.system_view(ui),
                View::Patch => self.patch_view(ui),
            },
        });
        let ctx = ui.ctx().clone();
        self.fx_picker(&ctx);
        self.dialogs(&ctx);
    }
}

impl Drop for EditorApp {
    /// Takes the pedal out of editor mode before quitting, as the official editor does.
    /// Also saves a setlist changed in the last moments.
    fn drop(&mut self) {
        self.save_active_setlist();
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

// ------------------------------------------------------------------ helpers

fn slot_type(data: &PatchData, slot: u8) -> Option<&'static FxTypeDef> {
    data.fx_slot(slot).and_then(|s| s.fx_type())
}

fn is_routing(data: &PatchData, slot: u8) -> bool {
    slot_type(data, slot).is_some_and(|t| ROUTING_TYPES.contains(&t.name))
}

/// Offset of FX slot `slot` from the start of a patch.
fn slot_offset(slot: u8) -> Option<u32> {
    patch::fx_slot_base(patch::temp_patch(), slot).and_then(patch::temp_offset)
}

/// The expression-pedal position of a pedal effect (FOOT VOLUME, WAH, PEDAL BEND…).
fn pedal_position(data: &PatchData, slot: u8, t: &FxTypeDef) -> Option<(&'static FxParamDef, i32)> {
    let p = t.param("pedal-position")?;
    Some((p, data.fx_param(slot, p)?))
}

/// How a wire is drawn: solid where the signal flows, dashed where it doesn't.
#[derive(Clone, Copy)]
struct Wire {
    stroke: Stroke,
    dashed: bool,
}

impl Wire {
    fn segment(self, painter: &egui::Painter, points: &[Pos2]) {
        if self.dashed {
            painter.extend(egui::Shape::dashed_line(points, self.stroke, 4.0, 4.0));
        } else {
            painter.add(egui::Shape::line(points.to_vec(), self.stroke));
        }
    }
}

/// Which branches a DIVIDER feeds: both in DUAL mode, else the one `ch-select` picks.
#[derive(Clone, Copy)]
struct Route {
    a: bool,
    b: bool,
}

impl Route {
    fn label(self) -> &'static str {
        match (self.a, self.b) {
            (true, true) => "A + B",
            (true, false) => "A only",
            (false, true) => "B only",
            (false, false) => "",
        }
    }
}

fn divider_route(data: &PatchData, slot: u8) -> Route {
    let get = |name: &str| slot_type(data, slot).and_then(|t| t.param(name)).and_then(|p| data.fx_param(slot, p));
    match (get("mode"), get("ch-select")) {
        (Some(1), _) | (None, _) => Route { a: true, b: true }, // DUAL (or unreadable: assume both)
        (Some(_), Some(1)) => Route { a: false, b: true },
        (Some(_), _) => Route { a: true, b: false },
    }
}

/// A wire from an output to an input: straight on one row, an S-curve between rows.
fn connect(painter: &egui::Painter, wire: Wire, from: Pos2, to: Pos2) {
    if (from.y - to.y).abs() < 0.5 {
        wire.segment(painter, &[from, to]);
    } else {
        let dx = ((to.x - from.x) / 2.0).max(8.0);
        let curve = egui::epaint::CubicBezierShape::from_points_stroke(
            [from, pos2(from.x + dx, from.y), pos2(to.x - dx, to.y), to],
            false,
            Color32::TRANSPARENT,
            wire.stroke,
        );
        wire.segment(painter, &curve.flatten(Some(0.5)));
    }
    // arrowhead
    if !wire.dashed {
        painter.add(egui::Shape::convex_polygon(
            vec![to, pos2(to.x - 6.0, to.y - 3.5), pos2(to.x - 6.0, to.y + 3.5)],
            wire.stroke.color,
            Stroke::NONE,
        ));
    }
}
