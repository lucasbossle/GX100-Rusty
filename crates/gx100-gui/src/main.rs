//! GX100-Rusty — a graphical editor for the BOSS GX-100.
//!
//! Run with `--sim` to use an in-memory simulated pedal instead of USB MIDI.

mod app;
mod setup;
mod store;
mod worker;

use eframe::egui;

/// The first-launch screen until the parameter tables are ready, then the editor.
enum Root {
    Setup { setup: setup::Setup, sim: bool },
    Editor(Box<app::EditorApp>),
}

impl eframe::App for Root {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Root::Setup { setup, sim } = self {
            if setup.poll() {
                *self = Root::Editor(Box::new(app::EditorApp::new(ctx, *sim)));
            }
        }
        if let Root::Editor(app) = self {
            app.logic(ctx, frame);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        match self {
            Root::Setup { setup, .. } => setup.ui(ui),
            Root::Editor(app) => app.ui(ui, frame),
        }
    }
}

fn main() -> eframe::Result {
    let sim = std::env::args().any(|a| a == "--sim");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("GX100-Rusty")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([700.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "GX100-Rusty",
        options,
        Box::new(move |cc| Ok(Box::new(Root::Setup { setup: setup::Setup::start(&cc.egui_ctx, None), sim }))),
    )
}
