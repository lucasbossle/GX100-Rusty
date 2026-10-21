//! Editing: value widgets, writing parameters of the live patch and the system settings, the
//! CTL/EXP, IN/OUT and System menus, and the dialogs that guard writes (WRITE, switching patches with
//! unsaved changes, saving an input setting, writing a setlist).
//!
//! Every change is sent to the pedal right away, as the official editor does. Changes to the live
//! patch are kept by the pedal until another patch is selected; WRITE stores them. System settings
//! take effect immediately.

use super::{Conn, CtlExpPage, EditorApp, AMBER, RED};
use crate::worker::{Cmd, PatchId};
use eframe::egui::{self, RichText};
use gx100_protocol::patch::{self, PatchData, USER_PATCHES};
use gx100_protocol::session::ParamValue;
use gx100_protocol::tables::{self, FxParamDef, ParamDef, ParamDoc};

/// A dialog that needs an answer before something is written.
pub(super) enum Dialog {
    /// WRITE: store the live patch.
    Save { target: u16, name: String },
    /// Switching patches would drop unsaved changes.
    ConfirmSwitch { to: u16 },
    /// Store the current input settings into the selected input-setting slot.
    SaveInputSetting { name: String },
    /// Write the active setlist into the pedal.
    ConfirmSetlistWrite,
}

/// One row of the CTL/EXP menu: the label and the parameter-name stem (`Num1` → `Num1Function`…).
const CONTROLS: &[(&str, &str)] = &[
    ("NUM1", "Num1"),
    ("NUM2", "Num2"),
    ("NUM3", "Num3"),
    ("NUM4", "Num4"),
    ("CURNUM", "CNum"),
    ("BANK ▼", "BankDown"),
    ("BANK ▲", "BankUp"),
    ("CTL1", "Ctl1"),
    ("CTL2", "Ctl2"),
    ("CTL3", "Ctl3"),
    ("CTL4", "Ctl4"),
    ("EXP1 SW", "Exp1Sw"),
    ("EXP1 PEDAL", "Exp1"),
    ("EXP2", "Exp2"),
    ("MANUAL 1", "ManualNum1"),
    ("MANUAL 2", "ManualNum2"),
    ("MANUAL 3", "ManualNum3"),
    ("MANUAL 4", "ManualNum4"),
];

/// IN/OUT menu, as in the official editor's INPUT / OUTPUT SETTINGS page.
const INPUT_OUTPUT: &[&str] = &[
    "Setup_efct.IOInputLevel",
    "Setup_efct.IOInstType",
    "System_inout.OutputSelect",
    "System_inout.TotalMainOutputLevel",
];

const GLOBAL_EQ: &[&str] = &[
    "System_control.GlobalEqSw",
    "Setup_efct.IOOutGlobalEqLowCut",
    "Setup_efct.IOOutGlobalEqLowGain",
    "Setup_efct.IOOutGlobalEqLowMidFreq",
    "Setup_efct.IOOutGlobalEqLowMidQ",
    "Setup_efct.IOOutGlobalEqLowMidGain",
    "Setup_efct.IOOutGlobalEqHighMidFreq",
    "Setup_efct.IOOutGlobalEqHighMidQ",
    "Setup_efct.IOOutGlobalEqHighMidGain",
    "Setup_efct.IOOutGlobalEqHighGain",
    "Setup_efct.IOOutGlobalEqHighCut",
    "Setup_efct.IOOutGlobalEqLevel",
];

