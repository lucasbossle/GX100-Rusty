//! Backups: everything on the pedal (system settings + all 200 user patches), saved to a file.
//! Restoring would write to the pedal, so it isn't enabled yet; backups can be browsed.

use super::{Conn, EditorApp, AMBER, GREEN, RED};
use crate::store::{fmt_time, Backup, Origin};
use crate::worker::Cmd;
use eframe::egui::{self, RichText};
use std::path::Path;

impl EditorApp {
    /// A finished backup arrived from the worker: save it.
    pub(super) fn backup_finished(&mut self, b: Backup) {
        let Some(store) = &self.store else {
            self.last_error = Some("backup read, but there's no data folder to save it in".into());
            return;
        };
        match store.save_backup(&b) {
            Ok(path) => {
                self.notice = Some(format!("Backup saved: {}", path.display()));
                self.backups = store.list_backups();
                self.open_backup = Some((path, b));
            }
            Err(e) => self.last_error = Some(format!("couldn't save the backup: {e}")),
        }
    }

    pub(super) fn backups_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Backups");
        ui.label(
            "A backup holds all 200 user patches and the system settings, the same data as the official \
             editor's backup. It takes a few minutes; you can keep using the editor meanwhile.",
        );
        ui.add_space(6.0);

        let connected = matches!(self.conn, Conn::Connected(_));
        match self.backup_progress {
            Some((done, total)) => {
                ui.horizontal(|ui| {
                    let what = if done == 0 {
                        "system settings".to_string()
                    } else {
                        format!("patch {done} of {}", total - 1)
                    };
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_width(320.0)
                            .text(format!("Backing up… {what}")),
                    );
                    if ui.button("Cancel").clicked() {
                        let _ = self.tx.send(Cmd::CancelBackup);
                    }
                });
            }
            None => {
                ui.horizontal(|ui| {
                    let b = ui.add_enabled(connected && self.store.is_some(), egui::Button::new("Back up now"));
                    if b.on_disabled_hover_text("Connect the pedal first").clicked() {
                        let _ = self.tx.send(Cmd::Backup);
                    }
                    if let Some(store) = &self.store {
                        let dir = store.backups_dir();
                        if ui.button("Open folder").clicked() {
                            reveal(&dir, false);
                        }
                        ui.label(RichText::new(dir.display().to_string()).weak().monospace());
                    }
                });
            }
        }
        ui.separator();

        if self.backups.is_empty() {
            ui.label(RichText::new("No backups yet.").weak());
            return;
        }
        let mut open = None;
        let mut delete = None;
        egui::Grid::new("backups").num_columns(3).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
            for b in &self.backups {
                let is_open = self.open_backup.as_ref().is_some_and(|(p, _)| p == &b.path);
                if ui.selectable_label(is_open, RichText::new(fmt_time(b.created)).monospace()).clicked() {
                    open = Some(b.path.clone());
                }
                let kind = if b.path.extension().is_some_and(|x| x == "alb") { "BOSS TONE STUDIO" } else { "editor" };
                ui.label(RichText::new(format!("{:.1} MB · {kind}", b.bytes as f64 / 1e6)).weak())
                    .on_hover_text(b.path.display().to_string());
                ui.horizontal(|ui| {
                    if ui.small_button("Show in Finder").clicked() {
                        reveal(&b.path, true);
                    }
                    if self.confirm_delete_backup.as_ref() == Some(&b.path) {
                        ui.label(RichText::new("Delete this backup?").color(RED));
                        if ui.small_button("Yes, delete").clicked() {
                            delete = Some(b.path.clone());
                        }
                        if ui.small_button("Keep").clicked() {
                            self.confirm_delete_backup = None;
                        }
                    } else if ui.small_button("Delete…").clicked() {
                        self.confirm_delete_backup = Some(b.path.clone());
                    }
                });
                ui.end_row();
            }
        });
        if let (Some(path), Some(store)) = (open, &self.store) {
            match store.load_backup(&path) {
                Ok(b) => self.open_backup = Some((path, b)),
                Err(e) => self.last_error = Some(format!("couldn't open {}: {e}", path.display())),
            }
        }
        if let (Some(path), Some(store)) = (delete, &self.store) {
            if let Err(e) = store.delete_backup(&path) {
                self.last_error = Some(format!("couldn't delete {}: {e}", path.display()));
            }
            if self.open_backup.as_ref().is_some_and(|(p, _)| p == &path) {
                self.open_backup = None;
            }
            self.confirm_delete_backup = None;
            self.backups = store.list_backups();
        }

        ui.separator();
        self.backup_contents(ui);
    }

    /// The opened backup's patches, marking the ones whose name differs on the pedal now.
    fn backup_contents(&mut self, ui: &mut egui::Ui) {
        let Some((_, b)) = &self.open_backup else {
            ui.label(RichText::new("Click a backup to see what's in it.").weak());
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Backup of {}", fmt_time(b.created))).strong());
            let made_by = match b.origin {
                Origin::Editor => format!("made by this editor (tables from BOSS TONE STUDIO {})", b.bts_version),
                Origin::BossToneStudio => "made by BOSS TONE STUDIO (.alb)".to_string(),
            };
            ui.label(
                RichText::new(format!("· {} patches · {} system areas · {made_by}", b.patches.len(), b.system.len()))
                    .weak(),
            );
            ui.add_enabled(false, egui::Button::new("Restore to pedal"))
                .on_disabled_hover_text("Not enabled yet: restoring writes to the pedal.");
        });
        let differs = b
            .patches
            .iter()
            .filter(|(n, name, _)| self.names.get(*n as usize - 1).cloned().flatten().is_some_and(|now| &now != name))
            .count();
        if differs > 0 {
            ui.colored_label(AMBER, format!("{differs} patch name(s) differ from the pedal now (shown in amber)."));
        } else if self.names.iter().all(|n| n.is_some()) {
            ui.colored_label(GREEN, "Every patch name matches the pedal.");
        }
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
        egui::ScrollArea::vertical().id_salt("backup-patches").auto_shrink(false).show_rows(
            ui,
            row_h,
            b.patches.len(),
            |ui, range| {
                for (n, name, _) in &b.patches[range] {
                    let now = self.names.get(*n as usize - 1).cloned().flatten();
                    ui.horizontal(|ui| {
                        let changed = now.as_ref().is_some_and(|now| now != name);
                        let text = RichText::new(format!("{n:03}  {name:<16}")).monospace();
                        ui.label(if changed { text.color(AMBER) } else { text });
                        if let (true, Some(now)) = (changed, now) {
                            ui.label(RichText::new(format!("now on the pedal: {now}")).weak());
                        }
                    });
                }
            },
        );
    }
}

/// Opens a folder in Finder, or (`select`) shows a file in it.
fn reveal(path: &Path, select: bool) {
    let mut cmd = std::process::Command::new("open");
    if select {
        cmd.arg("-R");
    }
    let _ = cmd.arg(path).spawn();
}
