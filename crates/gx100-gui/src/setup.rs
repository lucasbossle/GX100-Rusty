//! First launch: the parameter tables come from the user's own BOSS TONE STUDIO for GX-100. They're
//! read once (a second or two) and cached; until then this screen stands in for the editor.

use eframe::egui::{self, RichText};
use gx100_extract::{Error, Origin};
use gx100_protocol::tables::{self, Tables};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

const BOSS_SUPPORT: &str = "https://www.boss.info/";

type Prepared = Result<(Tables, Origin), Error>;

pub struct Setup {
    pending: Option<Receiver<Prepared>>,
    problem: Option<Error>,
}

impl Setup {
    pub fn start(ctx: &egui::Context, app: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(gx100_extract::prepare(app.as_deref()));
            ctx.request_repaint();
        });
        Setup { pending: Some(rx), problem: None }
    }

    /// Installs the tables once they're ready; `true` when the editor can start.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else { return false };
        let Ok(res) = rx.try_recv() else { return false };
        self.pending = None;
        match res {
            Ok((t, _)) => {
                // Nothing has read a table yet, so this can't fail; if it did, the editor would use
                // whatever is installed, which is still better than not starting.
                let _ = tables::install(t);
                true
            }
            Err(e) => {
                self.problem = Some(e);
                false
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let mut retry = None;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading("GX100-Rusty");
                ui.add_space(16.0);
                match &self.problem {
                    None => {
                        ui.spinner();
                        ui.label("Reading the parameter tables from BOSS TONE STUDIO for GX-100…");
                        ui.label(RichText::new("Only the first time: they're kept for next time.").weak());
                    }
                    Some(problem) => {
                        if *problem == Error::AppNotFound {
                            ui.label(RichText::new("BOSS TONE STUDIO for GX-100 is needed").strong());
                            ui.add_space(6.0);
                            ui.label(
                                "GX100-Rusty reads the pedal's parameter tables (names, ranges, effect lists) from \
                                 Roland's free official editor, installed on this Mac. It doesn't change it or send \
                                 anything anywhere.",
                            );
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label("Get it from");
                                ui.hyperlink_to("boss.info", BOSS_SUPPORT);
                                ui.label("(the GX-100's support page), install it, then try again.");
                            });
                        } else {
                            ui.label(RichText::new("Couldn't read the parameter tables").strong());
                            ui.add_space(6.0);
                            ui.label(problem.to_string());
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui.button("Try again").clicked() {
                                retry = Some(None);
                            }
                            if ui.button("Locate BOSS TONE STUDIO…").clicked() {
                                let picked = rfd::FileDialog::new()
                                    .set_title("Where is BOSS TONE STUDIO for GX-100?")
                                    .set_directory("/Applications")
                                    .add_filter("Application", &["app"])
                                    .pick_file();
                                if let Some(p) = picked {
                                    retry = Some(Some(p));
                                }
                            }
                        });
                    }
                }
            });
        });
        if let Some(app) = retry {
            *self = Setup::start(ui.ctx(), app);
        }
    }
}