/// System menu sections, as in the official editor's menu pages.
const SYSTEM: &[(&str, &[&str])] = &[
    (
        "Hardware",
        &[
            "System_common.AmpCtl1",
            "System_common.AmpCtl2",
            "System_common.Exp1Hold",
            "System_common.Exp2Hold",
            "System_common.AutoOff",
        ],
    ),
    (
        "Play options",
        &[
            "System_common.BankChangeMode",
            "System_common.BankExtentMin",
            "System_common.BankExtentMax",
            "System_efct.LooperMode",
            "System_efct.LooperRecAction",
            "System_common.WarningDelete",
            "System_common.WarningOverwrite",
        ],
    ),
    (
        "MIDI",
        &[
            "System_midi.RxChannel",
            "System_midi.TxChannel",
            "System_midi.MidiInThru",
            "System_midi.UsbInThru",
            "System_midi.SyncClock",
            "System_midi.ClockOut",
            "System_midi.MapSelect",
        ],
    ),
    (
        "MIDI control numbers",
        &[
            "System_midi.Num1Cc",
            "System_midi.Num2Cc",
            "System_midi.Num3Cc",
            "System_midi.Num4Cc",
            "System_midi.BankdownCc",
            "System_midi.BankupCc",
            "System_midi.Ctl1Cc",
            "System_midi.Ctl2Cc",
            "System_midi.Ctl3Cc",
            "System_midi.Ctl4Cc",
            "System_midi.Exp1Cc",
            "System_midi.Exp1SwCc",
            "System_midi.Exp2Cc",
        ],
    ),
    (
        "USB audio",
        &[
            "System_inout.UsbAudioMainEfxOutLevel",
            "System_inout.UsbAudioMainMixLevel",
            "System_inout.UsbAudioDryOutLevel",
            "System_inout.UsbAudioDryToEfxLevel",
            "Setup_efct.UsbAudioDirMonitorSw",
            "System_inout.UsbAudioLoopBack",
        ],
    ),
];

// ------------------------------------------------------------------ widgets

/// A picker (list values), checkbox (two values without labels) or slider for `value`.
/// Returns the new value when the user changed it. `doc`: the guide's text, shown when hovering an option.
#[allow(clippy::too_many_arguments)]
pub(super) fn value_widget(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: i32,
    (min, max): (i32, i32),
    options: &[(i32, &str)],
    display: &dyn Fn(i32) -> String,
    factor: i32,
    doc: Option<&ParamDoc>,
) -> Option<i32> {
    if !options.is_empty() {
        let mut picked = None;
        let current = display(value);
        let r = egui::ComboBox::from_id_salt(id).selected_text(&current).width(180.0).show_ui(ui, |ui| {
            for &(v, label) in options {
                let item = ui.selectable_label(v == value, label);
                let item = match doc.and_then(|d| d.option(label)) {
                    Some(t) => item.on_hover_text(t),
                    None => item,
                };
                if item.clicked() && v != value {
                    picked = Some(v);
                }
            }
        });
        if let Some(t) = doc.and_then(|d| d.option(&current)) {
            r.response.on_hover_text(t);
        }
        return picked;
    }
    if max - min == 1 {
        let mut on = value == max;
        return ui.checkbox(&mut on, "").changed().then_some(if on { max } else { min });
    }
    let mut v = value;
    ui.spacing_mut().slider_width = 220.0;
    let factor = factor.max(1) as f64;
    let slider =
        egui::Slider::new(&mut v, min..=max).custom_formatter(|x, _| display(x as i32)).custom_parser(move |s| {
            s.trim()
                .trim_end_matches(|c: char| c.is_alphabetic())
                .trim()
                .parse::<f64>()
                .ok()
                .map(|x| (x * factor).round())
        });
    ui.add(slider).changed().then_some(v)
}

pub(super) fn fx_options(p: &FxParamDef) -> Vec<(i32, &'static str)> {
    let spans = p.values.len() as i64 == p.max as i64 - p.min as i64 + 1;
    p.values.iter().enumerate().map(|(i, l)| (if spans { p.min + i as i32 } else { i as i32 }, *l)).collect()
}

fn param_options(p: &ParamDef) -> Vec<(i32, &'static str)> {
    p.labels().map(|v| v.options().collect()).unwrap_or_default()
}

/// A widget for an FX parameter. Returns the new value when changed.
pub(super) fn fx_param_widget(
    ui: &mut egui::Ui,
    slot: u8,
    p: &FxParamDef,
    value: i32,
    doc: Option<&ParamDoc>,
) -> Option<i32> {
    let opts = fx_options(p);
    value_widget(ui, ("fx", slot, p.unique_name), value, (p.min, p.max), &opts, &|v| p.display(v), p.factor, doc)
}

