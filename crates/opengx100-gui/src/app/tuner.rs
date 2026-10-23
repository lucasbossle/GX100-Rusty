//! The Tuner tab: shows the pedal's tuner (MONO, POLY or TT) while the tab is open, as the official
//! editor's TUNER popup does. Opening the tab switches the pedal's tuner on, leaving it switches it off;
//! switching the tuner on or off on the pedal opens or closes the tab.

use super::editing::{param_options, param_widget, value_widget};
use super::{EditorApp, View, AMBER, GREEN};
use crate::worker::Cmd;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke};
use opengx100_protocol::tables;
use opengx100_protocol::tuner::{self, Kind, Readings, FULL_SCALE};

/// The pedal's readings jitter; like the official editor, only move the display by more than this
/// (half a step of a 240-step scale).
const DEAD_BAND: u16 = FULL_SCALE / 240 / 2;

/// What the tuner shows: smoothed values of the latest readings (`None`: no signal).
#[derive(Default)]
pub(super) struct TunerState {
    /// We asked the pedal to show its tuner (or it did so itself).
    pub on: bool,
    /// The tab to go back to when the pedal switches its tuner off.
    pub back_to: Option<View>,
    pub latest: Readings,
    /// When we last switched the pedal's tuner on or off (its echo of that isn't news).
    sent: Option<std::time::Instant>,
    /// MONO/TT first, then strings 1-7.
    shown: [Option<u16>; 8],
}

impl TunerState {
    pub fn update(&mut self, r: Readings) {
        for (shown, reading) in self.shown.iter_mut().zip(std::iter::once(&r.mono).chain(&r.strings)) {
            *shown = match *shown {
                _ if !reading.has_signal() => None,
                Some(v) if v.abs_diff(reading.value) <= DEAD_BAND => Some(v),
                _ => Some(reading.value),
            };
        }
        self.latest = r;
    }
}

const TT_TYPES: &str = "System_pitch.PolyTunerType";

impl EditorApp {
    fn tuner_kind(&self) -> Kind {
        Kind::from_type(self.sys_value("System_common.TunerType").unwrap_or(1))
    }

    /// Keeps the pedal's tuner in step with the tab: on while it's open, off otherwise.
    pub(super) fn sync_tuner(&mut self) {
        if self.view != View::Tuner {
            self.tuner.back_to = Some(self.view);
        }
        let want = self.view == View::Tuner && self.connected();
        if want && !self.tuner.on {
            self.tuner.on = true;
            self.tuner.shown = Default::default();
            self.tuner.sent = Some(std::time::Instant::now());
            let _ = self.tx.send(Cmd::TunerOn(self.tuner_kind()));
        } else if !want && self.tuner.on {
            self.tuner.on = false;
            self.tuner.sent = Some(std::time::Instant::now());
            let _ = self.tx.send(Cmd::TunerOff);
        }
    }