/// A widget for a table parameter. Returns the new value when changed.
pub(super) fn param_widget(ui: &mut egui::Ui, p: &ParamDef, value: i32) -> Option<i32> {
    let opts = param_options(p);
    let doc = tables::param_doc(p.path);
    value_widget(ui, ("param", p.path), value, (p.min, p.max), &opts, &|v| p.display(v), p.factor, doc)
}

/// A setting's name, with the guide's explanation of it on hover.
pub(super) fn doc_label(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    doc: Option<&ParamDoc>,
) -> egui::Response {
    let r = ui.label(text);
    match doc.filter(|d| !d.text.is_empty()) {
        Some(d) => r.on_hover_text(d.text),
        None => r,
    }
}

/// The guide's explanation of the chosen option (e.g. which real unit an amp type models), as a
/// small note under the setting. Nothing for options it doesn't explain.
pub(super) fn option_note(ui: &mut egui::Ui, doc: Option<&ParamDoc>, label: &str) {
    if let Some(t) = doc.and_then(|d| d.option(label)) {
        ui.add(egui::Label::new(RichText::new(t).weak()).wrap());
    }
}

/// Whether two versions of a patch differ in anything stored. The expression-pedal position of
/// pedal effects (it follows your foot) and the LED block (it mirrors the effect switches) don't count.
pub(super) fn patch_differs(live: &PatchData, stored: &PatchData) -> bool {
    let mut ignore: Vec<(usize, usize)> = Vec::new();
    for slot in 1..=patch::FX_SLOTS {
        let Some(base) = super::slot_offset(slot) else { continue };
        if let Some(p) = live.fx_slot(slot).and_then(|s| s.fx_type()).and_then(|t| t.param("pedal-position")) {
            let at = (base + p.offset.linear()) as usize;
            ignore.push((at, at + p.encoding.size() as usize));
        }
    }
    patch::patch_blocks().iter().filter(|b| b.name != "led").any(|b| {
        (b.offset as usize..(b.offset + b.size) as usize)
            .any(|i| !ignore.iter().any(|&(s, e)| (s..e).contains(&i)) && live.bytes().get(i) != stored.bytes().get(i))
    })
}

impl EditorApp {
    // ------------------------------------------------------------------ writes

    pub(super) fn connected(&self) -> bool {
        matches!(self.conn, Conn::Connected(_))
    }

    /// Changes bytes of the live patch: on the pedal and in our copy.
    fn set_temp_bytes(&mut self, offset: u32, data: Vec<u8>) {
        if !self.connected() {
            return;
        }
        if let Some(p) = self.patches.get_mut(&PatchId::Temp) {
            p.apply(offset, &data);
        }
        let _ = self.tx.send(Cmd::WriteBytes { addr: patch::temp_patch().offset(offset), data });
    }

    pub(super) fn set_temp_param(&mut self, p: &ParamDef, value: i32) {
        let (Some(offset), Ok(data)) = (patch::temp_offset(p.addr), p.spec().encode(value)) else { return };
        self.set_temp_bytes(offset, data);
    }

    pub(super) fn set_fx_param(&mut self, slot: u8, p: &FxParamDef, value: i32) {
        let (Some(base), Ok(data)) = (super::slot_offset(slot), p.spec().encode(value)) else { return };
        self.set_temp_bytes(base + p.offset.linear(), data);
    }

    pub(super) fn set_fx_switch(&mut self, slot: u8, on: bool) {
        if let Some(base) = super::slot_offset(slot) {
            self.set_temp_bytes(base + 1, vec![on as u8]);
        }
    }

    pub(super) fn temp_value(&self, path: &str) -> Option<i32> {
        match self.patches.get(&PatchId::Temp)?.value(tables::param(path)?)? {
            ParamValue::Num(n) => Some(n),
            ParamValue::Text(_) => None,
        }
    }

    /// The bytes of `len` system-setting bytes at `addr`, from the copy read at connect.
    fn system_bytes(&self, addr: gx100_protocol::Address, len: u32) -> Option<&[u8]> {
        self.system.iter().find_map(|a| {
            let off = addr.since(a.addr)? as usize;
            a.data.get(off..off + len as usize)
        })
    }