    /// The pedal switched its tuner on (1, 2) or off (0) itself.
    pub(super) fn pedal_running_mode(&mut self, mode: u8) {
        // The pedal confirms what we just asked for; an echo of an earlier request mustn't undo a newer one.
        if self.tuner.sent.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500)) {
            return;
        }
        if mode == 0 {
            self.tuner.on = false;
            if self.view == View::Tuner {
                self.view = self.tuner.back_to.unwrap_or(View::Patch);
            }
        } else {
            self.tuner.on = true;
            self.view = View::Tuner;
        }
    }

    fn set_tuner_kind(&mut self, kind: Kind) {
        if let Some(p) = tables::param("System_common.TunerType") {
            if let Ok(d) = p.spec().encode(kind.type_value()) {
                self.apply_system(p.addr, &d);
            }
        }
        self.tuner.shown = Default::default();
        let _ = self.tx.send(Cmd::TunerOn(kind));
    }

    pub(super) fn tuner_view(&mut self, ui: &mut egui::Ui) {
        if !self.need_connection(ui) {
            return;
        }
        let kind = self.tuner_kind();
        let thru = self.sys_value("System_pitch.TunerOutput") == Some(2);
        ui.horizontal(|ui| {
            ui.heading("Tuner");
            ui.add_space(12.0);
            // The official editor doesn't change the kind while the output is THRU (POLY can't pass the signal).
            ui.add_enabled_ui(!thru, |ui| {
                for (k, label) in [(Kind::Mono, "MONO"), (Kind::Poly, "POLY"), (Kind::TrueTemperament, "TT")] {
                    if ui.selectable_label(kind == k, label).clicked() && kind != k {
                        self.set_tuner_kind(k);
                    }
                }
            })
            .response
            .on_disabled_hover_text("Set the tuner output to MUTE or BYPASS to change the tuner");
            ui.label(RichText::new("the pedal shows its tuner while this page is open").weak());
        });
        ui.add_space(6.0);
        self.tuner_settings(ui, kind);
        ui.separator();
        ui.add_space(8.0);
        match kind {
            Kind::Mono => self.mono_display(ui, None),
            Kind::TrueTemperament => {
                let target = self.tt_target();
                self.mono_display(ui, target)
            }
            Kind::Poly => self.poly_display(ui),
        }
    }

    fn tuner_settings(&mut self, ui: &mut egui::Ui, kind: Kind) {
        let mut rows: Vec<&str> = vec!["System_pitch.ReferencePitch", "System_pitch.TunerOutput"];
        if kind != Kind::Mono {
            rows.extend([TT_TYPES, "System_pitch.PolyTunerOffset"]);
        }
        if kind == Kind::TrueTemperament {
            rows.push("Setup_temp.TTMode");
        }
        egui::Grid::new("tuner-settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for path in rows {
                let (Some(p), Some(v)) = (tables::param(path), self.sys_value(path)) else { continue };
                ui.label(p.desc);
                let new = match path {
                    // POLY can't pass the signal through (THRU).
                    "System_pitch.TunerOutput" if kind == Kind::Poly => {
                        let options: Vec<(i32, &str)> = param_options(p).into_iter().take(2).collect();
                        value_widget(ui, ("set", path), v, (p.min, p.max), &options, &|x| p.display(x), 1, None)
                    }
                    // AUTO, or the string to tune: the list depends on the instrument.
                    "Setup_temp.TTMode" => {
                        let n = tuner::tt_strings(self.sys_value(TT_TYPES).unwrap_or(0)) as i32;
                        let names: Vec<String> = std::iter::once("AUTO".to_string())
                            .chain((1..=n).rev().map(|s| format!("String {s}")))
                            .collect();
                        let options: Vec<(i32, &str)> =
                            names.iter().enumerate().map(|(i, s)| (i as i32, s.as_str())).collect();
                        let label = |x: i32| names.get(x as usize).cloned().unwrap_or_else(|| x.to_string());
                        value_widget(ui, ("set", path), v, (0, n), &options, &label, 1, None)
                    }
                    _ => param_widget(ui, p, v),
                };
                if let Some(n) = new {
                    self.set_sys_param(p, n);
                }
                ui.end_row();
            }
        });
    }

    /// For TT: the string and note the pedal is tuning to, and its cent offset.
    fn tt_target(&self) -> Option<String> {
        let string = self.sys_value("Setup_temp.TTTargetStringNum").filter(|&s| s > 0)?;
        let note = self.sys_value("Setup_temp.TTTargetName")?;
        let offset = tables::param("Setup_temp.TTCentOffset")
            .and_then(|p| self.system_bytes(p.addr, 1))
            .map(|b| tuner::cent_offset(b[0]))
            .unwrap_or(0);
        Some(format!("String {string} · {} · {offset:+} ¢", tuner::note_name(note as u8)))
    }

    /// One note with a needle meter: MONO, and TT (with the string it aims at).
    fn mono_display(&mut self, ui: &mut egui::Ui, target: Option<String>) {
        let r = self.tuner.latest.mono;
        let shown = self.tuner.shown[0].filter(|_| r.has_signal());
        ui.vertical_centered(|ui| {
            let (text, color) = match shown {
                Some(_) => (
                    format!("{}{}", tuner::note_name(r.note), tuner::octave(r.note)),
                    if r.in_tune { GREEN } else { ui.visuals().strong_text_color() },
                ),
                None => ("—".to_string(), ui.visuals().weak_text_color()),
            };
            ui.label(RichText::new(text).size(64.0).color(color).strong());
            if let Some(t) = target.filter(|_| shown.is_some()) {
                ui.label(RichText::new(t).size(16.0));
            }
            ui.add_space(8.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(520.0), 70.0), Sense::hover());
            meter(ui.painter(), rect, shown, r.in_tune);
        });
    }

    /// One bar per string: left of the centre line when flat, right when sharp.
    fn poly_display(&mut self, ui: &mut egui::Ui) {
        let strings = tuner::open_strings(
            self.sys_value(TT_TYPES).unwrap_or(0),
            self.sys_value("System_pitch.PolyTunerOffset").unwrap_or(5),
        );
        let width = ui.available_width().min(560.0);
        for (i, open) in strings.iter().enumerate() {
            let Some(open) = open else { continue };
            let r = self.tuner.latest.strings[i];
            let shown = self.tuner.shown[i + 1].filter(|_| r.has_signal());
            ui.horizontal(|ui| {
                let color = if r.in_tune && shown.is_some() { GREEN } else { ui.visuals().text_color() };
                ui.label(
                    RichText::new(format!("{} {:<2}", i + 1, tuner::note_name(*open)))
                        .monospace()
                        .size(18.0)
                        .color(color),
                );
                let (rect, _) = ui.allocate_exact_size(vec2(width - 60.0, 26.0), Sense::hover());
                bar(ui.painter(), rect, shown, r.in_tune);
            });
            ui.add_space(4.0);
        }
    }
}

/// x position of a reading inside `rect` (left end flat, centre in tune, right end sharp).
fn x_of(rect: Rect, value: u16) -> f32 {
    rect.left() + rect.width() * value.min(FULL_SCALE) as f32 / FULL_SCALE as f32
}

/// The MONO/TT scale: ticks, ♭ and ♯ lit on the side the note is off, a needle where it is; both lit and
/// the needle green when the pedal says it's in tune.
fn meter(painter: &egui::Painter, rect: Rect, value: Option<u16>, in_tune: bool) {
    let dim = Color32::from_gray(80);
    let scale = Rect::from_min_max(
        pos2(rect.left() + 30.0, rect.top() + 10.0),
        pos2(rect.right() - 30.0, rect.bottom() - 10.0),
    );
    for i in 0..=10 {
        let x = scale.left() + scale.width() * i as f32 / 10.0;
        let h = if i == 5 {
            22.0
        } else if i % 5 == 0 {
            14.0
        } else {
            8.0
        };
        painter.line_segment([pos2(x, scale.center().y - h), pos2(x, scale.center().y + h)], Stroke::new(1.0, dim));
    }
    let (flat, sharp) = match value {
        Some(_) if in_tune => (GREEN, GREEN),
        Some(v) if v < tuner::CENTRE => (AMBER, dim),
        Some(_) => (dim, AMBER),
        None => (dim, dim),
    };
    let font = egui::FontId::proportional(22.0);
    painter.text(pos2(rect.left() + 12.0, scale.center().y), egui::Align2::CENTER_CENTER, "♭", font.clone(), flat);
    painter.text(pos2(rect.right() - 12.0, scale.center().y), egui::Align2::CENTER_CENTER, "♯", font, sharp);
    if let Some(v) = value {
        let x = x_of(scale, v);
        let color = if in_tune { GREEN } else { AMBER };
        painter.line_segment([pos2(x, scale.top() - 6.0), pos2(x, scale.bottom() + 6.0)], Stroke::new(3.0, color));
    }
}

/// One POLY string: a bar from the centre towards the flat (left) or sharp (right) side.
fn bar(painter: &egui::Painter, rect: Rect, value: Option<u16>, in_tune: bool) {
    let dim = Color32::from_gray(70);
    painter.rect_filled(rect, 4.0, Color32::from_gray(30));
    let centre = rect.center().x;
    painter.line_segment([pos2(centre, rect.top()), pos2(centre, rect.bottom())], Stroke::new(1.0, dim));
    let Some(v) = value else { return };
    if in_tune {
        painter.rect_filled(Rect::from_center_size(rect.center(), vec2(10.0, rect.height() - 6.0)), 3.0, GREEN);
        return;
    }
    let x = x_of(rect, v);
    let (l, r) = if x < centre { (x, centre) } else { (centre, x) };
    painter.rect_filled(
        Rect::from_min_max(pos2(l, rect.top() + 4.0), pos2(r.max(l + 2.0), rect.bottom() - 4.0)),
        2.0,
        AMBER,
    );
}