    pub(super) fn sys_value(&self, path: &str) -> Option<i32> {
        let p = tables::param(path)?;
        p.spec().decode(self.system_bytes(p.addr, p.encoding.size())?).ok()
    }

    /// Applies system bytes the pedal sent (or we wrote) to our copy.
    pub(super) fn apply_system(&mut self, addr: gx100_protocol::Address, data: &[u8]) {
        for a in &mut self.system {
            let Some(off) = addr.since(a.addr).map(|o| o as usize) else { continue };
            if off < a.data.len() {
                let end = (off + data.len()).min(a.data.len());
                a.data[off..end].copy_from_slice(&data[..end - off]);
                return;
            }
        }
    }

    pub(super) fn set_sys_param(&mut self, p: &ParamDef, value: i32) {
        let Ok(data) = p.spec().encode(value) else { return };
        if !self.connected() {
            return;
        }
        self.apply_system(p.addr, &data);
        let _ = self.tx.send(Cmd::WriteBytes { addr: p.addr, data });
    }

    /// The name of input setting `n` (1-based).
    fn input_setting_name(&self, n: u16) -> String {
        let Some(p) = tables::param(&format!("System_inputSetting({n})._Name(1)")) else { return String::new() };
        self.system_bytes(p.addr, 16)
            .map(|b| b.iter().map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { ' ' }).collect::<String>())
            .unwrap_or_default()
            .trim_end()
            .to_string()
    }

    // ------------------------------------------------------------------ live patch

    /// Whether the live patch has changes that aren't stored. `None` while unknown.
    pub(super) fn unsaved_changes(&self) -> Option<bool> {
        let n = self.current?;
        Some(patch_differs(self.patches.get(&PatchId::Temp)?, self.patches.get(&PatchId::User(n))?))
    }

    /// Switches the pedal to user patch `n`, asking first if that would drop unsaved changes.
    pub(super) fn switch_to(&mut self, n: u16) {
        if !self.connected() || self.current == Some(n) {
            return;
        }
        if self.unsaved_changes() == Some(true) {
            self.dialog = Some(Dialog::ConfirmSwitch { to: n });
        } else {
            self.do_switch(n);
        }
    }

    fn do_switch(&mut self, n: u16) {
        let _ = self.tx.send(Cmd::SelectPatch(n));
        if !self.loading.contains(&PatchId::Temp) {
            self.loading.push(PatchId::Temp);
        }
    }

    /// Drops unsaved changes: loads the stored copy of the current patch into the live patch.
    pub(super) fn revert(&mut self) {
        let Some(stored) = self.current.and_then(|n| self.patches.get(&PatchId::User(n))).cloned() else { return };
        let _ = self.tx.send(Cmd::LoadIntoTemp(Box::new(stored)));
        if !self.loading.contains(&PatchId::Temp) {
            self.loading.push(PatchId::Temp);
        }
    }

    pub(super) fn open_save_dialog(&mut self) {
        let name = self.patches.get(&PatchId::Temp).map(|p| p.name()).unwrap_or_default();
        self.dialog = Some(Dialog::Save { target: self.current.unwrap_or(1), name });
    }

    /// The live patch's settings row (level, BPM, key…), editable.
    pub(super) fn patch_settings_editor(&mut self, ui: &mut egui::Ui, settings: &[(&str, &str)]) {
        ui.horizontal_wrapped(|ui| {
            for (path, label) in settings {
                let Some(p) = tables::param(path) else { continue };
                let Some(v) = self.temp_value(path) else { continue };
                let flash = patch::temp_offset(p.addr).is_some_and(|o| self.flashing(o, p.encoding.size()));
                let color = if flash { AMBER } else { ui.visuals().weak_text_color() };
                doc_label(ui, RichText::new(format!("{label}:")).color(color), tables::param_doc(path));
                let new = match *label {
                    "Key" => {
                        let keys: Vec<(i32, &str)> =
                            super::KEYS.iter().enumerate().map(|(i, k)| (i as i32, *k)).collect();
                        value_widget(
                            ui,
                            ("set", *path),
                            v,
                            (p.min, p.max),
                            &keys,
                            &|x| super::KEYS.get(x as usize).unwrap_or(&"?").to_string(),
                            1,
                            None,
                        )
                    }
                    "BPM" => value_widget(
                        ui,
                        ("set", *path),
                        v,
                        (p.min, p.max),
                        &[],
                        &|x| format!("{:.1}", x as f32 / 10.0),
                        10,
                        None,
                    ),
                    _ => param_widget(ui, p, v),
                };
                if let Some(n) = new {
                    self.set_temp_param(p, n);
                }
                ui.add_space(10.0);
            }
        });
    }

    // ------------------------------------------------------------------ menus

    /// A labelled row for a system parameter.
    fn sys_row(&mut self, ui: &mut egui::Ui, path: &str) {
        let Some(p) = tables::param(path) else { return };
        doc_label(ui, p.desc, tables::param_doc(path));
        match self.sys_value(path) {
            Some(v) => {
                if let Some(n) = param_widget(ui, p, v) {
                    self.set_sys_param(p, n);
                }
            }
            None => {
                ui.label("—");
            }
        }
        ui.end_row();
    }

    pub(super) fn need_connection(&self, ui: &mut egui::Ui) -> bool {
        if self.system.is_empty() {
            ui.label(RichText::new(if self.connected() { "Reading settings…" } else { "Connect the pedal." }).weak());
            return false;
        }
        true
    }

    /// Which function each footswitch, CTL and expression pedal has. Each one uses either the patch's
    /// own setting (MEMORY) or the system-wide one (SYSTEM), as its PREFERENCE says.
    pub(super) fn ctl_exp_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("CTL / EXP");
            ui.separator();
            ui.selectable_value(&mut self.ctl_exp_page, CtlExpPage::Functions, "Control functions")
                .on_hover_text("What each footswitch, CTL and expression pedal does");
            ui.selectable_value(&mut self.ctl_exp_page, CtlExpPage::Assigns, "Assigns")
                .on_hover_text("Let any controller move any parameter of this patch");
        });
        if self.ctl_exp_page == CtlExpPage::Assigns {
            self.assigns_view(ui);
            return;
        }
        ui.label(
            "Each control uses either the patch's own function (MEMORY) or the system-wide one (SYSTEM). \
             The one in use is highlighted. Patch functions are part of the patch: WRITE it to keep them; \
             system functions apply right away.",
        );
        ui.add_space(6.0);
        if !self.need_connection(ui) {
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("ctl-exp").num_columns(6).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                for h in ["", "uses", "patch: function", "mode", "system: function", "mode"] {
                    ui.label(RichText::new(h).weak());
                }
                ui.end_row();
                for &(label, stem) in CONTROLS {
                    ui.label(RichText::new(label).strong());
                    let pref_path = format!("System_control.{stem}Preference");
                    let uses_system = match (tables::param(&pref_path), self.sys_value(&pref_path)) {
                        (Some(p), Some(v)) => {
                            if let Some(n) = param_widget(ui, p, v) {
                                self.set_sys_param(p, n);
                            }
                            v == 1
                        }
                        _ => {
                            ui.label("—");
                            false
                        }
                    };
                    for (side_system, part) in
                        [(false, "Function"), (false, "Mode"), (true, "Function"), (true, "Mode")]
                    {
                        let path = if side_system {
                            format!("System_control.{stem}{part}")
                        } else {
                            format!("Temp_patch.common.{stem}{part}")
                        };
                        let Some(p) = tables::param(&path) else {
                            ui.label("");
                            continue;
                        };
                        let value = if side_system { self.sys_value(&path) } else { self.temp_value(&path) };
                        let Some(v) = value else {
                            ui.label("—");
                            continue;
                        };
                        let active = side_system == uses_system;
                        ui.scope(|ui| {
                            if !active {
                                ui.multiply_opacity(0.45);
                            }
                            if let Some(n) = param_widget(ui, p, v) {
                                if side_system {
                                    self.set_sys_param(p, n);
                                } else {
                                    self.set_temp_param(p, n);
                                }
                            }
                        });
                    }
                    ui.end_row();
                }
            });
        });
    }

    /// Input and output settings, input-setting presets and the global EQ.
    pub(super) fn in_out_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("IN / OUT");
        if !self.need_connection(ui) {
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("in-out").num_columns(2).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
                // The input setting presets: choosing one loads it into the current input settings.
                if let (Some(p), Some(v)) =
                    (tables::param("System_control.InputSettingNum"), self.sys_value("System_control.InputSettingNum"))
                {
                    doc_label(ui, p.desc, tables::param_doc(p.path));
                    ui.horizontal(|ui| {
                        let names: Vec<String> = (p.min..=p.max)
                            .map(|i| format!("[{}] {}", i + 1, self.input_setting_name(i as u16 + 1)))
                            .collect();
                        let opts: Vec<(i32, &str)> = (p.min..=p.max).zip(names.iter().map(String::as_str)).collect();
                        let shown = names.get((v - p.min) as usize).cloned().unwrap_or_default();
                        if let Some(n) =
                            value_widget(ui, "input-setting", v, (p.min, p.max), &opts, &|_| shown.clone(), 1, None)
                        {
                            self.set_sys_param(p, n);
                            let _ = self.tx.send(Cmd::ReadSystem); // the pedal loads that setting
                        }
                        if ui.button("Save current settings here…").clicked() {
                            let name = self.input_setting_name(v as u16 + 1);
                            self.dialog = Some(Dialog::SaveInputSetting { name });
                        }
                    });
                    ui.end_row();
                }
                for path in INPUT_OUTPUT {
                    self.sys_row(ui, path);
                }
            });
            ui.add_space(10.0);
            ui.label(RichText::new("Global EQ").strong());
            egui::Grid::new("global-eq").num_columns(2).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
                for path in GLOBAL_EQ {
                    self.sys_row(ui, path);
                }
            });
        });
    }

    pub(super) fn system_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("System");
        ui.label("System settings apply to the whole pedal and take effect right away.");
        if !self.need_connection(ui) {
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (title, paths) in SYSTEM {
                egui::CollapsingHeader::new(RichText::new(*title).strong()).default_open(true).show(ui, |ui| {
                    egui::Grid::new(("system", *title)).num_columns(2).striped(true).spacing([24.0, 6.0]).show(
                        ui,
                        |ui| {
                            for path in *paths {
                                self.sys_row(ui, path);
                            }
                        },
                    );
                });
            }
        });
    }

    // ------------------------------------------------------------------ dialogs

    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let window = |title: &str| {
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        };
        match &mut dialog {
            Dialog::Save { target, name } => {
                window("Write patch").show(ctx, |ui| {
                    ui.label("Store the live patch, with its current settings, in a user patch.");
                    egui::Grid::new("save").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(name).char_limit(16).desired_width(180.0));
                        ui.end_row();
                        ui.label("Into patch");
                        ui.add(egui::DragValue::new(target).range(1..=USER_PATCHES));
                        ui.end_row();
                    });
                    let there = self.names.get(*target as usize - 1).cloned().flatten().unwrap_or_default();
                    if Some(*target) == self.current {
                        ui.label(RichText::new(format!("Updates {target:03} {there}, the patch you're on.")).weak());
                    } else {
                        ui.colored_label(AMBER, format!("Replaces {target:03} {there}."));
                    }
                    let valid = name.chars().all(|c| (' '..='~').contains(&c));
                    if !valid {
                        ui.colored_label(RED, "Names can only use plain letters, digits and symbols.");
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(valid, egui::Button::new("Write")).clicked() {
                            let _ = self.tx.send(Cmd::SavePatch { target: *target, name: name.clone() });
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
            }
            Dialog::ConfirmSwitch { to } => {
                let to = *to;
                window("Unsaved changes").show(ctx, |ui| {
                    ui.label("The live patch has changes that aren't written. Switching patches drops them.");
                    ui.horizontal(|ui| {
                        if ui.button("Write first…").clicked() {
                            keep = false;
                            self.open_save_dialog();
                        }
                        if ui.button(format!("Switch to {to:03} anyway")).clicked() {
                            keep = false;
                            self.do_switch(to);
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
            }
            Dialog::SaveInputSetting { name } => {
                let slot = self.sys_value("System_control.InputSettingNum").map(|v| v + 1).unwrap_or(1);
                window("Save input setting").show(ctx, |ui| {
                    ui.label(format!("Store the current input settings in input setting [{slot}]."));
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(name).char_limit(16).desired_width(180.0));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            let _ = self.tx.send(Cmd::SaveInputSetting { name: name.clone() });
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
            }
            Dialog::ConfirmSetlistWrite => {
                window("Write setlist to the pedal").show(ctx, |ui| {
                    keep = self.setlist_write_confirmation(ui);
                });
            }
        }
        if keep {
            self.dialog = Some(dialog);
        }
    }

    /// Shown when the active setlist is about to be written. Returns false once answered.
    fn setlist_write_confirmation(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(plan) = self.setlist_write_plan() else {
            ui.label("Nothing to write.");
            return !ui.button("Close").clicked();
        };
        ui.label(format!("{} patch(es) will be written and each read back to check it.", plan.len()));
        let targets: Vec<u16> = plan.iter().map(|(t, _)| *t).collect();
        let sources =
            self.active_setlist().map(|s| s.entries.iter().map(|e| e.source).collect::<Vec<_>>()).unwrap_or_default();
        let lost: Vec<String> = targets
            .iter()
            .filter(|t| !sources.contains(t))
            .map(|&t| format!("{t:03} {}", self.names.get(t as usize - 1).cloned().flatten().unwrap_or_default()))
            .collect();
        if !lost.is_empty() {
            ui.colored_label(RED, format!("These will be overwritten and aren't in the setlist: {}", lost.join(", ")));
        }
        if let Some(c) = self.current.filter(|c| targets.contains(c)) {
            ui.colored_label(
                AMBER,
                format!("The patch you're on ({c:03}) is rewritten; the pedal reloads it afterwards."),
            );
        }
        let newest = self.backups.first().map(|b| crate::store::fmt_time(b.created));
        match newest {
            Some(t) => ui.label(RichText::new(format!("Latest backup: {t}.")).weak()),
            None => ui.colored_label(AMBER, "There's no backup yet. Make one first (Backups tab)."),
        };
        let mut open = true;
        ui.horizontal(|ui| {
            if ui.button(format!("Write {} patch(es)", plan.len())).clicked() {
                self.setlist_rewrites_current =
                    self.current.and_then(|c| plan.iter().find(|(t, _)| *t == c)).map(|(t, d)| (*t, d.clone()));
                let _ = self.tx.send(Cmd::WriteSetlist(plan));
                self.setlist_write = Some((0, targets.len()));
                open = false;
            }
            if ui.button("Cancel").clicked() {
                open = false;
            }
        });
        open
    }

    /// (target, data) for every slot the active setlist would change.
    pub(super) fn setlist_write_plan(&self) -> Option<Vec<(u16, PatchData)>> {
        let s = self.active_setlist()?;
        let sources: Vec<u16> = s.entries.iter().map(|e| e.source).collect();
        let moves = gx100_protocol::librarian::plan_moves(&sources, s.target_start).ok()?;
        let mut plan = Vec::new();
        for (m, e) in moves.iter().zip(&s.entries) {
            let data = e.data.as_ref()?;
            let unchanged = m.source == m.target && self.patches.get(&PatchId::User(m.target)) == Some(data);
            if !unchanged {
                plan.push((m.target, data.clone()));
            }
        }
        (!plan.is_empty()).then_some(plan)
    }

    pub(super) fn setlist_write_finished(&mut self, written: usize, total: usize) {
        self.setlist_write = None;
        if written == total {
            self.notice = Some(format!("Setlist written: {written} patch(es), each read back and verified."));
        } else {
            self.last_error = Some(format!("Setlist write stopped after {written} of {total} patches."));
        }
        // The live patch still holds the old version of a rewritten current patch: load the new one.
        if let Some((_, data)) = self.setlist_rewrites_current.take().filter(|_| written == total) {
            let _ = self.tx.send(Cmd::LoadIntoTemp(Box::new(data)));
        }
    }
}
